import copy, json, sqlite3, subprocess, sys
from pathlib import Path

import pytest


def replay_report(start=10.0, prompt=20.0, spawn=3.0, cold=30.0):
    def stats(p95):
        return dict(n=300, p50=p95 / 2, p95=p95, p99=p95, max=p95)

    def arm(s, p):
        return dict(session_start_ms=stats(s), prompt_ms=stats(p),
                    session_start_printed_bytes=100, prompt_printed_bytes=80,
                    read_in_process_us=dict(p50=13, p95=57, max=91), read_chars=17)

    return dict(hook_spawn_ms={'1KB': stats(spawn)},
                read=dict(per_prompt=True, warm=arm(start, prompt), cold=arm(cold, 40.0),
                          drain=dict(ms=12.3, backlog=1920)))


def test_the_worst_run_is_chosen_for_each_hook_without_rounding():
    import hooks
    reports = [replay_report(19.125, 11.0, 4.5, 28.25),
               replay_report(12.0, 25.875, 6.0, 52.125), replay_report(13.0, 14.0)]
    rows = [hooks.measurements(r) for r in reports]
    out = hooks.summary(rows, 3)
    assert out == dict(counted_runs=3, line_ms=dict(session_start=19.125, prompt=25.875),
                       cold_session_start_ms=52.125)
    assert rows[0]['read_share_ms'] == dict(session_start=14.625, prompt=6.5)
    assert rows[1]['read_share_ms'] == dict(session_start=6.0, prompt=19.875)
    assert rows[0]['report']['read']['warm']['read_in_process_us']['p95'] == 57
    assert rows[0]['report']['read']['drain'] == dict(ms=12.3, backlog=1920)
    assert hooks.summary(rows[:2], 3)['line_ms'] == dict(session_start=None, prompt=None)
    assert hooks.measurements(replay_report(1, 2, 3))['read_share_ms'] == dict(session_start=-2, prompt=-1)


@pytest.mark.parametrize('field', ['session_start_printed_bytes', 'prompt_printed_bytes'])
def test_zero_warm_printed_bytes_sets_no_line(field):
    import hooks
    bad = replay_report(900, 900)
    bad['read']['warm'][field] = 0
    row = hooks.measurements(bad)
    assert not row['counted'] and any(field in reason for reason in row['reasons'])
    out = hooks.summary([hooks.measurements(replay_report()), row], 2)
    assert out == dict(counted_runs=1, line_ms=dict(session_start=None, prompt=None),
                       cold_session_start_ms=None)


def test_per_prompt_and_all_sample_sizes_are_required():
    import hooks
    bad = replay_report()
    bad['read']['per_prompt'] = False
    assert not hooks.measurements(bad)['counted']
    bad['read']['per_prompt'] = 'true'
    assert not hooks.measurements(bad)['counted']
    for arm in ('cold', 'warm'):
        for hook in ('session_start_ms', 'prompt_ms'):
            bad = replay_report()
            bad['read'][arm][hook]['n'] = 310
            row = hooks.measurements(bad)
            assert not row['counted'] and any('300' in reason for reason in row['reasons'])
    bad = replay_report()
    bad['hook_spawn_ms']['1KB']['n'] = 299
    assert not hooks.measurements(bad)['counted']


def test_drain_p95_uses_replays_upper_index_and_keeps_precision():
    import hooks
    assert hooks.p95(list(range(1, 20)) + [31.125]) == 31.125
    assert hooks.p95([9, 1, 2]) == 9
    assert hooks.p95([0.123456789]) == 0.123456789


