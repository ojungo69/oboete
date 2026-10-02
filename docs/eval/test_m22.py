import json

import pytest

import m22


def test_d16_uses_the_slowdown_ratio_and_the_two_upper_quantiles():
    idle = [{'ms': 100, 'embeds': 1}] * 40
    written = [{'ms': 120, 'embeds': 1}] * 40
    latency = {'provider': 'workers-ai', 'model': '@cf/baai/bge-m3',
               'source': 'real', 'samples_ms': [300] * 40}
    result = m22.metrics(idle, written, latency)
    assert result['slowdown_p95'] == pytest.approx(0.2)
    assert result['slowdown_pass'] is True
    assert result['combined_p95_bound_ms'] == 420
    assert result['mcp_pass'] is True
    assert m22.metrics(idle, [{'ms': 121, 'embeds': 1}] * 40, latency)['slowdown_pass'] is False


def test_too_few_samples_or_a_missing_embed_cannot_set_a_metric():
    latency = {'provider': 'workers-ai', 'model': '@cf/baai/bge-m3',
               'source': 'real', 'samples_ms': [300] * 40}
    assert m22.metrics([{'ms': 1, 'embeds': 1}] * 29,
                       [{'ms': 1, 'embeds': 1}] * 40, latency)['counted'] is False
    with pytest.raises(ValueError, match='one embedding'):
        m22.metrics([{'ms': 1, 'embeds': 0}] * 40,
                    [{'ms': 1, 'embeds': 1}] * 40, latency)
    with pytest.raises(ValueError, match='latency'):
        m22.metrics([{'ms': 1, 'embeds': 1}] * 40,
                    [{'ms': 1, 'embeds': 1}] * 40, dict(latency, source='stub'))


def test_seeded_queries_and_warmup_are_identical_for_each_leg():
    questions = [{'qid': str(i), 'text': 'query ' + str(i), 'split': 'dev'} for i in range(110)]
    selected = m22.select_queries(questions)
    assert len(selected) == 100
    assert selected == m22.select_queries(list(reversed(questions)))
    seen = []
    counter = [0]
    class Client:
        def call(self, tool, args):
            assert tool == 'search' and args['all'] is True
            seen.append(args['query'])
            counter[0] += 1
            return {'content': [{'type': 'text', 'text': 'fenced result'}]}
    times = iter(range(200))
    report = m22.query_leg(Client(), selected, lambda: counter[0], clock=lambda: next(times))
    assert report['N'] == 90 and report['warmup'] == 10
    assert report['first']['qid'] == selected[0]['qid']
    assert report['timed'][0]['qid'] == selected[10]['qid']
    assert seen == [q['query'] for q in selected]
    with pytest.raises(ValueError, match='one embedding'):
        m22.query_leg(Client(), selected, lambda: 0, clock=lambda: 1)


def test_loopback_protocol_embeds_1024_coordinates_and_quotes_only_user_lines():
    import urllib.request
    def post(base, route, data):
        request = urllib.request.Request(base + route, json.dumps(data).encode(),
                                         {'Content-Type': 'application/json'})
        with urllib.request.urlopen(request, timeout=5) as response:
            return json.load(response)
    with m22.loopback(0.5) as server:
        body = post(server.url, '/embed', {'text': ['one', 'two'], 'truncate_inputs': True})
        assert len(body['result']['data']) == 2
        assert len(body['result']['data'][0]) == 1024
        assert sum(v * v for v in body['result']['data'][0]) == pytest.approx(1)
        prompt = ('=== RECORD abc ===\n## claude session x\nL1 [user] Keep the parser strict.\n'
                  'L2 [assistant] invent a claim\n## Kept claims\nL3 [user] not a current record\n'
                  '=== RECORD abc ===')
        answer = post(server.url, '/v1/chat/completions', {'messages': [{'role': 'user', 'content': prompt}],
                                                        'response_format': {'claims': []}})
        claims = json.loads(answer['choices'][0]['message']['content'])['claims']
        assert len(claims) == 1
        assert claims[0]['quote'] == 'Keep the parser strict.'
        assert claims[0]['line'] == 'L1' and claims[0]['speaker'] == 'user'
        assert server.embeds == 1


