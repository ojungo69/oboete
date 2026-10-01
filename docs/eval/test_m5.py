import copy, importlib, json, os, re, sqlite3, subprocess, sys
from pathlib import Path

import pytest

REAL_RUN, REAL_POPEN = subprocess.run, subprocess.Popen

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))


def test_a_cut_is_seeded_and_thirty_minutes_in():
    m5 = importlib.import_module('m5')
    times = ['00:00:00', '00:20:00', '00:29:59', '00:30:00',
             '00:45:00', '01:00:00', '02:03:00']
    events = [{'ts': f'2026-09-30T{ts}Z'} for ts in times]
    # Independently worked SHA-256 draw: s1 chooses candidate 2/4, s2 chooses 0/4.
    assert m5.choose_cut('s1', events) == 5
    assert m5.choose_cut('s2', events) == 3
    assert m5.choose_cut('s1', events) == 5
    assert m5.choose_cut('s1', events[:3]) is None
    assert m5.choose_cut('s1', [events[0], events[3], events[2]]) is None
    assert m5.choose_cut('s1', []) is None


GRADERS = ('gpt-oss-120b', 'deepseek-v4-pro', 'glm-5.3')


def shown(kind, tier, yes):
    return {'session': 's1', 'kind': kind, 'tier': tier,
            'votes': {j: {'shown_open': yes} for j in GRADERS}}


def test_open_recall_closed_shown_open_and_the_none_tier_score_apart():
    m5 = importlib.import_module('m5')
    rows = ([shown('open', 'curated', i < 8) for i in range(10)]
            + [shown('open', 'none', i < 5) for i in range(10)]
            + [shown('closed', 'curated', i == 0) for i in range(10)]
            + [shown('closed', 'none', i < 7) for i in range(10)]
            + [shown('next', 'curated', False), shown('next', 'none', True)])
    out = m5.metrics(rows)
    assert out['complete'] and out['pass'] and out['n'] == 1
    assert out['tiers']['curated']['open'] == {'shown': 8, 'n': 10, 'rate': 0.8, 'pass': True}
    assert out['tiers']['none']['open'] == {'shown': 5, 'n': 10, 'rate': 0.5, 'pass': True}
    assert out['tiers']['curated']['closed'] == {'shown': 1, 'n': 10, 'rate': 0.1, 'pass': True}
    assert out['tiers']['none']['closed'] == {'shown': 7, 'n': 10, 'rate': 0.7}
    assert out['tiers']['curated']['next'] == {'shown': 0, 'n': 1, 'rate': 0.0}
    assert out['tiers']['none']['next'] == {'shown': 1, 'n': 1, 'rate': 1.0}
    assert all(v['n'] == 42 and v['agreement'] == 1.0 for v in out['agreement'].values())
    for index, yes in ((7, False), (14, False), (21, True)):
        changed = copy.deepcopy(rows)
        changed[index]['votes'] = {j: {'shown_open': yes} for j in GRADERS}
        assert not m5.metrics(changed)['pass']
    # Two identical grades never become a score while the third grader has no answer.
    pending = copy.deepcopy(rows)
    pending[0]['votes'][GRADERS[2]] = None
    assert m5.metrics(pending) == {'complete': False, 'pending': 1}


def test_the_prompts_are_the_protocols():
    m5 = importlib.import_module('m5')
    protocol = (Path(__file__).parents[1] / 'milestone-4.md').read_text()
    for name in ('LABEL', 'CHECK_LABELS', 'SHOWN'):
        expected = re.search(rf'^{name}:\n\n```text\n(.*?)```', protocol, re.S | re.M).group(1)
        assert getattr(m5, name) == expected