FAKE = '''import json, os, sqlite3, sys, tomllib
from pathlib import Path
home = Path(sys.argv[2])
command = sys.argv[3]
config = tomllib.loads((home / 'config.toml').read_text())
assert config['summary']['curate'] is False and config['summary']['shrink'] is False
assert config['embedding']['provider'] == 'none' and not config.get('providers')
assert config['inject']['per_prompt'] is (command == 'replay')
assert not any(s in k.upper() for k in os.environ for s in ('TOKEN', 'KEY', 'SECRET', 'PASSWORD'))
assert not any(k in os.environ for k in ('CLAUDECODE', 'OBOETE_SKIP', 'OBOETE_REPLAY', 'OBOETE_FIELD_CAP'))
assert ('OBOETE_NO_SPAWN' not in os.environ) if command == 'replay' else os.environ.get('OBOETE_NO_SPAWN') == '1'
assert home.stat().st_mode & 0o777 == 0o700
for p in home.rglob('*'):
    assert p.stat().st_mode & 0o777 == (0o700 if p.is_dir() else 0o600)
control = json.loads((home / 'control.json').read_text())
log = home.parent / 'commands.jsonl'
previous = ([json.loads(line) for line in log.open()]
            if log.exists() and command != 'hook' else [])
payload = json.load(sys.stdin) if command == 'hook' else None
with log.open('a') as f:
    f.write(json.dumps(dict(argv=sys.argv[1:], home=str(home), command=command, payload=payload)) + '\\n')
homes = {r['home'] for r in previous}
if 'copies_limit' in control and str(home) not in homes and len(homes) >= control['copies_limit']:
    sys.exit(1)
if command == 'replay':
    assert not (home / 'copy-only').exists()
    (home / 'copy-only').write_text('changed only in the copy')
    index = sum(r['command'] == 'replay' for r in previous)
    if index in control.get('fail_runs', []):
        print('PRIVATE-STDERR', file=sys.stderr)
        print('PRIVATE-STDOUT')
        sys.exit(1)
    rows = [json.loads(line) for line in Path(sys.argv[4]).open() if line.strip()]
    assert len(rows) == 1 and rows[0]['text'] == 'one\\u2028record'
    print(json.dumps(control['reports'][index]))
else:
    raw = sqlite3.connect(home / 'raw.db')
    k = sqlite3.connect(home / 'knowledge.db')
    if command == 'hook':
        assert sys.argv[4:] == ['claude', 'PostToolUse']
        assert payload['tool_name'] == 'Bash' and payload['tool_input'] == {'command': 'true'}
        assert payload['hook_event_name'] == 'PostToolUse' and payload['tool_response'] == 'ok'
        if not control.get('drop_hook'):
            seq = raw.execute("SELECT COALESCE(MAX(seq),0)+1 FROM records WHERE device='local'").fetchone()[0]
            raw.execute("INSERT INTO records VALUES('local', ?, ?, 'event')", (seq, payload['session_id']))
    else:
        assert sys.argv[4:] == ['--idle-ms', '0']
        # Real raw::open gives a copied store a new device, without relabelling old records.
        raw.execute("UPDATE meta SET value='local' WHERE key='device_id'")
        count = sum(r['command'] == 'worker' and r['home'] == str(home) for r in previous)
        if not (control.get('stuck_worker') and count > 0):
            top = raw.execute("SELECT COALESCE(MAX(seq),0) FROM records WHERE device='local'").fetchone()[0]
            for consumer in ('rescan', 'fts', 'anchors', 'manifest', 'gaps', 'compress'):
                k.execute('INSERT OR REPLACE INTO checkpoints VALUES(?,?,?)', (consumer, 'local', top))
            for device, top in raw.execute('SELECT device,MAX(op_seq) FROM ops GROUP BY device'):
                for consumer in ('claims', 'cards', 'turns', 'imported'):
                    k.execute('INSERT OR REPLACE INTO op_checkpoints VALUES(?,?,?)', (consumer, device, top))
    raw.commit()
    k.commit()
    raw.close()
    k.close()
'''


