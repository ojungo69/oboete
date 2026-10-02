import json, re, sqlite3, subprocess, sys
from pathlib import Path

import pytest

import common


def pool_rows(pool='dev'):
    n = 50 if pool == 'dev' else 100
    return [{'qid': f'{kind}-{i}', 'kind': kind, 'text': f'{kind} question {i}\u2028one line',
             'repo': 'example.org/project'}
            for kind in ('no-answer', 'false-premise') for i in range(n)]


def test_thirteen_of_two_hundred_passes_fourteen_fails():
    import inject
    # One-sided 95%, not the two-sided 95% interval (whose upper bound fails at 13).
    assert inject.wilson_upper(13, 200) == pytest.approx(0.0998728946, abs=1e-10)
    assert inject.wilson_upper(13, 200) <= 0.10
    assert inject.wilson_upper(14, 200) > 0.10
    assert inject.wilson_upper(0, 200) > 0
    assert inject.wilson_upper(200, 200) == pytest.approx(1)


def test_the_grid_has_thirteen_exact_thresholds():
    import inject
    assert inject.THRESHOLDS == ('0.30', '0.35', '0.40', '0.45', '0.50', '0.55', '0.60',
                                 '0.65', '0.70', '0.75', '0.80', '0.85', '0.90')


def test_the_sealed_pool_is_refused_before_its_contents_are_read(monkeypatch):
    import inject
    real_read = common.read_jsonl
    opened = []

    def read(path):
        opened.append(path)
        return real_read(path)

    monkeypatch.setattr(common, 'read_jsonl', read)
    for decide in (None, 'curator', 'wrong'):
        with pytest.raises(SystemExit, match='recorded curator deciding id'):
            inject.load_pool('test', decide)
    common.keep_json(f'{common.E}/deciding.json', {'curator': 'chosen'})
    with pytest.raises(SystemExit, match='recorded curator deciding id'):
        inject.load_pool('test', 'wrong')
    assert not opened
    rows = pool_rows('test')
    common.write_jsonl(f'{common.E}/inject/pool-test.jsonl', rows)
    assert inject.load_pool('test', 'chosen') == rows


@pytest.mark.parametrize('problem', ['count', 'balance', 'qid', 'text', 'kind', 'blank', 'repo', 'missing-repo'])
def test_pool_requires_exact_counts_and_unique_nonempty_questions(problem):
    import inject
    rows = pool_rows()
    if problem == 'count':
        rows.pop()
    elif problem == 'balance':
        rows[0]['kind'] = 'false-premise'
    elif problem in ('qid', 'text'):
        rows[0][problem] = rows[1][problem]
    elif problem == 'kind':
        rows[0]['kind'] = 'answerable'
    elif problem == 'repo':
        rows[0]['repo'] = '  '
    elif problem == 'missing-repo':
        del rows[0]['repo']
    else:
        rows[0]['text'] = '  '
    common.write_jsonl(f'{common.E}/inject/pool-dev.jsonl', rows)
    with pytest.raises(SystemExit, match='Inject pool'):
        inject.load_pool()


def test_pool_keeps_unicode_line_separators_and_prints_only_counts(capsys):
    import inject
    rows = pool_rows()
    common.write_jsonl(f'{common.E}/inject/pool-dev.jsonl', rows)
    assert inject.load_pool() == rows
    inject.main(['pool', '--pool', 'dev'])
    assert json.loads(capsys.readouterr().out) == {'n': 100, 'no-answer': 50, 'false-premise': 50}


def votes(*values):
    return {model: None if value is None else {'relevant': value}
            for model, value in zip(common.GRADERS, values)}


def scored_rows():
    import inject
    questions = [dict(q, source='inject') for q in pool_rows()]
    questions += [{'source': 'm6', 'qid': f'm{i}', 'text': f'M6 {i}', 'kind': 'm6',
                   'repo': 'example.org/project'} for i in range(40)]
    rows = [dict(q, threshold=t, claims=[], relevant=False) for t in inject.THRESHOLDS for q in questions]
    return questions, rows


