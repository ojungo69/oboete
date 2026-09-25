"""Milestone 1, Task 9: dev baselines on the frozen replay set (docs/spec.md 8.1 "Baselines").
Held-out transcripts are never touched here: they are replayed once, at milestone 4.

  baseline.py config <home>   write the API-only provider config into <home>/config.toml
  baseline.py oboete          each dev transcript -> fixture -> replay through today's oboete
  baseline.py claude-mem      claude-mem's own observations and summaries of the dev sessions
  baseline.py summary         per-session counts for docs/milestone-1.md"""
import glob, json, os, sqlite3, subprocess, sys

from common import E, clean_env, owner_only, read_jsonl, write_jsonl

B = f'{E}/baseline'
HOME = os.path.expanduser('~')
REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
# Today's default chain (src/config.rs default_providers) without the subscription CLIs, so a
# baseline spends no Claude or Codex allowance; 100 calls a day per provider leaves most of each
# free tier to the owner's own oboete.
PROVIDERS = [
    ('groq', 'https://api.groq.com/openai/v1', 'GROQ_API_KEY.md', 'openai/gpt-oss-120b', ''),
    ('groq-20b', 'https://api.groq.com/openai/v1', 'GROQ_API_KEY.md', 'openai/gpt-oss-20b', ''),
    ('nim', 'https://integrate.api.nvidia.com/v1', 'NVIDIA_NIM_KEY.md', 'nvidia/nemotron-3-super-120b-a12b',
     '[providers.extra]\nmax_tokens = 2000\n'),
    ('opencode-go', 'https://opencode.ai/zen/go/v1', 'OPENCODE_API_KEY.md', 'glm-5.3-flash',
     'headers = { "x-opencode-session" = "oboete" }\n'),
    ('openrouter', 'https://openrouter.ai/api/v1', 'OPENROUTER_API_KEY.md', 'nvidia/nemotron-3-super-120b-a12b:free',
     'retry_429 = false\n[providers.extra]\nmodels = ["qwen/qwen3.8-27b:free"]\nprovider = { require_parameters = true }\n'),
    ('mistral', 'https://api.mistral.ai/v1', 'MISTRAL_API_KEY.md', 'mistral-small-latest', ''),
]


def config(home):
    os.makedirs(home, exist_ok=True)
    blocks = [f'[[providers]]\nkind = "openai"\nname = "{n}"\nbase_url = "{u}"\nkey_file = "{HOME}/{k}"\n'
              f'model = "{m}"\ndaily_budget = 100\n{extra}' for n, u, k, m, extra in PROVIDERS]
    with open(f'{home}/config.toml', 'w') as f:
        f.write('\n'.join(blocks))


def dev_sessions():
    with open(f'{E}/replay/manifest.json') as f:
        return [s for s in json.load(f)['sessions'] if s['side'] == 'dev']


def version():
    head = subprocess.run(['git', 'rev-parse', '--short', 'HEAD'], capture_output=True, text=True, cwd=REPO,
                          env=clean_env()).stdout.strip()
    binary = subprocess.run(['oboete', '--version'], capture_output=True, text=True, env=clean_env()).stdout.strip()
    return head, binary


