"""M6 lookup on D12's fixed questions (docs/milestone-4.md, Task 12a).

Only a correct answer with live evidence counts. The answerer's results, keys and model
calls stay in the owner-only evaluation directory, never in the report.
"""
import argparse, collections, json, os, re, shutil, sqlite3, sys, tempfile, tomllib
from datetime import datetime, timedelta, timezone

import common
import m3

ANSWERER = 'claude-sonnet-5'

KEY = """You write the answer key for a question about a developer's earlier coding session.

Question (asked on {asked_at}):
<<<
{question}
>>>

Records from that session, numbered:
{records}

Answer the question from these records only, as briefly as it allows, in the question's language, and name the records that state the answer.
Answer with JSON only: {{"answer": "<answer>", "records": [<record number>, ...]}}, or {{"answer": null, "records": []}} if these records do not answer it.
"""

CHECK = """Here are a question about a developer's earlier coding session, records from that session, and an answer key written from them.

Question (asked on {asked_at}):
<<<
{question}
>>>

Records, numbered:
{records}

Answer key:
<<<
{key}
>>>

Does the answer key answer the question correctly from these records?
Answer with JSON only: {{"agree": true}} or {{"agree": false, "why": "<one sentence>"}}.
"""

QUERY = """Today is {asked_at}. A developer asks a coding agent about earlier work:
<<<
{question}
>>>

Write the search that finds the answer in the developer's memory of earlier sessions: a short query in the words those sessions would use, and the date range the question names, if it names one.
Answer with JSON only: {{"query": "<query>", "since": "<YYYY-MM-DD>" or null, "until": "<YYYY-MM-DD>" or null}}.
"""

ANSWER = """Today is {asked_at}. A developer asks:
<<<
{question}
>>>

Search results from the developer's memory of earlier sessions, each with its id:
{results}

Answer from these results only, in the question's language, and cite the ids of the results that state the answer.
Answer with JSON only: {{"answer": "<answer>", "cites": ["<id>", ...]}}, or {{"answer": null, "cites": []}} if they do not answer it.
"""

CORRECT = """Question:
<<<
{question}
>>>

Reference answer:
<<<
{key}
>>>

Candidate answer:
<<<
{answer}
>>>

Does the candidate give the reference answer without contradicting it? More detail is fine; another answer, or none, is not.
Answer with JSON only: {{"correct": true}} or {{"correct": false}}.
"""

HOLDS = """Question:
<<<
{question}
>>>

Reference answer:
<<<
{key}
>>>

Passage:
<<<
{span}
>>>

Does this passage by itself state the reference answer to the question?
Answer with JSON only: {{"holds": true}} or {{"holds": false}}.
"""


def directory():
    return f'{common.E}/m6'


def fixture(session):
    """Preserve physical line numbers, including blank lines; U+2028 is part of JSON text."""
    with open(f'{m3.M}/fixtures/{session}.jsonl', encoding='utf-8') as f:
        return {i: json.loads(line) for i, line in enumerate(f) if line.strip()}


def pool_sessions(pool, decide=None):
    common.guard([], decide, pool)
    with open(f'{common.E}/replay/manifest.json', encoding='utf-8') as f:
        manifest = json.load(f)['sessions']
    sides = {'test', 'held-out'} if pool == 'test' else {pool}
    return {s['session'] for s in manifest if s['side'] in sides}


def guard_home(home, arm='b', decide=None, pool='dev'):
    """Search is --all; questions being dev does not make every record in the home dev."""
    m3.guard_home(home, decide, pool)
    if arm == 'v1':
        wal = f'{home}/oboete.db-wal'
        if os.path.exists(wal) and os.path.getsize(wal):
            sys.exit('v1 needs a frozen checkpointed baseline; its original WAL is never opened')
        with sqlite3.connect(f'file:{home}/oboete.db?mode=ro&immutable=1', uri=True) as db:
            sessions = [s for s, in db.execute('SELECT id FROM sessions')]
        common.guard(sessions, decide, pool)


