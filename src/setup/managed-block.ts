// The one writer of foreign agent configuration. Every edit oboete makes to a file the developer
// owns lives in an oboete-managed region: a `# oboete:begin/end` block in TOML and handler objects
// carrying `"oboete": true` in JSON (research.md R8, spec FR-031). The TOML file is never
// re-serialized -- the block is spliced in as text, so comments, ordering and formatting outside
// it survive byte for byte (plan.md Complexity Tracking row 12). Each write is
// backup -> temporary file -> re-parse -> rename, and the backup and the rewritten file keep the
// developer's mode and owner. Only hook wiring is touched here; an agent's own memory store is
// never read or changed (FR-043).
import {
  chmodSync,
  chownSync,
  closeSync,
  existsSync,
  fsyncSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import type { Stats } from 'node:fs';
import { dirname } from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import { parse as parseToml } from 'smol-toml';

import { shellQuote } from './shell-quote.js';

export const BLOCK_BEGIN = '# oboete:begin';
export const BLOCK_END = '# oboete:end';
/** The pre-oboete copy of a foreign file: created when absent, deleted by removal. */
export const BACKUP_SUFFIX = '.oboete-backup';
/** contracts/agents.md: all three JSON files hold their hook wiring under a top-level `hooks`. */
const CONTAINER = 'hooks';

export type ManagedFileErrorCode =
  | 'symlink_escape'
  | 'malformed_block'
  | 'reparse_failed'
  | 'not_an_object'
  | 'unmarked_handler';

export class ManagedFileError extends Error {
  readonly code: ManagedFileErrorCode;
  readonly file: string;
  readonly foreignTable: boolean;

  constructor(message: string, code: ManagedFileErrorCode, file: string, foreignTable = false) {
    super(message);
    this.name = 'ManagedFileError';
    this.code = code;
    this.file = file;
    this.foreignTable = foreignTable;
  }
}

export type ManagedWriteOptions = {
  /**
   * The file may hold credentials (`~/.codex/config.toml` can carry API keys), so its backup is
   * created 0600 instead of copying the original mode (research.md R8).
   */
  credentialBearing?: boolean;
  /** BUG-ASSESSMENT.md: retain the old bundle identity before setup replaces its marked handlers. */
  previousBlockText?: string;
};

/**
 * A TOML block, appended when the file has none and replaced in place when it has one. `blockText`
 * must open its own table headers: it is spliced in as text, so a bare key would land in whichever
 * table the developer's last line opened.
 */
export function applyTomlBlock(
  file: string,
  blockText: string,
  options: ManagedWriteOptions = {},
): void {
  const target = resolveTarget(file);
  const lines = readLines(target);
  const region = findRegion(lines, file);
  const inner = trimNewlines(blockText);
  const block = inner === '' ? [BLOCK_BEGIN, BLOCK_END] : [BLOCK_BEGIN, ...inner.split('\n'), BLOCK_END];
  const outside = region ? [...lines.slice(0, region.start), ...lines.slice(region.end + 1)] : lines;
  const current = parseTomlOrThrow(joinLines(outside), file);
  const expected = parseTomlOrThrow(inner, file);
  const previous = parseTomlOrThrow(options.previousBlockText ?? '', file);
  const path = ['mcp_servers', 'oboete'];
  const existingMcp = tableAt(current, path);
  const expectedMcp = tableAt(expected, path);
  const foreignTable = existingMcp !== undefined && expectedMcp !== undefined &&
    !sameMcpCommand(existingMcp, expectedMcp) && !sameMcpCommand(existingMcp, tableAt(previous, path));
  const next = region
    ? [...lines.slice(0, region.start), ...block, ...lines.slice(region.end + 1)]
    : [...stripTomlTables(lines, current, expected, previous), ...block];
  writeManaged(target, file, joinLines(next), (text, name) => parseTomlOrThrow(text, name, foreignTable), options, true);
}

/** BUG-ASSESSMENT.md: removal must also find tables whose comment markers the agent dropped. */
export function removeTomlBlock(file: string, blockText = ''): void {
  const target = resolveTarget(file);
  if (existsSync(target)) {
    const lines = readLines(target);
    const region = findRegion(lines, file);
    let next = lines;
    if (region) {
      next = [...lines.slice(0, region.start), ...lines.slice(region.end + 1)];
    } else {
      // A marker-less file has to parse before oboete can tell whether its table is in there. One
      // that does not is left whole, and so is its backup, which the developer can restore.
      let current: unknown;
      try {
        current = parseToml(joinLines(lines));
      } catch (error) {
        throw new ManagedFileError(
          `${file} would not parse as TOML (${reason(error)}), so any \`[mcp_servers.oboete]\` table oboete wrote ` +
            `there was left for you to delete by hand (the ${BACKUP_SUFFIX} copy is kept)`,
          'reparse_failed',
          file,
        );
      }
      if (blockText !== '') next = stripTomlTables(lines, current, parseTomlOrThrow(blockText, file));
    }
    if (next !== lines) {
      writeManaged(target, file, joinLines(next), parseTomlOrThrow, {}, false);
    }
  }
  rmSync(target + BACKUP_SUFFIX, { force: true });
}

type McpCommand = { command: string; args: [string, 'mcp'] };

/** BUG-ASSESSMENT.md: a marked hook proves bundle ownership without guessing from a path fragment. */
export function readOboeteMcp(file: string, hooksFile: string, agent: 'codex' | 'claude-or-grok'): McpCommand | null {
  try {
    const mcp = tableAt(parseToml(readFileSync(file, 'utf8')), ['mcp_servers', 'oboete']);
    if (!isPlainObject(mcp) || typeof mcp.command !== 'string' || !Array.isArray(mcp.args) ||
      mcp.args.length !== 2 || typeof mcp.args[0] !== 'string' || mcp.args[1] !== 'mcp') return null;
    const root: unknown = JSON.parse(readFileSync(hooksFile, 'utf8'));
    if (!isPlainObject(root) || !isPlainObject(root.hooks)) return null;
    const prefix = `${shellQuote(mcp.command)} ${shellQuote(mcp.args[0])} hook --agent ${agent} --event `;
    const owned = Object.entries(root.hooks).some(([event, groups]) =>
      Array.isArray(groups) && groups.some((group) => isOboeteOwned(group) && isPlainObject(group) &&
        Array.isArray(group.hooks) && group.hooks.some((hook) => isPlainObject(hook) &&
          hook.type === 'command' && hook.command === prefix + event)),
    );
    return owned ? { command: mcp.command, args: [mcp.args[0], 'mcp'] } : null;
  } catch {
    return null;
  }
}

export function hasUnmarkedTomlBlock(file: string, blockText: string): boolean {
  try {
    const lines = readLines(file);
    return findRegion(lines, file) === null &&
      stripTomlTables(lines, parseToml(joinLines(lines)), parseToml(blockText)) !== lines;
  } catch {
    return false;
  }
}

/** The block text without leading and trailing newlines; the former regex re-scanned a long run. */
function trimNewlines(text: string): string {
  let start = 0;
  let end = text.length;
  while (start < end && text[start] === '\n') start += 1;
  while (end > start && text[end - 1] === '\n') end -= 1;
  return text.slice(start, end);
}

function tableAt(root: unknown, path: readonly string[]): unknown {
  let value = root;
  for (const key of path) {
    if (!isPlainObject(value) || !Object.hasOwn(value, key)) return undefined;
    value = value[key];
  }
  return value;
}

function sameMcpCommand(value: unknown, expected: unknown): boolean {
  return isPlainObject(value) && isPlainObject(expected) && typeof expected.command === 'string' &&
    Array.isArray(expected.args) && expected.args.length === 2 && expected.args[1] === 'mcp' &&
    value.command === expected.command && isDeepStrictEqual(value.args, expected.args);
}

function hasOnlyOwnedMcpKeys(value: Record<string, unknown>): boolean {
  return Object.keys(value).every((key) => ['command', 'args', 'enabled'].includes(key));
}

function sameTrustRow(value: Record<string, unknown>, expected: Record<string, unknown>): boolean {
  return Object.keys(expected).length === 1 && typeof expected.trusted_hash === 'string' &&
    isDeepStrictEqual(value, expected);
}

function sameOwnedTable(value: unknown, expected: unknown): boolean {
  if (!isPlainObject(value) || !isPlainObject(expected)) return false;
  if (sameMcpCommand(value, expected)) {
    return hasOnlyOwnedMcpKeys(value);
  }
  return sameTrustRow(value, expected);
}

/** Table headers outside multiline values, in source order; array tables have no removable path. */
function findTomlTableHeaders(lines: string[]): { start: number; path: string[] }[] {
  const headers: { start: number; path: string[] }[] = [];
  for (const [start, line] of lines.entries()) {
    if (!line.trimStart().startsWith('[')) continue;
    try {
      let header: unknown = parseToml(line);
      // ponytail: prefix parsing is quadratic in table count; use parser source spans if configs grow large.
      // BUG-ASSESSMENT.md: header text inside multiline values must not become a removal boundary.
      parseToml(joinLines(lines.slice(0, start + 1)));
      const path: string[] = [];
      while (isPlainObject(header) && Object.keys(header).length === 1) {
        const key = Object.keys(header)[0];
        path.push(key);
        header = header[key];
      }
      headers.push({ start, path: Array.isArray(header) ? [] : path });
    } catch {
      continue;
    }
  }
  return headers;
}

function stripTomlTables(lines: string[], current: unknown, expected: unknown, previous?: unknown): string[] {
  const headers = findTomlTableHeaders(lines);
  let next = lines;
  for (let index = headers.length - 1; index >= 0; index -= 1) {
    const { start, path } = headers[index];
    if (path.length === 0) continue;
    const value = tableAt(current, path);
    if (!sameOwnedTable(value, tableAt(expected, path)) && !sameOwnedTable(value, tableAt(previous, path))) continue;
    next = [...next.slice(0, start), ...next.slice(headers[index + 1]?.start ?? lines.length)];
  }
  return next;
}

/**
 * Merges oboete-owned handler objects into the hook arrays under the top-level `hooks` object, the
 * container contracts/agents.md gives all three JSON files (`~/.claude/settings.json`,
 * `~/.codex/hooks.json`, `~/.grok/hooks/oboete.json`). Entries the developer wrote keep their
 * order; a previous oboete entry is replaced where it stood. `handlers` is the whole of what oboete
 * owns here afterwards, so a repeat that no longer wires an event drops the handler it left there
 * last time instead of leaving it live (FR-031, setup is repeatable). The file is created when
 * missing.
 */
export function applyJsonHandlers(
  file: string,
  handlers: Record<string, readonly unknown[]>,
  options: ManagedWriteOptions = {},
): void {
  for (const entries of Object.values(handlers)) {
    for (const entry of entries) {
      // Without the marker the entry could never be found again, so removal would leave it behind.
      if (!isOboeteOwned(entry)) {
        throw new ManagedFileError(
          `a handler for ${file} is missing the "oboete": true marker that makes it removable`,
          'unmarked_handler',
          file,
        );
      }
    }
  }
  const target = resolveTarget(file);
  const root = readJsonRoot(target, file);
  const node: Record<string, unknown> = hookContainer(root, file) ?? (root[CONTAINER] = {});
  stripOwned(node, new Set(Object.keys(handlers)));
  for (const [event, entries] of Object.entries(handlers)) {
    const current = node[event];
    if (current !== undefined && !Array.isArray(current)) {
      throw new ManagedFileError(
        `${file} holds a ${typeof current} at ${CONTAINER}.${event} where oboete expects a list of handlers`,
        'not_an_object',
        file,
      );
    }
    node[event] = mergeEntries(current ?? [], entries);
  }
  writeManaged(target, file, serializeJson(root), parseJsonOrThrow, options, true);
}

/**
 * Strips the oboete-owned entries, drops the arrays and the container it emptied, deletes the
 * backup. `keepBackup` is for a rollback: the copy from before oboete first touched the file is
 * what `oboete setup --remove` restores, so undoing a later run must leave it where it is.
 */
export function removeJsonHandlers(file: string, options: { keepBackup?: boolean } = {}): void {
  const target = resolveTarget(file);
  if (existsSync(target)) {
    const original = readFileSync(target, 'utf8');
    const root = readJsonRoot(target, file);
    const node = hookContainer(root, file);
    if (node) {
      stripOwned(node, new Set());
      if (Object.keys(node).length === 0) delete root[CONTAINER];
    }
    const content = serializeJson(root);
    if (content !== original) writeManaged(target, file, content, parseJsonOrThrow, {}, false);
  }
  if (options.keepBackup !== true) rmSync(target + BACKUP_SUFFIX, { force: true });
}

/**
 * The path the write lands on. A symbolic link is followed only while it stays inside its own
 * directory, so a link to a sibling keeps working and survives the rename. A link that leaves the
 * directory -- a dotfiles checkout, `/etc` -- is refused rather than silently rewriting a file the
 * developer manages somewhere else.
 */
function resolveTarget(file: string): string {
  let link: boolean;
  try {
    link = lstatSync(file).isSymbolicLink();
  } catch {
    return file; // Not there yet; the write creates it.
  }
  if (!link) return file;
  let resolved: string;
  try {
    resolved = realpathSync(file);
  } catch {
    throw new ManagedFileError(`${file} is a symbolic link that does not resolve`, 'symlink_escape', file);
  }
  if (dirname(resolved) !== realpathSync(dirname(file))) {
    throw new ManagedFileError(
      `${file} is a symbolic link to ${resolved}, outside its own directory; point the agent at a real file`,
      'symlink_escape',
      file,
    );
  }
  return resolved;
}

/**
 * backup -> temporary file -> re-parse -> rename (research.md R8). The re-parse reads the bytes
 * back from disk, so a result oboete could not read again never replaces the developer's file:
 * the temporary file is removed and the original stays exactly as it was.
 */
function writeManaged(
  target: string,
  file: string,
  content: string,
  verify: (text: string, file: string) => void,
  options: ManagedWriteOptions,
  backup: boolean,
): void {
  const stats = existsSync(target) ? statSync(target) : null;
  // A file oboete creates holds hook wiring for this developer only.
  const mode = stats ? stats.mode & 0o7777 : 0o600;
  if (stats && backup) backupOnce(target, stats, options.credentialBearing === true);

  const temporary = `${target}.oboete-tmp-${process.pid}`;
  // A fresh machine has no `~/.grok/hooks/` for the file oboete is about to create.
  mkdirSync(dirname(target), { recursive: true });
  // `wx` like the backup below: the temporary name is predictable, so an exclusive create is what
  // keeps a link pre-placed there from being followed and the write landing on the file it points
  // at. A stale temporary is not unlinked first -- that would reopen the very race this closes --
  // so the rare leftover of a killed run with this pid surfaces as EEXIST naming the exact file.
  const handle = openSync(temporary, 'wx', mode);
  try {
    // `writeFileSync` on the handle loops until every byte is written; `writeSync` can return
    // after a partial write and leave a truncated file that the verification below has to catch.
    writeFileSync(handle, content);
    fsyncSync(handle);
  } finally {
    closeSync(handle);
  }
  try {
    chmodSync(temporary, mode); // The creation mode is masked by the umask; this is not.
    if (stats) preserveOwner(temporary, stats);
    verify(readFileSync(temporary, 'utf8'), file);
  } catch (error) {
    rmSync(temporary, { force: true });
    throw error;
  }
  renameSync(temporary, target);
}

/**
 * The backup is the state before oboete first touched the file: `wx` makes "only when none exists"
 * atomic, so a repeated setup keeps the original copy and only a removal clears it.
 */
function backupOnce(target: string, stats: Stats, credentialBearing: boolean): void {
  const backup = target + BACKUP_SUFFIX;
  const mode = credentialBearing ? 0o600 : stats.mode & 0o7777;
  try {
    writeFileSync(backup, readFileSync(target), { flag: 'wx', mode });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'EEXIST') return;
    throw error;
  }
  chmodSync(backup, mode);
  preserveOwner(backup, stats);
}

