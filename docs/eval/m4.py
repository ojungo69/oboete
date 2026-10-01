"""Milestone 4, Task 6: the test-split run on Design B (docs/milestone-4-plan.md D10, Task 6).
Every command that runs oboete takes the binary by path: `oboete` on PATH is the owner's v1.

  m4.py questions <split>          questions-<split>-m4.jsonl: the split's questions, and on test
                                   M21's 53 English ones (165)
  m4.py corpus                     corpus-m4.json: Raw's sessions and each agent's window
  m4.py replay <bin> <home>        the corpus's events before the copy time into <home> (b-m4),
                                   once
  m4.py run <bin> <home> <out>     `eval` of the test questions, the stores hashed before and after
  m4.py map <out> <runs>           <out>'s B runs into <runs>, imported uids as the v1 store's doc
                                   ids; b-rrf5 and b-only keep the Raw-eligible questions
  m4.py gate <runs> <home> <pre-registration commit> <owner's answer, ISO time> [--no-rerank]
                                   the checks before any grade; nothing is judged until it passes
"""
import collections, datetime, functools, glob, json, os, re, sqlite3, subprocess, sys, time, tomllib

from common import E, clean_env, h, owner_only, read_jsonl, sha256_file, write_jsonl

# The evaluation store's copy time: Raw's corpus ends here (spec 8.2 Raw).
CUT = datetime.datetime.fromisoformat('2026-09-24T09:56:46+09:00')
JST = datetime.timezone(datetime.timedelta(hours=9))
UUID = re.compile(r'([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.jsonl$')
# claude-mem runs its observer sessions here (~/.claude-mem/observer-sessions).
OBSERVER = os.path.expanduser('~/.claude-mem')
PART = 20_000  # events per replayed part: `oboete replay` reads a fixture whole
DEPTH = 50
ARMS = 'off,rrf:5,only'
RAW_RUNS = ('b-rrf5', 'b-only')
RUNS = ('b-off', 'b-rrf5', 'b-only', 'b-rerank', 'e0-trigram', 'hybrid-d2', 'claude-mem', 'claude-mem-nowindow')
# The questions M21 sets (spec 8.2): the 112 and 53 more English, so 60 English in all.
TEST_N, ENGLISH_N = 165, 60
# B's text and v1's text of a mapped document: the same document when they share this share of
# trigrams; the gate wants this share of a kind's sample to match (D10's match rate).
SAME_TEXT, MATCH_LINE, SAMPLE = 0.8, 0.95, 50
# The judge that passed calibration (spec 8.2 Judge), pinned for the run.
JUDGE = 'claude-sonnet-5'


def questions(split):
    """The split's questions as the run asks them: the 424's, and on test M21's 53 English
    (spec 8.2 M21); written to questions-<split>-m4.jsonl."""
    rows = [q for q in read_jsonl(f'{E}/queries.jsonl') if q['split'] == split]
    if split == 'test':
        rows += read_jsonl(f'{E}/queries-en.jsonl')
    seen = set()
    for q in rows:
        if q['qid'] in seen:
            raise ValueError(f'qid {q["qid"]} is used twice')
        seen.add(q['qid'])
    write_jsonl(f'{E}/questions-{split}-m4.jsonl', rows)
    return rows


@functools.cache
def v1_store(path):
    return sqlite3.connect(f'file:{path}?mode=ro', uri=True)


def v1():
    return v1_store(f'{E}/home/oboete.db')


@functools.cache
def v1_ids(path):
    """claude-mem's source ids to the v1 evaluation store's doc ids, through its `imports`."""
    db = v1_store(path)
    sources = [r[0] for r in db.execute('SELECT DISTINCT source FROM imports')]
    if sources != ['claude-mem']:
        raise ValueError(f'expected one imported claude-mem database, found {sources}')
    return dict(db.execute('SELECT source_id, doc FROM imports'))


def v1_doc(uid):
    """B's imported uid (`claude-mem:<database>:<source id>`) as the judged doc id (D10)."""
    source, _, source_id = uid.rpartition(':')
    if not source.startswith('claude-mem:'):
        raise KeyError(f'{uid} is not a claude-mem import')
    doc = v1_ids(f'{E}/home/oboete.db').get(source_id)
    if doc is None:
        raise KeyError(f'{uid} has no imports row in the v1 store')
    return doc


