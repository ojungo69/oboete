// Completed-run measurement and verdicts, separate from replay execution and report serialization.
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

import { CAPTURE_DEADLINE_MS } from '../capture.js';
import type { openDatabase } from '../db/open.js';
import type { oboetePaths } from '../paths.js';
import { isAllowed, loadDestinationRules, type Sensitivity } from '../privacy/egress.js';
import { PENDING_BOUND_MS, READY_BOUND_MS, fileBytes, ms, pendingSentence, percentile, recallRateOf, renderReport, statusOf, timingRows, type BoundRow } from './replay-report.js';
import type { Agent, DeliveredFactItem, Line, MeasureInput, RecallHit, RecallProbe, Sample } from './replay.js';

const WORKER_RSS_BOUND_KB = 150 * 1024;
const RECALL_BOUND = 0.9;

export type FactStage = { status: 'pass' | 'fail' | 'pending' | 'partial' | 'not_run'; reason: string;
  ids?: string[]; sources?: { id: string; total: number | null; ranges: number[][] }[] };
export type FactTrace = RecallHit & {
  factSeq: number; querySeq: number; availability: 'current_delivery' | 'prior_delivery' | 'missing';
  stages: Record<'capture' | 'coverage' | 'application' | 'retention' | 'retrieval' | 'delivery' | 'answer', FactStage>;
  firstFailure: string | null;
};

const OMISSIONS = new Set(['below_threshold', 'budget', 'duplicate_in_conversation', 'stale_path',
  'stale_commit', 'retired', 'mmr_redundant', 'not_delivered', 'secret_detected', 'directive']);

function factItems(db: ReturnType<typeof openDatabase>['db'], probe: RecallProbe, injectionIds: string[]) {
  if (probe.conversationId === null || probe.epoch === null || injectionIds.length === 0) return [];
  return db.prepare(`SELECT ii.injection_id AS injectionId, ii.memory_id AS memoryId, ii.raw_event_id AS rawEventId,
    ii.decision, ii.reason, i.state, i.kind, i.delivery_count AS deliveries
    FROM injection_items ii JOIN injections i ON i.id = ii.injection_id
    JOIN sessions s ON s.id = i.session_id AND s.repo_id = i.repo_id
    WHERE i.repo_id = ? AND i.conversation_id = ? AND i.context_epoch = ?
      AND ii.conversation_id = i.conversation_id AND ii.context_epoch = i.context_epoch
      AND i.id IN (SELECT value FROM json_each(?)) AND (
        ii.raw_event_id IN (SELECT value FROM json_each(?)) OR EXISTS (
          SELECT 1 FROM memories m JOIN memory_sources ms ON ms.memory_id = m.id
          WHERE m.id = ii.memory_id AND m.repo_id = i.repo_id AND ms.context_only = 0
            AND ms.raw_event_id IN (SELECT value FROM json_each(?))
            AND instr(COALESCE(m.title, '') || char(10) || COALESCE(m.body, ''), ?) > 0))
    ORDER BY i.created_at, ii.id LIMIT 100`)
    .all(probe.repoId, probe.conversationId, probe.epoch, JSON.stringify(injectionIds),
      JSON.stringify(probe.factSourceIds), JSON.stringify(probe.factSourceIds), probe.expect);
}

/** Called before the query as well as after the run: a pending Grok record is never prior delivery. */
export function deliveredFactItems(
  db: ReturnType<typeof openDatabase>['db'], probe: RecallProbe, injectionIds: string[],
): DeliveredFactItem[] {
  return factItems(db, probe, injectionIds).filter((row) => row.decision === 'included' && row.state === 'emitted'
    && (row.kind !== 'grok_deferred' || Number(row.deliveries) > 0))
    .map((row) => ({ injectionId: String(row.injectionId), memoryId: row.memoryId === null ? null : String(row.memoryId),
      rawEventId: row.rawEventId === null ? null : String(row.rawEventId) }));
}

