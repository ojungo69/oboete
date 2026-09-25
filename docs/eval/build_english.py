"""Milestone 1, Task 2: M21's new English test questions (docs/spec.md 8.2 M21, Appendix C item 13).
Same filters and order as build_queries.py's prompt set; English only; test side only; never a
question already in queries.jsonl. Writes ~/.oboete/eval/queries-en.jsonl, unjudged: it is scored
only in milestone 4's single test run."""
import json, os, re, sqlite3, sys

from common import E, JA, h, owner_only, split, write_jsonl

NEEDED = 53
# build_queries.py's MACHINE pattern, unchanged.
MACHINE = re.compile(
    r'^(New session - \d{4}-|Response constraint|"?(Return|Reply) (with )?exactly|DEFAULT-OK|'
    r'Review the (current|staged)|Project: /|Repository: /|以下を修正した。再レビュー|Continue the security review)',
    re.I)


def select(rows, have_qids, have_texts, n):
    """rows: (prompt id, session, body). The first n new English test-side prompts in hash order."""
    out, seen = [], set(have_texts)
    for pid, session, body in sorted(rows, key=lambda r: h(f'prompt:{r[0]}')):
        text = ' '.join(body.split())
        qid = f'p{pid}'
        if (qid in have_qids or text in seen or not 15 <= len(text) <= 600 or MACHINE.match(text)
                or JA.search(text) or split(session) != 'test'):
            continue
        seen.add(text)
        out.append({'qid': qid, 'set': 'prompt', 'text': text, 'session': session, 'split': 'test', 'lang': 'en'})
        if len(out) == n:
            break
    return out


if __name__ == '__main__':
    owner_only()
    from freeze import check
    bad = check()
    if bad:
        sys.exit('frozen inputs changed: ' + ', '.join(bad))
    target = f'{E}/queries-en.jsonl'
    if os.path.exists(target):
        sys.exit(f'{target} exists and is frozen')
    have = [json.loads(line) for line in open(f'{E}/queries.jsonl')]
    db = sqlite3.connect(f'file:{E}/home/oboete.db?mode=ro', uri=True)
    rows = db.execute(
        "SELECT p.id, p.session_id, p.body FROM imports i JOIN prompts p ON p.id = CAST(substr(i.doc, 2) AS INTEGER) "
        "WHERE i.source LIKE 'claude-mem%' AND i.source_id LIKE 'p%'").fetchall()
    got = select(rows, {q['qid'] for q in have}, {q['text'] for q in have}, NEEDED)
    if len(got) < NEEDED:
        sys.exit(f'only {len(got)} new English test questions; spec 8.2 M21 takes the rest from later sessions')
    write_jsonl(target, got)
    print(f'{len(got)} questions from {len({q["session"] for q in got})} sessions -> {target}')
