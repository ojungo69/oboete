import type { DatabaseSync } from 'node:sqlite';
import type { NearbyCandidate } from '../db/queries.js';
import { payloadOf, toolPaths, type RawEventRow } from '../worker/batches.js';
import { resolveRepoIdentity } from '../repo-identity.js';
import { compareCodeUnits } from '../hash.js';

type PrivacyContext = { root: string; paths: string[]; contextId: string | null };
const MAX_PRIVACY_BYTES = 2 * 1024 * 1024;

/** A stored root is usable only for its current Git generation; ambiguity never picks recency. */
export function verifiedRepoContext(db: DatabaseSync, repoId: string, explicit?: string) {
  let selected: { id: string; root: string; local_key: string } | null = null;
  let examined = 0;
  for (const row of db.prepare(`SELECT id, root, local_key FROM work_contexts WHERE repo_id = ?
    AND (? IS NULL OR id = ?) ORDER BY id LIMIT 101`).iterate(repoId, explicit ?? null, explicit ?? null)) {
    examined += 1;
    try {
      const identity = resolveRepoIdentity(String(row.root));
      if (identity.id !== repoId || identity.worktreeKey !== row.local_key) continue;
      if (selected !== null) return null;
      selected = { id: String(row.id), root: String(row.root), local_key: String(row.local_key) };
    } catch { /* Missing/unverifiable roots cannot select import policy. */ }
  }
  return examined === 101 ? null : selected;
}

export function canonicalContexts(contexts: SourceContext[]): PrivacyContext[] | null {
  const roots = new Map<string, PrivacyContext>();
  for (const context of contexts) {
    if (context.root === null || context.root === '' || context.paths === null) return null;
    const key = JSON.stringify([context.root, context.contextId]);
    const prior = roots.get(key);
    roots.set(key, { root: context.root, contextId: context.contextId, paths: [...new Set([...(prior?.paths ?? []), ...context.paths])].sort(compareCodeUnits) });
    if (roots.size > 50) return null;
  }
  const result = [...roots].sort(([left], [right]) => left.localeCompare(right))
    .map(([, context]) => context);
  return Buffer.byteLength(JSON.stringify(result)) > MAX_PRIVACY_BYTES ? null : result;
}

/** Current privacy metadata only: a long work history never causes a request-time ancestor walk. */
export function retainedContexts(db: DatabaseSync, id: string): PrivacyContext[] | null {
  const contexts: PrivacyContext[] = [];
  let bytes = 0;
  for (const row of db.prepare(`SELECT capture_root, source_paths_json, source_context_id FROM memory_sources
    WHERE memory_id = ? AND raw_event_id IS NULL AND source_memory_id IS NULL AND citation_kind IS NULL LIMIT 51`).iterate(id)) {
    if (contexts.length === 50 || typeof row.capture_root !== 'string' || row.capture_root === ''
      || typeof row.source_paths_json !== 'string') return null;
    bytes += Buffer.byteLength(row.capture_root) + Buffer.byteLength(row.source_paths_json);
    if (bytes > MAX_PRIVACY_BYTES) return null;
    let paths: unknown;
    try { paths = JSON.parse(row.source_paths_json); } catch { return null; }
    if (!Array.isArray(paths) || !paths.every((path) => typeof path === 'string')) return null;
    contexts.push({ root: row.capture_root, paths, contextId: typeof row.source_context_id === 'string' ? row.source_context_id : null });
  }
  if (contexts.length === 0) return null;
  const citations = db.prepare(`SELECT citation_value FROM memory_sources WHERE memory_id = ?
    AND context_only = 0 AND citation_kind IN ('file_read', 'file_modified') LIMIT 101`).all(id);
  if (citations.length > 100 || citations.some((row) => typeof row.citation_value !== 'string')) return null;
  for (const context of contexts) context.paths.push(...citations.map((row) => String(row.citation_value)));
  return canonicalContexts(contexts);
}

export type SourceContext = { root: string | null; paths: string[] | null; contextId: string | null };
function pathList(value: unknown): string[] | null {
  return Array.isArray(value) && value.every((path) => typeof path === 'string') ? value as string[] : null;
}

