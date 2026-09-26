"""Milestone 1, Task 10: M22's corpus, counted (docs/spec.md 8.2 M22). Read-only: the stores are
opened with mode=ro, the Windows claude-mem database is copied first and the copy removed, the
iMac is read with `sqlite3 -readonly` over SSH. Writes ~/.oboete/eval/corpus-count.json."""
import glob, json, os, shutil, sqlite3, subprocess, tempfile, time

from common import E, clean_env, owner_only

OBOETE = {t: f'SELECT count(*) FROM {t}' for t in ('observations', 'summaries', 'prompts', 'events')}
CLAUDE_MEM = {'observations': 'SELECT count(*) FROM observations',
              'summaries': 'SELECT count(*) FROM session_summaries',
              'prompts': 'SELECT count(*) FROM user_prompts'}
IMAC = 'asuka@100.79.238.11'


def counts(db, queries):
    return {k: db.execute(q).fetchone()[0] for k, q in queries.items()}


def readonly(path):
    return sqlite3.connect(f'file:{path}?mode=ro', uri=True)


def live_store():
    db = readonly(os.path.expanduser('~/.oboete/oboete.db'))
    out = counts(db, OBOETE)
    week_ago = int((time.time() - 7 * 86400) * 1000)
    out['events_last_7_days'] = db.execute('SELECT count(*) FROM events WHERE ts >= ?', (week_ago,)).fetchone()[0]
    return out


def windows_claude_mem():
    """claude-mem on Windows owns this database. From WSL neither a SQLite backup nor a read-only open
    is safe: a WAL database's shared-memory index is not shared across the WSL/Windows boundary. So
    the database and its WAL are copied, and the copy counts only if the source did not change while
    it was copied and the copy passes quick_check; otherwise it tries again."""
    src = '/mnt/c/Users/jura/.claude-mem/claude-mem.db'
    if not os.path.exists(src):
        return None

    def state():
        return tuple((os.stat(src + x).st_size, os.stat(src + x).st_mtime_ns) if os.path.exists(src + x) else None
                     for x in ('', '-wal'))

    for _ in range(5):
        with tempfile.TemporaryDirectory(dir=E) as d:
            before = state()
            for suffix in ('', '-wal'):
                if os.path.exists(src + suffix):
                    shutil.copyfile(src + suffix, f'{d}/copy.db{suffix}')
            if state() == before:
                db = sqlite3.connect(f'{d}/copy.db')
                if db.execute('PRAGMA quick_check').fetchone()[0] == 'ok':
                    out = counts(db, CLAUDE_MEM)
                    db.close()
                    return out
                db.close()
        time.sleep(2)
    return {'error': 'the database kept changing while it was copied (5 tries)'}


def imac_claude_mem():
    script = ('f="$HOME/.claude-mem/claude-mem.db"; test -f "$f" || { echo none; exit 0; }; '
              'for t in observations session_summaries user_prompts; do sqlite3 -readonly "$f" "SELECT count(*) FROM $t" || exit 1; done')
    r = subprocess.run(['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10', IMAC, script],
                       capture_output=True, text=True, timeout=120, env=clean_env())
    if r.returncode != 0:
        return {'error': r.stderr.strip()[-200:]}
    words = r.stdout.split()
    if words == ['none']:
        return None
    if len(words) != 3 or not all(w.isdigit() for w in words):
        return {'error': f'expected three counts, got {r.stdout.strip()[:80]!r}'}
    return dict(zip(('observations', 'summaries', 'prompts'), map(int, words)))


def raw_rate(days=90):
    """Hook events and bytes the agents' transcripts imply over the last `days`, scaled to a year:
    design B keeps every event and full tool outputs (spec 2.4). The bytes are replay-fixture JSON,
    before per-record compression: an upper bound on raw.db's size, not a disk estimate."""
    since = time.time() - days * 86400
    files = [(agent, p) for agent, pattern in (('claude', '~/.claude/projects/*/*.jsonl'),
                                                ('codex', '~/.codex/sessions/**/*.jsonl'))
             for p in glob.glob(os.path.expanduser(pattern), recursive=True) if os.path.getmtime(p) >= since]
    # A file touched in the window can hold older events (a resumed session): count by the event's
    # own time. ISO timestamps compare as strings.
    since_iso = time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(since))
    events = size = 0
    failed = []
    for agent, path in files:
        n = b = 0
        with subprocess.Popen(['oboete', 'transcript', path, '--agent', agent], stdout=subprocess.PIPE,
                              stderr=subprocess.DEVNULL, env=clean_env()) as p:
            for line in p.stdout:
                if json.loads(line).get('ts', '') >= since_iso:
                    n += 1
                    b += len(line)
        if p.returncode:       # a failed conversion is counted as failed, never as a smaller file
            failed.append(path)
            continue
        events, size = events + n, size + b
    return {'days': days, 'files': len(files), 'failed_files': len(failed), 'events': events, 'bytes': size,
            'events_per_year': round(events / days * 365), 'mb_per_year': round(size / days * 365 / 1e6)}


if __name__ == '__main__':
    owner_only()
    result = {'eval_store': counts(readonly(f'{E}/home/oboete.db'), OBOETE),
              'live_store': live_store(),
              'windows_claude_mem': windows_claude_mem(),
              'imac_claude_mem': imac_claude_mem(),
              'raw_rate': raw_rate()}
    with open(f'{E}/corpus-count.json', 'w') as f:
        json.dump(result, f, indent=1)
    print(json.dumps(result, indent=1))
