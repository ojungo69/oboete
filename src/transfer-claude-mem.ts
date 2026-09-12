import { z } from 'zod';

import { contentHash, materialHash } from './db/identity.js';
import { compareCodeUnits, sha256Hex, sha256Json } from './hash.js';
import { CLAUDE_MEM_FORMAT, CLAUDE_MEM_REVISION, type NativeMemory, type NativeSource } from './transfer-format.js';

const CLAUDE_MEM_SOURCE_FORMAT = CLAUDE_MEM_FORMAT;
const CLAUDE_MEM_SOURCE_REVISION = CLAUDE_MEM_REVISION;
export const MAX_CLAUDE_MEM_BYTES = 5 * 1024 * 1024;

const MAX_INPUT_RECORDS = 20_000;
const MAX_OUTPUT_RECORDS = 50_000;
const MAX_STRING_BYTES = 2 * 1024 * 1024;
const MAX_ID_LENGTH = 16_384;
const MAX_PROJECT_LENGTH = 16_384;
const MAX_LIST_ITEMS = 10_000;
const MAX_MEMORY_SOURCES = 50;
const MAX_MEMORY_SOURCE_BYTES = 2 * 1024 * 1024;
const MAX_PROJECTED_TITLE = 120;
const MAX_PROJECTED_BODY = 2_000;
const MAX_PROJECTED_CONCEPTS = 65_536;
const OMITTED = '\n[Imported content omitted]';

const safeInteger = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const boundedString = z.string().refine((value) => Buffer.byteLength(value) <= MAX_STRING_BYTES,
  { message: 'string_too_large' });
const idString = z.string().min(1).max(MAX_ID_LENGTH);
const nullableText = boundedString.nullable();
const nullableId = idString.nullable();
const projectString = z.string().min(1).max(MAX_PROJECT_LENGTH);

const topSchema = z.looseObject({
  exportedAt: idString,
  exportedAtEpoch: safeInteger,
  query: boundedString,
  project: projectString.optional(),
  totalObservations: safeInteger,
  totalSessions: safeInteger,
  totalSummaries: safeInteger,
  totalPrompts: safeInteger,
  observations: z.array(z.unknown()).max(MAX_INPUT_RECORDS),
  sessions: z.array(z.unknown()).max(MAX_INPUT_RECORDS),
  summaries: z.array(z.unknown()).max(MAX_INPUT_RECORDS),
  prompts: z.array(z.unknown()).max(MAX_INPUT_RECORDS),
});

const observationSchema = z.looseObject({
  id: safeInteger,
  memory_session_id: idString,
  project: projectString,
  text: nullableText,
  type: z.string().min(1).max(128),
  title: nullableText,
  subtitle: nullableText,
  facts: nullableText,
  narrative: nullableText,
  concepts: nullableText,
  files_read: nullableText,
  files_modified: nullableText,
  prompt_number: safeInteger.nullable(),
  discovery_tokens: safeInteger,
  created_at: idString,
  created_at_epoch: safeInteger,
});

const sessionSchema = z.looseObject({
  id: safeInteger,
  content_session_id: idString,
  memory_session_id: nullableId,
  project: projectString,
  user_prompt: nullableText,
  started_at: idString,
  started_at_epoch: safeInteger,
  completed_at: idString.nullable(),
  completed_at_epoch: safeInteger.nullable(),
  status: z.enum(['active', 'completed', 'failed']),
  platform_source: idString,
  custom_title: nullableText.optional(),
  model: nullableText.optional(),
  billing: nullableText.optional(),
});

const summarySchema = z.looseObject({
  id: safeInteger,
  memory_session_id: idString,
  project: projectString,
  request: nullableText,
  investigated: nullableText,
  learned: nullableText,
  completed: nullableText,
  next_steps: nullableText,
  files_read: nullableText,
  files_edited: nullableText,
  notes: nullableText,
  prompt_number: safeInteger.nullable(),
  discovery_tokens: safeInteger,
  created_at: idString,
  created_at_epoch: safeInteger,
});

