import json, os, subprocess, sys

import pytest

import common

_popen = subprocess.Popen


def fake_mcp(tmp_path, monkeypatch, behavior=''):
    server = tmp_path / 'mcp.py'
    server.write_text('#!' + sys.executable + '\n' + r'''
import json, sys
def send(reply):
    print(json.dumps(dict(jsonrpc='2.0', **reply)), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    assert request['jsonrpc'] == '2.0'
    if request['method'] == 'initialize':
        assert request['params']['capabilities'] == {}
        send({'id': request['id'], 'result': {'protocolVersion': '2024-11-05', 'capabilities': {}, 'serverInfo': {'name': 'fake', 'version': '1'}}})
    elif request['method'] == 'notifications/initialized':
        assert 'id' not in request
    else:
        assert request['method'] == 'tools/call'
        assert request['params']['name'] == 'search'
''' + behavior + r'''
        if request['id'] == 2:
            send({'method': 'notifications/message', 'params': {'level': 'info'}})
            send({'id': 3, 'result': {'content': [{'type': 'text', 'text': 'future'}]}})
            send({'id': 2, 'result': {'content': [{'type': 'text', 'text': request['params']['arguments']['query']}]}})
''', encoding='utf-8')
    server.chmod(0o700)

    def start(argv, **kwargs):
        assert argv == [str(server), '--home', str(tmp_path / 'home'), 'mcp']
        assert kwargs['cwd'] == str(tmp_path)
        assert not any(s in k.upper() for k in kwargs['env'] for s in ('KEY', 'TOKEN', 'SECRET', 'PASSWORD'))
        return _popen(argv, **kwargs)

    monkeypatch.setattr(subprocess, 'Popen', start)
    return str(server)


def test_the_mcp_client_waits_for_each_result_and_fails_on_an_error(tmp_path, monkeypatch):
    binary = fake_mcp(tmp_path, monkeypatch)
    with common.Mcp(binary, str(tmp_path / 'home'), str(tmp_path)) as client:
        assert client.call('search', {'query': 'first'}) == {'content': [{'type': 'text', 'text': 'first'}]}
        assert client.call('search', {'query': 'second'}) == {'content': [{'type': 'text', 'text': 'future'}]}
    client.close()
    for behavior in (
            "        send({'id': request['id'], 'error': {'code': -1, 'message': 'error'}})\n        continue\n",
            "        send({'id': request['id'], 'result': {'isError': True}})\n        continue\n",
            "        sys.exit(9)\n"):
        binary = fake_mcp(tmp_path, monkeypatch, behavior)
        with common.Mcp(binary, str(tmp_path / 'home'), str(tmp_path)) as client:
            with pytest.raises(ConnectionError):
                client.call('search', {'query': 'error'})


@pytest.mark.parametrize('behavior', [
    "        send({'id': request['id'], 'error': {'code': -1, 'message': 'PRIVATE-MCP-ERROR'}})\n        continue\n",
    "        send({'id': request['id'], 'result': {'isError': True, 'content': [{'type': 'text', 'text': 'PRIVATE-MCP-ERROR'}]}})\n        continue\n",
    "        sys.exit(9)\n",
    "        print('PRIVATE-MCP-ERROR', flush=True)\n        continue\n",
    "        send({'id': request['id'], 'result': 'PRIVATE-MCP-ERROR'})\n        continue\n",
])
def test_the_mcp_client_fails_safely_on_error_or_exit(tmp_path, monkeypatch, behavior, capsys):
    binary = fake_mcp(tmp_path, monkeypatch, behavior)
    with common.Mcp(binary, str(tmp_path / 'home'), str(tmp_path)) as client:
        with pytest.raises(ConnectionError) as failure:
            client.call('search', {'query': 'PRIVATE-PROMPT'})
    assert 'PRIVATE' not in str(failure.value)
    assert capsys.readouterr() == ('', '')


