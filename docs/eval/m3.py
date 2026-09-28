"""Milestone 3, Task 13: M3 (decisions) and M2's coverage on the dev transcripts, curated by Design B
(docs/milestone-3-plan.md Task 13, docs/spike/m3-dev.md). Dev only: held-out transcripts are never
read here. Every command takes the binary by path: `oboete` on PATH is the owner's v1.

  m3.py fixtures <bin>          each transcript the dev labels or the replay set's dev side need ->
                                a fixture, by <bin>'s own `transcript`
  m3.py replay <bin> <name>     every fixture, merged in time order, into one home; curation off
  m3.py map <bin> <name>        each labeled decision -> the records its quote is in (before stub:
                                the worker compresses the bodies it reads)
  m3.py stub <bin> <name> [--shrink] [--tokens=N]
                                curate that home with a localhost stub that answers no claims: the
                                windows, their estimated tokens, M2's coverage; no call leaves
  m3.py live <bin> <name> [--yes]
                                each labeled window sent again to one live entry by `recurate`
                                (estimates only without --yes)
  m3.py score <bin> <name>      M3's counts on the labeled items"""
import collections, glob, http.server, json, os, re, sqlite3, subprocess, sys, threading, time
from datetime import datetime

from common import E, clean_env, owner_only, read_jsonl, sha256_file

M = f'{E}/m3'
PART = 20_000  # events per replayed part: `oboete replay` reads a fixture whole


def home(binary, name):
    """The home a binary made, by its hash; or `name` itself when it is a path (a later binary on a
    home an earlier one cut, docs/spike/m3-dev.md)."""
    return name if name.startswith('/') else f'{M}/{sha256_file(binary)[:12]}/{name}'


def transcripts():
    """{session: (agent, path)} for every transcript of the replay set's dev side and dev-extra."""
    out = {}
    for side in ('dev', 'dev-extra'):
        for path in glob.glob(f'{E}/replay/{side}/*/*.jsonl'):
            out[os.path.basename(path)[:-6]] = (os.path.basename(os.path.dirname(path)), path)
    return out


def labels():
    """(decision keys with the owner's value, pair keys with the owner's value, drafts by id)."""
    value = lambda f: {r['id']: r['value'] for r in read_jsonl(f'{E}/labels/{f}')}
    dv, pv = value('dev-decisions.jsonl'), value('dev-pairs.jsonl')
    decisions = [dict(d, value=dv.get(d['id'])) for d in read_jsonl(f'{E}/labels/dev-decisions.key.jsonl')]
    pairs = [dict(p, value=pv.get(p['id'])) for p in read_jsonl(f'{E}/labels/dev-pairs.key.jsonl')]
    drafts = {d['id']: d for d in read_jsonl(f'{E}/labels/drafts/decisions.jsonl')}
    return decisions, pairs, drafts


def sessions():
    """The sessions to replay: the replay set's dev side, and every session a dev label is in."""
    with open(f'{E}/replay/manifest.json') as f:
        wanted = {s['session'] for s in json.load(f)['sessions'] if s['side'] == 'dev'}
    decisions, pairs, drafts = labels()
    wanted |= {d['session'] for d in decisions}
    wanted |= {drafts[p[k]]['session'] for p in pairs for k in ('earlier', 'later')}
    return sorted(wanted)


def fixtures(binary):
    known = transcripts()
    os.makedirs(f'{M}/fixtures', exist_ok=True)
    for s in sessions():
        out = f'{M}/fixtures/{s}.jsonl'
        if os.path.exists(out):
            continue
        agent, path = known[s]
        with open(out + '.part', 'w', encoding='utf-8') as f:
            subprocess.run([binary, 'transcript', path, '--agent', agent], stdout=f, check=True, env=clean_env())
        os.replace(out + '.part', out)
    print(f'{len(sessions())} fixtures in {M}/fixtures')