const promptSchema = z.looseObject({
  id: safeInteger,
  session_db_id: safeInteger.nullable().optional(),
  content_session_id: idString,
  memory_session_id: nullableId.optional(),
  project: projectString.optional(),
  platform_source: idString.optional(),
  prompt_number: safeInteger,
  prompt_text: boundedString,
  created_at: idString,
  created_at_epoch: safeInteger,
});

const SUPPORTED_TYPES = new Set<NativeMemory['type']>([
  'bugfix', 'feature', 'refactor', 'change', 'discovery', 'decision',
]);

type JsonPrimitive = null | boolean | number | string;
export type JsonValue = JsonPrimitive | JsonValue[] | JsonObject;
export type JsonObject = { [key: string]: JsonValue };
type Observation = z.infer<typeof observationSchema>;
type Session = z.infer<typeof sessionSchema>;
type Summary = z.infer<typeof summarySchema>;

type Parsed<T> = { known: T; external: JsonObject; index: number };

export type ExternalMemoryRecord = NativeMemory & {
  external_kind: 'observation' | 'summary';
  external_payload_hash: string;
  external_payload: JsonObject;
};

type ExternalSourceRecord = NativeSource & {
  external_kind: 'session' | 'prompt';
  external_payload_hash: string;
  external_payload: JsonObject;
};

export type ExternalSupportRecord = {
  kind: 'session' | 'prompt';
  id: string;
  repo_id: string | null;
  memory_ids: readonly string[];
  source_created_at: number;
  resolution: 'resolved' | 'unresolved' | 'ambiguous' | 'not_applicable';
  detail_code: string;
  external_payload_hash: string;
  external_payload: JsonObject;
};

export type ExternalExcludedRecord = {
  kind: 'excluded';
  record_kind: 'memory';
  id: string;
  repo_id: string;
  source_created_at: number;
  detail_code: 'unsupported_type';
  external_payload_hash: string;
  external_payload: JsonObject;
};

export type ClaudeMemNormalizedRecord =
  | ExternalMemoryRecord
  | NativeSource | ExternalSourceRecord
  | ExternalSupportRecord
  | ExternalExcludedRecord;

export type ClaudeMemAdapterResult = {
  header: {
    source_format: typeof CLAUDE_MEM_SOURCE_FORMAT;
    source_revision: typeof CLAUDE_MEM_SOURCE_REVISION;
    origin_id: null;
    bytes: number;
    exported_at: number;
    query: string;
    selected_project: string | null;
    query_scoped: true;
    source_tombstones: 'unavailable';
    counts: {
      observations: number;
      sessions: number;
      summaries: number;
      prompts: number;
      projected_memories: number;
      source_records: number;
      excluded: number;
      unresolved_prompts: number;
    };
    projects: readonly { id: string; source: string }[];
    external_metadata: JsonObject;
  };
  records: readonly ClaudeMemNormalizedRecord[];
};

export class ClaudeMemAdapterError extends Error {
  constructor(
    readonly code: string,
    readonly recordKind: 'header' | 'observation' | 'session' | 'summary' | 'prompt',
    readonly index = -1,
    readonly field = '',
  ) {
    super(code);
    this.name = 'ClaudeMemAdapterError';
  }
}

function fail(code: string, kind: ClaudeMemAdapterError['recordKind'], index = -1, field = ''): never {
  throw new ClaudeMemAdapterError(code, kind, index, field);
}

function jsonObject(value: unknown, kind: ClaudeMemAdapterError['recordKind'], index: number): JsonObject {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) fail('invalid_record', kind, index);
  return value as JsonObject;
}

function rows<T>(values: readonly unknown[], schema: z.ZodType<T>, kind: ClaudeMemAdapterError['recordKind']): Parsed<T>[] {
  const ids = new Set<number>();
  return values.map((value, index) => {
    const external = jsonObject(value, kind, index);
    const parsed = schema.safeParse(value);
    if (!parsed.success) {
      const issue = parsed.error.issues[0];
      fail('invalid_field', kind, index, issue?.path.map(String).join('.') ?? '');
    }
    const rawId = (parsed.data as { id: number }).id;
    if (ids.has(rawId)) fail('duplicate_source_id', kind, index, 'id');
    ids.add(rawId);
    return { known: parsed.data, external, index };
  });
}