/** Old payloads carry cwd, but a repo-wide last-seen path never establishes a source's origin. */
export function sourceContext(db: DatabaseSync, row: RawEventRow): SourceContext {
  const payload = payloadOf(row);
  const root = typeof payload?.capture_root === 'string' ? payload.capture_root
    : typeof payload?.cwd === 'string' ? payload.cwd : null;
  let paths = pathList(payload?.source_paths) ?? pathList(payload?.paths);
  if (paths === null && row.kind === 'tool_call') paths = toolPaths(row);
  if (paths === null && (row.kind === 'tool_result' || row.kind === 'tool_failure')) {
    const call = db.prepare(`SELECT payload_json FROM raw_events WHERE session_id = ? AND kind = 'tool_call'
      AND CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.tool_call_id') = ? ELSE 0 END LIMIT 1`)
      .get(row.session_id, typeof payload?.tool_call_id === 'string' ? payload.tool_call_id : null);
    if (call !== undefined) paths = toolPaths(call as unknown as RawEventRow);
  } else if (paths === null) paths = [];
  const context = db.prepare('SELECT context_id FROM work_bindings WHERE id = ?').get(row.work_binding_id ?? null);
  return { root, paths, contextId: typeof context?.context_id === 'string' ? context.context_id : null };
}

/** Only source metadata is read; evidence/body arrays are never materialized for this check. */
export function memoryContexts(db: DatabaseSync, candidate: Pick<NearbyCandidate, 'id' | 'work_id' | 'provenance_complete'>): SourceContext[] | null {
  if (candidate.provenance_complete === 0) return null;
  if (candidate.provenance_complete === 1) return retainedContexts(db, candidate.id);
  if (candidate.work_id !== null && candidate.work_id !== undefined) return null;
  const contexts: SourceContext[] = [];
  let bytes = 0;
  const legacy = 'ms.capture_root IS NULL AND ms.source_paths_json IS NULL';
  const sources = db.prepare(`SELECT DISTINCT ms.capture_root, ms.source_paths_json, ms.source_context_id, r.work_binding_id,
    CASE WHEN ${legacy} THEN r.payload_json END AS payload_json,
    CASE WHEN ${legacy} THEN r.session_id END AS session_id,
    CASE WHEN ${legacy} THEN r.repo_id END AS repo_id,
    CASE WHEN ${legacy} THEN r.kind END AS kind FROM memory_sources ms
    LEFT JOIN raw_events r ON r.id = ms.raw_event_id WHERE ms.memory_id = ? AND ms.context_only = 0 AND ms.raw_event_id IS NOT NULL
      AND (NOT (${legacy}) OR NOT EXISTS (SELECT 1 FROM memory_sources proof
        WHERE proof.memory_id = ms.memory_id AND proof.raw_event_id = ms.raw_event_id AND proof.context_only = 0
          AND proof.capture_root IS NOT NULL AND proof.source_paths_json IS NOT NULL))`);
  for (const source of sources.iterate(candidate.id)) {
    bytes += Buffer.byteLength(String(source.source_paths_json ?? source.payload_json ?? ''));
    if (contexts.length === 50 || bytes > 2 * 1024 * 1024) return null;
    if (source.capture_root !== null || source.source_paths_json !== null) {
      let paths: string[] | null = null;
      try { paths = pathList(JSON.parse(String(source.source_paths_json))); } catch { /* Missing legacy provenance stays unknown. */ }
      contexts.push({ root: typeof source.capture_root === 'string' ? source.capture_root : null, paths,
        contextId: typeof source.source_context_id === 'string' ? source.source_context_id : null });
    } else if (source.payload_json !== null) contexts.push(sourceContext(db, source as unknown as RawEventRow));
    else return null;
  }
  const citations = db.prepare(`SELECT citation_value FROM memory_sources WHERE memory_id = ?
    AND context_only = 0 AND citation_kind IN ('file_read', 'file_modified') LIMIT 101`).all(candidate.id);
  if (citations.length > 100) return null;
  const paths = citations.map((row) => String(row.citation_value ?? ''));
  if (contexts.length === 0) contexts.push({ root: null, paths, contextId: null });
  else for (const context of contexts) if (context.paths !== null) context.paths.push(...paths);
  return contexts;
}