def questions(pool='dev', decide=None):
    """Check the drafted set without printing its text or reading the 112 search questions."""
    wanted = pool_sessions(pool, decide)
    rows = common.read_jsonl(f'{directory()}/questions-{pool}.jsonl')
    common.guard([q.get('session', '') for q in rows], decide, pool)
    if len(rows) != 40 or len({q.get('id') for q in rows}) != 40:
        sys.exit('M6 requires 40 questions with distinct ids')
    if pool == 'dev' and len(wanted) != 30:
        sys.exit('D12 requires the manifest\'s 30 dev sessions')
    per = collections.Counter(q.get('session') for q in rows)
    if set(per) - wanted or max(per.values()) > 2:
        sys.exit('M6 requires manifest sessions and at most two questions per session')
    times = {q.get('asked_at') for q in rows}
    if len(times) != 1 or not all(isinstance(t, str) for t in times):
        sys.exit('M6 requires one asked_at for the whole set')
    events = {s: fixture(s) for s in sorted(wanted)}
    latest = max(m3.when(e['ts']) for records in events.values() for e in records.values())
    next_day = datetime.fromtimestamp(latest, timezone.utc).date() + timedelta(days=1)
    try:
        asked = datetime.fromisoformat(next(iter(times)).replace('Z', '+00:00')).date()
    except ValueError:
        sys.exit('M6 asked_at must be an ISO date or time')
    if asked != next_day:
        sys.exit('M6 asked_at must be the day after the last event of the pool')
    for q in rows:
        if (not isinstance(q.get('id'), str) or not q['id']
                or not isinstance(q.get('question'), str) or not q['question'].strip()
                or type(q.get('dated')) is not bool or type(q.get('record')) is not int
                or q['record'] not in events[q['session']]):
            sys.exit('M6 question fields or source line are invalid')
    return rows


def record_text(event):
    """Render the event's content as text; hook routing metadata is not answer evidence."""
    payload = event.get('payload', {})
    metadata = {'session_id', 'session', 'cwd', 'transcript_path', 'hook_event_name'}
    return '\n'.join(m3.strings({k: v for k, v in payload.items() if k not in metadata}))


def key_context(question, binary):
    """Blank lines keep their numbers but are not neighboring records."""
    records = fixture(question['session'])
    lines = list(records)
    at = lines.index(question['record'])
    return {i: common.gate(record_text(records[i]), binary)[:4000]
            for i in lines[max(0, at - 2):at + 3]}


def mapped_record(raw, session, line, event, binary):
    """Match the quote, else its first 12 characters, nearest the fixture's time, as map_labels does."""
    content = {k: v for k, v in event['payload'].items()
               if k not in {'session_id', 'session', 'cwd', 'transcript_path', 'hook_event_name'}}
    strings = sorted((common.gate(t, binary) for t in m3.strings(content) if t.strip()), key=len, reverse=True)
    records = [r for r in m3.records_of(raw, session) if r[2] not in m3.REPEATS]
    for match in ('quote', 'prefix'):
        hits = [(seq, ts) for seq, ts, _, texts in records
                if any((t if match == 'quote' else t[:12]) in text for t in strings for text in texts)]
        if hits:
            seq, _ = min(hits, key=lambda r: (abs(r[1] - int(m3.when(event['ts']) * 1000)), r[0]))
            devices = raw.execute("SELECT device FROM records WHERE seq = ? AND session = ? AND type = 'event'",
                                  (seq, session)).fetchall()
            if len(devices) == 1:
                return {'record': line, 'device': devices[0][0], 'seq': seq, 'match': match}
            break
    raise ValueError('Source record cannot be mapped uniquely')


