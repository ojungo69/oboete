"""Milestone 1, Task 8: decision and overturn candidates for the owner's dev labels (docs/spec.md 8.4
item 1: Claude drafts, the owner confirms or rejects).

  draft_candidates.py decisions [max calls]   dev transcripts -> labels/drafts/decisions.jsonl
  draft_candidates.py pairs [max calls]       decisions       -> labels/drafts/pairs.jsonl
  draft_candidates.py tasks                   -> labels/tasks/dev-decisions.jsonl, dev-pairs.jsonl
Every line sent passes `oboete gate`; a quote must appear verbatim in the gated line it cites.
Answers are cached per prompt, so a run stopped by its call budget resumes where it stopped."""
import hashlib, json, os, re, subprocess, sys

from common import E, SEED, claude_json, clean_env, gate, h, owner_only, read_jsonl, write_jsonl
from replay_set import NOT_TYPED

MODEL = 'claude-sonnet-5'
WINDOW, OVERLAP, CAP = 12_000, 10, 1_500
N_DECISIONS, N_PAIRS = 55, 25
WHO = {'user', 'assistant_accepted'}
RELATIONS = {'overturns', 'compatible'}
DRAFTS = f'{E}/labels/drafts'

DECISIONS_PROMPT = """You read part of a coding session between a developer (USER, USER ANSWERED) and an AI agent (ASSISTANT). Lines are numbered [L..].
List every decision the developer made or accepted in this part: a choice between options, a rule to follow from now on, a rejected option, or a reversal of an earlier decision. Include a decision the ASSISTANT proposed only if the developer accepted it in a later line (who = "assistant_accepted"); otherwise who = "user". Skip routine requests ("run the tests"), questions, and plans nobody confirmed.
For each decision give "line" (the id of the line that states or accepts it), "quote" (10-300 characters copied exactly, character for character, from that line), "who", "statement" (one Japanese sentence saying what was decided) and "topic" (2-5 words).
Answer with JSON only: {{"decisions": [...]}}, with an empty list if there is none.

--- SESSION PART ---
{text}
--- END ---"""

PAIRS_PROMPT = """Below are decisions from a developer's coding sessions in one repository, oldest first, one per line: id | date | statement | quote.
Find pairs (earlier, later) about the same subject where:
- "overturns": the later decision replaces, reverses or cancels the earlier one, so the earlier one is no longer in force;
- "compatible": the later decision is about the same subject but leaves the earlier one in force.
Give at most 15 pairs of each relation. Answer with JSON only: {{"pairs": [{{"earlier": "d3", "later": "d9", "relation": "overturns", "why": "<one Japanese sentence>"}}]}}.

{text}"""


def cap(text, n=CAP):
    return text if len(text) <= n else f'{text[: n // 2]} …[{len(text) - n} characters omitted]… {text[-n // 2:]}'


def render(events):
    """Dialogue lines of one parsed transcript as (line id, ts, text); tool calls collapse to names."""
    lines = []

    def add(ts, text):
        lines.append((f'L{len(lines) + 1}', ts, text))

    for e in events:
        p, kind = e['payload'], e['event']
        if kind == 'UserPromptSubmit':
            # Task notifications and teammate messages reach the prompt hook too; nobody typed them.
            if not p.get('prompt', '').lstrip().startswith(NOT_TYPED):
                add(e['ts'], 'USER: ' + cap(p.get('prompt', '')))
        elif kind in ('PostToolUse', 'PostToolUseFailure') and p.get('tool_name') == 'AskUserQuestion':
            for q, a in ((p.get('tool_input') or {}).get('answers') or {}).items():
                add(e['ts'], cap(f'USER ANSWERED: {q} -> {a}'))
        elif kind in ('PostToolUse', 'PostToolUseFailure'):
            if p.get('agent_id'):
                continue
            name = p.get('tool_name', '?')
            if lines and lines[-1][2].startswith('TOOLS: '):
                lid, ts, text = lines[-1]
                lines[-1] = (lid, ts, f'{text}, {name}')
            else:
                add(e['ts'], f'TOOLS: {name}')
        elif kind == 'Stop':
            add(e['ts'], 'ASSISTANT: ' + cap(p.get('last_assistant_message', '')))
        elif kind == 'PostCompact':
            add(e['ts'], 'COMPACTED: ' + cap(p.get('compact_summary', '')))
    return lines


def size(line):
    return len(line[0]) + len(line[2]) + 3


