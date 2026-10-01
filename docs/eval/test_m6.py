import json, os, sys
from pathlib import Path

import pytest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))


def test_an_answer_counts_only_when_correct_and_citing_a_live_span_that_holds_it():
    import m6
    live = {'live': True, 'holds': True}
    assert m6.counted(True, [live])
    assert not m6.counted(False, [live])
    assert not m6.counted(None, [live])
    assert not m6.counted(True, [{'live': False, 'holds': True}])
    assert not m6.counted(True, [{'live': True, 'holds': False}])
    assert not m6.counted(True, [])
    assert not m6.counted(True, [live, {'live': True, 'holds': None}])


def graded(n=40, correct=28):
    return [{'id': f'q{i}', 'dated': i < 8, 'correct': i < correct,
             'hits': [{'id': 'c', 'label': 'citable',
                       'spans': [{'live': True, 'holds': True}]}]}
            for i in range(n)]


def test_the_line_is_counted_not_floated():
    import m6
    assert m6.report(graded(), against=24)['pass']
    assert not m6.report(graded(correct=27), against=24)['pass']
    assert not m6.report(graded(correct=27), against=20)['pass']
    assert not m6.report(graded(), against=25)['pass']
    rows = graded()
    for r in rows[-3:]:
        r['hits'][0]['spans'][0]['live'] = False
    assert not m6.report(rows, against=20)['pass']
    rows = graded()
    for r in rows[-2:]:
        r['hits'][0]['spans'][0]['live'] = False
    assert m6.report(rows, against=24)['pass']  # exactly 38/40, not rounded


def test_validity_is_over_citable_hits_only():
    import m6
    rows = graded(correct=40)
    rows[0]['hits'] += [{'id': 'q', 'label': 'quote-only', 'spans': [{'live': False, 'holds': False}]},
                        {'id': 'i', 'label': 'imported', 'spans': [{'live': False, 'holds': False}]}]
    rows[1]['hits'] = [{'id': 'o1', 'label': 'attributed-only', 'spans': []}]
    out = m6.report(rows, against=24)
    assert out['validity'] == {'n': 39, 'valid': 39, 'rate': 1.0, 'line': 0.95}
    assert out['count'] == 39 and out['attributed_only'] == 1


def test_an_unlabelled_imported_hit_fails_the_run():
    import m6
    rows = graded(correct=40)
    rows[0]['unlabelled_imported'] = 1
    hit = m6.hit_lines('claude-mem:o12 2026-09-01 10:00 UTC change — text\n', 'b-cmem')[0]
    assert hit['unlabelled_imported'] == 1
    hit = m6.hit_lines('claude-mem:o12 2026-09-01 10:00 UTC change (imported) — text\n', 'b-cmem')[0]
    assert hit['unlabelled_imported'] == 0
    out = m6.report(rows, against=24)
    assert out['unlabelled_imported'] == 1 and not out['pass']
    assert not m6.report(rows, arm='b-cmem')['pass']


def test_the_dated_subset_has_its_own_n_and_line():
    import m6
    rows = graded(correct=40)
    rows[6]['correct'] = rows[7]['correct'] = False
    out = m6.report(rows, against=24)
    assert out['dated'] == {'n': 8, 'count': 6, 'rate': 0.75, 'line': 0.70, 'pass': True}
    assert out['pass']
    rows[5]['correct'] = False
    out = m6.report(rows, against=24)
    assert out['count'] == 37 and out['dated']['rate'] == 0.625 and not out['pass']


def test_the_v1_arm_counts_the_dates_it_drops(monkeypatch, tmp_path):
    import common, m6, subprocess
    calls = []
    monkeypatch.setenv('PANEL_API_KEY', 'never-in-child')

    def fake(argv, **kw):
        calls.append((argv, kw))
        return subprocess.CompletedProcess(argv, 0, 'p7 2026-09-03 10:00 prompt text\n', '')

    monkeypatch.setattr(common.subprocess, 'run', fake)
    query = {'query': '-search', 'since': '2026-09-01', 'until': '2026-09-04'}
    result, dropped = m6.search('fake', str(tmp_path), query, 'v1')
    assert result.startswith('p7') and dropped == {'since': 1, 'until': 1, 'questions': 1}
    assert calls[0][0] == ['fake', '--home', str(tmp_path), 'search', '--all', '--limit', '10', '--', '-search']
    assert not any('KEY' in k.upper() for k in calls[0][1]['env'])
    rows = graded()
    rows[0]['dropped_dates'] = dropped
    rows[1]['dropped_dates'] = {'since': 1, 'until': 0, 'questions': 1}
    assert m6.report(rows, arm='v1')['dropped_dates'] == {'since': 2, 'until': 1, 'questions': 2}