def keys(binary, home, pool='dev', decide=None):
    """Write and spot-check keys before compression makes the label mapper unreadable."""
    qs = questions(pool, decide)
    guard_home(home, decide=decide, pool=pool)
    common.owner_only()
    cache, rows, checks = common.Calls(), [], []
    with sqlite3.connect(f'file:{home}/raw.db?mode=ro', uri=True) as raw:
        packed = raw.execute("SELECT COUNT(*) FROM records WHERE type = 'event' AND enc != 'plain'").fetchone()[0]
        if packed:
            sys.exit(f'{packed} records are compressed: keys run after replay and before the worker')
        for q in qs:
            row = {'id': q['id'], 'session': q['session'], 'writer': common.draw('m6-key', q['id'])}
            try:
                records = fixture(q['session'])
                shown = key_context(q, binary)
                inputs = dict(asked_at=common.gate(q['asked_at'], binary),
                              question=common.gate(q['question'], binary),
                              records='\n\n'.join(f'[{i}]\n{text}' for i, text in shown.items()))

                def usable(answer):
                    return (set(answer) == {'answer', 'records'} and (answer['answer'] is None
                            or isinstance(answer['answer'], str) and bool(answer['answer'].strip()))
                            and isinstance(answer['records'], list)
                            and all(type(i) is int and i in shown for i in answer['records'])
                            and (bool(answer['records']) if answer['answer'] is not None else not answer['records']))

                key = cache.call(row['writer'], KEY.format(**inputs), usable)
                agree = None
                if common.checked('m6-check', q['id']):
                    checker = common.draw('m6-checker', q['id'], exclude=row['writer'])
                    check = cache.call(checker, CHECK.format(**inputs, key=common.gate(
                        json.dumps(key, ensure_ascii=False), binary)), lambda a: type(a.get('agree')) is bool)
                    row['checker'] = checker
                    agree = check['agree']
                    checks.append(agree)
                row.update(answer=key['answer'], records=[mapped_record(raw, q['session'], i, records[i], binary)
                                                         for i in dict.fromkeys(key['records'])],
                           checked=common.checked('m6-check', q['id']), agree=agree,
                           rejected=key['answer'] is None or agree is False)
            except (common.FailedCall, OSError, ValueError, RuntimeError):
                row['failed'] = True
            rows.append(row)
            common.write_jsonl(f'{directory()}/keys-{pool}.jsonl', rows)
    out = {'n': len(qs), 'mapped': sum(len(r.get('records', [])) for r in rows),
           'rejected': sum(bool(r.get('rejected')) for r in rows),
           'failed': sum(bool(r.get('failed')) for r in rows),
           'checks': {'n': len(checks), 'agree': sum(checks), 'rate': sum(checks) / len(checks) if checks else None}}
    common.keep_json(f'{directory()}/keys-{pool}.json', dict(common.record(binary, home, len(qs), 'off', cache.models()),
                                                          **out))
    return out


def search(binary, home, query, arm):
    """v1 has no time options; keep their loss per question instead of pretending it filtered."""
    argv = [binary, '--home', home, 'search', '--all', '--limit', '10']
    dropped = {'since': 0, 'until': 0, 'questions': 0}
    for field in ('since', 'until'):
        if query.get(field):
            if arm == 'v1':
                dropped[field] = 1
            else:
                argv += ['--' + field, query[field]]
    dropped['questions'] = int(bool(dropped['since'] or dropped['until']))
    return common.command(argv + ['--', query['query']]), dropped


def valid_query(answer):
    if (set(answer) != {'query', 'since', 'until'} or not isinstance(answer['query'], str)
            or not answer['query'].strip()):
        return False
    try:
        for f in ('since', 'until'):
            if answer[f] is not None:
                if not isinstance(answer[f], str) or not re.fullmatch(r'\d{4}-\d{2}-\d{2}', answer[f]):
                    return False
                datetime.fromisoformat(answer[f])
    except ValueError:
        return False
    return not answer['since'] or not answer['until'] or answer['since'] <= answer['until']


def valid_answer(answer):
    return (set(answer) == {'answer', 'cites'} and (answer['answer'] is None
            or isinstance(answer['answer'], str)) and isinstance(answer['cites'], list)
            and all(isinstance(c, str) and c and '\n' not in c for c in answer['cites'])
            and (answer['answer'] is not None or not answer['cites']))


