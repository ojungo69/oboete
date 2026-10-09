"""#317 step 2 spike: how much of each tool output the raw index keeps.

A sample of a home's raw.db, read only: each event's text as `fts::text` makes it, indexed in an
FTS5 trigram table without a copy (as `raw_fts` is since #417), once per variant of what is kept
of a tool record's text; and the CJK pairs #419 would index, in a table of their own. For each
variant: the index's size, and how many of the whole index's top 10 for the dev queries it still
returns. Prints aggregates only. Masks (tombstones) are not applied: sizes, not answers, are the
point, and the 634 tombstones of 300k records move neither.

  python3 tool_output.py RAW_DB QUERIES OUT_DIR [EVERY]

OUT_DIR is best a tmpfs (/dev/shm): the indexes are built there and deleted after each variant.
"""

import json
import os
import re
import sqlite3
import statistics
import sys
import time

import zstandard

RAW, QUERIES, OUT = sys.argv[1:4]
EVERY = int(sys.argv[4]) if len(sys.argv) > 4 else 5

VARIANTS = [
    ("full", None),
    ("tool 16+4 KB", (16_000, 4_000)),
    ("tool 4+4 KB", (4_000, 4_000)),
    ("tool 1+1 KB", (1_000, 1_000)),
    ("no tool", (0, 0)),
]


def text(body):
    """`fts::text`: every string of the JSON body, in order, joined by newlines; else the body."""
    try:
        v = json.loads(body)
    except ValueError:
        return body
    if not isinstance(v, (dict, list)):
        return body
    out = []

    def walk(x):
        if isinstance(x, str):
            out.append(x)
        elif isinstance(x, (list, dict)):
            for y in x.values() if isinstance(x, dict) else x:
                walk(y)

    walk(v)
    return "\n".join(out)


SEPARATORS = set("、。，．,.!?！？「」『』()（）[]{}:;：；\"'`<>")


def runs(query):
    out, cur = [], []
    for c in query:
        if c.isspace() or (ord(c) < 32 or 127 <= ord(c) < 160) or c in SEPARATORS:
            out.append("".join(cur))
            cur = []
        else:
            cur.append(c)
    out.append("".join(cur))
    return out


def hiragana(c):
    return "぀" <= c <= "ゟ"


def trigrams(query, cap=64):
    """`search::trigrams`."""
    out, seen = [], set()
    for run in runs(query):
        for i in range(len(run) - 2):
            w = run[i : i + 3]
            folded = "".join(c.lower() if c.isascii() else c for c in w)
            if len(out) < cap and not all(hiragana(c) for c in w) and folded not in seen:
                seen.add(folded)
                out.append(w)
    return out


def matching(grams):
    return " OR ".join('"' + g.replace('"', '""') + '"' for g in grams)


def kept(t, bound):
    head, tail = bound
    if head == 0 and tail == 0:
        return None
    if len(t) <= head + tail:
        return t
    return t[:head] + "\n" + t[-tail:]


CJK = re.compile(r"[㐀-鿿豈-﫿゠-ヿㇰ-ㇿ]")


def pairs(t):
    """The two-character runs #419 would index: one Han or katakana character in them at least."""
    out = set()
    for i in range(len(t) - 1):
        p = t[i : i + 2]
        if (CJK.match(p[0]) or CJK.match(p[1])) and not any(c.isspace() for c in p):
            out.add(p)
    return out


def size(path):
    c = sqlite3.connect(path)
    n = c.execute("PRAGMA page_count").fetchone()[0] * c.execute("PRAGMA page_size").fetchone()[0]
    c.close()
    return n


started = time.monotonic()
raw = sqlite3.connect(f"file:{RAW}?mode=ro", uri=True)
z = zstandard.ZstdDecompressor()
records = []  # (rowid, kind, text)
for rowid, kind, body, enc in raw.execute(
    "SELECT rowid, kind, body, enc FROM records WHERE type NOT IN ('removed', 'tombstone') "
    "AND rowid % ? = 0",
    (EVERY,),
):
    if enc == "zstd":
        body = z.decompress(body)
    records.append((rowid, kind, text(bytes(body).decode("utf-8", "replace"))))
