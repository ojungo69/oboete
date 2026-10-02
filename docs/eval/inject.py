"""D18's Inject sweep (docs/milestone-4.md, Task 12b).

pool checks the drafted questions; run sweeps a drained copy; label grades each pair
once; score applies the question-level Wilson line without making model calls.
All private results and temporary homes stay under OBOETE_EVAL/inject.
"""
import argparse, collections, copy, json, math, os, re, shutil, sqlite3, sys, tempfile, tomllib, uuid
from contextlib import closing
from pathlib import Path
from statistics import NormalDist

import common
import m3
import m6

THRESHOLDS = tuple(f'0.{n:02d}' for n in range(30, 91, 5))

INJECT = """A developer typed this prompt to a coding agent:
<<<
{prompt}
>>>

Before the agent saw it, a memory system added this note from earlier sessions:
<<<
{claim}
>>>

Does the note bear on the prompt: does it help answer it or carry it out, or does it correct a wrong belief in it?
Answer with JSON only: {{"relevant": true}} or {{"relevant": false}}.
"""


def path(name, pool, ext='jsonl'):
    return f'{common.E}/inject/{name}-{pool}.{ext}'


def load_pool(pool='dev', decide=None):
    common.guard(pool=pool, decide=decide)
    rows = common.read_jsonl(path('pool', pool))
    n = 50 if pool == 'dev' else 100
    if (len(rows) != 2 * n or any(not isinstance(r, dict) or any(
            not isinstance(r.get(f), str) or not r[f].strip() for f in ('qid', 'kind', 'text', 'repo'))
            for r in rows)
            or collections.Counter(r['kind'] for r in rows) != {'no-answer': n, 'false-premise': n}
            or any(len({r[f] for r in rows}) != len(rows) for f in ('qid', 'text'))):
        sys.exit('Inject pool needs balanced counts, unique qids/texts and a non-empty repo per row')
    return rows


def wilson_upper(count, n):
    """One-sided 95% Wilson upper bound, without rounding before comparison."""
    z = NormalDist().inv_cdf(0.95)
    p = count / n
    return (p + z * z / (2 * n) + z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n))) / (1 + z * z / n)


def pair_key(pair):
    return pair['source'], pair['qid'], pair['uid']


def pairs(rows):
    out = {}
    for row in rows:
        for claim in row['claims']:
            pair = dict(source=row['source'], qid=row['qid'], uid=claim['uid'],
                        prompt=row['text'], body=claim['body'])
            out[pair_key(pair)] = pair
    return out


def metrics(questions, rows, labels, pool='dev'):
    expected = {(q['source'], q['qid'], t) for q in questions for t in THRESHOLDS}
    observed = [(r['source'], r['qid'], r['threshold']) for r in rows]
    if len(observed) != len(expected) or set(observed) != expected:
        sys.exit('Inject run needs the complete question/threshold grid')
    wanted = pairs(rows)
    labelled = {pair_key(p): p for p in labels}
    if len(labelled) != len(labels) or set(labelled) - set(wanted) or any(
            any(p.get(f) != value for f, value in wanted[key].items()) for key, p in labelled.items()):
        sys.exit('Inject labels changed; they must describe the recorded pairs')
    decisions = {key: common.voted(labelled.get(key, {}).get('votes', {}), 'relevant') for key in wanted}
    pending = sum(v is None for v in decisions.values())
    if pending:
        return {'complete': False, 'pending': pending}
    thresholds = {}
    for t in THRESHOLDS:
        negative = [r for r in rows if r['threshold'] == t and r['source'] == 'inject']
        positive = [r for r in rows if r['threshold'] == t and r['source'] == 'm6']
        bad = sum(bool(r['claims']) and (r['kind'] == 'no-answer' or any(
            decisions[r['source'], r['qid'], c['uid']] is False for c in r['claims'])) for r in negative)
        upper = wilson_upper(bad, len(negative))
        relevant = sum(r['relevant'] for r in positive)
        thresholds[t] = {'n': len(negative), 'irrelevant': bad, 'share': bad / len(negative),
                         'upper95': upper, 'pass': upper <= 0.10,
                         'm6': {'n': len(positive), 'relevant': relevant, 'share': relevant / len(positive)}}
    chosen = next((t for t in THRESHOLDS if thresholds[t]['pass']), None) if pool == 'dev' else None
    return {'complete': True, 'thresholds': thresholds, 'chosen': chosen,
            'agreement': common.agreement([p['votes'] for p in labels], 'relevant')}


