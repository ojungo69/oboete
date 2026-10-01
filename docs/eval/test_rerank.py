"""Task 6's rerank contract, without network or model packages.
Run: uv run --with pytest==9.1.1 pytest -q test_rerank.py
"""
import json
import os

import pytest

import rerank


def inputs(tmp_path, hits, questions, docs):
    run, sidecar, queries = (tmp_path / name for name in ('b-off.trec', 'b-docs.jsonl', 'questions.jsonl'))
    run.write_text(''.join(f'{qid} Q0 {key} {100 - i} {999 - i} b-off\n'
                           for qid, keys in hits.items() for i, key in enumerate(keys)), encoding='utf-8')
    sidecar.write_text(''.join(json.dumps({'key': key, 'session': 's', 'ts': 123, 'kind': 'decision',
                                         'text': text}, ensure_ascii=False) + '\n'
                               for key, text in docs.items()), encoding='utf-8')
    queries.write_text(''.join(json.dumps({'qid': qid, 'text': text}, ensure_ascii=False) + '\n'
                              for qid, text in questions.items()), encoding='utf-8')
    return [str(run), str(sidecar), '--questions', str(queries)]


def test_the_rerank_run_is_a_permutation(tmp_path, capsys):
    hits = {'ja': ['a', 'r:dev:2', 'c', 'tail-1', 'tail-2'],
            'en': ['d', 'e', 'f', 'tail-3']}
    questions = {'en': 'What is the decision?', 'ja': 'どの決定？'}
    docs = {'a': '文書A', 'r:dev:2': '文書B', 'c': '文書C', 'd': 'D', 'e': 'E', 'f': 'F'}
    scores = {'どの決定？': [2., 7., 7.], 'What is the decision?': [-3., 0., -3.]}
    calls = []

    def score(query, texts):
        calls.append((query, texts))
        return scores[query]

    out = rerank.main(inputs(tmp_path, hits, questions, docs) + ['--k', '3'], scorer=score)
    rows = [line.split() for line in out.read_text(encoding='utf-8').splitlines()]
    expected = {'ja': ['r:dev:2', 'c', 'a'], 'en': ['e', 'd', 'f']}
    for qid, keys in hits.items():
        found = [row for row in rows if row[0] == qid]
        ordered = [row[2] for row in found]
        assert set(ordered[:3]) == set(keys[:3])
        assert ordered[:3] == expected[qid]
        assert ordered[3:] == keys[3:]
        assert [int(row[3]) for row in found] == list(range(1, len(keys) + 1))
        assert [int(row[4]) for row in found] == list(range(len(keys), 0, -1))
    assert all(len(row) == 6 and row[1] == 'Q0' and row[5] == 'b-rerank' for row in rows)
    assert rows[0][0] == 'ja'                       # run order, not question-file order
    assert calls == [(questions[qid], [docs[key] for key in keys[:3]]) for qid, keys in hits.items()]
    assert out == tmp_path / 'b-rerank.trec'
    assert 'threads=4' in capsys.readouterr().out


def test_a_missing_sidecar_key_stops_before_scoring_or_replacing_output(tmp_path, capsys):
    argv = inputs(tmp_path, {'q1': ['a'], 'q2': ['missing-key']}, {'q1': 'Q1', 'q2': 'Q2'}, {'a': 'A'})
    out = tmp_path / 'b-rerank.trec'
    out.write_text('previous result\n', encoding='utf-8')
    calls = []
    with pytest.raises(SystemExit) as error:
        rerank.main(argv, scorer=lambda query, docs: calls.append((query, docs)))
    assert error.value.code == 2
    assert 'missing-key' in capsys.readouterr().err
    assert calls == []
    assert out.read_text(encoding='utf-8') == 'previous result\n'


