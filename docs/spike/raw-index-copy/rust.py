"""#317 spike: the same reads through oboete itself, with and without raw_fts's copy of the text.

bench.py measured the SQLite layouts from Python. This replays bench.py's corpus (public crate
sources standing in for tool output, seeded the same way) into one home per oboete binary, as
Codex PostToolUse records, lets the worker index them, and times `oboete search --all --raw only`
from the command line: a query with trigrams, a rare two-character word and a common one. Each
binary's answers must be the same.

  python3 rust.py OUT MB copy=<main's oboete> none=<the branch's oboete>

The homes send nothing: no providers, curation off, embeddings none (the defaults).
"""

import json
import os
import random
import re
import statistics
import subprocess
import sys
import time

OUT = sys.argv[1]
TARGET = int(sys.argv[2]) * 1_000_000
BINARIES = [a.split("=", 1) for a in sys.argv[3:]]
random.seed(317)

files = []
for root, _, names in os.walk(os.path.expanduser("~/.cargo/registry/src")):
    for n in names:
        if n.endswith((".rs", ".md", ".toml", ".txt")):
            files.append(os.path.join(root, n))
files.sort()
random.shuffle(files)

RARE = "楓樹"
COMMON = "設計"
records = []
size = 0
for path in files:
    try:
        data = open(path, encoding="utf-8").read()
    except (UnicodeDecodeError, OSError):
        continue
    pos = 0
    while pos < len(data) and size < TARGET:
        n = random.choice([800, 2_000, 4_000, 6_000, 12_000, 40_000, 120_000])
        text = data[pos : pos + n]
        pos += n
        records.append(text)
        size += len(text.encode())
    if size >= TARGET:
        break
for i in range(0, len(records), 20):
    records[i] += f"\n{COMMON}の方針を確認した。"
step = max(1, len(records) // 30)
for i in range(0, len(records), step)[:30]:
    records[i] += f"\n{RARE}の件"
print(f"records {len(records)}, text {size / 1e6:.0f} MB", flush=True)

os.makedirs(OUT, exist_ok=True)
fixture = os.path.join(OUT, "fixture.jsonl")
with open(fixture, "w", encoding="utf-8") as f:
    for i, text in enumerate(records):
        payload = {
            "session_id": "s317",
            "transcript_path": "/nonexistent/s317.jsonl",
            "cwd": "__OBOETE_REPLAY_ROOT__",
            "hook_event_name": "PostToolUse",
            "model": "m",
            "permission_mode": "default",
            "turn_id": f"t{i // 50}",
            "tool_use_id": f"u{i}",
            "tool_name": "Bash",
            "tool_input": {"command": f"cat part-{i}"},
            "tool_response": text,
        }
        line = {"seq": i, "agent": "codex", "event": "PostToolUse", "session": "s317", "payload": payload}
        f.write(json.dumps(line, ensure_ascii=False) + "\n")

QUERIES = [("trigram", ["compress", "buffer"]), ("short rare", [RARE]), ("short common", [COMMON])]
RUNS = 7


def mb(path):
    return sum(os.path.getsize(path + s) for s in ("", "-wal") if os.path.exists(path + s)) / 1e6


rows, answers = [], {}
for label, binary in BINARIES:
    home = os.path.join(OUT, label)
    os.makedirs(home, exist_ok=True)
    with open(os.path.join(home, "config.toml"), "w") as f:
        f.write('providers = []\n\n[summary]\ncurate = false\n\n[embedding]\nprovider = "none"\n')
    env = {k: v for k, v in os.environ.items() if not any(s in k for s in ("KEY", "TOKEN", "SECRET", "PASSWORD"))}
    run = lambda *a: subprocess.run([binary, "--home", home, *a], env=env, capture_output=True, text=True, check=True)
    started = time.monotonic()
    run("replay", fixture, "--spawn-sample", "0", "--agent", "codex")
    replayed = time.monotonic() - started
    started = time.monotonic()
    run("worker", "--idle-ms", "3000")
    indexed = time.monotonic() - started - 3
    row = [label, f"{mb(os.path.join(home, 'raw.db')):.0f}", f"{mb(os.path.join(home, 'knowledge.db')):.0f}", f"{replayed:.0f}", f"{indexed:.0f}"]
    for name, words in QUERIES:
        times, out = [], ""
        for _ in range(RUNS):
            started = time.monotonic()
            out = run("search", "--all", "--raw", "only", "--limit", "20", "--", *words).stdout
            times.append((time.monotonic() - started) * 1000)
        # The homes differ in their device id, the records' times and their path.
        out = re.sub(r"(?m)^[0-9a-f]{8}:", "DEVICE:", out)
        out = re.sub(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2} UTC", "TIME", out).replace(home, "HOME")
        answers.setdefault(name, {})[label] = out
        row.append(f"{statistics.median(times):.0f}")
    rows.append(row)
    print(row, flush=True)

print("| binary | raw.db MB | knowledge.db MB | replay s | index s | trigram ms | short rare ms | short common ms |")
print("|---|---|---|---|---|---|---|---|")
for r in rows:
    print("| " + " | ".join(r) + " |")
for name, outs in answers.items():
    same = len(set(outs.values())) == 1
    print(f"{name}: same answers {same}, lines {[len(o.splitlines()) for o in outs.values()]}")