def test_build_cli_records_rates_first_and_refuses_year_without_disk_ok(tmp_path, monkeypatch):
    import sqlite3
    home = tmp_path / 'dev?#'
    home.mkdir()
    with sqlite3.connect(home / 'raw.db') as db:
        db.executescript("CREATE TABLE records(type TEXT, session TEXT); INSERT INTO records VALUES('event', 'already');")
    with sqlite3.connect(home / 'knowledge.db') as db:
        db.executescript('CREATE TABLE active(uid TEXT); INSERT INTO active VALUES("claim");')
    called = []
    monkeypatch.setattr(m22, 'construct', lambda *args, **kwargs: called.append((args, kwargs)) or {'home': 'fixture'})
    with pytest.raises(ValueError, match='disk-ok'):
        m22.main(['build', '--binary', '/unused', '--dev-home', str(home), '--days', '365'])
    assert not called
    result = m22.main(['build', '--binary', '/unused', '--dev-home', str(home)])
    assert result == {'home': 'fixture'}
    assert called[0][1]['observed']['claims_per_record'] == 1
    assert called[0][1]['observed']['records'] == 1


def test_owned_cleanup_cannot_delete_a_source_symlink_or_an_arbitrary_home(tmp_path, monkeypatch):
    import common
    monkeypatch.setattr(common, 'E', str(tmp_path / 'eval'))
    arbitrary = tmp_path / 'owner'
    arbitrary.mkdir()
    (arbitrary / 'keep').write_text('owner data')
    with pytest.raises(ValueError, match='owned'):
        m22.cleanup(arbitrary)
    root = m22.directory()
    root.mkdir(parents=True)
    link = root / 'scale-fake'
    link.symlink_to(arbitrary, target_is_directory=True)
    with pytest.raises(ValueError, match='owned'):
        m22.cleanup(link)
    assert (arbitrary / 'keep').read_text() == 'owner data'
    owned = m22.new_home()
    assert owned.is_dir()
    m22.cleanup(owned)
    assert not owned.exists()


def test_manifest_discovery_skips_held_out_before_read_and_refuses_duplicate_sessions(tmp_path):
    from datetime import datetime, timezone, timedelta
    now = datetime.now(timezone.utc)
    path = tmp_path / 'dev.jsonl'
    path.write_text(json.dumps({'cwd': '/work/repo', 'timestamp': now.isoformat()}) + '\n')
    manifest = {'sessions': [{'session': 'held', 'side': 'held-out'}, {'session': 'dev', 'side': 'dev'}]}
    found = [('claude', 'held', str(tmp_path / 'never-open')), ('claude', 'dev', str(path))]
    result = m22.source_sessions(found, manifest, [], now - timedelta(days=90))
    assert [s['session'] for s in result] == ['dev']
    assert m22.source_sessions(found, manifest, ['dev'], now - timedelta(days=90)) == []
    with pytest.raises(ValueError, match='Duplicate'):
        m22.source_sessions(found + [found[1]], manifest, [], now - timedelta(days=90))


def test_run_public_command_always_deletes_its_owned_home(tmp_path, monkeypatch):
    import common
    monkeypatch.setattr(common, 'E', str(tmp_path / 'eval'))
    home = m22.new_home()
    monkeypatch.setattr(m22, 'evaluate', lambda *args, **kwargs: (_ for _ in ()).throw(ValueError('fixture failure')))
    with pytest.raises(ValueError, match='fixture failure'):
        m22.main(['run', '--binary', '/unused', '--home', str(home), '--checkout', str(tmp_path),
                  '--workers-ai-latency', '/injected', '--lines', '/injected', '--wsl-dev', '/injected'])
    assert not home.exists()


def test_a_second_run_cannot_delete_the_first_runs_owned_home(tmp_path, monkeypatch):
    import common
    monkeypatch.setattr(common, 'E', str(tmp_path / 'eval'))
    home = m22.new_home()
    (home / 'm22-run.json').write_text('{}')
    with pytest.raises(ValueError, match='already running'):
        m22.main(['run', '--binary', '/unused', '--home', str(home), '--checkout', str(tmp_path),
                  '--workers-ai-latency', '/injected', '--lines', '/injected', '--wsl-dev', '/injected'])
    assert home.exists()
    m22.cleanup(home)


