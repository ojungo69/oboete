// `oboete export` / `oboete import` (spec FR-036, contracts/cli.md, data-model.md "Export line",
// research R12). Security-owned: the file carries no secret text (secret rows and tombstones travel
// as hashes), an imported row can only raise a sensitivity and never lowers one, a tombstone wins
// in both directions, and every active imported row is quarantined as `local_only` /
// `review_state = imported` until the worker classifies it. Nothing here is on the hook path.
import { chmodSync, createReadStream, statSync, writeFileSync } from 'node:fs';
import type { DatabaseSync } from 'node:sqlite';
import { parseArgs } from 'node:util';
import { z } from 'zod';

import { contentHash, materialHash, memoryIdFor } from './db/identity.js';
import { openDatabase } from './db/open.js';
import { sha256Hex } from './hash.js';
import { ensureDirectories, oboetePaths, resolveHome } from './paths.js';
import { cjkBigrams } from './retrieval/fts.js';

export const EXPORT_FORMAT = 'oboete-export/1';
const MAX_LINE_BYTES = 64 * 1024;
const MAX_FILE_BYTES = 256 * 1024 * 1024;
/** Every rejection rolls the import back, so listing more than this many helps nobody. */
export const MAX_REJECTED = 100;

/** data-model "memories": the stricter class wins on every merge. */
const SENSITIVITY_RANK = { eligible: 0, local_only: 1, private: 2, secret: 3 } as const;
type Sensitivity = keyof typeof SENSITIVITY_RANK;

const hash64 = z.string().regex(/^[0-9a-f]{64}$/u);
const timestamp = z.number().int().nonnegative();

const headerSchema = z.looseObject({
  format: z.literal(EXPORT_FORMAT),
  exported_at: timestamp.optional(),
  repos: z.array(
    z.looseObject({
      id: z.string().min(1),
      identity_kind: z.enum(['remote', 'common_dir']),
      normalized_identity: z.string().min(1),
    }),
  ),
});

const sourceSchema = z.looseObject({
  citation_kind: z.enum(['file_read', 'file_modified', 'commit']).nullable(),
  citation_value: z.string().nullable(),
  source_agent: z.string().nullable(),
});

const lineSchema = z.looseObject({
  id: z.string().min(1),
  repo_id: z.string().min(1),
  type: z.enum([
    'bugfix', 'feature', 'refactor', 'change', 'discovery', 'decision',
    'security_alert', 'security_note', 'session_summary',
  ]),
  title: z.string().nullable(),
  body: z.string().nullable(),
  concepts: z.string().nullable(),
  material_hash: hash64,
  content_hash: hash64,
  sensitivity: z.enum(['eligible', 'local_only', 'private', 'secret']),
  review_state: z.enum(['unreviewed', 'reviewed', 'imported']),
  degraded_reason: z.string().nullable(),
  source_session_id: z.string().nullable(),
  source_batch_id: z.string().nullable(),
  source_agent: z.string().nullable(),
  valid_from: timestamp.nullable(),
  valid_to: timestamp.nullable(),
  superseded_by: z.string().nullable(),
  pinned_at: timestamp.nullable(),
  pin_order: z.number().int().nullable(),
  deleted_at: timestamp.nullable(),
  created_at: timestamp.nullable(),
  sources: z.array(sourceSchema),
});
type ExportLine = z.infer<typeof lineSchema>;

export type ImportResult = {
  /** False on a dry run and when any line was rejected: then nothing was written. */
  applied: boolean;
  inserted: number;
  updated: number;
  tombstones: number;
  unchanged: number;
  rejected: { line: number; reason: string }[];
};

export type ImportOptions = {
  now: number;
  dryRun?: boolean;
  /** `--map-repo old=current`: a machine-local repository of another installation onto one here. */
  mapRepo?: Record<string, string>;
  maxFileBytes?: number;
};

const MEMORY_COLUMNS = `id, repo_id, type, title, body, concepts, material_hash, content_hash, sensitivity,
  review_state, degraded_reason, source_session_id, source_batch_id, valid_from, valid_to,
  superseded_by, pinned_at, pin_order, deleted_at, created_at`;