def hit_lines(text, arm):
    """Parse only hit headers; a v1 title's continuation and U+2028 never become extra hits."""
    hits = []
    for line in text.split('\n'):
        match = re.match(r'^(\S+)\s+\d{4}-\d{2}-\d{2}\s', line)
        if not match:
            continue
        uid = match[1]
        if arm == 'v1':
            label = 'citable' if re.fullmatch(r'p\d+', uid) else 'attributed-only'
        else:
            found = re.search(r'\((citable|quote-only|imported)\)', line.partition(' — ')[0])
            label = found[1] if found else 'unlabelled'
        imported = bool(re.fullmatch(r'.+:[osp]\d+', uid))
        hits.append({'id': uid, 'label': label, 'line': line,
                     'unlabelled_imported': int(arm != 'v1' and imported and label != 'imported')})
    return hits[:10]


def vector_side(home):
    """D12's home has no embedder; do not record full text for a configured vector run."""
    path = f'{home}/config.toml'
    with open(path, 'rb') as f:
        settings = tomllib.load(f)
    provider = settings.get('embedding', {}).get('provider', 'none')
    if provider != 'none':
        sys.exit('D12 requires a home without an embedder')
    return 'off'


def copy_home(home):
    """v1's db::open migrates schemas, so every invocation gets its own writable copy."""
    parent = f'{directory()}/homes'
    os.makedirs(parent, mode=0o700, exist_ok=True)
    if os.path.commonpath([os.path.abspath(parent), os.path.abspath(home)]) == os.path.abspath(home):
        sys.exit('v1 copy destination must be outside its source home')
    copied = tempfile.mkdtemp(prefix='v1-', dir=parent)
    shutil.copytree(home, copied, dirs_exist_ok=True)
    for root, _, files in os.walk(copied):
        os.chmod(root, 0o700)
        for name in files:
            os.chmod(os.path.join(root, name), 0o600)
    return copied


def run(binary, home, arm, pool='dev', decide=None):
    """Keep each completed item immediately; cached calls finish an interrupted item on resume."""
    qs = questions(pool, decide)
    common.owner_only()
    binary, home = os.path.abspath(binary), os.path.abspath(home)
    key_rows = common.read_jsonl(f'{directory()}/keys-{pool}.jsonl')
    keyed = {k['id']: k for k in key_rows}
    if set(keyed) != {q['id'] for q in qs} or any(k.get('rejected') or k.get('failed') for k in key_rows):
        sys.exit('M6 needs a checked key for every question before running an arm')
    path = f'{directory()}/runs/{arm}{"" if pool == "dev" else "-" + pool}.jsonl'
    fingerprint = {'sha256': common.sha256_file(binary), 'source_home': home,
                   'questions_sha256': common.sha256_file(f'{directory()}/questions-{pool}.jsonl'),
                   'keys_sha256': common.sha256_file(f'{directory()}/keys-{pool}.jsonl')}
    previous = common.read_jsonl(path) if os.path.exists(path) else []
    if any(r.get('input') != fingerprint for r in previous):
        sys.exit('M6 run inputs changed; keep the old run and choose a new evaluation directory')
    rows = {r['id']: r for r in previous}
    guard_home(home, arm, decide, pool)
    opened = copy_home(home) if arm == 'v1' else home
    guard_home(opened, arm, decide, pool)
    vector = vector_side(opened) if os.path.exists(f'{opened}/config.toml') else 'off'
    cache = common.Calls()
    model_names = {}
    key_metadata = f'{directory()}/keys-{pool}.json'
    if os.path.exists(key_metadata):
        with open(key_metadata, encoding='utf-8') as f:
            model_names = json.load(f).get('models', {})
    for q in qs:
        if q['id'] in rows and not rows[q['id']].get('failed'):
            continue
        row = dict(id=q['id'], session=q['session'], dated=q['dated'], question=q['question'],
                   asked_at=q['asked_at'], input=fingerprint, binary=binary, home=opened)
        try:
            question = common.gate(q['question'], binary)
            asked_at = common.gate(q['asked_at'], binary)
            query = cache.call(ANSWERER, QUERY.format(question=question, asked_at=asked_at),
                               valid_query, answerer=True)
            query = {k: common.gate(v, binary).strip() if v is not None else None for k, v in query.items()}
            if not valid_query(query):
                raise ValueError('Gated query is invalid')
            text, dropped = search(binary, opened, query, arm)
            hits = hit_lines(text, arm)
            gets = {hit['id']: common.command([binary, '--home', opened, 'get', hit['id']])
                    for hit in hits[:3]}
            results = common.gate(text, binary)[:4000] + '\n\n' + '\n\n'.join(
                common.gate(t, binary)[:4000] for t in gets.values())
            answer = cache.call(ANSWERER, ANSWER.format(question=question, asked_at=asked_at, results=results),
                                valid_answer, answerer=True)
            row.update(answer=answer['answer'], cites=answer['cites'], query=query,
                       hits=hits, gets=gets, dropped_dates=dropped,
                       unlabelled_imported=sum(h['unlabelled_imported'] for h in hits))
        except (common.FailedCall, OSError, ValueError, RuntimeError):
            row['failed'] = True
        rows[q['id']] = row
        common.write_jsonl(path, [rows[q['id']] for q in qs if q['id'] in rows])
        for model, names in cache.models().items():
            model_names[model] = sorted(set(model_names.get(model, [])) | set(names))
        common.keep_json(path[:-6] + '.json', dict(common.record(binary, home, len(qs), vector, model_names),
                                                input=fingerprint, completed=sum(not r.get('failed') for r in rows.values()),
                                                read_homes=sorted({r['home'] for r in rows.values()})))
    return {'n': len(qs), 'completed': sum(not r.get('failed') for r in rows.values()),
            'failed': sum(bool(r.get('failed')) for r in rows.values())}


