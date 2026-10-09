import json, sqlite3, sys, time
import zstandard
raw = sqlite3.connect(sys.argv[1])
d = zstandard.ZstdDecompressor()
def walk(b):
    out = []
    def w(x):
        if isinstance(x, str):
            out.append(x)
        elif isinstance(x, (list, dict)):
            for y in x.values() if isinstance(x, dict) else x:
                w(y)
    w(json.loads(b)); return "\n".join(out)
def scan(word, verify=True):
    # The decompressed body is searched first; only a body that holds the word is parsed, so a
    # word in a key is not taken. (A word JSON escapes would need every body parsed.)
    out = []
    needle = word.encode()
    for seq, z in raw.execute("SELECT seq, z FROM events ORDER BY ts DESC"):
        b = d.decompress(z)
        if needle in b and (not verify or word in walk(b)):
            out.append(seq)
            if len(out) == 20: break
    return out
for w in ["楓樹", "設計"]:
    best = None
    for _ in range(3):
        t = time.perf_counter(); r = scan(w); dt = time.perf_counter() - t
        best = dt if best is None else min(best, dt)
    print(f"{w}: {best*1000:.0f} ms, {len(r)} hits")
