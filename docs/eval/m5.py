"""Milestone 4, Task 12a: one-device resume (docs/milestone-4.md's fixed protocol).

  m5.py cuts <bin>   seeded cuts from m3's fixtures; the replay manifest's dev sessions only
  m5.py run <bin>    each prefix in fresh curated/none homes, inject, then the panel's labels
  m5.py score <bin>  three graders per item; counts only, and no next-step pass line

--pool test needs --decide matching deciding.json before any held-out contents are read.
Successful model calls share common.Calls' private cache; a failed call is retried next command.
"""
import argparse, json, os, shutil, sqlite3, subprocess, sys

import common, m3

LABEL = """Here is a developer's coding session up to a moment, its turns numbered:
{turns}

At the end of these turns, list:
- open: the work the developer asked for or the agent started that is not finished;
- next: the step the session would take next;
- closed: the work finished, dropped or withdrawn before the end.
Write each item as one short sentence in the session's language.
Answer with JSON only: {{"open": ["<item>", ...], "next": "<step>", "closed": ["<item>", ...]}}.
"""

CHECK_LABELS = """Here is a developer's coding session up to a moment, its turns numbered, and labels written for that moment:
{turns}

Labels:
{labels}

Are the labels right: every open item still open at the end, every closed item finished or withdrawn before it, and no open item left out?
Answer with JSON only: {{"agree": true}} or {{"agree": false, "why": "<one sentence>"}}.
"""

SHOWN = """A new coding session starts with this context:
<<<
{context}
>>>

An item of work:
<<<
{item}
>>>

Does the context show this item as still open, not finished? An item the context does not mention is not shown.
Answer with JSON only: {{"shown_open": true}} or {{"shown_open": false}}.
"""

# The core's POSIX process group kills this shim and its CLI together on timeout. The shim
# inherits the curator's isolated cwd/arguments, forwards stdout unchanged, and keeps names only.
CLI_MODELS = r'''import json, os, re, subprocess, sys
os.umask(0o077)
env = {k: v for k, v in os.environ.items() if k != 'CLAUDECODE'
       and not any(s in k.upper() for s in ('TOKEN', 'KEY', 'SECRET', 'PASSWORD'))}
try:
    child = subprocess.Popen([REAL, *sys.argv[1:]], stdin=sys.stdin.buffer,
                             stdout=subprocess.PIPE, env=env)
except OSError:
    sys.exit(1)
with open(LOG, 'a', encoding='utf-8') as log:
    seen = set()
    for part in child.stdout:
        sys.stdout.buffer.write(part)
        sys.stdout.buffer.flush()
        try:
            event = json.loads(part)
        except ValueError:
            event = None
        names = []
        if isinstance(event, dict):
            message, usage = event.get('message'), event.get('modelUsage')
            if event.get('type') == 'assistant' and isinstance(message, dict):
                names = [message.get('model')]
            elif event.get('type') == 'result' and isinstance(usage, dict):
                names = list(usage)
        for model in names:
            if isinstance(model, str) and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._:/@+-]{0,199}', model) and model not in seen:
                log.write(json.dumps({'model': model}) + '\n')
                log.flush()
                seen.add(model)
child.stdout.close()
sys.exit(child.wait())
'''


def choose_cut(session, events):
    """The seeded event among those at least thirty minutes after the first event."""
    if not events:
        return None
    first = m3.when(events[0]['ts'])
    if m3.when(events[-1]['ts']) - first < 1800:
        return None
    eligible = [i for i, e in enumerate(events) if m3.when(e['ts']) - first >= 1800]
    return eligible[common.h(f'm5-cut:{common.SEED}:{session}') % len(eligible)]