def windows(lines, limit=WINDOW, overlap=OVERLAP):
    out, cur = [], []
    for line in lines:
        if cur and sum(map(size, cur)) + size(line) > limit:
            out.append(cur)
            cur = cur[-overlap:]
            while cur and sum(map(size, cur)) + size(line) > limit:
                cur = cur[1:]
        cur.append(line)
    if cur:
        out.append(cur)
    return out


def valid_decisions(found, window):
    text = {lid: t for lid, _, t in window}
    ok = []
    for d in found:
        q = (d.get('quote') or '').strip()
        if (d.get('line') in text and 10 <= len(q) <= 300 and q in text[d['line']]
                and d.get('who') in WHO and (d.get('statement') or '').strip()):
            ok.append({'line': d['line'], 'quote': q, 'who': d['who'], 'statement': d['statement'].strip(),
                       'topic': (d.get('topic') or '').strip()})
    return ok


def valid_pairs(found, by_id):
    ok, seen = [], set()
    for p in found:
        a, b, rel = p.get('earlier'), p.get('later'), p.get('relation')
        if a in by_id and b in by_id and rel in RELATIONS and by_id[a]['ts'] < by_id[b]['ts'] and (a, b) not in seen:
            seen.add((a, b))
            ok.append({'earlier': a, 'later': b, 'relation': rel, 'why': p.get('why', '')})
    return ok


def ask(prompt, budget):
    """The model's JSON answer, from the cache when this prompt was asked before."""
    path = f'{DRAFTS}/cache/{hashlib.sha256(prompt.encode()).hexdigest()}.json'
    if os.path.exists(path):
        with open(path) as f:
            return json.load(f)
    if budget['left'] <= 0:
        return None
    budget['left'] -= 1
    m = re.search(r'\{.*\}', claude_json(prompt, MODEL), re.S)
    answer = json.loads(m.group(0)) if m else {}
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'w') as f:
        json.dump(answer, f, ensure_ascii=False)
    return answer


def dev_sessions():
    with open(f'{E}/replay/manifest.json') as f:
        return [s for s in json.load(f)['sessions'] if s['side'] == 'dev']


def rendered(s):
    """Parse, render and gate one dev transcript once; later runs read the saved lines."""
    path = f'{DRAFTS}/rendered/{s["session"]}.jsonl'
    if os.path.exists(path):
        return [tuple(r) for r in read_jsonl(path)]
    out = subprocess.run(['oboete', 'transcript', f'{E}/replay/dev/{s["agent"]}/{s["session"]}.jsonl', '--agent', s['agent']],
                         capture_output=True, text=True, check=True, env=clean_env()).stdout
    events = [json.loads(line) for line in out.splitlines()]
    repo = next((e['payload'].get('cwd') for e in events if e['payload'].get('cwd')), '.')
    lines = [(lid, ts, gate(text)) for lid, ts, text in render(events)]
    write_jsonl(path, [list(line) for line in lines])
    write_jsonl(f'{DRAFTS}/rendered/{s["session"]}.repo.jsonl', [{'repo': repo}])
    return lines


def decisions(budget):
    out = []
    for s in dev_sessions():
        lines = rendered(s)
        repo = read_jsonl(f'{DRAFTS}/rendered/{s["session"]}.repo.jsonl')[0]['repo']
        ts = {lid: t for lid, t, _ in lines}
        seen = set()
        for w in windows(lines):
            answer = ask(DECISIONS_PROMPT.format(text='\n'.join(f'[{lid}] {t}' for lid, _, t in w)), budget)
            if answer is None:
                print(f'call budget spent; rerun to continue (at {s["session"]})')
                return out, False
            for d in valid_decisions(answer.get('decisions') or [], w):
                if (d['line'], d['quote']) not in seen:
                    seen.add((d['line'], d['quote']))
                    out.append({'id': f'd{len(out) + 1}', 'session': s['session'], 'repo': repo, 'ts': ts[d['line']], **d})
    return out, True


def pairs(budget, found):
    by_id = {d['id']: d for d in found}
    per_repo = {}
    for d in sorted(found, key=lambda d: d['ts']):
        per_repo.setdefault(d['repo'], []).append(d)
    out = []
    for repo, ds in sorted(per_repo.items()):
        for i in range(0, len(ds), 80):
            chunk = ds[i:i + 80]
            listing = '\n'.join(f'{d["id"]} | {d["ts"][:10]} | {d["statement"]} | {d["quote"]}' for d in chunk)
            answer = ask(PAIRS_PROMPT.format(text=listing), budget)
            if answer is None:
                print(f'call budget spent; rerun to continue (at {repo})')
                return out
            out += valid_pairs(answer.get('pairs') or [], {d['id']: by_id[d['id']] for d in chunk})
    return out


