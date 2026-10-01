import datetime, json, os, sqlite3, subprocess, sys

import pytest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import m4
from common import read_jsonl, write_jsonl


@pytest.fixture
def ev(tmp_path, monkeypatch):
    monkeypatch.setattr(m4, 'E', str(tmp_path))
    return tmp_path


def v1_db(ev):
    os.makedirs(f'{ev}/home', exist_ok=True)
    db = sqlite3.connect(f'{ev}/home/oboete.db')
    db.executescript('CREATE TABLE imports(source TEXT, source_id TEXT, doc TEXT, PRIMARY KEY (source, source_id));'
                     'CREATE TABLE sessions(id TEXT PRIMARY KEY, agent TEXT, started_at INTEGER);')
    return db


def q(qid, split='test', lang='ja'):
    return {'qid': qid, 'split': split, 'lang': lang, 'set': 'prompt', 'session': f's-{qid}', 'text': 'x'}


def test_the_test_questions_are_112_and_53_with_60_english(ev):
    write_jsonl(f'{ev}/queries.jsonl', [q(f'p{i}', lang='en' if i < 7 else 'ja') for i in range(112)]
                + [q(f'p{1000 + i}', split='dev') for i in range(10)])
    write_jsonl(f'{ev}/queries-en.jsonl', [q(f'p{2000 + i}', lang='en') for i in range(53)])
    test = m4.questions('test')
    assert len(test) == m4.TEST_N == 165
    assert sum(x['lang'] == 'en' for x in test) == m4.ENGLISH_N == 60
    assert read_jsonl(f'{ev}/questions-test-m4.jsonl') == test
    assert len(m4.questions('dev')) == 10
    # A qid is asked once: M21's questions never repeat the 112's.
    write_jsonl(f'{ev}/queries-en.jsonl', [q('p1', lang='en')])
    with pytest.raises(ValueError):
        m4.questions('test')


def test_a_b_uid_maps_by_source_id(ev, tmp_path):
    db = v1_db(ev)
    db.executemany('INSERT INTO imports VALUES (?, ?, ?)', [('claude-mem', 'o1', 'o107'), ('claude-mem', 's1', 's15')])
    db.commit()
    assert m4.v1_doc('claude-mem:b62f07076e19:o1') == 'o107'
    assert m4.v1_doc('claude-mem:b62f07076e19:s1') == 's15'
    for uid in ('claude-mem:b62f07076e19:o2', 'transcript:x:o1'):
        with pytest.raises(KeyError):
            m4.v1_doc(uid)
    # A run maps by it: records keep their key, the Raw runs keep the eligible questions, and
    # each file keeps its run's time.
    out, runs = tmp_path / 'out', tmp_path / 'runs'
    out.mkdir()
    (out / 'b-off.trec').write_text('q1 Q0 claude-mem:x:o1 1 50 b-off\nq1 Q0 r:d:3 2 49 b-off\n')
    (out / 'b-only.trec').write_text('q1 Q0 r:d:3 1 50 b-only\nq2 Q0 r:d:4 1 50 b-only\n')
    os.utime(out / 'b-off.trec', (1000, 1000))
    write_jsonl(f'{out}/b-docs.jsonl', [{'key': 'claude-mem:x:o1', 'session': 'a', 'ts': 1, 'kind': 'decision', 'text': 't'},
                                        {'key': 'r:d:3', 'session': 'b', 'ts': 2, 'kind': 'prompt', 'text': 'u'}])
    (out / 'stores.json').write_text('{}')
    m4.map_runs(str(out), str(runs), {'q1'})
    assert (runs / 'b-off.trec').read_text() == 'q1 Q0 o107 1 50 b-off\nq1 Q0 r:d:3 2 49 b-off\n'
    assert (runs / 'b-only.trec').read_text() == 'q1 Q0 r:d:3 1 50 b-only\n'
    assert os.path.getmtime(runs / 'b-off.trec') == 1000
    assert [r['doc'] for r in read_jsonl(f'{runs}/b-docs.jsonl')] == ['o107', 'r:d:3']
    (out / 'b-off.trec').write_text('q1 Q0 claude-mem:x:o9 1 50 b-off\n')
    with pytest.raises(KeyError):
        m4.map_runs(str(out), str(runs), {'q1'})


