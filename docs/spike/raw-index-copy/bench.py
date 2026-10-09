"""#317 spike: the raw full-text index's own copy of the text.

Three layouts of knowledge.db's raw index over one synthetic corpus (public crate sources standing
in for tool output, plus a few Japanese records), each with the record bodies zstd-compressed in a
separate raw.db as oboete keeps them:

  copy        raw_fts(text) keeps its own copy of the text (today)
  none        raw_fts is contentless (contentless_delete=1); text is read back from raw.db
  compressed  raw_fts is contentless; knowledge.db keeps a zstd copy, read through a function

Measured: file sizes, build time, and the reads oboete makes of the text (a trigram query, a short
query with no trigram over every record, the same with matches near the newest, snippets of 20
hits), then a forget of 1,000 records and whether their canary is still in knowledge.db's bytes.
"""

import json
import os
import random
import sqlite3
import sys
import time

import zstandard

OUT = sys.argv[1]
TARGET = int(sys.argv[2]) * 1_000_000  # bytes of text
random.seed(317)

# -- corpus -------------------------------------------------------------------------------------
files = []
for root, _, names in os.walk(os.path.expanduser("~/.cargo/registry/src")):
    for n in names:
        if n.endswith((".rs", ".md", ".toml", ".txt")):
            files.append(os.path.join(root, n))
files.sort()
random.shuffle(files)

RARE = "楓樹"  # in 30 records spread over the history
COMMON = "設計"  # in every 20th record
CANARY = "canary-317-"

records = []  # (ts, text)
size = 0
ts = 0
for path in files:
    try:
        data = open(path, encoding="utf-8").read()
    except (UnicodeDecodeError, OSError):
        continue
    pos = 0
    while pos < len(data) and size < TARGET:
        # Tool outputs: mostly a few KB, some long.
        n = random.choice([800, 2_000, 4_000, 6_000, 12_000, 40_000, 120_000])
        text = data[pos : pos + n]
        pos += n
        ts += 1
        records.append([ts, text])
        size += len(text.encode())
    if size >= TARGET:
        break

for i, r in enumerate(records):
    if i % 20 == 0:
        r[1] += f"\n{COMMON}の方針を確認した。"
