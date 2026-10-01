import os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from m3 import its, shares


def test_a_claim_is_the_decision_whose_quote_it_shares():
    # One prompt, two decisions (d105, d106): each claim is the one its quote shares text with.
    both = 'じゃあCCSを完全削除して。あと、fccをxai oauthに対応させたい。'
    assert shares('CCSを完全削除して', 'じゃあCCSを完全削除して。')
    assert not shares('fccをxai oauthに対応させたい', 'じゃあCCSを完全削除して。')
    assert shares(both, 'あと、fccをxai oauthに対応させたい。')
    # Whitespace aside, and the whole of a quote shorter than the stretch.
    assert shares('tabs in every file', 'tabs\nin every  file of it')
    assert shares('１', '１')
    assert not shares('１', '２')
    # An empty quote is no one's.
    assert not shares('', 'じゃあCCSを完全削除して。')
    assert not shares(' ', 'x')


def test_a_record_of_two_decisions_gives_each_its_own_claims():
    # d105 and d106 share one prompt: a claim is the decision whose quote it shares, and one that
    # shares neither is both decisions' claim, as a record's claims all were before.
    quote_of = {'d105': 'じゃあCCSを完全削除して。', 'd106': 'あと、fccをxai oauthに対応させたい。'}
    on = ['d105', 'd106']
    assert its('d105', ['CCSを完全削除して'], on, quote_of)
    assert not its('d106', ['CCSを完全削除して'], on, quote_of)
    assert its('d106', ['fccをxai oauthに対応させたい'], on, quote_of)
    assert not its('d105', ['fccをxai oauthに対応させたい'], on, quote_of)
    assert its('d105', ['やって'], on, quote_of) and its('d106', ['やって'], on, quote_of)
    # Alone on its record, any claim there is the item's. The other end of an accepted proposal
    # joins the record's items: a quote of the item there is its own.
    assert its('d105', ['x'], ['d105'], quote_of)
    other_end = quote_of | {'d107': '１'}
    assert not its('d107', ['CCSを完全削除して'], ['d105'], other_end)
    assert its('d107', ['１'], ['d105'], other_end)


def test_dev_only_replay_selects_exactly_the_manifests_dev_sessions(monkeypatch, tmp_path):
    import json, subprocess
    import common, m3
    root = tmp_path / 'eval'
    (root / 'replay').mkdir(parents=True)
    (root / 'm3' / 'fixtures').mkdir(parents=True)
    monkeypatch.setattr(common, 'E', str(root))
    monkeypatch.setattr(m3, 'E', str(root))
    monkeypatch.setattr(m3, 'M', str(root / 'm3'))
    (root / 'replay' / 'manifest.json').write_text(json.dumps({'sessions': [
        {'session': 'dev-a', 'side': 'dev'}, {'session': 'dev-b', 'side': 'dev'},
        {'session': 'held-out', 'side': 'test'}]}))
    (root / 'labels' / 'drafts').mkdir(parents=True)
    (root / 'labels' / 'drafts' / 'extra.json').write_text('[{"session": "label-extra", "agent": "claude"}]')
    monkeypatch.setattr(m3, 'labels', lambda: ([{'session': 'label-extra'}], [], {}))
    assert m3.sessions(dev_only=True) == ['dev-a', 'dev-b']
    assert m3.sessions() == ['dev-a', 'dev-b', 'label-extra']
    for session, ts in [('dev-a', '2026-09-01T00:00:02Z'), ('dev-b', '2026-09-01T00:00:01Z'),
                        ('label-extra', '2026-09-01T00:00:03Z'), ('held-out', '2026-09-01T00:00:04Z')]:
        (root / 'm3' / 'fixtures' / f'{session}.jsonl').write_text(json.dumps(
            {'ts': ts, 'session': session, 'text': f'body of {session}\u2028still one event'},
            ensure_ascii=False) + '\n')
    seen = []
    def replay_part(argv, **kw):
        seen.extend(common.read_jsonl(argv[argv.index('replay') + 1]))
        return subprocess.CompletedProcess(argv, 0, '{}', '')
    monkeypatch.setattr(m3.subprocess, 'run', replay_part)
    monkeypatch.setattr(m3, 'sha256_file', lambda _: 'a' * 64)
    target = str(tmp_path / 'dev-home')
    assert m3.replay('test-binary', target, dev_only=True) == target
    assert [e['session'] for e in seen] == ['dev-b', 'dev-a']
    assert seen[0]['text'].endswith('\u2028still one event')
    report = json.loads((tmp_path / 'dev-home' / 'replay.json').read_text())
    assert report['sessions'] == 2 and report['events'] == 2
    assert 'curate = false' in (tmp_path / 'dev-home' / 'config.toml').read_text()


