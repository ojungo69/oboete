import os, sqlite3, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from calib import draw, kappa, repeat_allowed, store_doc_text
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


def test_the_owner_sees_the_window_the_judge_saw():
    db = sqlite3.connect(':memory:')
    db.execute('CREATE TABLE prompts (id INTEGER PRIMARY KEY, body TEXT)')
    db.execute("INSERT INTO prompts VALUES (1, ?)", ('x' * 2000,))
    text = store_doc_text(db)
    assert text('p1', 1200).startswith('x' * 1200 + '\n…(以下 800 文字は省略')
    assert text('p1', 4000) == 'x' * 2000
    assert text('p2', 4000) is None
    db.execute('CREATE TABLE observations (id INTEGER PRIMARY KEY, title TEXT, body TEXT)')
    db.execute("INSERT INTO observations VALUES (1, '  見出し', ?)", ('y' * 1300,))
    assert text('o1', 1200) == ('見出し\n' + 'y' * 1300)[:1200] + '\n…(以下 104 文字は省略。判定器も同じところまで読みました)'


def test_the_repeat_waits_a_week():
    day = 86400
    # Counted from the last first-round answer: pairs answered on the last day still get a week.
    assert not repeat_allowed(1_000_000, 1_000_000 + 6 * day)
    assert repeat_allowed(1_000_000, 1_000_000 + 7 * day)