def test_a_held_out_session_is_refused_without_the_recorded_id(monkeypatch, tmp_path):
    import common, m6
    # Refuse before even opening the held-out questions file.
    for decide in (None, 'curator', 'another'):
        with pytest.raises(SystemExit):
            common.guard([], decide, 'test')
        with pytest.raises(SystemExit):
            m6.questions(pool='test', decide=decide)
    os.makedirs(common.E, exist_ok=True)
    Path(common.E, 'deciding.json').write_text('{"curator": "curator"}')
    with pytest.raises(SystemExit):
        m6.questions(pool='test', decide='another')
    common.guard([], decide='curator', pool='test')
    Path(common.E, 'deciding.json').write_text('{"curator": "another"}')
    with pytest.raises(SystemExit):
        m6.questions(pool='test', decide='curator')


def test_the_prompts_are_the_protocols():
    import m6, re
    section = Path(__file__).resolve().parents[1].joinpath('milestone-4.md').read_text().split(
        "## Task 12a: the dev harnesses' protocol (D12)", 1)[1]
    for name in ('KEY', 'CHECK', 'QUERY', 'ANSWER', 'CORRECT', 'HOLDS'):
        text = re.search(rf'^{name}:\n\n```text\n(.*?)```', section, re.M | re.S)[1]
        assert getattr(m6, name) == text


def draft_pool(monkeypatch, tmp_path):
    import common, m3
    monkeypatch.setattr(m3, 'M', str(tmp_path / 'm3'))
    Path(common.E, 'replay').mkdir(parents=True)
    Path(m3.M, 'fixtures').mkdir(parents=True)
    manifest = [{'session': f's{i}', 'side': 'dev'} for i in range(30)]
    Path(common.E, 'replay/manifest.json').write_text(json.dumps({'sessions': manifest}))
    for s in manifest:
        common.write_jsonl(f'{m3.M}/fixtures/{s["session"]}.jsonl', [
            {'session': s['session'], 'agent': 'claude', 'event': 'UserPromptSubmit',
             'ts': '2026-09-01T10:00:00Z', 'payload': {'prompt': 'fixture answer\u2028still one line'}},
            {'session': s['session'], 'agent': 'claude', 'event': 'Stop',
             'ts': '2026-09-02T10:00:00Z', 'payload': {'last_assistant_message': 'fixture reply'}}])
    qs = [{'id': f'q{i}', 'session': f's{i // 2}', 'record': i % 2,
           'question': f'question {i}', 'asked_at': '2026-09-03', 'dated': i < 8} for i in range(40)]
    common.write_jsonl(f'{common.E}/m6/questions-dev.jsonl', qs)
    common.write_jsonl(f'{common.E}/m6/keys-dev.jsonl', [
        {'id': q['id'], 'answer': 'KEY-MUST-NOT-REACH-ANSWERER', 'records': [{'device': 'd', 'seq': 1}]}
        for q in qs])
    binary = tmp_path / 'fake-oboete'
    binary.write_text('a fake binary, subprocess is injected')
    home = tmp_path / 'source-home'
    home.mkdir()
    import sqlite3
    with sqlite3.connect(str(home / 'oboete.db')) as db:
        db.execute('CREATE TABLE sessions(id)')
        db.executemany('INSERT INTO sessions VALUES(?)', [(s['session'],) for s in manifest])
    return qs, str(binary), str(home)