/** Writes the header and one line per memory of every repository; returns what was written. */
export function exportMemories(
  db: DatabaseSync,
  write: (line: string) => void,
  now: number,
): { memories: number; tombstones: number } {
  const repos = db
    .prepare('SELECT id, identity_kind, normalized_identity FROM repos ORDER BY id')
    .all()
    .map((row) => ({
      id: String(row.id),
      identity_kind: String(row.identity_kind),
      normalized_identity: String(row.normalized_identity),
    }));
  write(JSON.stringify({ format: EXPORT_FORMAT, exported_at: now, repos }));

  const sourcesOf = db.prepare(
    'SELECT citation_kind, citation_value, source_agent FROM memory_sources WHERE memory_id = ? ORDER BY id',
  );
  const counts = { memories: 0, tombstones: 0 };
  for (const row of db.prepare(`SELECT ${MEMORY_COLUMNS} FROM memories ORDER BY created_at, id`).all()) {
    const sources = sourcesOf.all(String(row.id));
    // Tombstones and secret rows travel as hashes: identical content is still recognized on the
    // other side (FR-035, FR-020) and no secret text leaves this machine.
    const withoutText = row.deleted_at !== null || row.sensitivity === 'secret';
    write(
      JSON.stringify({
        ...row,
        title: withoutText ? '' : row.title,
        body: withoutText ? '' : row.body,
        concepts: withoutText ? '[]' : row.concepts,
        source_agent: withoutText ? null : (sources[0]?.source_agent ?? null),
        sources: withoutText ? [] : sources,
      }),
    );
    if (row.deleted_at === null) counts.memories += 1;
    else counts.tombstones += 1;
  }
  return counts;
}

function stricter(a: Sensitivity, b: Sensitivity): Sensitivity {
  return SENSITIVITY_RANK[a] >= SENSITIVITY_RANK[b] ? a : b;
}

class Rejection extends Error {}

/**
 * Applies one validated line inside the import transaction. Throws Rejection for a line the file
 * cannot carry (hash mismatch, unknown repository), which rolls the whole import back.
 */
function applyLine(
  db: DatabaseSync,
  line: ExportLine,
  repoOf: (fileRepoId: string) => string | null,
  counts: ImportResult,
): void {
  const repoId = repoOf(line.repo_id);
  if (repoId === null) {
    throw new Rejection(
      `repository ${line.repo_id} is not known here; map it with --map-repo ${line.repo_id}=<local repository id>`,
    );
  }
  const title = line.title ?? '';
  const body = line.body ?? '';
  const hasText = title !== '' || body !== '';
  if (hasText && materialHash(title, body) !== line.material_hash) {
    throw new Rejection('material_hash does not match the title and body');
  }
  // A secret row travels as its hashes only (FR-020): text under that label is not ours to store.
  if (line.sensitivity === 'secret' && (hasText || line.sources.length > 0 || (line.concepts ?? '[]') !== '[]')) {
    throw new Rejection('a secret row must carry no title, body, concepts or sources');
  }
  // Identity is recomputed here from the local repository and never taken from the file.
  const content = contentHash(repoId, line.material_hash);
  const id = memoryIdFor(content);
  const existing = db
    .prepare('SELECT id, sensitivity, deleted_at FROM memories WHERE content_hash = ?')
    .get(content) as { id: string; sensitivity: Sensitivity; deleted_at: number | null } | undefined;
  const tombstone = line.deleted_at !== null;

  if (existing !== undefined) {
    if (existing.deleted_at !== null) {
      // FR-035: a deleted memory never comes back, whatever the file says.
      counts.unchanged += 1;
      return;
    }
    if (tombstone) {
      db.prepare('UPDATE memories SET deleted_at = ? WHERE id = ?').run(line.deleted_at, existing.id);
      counts.tombstones += 1;
      return;
    }
    const merged = stricter(existing.sensitivity, line.sensitivity);
    if (merged !== existing.sensitivity) {
      db.prepare('UPDATE memories SET sensitivity = ? WHERE id = ?').run(merged, existing.id);
      counts.updated += 1;
    } else {
      counts.unchanged += 1;
    }
    return;
  }

  // R12: an active row lands quarantined; a tombstone keeps only its hashes and its time.
  const sensitivity = tombstone ? line.sensitivity : stricter('local_only', line.sensitivity);
  db.prepare(
    `INSERT INTO memories (${MEMORY_COLUMNS}, cjk_bigrams)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'imported', ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?)`,
  ).run(
    id,
    repoId,
    line.type,
    tombstone ? '' : title,
    tombstone ? '' : body,
    line.concepts,
    line.material_hash,
    content,
    sensitivity,
    line.degraded_reason,
    line.source_session_id,
    line.source_batch_id,
    line.valid_from,
    line.valid_to,
    tombstone ? null : line.pinned_at,
    tombstone ? null : line.pin_order,
    line.deleted_at,
    line.created_at,
    tombstone ? '' : cjkBigrams(`${title} ${body}`),
  );
  const insertSource = db.prepare(
    `INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, citation_value, source_agent)
     VALUES (?, NULL, ?, ?, ?)`,
  );
  for (const source of line.sources) {
    insertSource.run(id, source.citation_kind, source.citation_value, source.source_agent);
  }
  if (tombstone) counts.tombstones += 1;
  else counts.inserted += 1;
}