function assertTime(iso: string, epoch: number, kind: ClaudeMemAdapterError['recordKind'], index: number,
  field: string): void {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}(?:Z|[+-]\d{2}:\d{2})$/u.test(iso)
    || !Number.isFinite(Date.parse(iso)) || Date.parse(iso) !== epoch) fail('timestamp_mismatch', kind, index, field);
}

function typedList(value: string | null, kind: ClaudeMemAdapterError['recordKind'], index: number,
  field: string): string[] {
  if (value === null) return [];
  let parsed: unknown;
  try { parsed = JSON.parse(value); } catch { fail('invalid_typed_list', kind, index, field); }
  if (!Array.isArray(parsed) || parsed.length > MAX_LIST_ITEMS
    || !parsed.every((item) => typeof item === 'string' && Buffer.byteLength(item) <= MAX_STRING_BYTES)) {
    fail('invalid_typed_list', kind, index, field);
  }
  return parsed;
}

function payloadHash(payload: JsonObject): string {
  return sha256Hex(JSON.stringify(payload));
}

function recordId(kind: string, naturalKey: readonly unknown[], payload: JsonObject): string {
  // Payload is part of the opaque id so two rows with one documented natural key and changed
  // payload can coexist in the bounded scratch table. Raw upstream ids never escape the payload.
  return sha256Json(['claude-mem-query-record-v1', kind, naturalKey, payloadHash(payload)]);
}

export function claudeMemProjectId(project: string): string {
  return `project:${sha256Hex(project)}`;
}

function shorten(value: string, limit: number, suffix: string): string {
  if (value.length <= limit) return value;
  let end = Math.max(0, limit - suffix.length);
  // Do not manufacture an unpaired surrogate when a bounded display projection cuts Unicode text.
  if (end > 0 && end < value.length && /[\uD800-\uDBFF]/u.test(value[end - 1]!)
    && /[\uDC00-\uDFFF]/u.test(value[end]!)) end -= 1;
  return `${value.slice(0, end)}${suffix}`;
}

function observationTitle(row: Observation): string {
  const fallback = `Imported ${row.type}`;
  const title = row.title !== null && row.title !== '' ? row.title
    : row.subtitle !== null && row.subtitle !== '' ? row.subtitle : fallback;
  return shorten(title, MAX_PROJECTED_TITLE, '…');
}

function observationBody(row: Observation, facts: readonly string[]): string {
  const sections: string[] = [];
  if (row.narrative !== null && row.narrative !== '') sections.push(`Narrative:\n${row.narrative}`);
  if (row.text !== null && row.text !== '') sections.push(`Text:\n${row.text}`);
  if (facts.length > 0) sections.push(`Facts:\n${facts.map((fact) => `- ${fact}`).join('\n')}`);
  const body = sections.join('\n\n') || 'Imported observation has no text.';
  return shorten(body, MAX_PROJECTED_BODY, OMITTED);
}

function summaryBody(row: Summary): string {
  const sections: [string, string | null][] = [
    ['Request', row.request], ['Investigated', row.investigated], ['Learned', row.learned],
    ['Completed', row.completed], ['Next steps', row.next_steps], ['Notes', row.notes],
  ];
  const body = sections.filter((entry): entry is [string, string] => entry[1] !== null && entry[1] !== '')
    .map(([label, value]) => `${label}:\n${value}`).join('\n\n') || 'Imported session summary has no text.';
  return shorten(body, MAX_PROJECTED_BODY, OMITTED);
}

function projectedConcepts(concepts: readonly string[]): string {
  const exact = JSON.stringify(concepts);
  if (exact.length <= MAX_PROJECTED_CONCEPTS) return exact;
  const kept: string[] = [];
  for (const concept of concepts) {
    const next = JSON.stringify([...kept, concept, '[Imported concepts omitted]']);
    if (next.length > MAX_PROJECTED_CONCEPTS) break;
    kept.push(concept);
  }
  return JSON.stringify([...kept, '[Imported concepts omitted]']);
}