def metrics(rows):
    """Counts only; a missing grader leaves the entire run unscored."""
    decisions = [common.voted(r['votes'], 'shown_open') for r in rows]
    pending = sum(v is None for v in decisions)
    if pending:
        return {'complete': False, 'pending': pending}
    tiers = {}
    for tier, line in (('curated', 0.8), ('none', 0.5)):
        tiers[tier] = {}
        for kind in ('open', 'closed', 'next'):
            shown = [v for r, v in zip(rows, decisions) if r['tier'] == tier and r['kind'] == kind]
            rate = sum(shown) / len(shown) if shown else None
            count = {'shown': sum(shown), 'n': len(shown), 'rate': rate}
            if kind == 'open':
                count['pass'] = rate is not None and rate >= line
            elif kind == 'closed' and tier == 'curated':
                count['pass'] = rate is not None and rate <= 0.1
            tiers[tier][kind] = count
    return {'complete': True, 'n': len({r['session'] for r in rows}), 'tiers': tiers,
            'agreement': common.agreement([r['votes'] for r in rows], 'shown_open'),
            'pass': (tiers['curated']['open']['pass'] and tiers['none']['open']['pass']
                     and tiers['curated']['closed']['pass'])}


def path(pool, name):
    return f'{common.E}/m5/{name}-{pool}.jsonl'


def fixture_rows(session, pool='dev', decide=None, upto=None):
    common.guard(session_ids=[session], pool=pool, decide=decide)
    return [(i + 1, event, raw) for i, event, raw in m3.fixture_events(session) if upto is None or i < upto]


def cuts(binary, pool='dev', decide=None):
    common.guard(pool=pool, decide=decide)
    sessions = m3.pool_sessions(pool, decide)
    rows = []
    for session in sessions:
        events = fixture_rows(session, pool, decide)
        cut = choose_cut(session, [e for _, e, _ in events])
        if cut is not None:
            line, event, _ = events[cut]
            rows.append({'session': session, 'line': line, 'ts': event['ts'],
                         'fixture_sha256': common.sha256_file(f'{m3.M}/fixtures/{session}.jsonl')})
    existing = path(pool, 'cuts')
    if os.path.exists(existing) and common.read_jsonl(existing) != rows:
        raise SystemExit('Cuts changed; keep a new evaluation set before running it')
    common.write_jsonl(existing, rows)
    print(f'{len(rows)} cuts; {len(sessions) - len(rows)} short sessions')
    return rows


def turns(binary, events):
    text = []
    for line, event, _ in events:
        role, field = {'UserPromptSubmit': ('prompt', 'prompt'),
                       'Stop': ('reply', 'last_assistant_message')}.get(event['event'], (None, None))
        value = event.get('payload', {}).get(field)
        if role and isinstance(value, str) and value.strip():
            text.append((line, role, value))
    return '\n'.join(f'[{line}] {role}: {common.gate(value, binary)[:2000]}'
                     for line, role, value in text[-80:])


def valid_labels(answer):
    return (all(isinstance(answer.get(k), list)
                and all(isinstance(s, str) and s.strip() for s in answer[k])
                and len(set(answer[k])) == len(answer[k]) for k in ('open', 'closed'))
            and not set(answer['open']) & set(answer['closed'])
            and isinstance(answer.get('next'), str) and bool(answer['next'].strip()))


def checkout(h, session, events):
    with sqlite3.connect(f'file:{h}/raw.db?mode=ro', uri=True) as db:
        repos = db.execute("SELECT repo FROM records WHERE type = 'event' AND session = ? "
                           'AND repo IS NOT NULL ORDER BY seq DESC', (session,)).fetchall()
    for (repo,) in repos:
        if os.path.isabs(repo) and os.path.isdir(repo):
            return repo
    for _, event, _ in reversed(events):
        cwd = event.get('payload', {}).get('cwd')
        if isinstance(cwd, str) and os.path.isabs(cwd) and os.path.isdir(cwd):
            return cwd
    return None


def curator_worker(binary, h):
    env = common.clean_env()
    real = shutil.which('claude', path=env.get('PATH', ''))
    if os.name != 'posix' or real is None:
        sys.exit('M5 needs POSIX and the claude CLI on PATH')
    log, wrapper = f'{h}/curator-models.jsonl', f'{h}/wrap/claude'
    common.owner_only(wrapper)
    with open(wrapper, 'w', encoding='utf-8') as f:
        f.write(f'#!{sys.executable}\nREAL = {os.path.abspath(real)!r}\nLOG = {log!r}\n' + CLI_MODELS)
    os.chmod(wrapper, 0o700)
    env['PATH'] = f'{h}/wrap' + os.pathsep + env.get('PATH', '')
    common.command([binary, '--home', h, 'worker', '--idle-ms', '0'], env=env)
    models = sorted({r['model'] for r in common.read_jsonl(log)}) if os.path.exists(log) else []
    return {'status': 'ok' if models else 'missing_models', 'models': models}