function sourceStages(db: ReturnType<typeof openDatabase>['db'], probe: RecallProbe): Pick<FactTrace['stages'], 'capture' | 'coverage' | 'application'> {
  const sources = db.prepare(`SELECT id, classification_state, sensitivity, processing_state FROM raw_events
    WHERE repo_id = ? AND session_id = ? AND id IN (SELECT value FROM json_each(?))`)
    .all(probe.repoId, probe.sourceSessionId, JSON.stringify(probe.sourceIds));
  const receipts = db.prepare(`SELECT r.* FROM observation_batch_sources r
    JOIN observation_batches b ON b.id = r.batch_id
    JOIN sessions s ON s.id = b.session_id AND s.repo_id = b.repo_id
    WHERE b.repo_id = ? AND b.session_id = ? AND r.raw_event_id IN (SELECT value FROM json_each(?))
    ORDER BY r.recorded_at DESC, b.rowid DESC LIMIT 500`)
    .all(probe.repoId, probe.sourceSessionId, JSON.stringify(probe.factSourceIds));
  const captured = sources.filter((row) => probe.factSourceIds.includes(String(row.id))
    && row.classification_state !== 'failed' && row.sensitivity !== 'secret' && row.processing_state !== 'excluded');
  const capture: FactStage = { status: captured.length > 0 ? 'pass' : 'fail',
    reason: captured.length > 0 ? 'accepted' : sources.some((row) => row.sensitivity === 'secret' || row.processing_state === 'excluded')
      ? 'excluded' : sources.some((row) => row.classification_state === 'failed') ? 'classification_failed' : 'source_missing',
    ids: probe.sourceIds.slice(0, 50) };
  // Retained membership proves acceptance after ordinary raw retention has elapsed.
  if (sources.length === 0 && receipts.length > 0) { capture.status = 'pass'; capture.reason = 'retained_receipt'; }
  const ranges = probe.factSourceIds.slice(0, 50).map((id) => {
    const rows = receipts.filter((row) => row.raw_event_id === id);
    const latest = rows.find((row) => typeof row.source_total === 'number');
    const total = latest === undefined ? null : Number(latest.source_total);
    const spans = rows.filter((row) => typeof row.source_hash === 'string' && row.source_hash !== ''
      && row.source_hash === latest?.source_hash && row.source_total === total
      && typeof row.portion_start === 'number' && typeof row.portion_end === 'number'
      && row.portion_start >= 0 && row.portion_end > row.portion_start && row.portion_end <= Number(total))
      .map((row) => [Number(row.portion_start), Number(row.portion_end)]).sort((a, b) => a[0] - b[0]);
    const merged: number[][] = [];
    for (const span of spans) {
      const previous = merged.at(-1);
      if (previous !== undefined && span[0] <= previous[1]) previous[1] = Math.max(previous[1], span[1]);
      else merged.push(span);
    }
    return { id, total, ranges: merged };
  });
  const full = ranges.some((row) => row.total !== null && row.total > 0 && row.ranges.length === 1
    && row.ranges[0][0] === 0 && row.ranges[0][1] === row.total);
  const partial = ranges.some((row) => row.ranges.length > 0);
  const coverage: FactStage = { status: full ? 'pass' : partial ? 'partial' : 'pending',
    reason: full ? 'complete_range' : partial ? 'partial_range' : receipts.some((row) => row.reason === 'not_sent') ? 'not_sent' : 'no_range',
    sources: ranges };
  const latest = probe.factSourceIds.map((id) => receipts.find((row) => row.raw_event_id === id)).filter((row) => row !== undefined);
  const processed = latest.find((row) => row.outcome === 'processed');
  const application: FactStage = processed === undefined
    ? { status: latest.some((row) => row.outcome === 'rejected') ? 'fail' : 'pending',
      reason: latest.some((row) => row.reason === 'unaccounted') ? 'unaccounted'
        : latest.some((row) => row.outcome === 'rejected') ? 'rejected'
          : latest.some((row) => row.outcome === 'deferred') ? 'deferred'
            : latest.some((row) => row.outcome === 'legacy_unknown') ? 'legacy_unknown' : 'unprocessed' }
    : { status: 'pass', reason: processed.reason === 'no_memory' ? 'no_memory' : 'accounted' };
  return { capture, coverage, application };
}