def day(ms):
    return datetime.datetime.fromtimestamp(ms / 1000, JST).date().isoformat()


def raw_eligible(q, window):
    """Whether Raw scores `q`: its session (the v1 store's `sessions`) is a Claude Code or Codex
    session started on or after its agent's first event day (spec 8.2 Raw, A70)."""
    row = v1().execute('SELECT agent, started_at FROM sessions WHERE id = ?', (q['session'],)).fetchone()
    return bool(row) and row[0] in window and day(row[1]) >= window[row[0]]


def when(s):
    return datetime.datetime.fromisoformat(s.replace('Z', '+00:00'))


def head(agent, path):
    """(working directory, time of the first event, the session a Codex rollout was forked from) of
    a transcript, read until the working directory and the time are known."""
    cwd = first = forked = None
    with open(path, encoding='utf-8', errors='replace') as f:
        for line in f:
            try:
                o = json.loads(line)
            except ValueError:
                continue
            if not isinstance(o, dict):
                continue
            if agent == 'claude' and cwd is None and isinstance(o.get('cwd'), str):
                cwd = o['cwd']
            elif agent == 'codex' and cwd is None and o.get('type') == 'session_meta' \
                    and isinstance(o.get('payload'), dict):
                cwd, forked = o['payload'].get('cwd'), o['payload'].get('forked_from_id')
            if first is None and isinstance(o.get('timestamp'), str):
                first = when(o['timestamp'])
            if cwd is not None and first is not None:
                break
    return cwd, first, forked


def transcripts(claude='~/.claude/projects', codex='~/.codex/sessions'):
    """(agent, session, path) of every Claude Code and Codex transcript; Claude Code's subagent
    files go with their session (`oboete transcript` reads them)."""
    for path in sorted(glob.glob(os.path.expanduser(f'{claude}/*/*.jsonl'))):
        yield 'claude', os.path.basename(path)[:-6], path
    for path in sorted(glob.glob(os.path.expanduser(f'{codex}/**/*.jsonl'), recursive=True)):
        if m := UUID.search(path):
            yield 'codex', m.group(1), path


def under(path, root):
    return path == root or path.startswith(root.rstrip('/') + '/')


def corpus(found, held_out):
    """Raw's corpus (D10): every transcript with an event before the copy time, but those run
    under /tmp, claude-mem's observer sessions and the replay set's held-out sessions; and each
    agent's window, from the first event day of all its transcripts. A forked Codex rollout is left
    out too: it opens with a copy of its parent's history, which would be replayed as the fork's
    own records (the live hooks never send it again), past the question's own-session rule and
    past the parent's exclusion (Codex on 67cf8f4); its own later turns go with it."""
    sessions, left, first = [], collections.Counter(), {}
    for agent, session, path in found:
        cwd, start, forked = head(agent, path)
        if start is None:
            left['no time'] += 1
            continue
        first[agent] = min(first.get(agent, start), start)
        why = ('late' if start >= CUT else 'tmp' if cwd and under(cwd, '/tmp')
               else 'observer' if cwd and under(cwd, OBSERVER) else 'held-out' if session in held_out
               else 'fork' if forked else None)
        if why:
            left[why] += 1
        else:
            sessions.append({'agent': agent, 'session': session, 'path': path})
    return {'cut': CUT.isoformat(), 'window': {a: t.astimezone(JST).date().isoformat() for a, t in first.items()},
            'sessions': sessions, 'left_out': dict(left)}


def held_out():
    with open(f'{E}/replay/manifest.json') as f:
        return {s['session'] for s in json.load(f)['sessions'] if s['side'] == 'held-out'}


