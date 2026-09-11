import type { DatabaseSync, SQLInputValue, SQLOutputValue } from 'node:sqlite';
import { posix, win32 } from 'node:path';

import { contentHash, materialHash, memoryIdFor } from '../db/identity.js';
import { grantVisibility, type MemoryRow } from '../db/queries.js';
import { compareCodeUnits, sha256Hex, sha256Json } from '../hash.js';
import { rejectsDirectives } from '../observer/classify.js';
import type { DetectorInput, DetectorResult } from '../privacy/detect.js';
import { readSourcePrivacy, type SourceContext } from '../privacy/provenance.js';
import { canonicalContexts, memoryContexts, verifiedRepoContext } from '../privacy/source-context.js';
import { cjkBigrams } from '../retrieval/fts.js';
import { migrationPayloadRedaction, migrationPayloadShape } from '../transfer-format.js';
import { assertLease, transactionImmediate } from './lease.js';
import { runtimeStateGet, runtimeStateSet } from './purge.js';

const MAX_BYTES = 2 * 1024 * 1024;
const MAX_ROWS = 500;
const MAX_FIELDS = 4_096;
const TEXT_FIELDS = new Set(['title', 'body', 'concepts', 'citation_value', 'source_agent', 'evidence',
  'capture_root', 'source_paths_json', 'root', 'repo_secret_paths_json', 'purpose', 'candidate_title',
  'candidate_body', 'text', 'subtitle', 'narrative', 'facts', 'request', 'investigated', 'learned',
  'completed', 'next_steps', 'notes', 'prompt_text', 'user_prompt', 'custom_title', 'files_read',
  'files_modified', 'files_edited', 'query', 'project']);
const JSON_LIST_FIELDS = new Set(['concepts', 'facts', 'source_paths_json', 'repo_secret_paths_json',
  'source_event_ids_json', 'files_read', 'files_modified', 'files_edited']);
type Row = Record<string, SQLOutputValue>;
type Unit = { memoryId: string | null; repoId: string; importId: string | null; owned: boolean };
type Snapshot = { memory: MemoryRow | null; sources: Row[]; grants: Row[]; records: Row[]; stamp: string };
type Options = { home: string; env: NodeJS.ProcessEnv; deadline?: number };

function relativeInside(root: string, path: string): string | null {
  const api = /^[A-Za-z]:[\\/]|^\\\\/u.test(root) ? win32 : posix;
  if (!api.isAbsolute(root) || !api.isAbsolute(path)) return null;
  const relative = api.relative(root, path);
  return relative !== '..' && !relative.startsWith(`..${api.sep}`) && !api.isAbsolute(relative)
    ? relative.replaceAll('\\', '/') : null;
}

/** Bound the complete compared snapshot, including evidence, before parsing held JSON. */
function snapshot(db: DatabaseSync, unit: Unit): Snapshot | null {
  let bytes = 0;
  const take = (sql: string, values: SQLInputValue[]) => {
    const rows: Row[] = [];
    for (const row of db.prepare(sql).iterate(...values)) {
      bytes += Buffer.byteLength(JSON.stringify(row));
      if (rows.length === MAX_ROWS || bytes > MAX_BYTES) return null;
      rows.push(row);
    }
    return rows;
  };
  const memories = take('SELECT * FROM memories WHERE id = ?', [unit.memoryId]);
  const sources = unit.owned ? take('SELECT * FROM memory_sources WHERE memory_id = ? ORDER BY id', [unit.memoryId]) : [];
  const grants = unit.owned ? take('SELECT * FROM memory_visibility WHERE memory_id = ? ORDER BY id', [unit.memoryId]) : [];
  const records = take(`SELECT * FROM migration_records WHERE destination_memory_id IS ? AND destination_repo_id = ?
    AND ${unit.owned ? "effect = 'inserted'" : "effect <> 'inserted' AND first_import_id = ?"} ORDER BY id`,
  unit.owned ? [unit.memoryId, unit.repoId] : [unit.memoryId, unit.repoId, unit.importId]);
  if (memories === null || sources === null || grants === null || records === null) return null;
  const memory = (memories[0] ?? null) as MemoryRow | null;
  if (unit.owned && (memory === null || memory.review_state !== 'imported' || memory.deleted_at !== null)) return null;
  const value = { memory, sources, grants, records };
  return { ...value, stamp: sha256Hex(JSON.stringify(value)) };
}