def _m3_home(tmp_path, monkeypatch):
    import sqlite3
    import common, m3
    root, home = tmp_path / 'eval', tmp_path / 'home'
    root.mkdir()
    home.mkdir()
    monkeypatch.setattr(common, 'E', str(root))
    monkeypatch.setattr(m3, 'E', str(root))
    monkeypatch.setattr(m3, 'M', str(root / 'm3'))
    binary = tmp_path / 'binary'
    binary.write_text('stub binary, never executed')
    db = sqlite3.connect(home / 'knowledge.db')
    db.executescript('''CREATE TABLE active(uid TEXT, kind TEXT, body TEXT, status TEXT, speaker TEXT);
        CREATE TABLE claims(uid TEXT, op_device TEXT, op_seq INTEGER);
        CREATE TABLE evidence(op_device TEXT, op_seq INTEGER, idx INTEGER, seq INTEGER, quote TEXT);
        CREATE TABLE edges(op_device TEXT, op_seq INTEGER, to_uid TEXT, type TEXT);''')
    return root, home, binary, db


def test_kinds_writes_the_label_format(monkeypatch, tmp_path):
    import json, stat, subprocess
    import calib, common, m3
    root, home, binary, db = _m3_home(tmp_path, monkeypatch)
    for i in range(18):
        uid = f'claim-{i:02}'
        kind = 'decision' if i < 17 else 'repo fact'
        db.execute('INSERT INTO active VALUES (?,?,?,?,?)', (uid, kind, f'text {i} SECRET', 'decided', 'user'))
        db.execute('INSERT INTO claims VALUES (?, ?, ?)', (uid, 'device', i))
        db.execute('INSERT INTO evidence VALUES (?, ?, 0, ?, ?)', ('device', i, i, f'quote {i}\u2028SECRET'))
    # An inactive derivation must not supply a sampled claim's quotes.
    db.execute('INSERT INTO evidence VALUES (?, ?, 0, 0, ?)', ('device', 999, 'STALE QUOTE'))
    db.commit()
    prompts, gates = [], []
    def run(argv, **kw):
        assert argv == [str(binary), 'gate']
        gates.append(kw['input'])
        return subprocess.CompletedProcess(argv, 0, kw['input'].replace('SECRET', '[redacted]'), '')
    def chat(model, prompt):
        prompts.append((model, prompt))
        assert 'SECRET' not in prompt and 'STALE QUOTE' not in prompt
        return json.dumps({'borne_out': True, 'kind_right': model != 'glm-5.3'}), f'reported-{model}'
    monkeypatch.setattr(common.subprocess, 'run', run)
    monkeypatch.setattr(calib, 'chat', chat)
    result = m3.kinds(str(binary), str(home))
    rows = common.read_jsonl(result['labels'])
    expected = sorted((f'claim-{i:02}' for i in range(17)), key=lambda uid: common.h(f'kinds:{common.SEED}:{uid}'))[:15]
    assert [r['uid'] for r in rows if r['kind'] == 'decision'] == expected
    assert len(rows) == 16 and all(set(r) == {'uid', 'kind', 'text', 'quotes'} for r in rows)
    assert rows[-1] == {'uid': 'claim-17', 'kind': 'repo fact', 'text': 'text 17 SECRET',
                        'quotes': ['quote 17\u2028SECRET']}
    assert result['per_kind']['decision'] == {'n': 15, 'correct': 15, 'precision': 1.0}
    assert result['complete'] and len(prompts) == 48
    assert result['agreement']['kind_right']['glm-5.3']['agreement'] == 0.0
    assert stat.S_IMODE(os.stat(result['labels']).st_mode) == 0o600
    assert gates and all('reported-' in m for values in result['models'].values() for m in values)
    # The same calls resume from owner-only cache, including the minority vote.
    assert m3.kinds(str(binary), str(home))['complete'] and len(prompts) == 48


