"""PR-A2: does D1 accept an FTS5 trigram table? Creates a throwaway D1 database and deletes it."""
import cf, json
s, r = cf.call('GET', '/accounts'); A = r['result'][0]['id']
s, r = cf.call('POST', f'/accounts/{A}/d1/database', {'name': 'oboete-spike-trigram'})
assert s == 200, r
db = r['result']['uuid']
try:
    def q(sql, params=None):
        s, r = cf.call('POST', f'/accounts/{A}/d1/database/{db}/query', {'sql': sql, 'params': params or []})
        return s, (r['result'][0]['results'] if s == 200 else r.get('errors'))
    print(q("CREATE VIRTUAL TABLE fts USING fts5(body, tokenize='trigram')"))
    print(q("INSERT INTO fts(body) VALUES (?), (?), (?)", ['検索の精度を上げるための工夫', 'provider chain の既定順を決めた', 'SQLite の trigram で部分一致']))
    print(q("SELECT body FROM fts WHERE fts MATCH ?", ['精度を上']))
    print(q("SELECT body, snippet(fts, 0, '[', ']', '…', 8) AS s FROM fts WHERE fts MATCH ? ORDER BY bm25(fts)", ['trigram']))
    print(q("SELECT sqlite_version() AS v"))
finally:
    print('delete', cf.call('DELETE', f'/accounts/{A}/d1/database/{db}')[0])
