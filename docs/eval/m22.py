"""D16's isolated scale harness. No real provider calls are made here.

build --binary PATH --dev-home PATH [--days 90|365 --disk-ok]
run --binary PATH --home PATH --checkout PATH --workers-ai-latency FILE
    --lines FILE --wsl-dev FILE

The external latency JSON has provider=workers-ai, model=@cf/baai/bge-m3,
source=real and samples_ms (at least 30 successful query embeddings). It must
be measured separately. build leaves an owned home; run removes it even on failure.
"""
import argparse, copy, hashlib, http.server, json, math, os, re, shutil, sqlite3, subprocess
import tempfile, threading, time, tomllib
from contextlib import contextmanager, closing
from datetime import datetime, timezone, timedelta
from pathlib import Path
from urllib.parse import urlsplit

import common
import m3
import m4
import replay_set


DOCUMENTS, QUERIES, WARMUP = 178370, 100, 10


def number(value, positive=False):
    if type(value) not in (int, float) or not math.isfinite(value) or value < 0 or (positive and value == 0):
        raise ValueError('Measurement must be a finite non-negative number')
    return value


def percentile(values, pct):
    ordered = sorted(number(v) for v in values)
    if not ordered:
        raise ValueError('No measurement samples')
    return ordered[min(int(len(ordered) * pct / 100), len(ordered) - 1)]


def real_latency(row):
    if not isinstance(row, dict) or (row.get('provider'), row.get('model'), row.get('source')) != (
            'workers-ai', '@cf/baai/bge-m3', 'real'):
        raise ValueError('Real Workers AI query latency input is required')
    values = row.get('samples_ms')
    if not isinstance(values, list) or len(values) < 30:
        raise ValueError('Real latency needs at least 30 samples')
    return [number(v, True) for v in values]


def metrics(idle, written, latency):
    real = real_latency(latency)
    for rows in (idle, written):
        for row in rows:
            number(row['ms'], True)
            if type(row.get('embeds')) is not int or row['embeds'] != 1:
                raise ValueError('Each timed call needs exactly one embedding request')
    if min(len(idle), len(written)) < 30:
        return dict(counted=False, reason='Fewer than 30 timed queries', N=min(len(idle), len(written)))
    a, b = [r['ms'] for r in idle], [r['ms'] for r in written]
    idle95, write95 = percentile(a, 95), percentile(b, 95)
    bound = percentile(b, 97.5) + percentile(real, 97.5)
    return dict(counted=True, N=len(b), idle_p95_ms=idle95, writer_p95_ms=write95,
                slowdown_p95=write95 / idle95 - 1, slowdown_p50=percentile(b, 50) / percentile(a, 50) - 1,
                slowdown_pass=write95 <= idle95 * 1.2, store_p97_5_ms=percentile(b, 97.5),
                embedding_p97_5_ms=percentile(real, 97.5), combined_p95_bound_ms=bound,
                mcp_pass=bound <= 1500)


def select_queries(rows):
    selected, seen = [], set()
    for row in rows:
        if not isinstance(row, dict) or row.get('split') not in ('dev', 'test'):
            raise ValueError('Invalid question metadata')
        if row['split'] != 'dev':
            continue
        if not isinstance(row.get('qid'), str) or not row['qid'] or row['qid'] in seen:
            raise ValueError('Question ids must be unique non-empty strings')
        if not isinstance(row.get('text'), str) or not row['text'].strip():
            raise ValueError('Question text is missing')
        common.guard(session_ids=[row['session']] if 'session' in row else [])
        seen.add(row['qid'])
        selected.append(dict(row, query=row['text']))
    if len(selected) < QUERIES:
        raise ValueError('D16 requires 100 dev questions')
    return sorted(selected, key=lambda q: (common.h(f'm22-queries:{common.SEED}:{q["qid"]}'), q['qid']))[:QUERIES]


