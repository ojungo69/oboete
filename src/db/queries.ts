import type { DatabaseSync, SQLInputValue, SQLOutputValue } from 'node:sqlite';

import type { Destination, Sensitivity } from '../privacy/egress.js';
import { searchCandidates } from '../retrieval/query.js';
import { normalizeBm25, rrfFuse } from '../retrieval/rank.js';
import { strictest } from '../privacy/classify.js';
import { sha256Json } from '../hash.js';
import { prepared } from './statements.js';

export type ReviewState = 'unreviewed' | 'reviewed' | 'imported';
export type SummaryState = 'pending' | 'done' | 'no_content';

/** The public `memories` columns (0002_memory_search.sql), excluding the internal FTS rid. */
export type MemoryRow = {
  id: string;
  repo_id: string;
  type: string;
  title: string | null;
  body: string | null;
  concepts: string | null;
  cjk_bigrams: string | null;
  material_hash: string | null;
  content_hash: string;
  sensitivity: Sensitivity;
  review_state: ReviewState;
  degraded_reason: string | null;
  source_session_id: string | null;
  source_batch_id: string | null;
  source_captured_at?: number | null;
  work_id?: string | null;
  checkpoint_parent_id?: string | null;
  provenance_complete?: number | null;
  valid_from: number | null;
  valid_to: number | null;
  superseded_by: string | null;
  pinned_at: number | null;
  pin_order: number | null;
  last_injected_at: number | null;
  citations_head: string | null;
  citations_ok: number | null;
  deleted_at: number | null;
  created_at: number | null;
};

export type MemorySourceRow = {
  raw_event_id: string | null;
  citation_kind: string | null;
  citation_value: string | null;
  source_agent: string | null;
};

/** A `WHERE` fragment over the alias `m` plus its parameters, in that order. */
export type MemoryScope = { repoId: string; where: string; params: SQLInputValue[] };

/** One audience predicate for readers and observer context; lifecycle/egress is added by callers. */
export function visibilityScope(input: { repoId: string; workId?: string | null; personal?: boolean }) {
  return { where: `EXISTS (SELECT 1 FROM memory_visibility v
    LEFT JOIN sharing_proposals p ON p.id = v.proposal_id WHERE v.memory_id = m.id AND (
      (v.audience = 'project' AND v.repo_id = ? AND m.repo_id = v.repo_id)
      OR (v.audience = 'work' AND v.repo_id = ? AND m.repo_id = v.repo_id AND v.work_id = ?
        AND EXISTS (SELECT 1 FROM work_items w WHERE w.id = v.work_id AND w.repo_id = v.repo_id))
      ${input.personal === false ? '' : "OR (v.audience = 'personal' AND p.state = 'approved' AND p.projected_memory_id = m.id AND p.candidate_material_hash = m.material_hash AND p.candidate_title = m.title AND p.candidate_body = m.body)"}))`,
  params: [input.repoId, input.repoId, input.workId ?? null] as SQLInputValue[] };
}

export type VisibilityGrant =
  | { audience: 'work'; repoId: string; workId: string }
  | { audience: 'project'; repoId: string }
  | { audience: 'personal'; proposalId: string };

export function grantVisibility(db: DatabaseSync, memoryId: string, grant: VisibilityGrant,
  kind: 'migration' | 'observer' | 'explicit_adoption' | 'proposal_approval', now: number): void {
  const repoId = 'repoId' in grant ? grant.repoId : null;
  const workId = 'workId' in grant ? grant.workId : null;
  const proposalId = 'proposalId' in grant ? grant.proposalId : null;
  prepared(db, `INSERT OR IGNORE INTO memory_visibility
    (id, memory_id, audience, repo_id, work_id, proposal_id, grant_kind, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)`)
    .run(`v_${sha256Json([memoryId, grant.audience, repoId, workId])}`, memoryId,
      grant.audience, repoId, workId, proposalId, kind, now);
}

/** Included in prepared/final read and generation stamps, without loading any source body. */
export function memoryVisibility(db: DatabaseSync, memoryId: string) {
  return db.prepare(`SELECT v.*, p.state, p.projected_memory_id
    FROM memory_visibility v LEFT JOIN sharing_proposals p ON p.id = v.proposal_id
    WHERE v.memory_id = ? ORDER BY v.id`).all(memoryId);
}