/** Reuse the detector's field mapping; do not sanitize a JSON envelope as one prose string. */
function fieldsOf(value: unknown) {
  const fields: string[] = [];
  const indices = new Map<string, number>();
  const names = new Set<string>();
  const references: { parent: object; key: string; index: number; mutable: boolean }[] = [];
  const jsonFields: { parent: object; key: string; value: string[] }[] = [];
  const paths = new Set<string>();
  const absolutePaths = new Set<string>();
  const anchoredPaths = new Set<string>();
  const pending = [{ value, mutable: false }];
  const register = (parent: object, key: string, text: string, mutable: boolean): boolean => {
    if (references.length === 50_000) return false;
    let index = indices.get(text);
    if (index === undefined) {
      if (fields.length === MAX_FIELDS) return false;
      index = fields.length;
      indices.set(text, index);
      fields.push(text);
    }
    references.push({ parent, key, index, mutable });
    return true;
  };
  let nodes = 0;
  while (pending.length > 0) {
    const entry = pending.pop()!;
    const node = entry.value;
    if (node === null || typeof node !== 'object') continue;
    if (++nodes > 100_000) return null;
    const record = node as Record<string, unknown>;
    const addPath = (path: string) => {
      paths.add(path);
      if (posix.isAbsolute(path) || /^[A-Za-z]:[\\/]|^\\\\/u.test(path)) absolutePaths.add(path);
      if (typeof record.capture_root !== 'string') return;
      const relative = relativeInside(record.capture_root, path);
      if (relative === null) return;
      anchoredPaths.add(path);
      paths.add(relative);
    };
    if (record.citation_kind === 'file_read' || record.citation_kind === 'file_modified') {
      if (typeof record.citation_value === 'string') addPath(record.citation_value);
    }
    for (const [key, original] of Object.entries(record)) {
      let part = original;
      if (!Array.isArray(node)) names.add(key);
      if (JSON_LIST_FIELDS.has(key) && typeof part === 'string') {
        try { part = JSON.parse(part); } catch { return null; }
        if (!Array.isArray(part) || !part.every((value) => typeof value === 'string')) return null;
        Object.defineProperty(node, key, { value: part, enumerable: true, writable: true, configurable: true });
        jsonFields.push({ parent: node, key, value: part as string[] });
      }
      if (['source_paths_json', 'files_read', 'files_modified', 'files_edited'].includes(key) && part !== null) {
        let list: unknown = part;
        try { if (typeof part === 'string') list = JSON.parse(part); } catch { return null; }
        if (!Array.isArray(list) || !list.every((path) => typeof path === 'string')) return null;
        for (const path of list as string[]) addPath(path);
      }
      const mutable = Array.isArray(node) ? entry.mutable : TEXT_FIELDS.has(key);
      if (typeof part !== 'string') {
        if (part !== null && typeof part === 'object') pending.push({ value: part, mutable });
        continue;
      }
      if (!register(node, key, part, mutable)) return null;
    }
  }
  const originalPaths: string[] = [];
  const sanitizedPaths: string[] = [];
  const registeredPaths = new Set<string>();
  const addPaths = (values: string[]): boolean => {
    for (const value of values) {
      if (registeredPaths.has(value)) continue;
      if (!register(sanitizedPaths, String(sanitizedPaths.length), value, true)) return false;
      registeredPaths.add(value);
      originalPaths.push(value);
      sanitizedPaths.push(value);
    }
    originalPaths.sort(compareCodeUnits);
    return true;
  };
  if (!addPaths([...paths].sort(compareCodeUnits))) return null;
  return { fields, names: [...names].sort(compareCodeUnits).join('\n'), paths: originalPaths, sanitizedPaths, addPaths,
    unanchoredPaths: [...absolutePaths].filter((path) => !anchoredPaths.has(path)),
    immutableChanged: (texts: string[]) => references.some((reference) => !reference.mutable && texts[reference.index] !== fields[reference.index]),
    directive: references.some((reference) => rejectsDirectives(fields[reference.index]) !== null),
    replace: (texts: string[]) => {
      for (const reference of references) Object.defineProperty(reference.parent, reference.key,
        { value: texts[reference.index], enumerable: true, writable: true, configurable: true });
      for (const field of jsonFields) Object.defineProperty(field.parent, field.key,
        { value: JSON.stringify(field.value), enumerable: true, writable: true, configurable: true });
    } };
}