def input_hashes(pool):
    return {name: common.sha256_file(file) for name, file in (
        ('pool', path('pool', pool)), ('questions', f'{common.E}/m6/questions-{pool}.jsonl'),
        ('keys', f'{common.E}/m6/keys-{pool}.jsonl'))}


def read_db(home, name, immutable=False):
    uri = Path(home, name + '.db').as_uri() + '?mode=ro' + ('&immutable=1' if immutable else '')
    return closing(sqlite3.connect(uri, uri=True))


def frozen_home(home, pool, decide):
    """Read only metadata before copying. Refuse WAL writers and symlinks into another home."""
    for directory, dirs, files in os.walk(home):
        for name in dirs + files:
            p = Path(directory, name)
            if p.is_symlink() or name.endswith('-wal') and p.stat().st_size:
                sys.exit('Inject needs a frozen, checkpointed dev home without symlinks')
    with read_db(home, 'raw', immutable=True) as db:
        common.guard([r[0] for r in db.execute('SELECT DISTINCT session FROM records WHERE session IS NOT NULL')],
                     pool=pool, decide=decide)
    return {name: common.sha256_file(str(Path(home, name))) for name in ('raw.db', 'knowledge.db', 'config.toml')
            if Path(home, name).exists()}


def drain_config(home, per_prompt=False):
    """Disable model work and confine backups in the copy; preserve other TOML values."""
    config = Path(home, 'config.toml')
    text = config.read_text(encoding='utf-8') if config.exists() else ''
    expected = copy.deepcopy(tomllib.loads(text))
    for section, key, value in (('summary', 'curate', False), ('summary', 'shrink', False),
                                ('embedding', 'provider', 'none'), ('inject', 'per_prompt', per_prompt),
                                ('backup', 'dir', 'backups')):
        expected.setdefault(section, {})[key] = value
        assignment = f'{key} = {json.dumps(value)}\n'
        pattern = rf'(?ms)^\[{section}\][ \t]*(?:#[^\n]*)?\n(?P<body>.*?)(?=^\[|\Z)'
        match = re.search(pattern, text)
        if match:
            body = match['body']
            body = re.sub(rf'(?m)^{key}\s*=.*(?:\n|$)', '', body)
            text = text[:match.start('body')] + assignment + body + text[match.end('body'):]
        else:
            text += f'\n[{section}]\n{assignment}'
    if tomllib.loads(text) != expected:
        sys.exit('Cannot safely disable model work in the copied config')
    common.owner_only(str(config))
    config.write_text(text, encoding='utf-8')


def m6_inputs(home, qs, keys):
    keyed = {k['id']: k for k in keys}
    if (len(keyed) != len(keys) or set(keyed) != {q['id'] for q in qs}
            or any(k.get('rejected') or k.get('failed') or not k.get('records') for k in keys)):
        sys.exit('Inject needs a usable M6 key for every question')
    out = []
    with read_db(home, 'raw') as db:
        for q in qs:
            key = keyed[q['id']]
            repos = set()
            for record in key['records']:
                found = db.execute("SELECT repo FROM records WHERE device = ? AND seq = ? AND type = 'event'",
                                   (record['device'], record['seq'])).fetchall()
                if len(found) != 1 or not found[0][0]:
                    sys.exit('An M6 key record has no unique repository')
                repos.add(found[0][0])
            if len(repos) != 1:
                sys.exit('M6 key records span repositories')
            out.append({'source': 'm6', 'qid': q['id'], 'text': q['question'], 'kind': 'm6',
                        'repo': repos.pop(), 'session': q['session'], 'records': key['records']})
    return out


