"""PR-D spike, step 5: sqlite-vec exact kNN speed on the eval corpus, fp32, int8 and bit (+ fp32
rescoring). Usage: vecbench.py <scratch db path> [reuse] (about 1 GB; `reuse` skips the build)."""
import os, sqlite3, sys, time
import numpy as np, sqlite_vec

E = os.path.expanduser('~/.oboete/eval/vec')
docs = np.load(f'{E}/docs.npy', mmap_mode='r')
qs = np.load(f'{E}/queries.npy')
n, dim = docs.shape
out = sys.argv[1]
reuse = sys.argv[2:] == ['reuse'] and os.path.exists(out)
if not reuse and os.path.exists(out):
    os.remove(out)
db = sqlite3.connect(out)
db.enable_load_extension(True)
sqlite_vec.load(db)
if not reuse:
    db.execute(f'CREATE VIRTUAL TABLE v32 USING vec0(embedding float[{dim}] distance_metric=cosine)')
    db.execute(f'CREATE VIRTUAL TABLE v8 USING vec0(embedding int8[{dim}] distance_metric=cosine)')
    db.execute(f'CREATE VIRTUAL TABLE vb USING vec0(embedding bit[{dim}])')
    db.execute('CREATE TABLE f32(id INTEGER PRIMARY KEY, embedding BLOB NOT NULL)')
    t = time.time()
    B = 2000
    for s in range(0, n, B):
        chunk = np.asarray(docs[s:s + B], dtype=np.float32)
        q8 = np.clip(np.round(chunk * 127 / np.abs(chunk).max(axis=1, keepdims=True)), -127, 127).astype(np.int8)
        bits = np.packbits(chunk > 0, axis=1)
        db.executemany('INSERT INTO v32(rowid, embedding) VALUES (?, ?)', [(s + i + 1, r.tobytes()) for i, r in enumerate(chunk)])
        db.executemany('INSERT INTO v8(rowid, embedding) VALUES (?, vec_int8(?))', [(s + i + 1, r.tobytes()) for i, r in enumerate(q8)])
        db.executemany('INSERT INTO vb(rowid, embedding) VALUES (?, vec_bit(?))', [(s + i + 1, r.tobytes()) for i, r in enumerate(bits)])
        db.executemany('INSERT INTO f32(id, embedding) VALUES (?, ?)', [(s + i + 1, r.tobytes()) for i, r in enumerate(chunk)])
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
def bit_rescored(q, table):
    """The whole fallback path: 400 Hamming candidates, their fp32 rows read back from SQLite, cosine.
    `v32` is the vec0 table (stores vectors in chunks), `f32` a plain table with one BLOB per row."""
    cand = [r[0] for r in db.execute('SELECT rowid FROM vb WHERE embedding MATCH vec_bit(?) AND k = 400',
                                     (np.packbits(q > 0).tobytes(),))]
    key = 'rowid' if table == 'v32' else 'id'
    rows = [db.execute(f'SELECT embedding FROM {table} WHERE {key} = ?', (c,)).fetchone()[0] for c in cand]
    m = np.frombuffer(b''.join(rows), dtype=np.float32).reshape(len(rows), -1)
    sims = (m @ q) / np.linalg.norm(m, axis=1)
    return [cand[i] for i in np.argsort(-sims)[:100]]

for table in ('v32', 'f32'):
    times, resc = [], []
    for q in qs[:100]:
        t = time.perf_counter()
        resc.append(bit_rescored(q.astype(np.float32), table))
        times.append((time.perf_counter() - t) * 1000)
    times.sort()
    print(f'bit400 + fp32 rescore from {table} (whole path): p50 {times[49]:.0f} ms, p95 {times[94]:.0f} ms, max {times[-1]:.0f} ms', flush=True)

def overlap(a, b, k):
    return np.mean([len(set(x[:k]) & set(y[:k])) / k for x, y in zip(a, b)])
print(f'int8 top-10 overlap with fp32 {overlap(f32, i8, 10):.3f}, top-100 {overlap(f32, i8, 100):.3f}')
print(f'bit400+rescore top-10 overlap with fp32 {overlap(f32, resc, 10):.3f}, top-100 {overlap(f32, resc, 100):.3f}')