def full_uid(home, prefix):
    """Search prints 12 characters; cite requires the full claim uid."""
    with sqlite3.connect(f'file:{home}/knowledge.db?mode=ro', uri=True) as db:
        matches = [u for u, in db.execute('SELECT uid FROM active WHERE uid LIKE ?', (prefix + '%',))]
    if len(matches) != 1:
        raise ValueError('Cited claim is absent or ambiguous')
    return matches[0]


def score_answer(row, key, arm, cache):
    """Evidence is checked at scoring time; attributed-only citations never create a span."""
    out = {'id': row['id'], 'dated': row['dated'], 'correct': None, 'hits': [],
           'unlabelled_imported': row.get('unlabelled_imported', 0),
           'dropped_dates': row.get('dropped_dates', {}), 'complete': False}
    if row.get('failed'):
        return out
    binary, home = row['binary'], row['home']
    inputs = {'question': common.gate(row['question'], binary), 'key': common.gate(key['answer'], binary)}
    votes = cache.votes(CORRECT.format(**inputs, answer=common.gate(row['answer'] or 'null', binary)), ('correct',))
    out.update(correct=common.voted(votes, 'correct'), correct_votes=votes)
    hits = {h['id']: h for h in row['hits']}
    seen = set()
    for cited in dict.fromkeys(row['cites']):
        if cited not in hits and arm != 'v1' and re.fullmatch(r'[0-9a-fA-F]{64}', cited):
            for hit in row['hits']:
                if re.fullmatch(r'[0-9a-fA-F]{12}', hit['id']) and cited.startswith(hit['id']):
                    if full_uid(home, hit['id']) == cited:
                        hits[cited] = hit
                        break
        if cited not in hits:
            out.setdefault('unknown_cites', []).append(cited)
            continue
        hit = hits[cited]
        if hit['id'] in seen:
            continue
        seen.add(hit['id'])
        scored = {'id': cited, 'label': hit['label'], 'spans': []}
        out['hits'].append(scored)
        if hit['label'] != 'citable':
            continue
        try:
            if arm == 'v1' or re.fullmatch(r'.+:\d+', cited):
                text = row['gets'].get(cited)
                if text is None:
                    text = common.command([binary, '--home', home, 'get', cited])
                evidence = [{'live': True, 'quote': text}]
            else:
                uid = full_uid(home, cited)
                cited_rows = json.loads(common.command([binary, '--home', home, 'cite', uid]))
                if len(cited_rows) != 1 or cited_rows[0].get('error'):
                    raise ValueError('Cited claim cannot be read')
                cited_claim = cited_rows[0]
                if cited_claim['label'] != 'citable':
                    scored['label'] = cited_claim['label']
                    continue
                evidence = cited_claim['evidence']
                if not evidence:
                    raise ValueError('Cited claim has no evidence')
            for e in evidence:
                span = {'live': e['live'] is True, 'holds': False}
                if span['live']:
                    v = cache.votes(HOLDS.format(**inputs, span=common.gate(e['quote'], binary)), ('holds',))
                    span.update(holds=common.voted(v, 'holds'), votes=v)
                scored['spans'].append(span)
        except (OSError, ValueError, RuntimeError, sqlite3.Error, KeyError, TypeError):
            out['failed'] = True
    out['complete'] = (not out.get('failed') and out['correct'] is not None
                       and all(s['holds'] is not None for h in out['hits'] for s in h['spans']))
    return out