function preserveOwner(file: string, stats: Stats): void {
  try {
    chownSync(file, stats.uid, stats.gid);
  } catch {
    // Best effort: changing the owner needs root, and setup normally runs as the owner already.
  }
}

function readLines(target: string): string[] {
  if (!existsSync(target)) return [];
  const text = readFileSync(target, 'utf8');
  // The line terminator is restored on write, so a file that ended without one gains a newline.
  return text === '' ? [] : text.replace(/\n$/, '').split('\n');
}

function joinLines(lines: string[]): string {
  return lines.length === 0 ? '' : `${lines.join('\n')}\n`;
}

function findRegion(lines: string[], file: string): { start: number; end: number } | null {
  const begins: number[] = [];
  const ends: number[] = [];
  lines.forEach((line, index) => {
    const trimmed = line.trim();
    if (trimmed === BLOCK_BEGIN) begins.push(index);
    else if (trimmed === BLOCK_END) ends.push(index);
  });
  if (begins.length === 0 && ends.length === 0) return null;
  const start = begins[0];
  const end = ends[0];
  if (begins.length !== 1 || ends.length !== 1 || end < start) {
    throw new ManagedFileError(
      `${file} holds ${begins.length} '${BLOCK_BEGIN}' and ${ends.length} '${BLOCK_END}' lines; oboete edits exactly one region and does not guess which`,
      'malformed_block',
      file,
    );
  }
  return { start, end };
}