def claims(binary, home, text, cache):
    """The CLI prints full uids and rounded shares, including linked claims below the threshold."""
    out = []
    for line in text.split('\n'):
        if not line.strip():
            continue
        match = re.fullmatch(r'([0-9a-f]{64}) ([01]\.\d{3})', line)
        if not match:
            raise ValueError('Invalid injection report')
        uid = match[1]
        if uid not in cache:
            got = common.command([binary, '--home', home, 'get', uid])
            header, separator, rest = got.partition('\n\n')
            body, quotes, _ = rest.rpartition('\nquotes:\n')
            if not separator or not quotes or header.split(' ', 1)[0] != uid:
                raise ValueError('Injected claim body cannot be read')
            cited = json.loads(common.command([binary, '--home', home, 'cite', uid]))
            if len(cited) != 1 or cited[0].get('uid') != uid or cited[0].get('error'):
                raise ValueError('Injected claim evidence cannot be read')
            evidence = [{f: e[f] for f in ('device', 'seq', 'live')} for e in cited[0]['evidence']]
            cache[uid] = {'uid': uid, 'body': body.removesuffix('\n'), 'evidence': evidence}
        out.append(dict(cache[uid], share=float(match[2])))
    return out


def run(binary, home, pool='dev', decide=None):
    negative = load_pool(pool, decide)
    qs = m6.questions(pool, decide)
    keys = common.read_jsonl(f'{common.E}/m6/keys-{pool}.jsonl')
    common.guard([k.get('session', '') for k in keys if k.get('session')], pool=pool, decide=decide)
    binary, home = os.path.realpath(os.path.expanduser(binary)), os.path.realpath(os.path.expanduser(home))
    root = os.path.realpath(f'{common.E}/inject')
    if os.path.commonpath([root, home]) == home:
        sys.exit('Inject copy destination must be outside its source home')
    fingerprint = dict(input_hashes(pool), binary=common.sha256_file(binary), source_home=home,
                       source_files=frozen_home(home, pool, decide))
    if os.path.exists(path('runs', pool, 'json')) or os.path.exists(path('runs', pool)):
        sys.exit('Inject run already exists; keep it and choose a new evaluation directory')
    common.owner_only(path('runs', pool))
    with tempfile.TemporaryDirectory(prefix='home-', dir=root) as copied:
        shutil.copytree(home, copied, dirs_exist_ok=True)
        common.owner_only_tree(copied)
        if frozen_home(home, pool, decide) != fingerprint['source_files']:
            sys.exit('Source home changed while copying; no sweep made')
        m3.guard_home(copied, decide, pool)
        vector = m3.vector_side(copied)
        drain_config(copied)
        config_path = path('config', pool, 'toml')
        common.owner_only(config_path)
        shutil.copyfile(f'{copied}/config.toml', config_path)
        common.command([binary, '--home', copied, 'worker', '--idle-ms', '0'])
        questions = [dict(q, source='inject') for q in negative] + m6_inputs(copied, qs, keys)
        with open(f'{common.E}/m6/keys-{pool}.json', encoding='utf-8') as f:
            models = json.load(f).get('models', {})
        curation = Path(copied, 'curation.json')
        if curation.exists():
            models['curator'] = json.loads(curation.read_text(encoding='utf-8')).get('models', [])
        metadata = {'input': fingerprint, 'binary': binary, 'questions': questions, 'complete': False,
                    'config_sha256': common.sha256_file(config_path),
                    'metadata': common.record(binary, copied, len(negative), vector, models), 'm6_n': len(qs)}
        common.keep_json(path('runs', pool, 'json'), metadata)
        cache = {}
        with open(path('runs', pool), 'w', encoding='utf-8') as saved:
            for threshold in THRESHOLDS:
                for q in questions:
                    session = 'inject-eval-' + uuid.uuid4().hex
                    text = common.command([binary, '--home', copied, 'inject', '--prompt', '--repo', q['repo'],
                                           '--session', session, '--threshold', threshold], input=q['text'])
                    injected = claims(binary, copied, text, cache)
                    row = dict(q, threshold=threshold, call_session=session, claims=injected)
                    if q['source'] == 'm6':
                        records = {(e['device'], e['seq']) for e in q['records']}
                        row['relevant'] = any(e['live'] and (e['device'], e['seq']) in records
                                              for c in injected for e in c['evidence'])
                    saved.write(json.dumps(row, ensure_ascii=False) + '\n')
                    saved.flush()
        metadata.update(complete=True, runs_sha256=common.sha256_file(path('runs', pool)))
        common.keep_json(path('runs', pool, 'json'), metadata)
    return {'complete': True, 'n': len(negative), 'm6_n': len(qs), 'calls': len(questions) * len(THRESHOLDS)}


