r"""Milestone 4, Task 6, Step 6: the fixed CPU reranker (plan D10; spec 8.2).

From docs/eval, with Python 3.12:
  uv run --no-project --python 3.12 --with numpy==2.2.6 \
    --with onnxruntime==1.22.1 --with tokenizers==0.21.4 \
    python rerank.py <run.trec> <b-docs.jsonl> --questions <questions.jsonl> \
    --k 50 --max-length 512 --threads 4 --time
  uv run --no-project --python 3.12 --with numpy==2.2.6 \
    --with onnxruntime==1.22.1 --with tokenizers==0.21.4 \
    python rerank.py agreement ~/.oboete/eval/reranker-onnx ../../src/testdata/rerank-agreement
  uv run --no-project --python 3.12 --with pytest==9.1.1 pytest -q test_rerank.py

Export only when the owner has budgeted its disk use. Put uv's environment and
caches inside the same disposable directory as the download and export scratch:
  out="$HOME/.oboete/eval/reranker-onnx"
  mkdir -p "$out/.export-cache/tmp"
  UV_CACHE_DIR="$out/.export-cache/uv" \
    UV_PYTHON_INSTALL_DIR="$out/.export-cache/python" \
    HF_HOME="$out/.export-cache/hf" XDG_CACHE_HOME="$out/.export-cache/xdg" \
    TMPDIR="$out/.export-cache/tmp" PYTHONDONTWRITEBYTECODE=1 \
    uv run --no-project --python 3.12 --index https://download.pytorch.org/whl/cpu \
    --with torch==2.8.0 --with transformers==4.55.4 --with onnx==1.18.0 \
    --with onnxscript==0.4.0 --with huggingface-hub==0.34.4 \
    --with safetensors==0.6.2 --with tokenizers==0.21.4 --with numpy==2.2.6 \
    python rerank.py export "$out"

On macOS, omit --index https://download.pytorch.org/whl/cpu; its torch wheels
already run on CPU. Keep every package version unchanged.

The export directory must be empty except for .export-cache, which is deleted
even on failure. Only model.onnx, its external data, and the revision's tokenizer
files survive. Downloads are anonymous; no subprocess is started by this script.
Before running uv, remove environment variables whose names contain TOKEN, KEY,
SECRET or PASSWORD. main() also removes them before importing model packages.

Reranking reads only the supplied run, gated sidecar and questions. It writes
b-rerank.trec beside the run. The ONNX directory is ~/.oboete/eval/reranker-onnx by default,
outside the repository, with the evaluation's other owner-only files.
Scores are raw fp32 logits, without sigmoid. Each pair is truncated together
with longest_first, including special tokens. Documents run one at a time to
bound activation memory and give the later Rust implementation the same shape.
--time excludes model loading and file I/O; it includes pair tokenization and
scoring. Per question it prints the wall time, which a search would wait and
which spec 8.2's Rerank line (1.5 s less the hybrid's MCP p95) reads, and the
process CPU time, the sum over ONNX Runtime's threads. p50 is the median, p95 the
nearest rank, over the questions with hits. Peak RSS is process-wide, in kB on
Linux or bytes on macOS.
"""
import argparse, contextlib, gzip, io, json, math, os, resource, shutil, statistics, sys, tempfile, time
from pathlib import Path

from common import E, clean_env, read_jsonl, sha256_file

MODEL = 'BAAI/bge-reranker-v2-m3'
# Current main commit on huggingface.co, checked 2026-10-01 (not a PR's ONNX export).
REVISION = '953dc6f6f85a1b2dbfca4c34a2796e7dde08d41e'
DATASET = 'Shitao/MLDR'
DATASET_REVISION = 'd67138e705d963e346253a80e59676ddb418810a'
TOKENIZER_FILES = ('tokenizer.json', 'tokenizer_config.json', 'special_tokens_map.json',
                   'sentencepiece.bpe.model')
DEFAULT_ONNX = Path(E) / 'reranker-onnx'


def texts(path, field):
    result = {}
    for row in read_jsonl(path):
        if not isinstance(row, dict) or not isinstance(row.get(field), str) or not row[field]:
            raise ValueError(f'{path}: each row needs a nonempty {field}')
        if not isinstance(row.get('text'), str):
            raise ValueError(f'{path}: {row[field]} needs text')
        if row[field] in result:
            raise ValueError(f'{path}: duplicate {field} {row[field]}')
        result[row[field]] = row['text']
    return result