@pytest.fixture
def fixture_home(tmp_path, monkeypatch):
    import common, hooks
    monkeypatch.setattr(hooks.platform, 'node', lambda: 'test-host')
    source = tmp_path / 'dev-home'
    source.mkdir()
    (source / 'config.toml').write_text('[summary]\ncurate = true\n[embedding]\nprovider = "workers-ai"\n')
    with sqlite3.connect(source / 'raw.db') as db:
        db.executescript("""CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT);
            INSERT INTO meta VALUES('device_id', 'old');
            CREATE TABLE records(device TEXT, seq INTEGER, session TEXT, type TEXT);
            INSERT INTO records VALUES('old', 1, 'dev-a', 'event'), ('old', 2, 'dev-a', 'event');
            CREATE TABLE ops(device TEXT, op_seq INTEGER);
            INSERT INTO ops VALUES('remote', 3);""")
    with sqlite3.connect(source / 'knowledge.db') as db:
        for table in ('checkpoints', 'op_checkpoints'):
            db.execute(f'CREATE TABLE {table}(consumer TEXT, device TEXT, seq INTEGER, PRIMARY KEY(consumer,device))')
    (source / 'control.json').write_text(json.dumps({'reports': [replay_report()] * 3}))
    (source / 'models').mkdir()
    (source / 'models' / 'kept-vector').write_text('embedded beforehand')
    root = Path(common.E)
    (root / 'replay').mkdir(parents=True)
    (root / 'replay' / 'manifest.json').write_text(json.dumps({'sessions': [
        {'session': 'dev-a', 'side': 'dev'}, {'session': 'sealed', 'side': 'held-out'}]}))
    (root / 'replay' / 'events-1000.jsonl').write_text('{"text":"one\u2028record"}\n', encoding='utf-8')
    binary = tmp_path / 'oboete'
    binary.write_text(f'#!{sys.executable}\n' + FAKE)
    binary.chmod(0o700)
    checkout = tmp_path / 'checkout'
    checkout.mkdir()
    (checkout / '.git').mkdir()
    # Assert the environment BEFORE conftest's subprocess filter could hide a regression.
    allowed = subprocess.run

    def checked(argv, **kwargs):
        env = kwargs['env']
        assert not any(s in key.upper() for key in env for s in ('TOKEN', 'KEY', 'SECRET', 'PASSWORD'))
        assert 'CLAUDECODE' not in env
        return allowed(argv, **kwargs)

    monkeypatch.setattr(subprocess, 'run', checked)
    for key in ('PANEL_API_KEY', 'OWNER_TOKEN', 'OWNER_SECRET', 'OWNER_PASSWORD', 'CLAUDECODE',
                'OBOETE_SKIP', 'OBOETE_NO_SPAWN', 'OBOETE_REPLAY', 'OBOETE_FIELD_CAP'):
        monkeypatch.setenv(key, 'must-not-be-inherited')
    monkeypatch.setattr(common, 'Calls', lambda: pytest.fail('Timing must not ask a model'))
    return binary, source, checkout


def snapshot(path):
    return {str(p.relative_to(path)): (p.read_bytes(), p.stat().st_mode & 0o777)
            for p in path.rglob('*') if p.is_file()}


def control(home, **values):
    path = home / 'control.json'
    data = json.loads(path.read_text())
    path.write_text(json.dumps(data | values))


def test_line_cli_uses_exact_command_private_copies_and_metadata(fixture_home, capsys):
    import common, hooks
    binary, home, checkout = fixture_home
    original = snapshot(home)
    hooks.main(['line', '--binary', str(binary), '--dev-home', str(home), '--checkout', str(checkout),
                '--machine', 'windows-gnu'])
    out = json.loads(capsys.readouterr().out)
    path = Path(common.E, 'hooks', 'line-windows-gnu.json')
    assert json.loads(path.read_text()) == out
    assert out['counted_runs'] == out['requested_runs'] == 3 and out['warmup'] == 10
    rows = common.read_jsonl(Path(common.E, 'hooks', 'commands.jsonl'))
    assert len(rows) == 3 and len({r['home'] for r in rows}) == 3
    for row, metadata in zip(rows, out['runs']):
        copied = row['home']
        assert row['argv'] == ['--home', copied, 'replay', str(Path(common.E, 'replay/events-1000.jsonl')),
                               '--repo-root', str(checkout), '--read-sample', '300', '--read-warmup', '10',
                               '--spawn-sample', '300', '--sizes', '1']
        assert metadata['N'] == 300 and metadata['machine'] == 'test-host'
        assert metadata['sha256'] == common.sha256_file(binary)
        assert metadata['models'] == {} and metadata['vector'] == 'off' and metadata['home'] == copied
        assert Path(copied).parent == path.parent and not Path(copied).exists()
    assert snapshot(home) == original
    assert path.stat().st_mode & 0o777 == 0o600 and path.parent.stat().st_mode & 0o777 == 0o700
    with pytest.raises(ValueError, match='already'):
        hooks.line(binary, home, checkout, 'windows-gnu')
    assert json.loads(path.read_text()) == out