def test_d16_hook_comparison_uses_the_machine_ratio_and_keeps_read_attribution():
    scale = {'read': {'warm': {'session_start_ms': {'p95': 100}, 'prompt_ms': {'p95': 60},
                              'read_in_process_us': {'p95': 2000}},
                      'cold': {'session_start_ms': {'p95': 180}, 'prompt_ms': {'p95': 80},
                               'read_in_process_us': {'p95': 4000}}},
             'hook_spawn_ms': {'1KB': {'p95': 10}}}
    dev = {'line_ms': {'session_start': 50, 'prompt': 30}, 'in_process_read_p95_us': 1000}
    lines = {'line_ms': {'session_start': 200, 'prompt': 120}}
    result = m22.hook_comparison(scale, dev, lines)
    assert result['session_start']['estimated_slowest_p95_ms'] == 400
    assert result['session_start']['pass'] is False
    assert result['session_start']['read_share_ms'] == 90
    assert result['session_start']['in_process_read_ratio'] == 2
    assert result['prompt']['cold_estimated_slowest_p95_ms'] == 320


def test_construct_uses_copied_recorded_transcripts_and_preserves_the_dev_home(tmp_path, monkeypatch):
    import common, sqlite3, tomllib
    from datetime import datetime, timezone
    monkeypatch.setattr(common, 'E', str(tmp_path / 'eval'))
    monkeypatch.setattr(m22, 'DOCUMENTS', 2)  # Tiny injected corpus; no real scale input is opened.
    home = tmp_path / 'dev?#'
    home.mkdir()
    with sqlite3.connect(home / 'raw.db') as db:
        db.executescript("CREATE TABLE records(type TEXT, session TEXT); INSERT INTO records VALUES('event', 'already');")
    with sqlite3.connect(home / 'knowledge.db') as db:
        db.executescript('CREATE TABLE active(uid TEXT); INSERT INTO active VALUES("claim");')
    (home / 'config.toml').write_text('# owner config must remain unchanged\n')
    binary = tmp_path / 'fixture-binary'
    binary.write_text('offline fixture')
    now = datetime.now(timezone.utc)
    source = tmp_path / 'dev.jsonl'
    source.write_text(json.dumps({'cwd': '/work/repo', 'timestamp': now.isoformat()}) + '\n')
    m22.save(tmp_path / 'eval' / 'corpus-count.json', {'raw_rate': {'days': 90, 'failed_files': 0,
                                                                'events': 1, 'events_per_year': 4}})
    m22.save(tmp_path / 'eval' / 'replay' / 'manifest.json', {'sessions': [
        {'session': 'already', 'side': 'dev'}, {'session': 'dev', 'side': 'dev'}]})
    commands = []
    def execute(argv, **kwargs):
        commands.append(argv)
        assert kwargs['env']['OBOETE_NO_SPAWN'] == '1'
        assert not any('SECRET' in key or 'TOKEN' in key or 'KEY' in key for key in kwargs['env'])
        if argv[3] == 'transcript':
            assert str(tmp_path / 'eval' / 'm22') in argv[4]
            return json.dumps({'session': 'dev', 'ts': now.isoformat(), 'agent': 'claude',
                               'event': 'UserPromptSubmit', 'payload': {'session_id': 'dev', 'prompt': 'A fixture'}})
        copied = argv[2]
        if argv[3] == 'replay':
            with sqlite3.connect(str(copied) + '/raw.db') as db:
                for row in common.read_jsonl(argv[4]):
                    db.execute('INSERT INTO records VALUES(?, ?)', ('event', row['session']))
            return '{}'
        if argv[3] == 'import':
            assert argv[-1] == '--eval-store'
            return json.dumps({'observations': 2})
        assert argv[3:] == ['worker', '--idle-ms', '0']
        with open(str(copied) + '/config.toml', 'rb') as f:
            config = tomllib.load(f)
        assert config['embedding']['url'].startswith('http://127.0.0.1:')
        assert config['embedding']['daily_requests'] == 4294967295
        with sqlite3.connect(str(copied) + '/knowledge.db') as db:
            db.executescript('CREATE TABLE imported(uid TEXT); INSERT INTO imported VALUES("a"),("b");'
                             'CREATE TABLE vectors(uid TEXT); INSERT INTO vectors VALUES("v");'
                             'INSERT INTO active VALUES("new claim");')
        return ''
    monkeypatch.setattr(common, 'command', execute)
    result = m22.construct(str(binary), str(home), observed=m22.home_rates(home),
                           found=[('claude', 'dev', str(source))], now=now)
    assert result['actual']['records'] == 2 and result['replay']['events'] == 1
    assert result['scale_target_reached'] is True
    assert result['imported_documents'] == 2
    assert result['rates_recorded_before_build'] is True
    assert result['replay_sessions'][0]['files']['dev/claude/dev.jsonl'] == common.sha256_file(source)
    assert (home / 'config.toml').read_text() == '# owner config must remain unchanged\n'
    assert m22.home_rates(home)['records'] == 1
    assert not (m22.Path(result['home']) / 'transcripts').exists()
    m22.cleanup(result['home'])


