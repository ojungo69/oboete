// `oboete sync …` (contracts/sync.md "Commands and health"). A human runs every sync operation;
// the key is typed on a TTY with echo off and printed only by `key show` on a TTY. Exit codes:
// 0 ok, 1 nothing to do or a rejected bundle, 2 usage, 3 consent mismatch, 4 busy.
import { parseArgs } from 'node:util';

import { openDatabase } from './db/open.js';
import { oboetePaths, resolveHome, type OboetePaths } from './paths.js';
import { ResolveError } from './sync/apply.js';
import { initSpace, joinSpace, leaveSpace, mapRepo, pullSpace, pushSpace, resolveRow, showKey, withSpaceLock } from './sync/space.js';
import { consentTupleOf, loadSyncConfig, SyncError, syncStatus } from './sync/status.js';

const USAGE = 'Usage: oboete sync init <dir> [--classes eligible,local_only,private] [--json]\n' +
  '       oboete sync join <dir> [--classes ...] [--json]      (the key line is typed on the terminal)\n' +
  '       oboete sync key show\n' +
  '       oboete sync push [--republish] [--json]\n' +
  '       oboete sync pull [--json]\n' +
  '       oboete sync status [--json]\n' +
  '       oboete sync resolve <origin-id> --keep <revision-id|checkpoint-memory-origin> [--json]\n' +
  '       oboete sync map-repo <repo-key> <local-repo-id> [--json]\n' +
  '       oboete sync leave [--json]\n';

type Io = { out(text: string): void; err(text: string): void; isTty(): boolean; readSecret(prompt: string): Promise<string> };

function processIo(): Io {
  return {
    out: (text) => { process.stdout.write(text); },
    err: (text) => { process.stderr.write(text); },
    isTty: () => process.stdin.isTTY === true && process.stdout.isTTY === true,
    readSecret: (prompt) => new Promise<string>((resolve) => {
      // Raw mode: nothing typed is echoed, so the key never lands in the terminal scrollback.
      process.stdout.write(prompt);
      const stdin = process.stdin;
      stdin.setRawMode(true);
      stdin.resume();
      stdin.setEncoding('utf8');
      let line = '';
      const finish = (): void => { stdin.setRawMode(false); stdin.pause(); stdin.off('data', onData); process.stdout.write('\n'); };
      const onData = (chunk: string): void => {
        for (const character of chunk) {
          if (character === '\r' || character === '\n' || character === '\u0004') { finish(); resolve(line); return; }
          if (character === '\u0003') { finish(); process.exit(130); }
          if (character === '\u007f' || character === '\b') { line = line.slice(0, -1); continue; }
          line += character;
        }
      };
      stdin.on('data', onData);
    }),
  };
}

function classesOf(value: string | undefined): string[] {
  return (value ?? 'eligible,local_only,private').split(',').map((part) => part.trim()).filter((part) => part !== '');
}

function report(io: Io, json: boolean, value: unknown, text: string): void {
  io.out(json ? `${JSON.stringify(value)}\n` : text);
}

/**
 * What `init` and `join` report: the consent tuple the space just recorded the hash of, shown
 * before any push exports anything (contracts/sync.md "Consent"). A later push or pull that finds
 * one of these values changed performs no I/O, so the developer reads here what a push will send.
 */
function consentReport(paths: OboetePaths, done: string): { value: Record<string, unknown>; text: string } {
  const config = loadSyncConfig(paths);
  if (config === null) throw new SyncError('space_not_configured');
  const tuple = consentTupleOf(config);
  const text = `${['A push exports memories of the sensitivity classes below to this directory, encrypted:',
    `  Directory: ${tuple.directory} (${tuple.directory_realpath})`,
    `  Space: ${tuple.space_id}`,
    `  Key: ${tuple.key_id}`,
    `  Encryption: ${tuple.encryption}`,
    `  Sensitivity classes exported: ${tuple.classes.join(', ')}`,
    `  Network: ${tuple.network}`,
    'Nothing has left this machine yet; `oboete sync leave` undoes the space before the first push.',
    done,
  ].join('\n')}\n`;
  return { value: { space_id: tuple.space_id, consent: tuple }, text };
}

