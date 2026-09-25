import datetime, json, os, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from fixtures import OUT, generate

NAMES = ('long-24h', 'middle-only', 'overturn-cross', 'canaries')


def ts(s):
    return datetime.datetime.fromisoformat(s.replace('Z', '+00:00'))


def test_generation_is_deterministic_and_matches_the_committed_files():
    with tempfile.TemporaryDirectory() as d:
        generate(d)
        for name in sorted(os.listdir(d)):
            with open(os.path.join(d, name), 'rb') as a, open(os.path.join(OUT, name), 'rb') as b:
                assert a.read() == b.read(), name


def test_expectations_hold_in_the_fixtures():
    with open(f'{OUT}/expected.json', encoding='utf-8') as f:
        exp = json.load(f)
    text = {}
    for n in NAMES:
        with open(f'{OUT}/{n}.jsonl', encoding='utf-8') as f:
            text[n] = f.read()
        seqs = [json.loads(line)['seq'] for line in text[n].splitlines()]
        assert seqs == list(range(1, len(seqs) + 1)), n
    for c in exp['canaries']['canaries'] + [exp['canaries']['private']]:
        assert text['canaries'].count(c) == 1, c
    assert f"<private>{exp['canaries']['private']}</private>" in text['canaries']
    for n in ('long-24h', 'middle-only', 'overturn-cross'):
        for d in exp[n]['decisions']:
            assert d['fragment'] in text[n], (n, d['id'])
    lines = [json.loads(line) for line in text['long-24h'].splitlines()]
    span = (ts(lines[-1]['ts']) - ts(lines[0]['ts'])).total_seconds() / 3600
    assert 23.5 <= span <= 24.5
    m = text['middle-only']
    i = m.index(exp['middle-only']['decisions'][0]['fragment'])
    assert i > 20_000 and len(m) - i > 20_000       # past the 16,000-character prompt either side
    assert {p['relation'] for p in exp['overturn-cross']['pairs']} == {'overturns', 'compatible'}
