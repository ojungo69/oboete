"""Milestone 1, Task 6: B3, the judge-trust gate (docs/spec.md 8.1 "Judge trust"; owner decision 29).

  calib.py draw               50 dev pairs the judge graded -> labels/tasks/calib-50.jsonl and its key
  calib.py panel [max calls]  five API judges grade every pair -> labels/calib-50.panel-2.jsonl (resumes)
  calib.py kappa              each judge against the other five's majority, and the panel's Fleiss kappa
Relevant = grade >= 2. Every judge sees the question and the document as the judge under test saw
them (4,000 characters, 1,200 for grades written before `chars` was recorded), never a grade."""
import concurrent.futures, json, os, re, sqlite3, sys, threading, time, urllib.error, urllib.request

from common import E, SEED, h, owner_only, read_jsonl, write_jsonl

N, PASS_KAPPA = 50, 0.4
# Run 1 (`calib-50.panel.jsonl`, frozen) took the first entry of any answer; run 2 requires exactly
# the grade of memory `d` and is the one B3 is decided on (#77).
RUN = 'calib-50.panel-2'
# The result of run 2 under the rule that leaves a pair out for every judge (#77); the first result
# of run 2 (`calib-50.result-2.json`) is frozen as it was.
RESULT = 'calib-50.result-2b'
JUDGES = {'claude-sonnet-5', 'claude-sonnet'}   # the alias rows of 2026-09-24 came from claude-sonnet-5
UNDER_TEST = 'claude-sonnet-5'
GO = ('https://opencode.ai/zen/go/v1', 'OPENCODE_API_KEY.md', {'x-opencode-session': 'oboete'})
# Five more makers, chosen 2026-09-26 by what answered (NIM and Mistral gave 429 or 404 that day).
# API calls only: no tools. Keys are read in this process and never put in any environment.
PANEL = {'gpt-oss-120b': ('https://api.groq.com/openai/v1', 'GROQ_API_KEY.md', {}, 'openai/gpt-oss-120b'),
         'deepseek-v4-pro': (*GO, 'deepseek-v4-pro'), 'glm-5.3': (*GO, 'glm-5.3'),
         'kimi-k3': (*GO, 'kimi-k3'), 'qwen3.8-max': (*GO, 'qwen3.8-max')}
MAX_DOC_CHARS, OLD_CHARS = 4000, 1200   # judge.py's window, and the one before `chars` was recorded
QUESTION = 'この記憶は、この問いに答えるのに役に立ちますか？'
CHOICES = [{'value': 'yes', 'label': '役に立つ (答えに使える情報が入っている)'},
           {'value': 'no', 'label': '役に立たない (無関係、または話題が同じだけ)'}]
DOC_SQL = {'o': 'SELECT title, body FROM observations WHERE id=?',
           's': "SELECT '', body FROM summaries WHERE id=?",
           'p': "SELECT '', body FROM prompts WHERE id=?"}


def kappa(pairs):
    """Cohen's kappa for two binary raters; pairs of (bool, bool). None when both raters used one
    category only: kappa is undefined there, and the raw agreement is reported beside it."""
    n = len(pairs)
    po = sum(a == b for a, b in pairs) / n
    pa = sum(a for a, _ in pairs) / n
    pb = sum(b for _, b in pairs) / n
    pe = pa * pb + (1 - pa) * (1 - pb)
    return None if pe == 1 else (po - pe) / (1 - pe)


def fleiss(rows):
    """Fleiss' kappa for binary ratings, one row per item with every rater's rating. None when
    every rating is the same."""
    n = len(rows[0])
    p_bar = sum((sum(r) * (sum(r) - 1) + (n - sum(r)) * (n - sum(r) - 1)) / (n * (n - 1)) for r in rows) / len(rows)
    p = sum(sum(r) for r in rows) / (len(rows) * n)
    pe = p * p + (1 - p) * (1 - p)
    return None if pe == 1 else (p_bar - pe) / (1 - pe)


