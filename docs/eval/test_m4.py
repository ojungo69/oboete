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
    assert m4.recordless({'q1', 'q2'}, asked, str(tmp_path)) == ['q1: no record of its session']
    # B's text against v1's for the mapped documents: one matches, one is another document.
    write_jsonl(f'{tmp_path}/b-docs.jsonl', [
        {'key': 'u1', 'doc': 'o1', 'session': 'a', 'ts': 1, 'kind': 'decision', 'text': 'decision: cache\nkeep it on disk'},
        {'key': 'u2', 'doc': 'o2', 'session': 'a', 'ts': 1, 'kind': 'decision', 'text': 'decision: other\nnothing alike'},
        {'key': 'r:d:1', 'doc': 'r:d:1', 'session': 's2', 'ts': 1, 'kind': 'prompt', 'text': 'x'}])
    v1_text = {'o1': 'cache\nkeep it on disk', 'o2': 'a different note entirely'}.get
    assert m4.matches(str(tmp_path), v1_text) == {'o': (1, 2)}
