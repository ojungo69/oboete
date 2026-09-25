import json, os, sys, tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from replay_set import choose, copy_out, features


def claude_file(d, sid, prompts, ja=False, hours=1, tools=0):
    path = os.path.join(d, f'{sid}.jsonl')
    with open(path, 'w') as f:
        for i in range(prompts):
            ts = f'2026-09-0{1 + (i * hours) // 24}T{(i * hours) % 24:02d}:00:00Z'
            text = 'キャッシュの方針を決めたい' if ja else 'decide the cache policy'
            f.write(json.dumps({'type': 'user', 'sessionId': sid, 'timestamp': ts,
                                'message': {'role': 'user', 'content': text}}) + '\n')
            f.write(json.dumps({'type': 'user', 'sessionId': sid, 'timestamp': ts,   # a tool result: not typed
                                'message': {'role': 'user', 'content': [{'type': 'tool_result', 'tool_use_id': 't', 'content': 'x'}]}}) + '\n')
            f.write(json.dumps({'type': 'assistant', 'sessionId': sid, 'timestamp': ts, 'message': {'role': 'assistant',
                                'content': [{'type': 'tool_use', 'id': f't{i}{k}', 'name': 'Bash', 'input': {}} for k in range(tools)]}}) + '\n')
            f.write(json.dumps({'type': 'user', 'sessionId': sid, 'timestamp': ts,                # harness traffic: not typed
                                'message': {'role': 'user', 'content': '<task-notification>\n<summary>done</summary>\n</task-notification>'}}) + '\n')
            f.write('{"type":"atis-latch"}\n')                                        # unknown type: ignored
            f.write(json.dumps({'type': 'user', 'isSidechain': True, 'sessionId': sid, 'timestamp': ts,  # a subagent's task
                                'message': {'role': 'user', 'content': 'inline subagent task'}}) + '\n')
    return path


def test_features_count_typed_prompts_language_and_span():
    with tempfile.TemporaryDirectory() as d:
        f = features('claude', claude_file(d, 'a', 12, ja=True, hours=2, tools=5))
        assert f['prompts'] == 12 and f['tools'] == 60 and f['ja_ratio'] > 0.9 and f['stratum'] == ('claude', 'mid', 'ja')
        assert 21.9 < f['span_h'] < 22.1
        assert features('claude', claude_file(d, 'b', 0)) is None   # no typed prompt


def test_choose_is_deterministic_and_covers_every_stratum():
    pool = [{'session': f's{i}', 'agent': 'claude', 'stratum': ('claude', ['short', 'mid', 'long'][i % 3], 'ja')}
            for i in range(30)]
    a = choose(pool, {'claude': 6}, 'seed-1')
    assert a == choose(pool, {'claude': 6}, 'seed-1')
    assert len(a) == 6 and {p['stratum'][1] for p in a} == {'short', 'mid', 'long'}
    assert a != choose(pool, {'claude': 6}, 'seed-2')


def test_choose_never_exceeds_the_quota():
    sizes = [100, 1, 1, 1, 1, 1]
    pool = [{'session': f's{k}-{i}', 'agent': 'claude', 'stratum': ('claude', f'k{k}', 'ja')}
            for k, n in enumerate(sizes) for i in range(n)]
    got = choose(pool, {'claude': 24}, 'seed-1')
    assert len(got) == 24 and len({p['stratum'] for p in got}) == 6
    assert len(choose(pool[:3], {'claude': 24}, 'seed-1')) == 3            # never more than the pool


def test_copy_out_is_owner_only_and_hashed():
    with tempfile.TemporaryDirectory() as src, tempfile.TemporaryDirectory() as dst:
        path = claude_file(src, 'c', 3)
        os.makedirs(os.path.join(src, 'c', 'subagents'))
        with open(os.path.join(src, 'c', 'subagents', 'agent-1.jsonl'), 'w') as f:
            f.write('{}\n')
        chosen = [{'session': 'c', 'agent': 'claude', 'side': 'dev', 'path': path}]
        copy_out(chosen, dst)
        rel = 'dev/claude/c.jsonl'
        assert set(chosen[0]['files']) == {rel, 'dev/claude/c/subagents/agent-1.jsonl'}
        assert os.stat(os.path.join(dst, rel)).st_mode & 0o777 == 0o600