def majority(votes):
    yes = sum(votes)
    return None if yes * 2 == len(votes) else yes * 2 > len(votes)


def against_others(grades, judge):
    """(the judge's binary grade, the other judges' majority) per pair; ties left out. `grades`:
    pair id -> judge -> 0-3."""
    out = []
    for g in grades.values():
        ref = majority([v >= 2 for j, v in g.items() if j != judge])
        if judge in g and ref is not None:
            out.append((g[judge] >= 2, ref))
    return out


def parse_grade(text):
    """The grade of memory `d`, the only one asked about, in an answer to judge.py's prompt; a code
    fence or a reasoning block around the JSON is allowed. Anything else raises: more entries, another
    id, or a grade outside 0-3."""
    text = re.sub(r'<think>.*?</think>', '', text or '', flags=re.S)
    m = re.search(r'\{.*\}', text, re.S)
    answer = json.loads(m.group(0)) if m else None
    grades = answer.get('grades') if isinstance(answer, dict) else None
    ok = (isinstance(grades, list) and len(grades) == 1 and isinstance(grades[0], dict)
          and str(grades[0].get('id', '')).strip('[]') == 'd'
          and type(grades[0].get('grade')) is int and 0 <= grades[0]['grade'] <= 3)
    if not ok:
        raise ValueError(f'no usable grade: {text[:120]!r}')
    return grades[0]['grade']


def chat(member, prompt, timeout=300):
    """(answer text, the model the provider reports) of one chat completion from a panel judge,
    temperature 0; a 429 is retried twice."""
    base, key_file, headers, model = PANEL[member]
    with open(os.path.expanduser(f'~/{key_file}')) as f:
        key = f.read().split('\n')[1].strip()
    body = json.dumps({'model': model, 'temperature': 0, 'messages': [{'role': 'user', 'content': prompt}]}).encode()
    for attempt in range(3):
        req = urllib.request.Request(f'{base}/chat/completions', data=body, headers={
            'Authorization': f'Bearer {key}', 'Content-Type': 'application/json', 'User-Agent': 'oboete-eval', **headers})
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                answer = json.load(r)
            # The model the provider says answered: an alias can move to another model.
            return answer['choices'][0]['message'].get('content') or '', answer.get('model') or ''

        except urllib.error.HTTPError as e:
            if e.code != 429 or attempt == 2:
                raise
            time.sleep(20 * (attempt + 1))


def ask_panel(member, question, memory):
    """(grade, the model the provider reports)."""
    from judge import PROMPT
    text, model = chat(member, PROMPT.format(query=question, docs=f'[d]\n{memory}'))
    return parse_grade(text), model


