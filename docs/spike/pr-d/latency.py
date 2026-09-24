"""PR-D2: wall time of `oboete search --all --limit 10` (process start included) over one split's
questions, as the CLI and MCP see it. The store's `config.toml` decides full-text or hybrid.
Usage: latency.py <oboete binary> <home> <queries.jsonl> <split>"""
import json, statistics, subprocess, sys, time
oboete, home, path, split = sys.argv[1:5]
qs = [q['text'] for q in map(json.loads, open(path)) if q['split'] == split]
ms = []
for q in qs:
    t = time.perf_counter()
    r = subprocess.run([oboete, '--home', home, 'search', '--all', '--limit', '10', '--', q],
                       capture_output=True, text=True)
    ms.append((time.perf_counter() - t) * 1000)
    if r.returncode or r.stderr.strip():
        print('stderr:', r.stderr.strip()[:200], file=sys.stderr)
ms.sort()
print(f'{len(ms)} questions: p50 {statistics.median(ms):.0f} ms, p95 {ms[int(len(ms) * 0.95)]:.0f} ms, max {ms[-1]:.0f} ms')