def score(arm, against=None, pool='dev', decide=None):
    """Grade from kept inputs; completed calls across every arm share the same prompt cache."""
    common.guard([], decide, pool)
    path = f'{directory()}/runs/{arm}{"" if pool == "dev" else "-" + pool}.jsonl'
    rows = common.read_jsonl(path)
    common.guard([r['session'] for r in rows], decide, pool)
    qs = questions(pool, decide)
    keys = {k['id']: k for k in common.read_jsonl(f'{directory()}/keys-{pool}.jsonl')}
    if set(keys) != {q['id'] for q in qs} or any(k.get('rejected') or k.get('failed') for k in keys.values()):
        sys.exit('M6 keys are incomplete or rejected')
    if len(rows) != len(qs) or {r['id'] for r in rows} != set(keys):
        sys.exit('M6 run is incomplete')
    inputs = {'questions_sha256': common.sha256_file(f'{directory()}/questions-{pool}.jsonl'),
              'keys_sha256': common.sha256_file(f'{directory()}/keys-{pool}.jsonl')}
    if any(any(r['input'].get(k) != v for k, v in inputs.items()) for r in rows):
        sys.exit('M6 run inputs changed; keys and questions must stay fixed')
    if any(common.sha256_file(r['binary']) != r['input']['sha256'] for r in rows):
        sys.exit('M6 binary changed after the run')
    cache = common.Calls()
    scored = []
    for row in rows:
        guard_home(row['home'], arm, decide, pool)
        scored.append(score_answer(row, keys[row['id']], arm, cache))
        common.write_jsonl(path[:-6] + '.grades.jsonl', scored)
    baseline = None
    if arm == 'b' or against == 'v1':
        base = f'{directory()}/runs/v1{"" if pool == "dev" else "-" + pool}.score.json'
        if os.path.exists(base):
            with open(base, encoding='utf-8') as f:
                reference = json.load(f)
            if reference.get('complete') and reference.get('n') == 40 and reference.get('input') == inputs:
                baseline = reference['count']
    out = report(scored, baseline, arm)
    out['input'] = inputs
    out['against_v1'] = baseline
    out['comparison_required'] = arm == 'b'
    if arm == 'b' and baseline is None:
        out['pass'] = False
    correct = [r['correct_votes'] for r in scored if 'correct_votes' in r]
    holds = [s['votes'] for r in scored for h in r['hits'] for s in h['spans'] if 'votes' in s]
    if out['complete']:
        out['agreement'] = {'correct': common.agreement(correct, 'correct'), 'holds': common.agreement(holds, 'holds')}
    with open(path[:-6] + '.json', encoding='utf-8') as f:
        metadata = json.load(f)
    out['models'] = metadata['models'].copy()
    for model, names in cache.models().items():
        out['models'][model] = sorted(set(out['models'].get(model, [])) | set(names))
    out['run'] = metadata
    common.keep_json(path[:-6] + '.score.json', out)
    return out