def when(ts):
    return datetime.fromisoformat(ts.replace('Z', '+00:00')).timestamp()


def config(h, providers, curate, shrink=False, tokens=None):
    size = f'window_tokens = {tokens}\n' if tokens else ''
    with open(f'{h}/config.toml', 'w') as f:
        f.write(f'[summary]\ncurate = {str(curate).lower()}\nshrink = {str(shrink).lower()}\n{size}\n'
                + providers)


def replay(binary, name):
    """One home for every session, so a later session's claim can supersede an earlier one's; the
    events of all sessions in time order, as the hooks would have received them."""
    h = home(binary, name)
    if os.path.exists(f'{h}/raw.db'):
        sys.exit(f'{h} is replayed already: a second replay would insert every event twice')
    os.makedirs(h)
    config(h, '', False)
    events = []
    for s in sessions():
        for i, line in enumerate(open(f'{M}/fixtures/{s}.jsonl', encoding='utf-8')):
            if line.strip():
                events.append((when(json.loads(line)['ts']), s, i, line))
    events.sort(key=lambda e: e[:3])
    reports = []
    for n in range(0, len(events), PART):
        part = f'{h}/part.jsonl'
        with open(part, 'w', encoding='utf-8') as f:
            f.writelines(e[3] if e[3].endswith('\n') else e[3] + '\n' for e in events[n:n + PART])
        r = subprocess.run([binary, '--home', h, 'replay', part, '--agent', 'all', '--spawn-sample', '0'],
                           capture_output=True, text=True, check=True, env=clean_env())
        reports.append(json.loads(r.stdout))
        os.remove(part)
    with open(f'{h}/replay.json', 'w') as f:
        json.dump({'binary': sha256_file(binary)[:12], 'sessions': len(sessions()), 'events': len(events),
                   'parts': reports}, f, indent=1)
    print(f'{len(events)} events of {len(sessions())} sessions into {h}')


class Stub(http.server.BaseHTTPRequestHandler):
    """An OpenAI-compatible endpoint that answers every curator request with no claims and every
    digest request with no lines."""

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        schema = json.dumps(body.get('response_format', {}))
        content = {'lines': []} if '"lines"' in schema else {'claims': [], 'summary': 's'}
        out = json.dumps({'id': 'stub', 'object': 'chat.completion', 'model': 'stub', 'choices': [
            {'index': 0, 'message': {'role': 'assistant', 'content': json.dumps(content)},
             'finish_reason': 'stop'}]}).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(out)))
        self.end_headers()
        self.wfile.write(out)

    def log_message(self, *_):
        pass


def windows(h):
    """The window ops that move the checkpoint, in op_seq order."""
    raw = sqlite3.connect(f'file:{h}/raw.db?mode=ro', uri=True)
    ops = [json.loads(b) for (b,) in raw.execute("SELECT body FROM ops WHERE type = 'window' ORDER BY op_seq")]
    top = raw.execute("SELECT MAX(seq) FROM records").fetchone()[0]
    return [w for w in ops if not w.get('recurate')], top


def coverage(ops, top):
    """M2's coverage half (spec 8.2): the windows run from seq 1 to the last record with no gap and
    no overlap, each curated, covered or skipped. (the first break or None, outcome counts)"""
    at, counts = (1, None), {}
    for w in ops:
        start = (w['from_seq'], w.get('from_offset'))
        if start != at:
            return f'a window starts at {start}, the last one ended at {at}', counts
        counts[w['outcome']] = counts.get(w['outcome'], 0) + 1
        if w['outcome'] not in ('curated', 'covered', 'skipped'):
            return f'a window has outcome {w["outcome"]}', counts
        at = (w['to_seq'], w['to_offset']) if w.get('to_offset') is not None else (w['to_seq'] + 1, None)
    return (None if at == (top + 1, None) else f'the windows end at {at}, the records at {top}'), counts