function parseTomlOrThrow(text: string, file: string, foreignTable = false): unknown {
  try {
    return parseToml(text);
  } catch (error) {
    throw new ManagedFileError(
      `${file} would not parse as TOML after the oboete block (${reason(error)})`,
      'reparse_failed',
      file,
      foreignTable,
    );
  }
}

function parseJsonOrThrow(text: string, file: string): void {
  try {
    JSON.parse(text);
  } catch (error) {
    throw new ManagedFileError(
      `${file} would not parse as JSON after the oboete handlers (${reason(error)})`,
      'reparse_failed',
      file,
    );
  }
}

function serializeJson(root: Record<string, unknown>): string {
  return `${JSON.stringify(root, null, 2)}\n`;
}

export function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** The marker every entry oboete writes carries, and the only way removal finds it again. */
export function isOboeteOwned(entry: unknown): boolean {
  return isPlainObject(entry) && entry.oboete === true;
}

function readJsonRoot(target: string, file: string): Record<string, unknown> {
  if (!existsSync(target)) return {};
  const text = readFileSync(target, 'utf8');
  if (text.trim() === '') return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (error) {
    throw new ManagedFileError(
      `${file} is not valid JSON (${reason(error)}); oboete does not rewrite a file it cannot read`,
      'not_an_object',
      file,
    );
  }
  if (!isPlainObject(parsed)) {
    throw new ManagedFileError(`${file} does not hold a JSON object`, 'not_an_object', file);
  }
  return parsed;
}