total = sum(len(t.encode()) for _, _, t in records)
tool = sum(len(t.encode()) for _, k, t in records if k == "tool")
print(f"sample: 1 in {EVERY}, {len(records)} records, text {total / 1e6:.0f} MB, of it tool {tool / 1e6:.0f} MB "
      f"({time.monotonic() - started:.0f} s)", flush=True)

dev = [json.loads(l) for l in open(QUERIES, encoding="utf-8")]
dev = [q["text"] for q in dev if q.get("split") == "dev"]
queries = [matching(g) for g in (trigrams(q) for q in dev) if g]
print(f"dev queries with a trigram: {len(queries)} of {len(dev)}", flush=True)

os.makedirs(OUT, exist_ok=True)
whole = {}
rows = []
for name, bound in VARIANTS:
    path = os.path.join(OUT, "variant.db")
    if os.path.exists(path):
        os.remove(path)
    t0 = time.monotonic()
    db = sqlite3.connect(path)
    db.execute("CREATE VIRTUAL TABLE t USING fts5(text, tokenize='trigram', content='', contentless_delete=1)")
    kinds = {}
    with db:
        for rowid, kind, t in records:
            if kind == "tool" and bound is not None:
                t = kept(t, bound)
                if t is None:
                    continue
            kinds[rowid] = kind
            db.execute("INSERT INTO t(rowid, text) VALUES(?, ?)", (rowid, t))
    built = time.monotonic() - t0
    mb = size(path) / 1e6
    tops = []
    for q in queries:
        tops.append([r for (r,) in db.execute("SELECT rowid FROM t WHERE t MATCH ? ORDER BY rank LIMIT 10", (q,))])
    db.close()
    os.remove(path)
    if name == "full":
        whole = {"tops": tops, "kinds": kinds}
    overlap, lost_any, lost_tool, lost_other = [], 0, 0, 0
    for full_top, top in zip(whole["tops"], tops):
        if not full_top:
            continue
        gone = set(full_top) - set(top)
        overlap.append(1 - len(gone) / len(full_top))
        lost_any += bool(gone)
        lost_tool += sum(whole["kinds"].get(r) == "tool" for r in gone)
        lost_other += sum(whole["kinds"].get(r) != "tool" for r in gone)
    rows.append((name, mb, built, statistics.mean(overlap), lost_any, len(overlap), lost_tool, lost_other))
    print(rows[-1], flush=True)

# #419: the CJK pairs of every record's text (tool output whole), one row per record, positions not kept.
path = os.path.join(OUT, "pairs.db")
if os.path.exists(path):
    os.remove(path)
db = sqlite3.connect(path)
db.execute("CREATE VIRTUAL TABLE p USING fts5(pairs, tokenize='unicode61', content='', detail=none)")
n_pairs = 0
with db:
    for rowid, kind, t in records:
        ps = pairs(t)
        if ps:
            n_pairs += len(ps)
            db.execute("INSERT INTO p(rowid, pairs) VALUES(?, ?)", (rowid, " ".join(sorted(ps))))
pairs_mb = size(path) / 1e6
db.close()
os.remove(path)

print()
print(f"Sample: one record in {EVERY} of raw.db ({len(records)} records, {total / 1e6:.0f} MB of text, {tool / total:.0%} of it tool output).")
print()
print("| tool output kept | index MB (sample) | of the whole | top 10 kept, mean | queries losing a top-10 hit | lost hits: tool / other |")
print("|---|---|---|---|---|---|")
for name, mb, built, ov, la, nq, lt, lo in rows:
    print(f"| {name} | {mb:.0f} | {mb / rows[0][1]:.0%} | {ov:.0%} | {la} of {nq} | {lt} / {lo} |")
print()
print(f"#419's CJK pairs (detail=none, distinct per record): {pairs_mb:.0f} MB for {n_pairs:,} pairs, {pairs_mb / rows[0][1]:.0%} of the whole trigram index.")
