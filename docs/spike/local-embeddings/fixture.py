"""Task 10's public agreement fixture (docs/spike/local-embeddings.md, line A): 100 English and 100
Japanese texts from this repository's own docs, 25 of each language in each length band, and 20
queries written for it, with their Workers AI bge-m3 vectors. Writes
src/testdata/embed-agreement/items.jsonl and workers-ai.f32 (little-endian float32, 1,024 per item,
in the items' order). Run from the repository's root: python3 docs/spike/local-embeddings/fixture.py
"""
import hashlib, json, os, re, struct, subprocess, tempfile

BANDS = [(20, 200), (200, 1000), (1000, 3000), (3000, 6000)]
PER_BAND = 25
CJK = re.compile(r'[぀-ヿ㐀-鿿]')
QUERIES = [
    ('en', 'how does forgetting a record reach every device'),
    ('en', 'which providers can write the session summaries'),
    ('en', 'the full-text index keeps no copy of the text'),
    ('en', 'what a hook may never do'),
    ('en', 'how the viewer port and its token are chosen'),
    ('en', 'backups and restoring the store after a crash'),
    ('en', 'importing claude-mem history into the store'),
    ('en', 'the cost cap for embeddings'),
    ('en', 'Codex past its plan limit on credits'),
    ('en', 'how secrets are redacted before anything is sent'),
    ('ja', '記録の削除はどの端末まで届くか'),
    ('ja', '要約役を設定画面から追加する'),
    ('ja', 'ツール出力は検索に入るのか'),
    ('ja', 'フックが遅くならないための決まり'),
    ('ja', '埋め込みのモデルを手元で動かす'),
    ('ja', '同期のハブは Cloudflare の上で動く'),
    ('ja', '鍵を画面に表示しない'),
    ('ja', '精度の評価は完成後に一回だけ'),
    ('ja', 'claude-mem との違いと上位互換'),
    ('ja', 'セッションの要約はいつ作られるか'),
]


def paragraphs(path):
    out, buf, fence = [], [], False
    for line in open(path, encoding='utf-8').read().split('\n'):
        if line.strip().startswith('```'):
            fence = not fence
            continue
        if fence or line.lstrip().startswith('|'):
            continue
        if line.strip():
            buf.append(line.strip())
        elif buf:
            out.append(' '.join(buf)); buf = []
    if buf:
        out.append(' '.join(buf))
    return out


def lang(p):
    r = len(CJK.findall(p)) / max(1, len(p))
    return 'ja' if r > 0.2 else 'en' if r == 0 else None


files = subprocess.run(['git', 'ls-files', 'docs/*.md', 'README.md'], capture_output=True, text=True,
                       check=True).stdout.split()
files = [f for f in files if not f.startswith('docs/research/')]
cands = {'en': set(), 'ja': set()}
for f in files:
    ps = [(p, lang(p)) for p in paragraphs(f)]
    for i, (p, l) in enumerate(ps):
        if l is None:
            continue
        cands[l].add(p)
        # Longer texts: this paragraph and the next ones of its language, up to each band's top.
        joined = p
        for q, m in ps[i + 1:]:
            if m != l:
                break
            joined += '\n\n' + q
            if len(joined) >= 6000:
                break
            cands[l].add(joined)

key = lambda t: hashlib.sha256(t.encode()).hexdigest()
items = []
for l in ('en', 'ja'):
    for lo, hi in BANDS:
        band = sorted((t for t in cands[l] if lo <= len(t) < hi), key=key)[:PER_BAND]
        if len(band) < PER_BAND:
            raise SystemExit(f'{l} {lo}-{hi}: only {len(band)} texts')
        items += [{'id': f'{l}-{lo}-{n}', 'kind': 'text', 'lang': l, 'text': t} for n, t in enumerate(band)]
items += [{'id': f'q-{l}-{n}', 'kind': 'query', 'lang': l, 'text': t} for n, (l, t) in enumerate(QUERIES)]

def run(texts):
    """Workers AI's bge-m3 through the `cf` CLI (its own login; no key is read here)."""
    with tempfile.NamedTemporaryFile('w', suffix='.json', encoding='utf-8', delete=False) as f:
        json.dump({'text': texts}, f, ensure_ascii=False)
    try:
        out = subprocess.run(['cf', 'ai', 'run', '@cf/baai/bge-m3', '--body', '@' + f.name, '-q'],
                             capture_output=True, text=True, check=True, timeout=300).stdout
    finally:
        os.unlink(f.name)
    d = json.loads(out)
    return d['data'], d['meta']['neurons']


vecs, neurons = {}, 0.0
# At most 100 texts a request, and count x longest under 50,000 characters (PR-A2, a2_compare.py).
batches, cur = [], []
for x in sorted(items, key=lambda y: len(y['text'])):
    if cur and (len(cur) == 100 or (len(cur) + 1) * len(x['text']) > 50000):
        batches.append(cur); cur = []
    cur.append(x)
batches.append(cur)
for batch in batches:
    data, n = run([x['text'] for x in batch])
    if len(data) != len(batch):
        raise SystemExit(f'{len(data)} vectors for {len(batch)} texts')
    neurons += n
    for x, v in zip(batch, data):
        vecs[x['id']] = v
out = 'src/testdata/embed-agreement'
os.makedirs(out, exist_ok=True)
with open(f'{out}/items.jsonl', 'w', encoding='utf-8') as f:
    for x in items:
        f.write(json.dumps(x, ensure_ascii=False) + '\n')
with open(f'{out}/workers-ai.f32', 'wb') as f:
    for x in items:
        v = vecs[x['id']]
        if len(v) != 1024:
            raise SystemExit(f'{x["id"]}: {len(v)} dims')
        f.write(struct.pack('<1024f', *v))
chars = sum(len(x['text']) for x in items)
print(f'{len(items)} items ({chars} characters) in {len(batches)} requests, {neurons:.1f} neurons')