@pytest.mark.parametrize('hits', [{'q': ['a', 'b']}, {}])
def test_k_larger_than_hits_scores_all_and_questions_without_hits_write_nothing(tmp_path, hits):
    argv = inputs(tmp_path, hits, {'empty': 'No hits', 'q': 'Question'}, {'a': 'A', 'b': 'B'})
    calls = []

    def score(query, docs):
        calls.append((query, docs))
        return [-2., 3.]

    out = rerank.main(argv + ['--k', '50'], scorer=score)
    if hits:
        assert calls == [('Question', ['A', 'B'])]
        assert out.read_text(encoding='utf-8') == 'q Q0 b 1 2 b-rerank\nq Q0 a 2 1 b-rerank\n'
    else:
        assert calls == []
        assert out.read_text(encoding='utf-8') == ''


def test_time_reports_wall_and_cpu_and_names_rss_units(tmp_path, monkeypatch, capsys):
    argv = inputs(tmp_path, {'q': ['a']}, {'q': 'Q', 'empty': 'No hits'}, {'a': 'A'})
    walls, cpus = iter([5., 5.5]), iter([12., 12.25])
    monkeypatch.setattr(rerank.time, 'perf_counter', lambda: next(walls))
    monkeypatch.setattr(rerank.time, 'process_time', lambda: next(cpus))
    rerank.main(argv + ['--time', '--threads', '2'], scorer=lambda query, docs: [1.])
    printed = capsys.readouterr().out
    assert 'threads=2' in printed
    assert 'q wall=0.500000s CPU=0.250000s docs=1' in printed
    # A question with no hits is not timed.
    assert 'empty' not in printed
    assert 'wall p50=0.500000s p95=0.500000s n=1' in printed
    assert 'CPU p50=0.250000s p95=0.250000s n=1' in printed
    assert ('bytes (macOS)' if rerank.sys.platform == 'darwin' else 'kB (Linux)') in printed


@pytest.mark.parametrize('line', ['q Q0 a x 1.0 b-off', 'q Q0 a 1 nan b-off', 'q Q0 a 0 1.0 b-off'])
def test_a_bad_rank_or_score_names_its_line(tmp_path, line):
    run = tmp_path / 'b-off.trec'
    run.write_text('q Q0 b 1 2.0 b-off\n' + line + '\n', encoding='utf-8')
    with pytest.raises(ValueError, match=r'b-off\.trec:2: invalid rank or score'):
        rerank.read_run(str(run))


@pytest.mark.parametrize('scores', [[], [1., 2.], [float('nan')], [float('inf')]])
def test_invalid_scorer_results_do_not_write_a_run(tmp_path, capsys, scores):
    argv = inputs(tmp_path, {'q': ['a']}, {'q': 'Q'}, {'a': 'A'})
    with pytest.raises(SystemExit) as error:
        rerank.main(argv, scorer=lambda query, docs: scores)
    assert error.value.code == 2
    assert 'one finite score per document' in capsys.readouterr().err
    assert not (tmp_path / 'b-rerank.trec').exists()


def test_a_later_failure_or_a_failed_write_keeps_the_previous_run(tmp_path, monkeypatch, capsys):
    argv = inputs(tmp_path, {'q1': ['a'], 'q2': ['a']}, {'q1': 'Q1', 'q2': 'Q2'}, {'a': 'A'})
    out = tmp_path / 'b-rerank.trec'
    out.write_text('previous result\n', encoding='utf-8')
    answers = iter([[1.], []])
    with pytest.raises(SystemExit):
        rerank.main(argv, scorer=lambda query, docs: next(answers))
    assert out.read_text(encoding='utf-8') == 'previous result\n'

    def no_replace(*args):
        raise OSError('disk full')

    monkeypatch.setattr(rerank.os, 'replace', no_replace)
    with pytest.raises(SystemExit) as error:
        rerank.main(argv, scorer=lambda query, docs: [1.])
    assert error.value.code == 2 and 'disk full' in capsys.readouterr().err
    assert out.read_text(encoding='utf-8') == 'previous result\n'
    assert sorted(p.name for p in tmp_path.iterdir()) == ['b-docs.jsonl', 'b-off.trec', 'b-rerank.trec',
                                                         'questions.jsonl']