export function visibilityUnchanged(db: DatabaseSync, candidate: Pick<NearbyCandidate, 'id' | 'visibility_stamp'>): boolean {
  return candidate.visibility_stamp !== undefined && candidate.visibility_stamp === sha256Json(memoryVisibility(db, candidate.id));
}

export type TimelineTurn = {
  id: string;
  ordinal: number;
  started_at: number | null;
  ended_at: number | null;
  memory_ids: string[];
};

export type TimelineMemory = Pick<
  MemoryRow,
  | 'id'
  | 'type'
  | 'title'
  | 'body'
  | 'sensitivity'
  | 'review_state'
  | 'degraded_reason'
  | 'source_session_id'
  | 'source_batch_id'
  | 'pinned_at'
  | 'pin_order'
  | 'created_at'
> & {
  turn_ids: string[];
  sources: MemorySourceRow[];
};

export type TimelineSession = {
  id: string;
  agent: string;
  started_at: number | null;
  ended_at: number | null;
  status: string;
  turn_count: number;
  summary_state: SummaryState | null;
  turns: TimelineTurn[];
  memory_ids: string[];
  memories: TimelineMemory[];
};

export type SessionState = {
  sessionId: string;
  summaryState: SummaryState | null;
  endedAt: number | null;
};

/**
 * The one filter every reader appends: retrieval, injection, the command line, MCP and the viewer.
 * No caller writes its own repository, sensitivity, review state, tombstone or validity condition
 * (docs/dev/conventions.md "Sensitivity and egress").
 */
export function memoryScope(
  db: DatabaseSync,
  input: { repoId: string; destination: Destination; workId?: string | null; history?: boolean },
): MemoryScope {
  if (input.destination === 'sync') {
    // Synchronization is M2 (plan.md "Constitution Check" V); refusing beats a silent wider scope.
    throw new Error('Synchronization is not available in M1.');
  }

  // data-model "destination_rules": one seeded table governs every egress decision, so the list is
  // read here and never hardcoded. privacy/egress.ts evaluates the same table row by row on the
  // observer path.
  const allowed = db
    .prepare(
      'SELECT sensitivity AS sensitivity FROM destination_rules WHERE destination = ? AND allowed = 1 ORDER BY sensitivity',
    )
    .all(input.destination)
    .map((row) => String(row.sensitivity))
    // FR-020: a secret row is never readable, whatever the table happens to say, so a restored or
    // tampered database can only narrow this set. privacy/egress.ts isAllowed holds the same hard
    // rule for the observer path.
    .filter((sensitivity) => sensitivity !== 'secret');

  const visibility = visibilityScope(input);
  const conditions = [
    visibility.where,
    '(m.work_id IS NULL OR (m.work_id = ? AND m.repo_id = ?))',
    'm.deleted_at IS NULL', // FR-035: a tombstone never surfaces again.
    ...(input.history === true ? [] : ['m.valid_to IS NULL']),
    "m.review_state <> 'imported'", // R12 "Export/import": imported rows stay quarantined.
    // Fails closed: with no allowed sensitivity the fragment matches no row at all.
    allowed.length === 0 ? '0' : `m.sensitivity IN (${allowed.map(() => '?').join(', ')})`,
  ];

  const params: SQLInputValue[] = [...visibility.params, input.workId ?? null, input.repoId, ...allowed];
  if (input.history !== true) {
    conditions.push(input.workId == null ? "m.type <> 'session_summary'"
      : `(m.type <> 'session_summary' OR (m.work_id = ? AND m.id = (
        SELECT current_checkpoint_memory_id FROM work_items WHERE id = ? AND repo_id = ? AND state = 'active')))`);
    if (input.workId != null) params.push(input.workId, input.workId, input.repoId);
  }
  return { repoId: input.repoId, where: `(${conditions.join(' AND ')})`, params };
}

/** Null for a missing id and for one outside the scope alike (contracts/cli.md, contracts/mcp.md). */
export function getMemory(db: DatabaseSync, id: string, scope: MemoryScope): MemoryRow | null {
  const row = db
    .prepare(`SELECT m.* FROM memories m WHERE ${scope.where} AND m.id = ?`)
    .get(...scope.params, id);
  return row === undefined ? null : asMemoryRows([row])[0];
}

