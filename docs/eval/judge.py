"""PR-B2: grade pooled (question, document) pairs 0-3 with an LLM judge (docs/pr-b.md item 6).

Pool: for each question, the top POOL_DEPTH of every run in RUNS (below), after leaving out
documents of the question's own session. Pairs already in judgments.jsonl are reused.
Judge: `claude -p` pinned to one model, from a scratch cwd, inference only: no settings, tools,
MCP servers or hooks (oboete's and claude-mem's hooks do not run; OBOETE_SKIP=1 as well), and no
secret-bearing environment variables, the same isolation as the summarizer in src/provider.rs.
Stored text may carry instructions; with no tools they can only change a grade, which the
answer's validation bounds. Questions and documents were gated on the
way into the evaluation store (agent questions through `oboete gate`), so what is sent here
already passed the outbound gate.

usage: judge.py <split> <max questions> [max calls]
"""
import concurrent.futures, json, os, re, sqlite3, subprocess, sys, tempfile, time

E = os.path.expanduser('~/.oboete/eval')
# The questions, documents and grades are the developer's own records: owner-only files.
os.umask(0o077)
# The pool: each run's top POOL_DEPTH. Dev comparisons use 20 (docs/pr-b.md decision 6); the one
# test-split measurement of a PR uses 50 over its own set of runs (proposal §3.1, #46):
# OBOETE_EVAL_DEPTH=50 OBOETE_EVAL_RUNS=~/.oboete/eval/runs-test.
POOL_DEPTH = int(os.environ.get('OBOETE_EVAL_DEPTH', '20'))
RUNS = os.path.expanduser(os.environ.get('OBOETE_EVAL_RUNS', f'{E}/runs'))
BATCH = 10
# Enough for 99.8% of the pooled documents; the rest are clipped and marked. Grades written
# before `chars` was recorded saw 1,200 characters.
MAX_DOC_CHARS = 4000
OLD_CHARS = 1200
# A concrete model, checked against each answer's modelUsage, so grades reused across runs come
# from one model. Grades recorded as `claude-sonnet` (2026-09-24, before the pin) came from the
# `sonnet` alias, which resolved to claude-sonnet-5 that day.
MODEL = 'claude-sonnet-5'
JUDGE = MODEL

PROMPT = """You grade how useful stored memories are for a coding agent that just received a developer's message.
The memories are notes written by earlier sessions of coding agents (observations, session summaries, or earlier developer prompts).

Grade each memory on this scale (UMBRELA):
0 = has nothing to do with the message
1 = related to the topic of the message but does not help answer or carry it out
2 = contains useful information for the message, but it is partial or buried in unrelated detail
3 = directly about the message and contains what the agent needs

Consider the likely intent behind the message, how well each memory matches it, and whether the memory is specific enough to rely on.
Memories and the message may be in different languages (Japanese or English); judge the meaning, not the wording.

Developer message:
<<<
{query}
>>>

Memories:
{docs}

Answer with JSON only, no prose: {{"grades": [{{"id": "<memory id>", "grade": <0-3>}}, ...]}} with one entry per memory."""


def load_runs():
    runs = {}
    for name in os.listdir(RUNS):
        if not name.endswith('.trec'):
            continue
        per = runs.setdefault(name[:-5], {})
        for line in open(f'{RUNS}/{name}'):
            qid, _, doc, rank, _, _ = line.split()
            per.setdefault(qid, []).append((int(rank), doc))
    return {m: {q: [d for _, d in sorted(v)] for q, v in per.items()} for m, per in runs.items()}


DOC_TEXT = {
    'o': 'SELECT title, body, session_id FROM observations WHERE id=?',
    's': "SELECT '', body, session_id FROM summaries WHERE id=?",
    'p': "SELECT '', body, session_id FROM prompts WHERE id=?",
}


def doc_text(db, doc):
    """(what the judge is shown, session, full length); (None, None, 0) for a deleted document."""
    row = db.execute(DOC_TEXT[doc[0]], (int(doc[1:]),)).fetchone()
    if row is None:
        return None, None, 0
    title, body, session = row
    text = (title + '\n' + body).strip() if title else body
    shown = text if len(text) <= MAX_DOC_CHARS else text[:MAX_DOC_CHARS] + '\n…[clipped]'
    return shown, session, len(text)


def covers(j, length):
    """A grade counts only if the judge saw as much of the document as this version shows."""
    return j.get('chars', OLD_CHARS) >= min(length, MAX_DOC_CHARS)