def test_raw_eligibility_follows_the_agents_window(ev):
    db = v1_db(ev)
    at = lambda s: int(datetime.datetime.fromisoformat(s).timestamp() * 1000)
    # The first day is Japan's: 00:30 JST on the 19th is the 18th in UTC.
    db.executemany('INSERT INTO sessions VALUES (?, ?, ?)', [
        ('c-first-day', 'claude', at('2026-06-19T00:30:00+09:00')),
        ('c-before', 'claude', at('2026-06-18T23:59:00+09:00')),
        ('x-first-day', 'codex', at('2026-08-23T08:00:00+09:00')),
        ('x-before', 'codex', at('2026-08-22T12:00:00+09:00')),
        ('o-late', 'opencode', at('2026-09-01T00:00:00+09:00'))])
    db.commit()
    window = {'claude': '2026-06-19', 'codex': '2026-08-23'}
    sessions = ('c-first-day', 'c-before', 'x-first-day', 'x-before', 'o-late', 'unknown')
    assert {s for s in sessions if m4.raw_eligible({'session': s}, window)} == {'c-first-day', 'x-first-day'}


def claude(root, session, cwd, *times):
    os.makedirs(f'{root}/project', exist_ok=True)
    with open(f'{root}/project/{session}.jsonl', 'w') as f:
        f.write('{"type":"file-history-snapshot"}\n')
        for t in times:
            f.write(json.dumps({'type': 'user', 'cwd': cwd, 'sessionId': session, 'timestamp': t}) + '\n')


def codex(root, session, cwd, t, forked_from=None):
    os.makedirs(f'{root}/2026/08/23', exist_ok=True)
    with open(f'{root}/2026/08/23/rollout-2026-08-23T00-00-00-{session}.jsonl', 'w') as f:
        meta = {'id': session, 'cwd': cwd, 'forked_from_id': forked_from}
        f.write(json.dumps({'timestamp': t, 'type': 'session_meta', 'payload': meta}) + '\n')


def test_the_corpus_leaves_out_late_tmp_observer_and_held_out_sessions(tmp_path):
    c, x = tmp_path / 'claude', tmp_path / 'codex'
    # A session that runs past the copy time stays: its later events are left at the replay.
    claude(c, 'kept', '/home/u/p', '2026-06-19T00:00:00Z', '2026-09-25T00:00:00Z')
    claude(c, 'late', '/home/u/p', '2026-09-24T00:56:46Z')
    claude(c, 'tmp', '/tmp/x', '2026-07-01T00:00:00Z')
    claude(c, 'tmpfoo', '/tmpfoo/x', '2026-07-01T00:00:00Z')
    claude(c, 'observer', f'{m4.OBSERVER}/observer-sessions', '2026-07-01T00:00:00Z')
    claude(c, 'held', '/home/u/p', '2026-07-01T00:00:00Z')
    claude(c, 'timeless', '/home/u/p')
    one, two = '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000002'
    codex(x, one, '/home/u/p', '2026-08-23T00:00:00Z')
    codex(x, two, '/tmp', '2026-09-01T00:00:00Z')
    # A fork opens with a copy of its parent's history.
    codex(x, '00000000-0000-0000-0000-000000000003', '/home/u/p', '2026-09-01T00:00:00Z', forked_from=one)
    got = m4.corpus(m4.transcripts(str(c), str(x)), {'held'})
    assert sorted(s['session'] for s in got['sessions']) == [one, 'kept', 'tmpfoo']
    assert got['left_out'] == {'late': 1, 'tmp': 2, 'observer': 1, 'held-out': 1, 'no time': 1, 'fork': 1}
    assert got['window'] == {'claude': '2026-06-19', 'codex': '2026-08-23'}