@pytest.fixture
def harness(monkeypatch, tmp_path):
    import calib, common, m3
    m5 = importlib.import_module('m5')
    evaluation = Path(common.E)
    fixtures = evaluation / 'm3' / 'fixtures'
    fixtures.mkdir(parents=True)
    monkeypatch.setattr(m3, 'E', common.E)
    monkeypatch.setattr(m3, 'M', str(evaluation / 'm3'))
    (evaluation / 'replay').mkdir()
    (evaluation / 'replay' / 'manifest.json').write_text(json.dumps({'sessions': [
        {'session': 's1', 'side': 'dev'}, {'session': 'held-out', 'side': 'held-out'}]}))
    binary = tmp_path / 'binary'
    binary.write_bytes(b'offline-binary-fixture')
    recorded = tmp_path / 'recorded-checkout'
    recorded.mkdir()
    repo = tmp_path / 'raw-repo'
    repo.mkdir()
    cli = tmp_path / 'bin' / 'claude'
    cli.parent.mkdir()
    cli.write_text(f'#!{sys.executable}\nraise SystemExit(1)\n')
    cli.chmod(0o700)
    monkeypatch.setenv('PATH', str(cli.parent) + os.pathsep + os.environ.get('PATH', ''))
    events = [{'agent': 'claude', 'session': 's1', 'event': 'SessionStart',
               'ts': '2026-09-30T00:00:00Z', 'payload': {'cwd': str(recorded)}}]
    for i in range(82):
        events.append({'agent': 'claude', 'session': 's1', 'event': 'UserPromptSubmit',
                       'ts': '2026-09-30T00:01:00Z', 'payload': {'cwd': str(recorded),
                           'prompt': f'PROMPT-{i:02d}-SENSITIVE-' + 'x' * 2200}})
    events.extend([
        {'agent': 'claude', 'session': 's1', 'event': 'PostToolUse',
         'ts': '2026-09-30T00:15:00Z', 'payload': {'tool_response': 'TOOL-SENTINEL'}},
        {'agent': 'claude', 'session': 's1', 'event': 'Stop',
         'ts': '2026-09-30T00:20:00Z', 'payload': {'last_assistant_message': 'REPLY-SENSITIVE'}}])
    for ts in ('00:30:00', '00:45:00', '01:00:00', '02:03:00'):
        events.append({'agent': 'claude', 'session': 's1', 'event': 'UserPromptSubmit',
                       'ts': f'2026-09-30T{ts}Z',
                       'payload': {'cwd': str(recorded), 'prompt': f'AT-{ts}-SENSITIVE'}})
    # The blank line ensures the model sees original fixture line numbers.
    path = fixtures / 's1.jsonl'
    path.write_text(json.dumps(events[0]) + '\n\n' + ''.join(json.dumps(e) + '\n' for e in events[1:]))
    state = {'repo': str(repo), 'calls': [], 'processes': [], 'replayed': [], 'fail_grade': False,
             'first_turn': '[9]', 'session': 's1', 'bad_labels': 0, 'partial_worker': False,
             'no_models': False}

    def process(argv, **kw):
        state['processes'].append((argv, kw))
        assert not any(any(secret in k.upper() for secret in ('TOKEN', 'KEY', 'SECRET', 'PASSWORD'))
                       for k in kw['env'])
        if argv[1:] == ['gate'] or argv[-1] == 'gate':
            return subprocess.CompletedProcess(argv, 0, kw['input'].replace('SENSITIVE', '[redacted]'), '')
        home = Path(argv[argv.index('--home') + 1])
        if 'replay' in argv:
            part = Path(argv[argv.index('replay') + 1])
            prefix = [json.loads(line) for line in part.read_text().splitlines()]
            state['replayed'].append(prefix)
            assert 'curate = false' in (home / 'config.toml').read_text()
            with sqlite3.connect(home / 'raw.db') as db:
                db.execute('CREATE TABLE records(seq INTEGER, ts INTEGER, type TEXT, session TEXT, repo TEXT)')
                db.executemany('INSERT INTO records VALUES (?, ?, ?, ?, ?)',
                               [(i + 1, i, 'event', state['session'], state['repo']) for i in range(len(prefix))])
                db.execute('CREATE TABLE ops(op_seq INTEGER, type TEXT, body TEXT)')
            os.chmod(home / 'raw.db', 0o666)
            return subprocess.CompletedProcess(argv, 0, '{"events": 1}', '')
        if 'worker' in argv:
            assert argv[-2:] == ['--idle-ms', '0']
            config = (home / 'config.toml').read_text()
            curated = 'curate = true' in config
            assert ('model = "haiku"' in config) == curated
            if curated:
                assert kw['env']['PATH'].split(os.pathsep)[0] == str(home / 'wrap')
                if not state['no_models']:
                    (home / 'curator-models.jsonl').write_text('{"model": "claude-haiku-reported-2099"}\n')
                none = home.parent / 'none'
                with sqlite3.connect(none / 'raw.db') as db:
                    assert db.execute('SELECT COUNT(*) FROM ops').fetchone()[0] == 0
                with sqlite3.connect(home / 'raw.db') as db:
                    top = db.execute('SELECT MAX(seq) FROM records').fetchone()[0]
                    previous = db.execute('SELECT op_seq, body FROM ops ORDER BY op_seq DESC LIMIT 1').fetchone()
                    after = json.loads(previous[1])['to_seq'] if previous else 0
                    db.execute('INSERT INTO ops VALUES (?, "window", ?)',
                               (previous[0] + 1 if previous else 1,
                                json.dumps({'outcome': 'curated', 'from_seq': after + 1,
                                            'to_seq': 1 if state['partial_worker'] else top})))
                    state['partial_worker'] = False
            with sqlite3.connect(home / 'knowledge.db') as db:
                db.execute('CREATE TABLE IF NOT EXISTS active(kind TEXT)')
                if curated:
                    db.execute('INSERT INTO active VALUES ("open_item")')
            with sqlite3.connect(home / 'providers.db') as db:
                db.execute('CREATE TABLE IF NOT EXISTS provider_calls(provider TEXT, role TEXT, outcome TEXT, est_tokens INTEGER)')
                if curated:
                    db.execute('INSERT INTO provider_calls VALUES ("claude", "curator", "ok", 1)')
            return subprocess.CompletedProcess(argv, 0, '', '')
        if 'inject' in argv:
            assert argv[-1] == 'inject'       # A new session; no old session id passed.
            assert Path(kw['cwd']).is_dir()
            return subprocess.CompletedProcess(argv, 0, 'CONTEXT-SENSITIVE-' + home.name, '')
        raise AssertionError('unexpected subprocess')

    def chat(model, prompt, **kw):
        state['calls'].append((model, prompt))
        assert 'SENSITIVE' not in prompt and 'TOOL-SENTINEL' not in prompt
        if prompt.startswith('Here is a developer') and 'Labels:' not in prompt:
            assert 'PROMPT-00' not in prompt and 'REPLY-[redacted]' in prompt
            turns = prompt.split('\n\nAt the end')[0].split('\n')[1:]
            assert len(turns) == 80 and turns[0].startswith(state['first_turn'])
            assert all(len(t.split('] ', 1)[1].split(': ', 1)[1]) <= 2000 for t in turns)
            answer = {'open': ['OPEN-SENTINEL'], 'closed': ['CLOSED-SENTINEL'], 'next': 'NEXT-SENTINEL'}
            if state['bad_labels']:
                state['bad_labels'] -= 1
                answer['next'] = False
        elif 'Labels:' in prompt:
            answer = {'agree': True}
        else:
            if state['fail_grade'] and model == GRADERS[2]:
                state['fail_grade'] = False
                raise ConnectionError('SHOULD-NOT-PRINT')
            answer = {'shown_open': '\nOPEN-SENTINEL\n' in prompt}
        return json.dumps(answer), model + '-reported'

    monkeypatch.setattr(subprocess, 'run', process)
    monkeypatch.setattr(calib, 'chat', chat)
    monkeypatch.setenv('EVAL_TEST_SECRET', 'no subprocess may inherit this')
    return m5, str(binary), state, recorded, repo, events