def stub(binary, name, shrink, tokens):
    h = home(binary, name)
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Stub)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    config(h, f'[[providers]]\nkind = "openai"\nname = "stub"\n'
              f'base_url = "http://127.0.0.1:{server.server_port}/v1"\nmodel = "stub"\n'
              f'daily_budget = 1000000\n', True, shrink, tokens)
    # A worker exits when it is idle, and a window at the last record waits for the owner's next
    # hook record; replayed records are not hook records, so none waits here.
    subprocess.run([binary, '--home', h, 'worker', '--idle-ms', '0'], check=True, env=clean_env())
    server.shutdown()
    ops, top = windows(h)
    broken, counts = coverage(ops, top)
    db = sqlite3.connect(f'file:{h}/providers.db?mode=ro', uri=True)
    calls = db.execute("SELECT role, outcome, COUNT(*), SUM(est_tokens), MAX(est_tokens), SUM(bytes_out) "
                       "FROM provider_calls GROUP BY role, outcome").fetchall()
    raw = sqlite3.connect(f'file:{h}/raw.db?mode=ro', uri=True)
    repos = raw.execute("SELECT repo IS NULL, COUNT(DISTINCT repo), COUNT(DISTINCT session), COUNT(*) "
                        "FROM records WHERE type = 'event' GROUP BY repo IS NULL").fetchall()
    shortened = sum(len(w.get('shortened', [])) for w in ops)
    report = {'shrink': shrink, 'tokens': tokens, 'records': top, 'windows': len(ops), 'outcomes': counts,
              'coverage': broken or '100%', 'shortened': shortened,
              'calls': [dict(zip(('role', 'outcome', 'calls', 'est_tokens', 'max_est', 'bytes'), c)) for c in calls],
              'repos': [dict(zip(('no_repo', 'repos', 'sessions', 'records'), r)) for r in repos]}
    with open(f'{h}/stub.json', 'w') as f:
        json.dump(report, f, indent=1)
    print(json.dumps(report, indent=1))


def strings(v):
    if isinstance(v, str):
        yield v
    elif isinstance(v, dict):
        for x in v.values():
            yield from strings(x)
    elif isinstance(v, list):
        for x in v:
            yield from strings(x)


def records_of(raw, session):
    for seq, ts, kind, enc, body in raw.execute(
            "SELECT seq, ts, kind, enc, body FROM records WHERE type = 'event' AND session = ? ORDER BY seq",
            (session,)):
        if enc != 'plain' or body is None:
            continue
        yield seq, ts, kind, list(strings(json.loads(body)))


# Records that repeat earlier text (a compaction's summary, a subagent's envelope) never anchor one.
REPEATS = ('compaction', 'envelope')


def map_labels(name, binary):
    """Each labeled decision (and each draft a pair names) -> the record its quote is in: of the
    session's records that hold the quote (else its first 12 characters, where the transcript and
    the hook shaped the text apart), the one nearest the label's time. Run before the worker, whose
    compression leaves bodies unreadable here. Counts only: no label or record text is printed."""
    h = home(binary, name)
    raw = sqlite3.connect(f'file:{h}/raw.db?mode=ro', uri=True)
    # records_of skips a compressed body: a map after the stub would leave most labels unmapped.
    (packed,) = raw.execute("SELECT COUNT(*) FROM records WHERE type = 'event' AND enc != 'plain'").fetchone()
    if packed:
        sys.exit(f'{packed} records are compressed: map runs after replay and before stub')
    decisions, pairs, drafts = labels()
    items = {d['id']: d for d in decisions}
    for p in pairs:
        for k in ('earlier', 'later'):
            items.setdefault(p[k], drafts[p[k]])
    found, cache, how = {}, {}, {}
    for i, d in items.items():
        if d['session'] not in cache:
            cache[d['session']] = [r for r in records_of(raw, d['session']) if r[2] not in REPEATS]
        label_ms = int(when(d['ts']) * 1000)
        for match, needle in (('quote', d['quote']), ('prefix', d['quote'][:12])):
            hits = [(seq, ts) for seq, ts, _, texts in cache[d['session']] if any(needle in t for t in texts)]
            if hits:
                break
        else:
            match = 'none'
        seq = min(hits, key=lambda x: abs(x[1] - label_ms))[0] if hits else None
        tool = raw.execute("SELECT json_extract(body, '$.tool') FROM records WHERE seq = ? AND kind = 'tool'",
                           (seq,)).fetchone() if seq else None
        found[i] = {'who': d['who'], 'seq': seq, 'match': match, 'hits': len(hits), 'tool': tool and tool[0]}
        how[(d['who'], match)] = how.get((d['who'], match), 0) + 1
    with open(f'{h}/map.json', 'w') as f:
        json.dump(found, f, indent=1)
    print(f'{len(found)} labeled items:', {f'{w} {m}': c for (w, m), c in sorted(how.items())})