def run_oboete():
    """One replay per session into one home: `oboete replay` reads its fixture whole, so a single
    concatenated fixture would hold every dev transcript in memory at once. A session whose report
    exists was replayed before and is skipped: replaying it again would insert its events twice."""
    head, binary = version()
    home = f'{B}/oboete-{head}'
    if not os.path.exists(f'{home}/config.toml'):
        config(home)
        with open(f'{home}/version.txt', 'w') as f:
            f.write(f'{binary} installed from {head}\n')
    os.makedirs(f'{B}/fixtures', exist_ok=True)
    done = 0
    for s in dev_sessions():
        report = f'{home}/replay-{s["session"]}.json'
        if os.path.exists(report):
            continue
        fixture = f'{B}/fixtures/{s["session"]}.jsonl'
        with open(fixture, 'w', encoding='utf-8') as out:
            subprocess.run(['oboete', 'transcript', f'{E}/replay/dev/{s["agent"]}/{s["session"]}.jsonl',
                            '--agent', s['agent']], stdout=out, check=True, env=clean_env())
        with open(report + '.part', 'w') as r:
            if subprocess.run(['oboete', 'replay', fixture, '--home', home, '--agent', 'all', '--spawn-sample', '0'],
                              stdout=r, env=clean_env()).returncode != 0:
                sys.exit(f'replay of {s["session"]} failed part way; its events may be in {home} already. '
                         f'Remove {home} and run this again.')
        os.replace(report + '.part', report)
        done += 1
    print(f'{done} sessions replayed into {home}')


def claude_mem():
    cm = sqlite3.connect(f'file:{E}/claude-mem-2026-09-24.db?mode=ro', uri=True)
    join = ('JOIN sdk_sessions x ON x.memory_session_id = t.memory_session_id '
            'WHERE x.content_session_id = ? AND x.platform_source = ? ORDER BY t.created_at_epoch')
    rows = []
    for s in dev_sessions():
        key = (s['session'], s['agent'])
        obs = cm.execute(f'SELECT t.type, t.title, t.narrative, t.facts, t.created_at FROM observations t {join}', key)
        sums = cm.execute(f'SELECT t.request, t.investigated, t.learned, t.completed, t.next_steps, t.created_at '
                          f'FROM session_summaries t {join}', key)
        rows.append({'session': s['session'], 'agent': s['agent'],
                     'observations': [dict(zip(('type', 'title', 'narrative', 'facts', 'created_at'), r)) for r in obs],
                     'summaries': [dict(zip(('request', 'investigated', 'learned', 'completed', 'next_steps', 'created_at'), r))
                                   for r in sums]})
    write_jsonl(f'{B}/claude-mem-dev.jsonl', rows)
    print(f'{len(rows)} sessions, {sum(len(r["observations"]) for r in rows)} observations')


def summary():
    home = sorted(glob.glob(f'{B}/oboete-*'), key=os.path.getmtime)[-1]
    db = sqlite3.connect(f'file:{home}/oboete.db?mode=ro', uri=True)
    cm = {r['session']: r for r in read_jsonl(f'{B}/claude-mem-dev.jsonl')}
    print(f'today\'s oboete: {open(f"{home}/version.txt").read().strip()}\n')
    print('| session | agent | stratum | oboete observations | oboete summaries | claude-mem observations | claude-mem summaries |')
    print('|---|---|---|---|---|---|---|')
    for s in dev_sessions():
        n = lambda t: db.execute(f'SELECT count(*) FROM {t} WHERE session_id = ?', (s['session'],)).fetchone()[0]
        c = cm.get(s['session'], {})
        print(f'| {s["session"][:8]} | {s["agent"]} | {"/".join(s["stratum"])} | {n("observations")} | {n("summaries")} '
              f'| {len(c.get("observations", []))} | {len(c.get("summaries", []))} |')
    print('\n| provider | outcome | calls |\n|---|---|---|')
    for p, o, k in db.execute('SELECT provider, outcome, count(*) FROM provider_calls GROUP BY 1, 2 ORDER BY 1, 2'):
        print(f'| {p} | {o} | {k} |')


if __name__ == '__main__':
    owner_only()
    from freeze import check
    bad = check()
    if bad:
        sys.exit('frozen inputs changed: ' + ', '.join(bad))
    cmd = sys.argv[1:2]
    if cmd == ['config'] and len(sys.argv) == 3:
        config(sys.argv[2])
    elif cmd == ['oboete']:
        run_oboete()
    elif cmd == ['claude-mem']:
        claude_mem()
    elif cmd == ['summary']:
        summary()
    else:
        sys.exit(__doc__)