def read_run(path):
    hits = {}
    with open(path, encoding='utf-8') as f:
        for number, line in enumerate(f, 1):
            if not line.strip():
                continue
            fields = line.split()
            if len(fields) != 6 or fields[1] != 'Q0':
                raise ValueError(f'{path}:{number}: expected qid Q0 key rank score tag')
            qid, _, key, rank, score, _ = fields
            if int(rank) < 1 or not math.isfinite(float(score)):
                raise ValueError(f'{path}:{number}: invalid rank or score')
            # File order defines the shortlist and ties, rather than the printed rank.
            hits.setdefault(qid, []).append(key)
    return hits


def checked_scores(score, query, docs):
    scores = [float(value) for value in score(query, docs)]
    if len(scores) != len(docs) or not all(math.isfinite(value) for value in scores):
        raise ValueError('scorer must return one finite score per document')
    return scores


def timing_summary(times):
    for name, values in (('wall', [w for w, _ in times]), ('CPU', [c for _, c in times])):
        if values:
            print(f'{name} p50={statistics.median(values):.6f}s '
                  f'p95={sorted(values)[math.ceil(.95 * len(values)) - 1]:.6f}s n={len(values)}')
        else:
            print(f'{name} p50=NA p95=NA n=0')
    unit = 'bytes (macOS)' if sys.platform == 'darwin' else 'kB (Linux)'
    print(f'peak RSS={resource.getrusage(resource.RUSAGE_SELF).ru_maxrss} {unit}')


def rerank_run(run, docs, questions, score=None, k=50, max_length=512, threads=4,
               timed=False, onnx_dir=DEFAULT_ONNX):
    run = Path(run)
    out = run.with_name('b-rerank.trec')
    if out.resolve() == run.resolve():
        raise ValueError('input run must not be b-rerank.trec')
    hits, document_text, question_text = read_run(run), texts(docs, 'key'), texts(questions, 'qid')
    # Validate every shortlist before loading a model or replacing a previous result.
    for qid, keys in hits.items():
        if qid not in question_text:
            raise ValueError(f'missing question text for {qid}')
        for key in keys[:k]:
            if key not in document_text:
                raise ValueError(f'missing sidecar row for key {key} (question {qid})')
            if not document_text[key]:
                raise ValueError(f'empty sidecar text for key {key} (question {qid})')
    print(f'CPUExecutionProvider threads={threads} (intra-op), inter-op=1; max_length={max_length}')
    if hits and score is None:
        score = onnx_scorer(onnx_dir, max_length, threads)
    lines, times = [], []
    for qid, keys in hits.items():
        head = keys[:k]
        wall, cpu = time.perf_counter(), time.process_time()
        scores = checked_scores(score, question_text[qid], [document_text[key] for key in head])
        if timed:
            times.append((time.perf_counter() - wall, time.process_time() - cpu))
            print(f'{qid} wall={times[-1][0]:.6f}s CPU={times[-1][1]:.6f}s docs={len(head)}')
        # Python's stable sort keeps the input order when logits tie.
        head = [key for _, key in sorted(zip(scores, head), key=lambda pair: -pair[0])]
        ordered = head + keys[k:]
        lines.extend(f'{qid} Q0 {key} {i + 1} {len(keys) - i} b-rerank\n'
                     for i, key in enumerate(ordered))
    # A scoring failure must not leave a plausible but incomplete TREC run.
    out.write_text(''.join(lines), encoding='utf-8')
    if timed:
        timing_summary(times)
    return out


def onnx_scorer(directory, max_length=512, threads=4):
    import numpy as np
    import onnxruntime as ort
    from tokenizers import Tokenizer

    tokenizer = Tokenizer.from_file(str(Path(directory) / 'tokenizer.json'))
    if max_length < tokenizer.num_special_tokens_to_add(is_pair=True):
        raise ValueError('max-length is shorter than the pair special tokens')
    tokenizer.enable_truncation(max_length=max_length, strategy='longest_first', direction='right')
    tokenizer.no_padding()
    options = ort.SessionOptions()
    options.intra_op_num_threads, options.inter_op_num_threads = threads, 1
    options.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
    session = ort.InferenceSession(str(Path(directory) / 'model.onnx'), sess_options=options,
                                   providers=['CPUExecutionProvider'])

    def score(query, docs):
        scores = []
        for text in docs:
            pair = tokenizer.encode(query, text)
            inputs = {'input_ids': np.asarray([pair.ids], dtype=np.int64),
                      'attention_mask': np.asarray([pair.attention_mask], dtype=np.int64)}
            scores.append(float(session.run(['logits'], inputs)[0].reshape(-1)[0]))
        return scores

    return score