def test_score_counts_questions_any_bad_claim_no_answer_and_lowest_pass():
    import inject
    questions, rows = scored_rows()
    for r in rows:
        if r['source'] == 'm6':
            r['relevant'] = int(r['qid'][1:]) < 24
        elif r['qid'] == 'false-premise-0':
            r['claims'] = [{'uid': 'a', 'body': 'relevant'}, {'uid': 'b', 'body': 'irrelevant'}]
        elif r['qid'] in {f'no-answer-{i}' for i in range(9)} and r['threshold'] in ('0.30', '0.35'):
            r['claims'] = [{'uid': 'a', 'body': 'relevant'}]
    labels = [dict(p, votes=votes(True, True, False) if p['uid'] == 'a' else votes(False, True, False))
              for p in inject.pairs(rows).values()]
    out = inject.metrics(questions, rows, labels)
    assert out['complete'] and out['chosen'] == '0.40'
    assert out['thresholds']['0.30']['irrelevant'] == 10  # Questions, never the count of claims.
    assert not out['thresholds']['0.35']['pass']
    assert out['thresholds']['0.40']['irrelevant'] == 1
    assert out['thresholds']['0.40']['m6'] == {'n': 40, 'relevant': 24, 'share': 0.6}
    assert inject.metrics(questions, rows, labels, pool='test')['chosen'] is None
    for r in rows:
        if r['source'] == 'inject':
            r['claims'] = [{'uid': 'a', 'body': 'relevant'}]
    labels = [dict(p, votes=votes(True, True, True)) for p in inject.pairs(rows).values()]
    assert inject.metrics(questions, rows, labels)['chosen'] is None


def test_score_waits_for_all_graders_and_every_question_at_every_threshold():
    import inject
    questions, rows = scored_rows()
    rows[0]['claims'] = [{'uid': 'a', 'body': 'note'}]  # Even a no-answer pair must be labelled.
    labels = [dict(p, votes=votes(True, True, None)) for p in inject.pairs(rows).values()]
    out = inject.metrics(questions, rows, labels)
    assert not out['complete'] and out['pending'] == 1 and 'thresholds' not in out
    labels[0]['votes'] = votes(True, True, False)
    assert inject.metrics(questions, rows, labels)['complete']
    for broken in (rows[:-1], rows + [rows[0]], [dict(rows[0], threshold='0.31')] + rows[1:]):
        with pytest.raises(SystemExit, match='grid'):
            inject.metrics(questions, broken, labels)


def test_the_inject_prompt_is_the_protocols():
    import inject
    text = Path(__file__).resolve().parents[1].joinpath('milestone-4.md').read_text()
    assert inject.INJECT == re.search(r'^INJECT:\n\n```text\n(.*?)```', text, re.M | re.S)[1]


