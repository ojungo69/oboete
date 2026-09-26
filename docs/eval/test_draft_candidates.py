import json, os, sys

import pytest

import draft_candidates

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from draft_candidates import (WEEK, context, decision_item, events_of, pair_chunks, pair_item, panel_targets, parse_answer, refill, render,
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
    # The model writes line ids as 1, '1' or '[L1]' too; the owner's answer to a question is their own.
    window.append(('L3', 't3', 'USER ANSWERED: どちら? -> A にする'))
    more = [{**base, 'line': 1, 'prompt_line': '[L3]', 'statement': '数字の行'}]
    assert [(d['line'], d['prompt']) for d in valid_decisions(more, window)] == [('L1', 'どちら? -> A にする')]


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
    got = repeat_items(items, answers, 1_000_000, 1_000_000 + WEEK, n=2)
    assert sorted(i['id'] for i in got) == ['ra', 'rc']
    assert repeat_items(items, answers, 1_000_000, 1_000_000 + WEEK, n=3) is None     # only 2 judged yet


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


def test_transcript_lines_split_on_newlines_only():
    out = '{"event": "Stop", "payload": {"last_assistant_message": "a\u2028b"}}\n' + '{"event": "SessionEnd", "payload": {}}\n'
    assert [e['event'] for e in events_of(out)] == ['Stop', 'SessionEnd']


def test_every_two_decisions_of_a_repository_meet_in_one_prompt():
    for n in (30, 80, 81, 200):
        ds = list(range(n))
        chunks = pair_chunks(ds)
        assert max(map(len, chunks)) <= 80
        assert all(any(a in c and b in c for c in chunks) for a in ds for b in ds if a < b)
    assert pair_chunks(list(range(80))) == [list(range(80))]       # as one prompt, as before


def test_the_panel_sees_the_proposal_and_the_owners_acceptance():
    lines = [(f'L{i}', 't', f'line {i}') for i in range(1, 21)]
    text = context({'line': 'L3', 'prompt_line': 'L15'}, around=1, lines=lines)
    assert text.split('\n') == ['  line 2', '▶ line 3', '  line 4', '  …', '  line 14', '▶ line 15', '  line 16']


def test_a_pair_met_in_several_prompts_is_kept_once(monkeypatch):
    ds = [{'id': f'd{i}', 'repo': 'r', 'ts': f'2026-09-01T00:{i // 60:02d}:{i % 60:02d}Z', 'statement': 's', 'quote': 'q'}
          for i in range(120)]
    # Every prompt answers about d1 and d2 (one block), once as each relation.
    answers = iter([{'pairs': [{'earlier': 'd1', 'later': 'd2', 'relation': rel}]} for rel in ('overturns', 'compatible') * 3])
    monkeypatch.setattr(draft_candidates, 'ask', lambda prompt, budget: next(answers))
    got = draft_candidates.pairs({'left': 10}, ds)
    assert [(p['earlier'], p['later'], p['relation']) for p in got] == [('d1', 'd2', 'overturns')]


def test_panel_parses_the_text_of_each_completion(monkeypatch, tmp_path):
    monkeypatch.setattr(draft_candidates, 'LABELS', str(tmp_path))
    monkeypatch.setattr(draft_candidates, 'DRAFTS', str(tmp_path))
    monkeypatch.setattr(draft_candidates, 'PANEL', {'j1': None, 'j2': None})
    monkeypatch.setattr(draft_candidates, 'chat', lambda member, prompt: ('{"answer": "yes"}', f'{member}-model'))
    monkeypatch.setattr(draft_candidates, 'gate', lambda text: text)
    monkeypatch.setattr(draft_candidates, 'context', lambda d: 'ctx')
    (tmp_path / 'decisions.jsonl').write_text('')
    (tmp_path / 'dev-decisions.key.jsonl').write_text('{"id": "t1", "statement": "s"}\n')
    (tmp_path / 'dev-decisions.jsonl').write_text('{"id": "t1", "value": "unknown", "ts": 1}\n')
    (tmp_path / 'dev-pairs.key.jsonl').write_text('')
    draft_candidates.panel()
    rows = [json.loads(l) for l in (tmp_path / 'dev-decisions.panel.jsonl').read_text().splitlines()]
    assert sorted((r['judge'], r['value'], r['model']) for r in rows) == [('j1', 'yes', 'j1-model'), ('j2', 'yes', 'j2-model')]


def test_a_message_read_by_two_windows_gives_its_decisions_once(monkeypatch, tmp_path):
    lines = [('L1', 't1', 'USER: キャッシュは SQLite に置く'), ('L2', 't2', 'USER: 同期は 45 秒ごとに行う')]
    (tmp_path / 'rendered').mkdir()
    (tmp_path / 'rendered' / 's.repo.jsonl').write_text('{"repo": "r"}\n')
    monkeypatch.setattr(draft_candidates, 'DRAFTS', str(tmp_path))
    monkeypatch.setattr(draft_candidates, 'dev_sessions', lambda: [{'session': 's'}])
    monkeypatch.setattr(draft_candidates, 'rendered', lambda s: lines)
    monkeypatch.setattr(draft_candidates, 'windows', lambda ls: [ls[:1], ls])       # L1 is in both windows
    d = lambda line, quote: {'line': line, 'prompt_line': line, 'quote': quote, 'who': 'user', 'statement': quote, 'topic': 't'}
    answers = iter([{'decisions': [d('L1', 'キャッシュは SQLite')]},
                    {'decisions': [d('L1', 'SQLite に置く'), d('L2', '同期は 45 秒ごとに行う')]}])       # L1 again, another quote
    monkeypatch.setattr(draft_candidates, 'ask', lambda prompt, budget: next(answers))
    got, complete = draft_candidates.decisions({'left': 9})
    assert complete and [(x['id'], x['quote']) for x in got] == [('d1', 'キャッシュは SQLite'), ('d3', '同期は 45 秒ごとに行う')]


def test_a_judge_whose_alias_moved_since_calibration_gives_no_labels(monkeypatch, tmp_path):
    monkeypatch.setattr(draft_candidates, 'LABELS', str(tmp_path))
    monkeypatch.setattr(draft_candidates, 'PANEL', {'j1': None, 'grok-4.7': None})
    result = {'panel_pass': True, 'judges': {'j1': {'pass': True}, 'grok-4.7': {'pass': True}}, 'models': {
        'j1': ['openai/x (requested; the reply was not recorded)'], 'grok-4.7': ['grok-4.7-build']}}
    (tmp_path / 'calib-50.result-3.json').write_text(json.dumps(result))
    (tmp_path / 'dev-decisions.key.jsonl').write_text('{"id": "a"}\n')
    (tmp_path / 'dev-decisions.jsonl').write_text('{"id": "a", "value": "unknown", "ts": 1}\n')
    (tmp_path / 'dev-pairs.key.jsonl').write_text('')
    for model, labelled in (('grok-4.7-build', True), ('grok-5', False)):
        (tmp_path / 'dev-decisions.panel.jsonl').write_text(''.join(json.dumps(
            {'id': 'a', 'judge': j, 'value': 'yes', 'model': m}) + '\n' for j, m in (('j1', 'x'), ('grok-4.7', model))))
        draft_candidates.report()
        r = json.loads((tmp_path / 'dev-labels.result.json').read_text())['dev-decisions']
        assert (r['panel_on_unknown'] is not None) == labelled
        assert r['moved_since_calibration'] == ([] if labelled else ['grok-4.7'])
    result['judges']['grok-4.7']['pass'] = False                       # a judge that failed B3
    (tmp_path / 'calib-50.result-3.json').write_text(json.dumps(result))
    draft_candidates.report()
    r = json.loads((tmp_path / 'dev-labels.result.json').read_text())['dev-decisions']
    assert r['panel_on_unknown'] is None and r['not_calibrated'] == ['grok-4.7']
