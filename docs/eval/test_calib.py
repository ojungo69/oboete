import os, sqlite3, sys

import pytest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from calib import against_others, draw, fleiss, kappa, majority, parse_grade, store_doc_text
from common import split


def test_kappa_matches_a_worked_example():
    pairs = [(True, True)] * 20 + [(True, False)] * 5 + [(False, True)] * 5 + [(False, False)] * 20
    assert abs(kappa(pairs) - 0.6) < 1e-9          # po 0.8, pe 0.5
    assert kappa([(True, True), (False, False)]) == 1.0


def sessions(side, n):
    out, i = [], 0
    while len(out) < n:
        if split(f's{i}') == side:
            out.append(f's{i}')
        i += 1
    return out


def test_kappa_is_undefined_when_both_raters_use_one_category():
    assert kappa([(True, True)] * 20) is None


def test_draw_takes_dev_pairs_balanced_one_per_question_and_blind():
    dev, test = sessions('dev', 80), sessions('test', 5)
    queries = [{'qid': f'q{i}', 'text': f'question {i}', 'session': s, 'split': 'dev'} for i, s in enumerate(dev)]
    queries += [{'qid': f't{i}', 'text': 't', 'session': s, 'split': 'test'} for i, s in enumerate(test)]
    judgments = []
    for i in range(80):
        for g in range(4):
            judgments.append({'qid': f'q{i}', 'doc': f'o{i}{g}', 'grade': g, 'judge': 'claude-sonnet'})
    judgments += [{'qid': 't0', 'doc': 'o999', 'grade': 3, 'judge': 'claude-sonnet-5'}]      # test side: never
    judgments += [{'qid': 'q0', 'doc': 'o00', 'grade': 3, 'judge': 'some-other-model'}]       # other judge: ignored
    items, key = draw(queries, judgments, lambda doc, chars: f'text of {doc}')
    assert len(items) == len(key) == 50
    grades = sorted(k['grade'] for k in key)
    assert sum(g >= 2 for g in grades) == 25 and grades.count(1) == 12 and grades.count(0) == 13
    assert len({k['qid'] for k in key}) == 50 and all(k['qid'].startswith('q') for k in key)
    assert all('grade' not in str(i) and len(i['choices']) == 2 for i in items)
    assert (items, key) == draw(queries, judgments, lambda doc, chars: f'text of {doc}')   # deterministic
    assert {k['chars'] for k in key} == {1200}   # grades without `chars` saw 1,200 characters


def test_the_panel_sees_the_window_the_judge_saw():
    db = sqlite3.connect(':memory:')
    db.execute('CREATE TABLE prompts (id INTEGER PRIMARY KEY, body TEXT)')
    db.execute("INSERT INTO prompts VALUES (1, ?)", ('x' * 2000,))
    text = store_doc_text(db)
    assert text('p1', 1200) == 'x' * 1200 + '\n…[clipped]'
    assert text('p1', 4000) == 'x' * 2000
    assert text('p2', 4000) is None
    db.execute('CREATE TABLE observations (id INTEGER PRIMARY KEY, title TEXT, body TEXT)')
    db.execute("INSERT INTO observations VALUES (1, '  見出し', ?)", ('y' * 1300,))
    assert text('o1', 1200) == ('見出し\n' + 'y' * 1300)[:1200] + '\n…[clipped]'


def test_fleiss_matches_a_worked_example():
    rows = [[True, True, True], [False, False, False], [True, True, False], [True, False, False]]
    assert abs(fleiss(rows) - 1 / 3) < 1e-9       # P-bar 2/3, Pe 1/2
    assert fleiss([[True, True], [True, True]]) is None


def test_the_reference_never_contains_the_judge_itself():
    grades = {'p1': {'a': 3, 'b': 0, 'c': 0, 'd': 0}, 'p2': {'a': 0, 'b': 2, 'c': 3, 'd': 2},
              'p3': {'a': 2, 'b': 2, 'c': 0, 'd': 1}}
    assert against_others(grades, 'a') == [(True, False), (False, True), (True, False)]
    assert majority([True, False]) is None
    # A tie among the others leaves the pair out.
    assert against_others({'p': {'a': 3, 'b': 3, 'c': 0}}, 'a') == []


def test_parse_grade_takes_fenced_or_reasoned_answers_only():
    assert parse_grade('```json\n{"grades": [{"id": "d", "grade": 2}]}\n```') == 2
    assert parse_grade('<think>maybe {"grade": 3}</think>{"grades": [{"id": "d", "grade": 1}]}') == 1
    assert parse_grade('{"grades": [{"id": "[d]", "grade": 0}]}') == 0
    for bad in ('3', '{"grades": []}', '{"grades": [{"id": "d", "grade": 7}]}', None,
                '{"grades": [{"id": "e", "grade": 2}]}', '{"grades": [{"id": "d", "grade": true}]}',
                '{"grades": [{"id": "d", "grade": 1}, {"id": "d", "grade": 3}]}', '{"grades": [null]}', '{"grades": [1]}'):
        with pytest.raises(ValueError):
            parse_grade(bad)


def test_a_429_waits_its_retry_after_unless_it_is_long(monkeypatch, tmp_path):
    import io, json, urllib.error
    import calib
    from email.message import Message
    def limited(after):
        h = Message()
        h['Retry-After'] = after
        return urllib.error.HTTPError('u', 429, 'limit', h, io.BytesIO(b''))
    class Reply(io.BytesIO):
        def __enter__(self): return self
        def __exit__(self, *a): pass
    replies, slept = [limited('7'), Reply(json.dumps({'choices': [{'message': {'content': 'ok'}}], 'model': 'm'}).encode())], []
    def urlopen(req, timeout):
        r = replies.pop(0)
        if isinstance(r, Exception):
            raise r
        return r
    monkeypatch.setattr(calib.urllib.request, 'urlopen', urlopen)
    monkeypatch.setattr(calib.time, 'sleep', slept.append)
    (tmp_path / 'key.md').write_text('# test\nnot-a-key\n')
    monkeypatch.setattr(calib.os.path, 'expanduser', lambda p: str(tmp_path / 'key.md'))
    assert calib.chat('glm-5.3', 'x') == ('ok', 'm') and slept == [8]
    for long in ('13809', 'Sat, 26 Sep 2026 12:32:00 GMT'):   # a 5-hour limit, as seconds or a date: fail now
        replies[:] = [limited(long)]
        with pytest.raises(urllib.error.HTTPError):
            calib.chat('glm-5.3', 'x')
    assert slept == [8]
