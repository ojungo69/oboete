"""Task 12b: D15's read-hook line and D17's SessionStart drain measurement.

line --binary PATH --dev-home PATH --checkout PATH [--machine NAME] [--runs 3]
    Measure fresh copies and keep hooks/line-NAME.json. Existing lines are never overwritten.
combine line-*.json
    Print the maximum machine number for each hook, without further rounding.
drain --binary PATH --dev-home PATH
    Keep hooks/drain-NAME.json. The worker CLI is a proxy including process startup and backup,
    not an in-process worker::drained measurement. The MCP half is outside this harness.
"""
import argparse, glob, json, os, platform, re, shutil, sqlite3, tempfile, time
from contextlib import closing, contextmanager
from pathlib import Path

import common
import inject
import m3


SAMPLES, WARMUP = 300, 10
HOOKS = ('session_start', 'prompt')
DRAIN_METHOD = ('worker --idle-ms 0 wall time, including process startup, consumer drain, '
                'worker maintenance and backup; the CLI does not expose worker::drained alone')


def directory():
    return os.path.abspath(os.path.join(common.E, 'hooks'))


def machine_name(name=None):
    name = platform.node() if name is None else name
    if not re.fullmatch(r'[A-Za-z0-9_][A-Za-z0-9_.-]*', name):
        raise ValueError('Machine name must be a filename component')
    return name


def source_home(home):
    home = os.path.realpath(os.path.expanduser(home))
    if not os.path.isdir(home):
        raise ValueError('Dev home is not a directory')
    if Path(directory()).resolve().is_relative_to(home):
        raise ValueError('Evaluation copies must be outside their source home')
    return home


def environment(no_spawn=False):
    env = common.clean_env()
    # Inherited hook controls must not silently suppress writes or the D15 lock attempt.
    for key in ('OBOETE_SKIP', 'OBOETE_NO_SPAWN', 'OBOETE_REPLAY', 'OBOETE_FIELD_CAP'):
        env.pop(key, None)
    if no_spawn:
        env['OBOETE_NO_SPAWN'] = '1'
    return env


@contextmanager
def copied_home(home, prefix, per_prompt):
    source_files = inject.frozen_home(home, 'dev', None)
    with tempfile.TemporaryDirectory(prefix=prefix + '-', dir=directory()) as copied:
        shutil.copytree(home, copied, dirs_exist_ok=True)
        common.owner_only_tree(copied)
        if inject.frozen_home(home, 'dev', None) != source_files:
            raise ValueError('Source home changed while copying')
        m3.guard_home(copied)
        inject.drain_config(copied, per_prompt=per_prompt)
        yield copied


def measurements(report):
    """Keep the binary's statistics: its p95 already excludes its ten warm-up spawns."""
    read = report['read']
    warm = read['warm']
    reasons = []
    if read['per_prompt'] is not True:
        reasons.append('read.per_prompt is not true')
    for hook in HOOKS:
        field = hook + '_printed_bytes'
        if warm[field] == 0:
            reasons.append('read.warm.' + field + ' is zero')
    baseline = report['hook_spawn_ms']['1KB']
    for stats in [baseline] + [read[arm][hook + '_ms']
                               for arm in ('cold', 'warm') for hook in HOOKS]:
        if stats['n'] != SAMPLES:
            reasons.append('Each spawned sample must contain 300 timed calls')
    return dict(counted=not reasons, reasons=reasons, report=report,
                read_share_ms={hook: warm[hook + '_ms']['p95'] - baseline['p95'] for hook in HOOKS})


def summary(rows, expected):
    counted = [r for r in rows if r['counted']]
    complete = expected == 3 and len(rows) == len(counted) == expected
    return dict(counted_runs=len(counted),
                line_ms={hook: max(r['report']['read']['warm'][hook + '_ms']['p95']
                                   for r in counted) if complete else None for hook in HOOKS},
                cold_session_start_ms=max(r['report']['read']['cold']['session_start_ms']['p95']
                                          for r in counted) if complete else None)


