import os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from draft_candidates import render, valid_decisions, valid_pairs, windows


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


def test_a_quote_must_be_verbatim_in_the_line_it_cites():
    window = [('L1', 't1', 'USER: キャッシュは SQLite に置く。依存を増やしたくない'), ('L2', 't2', 'ASSISTANT: 了解')]
    found = [
        {'line': 'L1', 'quote': 'キャッシュは SQLite に置く', 'who': 'user', 'statement': 'キャッシュは SQLite', 'topic': 'キャッシュ'},
        {'line': 'L1', 'quote': 'キャッシュは SQLite にする', 'who': 'user', 'statement': '言い換え', 'topic': 'x'},   # paraphrase
        {'line': 'L2', 'quote': 'キャッシュは SQLite に置く', 'who': 'user', 'statement': '別の行', 'topic': 'x'},    # wrong line
        {'line': 'L1', 'quote': 'キャッシュは SQLite に置く', 'who': 'assistant_only', 'statement': 's', 'topic': 'x'},
        {'line': 'L9', 'quote': 'x' * 20, 'who': 'user', 'statement': 's', 'topic': 'x'},
    ]
    assert [d['statement'] for d in valid_decisions(found, window)] == ['キャッシュは SQLite']


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