/**
 * Reads `oboete-export/1` text and applies it as one unit: a rejected line, or `--dry-run`, rolls
 * everything back, so the database is either fully imported or untouched. The caller bounds the
 * text (runImport reads at most MAX_FILE_BYTES before opening the database).
 */
export function importMemories(db: DatabaseSync, source: string, options: ImportOptions): ImportResult {
  const result: ImportResult = { applied: false, inserted: 0, updated: 0, tombstones: 0, unchanged: 0, rejected: [] };
  const maxFileBytes = options.maxFileBytes ?? MAX_FILE_BYTES;
  const reject = (line: number, reason: string): void => {
    result.rejected.push({ line, reason });
  };
  const mapRepo = new Map(Object.entries(options.mapRepo ?? {}));
  const localRepos = new Set(db.prepare('SELECT id FROM repos').all().map((row) => String(row.id)));
  const fileRepos = new Map<string, { identity_kind: string; normalized_identity: string }>();
  const repoOf = (fileRepoId: string): string | null => {
    const mapped = mapRepo.get(fileRepoId);
    if (mapped !== undefined) return localRepos.has(mapped) ? mapped : null;
    if (localRepos.has(fileRepoId)) return fileRepoId;
    const known = fileRepos.get(fileRepoId);
    // A remote identity means the same repository on every machine; a machine-local one needs
    // the developer to say which repository here it is (--map-repo).
    if (known?.identity_kind !== 'remote') return null;
    if (sha256Hex(known.normalized_identity).slice(0, 16) !== fileRepoId) return null;
    db.prepare(
      `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
       VALUES (?, 'remote', ?, ?, ?, ?)`,
    ).run(fileRepoId, known.normalized_identity, known.normalized_identity, options.now, options.now);
    localRepos.add(fileRepoId);
    return fileRepoId;
  };

  for (const [old, current] of mapRepo) {
    if (!localRepos.has(current)) reject(0, `--map-repo ${old}=${current}: no repository ${current} here`);
  }

  db.exec('BEGIN IMMEDIATE');
  let committed = false;
  try {
    let number = 0;
    let bytes = 0;
    let headerSeen = false;
    for (const raw of source.split('\n')) {
      // The physical source line, so a rejection names the line the developer sees in the file.
      number += 1;
      const size = Buffer.byteLength(raw, 'utf8') + 1;
      bytes += size;
      if (bytes > maxFileBytes) {
        reject(number, `file size exceeds ${Math.floor(maxFileBytes / (1024 * 1024))} MB; the import stopped here`);
        break;
      }
      if (result.rejected.length >= MAX_REJECTED) {
        reject(number, `more than ${MAX_REJECTED} lines were rejected; the import stopped here`);
        break;
      }
      const line = raw.replace(/\r$/u, '');
      if (line.trim() === '') continue;
      if (size > MAX_LINE_BYTES + 1) {
        reject(number, `line exceeds ${MAX_LINE_BYTES / 1024} KB`);
        continue;
      }
      let parsed: unknown;
      try {
        parsed = JSON.parse(line);
      } catch {
        reject(number, 'not valid JSON');
        continue;
      }
      if (!headerSeen) {
        const header = headerSchema.safeParse(parsed);
        if (!header.success) {
          reject(number, `the first line must be an ${EXPORT_FORMAT} header`);
          break;
        }
        headerSeen = true;
        for (const repo of header.data.repos) fileRepos.set(repo.id, repo);
        continue;
      }
      const memory = lineSchema.safeParse(parsed);
      if (!memory.success) {
        reject(number, z.prettifyError(memory.error).split('\n')[0] ?? 'invalid line');
        continue;
      }
      try {
        applyLine(db, memory.data, repoOf, result);
      } catch (error) {
        if (!(error instanceof Rejection)) throw error;
        reject(number, error.message);
      }
    }
    if (!headerSeen && result.rejected.length === 0) reject(0, `the file is empty; expected an ${EXPORT_FORMAT} header`);
    if (result.rejected.length > 0) {
      // The file is applied as a whole: a rejected line means none of it was written.
      result.inserted = result.updated = result.tombstones = result.unchanged = 0;
    } else if (options.dryRun !== true) {
      db.exec('COMMIT');
      committed = true;
      result.applied = true;
    }
  } finally {
    if (!committed && db.isTransaction) db.exec('ROLLBACK');
  }
  return result;
}

type Io = { writeOut(text: string): void; writeError(text: string): void };

function processIo(): Io {
  return { writeOut: (t) => process.stdout.write(t), writeError: (t) => process.stderr.write(t) };
}

/** The whole stream as one string, or null once it exceeds `limit` bytes (reading stops there). */
async function readBounded(input: NodeJS.ReadableStream, limit: number): Promise<string | null> {
  const chunks: Buffer[] = [];
  let bytes = 0;
  for await (const chunk of input) {
    const buffer = typeof chunk === 'string' ? Buffer.from(chunk, 'utf8') : chunk;
    bytes += buffer.length;
    if (bytes > limit) return null;
    chunks.push(buffer);
  }
  return Buffer.concat(chunks).toString('utf8');
}