def test_p50_is_the_median_and_p95_the_nearest_rank(tmp_path, monkeypatch, capsys):
    # Skewed so that the median, the mean, the nearest-rank p95 and the maximum all differ.
    # Not in order, so the percentiles must sort.
    walls = [5.] + [1.] * 9 + [.1] * 10
    cpus = [2.] * 9 + [8.] + [.2] * 10
    qids = [f'q{i:02}' for i in range(20)]
    argv = inputs(tmp_path, {q: ['a'] for q in qids}, {q: 'Q' for q in qids}, {'a': 'A'})
    clock = lambda spans: iter(t for d in spans for t in (0., d))
    monkeypatch.setattr(rerank.time, 'perf_counter', clock(walls).__next__)
    monkeypatch.setattr(rerank.time, 'process_time', clock(cpus).__next__)
    rerank.main(argv + ['--time'], scorer=lambda query, docs: [1.])
    printed = capsys.readouterr().out
    assert 'wall p50=0.550000s p95=1.000000s n=20' in printed
    assert 'CPU p50=1.100000s p95=2.000000s n=20' in printed


@pytest.mark.parametrize('case', ['not empty', 'symbolic link'])
def test_a_wrong_export_directory_is_refused_before_anything_is_deleted(tmp_path, case):
    out = tmp_path / 'onnx'
    out.mkdir()
    if case == 'not empty':
        (out / '.export-cache').mkdir()
        (out / '.export-cache' / 'kept').write_text('x')
        (out / 'model.onnx').write_text('earlier export')
    else:
        elsewhere = tmp_path / 'elsewhere'
        elsewhere.mkdir()
        (elsewhere / 'kept').write_text('x')
        (out / '.export-cache').symlink_to(elsewhere, target_is_directory=True)
    with pytest.raises(ValueError):
        rerank.export_model(out)
    assert (out / '.export-cache' / 'kept').read_text() == 'x'


def test_the_download_names_huggingface_co_whatever_the_environment_says(tmp_path, monkeypatch):
    import sys, types

    asked = {}

    class Stop(Exception):
        pass

    def download(*args, **kwargs):
        asked.update(kwargs)
        raise Stop

    monkeypatch.setenv('HF_ENDPOINT', 'https://mirror.invalid')
    monkeypatch.setenv('HUGGINGFACE_CO_STAGING', '1')
    for name, module in (('torch', types.SimpleNamespace()),
                         ('huggingface_hub', types.SimpleNamespace(snapshot_download=download)),
                         ('transformers', types.SimpleNamespace(AutoModelForSequenceClassification=None,
                                                                AutoTokenizer=None))):
        monkeypatch.setitem(sys.modules, name, module)
    with pytest.raises(Stop):
        rerank.export_model(tmp_path / 'onnx')
    assert (asked['endpoint'], asked['revision'], asked['token']) == ('https://huggingface.co', rerank.REVISION, False)


def test_the_export_deletes_no_cache_itself(tmp_path, monkeypatch):
    # The command that made .export-cache deletes it; the script never does, so it cannot
    # delete one another export is using.
    out = tmp_path / 'onnx'
    (out / '.export-cache').mkdir(parents=True)
    (out / '.export-cache' / 'kept').write_text('x')
    monkeypatch.setenv('HF_HOME', 'before')
    monkeypatch.delenv('NETRC', raising=False)
    monkeypatch.setattr(rerank.tempfile, 'tempdir', 'before')
    with pytest.raises(RuntimeError):
        with rerank.export_cache(out):
            assert os.environ['NETRC'] == os.devnull
            assert os.environ['HF_HOME'] == str(out / '.export-cache' / 'hf')
            raise RuntimeError('the export failed')
    assert (out / '.export-cache' / 'kept').read_text() == 'x'
    # The environment and the temporary directory are as they were.
    assert (os.environ['HF_HOME'], 'NETRC' in os.environ) == ('before', False)
    assert rerank.tempfile.tempdir == 'before'


def failing_on(n, real, monkeypatch, owner, name):
    calls = []

    def call(*args, **kwargs):
        calls.append(args)
        if len(calls) == n:
            raise OSError('disk full')
        return real(*args, **kwargs)

    monkeypatch.setattr(owner, name, call)