def test_line_keeps_unrelated_source_settings_in_each_worker_copy(fixture_home):
    import hooks
    binary, home, checkout = fixture_home
    (home / 'config.toml').write_text('''[summary]
curate = true
shrink = true
language = "ja"
window_tokens = 1717
[embedding]
provider = "workers-ai"
[inject]
per_prompt = false
session_start_chars = 1234
per_prompt_chars = 777
correction_chars = 555
[redaction]
allowlist = ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
[capture]
store_prompts = false
''')
    script = binary.read_text()
    binary.write_text(script.replace("config = tomllib.loads((home / 'config.toml').read_text())\n",
        "config = tomllib.loads((home / 'config.toml').read_text())\n"
        "assert config['summary']['language'] == 'ja' and config['summary']['window_tokens'] == 1717\n"
        "assert config['inject']['session_start_chars'] == 1234\n"
        "assert config['inject']['per_prompt_chars'] == 777 and config['inject']['correction_chars'] == 555\n"
        "assert config['redaction']['allowlist'] == ['a' * 64]\n"
        "assert config['capture']['store_prompts'] is False\n"))
    out = hooks.line(binary, home, checkout)
    assert out['counted_runs'] == 3
    assert out['line_ms'] == dict(session_start=10.0, prompt=20.0)


def test_failed_replay_is_recorded_and_copies_are_deleted(fixture_home, capsys):
    import common, hooks
    binary, home, checkout = fixture_home
    control(home, fail_runs=[1])
    out = hooks.line(binary, home, checkout)
    assert out['counted_runs'] == 2 and out['line_ms'] == dict(session_start=None, prompt=None)
    assert not out['runs'][1]['counted'] and out['runs'][1]['reasons']
    assert all(not Path(row['home']).exists() for row in out['runs'])
    assert 'PRIVATE-' not in json.dumps(out) and capsys.readouterr() == ('', '')
    assert not list(Path(common.E, 'hooks').glob('line-*/'))


def test_a_sealed_home_is_refused_before_any_copy(fixture_home, monkeypatch):
    import common, hooks
    binary, home, checkout = fixture_home
    with sqlite3.connect(home / 'raw.db') as db:
        db.execute("UPDATE records SET session='sealed'")
    monkeypatch.setattr(hooks.shutil, 'copytree',
                        lambda *args, **kwargs: pytest.fail('held-out source was copied'))
    with pytest.raises(SystemExit, match='Held-out'):
        hooks.line(binary, home, checkout)
    assert not Path(common.E, 'hooks', 'commands.jsonl').exists()
    assert not list(Path(common.E, 'hooks').glob('line-*/'))


def test_an_interrupted_run_removes_its_copy_and_leaves_no_line(fixture_home, monkeypatch):
    import common, hooks
    binary, home, checkout = fixture_home

    def interrupted(*args, **kwargs):
        raise KeyboardInterrupt

    monkeypatch.setattr(common, 'command', interrupted)
    with pytest.raises(KeyboardInterrupt):
        hooks.line(binary, home, checkout)
    path = Path(common.E, 'hooks', 'line-test-host.json')
    assert json.loads(path.read_text())['line_ms'] == dict(session_start=None, prompt=None)
    assert not list(path.parent.glob('line-*/'))


def test_combine_uses_each_hooks_slowest_machine_and_refuses_a_partial_line(tmp_path, capsys):
    import common, hooks
    paths = []
    for machine, start, prompt in [('wsl', 11.125, 30.875), ('imac', 23.375, 19.125)]:
        path = tmp_path / f'line-{machine}.json'
        row = dict(command='line', machine=machine, machine_label=machine, requested_runs=3,
                   runs=[hooks.measurements(replay_report(start, prompt)) for _ in range(3)])
        common.keep_json(path, row)
        paths.append(str(path))
    hooks.main(['combine', *paths])
    out = json.loads(capsys.readouterr().out)
    assert out['complete'] and out['line_ms'] == dict(session_start=23.375, prompt=30.875)
    row['runs'].pop()
    common.keep_json(paths[-1], row)
    assert hooks.combine(paths)['line_ms'] == dict(session_start=None, prompt=None)


def test_one_run_cannot_publish_a_d15_line(fixture_home, tmp_path, capsys):
    import common, hooks
    binary, home, checkout = fixture_home
    with pytest.raises(SystemExit) as error:
        hooks.main(['line', '--binary', str(binary), '--dev-home', str(home),
                    '--checkout', str(checkout), '--runs', '1'])
    assert error.value.code == 1
    assert 'Hook evaluation failed' in capsys.readouterr().err
    assert not Path(common.E, 'hooks', 'line-test-host.json').exists()
    path = tmp_path / 'line-one.json'
    common.keep_json(path, dict(machine='test-host', machine_label='test-host',
                                requested_runs=1, runs=[hooks.measurements(replay_report())]))
    combined = hooks.combine([path])
    assert combined['complete'] is False
    assert combined['line_ms'] == dict(session_start=None, prompt=None)


