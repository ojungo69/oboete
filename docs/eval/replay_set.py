"""Milestone 1, Task 3: the replay set (docs/spec.md 8.4 item 1; rules in docs/milestone-1.md).
Reads the agents' transcripts and the frozen claude-mem copy read-only; copies the chosen
transcripts into ~/.oboete/eval/replay with a manifest of sha256s."""
import datetime, glob, json, os, re, shutil, sqlite3, sys

from common import E, JA, SEED, h, owner_only, sha256_file, split

PER_SIDE = {'claude': 24, 'codex': 6}
SHORT, LONG = 50, 300          # tool calls: short < 50 <= mid < 300 <= long
LONG_SPAN_H = 20
UUID = re.compile(r'([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.jsonl$')
# Codex puts harness context in user messages; these are not typed prompts.
CODEX_CONTEXT = ('<environment_context>', '<user_instructions>', '# AGENTS.md', '<permissions', '<INSTRUCTIONS>')
# Claude Code "user" records the developer did not type: the hook's ENVELOPES (src/hook.rs), teammate
# messages, and transcript-only records (command output, slash-command tags, bash mode, interrupts).
# 1,063 task notifications in the first draw's Claude files made nearly every session look English.
NOT_TYPED = ('<task-notification', '<agent-message', '<system_notification', '<bash-notification', '<<autonomous-loop',
             'Another Claude session sent a message', '<local-command-', '<command-name>', '<command-message>',
             '<bash-input', '<bash-stdout', '<bash-stderr', '[Request interrupted')


def ts(s):
    return datetime.datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp()


def typed(agent, o):
    """The text the developer typed in this record, or None."""
    if agent == 'claude':
        m = o.get('message') or {}
        # isSidechain: a subagent's turn, written inline by older Claude Code; not typed.
        if (o.get('type') != 'user' or o.get('isMeta') or o.get('isCompactSummary') or o.get('isSidechain')
                or m.get('role') != 'user'):
            return None
        c = m.get('content')
        if isinstance(c, str):
            text = c
        else:
            text = '\n'.join(x.get('text', '') for x in c or [] if isinstance(x, dict) and x.get('type') == 'text')
        return None if not text or text.lstrip().startswith(NOT_TYPED) else text
    # Codex: the response_item user message. Its rare event_msg `user_message` twin (3 in 40 rollouts
    # of 2026-08) repeats the same text, so it is not counted.
    p = o.get('payload') if isinstance(o.get('payload'), dict) else {}
    if o.get('type') == 'response_item' and p.get('type') == 'message' and p.get('role') == 'user':
        text = '\n'.join(x.get('text', '') for x in p.get('content') or [] if isinstance(x, dict))
        return None if text.lstrip().startswith(CODEX_CONTEXT) else text
    return None


def tool_calls(agent, o):
    if agent == 'claude':
        c = (o.get('message') or {}).get('content') if o.get('type') == 'assistant' else None
        return sum(1 for x in c if isinstance(x, dict) and x.get('type') == 'tool_use') if isinstance(c, list) else 0
    p = o.get('payload') if isinstance(o.get('payload'), dict) else {}
    return int(o.get('type') == 'response_item' and p.get('type') in ('function_call', 'custom_tool_call'))


def session_files(agent, path):
    """The main transcript and, for Claude Code, its subagent files: the replay includes their tool
    calls, so the length strata count them too."""
    sub = os.path.join(os.path.dirname(path), os.path.basename(path)[:-6], 'subagents')
    return [path] + (sorted(glob.glob(os.path.join(sub, '*.jsonl'))) if agent == 'claude' else [])


def lines_of(files):
    for p in files:
        with open(p, encoding='utf-8', errors='replace') as f:
            yield from f


def features(agent, path):
    """Stratum features of one session, streamed line by line; None without a typed prompt.
    Length is tool calls, not prompts: one typed prompt can start hours of work."""
    prompts = chars = ja = tools = 0
    first = last = None
    for line in lines_of(session_files(agent, path)):
        try:
            o = json.loads(line)
        except ValueError:
            continue
        if isinstance(o.get('timestamp'), str):
            t = ts(o['timestamp'])
            first = t if first is None else min(first, t)
            last = t if last is None else max(last, t)
        tools += tool_calls(agent, o)
        text = typed(agent, o)
        if text and text.strip():
            prompts += 1
            chars += len(text)
            ja += len(JA.findall(text))
    if prompts < 1:
        return None
    ja_ratio = ja / chars if chars else 0.0
    length = 'short' if tools < SHORT else 'mid' if tools < LONG else 'long'
    return {'prompts': prompts, 'tools': tools, 'chars': chars, 'ja_ratio': round(ja_ratio, 3),
            'span_h': round(((last or 0) - (first or 0)) / 3600, 2),
            'stratum': (agent, length, 'ja' if ja_ratio >= 0.3 else 'en')}