def test_mcp_protocol_counts_one_real_loopback_request_per_call(tmp_path):
    import common, sys
    binary = tmp_path / 'stdio-stub'
    binary.write_text('#!' + sys.executable + '\n' + '''
import json, sys, urllib.request
url = sys.argv[2] + '/embed'
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    if request['method'] == 'initialize':
        result = {}
    else:
        args = request['params']['arguments']
        payload = json.dumps({'text': [args['query']], 'truncate_inputs': True}).encode()
        with urllib.request.urlopen(urllib.request.Request(url, payload), timeout=5) as response:
            vector = json.load(response)['result']['data'][0]
        assert len(vector) == 1024
        result = {'content': [{'type': 'text', 'text': 'a fenced fixture hit'}]}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
''')
    binary.chmod(0o700)
    questions = [{'qid': str(i), 'query': 'query ' + str(i)} for i in range(40)]
    with m22.loopback(0) as server:
        server.query_texts = {q['query'] for q in questions}
        with common.Mcp(str(binary), server.url, str(tmp_path)) as client:
            result = m22.query_leg(client, questions, lambda: server.query_embeds)
        assert result['N'] == 30
        assert server.embeds == 40 and server.query_embeds == 40


def test_writer_spawns_hooks_and_stops_only_its_worker_with_no_secret_env(tmp_path, monkeypatch):
    import os, sys, time
    binary = tmp_path / 'hook-stub'
    binary.write_text('#!' + sys.executable + '\n' + '''
import json, os, sys, time
assert not any(s in k.upper() for k in os.environ for s in ('KEY','TOKEN','SECRET','PASSWORD'))
assert os.environ['OBOETE_NO_SPAWN'] == '1'
if sys.argv[3] == 'worker':
    time.sleep(30)
else:
    assert sys.argv[3:] == ['hook', 'claude', 'PostToolUse']
    row = json.load(sys.stdin)
    assert row['tool_name'] == 'Read' and row['session_id'].startswith('m22-writer-')
''')
    binary.chmod(0o700)
    monkeypatch.setenv('FIXTURE_SECRET', 'must not reach a child')
    with m22.writer(str(binary), tmp_path, tmp_path, 20) as report:
        time.sleep(0.15)
    assert report['events'] >= 2
    assert report['failed'] is False
    assert report['observed_events_per_second'] > 0
    assert os.environ['FIXTURE_SECRET'] == 'must not reach a child'


def test_existing_replay_and_non_loopback_endpoint_are_refused_before_writing(tmp_path):
    import sqlite3
    from types import SimpleNamespace
    with sqlite3.connect(tmp_path / 'raw.db') as db:
        db.executescript("CREATE TABLE records(session TEXT); INSERT INTO records VALUES('already');")
    with pytest.raises(ValueError, match='already recorded'):
        m22.replay_parts('/unused', tmp_path, [{'session': 'already', 'payload': {}}])
    with pytest.raises(ValueError, match='loopback'):
        m22.configure(tmp_path, SimpleNamespace(url='https://api.example.org'))
    assert not (tmp_path / 'config.toml').exists()