# The live half's one entry: a single model for both arms, so a difference is the shrink's and
# not the chain's (docs/spike/m3-dev.md). A subscription: no bill, within owner decision 30.
# A timeout well past haiku's slowest answer (162 s of 22): at the shipped 180 s one call in about
# 40 timed out and cooled the entry for ten minutes (#193); the product sends that window later.
LIVE = '[[providers]]\nkind = "cli"\nname = "claude"\ncli = "claude"\nmodel = "haiku"\ntimeout_s = 600\n'


def spans(h, tool=None):
    """The windows that hold a labeled record (only those in a call of `tool`, when given), as
    record spans in seq order, each once. Windows that share a record (a record split across
    windows) are one span: `recurate` cuts a span's windows itself, and two spans sharing a record
    would send that record twice, the second run retracting what the first derived."""
    ops, _ = windows(h)
    with open(f'{h}/map.json') as f:
        seqs = {r['seq'] for r in json.load(f).values() if r['seq'] is not None}
    if tool:
        with open(f'{h}/map.json') as f:
            seqs = {r['seq'] for r in json.load(f).values() if r['seq'] is not None and r.get('tool') == tool}
    out = []
    for a, b in sorted({(w['from_seq'], w['to_seq']) for w in ops
                        if any(w['from_seq'] <= s <= w['to_seq'] for s in seqs)}):
        if out and a <= out[-1][1]:
            out[-1] = (out[-1][0], max(out[-1][1], b))
        else:
            out.append((a, b))
    return out


def partly(out):
    """`recurate` printed that it curated some of the span's windows."""
    return re.search(r'^[1-9][0-9]* window\(s\) curated again', out, re.M)


def cooldown(h):
    """Sleep until the live entry's (claude's, LIVE) cooldown is over, and 5 s more. An entry held
    until the owner acts (`credits_required`, `OWNER_HOLD` in src/providers_db.rs) has no end to
    sleep to, so the pass stops there (#228)."""
    p = sqlite3.connect(f'file:{h}/providers.db?mode=ro', uri=True)
    row = p.execute("SELECT down_until FROM provider_state WHERE provider = 'claude'").fetchone()
    p.close()
    until = row[0] if row else 0
    if until == 2**63 - 1:
        sys.exit(f'claude is held until the owner acts: run `<binary> --home {h} resume claude`, then this '
                 'again; nothing more was sent')
    time.sleep(max(0, until / 1000 - time.time()) + 5)