function memory(row: {
  id: string;
  repoId: string;
  type: NativeMemory['type'];
  title: string;
  body: string;
  concepts: string;
  createdAt: number;
  externalKind: ExternalMemoryRecord['external_kind'];
  external: JsonObject;
}): ExternalMemoryRecord {
  const material = materialHash(row.title, row.body);
  return {
    kind: 'memory', id: row.id, repo_id: row.repoId, type: row.type,
    title: row.title, body: row.body, concepts: row.concepts,
    material_hash: material, content_hash: contentHash(row.repoId, material),
    sensitivity: 'local_only', review_state: 'imported', degraded_reason: null,
    source_session_id: null, source_batch_id: null, source_agent: null,
    valid_from: row.createdAt, valid_to: null, superseded_by: null,
    pinned_at: null, pin_order: null, deleted_at: null, created_at: row.createdAt,
    identity_domain: 'ordinary', work_id: null, checkpoint_parent_id: null,
    provenance_complete: 0, source_captured_at: row.createdAt,
    external_kind: row.externalKind, external_payload_hash: payloadHash(row.external),
    external_payload: row.external,
  };
}

function source(input: {
  id: string;
  memoryId: string;
  citationKind: NativeSource['citation_kind'];
  citationValue: string | null;
  evidence: string | null;
  sourceAgent: string | null;
  paths: readonly string[] | null;
  capturedAt: number;
}): NativeSource {
  return {
    kind: 'source', id: input.id, memory_id: input.memoryId,
    raw_event_id: null, source_memory_id: null, source_context_id: null,
    citation_kind: input.citationKind, citation_value: input.citationValue,
    source_agent: input.sourceAgent, portion_start: null, portion_end: null, source_total: null,
    source_hash: input.evidence === null ? null : sha256Hex(input.evidence),
    evidence: input.evidence, captured_at: input.capturedAt, source_processed_at: null,
    capture_root: null, source_paths_json: input.paths === null ? null : JSON.stringify(input.paths),
    context_only: 0,
  };
}

function topMetadata(raw: JsonObject): JsonObject {
  const known = new Set([
    'exportedAt', 'exportedAtEpoch', 'query', 'project',
    'totalObservations', 'totalSessions', 'totalSummaries', 'totalPrompts',
    'observations', 'sessions', 'summaries', 'prompts',
  ]);
  return Object.fromEntries(Object.entries(raw).filter(([key]) => !known.has(key)));
}

/**
 * Converts a parsed official v13.24.5 query export into inert transfer records. The caller must
 * enforce MAX_CLAUDE_MEM_BYTES before JSON.parse; rawByteLength is checked again here. This function
 * performs no filesystem, database, provider, local-context, work, raw-event or approval operation.
 */
