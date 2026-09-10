// `oboete export` / `oboete import` (spec FR-036, contracts/cli.md, data-model.md "Export line",
// research R12). Security-owned: the file carries no secret text (secret rows and tombstones travel
// as hashes), an imported row can only raise a sensitivity and never lowers one, a tombstone wins
// in both directions, and every active imported row is quarantined as `local_only` /
// `review_state = imported` until the worker classifies it. Nothing here is on the hook path.
import { closeSync, createReadStream, existsSync, mkdtempSync, openSync, realpathSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { once } from 'node:events';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { parseArgs } from 'node:util';
import { Readable } from 'node:stream';

import { DatabaseMissingError, SchemaAheadError, openDatabase } from './db/open.js';
import { sha256Hex } from './hash.js';
import { ensureDirectories, oboetePaths, resolveHome } from './paths.js';

import { EXPORT_FORMAT, MAX_LINE_BYTES, MAX_FILE_BYTES, type ImportResult, type ImportOptions } from './transfer-format.js';
import { readTransferPlan, TransferInputError, type TransferPlan } from './transfer-plan.js';
import { mergeTransferPlan } from './transfer-merge.js';
import { runImportPromote } from './transfer-promote.js';
import { NATIVE_FORMAT, NATIVE_REVISION, MAX_NATIVE_LINE_BYTES, nativeRecordSchema } from './transfer-format.js';
export { EXPORT_FORMAT } from './transfer-format.js';

export type { ImportResult, ImportOptions } from './transfer-format.js';

const MEMORY_COLUMNS = `id, repo_id, type, title, body, concepts, material_hash, content_hash, sensitivity,
  review_state, degraded_reason, source_session_id, source_batch_id, valid_from, valid_to,
  superseded_by, pinned_at, pin_order, deleted_at, created_at`;

/** Writes the header and one line per memory of every repository; returns what was written. */
export function exportMemories(
  db: DatabaseSync,
  write: (line: string) => void,
  now: number,
): { memories: number; tombstones: number } {
  const repos = [];
  let repoBytes = 0;
  for (const row of db.prepare('SELECT id, identity_kind, normalized_identity FROM repos ORDER BY id').iterate()) {
    const repo = {
      id: String(row.id),
      identity_kind: String(row.identity_kind),
      normalized_identity: String(row.normalized_identity),
    };
    repoBytes += Buffer.byteLength(JSON.stringify(repo)) + 1;
    if (repoBytes > MAX_LINE_BYTES) throw new Rejection('export_header_too_large');
    repos.push(repo);
  }
  write(JSON.stringify({ format: EXPORT_FORMAT, exported_at: now, repos }));

  const sourcesOf = db.prepare(
    'SELECT citation_kind, citation_value, source_agent FROM memory_sources WHERE memory_id = ? AND context_only = 0 ORDER BY id',
  );
  const counts = { memories: 0, tombstones: 0 };
  for (const row of db.prepare(`SELECT ${MEMORY_COLUMNS} FROM memories ORDER BY created_at, id`).iterate()) {
    // Tombstones and secret rows travel as hashes: identical content is still recognized on the
    // other side (FR-035, FR-020) and no secret text leaves this machine.
    const withoutText = row.deleted_at !== null || row.sensitivity === 'secret';
    const sources = [];
    let sourceBytes = 0;
    if (!withoutText) for (const source of sourcesOf.iterate(String(row.id))) {
      sourceBytes += Buffer.byteLength(JSON.stringify(source)) + 1;
      if (sourceBytes > MAX_LINE_BYTES) throw new Rejection('export_record_too_large');
      sources.push(source);
    }
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

function exportNative(db: DatabaseSync, write: (line: string) => void, now: number) {
  const emit = (record: unknown): void => {
    if (!nativeRecordSchema.safeParse(record).success) throw new Rejection('invalid_export_record');
    write(JSON.stringify(record));
  };
  const originId = db.prepare('SELECT origin_id FROM replica_identity WHERE id = 1').get()?.origin_id;
  if (typeof originId !== 'string') throw new Rejection('missing_origin_identity');
  write(JSON.stringify({ format: NATIVE_FORMAT, revision: NATIVE_REVISION, origin_id: originId, exported_at: now }));
  for (const row of db.prepare('SELECT id, identity_kind, normalized_identity FROM repos ORDER BY id').iterate()) {
    emit({ kind: 'repo', ...row });
  }
  const counts = { memories: 0, tombstones: 0 };
  const personal = db.prepare(`SELECT 1 FROM memory_visibility WHERE memory_id = ? AND audience = 'personal'
    UNION SELECT 1 FROM migration_records WHERE destination_memory_id = ? AND identity_domain = 'personal_projection' LIMIT 1`);
  for (const row of db.prepare(`SELECT ${MEMORY_COLUMNS}, work_id, checkpoint_parent_id,
    provenance_complete, source_captured_at FROM memories ORDER BY created_at, id`).iterate()) {
    const withoutText = row.deleted_at !== null || row.sensitivity === 'secret';
    emit({ kind: 'memory', ...row, identity_domain: personal.get(row.id, row.id) ? 'personal_projection' : 'ordinary',
      title: withoutText ? '' : row.title, body: withoutText ? '' : row.body,
      concepts: withoutText ? '[]' : row.concepts, source_agent: null });
    if (row.deleted_at === null) counts.memories += 1;
    else counts.tombstones += 1;
  }
  for (const row of db.prepare(`SELECT s.*, m.deleted_at AS parent_deleted_at, m.sensitivity AS parent_sensitivity
    FROM memory_sources s JOIN memories m ON m.id = s.memory_id ORDER BY s.id`).iterate()) {
    const { parent_deleted_at, parent_sensitivity, ...source } = row;
    const redacted = parent_deleted_at !== null || parent_sensitivity === 'secret';
    emit({ kind: 'source', ...source, id: String(row.id), evidence: redacted ? null : row.evidence,
      citation_value: redacted ? null : row.citation_value, source_agent: redacted ? null : row.source_agent,
      capture_root: redacted ? null : row.capture_root, source_paths_json: redacted ? null : row.source_paths_json });
  }
  for (const row of db.prepare('SELECT * FROM work_contexts ORDER BY id').iterate()) emit({ kind: 'context', ...row, redacted: false });
  for (const row of db.prepare('SELECT * FROM work_items ORDER BY id').iterate()) {
    const redacted = row.purpose_sensitivity === 'secret';
    emit({ kind: 'work', ...row, purpose: redacted ? null : row.purpose, redacted });
  }
  for (const row of db.prepare('SELECT * FROM memory_visibility ORDER BY id').iterate()) emit({ kind: 'visibility', ...row });
  for (const row of db.prepare(`SELECT p.*, m.deleted_at AS origin_deleted_at, m.sensitivity AS origin_sensitivity,
    projected.deleted_at AS projection_deleted_at, projected.sensitivity AS projection_sensitivity
    FROM sharing_proposals p JOIN memories m ON m.id = p.origin_memory_id
    LEFT JOIN memories projected ON projected.id = p.projected_memory_id ORDER BY p.id`).iterate()) {
    const { origin_deleted_at, origin_sensitivity, projection_deleted_at, projection_sensitivity, ...proposal } = row;
    const redacted = origin_deleted_at !== null || origin_sensitivity === 'secret' || row.candidate_sensitivity === 'secret'
      || projection_deleted_at !== null || projection_sensitivity === 'secret';
    emit({ kind: 'sharing_proposal', ...proposal, redacted,
      candidate_title: redacted ? '' : row.candidate_title, candidate_body: redacted ? '' : row.candidate_body,
        source_event_ids_json: redacted ? '[]' : row.source_event_ids_json });
  }
  for (const row of db.prepare(`SELECT r.*, m.deleted_at AS parent_deleted_at, m.sensitivity AS parent_sensitivity
    FROM migration_records r LEFT JOIN memories m ON m.id = r.destination_memory_id ORDER BY r.id`).iterate()) {
    const redacted = row.parent_deleted_at !== null || row.parent_sensitivity === 'secret'
      || row.classification_state === 'secret' || row.payload_json === null;
    const payload = redacted ? null : JSON.parse(String(row.payload_json)) as Record<string, unknown>;
    emit({ kind: 'migration_origin', id: row.origin_key, origin_json: row.origin_json, payload_hash: row.payload_hash,
      stored_payload_hash: payload === null ? null : sha256Hex(JSON.stringify(payload)), payload,
      record_kind: row.record_kind, repo_id: row.destination_repo_id, memory_id: row.destination_memory_id,
      classification_state: row.classification_state });
  }
  return counts;
}

class Rejection extends Error {}

/** The in-process test/embedding seam shares the streaming CLI planner and merge. */
export async function importMemories(db: DatabaseSync, source: string, options: ImportOptions): Promise<ImportResult> {
  let plan: TransferPlan | undefined;
  try {
    if (Buffer.byteLength(source) > (options.maxFileBytes ?? MAX_FILE_BYTES)) throw new TransferInputError('file size exceeds the import limit');
    plan = await readTransferPlan(Readable.from([Buffer.from(source)]), options.from);
    return mergeTransferPlan(db, plan, options);
  } catch (error) {
    if (!(error instanceof TransferInputError)) throw error;
    return { applied: false, inserted: 0, updated: 0, tombstones: 0, unchanged: 0,
      rejected: [{ line: error.line, reason: error.reason }] };
  } finally { plan?.close(); }
}

type Io = { writeOut(text: string): void | Promise<void>; writeError(text: string): void };

function processIo(): Io {
  return { writeOut: async (text) => {
    if (!process.stdout.write(text)) await once(process.stdout, 'drain');
  }, writeError: (t) => { process.stderr.write(t); } };
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

function assertExportTarget(target: string): void {
  if (target === '-') return;
  const physical = (path: string) => existsSync(path) ? realpathSync(path)
    : existsSync(dirname(path)) ? join(realpathSync(dirname(path)), basename(path)) : resolve(path);
  const output = physical(resolve(target));
  const database = physical(oboetePaths(resolveHome()).db);
  for (const suffix of ['', '-wal', '-shm', '-journal']) {
    const path = `${database}${suffix}`;
    if (output === path) throw new Rejection('export_target_is_database');
    if (existsSync(output) && existsSync(path)) {
      const destination = statSync(output);
      const source = statSync(path);
      if (destination.dev === source.dev && destination.ino === source.ino) throw new Rejection('export_target_is_database');
    }
  }
}

/** `oboete export [file|-]`: the file, or stdout for `-` and when no file is named. */
export async function runExport(argv: string[], io: Io = processIo()): Promise<number> {
  let target: string;
  let format: '1' | '2';
  try {
    const { values, positionals } = parseArgs({ args: argv, allowPositionals: true, strict: true,
      options: { format: { type: 'string' } } });
    if (positionals.length > 1) throw new Error('export takes at most one file argument.');
    if (values.format !== undefined && values.format !== '1' && values.format !== '2') throw new Error('unsupported_export_format');
    format = values.format ?? '2';
    target = positionals[0] ?? '-';
    assertExportTarget(target);
  } catch (error) {
    io.writeError(`${error instanceof Error ? error.message : String(error)}\n`);
    return 2;
  }
  return await withDatabase(async (db) => {
    const directory = mkdtempSync(join(target === '-' ? tmpdir() : dirname(resolve(target)), '.oboete-export-'));
    const temporary = join(directory, 'memories.jsonl');
    let descriptor: number | null = null;
    try {
      descriptor = openSync(temporary, 'wx', 0o600);
      let bytes = 0;
      const write = (line: string): void => {
        const size = Buffer.byteLength(line, 'utf8');
        bytes += size + 1;
        if (size > (format === '1' ? MAX_LINE_BYTES : MAX_NATIVE_LINE_BYTES)) throw new Rejection('export_record_too_large');
        if (bytes > MAX_FILE_BYTES) throw new Rejection('export_file_too_large');
        writeFileSync(descriptor!, `${line}\n`);
      };
      const version = db.prepare('PRAGMA data_version').get()?.data_version;
      db.exec('BEGIN');
      let counts;
      try { counts = (format === '1' ? exportMemories : exportNative)(db, write, Date.now()); }
      finally { db.exec('ROLLBACK'); }
      closeSync(descriptor);
      descriptor = null;
      if (format === '2') {
        try {
          const validation = await readTransferPlan(createReadStream(temporary));
          validation.close();
        } catch { throw new Rejection('invalid_export_graph'); }
      }
      if (db.prepare('PRAGMA data_version').get()?.data_version !== version) throw new Rejection('export_changed_retry');
      assertExportTarget(target);
      if (target === '-') {
        for await (const chunk of createReadStream(temporary, { encoding: 'utf8' })) await io.writeOut(chunk);
        io.writeError(`Exported ${plural(counts.memories, 'memory', 'memories')} and ${plural(counts.tombstones, 'tombstone')}.\n`);
      } else {
        renameSync(temporary, resolve(target));
        await io.writeOut(`Exported ${plural(counts.memories, 'memory', 'memories')} and ${plural(counts.tombstones, 'tombstone')} to ${target}.\n`);
      }
      return 0;
    } catch (error) {
      io.writeError(`${error instanceof Rejection ? error.message : 'export_failed'}\n`);
      return 2;
    } finally {
      if (descriptor !== null) closeSync(descriptor);
      rmSync(directory, { recursive: true, force: true });
    }
  });
}

type ImportArgs = { file: string; dryRun: boolean; apply: boolean; json: boolean;
  mapRepo: Record<string, string>; mapWork: Record<string, string>; mapContext: Record<string, string>;
  from?: 'claude-mem'; mapProject: Record<string, string>; mapProjectHash: Record<string, string> };

function importArgs(argv: string[]): ImportArgs | { error: string } {
  try {
    const { values, positionals } = parseArgs({ args: argv, allowPositionals: true, strict: true,
      options: { 'dry-run': { type: 'boolean' }, apply: { type: 'boolean' }, json: { type: 'boolean' },
        'map-repo': { type: 'string', multiple: true }, 'map-work': { type: 'string', multiple: true },
        'map-context': { type: 'string', multiple: true }, from: { type: 'string' },
        'map-project': { type: 'string', multiple: true }, 'map-project-hash': { type: 'string', multiple: true } } });
    if (positionals.length > 1 || (values.apply && values['dry-run'])) throw new Error('invalid_import_arguments');
    if (values.from !== undefined && values.from !== 'claude-mem') throw new Error('unsupported_source_format');
    if (values.from === 'claude-mem' ? values['map-repo'] !== undefined || values['map-work'] !== undefined
      : values['map-project'] !== undefined || values['map-project-hash'] !== undefined) throw new Error('invalid_mapping_format');
    const mapping = (entries: string[] | undefined, limit = 512): Record<string, string> => {
      const result: Record<string, string> = Object.create(null) as Record<string, string>;
      if ((entries?.length ?? 0) > 1000) throw new Error('too_many_mappings');
      for (const entry of entries ?? []) {
        const split = entry.lastIndexOf('=');
        const old = entry.slice(0, split);
        const current = entry.slice(split + 1);
        if (split < 1 || old.length > limit || current === '' || current.length > 512 || Object.hasOwn(result, old)) {
          throw new Error('invalid_import_mapping');
        }
        result[old] = current;
      }
      return result;
    };
    return { file: positionals[0] ?? '-', dryRun: values['dry-run'] === true, apply: values.apply === true,
      json: values.json === true, mapRepo: mapping(values['map-repo']), mapWork: mapping(values['map-work']),
      mapContext: mapping(values['map-context']), from: values.from,
      mapProject: mapping(values['map-project'], 16_384), mapProjectHash: mapping(values['map-project-hash']) };
  } catch { return { error: 'invalid_import_arguments_or_mapping' }; }
}

function importSummary(result: ImportResult): string {
  return `${plural(result.inserted, 'memory', 'memories')} added, ${result.updated} raised in sensitivity, ${plural(result.tombstones, 'tombstone')} applied, ${result.unchanged} unchanged`;
}

/** Only identities and counts leave the private plan; exact project names remain inside it. */
function previewMetadata(plan: TransferPlan, result: ImportResult, schema: string) {
  const counts: Record<string, number> = Object.create(null) as Record<string, number>;
  for (const row of plan.db.prepare('SELECT kind, COUNT(*) AS n FROM transfer_rows GROUP BY kind').iterate()) {
    counts[String(row.kind)] = Number(row.n);
  }
  const projects = plan.db.prepare(`SELECT data, destination_repo_id, destination_context_id
    FROM transfer_rows WHERE kind = 'repo' ORDER BY origin LIMIT 100`).all().map((row) => {
    const repo = JSON.parse(String(row.data)) as { id: string; normalized_identity: string };
    return { sourceHash: plan.external === undefined ? sha256Hex(repo.id) : sha256Hex(repo.normalized_identity),
      destinationRepo: row.destination_repo_id, context: row.destination_context_id };
  });
  const collisions = Number(plan.db.prepare(`SELECT COUNT(*) AS n FROM (
    SELECT destination_repo_id FROM transfer_rows WHERE kind = 'repo' AND destination_repo_id IS NOT NULL
    GROUP BY destination_repo_id HAVING COUNT(*) > 1)`).get()?.n ?? 0);
  return { source: { format: plan.format, revision: plan.revision, sha256: plan.sourceHash, bytes: plan.bytes,
    counts: plan.external?.counts ?? counts, queryScoped: plan.external !== undefined,
    tombstones: plan.external === undefined ? 'included' : 'unavailable' },
    mapping: { projects, omitted: Math.max(0, (counts.repo ?? 0) - projects.length), collisions },
    quarantine: result.inserted, excluded: counts.excluded ?? 0,
    held: Number(plan.db.prepare("SELECT COUNT(*) AS n FROM transfer_rows WHERE kind <> 'repo' AND kind <> 'memory'").get()?.n ?? 0),
    applyPossible: schema === 'ready' && result.rejected.length === 0 };
}

/** Input is staged before any destination open; previews never create or migrate that store. */
export async function runImport(argv: string[], io: Io = processIo()): Promise<number> {
  if (argv[0] === 'promote') return runImportPromote(argv.slice(1), io);
  const args = importArgs(argv);
  if ('error' in args) { io.writeError(`${args.error}\n`); return 2; }
  let plan: TransferPlan | undefined;
  let db: DatabaseSync | undefined;
  try {
    plan = await readTransferPlan(args.file === '-' ? process.stdin : createReadStream(args.file), args.from);
    const dryRun = args.dryRun || (plan.format !== EXPORT_FORMAT && !args.apply);
    const paths = oboetePaths(resolveHome());
    let schema: 'ready' | 'missing' | 'behind' | 'ahead' = 'ready';
    if (dryRun || plan.format !== EXPORT_FORMAT) {
      try {
        const opened = openDatabase({ path: paths.db, timeoutMs: 2_000, readOnly: true });
        db = opened.db;
        if (opened.schemaBehind) { schema = 'behind'; db.close(); db = undefined; }
      } catch (error) {
        if (error instanceof DatabaseMissingError) schema = 'missing';
        else if (error instanceof SchemaAheadError) schema = 'ahead';
        else throw error;
      }
    }
    if (!dryRun && schema === 'ready') {
      db?.close();
      ensureDirectories(paths);
      db = openDatabase({ path: paths.db, timeoutMs: 2_000 }).db;
    }
    const result = mergeTransferPlan(db, plan, { now: Date.now(), dryRun,
      mapRepo: args.mapRepo, mapWork: args.mapWork, mapContext: args.mapContext,
      mapProject: args.mapProject, mapProjectHash: args.mapProjectHash });
    if (!dryRun && schema !== 'ready') result.rejected.unshift({ line: 0, reason: 'destination_schema_not_ready' });
    const metadata = previewMetadata(plan, result, schema);
    if (args.json) await io.writeOut(`${JSON.stringify({ ...metadata, destinationSchema: schema, ...result })}\n`);
    else {
      for (const issue of result.rejected) io.writeError(`line ${issue.line}: ${issue.reason}\n`);
      if (result.rejected.length === 0) await io.writeOut(dryRun
        ? `Dry run: ${importSummary(result)} would be written. Destination schema: ${schema}.\n`
        : `${result.duplicate ? 'Already imported' : 'Imported'}${plan.format === EXPORT_FORMAT && !args.apply ? ' (v1 implicit apply)' : ''}: ${importSummary(result)}.\n`);
      if (plan.external !== undefined) await io.writeOut(`Source: ${plan.format}@${plan.revision}, SHA-256 ${plan.sourceHash}, ${plan.bytes} bytes.\n`
        + `Query-scoped export; source deletion history is unavailable. ${metadata.mapping.collisions} project collisions.\n`
        + `${metadata.excluded} unsupported, ${metadata.held} held records. Apply possible: ${metadata.applyPossible}.\n`
        + metadata.mapping.projects.map((project) => `Project SHA-256 ${project.sourceHash}: ${project.destinationRepo === null ? 'mapping required' : 'mapped'}.\n`).join('')
        + (metadata.mapping.omitted > 0 ? `${metadata.mapping.omitted} project details omitted.\n` : ''));
    }
    return result.rejected.length > 0 ? 2 : 0;
  } catch (error) {
    io.writeError(`${error instanceof TransferInputError ? `line ${error.line}: ${error.reason}` : 'import_failed'}\n`);
    return 2;
  } finally { db?.close(); plan?.close(); }
}