def replay(binary, home, sessions):
    """The corpus's events before the copy time, each session's in time order, into `home` as live
    records (D10). The mark is written first, so a replay that stopped is never added to: the home
    is made again from b-import."""
    mark = f'{home}/replay-m4.json'
    with open(f'{home}/config.toml', 'rb') as f:
        config = tomllib.load(f)
    if config.get('summary', {}).get('curate') is not False or 'embedding' in config:
        raise RuntimeError('b-m4 curates nothing and has no embedding provider until the owner answers (D10)')
    report = {'binary': sha256_file(binary)[:12], 'sessions': 0, 'events': 0, 'parts': []}
    # Made only if absent, so of two replays started together one stops here (Codex on 67cf8f4).
    try:
        with open(mark, 'x') as f:
            json.dump({'started': report['binary']}, f)
    except FileExistsError:
        raise RuntimeError(f'{home} holds a Raw replay already: a second would insert its events twice') from None
    part = []

    def flush():
        path = f'{home}/part-m4.jsonl'
        with open(path, 'w', encoding='utf-8') as f:
            f.writelines(line + '\n' for line in part)
        r = subprocess.run([binary, '--home', home, 'replay', path, '--agent', 'all', '--spawn-sample', '0'],
                           capture_output=True, text=True, check=True, env=clean_env())
        report['parts'].append(json.loads(r.stdout))
        os.remove(path)
        part.clear()

    for s in sessions:
        r = subprocess.run([binary, 'transcript', s['path'], '--agent', s['agent']],
                           capture_output=True, text=True, check=True, env=clean_env())
        events = []
        # split('\n'), not splitlines(): a JSON string may hold U+2028, which splitlines() breaks on.
        for line in r.stdout.split('\n'):
            if line.strip() and (t := when(json.loads(line)['ts'])) < CUT:
                events.append((t, len(events), line))
        events.sort()
        part.extend(line for *_, line in events)
        report['sessions'] += 1
        report['events'] += len(events)
        if len(part) >= PART:
            flush()
    if part:
        flush()
    with open(mark, 'w') as f:
        json.dump(report, f, indent=1)
    return report


def hashes(binary, home):
    return {'binary': sha256_file(binary), 'raw.db': sha256_file(f'{home}/raw.db'),
            'knowledge.db': sha256_file(f'{home}/knowledge.db')}


def run(binary, home, out):
    """`eval` of the test questions on `home`, no worker running: the stores hashed before and after
    (Step 12), and the time it started, which the gate compares with the pre-registration."""
    started = time.time()
    before = hashes(binary, home)
    subprocess.run([binary, '--home', home, 'eval', f'{E}/questions-test-m4.jsonl', '--depth', str(DEPTH),
                    '--arms', ARMS, '--out', out], check=True, env=clean_env())
    with open(f'{out}/stores.json', 'w') as f:
        json.dump({'started': started, 'before': before, 'after': hashes(binary, home)}, f, indent=1)


def map_runs(out, runs, eligible):
    """B's runs in `out` into `runs`: an imported uid becomes the judged doc id, a record's `r:` key
    stays; b-rrf5 and b-only keep the `eligible` questions. Each file keeps its run's time, which
    the gate compares with the pre-registration. The sidecar and the store hashes go along."""
    os.makedirs(runs, exist_ok=True)
    doc = lambda key: key if key.startswith('r:') else v1_doc(key)
    for path in sorted(glob.glob(f'{out}/b-*.trec')):
        name = os.path.basename(path)
        target = f'{runs}/{name}'
        with open(path) as f, open(target + '.part', 'w') as w:
            for line in f:
                qid, q0, key, rank, score, tag = line.split()
                if name[:-5] in RAW_RUNS and qid not in eligible:
                    continue
                w.write(f'{qid} {q0} {doc(key)} {rank} {score} {tag}\n')
        os.replace(target + '.part', target)
        st = os.stat(path)
        os.utime(target, (st.st_atime, st.st_mtime))
    write_jsonl(f'{runs}/b-docs.jsonl', [dict(r, doc=doc(r['key'])) for r in read_jsonl(f'{out}/b-docs.jsonl')])
    with open(f'{out}/stores.json') as f, open(f'{runs}/stores.json', 'w') as w:
        w.write(f.read())


def sidecar(runs):
    """The run's sidecar by doc id: a record's text, session and time, which the v1 store lacks. A
    row with no doc id was never mapped: it is left out, and the gate reports it."""
    return {r['doc']: r for r in read_jsonl(f'{runs}/b-docs.jsonl') if 'doc' in r}


def record(rows, key):
    if key not in rows:
        raise KeyError(f'{key} has no sidecar row')
    return rows[key]