FAKE = '''#!/usr/bin/env python3
import json, shutil, sys
args = sys.argv[1:]
if args[0] == 'transcript':
    for ts in ('2026-09-24T00:56:47Z', '2026-09-20T00:00:02Z', '2026-09-20T00:00:01Z'):
        print(json.dumps({'agent': 'claude', 'event': 'UserPromptSubmit', 'session': 's', 'ts': ts,
                          'payload': {'prompt': 'line\u2028separator'}}, ensure_ascii=False))
else:
    shutil.copy(args[3], args[1] + '/replayed.jsonl')
    print('{}')
'''


def test_a_second_replay_is_refused(tmp_path):
    home = tmp_path / 'b-m4'
    home.mkdir()
    (home / 'config.toml').write_text('[summary]\ncurate = false\n')
    binary = tmp_path / 'oboete'
    binary.write_text(FAKE)
    binary.chmod(0o755)
    sessions = [{'agent': 'claude', 'session': 's', 'path': 'unused'}]
    report = m4.replay(str(binary), str(home), sessions)
    # Events before the copy time only, each session's in time order.
    # A raw U+2028 inside a JSON string does not end its line.
    replayed = [json.loads(line)['ts'] for line in (home / 'replayed.jsonl').open(encoding='utf-8')]
    assert replayed == ['2026-09-20T00:00:01Z', '2026-09-20T00:00:02Z']
    assert (report['sessions'], report['events']) == (1, 2)
    with pytest.raises(RuntimeError, match='already'):
        m4.replay(str(binary), str(home), sessions)
    # A replay that stopped leaves its mark too: the home is made again, never added to.
    stops = tmp_path / 'stops'
    stops.mkdir()
    (stops / 'config.toml').write_text('[summary]\ncurate = false\n')
    failing = tmp_path / 'failing'
    failing.write_text('#!/bin/sh\nexit 3\n')
    failing.chmod(0o755)
    with pytest.raises(subprocess.CalledProcessError):
        m4.replay(str(failing), str(stops), sessions)
    with pytest.raises(RuntimeError, match='already'):
        m4.replay(str(binary), str(stops), sessions)
    # A home with an embedding provider, or one that curates, is refused before anything.
    for config in ('[summary]\ncurate = false\n[embedding]\nprovider = "workers-ai"\n', '[summary]\ncurate = true\n'):
        other = tmp_path / 'other'
        other.mkdir(exist_ok=True)
        (other / 'config.toml').write_text(config)
        with pytest.raises(RuntimeError, match='owner answers'):
            m4.replay(str(binary), str(other), sessions)
        assert not (other / 'replay-m4.json').exists()


def test_a_missing_sidecar_row_raises(tmp_path):
    write_jsonl(f'{tmp_path}/b-docs.jsonl', [{'key': 'r:d:1', 'doc': 'r:d:1', 'session': 's', 'ts': 1,
                                              'kind': 'prompt', 'text': 'x'}])
    rows = m4.sidecar(str(tmp_path))
    assert m4.record(rows, 'r:d:1')['session'] == 's'
    with pytest.raises(KeyError):
        m4.record(rows, 'r:d:2')


