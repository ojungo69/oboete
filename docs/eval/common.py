"""Shared by the milestone-1 evaluation scripts (docs/milestone-1-plan.md). Everything they read or
write stays on this machine, in owner-only files under ~/.oboete/eval (docs/pr-b.md decision 1)."""
import hashlib, json, os, platform, re, subprocess, tempfile

E = os.path.expanduser(os.environ.get('OBOETE_EVAL', '~/.oboete/eval'))
JA = re.compile(r'[぀-ヿ㐀-鿿]')
# Every draw of milestone 1 is ordered by h(f'<purpose>:{SEED}:<id>'); recorded in docs/milestone-1.md.
SEED = 'oboete-milestone-1-2026-09-26'
GRADERS = ('gpt-oss-120b', 'deepseek-v4-pro', 'glm-5.3')
PANEL = ('claude-sonnet-5', *GRADERS, 'kimi-k3', 'qwen3.8-max', 'grok-4.7', 'gpt-6-astra')


def h(s):
    return int(hashlib.sha256(s.encode()).hexdigest()[:8], 16)


def split(session):
    """The dev/test split of build_queries.py: by session hash, 70/30."""
    return 'test' if h('split:' + session) % 10 < 3 else 'dev'


def draw(purpose, item, exclude=None):
    candidates = [m for m in PANEL if m != exclude]
    return candidates[h(f'{purpose}:{SEED}:{item}') % len(candidates)]


def checked(purpose, item):
    return h(f'{purpose}:{SEED}:{item}') % 5 == 0


def guard(session_ids=(), decide=None, pool='dev'):
    """Check split metadata before a caller opens any held-out contents (A88)."""
    sides = {}
    manifest = f'{E}/replay/manifest.json'
    if os.path.exists(manifest):
        with open(manifest, encoding='utf-8') as f:
            sides = {r['session']: r['side'] for r in json.load(f)['sessions']}
    extra = f'{E}/labels/drafts/extra.json'
    if os.path.exists(extra):
        with open(extra, encoding='utf-8') as f:
            for row in json.load(f):
                sides.setdefault(row['session'], 'dev-extra')
    held_out = pool not in ('dev', 'dev-extra') or any(
        sides.get(s, split(s)) not in ('dev', 'dev-extra') for s in session_ids)
    if not held_out:
        return
    try:
        with open(f'{E}/deciding.json', encoding='utf-8') as f:
            recorded = json.load(f).get('curator')
    except (OSError, ValueError, AttributeError):
        recorded = None
    if not isinstance(recorded, str) or not recorded or decide != recorded:
        raise SystemExit('Held-out input requires the recorded curator deciding id')


def owner_only(path=None):
    os.umask(0o077)
    os.makedirs(E, mode=0o700, exist_ok=True)
    os.chmod(E, 0o700)
    if path is not None:
        parent = os.path.dirname(path) or '.'
        os.makedirs(parent, mode=0o700, exist_ok=True)
        os.chmod(parent, 0o700)
        if os.path.exists(path):
            os.chmod(path, 0o600)


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
    owner_only(path)
    with open(path, 'w', encoding='utf-8') as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + '\n')


def keep_json(path, data):
    owner_only(path)
    with open(path, 'w', encoding='utf-8') as f:
        json.dump(data, f, ensure_ascii=False, indent=1)


def record(binary, home, n, vector, models):
    return {'N': n, 'machine': platform.node(), 'sha256': sha256_file(binary),
            'home': os.fspath(home), 'vector': vector, 'models': models}


def clean_env():
    """The environment without secret-bearing variables: keys never reach a subprocess environment
    (project CLAUDE.md). Every subprocess these scripts start gets this."""
    return {k: v for k, v in os.environ.items()
            if k != 'CLAUDECODE' and not any(s in k.upper() for s in ('TOKEN', 'KEY', 'SECRET', 'PASSWORD'))}


def command(argv, **kwargs):
    """Capture a command's text without exposing private output in an exception."""
    kwargs.update(capture_output=True, text=True, env=clean_env(), check=False)
    try:
        run = subprocess.run(argv, **kwargs)
    except (OSError, subprocess.SubprocessError):
        raise RuntimeError('Evaluation command failed') from None
    if run.returncode:
        raise RuntimeError('Evaluation command failed')
    return run.stdout


def gate(text, oboete='oboete'):
    """The outbound gate of the shipped binary: <private> blocks removed, secrets redacted."""
    return command([oboete, 'gate'], input=text)


def claude_json(prompt, model, timeout=300):
    """One `claude -p` call with the judge's isolation (docs/eval/judge.py `ask`); returns the result
    text. judge.py keeps its own copy: its recorded grades were made through it."""
    env = clean_env()
    env['OBOETE_SKIP'] = '1'
    try:
        with tempfile.TemporaryDirectory() as cwd:
            r = subprocess.run(
                ['claude', '-p', '--model', model, '--setting-sources', '', '--tools', '', '--strict-mcp-config',
                 '--no-session-persistence', '--settings', '{"disableAllHooks":true}', '--output-format', 'json'],
                input=prompt, capture_output=True, text=True, cwd=cwd, env=env, timeout=timeout)
        if r.returncode:
            raise ValueError()
        answer = json.loads(r.stdout)
        used = sorted((answer.get('modelUsage') or {}).keys())
        if used != [model] or answer.get('is_error') or not isinstance(answer.get('result'), str):
            raise ValueError()
        return answer['result']
    except (OSError, subprocess.SubprocessError, ValueError, TypeError, AttributeError):
        raise RuntimeError('Claude answer failed') from None