/** The `hooks` object of the file, or null when it has none; anything else there is refused. */
function hookContainer(root: Record<string, unknown>, file: string): Record<string, unknown> | null {
  const node = root[CONTAINER];
  if (node === undefined) return null;
  if (!isPlainObject(node)) {
    throw new ManagedFileError(
      `${file} holds a ${Array.isArray(node) ? 'list' : typeof node} at ${CONTAINER} where oboete expects an object`,
      'not_an_object',
      file,
    );
  }
  return node;
}

/**
 * Takes the oboete-owned entries out of every hook array under `node` except the events in `keep`,
 * and drops an array it empties: an event whose only handler was oboete's returns to absent, the
 * state the developer's file was in before setup.
 */
function stripOwned(node: Record<string, unknown>, keep: ReadonlySet<string>): void {
  for (const [event, value] of Object.entries(node)) {
    if (keep.has(event) || !Array.isArray(value)) continue;
    const kept = value.filter((entry) => !isOboeteOwned(entry));
    if (kept.length === value.length) continue;
    if (kept.length === 0) delete node[event];
    else node[event] = kept;
  }
}

function mergeEntries(existing: readonly unknown[], next: readonly unknown[]): unknown[] {
  const at = existing.findIndex(isOboeteOwned);
  if (at < 0) return [...existing, ...next];
  // oboete keeps the slot the file already gave it, so a repeated setup does not reorder the
  // developer's handlers around it.
  return [
    ...existing.slice(0, at),
    ...next,
    ...existing.slice(at + 1).filter((entry) => !isOboeteOwned(entry)),
  ];
}

function reason(error: unknown): string {
  return error instanceof Error ? error.message.split('\n')[0] : String(error);
}