@pytest.mark.parametrize('checkout', ['repo', 'recorded', 'missing'])
def test_run_replays_only_the_cut_and_keeps_the_none_home_uncurated(harness, checkout, capsys):
    m5, binary, state, recorded, repo, events = harness
    if checkout != 'repo':
        state['repo'] = 'github.com/example/project'
    if checkout == 'missing':
        recorded.rmdir()
    cuts = m5.cuts(binary)
    assert len(cuts) == 1 and cuts[0]['line'] == 89
    rows = m5.run(binary)
    assert len(state['replayed']) == 1 and state['replayed'][0] == events[:-1]
    assert rows[0]['no_checkout'] == (checkout == 'missing')
    if checkout != 'missing':
        assert set(rows[0]['contexts']) == {'curated', 'none'}
        injection = [kw['cwd'] for argv, kw in state['processes'] if 'inject' in argv]
        assert injection == [str(repo if checkout == 'repo' else recorded)] * 2
        assert rows[0]['metadata']['vector'] == 'off'
        assert rows[0]['metadata']['models']['curator'] == ['claude-haiku-reported-2099']
        assert rows[0]['curation']['claims'] == {'open_item': 1}
        assert (Path(rows[0]['homes']['none']) / 'raw.db').stat().st_mode & 0o777 == 0o600
        processes, calls = len(state['processes']), len(state['calls'])
        assert m5.run(binary) == rows
        assert len(state['processes']) == processes and len(state['calls']) == calls
    text = capsys.readouterr().out
    assert all(s not in text for s in ('PROMPT-', 'REPLY-', 'OPEN-SENTINEL', 'CLOSED-SENTINEL', 'NEXT-SENTINEL'))