def draw(queries, judgments, doc_text, n=N):
    """Half graded 2-3, the rest split between 1 (near misses) and 0; dev questions only; one pair
    per question; the latest grade of a pair counts; seeded hash order."""
    dev = {q['qid']: q for q in queries if q['split'] == 'dev'}
    graded = {}
    for j in judgments:
        if j['qid'] in dev and j['judge'] in JUDGES:
            graded[(j['qid'], j['doc'])] = (j['grade'], j['judge'], j.get('chars', OLD_CHARS))
    quota = {'relevant': n // 2, 1: (n - n // 2) // 2, 0: n - n // 2 - (n - n // 2) // 2}
    items, key, used = [], [], set()
    for (qid, doc), (grade, judge, chars) in sorted(graded.items(), key=lambda kv: h(f'calib:{SEED}:{kv[0][0]}:{kv[0][1]}')):
        bucket = 'relevant' if grade >= 2 else grade
        if qid in used or quota[bucket] == 0:
            continue
        text = doc_text(doc, chars)
        if text is None:          # deleted from the store since it was judged
            continue
        quota[bucket] -= 1
        used.add(qid)
        item_id = f'c{len(items) + 1:02d}'
        items.append({'id': item_id, 'question': QUESTION, 'choices': CHOICES, 'fields': [
            {'label': '問い (開発者が AI に送ったメッセージ)', 'text': dev[qid]['text']},
            {'label': '記憶 (以前のセッションで残されたメモ)', 'text': text}]})
        key.append({'id': item_id, 'qid': qid, 'doc': doc, 'grade': grade, 'judge': judge, 'chars': chars})
    order = sorted(range(len(items)), key=lambda i: h(f'calib-order:{SEED}:{items[i]["id"]}'))
    return [items[i] for i in order], [key[i] for i in order]


def store_doc_text(db):
    def text(doc, chars):
        row = db.execute(DOC_SQL[doc[0]], (int(doc[1:]),)).fetchone()
        if row is None:
            return None
        title, text = row
        # judge.py's own serialization, so the clip lands on the same character.
        body, cut = (title + '\n' + text).strip() if title else text, min(chars, MAX_DOC_CHARS)
        return body if len(body) <= cut else body[:cut] + '\n…[clipped]'   # judge.py's own mark
    return text


def main(cmd):
    from freeze import check
    bad = check()
    if bad:
        sys.exit('frozen inputs changed: ' + ', '.join(bad))
    tasks, labels = f'{E}/labels/tasks', f'{E}/labels'
    if cmd == 'draw':
        if os.path.exists(f'{tasks}/calib-50.jsonl'):
            sys.exit('calib-50 exists; its labels belong to it')
        db = sqlite3.connect(f'file:{E}/home/oboete.db?mode=ro', uri=True)
        items, key = draw(read_jsonl(f'{E}/queries.jsonl'), read_jsonl(f'{E}/judgments.jsonl'), store_doc_text(db))
        if len(items) < N:
            sys.exit(f'only {len(items)} of {N} pairs fill the grade quotas; nothing written')
        write_jsonl(f'{tasks}/calib-50.jsonl', items)
        write_jsonl(f'{labels}/calib-50.key.jsonl', key)
        print(f'{len(items)} pairs -> {tasks}/calib-50.jsonl')
    elif cmd == 'panel':
        budget = int(sys.argv[2]) if len(sys.argv) > 2 else 1000
        key = read_jsonl(f'{labels}/calib-50.key.jsonl')
        path = f'{labels}/{RUN}.jsonl'
        done = {(r['id'], r['judge']) for r in read_jsonl(path)} if os.path.exists(path) else set()
        # What every panel judge reads, written once and frozen: the store is not under the freeze.
        inputs = f'{labels}/calib-50.inputs.jsonl'
        if not os.path.exists(inputs):
            questions = {q['qid']: q['text'] for q in read_jsonl(f'{E}/queries.jsonl')}
            text = store_doc_text(sqlite3.connect(f'file:{E}/home/oboete.db?mode=ro', uri=True))
            write_jsonl(inputs, [{'id': k['id'], 'question': questions[k['qid']], 'memory': text(k['doc'], k['chars'])}
                                 for k in key])
        given = {r['id']: r for r in read_jsonl(inputs)}
        todo = [(k, m) for k in key for m in PANEL if (k['id'], m) not in done][:budget]
        lock = threading.Lock()

        def grade(k, member):
            row = {'id': k['id'], 'judge': member}
            for attempt in range(3):
                try:
                    row['grade'], row['model'] = ask_panel(member, given[k['id']]['question'], given[k['id']]['memory'])
                    break
                except ValueError as e:      # an answer with no usable grade: asked again, then recorded
                    row.update(grade=None, unusable=str(e)[:200])
                except (OSError, KeyError) as e:      # the call failed: left for the next run
                    print(f'{k["id"]} {member}: {type(e).__name__} {str(e)[:120]}', file=sys.stderr)
                    return False
            with lock, open(path, 'a') as f:
                f.write(json.dumps(row) + '\n')
            return row['grade'] is not None

        # One worker per judge: each provider sees one call at a time.
        with concurrent.futures.ThreadPoolExecutor(len(PANEL)) as pool:
            jobs = [pool.submit(lambda m=m: [grade(k, m) for k, mm in todo if mm == m]) for m in PANEL]
            failed = sum(r.count(False) for r in (j.result() for j in jobs))
        have = len(read_jsonl(path)) if os.path.exists(path) else 0
        print(f'{have} of {len(key) * len(PANEL)} panel grades; {failed} failed this run')
    elif cmd == 'kappa':
        grades = {k['id']: {UNDER_TEST: k['grade']} for k in read_jsonl(f'{labels}/calib-50.key.jsonl')}
        recorded = read_jsonl(f'{labels}/{RUN}.jsonl')
        for r in recorded:
            if r['grade'] is not None:       # an unusable answer leaves the pair out for that judge only
                grades[r['id']][r['judge']] = r['grade']
        first = {(r['id'], r['judge']): r['grade'] for r in read_jsonl(f'{labels}/calib-50.panel.jsonl')}
        again = [(first[i, j], g) for i, js in grades.items() for j, g in js.items() if (i, j) in first]
        judges = [UNDER_TEST, *PANEL]
        # A pair any judge could not grade is left out for every judge and counted, like a tie
        # (spec 8.1): each judge is then measured on the same pairs against all five others.
        left_out = sorted(i for i, g in grades.items() if len(g) < len(judges))
        grades = {i: g for i, g in grades.items() if len(g) == len(judges)}
        # Complete when every judge answered every pair, with a grade or with an unusable answer 3 times.
        complete = len({(r['id'], r['judge']) for r in recorded}) == N * len(PANEL)
        # One model per judge. A run recorded before replies carried the model (run 2) has none at
        # all; a run with some rows missing it, or with two models, cannot pass (spec 8.1: a model
        # change means a new calibration).
        reported = {j: {r.get('model') for r in recorded if r['judge'] == j and r['grade'] is not None} for j in PANEL}
        one_model = all(len(m) == 1 for m in reported.values()) and (
            all(m == {None} for m in reported.values()) or all(None not in m for m in reported.values()))
        complete = complete and one_model
        each = {}
        for j in judges:
            pairs = against_others(grades, j)
            k = kappa(pairs) if pairs else None
            each[j] = {'n': len(pairs), 'kappa': k, 'agreement': sum(a == b for a, b in pairs) / len(pairs) if pairs else None,
                       'pass': bool(complete and k is not None and k >= PASS_KAPPA)}
        rows = [[g[j] >= 2 for j in judges] for g in grades.values()]
        fk = fleiss(rows) if rows else None
        panel_pass = bool(complete and fk is not None and fk >= PASS_KAPPA)
        out = {'run': RUN, 'complete': complete, 'judges': each,
               'unusable': [(r['id'], r['judge']) for r in recorded if r['grade'] is None], 'left_out': left_out,
               'changed_from_run_1': {'n': len(again), 'grade': sum(a != b for a, b in again),
                                      'relevance': sum((a >= 2) != (b >= 2) for a, b in again)}, 'fleiss': fk, 'panel_pass': panel_pass,
               'pass': panel_pass and each[UNDER_TEST]['pass'],
               'one_model_per_judge': one_model,
               'models': {UNDER_TEST: [UNDER_TEST], **{j: sorted(m or f'{PANEL[j][3]} (requested; the reply was not recorded)'
                                                                   for m in reported[j]) for j in PANEL}}}
        from freeze import load
        if f'labels/{RESULT}.json' in load()['files']:
            sys.exit(f'labels/{RESULT}.json is frozen; a changed rule writes a new result')
        with open(f'{labels}/{RESULT}.json', 'w') as f:
            json.dump(out, f, indent=1)
        print(json.dumps(out, indent=1))
    else:
        sys.exit(__doc__)


if __name__ == '__main__':
    owner_only()
    main(sys.argv[1] if len(sys.argv) > 1 else '')