step = max(1, len(records) // 30)
for i in range(0, len(records), step)[:30]:
    records[i][1] += f"\n{RARE}の件"
forgotten = list(range(len(records) // 2, len(records) // 2 + 1000))
for i in forgotten:
    records[i][1] += f"\n{CANARY}{i:06d}"

print(f"records {len(records)}, text {size / 1e6:.0f} MB", flush=True)

cctx = zstandard.ZstdCompressor(level=3)
dctx = zstandard.ZstdDecompressor()


def unz(b):
    return dctx.decompress(b).decode()


def body(text):
    return json.dumps({"output": text})


def walk_text(b):
    # fts::text: every string of the JSON body
    v = json.loads(b)
    out = []

    def w(x):
        if isinstance(x, str):
            out.append(x)
        elif isinstance(x, list):
            for y in x:
                w(y)
        elif isinstance(x, dict):
            for y in x.values():
                w(y)

    w(v)
    return "\n".join(out)


# -- raw.db (shared by all layouts) -------------------------------------------------------------
os.makedirs(OUT, exist_ok=True)
rawp = os.path.join(OUT, "raw.db")
if os.path.exists(rawp):
    os.remove(rawp)
raw = sqlite3.connect(rawp)
raw.execute("CREATE TABLE events(seq INTEGER PRIMARY KEY, ts INTEGER, z BLOB)")
with raw:
    raw.executemany(
        "INSERT INTO events VALUES (?, ?, ?)",
        ((i + 1, ts, cctx.compress(body(t).encode())) for i, (ts, t) in enumerate(records)),
    )
raw.close()


def build(layout):
    p = os.path.join(OUT, f"knowledge-{layout}.db")
    if os.path.exists(p):
        os.remove(p)
    k = sqlite3.connect(p)
    k.create_function("unz", 1, unz, deterministic=True)
    k.execute(
        "CREATE TABLE raw_docs(rowid INTEGER PRIMARY KEY, seq INTEGER NOT NULL, ts INTEGER NOT NULL)"
    )
    k.execute("CREATE INDEX raw_docs_ts ON raw_docs(ts)")
    if layout == "copy":
        k.execute("CREATE VIRTUAL TABLE raw_fts USING fts5(text, tokenize='trigram')")
    else:
        k.execute(
            "CREATE VIRTUAL TABLE raw_fts USING fts5(text, tokenize='trigram', content='', contentless_delete=1)"
        )
    if layout == "compressed":
        k.execute("CREATE TABLE raw_text(rowid INTEGER PRIMARY KEY, z BLOB NOT NULL)")
    t0 = time.perf_counter()
    with k:
        for i, (ts, text) in enumerate(records):
            rid = i + 1
            k.execute("INSERT INTO raw_docs VALUES (?, ?, ?)", (rid, rid, ts))
            k.execute("INSERT INTO raw_fts(rowid, text) VALUES (?, ?)", (rid, text))
            if layout == "compressed":
                k.execute("INSERT INTO raw_text VALUES (?, ?)", (rid, cctx.compress(text.encode())))
    built = time.perf_counter() - t0
    k.close()
    return p, built


def mb(p):
    return os.path.getsize(p) / 1e6


def timed(f, n=3):
    best = None
    for _ in range(n):
        t0 = time.perf_counter()
        r = f()
        dt = time.perf_counter() - t0
        best = dt if best is None else min(best, dt)
    return best, r


def queries(layout, p):
    k = sqlite3.connect(p)
    k.create_function("unz", 1, unz, deterministic=True)
    raw = sqlite3.connect(rawp)
    res = {}

    # 1. trigram query, newest 20
    def tri():
        return k.execute(
            "SELECT d.seq FROM raw_fts f JOIN raw_docs d ON d.rowid = f.rowid "
            "WHERE raw_fts MATCH ? ORDER BY d.ts DESC LIMIT 20",
            ('"serialize"',),
        ).fetchall()

    res["trigram"] = timed(tri)

    # 2./3. a short query with no trigram: LIKE over the text, newest first, 20 rows
    def short(word):
        pat = f"%{word}%"
        if layout == "copy":
            sql = (
                "SELECT d.seq FROM raw_fts f JOIN raw_docs d ON d.rowid = f.rowid "
                "WHERE f.text LIKE ? ORDER BY d.ts DESC LIMIT 20"
            )
            return lambda: k.execute(sql, (pat,)).fetchall()
        if layout == "compressed":
            sql = (
                "SELECT d.seq FROM raw_docs d JOIN raw_text t ON t.rowid = d.rowid "
                "WHERE unz(t.z) LIKE ? ORDER BY d.ts DESC LIMIT 20"
            )
            return lambda: k.execute(sql, (pat,)).fetchall()

        # none: newest first from raw.db, each body read back, as Rust would page through it
        def scan():
            out = []
            for seq, z in raw.execute("SELECT seq, z FROM events ORDER BY ts DESC"):
                if word in walk_text(unz(z)):
                    out.append((seq,))
                    if len(out) == 20:
                        break
            return out

        return scan

    res["short_rare"] = timed(short(RARE), n=1)
    res["short_common"] = timed(short(COMMON))

    # 4. snippets of 20 hits
    seqs = [s for (s,) in res["trigram"][1]]

    def snippets():
        out = []
        for s in seqs:
            if layout == "copy":
                (t,) = k.execute("SELECT text FROM raw_fts WHERE rowid = ?", (s,)).fetchone()
            elif layout == "compressed":
                (t,) = k.execute("SELECT unz(z) FROM raw_text WHERE rowid = ?", (s,)).fetchone()
            else:
                (z,) = raw.execute("SELECT z FROM events WHERE seq = ?", (s,)).fetchone()
                t = walk_text(unz(z))
            out.append(t[:200])
        return out

    res["snippets"] = timed(snippets)
    k.close()
    raw.close()
    return res


def forget(layout, p):
    k = sqlite3.connect(p)
    with k:
        for i in forgotten:
            rid = i + 1
            k.execute("DELETE FROM raw_fts WHERE rowid = ?", (rid,))
            k.execute("DELETE FROM raw_docs WHERE rowid = ?", (rid,))
            if layout == "compressed":
                k.execute("DELETE FROM raw_text WHERE rowid = ?", (rid,))
    k.execute("VACUUM")
    k.close()
    data = open(p, "rb").read()
    return sum(data.count(f"{CANARY}{i:06d}".encode()) for i in forgotten[:100])


rows = []
for layout in ["copy", "none", "compressed"]:
    p, built = build(layout)
    size_k = mb(p)
    r = queries(layout, p)
    left = forget(layout, p)
    rows.append(
        (
            layout,
            size_k,
            built,
            r["trigram"][0],
            len(r["trigram"][1]),
            r["short_rare"][0],
            len(r["short_rare"][1]),
            r["short_common"][0],
            r["snippets"][0],
            left,
        )
    )
    print(rows[-1], flush=True)

print(f"raw.db {mb(rawp):.0f} MB")
print(
    "| layout | knowledge.db MB | build s | trigram ms (hits) | short rare ms (hits) | short common ms | 20 snippets ms | canaries left |"
)
print("|---|---|---|---|---|---|---|---|")
for l, s, b, t, tn, sr, srn, sc, sn, left in rows:
    print(
        f"| {l} | {s:.0f} | {b:.0f} | {t * 1000:.1f} ({tn}) | {sr * 1000:.0f} ({srn}) | {sc * 1000:.1f} | {sn * 1000:.1f} | {left} |"
    )