@pytest.fixture
def harness(monkeypatch, tmp_path):
    import m3
    monkeypatch.setattr(m3, 'M', f'{common.E}/m3')
    manifest = [{'session': f's{i}', 'side': 'dev'} for i in range(30)]
    common.keep_json(f'{common.E}/replay/manifest.json', {'sessions': manifest})
    for s in manifest:
        common.write_jsonl(f'{m3.M}/fixtures/{s["session"]}.jsonl', [
            {'ts': '2026-09-01T10:00:00Z', 'payload': {'prompt': 'old work'}},
            {'ts': '2026-09-02T10:00:00Z', 'payload': {'prompt': 'more work'}}])
    qs = [{'id': f'm{i}', 'session': f's{i // 2}', 'record': i % 2, 'dated': i < 8,
           'question': f'M6 {i}', 'asked_at': '2026-09-03'} for i in range(40)]
    keys = [{'id': q['id'], 'session': q['session'], 'answer': 'answer',
             'records': [{'device': 'd', 'seq': i + 1}]} for i, q in enumerate(qs)]
    common.write_jsonl(f'{common.E}/m6/questions-dev.jsonl', qs)
    common.write_jsonl(f'{common.E}/m6/keys-dev.jsonl', keys)
    common.keep_json(f'{common.E}/m6/keys-dev.json', {'models': {'key-writer': ['writer-reported']}})
    negatives = pool_rows()
    for i, q in enumerate(negatives):
        q['repo'] = 'example.org/' + ('one' if i % 2 == 0 else 'two')
    common.write_jsonl(f'{common.E}/inject/pool-dev.jsonl', negatives)
    source = tmp_path / 'dev-home'
    source.mkdir()
    (source / 'config.toml').write_text('[summary]\ncurate = true\n[embedding]\nprovider = "none"\n'
                                      '[redaction]\npatterns = ["SENSITIVE"]\n')
    common.keep_json(str(source / 'curation.json'), {'models': ['haiku-reported']})
    with sqlite3.connect(source / 'raw.db') as db:
        db.execute('CREATE TABLE records(device, seq, type, session, repo)')
        db.executemany('INSERT INTO records VALUES (?,?,?,?,?)', [
            ('d', i + 1, 'event', f's{i // 2}', 'example.org/' + ('one' if i % 2 == 0 else 'two'))
            for i in range(60)])
    with sqlite3.connect(source / 'knowledge.db') as db:
        db.execute('CREATE TABLE active(uid, body)')
    binary = tmp_path / 'fake-oboete'
    binary.write_text(f'#!{sys.executable}\n' + r'''
import json, os, pathlib, sqlite3, sys, tomllib
args = sys.argv[1:]
assert args[0] == '--home'
home = pathlib.Path(args[1])
assert home.is_dir() and home.parent.name == 'inject'
assert all(not any(s in k.upper() for s in ('TOKEN', 'KEY', 'SECRET', 'PASSWORD')) for k in os.environ)
config = tomllib.loads((home/'config.toml').read_text())
assert config['redaction']['patterns'] == ['SENSITIVE']
assert config['summary']['curate'] is False and config['embedding']['provider'] == 'none'
command = args[2]
if command == 'worker':
    assert args[3:] == ['--idle-ms', '0']
    (home/'warm').touch()
elif command == 'gate':
    sys.stdout.write(sys.stdin.read().replace('SENSITIVE', '[redacted]'))
elif command == 'inject':
    assert (home/'warm').exists()
    assert '--prompt' in args and '--repo' in args and '--session' in args and '--threshold' in args
    text = sys.stdin.read()
    t = args[args.index('--threshold') + 1]
    if text.startswith('M6 '):
        n = int(text[3:])
        if n < 27:
            print(f'{n + 16:064x} 0.990')
    elif text.startswith('no-answer question ') and t in ('0.30', '0.35'):
        if int(text.split('question ')[1].split('\u2028')[0]) < 9:
            print('a' * 64 + ' 0.700')
    elif text.startswith('false-premise question 0\u2028'):
        print('a' * 64 + ' 0.990')
        print('b' * 64 + ' 0.010')  # A linked claim can be below the threshold.
elif command == 'get':
    uid = args[3]
    body = 'IRRELEVANT' if uid == 'b' * 64 else 'note SENSITIVE\nquotes:\ninside body\u2028kept'
    print(f'{uid} 2026-09-01 decision decided example.org/one (citable)\nspeaker: user, scope: repo\n\n{body}\n\nquotes:\n- d:1: QUOTE-NOT-THE-BODY')
elif command == 'cite':
    uid = args[3]
    n = int(uid, 16) - 16 if uid[0] == '0' else 0
    print(json.dumps([{'uid': uid, 'label': 'citable', 'evidence': [
        {'device': 'other' if n == 25 else 'd', 'seq': 999 if n == 26 else n + 1,
         'live': n != 24, 'quote': 'quote', 'offset': 0, 'length': 5}]}]))
else:
    raise AssertionError('unexpected command')
''')
    binary.chmod(0o700)
    commands = []
    real_run = subprocess.run

    def run(argv, **kwargs):
        assert argv[0] == str(binary)
        assert not any(s in k.upper() for k in kwargs['env'] for s in ('TOKEN', 'KEY', 'SECRET', 'PASSWORD'))
        commands.append((list(argv), dict(kwargs)))
        return real_run(argv, **kwargs)

    monkeypatch.setattr(subprocess, 'run', run)
    monkeypatch.setenv('EVAL_PRIVATE_TOKEN', 'must never reach command')
    return str(binary), str(source), commands


