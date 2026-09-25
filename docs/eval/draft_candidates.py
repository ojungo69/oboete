"""Milestone 1, Task 8: the owner's own decisions for the dev labels (docs/spec.md 8.1, 8.4 item 1).
Owner decision 29: the owner confirms only their own decisions, each as one plain-Japanese sentence with
the owner's own message, and may always answer 判断できない; the panel of Task 6 takes those.

  draft_candidates.py decisions [max calls]   dev transcripts -> labels/drafts/decisions.jsonl
  draft_candidates.py extra <n>               n more dev-split transcripts, outside the replay set, for drafting
  draft_candidates.py pairs [max calls]       decisions       -> labels/drafts/pairs.jsonl (kept pairs stay)
  draft_candidates.py tasks                   adds items to labels/tasks/dev-decisions.jsonl and dev-pairs.jsonl
                                              until the owner's counts can be reached; rerun after a sitting
  draft_candidates.py panel                   the five API judges: every `unknown` item, and a blind sample
                                              of the owner's answered items (the overlap, #76)
  draft_candidates.py report                  counts, and the panel's agreement with the owner on the overlap
  draft_candidates.py repeat                  a week after the last answer: 20 answered items again, blind
  draft_candidates.py agreement               the owner against their own earlier answers
Every line sent passes `oboete gate`; a quote must appear verbatim in the gated line it cites.
Answers are cached per prompt, so a run stopped by its call budget resumes where it stopped."""
import hashlib, json, os, re, subprocess, sys, time

from calib import PANEL, chat, kappa, majority
from common import E, SEED, claude_json, clean_env, gate, h, owner_only, read_jsonl, write_jsonl
from replay_set import NOT_TYPED

MODEL = 'claude-sonnet-5'
WINDOW, OVERLAP, CAP, PROMPT_CAP = 12_000, 10, 1_500, 600
# The owner's counts (spec 8.4 item 1): answered decisions, and pairs confirmed as each relation.
N_DECISIONS, N_PAIRS, N_OVERLAP, N_REPEAT, WEEK = 50, 20, 40, 20, 7 * 86400
WHO = {'user', 'assistant_accepted'}
RELATIONS = ('overturns', 'compatible')
DRAFTS = f'{E}/labels/drafts'
TASKS, LABELS = f'{E}/labels/tasks', f'{E}/labels'

