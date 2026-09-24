"""PR-D spike, step 3: dense (bge-m3, exact cosine over every document) and hybrid (RRF k=60 of
e0-trigram and dense) runs, same-session documents left out before ranking, depth 50."""
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
queries = [json.loads(l) for l in open(f'{E}/queries.jsonl')]
e0 = defaultdict(list)
for line in open(f'{E}/runs/e0-trigram.trec'):
    qid, _, doc, rank, _, _ = line.split()
    e0[qid].append((int(rank), doc))
os.makedirs(f'{E}/runs-d0', exist_ok=True)
with open(f'{E}/runs-d0/vec-bge-m3.trec', 'w') as wv, open(f'{E}/runs-d0/hybrid-rrf.trec', 'w') as wh:
    for i, q in enumerate(queries):
        scores = D @ Q[i]
        top = np.argpartition(-scores, 400)[:400]
        ranked = [docs[j] for j in top[np.argsort(-scores[top])] if session.get(docs[j]) != q['session']][:100]
        for r, d in enumerate(ranked[:DEPTH], 1):
            wv.write(f"{q['qid']} Q0 {d} {r} {DEPTH - r + 1} vec-bge-m3\n")
        fused = defaultdict(float)
        for r, d in enumerate(ranked, 1):
            fused[d] += 1 / (60 + r)
        for r, d in enumerate([d for _, d in sorted(e0[q['qid']])], 1):
            fused[d] += 1 / (60 + r)
        for r, d in enumerate(sorted(fused, key=lambda d: -fused[d])[:DEPTH], 1):
            wh.write(f"{q['qid']} Q0 {d} {r} {DEPTH - r + 1} hybrid-rrf\n")
print('written')
