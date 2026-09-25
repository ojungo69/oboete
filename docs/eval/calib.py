"""Milestone 1, Task 6: B3, the judge-trust gate (docs/spec.md 8.1 "Judge trust").

  calib.py draw           50 dev pairs the judge graded -> labels/tasks/calib-50.jsonl and its key
  calib.py kappa          owner vs judge on binary relevance (judge grade >= 2); passes at kappa >= 0.4
  calib.py repeat         a week or more after the last first-round answer: 20 of the 50 again, blind
  calib.py kappa-repeat   owner vs owner on those 20
The owner sees the question and the document as that grade's judge saw them (4,000 characters, 1,200 for
grades written before `chars` was recorded), never the grade."""
import json, os, sqlite3, sys, time

from common import E, SEED, h, owner_only, read_jsonl, write_jsonl
from label import latest_labels

N, N_REPEAT, PASS_KAPPA, WEEK = 50, 20, 0.4, 7 * 86400
JUDGES = {'claude-sonnet-5', 'claude-sonnet'}   # the alias rows of 2026-09-24 came from claude-sonnet-5
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


def repeat_allowed(finished_ts, now):
    return now - finished_ts >= WEEK


def store_doc_text(db):
    def text(doc, chars):
        row = db.execute(DOC_SQL[doc[0]], (int(doc[1:]),)).fetchone()
        if row is None:
            return None
        title, text = row
        # judge.py's own serialization, so the clip lands on the same character.
        body, cut = (title + '\n' + text).strip() if title else text, min(chars, MAX_DOC_CHARS)
        return body if len(body) <= cut else body[:cut] + f'\n…(以下 {len(body) - cut} 文字は省略。判定器も同じところまで読みました)'
    return text


def report(name, pairs, extra):
    k = kappa(pairs) if pairs else None
    agree = sum(a == b for a, b in pairs) / len(pairs) if pairs else None
    out = {'n': len(pairs), 'kappa': k, 'agreement': agree, **extra}
    with open(f'{E}/labels/{name}.result.json', 'w') as f:
        json.dump(out, f, indent=1)
    print(json.dumps(out, indent=1))


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
    elif cmd == 'kappa':
        latest = latest_labels(f'{labels}/calib-50.jsonl')
        key = read_jsonl(f'{labels}/calib-50.key.jsonl')
        pairs = [(latest[k['id']] == 'yes', k['grade'] >= 2) for k in key if latest.get(k['id']) in ('yes', 'no')]
        confusion = {f'owner_{o}_judge_{j}': sum(1 for a, b in pairs if a == (o == 'yes') and b == (j == 'yes'))
                     for o in ('yes', 'no') for j in ('yes', 'no')}
        done = len(pairs) == len(key) == N
        k = kappa(pairs) if pairs else None
        report('calib-50', pairs, {'confusion': confusion, 'complete': done,
                                   'judges': sorted({k['judge'] for k in key}),
                                   'pass': bool(done and k is not None and k >= PASS_KAPPA)})
    elif cmd == 'repeat':
        # A week after the first round is finished: the latest standing answer, not the first one.
        standing = {}
        for r in read_jsonl(f'{labels}/calib-50.jsonl'):
            standing[r['id']] = r
        answered = [r['ts'] for r in standing.values() if r['value'] is not None]
        if len(answered) < N:
            sys.exit(f'the first round has {len(answered)} of {N} answers; the blind repeat opens a week after it is finished')
        if not repeat_allowed(max(answered), int(time.time())):
            sys.exit(f'the blind repeat opens on {time.strftime("%Y-%m-%d", time.localtime(max(answered) + WEEK))}')
        items = read_jsonl(f'{tasks}/calib-50.jsonl')
        chosen = sorted(items, key=lambda i: h(f'repeat:{SEED}:{i["id"]}'))[:N_REPEAT]
        chosen = [{**i, 'id': 'r' + i['id']} for i in sorted(chosen, key=lambda i: h(f'repeat-order:{SEED}:{i["id"]}'))]
        write_jsonl(f'{tasks}/calib-repeat-20.jsonl', chosen)
        print(f'{len(chosen)} pairs -> {tasks}/calib-repeat-20.jsonl')
    elif cmd == 'kappa-repeat':
        first = latest_labels(f'{labels}/calib-50.jsonl')
        again = latest_labels(f'{labels}/calib-repeat-20.jsonl')
        pairs = [(first[i[1:]] == 'yes', v == 'yes') for i, v in again.items()
                 if v in ('yes', 'no') and first.get(i[1:]) in ('yes', 'no')]
        report('calib-repeat-20', pairs, {'what': 'the owner against their own earlier answers',
                                          'complete': len(pairs) == N_REPEAT})
    else:
        sys.exit(__doc__)


if __name__ == '__main__':
    owner_only()
    main(sys.argv[1] if len(sys.argv) > 1 else '')