/** Only the selected work's published checkpoint can represent its current progress. */
export function currentWorkCheckpoint(db: DatabaseSync, workId: string, scope: MemoryScope): MemoryRow | null {
  const row = db.prepare(`SELECT m.* FROM work_items w JOIN memories m ON m.id = w.current_checkpoint_memory_id
    WHERE w.id = ? AND w.repo_id = ? AND m.work_id = w.id AND ${scope.where}`)
    .get(workId, scope.repoId, ...scope.params);
  return row === undefined ? null : asMemoryRows([row])[0];
}

export function memorySources(db: DatabaseSync, id: string): MemorySourceRow[] {
  return db
    .prepare(
      `SELECT raw_event_id, citation_kind, citation_value, source_agent
       FROM memory_sources WHERE memory_id = ? AND context_only = 0 ORDER BY id`,
    )
    .all(id) as unknown as MemorySourceRow[];
}

/** Pins or unpins one row the same scope shows the reader, so a hidden id changes nothing. */
export function setPinned(
  db: DatabaseSync,
  input: { id: string; scope: MemoryScope; pinnedAt: number | null; pinOrder: number | null },
): boolean {
  const result = db
    .prepare(`UPDATE memories AS m SET pinned_at = ?, pin_order = ? WHERE ${input.scope.where} AND m.id = ?`)
    .run(input.pinnedAt, input.pinOrder, ...input.scope.params, input.id);
  return Number(result.changes) !== 0;
}

/** The viewer's review action (data-model "memories" review_state), scoped like every write. */
export function setReviewed(db: DatabaseSync, input: { id: string; scope: MemoryScope }): boolean {
  const result = db
    .prepare(`UPDATE memories AS m SET review_state = 'reviewed' WHERE ${input.scope.where} AND m.id = ?`)
    .run(...input.scope.params, input.id);
  return Number(result.changes) !== 0;
}

/**
 * Leaves the hashes and content in place so identical content cannot be recreated (FR-035).
 * Scoped like the reads: an id outside the boundary is refused exactly like a missing one, so the
 * exit code never reveals a row the developer may not see (contracts/cli.md).
 */
export function tombstone(
  db: DatabaseSync,
  input: { id: string; scope: MemoryScope; deletedAt: number },
): boolean {
  const result = db
    .prepare(`UPDATE memories AS m SET deleted_at = ? WHERE ${input.scope.where} AND m.id = ?`)
    .run(input.deletedAt, ...input.scope.params, input.id);
  return Number(result.changes) !== 0;
}

export function listMemories(
  db: DatabaseSync,
  scope: MemoryScope,
  options: { limit: number; offset?: number },
): MemoryRow[] {
  return asMemoryRows(
    db
      .prepare(
        `SELECT m.* FROM memories m WHERE ${scope.where}
         ORDER BY m.created_at DESC, m.id DESC LIMIT ? OFFSET ?`,
      )
      .all(...scope.params, options.limit, options.offset ?? 0),
  );
}

export function pinnedMemories(db: DatabaseSync, scope: MemoryScope): MemoryRow[] {
  return asMemoryRows(
    db
      .prepare(
        `SELECT m.* FROM memories m WHERE ${scope.where} AND m.pinned_at IS NOT NULL
         ORDER BY m.pin_order, m.pinned_at, m.id`,
      )
      .all(...scope.params),
  );
}

/**
 * Legacy history inspection only. Active progress is resolved through the work pointer.
 */
export function latestSessionSummary(db: DatabaseSync, repoId: string, workId?: string): MemoryRow | null {
  const scope = memoryScope(db, { repoId, workId, destination: 'injection', history: true });
  const row = db
    .prepare(
      `SELECT m.*, s.summary_degraded_reason AS degraded_reason
       FROM sessions s JOIN memories m ON m.id = s.latest_summary_memory_id
       WHERE ${scope.where} AND m.valid_to IS NULL AND s.repo_id = ? AND s.status = 'ended' AND s.summary_state = 'done'
       ORDER BY s.ended_at DESC, s.id DESC LIMIT 1`,
    )
    .get(...scope.params, repoId);
  return row === undefined ? null : asMemoryRows([row])[0];
}