def live(binary, name, send, tool=None):
    """Each labeled window sent again to the live entry with `oboete recurate`, one span at a time
    in seq order, so an earlier claim is a candidate for a later window. Without `send`, only the
    estimates `recurate` prints."""
    h = home(binary, name)
    with open(f'{h}/config.toml') as f:
        cut = f.read()
    size = [line.split('=')[1].strip() for line in cut.splitlines() if line.startswith('window_tokens')]
    config(h, LIVE, False, 'shrink = true' in cut, size[0] if size else None)
    raw = sqlite3.connect(f'file:{h}/raw.db?mode=ro', uri=True)
    device = raw.execute("SELECT value FROM meta WHERE key = 'device_id'").fetchone()[0]
    raw.close()
    log = f'{h}/live{"-sent" if send else ""}.jsonl'
    # The curator's claude through a wrapper that keeps its stdout under M/answers, so the drafts
    # the gates drop can be read (`m3.py drafts`); the wrapper is made by hand, owner-only.
    env = clean_env()
    if os.path.exists(f'{M}/wrap/claude'):
        env['PATH'] = f'{M}/wrap:' + env['PATH']
    # A span is done once its attempts ended: curated, partly curated (below), or failed three
    # times. A pass cut between two calls goes on where it stopped, so a resumed arm is still one pass.
    tries, done = collections.Counter(), set()
    for r in read_jsonl(log) if os.path.exists(log) else []:
        k = json.dumps(r['span'])
        tries[k] += 1
        if r['code'] == 0 and (not send or 'not curated' not in r['out']) or send and partly(r['out']) or tries[k] == 3:
            done.add(k)
    # The log must be this pass's first spans. One cut another way (another --tool selection, or
    # spans not yet merged) would send records again, or curate an earlier span after a later one
    # and so with claims from its future as candidates (#228).
    todo = spans(h, tool)
    if set(tries) != {json.dumps([a, b]) for a, b in todo[:len(tries)]}:
        sys.exit(f'{log} is not the first spans of this pass: it was cut with another --tool selection or '
                 'before spans were merged. Resume it with that selection, or use a new home')
    if send:
        cooldown(h)  # the pass may have been cut while it waited out a cooldown
    tokens = 0
    for a, b in todo:
        if json.dumps([a, b]) in done:
            continue
        # A failure is retried after the entry's cooldown, up to three times: the product would
        # list the span as skipped and send it on a later run, and a cooled-down entry must not
        # turn the rest of the pass into instant failures (the whole arm lost 65 spans that way).
        for attempt in range(tries[json.dumps([a, b])], 3):
            r = subprocess.run([binary, '--home', h, 'recurate', f'{device}:{a}-{b}'] + (['--yes'] if send else []),
                               capture_output=True, text=True, env=env)
            out = r.stdout.strip()
            with open(log, 'a', encoding='utf-8') as f:
                f.write(json.dumps({'span': [a, b], 'code': r.returncode, 'out': out, 'err': r.stderr[-500:],
                                    'attempt': attempt}) + '\n')
            if r.returncode == 0 and (not send or 'not curated' not in out):
                break
            # Some of the span's windows were curated and one was not: sending the span again would
            # curate those windows a second time with their first claims as candidates, a history
            # the other arm does not have (review on #218). The window is left, as the product
            # leaves it for its next run.
            if send and partly(out):
                break
            cooldown(h)
        for word in out.split(','):
            if 'tokens' in word and 'about' in word:
                tokens += int(word.split('about')[1].split('tokens')[0].strip().replace(',', ''))
    print(f'{len(todo)} spans, about {tokens} tokens this pass; log {log}')