def test_the_gate_fails(tmp_path):
    asked = {'q1': {'qid': 'q1', 'session': 's1'}, 'q2': {'qid': 'q2', 'session': 's2'}}
    # A missing question: every run has every question of its set.
    runs = {'b-off': {'q1': ['o1']}, 'b-only': {'q2': ['r:d:1']}}
    assert m4.missing(runs, ['b-off', 'b-only', 'hybrid-d2'], {'q1', 'q2'}, {'q2'}) == [
        'hybrid-d2: no run', 'b-off: no line for q2']
    # A hit of the question's own session.
    session = {'o1': 's2', 'r:d:1': 's2'}.get
    assert m4.own_session(runs, asked, session) == ['b-only: q2 has r:d:1 of its own session']
    # A key the gate already reports as missing or unmapped has no session: it does not stop the gate.
    of = m4.session_of({'r:d:1': {'session': 's2'}}, {'o1': ('text', 's1', 4)}.__getitem__)
    assert [of(d) for d in ('r:d:1', 'r:d:9', 'o1', 'o+1', 'claude-mem:db:o5')] == ['s2', None, 's1', None, None]
    # A candidate run made before the pre-registration's main commit; a baseline's time is its own.
    for name in ('b-off', 'hybrid-d2'):
        (tmp_path / f'{name}.trec').write_text('q1 Q0 o1 1 50 x\n')
        os.utime(tmp_path / f'{name}.trec', (1000, 1000))
    assert m4.older(str(tmp_path), ['b-off', 'hybrid-d2'], 1000, 1001) == ['b-off: made before the pre-registration']
    assert m4.older(str(tmp_path), ['b-off'], 999, 1001) == []
    # An eval that started before the commit and ended after it.
    assert m4.older(str(tmp_path), ['b-off'], 999, 998) == ['the eval started before the pre-registration']
    # A run file the gate does not check, which the judge would still pool.
    assert m4.unexpected(str(tmp_path), ['b-off']) == ['hybrid-d2: a run the gate does not check']
    # A Raw question whose session has no record.
    k = sqlite3.connect(tmp_path / 'knowledge.db')
    k.execute('CREATE TABLE raw_docs(device TEXT, seq INTEGER, session TEXT)')
    k.execute("INSERT INTO raw_docs VALUES ('d', 1, 's2')")
    k.commit()
    # A Raw question whose session has no record in the corpus's replay, or is not in the corpus.
    asked['q3'] = {'qid': 'q3', 'session': 's3'}
    assert m4.recordless({'q1', 'q2', 'q3'}, asked, str(tmp_path), {'s1', 's2'}) == [
        'q1: no record of its session', 'q3: its session is not in the corpus']
    # B's text against v1's for the mapped documents: one matches, one is another document.
    write_jsonl(f'{tmp_path}/b-docs.jsonl', [
        {'key': 'u1', 'doc': 'o1', 'session': 'a', 'ts': 1, 'kind': 'decision', 'text': 'decision: cache\nkeep it on disk'},
        {'key': 'u2', 'doc': 'o2', 'session': 'a', 'ts': 1, 'kind': 'decision', 'text': 'decision: other\nnothing alike'},
        {'key': 'r:d:1', 'doc': 'r:d:1', 'session': 's2', 'ts': 1, 'kind': 'prompt', 'text': 'x'}])
    v1_text = {'o1': 'cache\nkeep it on disk', 'o2': 'a different note entirely'}.get
    assert m4.matches(str(tmp_path), v1_text) == {'o': (1, 2)}


def test_the_gate_lists_hits_it_cannot_read_instead_of_stopping(ev, monkeypatch, capsys):
    import freeze, judge
    monkeypatch.setattr(freeze, 'check', lambda: [])
    monkeypatch.setattr(judge, 'E', str(ev))
    runs, home = ev / 'runs', ev / 'b'
    for key, value in (('OBOETE_EVAL_RUNS', str(runs)), ('OBOETE_EVAL_DEPTH', '50'),
                       ('OBOETE_EVAL_QUESTIONS', f'{ev}/questions-test-m4.jsonl')):
        monkeypatch.setenv(key, value)  # gate() sets them too: these restore them afterwards
    monkeypatch.setattr(judge, 'RUNS', str(runs))
    db = v1_db(ev)
    db.executescript("CREATE TABLE observations(id INTEGER PRIMARY KEY, title TEXT, body TEXT, session_id TEXT);"
                     "CREATE TABLE summaries(id INTEGER PRIMARY KEY, body TEXT, session_id TEXT);"
                     "CREATE TABLE prompts(id INTEGER PRIMARY KEY, body TEXT, session_id TEXT);"
                     "INSERT INTO sessions VALUES ('s-p1', 'claude', 1790000000000);"
                     "INSERT INTO prompts VALUES (1, 'x', 's-p1');"
                     "INSERT INTO observations VALUES (1, 'title', 'body', 's-p1');")
    db.commit()
    write_jsonl(f'{ev}/questions-test-m4.jsonl', [q('p1')])
    (ev / 'corpus-m4.json').write_text(json.dumps({'window': {'claude': '2026-06-19'}, 'sessions': []}))
    # p1 counts on Raw's N; its session is a held-out one, which the corpus leaves out.
    (ev / 'replay').mkdir()
    (ev / 'replay' / 'manifest.json').write_text(json.dumps({'sessions': [{'session': 's-p1', 'side': 'held-out'}]}))
    runs.mkdir()
    # o+1 reads o1, of the question's own session; the other two the judge cannot read at all.
    hits = ['o+1', 'claude-mem:db:o5', 'r:d:9']
    (runs / 'b-off.trec').write_text(''.join(f'p1 Q0 {d} {i} 1 b-off\n' for i, d in enumerate(hits, 1)))
    write_jsonl(f'{runs}/b-docs.jsonl', [{'key': 'u1', 'session': 's', 'ts': 1, 'kind': 'decision', 'text': 'x'}])
    (runs / 'stores.json').write_text(json.dumps({'started': 2 ** 40, 'before': {}, 'after': {}}))
    home.mkdir()
    sqlite3.connect(home / 'knowledge.db').executescript(
        'CREATE TABLE raw_docs(session TEXT); CREATE TABLE vector_todo(x); CREATE TABLE vector_keys(skipped TEXT);')
    sqlite3.connect(home / 'providers.db').executescript('CREATE TABLE provider_calls(role TEXT, ts INTEGER);')
    problems = m4.gate(str(runs), str(home), 'HEAD', '2026-10-01T00:00:00+09:00', rerank=False)
    for line in ('b-off: unsupported id o+1', 'b-off: unmapped claude-mem:db:o5', 'b-off: no sidecar row for r:d:9',
                 'b-docs.jsonl: no doc id for u1'):
        assert line in problems
    # A held-out session's question is named apart, not a problem.
    assert "of held-out sessions (no records): 1 ['p1']" in capsys.readouterr().out
    assert not [p for p in problems if p.startswith('p1:')]