def pool(db, runs, q):
    """(doc, shown text, length): each run's top POOL_DEPTH after leaving out the question's session."""
    out = {}
    for per in runs.values():
        kept = 0
        for doc in per.get(q['qid'], []):
            if kept == POOL_DEPTH:
                break
            text, session, length = doc_text(db, doc)
            if text is None or session == q['session']:
                continue
            kept += 1
            out.setdefault(doc, (doc, text, length))
    return list(out.values())


def latest(judge=JUDGE):
    """The last grade per (question, document) from this judge."""
    path = f'{E}/judgments.jsonl'
    if not os.path.exists(path):
        return {}
    names = {judge, 'claude-sonnet'} if judge == MODEL else {judge}
    return {(j['qid'], j['doc']): j for j in map(json.loads, open(path)) if j['judge'] in names}


def ask(query, docs):
    listing = '\n\n'.join(f'[{d}]\n{t}' for d, t in docs)
    prompt = PROMPT.format(query=query, docs=listing)
    env = {k: v for k, v in os.environ.items()
           if k != 'CLAUDECODE' and not any(s in k for s in ('TOKEN', 'KEY', 'SECRET', 'PASSWORD'))}
    env['OBOETE_SKIP'] = '1'
    with tempfile.TemporaryDirectory() as cwd:
        r = subprocess.run(
            ['claude', '-p', '--model', MODEL, '--setting-sources', '', '--tools', '', '--strict-mcp-config',
             '--no-session-persistence', '--settings', '{"disableAllHooks":true}', '--output-format', 'json'],
            input=prompt, capture_output=True, text=True, cwd=cwd, env=env, timeout=300)
    if r.returncode != 0:
        raise RuntimeError(r.stderr[-300:] or r.stdout[-300:])
    answer = json.loads(r.stdout)
    used = sorted((answer.get('modelUsage') or {}).keys())
    if used != [MODEL]:
        raise RuntimeError(f'answered by {used}, not {MODEL}')
    result = answer['result']
    m = re.search(r'\{.*\}', result, re.S)
    if not m:
        raise RuntimeError(f'no JSON in answer: {result[:120]}')
    grades = json.loads(m.group(0))['grades']
    want = {d for d, _ in docs}
    out = {g['id'].strip('[]'): int(g['grade']) for g in grades if g['id'].strip('[]') in want}
    if not out or not all(0 <= v <= 3 for v in out.values()):
        raise RuntimeError(f'no usable grades: {len(out)}/{len(want)}')
    # A skipped document stays unjudged and goes into a smaller batch on the next run.
    return out


def main():
    split, max_questions = sys.argv[1], int(sys.argv[2])
    max_calls = int(sys.argv[3]) if len(sys.argv) > 3 else 600
    db = sqlite3.connect(f'file:{E}/home/oboete.db?mode=ro', uri=True)
    queries = [json.loads(l) for l in open(f'{E}/queries.jsonl')]
    queries = [q for q in queries if q['split'] == split][:max_questions]
    runs = load_runs()
    done = latest()
    path = f'{E}/judgments.jsonl'
    jobs = []
    for q in queries:
        todo = [(d, t) for d, t, n in pool(db, runs, q)
                if not ((q['qid'], d) in done and covers(done[(q['qid'], d)], n))]
        for i in range(0, len(todo), BATCH):
            jobs.append((q, todo[i:i + BATCH]))
    print(f'{len(queries)} questions, {len(jobs)} calls needed, running {min(len(jobs), max_calls)}', flush=True)
    jobs = jobs[:max_calls]
    failed = 0
    with open(path, 'a') as w, concurrent.futures.ThreadPoolExecutor(2) as ex:
        futures = {ex.submit(ask, q['text'], docs): (q, docs) for q, docs in jobs}
        for n, f in enumerate(concurrent.futures.as_completed(futures), 1):
            q, batch = futures[f]
            try:
                grades = f.result()
            except Exception as e:
                failed += 1
                print(f'failed {q["qid"]}: {str(e)[:120]}', flush=True)
                continue
            if len(grades) < len(batch):
                print(f"partial {q['qid']}: {len(grades)}/{len(batch)}", flush=True)
            for doc, grade in grades.items():
                w.write(json.dumps({'qid': q['qid'], 'doc': doc, 'grade': grade, 'judge': JUDGE,
                                    'chars': MAX_DOC_CHARS, 'ts': int(time.time())}) + '\n')
            w.flush()
            if n % 10 == 0:
                print(f'{n}/{len(jobs)} calls', flush=True)
    print(f'done; failed calls: {failed}')


if __name__ == '__main__':
    main()