def score(binary, name):
    """M3 on the labeled items, by the definitions of docs/spike/m3-dev.md. Counts only."""
    h = home(binary, name)
    k = sqlite3.connect(f'file:{h}/knowledge.db?mode=ro', uri=True)
    with open(f'{h}/map.json') as f:
        where = json.load(f)
    decisions, pairs, drafts = labels()
    # Each active claim with the seqs its active derivation quotes.
    claims = {}
    for uid, status, seq in k.execute(
            "SELECT a.uid, a.status, e.seq FROM active a JOIN claims c ON c.uid = a.uid "
            "JOIN evidence e ON e.op_device = c.op_device AND e.op_seq = c.op_seq"):
        claims.setdefault(uid, [status, set()])[1].add(seq)
    superseded = {u for (u,) in k.execute(
        "SELECT e.to_uid FROM edges e JOIN claims c ON c.op_device = e.op_device AND c.op_seq = e.op_seq "
        "WHERE e.type = 'supersedes'")}

    raw = sqlite3.connect(f'file:{h}/raw.db?mode=ro', uri=True)

    def records_of_item(item):
        """The labeled record, and for an accepted proposal whose label quotes the owner's
        acceptance (a prompt), the assistant's reply right before it in the session: the claim an
        acceptance promotes quotes the proposal, not the "yes"."""
        seq = where[item]['seq']
        if where[item]['who'] != 'assistant_accepted':
            return {seq}
        kind, session = raw.execute('SELECT kind, session FROM records WHERE seq = ?', (seq,)).fetchone()
        if kind != 'prompt':
            return {seq}
        (reply,) = raw.execute("SELECT max(seq) FROM records WHERE session = ? AND kind = 'reply' AND seq < ?",
                               (session, seq)).fetchone()
        return {seq, reply} - {None}

    def state(item):
        at = records_of_item(item)
        mine = [(u, st) for u, (st, seqs) in claims.items() if at & seqs]
        if not mine:
            return 'none'
        if any(st == 'decided' and u not in superseded for u, st in mine):
            return 'current'
        if any(st == 'decided' for _, st in mine):
            return 'decided, superseded'
        if any(u in superseded for u, _ in mine):
            return 'superseded'
        if any(st == 'retracted' for _, st in mine):
            return 'retracted'
        return 'other:' + ','.join(sorted({st for _, st in mine}))

    out = {'decisions': {}, 'pairs': {}}
    for d in decisions:
        if where[d['id']]['seq'] is None:
            continue
        key = f'{d["value"]}'
        st = state(d['id'])
        out['decisions'].setdefault(key, {}).setdefault(st, 0)
        out['decisions'][key][st] += 1
    for p in pairs:
        if where[p['earlier']]['seq'] is None or where[p['later']]['seq'] is None:
            continue
        cross = drafts[p['earlier']]['session'] != drafts[p['later']]['session']
        key = f'{p["value"]}{" cross" if cross else ""}'
        st = state(p['earlier'])
        out['pairs'].setdefault(key, {}).setdefault(st, 0)
        out['pairs'][key][st] += 1
    decided = sum(1 for st, _ in claims.values() if st == 'decided')
    out['claims'] = {'active': len(claims), 'decided': decided}
    yes = out['decisions'].get('yes', {})
    out['recall'] = f'{yes.get("current", 0) + yes.get("decided, superseded", 0)} of {sum(yes.values())}'
    with open(f'{h}/score.json', 'w') as f:
        json.dump(out, f, indent=1)
    print(json.dumps(out, indent=1, ensure_ascii=False))


def answer_of(path):
    """The curator's answer in one kept claude stream (M/answers), or None."""
    for line in open(path, encoding='utf-8'):
        try:
            e = json.loads(line)
        except ValueError:
            continue
        if e.get('type') == 'result' and not e.get('is_error'):
            text = e.get('result', '').strip()
            if text.startswith('```'):
                text = text.split('\n', 1)[1].rsplit('```', 1)[0]
            try:
                return json.loads(text)
            except ValueError:
                return None
    return None