def query_leg(client, questions, counter, clock=time.perf_counter):
    rows = []
    for q in questions:
        before = counter()
        start = clock()
        reply = client.call('search', {'query': q['query'], 'all': True, 'limit': 10})
        elapsed = (clock() - start) * 1000
        if (not isinstance(reply, dict) or reply.get('isError') or
                not isinstance(reply.get('content'), list) or not reply['content'] or
                any(not isinstance(c, dict) or c.get('type') != 'text' or
                    not isinstance(c.get('text'), str) for c in reply['content'])):
            raise ValueError('Invalid MCP search result')
        row = dict(qid=q['qid'], ms=number(elapsed), embeds=counter() - before)
        if row['embeds'] != 1:
            raise ValueError('Each timed call needs exactly one embedding request')
        rows.append(row)
    return dict(N=len(rows[WARMUP:]), warmup=WARMUP, first=rows[0], timed=rows[WARMUP:])


class Stub(m3.Stub):
    """The existing curator protocol, with literal user quotes at the recorded claim rate."""

    def do_POST(self):
        with self.server.mutex:
            self.server.requests += 1
        try:
            length = int(self.headers.get('Content-Length', '0'))
            if not 0 < length <= 8 << 20 or self.headers.get('Authorization') not in (
                    None, 'Bearer m22-loopback-only'):
                raise ValueError()
            body = json.loads(self.rfile.read(length))
            if self.path == '/embed':
                texts = body['text']
                if not isinstance(texts, list) or not 1 <= len(texts) <= 100 or any(
                        not isinstance(t, str) or not t.strip() for t in texts):
                    raise ValueError()
                vectors = []
                for text in texts:
                    vector = [0.0] * 1024
                    # ponytail: deterministic unit vectors measure index cost, not retrieval quality.
                    for token in re.findall(r'\w+|[^\s]', text):
                        vector[common.h(token) % 1024] += 1
                    norm = math.sqrt(sum(v * v for v in vector))
                    vectors.append([v / norm for v in vector])
                out = {'success': True, 'result': {'shape': [len(texts), 1024], 'data': vectors}}
                with self.server.mutex:
                    self.server.embeds += 1
                    if len(texts) == 1 and texts[0] in self.server.query_texts:
                        self.server.query_embeds += 1
            elif self.path == '/v1/chat/completions':
                schema = json.dumps(body.get('response_format', {}))
                content = {'lines': []}
                if '"lines"' not in schema:
                    text = '\n'.join(m['content'] for m in body['messages'] if isinstance(m.get('content'), str))
                    fence = re.search(r'(=== RECORD [^\n]+ ===)\n', text)
                    if not fence:
                        raise ValueError()
                    current = text[fence.end():].split(fence[1], 1)[0].split('## Kept claims', 1)[0]
                    records = re.findall(r'^L\d+ ', current, re.M)
                    # ponytail: HTTP exposes visible lines, not raw spans; report empty-record rate shortfalls.
                    users = re.findall(r'^(L\d+) \[user\] ([^\n]+)', current, re.M)
                    users = [(line, text[:200]) for line, text in users if text.strip() and '[REDACTED]' not in text]
                    with self.server.mutex:
                        self.server.claim_budget += self.server.rate * len(records)
                        count = min(int(self.server.claim_budget), len(users))
                        self.server.claim_budget -= count
                    content = {'summary': 'Scale fixture.', 'claims': [
                        dict(id=f'c{i + 1}', kind='decision', status='decided', speaker='user', scope='repo',
                             body=quote, quote=quote, line=line, supersedes=[], why='')
                        for i, (line, quote) in enumerate(users[:count])]}
                out = {'id': 'm22-stub', 'object': 'chat.completion', 'model': 'm22-stub', 'choices': [
                    {'index': 0, 'message': {'role': 'assistant', 'content': json.dumps(content)}, 'finish_reason': 'stop'}]}
            else:
                raise ValueError()
            data = json.dumps(out).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        except (ValueError, KeyError, TypeError, ZeroDivisionError):
            self.send_error(400, 'Invalid scale stub request')


@contextmanager
def loopback(rate):
    number(rate)
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
    server.url = f'http://127.0.0.1:{server.server_port}'
    server.rate, server.claim_budget, server.embeds, server.requests = rate, 0.0, 0, 0
    server.query_texts, server.query_embeds = set(), 0
    server.mutex = threading.Lock()
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield server
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def directory():
    return Path(common.E).resolve() / 'm22'


