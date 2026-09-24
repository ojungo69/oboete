"""PR-B2: claude-mem's own search as a TREC run (decision 22's baseline, docs/pr-b.md item 5).
Calls the running worker read-only (GET /api/search, format=json), maps the observation ids to the
evaluation store's docs through `imports`, and drops (and counts) ids newer than the copy.

    run_claude_mem.py              -> runs/claude-mem.trec, as the owner's claude-mem searches today
    run_claude_mem.py --no-window  -> runs/claude-mem-nowindow.trec, without its 90-day window

By default claude-mem keeps only Chroma hits from the last 90 days, counted from today, so older
questions lose their answers; `dateStart` replaces that window (SearchManager.ts, 13.25.3)."""
import json, os, sqlite3, sys, urllib.parse, urllib.request

E = os.path.expanduser('~/.oboete/eval')
# The questions and runs are the developer's own records: owner-only files.
os.umask(0o077)
os.makedirs(E, mode=0o700, exist_ok=True)
os.chmod(E, 0o700)
DEPTH = 50
NAME = 'claude-mem-nowindow' if sys.argv[1:] == ['--no-window'] else 'claude-mem'
WINDOW = {'dateStart': '2000-01-01'} if NAME == 'claude-mem-nowindow' else {}
db = sqlite3.connect(f'file:{E}/home/oboete.db?mode=ro', uri=True)
# One claude-mem database per evaluation store: `oboete import` names it claude-mem:<id>.
sources = [r[0] for r in db.execute("SELECT DISTINCT source FROM imports WHERE source LIKE 'claude-mem%'")]
if len(sources) != 1:
    sys.exit(f'expected one imported claude-mem database, found {sources}')
doc_of = dict(db.execute("SELECT source_id, doc FROM imports WHERE source=? AND source_id LIKE 'o%'", sources))
missing = errors = 0
# A failed query would look like a search that found nothing, so a run with failures is not kept.
with open(f'{E}/runs/{NAME}.trec.part', 'w') as out:
    for line in open(f'{E}/queries.jsonl'):
        q = json.loads(line)
        url = 'http://127.0.0.1:37777/api/search?' + urllib.parse.urlencode(
            {'query': q['text'], 'format': 'json', 'type': 'observations', 'limit': DEPTH, **WINDOW})
        try:
            with urllib.request.urlopen(url, timeout=60) as r:
                obs = json.loads(r.read()).get('observations') or []
        except Exception as e:
            errors += 1
            print('error', q['qid'], str(e)[:80], file=sys.stderr)
            continue
        rank = 0
        for o in obs:
            doc = doc_of.get(f"o{o['id']}")
            if doc is None:
                missing += 1
                continue
            rank += 1
            out.write(f"{q['qid']} Q0 {doc} {rank} {DEPTH - rank + 1} {NAME}\n")
print(f'ids newer than the copy (dropped): {missing}; failed queries: {errors}')
if errors:
    sys.exit('not kept: run it again')
os.replace(f'{E}/runs/{NAME}.trec.part', f'{E}/runs/{NAME}.trec')