def test_run_sweeps_a_drained_copy_and_uses_each_repo_stdin_and_fresh_session(harness, capsys):
    import inject
    binary, source, commands = harness
    before = {p.name: p.read_bytes() for p in Path(source).iterdir()}
    out = inject.main(['run', '--binary', binary, '--dev-home', source, '--pool', 'dev'])
    assert out['complete'] and out['n'] == 100 and out['m6_n'] == 40
    assert {p.name: p.read_bytes() for p in Path(source).iterdir()} == before
    rows = common.read_jsonl(inject.path('runs', 'dev'))
    calls = [(a, kw) for a, kw in commands if 'inject' in a]
    assert len(rows) == len(calls) == 1820
    assert len({a[a.index('--session') + 1] for a, _ in calls}) == 1820
    assert {a[a.index('--threshold') + 1] for a, _ in calls} == set(inject.THRESHOLDS)
    for row, (a, kw) in zip(rows, calls):
        assert kw['input'] == row['text'] and a[a.index('--repo') + 1] == row['repo']
    m6_rows = [r for r in rows if r['source'] == 'm6' and r['threshold'] == '0.30']
    assert sum(r['relevant'] for r in m6_rows) == 24
    false_premise = next(r for r in rows if r['qid'] == 'false-premise-0')
    assert len(false_premise['claims']) == 2
    assert false_premise['claims'][0]['body'] == 'note SENSITIVE\nquotes:\ninside body\u2028kept'
    assert false_premise['claims'][1]['uid'] == 'b' * 64
    saved = json.loads(Path(inject.path('runs', 'dev', 'json')).read_text())
    metadata = saved['metadata']
    assert metadata['N'] == 100 and metadata['sha256'] == common.sha256_file(binary)
    assert metadata['machine'] and metadata['vector'] == 'off'
    assert metadata['models']['curator'] == ['haiku-reported']
    assert not Path(metadata['home']).exists()
    assert not list(Path(common.E, 'inject').glob('home-*'))
    for p in Path(common.E, 'inject').rglob('*'):
        assert p.stat().st_mode & 0o777 == (0o700 if p.is_dir() else 0o600)
    assert 'SENSITIVE' not in capsys.readouterr().out


def saved_run(harness):
    import inject
    binary, _, _ = harness
    questions, rows = scored_rows()
    for r in rows:
        if r['qid'] in ('no-answer-0', 'false-premise-0'):
            r['claims'] = [{'uid': 'a' * 64, 'body': 'note SENSITIVE'}]
        if r['qid'] == 'false-premise-0' and r['threshold'] == '0.30':
            r['claims'].append({'uid': 'b' * 64, 'body': 'IRRELEVANT'})
        if r['qid'] == 'm0':
            r['claims'] = [{'uid': 'a' * 64, 'body': 'note SENSITIVE'}]
            r['relevant'] = True
    common.write_jsonl(inject.path('runs', 'dev'), rows)
    config = inject.path('config', 'dev', 'toml')
    common.owner_only(config)
    Path(config).write_text('[summary]\ncurate = false\n[embedding]\nprovider = "none"\n'
                            '[redaction]\npatterns = ["SENSITIVE"]\n')
    kept = {'complete': True, 'questions': questions, 'binary': binary,
            'input': dict(inject.input_hashes('dev'), binary=common.sha256_file(binary)),
            'runs_sha256': common.sha256_file(inject.path('runs', 'dev')),
            'config_sha256': common.sha256_file(config),
            'metadata': common.record(binary, '/already-deleted-copy', 100, 'off', {}), 'm6_n': 40}
    common.keep_json(inject.path('runs', 'dev', 'json'), kept)
    return rows


