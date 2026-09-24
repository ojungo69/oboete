"""PR-A2: Workers AI bge-m3 input limit, truncate_inputs, and round trip from Japan. Synthetic text only."""
import cf, time, json, statistics, math
s, r = cf.call('GET', '/accounts'); A = r['result'][0]['id']
URL = f'/accounts/{A}/ai/run/@cf/baai/bge-m3'
ja = '検索の精度を上げるために、観測と要約をベクトルにして全文検索と組み合わせる。'  # 37 chars
en = 'The hook stores each event in SQLite and a detached observer summarizes the session later. '
def cos(a, b):
    return sum(x*y for x, y in zip(a, b)) / math.sqrt(sum(x*x for x in a) * sum(y*y for y in b))
print('## input limit')
for name, unit in (('ja', ja), ('en', en)):
    for n in (2000, 4000, 8000, 16000, 32000):
        text = (unit * (n // len(unit) + 1))[:n]
        for trunc in (False, True):
            t = time.time(); st, res = cf.call('POST', URL, {'text': [text], 'truncate_inputs': trunc}, timeout=120)
            ms = (time.time() - t) * 1000
            ok = st == 200 and res.get('success')
            info = f"shape={len(res['result']['data'])}x{len(res['result']['data'][0])} pooling={res['result'].get('pooling')}" if ok else json.dumps(res.get('errors'))[:160]
            print(f'{name} {n:>6} chars trunc={trunc!s:5} http={st} {ms:6.0f} ms {info}')
print('## round trip, 1 short text, 30 calls')
lat = []
for i in range(30):
    t = time.time(); st, res = cf.call('POST', URL, {'text': [f'{ja} {i}']}); lat.append((time.time() - t) * 1000)
lat.sort(); print(f'p50 {statistics.median(lat):.0f} ms  p95 {lat[int(len(lat)*0.95)-1]:.0f} ms  max {lat[-1]:.0f} ms')
print('## batch of 100 texts of ~300 chars, 5 calls')
lat = []
for i in range(5):
    batch = [(ja * 9)[:300] + f' {i}-{j}' for j in range(100)]
    t = time.time(); st, res = cf.call('POST', URL, {'text': batch}, timeout=120); lat.append((time.time() - t) * 1000)
    assert st == 200, res
print('ms per call', [round(x) for x in lat])
print('## prefix vs truncate: does the 8,000-char text embed like its head?')
long = (ja * 300)[:8000]
st, a = cf.call('POST', URL, {'text': [long], 'truncate_inputs': True}, timeout=120)
for head in (1000, 2000, 4000):
    st, b = cf.call('POST', URL, {'text': [long[:head]]})
    print(f'cos(8000 truncated, first {head}) = {cos(a["result"]["data"][0], b["result"]["data"][0]):.4f}')