def counted(correct, spans):
    """An incomplete vote cannot count, even if another passage has already passed."""
    return (correct is True and all(s['holds'] is not None for s in spans)
            and any(s['live'] and s['holds'] for s in spans))


def report(rows, against=None, arm='b'):
    """The 0.70 and +0.10 lines are integer counts, not rounded answer rates."""
    complete = (all(r['correct'] is not None and r.get('complete', True) for r in rows)
                and all(s['holds'] is not None for r in rows for h in r['hits'] if h['label'] == 'citable'
                        for s in h.get('spans', [])))
    if not complete:
        return {'n': len(rows), 'complete': False, 'pass': False,
                'pending': sum(r['correct'] is None or not r.get('complete', True) for r in rows)}
    spans = [s for r in rows for hit in r['hits'] if hit['label'] == 'citable'
             for s in hit.get('spans', [])]
    counts = [counted(r['correct'], [s for hit in r['hits'] if hit['label'] == 'citable'
                                   for s in hit.get('spans', [])]) for r in rows]
    total = sum(counts)
    valid = sum(s['live'] and s['holds'] is True for s in spans)
    attributed = sum(bool(r['hits']) and all(hit['label'] != 'citable' for hit in r['hits']) for r in rows)
    unlabelled = sum(r.get('unlabelled_imported', 0) for r in rows)
    dated_n = sum(r['dated'] for r in rows)
    dated_count = sum(c for r, c in zip(rows, counts) if r['dated'])
    dated_pass = bool(dated_n and dated_count * 10 >= dated_n * 7)
    passed = bool(complete and len(rows) == 40 and total >= 28
                  and (arm == 'v1' or against is not None and total >= against + 4)
                  and spans and valid * 100 >= len(spans) * 95 and dated_pass)
    return {'n': len(rows), 'count': total, 'complete': complete, 'attributed_only': attributed,
            'unlabelled_imported': unlabelled,
            'dated': {'n': dated_n, 'count': dated_count,
                      'rate': dated_count / dated_n if dated_n else None,
                      'line': 0.70, 'pass': dated_pass},
            'dropped_dates': {field: sum(r.get('dropped_dates', {}).get(field, 0) for r in rows)
                              for field in ('since', 'until', 'questions')},
            'validity': {'n': len(spans), 'valid': valid,
                         'rate': valid / len(spans) if spans else None, 'line': 0.95},
            'pass': complete and unlabelled == 0 and (arm == 'b-cmem' or passed)}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    for name in ('questions', 'keys', 'run', 'score'):
        p = sub.add_parser(name)
        p.add_argument('--pool', choices=('dev', 'test'), default='dev')
        p.add_argument('--decide')
        if name in ('keys', 'run'):
            p.add_argument('binary')
            p.add_argument('home')
        if name == 'run':
            p.add_argument('--arm', choices=('b', 'v1', 'b-cmem'), required=True)
        if name == 'score':
            p.add_argument('arm', choices=('b', 'v1', 'b-cmem'))
            p.add_argument('--against', choices=('v1',))
    args = parser.parse_args(argv)
    common.owner_only()
    try:
        if args.command == 'questions':
            qs = questions(args.pool, args.decide)
            out = {'n': len(qs), 'sessions': len({q['session'] for q in qs}), 'dated': sum(q['dated'] for q in qs)}
        elif args.command == 'keys':
            out = keys(args.binary, args.home, args.pool, args.decide)
        elif args.command == 'run':
            out = run(args.binary, args.home, args.arm, args.pool, args.decide)
        else:
            out = score(args.arm, args.against, args.pool, args.decide)
    except (OSError, ValueError, RuntimeError, sqlite3.Error, KeyError, TypeError):
        sys.exit('M6 command failed; private text is not printed')
    print(json.dumps(out, ensure_ascii=False))


if __name__ == '__main__':
    main()