def test_the_kind_prompt_is_the_protocols():
    import pathlib, re
    import m3
    protocol = (pathlib.Path(__file__).parents[1] / 'milestone-4.md').read_text()
    assert m3.KIND == re.search(r'^KIND:\n\n```text\n(.*?)```', protocol, re.M | re.S).group(1)
    assert m3.MEANINGS == {
        'decision': 'a choice the developer made',
        'preference': 'how the developer wants work done, beyond one task',
        'lesson': 'what to do or avoid, learned from a failure',
        'fix': 'how a problem was fixed: its symptom, cause and fix',
        'open item': 'work still to do',
        'repo fact': 'a fact about the repository or its tools',
        'change': 'what was changed'}


def test_overturned_counts_linked_pairs_and_reports_the_rest_apart(monkeypatch, tmp_path):
    import json, sqlite3, subprocess
    import common, m3
    root, home, binary, db = _m3_home(tmp_path, monkeypatch)
    (home / 'config.toml').write_text('[summary]\ncurate = false\n')
    raw = sqlite3.connect(home / 'raw.db')
    raw.execute('CREATE TABLE records(seq INTEGER, kind TEXT, session TEXT)')
    (root / 'replay').mkdir()
    (root / 'replay' / 'manifest.json').write_text('{"sessions": [{"session": "dev", "side": "dev"}]}')
    where, pairs, drafts, old, new = {}, [], {}, {}, {}
    for i in range(1, 10):
        old[i], new[i] = f'{i:012x}' + 'a' * 52, f'{i + 10:012x}' + 'b' * 52
        for end, uid, seq in [('earlier', old[i], i * 2), ('later', new[i], i * 2 + 1)]:
            label = f'{end}-{i}'
            where[label] = {'who': 'user', 'seq': seq}
            drafts[label] = {'quote': f'{end} WORDS {i}', 'session': 'dev'}
            raw.execute('INSERT INTO records VALUES (?, ?, ?)', (seq, 'prompt', 'dev'))
            if i == 6 and end == 'earlier' or i == 7 and end == 'later':
                continue
            db.execute('INSERT INTO active VALUES (?,?,?,?,?)', (uid, 'decision', end, 'decided', 'user'))
            db.execute('INSERT INTO claims VALUES (?, ?, ?)', (uid, 'd', seq))
            db.execute('INSERT INTO evidence VALUES (?, ?, 0, ?, ?)', ('d', seq, seq, drafts[label]['quote']))
        pairs.append({'id': f'p{i}', 'earlier': f'earlier-{i}', 'later': f'later-{i}', 'value': 'overturns'})
        if i not in (5, 6, 7, 8):
            db.execute('INSERT INTO edges VALUES (?, ?, ?, ?)', ('d', i * 2 + 1, old[i], 'supersedes'))
    # A link from an inactive derivation must not turn a curation miss into a linked pair.
    db.execute('INSERT INTO edges VALUES (?, 999, ?, ?)', ('d', old[8], 'supersedes'))
    done = 'f' * 64
    db.execute('INSERT INTO active VALUES (?,?,?,?,?)', (done, 'fix', 'finished', 'done', 'user'))
    db.execute('INSERT INTO claims VALUES (?, ?, 1000)', (done, 'd'))
    db.commit()
    raw.commit()
    (home / 'map.json').write_text(json.dumps(where))
    monkeypatch.setattr(m3, 'labels', lambda pool='dev': ([], pairs, drafts))
    current_raw = 'd:900 2026-09-01 00:00 UTC prompt (quote-only) — raw text\u2028still this one hit\n'
    def line(uid):
        return f'{uid[:12]} 2026-09-01 00:00 UTC decision decided (citable) — words\n'
    ranks = {1: [line(new[1]), line(old[1]), current_raw],
             2: [line(new[2]), current_raw, line(old[2])],
             3: [current_raw, line(old[3])],
             4: [current_raw, line(old[4]), 'o900 2026-09-01 00:00 UTC observation (imported) — current\n'],
             5: [line(old[5]), current_raw], 7: [line(old[7]), current_raw],
             8: [line(old[8]), current_raw], 9: [line(old[9]), line(done)]}
    seen_cli, seen_mcp = [], []
    def response(query, history):
        i = int(query.rsplit(' ', 1)[1])
        return (line(old[i]) if i != 8 else 'no hits\n') if history else ''.join(ranks[i])
    def run(argv, **kw):
        assert argv[:3] == [str(binary), '--home', str(home)]
        assert not any('KEY' in key.upper() or 'TOKEN' in key.upper() for key in kw['env'])
        if 'get' in argv:
            uid = argv[-1]
            delivered = uid in (old[1], old[2]) or uid not in old.values()
            status = '' if delivered else 'superseded by a-later-id\n'
            text = f'{uid} 2026-09-01 00:00 UTC decision decided repo (citable)\n{status}speaker: user\n'
        else:
            assert 'search' in argv and '--all' in argv and argv[argv.index('--limit') + 1] == '10'
            assert argv[-2] == '--'
            seen_cli.append(argv)
            text = response(argv[-1], '--history' in argv)
        return subprocess.CompletedProcess(argv, 0, text, '')
    class Mcp:
        def __init__(self, b, h, cwd):
            assert b == str(binary) and h == str(home)
        def __enter__(self): return self
        def __exit__(self, *args): pass
        def call(self, tool, args):
            assert tool == 'search' and args['all'] is True and args['limit'] == 10
            seen_mcp.append(args)
            return {'content': [{'type': 'text', 'text': '<memory-data>\n' + response(args['query'], args['history'])
                                + '</memory-data>'}]}
    monkeypatch.setattr(common.subprocess, 'run', run)
    monkeypatch.setattr(common, 'Mcp', Mcp)
    result = m3.overturned(str(binary), str(home))
    assert result['N'] == 9 and result['linked'] == 5
    assert result['curation_miss'] == {'missing_earlier': 1, 'missing_later': 1, 'unlinked': 2}
    for surface in ('cli', 'mcp'):
        assert result[surface]['ranked_current'] == {'bad': 2, 'n': 5, 'rate': 0.4, 'pass': False}
        assert result[surface]['history'] == {'recalled': 7, 'n': 8, 'recall': 0.875, 'pass': True}
    assert len(seen_cli) == len(seen_mcp) == 16
    assert {a['query'] for a in seen_mcp} == {f'earlier WORDS {i}' for i in (1, 2, 3, 4, 5, 7, 8, 9)}