def test_json_parsing_reads_as_calib_parse_grade_does():
    assert common.parse_json('```json\n{"ok": true}\n```') == {'ok': True}
    assert common.parse_json('<think>{"ok": false}</think> {"ok": true}') == {'ok': True}
    assert common.parse_json('{"ok": true}<think>{"ok": false}</think>') == {'ok': True}
    assert common.parse_json('```json\n{"ok": true}\n```<think>reason</think>') == {'ok': True}
    assert common.parse_json('```json\n  <think>reason</think>\n {"ok": true} \n```') == {'ok': True}
    assert common.parse_json('{"text":"<think>literal text</think>"}') == {'text': '<think>literal text</think>'}
    # A sentence around the JSON, as calib.parse_grade allows: a judge that adds one is not failed.
    for prose in ('Here is JSON: {"ok": true}', '{"ok": true} done', '```json\n{"ok": true}\n``` done',
                  '```json {"ok": true} ```', '<think>unfinished {"ok": true}'):
        assert common.parse_json(prose) == {'ok': True}
    for bad in ('[]', 'true', None, 'no answer', '{"ok": true} and {"ok": false}', '{not json}'):
        with pytest.raises(ValueError):
            common.parse_json(bad)


def boolean_answer(answer):
    return type(answer.get('ok')) is bool


def test_a_successful_model_call_is_shared_by_model_and_prompt_and_kept_owner_only(monkeypatch):
    import calib
    seen = []

    def chat(model, prompt):
        seen.append((model, prompt))
        return '{"ok": true}', model + '-reported'

    monkeypatch.setattr(calib, 'chat', chat)
    first = common.Calls()
    assert first.call('glm-5.3', 'PRIVATE-PROMPT', boolean_answer) == {'ok': True}
    path = common.E + '/m6/calls.jsonl'
    os.chmod(path, 0o644)               # a previous run with loose permissions is repaired too
    resumed = common.Calls()
    assert resumed.call('glm-5.3', 'PRIVATE-PROMPT', boolean_answer) == {'ok': True}
    assert resumed.call('glm-5.3', 'another prompt', boolean_answer) == {'ok': True}
    assert resumed.call('deepseek-v4-pro', 'PRIVATE-PROMPT', boolean_answer) == {'ok': True}
    assert seen == [('glm-5.3', 'PRIVATE-PROMPT'), ('glm-5.3', 'another prompt'), ('deepseek-v4-pro', 'PRIVATE-PROMPT')]
    assert resumed.models() == {'glm-5.3': ['glm-5.3-reported'], 'deepseek-v4-pro': ['deepseek-v4-pro-reported']}
    assert 'PRIVATE-PROMPT' not in open(path).read()
    assert os.stat(path).st_mode & 0o777 == 0o600
    assert os.stat(os.path.dirname(path)).st_mode & 0o777 == 0o700


@pytest.mark.parametrize('replies', [
    ['PRIVATE-RAW-REPLY', 'PRIVATE-RAW-REPLY'],
    ['{"ok": "yes"}', '{"ok": "yes"}'],
    [ConnectionError('PRIVATE-ERROR')],
])
def test_a_failed_call_is_private_and_retried_on_the_next_run(monkeypatch, replies, capsys):
    import calib
    pending = list(replies)

    def chat(model, prompt):
        reply = pending.pop(0)
        if isinstance(reply, Exception):
            raise reply
        return reply, 'reported'

    monkeypatch.setattr(calib, 'chat', chat)
    calls = common.Calls()
    with pytest.raises(common.FailedCall) as failure:
        calls.call('glm-5.3', 'PRIVATE-PROMPT', boolean_answer)
    assert pending == []                       # invalid twice, failed transport once
    assert 'PRIVATE' not in str(failure.value)
    path = common.E + '/m6/calls.jsonl'
    rows = common.read_jsonl(path)
    assert len(rows) == 1 and rows[0]['failed']
    if isinstance(replies[0], str):
        assert rows[0]['model'] == 'reported'
    assert 'PRIVATE' not in open(path).read()
    with pytest.raises(common.FailedCall):
        calls.call('glm-5.3', 'PRIVATE-PROMPT', boolean_answer)
    assert len(common.read_jsonl(path)) == 1     # failures wait for the next command
    pending[:] = ['{"ok": true}']
    assert common.Calls().call('glm-5.3', 'PRIVATE-PROMPT', boolean_answer) == {'ok': True}
    assert capsys.readouterr() == ('', '')


def test_an_invalid_answer_is_retried_once_before_success_and_cache_is_revalidated(monkeypatch):
    import calib
    replies = ['{"ok": 1}', '{"ok": true}']
    monkeypatch.setattr(calib, 'chat', lambda model, prompt: (replies.pop(0), 'reported'))
    assert common.Calls().call('glm-5.3', 'q', boolean_answer) == {'ok': True}
    assert replies == []
    with pytest.raises(common.FailedCall):
        common.Calls().call('glm-5.3', 'q', lambda answer: False)