export function evaluateRecall(db: ReturnType<typeof openDatabase>['db'], probe: RecallProbe): FactTrace {
  const rules = loadDestinationRules(db);
  const memories = db.prepare(`SELECT DISTINCT m.id, m.deleted_at, m.valid_to, m.degraded_reason, m.sensitivity, m.review_state
    FROM memories m JOIN memory_sources ms ON ms.memory_id = m.id WHERE m.repo_id = ? AND ms.context_only = 0
      AND ms.raw_event_id IN (SELECT value FROM json_each(?))
      AND instr(COALESCE(m.title, '') || char(10) || COALESCE(m.body, ''), ?) > 0 LIMIT 100`)
    .all(probe.repoId, JSON.stringify(probe.factSourceIds), probe.expect);
  const active = memories.filter((row) => row.deleted_at === null && row.valid_to === null && row.degraded_reason === null
    && isAllowed(rules, 'injection', row.sensitivity as Sensitivity, true) && row.review_state !== 'imported');
  const items = factItems(db, probe, probe.currentInjectionIds);
  const current = probe.currentTextHit ? deliveredFactItems(db, probe, probe.currentInjectionIds) : [];
  const prior = probe.priorDelivery;
  const availability = current.length > 0 ? 'current_delivery' : prior.length > 0 ? 'prior_delivery' : 'missing';
  const omission = items.find((row) => row.decision === 'omitted');
  const retrievalPass = prior.length > 0 || items.some((row) => row.decision === 'planned' || row.decision === 'included');
  const stages: FactTrace['stages'] = { ...sourceStages(db, probe),
    retention: { status: active.length > 0 ? 'pass' : 'fail', reason: active.length > 0 ? 'retained_fact'
      : memories.some((row) => row.degraded_reason !== null) ? 'temporary_only'
        : memories.length > 0 ? 'retired_or_withheld' : 'no_linked_fact', ids: active.map((row) => String(row.id)) },
    retrieval: { status: retrievalPass ? 'pass' : 'fail', reason: prior.length > 0 ? 'prior_delivery'
      : retrievalPass ? 'selected' : OMISSIONS.has(String(omission?.reason)) ? String(omission?.reason) : 'no_candidate',
      ids: [...new Set(items.map((row) => String(row.injectionId)))] },
    delivery: { status: availability === 'missing' ? 'fail' : 'pass', reason: availability,
      ids: [...new Set([...current, ...prior].map((row) => row.injectionId))] },
    answer: { status: 'not_run', reason: 'receiving_agent_not_run' } };
  const firstFailure = Object.entries(stages).find(([, stage]) => stage.status !== 'pass' && stage.status !== 'not_run')?.[0] ?? null;
  return { id: probe.id, lang: probe.lang, query: probe.query, expect: probe.expect, factSeq: probe.factSeq,
    querySeq: probe.querySeq, hit: availability !== 'missing', availability, stages, firstFailure };
}

export function sessionStartEvent(agent: Agent, event: string): boolean {
  return agent === 'pi' ? event === 'session_start' : event === 'SessionStart';
}

export function nativeSessionId(agent: Agent, payload: unknown): string {
  if (payload === null || typeof payload !== 'object') return '';
  const row = payload as Record<string, unknown>;
  if (agent === 'grok') {
    return typeof row.sessionId === 'string' ? row.sessionId : String(row.session_id ?? '');
  }
  return typeof row.session_id === 'string' ? row.session_id : '';
}

function walkFiles(root: string): string[] {
  if (!existsSync(root)) return [];
  try {
    return readdirSync(root, { recursive: true, withFileTypes: true })
      .filter((entry) => entry.isFile())
      .map((entry) => join(entry.parentPath, entry.name));
  } catch {
    return [];
  }
}

function bufferHas(haystack: Buffer, needle: string): boolean {
  if (needle === '') return false;
  return haystack.includes(Buffer.from(needle, 'utf8'));
}

function countQuery(db: ReturnType<typeof openDatabase>['db'], sql: string, repoId: string): number {
  const row = db.prepare(sql).get(repoId) as { n?: unknown } | undefined;
  return typeof row?.n === 'number' ? row.n : 0;
}

function sessionOrder(lines: Line[]): Record<Agent, string[]> {
  const order: Record<Agent, string[]> = { claude: [], codex: [], grok: [], pi: [] };
  const seen = new Set<string>();
  for (const line of lines) {
    const key = `${line.agent}:${line.session}`;
    if (seen.has(key)) continue;
    seen.add(key);
    order[line.agent].push(line.session);
  }
  return order;
}

function neighborSession(order: string[], label: string, offset: number): string | undefined {
  const index = order.indexOf(label);
  if (index < 0) return undefined;
  return order[index + offset];
}

/** The share of samples inside a bound, and the p99 the report prints beside it. */
function shareUnder(values: number[], bound: number): { under: number; p99: number } {
  const under = values.length === 0 ? 1 : values.filter((value) => value <= bound).length / values.length;
  return { under, p99: percentile(values, 99) };
}

function maxMs(samples: Sample[]): number {
  return samples.length === 0 ? 0 : Math.max(...samples.map((sample) => sample.ms));
}

