"""Milestone 1, Task 5: the owner's labelling page (docs/milestone-1-plan.md).

  python3 label.py <task name>    serves ~/.oboete/eval/labels/tasks/<name>.jsonl on 127.0.0.1

Each answer appends {"id", "value", "note", "ts"} to ~/.oboete/eval/labels/<name>.jsonl; the latest
line per id wins (value null = withdrawn), so a sitting can stop and resume at any time. Item text
travels as JSON and is put on the page with textContent: transcripts contain markup."""
import http.server, json, os, secrets, sys, threading, time, urllib.parse

from common import E, owner_only, read_jsonl

PAGE = """<!doctype html>
<html lang="ja"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>ラベル付け</title>
<style nonce="@@NONCE@@">
:root { --bg: #ffffff; --fg: #1a1a1a; --muted: #555; --box: #f4f4f4; --line: #999; }
@media (prefers-color-scheme: dark) { :root { --bg: #151515; --fg: #ececec; --muted: #aaa; --box: #242424; --line: #666; } }
body { font-family: system-ui, sans-serif; max-width: 60rem; margin: 1rem auto; padding: 0 16px; line-height: 1.7; background: var(--bg); color: var(--fg); }
#progress { color: var(--muted); }
.field h3 { margin: 1rem 0 .3rem; font-size: 1rem; color: var(--muted); }
.field pre { white-space: pre-wrap; word-break: break-word; background: var(--box); padding: .8rem; border-radius: 6px; max-height: 28rem; overflow: auto; margin: 0; }
button { font-size: 1.05rem; margin: .3rem .3rem .3rem 0; padding: .6rem 1rem; border-radius: 6px; border: 1px solid var(--line); background: var(--box); color: var(--fg); cursor: pointer; }
textarea { width: 100%; min-height: 3rem; box-sizing: border-box; background: var(--box); color: var(--fg); border: 1px solid var(--line); border-radius: 6px; }
</style></head><body>
<p id="progress"></p>
<h2 id="question"></h2>
<div id="fields"></div>
<div id="choices"></div>
<p><label>メモ (任意。直してほしい点などがあれば書いてください)<br><textarea id="note"></textarea></label></p>
<p><button id="back">ひとつ前の答えをやり直す</button></p>
<p id="hint">数字キー 1〜9 でも選べます。途中でやめても、次回は続きから始まります。</p>
<script nonce="@@NONCE@@">
const base = location.pathname.replace(/\\/$/, '');
let current = null;
function el(tag, text) { const e = document.createElement(tag); e.textContent = text; return e; }
async function load() {
  const d = await (await fetch(base + '/next')).json();
  document.getElementById('progress').textContent = `${d.done} / ${d.total} 件 済み`;
  const fields = document.getElementById('fields'), choices = document.getElementById('choices');
  fields.replaceChildren(); choices.replaceChildren();
  document.getElementById('note').value = '';
  current = d.item;
  if (!current) { document.getElementById('question').textContent = 'すべて終わりました。ありがとうございました。このタブは閉じてかまいません。'; return; }
  document.getElementById('question').textContent = current.question;
  for (const f of current.fields) {
    const box = document.createElement('div'); box.className = 'field';
    box.append(el('h3', f.label), el('pre', f.text)); fields.append(box);
  }
  current.choices.forEach((c, i) => {
    const b = el('button', `${i + 1}. ${c.label}`); b.onclick = () => answer(c.value); choices.append(b);
  });
}
async function answer(value) {
  await fetch(base + '/label', { method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id: current.id, value, note: document.getElementById('note').value }) });
  load();
}
document.getElementById('back').onclick = async () => {
  await fetch(base + '/back', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' }); load();
};
document.addEventListener('keydown', e => {
  if (e.target.tagName === 'TEXTAREA' || !current) return;
  const i = parseInt(e.key, 10) - 1;
  if (i >= 0 && i < current.choices.length) answer(current.choices[i].value);
});
load();
</script></body></html>"""


def latest_labels(path):
    out = {}
    if os.path.exists(path):
        for r in read_jsonl(path):
            out[r['id']] = r['value']
    return out


