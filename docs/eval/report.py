"""PR-B2: metrics from the runs and the judge's grades (proposal §3.2), with ranx.

    uv run --with ranx python report.py <split> [judge]

Compares the runs in ~/.oboete/eval/runs; to compare another method, put its run there and judge
the new pooled pairs first.

Relevant = grade 2 or 3 (UMBRELA "has an answer"); nDCG uses the grades. A question counts only
if the judge found at least one relevant document for it (the rest have no answer in the pool and
are reported as a count, not scored). Unjudged documents count as not relevant, so a run is only
comparable to the runs that fed the pool. Documents of the question's own session are left out
of every run first (the same rule as `oboete eval`).
"""
import json, os, sqlite3, sys
from collections import defaultdict

from ranx import Qrels, Run, compare

import judge as J

E = J.E
split = sys.argv[1]
judge = sys.argv[2] if len(sys.argv) > 2 else J.JUDGE
db = sqlite3.connect(f'file:{E}/home/oboete.db?mode=ro', uri=True)
queries = {q['qid']: q for q in map(json.loads, open(f'{E}/queries.jsonl')) if q['split'] == split}
latest = J.latest(judge)

# A partly judged pool would score whichever questions happened to be judged first.
runs_now = J.load_runs()
pools = {qid: J.pool(db, runs_now, q) for qid, q in queries.items()}
missing = sum(1 for qid, pool in pools.items() for doc, _, n in pool
              if not ((qid, doc) in latest and J.covers(latest[(qid, doc)], n)))
if missing:
    sys.exit(f'{missing} pooled pairs have no grade for the text the judge now sees; '
             f'run judge.py {split} {len(queries)} first')

# Qrels are the grades of this pool: the runs in runs/ now. Grades kept from other experiments'
# pools would change which questions count and the recall denominators.
judged = defaultdict(dict)
for qid, pool in pools.items():
    for doc, _, _ in pool:
        judged[qid][doc] = latest[(qid, doc)]['grade']
answerable = {q for q, d in judged.items() if max(d.values()) >= 2}


SESSION_OF = {
    'o': 'SELECT session_id FROM observations WHERE id=?',
    's': 'SELECT session_id FROM summaries WHERE id=?',
    'p': 'SELECT session_id FROM prompts WHERE id=?',
}


def session_of(doc, cache={}):
    if doc not in cache:
        row = db.execute(SESSION_OF[doc[0]], (int(doc[1:]),)).fetchone()
        cache[doc] = row[0] if row else None
    return cache[doc]


runs = {}
for name in sorted(os.listdir(f'{E}/runs')):
    if not name.endswith('.trec'):
        continue
    scores = defaultdict(dict)
    for line in open(f'{E}/runs/{name}'):
        qid, _, doc, rank, _, _ = line.split()
        if qid in answerable and session_of(doc) != queries[qid]['session']:
            scores[qid][doc] = 1000 - int(rank)
    for qid in answerable:
        scores.setdefault(qid, {})
    runs[name[:-5]] = scores

qrels = Qrels({q: {d: g for d, g in judged[q].items() if g > 0} for q in answerable})
# `-l2`: only grade 2 and 3 count as relevant; nDCG uses the grades themselves.
metrics = ['ndcg@10', 'mrr@10-l2', 'recall@10-l2', 'hit_rate@10-l2']
print(f'split={split} judge={judge} questions={len(queries)} judged={len(judged)} '
      f'with an answer={len(answerable)} without={len(judged) - len(answerable)}')


def table(label, keep):
    qs = [q for q in answerable if keep(queries[q])]
    if len(qs) < 5:
        print(f'\n## {label}: {len(qs)} questions (too few to score)')
        return
    sub = Qrels({q: qrels.to_dict()[q] for q in qs if q in qrels.to_dict()})
    rs = [Run({q: runs[m].get(q, {}) for q in qs}, name=m) for m in runs]
    print(f'\n## {label}: {len(qs)} questions')
    print(compare(sub, rs, metrics=metrics, max_p=0.05, make_comparable=True))


table('all', lambda q: True)
table('question in Japanese', lambda q: q['lang'] == 'ja')
table('question in English', lambda q: q['lang'] == 'en')
table('developer prompts', lambda q: q['set'] == 'prompt')
table('agent searches', lambda q: q['set'] == 'agent')