function policyFor(db: DatabaseSync, unit: Unit, current: Snapshot, paths: string[], options: Options) {
  const location = { repoId: unit.repoId, home: options.home, bindingId: null };
  let contexts: SourceContext[] | null = [];
  const selected = new Set<string>();
  for (const row of current.records) {
    if (typeof row.destination_context_id !== 'string' || typeof row.destination_context_key !== 'string') { contexts = null; break; }
    const key = String(row.destination_context_id);
    if (selected.has(key)) continue;
    selected.add(key);
    const context = db.prepare('SELECT root, local_key FROM work_contexts WHERE id = ? AND repo_id = ?').get(key, unit.repoId);
    if (context?.local_key !== row.destination_context_key) { contexts = null; break; }
    contexts.push({ root: String(context.root), contextId: key, paths });
  }
  if (current.records.length === 0 && current.memory !== null) {
    // Legacy imports lack receipts. They still need their retained proof or one verified context.
    contexts = memoryContexts(db, current.memory);
    if (contexts?.length === 1 && contexts[0].root === null) {
      const context = verifiedRepoContext(db, unit.repoId);
      contexts = context === null ? null : [{ root: context.root, contextId: context.id, paths }];
    } else if (contexts !== null) contexts = contexts.map((context) => ({ ...context, paths: [...(context.paths ?? []), ...paths] }));
  }
  contexts = contexts === null || contexts.length === 0 ? null : canonicalContexts(contexts);
  try {
    const base = readSourcePrivacy(db, location, null, options.env);
    if (base === null) return null;
    const policies = contexts?.map((context) => readSourcePrivacy(db, location, context, options.env)) ?? [];
    if (policies.some((policy) => policy === null)) contexts = null;
    const valid = policies.filter((policy) => policy !== null);
    return { contexts, policies: [base, ...valid], stamp: sha256Json([base.stamp, contexts, policies.map((policy) => policy?.stamp)]) };
  } catch { return null; }
}

function clearRecords(db: DatabaseSync, records: Row[], code: string): void {
  const update = db.prepare("UPDATE migration_records SET payload_json = NULL, classification_state = 'secret', detail_code = ? WHERE id = ?");
  for (const record of records) update.run(code, record.id);
}

function retire(db: DatabaseSync, id: string, now: number, secret: boolean): void {
  db.prepare(`UPDATE memories SET deleted_at = COALESCE(deleted_at, ?), title = '', body = '',
    concepts = '[]', cjk_bigrams = '', provenance_complete = 0,
    sensitivity = CASE WHEN ? THEN 'secret' ELSE sensitivity END WHERE id = ?`).run(now, secret ? 1 : 0, id);
  db.prepare('UPDATE memory_sources SET source_agent = NULL WHERE memory_id = ?').run(id);
}

