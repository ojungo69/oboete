"""Task 6's rerank contract, without network or model packages.
Run: uv run --with pytest==9.1.1 pytest -q test_rerank.py
"""
import json

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


@pytest.mark.parametrize('scores', [[], [1., 2.], [float('nan')], [float('inf')]])
def test_invalid_scorer_results_do_not_write_a_run(tmp_path, capsys, scores):
    argv = inputs(tmp_path, {'q': ['a']}, {'q': 'Q'}, {'a': 'A'})
    with pytest.raises(SystemExit) as error:
        rerank.main(argv, scorer=lambda query, docs: scores)
    assert error.value.code == 2
    assert 'one finite score per document' in capsys.readouterr().err
    assert not (tmp_path / 'b-rerank.trec').exists()