def test_label_once_per_pair_gates_inputs_keeps_models_and_score_calls_no_grader(harness, monkeypatch, capsys):
    import inject
    saved_run(harness)
    asked = []
    missing = [True]

    class Calls:
        def votes(self, prompt, fields):
            assert fields == ('relevant',)
            assert 'SENSITIVE' not in prompt
            assert '[redacted]' in prompt or 'IRRELEVANT' in prompt
            asked.append(prompt)
            return votes(False, True, False) if 'IRRELEVANT' in prompt else votes(True, True, None if missing[0] else False)

        def models(self, *earlier):
            out = {k: v for group in earlier for k, v in group.items()}
            out.update({m: [m + '-reported'] for m in common.GRADERS})
            return out

    monkeypatch.setattr(common, 'Calls', Calls)
    first = inject.main(['label'])
    assert len(asked) == 4 and first['n'] == 4 and not first['complete']
    pending = inject.main(['score', '--pool', 'dev'])
    assert not pending['complete'] and 'thresholds' not in pending
    assert len(asked) == 4
    missing[0] = False
    assert inject.main(['label'])['complete']
    assert len(asked) == 7  # Only the three unfinished pairs are retried.
    assert inject.main(['label'])['complete'] and len(asked) == 7
    out = inject.main(['score', '--pool', 'dev'])
    assert len(asked) == 7 and out['chosen'] == '0.30'
    assert out['thresholds']['0.30']['irrelevant'] == 2
    assert out['thresholds']['0.35']['irrelevant'] == 1
    assert out['chosen_m6'] == 1 / 40
    assert out['metadata']['models']['glm-5.3'] == ['glm-5.3-reported']
    assert json.loads(Path(inject.path('score', 'dev', 'json')).read_text()) == out
    assert not list(Path(common.E, 'inject').glob('gate-*'))
    assert 'SENSITIVE' not in capsys.readouterr().out


@pytest.mark.parametrize('command', ['pool', 'run', 'label', 'score'])
def test_every_command_guards_the_test_pool_before_reading(command, harness):
    import inject
    binary, source, commands = harness
    args = [command, '--pool', 'test']
    if command == 'run':
        args += ['--binary', binary, '--dev-home', source]
    with pytest.raises(SystemExit, match='recorded curator deciding id'):
        inject.main(args)
    assert not commands


@pytest.mark.parametrize('changed', ['pool', 'keys', 'questions', 'binary', 'runs', 'config'])
def test_label_and_score_refuse_changed_inputs_before_calling_anything(changed, harness):
    import inject
    saved_run(harness)
    binary, _, commands = harness
    files = {'pool': inject.path('pool', 'dev'), 'keys': f'{common.E}/m6/keys-dev.jsonl',
             'questions': f'{common.E}/m6/questions-dev.jsonl', 'binary': binary,
             'runs': inject.path('runs', 'dev'), 'config': inject.path('config', 'dev', 'toml')}
    with open(files[changed], 'a') as f:
        f.write('\n')
    for command in ('label', 'score'):
        with pytest.raises(SystemExit, match='changed'):
            inject.main([command])
    assert not commands