def line(binary, dev_home, checkout, machine=None, runs=3):
    if runs != 3:
        raise ValueError('D15 requires three runs per machine')
    binary = os.path.abspath(os.path.expanduser(binary))
    home, checkout = source_home(dev_home), os.path.realpath(os.path.expanduser(checkout))
    if not os.path.isdir(checkout):
        raise ValueError('Checkout is not a directory')
    label = machine_name(machine)
    path = os.path.join(directory(), f'line-{label}.json')
    if os.path.lexists(path):
        raise ValueError('The machine already has a line record; keep the original measurement')
    fixture = os.path.abspath(os.path.join(common.E, 'replay', 'events-1000.jsonl'))
    out = dict(common.record(binary, home, SAMPLES, 'off', {}),
               machine_label=label, checkout=checkout, fixture=fixture,
               fixture_sha256=common.sha256_file(fixture), warmup=WARMUP,
               requested_runs=runs, runs=[])
    out.update(summary([], runs))
    common.keep_json(path, out)
    for index in range(runs):
        row = dict(common.record(binary, home, SAMPLES, 'off', {}), run=index + 1)
        try:
            with copied_home(home, 'line', True) as copied:
                row['home'] = copied
                argv = [binary, '--home', copied, 'replay', fixture, '--repo-root', checkout,
                        '--read-sample', str(SAMPLES), '--read-warmup', str(WARMUP),
                        '--spawn-sample', str(SAMPLES), '--sizes', '1']
                row.update(measurements(json.loads(common.command(argv, env=environment(), cwd=copied))))
        except (OSError, RuntimeError, ValueError, KeyError, TypeError, sqlite3.Error):
            # Process output can contain private text; keep only a fixed failure reason.
            row.update(counted=False, reasons=['Copy, replay or report validation failed'])
        out['runs'].append(row)
        out.update(summary(out['runs'], runs))
        common.keep_json(path, out)
    return out


def combine(paths):
    machines = []
    for path in paths:
        with open(path, encoding='utf-8') as f:
            row = json.load(f)
        result = summary(row['runs'], row['requested_runs'])
        machines.append(dict(path=os.path.abspath(path), machine=row['machine'],
                             machine_label=row['machine_label'], **result))
    complete = all(m['line_ms'][hook] is not None for m in machines for hook in HOOKS)
    return dict(machines=machines, complete=complete,
                line_ms={hook: max(m['line_ms'][hook] for m in machines) if complete else None
                         for hook in HOOKS})