/** Every byte this run wrote anywhere, so a leaked secret is found wherever it landed. */
function writtenSurfaces(paths: ReturnType<typeof oboetePaths>, packBlob: string): Buffer[] {
  const dbBuffers = [paths.db, `${paths.db}-wal`, `${paths.db}-shm`]
    .filter((path) => existsSync(path))
    .map((path) => readFileSync(path));
  const extraFiles = [...walkFiles(paths.spool), ...walkFiles(paths.logs)].map((path) => readFileSync(path));
  return [...dbBuffers, ...extraFiles, Buffer.from(packBlob, 'utf8')];
}

/** SC-005 and FR-021: no planted secret reaches a written surface, no directive reaches a memory. */
function privacyChecks(
  db: ReturnType<typeof openDatabase>['db'],
  paths: ReturnType<typeof oboetePaths>,
  input: { maps: MeasureInput['maps']; packBlob: string },
): { leakedSecrets: string[]; leakedDirectives: string[]; negativesUnredacted: number; rawDirectiveRows: number } {
  const surfaces = writtenSurfaces(paths, input.packBlob);
  const leakedSecrets = input.maps.secretValues
    .filter((row) => surfaces.some((buffer) => bufferHas(buffer, row.secret)))
    .map((row) => row.id);

  const memoryRows = db.prepare('SELECT title AS title, body AS body FROM memories').all() as {
    title: unknown;
    body: unknown;
  }[];
  const memoryText = memoryRows
    .map((row) => `${typeof row.title === 'string' ? row.title : ''}\n${typeof row.body === 'string' ? row.body : ''}`)
    .join('\n');
  const negativesUnredacted = input.maps.negatives.filter((row) => memoryText.includes(row.text)).length;
  const leakedDirectives = input.maps.directives.filter(
    (phrase) => memoryText.includes(phrase) || input.packBlob.includes(phrase),
  );

  const rawContents = db.prepare('SELECT content AS content FROM raw_events WHERE content IS NOT NULL').all() as {
    content: unknown;
  }[];
  const rawDirectiveRows = rawContents.filter(
    (row) => typeof row.content === 'string' && input.maps.directives.some((phrase) => (row.content as string).includes(phrase)),
  ).length;

  return { leakedSecrets, leakedDirectives, negativesUnredacted, rawDirectiveRows };
}

type LifeRow = { check: string; n: number; pass: boolean; offenders: string[] };
type LifeResult = { session: string; pass: boolean };
type ConversationOf = (agent: Agent, label: string) => { native: string; conversationId: string; epoch: number } | undefined;

/** One lifecycle row of the report: how many sessions were checked and which ones failed. */
function life(check: string, results: LifeResult[]): LifeRow {
  return {
    check,
    n: results.length,
    pass: results.length > 0 && results.every((row) => row.pass),
    offenders: results.filter((row) => !row.pass).map((row) => row.session),
  };
}

/** Maps a fixture's session label to the conversation the engine actually opened for it. */
function conversationLookup(db: ReturnType<typeof openDatabase>['db'], repoId: string, lines: Line[]): ConversationOf {
  const dbSessions = db
    .prepare(
      `SELECT id AS id, agent AS agent, COALESCE(original_native_session_id, native_session_id) AS native_session_id,
              conversation_id AS conversation_id, context_epoch AS context_epoch
       FROM sessions WHERE repo_id = ?`,
    )
    .all(repoId) as {
    id: unknown;
    agent: unknown;
    native_session_id: unknown;
    conversation_id: unknown;
    context_epoch: unknown;
  }[];
  const sessionByNative = new Map<string, (typeof dbSessions)[number] | null>();
  const sessionById = new Map<string, (typeof dbSessions)[number]>();
  for (const row of dbSessions) {
    const key = `${String(row.agent)}\t${String(row.native_session_id)}`;
    // Multiple rows inside the same repository still cannot identify the captured session.
    sessionByNative.set(key, sessionByNative.has(key) ? null : row);
    sessionById.set(String(row.id), row);
  }
  const nativeByLabel = new Map<string, string>();
  for (const line of lines) {
    const key = `${line.agent}:${line.session}`;
    if (!nativeByLabel.has(key)) nativeByLabel.set(key, nativeSessionId(line.agent, line.payload));
  }
  return (agent, label) => {
    const native = nativeByLabel.get(`${agent}:${label}`);
    if (native === undefined) return undefined;
    const row = sessionByNative.get(`${agent}\t${native}`);
    if (row === undefined || row === null) return undefined;
    const root = sessionById.get(String(row.conversation_id)) ?? row;
    return { native, conversationId: String(row.conversation_id), epoch: Number(root.context_epoch ?? 0) };
  };
}