def parse_json(text):
    """One JSON object, optionally fenced or preceded by a reasoning block; no prose."""
    if not isinstance(text, str):
        raise ValueError('Invalid model answer')
    text = re.sub(r'^(?:\s*<think>.*?</think>\s*)+', '', text, flags=re.S).strip()
    trailing = r'(?:\s*<think>.*?</think>\s*)*'
    fenced = re.fullmatch(r'```(?:json)?\s*\n(.*?)\n```(' + trailing + ')', text, flags=re.S)
    if fenced:
        text = re.sub(r'^(?:\s*<think>.*?</think>\s*)+', '', fenced[1] + fenced[2], flags=re.S).strip()
    try:
        answer, end = json.JSONDecoder().raw_decode(text)
    except ValueError:
        raise ValueError('Invalid model answer') from None
    if not isinstance(answer, dict) or not re.fullmatch(trailing, text[end:], flags=re.S):
        raise ValueError('Invalid model answer')
    return answer


class FailedCall(RuntimeError):
    """A provider or answer failed; its private text never becomes an error message."""


class Calls:
    """The shared successful model calls; failures are recorded but never reused."""

    def __init__(self):
        self.path = f'{E}/m6/calls.jsonl'
        owner_only(self.path)
        self.done = {(r['requested'], r['prompt_sha256']): r for r in
                     (read_jsonl(self.path) if os.path.exists(self.path) else []) if not r.get('failed')}
        self.used, self.failed = {}, set()

    def call(self, model, prompt, validate, answerer=False):
        from calib import chat
        digest = hashlib.sha256(prompt.encode()).hexdigest()
        key = model, digest
        if key in self.failed:
            raise FailedCall('Model call failed')
        row = self.done.get(key)
        if row is not None:
            try:
                if validate(row['answer']) is False:
                    raise ValueError()
            except Exception:
                raise FailedCall('Model call failed') from None
        else:
            row = {'requested': model, 'model': None, 'prompt_sha256': digest, 'failed': True}
            for attempt in range(2):
                try:
                    text, reported = (claude_json(prompt, model), model) if answerer else chat(model, prompt)
                    if not isinstance(reported, str) or not reported:
                        break
                except Exception:
                    break                         # calib.chat already retried the transport
                row['model'] = reported
                self.used.setdefault(model, set()).add(reported)
                try:
                    answer = parse_json(text)
                    if validate(answer) is False:
                        raise ValueError()
                except Exception:
                    continue
                row.update(model=reported, answer=answer, failed=False)
                break
            with open(self.path, 'a', encoding='utf-8') as f:
                f.write(json.dumps(row, ensure_ascii=False) + '\n')
            if row['failed']:
                self.failed.add(key)
                raise FailedCall('Model call failed') from None
            self.done[key] = row
        self.used.setdefault(model, set()).add(row['model'])
        return row['answer']

    def models(self):
        return {m: sorted(names) for m, names in self.used.items()}

    def votes(self, prompt, fields):
        fields = tuple(fields)
        out = {}
        for model in GRADERS:
            try:
                out[model] = self.call(model, prompt,
                                       lambda answer: all(type(answer.get(f)) is bool for f in fields))
            except FailedCall:
                out[model] = None
        return out


def voted(votes, field):
    values = [(votes.get(m) or {}).get(field) for m in GRADERS]
    return sum(values) >= 2 if all(type(v) is bool for v in values) else None


def agreement(rows, field):
    out = {}
    for model in GRADERS:
        pairs = []
        for votes in rows:
            if voted(votes, field) is None:
                continue
            others = [votes[m][field] for m in GRADERS if m != model]
            if others[0] == others[1]:
                pairs.append(votes[model][field] == others[0])
        out[model] = {'n': len(pairs), 'agreement': sum(pairs) / len(pairs) if pairs else None}
    return out


class Mcp:
    """One newline JSON-RPC session with `oboete mcp`; tool results stay as dictionaries."""

    def __init__(self, binary, home, cwd):
        self.next_id, self.pending = 0, {}
        self.process = subprocess.Popen([binary, '--home', home, 'mcp'], cwd=cwd, env=clean_env(),
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.DEVNULL, text=True, bufsize=1)
        try:
            self._request('initialize', {'protocolVersion': '2024-11-05', 'capabilities': {},
                                        'clientInfo': {'name': 'oboete-eval', 'version': '1'}})
            self._send({'method': 'notifications/initialized'})
        except Exception:
            self.close()
            raise

    def _send(self, message):
        try:
            self.process.stdin.write(json.dumps(dict(jsonrpc='2.0', **message)) + '\n')
            self.process.stdin.flush()
        except (OSError, ValueError):
            raise ConnectionError('MCP connection closed') from None

    def _request(self, method, params):
        self.next_id += 1
        wanted = self.next_id
        self._send({'id': wanted, 'method': method, 'params': params})
        while wanted not in self.pending:
            line = self.process.stdout.readline()
            if not line:
                raise ConnectionError('MCP connection closed')
            try:
                reply = json.loads(line)
            except ValueError:
                raise ConnectionError('Invalid MCP reply') from None
            if not isinstance(reply, dict) or reply.get('jsonrpc') != '2.0':
                raise ConnectionError('Invalid MCP reply')
            if 'id' in reply:
                self.pending[reply['id']] = reply
        reply = self.pending.pop(wanted)
        result = reply.get('result')
        if 'error' in reply or not isinstance(result, dict) or result.get('isError'):
            raise ConnectionError('MCP request failed')
        return result

    def call(self, tool, args):
        return self._request('tools/call', {'name': tool, 'arguments': args})

    def close(self):
        try:
            self.process.stdin.close()
        except OSError:
            pass
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.process.stdout.close()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()
