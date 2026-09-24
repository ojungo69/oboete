"""PR-A2: build the comparison corpus from real data, read-only. Docs: every oboete observation,
summary and prompt (prompt head 1,000 chars, as the proposal embeds it), plus a fixed sample of
claude-mem observations and prompts (10 long prompts kept up to 8,000 chars). Queries ("q:" ids):
observation titles and prompt heads. Prints {"id","text"} lines; the gate runs in `oboete spike-embed`."""
import json, os, sqlite3
def ro(path):
    return sqlite3.connect(f'file:{os.path.expanduser(path)}?mode=ro', uri=True)
o, c = ro('~/.oboete/oboete.db'), ro('~/.claude-mem/claude-mem.db')
rows = []
for i, t, b in o.execute('select id, title, body from observations order by id'):
    rows.append((f'o{i}', f'{t}\n{b}'))
    if i % 5 == 0: rows.append((f'q:o{i}', t))
rows += [(f's{i}', b) for i, b in o.execute('select id, body from summaries order by id')]
for i, b in o.execute('select id, body from prompts order by id'):
    rows += [(f'p{i}', b[:1000]), (f'q:p{i}', b[:200])]
# Fixed pseudo-random order by id (the same sample every run).
for n, (i, t, b) in enumerate(c.execute("select id, title, narrative from observations where coalesce(narrative,'') != '' and coalesce(title,'') != '' order by (id * 2654435761) % 4294967296 limit 100")):
    rows.append((f'c{i}', f'{t}\n{b}'))
    if n % 4 == 0: rows.append((f'q:c{i}', t))
rows += [(f'cp{i}', b[:1000]) for i, b in c.execute('select id, prompt_text from user_prompts where length(prompt_text) between 20 and 2000 order by (id * 2654435761) % 4294967296 limit 30')]
rows += [(f'cl{i}', b[:8000]) for i, b in c.execute('select id, prompt_text from user_prompts where length(prompt_text) > 3000 order by (id * 2654435761) % 4294967296 limit 10')]
for i, t in rows:
    print(json.dumps({'id': i, 'text': t}, ensure_ascii=False))