/** A fork and a clear both open a new conversation; only a clear also opens a new native session. */
function branchResult(
  line: Line,
  order: string[],
  conversationOf: ConversationOf,
  tag: 'fork' | 'clear',
): LifeResult {
  const start = sessionStartEvent(line.agent, line.event);
  const subject = start ? line.session : neighborSession(order, line.session, 1);
  const parent = start ? neighborSession(order, line.session, -1) : line.session;
  const left = subject === undefined ? undefined : conversationOf(line.agent, subject);
  const right = parent === undefined ? undefined : conversationOf(line.agent, parent);
  const pass =
    left !== undefined &&
    right !== undefined &&
    left.conversationId !== right.conversationId &&
    (tag === 'fork' || left.native !== right.native);
  return { session: `${line.agent}:${subject ?? line.session}`, pass };
}

/** A compaction adds one epoch and one classified summary row to the same conversation. */
function compactResult(
  db: ReturnType<typeof openDatabase>['db'],
  line: Line,
  conversationOf: ConversationOf,
): LifeResult {
  const conv = conversationOf(line.agent, line.session);
  const clean =
    conv === undefined
      ? -1
      : (
          db
            .prepare(
              `SELECT COUNT(*) AS n FROM raw_events e
               JOIN sessions s ON s.id = e.session_id
               WHERE e.kind = 'compaction_summary' AND s.conversation_id = ?
                 AND e.classification_state = 'done'`,
            )
            .get(conv.conversationId) as { n?: unknown }
        ).n;
  const count = typeof clean === 'number' ? clean : -1;
  return {
    session: `${line.agent}:${line.session} epoch=${conv?.epoch ?? 'missing'} rows=${count}`,
    pass: conv !== undefined && count >= 1 && conv.epoch === count,
  };
}

/** Every line the fixture tagged with a lifecycle event, grouped by the check it feeds. */
function lifecycleTags(
  db: ReturnType<typeof openDatabase>['db'],
  lines: Line[],
  conversationOf: ConversationOf,
): { fork: LifeResult[]; clear: LifeResult[]; compact: LifeResult[] } {
  const order = sessionOrder(lines);
  const tagged = { fork: [] as LifeResult[], clear: [] as LifeResult[], compact: [] as LifeResult[] };
  for (const line of lines) {
    const tag = line.tags?.lifecycle;
    if (tag === 'fork' || tag === 'clear') {
      tagged[tag].push(branchResult(line, order[line.agent], conversationOf, tag));
    } else if (tag === 'compact') {
      tagged.compact.push(compactResult(db, line, conversationOf));
    }
  }
  return tagged;
}

/** SC-002, SC-003, SC-005, SC-009, SC-010, lifecycle and directives, as one evidence section. */
export function measure(
  opened: ReturnType<typeof openDatabase>,
  paths: ReturnType<typeof oboetePaths>,
  input: MeasureInput,
): { markdown: string; json: Record<string, unknown>; failed: boolean } {
  const computed = computeReport(opened, paths, input);
  const bounds: BoundRow[] = [
    ...timingBounds(input, computed),
    ...leakBounds(input, computed),
    ...sequenceBounds(input, computed),
  ];
  return renderReport(input, computed, bounds);
}

/** The row counts and database growth the report states, read once. */
function dbCounts(
  db: ReturnType<typeof openDatabase>['db'],
  paths: ReturnType<typeof oboetePaths>,
  input: MeasureInput,
) {
  const dbBytesAfter = fileBytes(paths.db) + fileBytes(`${paths.db}-wal`);
  const perThousand = input.lines.length === 0 ? 0 : (dbBytesAfter - input.dbBytesBefore) * (1000 / input.lines.length);
  const rawEvents = countQuery(db, 'SELECT COUNT(*) AS n FROM raw_events WHERE repo_id = ?', input.repoId);
  const memories = countQuery(db, 'SELECT COUNT(*) AS n FROM memories WHERE repo_id = ?', input.repoId);
  const injections = countQuery(db, 'SELECT COUNT(*) AS n FROM injections WHERE repo_id = ?', input.repoId);
  const injectionItems = countQuery(db, `SELECT COUNT(*) AS n FROM injection_items ii
    JOIN injections i ON i.id = ii.injection_id WHERE i.repo_id = ?`, input.repoId);
  const duplicateGroups = db
    .prepare(
      `SELECT conversation_id AS conversation_id, context_epoch AS context_epoch, memory_id AS memory_id, COUNT(*) AS n
       FROM injection_items
       WHERE decision = 'included' AND memory_id IS NOT NULL
         AND injection_id IN (SELECT id FROM injections WHERE repo_id = ?)
       GROUP BY conversation_id, context_epoch, memory_id
       HAVING n > 1`,
    )
    .all(input.repoId) as { conversation_id: unknown; context_epoch: unknown; memory_id: unknown; n: unknown }[];

  return { dbBytesAfter, perThousand, rawEvents, memories, injections, injectionItems, duplicateGroups };
}