def drafts(binary, name):
    """For each owner-yes decision in a recurated window whose answer was kept: what the curator
    drafted about it and what the gates did (kept, lowered, dropped), so a miss is told apart from
    a draft the gates dropped. A draft is about a label when its quote shares 8 characters with
    the label's quote or its prompt line."""
    h = home(binary, name)
    where = json.load(open(f'{h}/map.json'))
    decisions, _, _ = labels()
    # Every kept answer by its summary; a window op takes the one whose drafts it kept (ids and
    # bodies alike), so two answers with one summary (the same window in two arms, a rerun) are
    # told apart, and a window two of them fit is left out and counted (#196).
    answers = collections.defaultdict(list)
    for f in glob.glob(f'{M}/answers/*.jsonl'):
        a = answer_of(f)
        if a and a.get('summary'):
            answers[a['summary'].strip()].append(a)

    def answer_for(b):
        fits = [a for a in answers.get(b.get('summary', '').strip(), [])
                if all(any(c.get('id') == i and c.get('body') == k.get('body') for c in a.get('claims', []))
                       for i, k in b['claims'].items())]
        return fits[0] if len(fits) == 1 else ('ambiguous' if fits else None)
    raw = sqlite3.connect(f'file:{h}/raw.db?mode=ro', uri=True)
    ops = [(s, t, json.loads(b)) for s, t, b in raw.execute('SELECT op_seq, type, body FROM ops ORDER BY op_seq')]
    wins, cur = [], None
    for s, t, b in ops:
        if t == 'window':
            cur = b if b.get('recurate') and b.get('outcome') == 'curated' else None
            if cur is not None:
                cur['claims'] = {}
                wins.append(cur)
        elif t == 'claim' and cur is not None:
            cur['claims'][b['id']] = b

    def near(q, label):
        q = q.strip()
        return any(q[i:i + 8] in label for i in range(0, max(1, len(q) - 7)))

    tally = collections.Counter()
    for d in decisions:
        w = where.get(d['id'])
        if d['value'] != 'yes' or not w or w['seq'] is None:
            continue
        mine = [b for b in wins if b['from_seq'] <= w['seq'] <= b['to_seq']]
        if not mine:
            continue
        b = mine[-1]
        a = answer_for(b)
        kind = f"{w['who']}/{w.get('tool') or '-'}"
        if a is None or a == 'ambiguous':
            tally[f'{kind}: answer {"not kept" if a is None else "ambiguous"}'] += 1
            continue
        label = d['quote'] + '\n' + d.get('prompt', '')
        about = [c for c in a.get('claims', []) if near(c.get('quote', ''), label)]
        dropped = dict((i, why) for i, why in b.get('dropped', []))
        lowered = collections.defaultdict(list)
        for i, why in b.get('lowered', []):
            lowered[i].append(why)
        if not about:
            tally[f'{kind}: not drafted'] += 1
            print(d['id'], kind, 'NOT DRAFTED', repr(d['quote'][:60]))
            continue
        for c in about:
            if c['id'] in dropped:
                out = f"dropped: {dropped[c['id']]}"
            else:
                k = b['claims'].get(c['id'], {})
                out = f"{k.get('status')} ({k.get('speaker')})" + (f" lowered: {'; '.join(lowered[c['id']])}" if lowered[c['id']] else '')
            tally[f"{kind}: drafted {c['status']} ({c['speaker']}) -> {out}"] += 1
            print(d['id'], kind, c['status'], c['speaker'], '->', out, '|', repr(c.get('quote', '')[:60]), '| label', repr(d['quote'][:40]))
    for k, v in tally.most_common():
        print(v, k)


if __name__ == '__main__':
    owner_only()
    cmd, args = sys.argv[1], sys.argv[2:]
    if cmd == 'fixtures':
        fixtures(args[0])
    elif cmd == 'replay':
        replay(args[0], args[1])
    elif cmd == 'stub':
        tokens = next((a.split('=')[1] for a in args if a.startswith('--tokens=')), None)
        stub(args[0], args[1], '--shrink' in args, tokens)
    elif cmd == 'map':
        map_labels(args[1], args[0])
    elif cmd == 'live':
        live(args[0], args[1], '--yes' in args,
             next((a.split('=')[1] for a in args if a.startswith('--tool=')), None))
    elif cmd == 'score':
        score(args[0], args[1])
    elif cmd == 'drafts':
        drafts(args[0], args[1])
    else:
        sys.exit(__doc__)