def context(session, line_id, around=3):
    lines = [tuple(r) for r in read_jsonl(f'{DRAFTS}/rendered/{session}.jsonl')]
    i = next(k for k, line in enumerate(lines) if line[0] == line_id)
    return '\n'.join(('▶ ' if k == i else '  ') + lines[k][2] for k in range(max(0, i - around), min(len(lines), i + around + 1)))


def tasks():
    found = read_jsonl(f'{DRAFTS}/decisions.jsonl')
    by_id = {d['id']: d for d in found}
    chosen = sorted(found, key=lambda d: h(f'decision:{SEED}:{d["id"]}'))[:N_DECISIONS]
    items = [{'id': d['id'], 'question': 'これは、あなた (開発者) が決めたことですか？', 'fields': [
        {'label': 'Claude の要約', 'text': d['statement']},
        {'label': '会話の該当部分 (▶ が根拠の行)', 'text': context(d['session'], d['line'])}],
        'choices': [{'value': 'yes', 'label': 'はい、決めたこと'},
                    {'value': 'partly', 'label': '一部違う (直し方をメモに書いてください)'},
                    {'value': 'no', 'label': 'いいえ、決めていない'}]} for d in chosen]
    write_jsonl(f'{E}/labels/tasks/dev-decisions.jsonl', items)
    write_jsonl(f'{E}/labels/dev-decisions.key.jsonl', chosen)

    def side(d, name):
        return {'label': f'{name} ({d["ts"][:10]})', 'text': f'{d["statement"]}\n\n根拠: 「{d["quote"]}」'}

    found_pairs = read_jsonl(f'{DRAFTS}/pairs.jsonl')
    chosen_pairs = []
    for rel in ('overturns', 'compatible'):
        ps = [p for p in found_pairs if p['relation'] == rel]
        # Pairs across sessions first: MUST-M3 counts them separately (spec 8.2 M3).
        ps.sort(key=lambda p: (by_id[p['earlier']]['session'] == by_id[p['later']]['session'],
                               h(f'pair:{SEED}:{p["earlier"]}:{p["later"]}')))
        chosen_pairs += ps[:N_PAIRS]
    chosen_pairs.sort(key=lambda p: h(f'pair-order:{SEED}:{p["earlier"]}:{p["later"]}'))
    items = [{'id': f'{p["earlier"]}-{p["later"]}', 'question': '後の決定は、前の決定を覆していますか？', 'fields': [
        side(by_id[p['earlier']], '前の決定'), side(by_id[p['later']], '後の決定')],
        'choices': [{'value': 'overturns', 'label': 'はい、覆している (前の決定はもう有効ではない)'},
                    {'value': 'compatible', 'label': 'いいえ、両方とも有効'},
                    {'value': 'unsure', 'label': 'わからない'}]} for p in chosen_pairs]
    write_jsonl(f'{E}/labels/tasks/dev-pairs.jsonl', items)
    write_jsonl(f'{E}/labels/dev-pairs.key.jsonl', chosen_pairs)
    print(f'{len(chosen)} decisions, {len(chosen_pairs)} pairs '
          f'({sum(p["relation"] == "overturns" for p in chosen_pairs)} drafted as overturns)')


if __name__ == '__main__':
    owner_only()
    from freeze import check
    bad = check()
    if bad:
        sys.exit('frozen inputs changed: ' + ', '.join(bad))
    cmd = sys.argv[1] if len(sys.argv) > 1 else ''
    budget = {'left': int(sys.argv[2]) if len(sys.argv) > 2 else 300}
    if cmd == 'decisions':
        found, complete = decisions(budget)
        write_jsonl(f'{DRAFTS}/decisions.jsonl', found)
        print(f'{len(found)} decisions from {len({d["session"] for d in found})} sessions'
              + ('' if complete else ' (incomplete: rerun)'))
    elif cmd == 'pairs':
        found = pairs(budget, read_jsonl(f'{DRAFTS}/decisions.jsonl'))
        write_jsonl(f'{DRAFTS}/pairs.jsonl', found)
        print(f'{len(found)} pairs')
    elif cmd == 'tasks':
        tasks()
    else:
        sys.exit(__doc__)