function withDatabase<T>(fn: (db: DatabaseSync) => T | Promise<T>): Promise<T> {
  const paths = oboetePaths(resolveHome());
  ensureDirectories(paths);
  const opened = openDatabase({ path: paths.db, timeoutMs: 2_000 });
  return Promise.resolve()
    .then(() => fn(opened.db))
    .finally(() => opened.db.close());
}

function plural(count: number, noun: string, plural = `${noun}s`): string {
  return `${count} ${count === 1 ? noun : plural}`;
}

/** `oboete export [file|-]`: the file, or stdout for `-` and when no file is named. */
export async function runExport(argv: string[], io: Io = processIo()): Promise<number> {
  let target: string;
  try {
    const { positionals } = parseArgs({ args: argv, allowPositionals: true, strict: true, options: {} });
    if (positionals.length > 1) throw new Error('export takes at most one file argument.');
    target = positionals[0] ?? '-';
  } catch (error) {
    io.writeError(`${error instanceof Error ? error.message : String(error)}\n`);
    return 2;
  }
  return await withDatabase(async (db) => {
    if (target === '-') {
      const counts = exportMemories(db, (line) => io.writeOut(`${line}\n`), Date.now());
      io.writeError(`Exported ${plural(counts.memories, 'memory', 'memories')} and ${plural(counts.tombstones, 'tombstone')}.\n`);
      return 0;
    }
    const lines: string[] = [];
    const counts = exportMemories(db, (line) => lines.push(line), Date.now());
    // The file carries private and local-only text: owner-only whether it is new or reused
    // (`mode` only applies when writeFileSync creates the file).
    writeFileSync(target, `${lines.join('\n')}\n`, { mode: 0o600 });
    chmodSync(target, 0o600);
    io.writeOut(`Exported ${plural(counts.memories, 'memory', 'memories')} and ${plural(counts.tombstones, 'tombstone')} to ${target}.\n`);
    return 0;
  });
}

/** `oboete import [file|-] [--dry-run] [--map-repo <old>=<current>]`: exit 2 on an invalid file. */
export async function runImport(argv: string[], io: Io = processIo()): Promise<number> {
  let file: string;
  let dryRun: boolean;
  const mapRepo: Record<string, string> = {};
  try {
    const { values, positionals } = parseArgs({
      args: argv,
      allowPositionals: true,
      strict: true,
      options: { 'dry-run': { type: 'boolean' }, 'map-repo': { type: 'string', multiple: true } },
    });
    if (positionals.length > 1) throw new Error('import takes at most one file argument.');
    file = positionals[0] ?? '-';
    dryRun = values['dry-run'] === true;
    for (const mapping of values['map-repo'] ?? []) {
      const [old, current, ...rest] = mapping.split('=');
      if (!old || !current || rest.length > 0) throw new Error('--map-repo takes <old-id>=<current-id>.');
      mapRepo[old] = current;
    }
  } catch (error) {
    io.writeError(`${error instanceof Error ? error.message : String(error)}\n`);
    return 2;
  }
  // Every input is read to a bounded string before the database is opened: a slow pipe or a file
  // that grows after this check never holds the write lock, and the bound applies to what was read.
  if (file !== '-') {
    try {
      if (statSync(file).size > MAX_FILE_BYTES) {
        io.writeError(`${file} exceeds ${MAX_FILE_BYTES / (1024 * 1024)} MB; nothing was imported.\n`);
        return 2;
      }
    } catch {
      io.writeError(`${file} could not be read.\n`);
      return 2;
    }
  }
  const source = await readBounded(file === '-' ? process.stdin : createReadStream(file), MAX_FILE_BYTES);
  if (source === null) {
    io.writeError(`${file === '-' ? 'standard input' : file} exceeds ${MAX_FILE_BYTES / (1024 * 1024)} MB; nothing was imported.\n`);
    return 2;
  }
  return await withDatabase(async (db) => {
    const result = importMemories(db, source, { now: Date.now(), dryRun, mapRepo });
    for (const item of result.rejected) io.writeError(`line ${item.line}: ${item.reason}\n`);
    const summary = `${plural(result.inserted, 'memory', 'memories')} added, ${result.updated} raised in sensitivity, ${plural(result.tombstones, 'tombstone')} applied, ${result.unchanged} unchanged`;
    if (result.rejected.length > 0) {
      io.writeError(`Nothing was imported: ${plural(result.rejected.length, 'line')} rejected.\n`);
      return 2;
    }
    io.writeOut(dryRun ? `Dry run: ${summary} would be written.\n` : `Imported: ${summary}.\n`);
    return 0;
  });
}