def load(runs, names):
    """{run: {qid: [doc, ...] by rank}} of the named runs present in `runs`."""
    out = {}
    for name in names:
        path = f'{runs}/{name}.trec'
        if not os.path.exists(path):
            continue
        ranked = collections.defaultdict(list)
        for line in open(path):
            qid, _, doc, rank, _, _ = line.split()
            ranked[qid].append((int(rank), doc))
        out[name] = {q: [d for _, d in sorted(v)] for q, v in ranked.items()}
    return out


def missing(runs, names, every, eligible):
    """Each run absent, and each question of its set it has no line for."""
    out = [f'{name}: no run' for name in names if name not in runs]
    for name, per in runs.items():
        want = eligible if name in RAW_RUNS else every
        out += [f'{name}: no line for {qid}' for qid in sorted(want - per.keys())]
    return out


# A v1 document's id as a mapped run holds it. judge.py reads the row by int(), so an alias such as
# o+1 or o01 would read o1's row under another id (Codex on 4cc0ad2).
V1_DOC = re.compile('[osp][1-9][0-9]*')


def unreadable(runs, side):
    """Each hit the judge cannot read as its run meant it: an imported uid with no v1 doc id, a record
    with no sidecar row, or any other id."""
    def why(doc):
        if doc.startswith('claude-mem:'):
            return 'unmapped'
        if doc.startswith('r:'):
            return None if doc in side else 'no sidecar row for'
        return None if V1_DOC.fullmatch(doc) else 'unsupported id'
    return [f'{name}: {why(doc)} {doc}' for name, per in runs.items() for docs in per.values()
            for doc in docs if why(doc)]


def session_of(side, doc_text):
    """The session of a hit, for the own-session check: a record's from the sidecar, a v1 document's
    from `doc_text`; None for a hit `unreadable` reports, so the gate lists its problems and never
    stops on one (OpenCodeReview on 606fadc)."""
    def of(doc):
        if doc.startswith('r:'):
            return side[doc]['session'] if doc in side else None
        return doc_text(doc)[1] if V1_DOC.fullmatch(doc) else None
    return of


def own_session(runs, asked, session_of):
    """Each hit of the question's own session: every leg leaves it out before its limit."""
    return [f'{name}: {qid} has {doc} of its own session'
            for name, per in runs.items() for qid, docs in per.items() for doc in docs
            if qid in asked and session_of(doc) == asked[qid]['session']]


def older(runs, names, since, started):
    """Each candidate run made, or the eval that made B's runs started, before the
    pre-registration merged (`since`, its main commit's time; Codex on 67cf8f4)."""
    out = [] if started > since else ['the eval started before the pre-registration']
    return out + [f'{name}: made before the pre-registration'
                  for name in names if name.startswith('b-') and os.path.exists(f'{runs}/{name}.trec')
                  and os.path.getmtime(f'{runs}/{name}.trec') <= since]


def unexpected(runs, names):
    """Each run file the gate does not check: judge.py and report.py read every one (Codex on
    67cf8f4)."""
    return [f'{n[:-5]}: a run the gate does not check' for n in sorted(os.listdir(runs))
            if n.endswith('.trec') and n[:-5] not in names]


def recordless(eligible, asked, home, corpus):
    """Each Raw question whose session the corpus replays and `home` holds no record of, or whose
    session the corpus does not hold (D10). The gate names the held-out sessions' questions apart."""
    k = sqlite3.connect(f'file:{home}/knowledge.db?mode=ro', uri=True)
    out = []
    for qid in sorted(eligible):
        session = asked[qid]['session']
        if session not in corpus:
            out.append(f'{qid}: its session is not in the corpus')
        elif not k.execute('SELECT 1 FROM raw_docs WHERE session = ? LIMIT 1', (session,)).fetchone():
            out.append(f'{qid}: no record of its session')
    return out


def trigrams(text):
    return {text[i:i + 3] for i in range(len(text) - 2)}


