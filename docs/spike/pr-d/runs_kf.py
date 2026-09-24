"""PR-D spike, step 4: the product's order (knowledge = observations and summaries first, prompts
after) applied to dense and hybrid. Dense is ranked within each kind; hybrid fuses e0 and dense
within each kind (RRF k=60). Same-session documents left out, depth 50."""
import json, os, sqlite3
from collections import defaultdict
import numpy as np
os.umask(0o077)
E = os.path.expanduser('~/.oboete/eval')
V = f'{E}/vec'
DEPTH = 50
db = sqlite3.connect(f'file:{E}/home/oboete.db?mode=ro', uri=True)
docs = [json.loads(l)['doc'] for l in open(f'{V}/docs.jsonl')]
session = {}
for t, k in (('observations', 'o'), ('summaries', 's'), ('prompts', 'p')):
    for i, s in db.execute(f'SELECT id, session_id FROM {t}'):
        session[f'{k}{i}'] = s
D = np.load(f'{V}/docs.npy', mmap_mode='r')
D = D / np.maximum(np.linalg.norm(D, axis=1, keepdims=True), 1e-9)
Q = np.load(f'{V}/queries.npy')
Q = Q / np.maximum(np.linalg.norm(Q, axis=1, keepdims=True), 1e-9)
is_prompt = np.array([d[0] == 'p' for d in docs])
queries = [json.loads(l) for l in open(f'{E}/queries.jsonl')]
e0 = defaultdict(list)
for line in open(f'{E}/runs/e0-trigram.trec'):
    qid, _, doc, rank, _, _ = line.split()
    e0[qid].append((int(rank), doc))

def dense(scores, mask, sess):
    s = np.where(mask, scores, -np.inf)
    top = np.argpartition(-s, 400)[:400]
    return [docs[j] for j in top[np.argsort(-s[top])] if np.isfinite(s[j]) and session.get(docs[j]) != sess][:100]

def rrf(*lists):
    fused = defaultdict(float)
    for l in lists:
        for r, d in enumerate(l, 1):
            fused[d] += 1 / (60 + r)
    return sorted(fused, key=lambda d: -fused[d])

with open(f'{E}/runs-d0/vec-kf.trec', 'w') as wv, open(f'{E}/runs-d0/hybrid-kf.trec', 'w') as wh:
    for i, q in enumerate(queries):
        scores = D @ Q[i]
        know, prom = dense(scores, ~is_prompt, q['session']), dense(scores, is_prompt, q['session'])
        for r, d in enumerate((know + prom)[:DEPTH], 1):
            wv.write(f"{q['qid']} Q0 {d} {r} {DEPTH - r + 1} vec-kf\n")
        lex = [d for _, d in sorted(e0[q['qid']])]
        fused = rrf(know, [d for d in lex if d[0] != 'p']) + rrf(prom, [d for d in lex if d[0] == 'p'])
        for r, d in enumerate(fused[:DEPTH], 1):
            wh.write(f"{q['qid']} Q0 {d} {r} {DEPTH - r + 1} hybrid-kf\n")
print('written')