def test_holm_adjusts_step_down():
    assert m4.holm([0.04, 0.01, 0.03]) == pytest.approx([0.06, 0.03, 0.06])
    assert m4.holm([]) == []
    assert m4.holm([0.7, 0.7]) == [1.0, 1.0]


def test_the_english_gap_has_a_welch_interval():
    import math

    en = [0.51 - 0.19 * math.sqrt(59 / 60), 0.51 + 0.19 * math.sqrt(59 / 60)] * 30
    ja = [0.5 - 0.19, 0.5 + 0.19] * 51 + [0.5]
    diff, low, high = m4.gap(en, ja)
    assert diff == pytest.approx(0.01)
    assert 0.055 < (high - low) / 2 < 0.065
    assert (high + low) / 2 == pytest.approx(diff)
    assert m4.gap([0.6] * 3, [0.5] * 5) == pytest.approx((0.1, 0.1, 0.1))
    # Unequal spread and N: Welch's interval, not a pooled or a z one.
    from scipy.stats import ttest_ind
    en, ja = [0.2, 0.9, 0.4, 0.7, 0.1], [0.5, 0.52, 0.48, 0.51, 0.49, 0.5, 0.53, 0.47]
    ci = ttest_ind(en, ja, equal_var=False).confidence_interval(0.95)
    assert m4.gap(en, ja)[1:] == pytest.approx((ci.low, ci.high))
    with pytest.raises(ValueError, match='two questions'):
        m4.gap([0.5], [0.5, 0.6])


def test_cross_lingual_tables_pick_by_language_not_grade(report_module):
    report = report_module

    queries = {name: q(name, lang=lang) for name, lang in (('ja', 'ja'), ('en', 'en'))}
    judged = {name: {'o107': 3, 'o108': 2, 'o109': 1, 'o110': 1} for name in queries}
    runs = {'b-off': {name: list(judged[name]) for name in queries}}
    shown = {'o107': '日本語の記録', 'o108': 'English note', 'o109': 'related English', 'o110': '関連する記録'}
    views = report.tables(runs, judged, queries, {}, 0, shown, {}, m4_mode=True)
    ja_qrels, ja_runs = views['Japanese questions, documents without Japanese characters']
    en_qrels, en_runs = views['English questions, documents with Japanese characters']
    assert ja_qrels == {'ja': {'o108': 2, 'o109': 1}}
    assert en_qrels == {'en': {'o107': 3, 'o110': 1}}
    assert ja_runs == {'b-off': {'ja': ['o108', 'o109']}}
    assert en_runs == {'b-off': {'en': ['o107', 'o110']}}