def curation(h, models):
    windows, top = m3.windows(h)
    broken, outcomes = m3.coverage(windows, top)
    with sqlite3.connect(f'file:{h}/knowledge.db?mode=ro', uri=True) as db:
        claims = dict(db.execute('SELECT kind, COUNT(*) FROM active GROUP BY kind'))
    with sqlite3.connect(f'file:{h}/providers.db?mode=ro', uri=True) as db:
        calls = [dict(zip(('provider', 'role', 'outcome', 'n', 'est_tokens'), row)) for row in db.execute(
            'SELECT provider, role, outcome, COUNT(*), SUM(est_tokens) FROM provider_calls '
            'GROUP BY provider, role, outcome')]
    out = {'records': top, 'windows': len(windows), 'outcomes': outcomes, 'claims': claims,
           'coverage': broken or '100%', 'models': models,
           'provider_calls': calls, 'requested_curator': {'provider': 'claude', 'model': 'haiku'}}
    common.keep_json(f'{h}/curation.json', out)
    return out


def run(binary, pool='dev', decide=None):
    common.guard(pool=pool, decide=decide)
    selected = common.read_jsonl(path(pool, 'cuts'))
    common.guard(session_ids=[r['session'] for r in selected], pool=pool, decide=decide)
    saved = path(pool, 'runs')
    previous = {r['session']: r for r in common.read_jsonl(saved)} if os.path.exists(saved) else {}
    calls, rows = common.Calls(), []
    binary_hash = common.sha256_file(binary)
    for cut in selected:
        session = cut['session']
        if common.sha256_file(f'{m3.M}/fixtures/{session}.jsonl') != cut['fixture_sha256']:
            raise SystemExit('A cut fixture changed; no run made')
        root = f'{common.E}/m5/{binary_hash[:12]}/{pool}/{session}'
        h, none = f'{root}/curated', f'{root}/none'
        row = previous.get(session, {'session': session, 'cut': cut, 'homes': {'curated': h, 'none': none}})
        if row['cut'] != cut or row['homes'] != {'curated': h, 'none': none}:
            raise SystemExit('Run inputs changed; keep a new evaluation set')
        if not row.get('complete'):
            events = fixture_rows(session, pool, decide, cut['line'])
            if not os.path.exists(f'{h}/replay.json'):
                # A replay stopped part way leaves raw.db without replay.json: made again whole.
                for home in (h, none):
                    if os.path.exists(home):
                        shutil.rmtree(home)
                m3.replay_events(binary, h, [(m3.when(e['ts']), session, line - 1, raw)
                                            for line, e, raw in events], 1)
            if not os.path.exists(none):
                if os.path.exists(none + '.part'):
                    shutil.rmtree(none + '.part')
                shutil.copytree(h, none + '.part')
                os.replace(none + '.part', none)
            common.owner_only_tree(none)
            cwd = checkout(h, session, events)
            row['no_checkout'] = cwd is None
            if cwd is None:
                row['complete'] = True
            else:
                if 'contexts' not in row:
                    m3.config(h, m3.LIVE, True)
                    row['curator'] = curator_worker(binary, h)
                    row['pending_metadata'] = row['curator']['status'] != 'ok'
                    common.command([binary, '--home', none, 'worker', '--idle-ms', '0'])
                    row['curation'] = curation(h, row['curator']['models'])
                    row['pending_curation'] = row['curation']['coverage'] != '100%'
                    if not row.get('pending_curation') and not row['pending_metadata']:
                        row['contexts'] = {tier: common.gate(common.command(
                            [binary, '--home', home, 'inject'], cwd=cwd), binary)
                            for tier, home in row['homes'].items()}
                if row.get('pending_curation') or row.get('pending_metadata'):
                    row['complete'] = False
                else:
                    listing = turns(binary, events)
                    labeller = common.draw('m5-label', session)
                    row['labeller'] = labeller
                    try:
                        row['labels'] = calls.call(labeller, LABEL.format(turns=listing), valid_labels)
                        if common.checked('m5-check', session):
                            row['checker'] = common.draw('m5-checker', session, exclude=labeller)
                            row['check'] = calls.call(row['checker'], CHECK_LABELS.format(
                                turns=listing, labels=common.gate(json.dumps(row['labels'], ensure_ascii=False), binary)),
                                lambda a: type(a.get('agree')) is bool)['agree']
                        row['complete'] = True
                    except common.FailedCall:
                        row['complete'] = False
            models = calls.models()
            if row.get('curator', {}).get('models'):
                models['curator'] = row['curator']['models']
            row['metadata'] = common.record(binary, root, 1, 'off', models)
        rows.append(row)
        common.write_jsonl(saved, rows + [previous[c['session']] for c in selected[len(rows):]
                                       if c['session'] in previous])
    print(f'{len(rows)} sessions; {sum(r["no_checkout"] for r in rows)} without a checkout; '
          f'{sum(not r["complete"] for r in rows)} awaiting curation, metadata or labels')
    return rows


