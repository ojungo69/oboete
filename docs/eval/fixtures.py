"""Milestone 1, Task 7: synthetic failure fixtures (docs/spec.md 8.4 item 1): the 24-hour session, a
decision only in the middle, overturned and control pairs across sessions, deletion canaries.
`python3 fixtures.py` rewrites src/testdata/fixtures/ byte for byte (seeded). Synthetic on purpose:
owner transcripts never enter the repository; the real long session is in the replay set."""
import datetime, json, os, random

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.normpath(os.path.join(HERE, '..', '..', 'src', 'testdata', 'fixtures'))
ROOT = '__OBOETE_REPLAY_ROOT__'
T0 = datetime.datetime(2026, 9, 1, tzinfo=datetime.timezone.utc)
WORDS = ['cache', 'index', 'hook', 'worker', 'sync', 'viewer', 'search', 'chain', 'window', 'claim']
JA = ['テストを回します。', 'ログを確認しました。', '差分を読みます。', '型エラーを直しました。', '計測結果をまとめます。']


class Session:
    def __init__(self, sid, start):
        self.sid, self.t, self.lines = sid, start, []
        self.ev('SessionStart', 0, source='startup')

    def ev(self, event, dt, **payload):
        self.t += datetime.timedelta(seconds=dt)
        base = {'session_id': self.sid, 'transcript_path': f'{ROOT}/.oboete-replay/{self.sid}.jsonl',
                'cwd': ROOT, 'hook_event_name': event}
        self.lines.append({'agent': 'claude', 'event': event, 'session': self.sid,
                           'ts': self.t.strftime('%Y-%m-%dT%H:%M:%S.000Z'), 'payload': {**base, **payload}})

    def turn(self, rng, prompt, answer, dt=60, out_lines=(2, 10)):
        """One prompt, two tool calls, one answer: dt + 3 * (dt // 4) seconds."""
        self.ev('UserPromptSubmit', dt, prompt=prompt)
        for _ in range(2):
            w = rng.choice(WORDS)
            out = '\n'.join(f'src/{w}.rs:{rng.randint(1, 900)}: {rng.choice(WORDS)} {rng.choice(JA)}'
                            for _ in range(rng.randint(*out_lines)))
            self.ev('PostToolUse', dt // 4, tool_name='Bash', tool_input={'command': f'rg {w} src'},
                    tool_response={'stdout': out, 'stderr': '', 'interrupted': False})
        self.ev('Stop', dt // 4, last_assistant_message=answer)

    def filler(self, rng, n, dt=60, out_lines=(2, 10)):
        for i in range(n):
            w = rng.choice(WORDS)
            self.turn(rng, f'{w} の周りを確認して ({i})', f'{w} を確認しました。{rng.choice(JA)}', dt, out_lines)


def write(path, sessions):
    seq = 0
    with open(path, 'w', encoding='utf-8') as f:
        for s in sessions:
            s.ev('SessionEnd', 5, reason='other')
            for line in s.lines:
                seq += 1
                f.write(json.dumps({'seq': seq, **line}, ensure_ascii=False) + '\n')


def generate(out):
    os.makedirs(out, exist_ok=True)
    rng = random.Random('oboete-fixtures-2026-09-26')
    expected = {}

    # 300 turns of 288 s (165 + 3 * 41) span 24 hours; D1 at 40%, a compaction at 50%, D2 at 80%.
    s = Session('long-24h', T0)
    for i in range(300):
        if i == 120:
            s.turn(rng, '決めた: キャッシュは Redis ではなく SQLite に置く。依存を増やしたくないから。', 'キャッシュを SQLite に置く方針で進めます。', 165)
        elif i == 150:
            s.ev('PostCompact', 30, trigger='auto', compact_summary='これまで: キャッシュは SQLite に置くと決めた。検索と hook の確認を続けている。')
        elif i == 240:
            s.turn(rng, 'やっぱりキャッシュは持たない。SQLite のキャッシュ層は消して。', 'キャッシュ層を削除しました。', 165)
        else:
            s.filler(rng, 1, 165)
    write(f'{out}/long-24h.jsonl', [s])
    expected['long-24h'] = {'span_h': 24, 'decisions': [
        {'id': 'L1', 'session': 'long-24h', 'fragment': 'キャッシュは Redis ではなく SQLite に置く', 'status': 'overturned'},
        {'id': 'L2', 'session': 'long-24h', 'fragment': 'やっぱりキャッシュは持たない', 'status': 'current'}]}

    # One decision in the middle of 121 turns whose tool output fills more than 20,000 characters on each side.
    s = Session('middle-only', T0)
    s.filler(rng, 60, out_lines=(8, 14))
    s.turn(rng, '決定: 同期の間隔は 45 秒にする。30 秒だと hub への要求が多すぎる。', '同期の間隔を 45 秒にしました。')
    s.filler(rng, 60, out_lines=(8, 14))
    write(f'{out}/middle-only.jsonl', [s])
    expected['middle-only'] = {'decisions': [
        {'id': 'M1', 'session': 'middle-only', 'fragment': '同期の間隔は 45 秒にする', 'status': 'current'}]}

    # Across sessions: B overturns A; C is on the same subject and leaves A's successor in force;
    # D decides and overturns within one session.
    day = datetime.timedelta(days=1)
    a, b, c, d = (Session(n, T0 + k * day) for n, k in (('cross-a', 0), ('cross-b', 2), ('cross-c', 3), ('cross-d', 4)))
    a.filler(rng, 3); a.turn(rng, 'テストは cargo nextest で回すことにする。', 'nextest に切り替えました。'); a.filler(rng, 4)
    b.filler(rng, 2); b.turn(rng, 'nextest はやめて、テストは cargo test に戻す。CI での導入が重い。', 'cargo test に戻しました。'); b.filler(rng, 4)
    c.filler(rng, 4); c.turn(rng, 'CI のタイムアウトは 20 分にする。', 'タイムアウトを 20 分にしました。'); c.filler(rng, 3)
    d.filler(rng, 1); d.turn(rng, 'ログは JSON で出す。', 'ログを JSON にしました。'); d.filler(rng, 4)
    d.turn(rng, 'やっぱりログはテキストのまま。JSON はやめる。', 'ログをテキストに戻しました。'); d.filler(rng, 2)
    write(f'{out}/overturn-cross.jsonl', [a, b, c, d])
    expected['overturn-cross'] = {
        'decisions': [
            {'id': 'A1', 'session': 'cross-a', 'fragment': 'テストは cargo nextest で回す', 'status': 'overturned'},
            {'id': 'B1', 'session': 'cross-b', 'fragment': 'テストは cargo test に戻す', 'status': 'current'},
            {'id': 'C1', 'session': 'cross-c', 'fragment': 'CI のタイムアウトは 20 分にする', 'status': 'current'},
            {'id': 'D1', 'session': 'cross-d', 'fragment': 'ログは JSON で出す', 'status': 'overturned'},
            {'id': 'D2', 'session': 'cross-d', 'fragment': 'やっぱりログはテキストのまま', 'status': 'current'}],
        'pairs': [{'earlier': 'A1', 'later': 'B1', 'relation': 'overturns'},
                  {'earlier': 'B1', 'later': 'C1', 'relation': 'compatible'},
                  {'earlier': 'D1', 'later': 'D2', 'relation': 'overturns'}]}

    # One canary per place a forget must reach (spec 6.2, M4), and one inside <private> that must
    # never be stored at all (spec 2.2).
    k = {f: f'OBOETE-CANARY-{f}-{rng.getrandbits(32):08x}' for f in
         ('PROMPT', 'TOOLIN', 'TOOLOUT', 'ASSIST', 'COMPACT', 'ANSWER', 'SUBAGENT')}
    private = f'OBOETE-CANARY-PRIVATE-{rng.getrandbits(32):08x}'
    s = Session('canaries', T0)
    s.filler(rng, 2)
    s.ev('UserPromptSubmit', 60, prompt=f'この値を覚えておいて: {k["PROMPT"]}')
    s.ev('PostToolUse', 10, tool_name='Bash', tool_input={'command': f'echo {k["TOOLIN"]}'},
         tool_response={'stdout': f'value {k["TOOLOUT"]}', 'stderr': '', 'interrupted': False})
    s.ev('Stop', 10, last_assistant_message=f'覚えました: {k["ASSIST"]}')
    s.ev('PostToolUse', 10, tool_name='AskUserQuestion',
         tool_input={'questions': [{'question': 'どの値にしますか?'}], 'answers': {'どの値にしますか?': k['ANSWER']}},
         tool_response={'answered': True})   # the canary once: in the answers, where observe reads it
    s.ev('PostToolUse', 10, tool_name='Grep', agent_id='a1', tool_input={'pattern': 'value'},
         tool_response={'stdout': k['SUBAGENT']})
    s.ev('PostCompact', 30, trigger='auto', compact_summary=f'要約: 値 {k["COMPACT"]} を覚えた。')
    s.ev('UserPromptSubmit', 60, prompt=f'次の値は保存しないで <private>{private}</private> と言ったら無視して')
    s.filler(rng, 2)
    write(f'{out}/canaries.jsonl', [s])
    expected['canaries'] = {'canaries': list(k.values()), 'private': private}

    with open(f'{out}/expected.json', 'w', encoding='utf-8') as f:
        json.dump(expected, f, ensure_ascii=False, indent=1)
        f.write('\n')


if __name__ == '__main__':
    generate(OUT)
    print('wrote', OUT)