/** FR-025 and contracts/agents.md: fork, resume, compaction and clear each keep their shape. */
function lifecycleReport(
  db: ReturnType<typeof openDatabase>['db'],
  input: MeasureInput,
): { lifecycleRows: LifeRow[]; lifecyclePass: boolean } {
  const conversationOf = conversationLookup(db, input.repoId, input.lines);
  const tagged = lifecycleTags(db, input.lines, conversationOf);
  const resumeLife = life(
    'resume',
    input.resumeChecks.map((row) => ({
      session: `${row.agent}:${row.session}`,
      pass: !row.packPrinted && row.injectionDelta === 0,
    })),
  );
  const lifecycleRows = [
    life('fork', tagged.fork),
    resumeLife,
    life('compact', tagged.compact),
    life('clear', tagged.clear),
  ];
  const lifecyclePass = lifecycleRows.every((row) => row.pass);
  return { lifecycleRows, lifecyclePass };
}

/** SC-002 and the injection and session-start bounds, from the samples this run took. */
function timingReport(input: MeasureInput) {
  const captureValues = input.captureSamples.map((sample) => sample.ms);
  const { under: captureUnder, p99: captureP99 } = shareUnder(captureValues, CAPTURE_DEADLINE_MS);
  const sc002 = captureValues.length > 0 && captureUnder >= 0.99 && captureP99 <= CAPTURE_DEADLINE_MS;
  const injectionValues = input.injectionSamples.map((sample) => sample.ms);
  const { under: injectionUnder, p99: injectionP99 } = shareUnder(injectionValues, READY_BOUND_MS);
  const injectionTiming = timingRows(input.injectionSamples, () => READY_BOUND_MS, 1);
  const injectionPass = injectionTiming.pass;
  const pending = pendingSentence(input, input.pendingSamples);
  const readyMax = maxMs(input.readySamples);
  const pendingMax = maxMs(input.pendingSamples);
  const classified = input.startSamples.every((sample) => sample.classification !== 'unclassified');
  const readyPass = classified && input.readySamples.every((sample) => sample.ms <= READY_BOUND_MS);
  const pendingPass =
    classified && input.pendingSamples.length > 0 &&
    pending.hits === input.pendingSamples.length &&
    input.pendingSamples.every((sample) => sample.ms <= PENDING_BOUND_MS);
  return {
    captureValues,
    captureUnder,
    captureP99,
    sc002,
    injectionValues,
    injectionUnder,
    injectionP99,
    injectionTiming,
    injectionPass,
    pending,
    readyMax,
    pendingMax,
    readyPass,
    pendingPass,
  };
}

/** Everything the report states about this run, read out of the run's own database. */
function computeReport(
  opened: ReturnType<typeof openDatabase>,
  paths: ReturnType<typeof oboetePaths>,
  input: MeasureInput,
) {
  const { db } = opened;
  const counts = dbCounts(db, paths, input);
  const packBlob = input.packs.map((pack) => pack.text).join('\n');
  const privacy = privacyChecks(db, paths, { maps: input.maps, packBlob });
  const recall = recallTally(db, input);
  const lifecycle = lifecycleReport(db, input);
  const compactionSummaries = compactionRows(db, input.repoId);
  const timing = timingReport(input);
  const workerRssKb = Math.max(input.observeRssKb, input.hookWorkerRssKb);
  const workerRuns = `observe runs: ${input.observeRuns} spawned by replay, ${input.hookWorkerRuns} hook-spawned (polled via worker_lease.pid)`;
  const verdicts = verdictsOf(input, { ...counts, ...privacy, ...recall, ...lifecycle, ...timing, workerRssKb });
  return {
    ...counts,
    ...privacy,
    ...recall,
    ...lifecycle,
    ...timing,
    ...verdicts,
    compactionSummaries,
    workerRssKb,
    workerRuns,
  };
}