export async function runSync(argv: string[], io: Io = processIo(), paths: OboetePaths = oboetePaths(resolveHome()), now = Date.now()): Promise<number> {
  let parsed: ReturnType<typeof parseArgs>;
  try {
    parsed = parseArgs({ args: argv, strict: true, allowPositionals: true, options: {
      json: { type: 'boolean' }, help: { type: 'boolean' }, classes: { type: 'string' }, keep: { type: 'string' }, republish: { type: 'boolean' },
    } });
  } catch { io.err(USAGE); return 2; }
  if (parsed.values.help) { io.out(USAGE); return 0; }
  const json = parsed.values.json === true;
  const [command, ...args] = parsed.positionals;
  const arity: Record<string, number> = { init: 1, join: 1, key: 1, push: 0, pull: 0, status: 0, resolve: 1, 'map-repo': 2, leave: 0 };
  if (command === undefined || arity[command] === undefined || args.length !== arity[command] || (command === 'key' && args[0] !== 'show')
    || (command === 'resolve') !== (typeof parsed.values.keep === 'string') || args.some((arg) => arg.length > 4_096)
    // Classes are consent-bound: they are recorded once, at `init`/`join`, and the consent hash a
    // push checks is taken over them. Accepting `--classes` on a push and ignoring it would export
    // the recorded classes against the intent the developer just typed, so it is refused instead.
    || (parsed.values.classes !== undefined && command !== 'init' && command !== 'join')
    || (parsed.values.republish === true && command !== 'push')
    // `key show` prints the key line itself, which is not JSON and is never meant for a machine to
    // read: it is shown on a terminal to be carried to the next device by hand. The usage line is
    // the only one without `[--json]`, and the parser refuses the combination rather than printing
    // a secret to something that asked for a parseable stream.
    || (command === 'key' && json)) {
    io.err(USAGE);
    return 2;
  }
  if (command === 'key') {
    if (!io.isTty()) { io.err('The key is shown only on a terminal.\n'); return 2; }
    try { io.out(`${showKey(paths)}\n`); return 0; } catch (error) { return fail(io, json, error); }
  }
  const { db } = openDatabase({ path: paths.db, timeoutMs: 2_000 });
  try {
    switch (command) {
      case 'init': {
        const result = initSpace(db, paths, { directory: args[0]!, classes: classesOf(parsed.values.classes as string | undefined), now });
        const consent = consentReport(paths, `Sync space ${result.spaceId} created. Run \`oboete sync key show\` on a terminal to carry the key to the next device.`);
        report(io, json, consent.value, consent.text);
        return 0;
      }
      case 'join': {
        if (!io.isTty()) { io.err('The key line is typed on a terminal; it is never accepted as an argument, variable or file.\n'); return 2; }
        const line = await io.readSecret('Key line (oboete-sync-key/1:…): ');
        const result = joinSpace(db, paths, { directory: args[0]!, keyLine: line, classes: classesOf(parsed.values.classes as string | undefined), now });
        const consent = consentReport(paths, `Joined sync space ${result.spaceId}.`);
        report(io, json, consent.value, consent.text);
        return 0;
      }
      case 'push': {
        const result = pushSpace(db, paths, { now, republish: parsed.values.republish === true });
        report(io, json, result, result.outcome === 'unchanged' ? 'Nothing changed since the last push.\n'
          : `Published ${result.revisionLines} revision lines (${result.heads} heads; withheld: ${JSON.stringify(result.withheld)}).\n`);
        return 0;
      }
      case 'pull': {
        const result = pullSpace(db, paths, { now });
        report(io, json, result, result.bundles.length === 0 ? 'No bundles from other devices were found.\n'
          : result.bundles.map((bundle) => `${bundle.replica}: ${bundle.outcome}${bundle.reason === null ? '' : ` (${bundle.reason})`}`
            + (bundle.result === undefined ? '' : ` stored ${bundle.result.stored}, materialized ${bundle.result.materialized}, withheld ${bundle.result.withheldOnApply}, conflicts ${bundle.result.conflicts}`)).join('\n') + '\n');
        return result.bundles.some((bundle) => bundle.outcome === 'rejected') ? 1 : 0;
      }
      case 'status': {
        const status = syncStatus(db, paths);
        report(io, json, status, status.configured
          ? `Space ${status.space_id} in ${status.directory} (classes: ${status.classes!.join(', ')}); ${status.replicas.length} other replicas seen; `
            + `${status.totals.conflicts} open conflicts; ${status.totals.withheld_on_apply} withheld on apply; ${status.totals.unmapped_repos} unmapped repositories.\n`
          : 'Sync is not configured.\n');
        return 0;
      }
      case 'resolve': {
        const config = loadSyncConfig(paths);
        if (config === null) throw new SyncError('space_not_configured');
        const result = withSpaceLock(paths, config.space_id, () => {
          db.exec('BEGIN IMMEDIATE');
          try { const kept = resolveRow(db, args[0]!, parsed.values.keep as string, now); db.exec('COMMIT'); return kept; }
          catch (error) { db.exec('ROLLBACK'); throw error; }
        });
        report(io, json, result, `Resolved: the successor revision is ${result.revision_id}.\n`);
        return 0;
      }
      case 'map-repo': {
        const result = mapRepo(db, paths, { repoKey: args[0]!, localRepoId: args[1]!, now });
        report(io, json, result, `Mapped. ${result.reapplied} withheld rows were re-evaluated.\n`);
        return 0;
      }
      case 'leave': {
        leaveSpace(db, paths);
        report(io, json, { left: true }, 'Left the sync space. The key, cursors and this device\'s bundle were removed.\n');
        return 0;
      }
      default: io.err(USAGE); return 2;
    }
  } catch (error) {
    return fail(io, json, error);
  } finally { db.close(); }
}