def test_score_retries_only_the_failed_grader_and_never_scores_two_votes(harness, capsys):
    m5, binary, state, *_ = harness
    m5.cuts(binary)
    m5.run(binary)
    state['fail_grade'] = True
    pending = m5.score(binary)
    assert not pending['complete'] and 'tiers' not in pending
    before = len(state['calls'])
    completed = m5.score(binary)
    assert completed['complete'] and completed['pass']
    assert len(state['calls']) == before + 1
    assert completed['tiers']['curated']['next'] == {'shown': 0, 'n': 1, 'rate': 0.0}
    before = len(state['calls'])
    assert m5.score(binary) == completed and len(state['calls']) == before
    text = capsys.readouterr().out
    assert all(s not in text for s in ('OPEN-SENTINEL', 'CLOSED-SENTINEL', 'NEXT-SENTINEL', 'SHOULD-NOT-PRINT'))


def test_a_failed_labeller_is_retried_without_replaying_or_recurating(harness):
    m5, binary, state, *_ = harness
    m5.cuts(binary)
    state['bad_labels'] = 2
    rows = m5.run(binary)
    assert not rows[0]['complete']
    processes = [(argv, kw) for argv, kw in state['processes'] if 'gate' not in argv]
    assert not m5.score(binary)['complete']
    assert m5.run(binary)[0]['complete']
    assert [(argv, kw) for argv, kw in state['processes'] if 'gate' not in argv] == processes
    assert len(state['calls']) == 3


def test_a_partial_curator_pass_waits_for_the_remaining_windows(harness):
    m5, binary, state, *_ = harness
    m5.cuts(binary)
    state['partial_worker'] = True
    row = m5.run(binary)[0]
    assert not row['complete'] and row['pending_curation']
    assert 'contexts' not in row and not state['calls']
    assert not m5.score(binary)['complete']
    resumed = m5.run(binary)[0]
    assert resumed['complete'] and not resumed['pending_curation']
    assert len(state['replayed']) == 1 and resumed['curation']['coverage'] == '100%'