def save(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with path.open('w', encoding='utf-8') as f:
        os.chmod(path, 0o600)
        json.dump(data, f, ensure_ascii=False, indent=1, allow_nan=False)


def load(path):
    with open(path, encoding='utf-8') as f:
        return json.load(f)


def database(home, name):
    return closing(sqlite3.connect((Path(home).resolve() / name).as_uri() + '?mode=ro', uri=True))


def home_rates(home):
    with database(home, 'raw.db') as raw, database(home, 'knowledge.db') as knowledge:
        records = raw.execute("SELECT COUNT(*) FROM records WHERE type = 'event'").fetchone()[0]
        claims = knowledge.execute('SELECT COUNT(*) FROM active').fetchone()[0]
        sessions = sorted(s for (s,) in raw.execute('SELECT DISTINCT session FROM records') if s)
    if records < 1:
        raise ValueError('The dev home has no records to measure its claim rate')
    return dict(records=records, claims=claims, claims_per_record=claims / records, sessions=sessions)


def new_home():
    root = directory()
    if root.is_symlink():
        raise ValueError('Scale directory must be owned without symlinks')
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    home = Path(tempfile.mkdtemp(prefix='scale-', dir=root))
    save(home / 'm22-owned.json', {'home': str(home), 'purpose': 'm22-scale'})
    return home


def owned_home(home):
    home = Path(home).absolute()
    marker = home / 'm22-owned.json'
    if (home.is_symlink() or home.parent != directory() or not home.name.startswith('scale-') or
            not home.is_dir() or marker.is_symlink() or not marker.is_file()):
        raise ValueError('Not an owned M22 temporary home')
    row = load(marker)
    if row != {'home': str(home), 'purpose': 'm22-scale'}:
        raise ValueError('Not an owned M22 temporary home')
    return home


def cleanup(home):
    shutil.rmtree(owned_home(home))


def environment():
    env = common.clean_env()
    for key in ('OBOETE_SKIP', 'OBOETE_REPLAY', 'OBOETE_FIELD_CAP', 'OBOETE_HOME'):
        env.pop(key, None)
    env['OBOETE_NO_SPAWN'] = '1'
    return env


def toml_value(value):
    if isinstance(value, dict):
        return '{' + ', '.join(json.dumps(k, ensure_ascii=False) + ' = ' + toml_value(v)
                              for k, v in value.items()) + '}'
    if isinstance(value, list):
        return '[' + ', '.join(toml_value(v) for v in value) + ']'
    if type(value) in (str, bool, int, float):
        return json.dumps(value, ensure_ascii=False, allow_nan=False)
    raise ValueError('Unsupported copied configuration value')


def configure(home, server=None):
    path = Path(home) / 'config.toml'
    config = tomllib.loads(path.read_text(encoding='utf-8')) if path.exists() else {}
    # All recording/read settings survive; replace every provider route, including implicit Gemini.
    for key in ('gemini', 'chain'):
        config.pop(key, None)
    config['providers'] = []
    config['embedding'] = {'provider': 'none'}
    config.setdefault('summary', {})['curate'] = server is not None
    config.setdefault('inject', {})['per_prompt'] = True
    config.setdefault('backup', {})['dir'] = 'backups'
    if server is not None:
        url = urlsplit(server.url)
        if (url.scheme != 'http' or url.hostname != '127.0.0.1' or not url.port or url.username or
                url.password or url.path or url.query or url.fragment):
            raise ValueError('Embedding and curator endpoints must be the owned loopback stub')
        # The only key this harness writes is a public sentinel for its loopback endpoint.
        key = Path(home) / 'm22-loopback.md'
        key.write_text('# evaluation sentinel\nm22-loopback-only\n', encoding='utf-8')
        os.chmod(key, 0o600)
        config['providers'] = [dict(kind='openai', name='m22-stub', model='m22-stub',
                                    base_url=server.url + '/v1', daily_budget=4294967295)]
        config['embedding'] = dict(provider='workers-ai', account_id='m22-loopback', url=server.url + '/embed',
                                   key_file=str(key), daily_requests=4294967295, monthly_usd=1000000)
    text = '\n'.join(json.dumps(k, ensure_ascii=False) + ' = ' + toml_value(v) for k, v in config.items()) + '\n'
    if tomllib.loads(text) != config:
        raise ValueError('Copied configuration did not preserve its parsed values')
    path.write_text(text, encoding='utf-8')
    os.chmod(path, 0o600)


def copy_dev(source, home):
    source = Path(source).resolve()
    if home.is_relative_to(source) or not source.is_dir():
        raise ValueError('Scale copies must be outside the dev home')
    if any(p.is_symlink() for p in source.rglob('*')):
        raise ValueError('Dev home contains a symlink')
    # Copy DBs through SQLite's snapshot API; copying a live WAL file can lose records.
    for name in ('raw.db', 'knowledge.db', 'providers.db'):
        if (source / name).is_file():
            with database(source, name) as original, closing(sqlite3.connect(home / name)) as copied:
                original.backup(copied)
    if (source / 'config.toml').is_file():
        shutil.copyfile(source / 'config.toml', home / 'config.toml')
    common.owner_only_tree(home)
    configure(home)


def source_sessions(found, manifest, already, since):
    """Never open held-out contents. Discovery comes from m4, not corpus-count's file count."""
    sides = {}
    for row in manifest['sessions']:
        session = row['session']
        if session in sides:
            raise ValueError('Duplicate replay manifest session')
        sides[session] = row['side']
    chosen, seen = [], set()
    for agent, session, path in found:
        if session in seen:
            raise ValueError('Duplicate transcript session')
        seen.add(session)
        if session in already or sides.get(session, common.split(session)) not in ('dev', 'dev-extra'):
            continue
        path = Path(path).resolve()
        if not path.is_file():
            raise ValueError('Transcript copy is missing')
        cwd, start, forked = m4.head(agent, str(path))
        if not start or forked or (cwd and (m4.under(cwd, '/tmp') or m4.under(cwd, m4.OBSERVER))):
            continue
        # A resumed session can contain a recent event despite an old first event.
        if path.stat().st_mtime < since.timestamp():
            continue
        common.guard(session_ids=[session])
        chosen.append(dict(agent=agent, session=session, path=str(path)))
    return chosen


def replay_parts(binary, home, events):
    with database(home, 'raw.db') as raw:
        already = {s for (s,) in raw.execute('SELECT DISTINCT session FROM records') if s}
    report = dict(events=0, parts=0)
    batch = []
    def flush():
        path = home / 'm22-part.jsonl'
        with path.open('w', encoding='utf-8') as f:
            f.writelines(json.dumps(e, ensure_ascii=False) + '\n' for e in batch)
        reply = json.loads(common.command([binary, '--home', str(home), 'replay', str(path),
                                          '--agent', 'all', '--spawn-sample', '0'], env=environment()))
        if not isinstance(reply, dict):
            raise ValueError('Invalid replay report')
        path.unlink()
        report['events'] += len(batch)
        report['parts'] += 1
        batch.clear()
    for event in events:
        if (not isinstance(event, dict) or not isinstance(event.get('session'), str) or
                not event['session'] or event['session'] in already or
                not isinstance(event.get('payload'), dict)):
            raise ValueError('Replay input contains an invalid or already recorded session')
        batch.append(event)
        if len(batch) >= m3.PART:
            flush()
    if batch:
        flush()
    return report


def construct(binary, dev_home, days=90, disk_ok=False, *, observed, found=None, now=None):
    if days not in (90, 365) or (days == 365 and not disk_ok):
        raise ValueError('The year requires --disk-ok; supported sizes are 90 and 365 days')
    binary = str(Path(binary).expanduser().resolve())
    common.guard(session_ids=observed['sessions'])
    count_path = Path(common.E) / 'corpus-count.json'
    counts = load(count_path)['raw_rate']
    if counts.get('days') != 90 or counts.get('failed_files') != 0:
        raise ValueError('A successful recorded 90-day corpus count is required')
    target = counts['events'] if days == 90 else counts['events_per_year']
    if type(target) is not int or target <= 0:
        raise ValueError('Invalid recorded event rate')
    now = now or datetime.fromtimestamp(count_path.stat().st_mtime, timezone.utc)
    since = now - timedelta(days=90)
    manifest = load(Path(common.E) / 'replay' / 'manifest.json')
    selected = source_sessions(m4.transcripts() if found is None else found, manifest, observed['sessions'], since)
    if not selected:
        raise ValueError('No new dev transcripts for the scale home')
    home = new_home()
    try:
        copy_dev(dev_home, home)
        if home_rates(home) != observed:
            raise ValueError('The dev home changed while its rates and snapshot were recorded')
        out = dict(common.record(binary, str(home), 0, 'loopback-stub', {'curator': ['m22-stub'], 'embedder': ['bge-m3-stub']}),
                   complete=False, days=days, dev_home=str(Path(dev_home).resolve()), observed=observed,
                   target_events=target, imported_documents_expected=DOCUMENTS, replay_sessions=[],
                   rates_recorded_before_build=True, rate_window_end=now.isoformat(),
                   rate_window_source='corpus-count file modification time',
                   corpus_count_sha256=common.sha256_file(count_path))
        save(home / 'm22-build.json', out)
        copies = home / 'transcripts'
        copies.mkdir()
        selected = [dict(s, side='dev') for s in selected]
        replay_set.copy_out(selected, str(copies))
        # copy_out records every main/subagent file hash before transcript conversion.
        out['replay_sessions'] = selected
        save(home / 'm22-build.json', out)
        spool = home / 'm22-events.jsonl'
        total = 0
        with spool.open('x', encoding='utf-8') as output:
            for s in selected:
                path = copies / 'dev' / s['agent'] / (s['session'] + '.jsonl')
                for relative, digest in s['files'].items():
                    if common.sha256_file(copies / relative) != digest:
                        raise ValueError('Recorded transcript copy changed before replay')
                text = common.command([binary, '--home', str(home), 'transcript', str(path), '--agent', s['agent']],
                                      env=environment())
                events = []
                for line in text.split('\n'):
                    if line.strip():
                        event = json.loads(line)
                        stamp = datetime.fromisoformat(event['ts'].replace('Z', '+00:00'))
                        if since <= stamp <= now:
                            if event.get('session') != s['session']:
                                raise ValueError('Transcript conversion changed the recorded session')
                            events.append(event)
                events.sort(key=lambda e: e['ts'])
                for e in events:
                    output.write(json.dumps(e, ensure_ascii=False) + '\n')
                total += len(events)
        if total == 0:
            raise ValueError('No events in the measured 90-day window')
        def events():
            with spool.open(encoding='utf-8') as f:
                yield from (json.loads(line) for line in f)
        replay = replay_parts(binary, home, events())
        if days == 365:
            # Real copied events, shifted in time and session identity, never synthesized claims/rows.
            remaining, cycle = max(0, target - total), 1
            cycles = math.ceil(remaining / total)
            while remaining:
                take = min(remaining, total)
                def shifted():
                    for i, e in enumerate(events()):
                        if i >= take:
                            break
                        e = copy.deepcopy(e)
                        old = e['session']
                        e['session'] = f'm22-year-{cycle}-{old}'
                        e['payload']['session_id'] = e['session']
                        stamp = datetime.fromisoformat(e['ts'].replace('Z', '+00:00')) - timedelta(
                            days=275 * cycle / cycles)
                        e['ts'] = stamp.isoformat()
                        yield e
                extra = replay_parts(binary, home, shifted())
                replay['events'] += extra['events']
                replay['parts'] += extra['parts']
                remaining -= take
                cycle += 1
        source = Path(common.E) / 'claude-mem-2026-09-24.db'
        imported = json.loads(common.command([binary, '--home', str(home), 'import', 'claude-mem', str(source),
                                               '--eval-store'], env=environment()))
        if not isinstance(imported, dict):
            raise ValueError('Invalid import result')
        with loopback(observed['claims_per_record']) as server:
            configure(home, server)
            common.command([binary, '--home', str(home), 'worker', '--idle-ms', '0'], env=environment())
        with database(home, 'knowledge.db') as k:
            documents = k.execute('SELECT COUNT(*) FROM imported').fetchone()[0]
            vectors = k.execute('SELECT COUNT(*) FROM vectors').fetchone()[0]
        if documents != DOCUMENTS or vectors < 1:
            raise ValueError('Imported corpus or embedded scale store is incomplete')
        actual = home_rates(home)
        out.update(complete=True, N=actual['records'], actual=actual, imported_documents=documents,
                   vectors=vectors, replay=replay, scale_target_reached=replay['events'] >= target,
                   binary_sha256=common.sha256_file(binary), event_rate_per_day=counts['events'] / 90)
        out['new_claims_per_record'] = ((actual['claims'] - observed['claims']) /
                                      (actual['records'] - observed['records'])
                                      if actual['records'] > observed['records'] else None)
        out['new_claims_target'] = int(observed['claims_per_record'] * (actual['records'] - observed['records']))
        out['claim_rate_target_reached'] = actual['claims'] - observed['claims'] >= out['new_claims_target']
        save(home / 'm22-build.json', out)
        shutil.rmtree(copies)
        spool.unlink()
        configure(home)
        return out
    except Exception:
        cleanup(home)
        raise


def hook_comparison(scale, dev, lines):
    baseline = number(scale['hook_spawn_ms']['1KB']['p95'])
    read = number(scale['read']['warm']['read_in_process_us']['p95'])
    dev_read = number(dev['in_process_read_p95_us'], True)
    out = {}
    for hook in ('session_start', 'prompt'):
        line = number(lines['line_ms'][hook], True)
        dev95 = number(dev['line_ms'][hook], True)
        warm = number(scale['read']['warm'][hook + '_ms']['p95'])
        cold = number(scale['read']['cold'][hook + '_ms']['p95'])
        ratio = line / dev95
        out[hook] = dict(line_ms=line, wsl_dev_p95_ms=dev95, wsl_scale_p95_ms=warm,
                         machine_ratio=ratio, estimated_slowest_p95_ms=warm * ratio,
                         cold_estimated_slowest_p95_ms=cold * ratio, **{'pass': warm * ratio <= line},
                         cold_pass=cold * ratio <= line, read_share_ms=warm - baseline,
                         in_process_read_p95_us=read, in_process_read_ratio=read / dev_read)
    return out


@contextmanager
def no_auto_worker():
    previous = os.environ.get('OBOETE_NO_SPAWN')
    os.environ['OBOETE_NO_SPAWN'] = '1'
    try:
        yield
    finally:
        if previous is None:
            os.environ.pop('OBOETE_NO_SPAWN', None)
        else:
            os.environ['OBOETE_NO_SPAWN'] = previous


def query_calls(home):
    with database(home, 'providers.db') as db:
        return db.execute("SELECT COUNT(*) FROM provider_calls WHERE role = 'query'").fetchone()[0]


@contextmanager
def writer(binary, home, checkout, rate):
    """One owned worker plus scheduled spawned hooks; report the cadence actually achieved."""
    stop, ready = threading.Event(), threading.Event()
    report = dict(requested_events_per_second=rate, stamps=[], failed=False)
    worker = subprocess.Popen([binary, '--home', str(home), 'worker', '--idle-ms', '60000'],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=environment())
    def write():
        start = time.perf_counter()
        index = 0
        try:
            while not stop.is_set():
                payload = dict(session_id='m22-writer-' + home.name, cwd=str(checkout), tool_name='Read',
                               tool_input={'file_path': 'm22-scale-fixture'},
                               tool_response='Concurrent scale record ' + str(index), tool_use_id=f'm22-{index}')
                common.command([binary, '--home', str(home), 'hook', 'claude', 'PostToolUse'],
                               input=json.dumps(payload), env=environment(), cwd=str(checkout), timeout=30)
                report['stamps'].append(time.perf_counter())
                ready.set()
                index += 1
                stop.wait(max(0, start + index / rate - time.perf_counter()))
        except Exception:
            report['failed'] = True
            ready.set()
    thread = threading.Thread(target=write, daemon=True)
    thread.start()
    try:
        if not ready.wait(30) or report['failed'] or worker.poll() is not None:
            raise ValueError('Concurrent writer or worker did not start')
        yield report
    finally:
        stop.set()
        thread.join(timeout=35)
        if thread.is_alive():
            report['failed'] = True
        if worker.poll() is not None:
            report['failed'] = True
        if worker.poll() is None:
            worker.terminate()
        try:
            worker.wait(timeout=10)
        except subprocess.TimeoutExpired:
            worker.kill()
            worker.wait()
        stamps = report.pop('stamps')
        report['events'] = len(stamps)
        report['observed_events_per_second'] = ((len(stamps) - 1) / (stamps[-1] - stamps[0])
                                                 if len(stamps) >= 2 and stamps[-1] > stamps[0] else None)
        report['counted'] = (not report['failed'] and report['observed_events_per_second'] is not None
                             and abs(report['observed_events_per_second'] / rate - 1) <= 0.2)


def read_fixture(home):
    original = common.read_jsonl(Path(common.E) / 'replay' / 'events-1000.jsonl')
    if len(original) != 1000:
        raise ValueError('Cold scale injection needs the recorded 1,000-event dev fixture twice')
    path = home / 'm22-read-2000.jsonl'
    with path.open('x', encoding='utf-8') as f:
        for arm in range(2):
            for event in original:
                event = copy.deepcopy(event)
                event['session'] = f'm22-read-{home.name}-{arm}-' + event['session']
                event['payload']['session_id'] = event['session']
                f.write(json.dumps(event, ensure_ascii=False) + '\n')
    return path


def evaluate(binary, home, checkout, workers_ai_latency, lines, wsl_dev):
    import hooks                         # supplied by Task 12b's independent hooks writer
    home = owned_home(home)
    binary, checkout = str(Path(binary).expanduser().resolve()), Path(checkout).expanduser().resolve()
    if not checkout.is_dir():
        raise ValueError('Checkout is not a directory')
    built = load(home / 'm22-build.json')
    if not built.get('complete') or built.get('binary_sha256') != common.sha256_file(binary):
        raise ValueError('Complete scale home from this binary is required')
    latency, all_lines, dev = load(workers_ai_latency), load(lines), load(wsl_dev)
    real_latency(latency)
    dev_summary = hooks.summary(dev['runs'], dev['requested_runs'])
    if (dev['sha256'] != common.sha256_file(binary) or dev['home'] != built['dev_home'] or
            dev['machine'] != common.record(binary, str(home), 0, '', {})['machine'] or
            any(dev_summary['line_ms'][h] is None for h in hooks.HOOKS)):
        raise ValueError('WSL dev line must match this binary, machine and original dev home')
    if (not all_lines.get('complete') or len(all_lines.get('machines', [])) != 3 or
            len({m['machine_label'] for m in all_lines['machines']}) != 3):
        raise ValueError('The fixed combined line needs all three distinct machines')
    dev_summary['in_process_read_p95_us'] = max(
        number(r['report']['read']['warm']['read_in_process_us']['p95'], True) for r in dev['runs'])
    questions = select_queries(common.read_jsonl(Path(common.E) / 'queries.jsonl'))
    result = dict(common.record(binary, str(home), len(questions) - WARMUP, 'loopback-stub', built['models']),
                  build=built, warmup=WARMUP, query_ids=[q['qid'] for q in questions], legs={},
                  embedding_latency_sha256=common.sha256_file(workers_ai_latency),
                  lines_sha256=common.sha256_file(lines), wsl_dev_sha256=common.sha256_file(wsl_dev),
                  complete=False, forget='milestone 5; not measured', first_sync='milestone 6; not measured',
                  imac_scale='not measured; run on the iMac at the size its disk holds')
    path = directory() / (home.name + '-result.json')
    save(path, result)
    with loopback(built['observed']['claims_per_record']) as server, no_auto_worker():
        configure(home, server)
        # The binary's outbound gate has the exact query text the embed endpoint receives.
        server.query_texts = {common.command([binary, '--home', str(home), 'gate'], input=q['query'],
                                            env=environment())[:1000] for q in questions}
        def measure():
            before = query_calls(home)
            with common.Mcp(binary, str(home), str(checkout)) as client:
                measured = query_leg(client, questions, lambda: server.query_embeds)
            if query_calls(home) - before != len(questions):
                raise ValueError('Query-role ledger does not show one embedding per call')
            return measured
        for rate in (2, 20):
            # This owned worker exits idle; its final step runs backup::run before releasing its lock.
            common.command([binary, '--home', str(home), 'worker', '--idle-ms', '0'], env=environment())
            idle = measure()
            with writer(binary, home, checkout, rate) as load_report:
                written = measure()
            comparison = metrics(idle['timed'], written['timed'], latency)
            if not load_report['counted']:
                comparison = dict(counted=False, reason='The requested concurrent writer cadence was not achieved')
            result['legs'][str(rate)] = dict(idle=idle, written=written, writer=load_report, comparison=comparison)
            save(path, result)
        common.command([binary, '--home', str(home), 'worker', '--idle-ms', '0'], env=environment())
        # Read hooks must make no provider call; a configured stub detects violations.
        fixture = read_fixture(home)
        before = server.requests
        report = json.loads(common.command([binary, '--home', str(home), 'replay', str(fixture),
                                           '--repo-root', str(checkout), '--read-sample', str(hooks.SAMPLES),
                                           '--read-warmup', str(hooks.WARMUP), '--spawn-sample', str(hooks.SAMPLES),
                                           '--sizes', '1'], env=environment(), cwd=str(home)))
        checked = hooks.measurements(report)
        if not checked['counted'] or server.requests != before:
            raise ValueError('Scale read-hook samples or no-embedding invariant failed')
        result['injection'] = dict(samples=hooks.SAMPLES, warmup=hooks.WARMUP, fixture_events=2000,
                                    comparison=hook_comparison(report, dev_summary, all_lines), report=report)
    result['complete'] = bool(built.get('scale_target_reached') and built.get('claim_rate_target_reached') and
                              all(leg['comparison']['counted'] for leg in result['legs'].values()))
    save(path, result)
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    build = commands.add_parser('build', help='Build a copied dev home with the recorded scale corpus')
    build.add_argument('--binary', required=True)
    build.add_argument('--dev-home', required=True)
    build.add_argument('--days', type=int, choices=(90, 365), default=90)
    build.add_argument('--disk-ok', action='store_true')
    run = commands.add_parser('run', help='Measure then delete an owned scale home')
    for name in ('binary', 'home', 'checkout', 'workers-ai-latency', 'lines', 'wsl-dev'):
        run.add_argument('--' + name, required=True)
    args = parser.parse_args(argv)
    if args.command == 'run':
        home = owned_home(args.home)
        try:
            with (home / 'm22-run.json').open('x') as f:
                json.dump({'pid': os.getpid()}, f)
        except FileExistsError:
            raise ValueError('This scale home is already running; do not replay or delete it twice') from None
        try:
            return evaluate(args.binary, home, args.checkout, args.workers_ai_latency, args.lines, args.wsl_dev)
        except Exception:
            path = directory() / (home.name + '-result.json')
            result = load(path) if path.exists() else {'home': str(home), 'complete': False}
            result.update(complete=False, failure='M22 run failed before completion')
            save(path, result)
            raise
        finally:
            cleanup(home)
    if args.days == 365 and not args.disk_ok:
        raise ValueError('The year requires --disk-ok')
    measured = home_rates(args.dev_home)
    common.guard(session_ids=measured['sessions'])
    return construct(args.binary, args.dev_home, args.days, args.disk_ok, observed=measured)


if __name__ == '__main__':
    print(json.dumps(main(), indent=1))
