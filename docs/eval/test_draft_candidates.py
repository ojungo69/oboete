import os, sys

import pytest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from draft_candidates import (WEEK, decision_item, pair_item, panel_targets, parse_answer, refill, render,
                              repeat_items, valid_decisions, valid_pairs, windows)


def ev(event, ts, **p):
    return {'event': event, 'ts': ts, 'payload': p}


def test_render_keeps_dialogue_answers_and_collapses_tools():
    lines = render([
        ev('SessionStart', 't0', source='startup'),
        ev('UserPromptSubmit', 't1', prompt='キャッシュの方針を決めたい'),
        ev('UserPromptSubmit', 't1', prompt='<task-notification>\n<task-id>b1</task-id> done</task-notification>'),
        ev('PostToolUse', 't2', tool_name='Read', tool_input={}, tool_response='x' * 5000),
        ev('PostToolUse', 't3', tool_name='Bash', tool_input={}, tool_response='y'),
        ev('PostToolUse', 't4', tool_name='AskUserQuestion', tool_input={'answers': {'どちら?': 'A にする'}}),
        ev('PostToolUse', 't5', tool_name='Grep', agent_id='a1', tool_input={}, tool_response='z'),
        ev('Stop', 't6', last_assistant_message='a' * 4000),
        ev('PostCompact', 't7', compact_summary='要約'),
    ])
    assert [t for _, _, t in lines][:3] == ['USER: キャッシュの方針を決めたい', 'TOOLS: Read, Bash', 'USER ANSWERED: どちら? -> A にする']
    assert [lid for lid, _, _ in lines] == ['L1', 'L2', 'L3', 'L4', 'L5']
    assert lines[3][2].startswith('ASSISTANT: ') and 'characters omitted' in lines[3][2] and len(lines[3][2]) < 1700
    assert lines[4][2] == 'COMPACTED: 要約'


def test_windows_overlap_and_respect_the_limit():
    lines = [(f'L{i}', 't', 'x' * 100) for i in range(1, 101)]
    ws = windows(lines, limit=2000, overlap=3)
    assert all(sum(len(a) + len(c) + 3 for a, _, c in w) <= 2000 for w in ws)
    assert ws[1][:3] == ws[0][-3:] and ws[-1][-1] == lines[-1]


def test_a_quote_must_be_verbatim_and_the_message_the_owners_own():
    window = [('L1', 't1', 'USER: キャッシュは SQLite に置く。依存を増やしたくない'), ('L2', 't2', 'ASSISTANT: 了解')]
    base = {'quote': 'キャッシュは SQLite に置く', 'who': 'user', 'topic': 'x', 'prompt_line': 'L1'}
    found = [
        {**base, 'line': 'L1', 'statement': 'キャッシュは SQLite'},
        {**base, 'line': 'L1', 'quote': 'キャッシュは SQLite にする', 'statement': '言い換え'},   # paraphrase
        {**base, 'line': 'L2', 'statement': '別の行'},                                         # wrong line
        {**base, 'line': 'L1', 'who': 'assistant_only', 'statement': 's'},
        {**base, 'line': 'L9', 'quote': 'x' * 20, 'statement': 's'},
        {**base, 'line': 'L1', 'prompt_line': 'L2', 'statement': 'エージェントの行'},            # not the owner's message
        {**base, 'line': 'L1', 'prompt_line': 'L7', 'statement': '窓の外'},                    # outside the window
    ]
    ok = valid_decisions(found, window)
    assert [d['statement'] for d in ok] == ['キャッシュは SQLite']
    assert ok[0]['prompt'] == 'キャッシュは SQLite に置く。依存を増やしたくない'


def test_the_owner_sees_only_a_plain_sentence_and_their_own_message():
    d = {'id': 'd1', 'ts': '2026-09-01T00:00:00Z', 'statement': 'キャッシュは手元の小さなデータベースに置く',
         'prompt': 'キャッシュは SQLite に置く', 'quote': 'ASSISTANT line text', 'line': 'L3'}
    e = {**d, 'id': 'd2', 'ts': '2026-09-03T00:00:00Z'}
    for item in (decision_item(d), pair_item({'earlier': 'd1', 'later': 'd2'}, {'d1': d, 'd2': e})):
        assert [c['value'] for c in item['choices']][-1] == 'unknown' and len(item['choices']) == 3
        assert {f['text'] for f in item['fields']} == {d['statement'], d['prompt']}


def test_refill_brings_one_more_for_each_answer_that_cannot_count():
    answers = {'a': 'yes', 'b': 'unknown', 'c': 'unknown', 'd': 'no'}
    counts = lambda i: answers.get(i) != 'unknown'
    assert refill(list('abcdefgh'), set('abcde'), 5, counts) == ['f', 'g']
    assert refill(list('abcdefgh'), set(), 3, counts) == ['a', 'b', 'c']
    assert refill(list('ab'), set('ab'), 5, counts) == []          # nothing left to draw: the note says so


def test_the_panel_gets_unknown_items_and_a_blind_sample_of_judged_ones():
    answers = {'a': 'unknown', 'b': 'yes', 'c': 'no', 'd': 'unknown'}       # e: not answered yet
    unknown, sample = panel_targets(list('abcde'), answers, n=5)
    assert unknown == ['a', 'd'] and set(sample) == {'b', 'c'}
    assert len(panel_targets(list('abcde'), answers, n=1)[1]) == 1


def test_a_panel_answer_outside_the_choices_is_refused():
    assert parse_answer('```json\n{"answer": "yes"}\n```', {'yes', 'no'}) == 'yes'
    for bad in ('{"answer": "unknown"}', '{"answer": "maybe"}', 'yes', None):
        with pytest.raises(ValueError):
            parse_answer(bad, {'yes', 'no'})


def test_the_repeat_is_blind_judged_only_and_waits_a_week():
    items = {i: {'id': i, 'fields': []} for i in 'abc'}
    answers = {'a': 'yes', 'b': 'unknown', 'c': 'compatible'}
    assert repeat_items(items, answers, 1_000_000, 1_000_000 + WEEK - 1) is None
    got = repeat_items(items, answers, 1_000_000, 1_000_000 + WEEK)
    assert sorted(i['id'] for i in got) == ['ra', 'rc']


def test_pairs_need_known_ids_in_time_order():
    by_id = {'d1': {'ts': '2026-09-01T00:00:00Z'}, 'd2': {'ts': '2026-09-03T00:00:00Z'}}
    found = [
        {'earlier': 'd1', 'later': 'd2', 'relation': 'overturns', 'why': 'w'},
        {'earlier': 'd2', 'later': 'd1', 'relation': 'overturns', 'why': 'reversed time'},
        {'earlier': 'd1', 'later': 'd9', 'relation': 'compatible', 'why': 'unknown id'},
        {'earlier': 'd1', 'later': 'd2', 'relation': 'maybe', 'why': 'unknown relation'},
        {'earlier': 'd1', 'later': 'd2', 'relation': 'overturns', 'why': 'duplicate'},
    ]
    assert valid_pairs(found, by_id) == [{'earlier': 'd1', 'later': 'd2', 'relation': 'overturns', 'why': 'w'}]