/** Lets the pack builder tell a pending summary from a session that had nothing to summarize. */
export function latestSessionState(db: DatabaseSync, repoId: string): SessionState | null {
  const row = db
    .prepare(
      `SELECT id AS id, summary_state AS summary_state, ended_at AS ended_at FROM sessions
       WHERE repo_id = ? AND status = 'ended' ORDER BY ended_at DESC, id DESC LIMIT 1`,
    )
    .get(repoId);
  if (row === undefined) return null;
  return {
    sessionId: String(row.id),
    summaryState: (row.summary_state as SummaryState | null) ?? null,
    endedAt: (row.ended_at as number | null) ?? null,
  };
}

function timelineMemories(
  db: DatabaseSync,
  sessionId: string,
  memories: MemoryRow[],
): TimelineMemory[] {
  if (memories.length === 0) return [];
  const sourceRows = db
    .prepare(
      `SELECT ms.memory_id, ms.raw_event_id, ms.citation_kind, ms.citation_value,
              ms.source_agent, e.session_id AS event_session_id, e.turn_id
       FROM memory_sources ms LEFT JOIN raw_events e ON e.id = ms.raw_event_id
       WHERE ms.context_only = 0 AND ms.memory_id IN (${memories.map(() => '?').join(', ')}) ORDER BY ms.id`,
    )
    .all(...memories.map((memory) => memory.id));
  const sources = new Map<string, MemorySourceRow[]>();
  const turnIds = new Map<string, Set<string>>();
  for (const row of sourceRows) {
    const memoryId = String(row.memory_id);
    const list = sources.get(memoryId) ?? [];
    list.push({
      raw_event_id: typeof row.raw_event_id === 'string' ? row.raw_event_id : null,
      citation_kind: typeof row.citation_kind === 'string' ? row.citation_kind : null,
      citation_value: typeof row.citation_value === 'string' ? row.citation_value : null,
      source_agent: typeof row.source_agent === 'string' ? row.source_agent : null,
    });
    sources.set(memoryId, list);
    if (row.event_session_id === sessionId && typeof row.turn_id === 'string') {
      const ids = turnIds.get(memoryId) ?? new Set<string>();
      ids.add(row.turn_id);
      turnIds.set(memoryId, ids);
    }
  }

  return memories.map((memory) => ({
    id: memory.id,
    type: memory.type,
    title: memory.title,
    body: memory.body,
    sensitivity: memory.sensitivity,
    review_state: memory.review_state,
    degraded_reason: memory.degraded_reason,
    source_session_id: memory.source_session_id,
    source_batch_id: memory.source_batch_id,
    pinned_at: memory.pinned_at,
    pin_order: memory.pin_order,
    created_at: memory.created_at,
    turn_ids: [...(turnIds.get(memory.id) ?? [])],
    sources: sources.get(memory.id) ?? [],
  }));
}

