"""PR-B2: grade pooled (question, document) pairs 0-3 with an LLM judge (docs/pr-b.md item 6).

Pool: for each question, the top POOL_DEPTH of every run in ~/.oboete/eval/runs, after leaving
out documents of the question's own session. Pairs already in judgments.jsonl are reused.
Judge: `claude -p --model sonnet` from a scratch cwd with user settings off, so neither oboete's
nor claude-mem's hooks run (OBOETE_SKIP=1 as well). Questions and documents were gated on the
way into the evaluation store (agent questions through `oboete gate`), so what is sent here
already passed the outbound gate.

usage: judge.py <split> <max questions> [max calls]
"""
import concurrent.futures, json, os, re, sqlite3, subprocess, sys, tempfile, time

E = os.path.expanduser('~/.oboete/eval')
POOL_DEPTH = 20
BATCH = 10
# Enough for 99.8% of the pooled documents; the rest are clipped and marked. Grades written
# before `chars` was recorded saw 1,200 characters.
MAX_DOC_CHARS = 4000
OLD_CHARS = 1200
JUDGE = 'claude-sonnet'

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
    for name in os.listdir(f'{E}/runs'):
        if not name.endswith('.trec'):
            continue
        per = runs.setdefault(name[:-5], {})
        for line in open(f'{E}/runs/{name}'):
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
    return {(j['qid'], j['doc']): j for j in map(json.loads, open(path)) if j['judge'] == judge}


def ask(query, docs):
    listing = '\n\n'.join(f'[{d}]\n{t}' for d, t in docs)
    prompt = PROMPT.format(query=query, docs=listing)
    env = dict(os.environ, OBOETE_SKIP='1')
    with tempfile.TemporaryDirectory() as cwd:
        r = subprocess.run(
            ['claude', '-p', '--model', 'sonnet', '--setting-sources', 'project', '--strict-mcp-config',
             '--no-session-persistence', '--output-format', 'json'],
            input=prompt, capture_output=True, text=True, cwd=cwd, env=env, timeout=300)
    if r.returncode != 0:
        raise RuntimeError(r.stderr[-300:] or r.stdout[-300:])
    result = json.loads(r.stdout)['result']
    m = re.search(r'\{.*\}', result, re.S)
    if not m:
        raise RuntimeError(f'no JSON in answer: {result[:120]}')
    grades = json.loads(m.group(0))['grades']
    want = {d for d, _ in docs}
    out = {g['id'].strip('[]'): int(g['grade']) for g in grades if g['id'].strip('[]') in want}
    if set(out) != want or not all(0 <= v <= 3 for v in out.values()):
        raise RuntimeError(f'incomplete grades: {len(out)}/{len(want)}')
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
            q, _ = futures[f]
            try:
                grades = f.result()
            except Exception as e:
                failed += 1
                print(f'failed {q["qid"]}: {str(e)[:120]}', flush=True)
                continue
            for doc, grade in grades.items():
                w.write(json.dumps({'qid': q['qid'], 'doc': doc, 'grade': grade, 'judge': JUDGE,
                                    'chars': MAX_DOC_CHARS, 'ts': int(time.time())}) + '\n')
            w.flush()
            if n % 10 == 0:
                print(f'{n}/{len(jobs)} calls', flush=True)
    print(f'done; failed calls: {failed}')


if __name__ == '__main__':
    main()