def test_a_failed_move_takes_the_exported_files_back_out(tmp_path, monkeypatch):
    # The export's out held only .export-cache, so a failure after the first move leaves it so
    # and a rerun is not refused (Codex on d45266a).
    stage, out = tmp_path / 'onnx' / '.export-cache' / 'exported', tmp_path / 'onnx'
    stage.mkdir(parents=True)
    for name in ('model.onnx', 'model.onnx.data', 'tokenizer.json'):
        (stage / name).write_text(name, encoding='utf-8')
    failing_on(2, os.replace, monkeypatch, rerank.os, 'replace')
    with pytest.raises(OSError):
        rerank.publish(stage, out)
    assert [p.name for p in out.iterdir()] == ['.export-cache']


@pytest.mark.parametrize('fails', [None, 'write', 'replace'])
def test_agreement_publishes_both_files_or_keeps_the_previous_ones(tmp_path, monkeypatch, fails):
    out = tmp_path / 'agreement'
    out.mkdir()
    for name in ('README.md', 'sets.jsonl'):
        (out / name).write_text(f'previous {name}\n', encoding='utf-8')
    monkeypatch.setattr(rerank, 'onnx_scorer', lambda *args: lambda query, docs: [1.] * len(docs))
    monkeypatch.setattr(rerank, 'public_sets', lambda language: [(f'{language} Q', ['D'], 'q', ['d'])])
    monkeypatch.setattr(rerank, 'fingerprints', lambda directory: ['model.onnx: SHA-256=0 size=0 bytes'])
    if fails == 'write':
        failing_on(2, rerank.Path.write_text, monkeypatch, rerank.Path, 'write_text')
    elif fails == 'replace':
        failing_on(2, os.replace, monkeypatch, rerank.os, 'replace')
    if fails:
        with pytest.raises(OSError):
            rerank.agreement('onnx', out)
    else:
        rerank.agreement('onnx', out)
    assert sorted(p.name for p in out.iterdir()) == ['README.md', 'sets.jsonl']
    sets, readme = (out / 'sets.jsonl').read_text(encoding='utf-8'), (out / 'README.md').read_text(encoding='utf-8')
    if fails:
        assert (sets, readme) == ('previous sets.jsonl\n', 'previous README.md\n')
    else:
        assert [json.loads(line)['query'] for line in sets.split('\n')[:-1]] == ['ja Q', 'en Q']
        assert readme.startswith('# Reranker agreement sets') and 'model.onnx: SHA-256=0' in readme


def test_an_undo_that_fails_too_leaves_the_previous_file_in_the_stage(tmp_path, monkeypatch):
    # The second rename fails, then putting README.md back fails: the previous README.md stays
    # in the stage, which agreement keeps (Codex on 10c8a25).
    out = tmp_path / 'agreement'
    out.mkdir()
    for name in ('README.md', 'sets.jsonl'):
        (out / name).write_text(f'previous {name}\n', encoding='utf-8')
    monkeypatch.setattr(rerank, 'onnx_scorer', lambda *args: lambda query, docs: [1.] * len(docs))
    monkeypatch.setattr(rerank, 'public_sets', lambda language: [(f'{language} Q', ['D'], 'q', ['d'])])
    monkeypatch.setattr(rerank, 'fingerprints', lambda directory: ['model.onnx: SHA-256=0 size=0 bytes'])
    replace, calls = os.replace, []

    def second_and_third_fail(*args):
        calls.append(args)
        if len(calls) in (2, 3):
            raise OSError('disk full')
        return replace(*args)

    monkeypatch.setattr(rerank.os, 'replace', second_and_third_fail)
    with pytest.raises(OSError):
        rerank.agreement('onnx', out)
    [stage] = out.glob('.agreement-*')
    assert (stage / 'README.md.kept').read_text(encoding='utf-8') == 'previous README.md\n'
    assert (out / 'sets.jsonl').read_text(encoding='utf-8') == 'previous sets.jsonl\n'