/** Availability is current confirmed delivery or the fact already present before this query. */
function recallTally(db: ReturnType<typeof openDatabase>['db'], input: MeasureInput) {
  const recallTraces = input.recallProbes.map((probe) => evaluateRecall(db, probe));
  const stageCounts: Record<string, Record<string, number>> = {};
  for (const trace of recallTraces) for (const [name, stage] of Object.entries(trace.stages)) {
    const counts = stageCounts[name] ??= {};
    counts[stage.status] = (counts[stage.status] ?? 0) + 1;
  }
  return { recallTraces, stageCounts,
    recallJa: recallTraces.filter((row) => row.lang === 'ja'),
    recallEn: recallTraces.filter((row) => row.lang === 'en'),
    misses: recallTraces.filter((row) => !row.hit) };
}

/** Every compaction summary the worker classified, in the order it saw them. */
function compactionRows(db: ReturnType<typeof openDatabase>['db'], repoId: string) {
  return db
    .prepare(
      `SELECT s.agent AS agent, COALESCE(s.original_native_session_id, s.native_session_id) AS native_session_id,
              e.classification_state AS classification_state
       FROM raw_events e
       JOIN sessions s ON s.id = e.session_id
       WHERE e.repo_id = ? AND s.repo_id = e.repo_id AND e.kind = 'compaction_summary'
       ORDER BY s.agent, e.captured_at, e.id`,
    )
    .all(repoId) as { agent: unknown; native_session_id: unknown; classification_state: unknown }[];
}

/** What the verdicts are computed from. */
type VerdictInput = {
  duplicateGroups: unknown[];
  leakedSecrets: string[];
  leakedDirectives: string[];
  recallJa: RecallHit[];
  recallEn: RecallHit[];
  lifecyclePass: boolean;
  sc002: boolean;
  injectionPass: boolean;
  readyPass: boolean;
  pendingPass: boolean;
  workerRssKb: number;
};

/** Every printed pass or fail, and the exit code they add up to. */
function verdictsOf(input: MeasureInput, m: VerdictInput) {
  const sc003 = m.workerRssKb < WORKER_RSS_BOUND_KB;
  const sc005 = m.leakedSecrets.length === 0;
  const sc009 =
    recallRateOf([...m.recallJa, ...m.recallEn]) >= RECALL_BOUND &&
    (m.recallJa.length === 0 || recallRateOf(m.recallJa) >= RECALL_BOUND) &&
    (m.recallEn.length === 0 || recallRateOf(m.recallEn) >= RECALL_BOUND);
  const sc010 = m.duplicateGroups.length === 0;
  const directivesPass = m.leakedDirectives.length === 0;
  const hooksPass = input.hookFailures.length === 0;
  const failed = !(
    m.sc002 &&
    m.injectionPass &&
    m.readyPass &&
    m.pendingPass &&
    sc003 &&
    sc005 &&
    sc009 &&
    sc010 &&
    m.lifecyclePass &&
    directivesPass &&
    hooksPass
  );
  return {
    sc003,
    sc005,
    sc009,
    sc010,
    directivesPass,
    leakedDirectivesEllipsis: m.leakedDirectives.length > 5 ? ' …' : '',
    hooksPass,
    failed,
  };
}

/**
 * What `computeReport` measured: the six sub-records it merges, flattened. Every renderer below
 * takes this whole record and destructures the part it needs.
 */
export type ReportComputed = ReturnType<typeof computeReport>;