def test_r_keys_and_the_132_leave_runs_and_qrels(report_module):
    report = report_module

    own = {f'{kind}{i}' for kind, end in (('o', 106), ('s', 14), ('p', 12)) for i in range(1, end + 1)}
    assert len(own) == 132
    queries = {name: q(name) for name in ('imported', 'raw-only', 'v1-only')}
    judged = {'imported': {'o1': 3, 's1': 3, 'p1': 3, 'r:d:1': 3, 'o107': 2},
              'raw-only': {'r:d:1': 3}, 'v1-only': {'o1': 3}}
    runs = {name: {qid: list(grades) for qid, grades in judged.items()} for name in m4.RUNS}
    grades, filtered = report.table_data(runs, judged, queries, own=own)
    assert grades == {'imported': {'o107': 2}}
    assert all(per == {'imported': ['o107']} for per in filtered.values())
    raw_grades, raw_runs = report.table_data(runs, judged, queries, own=own, raw=True)
    assert raw_grades == {'imported': {'r:d:1': 3, 'o107': 2}, 'raw-only': {'r:d:1': 3}}
    assert all(per == {'imported': ['r:d:1', 'o107'], 'raw-only': ['r:d:1']} for per in raw_runs.values())
    assert runs['b-off']['imported'] == ['o1', 's1', 'p1', 'r:d:1', 'o107']


def test_a_slice_drop_is_named_and_a_small_slice_counted(capsys, report_module):
    report = report_module

    queries = {f'q{i}': q(f'q{i}', lang='ja' if i < 5 else 'en') for i in range(9)}
    judged = {qid: {'o107': 3} for qid in queries}
    runs = {name: {qid: ['o107'] for qid in queries} for name in m4.RUNS if name != 'b-rerank'}
    runs['b-off'] = {qid: [] if queries[qid]['lang'] == 'ja' else ['o107'] for qid in queries}
    report.m4_report(runs, judged, queries, set(), set(), {}, 0, {'o107': 'a memory'}, {})
    out = capsys.readouterr().out
    assert 'FAIL b-off [question in Japanese] recall@10' in out
    assert 'question in English: 4 questions with an answer' in out
    assert 'b-off [question in English]' not in out
    assert 'Holm family: m=3' in out
    assert 'NO-GO b-rerank: absent' in out
    assert [line for line in out.splitlines() if line.startswith('FAIL b-off [all]') and 'FLOOR=' in line]
    # The Raw runs, which hold the eligible questions only, are in no other table.
    assert '\nb-rrf5 ' not in out.split('## Holm family')[0]


def test_the_english_gap_line_is_one_sided(capsys, report_module):
    report = report_module

    queries = {f'q{i}': q(f'q{i}', lang='ja' if i < 6 else 'en') for i in range(12)}
    judged = {qid: {'o107': 3, 'o108': 0} for qid in queries}
    for better in ('en', 'ja'):
        runs = {name: {qid: ['o108', 'o107'] for qid in queries} for name in m4.RUNS}
        runs['b-off'] = {qid: ['o107', 'o108'] if queries[qid]['lang'] == better else ['o108', 'o107']
                         for qid in queries}
        report.m4_report(runs, judged, queries, set(), set(), {}, 0, {'o107': 'a memory', 'o108': 'x'}, {})
        out = capsys.readouterr().out
        assert [line for line in out.splitlines() if line.startswith('PASS b-off [all]') and 'FLOOR=' in line]
        gap = out.split('English-minus-Japanese gap')[1]
        assert ('PASS b-off' if better == 'en' else 'FAIL b-off') in gap
        assert 'hybrid-d2: English N=6' in gap and '(holds no line)' in gap


def test_report_import_does_not_open_the_evaluation_store(monkeypatch, report_module):
    import builtins, importlib

    def forbidden(*args, **kwargs):
        raise AssertionError('import must not access evaluation files or databases')

    with monkeypatch.context() as guard:
        guard.setattr(builtins, 'open', forbidden)
        guard.setattr(sqlite3, 'connect', forbidden)
        guard.setattr(os, 'listdir', forbidden)
        guard.setattr(os.path, 'getmtime', forbidden)
        importlib.reload(report_module)