def test_a_requested_curator_alias_cannot_replace_missing_reported_models(harness):
    m5, binary, state, *_ = harness
    m5.cuts(binary)
    state['no_models'] = True
    row = m5.run(binary)[0]
    assert not row['complete'] and row['pending_metadata'] and not state['calls']
    assert row['curator']['status'] == 'missing_models' and row['curator']['models'] == []
    assert row['curation']['requested_curator']['model'] == 'haiku'
    assert 'curator' not in row['metadata']['models']
    assert not m5.score(binary)['complete']


def test_a_seeded_checker_reads_the_same_turns_from_a_different_judge(harness):
    import common, m3
    m5, binary, state, _, _, events = harness
    state.update(session='s3', first_turn='[7]')
    kept = [dict(e, session='s3') for e in events[:-3]]
    (Path(m3.M) / 'fixtures' / 's3.jsonl').write_text(json.dumps(kept[0]) + '\n\n'
                                                  + ''.join(json.dumps(e) + '\n' for e in kept[1:]))
    (Path(common.E) / 'replay' / 'manifest.json').write_text(json.dumps({'sessions': [
        {'session': 's3', 'side': 'dev'}]}))
    m5.cuts(binary)
    row = m5.run(binary)[0]
    assert row['check'] and row['labeller'] != row['checker']
    assert len(state['calls']) == 2
    labelled_turns = state['calls'][0][1].split('\n\nAt the end')[0].split('\n', 1)[1]
    checked_turns = state['calls'][1][1].split('\n\nLabels:')[0].split('\n', 1)[1]
    assert labelled_turns == checked_turns
    assert m5.score(binary)['label_agreement'] == {'n': 1, 'agree': 1, 'rate': 1.0}


def test_held_out_inputs_are_guarded_before_fixture_reads(harness, monkeypatch):
    import builtins, common
    m5, binary, *_ = harness
    original = builtins.open
    opened = []

    def open_file(file, *args, **kwargs):
        if str(file).endswith('/held-out.jsonl'):
            opened.append(file)
            raise AssertionError('held-out contents must not be opened')
        return original(file, *args, **kwargs)

    monkeypatch.setattr(builtins, 'open', open_file)
    for command in (m5.cuts, m5.run, m5.score):
        with pytest.raises(SystemExit, match='recorded curator deciding id'):
            command(binary, pool='test')
    common.keep_json(f'{common.E}/deciding.json', {'curator': 'accepted-run'})
    with pytest.raises(SystemExit, match='recorded curator deciding id'):
        m5.cuts(binary, pool='test', decide='another-run')
    assert not opened
    common.write_jsonl(m5.path('dev', 'cuts'), [{'session': 'held-out'}])
    with pytest.raises(SystemExit, match='recorded curator deciding id'):
        m5.run(binary)
    assert not opened


def test_score_waits_for_every_selected_session_and_refuses_changed_cuts(harness):
    import common
    m5, binary, state, *_ = harness
    selected = m5.cuts(binary)
    m5.run(binary)
    manifest = Path(common.E) / 'replay' / 'manifest.json'
    value = json.loads(manifest.read_text())
    value['sessions'].append({'session': 's2', 'side': 'dev'})
    manifest.write_text(json.dumps(value))
    common.write_jsonl(m5.path('dev', 'cuts'), selected + [dict(selected[0], session='s2')])
    calls = len(state['calls'])
    out = m5.score(binary)
    assert not out['complete'] and out['pending_sessions'] == 1 and len(state['calls']) == calls
    changed = [dict(selected[0], line=selected[0]['line'] - 1)]
    common.write_jsonl(m5.path('dev', 'cuts'), changed)
    with pytest.raises(SystemExit, match='inputs changed'):
        m5.score(binary)
    assert len(state['calls']) == calls


