"""PR-D spike, step 5: sqlite-vec exact kNN speed on the eval corpus, fp32, int8 and bit (+ fp32
rescoring). Usage: vecbench.py <scratch db path> (about 1 GB)."""
import os, sqlite3, sys, time
import numpy as np, sqlite_vec

E = os.path.expanduser('~/.oboete/eval/vec')
docs = np.load(f'{E}/docs.npy', mmap_mode='r')
qs = np.load(f'{E}/queries.npy')
n, dim = docs.shape
out = sys.argv[1]
if os.path.exists(out):
    os.remove(out)
db = sqlite3.connect(out)
db.enable_load_extension(True)
sqlite_vec.load(db)
db.execute(f'CREATE VIRTUAL TABLE v32 USING vec0(embedding float[{dim}] distance_metric=cosine)')
db.execute(f'CREATE VIRTUAL TABLE v8 USING vec0(embedding int8[{dim}] distance_metric=cosine)')
db.execute(f'CREATE VIRTUAL TABLE vb USING vec0(embedding bit[{dim}])')
t = time.time()
B = 2000
for s in range(0, n, B):
    chunk = np.asarray(docs[s:s + B], dtype=np.float32)
    q8 = np.clip(np.round(chunk * 127 / np.abs(chunk).max(axis=1, keepdims=True)), -127, 127).astype(np.int8)
    bits = np.packbits(chunk > 0, axis=1)
    db.executemany('INSERT INTO v32(rowid, embedding) VALUES (?, ?)', [(s + i + 1, r.tobytes()) for i, r in enumerate(chunk)])
    db.executemany('INSERT INTO v8(rowid, embedding) VALUES (?, vec_int8(?))', [(s + i + 1, r.tobytes()) for i, r in enumerate(q8)])
    db.executemany('INSERT INTO vb(rowid, embedding) VALUES (?, vec_bit(?))', [(s + i + 1, r.tobytes()) for i, r in enumerate(bits)])
db.commit()
print(f'load {time.time() - t:.0f} s, file {os.path.getsize(out) / 1e6:.0f} MB', flush=True)

def bench(name, sql, arg):
    times, res = [], []
    for q in qs[:100]:
        t = time.perf_counter()
        rows = db.execute(sql, (arg(q),)).fetchall()
        times.append((time.perf_counter() - t) * 1000)
        res.append([r[0] for r in rows])
    times.sort()
    print(f'{name}: p50 {times[49]:.0f} ms, p95 {times[94]:.0f} ms, max {times[-1]:.0f} ms', flush=True)
    return res

f32 = bench('fp32', 'SELECT rowid FROM v32 WHERE embedding MATCH ? AND k = 100', lambda q: q.astype(np.float32).tobytes())
i8 = bench('int8', 'SELECT rowid FROM v8 WHERE embedding MATCH vec_int8(?) AND k = 100',
           lambda q: np.clip(np.round(q * 127 / np.abs(q).max()), -127, 127).astype(np.int8).tobytes())
bt = bench('bit', 'SELECT rowid FROM vb WHERE embedding MATCH vec_bit(?) AND k = 400', lambda q: np.packbits(q > 0).tobytes())
def overlap(a, b, k):
    return np.mean([len(set(x[:k]) & set(y[:k])) / k for x, y in zip(a, b)])
print(f'int8 top-10 overlap with fp32 {overlap(f32, i8, 10):.3f}, top-100 {overlap(f32, i8, 100):.3f}')
# bit top 400, rescored with fp32 from the numpy array (what a Rust version would read from the fp32 table).
resc = []
for q, cand in zip(qs[:100], bt):
    ids = np.array(cand) - 1
    m = np.asarray(docs[ids], dtype=np.float32)
    sims = (m @ q) / np.linalg.norm(m, axis=1)
    resc.append([int(ids[i]) + 1 for i in np.argsort(-sims)[:100]])
print(f'bit400+rescore top-10 overlap with fp32 {overlap(f32, resc, 10):.3f}, top-100 {overlap(f32, resc, 100):.3f}')
