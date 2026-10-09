import os, sqlite3, sys
d = sys.argv[1]
for layout in ["copy", "none"]:
    p = os.path.join(d, f"f-{layout}.db")
    if os.path.exists(p): os.remove(p)
    k = sqlite3.connect(p)
    opts = "" if layout == "copy" else ", content='', contentless_delete=1"
    k.execute(f"CREATE VIRTUAL TABLE raw_fts USING fts5(text, tokenize='trigram'{opts})")
    with k:
        for i in range(1, 2001):
            k.execute("INSERT INTO raw_fts(rowid, text) VALUES (?, ?)", (i, f"common text number {i} " * 20))
    with k:
        k.execute("INSERT INTO raw_fts(rowid, text) VALUES (99999, 'qjzvwk qjzvwk')")
    def grep(tag):
        b = open(p, "rb").read()
        print(layout, tag, "qjz:", b.count(b"qjz"), "jzv:", b.count(b"jzv"), "zvw:", b.count(b"zvw"))
    grep("before")
    with k:
        k.execute("DELETE FROM raw_fts WHERE rowid = 99999")
    k.execute("VACUUM")
    grep("deleted+vacuum")
    with k:
        k.execute("INSERT INTO raw_fts(raw_fts) VALUES('optimize')")
    k.execute("VACUUM")
    grep("optimized+vacuum")
    k.close()