@contextlib.contextmanager
def export_cache(out):
    cache = out / '.export-cache'
    environment, previous_temp = clean_env(), tempfile.tempdir
    # The download's and torch's caches; uv's own are set on its command line (above).
    paths = {'HF_HOME': 'hf',
             'HF_HUB_CACHE': 'hf/hub', 'HF_ASSETS_CACHE': 'hf/assets', 'HF_XET_CACHE': 'hf/xet',
             'XDG_CACHE_HOME': 'xdg',
             'TORCH_HOME': 'torch', 'TORCHINDUCTOR_CACHE_DIR': 'inductor',
             'TMPDIR': 'tmp', 'TMP': 'tmp', 'TEMP': 'tmp'}
    try:
        for name, suffix in paths.items():
            path = cache / suffix
            path.mkdir(parents=True, exist_ok=True)
            os.environ[name] = str(path)
        tempfile.tempdir = None
        yield cache
    finally:
        os.environ.clear()
        os.environ.update(environment)
        tempfile.tempdir = previous_temp
        shutil.rmtree(cache)


def fingerprints(directory):
    directory = Path(directory)
    paths = sorted(directory.glob('*.onnx*')) + [directory / 'tokenizer.json']
    return [f'{path.name}: SHA-256={sha256_file(path)} size={path.stat().st_size} bytes'
            for path in paths]