class Store:
    def __init__(self, name, root):
        self.items = read_jsonl(f'{root}/labels/tasks/{name}.jsonl')
        self.by_id = {i['id']: i for i in self.items}
        self.path = f'{root}/labels/{name}.jsonl'
        self.lock = threading.RLock()   # back() withdraws through label() under the same lock

    def next(self):
        latest = latest_labels(self.path)
        todo = [i for i in self.items if latest.get(i['id']) is None]
        return {'done': len(self.items) - len(todo), 'total': len(self.items), 'item': todo[0] if todo else None}

    def label(self, item_id, value, note=''):
        item = self.by_id.get(item_id)
        if item is None:
            raise ValueError(f'unknown item {item_id!r}')
        if value is not None and value not in {c['value'] for c in item['choices']}:
            raise ValueError(f'unknown choice {value!r}')
        line = json.dumps({'id': item_id, 'value': value, 'note': note or '', 'ts': int(time.time())}, ensure_ascii=False)
        with self.lock, open(self.path, 'a', encoding='utf-8') as f:
            f.write(line + '\n')

    def back(self):
        """Withdraw the most recent answer that still stands."""
        with self.lock:   # one snapshot: an answer posted meanwhile must not be skipped
            rows = read_jsonl(self.path) if os.path.exists(self.path) else []
            latest = {r['id']: r['value'] for r in rows}
            for r in reversed(rows):
                if latest.get(r['id']) is not None:
                    self.label(r['id'], None)
                    return


def handler(store, token):
    class H(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def action(self):
            """The path after the token, or None after an error was sent."""
            if self.headers.get('Host', '') != f'127.0.0.1:{self.server.server_address[1]}':
                self.send_error(403)
                return None
            parts = urllib.parse.urlsplit(self.path).path.split('/')
            if len(parts) < 2 or not secrets.compare_digest(parts[1], token):
                self.send_error(404)
                return None
            return parts[2] if len(parts) > 2 else ''

        def send(self, code, body, ctype, extra=()):
            data = body.encode()
            self.send_response(code)
            self.send_header('Content-Type', ctype)
            self.send_header('Content-Length', str(len(data)))
            self.send_header('Cache-Control', 'no-store')
            self.send_header('X-Content-Type-Options', 'nosniff')
            for k, v in extra:
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            a = self.action()
            if a == '':
                nonce = secrets.token_urlsafe(16)
                csp = (f"default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; "
                       "connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")
                self.send(200, PAGE.replace('@@NONCE@@', nonce), 'text/html; charset=utf-8',
                          [('Content-Security-Policy', csp)])
            elif a == 'next':
                self.send(200, json.dumps(store.next(), ensure_ascii=False), 'application/json; charset=utf-8')
            elif a is not None:
                self.send_error(404)

        def do_POST(self):
            a = self.action()
            if a is None:
                return
            if a not in ('label', 'back'):
                self.send_error(404)
                return
            if self.headers.get('Content-Type', '').split(';')[0].strip() != 'application/json':
                self.send_error(415)
                return
            n = int(self.headers.get('Content-Length') or 0)
            if n > 65536:
                self.send_error(413)
                return
            body = self.rfile.read(n)
            try:
                if a == 'label':
                    d = json.loads(body)
                    store.label(d['id'], d['value'], d.get('note', ''))
                else:
                    store.back()
            except (ValueError, KeyError, TypeError) as e:
                self.send(400, json.dumps({'error': str(e)}), 'application/json')
                return
            self.send(200, '{}', 'application/json')

    return H


def serve(name, root=E, port=0):
    token = secrets.token_urlsafe(24)
    srv = http.server.ThreadingHTTPServer(('127.0.0.1', port), handler(Store(name, root), token))
    return srv, token


if __name__ == '__main__':
    owner_only()
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    srv, token = serve(sys.argv[1])
    print(f'ブラウザで開いてください: http://127.0.0.1:{srv.server_address[1]}/{token}/', flush=True)
    print('終わったら Ctrl+C で止めます。途中で止めても、次回は続きから始まります。', flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
