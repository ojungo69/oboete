"""Shared by the milestone-1 evaluation scripts (docs/milestone-1-plan.md). Everything they read or
write stays on this machine, in owner-only files under ~/.oboete/eval (docs/pr-b.md decision 1)."""
import hashlib, json, os, re, subprocess, tempfile

E = os.path.expanduser(os.environ.get('OBOETE_EVAL', '~/.oboete/eval'))
JA = re.compile(r'[぀-ヿ㐀-鿿]')
# Every draw of milestone 1 is ordered by h(f'<purpose>:{SEED}:<id>'); recorded in docs/milestone-1.md.
SEED = 'oboete-milestone-1-2026-09-26'


def h(s):
    return int(hashlib.sha256(s.encode()).hexdigest()[:8], 16)


def split(session):
    """The dev/test split of build_queries.py: by session hash, 70/30."""
    return 'test' if h('split:' + session) % 10 < 3 else 'dev'


def owner_only():
    os.umask(0o077)
    os.makedirs(E, mode=0o700, exist_ok=True)
    os.chmod(E, 0o700)


def sha256_file(path):
    d = hashlib.sha256()
    with open(path, 'rb') as f:
        for block in iter(lambda: f.read(1 << 20), b''):
            d.update(block)
    return d.hexdigest()


def read_jsonl(path):
    with open(path, encoding='utf-8') as f:
        return [json.loads(line) for line in f if line.strip()]


def write_jsonl(path, rows):
    os.makedirs(os.path.dirname(path) or '.', exist_ok=True)
    with open(path, 'w', encoding='utf-8') as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + '\n')


def clean_env():
    """The environment without secret-bearing variables: keys never reach a subprocess environment
    (project CLAUDE.md). Every subprocess these scripts start gets this."""
    return {k: v for k, v in os.environ.items()
            if k != 'CLAUDECODE' and not any(s in k.upper() for s in ('TOKEN', 'KEY', 'SECRET', 'PASSWORD'))}


def gate(text, oboete='oboete'):
    """The outbound gate of the shipped binary: <private> blocks removed, secrets redacted."""
    return subprocess.run([oboete, 'gate'], input=text, capture_output=True, text=True, check=True,
                          env=clean_env()).stdout


def claude_json(prompt, model, timeout=300):
    """One `claude -p` call with the judge's isolation (docs/eval/judge.py `ask`); returns the result
    text. judge.py keeps its own copy: its recorded grades were made through it."""
    env = clean_env()
    env['OBOETE_SKIP'] = '1'
    with tempfile.TemporaryDirectory() as cwd:
        r = subprocess.run(
            ['claude', '-p', '--model', model, '--setting-sources', '', '--tools', '', '--strict-mcp-config',
             '--no-session-persistence', '--settings', '{"disableAllHooks":true}', '--output-format', 'json'],
            input=prompt, capture_output=True, text=True, cwd=cwd, env=env, timeout=timeout)
    if r.returncode != 0:
        raise RuntimeError(r.stderr[-300:] or r.stdout[-300:])
    answer = json.loads(r.stdout)
    used = sorted((answer.get('modelUsage') or {}).keys())
    if used != [model]:
        raise RuntimeError(f'answered by {used}, not {model}')
    return answer['result']