def test_paired_statistics_and_detectable_difference(report_module):
    import math
    from scipy.stats import nct, t

    report = report_module
    result = report.paired([0.1, 0.2, 0.3, 0.4], [0.0] * 4)
    assert result['diff'] == pytest.approx(0.25)
    assert result['sd'] == pytest.approx(0.1290994449)
    assert result['p'] == pytest.approx(0.03046629166)
    assert (result['low'], result['high']) == pytest.approx((0.04457397433, 0.45542602567))
    assert report.paired([0.5] * 5, [0.5] * 5)['p'] == 1.0
    assert report.paired([0.6] * 5, [0.5] * 5)['p'] == 0.0
    assert report.paired([], [])['p'] == 1.0
    assert math.isnan(report.detectable(1, 0.19))
    detectable = report.detectable(25, 0.19)
    critical = t.ppf(0.975, 24)
    noncentrality = detectable * 5 / 0.19
    assert nct.sf(critical, 24, noncentrality) + nct.cdf(-critical, 24, noncentrality) == pytest.approx(0.8)
    assert 0.11 < detectable < 0.112


def test_raw_uses_only_eligible_questions_and_near_copy_is_reported_only(capsys, report_module):
    report = report_module

    queries = {f'q{i}': dict(q(f'q{i}', lang='ja' if i % 10 < 5 else 'en'),
                             text='Where is the persisted record kept?') for i in range(20)}
    eligible = {f'q{i}' for i in range(10)}
    judged = {qid: {'o107': 3, 'o108': 0, **({'r:d:1': 3} if qid in eligible else {})} for qid in queries}
    runs = {name: {qid: ['o108', 'o107'] for qid in queries} for name in m4.RUNS}
    runs['b-rerank'] = {qid: ['o107'] for qid in queries}
    runs['claude-mem'] = runs['claude-mem-nowindow'] = runs['b-rerank']
    runs['b-rrf5'] = {qid: ['r:d:1', 'o107'] for qid in eligible}
    runs['b-only'] = {qid: ['r:d:1'] for qid in eligible}
    shown = {'o107': 'A useful memory', 'o108': 'unrelated', 'r:d:1': queries['q0']['text']}
    now = 200 * report.DAY
    asked = {qid: now - (89 if qid in eligible else 90) * report.DAY for qid in queries}
    report.m4_report(runs, judged, queries, set(), eligible, asked, now, shown, {})
    out = capsys.readouterr().out
    primary, clean = out.split('## Raw, near-copy records removed:')
    assert 'Holm family: m=4' in primary
    assert 'b-rerank against b-off: N=20' in primary
    assert 'b-rrf5 against b-off: N=10' in primary
    assert '\nGO b-rerank:' in primary
    assert 'Raw N = 10 eligible questions, 10 with an answer, 0 without' in primary
    assert '\nGO b-rrf5:' in primary and '\nGO b-only:' in primary
    assert 'Raw [prompts typed 90 days ago or earlier]: 0 questions with an answer (holds no line)' in primary
    assert 'near-copy records in Raw pools: 1 unique, 10 question-record pairs' in primary
    assert '(reported only)' in clean and '\nNO-GO b-only:' in clean
    assert 'paired SD=' in primary and 'detectable difference=' in primary


def test_dev_tables_keep_the_existing_slices_and_documents(report_module):
    report = report_module

    queries = {'agent': dict(q('agent'), set='agent'), 'prompt': q('prompt')}
    judged = {qid: {'o1': 3, 'r:d:1': 3} for qid in queries}
    runs = {'dev': {qid: list(judged[qid]) for qid in queries}}
    views = report.tables(runs, judged, queries, {}, 0, {'o1': 'x', 'r:d:1': 'y'}, {})
    assert len(views) == 10
    assert views['all'] == (judged, runs)
    assert views['agent searches'][0] == {'agent': judged['agent']}


