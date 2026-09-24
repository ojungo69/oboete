"""PR-B2: build the evaluation questions (docs/pr-b.md). Reads the evaluation store (a copy of the
oboete store with claude-mem imported) and the agents' transcripts, read-only; writes
~/.oboete/eval/queries.jsonl. Nothing here goes to the repository or leaves the machine.

Sets:
  prompt  claude-mem prompts (already cleaned by the import) -> other sessions' knowledge
  agent   memory searches agents actually typed (claude-mem `search` calls in transcripts),
          passed through `oboete gate`
Each question carries its session, so documents from that session can be left out of the
judging (proposal §3.1), and a 70/30 dev/test split by session hash.
"""
import glob, hashlib, json, os, re, sqlite3, subprocess, sys

E = os.path.expanduser('~/.oboete/eval')
# The questions and runs are the developer's own records: owner-only files.
os.umask(0o077)
os.makedirs(f'{E}/runs', mode=0o700, exist_ok=True)
os.chmod(E, 0o700)
OBOETE = sys.argv[1] if len(sys.argv) > 1 else 'oboete'
JA = re.compile(r'[぀-ヿ㐀-鿿]')
# Prompts no developer typed: session openers, reply-format probes, and review requests one agent
# sends another (the same shapes as the delegation prompts in this repository's scratchpads).
MACHINE = re.compile(
    r'^(New session - \d{4}-|Response constraint|"?(Return|Reply) (with )?exactly|DEFAULT-OK|'
    r'Review the (current|staged)|Project: /|Repository: /|以下を修正した。再レビュー|Continue the security review)',
    re.I)

def h(s):
    return int(hashlib.sha256(s.encode()).hexdigest()[:8], 16)

def split(session):
    return 'test' if h('split:' + session) % 10 < 3 else 'dev'

def gate(text):
    return subprocess.run([OBOETE, 'gate'], input=text, capture_output=True, text=True, check=True).stdout

# Runs and judgments are keyed by qid, and agent qids are numbered in transcript order, so a
# rebuilt set could hand old grades to different questions. One set per evaluation directory.
if os.path.exists(f'{E}/queries.jsonl'):
    sys.exit(f'{E}/queries.jsonl exists; its runs and judgments belong to it. Move the directory aside to build a new set.')
db = sqlite3.connect(f'file:{E}/home/oboete.db?mode=ro', uri=True)
out = []
# Prompts: typed questions and requests, not one-word replies or pasted walls of text.
rows = db.execute(
    "SELECT p.id, p.session_id, p.body FROM imports i JOIN prompts p ON p.id = CAST(substr(i.doc, 2) AS INTEGER) "
    "WHERE i.source LIKE 'claude-mem%' AND i.source_id LIKE 'p%' AND length(p.body) BETWEEN 15 AND 600").fetchall()
rows.sort(key=lambda r: h(f'prompt:{r[0]}'))
seen = set()
for pid, session, body in rows:
    text = ' '.join(body.split())
    if text in seen or MACHINE.match(text):
        continue
    seen.add(text)
    out.append({'qid': f'p{pid}', 'set': 'prompt', 'text': text, 'session': session})
    if len(out) == 400:
        break
# Agent searches: the query of each claude-mem `search` call in Claude Code transcripts.
n = 0
for f in sorted(glob.glob(os.path.expanduser('~/.claude/projects/*/*.jsonl'))):
    session = os.path.basename(f)[:-6]
    for line in open(f, errors='replace'):
        if 'mcp-search__search' not in line:
            continue
        try:
            content = json.loads(line).get('message', {}).get('content') or []
        except json.JSONDecodeError:
            continue
        for c in content:
            if isinstance(c, dict) and c.get('type') == 'tool_use' and c.get('name', '').endswith('mcp-search__search'):
                q = (c.get('input') or {}).get('query', '').strip()
                if q and q not in seen:
                    seen.add(q)
                    n += 1
                    out.append({'qid': f'a{n}', 'set': 'agent', 'text': gate(q).strip(), 'session': session})
for q in out:
    q['split'] = split(q['session'])
    q['lang'] = 'ja' if JA.search(q['text']) else 'en'
with open(f'{E}/queries.jsonl', 'w') as w:
    for q in out:
        w.write(json.dumps(q, ensure_ascii=False) + '\n')
from collections import Counter
print(len(out), Counter((q['set'], q['split'], q['lang']) for q in out))
