"""PR-D spike, step 1: the evaluation store's documents as the text PR-D would embed (proposal
§2.5), passed through `oboete gate` in chunks. Writes ~/.oboete/eval/vec/docs.jsonl (0600)."""
import json, os, sqlite3, subprocess, sys
os.umask(0o077)
E = os.path.expanduser('~/.oboete/eval')
OUT = f'{E}/vec'
os.makedirs(OUT, exist_ok=True)
OBOETE = sys.argv[1]
db = sqlite3.connect(f'file:{E}/home/oboete.db?mode=ro', uri=True)
rows = []
for i, kind, title, body in db.execute('SELECT id, kind, title, body FROM observations'):
    rows.append({'doc': f'o{i}', 'text': f'{kind}: {title}\n{body}'})
for i, body in db.execute('SELECT id, body FROM summaries'):
    rows.append({'doc': f's{i}', 'text': body})
for i, body in db.execute('SELECT id, body FROM prompts'):
    rows.append({'doc': f'p{i}', 'text': body[:1000]})
print(len(rows), 'documents', flush=True)

def gate(chunk):
    text = ''.join(json.dumps(r, ensure_ascii=False) + '\n' for r in chunk)
    out = subprocess.run([OBOETE, 'gate'], input=text, capture_output=True, text=True, check=True).stdout
    lines = out.splitlines()
    try:
        got = [json.loads(l) for l in lines]
        if len(got) == len(chunk) and all(g['doc'] == r['doc'] for g, r in zip(got, chunk)):
            return got
    except ValueError:
        pass
    # A block that spans documents broke the chunk: gate them one by one.
    return [json.loads(subprocess.run([OBOETE, 'gate'], input=json.dumps(r, ensure_ascii=False),
                                      capture_output=True, text=True, check=True).stdout) for r in chunk]

changed = 0
with open(f'{OUT}/docs.jsonl', 'w') as w:
    for k in range(0, len(rows), 500):
        chunk = rows[k:k + 500]
        for r, g in zip(chunk, gate(chunk)):
            changed += r['text'] != g['text']
            w.write(json.dumps(g, ensure_ascii=False) + '\n')
print('gate changed', changed, 'texts')