DECISIONS_PROMPT = """You read part of a coding session between a developer (USER, USER ANSWERED) and an AI agent (ASSISTANT). Lines are numbered [L..].
List every decision the developer made or accepted in this part: a choice between options, a rule to follow from now on, a rejected option, or a reversal of an earlier decision. Include a decision the ASSISTANT proposed only if the developer accepted it in a later line (who = "assistant_accepted"); otherwise who = "user". Skip routine requests ("run the tests"), questions, and plans nobody confirmed.
For each decision give "line" (the id of the line that states or accepts it), "quote" (10-300 characters copied exactly, character for character, from that line), "who", "prompt_line" (the id of the USER line, the developer's own message, in which the decision was made or accepted), "statement" and "topic" (2-5 words).
"statement" is one plain Japanese sentence that a person who does not program can follow: no code, no file or command names, no untranslated English terms; say what the choice means in everyday words.
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

# The panel reads English, so it gets the lines around each decision; the owner never does.
PANEL_DECISION = """You check a decision that was drafted from a developer's coding session. The lines around it (▶ marks the line it cites):
<<<
{context}
>>>
Drafted decision (Japanese): {statement}
Did the developer make or accept this decision in this session? Answer with JSON only: {{"answer": "yes"}} or {{"answer": "no"}}."""

PANEL_PAIR = """Two decisions from a developer's coding sessions, each with the lines around it (▶ marks the line it cites).
Earlier ({earlier_ts}): {earlier}
<<<
{earlier_context}
>>>
Later ({later_ts}): {later}
<<<
{later_context}
>>>
Does the later decision replace, reverse or cancel the earlier one, so the earlier one is no longer in force? Answer with JSON only: {{"answer": "overturns"}} or {{"answer": "compatible"}}."""

DECISION_CHOICES = [{'value': 'yes', 'label': 'はい、私が決めたこと'},
                    {'value': 'no', 'label': 'いいえ、決めていない'},
                    {'value': 'unknown', 'label': '判断できない'}]
PAIR_CHOICES = [{'value': 'overturns', 'label': 'はい、後の決定が前の決定を覆している'},
                {'value': 'compatible', 'label': 'いいえ、両方とも有効'},
                {'value': 'unknown', 'label': '判断できない'}]


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


OWN = ('USER: ', 'USER ANSWERED: ')   # the owner's typed message, or their answer to a question


def line_id(v):
    """'L4' for 'L4', '[L4]', 4 or '4' (the model writes all of them)."""
    v = str(v if v is not None else '').strip().strip('[]')
    return f'L{v}' if v.isdigit() else v


def valid_decisions(found, window):
    """Decisions whose quote is verbatim in the line it cites and whose `prompt_line` is the owner's
    own message in the same window; that message is kept for the owner to see."""
    text = {lid: t for lid, _, t in window}
    ok = []
    for d in found:
        q = (d.get('quote') or '').strip()
        line, own_line = line_id(d.get('line')), line_id(d.get('prompt_line'))
        own = text.get(own_line, '')
        prefix = next((p for p in OWN if own.startswith(p)), None)
        if (line in text and 10 <= len(q) <= 300 and q in text[line] and prefix
                and d.get('who') in WHO and (d.get('statement') or '').strip()):
            ok.append({'line': line, 'quote': q, 'who': d['who'], 'statement': d['statement'].strip(),
                       'topic': (d.get('topic') or '').strip(), 'prompt_line': own_line,
                       'prompt': cap(own[len(prefix):], PROMPT_CAP)})
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
    """The replay set's dev transcripts, then the extra ones (`extra`), in that order, so the ids
    of decisions drafted earlier never change."""
    with open(f'{E}/replay/manifest.json') as f:
        out = [{**s, 'dir': 'replay/dev'} for s in json.load(f)['sessions'] if s['side'] == 'dev']
    path = f'{DRAFTS}/extra.json'
    if os.path.exists(path):
        with open(path) as f:
            out += [{**s, 'dir': 'replay/dev-extra'} for s in json.load(f)]
    return out


def extra(n):
    """n more dev-split transcripts claude-mem recorded, outside the replay set (either side), the
    ones with the most typed prompts first (they hold the most decisions); copied owner-only."""
    from replay_set import copy_out, inventory
    with open(f'{E}/replay/manifest.json') as f:
        taken = {s['session'] for s in json.load(f)['sessions']}
    path = f'{DRAFTS}/extra.json'
    old = json.load(open(path)) if os.path.exists(path) else []
    taken |= {s['session'] for s in old}
    pool, _ = inventory()
    fresh = sorted((p for p in pool if p['side'] == 'dev' and p['session'] not in taken),
                   key=lambda p: (-p['prompts'], h(f'extra:{SEED}:{p["session"]}')))[:n]
    for p in fresh:
        p['side'] = 'dev-extra'
    copy_out(fresh, f'{E}/replay')
    new = old + [{'session': p['session'], 'agent': p['agent']} for p in fresh]
    with open(path, 'w') as f:
        json.dump(new, f, indent=1)
    return len(fresh)


def events_of(out):
    """`oboete transcript` output as events. split('\\n'), not splitlines(): a JSON string may hold
    U+2028, which splitlines() breaks on."""
    return [json.loads(line) for line in out.split('\n') if line]


def rendered(s):
    """Parse, render and gate one dev transcript once; later runs read the saved lines."""
    path = f'{DRAFTS}/rendered/{s["session"]}.jsonl'
    if os.path.exists(path):
        return [tuple(r) for r in read_jsonl(path)]
    out = subprocess.run(['oboete', 'transcript', f'{E}/{s["dir"]}/{s["agent"]}/{s["session"]}.jsonl', '--agent', s['agent']],
                         capture_output=True, text=True, check=True, env=clean_env()).stdout
    events = events_of(out)
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


def decision_fields(d, side=''):
    return [{'label': f'{side}この決定 (Claude がやさしい日本語でまとめたもの)', 'text': d['statement']},
            {'label': f'{side}そのときのあなたのメッセージ', 'text': d['prompt']}]


def decision_item(d):
    return {'id': d['id'], 'question': 'これは、あなたが決めたことですか？', 'fields': decision_fields(d),
            'choices': DECISION_CHOICES}


def pair_id(p):
    return f'{p["earlier"]}-{p["later"]}'


def pair_item(p, by_id):
    a, b = by_id[p['earlier']], by_id[p['later']]
    return {'id': pair_id(p), 'question': '後の決定は、前の決定を覆していますか？',
            'fields': decision_fields(a, f'前 ({a["ts"][:10]}) ') + decision_fields(b, f'後 ({b["ts"][:10]}) '),
            'choices': PAIR_CHOICES}


def refill(order, shown, target, counts):
    """The next ids of `order` to show so that `target` shown ids can still count; `counts(id)` says
    whether a shown id counts or may still (answered so, or not answered yet). An `unknown` answer
    never counts, so each one brings in another candidate (#76)."""
    have = sum(1 for i in shown if counts(i))
    return [i for i in order if i not in shown][:max(0, target - have)]


def pair_order(found_pairs, by_id, rel):
    ps = [p for p in found_pairs if p['relation'] == rel]
    # Pairs across sessions first: MUST-M3 counts them separately (spec 8.2 M3).
    ps.sort(key=lambda p: (by_id[p['earlier']]['session'] == by_id[p['later']]['session'],
                           h(f'pair:{SEED}:{p["earlier"]}:{p["later"]}')))
    return [pair_id(p) for p in ps]


def standing(name):
    """id -> (value, ts) of the owner's latest answer; a withdrawn answer (value null) is none."""
    out = {}
    path = f'{LABELS}/{name}.jsonl'
    for r in read_jsonl(path) if os.path.exists(path) else []:
        out[r['id']] = (r['value'], r['ts'])
    return {i: v for i, v in out.items() if v[0] is not None}


def append(name, items, keys):
    for path, rows in ((f'{TASKS}/{name}.jsonl', items), (f'{LABELS}/{name}.key.jsonl', keys)):
        old = read_jsonl(path) if os.path.exists(path) else []
        write_jsonl(path, old + rows)


def tasks():
    found = read_jsonl(f'{DRAFTS}/decisions.jsonl')
    by_id = {d['id']: d for d in found}
    shown = {k['id'] for k in read_jsonl(f'{LABELS}/dev-decisions.key.jsonl')} \
        if os.path.exists(f'{LABELS}/dev-decisions.key.jsonl') else set()
    answers = {i: v for i, (v, _) in standing('dev-decisions').items()}
    order = [d['id'] for d in sorted(found, key=lambda d: h(f'decision:{SEED}:{d["id"]}'))]
    new = refill(order, shown, N_DECISIONS, lambda i: answers.get(i) != 'unknown')
    append('dev-decisions', [decision_item(by_id[i]) for i in new], [by_id[i] for i in new])

    found_pairs = {pair_id(p): p for p in read_jsonl(f'{DRAFTS}/pairs.jsonl')}
    shown = {k['id'] for k in read_jsonl(f'{LABELS}/dev-pairs.key.jsonl')} \
        if os.path.exists(f'{LABELS}/dev-pairs.key.jsonl') else set()
    answers = {i: v for i, (v, _) in standing('dev-pairs').items()}
    added = []
    for rel in RELATIONS:
        # A pair counts toward the relation the owner gave it; one not answered yet, toward its draft's.
        counts = lambda i, rel=rel: answers.get(i, found_pairs[i]['relation']) == rel
        added += refill(pair_order(found_pairs.values(), by_id, rel), shown | set(added), N_PAIRS, counts)
    added.sort(key=lambda i: h(f'pair-order:{SEED}:{i}'))
    append('dev-pairs', [pair_item(found_pairs[i], by_id) for i in added], [{**found_pairs[i], 'id': i} for i in added])
    print(f'added {len(new)} decisions and {len(added)} pairs')


def context(d, around=3):
    lines = [tuple(r) for r in read_jsonl(f'{DRAFTS}/rendered/{d["session"]}.jsonl')]
    i = next(k for k, line in enumerate(lines) if line[0] == d['line'])
    return '\n'.join(('▶ ' if k == i else '  ') + lines[k][2] for k in range(max(0, i - around), min(len(lines), i + around + 1)))


def panel_targets(ids, answers, n=N_OVERLAP):
    """(the items the owner could not judge, a seeded sample of those the owner did judge). The
    sample is graded blind: panel prompts are built from the drafts, never from the owner's answers."""
    unknown = [i for i in ids if answers.get(i) == 'unknown']
    judged = sorted((i for i in ids if answers.get(i) not in (None, 'unknown')), key=lambda i: h(f'overlap:{SEED}:{i}'))
    return unknown, judged[:n]


def parse_answer(text, allowed):
    text = re.sub(r'<think>.*?</think>', '', text or '', flags=re.S)
    m = re.search(r'\{.*\}', text, re.S)
    value = json.loads(m.group(0)).get('answer') if m else None
    if value not in allowed:
        raise ValueError(f'answer outside {sorted(allowed)}: {text[:120]!r}')
    return value


def panel():
    by_id = {d['id']: d for d in read_jsonl(f'{DRAFTS}/decisions.jsonl')}
    for name in ('dev-decisions', 'dev-pairs'):
        keys = {k['id']: k for k in read_jsonl(f'{LABELS}/{name}.key.jsonl')}
        answers = {i: v for i, (v, _) in standing(name).items()}
        unknown, sample = panel_targets(list(keys), answers)
        path = f'{LABELS}/{name}.panel.jsonl'
        done = {(r['id'], r['judge']) for r in read_jsonl(path)} if os.path.exists(path) else set()
        for i in unknown + sample:
            k = keys[i]
            if name == 'dev-decisions':
                prompt, allowed = PANEL_DECISION.format(context=context(k), statement=k['statement']), {'yes', 'no'}
            else:
                a, b = by_id[k['earlier']], by_id[k['later']]
                prompt = PANEL_PAIR.format(earlier_ts=a['ts'][:10], earlier=a['statement'], earlier_context=context(a),
                                           later_ts=b['ts'][:10], later=b['statement'], later_context=context(b))
                allowed = set(RELATIONS)
            prompt = gate(prompt)
            for member in PANEL:
                if (i, member) in done:
                    continue
                try:
                    value = parse_answer(chat(member, prompt), allowed)
                except (OSError, ValueError, KeyError) as e:      # left for the next run
                    print(f'{i} {member}: {type(e).__name__} {str(e)[:120]}', file=sys.stderr)
                    continue
                with open(path, 'a') as f:
                    f.write(json.dumps({'id': i, 'judge': member, 'value': value,
                                        'why': 'unknown' if i in unknown else 'overlap'}) + '\n')
    print('panel done; rerun if any call failed')


def report():
    out = {}
    for name, yes in (('dev-decisions', 'yes'), ('dev-pairs', 'overturns')):
        answers = {i: v for i, (v, _) in standing(name).items()}
        path = f'{LABELS}/{name}.panel.jsonl'
        votes = {}
        for r in read_jsonl(path) if os.path.exists(path) else []:
            votes.setdefault(r['id'], {})[r['judge']] = r['value'] == yes
        counts = {v: sum(1 for a in answers.values() if a == v) for v in sorted(set(answers.values()))}
        overlap = {i: v for i, v in votes.items() if answers.get(i) not in (None, 'unknown')}
        agree = {}
        for judge in [*PANEL, 'majority']:
            pairs = [(answers[i] == yes, majority(list(v.values())) if judge == 'majority' else v.get(judge))
                     for i, v in overlap.items()]
            pairs = [p for p in pairs if p[1] is not None]
            agree[judge] = {'n': len(pairs), 'kappa': kappa(pairs) if pairs else None,
                            'agreement': sum(a == b for a, b in pairs) / len(pairs) if pairs else None}
        out[name] = {'owner': counts, 'panel_on_unknown': {
            i: majority(list(v.values())) for i, v in votes.items() if answers.get(i) == 'unknown'},
            'overlap': agree,
            'what': 'agreement with the owner on the owner\'s own decisions, not a check of technical relevance'}
    with open(f'{LABELS}/dev-labels.result.json', 'w') as f:
        json.dump(out, f, indent=1, ensure_ascii=False)
    print(json.dumps(out, indent=1, ensure_ascii=False))


def repeat_items(tasks_by_id, answers, last_ts, now, n=N_REPEAT):
    """20 items the owner answered other than `unknown`, blind, with fresh ids; None before a week."""
    if now - last_ts < WEEK:
        return None
    judged = sorted((i for i, v in answers.items() if v != 'unknown'), key=lambda i: h(f'repeat:{SEED}:{i}'))[:n]
    return [{**tasks_by_id[i], 'id': 'r' + i} for i in judged]


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
    elif cmd == 'extra':
        print(f'{extra(int(sys.argv[2]))} more dev transcripts')
    elif cmd == 'pairs':
        # Pairs found before stay (the owner may have answered them); new ones are added.
        old = read_jsonl(f'{DRAFTS}/pairs.jsonl') if os.path.exists(f'{DRAFTS}/pairs.jsonl') else []
        known = {pair_id(p) for p in old}
        found = old + [p for p in pairs(budget, read_jsonl(f'{DRAFTS}/decisions.jsonl')) if pair_id(p) not in known]
        write_jsonl(f'{DRAFTS}/pairs.jsonl', found)
        print(f'{len(found)} pairs ({len(found) - len(old)} new)')
    elif cmd == 'tasks':
        tasks()
    elif cmd == 'panel':
        panel()
    elif cmd == 'report':
        report()
    elif cmd in ('repeat', 'agreement'):
        answers = {**standing('dev-decisions'), **standing('dev-pairs')}
        if cmd == 'repeat':
            items = {i['id']: i for n in ('dev-decisions', 'dev-pairs') for i in read_jsonl(f'{TASKS}/{n}.jsonl')}
            last = max((ts for _, ts in answers.values()), default=None)
            chosen = repeat_items(items, {i: v for i, (v, _) in answers.items()}, last or 0, int(time.time())) if last else None
            if chosen is None:
                sys.exit('the blind repeat opens a week after the last answer'
                         + (f': {time.strftime("%Y-%m-%d", time.localtime(last + WEEK))}' if last else ''))
            write_jsonl(f'{TASKS}/dev-repeat-20.jsonl', chosen)
            print(f'{len(chosen)} items -> {TASKS}/dev-repeat-20.jsonl')
        else:
            again = {i[1:]: v for i, (v, _) in standing('dev-repeat-20').items() if v != 'unknown'}
            yes = {'yes', 'overturns'}
            rated = [(answers[i][0] in yes, v in yes) for i, v in again.items() if i in answers]
            result = {'n': len(rated), 'kappa': kappa(rated) if rated else None,
                      'agreement': sum(a == b for a, b in rated) / len(rated) if rated else None,
                      'what': 'the owner against their own earlier answers'}
            with open(f'{LABELS}/dev-repeat-20.result.json', 'w') as f:
                json.dump(result, f, indent=1)
            print(json.dumps(result, indent=1))
    else:
        sys.exit(__doc__)