function fail(io: Io, json: boolean, error: unknown): number {
  if (error instanceof SyncError) {
    const code = error.code === 'consent_mismatch' ? 3 : error.code === 'busy' ? 4 : 1;
    io.err(json ? `${JSON.stringify({ error: error.code, ...error.detail })}\n` : `${describe(error)}\n`);
    return code;
  }
  if (error instanceof ResolveError) { io.err(json ? `${JSON.stringify({ error: error.code })}\n` : `Resolve failed: ${error.code}.\n`); return 1; }
  throw error;
}

function describe(error: SyncError): string {
  switch (error.code) {
    case 'consent_mismatch': return `The sync consent no longer matches (${(error.detail.changed as string[]).join(', ')}). Run \`oboete sync leave\` and set the space up again.`;
    case 'busy': return 'Another sync command holds this space. Try again when it finishes.';
    case 'space_not_configured': return 'Sync is not configured. Run `oboete sync init <dir>` or `oboete sync join <dir>`.';
    case 'space_exists': return 'A sync space is already configured. Run `oboete sync leave` first.';
    case 'too_many_replicas': return `The space directory holds ${String(error.detail.count)} bundles (limit 32). Remove bundles of retired replicas: ${(error.detail.without_cursor as string[]).join(', ')}.`;
    case 'directory_inside_home': return 'The sync directory must be outside the oboete home.';
    case 'invalid_key_line': return 'That is not a valid key line.';
    case 'key_missing': return 'The space key file is missing.';
    case 'key_permissions': return 'The space key file must be readable by the owner only.';
    case 'key_mismatch': return 'The space key file does not match the key this space was set up with. Restore the original key, or run `oboete sync leave` and join the space again.';
    case 'publish_failed': return `This device's snapshot could not be built (${String(error.detail.code)}); nothing was written.`;
    case 'invalid_classes': return 'Classes must be a non-empty list drawn from eligible, local_only and private; secret is never selectable.';
    case 'plaintext_too_large': return `This device's snapshot is ${String(error.detail.bytes)} bytes, over the 256 MiB bundle bound; nothing was written.`;
    default: return `Sync failed: ${error.code}.`;
  }
}