def p95(values):
    """replay::pct: zero-based floor(n * 95 / 100), capped at the last sample."""
    ordered = sorted(values)
    return ordered[min(len(ordered) * 95 // 100, len(ordered) - 1)]


def backlog(home):
    """Mirror worker::backlog, without opening either store through a writing CLI command."""
    def database(name):
        return closing(sqlite3.connect(Path(home, name).as_uri() + '?mode=ro', uri=True))

    with database('raw.db') as raw, database('knowledge.db') as knowledge:
        device = raw.execute("SELECT value FROM meta WHERE key = 'device_id'").fetchone()[0]
        top, count = raw.execute('SELECT COALESCE(MAX(seq), 0), COUNT(*) FROM records WHERE device = ?',
                                 (device,)).fetchone()
        most = 0
        for table, consumers, tops in (
            ('checkpoints', ('rescan', 'fts', 'anchors', 'manifest', 'gaps', 'compress'), [(device, top)]),
            ('op_checkpoints', ('claims', 'cards', 'turns', 'imported'),
             raw.execute('SELECT device, MAX(op_seq) FROM ops GROUP BY device').fetchall()),
        ):
            for origin, end in tops:
                for consumer in consumers:
                    read = knowledge.execute(f'SELECT seq FROM {table} WHERE consumer = ? AND device = ?',
                                             (consumer, origin)).fetchone()
                    most = max(most, end - (read[0] if read else 0))
        return dict(records=count, backlog=most)


def matching_line(metadata):
    matches = []
    for path in sorted(glob.glob(os.path.join(directory(), 'line-*.json'))):
        with open(path, encoding='utf-8') as f:
            row = json.load(f)
        if all(row.get(k) == metadata[k] for k in ('machine', 'sha256', 'home')):
            matches.append((path, row))
    if len(matches) != 1:
        return None, 'Need one line record for this machine, binary and dev home'
    path, row = matches[0]
    result = summary(row['runs'], row['requested_runs'])
    if result['cold_session_start_ms'] is None:
        return None, 'The matching line record has too few counted runs'
    return dict(path=path, machine_label=row['machine_label'], checkout=row['checkout'], **result), None


def drain(binary, dev_home):
    binary = os.path.abspath(os.path.expanduser(binary))
    home = source_home(dev_home)
    out = dict(common.record(binary, home, 20, 'off', {}), method=DRAIN_METHOD,
               runs=[], slope=[], drain_p95_ms=None, session_start_ms=None)
    matched, issue = matching_line(out)
    out.update(line=matched, line_issue=issue)
    label = machine_name(matched['machine_label'] if matched else None)
    path = os.path.join(directory(), f'drain-{label}.json')
    if os.path.lexists(path):
        raise ValueError('Drain record already exists; keep the original measurement')
    common.keep_json(path, out)
    for size in (1, 20, 200, 2000):
        for index in range(20 if size == 20 else 1):
            row = dict(common.record(binary, home, size, 'off', {}), backlog=size, run=index + 1, counted=False)
            try:
                with copied_home(home, 'drain', False) as copied:
                    row['home'] = copied
                    worker = [binary, '--home', copied, 'worker', '--idle-ms', '0']
                    env = environment(no_spawn=True)
                    # Neither an existing backlog nor config-driven rescans belong to timed N.
                    common.command(worker, env=env, cwd=copied)
                    before = backlog(copied)
                    if before['backlog']:
                        raise ValueError('Baseline did not drain')
                    checkout = matched['checkout'] if matched else os.path.join(copied, 'repo')
                    if not matched:
                        os.makedirs(os.path.join(checkout, '.git'), mode=0o700)
                    payload = json.dumps(dict(session_id=f'drain-{size}-{index}', cwd=checkout,
                                              hook_event_name='PostToolUse', tool_name='Bash',
                                              tool_input={'command': 'true'}, tool_response='ok'))
                    for _ in range(size):
                        common.command([binary, '--home', copied, 'hook', 'claude', 'PostToolUse'],
                                       input=payload, env=env, cwd=copied)
                    pending = backlog(copied)
                    row['observed'] = pending
                    if pending['records'] - before['records'] != size or pending['backlog'] != size:
                        raise ValueError('Hooks did not create the requested backlog')
                    started = time.perf_counter_ns()
                    common.command(worker, env=env, cwd=copied)
                    elapsed = (time.perf_counter_ns() - started) / 1_000_000
                    if backlog(copied)['backlog']:
                        raise ValueError('Timed worker did not drain')
                    row.update(counted=True, ms=elapsed)
            except (OSError, RuntimeError, ValueError, KeyError, TypeError, sqlite3.Error):
                row['reason'] = 'Copy, hook or worker failed, or the verified backlog differs'
            out['runs'].append(row)
            common.keep_json(path, out)
        rows = [r for r in out['runs'] if r['backlog'] == size]
        values = [r['ms'] for r in rows if r['counted']]
        out['slope'].append(dict(backlog=size, N=len(rows), counted=len(values), ms=values,
                                 p95_ms=p95(values) if len(values) == len(rows) else None))
    out['drain_p95_ms'] = out['slope'][1]['p95_ms']
    if matched is not None and out['drain_p95_ms'] is not None:
        out['session_start_ms'] = matched['cold_session_start_ms'] + out['drain_p95_ms']
    common.keep_json(path, out)
    return out


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest='command', required=True)
    for name in ('line', 'drain'):
        p = sub.add_parser(name)
        p.add_argument('--binary', required=True)
        p.add_argument('--dev-home', required=True)
        if name == 'line':
            p.add_argument('--checkout', required=True)
            p.add_argument('--machine')
            p.add_argument('--runs', type=int, default=3)
    sub.add_parser('combine').add_argument('paths', nargs='+')
    args = parser.parse_args(argv)
    try:
        if args.command == 'line':
            out = line(args.binary, args.dev_home, args.checkout, args.machine, args.runs)
        elif args.command == 'drain':
            out = drain(args.binary, args.dev_home)
        else:
            out = combine(args.paths)
    except (OSError, RuntimeError, ValueError, KeyError, TypeError, sqlite3.Error):
        parser.exit(1, 'Hook evaluation failed; check paths and existing records. Private text is not printed.\n')
    print(json.dumps(out, ensure_ascii=False))


if __name__ == '__main__':
    main()
