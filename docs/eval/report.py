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
import functools, json, os, sqlite3, sys, time
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
for name in sorted(os.listdir(J.RUNS)):
    if not name.endswith('.trec'):
        continue
    ranked = defaultdict(list)
    for line in open(f'{J.RUNS}/{name}'):
        qid, _, doc, rank, _, _ = line.split()
        if qid in queries and session_of(doc) != queries[qid]['session']:
            ranked[qid].append((int(rank), doc))
    runs[name[:-5]] = {q: [d for _, d in sorted(v)] for q, v in ranked.items()}

TS = {
    'o': 'SELECT ts FROM observations WHERE id=?',
    's': 'SELECT ts FROM summaries WHERE id=?',
    'p': 'SELECT ts FROM prompts WHERE id=?',
}


@functools.cache
def ts(doc):
    row = db.execute(TS[doc[0]], (int(doc[1:]),)).fetchone()
    return row[0] if row else 0


# When a developer prompt was typed (agent searches carry no time).
asked = {q: ts(q) for q in queries if queries[q]['set'] == 'prompt'}
# claude-mem's window counts back from when its run was collected, not from this report.
now = os.path.getmtime(f'{J.RUNS}/claude-mem.trec') * 1000 if os.path.exists(f'{J.RUNS}/claude-mem.trec') else time.time() * 1000
DAY = 86_400_000
shown = {doc: text for pool in pools.values() for doc, text, _ in pool}


def near_copy(qid, doc):
    """The document holds 80% of the question's trigrams: it quotes the question."""
    a, b = queries[qid]['text'], shown.get(doc, '')
    grams = {a[i:i + 3] for i in range(len(a) - 2)}
    return len(grams & {b[i:i + 3] for i in range(len(b) - 2)}) >= 0.8 * max(1, len(grams))


# `-l2`: only grade 2 and 3 count as relevant; nDCG uses the grades themselves.
metrics = ['ndcg@10', 'mrr@10-l2', 'recall@10-l2', 'hit_rate@10-l2']
print(f'split={split} judge={judge} questions={len(queries)} judged={len(judged)} '
      f'with an answer={len(answerable)} without={len(judged) - len(answerable)}')


def table(label, keep, drop=lambda qid, doc: False):
    """Questions that `keep` accepts; `drop` removes documents from the runs and the qrels alike."""
    qrels = {}
    for q in filter(lambda q: keep(queries[q]), queries):
        grades = {d: g for d, g in judged[q].items() if g > 0 and not drop(q, d)}
        if grades and max(grades.values()) >= 2:
            qrels[q] = grades
    if len(qrels) < 5:
        print(f'\n## {label}: {len(qrels)} questions (too few to score)')
        return
    # Filter inside each run's judged top 20: a document lifted from below it would be unjudged
    # and count as not relevant. A run that loses many of its 20 then shows fewer than 10.
    rs = [Run({q: {d: 1000 - i for i, d in enumerate([d for d in r.get(q, [])[:J.POOL_DEPTH] if not drop(q, d)])}
               for q in qrels}, name=m) for m, r in runs.items()]
    print(f'\n## {label}: {len(qrels)} questions')
    print(compare(Qrels(qrels), rs, metrics=metrics, max_p=0.05, make_comparable=True))


table('all', lambda q: True)
table('question in Japanese', lambda q: q['lang'] == 'ja')
table('question in English', lambda q: q['lang'] == 'en')
table('developer prompts', lambda q: q['set'] == 'prompt')
table('agent searches', lambda q: q['set'] == 'agent')
# claude-mem's default search drops what is older than 90 days, counted from today.
table('prompts typed within 90 days', lambda q: q['qid'] in asked and now - asked[q['qid']] < 90 * DAY)
table('prompts typed 90 days ago or earlier', lambda q: q['qid'] in asked and now - asked[q['qid']] >= 90 * DAY)
# What a search at the time of the question could have found.
table('prompts, documents written before them only', lambda q: q['qid'] in asked,
      drop=lambda qid, doc: ts(doc) >= asked[qid])
# A document that quotes the question matches it word for word, which favours full-text search.
table('documents quoting the question removed', lambda q: True, drop=near_copy)
# claude-mem's run returns observations only (`type=observations`); oboete also returns summaries
# and prompts.
table('observations only', lambda q: True, drop=lambda qid, doc: doc[0] != 'o')