def test_the_recorded_deciding_id_opens_the_manifest_held_out_pool(harness):
    import common, m3
    m5, binary, *_ = harness
    events = [{'session': 'held-out', 'ts': ts} for ts in
              ('2026-09-30T00:00:00Z', '2026-09-30T00:30:00Z')]
    common.write_jsonl(f'{m3.M}/fixtures/held-out.jsonl', events)
    common.keep_json(f'{common.E}/deciding.json', {'curator': 'accepted-run'})
    cuts = m5.cuts(binary, pool='test', decide='accepted-run')
    assert len(cuts) == 1 and cuts[0]['session'] == 'held-out' and cuts[0]['line'] == 2


def test_curator_capture_preserves_stdout_and_keeps_only_model_names(monkeypatch, tmp_path):
    import common
    m5 = importlib.import_module('m5')
    home = Path(common.E) / 'm5' / 'capture'
    home.mkdir(parents=True)
    cli = tmp_path / 'cli' / 'claude'
    cli.parent.mkdir()
    output = (b'not json\n' + json.dumps({'type': 'assistant', 'message': {
        'model': 'claude-haiku-2099', 'content': 'PRIVATE-REPLY-SENTINEL'}}, ensure_ascii=False).encode() + b'\n'
        + json.dumps({'type': 'result', 'modelUsage': {'claude-haiku-2099': {}, 'claude-haiku-2100': {}},
                      'result': 'PRIVATE-ANSWER-SENTINEL'}).encode())
    cli.write_text(f'#!{sys.executable}\nimport os, sys\n'
                   "assert 'SECRET' not in ' '.join(os.environ)\n"
                   "assert sys.stdin.read() == 'PRIVATE-PROMPT-SENTINEL'\n"
                   f'sys.stdout.buffer.write({output!r})\n')
    cli.chmod(0o700)
    worker = tmp_path / 'offline-worker'
    worker.write_text(f'#!{sys.executable}\nimport os, pathlib, shutil, subprocess, sys\n'
                      "assert 'SECRET' not in ' '.join(os.environ)\n"
                      "home = pathlib.Path(sys.argv[sys.argv.index('--home') + 1])\n"
                      "child = subprocess.run([shutil.which('claude'), '-p'], input=b'PRIVATE-PROMPT-SENTINEL', capture_output=True)\n"
                      "(home / 'test-output').write_bytes(child.stdout)\n"
                      "sys.exit(child.returncode)\n")
    worker.chmod(0o700)
    monkeypatch.setenv('PATH', str(cli.parent) + os.pathsep + os.environ.get('PATH', ''))
    monkeypatch.setenv('CAPTURE_TEST_SECRET', 'removed before both children')
    allowed = [str(worker), '--home', str(home), 'worker', '--idle-ms', '0']

    def run(argv, **kw):
        assert argv == allowed
        return REAL_RUN(argv, **kw)

    def popen(argv, *args, **kw):
        assert argv == allowed
        return REAL_POPEN(argv, *args, **kw)

    monkeypatch.setattr(subprocess, 'run', run)
    monkeypatch.setattr(subprocess, 'Popen', popen)
    out = m5.curator_worker(str(worker), str(home))
    assert out == {'status': 'ok', 'ran': True, 'models': ['claude-haiku-2099', 'claude-haiku-2100']}
    assert (home / 'test-output').read_bytes() == output
    log = home / 'curator-models.jsonl'
    assert {r['model'] for r in common.read_jsonl(str(log))} == {'claude-haiku-2099', 'claude-haiku-2100'}
    assert 'PRIVATE-' not in log.read_text() and log.stat().st_mode & 0o777 == 0o600
    assert (home / 'wrap' / 'claude').stat().st_mode & 0o777 == 0o700
    with monkeypatch.context() as unsupported:
        unsupported.setattr(m5.os, 'name', 'nt')
        unavailable = m5.curator_worker(str(worker), str(home))
    assert unavailable == {'status': 'unsupported_platform', 'ran': False, 'models': []}