def read_run(pool, decide):
    common.guard(pool=pool, decide=decide)
    with open(path('runs', pool, 'json'), encoding='utf-8') as f:
        run = json.load(f)
    common.guard([q['session'] for q in run['questions'] if q['source'] == 'm6' and 'session' in q],
                 pool=pool, decide=decide)
    if not run['complete']:
        sys.exit('Inject sweep is incomplete; no labels or score made')
    if (any(run['input'].get(k) != v for k, v in input_hashes(pool).items())
            or common.sha256_file(run['binary']) != run['input']['binary']
            or common.sha256_file(path('runs', pool)) != run['runs_sha256']
            or common.sha256_file(path('config', pool, 'toml')) != run['config_sha256']):
        sys.exit('Inject inputs changed; keep the old run and choose a new evaluation directory')
    return run, common.read_jsonl(path('runs', pool))


def label(pool='dev', decide=None):
    run, rows = read_run(pool, decide)
    wanted = pairs(rows)
    saved = path('labels', pool)
    previous = common.read_jsonl(saved) if os.path.exists(saved) else []
    # Validate old labels against the same bodies before reusing a vote.
    metrics(run['questions'], rows, previous, pool)
    labelled = {pair_key(p): p for p in previous}
    cache = common.Calls()
    models = cache.models(run['metadata']['models'], *(p.get('models', {}) for p in previous))
    with tempfile.TemporaryDirectory(prefix='gate-', dir=f'{common.E}/inject') as home:
        shutil.copyfile(path('config', pool, 'toml'), f'{home}/config.toml')
        common.owner_only_tree(home)
        for key, pair in wanted.items():
            if common.voted(labelled.get(key, {}).get('votes', {}), 'relevant') is not None:
                continue
            # Explicit home even for gate: the binary creates its home before dispatching.
            prompt, body = (common.command([run['binary'], '--home', home, 'gate'], input=pair[f])
                            for f in ('prompt', 'body'))
            votes = cache.votes(INJECT.format(prompt=prompt, claim=body), ('relevant',))
            models = cache.models(models)
            labelled[key] = dict(pair, votes=votes, models=models)
            common.write_jsonl(saved, [labelled[k] for k in wanted if k in labelled])
    pending = sum(common.voted(labelled[k]['votes'], 'relevant') is None for k in wanted)
    out = {'n': len(wanted), 'pending': pending, 'complete': pending == 0,
           'metadata': common.record(run['binary'], run['metadata']['home'], len(wanted), 'off', models)}
    common.keep_json(path('labels', pool, 'json'), out)
    return out


def score(pool='dev', decide=None):
    run, rows = read_run(pool, decide)
    labels = common.read_jsonl(path('labels', pool)) if os.path.exists(path('labels', pool)) else []
    out = metrics(run['questions'], rows, labels, pool)
    models = common.Calls().models(run['metadata']['models'], *(p.get('models', {}) for p in labels))
    if out['complete']:
        chosen = out['chosen']
        out['chosen_m6'] = out['thresholds'][chosen]['m6']['share'] if chosen else None
    out.update(metadata=common.record(run['binary'], run['metadata']['home'], run['metadata']['N'],
                                      run['metadata']['vector'], models), run=run['metadata'])
    common.keep_json(path('score', pool, 'json'), out)
    return out


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    for name in ('pool', 'run', 'label', 'score'):
        p = sub.add_parser(name)
        p.add_argument('--pool', choices=('dev', 'test'), default='dev')
        p.add_argument('--decide')
        if name == 'run':
            p.add_argument('--binary', required=True)
            p.add_argument('--dev-home', required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == 'pool':
            rows = load_pool(args.pool, args.decide)
            out = dict(n=len(rows), **collections.Counter(r['kind'] for r in rows))
        elif args.command == 'run':
            out = run(args.binary, args.dev_home, args.pool, args.decide)
        elif args.command == 'label':
            out = label(args.pool, args.decide)
        else:
            out = score(args.pool, args.decide)
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, sqlite3.Error):
        sys.exit('Inject command failed; private text is not printed')
    print(json.dumps(out, ensure_ascii=False))
    return out


if __name__ == '__main__':
    main()