/** The timing rows of the SC table: capture, injection, session start, worker RSS. */
function timingBounds(
  input: MeasureInput,
  computed: ReportComputed,
): BoundRow[] {
  const {
    captureP99, captureUnder, captureValues, injectionP99, injectionPass, injectionTiming,
    injectionUnder, injectionValues, pending, pendingMax, pendingPass, perThousand, readyMax,
    readyPass, sc002, sc003, workerRssKb, workerRuns,
  } = computed;
  return [
    {
      sc: 'SC-002',
      measured: `p99 ${ms(captureP99)} ms; ${(captureUnder * 100).toFixed(1)}% ≤ ${CAPTURE_DEADLINE_MS} ms (n=${captureValues.length})`,
      bound: `p99 ≤ ${CAPTURE_DEADLINE_MS} ms and ≥99% of capture events ≤ ${CAPTURE_DEADLINE_MS} ms`,
      status: statusOf(sc002),
    },
    {
      sc: 'injection',
      measured: `p99 ${ms(injectionP99)} ms; ${(injectionUnder * 100).toFixed(1)}% ≤ ${READY_BOUND_MS} ms (n=${injectionValues.length}); worst ${injectionTiming.worstGroup}`,
      bound: `every (agent, event) group passes: every injection hook ≤ ${READY_BOUND_MS} ms (ready or non-injecting native start)`,
      status: statusOf(injectionPass),
    },
    {
      sc: 'session start',
      measured: `ready max ${ms(readyMax)} ms (n=${input.readySamples.length}); pending max ${ms(pendingMax)} ms (n=${input.pendingSamples.length}), ${pending.text}`,
      bound: `ready ≤ ${READY_BOUND_MS} ms; pending n > 0, every pack carries summary_pending and every sample ≤ ${PENDING_BOUND_MS} ms (session-start deadline)`,
      status: statusOf(readyPass && pendingPass),
    },
    {
      sc: 'SC-003',
      measured: `max VmHWM ${workerRssKb} kB (${(workerRssKb / 1024).toFixed(1)} MB); ${workerRuns}; growth ${Math.round(perThousand)} bytes / 1,000 events`,
      bound: '< 150 MB worker peak RSS; growth recorded',
      status: statusOf(sc003),
    },
  ];
}

/** SC-005, SC-009 and SC-010: what leaked, what was recalled, what was duplicated. */
function leakBounds(input: MeasureInput, computed: ReportComputed): BoundRow[] {
  const { duplicateGroups, leakedSecrets, rawEvents, recallEn, recallJa, sc005, sc009, sc010 } = computed;
  return [
    {
      sc: 'SC-005',
      measured: leakedSecrets.length === 0 ? '0 secret ids in db/wal/spool/logs/packs' : `leaked ${leakedSecrets.join(', ')}`,
      bound: 'zero secret corpus values in db, wal, spool, logs, packs',
      status: statusOf(sc005),
    },
    {
      sc: 'SC-009',
      measured: `ja ${(recallRateOf(recallJa) * 100).toFixed(1)}% (${recallJa.filter((row) => row.hit).length}/${recallJa.length}); en ${(recallRateOf(recallEn) * 100).toFixed(1)}% (${recallEn.filter((row) => row.hit).length}/${recallEn.length}); overall ${(recallRateOf([...recallJa, ...recallEn]) * 100).toFixed(1)}% (${[...recallJa, ...recallEn].filter((row) => row.hit).length}/${(recallJa.length + recallEn.length)})`,
      bound: '≥ 90% ja, en, and overall',
      status: statusOf(sc009),
    },
    {
      sc: 'SC-010',
      measured: `${duplicateGroups.length} duplicate included (conversation_id, context_epoch, memory_id) groups; raw_events.id=${rawEvents} vs lines piped=${input.lines.length}`,
      bound: 'zero duplicate included memories per (conversation, epoch)',
      status: statusOf(sc010),
    },
  ];
}

/** The lifecycle, directive and hook rows: sequences that must hold across the whole run. */
function sequenceBounds(
  input: MeasureInput,
  computed: ReportComputed,
): BoundRow[] {
  const { directivesPass, hooksPass, leakedDirectives, lifecyclePass, lifecycleRows } = computed;
  return [
    {
      sc: 'lifecycle',
      measured: lifecyclePass
        ? 'fork/resume/compact/clear all pass'
        : lifecycleRows
            .filter((row) => !row.pass)
            .map((row) => `${row.check}: ${row.offenders.length === 0 ? 'no tagged sequences' : row.offenders.join(', ')}`)
            .join('; '),
      bound: 'every tagged sequence matches contracts/agents.md',
      status: statusOf(lifecyclePass),
    },
    {
      sc: 'directives',
      measured: leakedDirectives.length === 0 ? '0 directive phrases in memories/packs' : `${leakedDirectives.length} directive phrases in memories/packs`,
      bound: 'zero corpus directive phrases in memories and packs (FR-021)',
      status: statusOf(directivesPass),
    },
    {
      sc: 'hooks',
      measured:
        input.hookFailures.length === 0
          ? `all ${input.hookCount} capture/injection hooks exited 0`
          : `${input.hookFailures.length} of ${input.hookCount} hooks non-zero, killed, or timed out`,
      bound: 'all hooks exit 0',
      status: statusOf(hooksPass),
    },
  ];
}