def matches(runs_dir, v1_text):
    """{kind: (matched, sampled)}: B's text of a seeded sample of mapped documents per kind against
    v1's text of the doc id it maps to, so the mapping holds the same documents (D10)."""
    by_kind = collections.defaultdict(list)
    for row in read_jsonl(f'{runs_dir}/b-docs.jsonl'):
        # A row that was never mapped has no `doc`: the gate reports it.
        if V1_DOC.fullmatch(row.get('doc', '')):
            by_kind[row['doc'][0]].append(row)
    out = {}
    for kind, rows in sorted(by_kind.items()):
        sample = sorted(rows, key=lambda r: h(f'm4-match:{r["doc"]}'))[:SAMPLE]
        same = 0
        for r in sample:
            a, b = trigrams(r['text']), trigrams(v1_text(r['doc']) or '')
            same += len(a & b) >= SAME_TEXT * max(1, min(len(a), len(b)))
        out[kind] = (same, len(sample))
    return out


def v1_own():
    """The documents v1 wrote into the evaluation store itself, with no imports row: B never
    returns them, so they leave every run and the qrels (D10; 132 on 2026-09-24)."""
    db = v1()
    return {f'{k}{i}' for k, table in (('o', 'observations'), ('s', 'summaries'), ('p', 'prompts'))
            for (i,) in db.execute(f"SELECT id FROM {table} WHERE '{k}' || id NOT IN (SELECT doc FROM imports)")}


def gate(runs_dir, home, commit, answered, rerank=True):
    """D10's gate: Raw's N and the slices first, then every check; the problems, or none."""
    from freeze import check
    os.environ.update({'OBOETE_EVAL_RUNS': runs_dir, 'OBOETE_EVAL_DEPTH': str(DEPTH),
                       'OBOETE_EVAL_QUESTIONS': f'{E}/questions-test-m4.jsonl'})
    import judge
    asked = {q['qid']: q for q in read_jsonl(f'{E}/questions-test-m4.jsonl')}
    with open(f'{E}/corpus-m4.json') as f:
        corpus = json.load(f)
    window = corpus['window']
    eligible = {qid for qid, q in asked.items() if raw_eligible(q, window)}
    by = collections.Counter((v1().execute('SELECT agent FROM sessions WHERE id = ?', (asked[q]['session'],))
                              .fetchone()[0], asked[q]['lang']) for q in eligible)
    print(f'Raw N = {len(eligible)}; by agent and language: {dict(sorted(by.items()))}')
    # The corpus leaves the replay set's held-out sessions out (D10): their questions count on
    # Raw's N with no record of their own session.
    held = held_out()
    apart = sorted(qid for qid in eligible if asked[qid]['session'] in held)
    print(f'of them, of held-out sessions (no records): {len(apart)} {apart}')
    print(f'questions {len(asked)}: ' + ', '.join(f'{k} {v}' for k, v in sorted(collections.Counter(
        f'{q["lang"]}/{q["set"]}' for q in asked.values()).items())))
    print(f'comparisons, before the questions with no answer leave: b-off/hybrid-d2 and b-rerank/b-off on '
          f'{len(asked)}; b-rrf5/b-off and b-only/b-off on {len(eligible)}')
    names = [n for n in RUNS if rerank or n != 'b-rerank']
    runs = load(runs_dir, names)
    side = sidecar(runs_dir)
    problems = [f'frozen: {b}' for b in check()] + unexpected(runs_dir, names)
    if len(asked) != TEST_N or sum(q['lang'] == 'en' for q in asked.values()) != ENGLISH_N:
        problems.append(f'the test questions are not {TEST_N} with {ENGLISH_N} English (spec 8.2 M21)')
    problems += missing(runs, names, set(asked), eligible)
    problems += unreadable(runs, side)
    problems += [f'b-docs.jsonl: no doc id for {r.get("key")}' for r in read_jsonl(f'{runs_dir}/b-docs.jsonl')
                 if 'doc' not in r]
    db = v1()
    session = session_of(side, lambda doc: judge.doc_text(db, doc))
    problems += own_session(runs, asked, session)
    # The positive control: a developer prompt's qid is its own prompt in the v1 store, of its session.
    q = next(q for q in asked.values() if q['set'] == 'prompt')
    if not own_session({'control': {q['qid']: [q['qid']]}}, asked, session):
        problems.append('the own-session check misses its positive control')
    since = int(subprocess.run(['git', 'show', '-s', '--format=%ct', commit], capture_output=True, text=True,
                               check=True, cwd=os.path.dirname(os.path.abspath(__file__)), env=clean_env()).stdout)
    with open(f'{runs_dir}/stores.json') as f:
        stores = json.load(f)
    problems += older(runs_dir, names, since, stores['started'])
    problems += recordless(eligible - set(apart), asked, home, {s['session'] for s in corpus['sessions']})
    k = sqlite3.connect(f'file:{home}/knowledge.db?mode=ro', uri=True)
    waiting = k.execute('SELECT COUNT(*) FROM vector_todo').fetchone()[0]
    held = k.execute("SELECT COUNT(*) FROM vector_keys WHERE skipped = 'held'").fetchone()[0]
    if waiting or held:
        problems.append(f'vectors not complete: {waiting} waiting, {held} held')
    if stores['before'] != stores['after']:
        problems.append('the stores changed during the runs')
    # The eval refuses a home with an exclusion or a claim (search::b::trec_run), and raw.db's hash
    # holds across the runs: the exclusion list was empty.
    p = sqlite3.connect(f'file:{home}/providers.db?mode=ro', uri=True)
    since_answer = int(datetime.datetime.fromisoformat(answered).timestamp() * 1000)
    early = p.execute("SELECT COUNT(*) FROM provider_calls WHERE role = 'embed' AND ts < ?", (since_answer,)).fetchone()[0]
    if early:
        problems.append(f'{early} embedding requests before the owner answered')
    for kind, (same, n) in matches(runs_dir, lambda d: judge.doc_text(db, d)[0]).items():
        print(f'match rate {kind}: {same}/{n}')
        if same < MATCH_LINE * n:
            problems.append(f'B text of kind {kind} matches v1 text in {same} of {n}')
    own = v1_own()
    pooled = {d for per in runs.values() for docs in per.values() for d in docs[:DEPTH] if d in own}
    print(f'v1-written documents: {len(own)}; in the pool: {len(pooled)} (they leave every run and the qrels)')
    # The model judge.py grades with and whose grades it reuses (`latest`): a change to judge.py
    # between the pre-registration and this gate shows here.
    if judge.JUDGE != JUDGE:
        problems.append(f'the judge is {judge.JUDGE}, not the pinned {JUDGE}')
    backup = f'{E}/judgments.jsonl.m4'
    if not (os.path.exists(backup) and sha256_file(backup) == sha256_file(f'{E}/judgments.jsonl')):
        problems.append(f'judgments.jsonl has no current backup at {backup}')
    # judge.py reads every run file, so a gate that failed may hand it a hit it cannot read.
    if not problems:
        print(f'judge.py would make {len(judge.jobs_for(db, list(asked.values()), judge.load_runs(), judge.latest()))} calls')
    return problems