def test_the_pass_rule_uses_the_wilson_bound_over_two_hundred_questions():
    import inject
    _, dev_rows = scored_rows()
    m6_questions = [q for q in scored_rows()[0] if q['source'] == 'm6']
    questions = [dict(q, source='inject') for q in pool_rows('test')] + m6_questions
    for count, passed in ((13, True), (14, False)):
        rows = [dict(q, threshold=t, claims=[{'uid': 'a', 'body': 'note'}] if i < count else [])
                for t in inject.THRESHOLDS for i, q in enumerate(questions) if q['source'] == 'inject']
        rows += [r for r in dev_rows if r['source'] == 'm6']
        labels = [dict(p, votes=votes(True, True, True)) for p in inject.pairs(rows).values()]
        out = inject.metrics(questions, rows, labels, pool='test')
        assert out['thresholds']['0.30']['n'] == 200
        assert out['thresholds']['0.30']['irrelevant'] == count
        assert out['thresholds']['0.30']['pass'] is passed


@pytest.mark.parametrize('problem', ['held-out', 'wal', 'symlink', 'embedder'])
def test_unsafe_source_homes_are_refused_without_mutating_them(harness, problem):
    import inject
    binary, home, commands = harness
    source = Path(home)
    if problem == 'held-out':
        with sqlite3.connect(source / 'raw.db') as db:
            db.execute("INSERT INTO records VALUES ('d',999,'event','hidden','example.org/one')")
        common.keep_json(f'{common.E}/replay/manifest.json', {'sessions': [
            *[{'session': f's{i}', 'side': 'dev'} for i in range(30)],
            {'session': 'hidden', 'side': 'held-out'}]})
    elif problem == 'wal':
        (source / 'raw.db-wal').write_bytes(b'active WAL')
    elif problem == 'symlink':
        (source / 'elsewhere').symlink_to(source.parent / 'repo-one', target_is_directory=True)
    else:
        p = source / 'config.toml'
        p.write_text(p.read_text().replace('"none"', '"workers-ai"'))
    before = {p.name: p.read_bytes() for p in source.iterdir() if p.is_file()}
    with pytest.raises(SystemExit):
        inject.run(binary, home)
    assert before == {p.name: p.read_bytes() for p in source.iterdir() if p.is_file()}
    assert not commands
    assert not list(Path(common.E, 'inject').glob('home-*'))


@pytest.mark.parametrize('stage', ['worker', 'inject', 'get', 'cite'])
def test_command_failure_is_private_and_the_copy_is_deleted(harness, stage, capsys):
    import inject
    binary, home, _ = harness
    script = Path(binary)
    script.write_text(script.read_text().replace("command = args[2]", "command = args[2]\n"
        f"if command == {stage!r}:\n    print('PRIVATE-FAILURE', file=sys.stderr)\n    sys.exit(1)"))
    with pytest.raises(SystemExit, match='private text is not printed'):
        inject.main(['run', '--binary', binary, '--dev-home', home])
    assert not list(Path(common.E, 'inject').glob('home-*'))
    assert 'PRIVATE-FAILURE' not in ''.join(capsys.readouterr())
    saved = Path(inject.path('runs', 'dev', 'json'))
    if saved.exists():
        assert not json.loads(saved.read_text())['complete']


def test_m6_repository_fallback_refuses_missing_or_ambiguous_key_records(harness):
    import inject
    _, home, _ = harness
    q = {'id': 'm0', 'session': 's0', 'question': 'prompt'}
    for records in ([{'device': 'd', 'seq': 999}], [{'device': 'd', 'seq': 1}, {'device': 'd', 'seq': 2}]):
        with pytest.raises(SystemExit, match='M6 key record'):
            inject.m6_inputs(home, [q], [{'id': 'm0', 'records': records}])


def test_a_malformed_injection_report_is_not_scored_as_no_injection(harness):
    import inject
    binary, home, commands = harness
    for text in ('warning: cannot read claims\n', 'prefix 0.500\n', 'a' * 64 + ' NaN\n',
                 'a' * 64 + ' 0.500\u2028' + 'b' * 64 + ' 0.500\n'):
        with pytest.raises(ValueError, match='report'):
            inject.claims(binary, home, text, {})
    assert not commands