def choose(pool, quotas, seed):
    """quotas: {agent: n}. One per non-empty stratum first, then the rest of the quota in proportion
    to what each stratum has left (largest remainder), in seeded hash order inside a stratum.
    Never more than n, never more than a stratum holds (issue #65)."""
    chosen = []
    for agent, n in quotas.items():
        strata = {}
        for p in pool:
            if p['agent'] == agent:
                strata.setdefault(p['stratum'], []).append(p)
        keys = sorted(strata)
        first = set(sorted(keys, key=lambda k: h(f'stratum:{seed}:{k}'))[:n])
        take = {k: int(k in first) for k in keys}
        left = n - sum(take.values())
        room = {k: len(strata[k]) - take[k] for k in keys}
        if left > 0 and sum(room.values()) > 0:
            share = {k: left * room[k] / sum(room.values()) for k in keys}
            add = {k: min(room[k], int(share[k])) for k in keys}
            for k in sorted(keys, key=lambda k: (add[k] - share[k], k)):
                if sum(add.values()) >= left:
                    break
                if add[k] < room[k]:
                    add[k] += 1
            take = {k: take[k] + add[k] for k in keys}
        for k in keys:
            ranked = sorted(strata[k], key=lambda p: h(f'replay:{seed}:{p["session"]}'))
            chosen += ranked[:take[k]]
    return chosen


def copy_out(chosen, dest_root):
    for c in chosen:
        base = os.path.join(c['side'], c['agent'])
        main, *subs = session_files(c['agent'], c['path'])
        files = {os.path.join(base, f'{c["session"]}.jsonl'): main}
        for f in subs:
            files[os.path.join(base, c['session'], 'subagents', os.path.basename(f))] = f
        c['files'] = {}
        for rel, src in files.items():
            dst = os.path.join(dest_root, rel)
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            shutil.copyfile(src, dst)
            os.chmod(dst, 0o600)
            c['files'][rel] = sha256_file(dst)


def inventory():
    cm = sqlite3.connect(f'file:{E}/claude-mem-2026-09-24.db?mode=ro', uri=True)
    recorded = {(p, s) for p, s in cm.execute(
        "SELECT s.platform_source, s.content_session_id FROM sdk_sessions s "
        "WHERE EXISTS (SELECT 1 FROM observations o WHERE o.memory_session_id = s.memory_session_id)")}
    found = []
    for path in glob.glob(os.path.expanduser('~/.claude/projects/*/*.jsonl')):
        found.append(('claude', os.path.basename(path)[:-6], path))
    for path in glob.glob(os.path.expanduser('~/.codex/sessions/**/*.jsonl'), recursive=True):
        m = UUID.search(path)
        if m:
            found.append(('codex', m.group(1), path))
    pool, longest = [], None
    for agent, session, path in found:
        f = features(agent, path)
        if f is None:
            continue
        row = {'session': session, 'agent': agent, 'path': path, 'side': 'held-out' if split(session) == 'test' else 'dev', **f}
        if longest is None or row['span_h'] > longest['span_h']:
            longest = row
        if (agent, session) in recorded:
            pool.append(row)
    return pool, longest


if __name__ == '__main__':
    owner_only()
    from freeze import check
    bad = check()
    if bad:
        sys.exit('frozen inputs changed: ' + ', '.join(bad))
    root = f'{E}/replay'
    if os.path.exists(f'{root}/manifest.json'):
        sys.exit(f'{root}/manifest.json exists and is frozen')
    pool, longest = inventory()
    chosen = []
    for side in ('held-out', 'dev'):
        chosen += choose([p for p in pool if p['side'] == side], PER_SIDE, f'{SEED}:{side}')
    for c in chosen:
        c['long_span'] = False
    if longest and longest['span_h'] >= LONG_SPAN_H and longest['session'] not in {c['session'] for c in chosen}:
        chosen.append({**longest, 'long_span': True})
    copy_out(chosen, root)
    shutil.copyfile(os.path.expanduser('~/projects/free-mem/test/fixtures/events-1000.jsonl'), f'{root}/events-1000.jsonl')
    os.chmod(f'{root}/events-1000.jsonl', 0o600)
    manifest = {'seed': SEED, 'rules': 'docs/milestone-1.md, Task 3',
                'sessions': [{k: (list(v) if k == 'stratum' else v) for k, v in c.items() if k != 'path'} for c in chosen]}
    with open(f'{root}/manifest.json', 'w') as f:
        json.dump(manifest, f, indent=1, ensure_ascii=False)
    by = {}
    for c in chosen:
        by[(c['side'], c['agent'])] = by.get((c['side'], c['agent']), 0) + 1
    print(by, 'long span:', longest and longest['span_h'])
