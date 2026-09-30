"""claude-mem's side of M3's recall (docs/spike/cmem-recall.md): for each of the owner's labeled dev
decisions, the claude-mem prompt that holds it and the observations and summaries claude-mem made
for that prompt and the next one, written outside the repository for the judges.

  cmem_candidates.py <claude-mem db> <out dir>

Writes <out dir>/items.json (one entry per label: its words and its parts) and
<out dir>/labels/<id>.part<k>.jsonl (the records, fields cut for length, at most 100,000
characters a part). Dev only, read-only on the database."""
import json, os, sqlite3, sys

import m3
from common import owner_only

PART = 100_000


def squash(t):
    return ''.join((t or '').split())


def candidates(db, d):
    mems = [m for (m,) in db.execute(
        "SELECT memory_session_id FROM sdk_sessions WHERE content_session_id = ? AND memory_session_id IS NOT NULL",
        (d['session'],))]
    prompts = db.execute("SELECT prompt_number, prompt_text, created_at FROM user_prompts "
                         "WHERE content_session_id = ? ORDER BY prompt_number", (d['session'],)).fetchall()
    q = squash(d['quote'])
    hit = [n for n, t, _ in prompts if q and (q in squash(t) or (squash(t)[:200] and squash(t)[:200] in q))]
    # A pick in AskUserQuestion or a line inside a longer turn is in no prompt's text: take the
    # session's last prompt before the label's time instead.
    before = [n for n, _, c in prompts if c <= d['ts']]
    n = hit[0] if hit else (before[-1] if before else None)
    rows = []
    if n is None:
        return rows
    cut = lambda t, k: (t or '')[:k]
    for m in mems:
        for i, typ, title, sub, facts, narr, pn in db.execute(
                "SELECT id, type, title, subtitle, facts, narrative, prompt_number FROM observations "
                "WHERE memory_session_id = ? AND prompt_number IN (?, ?) ORDER BY id", (m, n, n + 1)):
            rows.append({'ref': f'obs:{i}', 'prompt': pn, 'type': typ, 'title': title, 'subtitle': sub,
                         'facts': cut(facts, 300), 'narrative': cut(narr, 300)})
        for i, req, done, learned, nxt, notes, pn in db.execute(
                "SELECT id, request, completed, learned, next_steps, notes, prompt_number FROM session_summaries "
                "WHERE memory_session_id = ? AND prompt_number IN (?, ?) ORDER BY id", (m, n, n + 1)):
            rows.append({'ref': f'sum:{i}', 'prompt': pn, 'request': cut(req, 300), 'completed': cut(done, 400),
                         'learned': cut(learned, 300), 'next_steps': cut(nxt, 300), 'notes': cut(notes, 200)})
    return rows


def main(db_path, out):
    db = sqlite3.connect(f'file:{db_path}?mode=ro', uri=True)
    decisions, _, _ = m3.labels()
    os.makedirs(f'{out}/labels', exist_ok=True)
    items = []
    for d in decisions:
        if d['value'] != 'yes':
            continue
        lines = [json.dumps(r, ensure_ascii=False) + '\n' for r in candidates(db, d)]
        parts, cur, size = [], [], 0
        for line in lines:
            if cur and size + len(line) > PART:
                parts.append(cur)
                cur, size = [], 0
            cur.append(line)
            size += len(line)
        if cur:
            parts.append(cur)
        paths = []
        for k, p in enumerate(parts):
            paths.append(f"{out}/labels/{d['id']}.part{k}.jsonl")
            with open(paths[-1], 'w') as f:
                f.writelines(p)
        items.append({'id': d['id'], 'quote': d['quote'], 'statement': d['statement'], 'topic': d['topic'],
                      'parts': paths})
    with open(f'{out}/items.json', 'w') as f:
        json.dump(items, f, ensure_ascii=False, indent=0)
    print(f'{len(items)} labels, {sum(len(i["parts"]) for i in items)} parts, '
          f'no candidate: {[i["id"] for i in items if not i["parts"]]}')


if __name__ == '__main__':
    owner_only()
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
