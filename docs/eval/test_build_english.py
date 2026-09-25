import os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from build_english import select
from common import h, split


def session_on(side, start=0):
    i = start
    while split(f's{i}') != side:
        i += 1
    return f's{i}'


def test_select_takes_new_english_test_prompts_in_hash_order():
    t = session_on('test')
    d = session_on('dev')
    rows = [
        (1, t, 'How do I rotate the Groq key without restarting?'),
        (2, t, 'キーを回したい'),                                   # Japanese: out
        (3, d, 'How do I rotate the NIM key?'),                  # dev side: out
        (4, t, 'Review the staged diff please'),                 # machine prompt: out
        (5, t, 'The same text as a question already in the 424'),   # text taken: out
        (6, t, 'Already in the 424 set, as p6'),                 # qid taken: out
        (7, t, 'short'),                                         # under 15 characters: out
        (8, t, 'Why does the viewer refuse a DELETE from another host?'),
    ]
    taken = {'The same text as a question already in the 424'}
    got = select(rows, have_qids={'p6'}, have_texts=taken, n=5)
    assert [q['qid'] for q in got] == sorted(['p1', 'p8'], key=lambda q: h(f'prompt:{q[1:]}'))
    assert all(q['split'] == 'test' and q['lang'] == 'en' for q in got)
    assert select(rows, have_qids={'p6'}, have_texts=taken, n=1) == got[:1]