def export_model(out):
    out = Path(out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    with export_cache(out) as cache:
        if any(path.name != '.export-cache' for path in out.iterdir()):
            raise ValueError('export directory must be empty except for .export-cache')
        print(f'{MODEL} revision={REVISION}', flush=True)
        # Imports come after cache setup so even package initialization stays in scratch.
        import torch
        from huggingface_hub import snapshot_download
        from transformers import AutoModelForSequenceClassification, AutoTokenizer

        snapshot = snapshot_download(MODEL, revision=REVISION, token=False,
                                     allow_patterns=['config.json', 'model.safetensors', *TOKENIZER_FILES],
                                     cache_dir=str(cache / 'hf/hub'))
        tokenizer = AutoTokenizer.from_pretrained(snapshot, local_files_only=True, trust_remote_code=False)
        model = AutoModelForSequenceClassification.from_pretrained(
            snapshot, local_files_only=True, trust_remote_code=False, use_safetensors=True,
            torch_dtype=torch.float32, attn_implementation='eager').cpu().float().eval()
        torch.set_num_threads(4)

        class Logits(torch.nn.Module):
            def __init__(self):
                super().__init__()
                self.model = model

            def forward(self, input_ids, attention_mask):
                return self.model(input_ids=input_ids, attention_mask=attention_mask).logits

        inputs = tokenizer(['What is a panda?', 'パンダとは？'],
                           ['A panda is a bear.', 'パンダはクマの仲間です。'],
                           padding=True, truncation=True, max_length=512, return_tensors='pt')
        stage = cache / 'exported'
        stage.mkdir()
        shape = {0: torch.export.Dim('batch', min=1), 1: torch.export.Dim('tokens', min=4, max=8192)}
        with torch.no_grad():
            torch.onnx.export(Logits().eval(), (inputs['input_ids'], inputs['attention_mask']),
                              str(stage / 'model.onnx'), dynamo=True, external_data=True,
                              input_names=['input_ids', 'attention_mask'], output_names=['logits'],
                              dynamic_shapes={'input_ids': shape, 'attention_mask': shape},
                              opset_version=18)
        for name in TOKENIZER_FILES:
            shutil.copyfile(Path(snapshot) / name, stage / name)
        for line in fingerprints(stage):
            print(line, flush=True)
        for path in stage.iterdir():
            shutil.move(path, out / path.name)


def public_sets(language):
    from urllib.request import urlopen

    url = (f'https://huggingface.co/datasets/{DATASET}/resolve/{DATASET_REVISION}/'
           f'mldr-v1.0-{language}/dev.jsonl.gz')
    rows, seen = [], set()
    # Read only enough of the public dev file; no full corpus or authenticated cache.
    with urlopen(url, timeout=60) as response, gzip.GzipFile(fileobj=response) as stream:
        for line in io.TextIOWrapper(stream, encoding='utf-8'):
            row = json.loads(line)
            passage = row['positive_passages'][0]
            if passage['docid'] in seen:
                continue
            seen.add(passage['docid'])
            rows.append((row['query_id'], row['query'], passage['docid'], passage['text'][:4000]))
            if len(rows) == 14:
                break
    if len(rows) != 14:
        raise ValueError(f'{DATASET}: fewer than 14 distinct {language} dev documents')
    for i in range(10):
        yield rows[i][1], [row[3] for row in rows[i:i + 5]], rows[i][0], [row[2] for row in rows[i:i + 5]]


def agreement(directory, out, max_length=512, threads=4):
    score = onnx_scorer(directory, max_length, threads)
    print(f'CPUExecutionProvider threads={threads} (intra-op), inter-op=1; max_length={max_length}')
    lines, sources = [], []
    for language in ('ja', 'en'):
        for query, docs, qid, ids in public_sets(language):
            values = [round(v, 6) for v in checked_scores(score, query, docs)]
            lines.append(json.dumps({'query': query, 'docs': docs, 'scores': values}, ensure_ascii=False) + '\n')
            sources.append(f'- Row {len(lines)}: {language}, query `{qid}`, documents `{", ".join(ids)}`.')
    readme = (f'# Reranker agreement sets\n\n'
              f'Source: [{DATASET}](https://huggingface.co/datasets/{DATASET}), '
              f'revision `{DATASET_REVISION}`. The dataset card publishes MLDR under the '
              f'[MIT license](https://huggingface.co/datasets/{DATASET}/blob/{DATASET_REVISION}/README.md).\n\n'
              'Ten Japanese dev queries, then ten English dev queries. For each language, take '
              'the first 14 rows with distinct first positive document IDs. Each of the first '
              'ten queries uses its positive document followed by the next four rows\' documents '
              '(distractors, not relevance judgments). Document text is cut to 4,000 characters, '
              'matching the evaluation sidecar. No owner questions or documents are used.\n\n'
              f'Model: [{MODEL}](https://huggingface.co/{MODEL}/tree/{REVISION}), '
              f'revision `{REVISION}`, fp32 ONNX, CPUExecutionProvider.\n'
              f'Threads: {threads} intra-op, 1 inter-op, sequential execution. '
              f'Max length: {max_length} tokens for the pair, longest_first, including special tokens. '
              'One document per inference. Scores are raw logits rounded to six decimals; Rust checks '
              'them in this order with absolute tolerance 1e-3.\n\n'
              + '\n'.join(f'- {line}' for line in fingerprints(directory))
              + '\n\n## Source IDs in file order\n\n' + '\n'.join(sources) + '\n')
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    (out / 'sets.jsonl').write_text(''.join(lines), encoding='utf-8')
    (out / 'README.md').write_text(readme, encoding='utf-8')


def positive(value):
    value = int(value)
    if value < 1:
        raise argparse.ArgumentTypeError('must be positive')
    return value


def main(argv=None, scorer=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    command = argv.pop(0) if argv and argv[0] in ('export', 'agreement') else 'rerank'
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0],
                                     epilog='Subcommands: export OUT; agreement ONNX_DIR OUT.')
    if command == 'export':
        parser.add_argument('out', type=Path)
    else:
        if command == 'agreement':
            parser.add_argument('onnx_dir', type=Path)
            parser.add_argument('out', type=Path)
        else:
            parser.add_argument('run', type=Path)
            parser.add_argument('docs', type=Path)
            parser.add_argument('--questions', type=Path, required=True)
            parser.add_argument('--onnx-dir', type=Path, default=DEFAULT_ONNX)
            parser.add_argument('--k', type=positive, default=50)
            parser.add_argument('--time', action='store_true')
        parser.add_argument('--max-length', type=positive, default=512)
        parser.add_argument('--threads', type=positive, default=4)
    args = parser.parse_args(argv)
    environment = clean_env()
    os.environ.clear()
    os.environ.update(environment)
    os.umask(0o077)
    try:
        if command == 'export':
            export_model(args.out)
        elif command == 'agreement':
            agreement(args.onnx_dir, args.out, args.max_length, args.threads)
        else:
            return rerank_run(args.run, args.docs, args.questions, score=scorer, k=args.k,
                              max_length=args.max_length, threads=args.threads, timed=args.time,
                              onnx_dir=args.onnx_dir)
    except (OSError, ValueError, RuntimeError, ImportError) as error:
        parser.error(str(error))


if __name__ == '__main__':
    main()