@pytest.fixture
def report_module(monkeypatch):
    import builtins, importlib
    importlib.import_module('ranx')

    def forbidden(*args, **kwargs):
        raise AssertionError('import must not access evaluation files or databases')

    # Load the dependency first, then guard the report's first import as well as its reload.
    with monkeypatch.context() as guard:
        guard.setattr(builtins, 'open', forbidden)
        guard.setattr(sqlite3, 'connect', forbidden)
        guard.setattr(os, 'listdir', forbidden)
        guard.setattr(os.path, 'getmtime', forbidden)
        return importlib.import_module('report')


def test_m4_reports_the_pinned_judge_only(monkeypatch, report_module):
    """D10: the deciding report reads the calibrated judge's grades, never another's (Codex's
    review of #319)."""
    report = report_module
    monkeypatch.setattr(report.J, 'POOL_DEPTH', m4.DEPTH)
    monkeypatch.setattr(sys, 'argv', ['report.py', 'test', 'other-model', '--m4'])
    with pytest.raises(SystemExit, match='pinned judge'):
        report.main()


def test_go_reads_the_holm_p_and_lines_unrounded(capsys, monkeypatch, report_module):
    """A family whose smallest p, 0.02, sits beside 0.4, 0.5 and 0.6 holds no GO after Holm (0.08);
    a gain printed as +0.030000 but below +0.03 holds none either (Codex's review of #319)."""
    report = report_module
    queries = {f'q{i}': q(f'q{i}') for i in range(6)}
    judged = {qid: {'o107': 3} for qid in queries}
    runs = {name: {qid: ['o107'] for qid in queries} for name in m4.RUNS}
    for gain, p, why in ((0.05, 0.02, 'Holm p >= 0.05'), (0.0299996, 1e-9, 'difference below +0.03')):
        ps = {'b-off': 0.4, 'b-rerank': p, 'b-rrf5': 0.5, 'b-only': 0.6}
        monkeypatch.setattr(report, 'contrast', lambda rs, name, base, ps=ps, gain=gain: {
            'n': 6, 'diff': gain if name == 'b-rerank' else 0.0, 'sd': 0.1, 'low': 0.0, 'high': 0.0,
            'p': ps[name]})
        report.m4_report(runs, judged, queries, set(), set(queries), {}, 0, {'o107': 'y'}, {})
        out = capsys.readouterr().out
        line = next(l for l in out.split('\n') if l.startswith(('GO b-rerank', 'NO-GO b-rerank')))
        assert line.startswith('NO-GO b-rerank') and why in line, line


def test_the_132_leave_every_table(capsys, report_module):
    """A v1-owned document is relevant but no system can return it: it leaves the qrels too."""
    report = report_module

    queries = {f'q{i}': q(f'q{i}') for i in range(6)}
    judged = {qid: {'o1': 3, 'o107': 3} for qid in queries}
    runs = {name: {qid: ['o107'] for qid in queries} for name in m4.RUNS}
    report.m4_report(runs, judged, queries, {'o1'}, set(queries), {}, 0, {'o1': 'x', 'o107': 'y'}, {})
    primary, clean = capsys.readouterr().out.split('## Raw, near-copy records removed:')
    assert '\nb-off 1.000000 ' in primary
    assert 'b-rrf5: ndcg@10=1.000000' in primary and 'b-rrf5: ndcg@10=1.000000' in clean


def test_milestone_4_test_runs_need_m4_and_dev_runs_keep_the_plain_report(tmp_path, monkeypatch, report_module):
    report = report_module
    (tmp_path / 'b-off.trec').write_text('')
    monkeypatch.setattr(report.J, 'RUNS', str(tmp_path))

    def past_the_gate(*args, **kwargs):
        raise RuntimeError('past the gate')

    monkeypatch.setattr(report.sqlite3, 'connect', past_the_gate)
    monkeypatch.setattr(sys, 'argv', ['report.py', 'test'])
    with pytest.raises(SystemExit, match='add --m4'):
        report.main()
    monkeypatch.setattr(sys, 'argv', ['report.py', 'dev'])
    with pytest.raises(RuntimeError, match='past the gate'):
        report.main()
