"""PR-B2: claude-mem's own search as a TREC run (decision 22's baseline, docs/pr-b.md item 5).
Calls the running worker read-only (GET /api/search, format=json), maps the observation ids to the
evaluation store's docs through `imports`, and drops (and counts) ids newer than the copy."""
import json, os, sqlite3, sys, urllib.parse, urllib.request

E = os.path.expanduser('~/.oboete/eval')
DEPTH = 50
db = sqlite3.connect(f'file:{E}/home/oboete.db?mode=ro', uri=True)
# One claude-mem database per evaluation store: `oboete import` names it claude-mem:<id>.
sources = [r[0] for r in db.execute("SELECT DISTINCT source FROM imports WHERE source LIKE 'claude-mem%'")]
if len(sources) != 1:
    sys.exit(f'expected one imported claude-mem database, found {sources}')
doc_of = dict(db.execute("SELECT source_id, doc FROM imports WHERE source=? AND source_id LIKE 'o%'", sources))
missing = errors = 0
# A failed query would look like a search that found nothing, so a run with failures is not kept.
with open(f'{E}/runs/claude-mem.trec.part', 'w') as out:
    for line in open(f'{E}/queries.jsonl'):
        q = json.loads(line)
        url = 'http://127.0.0.1:37777/api/search?' + urllib.parse.urlencode(
            {'query': q['text'], 'format': 'json', 'type': 'observations', 'limit': DEPTH})
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
            out.write(f"{q['qid']} Q0 {doc} {rank} {DEPTH - rank + 1} claude-mem\n")
print(f'ids newer than the copy (dropped): {missing}; failed queries: {errors}')
if errors:
    sys.exit('not kept: run it again')
os.replace(f'{E}/runs/claude-mem.trec.part', f'{E}/runs/claude-mem.trec')
