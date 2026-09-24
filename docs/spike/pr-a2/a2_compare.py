"""PR-A2: Workers AI vs local fastembed bge-m3 on the gated corpus. Sends only the gated text that
`oboete spike-embed` printed (never raw.jsonl). Pass lines: cos min >= 0.99 and top-10 overlap >= 9."""
import cf, json, math, statistics, sys, time
D = sys.argv[1]
raw = {r['id']: r['text'] for r in map(json.loads, open(f'{D}/raw.jsonl'))}
rows = [json.loads(l) for l in open(f'{D}/local.jsonl')]
print(f'gate changed {sum(raw[r["id"]] != r["text"] for r in rows)} of {len(rows)} texts')
s, r = cf.call('GET', '/accounts'); A = r['result'][0]['id']
URL = f'/accounts/{A}/ai/run/@cf/baai/bge-m3'
wa, lat = {}, []
# A request holds at most 100 texts and 60,000 tokens, counted after padding every text to the
# longest one in the request (measured: 20 texts -> "Max context reached 116200 tokens"). Batch
# texts of similar length and keep count x longest under 50,000 chars.
batches, cur = [], []
for x in sorted(rows, key=lambda y: len(y['text'])):
    if cur and (len(cur) == 100 or (len(cur) + 1) * len(x['text']) > 50000):
        batches.append(cur); cur = []
    cur.append(x)
batches.append(cur)
for batch in batches:
    t = time.time(); st, res = cf.call('POST', URL, {'text': [x['text'] for x in batch]}, timeout=180)
    lat.append(round((time.time() - t) * 1000)); assert st == 200 and res.get('success'), json.dumps(res.get('errors'))[:300]
    for x, v in zip(batch, res['result']['data']): wa[x['id']] = v
print('Workers AI ms per batch:', lat, 'sizes', [len(b) for b in batches])
def norm(v):
    n = math.sqrt(sum(x * x for x in v)); return [x / n for x in v]
def dot(a, b): return sum(x * y for x, y in zip(a, b))
W = {k: norm(v) for k, v in wa.items()}; L = {r['id']: norm(r['vec']) for r in rows}
txt = {r['id']: r['text'] for r in rows}
cos = sorted((dot(W[k], L[k]), k) for k in L)
vals = [c for c, _ in cos]
print(f'cos(WA, local) over {len(vals)} texts: min {vals[0]:.5f} ({cos[0][1]}, {len(txt[cos[0][1]])} chars)  p1 {vals[len(vals)//100]:.5f}  p50 {statistics.median(vals):.5f}')
for lo, hi in ((0, 200), (200, 1000), (1000, 3000), (3000, 8001)):
    b = [c for c, k in cos if lo <= len(txt[k]) < hi]
    if b: print(f'  {lo:>5}-{hi:<5} chars: n={len(b):3} min {min(b):.5f}')
docs = [k for k in L if not k.startswith('q:')]; qs = [k for k in L if k.startswith('q:')]
def top(qv, space): return {d for _, d in sorted(((-dot(qv, space[d]), d) for d in docs))[:10]}
for name, qspace, dspace in (('local query, WA docs (offline query)', L, W), ('local query, local docs (local-only)', L, L)):
    ov = [len(top(W[q], W) & top(qspace[q], dspace)) for q in qs]
    print(f'top-10 overlap vs WA/WA, {name}: min {min(ov)}  mean {statistics.mean(ov):.2f}  queries<9: {sum(o < 9 for o in ov)}/{len(ov)}')
