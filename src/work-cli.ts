import { parseArgs } from 'node:util';

import { openDatabase } from './db/open.js';
import { oboetePaths, resolveHome } from './paths.js';
import { resolveRepoIdentity } from './repo-identity.js';
import { chooseSourceWork, chooseWork, completeWork, readWorkSelection, workStatus } from './work.js';
import { filterReadOutput } from './privacy/provenance.js';

const USAGE = 'Usage: oboete work status [--all] [--json]\n' +
  '       oboete work choose <binding-id> <work-id|new> [--json]\n' +
  '       oboete work choose-source <source-id> <work-id|new> [--json]\n' +
  '       oboete work complete <work-id> [--json]\n';

export async function runWork(argv: string[]): Promise<number> {
  let parsed: ReturnType<typeof parseArgs>;
  try {
    parsed = parseArgs({ args: argv, strict: true, allowPositionals: true,
      options: { all: { type: 'boolean' }, json: { type: 'boolean' }, help: { type: 'boolean' } } });
  } catch {
    process.stderr.write(USAGE);
    return 2;
  }
  if (parsed.values.help) { process.stdout.write(USAGE); return 0; }
  const [command, ...args] = parsed.positionals;
  const required = command === 'status' ? 0 : command === 'choose' || command === 'choose-source' ? 2 : command === 'complete' ? 1 : -1;
  if (args.length !== required || (parsed.values.all && command !== 'status') || args.some((arg) => arg.length > 128)) {
    process.stderr.write(USAGE);
    return 2;
  }
  const identity = resolveRepoIdentity(process.cwd());
  const location = { repoId: identity.id, contextKey: identity.worktreeKey };
  const { db } = openDatabase({ path: oboetePaths(resolveHome()).db, timeoutMs: 2_000 });
  try {
    if (command === 'status') {
      const stored = workStatus(db, location, parsed.values.all === true);
      const selection = readWorkSelection(db, location);
      const checkpoints = stored.works.flatMap((work) => work.checkpoint === null ? [] : [work.checkpoint]);
      const checked = parsed.values.all === true ? { memories: checkpoints, works: stored.works }
        : await filterReadOutput(db, { ...location, repoRoot: identity.root, home: resolveHome(),
          bindingId: selection.bindingId, workId: selection.workId, history: true }, checkpoints, stored.works);
      const visible = new Map(checked.memories.map((memory) => [memory.id, memory]));
      const status = { ...stored, works: checked.works.map((work) => ({ ...work,
        checkpoint: work.checkpoint === null ? null : visible.get(work.checkpoint.id) ?? null })) };
      if (parsed.values.json) process.stdout.write(`${JSON.stringify(status)}\n`);
      else {
        if (!status.contextVerified) process.stdout.write('The current context identity could not be verified. Cross-session continuation is unavailable.\n');
        process.stdout.write(status.works.length === 0 ? 'No work was found.\n' : status.works.map((work) =>
          `${String(work.id)}  ${String(work.state)}  ${JSON.stringify(work.purpose ?? 'Untitled work')}`).join('\n') + '\n');
        for (const binding of status.bindings) process.stdout.write(`Binding ${String(binding.id)}: ${String(binding.work_id ?? 'Selection required')}\n`);
        for (const work of status.works) if (work.checkpoint !== null) process.stdout.write(
          `Checkpoint for ${work.id}: ${JSON.stringify(work.checkpoint.title)}\n${JSON.stringify(work.checkpoint.body)}\n`);
        if (status.hasMore) process.stdout.write('Additional work or bindings were omitted from this bounded listing.\n');
      }
      return 0;
    }
    const now = Date.now();
    const result = command === 'choose-source' ? chooseSourceWork(db, { ...location, root: identity.root, sourceId: args[0], workId: args[1], now })
      : command === 'choose' ? chooseWork(db, { ...location, bindingId: args[0], workId: args[1], now })
      : completeWork(db, { repoId: identity.id, workId: args[0], now });
    if (!result) {
      process.stderr.write('The work or binding was not found in the current scope, or the choice is no longer current.\n');
      return 1;
    }
    process.stdout.write(parsed.values.json ? `${JSON.stringify(result)}\n`
      : command === 'complete' ? 'The work was marked completed.\n' : 'The work selection was saved.\n');
    return 0;
  } finally {
    db.close();
  }
}