def test_a_stopped_run_resumes_without_asking_again(monkeypatch, tmp_path):
    import common, m6, subprocess
    qs, binary, home = draft_pool(monkeypatch, tmp_path)
    prompts, opened = [], []
    stop = [True]
    original = Path(home, 'oboete.db').read_bytes()

    def answer(prompt, model):
        assert model == 'claude-sonnet-5'
        assert 'KEY-MUST-NOT-REACH-ANSWERER' not in prompt
        prompts.append(prompt)
        if 'Write the search' in prompt:
            return '{"query": "needle", "since": null, "until": null}'
        return '{"answer": "answer", "cites": ["p7"]}'

    def fake(argv, **kw):
        if argv[-1] == 'gate':
            return subprocess.CompletedProcess(argv, 0, kw['input'], '')
        opened.append(argv[2])
        assert argv[2] != home
        assert Path(argv[2], 'oboete.db').read_bytes() == original
        if 'search' in argv and opened.count(argv[2]) == 3 and stop[0]:
            stop[0] = False
            raise KeyboardInterrupt
        return subprocess.CompletedProcess(argv, 0, 'p7 2026-09-01 10:00 prompt needle\n', '')

    monkeypatch.setattr(common, 'claude_json', answer)
    monkeypatch.setattr(common.subprocess, 'run', fake)
    with pytest.raises(KeyboardInterrupt):
        m6.run(binary, home, 'v1')
    assert len(prompts) == 3  # first QUERY/ANSWER and second QUERY were kept
    assert len(common.read_jsonl(f'{common.E}/m6/runs/v1.jsonl')) == 1
    m6.run(binary, home, 'v1')
    assert len(prompts) == 80
    assert len(common.read_jsonl(f'{common.E}/m6/runs/v1.jsonl')) == 40
    assert len(set(opened)) == 2  # a fresh copy for each invocation
    assert Path(home, 'oboete.db').read_bytes() == original
    m6.run(binary, home, 'v1')
    assert len(prompts) == 80
    assert json.loads(Path(common.E, 'm6/runs/v1.json').read_text())['models'] == {
        'claude-sonnet-5': ['claude-sonnet-5']}


def test_questions_check_every_dev_event_and_preserve_json_line_numbers(monkeypatch, tmp_path):
    import common, m3, m6
    qs, _, _ = draft_pool(monkeypatch, tmp_path)
    assert m6.questions() == qs
    path = Path(m3.M, 'fixtures/s0.jsonl')
    text = path.read_text()
    path.write_text('\n' + text)
    with pytest.raises(SystemExit):
        m6.questions()  # record 0 is now a blank physical line
    path.write_text(text)
    late = Path(m3.M, 'fixtures/s29.jsonl')
    late.write_text(late.read_text().replace('2026-09-02', '2026-09-04'))
    with pytest.raises(SystemExit):
        m6.questions()  # last event is in a session with no question


def test_keys_use_the_draw_and_map_plain_records_before_a_worker(monkeypatch, tmp_path):
    import calib, common, m3, m6, sqlite3, subprocess
    qs, binary, home = draft_pool(monkeypatch, tmp_path)
    raw = sqlite3.connect(f'{home}/raw.db')
    raw.execute('CREATE TABLE records(device,seq,ts,kind,enc,body,type,session)')
    expected, calls = {}, []
    for i in range(30):
        for line, ev in m6.fixture(f's{i}').items():
            seq = i * 2 + line + 1
            kind = 'prompt' if line == 0 else 'reply'
            body = {'text': next(m3.strings(ev['payload']))}
            raw.execute('INSERT INTO records VALUES(?,?,?,?,?,?,?,?)',
                        ('d', seq, int(m3.when(ev['ts']) * 1000), kind, 'plain', json.dumps(body), 'event', f's{i}'))
            expected[f'q{i * 2 + line}'] = seq
    raw.commit()

    def panel(member, prompt):
        calls.append((member, prompt))
        import re
        qid = re.search(r'question (\d+)', prompt)[1]
        if prompt.startswith('You write'):
            return json.dumps({'answer': 'fixture answer', 'records': [int(qid) % 2]}), member + '-reported'
        return '{"agree": true}', member + '-reported'

    def gate(argv, **kw):
        assert argv == [binary, 'gate']
        return subprocess.CompletedProcess(argv, 0, kw['input'], '')

    monkeypatch.setattr(calib, 'chat', panel)
    monkeypatch.setattr(common.subprocess, 'run', gate)
    out = m6.keys(binary, home)
    assert out['n'] == 40 and out['rejected'] == 0
    written = common.read_jsonl(f'{common.E}/m6/keys-dev.jsonl')
    assert all(k['records'][0]['device'] == 'd' and k['records'][0]['seq'] == expected[k['id']] for k in written)
    writers = [m for m, p in calls if p.startswith('You write')]
    assert writers == [common.draw('m6-key', q['id']) for q in qs]
    # The checker: drawn by m6-checker from the other seven, on the m6-check draw's 20%.
    checkers = [m for m, p in calls if p.startswith('Here are a question')]
    assert checkers == [common.draw('m6-checker', q['id'], exclude=common.draw('m6-key', q['id']))
                        for q in qs if common.checked('m6-check', q['id'])]
    assert len(calls) == 40 + sum(common.checked('m6-check', q['id']) for q in qs)
    assert Path(common.E, 'm6/keys-dev.jsonl').stat().st_mode & 0o777 == 0o600
    raw.execute("UPDATE records SET enc = 'zstd' WHERE seq = 1")
    raw.commit()
    with pytest.raises(SystemExit):
        m6.keys(binary, home)
    assert len(calls) == 40 + sum(common.checked('m6-check', q['id']) for q in qs)