/** Sessions of the repository with their turns and the memories that reach the reader. */
export function timeline(
  db: DatabaseSync,
  repoId: string,
  options: { sessionId?: string; limit: number; workId?: string | null; history?: boolean },
): TimelineSession[] {
  const scope = memoryScope(db, { repoId, destination: 'injection', workId: options.workId, history: options.history });
  const sessionFilter = options.sessionId === undefined ? '' : 'AND s.id = ?';
  const sessionParams: SQLInputValue[] =
    options.sessionId === undefined
      ? [repoId, options.limit]
      : [repoId, options.sessionId, options.limit];

  const sessions = db
    .prepare(
      `SELECT s.id AS id, s.agent AS agent, s.started_at AS started_at, s.ended_at AS ended_at,
              s.status AS status, s.turn_count AS turn_count, s.summary_state AS summary_state
       FROM sessions s WHERE s.repo_id = ? ${sessionFilter}
       ORDER BY s.started_at DESC, s.id DESC LIMIT ?`,
    )
    .all(...sessionParams);

  const turnsOf = db.prepare(
    `SELECT t.id AS id, t.ordinal AS ordinal, t.started_at AS started_at, t.ended_at AS ended_at
     FROM turns t WHERE t.session_id = ? ORDER BY t.ordinal`,
  );

  // ponytail: three fixed queries per session; the listing caps at 50 (contracts/cli.md).
  return sessions.map((session) => {
    const sessionId = String(session.id);
    const memories = timelineMemories(db, sessionId, memoriesForSession(db, sessionId, scope));
    const memoryIdsByTurn = new Map<string, string[]>();
    for (const memory of memories) {
      for (const turnId of memory.turn_ids) {
        const ids = memoryIdsByTurn.get(turnId) ?? [];
        ids.push(memory.id);
        memoryIdsByTurn.set(turnId, ids);
      }
    }
    return {
      id: sessionId,
      agent: String(session.agent),
      started_at: (session.started_at as number | null) ?? null,
      ended_at: (session.ended_at as number | null) ?? null,
      status: String(session.status),
      turn_count: Number(session.turn_count),
      summary_state: (session.summary_state as SummaryState | null) ?? null,
      turns: turnsOf.all(sessionId).map((turn) => ({
        id: String(turn.id),
        ordinal: Number(turn.ordinal),
        started_at: (turn.started_at as number | null) ?? null,
        ended_at: (turn.ended_at as number | null) ?? null,
        memory_ids: memoryIdsByTurn.get(String(turn.id)) ?? [],
      })),
      memory_ids: memories.map((memory) => memory.id),
      memories,
    };
  });
}

/** The memories a session produced, including after raw-event expiry (FR-008, data-model "raw_events"). */
const SESSION_MEMORY_IDS_SQL = `SELECT s.id FROM memories s WHERE s.source_session_id = ?
  UNION ALL SELECT ms.memory_id FROM memory_sources ms JOIN raw_events e ON e.id = ms.raw_event_id
    WHERE e.session_id = ? AND ms.context_only = 0`;

export function memoriesForSession(
  db: DatabaseSync,
  sessionId: string,
  scope: MemoryScope,
): MemoryRow[] {
  // The two paths to a session are collected as ids and the rows are then read by primary key:
  // joining them onto the row itself made SQLite expand every in-scope memory by its
  // `memory_sources` rows and de-duplicate whole rows, about a third slower over 4,000 memories.
  return asMemoryRows(
    db
      .prepare(
        `SELECT m.* FROM memories m WHERE ${scope.where} AND m.id IN (${SESSION_MEMORY_IDS_SQL})
         ORDER BY m.created_at DESC, m.id DESC`,
      )
      .all(...scope.params, sessionId, sessionId),
  );
}

/** Summary headings and a total, without allocating every linked memory body. */
export function memoryTitlesForSession(db: DatabaseSync, sessionId: string, scope: MemoryScope, limit: number) {
  const rows = db.prepare(`SELECT m.id, m.title, COUNT(*) OVER () AS total,
    MAX(CASE WHEN m.sensitivity = 'local_only' THEN m.sensitivity END) OVER () AS source_local,
    MAX(CASE WHEN m.sensitivity = 'private' THEN m.sensitivity END) OVER () AS source_private,
    MAX(CASE WHEN m.sensitivity = 'secret' THEN m.sensitivity END) OVER () AS source_secret FROM memories m
    WHERE ${scope.where} AND m.type <> 'session_summary' AND COALESCE(m.title, '') <> ''
      AND m.id IN (${SESSION_MEMORY_IDS_SQL}) ORDER BY m.created_at DESC, m.id DESC LIMIT ?`)
    .all(...scope.params, sessionId, sessionId, limit);
  return { items: rows.map((row) => String(row.title)), memoryIds: rows.map((row) => String(row.id)), total: Number(rows[0]?.total ?? 0),
    sensitivity: strictest('eligible', ...rows.slice(0, 1).flatMap((row) =>
      [row.source_local, row.source_private, row.source_secret].filter((value): value is Sensitivity => typeof value === 'string'))) };
}

/** Records delivery for the 90-day retirement (data-model "memories"). */
export function markInjected(db: DatabaseSync, ids: string[], now: number): void {
  // Hooks write this outside the worker lease, so it is a plain UPDATE and not a fenced one
  // (docs/dev/conventions.md "Database access").
  const update = db.prepare('UPDATE memories SET last_injected_at = ? WHERE id = ?');
  for (const id of ids) {
    update.run(now, id);
  }
}