function releaseMemory(db: DatabaseSync, current: Snapshot, clean: { title: string; body: string; concepts: string | null },
  contexts: SourceContext[], now: number): { id: string; attach: boolean } {
  const memory = current.memory!;
  const personal = current.records.some((record) => record.identity_domain === 'personal_projection');
  const changed = clean.title !== memory.title || clean.body !== memory.body;
  const material = materialHash(clean.title, clean.body);
  const content = personal ? sha256Json(['personal-projection-v1', clean.title, clean.body]) : contentHash(memory.repo_id, material);
  const id = changed ? memoryIdFor(content) : memory.id;
  if (id !== memory.id) {
    const existing = db.prepare('SELECT id, deleted_at, sensitivity FROM memories WHERE content_hash = ?').get(content);
    if (existing !== undefined) {
      // The convergent row owns its scope, text and sources; classification changes only our hold.
      db.prepare("UPDATE migration_records SET destination_memory_id = ?, effect = ? WHERE destination_memory_id = ?")
        .run(existing.id, existing.deleted_at === null && existing.sensitivity !== 'secret' ? 'matched_existing' : 'held_by_tombstone', memory.id);
      if (existing.deleted_at !== null || existing.sensitivity === 'secret') {
        db.prepare(`UPDATE migration_records SET payload_json = NULL,
          classification_state = CASE WHEN classification_state = 'secret' THEN 'secret' ELSE 'not_applicable' END,
          detail_code = 'sanitized_tombstone' WHERE destination_memory_id = ?`).run(existing.id);
      }
      retire(db, memory.id, now, false);
      db.prepare('DELETE FROM memory_sources WHERE memory_id = ?').run(memory.id);
      return { id: String(existing.id), attach: false };
    }
    db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, concepts, material_hash, content_hash,
      sensitivity, review_state, degraded_reason, valid_from, valid_to, created_at, source_captured_at,
      pinned_at, pin_order, provenance_complete, cjk_bigrams)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'imported', ?, ?, ?, ?, ?, ?, ?, 0, ?)`)
      .run(id, memory.repo_id, memory.type, clean.title, clean.body, clean.concepts, material, content,
        memory.sensitivity, memory.degraded_reason, memory.valid_from, memory.valid_to, memory.created_at,
        memory.source_captured_at ?? null, memory.pinned_at, memory.pin_order, cjkBigrams(`${clean.title} ${clean.body}`));
    db.prepare('UPDATE memory_sources SET memory_id = ? WHERE memory_id = ?').run(id, memory.id);
    db.prepare('UPDATE migration_records SET destination_memory_id = ? WHERE destination_memory_id = ?').run(id, memory.id);
    for (const grant of current.grants) {
      if (grant.grant_kind !== 'migration') continue;
      if (grant.audience === 'project') grantVisibility(db, id, { audience: 'project', repoId: memory.repo_id }, 'migration', now);
      else if (grant.audience === 'work' && typeof grant.work_id === 'string') {
        grantVisibility(db, id, { audience: 'work', repoId: memory.repo_id, workId: grant.work_id }, 'migration', now);
      }
    }
    db.prepare('DELETE FROM memory_visibility WHERE memory_id = ?').run(memory.id);
    retire(db, memory.id, now, false);
  }
  db.prepare(`UPDATE memories SET title = ?, body = ?, concepts = ?, review_state = 'unreviewed',
    cjk_bigrams = ?, provenance_complete = 1 WHERE id = ?`)
    .run(clean.title, clean.body, clean.concepts, cjkBigrams(`${clean.title} ${clean.body}`), id);
  if (!personal) {
    db.prepare(`DELETE FROM memory_sources WHERE memory_id = ? AND context_only = 1
      AND raw_event_id IS NULL AND source_memory_id IS NULL AND citation_kind IS NULL`).run(id);
    const insert = db.prepare(`INSERT INTO memory_sources
      (memory_id, capture_root, source_paths_json, source_context_id, context_only) VALUES (?, ?, ?, ?, 1)`);
    for (const context of contexts) insert.run(id, context.root, JSON.stringify(context.paths), context.contextId);
  }
  return { id, attach: true };
}

async function classifyUnit(db: DatabaseSync, unit: Unit, token: string, now: () => number,
  detect: (input: DetectorInput) => Promise<DetectorResult>, options: Options): Promise<boolean> {
  const current = snapshot(db, unit);
  if (current === null) return false;
  const memory = current.memory;
  const clean = { title: String(memory?.title ?? ''), body: String(memory?.body ?? ''), concepts: memory?.concepts ?? null };
  const payloads = current.records.map((record) => record.payload_json === null ? null : JSON.parse(String(record.payload_json)) as unknown);
  const terminalPayload = payloads.some((payload) => migrationPayloadRedaction(payload) !== null);
  if (!terminalPayload && payloads.some((payload) => payload !== null && !migrationPayloadShape(payload))) return false;
  const sources = structuredClone(current.sources);
  const fields = fieldsOf({ memory: unit.owned ? clean : null, payloads, sources });
  if (fields === null) return false;
  let policy = policyFor(db, unit, current, fields.paths, options);
  if (policy === null) return false;
  const pathCount = fields.paths.length;
  if (!fields.addPaths(policy.contexts?.flatMap((context) => context.paths ?? []) ?? [])) return false;
  if (fields.paths.length !== pathCount) policy = policyFor(db, unit, current, fields.paths, options);
  if (policy === null) return false;
  const pathsComplete = fields.unanchoredPaths.every((path) => policy!.contexts?.every((context) =>
    context.root !== null && relativeInside(context.root, path) !== null) === true);
  let secret = current.records.some((record) => record.classification_state === 'secret')
    || terminalPayload || (unit.owned && memory?.sensitivity === 'secret');
  let sanitized: string[] | undefined;
  let sameSanitized = true;
  for (const part of policy.policies) {
    const result = await detect({ ...part.detector, text: fields.names, fields: fields.fields });
    if (!result.ok) return false;
    secret ||= result.sensitivity === 'secret' || result.text !== fields.names;
    if (result.sensitivity !== 'secret' && result.texts.length !== fields.fields.length) return false;
    if (result.sensitivity !== 'secret') secret ||= fields.immutableChanged(result.texts);
    if (sanitized === undefined) sanitized = result.texts;
    else sameSanitized &&= result.texts.length === sanitized.length && result.texts.every((text, index) => text === sanitized![index]);
  }
  secret ||= fields.directive || rejectsDirectives(fields.names) !== null;
  if (!secret && (!pathsComplete || policy.contexts === null
    || !sameSanitized || sanitized === undefined)) return false;
  if (!secret) fields.replace(sanitized!);
  if (!secret && payloads.some((payload) => payload !== null && !migrationPayloadShape(payload))) return false;
  const proof = policy.contexts?.map((context) => ({ ...context, paths: [...new Set(fields.sanitizedPaths)].sort(compareCodeUnits) })) ?? null;
  const checkedPolicy = policy;
  return transactionImmediate(db, () => {
    if (!assertLease(db, token, now())) { db.exec('ROLLBACK'); return true; }
    const fresh = snapshot(db, unit);
    if (fresh?.stamp !== current.stamp || policyFor(db, unit, fresh, fields.paths, options)?.stamp !== checkedPolicy.stamp) return false;
    if (secret) {
      clearRecords(db, current.records, 'local_secret_or_directive');
      if (unit.owned) retire(db, unit.memoryId!, now(), true);
      return false;
    }
    const released = unit.owned ? releaseMemory(db, current, clean, proof!, now()) : null;
    const update = db.prepare(`UPDATE migration_records SET payload_json = ?, classification_state = 'clean',
      detail_code = 'classified_local' WHERE id = ? AND payload_json IS NOT NULL AND classification_state IN ('pending', 'clean')`);
    for (const [index, record] of current.records.entries()) {
      if (payloads[index] !== null) update.run(JSON.stringify(payloads[index]), record.id);
    }
    if (released?.attach) {
      const source = db.prepare(`UPDATE memory_sources SET citation_value = ?, source_agent = ?, evidence = ?,
        capture_root = ?, source_paths_json = ? WHERE id = ? AND memory_id = ?`);
      for (const row of sources) source.run(row.citation_value, row.source_agent, row.evidence,
        row.capture_root, row.source_paths_json, row.id, released.id);
    }
    return false;
  });
}

/** Bounded local classification; a persisted cursor prevents failing rows from starving later ones. */
export async function reclassifyImported(db: DatabaseSync, token: string, now: () => number,
  detect: (input: DetectorInput) => Promise<DetectorResult>, options: Options): Promise<{ examined: number; leaseLost: boolean }> {
  type Cursor = { phase: 'memory' | 'holds'; after: string };
  let cursor: Cursor = { phase: 'memory', after: '' };
  try {
    const saved: unknown = JSON.parse(runtimeStateGet(db, 'migration_classification_cursor') ?? 'null');
    if (saved !== null && typeof saved === 'object' && 'phase' in saved && 'after' in saved
      && (saved.phase === 'memory' || saved.phase === 'holds') && typeof saved.after === 'string' && saved.after.length <= 512) {
      cursor = { phase: saved.phase, after: saved.after };
    }
  } catch { /* A malformed private cursor only restarts the idempotent scan. */ }
  const initial = JSON.stringify(cursor);
  let examined = 0;
  scan: while (examined < 100 && now() < (options.deadline ?? Infinity)) {
    const owned = cursor.phase === 'memory';
    const limit = Math.min(50, 100 - examined);
    const rows = owned ? db.prepare(`SELECT id, repo_id FROM memories
      WHERE review_state = 'imported' AND deleted_at IS NULL AND id > ? ORDER BY id LIMIT ?`).all(cursor.after, limit)
      : db.prepare(`SELECT MIN(id) AS id, first_import_id, destination_memory_id, destination_repo_id
      FROM migration_records WHERE effect <> 'inserted' AND destination_repo_id IS NOT NULL
      GROUP BY first_import_id, destination_memory_id, destination_repo_id
      HAVING MIN(id) > ? AND SUM(classification_state = 'pending') > 0 ORDER BY id LIMIT ?`).all(cursor.after, limit);
    if (rows.length === 0) {
      cursor = { phase: owned ? 'holds' : 'memory', after: '' };
      if (!owned) break;
      continue;
    }
    for (const row of rows) {
      if (now() >= (options.deadline ?? Infinity)) break scan;
      const unit: Unit = owned ? { memoryId: String(row.id), repoId: String(row.repo_id), importId: null, owned }
        : { memoryId: typeof row.destination_memory_id === 'string' ? row.destination_memory_id : null,
          repoId: String(row.destination_repo_id), importId: String(row.first_import_id), owned };
      examined += 1;
      if (await classifyUnit(db, unit, token, now, detect, options)) return { examined, leaseLost: true };
      cursor.after = String(row.id);
    }
  }
  const value = JSON.stringify(cursor);
  const leaseLost = (examined > 0 || value !== initial) && transactionImmediate(db, () => {
    if (!assertLease(db, token, now())) { db.exec('ROLLBACK'); return true; }
    runtimeStateSet(db, 'migration_classification_cursor', value, now());
    return false;
  });
  return { examined, leaseLost };
}