def test_score_resolves_full_claim_uids_and_waits_for_every_grader(monkeypatch, tmp_path):
    import calib, common, m6, sqlite3, subprocess
    home = tmp_path / 'home'
    home.mkdir()
    uid = 'a1' * 32
    knowledge = sqlite3.connect(str(home / 'knowledge.db'))
    knowledge.execute('CREATE TABLE active(uid)')
    knowledge.execute('INSERT INTO active VALUES(?)', (uid,))
    knowledge.commit()
    seen, fail = [], [True]

    def fake(argv, **kw):
        if argv[-1] == 'gate':
            return subprocess.CompletedProcess(argv, 0, kw['input'], '')
        assert argv == ['fake', '--home', str(home), 'cite', uid]
        return subprocess.CompletedProcess(argv, 0, json.dumps([{'uid': uid, 'label': 'citable', 'evidence': [
            {'live': True, 'quote': 'passage', 'device': 'd', 'seq': 1, 'offset': 0, 'length': 7},
            {'live': False, 'quote': '[redacted]', 'device': 'd', 'seq': 2, 'offset': 0, 'length': 3}]}]), '')

    def panel(member, prompt):
        seen.append((member, prompt))
        correct = 'Candidate answer:' in prompt
        if correct and member == common.GRADERS[-1] and fail[0]:
            fail[0] = False
            raise ConnectionError('do not print this private reply')
        return json.dumps({'correct' if correct else 'holds': True}), member + '-version'

    monkeypatch.setattr(common.subprocess, 'run', fake)
    monkeypatch.setattr(calib, 'chat', panel)
    row = {'id': 'q', 'question': 'question', 'dated': True, 'answer': 'answer', 'cites': [uid[:12]],
           'hits': [{'id': uid[:12], 'label': 'citable'}], 'gets': {}, 'binary': 'fake', 'home': str(home)}
    key = {'answer': 'reference'}
    first = m6.score_answer(row, key, 'b', common.Calls())
    assert first['correct'] is None and not first['complete']
    assert len(seen) == 6 and not any('[redacted]' in prompt for _, prompt in seen)
    again = m6.score_answer(row, key, 'b', common.Calls())
    assert again['correct'] and again['complete'] and len(seen) == 7
    out = m6.report([again], against=0)
    assert out['count'] == 1 and out['validity']['n'] == 2 and out['validity']['valid'] == 1


def test_full_claim_uids_from_get_are_valid_answer_citations(monkeypatch, tmp_path):
    import calib, common, m6, sqlite3, subprocess
    uid, home = 'a1' * 32, str(tmp_path)
    with sqlite3.connect(f'{home}/knowledge.db') as db:
        db.execute('CREATE TABLE active(uid)')
        db.execute('INSERT INTO active VALUES(?)', (uid,))

    def fake(argv, **kw):
        text = kw['input'] if argv[-1] == 'gate' else json.dumps([
            {'uid': uid, 'label': 'citable', 'evidence': [{'live': True, 'quote': 'quote'}]}])
        return subprocess.CompletedProcess(argv, 0, text, '')

    monkeypatch.setattr(common.subprocess, 'run', fake)
    monkeypatch.setattr(calib, 'chat', lambda m, p: (
        json.dumps({'correct' if 'Candidate answer:' in p else 'holds': True}), m))
    row = {'id': 'q', 'dated': False, 'question': 'q', 'answer': 'a', 'cites': [uid],
           'hits': [{'id': uid[:12], 'label': 'citable'}], 'gets': {uid[:12]: uid + ' full text'},
           'binary': 'fake', 'home': home}
    scored = m6.score_answer(row, {'answer': 'key'}, 'b', common.Calls())
    assert scored['complete'] and m6.report([scored], against=0)['count'] == 1