function asMemoryRows(rows: Record<string, SQLOutputValue>[]): MemoryRow[] {
  for (const row of rows) delete row.rid;
  return rows as unknown as MemoryRow[];
}

/**
 * A classification candidate: a memory of the same repository as the batch, tombstones and
 * superseded rows included (R10, contracts/observer.md "Worker rules after either path").
 */
export type NearbyCandidate = {
  id: string;
  repo_id: string;
  type: string;
  title: string;
  body: string;
  content_hash: string;
  deleted: boolean;
  sensitivity: Sensitivity;
  review_state?: ReviewState;
  source_captured_at?: number | null;
  valid_to?: number | null;
  work_id?: string | null;
  checkpoint_parent_id?: string | null;
  provenance_complete?: number | null;
  material_hash?: string | null;
  privacy_stamp?: string;
  visibility_stamp?: string;
};

/**
 * The top `limit` same-repository memories for the batch text (R10: top 8, tombstones included).
 * The scope is deliberately the repository alone: `memoryScope` hides tombstoned and superseded
 * rows, which are exactly the rows the tombstone check and an `update` decision need (FR-035).
 * Sensitivity travels with each row so `observer/request.ts` can drop what a destination may not
 * receive; this function never decides egress itself.
 */
export function nearbyCandidates(
  db: DatabaseSync,
  input: { repoId: string; workId?: string | null; text: string; limit?: number },
): NearbyCandidate[] {
  const limit = input.limit ?? 8;
  const visibility = visibilityScope({ ...input, personal: false });
  const found = searchCandidates(db, {
    text: input.text,
    // R12 quarantine: an imported row the worker has not classified is offered to no summarizer.
    scope: { where: `${visibility.where} AND m.review_state <> 'imported' AND m.type <> 'session_summary'
      AND NOT EXISTS (SELECT 1 FROM memory_visibility personal WHERE personal.memory_id = m.id AND personal.audience = 'personal')`, params: visibility.params },
    limit,
  });
  const ranked = rrfFuse(normalizeBm25(normalizeBm25(found.rows, 'scoreTrigram'), 'scoreCjk'))
    .sort((left, right) => (right.score_rrf ?? 0) - (left.score_rrf ?? 0) || (left.id < right.id ? -1 : 1))
    .slice(0, limit);
  if (ranked.length === 0) return [];

  const byId = new Map<string, NearbyCandidate>();
  const rows = db
    .prepare(
      `SELECT m.id, m.repo_id, m.type, m.title, m.body, m.content_hash, m.sensitivity, m.review_state, m.deleted_at, m.source_captured_at, m.valid_to,
        m.work_id, m.checkpoint_parent_id, m.provenance_complete, m.material_hash
       FROM memories m WHERE m.id IN (${ranked.map(() => '?').join(', ')})`,
    )
    .all(...ranked.map((row) => row.id));
  for (const row of rows) {
    byId.set(String(row.id), {
      id: String(row.id),
      repo_id: String(row.repo_id),
      type: String(row.type),
      title: typeof row.title === 'string' ? row.title : '',
      body: typeof row.body === 'string' ? row.body : '',
      content_hash: String(row.content_hash),
      deleted: row.deleted_at !== null,
      valid_to: typeof row.valid_to === 'number' ? row.valid_to : null,
      sensitivity: row.sensitivity as Sensitivity,
      review_state: row.review_state as ReviewState,
      source_captured_at: typeof row.source_captured_at === 'number' ? row.source_captured_at : null,
      visibility_stamp: sha256Json(memoryVisibility(db, String(row.id))),
      work_id: typeof row.work_id === 'string' ? row.work_id : null,
      checkpoint_parent_id: typeof row.checkpoint_parent_id === 'string' ? row.checkpoint_parent_id : null,
      provenance_complete: typeof row.provenance_complete === 'number' ? row.provenance_complete : null,
      material_hash: typeof row.material_hash === 'string' ? row.material_hash : null,
    });
  }
  // The search decided the order; the second query only fills the columns it does not return.
  return ranked.map((row) => byId.get(row.id)).filter((row): row is NearbyCandidate => row !== undefined);
}