def test_paths_and_run_count_are_checked_before_writes(fixture_home):
    import common, hooks
    binary, home, checkout = fixture_home
    for name in ('../outside', 'a/b', '', '..'):
        with pytest.raises(ValueError):
            hooks.line(binary, home, checkout, machine=name)
    for runs in (0, -1, 1, 2, 4):
        with pytest.raises(ValueError):
            hooks.line(binary, home, checkout, runs=runs)
    with pytest.raises(ValueError, match='outside'):
        hooks.line(binary, Path(common.E), checkout)
    assert not Path(common.E, 'hooks').exists()


def test_relative_eval_paths_and_sqlite_uri_characters_work(fixture_home, monkeypatch):
    import common, hooks
    binary, home, checkout = fixture_home
    moved = Path(common.E).with_name('eval # run')
    Path(common.E).rename(moved)
    monkeypatch.chdir(moved.parent)
    monkeypatch.setattr(common, 'E', moved.name)
    out = hooks.line(binary, home, checkout)
    assert out['counted_runs'] == 3 and out['line_ms'] == dict(session_start=10.0, prompt=20.0)
    assert Path(out['fixture']).is_absolute()
    assert Path(out['runs'][0]['home']).is_absolute()
    with sqlite3.connect(home / 'raw.db') as db:
        db.execute("UPDATE records SET session='sealed'")
    with pytest.raises(SystemExit, match='Held-out'):
        hooks.line(binary, home, checkout, machine='sealed')


def test_drain_times_only_the_worker_with_exact_backlogs_and_same_machine_cold_line(fixture_home, monkeypatch):
    import common, hooks
    binary, home, checkout = fixture_home
    control(home, reports=[replay_report(19.125, 21.125, cold=28.25),
                           replay_report(12.0, 25.875, cold=52.125), replay_report()])
    hooks.line(binary, home, checkout, 'windows-gnu')
    original = snapshot(home)
    clock, workers = [0], {}
    durations = [1.25, *range(1, 20), 31.125, 200.125, 2000.875]
    pending_durations = iter(durations)
    command = common.command

    def timed(argv, **kwargs):
        value = command(argv, **kwargs)
        copied = argv[2]
        if argv[3] == 'worker':
            workers[copied] = workers.get(copied, 0) + 1
            # Baseline work and hook spawns must never enter the timed samples.
            milliseconds = 9000 if workers[copied] == 1 else next(pending_durations)
        else:
            milliseconds = 100
        clock[0] += int(milliseconds * 1_000_000)
        return value

    monkeypatch.setattr(common, 'command', timed)
    monkeypatch.setattr(hooks.time, 'perf_counter_ns', lambda: clock[0])
    out = hooks.drain(binary, home)
    assert out['N'] == 20 and len(out['runs']) == 23
    assert [r['ms'] for r in out['runs']] == durations
    assert [(p['backlog'], p['N'], p['counted'], p['p95_ms']) for p in out['slope']] == [
        (1, 1, 1, 1.25), (20, 20, 20, 31.125), (200, 1, 1, 200.125), (2000, 1, 1, 2000.875)]
    assert out['drain_p95_ms'] == 31.125 and out['session_start_ms'] == 83.25
    assert out['line']['cold_session_start_ms'] == 52.125 and out['line_issue'] is None
    assert 'worker --idle-ms 0' in out['method'] and 'startup' in out['method'] and 'backup' in out['method']
    assert 'worker::drained' in out['method']
    commands = common.read_jsonl(Path(common.E, 'hooks/commands.jsonl'))
    hooks_run = [r for r in commands if r['command'] == 'hook']
    assert len(hooks_run) == 1 + 20 * 20 + 200 + 2000
    assert all(r['payload']['cwd'] == str(checkout) for r in hooks_run)
    for row in out['runs']:
        copied = row['home']
        spawned = [r for r in commands if r['home'] == copied]
        assert [r['command'] for r in spawned] == ['worker'] + ['hook'] * row['N'] + ['worker']
        assert row['observed'] == dict(records=row['N'], backlog=row['N'])
        assert row['machine'] == 'test-host' and row['sha256'] == common.sha256_file(binary)
        assert row['models'] == {} and row['vector'] == 'off'
        assert not Path(copied).exists()
    assert len(workers) == 23 and set(workers.values()) == {2}
    assert snapshot(home) == original
    path = Path(common.E, 'hooks', 'drain-windows-gnu.json')
    assert json.loads(path.read_text()) == out and path.stat().st_mode & 0o777 == 0o600


