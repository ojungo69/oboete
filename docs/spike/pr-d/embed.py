"""PR-D spike, step 2: Workers AI bge-m3 vectors for docs.jsonl and the questions, into
~/.oboete/eval/vec/{docs,queries}.npy (float32, row order of the inputs). Resumable: a row is done
when its norm is non-zero. Only gated text is sent (export.py; the questions were gated on the way
into queries.jsonl)."""
import concurrent.futures, json, os, sys, time
import numpy as np
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'pr-a2'))
import cf  # noqa: E402  (the PR-A2 helper; never prints the key)
os.umask(0o077)
E = os.path.expanduser('~/.oboete/eval')
V = f'{E}/vec'
s, r = cf.call('GET', '/accounts'); A = r['result'][0]['id']
URL = f'/accounts/{A}/ai/run/@cf/baai/bge-m3'

def run(name, texts):
    path = f'{V}/{name}.npy'
    if os.path.exists(path):
        out = np.load(path, mmap_mode='r+')
    else:
        out = np.lib.format.open_memmap(path, mode='w+', dtype=np.float32, shape=(len(texts), 1024))
    todo = [i for i in range(len(texts)) if not out[i].any()]
    print(name, len(texts), 'texts,', len(todo), 'to embed', flush=True)
    # A request: at most 100 texts and 60,000 tokens counted after padding to the longest
    # (PR-A2); texts of similar length together, count x longest under 50,000 characters.
    todo.sort(key=lambda i: len(texts[i]))
    batches, cur = [], []
    for i in todo:
        n = min(len(texts[i]), 12000)
        if cur and (len(cur) == 100 or (len(cur) + 1) * n > 50000):
            batches.append(cur); cur = []
        cur.append(i)
    if cur:
        batches.append(cur)
    def one(batch):
        for attempt in range(5):
            st, res = cf.call('POST', URL, {'text': [texts[i] for i in batch], 'truncate_inputs': True}, timeout=180)
            if st == 200 and res.get('success'):
                return batch, res['result']['data']
            time.sleep(2 ** attempt)
        raise SystemExit(f'failed batch of {len(batch)}: {st} {json.dumps(res.get("errors"))[:200]}')
    done = 0
    with concurrent.futures.ThreadPoolExecutor(4) as ex:
        for n, (batch, vecs) in enumerate(ex.map(one, batches), 1):
            out[batch] = np.asarray(vecs, dtype=np.float32)
            done += len(batch)
            if n % 100 == 0:
                out.flush(); print(f'{done}/{len(todo)}', flush=True)
    out.flush()

docs = [json.loads(l)['text'] for l in open(f'{V}/docs.jsonl')]
queries = [json.loads(l)['text'] for l in open(f'{E}/queries.jsonl')]
run('queries', queries)
run('docs', docs)
print('done')