def test_kinds_waits_for_a_missing_grader_and_reuses_the_other_answers(monkeypatch, tmp_path):
    import json, subprocess
    import calib, common, m3
    root, home, binary, db = _m3_home(tmp_path, monkeypatch)
    for seq, text in [(1, 'use tabs'), (2, 'other work')]:
        db.execute("INSERT INTO active VALUES (?, 'decision', ?, 'decided', 'user')", (f'u{seq}', text))
        db.execute("INSERT INTO claims VALUES (?, 'd', ?)", (f'u{seq}', seq))
        db.execute("INSERT INTO evidence VALUES ('d', ?, 0, ?, ?)", (seq, seq, text))
    db.commit()
    monkeypatch.setattr(common.subprocess, 'run', lambda a, **kw: subprocess.CompletedProcess(a, 0, kw['input'], ''))
    asked, unavailable = [], True
    def chat(model, prompt):
        asked.append(model)
        if model == 'glm-5.3' and unavailable and 'other work' in prompt:
            raise OSError('PRIVATE FAILURE SENTINEL')
        return '{"borne_out": true, "kind_right": true}', model
    monkeypatch.setattr(calib, 'chat', chat)
    pending = m3.kinds(str(binary), str(home))
    assert pending['complete'] is False
    assert pending['pending'] == 1
    assert 'per_kind' not in pending and 'agreement' not in pending
    unavailable = False
    done = m3.kinds(str(binary), str(home))
    assert done['complete'] and done['per_kind']['decision']['precision'] == 1.0
    assert asked.count('gpt-6-astra') == asked.count('deepseek-v4-pro') == 2
    assert asked.count('glm-5.3') == 3
    assert 'PRIVATE FAILURE SENTINEL' not in (root / 'm6' / 'calls.jsonl').read_text()


