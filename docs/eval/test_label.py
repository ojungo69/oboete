import http.client, json, os, sys, tempfile, threading

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import write_jsonl
from label import latest_labels, serve

ITEMS = [{'id': f'i{n}', 'question': '役に立ちますか？', 'fields': [{'label': '問い', 'text': t}],
          'choices': [{'value': 'yes', 'label': 'はい'}, {'value': 'no', 'label': 'いいえ'}]}
         for n, t in enumerate(['<script>alert(1)</script><b>x</b>', 'plain', 'third'])]


class Server:
    def __init__(self):
        self.dir = tempfile.TemporaryDirectory()
        write_jsonl(f'{self.dir.name}/labels/tasks/t.jsonl', ITEMS)
        self.srv, self.token = serve('t', root=self.dir.name)
        self.port = self.srv.server_address[1]
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()

    def req(self, method, path, body=None, host=None, ctype='application/json'):
        c = http.client.HTTPConnection('127.0.0.1', self.port)
        headers = {'Host': host or f'127.0.0.1:{self.port}'}
        if body is not None:
            headers['Content-Type'] = ctype
        c.request(method, path, body=None if body is None else json.dumps(body), headers=headers)
        r = c.getresponse()
        data = r.read().decode()
        c.close()
        return r.status, data, r

    def close(self):
        self.srv.shutdown()
        self.srv.server_close()
        self.dir.cleanup()


def test_host_and_token_are_checked():
    s = Server()
    try:
        assert s.req('GET', f'/{s.token}/', host='evil.example')[0] == 403
        assert s.req('GET', '/not-the-token/')[0] == 404
        assert s.req('GET', f'/{s.token}/')[0] == 200
    finally:
        s.close()


def test_item_text_is_never_html():
    s = Server()
    try:
        _, page, r = s.req('GET', f'/{s.token}/')
        assert '<script>alert(1)</script>' not in page      # items never go into the HTML
        assert 'innerHTML' not in page                        # the page sets text with textContent
        assert "default-src 'none'" in r.getheader('Content-Security-Policy')
        _, data, r = s.req('GET', f'/{s.token}/next')
        assert r.getheader('Content-Type').startswith('application/json')
        assert json.loads(data)['item']['fields'][0]['text'].startswith('<script>')
    finally:
        s.close()


def test_latest_label_wins_and_next_skips_labelled():
    s = Server()
    try:
        b = f'/{s.token}'
        assert s.req('POST', f'{b}/label', {'id': 'i0', 'value': 'yes', 'note': ''})[0] == 200
        assert s.req('POST', f'{b}/label', {'id': 'i0', 'value': 'no', 'note': '見直した'})[0] == 200  # second tab
        nxt = json.loads(s.req('GET', f'{b}/next')[1])
        assert nxt['done'] == 1 and nxt['total'] == 3 and nxt['item']['id'] == 'i1'
        assert s.req('POST', f'{b}/label', {'id': 'i1', 'value': 'maybe'})[0] == 400                 # not a choice
        assert s.req('POST', f'{b}/label', {'id': 'zz', 'value': 'yes'})[0] == 400                   # not an item
        assert s.req('POST', f'{b}/label', {'id': 'i1', 'value': 'yes'}, ctype='text/plain')[0] == 415
        assert s.req('POST', f'{b}/back', {})[0] == 200                                              # withdraw i0
        assert json.loads(s.req('GET', f'{b}/next')[1])['item']['id'] == 'i0'
        assert latest_labels(f'{s.dir.name}/labels/t.jsonl') == {'i0': None}
    finally:
        s.close()