if __name__ == '__main__':
    owner_only()
    cmd, args = sys.argv[1:2], sys.argv[2:]
    if cmd == ['questions'] and len(args) == 1:
        print(len(questions(args[0])), 'questions')
    elif cmd == ['corpus'] and not args:
        c = corpus(transcripts(), held_out())
        with open(f'{E}/corpus-m4.json.part', 'w') as f:
            json.dump(c, f, indent=1)
        os.replace(f'{E}/corpus-m4.json.part', f'{E}/corpus-m4.json')
        print(len(c['sessions']), 'sessions', c['window'], 'left out', c['left_out'])
    elif cmd == ['replay'] and len(args) == 2:
        with open(f'{E}/corpus-m4.json') as f:
            print(json.dumps({k: v for k, v in replay(*args, json.load(f)['sessions']).items() if k != 'parts'}))
    elif cmd == ['run'] and len(args) == 3:
        run(*args)
    elif cmd == ['map'] and len(args) == 2:
        with open(f'{E}/corpus-m4.json') as f:
            window = json.load(f)['window']
        map_runs(*args, {q['qid'] for q in read_jsonl(f'{E}/questions-test-m4.jsonl') if raw_eligible(q, window)})
    elif cmd == ['gate'] and len(args) in (4, 5):
        problems = gate(*args[:4], rerank=args[4:] != ['--no-rerank'])
        print('\n'.join(problems) or 'gate: pass')
        sys.exit(1 if problems else 0)
    else:
        sys.exit(__doc__)