def test_kind_and_overturn_test_pools_require_the_recorded_curator_before_reading(monkeypatch, tmp_path):
    import json
    import pytest
    import common, m3
    root = tmp_path / 'eval'
    root.mkdir()
    monkeypatch.setattr(common, 'E', str(root))
    for run in (m3.kinds, m3.overturned):
        with pytest.raises(SystemExit):
            run('/unopened-binary', '/unopened-home', pool='test')
    (root / 'deciding.json').write_text(json.dumps({'curator': 'chosen'}))
    for run in (m3.kinds, m3.overturned):
        with pytest.raises(SystemExit):
            run('/unopened-binary', '/unopened-home', decide='different', pool='test')


def test_new_commands_require_the_explicit_binary_and_keep_pool_options(monkeypatch, tmp_path):
    import pytest
    import m3
    with pytest.raises(SystemExit) as missing:
        m3.main(['kinds', str(tmp_path)])
    assert missing.value.code == 2
    seen = []
    monkeypatch.setattr(m3, 'kinds', lambda *args: seen.append(args))
    m3.main(['kinds', str(tmp_path), '--binary', '/explicit/binary', '--pool', 'test', '--decide', 'chosen'])
    assert seen == [('/explicit/binary', str(tmp_path), 'chosen', 'test')]


def test_overturned_refuses_an_embedder_before_starting_either_search_arm(monkeypatch, tmp_path):
    import pytest
    import m3
    root, home, binary, db = _m3_home(tmp_path, monkeypatch)
    (home / 'config.toml').write_text('[embedding]\nprovider = "workers-ai"\n')
    with pytest.raises(SystemExit, match='full text'):
        m3.overturned(str(binary), str(home))


def test_draft_diagnostics_print_states_without_labels_or_quotes(monkeypatch, tmp_path, capsys):
    import json, sqlite3
    import common, m3
    root, home, binary, db = _m3_home(tmp_path, monkeypatch)
    (root / 'm3' / 'answers').mkdir(parents=True)
    (home / 'map.json').write_text(json.dumps({f'd{i}': {'seq': i, 'who': 'user'} for i in (1, 2)}))
    raw = sqlite3.connect(home / 'raw.db')
    raw.execute('CREATE TABLE ops(op_seq INTEGER, type TEXT, body TEXT)')
    labels = []
    for i in (1, 2):
        quote = f'NEVER_PRINT_LABEL_{i}'
        labels.append({'id': f'd{i}', 'value': 'yes', 'quote': quote})
        win = {'recurate': True, 'outcome': 'curated', 'from_seq': i, 'to_seq': i, 'summary': f's{i}'}
        claim = {'id': f'c{i}', 'body': f'body {i}', 'status': 'decided', 'speaker': 'user'}
        raw.execute('INSERT INTO ops VALUES (?, ?, ?)', (2 * i, 'window', json.dumps(win)))
        raw.execute('INSERT INTO ops VALUES (?, ?, ?)', (2 * i + 1, 'claim', json.dumps(claim)))
        answer = {'summary': f's{i}', 'claims': [dict(claim, quote=quote if i == 1 else 'unrelated words')]}
        common.write_jsonl(str(root / 'm3' / 'answers' / f'{i}.jsonl'),
                           [{'type': 'result', 'result': json.dumps(answer)}])
    raw.commit()
    monkeypatch.setattr(m3, 'labels', lambda: (labels, [], {}))
    m3.drafts(str(binary), str(home))
    printed = capsys.readouterr().out
    assert 'NEVER_PRINT_LABEL' not in printed
    assert 'd1' in printed and 'decided' in printed and 'd2' in printed and 'NOT DRAFTED' in printed