def test_votes_are_pending_until_all_graders_answer_and_agreement_omits_other_ties(monkeypatch):
    import calib
    replies = {'gpt-oss-120b': '{"ok": true}', 'deepseek-v4-pro': '{"ok": false}',
               'glm-5.3': '{"ok": true}'}

    def chat(model, prompt):
        if model == 'glm-5.3' and prompt == 'pending':
            raise ConnectionError('PRIVATE-ERROR')
        return replies[model], model

    monkeypatch.setattr(calib, 'chat', chat)
    calls = common.Calls()
    full = calls.votes('complete', ('ok',))
    pending = calls.votes('pending', ('ok',))
    assert common.voted(full, 'ok') is True
    assert common.voted(pending, 'ok') is None
    assert pending['glm-5.3'] is None
    assert common.agreement([full, pending], 'ok') == {
        'gpt-oss-120b': {'n': 0, 'agreement': None},
        'deepseek-v4-pro': {'n': 1, 'agreement': 0.0},
        'glm-5.3': {'n': 0, 'agreement': None}}
    replies['glm-5.3'] = '{"ok": 1}'        # numeric truth is not a Boolean vote
    assert common.Calls().votes('wrong type', ('ok',))['glm-5.3'] is None


def test_panel_draw_and_check_use_the_recorded_seed_and_another_checker():
    import calib
    assert common.PANEL == ('claude-sonnet-5', *calib.PANEL)
    assert common.draw('m6-key', 'q1') == 'kimi-k3'
    assert common.draw('m6-checker', 'q1', exclude='kimi-k3') == 'glm-5.3'
    assert common.checked('m6-check', 'q1') is True
    assert common.checked('m6-check', 'q3') is False


def test_the_pinned_sonnet_uses_the_existing_isolated_answerer(monkeypatch):
    import calib
    seen = []
    monkeypatch.setattr(common, 'claude_json', lambda prompt, model, timeout: seen.append((prompt, model, timeout)) or '{"ok": true}')
    original_panel = dict(calib.PANEL)
    assert calib.chat('claude-sonnet-5', 'p') == ('{"ok": true}', 'claude-sonnet-5')
    assert seen == [('p', 'claude-sonnet-5', 300)]
    assert calib.PANEL == original_panel


def test_the_answerer_uses_the_pinned_claude_call_and_shared_cache(monkeypatch):
    import calib
    seen = []
    monkeypatch.setattr(common, 'claude_json', lambda prompt, model: seen.append((prompt, model)) or '{"ok": true}')
    monkeypatch.setattr(calib, 'chat', lambda *args: pytest.fail('answerer is common.claude_json'))
    calls = common.Calls()
    assert calls.call('claude-sonnet-5', 'p', boolean_answer, answerer=True) == {'ok': True}
    assert common.Calls().call('claude-sonnet-5', 'p', boolean_answer, answerer=True) == {'ok': True}
    assert seen == [('p', 'claude-sonnet-5')]
    assert calls.models() == {'claude-sonnet-5': ['claude-sonnet-5']}


def test_held_out_sessions_or_pools_require_the_recorded_curator_id(tmp_path, monkeypatch):
    monkeypatch.setattr(common, 'E', str(tmp_path))
    for decide in (None, 'wrong', 'curator'):
        with pytest.raises(SystemExit):
            common.guard(pool='test', decide=decide)
    (tmp_path / 'deciding.json').write_text('{"curator": "run-42"}')
    with pytest.raises(SystemExit):
        common.guard(pool='test', decide='curator')
    common.guard(pool='test', decide='run-42')
    (tmp_path / 'replay').mkdir()
    (tmp_path / 'replay/manifest.json').write_text(json.dumps({'sessions': [
        {'session': 's1', 'side': 'dev'}, {'session': 's2', 'side': 'held-out'}]}))
    # The frozen manifest takes precedence over a hash split.
    common.guard(['s1'])
    with pytest.raises(SystemExit):
        common.guard(['s2'])
    common.guard(['s2'], decide='run-42')
    (tmp_path / 'labels/drafts').mkdir(parents=True)
    (tmp_path / 'labels/drafts/extra.json').write_text('[{"session": "s0", "agent": "claude"}]')
    common.guard(['s0'])
    with pytest.raises(SystemExit):
        common.guard(['s4'])          # an unknown session falls back to the hash