def test_dev_run_refuses_a_home_holding_a_held_out_session(monkeypatch, tmp_path):
    import common, m6, sqlite3
    _, binary, home = draft_pool(monkeypatch, tmp_path)
    manifest = Path(common.E, 'replay/manifest.json')
    data = json.loads(manifest.read_text())
    data['sessions'].append({'session': 'hidden', 'side': 'held-out'})
    manifest.write_text(json.dumps(data))
    with sqlite3.connect(f'{home}/raw.db') as db:
        db.execute('CREATE TABLE records(session)')
        db.execute("INSERT INTO records VALUES('hidden')")
    with pytest.raises(SystemExit, match='Held-out'):
        m6.run(binary, home, 'b')


def test_a_guarded_pool_uses_the_manifests_held_out_side(tmp_path):
    import common, m6
    Path(common.E, 'replay').mkdir(parents=True)
    Path(common.E, 'deciding.json').write_text('{"curator":"curator"}')
    Path(common.E, 'replay/manifest.json').write_text(json.dumps({'sessions': [
        {'session': 'd', 'side': 'dev'}, {'session': 'h', 'side': 'held-out'}]}))
    assert m6.pool_sessions('test', 'curator') == {'h'}


def test_the_full_text_home_uses_the_binarys_none_setting(tmp_path):
    import m6
    (tmp_path / 'config.toml').write_text('[embedding]\nprovider = "none"\n')
    assert m6.vector_side(str(tmp_path)) == 'off'


def test_key_writer_keeps_two_neighbor_records_around_blank_lines(monkeypatch, tmp_path):
    import common, m3, m6, subprocess
    monkeypatch.setattr(m3, 'M', str(tmp_path))
    (tmp_path / 'fixtures').mkdir()
    events = [json.dumps({'ts': '2026-09-01T10:00:00Z', 'payload': {'prompt': 'r' + str(i) + 'x' * 5000}})
              for i in range(5)]
    (tmp_path / 'fixtures/s.jsonl').write_text('\n'.join(events[:2] + ['', ''] + events[2:]) + '\n')
    monkeypatch.setattr(common.subprocess, 'run', lambda argv, **kw: subprocess.CompletedProcess(
        argv, 0, kw['input'].replace('x', 'g'), ''))
    shown = m6.key_context({'session': 's', 'record': 4}, 'fake')
    assert list(shown) == [0, 1, 4, 5, 6]
    assert all(len(text) == 4000 and 'x' not in text for text in shown.values())


def test_incomplete_grades_leave_the_entire_run_unscored():
    import m6
    rows = graded()
    rows[-1]['correct'] = None
    out = m6.report(rows, against=24)
    assert not out['complete'] and not out['pass'] and 'count' not in out and 'validity' not in out


def test_score_refuses_changed_keys_before_any_grader(monkeypatch, tmp_path):
    import common, m6
    qs, binary, home = draft_pool(monkeypatch, tmp_path)
    input_record = {'sha256': common.sha256_file(binary), 'source_home': home,
                    'questions_sha256': common.sha256_file(f'{common.E}/m6/questions-dev.jsonl'),
                    'keys_sha256': common.sha256_file(f'{common.E}/m6/keys-dev.jsonl')}
    rows = [dict(q, input=input_record, binary=binary, home=home) for q in qs]
    common.write_jsonl(f'{common.E}/m6/runs/v1.jsonl', rows)
    path = Path(common.E, 'm6/keys-dev.jsonl')
    path.write_text(path.read_text().replace('KEY-MUST-NOT-REACH-ANSWERER', 'CHANGED-KEY'))
    with pytest.raises(SystemExit, match='inputs changed'):
        m6.score('v1')


def test_the_v1_metadata_guard_cannot_write_to_its_source(tmp_path):
    import common, m6, sqlite3
    home = str(tmp_path)
    with sqlite3.connect(f'{home}/oboete.db') as db:
        db.execute('PRAGMA journal_mode=WAL')
        db.execute('CREATE TABLE sessions(id)')
    db.close()  # the frozen baseline is checkpointed and has no live writer
    before = {p.name: p.read_bytes() for p in tmp_path.iterdir()}
    m6.guard_home(home, 'v1')
    assert {p.name: p.read_bytes() for p in tmp_path.iterdir()} == before


def test_invalid_query_dates_are_rejected_for_model_retry():
    import m6
    assert not m6.valid_query({'query': 'q', 'since': '2026-02-30', 'until': None})
    assert not m6.valid_query({'query': 'q', 'since': '2026-09-03', 'until': '2026-09-01'})
    assert m6.valid_query({'query': 'q', 'since': '2026-02-28', 'until': None})