@pytest.mark.parametrize('failure', ['drop_hook', 'stuck_worker'])
def test_fail_open_hooks_or_undrained_workers_never_become_drain_samples(fixture_home, monkeypatch, failure):
    import common, hooks
    binary, home, _ = fixture_home
    control(home, **{failure: True, 'copies_limit': 1})
    times = iter([0, 1_000_000])
    monkeypatch.setattr(hooks.time, 'perf_counter_ns', lambda: next(times))
    out = hooks.drain(binary, home)
    assert len(out['runs']) == 23 and not any(r['counted'] for r in out['runs'])
    assert all(r['reason'] and not Path(r['home']).exists() for r in out['runs'])
    assert out['drain_p95_ms'] is None and out['session_start_ms'] is None
    assert all(p['p95_ms'] is None for p in out['slope'])
    assert out['line'] is None and out['line_issue']
    commands = common.read_jsonl(Path(common.E, 'hooks/commands.jsonl'))
    first = [r['command'] for r in commands if r['home'] == commands[0]['home']]
    assert first == ['worker', 'hook'] + (['worker'] if failure == 'stuck_worker' else [])


def test_fewer_than_twenty_successful_drains_give_no_p95(fixture_home, monkeypatch):
    import hooks
    binary, home, _ = fixture_home
    control(home, copies_limit=2)
    times = iter([0, 1_000_000, 5_000_000, 7_000_000])
    monkeypatch.setattr(hooks.time, 'perf_counter_ns', lambda: next(times))
    out = hooks.drain(binary, home)
    assert out['slope'][0]['p95_ms'] == 1.0
    assert out['slope'][1] == dict(backlog=20, N=20, counted=1, ms=[2.0], p95_ms=None)
    assert out['drain_p95_ms'] is None and out['session_start_ms'] is None


def test_backlog_checks_every_consumer_and_remote_ops(fixture_home):
    import hooks
    _, home, _ = fixture_home
    assert hooks.backlog(home) == dict(records=2, backlog=3)
    with sqlite3.connect(home / 'knowledge.db') as db:
        for consumer in ('rescan', 'fts', 'anchors', 'manifest', 'gaps', 'compress'):
            db.execute('INSERT INTO checkpoints VALUES(?,?,?)', (consumer, 'old', 2))
        for consumer in ('claims', 'cards', 'turns', 'imported'):
            db.execute('INSERT INTO op_checkpoints VALUES(?,?,?)', (consumer, 'remote', 3))
    assert hooks.backlog(home)['backlog'] == 0
    for table, consumers, top in [
        ('checkpoints', ('rescan', 'fts', 'anchors', 'manifest', 'gaps', 'compress'), 2),
        ('op_checkpoints', ('claims', 'cards', 'turns', 'imported'), 3),
    ]:
        for consumer in consumers:
            with sqlite3.connect(home / 'knowledge.db') as db:
                db.execute(f'UPDATE {table} SET seq=0 WHERE consumer=?', (consumer,))
            assert hooks.backlog(home)['backlog'] == top
            with sqlite3.connect(home / 'knowledge.db') as db:
                db.execute(f'UPDATE {table} SET seq=? WHERE consumer=?', (top, consumer))


def test_cold_line_must_match_host_binary_home_and_all_requested_runs(fixture_home):
    import common, hooks
    binary, home, checkout = fixture_home
    out = hooks.line(binary, home, checkout)
    matched, issue = hooks.matching_line(out)
    assert issue is None and matched['cold_session_start_ms'] == 30
    for field in ('machine', 'sha256', 'home'):
        assert hooks.matching_line(out | {field: 'other'})[0] is None
    path = Path(common.E, 'hooks', 'line-test-host.json')
    partial = copy.deepcopy(out)
    partial['runs'].pop()
    common.keep_json(path, partial)
    assert hooks.matching_line(out)[0] is None
    common.keep_json(path, out)
    common.keep_json(path.with_name('line-another.json'), out)
    assert hooks.matching_line(out)[0] is None