export function adaptClaudeMemQueryExport(input: unknown, rawByteLength: number): ClaudeMemAdapterResult {
  if (!Number.isSafeInteger(rawByteLength) || rawByteLength < 0 || rawByteLength > MAX_CLAUDE_MEM_BYTES) {
    fail('source_file_too_large', 'header');
  }
  const rawTop = jsonObject(input, 'header', -1);
  const parsedTop = topSchema.safeParse(input);
  if (!parsedTop.success) {
    const issue = parsedTop.error.issues[0];
    fail('invalid_field', 'header', -1, issue?.path.map(String).join('.') ?? '');
  }
  const top = parsedTop.data;
  const totalRows = top.observations.length + top.sessions.length + top.summaries.length + top.prompts.length;
  if (totalRows > MAX_INPUT_RECORDS) fail('too_many_source_records', 'header');
  if (top.totalObservations !== top.observations.length || top.totalSessions !== top.sessions.length
    || top.totalSummaries !== top.summaries.length || top.totalPrompts !== top.prompts.length) {
    fail('count_mismatch', 'header');
  }
  assertTime(top.exportedAt, top.exportedAtEpoch, 'header', -1, 'exportedAt');

  const observations = rows(top.observations, observationSchema, 'observation');
  const sessions = rows(top.sessions, sessionSchema, 'session');
  const summaries = rows(top.summaries, summarySchema, 'summary');
  const prompts = rows(top.prompts, promptSchema, 'prompt');

  const sessionsById = new Map<number, Parsed<Session>>();
  const sessionsByMemory = new Map<string, Parsed<Session>>();
  const sessionsByContent = new Map<string, Parsed<Session>[]>();
  const sessionKeys = new Set<string>();
  for (const item of sessions) {
    const row = item.known;
    assertTime(row.started_at, row.started_at_epoch, 'session', item.index, 'started_at');
    if ((row.completed_at === null) !== (row.completed_at_epoch === null)) {
      fail('timestamp_pair_mismatch', 'session', item.index, 'completed_at');
    }
    if (row.completed_at !== null && row.completed_at_epoch !== null) {
      assertTime(row.completed_at, row.completed_at_epoch, 'session', item.index, 'completed_at');
    }
    const sessionKey = JSON.stringify([row.platform_source, row.content_session_id]);
    if (sessionKeys.has(sessionKey)) fail('duplicate_session_identity', 'session', item.index);
    sessionKeys.add(sessionKey);
    sessionsById.set(row.id, item);
    if (row.memory_session_id !== null) {
      if (sessionsByMemory.has(row.memory_session_id)) fail('duplicate_memory_session', 'session', item.index);
      sessionsByMemory.set(row.memory_session_id, item);
    }
    const content = sessionsByContent.get(row.content_session_id) ?? [];
    content.push(item);
    sessionsByContent.set(row.content_session_id, content);
  }

  const projects = new Set<string>();
  for (const item of [...observations, ...summaries]) {
    const row = item.known;
    const kind = 'type' in row ? 'observation' : 'summary';
    assertTime(row.created_at, row.created_at_epoch, kind, item.index, 'created_at');
    const owner = sessionsByMemory.get(row.memory_session_id);
    if (owner === undefined || owner.known.project !== row.project) {
      fail('memory_session_project_mismatch', kind, item.index, 'memory_session_id');
    }
    projects.add(row.project);
  }
  for (const item of sessions) projects.add(item.known.project);

  const records: ClaudeMemNormalizedRecord[] = [];
  const outputIds = new Set<string>();
  const memoryIdsBySession = new Map<string, string[]>();
  const memoryIdsBySessionPrompt = new Map<string, string[]>();
  const sourceStats = new Map<string, { count: number; bytes: number }>();
  let projectedMemories = 0;
  let sourceRecords = 0;
  let excluded = 0;
  let unresolvedPrompts = 0;

  const append = (record: ClaudeMemNormalizedRecord): void => {
    if (records.length === MAX_OUTPUT_RECORDS) fail('too_many_normalized_records', 'header');
    const unique = `${record.kind}:${record.id}`;
    if (outputIds.has(unique)) fail('duplicate_normalized_origin', 'header');
    outputIds.add(unique);
    records.push(record);
  };
  const appendSource = (record: NativeSource | ExternalSourceRecord): void => {
    const current = sourceStats.get(record.memory_id) ?? { count: 0, bytes: 0 };
    const next = { count: current.count + 1, bytes: current.bytes + Buffer.byteLength(JSON.stringify(record)) };
    if (next.count > MAX_MEMORY_SOURCES || next.bytes > MAX_MEMORY_SOURCE_BYTES) {
      fail('memory_provenance_too_large', 'header');
    }
    sourceStats.set(record.memory_id, next);
    append(record);
    sourceRecords += 1;
  };
  const relate = (memorySession: string, promptNumber: number | null, memoryId: string): void => {
    const sessionRelated = memoryIdsBySession.get(memorySession) ?? [];
    sessionRelated.push(memoryId);
    memoryIdsBySession.set(memorySession, sessionRelated);
    if (promptNumber === null) return;
    const key = JSON.stringify([memorySession, promptNumber]);
    const related = memoryIdsBySessionPrompt.get(key) ?? [];
    related.push(memoryId);
    memoryIdsBySessionPrompt.set(key, related);
  };
  const fileSources = (memoryId: string, capturedAt: number, read: readonly string[], modified: readonly string[]): void => {
    for (const [kind, paths] of [['file_read', read], ['file_modified', modified]] as const) {
      for (const [index, path] of paths.entries()) {
        appendSource(source({
          id: sha256Json(['claude-mem-file-source-v1', memoryId, kind, index, sha256Hex(path)]),
          memoryId, citationKind: kind, citationValue: path, evidence: null, sourceAgent: null,
          paths: [path], capturedAt,
        }));
      }
    }
  };

  for (const item of observations) {
    const row = item.known;
    const externalHash = payloadHash(item.external);
    const id = recordId('observation', [row.memory_session_id, row.title, row.created_at_epoch], item.external);
    const repoId = claudeMemProjectId(row.project);
    const facts = typedList(row.facts, 'observation', item.index, 'facts');
    const concepts = typedList(row.concepts, 'observation', item.index, 'concepts');
    const read = typedList(row.files_read, 'observation', item.index, 'files_read');
    const modified = typedList(row.files_modified, 'observation', item.index, 'files_modified');
    if (!SUPPORTED_TYPES.has(row.type as NativeMemory['type'])) {
      append({ kind: 'excluded', record_kind: 'memory', id, repo_id: repoId,
        source_created_at: row.created_at_epoch, detail_code: 'unsupported_type',
        external_payload_hash: externalHash, external_payload: item.external });
      excluded += 1;
      continue;
    }
    const projected = memory({ id, repoId, type: row.type as NativeMemory['type'],
      title: observationTitle(row), body: observationBody(row, facts), concepts: projectedConcepts(concepts),
      createdAt: row.created_at_epoch, externalKind: 'observation', external: item.external });
    append(projected);
    projectedMemories += 1;
    relate(row.memory_session_id, row.prompt_number, id);
    fileSources(id, row.created_at_epoch, read, modified);
  }

  for (const item of summaries) {
    const row = item.known;
    const id = recordId('summary', [row.memory_session_id], item.external);
    const repoId = claudeMemProjectId(row.project);
    const read = typedList(row.files_read, 'summary', item.index, 'files_read');
    const edited = typedList(row.files_edited, 'summary', item.index, 'files_edited');
    const projected = memory({ id, repoId, type: 'session_summary', title: 'Imported session summary',
      body: summaryBody(row), concepts: '[]', createdAt: row.created_at_epoch,
      externalKind: 'summary', external: item.external });
    append(projected);
    projectedMemories += 1;
    relate(row.memory_session_id, row.prompt_number, id);
    fileSources(id, row.created_at_epoch, read, edited);
  }

  for (const item of sessions) {
    const row = item.known;
    const id = recordId('session', [row.platform_source, row.content_session_id], item.external);
    const memoryIds = row.memory_session_id === null ? []
      : [...(memoryIdsBySession.get(row.memory_session_id) ?? [])].sort(compareCodeUnits);
    append({ kind: 'session', id, repo_id: claudeMemProjectId(row.project), memory_ids: [...new Set(memoryIds)],
      source_created_at: row.started_at_epoch, resolution: 'not_applicable', detail_code: 'support_only',
      external_payload_hash: payloadHash(item.external), external_payload: item.external });
    // ponytail: support is retained per memory within the 50-row/2-MiB provenance bound;
    // normalize shared references only if real exports hit that storage ceiling.
    for (const memoryId of new Set(memoryIds)) {
      appendSource({ ...source({ id: sha256Json(['claude-mem-session-source-v1', memoryId, id]), memoryId,
        citationKind: null, citationValue: null, evidence: null, sourceAgent: row.platform_source,
        paths: null, capturedAt: row.started_at_epoch }), external_kind: 'session',
        external_payload_hash: payloadHash(item.external), external_payload: item.external });
    }
  }

  for (const item of prompts) {
    const row = item.known;
    assertTime(row.created_at, row.created_at_epoch, 'prompt', item.index, 'created_at');
    const contentCandidates = sessionsByContent.get(row.content_session_id) ?? [];
    let candidates = contentCandidates;
    if (row.session_db_id !== undefined && row.session_db_id !== null) {
      const direct = sessionsById.get(row.session_db_id);
      if (direct !== undefined && direct.known.content_session_id !== row.content_session_id) {
        fail('prompt_session_conflict', 'prompt', item.index, 'session_db_id');
      }
      if (direct !== undefined && ((row.platform_source !== undefined
        && row.platform_source !== direct.known.platform_source)
        || (row.project !== undefined && row.project !== direct.known.project)
        || (row.memory_session_id !== undefined && row.memory_session_id !== null
          && row.memory_session_id !== direct.known.memory_session_id))) {
        fail('prompt_session_conflict', 'prompt', item.index, 'session_db_id');
      }
      candidates = direct === undefined ? [] : [direct];
    }
    if (row.platform_source !== undefined) candidates = candidates.filter((s) => s.known.platform_source === row.platform_source);
    if (row.project !== undefined) candidates = candidates.filter((s) => s.known.project === row.project);
    if (row.memory_session_id !== undefined && row.memory_session_id !== null) {
      candidates = candidates.filter((s) => s.known.memory_session_id === row.memory_session_id);
    }
    if (contentCandidates.length > 0 && candidates.length === 0) {
      fail('prompt_session_conflict', 'prompt', item.index);
    }
    const resolved = candidates.length === 1 ? candidates[0] : undefined;
    if (resolved !== undefined && ((row.platform_source !== undefined && row.platform_source !== resolved.known.platform_source)
      || (row.project !== undefined && row.project !== resolved.known.project)
      || (row.memory_session_id !== undefined && row.memory_session_id !== null
        && row.memory_session_id !== resolved.known.memory_session_id))) {
      fail('prompt_session_conflict', 'prompt', item.index);
    }
    const contextProject = resolved?.known.project ?? row.project ?? null;
    const contextPlatform = resolved?.known.platform_source ?? row.platform_source ?? null;
    if (contextProject !== null) projects.add(contextProject);
    const id = recordId('prompt', [row.content_session_id, row.prompt_number, contextPlatform, contextProject], item.external);
    const memorySession = resolved?.known.memory_session_id ?? null;
    const memoryIds = memorySession === null ? []
      : [...new Set(memoryIdsBySessionPrompt.get(JSON.stringify([memorySession, row.prompt_number])) ?? [])]
        .sort(compareCodeUnits);
    const resolution = candidates.length === 1 ? 'resolved' : candidates.length > 1 ? 'ambiguous' : 'unresolved';
    if (resolution !== 'resolved') unresolvedPrompts += 1;
    append({ kind: 'prompt', id, repo_id: contextProject === null ? null : claudeMemProjectId(contextProject),
      memory_ids: memoryIds, source_created_at: row.created_at_epoch, resolution,
      detail_code: resolution === 'resolved' ? 'support_only' : 'support_only_unresolved',
      external_payload_hash: payloadHash(item.external), external_payload: item.external });
    if (resolved !== undefined) {
      for (const memoryId of memoryIds) {
        appendSource({ ...source({
          id: sha256Json(['claude-mem-prompt-source-v1', memoryId, id]), memoryId,
          citationKind: null, citationValue: null, evidence: row.prompt_text,
          sourceAgent: resolved.known.platform_source, paths: null, capturedAt: row.created_at_epoch,
        }), external_kind: 'prompt', external_payload_hash: payloadHash(item.external), external_payload: item.external });
      }
    }
  }

  if (top.project !== undefined) {
    for (const project of projects) if (project !== top.project) fail('selected_project_mismatch', 'header', -1, 'project');
  }

  return {
    header: {
      source_format: CLAUDE_MEM_SOURCE_FORMAT,
      source_revision: CLAUDE_MEM_SOURCE_REVISION,
      origin_id: null,
      bytes: rawByteLength,
      exported_at: top.exportedAtEpoch,
      query: top.query,
      selected_project: top.project ?? null,
      query_scoped: true,
      source_tombstones: 'unavailable',
      counts: {
        observations: observations.length, sessions: sessions.length,
        summaries: summaries.length, prompts: prompts.length,
        projected_memories: projectedMemories, source_records: sourceRecords,
        excluded, unresolved_prompts: unresolvedPrompts,
      },
      projects: [...projects].sort(compareCodeUnits).map((source) => ({ id: claudeMemProjectId(source), source })),
      external_metadata: topMetadata(rawTop),
    },
    records,
  };
}
