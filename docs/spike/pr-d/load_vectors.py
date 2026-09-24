"""PR-D2 measurement: the spike's vectors (docs.jsonl + docs.npy, embed.py) into a store's
`embeddings` table as PR-D1 writes them (unit length, float32 little-endian, text_sha of the gated
text), so `oboete reindex` indexes them without calling the model again. Rows the spike left at
zero norm are skipped; D1 embeds those on its next run. Usage: load_vectors.py <oboete.db>
(open the store once with a D1 binary first so the table exists)."""
import hashlib, json, os, sqlite3, sys
import numpy as np
V = os.path.expanduser('~/.oboete/eval/vec')
db = sqlite3.connect(sys.argv[1])
vecs = np.load(f'{V}/docs.npy', mmap_mode='r')
rows, skipped = [], 0
with open(f'{V}/docs.jsonl') as f:
    for i, line in enumerate(f):
        d = json.loads(line)
        v = np.asarray(vecs[i], dtype=np.float32)
        n = float(np.linalg.norm(v))
        if not d['text'].strip() or not np.isfinite(n) or n == 0:
            skipped += 1
            continue
        rows.append((d['doc'], 'bge-m3', hashlib.sha256(d['text'].encode()).hexdigest(),
                     (v / n).astype('<f4').tobytes()))
        if len(rows) == 5000:
            db.executemany('INSERT OR REPLACE INTO embeddings(doc, embedder, text_sha, vec) VALUES(?,?,?,?)', rows)
            db.commit()
            rows = []
db.executemany('INSERT OR REPLACE INTO embeddings(doc, embedder, text_sha, vec) VALUES(?,?,?,?)', rows)
db.commit()
print(db.execute('SELECT count(*) FROM embeddings').fetchone()[0], 'embeddings;', skipped, 'skipped')