def test_the_command_helper_uses_clean_env_and_never_surfaces_process_output(monkeypatch):
    monkeypatch.setenv('PRIVATE_API_KEY', 'do-not-inherit')
    replies = [subprocess.CompletedProcess([], 0, 'ok', 'private-stderr'),
               subprocess.CompletedProcess([], 1, 'PRIVATE-OUTPUT', 'PRIVATE-ERROR'),
               subprocess.TimeoutExpired(['binary', 'PRIVATE-ARG'], 1, output='PRIVATE-OUTPUT')]

    def run(argv, **kwargs):
        assert not any('KEY' in k.upper() for k in kwargs['env'])
        assert kwargs['capture_output'] and kwargs['text']
        reply = replies.pop(0)
        if isinstance(reply, Exception):
            raise reply
        return reply

    monkeypatch.setattr(subprocess, 'run', run)
    assert common.command(['binary']) == 'ok'
    for _ in range(2):
        with pytest.raises(RuntimeError) as failure:
            common.command(['binary'])
        assert 'PRIVATE' not in str(failure.value)


def test_run_metadata_and_rewritten_output_remain_owner_only(tmp_path):
    binary = tmp_path / 'binary'
    binary.write_bytes(b'abc')
    metadata = common.record(str(binary), '/isolated/home', 40, 'off', {'glm-5.3': ['reported']})
    assert metadata == {'N': 40, 'machine': metadata['machine'],
                        'sha256': 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
                        'home': '/isolated/home', 'vector': 'off', 'models': {'glm-5.3': ['reported']}}
    assert metadata['machine']
    rows = tmp_path / 'results' / 'rows.jsonl'
    common.write_jsonl(str(rows), [metadata])
    rows.chmod(0o644)
    common.write_jsonl(str(rows), [metadata])
    assert rows.stat().st_mode & 0o777 == 0o600
    assert rows.parent.stat().st_mode & 0o777 == 0o700
    report = tmp_path / 'results' / 'report.json'
    common.keep_json(str(report), metadata)
    assert json.loads(report.read_text()) == metadata
    assert report.stat().st_mode & 0o777 == 0o600


def test_claude_answerer_is_isolated_pinned_and_fails_without_private_output(monkeypatch, capsys):
    monkeypatch.setenv('CLAUDECODE', 'owner-session')
    monkeypatch.setenv('PRIVATE_API_KEY', 'owner-key')
    replies = [subprocess.CompletedProcess([], 0, json.dumps({'result': '{"ok": true}',
                'modelUsage': {'claude-sonnet-5': {}}}), ''),
               subprocess.CompletedProcess([], 1, 'PRIVATE-OUTPUT', 'PRIVATE-ERROR'),
               subprocess.CompletedProcess([], 0, json.dumps({'result': 'PRIVATE-OUTPUT',
                'modelUsage': {'another-model': {}}}), ''),
               subprocess.CompletedProcess([], 0, 'PRIVATE-OUTPUT', ''),
               subprocess.TimeoutExpired(['claude'], 1, output='PRIVATE-OUTPUT')]

    def run(argv, **kwargs):
        assert kwargs['input'] == 'PRIVATE-PROMPT' and 'PRIVATE-PROMPT' not in argv
        assert kwargs['env']['OBOETE_SKIP'] == '1'
        assert 'CLAUDECODE' not in kwargs['env'] and 'PRIVATE_API_KEY' not in kwargs['env']
        assert argv[argv.index('--tools') + 1] == ''
        assert argv[argv.index('--setting-sources') + 1] == ''
        assert '--strict-mcp-config' in argv and '--no-session-persistence' in argv
        assert json.loads(argv[argv.index('--settings') + 1])['disableAllHooks']
        assert os.path.isdir(kwargs['cwd'])
        reply = replies.pop(0)
        if isinstance(reply, Exception):
            raise reply
        return reply

    monkeypatch.setattr(subprocess, 'run', run)
    assert common.claude_json('PRIVATE-PROMPT', 'claude-sonnet-5') == '{"ok": true}'
    for _ in range(4):
        with pytest.raises(RuntimeError) as failure:
            common.claude_json('PRIVATE-PROMPT', 'claude-sonnet-5')
        assert 'PRIVATE' not in str(failure.value)
    assert capsys.readouterr() == ('', '')