def score(binary, pool='dev', decide=None):
    common.guard(pool=pool, decide=decide)
    selected = common.read_jsonl(path(pool, 'cuts'))
    runs = common.read_jsonl(path(pool, 'runs'))
    common.guard(session_ids=[r['session'] for r in selected + runs], pool=pool, decide=decide)
    expected = {r['session']: r for r in selected}
    binary_hash = common.sha256_file(binary)
    if any(r['cut'] != expected.get(r['session']) or r['metadata']['sha256'] != binary_hash for r in runs):
        raise SystemExit('M5 run inputs changed; no score made')
    missing = len(set(expected) - {r['session'] for r in runs})
    calls = common.Calls()
    if missing or any(not r.get('complete') for r in runs):
        out = {'complete': False, 'pending_sessions': missing + sum(not r.get('complete') for r in runs)}
    else:
        grades = []
        for row in runs:
            if row['no_checkout']:
                continue
            for tier, context in row['contexts'].items():
                context = common.gate(context, binary)
                for kind in ('open', 'closed', 'next'):
                    items = [row['labels']['next']] if kind == 'next' else row['labels'][kind]
                    for ordinal, item in enumerate(items):
                        votes = calls.votes(SHOWN.format(context=context, item=common.gate(item, binary)), ['shown_open'])
                        grades.append({'session': row['session'], 'tier': tier, 'kind': kind,
                                       'ordinal': ordinal, 'votes': votes})
        common.write_jsonl(path(pool, 'grades'), grades)
        out = metrics(grades)
        checks = [r['check'] for r in runs if 'check' in r]
        out['label_agreement'] = {'n': len(checks), 'agree': sum(checks),
                                  'rate': sum(checks) / len(checks) if checks else None}
        out['no_checkout'] = sum(r['no_checkout'] for r in runs)
    models = calls.models(*(r['metadata']['models'] for r in runs))
    out['metadata'] = common.record(binary, f'{common.E}/m5', len(selected), 'off', models)
    common.keep_json(f'{common.E}/m5/score-{pool}.json', out)
    print(json.dumps(out, ensure_ascii=False, indent=1))
    return out


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('cuts', 'run', 'score'))
    parser.add_argument('binary')
    parser.add_argument('--pool', choices=('dev', 'test'), default='dev')
    parser.add_argument('--decide')
    args = parser.parse_args(argv)
    try:
        return globals()[args.command](os.path.abspath(args.binary), args.pool, args.decide)
    except (OSError, ValueError, KeyError, RuntimeError, sqlite3.Error):
        raise SystemExit('M5 evaluation failed; private outputs were not printed') from None


if __name__ == '__main__':
    common.owner_only()
    main()
