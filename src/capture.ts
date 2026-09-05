// The capture hook: one short-lived process per agent event, an absolute deadline measured from
// process start, the detector before the first write anywhere, and a row in `raw_events` or a
// sanitized spool file. Sources: contracts/agents.md ("Hook process rules and SLAs", "Size cap
// (R4, A7)", "Agent identity (fixed selectors)", "Event identity and conversation identity"),
// contracts/cli.md (`hook`, `capture`), research.md R1, R4, R6, R7, R12, data-model.md
// (raw_events, sessions, turns, diagnostics, "Spool entry"), spec FR-001 to FR-008, FR-017 to
// FR-019, FR-021, FR-024/FR-026, and amendments A7, A12, A14, A16, A18.
import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, readSync, renameSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { performance } from 'node:perf_hooks';
import type { DatabaseSync } from 'node:sqlite';
import { parseArgs } from 'node:util';

import {
  adapt,
  resolveAgent,
  scanPartialPrefix,
  textFields,
  type AdapterAgent,
  type AdapterOutput,
} from './agents/index.js';
import {
  ConfigError,
  RepoConfigError,
  isPaused,
  loadConfig,
  loadRepoRules,
  type OboeteConfig,
} from './config.js';
import { openDatabase } from './db/open.js';
import {
  contentHash,
  conversationPolicy,
  eventId,
  type AgentName,
  type EventKind,
  type NormalizedEvent,
  type SessionStartSource,
} from './events.js';
import { appendLogQuietly, credentialValues, errorCode } from './log.js';
import { ensureDirectories, oboetePaths, resolveHome, type OboetePaths } from './paths.js';
import { detectInWorker, type DetectorInput, type DetectorResult } from './privacy/detect.js';
import { resolveRepoIdentity, type GitSpawn, type RepoIdentity } from './repo-identity.js';
import { writeSpoolEntry, type SpoolEntry } from './spool.js';
import { isLeaseFree, transactionImmediate } from './worker/lease.js';
import type { HookContext } from './injection/inject.js';
import { stripRecognizedPacks } from './injection/recognize.js';
import { testFault } from './testing/faults.js';

/** The absolute budget of a capture hook, measured from process start (contracts/agents.md). */
export const CAPTURE_DEADLINE_MS = 300;
/** Capture's 300 ms plus A2's one-second session-summary wait and delivery margin. */
export const INJECTION_DEADLINE_MS = 1_300;
/** Held back first, so a sanitized event always has time to reach the spool. */
export const SPOOL_RESERVE_MS = 40;
/** Between the detector's hard cutoff and the spool reserve: building and inserting the rows. */
export const ROW_BUILD_MARGIN_MS = 20;
/**
 * The slice the detector keeps whatever git does. Repository identity is resolved before the
 * detector runs, so a slow git (cold disk, WSL `/mnt/c`, NFS) would otherwise push the detector
 * cutoff to zero and store a content-less `failed` row although the database was writable.
 */
export const DETECTOR_MIN_MS = 60;
/**
 * How much of stdin the hook reads before it stops (A7 with the A14 default, 2026-09-04). The
 * secret-dense worst case of the full detector measures 406-665 ms per 1 MB on Node 22 and 24, so
 * the 1 MB bound of the spec cannot hold the 240 ms detector cutoff; 256 KiB keeps that worst case
 * near 100-170 ms and stays above A14's 200 KB escalation floor. The read part is treated exactly
 * as A7 prescribes: a redacted `partial` row marked truncated.
 */
export const STDIN_READ_BOUND = 262_144;
/** data-model raw_events: `expires_at` = captured_at + 7 days (FR-008). */
export const RAW_EVENT_TTL_MS = 7 * 24 * 60 * 60 * 1_000;

const BUSY_TIMEOUT_CEILING_MS = 150;
const SPAWN_MIN_REMAINING_MS = 10;
const TURN_BATCH = 10;
const STDIN_WAIT_LIMIT = 100;
const UNKNOWN_SESSION = 'unknown';
/** The key that records "this epoch was opened by Claude Code's SessionStart(compact)" (A16). */
const CLAUDE_COMPACT_START_KEY = 'session_start:compact';
/**
 * The same record for the other order: Claude Code's two compaction hooks fire about 24 ms apart
 * and are serialized only by `BEGIN IMMEDIATE`, so `PostCompact` can commit first. The prefix marks
 * an epoch its `PostCompact` opened, so the companion `SessionStart(compact)` confirms it instead of
 * advancing a second time (A16).
 */
const CLAUDE_COMPACT_POST_PREFIX = 'post_compact:';

// The normalized kind of an event name, used where no adapter runs: the partial row of an
// oversized payload and the rows of an unreadable payload keep the kind of the fixed `--event`
// argument setup wrote (contracts/agents.md "Size cap").
const EVENT_KIND_BY_NAME: Readonly<Record<string, EventKind>> = {
  SessionStart: 'session_start',
  UserPromptSubmit: 'prompt',
  PreToolUse: 'tool_call',
  PostToolUse: 'tool_result',
  PostToolUseFailure: 'tool_failure',
  PermissionDenied: 'tool_failure',
  Stop: 'turn_end',
  PostCompact: 'compaction_summary',
  SessionEnd: 'session_end',
  session_start: 'session_start',
  input: 'prompt',
  tool_result: 'tool_result',
  agent_settled: 'turn_end',
  session_shutdown: 'session_end',
  session_compact: 'compaction_summary',
};

// The kinds that end a turn or a session and therefore make a batch worth summarizing (FR-010).
const BATCH_TRIGGER_KINDS: ReadonlySet<EventKind> = new Set<EventKind>([
  'turn_end',
  'session_end',
  'last_assistant_message',
]);

export type StdinRead = { text: string; truncated: boolean };

export type CaptureDeps = {
  /** The detector, cut off at `cutoffMs`; the hook runs it in a Worker (contracts/agents.md). */
  detect(input: DetectorInput, cutoffMs: number): Promise<DetectorResult>;
  /** Wall clock for `captured_at`. */
  now(): number;
  /** Milliseconds since this process started, which is where the absolute deadline is measured from. */
  elapsedMs(): number;
  spawnWorker(): void;
  /** The git child repository identity runs; a test injects its own (FR-004, repo-identity.ts). */
  gitSpawn?: GitSpawn;
};

/** What is left of the hook's deadline for git, with the detector's slice held back (FR-002). */
function gitOptions(deps: CaptureDeps, deadlineMs: number): { spawn?: GitSpawn; budgetMs: number } {
  return {
    spawn: deps.gitSpawn,
    budgetMs:
      deadlineMs - deps.elapsedMs() - SPOOL_RESERVE_MS - ROW_BUILD_MARGIN_MS - DETECTOR_MIN_MS,
  };
}

export type CaptureInput = {
  agent: AgentName;
  eventName: string;
  paths: OboetePaths;
  /** Called after the paused check, so a paused installation reads nothing (R12). */
  readStdin(): StdinRead;
  /** Pi hands the extension's in-memory failure codes to the next child (FR-007, A8). */
  priorFailures?: string[];
};

export type CaptureOutcome = {
  outcome: 'paused' | 'not_captured' | 'stored' | 'spooled' | 'dropped';
  rows: number;
  reason?: string;
  /** Native hook transport written by runHook after capture and logging complete. */
  stdout?: string;
};

type InjectionSeed = {
  eventName: string;
  event: NormalizedEvent;
  config: OboeteConfig;
  secretPaths: string[];
};

type Diagnostic = { kind: string; agent: AgentName; messageCode: string };

type RowDraft = {
  /**
   * The row id. `turnOrdinal` is R7's turn key for an event that carries no per-turn value of its
   * own; the direct path reads it from `sessions.turn_count` and the spool path has none.
   */
  identify(turnOrdinal?: number): string;
  agent: AgentName;
  nativeSessionId: string;
  kind: EventKind;
  capturedAt: number;
  source?: SessionStartSource;
  model?: string;
  content: string | null;
  contentHash: string | null;
  payload: Record<string, unknown>;
  sensitivity: 'local_only' | 'secret';
  classificationState: 'done' | 'partial' | 'failed';
  truncated: 0 | 1;
  /** Set only for a whole, detector-clean event: the one shape that may open a context epoch (A16). */
  event?: NormalizedEvent;
};

/** The stored `raw_events.content` of one event: its content fields, or NULL when it has none. */
export function contentText(event: NormalizedEvent): string | null {
  const values = textFields(event)
    .filter((field) => field.content)
    .map((field) => field.read());
  return values.length === 0 ? null : values.join('\n');
}

/** The normalized event without its content strings (data-model raw_events.payload_json). */
export function payloadJson(event: NormalizedEvent): Record<string, unknown> {
  const payload: Record<string, unknown> = { ...event };
  switch (event.kind) {
    case 'prompt':
    case 'compaction_summary':
    case 'last_assistant_message':
      delete payload.text;
      break;
    case 'tool_call':
      payload.input = { ...event.input, command: undefined, text: undefined };
      break;
    case 'tool_result':
      delete payload.output;
      break;
    case 'tool_failure':
      delete payload.error;
      break;
    default:
      break;
  }
  return payload;
}

export type CompactionState = { contextEpoch: number; lastCompactionKey: string | null };

/**
 * The context epoch of a conversation (A12): 0 at the root, +1 per compaction. The authoritative
 * event is one per agent, and the key that distinguishes two compactions is what the R13 probe
 * found ("Compaction identity and order", 2026-09-03): Grok Build's `PostCompact.timestamp`, Pi's
 * `compactionEntry.id`, and on Claude Code and Codex the compaction event's own id, which the
 * caller passes as `eventIdentity` because it is the stored `raw_events.id` of that very row (A16,
 * which collapses two byte-identical compactions of one turn). Claude Code is the one agent whose
 * `SessionStart source = compact` runs about 24 ms *before* `PostCompact`, so there that hook opens
 * the epoch and `PostCompact` only confirms it. Returns the new state, or null when the event
 * leaves the epoch untouched.
 */
export function applyCompaction(
  agent: AgentName,
  event: NormalizedEvent,
  state: CompactionState,
  eventIdentity: string,
): CompactionState | null {
  const stored = state.lastCompactionKey;
  // The compaction the current epoch belongs to, whichever of Claude Code's two hooks opened it.
  const openedKey =
    stored !== null && stored.startsWith(CLAUDE_COMPACT_POST_PREFIX)
      ? stored.slice(CLAUDE_COMPACT_POST_PREFIX.length)
      : stored;

  if (agent === 'claude' && event.kind === 'session_start' && event.source === 'compact') {
    if (stored === CLAUDE_COMPACT_START_KEY) return null;
    // The `PostCompact` of this same compaction committed first and already opened the epoch, so
    // this hook only consumes the marker; the next `PostCompact` opens the next epoch (A16).
    if (openedKey !== stored) return { contextEpoch: state.contextEpoch, lastCompactionKey: openedKey };
    return { contextEpoch: state.contextEpoch + 1, lastCompactionKey: CLAUDE_COMPACT_START_KEY };
  }
  if (event.kind !== 'compaction_summary') return null;

  const key = event.compaction_key !== '' ? event.compaction_key : eventIdentity;
  if (openedKey === key) return null;
  // The companion hook already opened this epoch, so PostCompact only records its own key, which
  // is what lets the next SessionStart(compact) open the next epoch.
  if (agent === 'claude' && stored === CLAUDE_COMPACT_START_KEY) {
    return { contextEpoch: state.contextEpoch, lastCompactionKey: key };
  }
  return {
    contextEpoch: state.contextEpoch + 1,
    // ponytail: the marker is the record that this epoch is still waiting for its companion hook;
    // a Claude session whose SessionStart(compact) never arrives keeps it until the next compaction.
    lastCompactionKey: agent === 'claude' ? `${CLAUDE_COMPACT_POST_PREFIX}${key}` : key,
  };
}

function deterministicId(parts: string[]): string {
  return createHash('sha256').update(JSON.stringify(parts), 'utf8').digest('hex');
}

function metadataRow(fields: {
  agent: AgentName;
  nativeSessionId: string;
  kind: EventKind;
  eventName: string;
  capturedAt: number;
  payloadHash: string;
  payload: Record<string, unknown>;
  sensitivity?: 'local_only' | 'secret';
  classificationState?: 'partial' | 'failed';
  truncated?: 0 | 1;
}): RowDraft {
  return {
    // The payload hash keeps two different events of one session apart while a re-delivery of the
    // same bytes still collapses, the property R7 asks of every event key. A metadata-only row has
    // no normalized event, so no turn ordinal enters its id.
    identify: () =>
      deterministicId([
        'v1',
        fields.agent,
        fields.nativeSessionId,
        fields.kind,
        fields.eventName,
        fields.payloadHash,
      ]),
    agent: fields.agent,
    nativeSessionId: fields.nativeSessionId,
    kind: fields.kind,
    capturedAt: fields.capturedAt,
    content: null,
    contentHash: null,
    payload: fields.payload,
    sensitivity: fields.sensitivity ?? 'local_only',
    classificationState: fields.classificationState ?? 'failed',
    truncated: fields.truncated ?? 0,
  };
}

function eventRow(
  event: NormalizedEvent,
  detected: Extract<DetectorResult, { ok: true }>,
): RowDraft {
  const pathRuleHit = detected.pathRule !== null;
  const content = pathRuleHit ? null : contentText(event);
  const payload = payloadJson(event);
  if (pathRuleHit) payload.path_rule = detected.pathRule;
  return {
    identify: (turnOrdinal) => eventId(event, turnOrdinal),
    agent: event.agent,
    nativeSessionId: event.native_session_id,
    kind: event.kind,
    capturedAt: event.captured_at,
    source: event.kind === 'session_start' ? event.source : undefined,
    model: event.model,
    content,
    contentHash: content === null ? null : contentHash(content),
    payload,
    sensitivity: detected.sensitivity,
    classificationState: 'done',
    truncated: 0,
    // data-model "Spool entry": only a whole, detector-clean event has a spool representation.
    event: pathRuleHit ? undefined : event,
  };
}

function failedEventRow(event: NormalizedEvent, reason: string): RowDraft {
  // The detector did not finish, so no string of this event may be stored: every slot is blanked
  // before payload_json is taken (R4 fails closed on its own failure).
  for (const field of textFields(event)) field.write('');
  return {
    identify: (turnOrdinal) => eventId(event, turnOrdinal),
    agent: event.agent,
    nativeSessionId: event.native_session_id,
    kind: event.kind,
    capturedAt: event.captured_at,
    source: event.kind === 'session_start' ? event.source : undefined,
    model: event.model,
    content: null,
    contentHash: null,
    payload: { ...payloadJson(event), failure_reason: reason },
    sensitivity: 'local_only',
    classificationState: 'failed',
    truncated: 0,
  };
}

type SessionRow = {
  id: string;
  conversationId: string;
  turnCount: number;
  contextEpoch: number;
  lastCompactionKey: string | null;
  status: 'active' | 'ended';
};

function readSession(db: DatabaseSync, agent: AgentName, nativeSessionId: string): SessionRow | undefined {
  const row = db
    .prepare(
      'SELECT id, conversation_id, turn_count, context_epoch, last_compaction_key, status FROM sessions WHERE agent = ? AND native_session_id = ?',
    )
    .get(agent, nativeSessionId);
  if (row === undefined) return undefined;
  return {
    id: String(row.id),
    conversationId: String(row.conversation_id),
    turnCount: Number(row.turn_count ?? 0),
    contextEpoch: Number(row.context_epoch ?? 0),
    lastCompactionKey: row.last_compaction_key === null ? null : String(row.last_compaction_key),
    status: row.status === 'ended' ? 'ended' : 'active',
  };
}

/**
 * FR-024 and FR-010: a resumed session keeps its row (Claude `resume`, Grok `load`, a Codex or Pi
 * resume on the same id), so the first new event after `SessionEnd` reopens it. Without this the
 * row stays `ended` with its summary `done`, the finished summary keeps standing for the new work
 * and the worker cuts a session-end batch on every pass. `latest_summary_memory_id` stays until the
 * next summary replaces it.
 */
function reopenSession(db: DatabaseSync, session: SessionRow): void {
  if (session.status !== 'ended') return;
  db.prepare(
    `UPDATE sessions SET status = 'active', ended_at = NULL, summary_state = NULL WHERE id = ?`,
  ).run(session.id);
  session.status = 'active';
}

function upsertSession(db: DatabaseSync, row: RowDraft, repoId: string): SessionRow {
  const existing = readSession(db, row.agent, row.nativeSessionId);
  const decision = conversationPolicy({
    agent: row.agent,
    source: row.source,
    nativeSessionIdKnown: existing !== undefined,
  });

  if (existing === undefined) {
    const id = randomUUID();
    db.prepare(
      `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, model, started_at, status, turn_count, context_epoch)
       VALUES (?, ?, ?, ?, ?, ?, ?, 'active', 0, 0)`,
    ).run(id, repoId, row.agent, row.nativeSessionId, id, row.model ?? null, row.capturedAt);
    return {
      id,
      conversationId: id,
      turnCount: 0,
      contextEpoch: 0,
      lastCompactionKey: null,
      status: 'active',
    };
  }

  // An agent that reports a fresh conversation on a session id oboete already knows (Grok `new`)
  // detaches that row from its old root rather than joining the two (R7).
  if (decision === 'new_root' && existing.conversationId !== existing.id) {
    db.prepare('UPDATE sessions SET conversation_id = id WHERE id = ?').run(existing.id);
    existing.conversationId = existing.id;
  }
  if (row.model !== undefined) {
    db.prepare('UPDATE sessions SET model = ? WHERE id = ? AND model IS NULL').run(row.model, existing.id);
  }
  return existing;
}

function readEpoch(db: DatabaseSync, sessionId: string): CompactionState | null {
  const row = db
    .prepare('SELECT context_epoch, last_compaction_key FROM sessions WHERE id = ?')
    .get(sessionId);
  if (row === undefined) return null;
  return {
    contextEpoch: Number(row.context_epoch ?? 0),
    lastCompactionKey: row.last_compaction_key === null ? null : String(row.last_compaction_key),
  };
}

function advanceEpoch(
  db: DatabaseSync,
  session: SessionRow,
  row: RowDraft,
  eventIdentity: string,
): void {
  const event = row.event;
  // Only a whole, detector-clean compaction event may open an epoch: a metadata-only row cannot
  // be shown to distinguish two compactions (A16).
  if (event === undefined) return;

  // A12: the epoch lives on the root session of the conversation, which is the row this one
  // resumes from when it is not the root itself.
  const root =
    session.conversationId === session.id
      ? { contextEpoch: session.contextEpoch, lastCompactionKey: session.lastCompactionKey }
      : readEpoch(db, session.conversationId);
  if (root === null) return;

  const next = applyCompaction(row.agent, event, root, eventIdentity);
  if (next === null) return;
  db.prepare('UPDATE sessions SET context_epoch = ?, last_compaction_key = ? WHERE id = ?').run(
    next.contextEpoch,
    next.lastCompactionKey,
    session.conversationId,
  );
}

/**
 * The turn an event attaches to (R7's turn key): the turn a prompt opens, or the open turn of the
 * session. A session with no turn yet has none, and the key is then empty (events.ts eventIdKey).
 */
function turnOrdinalOf(row: RowDraft, session: SessionRow): number | undefined {
  if (row.kind === 'prompt') return session.turnCount + 1;
  return session.turnCount === 0 ? undefined : session.turnCount;
}

function placeInTurn(db: DatabaseSync, session: SessionRow, row: RowDraft): string | null {
  if (row.kind === 'prompt') {
    const ordinal = session.turnCount + 1;
    const id = randomUUID();
    db.prepare('INSERT INTO turns (id, session_id, ordinal, started_at) VALUES (?, ?, ?, ?)').run(
      id,
      session.id,
      ordinal,
      row.capturedAt,
    );
    db.prepare('UPDATE sessions SET turn_count = ? WHERE id = ?').run(ordinal, session.id);
    session.turnCount = ordinal;
    return id;
  }
  if (session.turnCount === 0) return null;

  const open = db
    .prepare('SELECT id FROM turns WHERE session_id = ? AND ordinal = ?')
    .get(session.id, session.turnCount);
  const turnId = open === undefined ? null : String(open.id);
  if (row.kind === 'turn_end' && turnId !== null) {
    db.prepare('UPDATE turns SET ended_at = ? WHERE id = ?').run(row.capturedAt, turnId);
  }
  return turnId;
}

/**
 * FR-021: a pack this database issued is removed from the content before it is stored, and the row
 * remembers which packs it carried. The spool path cannot look the hash up (no database); the
 * worker does it when it recovers the entry (batches.ts recoverSpool).
 */
function recognizePacks(db: DatabaseSync, rows: RowDraft[]): void {
  for (const row of rows) {
    if (row.content === null) continue;
    const recognized = stripRecognizedPacks(db, row.content);
    if (recognized.hashes.length === 0) continue;
    row.content = recognized.text;
    row.contentHash = contentHash(recognized.text);
    row.payload.recognized_packs = recognized.hashes;
  }
}

function storeRows(
  db: DatabaseSync,
  identity: RepoIdentity,
  rows: RowDraft[],
  diagnostics: Diagnostic[],
  capturedAt: number,
): { inserted: number; trigger: boolean } {
  for (const diagnostic of diagnostics) recordDiagnostic(db, diagnostic, capturedAt);
  if (rows.length === 0) return { inserted: 0, trigger: false };

  db.prepare(
    `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
     VALUES (?, ?, ?, ?, ?, ?)
     ON CONFLICT(id) DO UPDATE SET display_root = excluded.display_root, last_seen_at = excluded.last_seen_at`,
  ).run(
    identity.id,
    identity.identityKind,
    identity.normalizedIdentity,
    identity.root,
    capturedAt,
    capturedAt,
  );

  const seen = db.prepare('SELECT 1 AS present FROM raw_events WHERE id = ?');
  let inserted = 0;
  let trigger = false;
  for (const row of rows) {
    // The session is read before the id, because the id of an event with no per-turn value of its
    // own carries the ordinal of the turn it attaches to (R7, events.ts eventIdKey).
    const session = upsertSession(db, row, identity.id);
    const id = row.identify(turnOrdinalOf(row, session));
    // R7: the id carries no delivery counter, so a re-delivery is recognized here and changes
    // nothing else either - no second turn, no second epoch.
    if (seen.get(id) !== undefined) continue;

    // Only a row this session has not seen reopens it: a re-delivered `session_end` must not.
    reopenSession(db, session);
    advanceEpoch(db, session, row, id);
    const turnId = placeInTurn(db, session, row);
    if (row.kind === 'session_end') {
      db.prepare(
        `UPDATE sessions SET status = 'ended', ended_at = ?, summary_state = COALESCE(summary_state, 'pending') WHERE id = ?`,
      ).run(row.capturedAt, session.id);
    }

    db.prepare(
      `INSERT OR IGNORE INTO raw_events (
         id, repo_id, session_id, turn_id, agent, kind, content, truncated, payload_json,
         content_hash, sensitivity, classification_state, captured_at, expires_at, via_spool)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)`,
    ).run(
      id,
      identity.id,
      session.id,
      turnId,
      row.agent,
      row.kind,
      row.content,
      row.truncated,
      JSON.stringify(row.payload),
      row.contentHash,
      row.sensitivity,
      row.classificationState,
      row.capturedAt,
      row.capturedAt + RAW_EVENT_TTL_MS,
    );
    inserted += 1;
    trigger ||=
      BATCH_TRIGGER_KINDS.has(row.kind) ||
      (row.kind === 'prompt' && session.turnCount % TURN_BATCH === 0);
  }
  return { inserted, trigger };
}

function recordDiagnostic(db: DatabaseSync, diagnostic: Diagnostic, now: number): void {
  db.prepare(
    `INSERT INTO diagnostics (id, kind, severity, agent, message_code, count, first_seen_at, last_seen_at)
     VALUES (?, ?, 'warn', ?, ?, 1, ?, ?)
     ON CONFLICT(id) DO UPDATE SET count = count + 1, last_seen_at = excluded.last_seen_at, cleared_at = NULL`,
  ).run(
    deterministicId([diagnostic.kind, diagnostic.agent, diagnostic.messageCode]),
    diagnostic.kind,
    diagnostic.agent,
    diagnostic.messageCode,
    now,
    now,
  );
}

/**
 * The spool file of one row: the row itself plus the parent rows recovery needs, all of them values
 * this hook derived (data-model.md "Spool entry"). The hook did not read the database, so the
 * session id is a fresh one and the row carries no turn; recovery resolves both against the rows
 * that exist by then (worker/batches.ts ensureSessionRows).
 */
function spoolEntryFor(identity: RepoIdentity, row: RowDraft): SpoolEntry {
  const sessionId = randomUUID();
  return {
    repo: {
      id: identity.id,
      identity_kind: identity.identityKind,
      normalized_identity: identity.normalizedIdentity,
      display_root: identity.root,
    },
    session: {
      id: sessionId,
      repo_id: identity.id,
      agent: row.agent,
      native_session_id: row.nativeSessionId,
      conversation_id: sessionId,
      model: row.model ?? null,
      started_at: row.capturedAt,
      status: 'active',
    },
    row: {
      // R7: the ordinal the hook used, which on this path is none at all; recovery replays this id
      // and never recomputes it (events.ts eventIdKey).
      id: row.identify(),
      repo_id: identity.id,
      session_id: sessionId,
      turn_id: null,
      agent: row.agent,
      kind: row.kind,
      content: row.content,
      truncated: row.truncated,
      payload_json: JSON.stringify(row.payload),
      content_hash: row.contentHash,
      sensitivity: row.sensitivity,
      classification_state: row.classificationState,
      captured_at: row.capturedAt,
      expires_at: row.capturedAt + RAW_EVENT_TTL_MS,
    },
  };
}

function spoolAll(paths: OboetePaths, identity: RepoIdentity, rows: RowDraft[]): CaptureOutcome {
  let spooled = 0;
  let lost = 0;
  for (const row of rows) {
    try {
      writeSpoolEntry(paths, spoolEntryFor(identity, row));
      spooled += 1;
    } catch {
      lost += 1;
    }
  }
  if (lost > 0) {
    // R6: when neither the database nor the spool is writable the hook exits 0 and reports the
    // count to stderr, so the loss is visible without blocking the agent (FR-002).
    process.stderr.write(
      `oboete could not store ${lost} event${lost === 1 ? '' : 's'}: neither the database nor the spool could be written.\n`,
    );
  }
  return { outcome: spooled > 0 ? 'spooled' : 'dropped', rows: spooled };
}

const INJECTION_BRANCHES: Readonly<Record<AgentName, ReadonlySet<string>>> = {
  claude: new Set(['SessionStart', 'UserPromptSubmit']),
  codex: new Set(['SessionStart', 'UserPromptSubmit']),
  grok: new Set([
    'SessionStart',
    'UserPromptSubmit',
    'PreToolUse',
    'PostToolUse',
    'PostToolUseFailure',
    'PermissionDenied',
    'Stop',
  ]),
  pi: new Set(),
  unknown: new Set(),
};

const INJECTION_DEADLINE_EVENTS: Readonly<Record<AgentName, ReadonlySet<string>>> = {
  claude: new Set(['SessionStart', 'UserPromptSubmit']),
  codex: new Set(['SessionStart', 'UserPromptSubmit']),
  grok: new Set(['SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PostToolUse']),
  pi: new Set(),
  unknown: new Set(),
};

/** The outer hook deadline table; capture-only events retain the 300 ms deadline. */
export function hookDeadlineMs(agent: AgentName, eventName: string): number {
  return INJECTION_DEADLINE_EVENTS[agent].has(eventName)
    ? INJECTION_DEADLINE_MS
    : CAPTURE_DEADLINE_MS;
}

function currentTurnId(db: DatabaseSync, sessionId: string): string | null {
  const row = db
    .prepare(
      `SELECT id FROM turns WHERE session_id = ?
       AND ordinal = (SELECT turn_count FROM sessions WHERE id = ?)`,
    )
    .get(sessionId, sessionId);
  return row === undefined ? null : String(row.id);
}

async function injectAfterCapture(
  deps: CaptureDeps,
  paths: OboetePaths,
  identity: RepoIdentity,
  seed: InjectionSeed | undefined,
  db: DatabaseSync | undefined,
  sessionCreated = false,
): Promise<string> {
  if (seed === undefined || !INJECTION_BRANCHES[seed.event.agent].has(seed.eventName)) return '';

  const session =
    db === undefined
      ? undefined
      : readSession(db, seed.event.agent, seed.event.native_session_id);
  const conversationId = session?.conversationId ?? seed.event.native_session_id;
  const root = db === undefined ? null : readEpoch(db, conversationId);
  const context: HookContext = {
    agent: seed.event.agent,
    eventName: seed.eventName,
    event: seed.event,
    sessionId: session?.id ?? seed.event.native_session_id,
    conversationId,
    turnId: db === undefined || session === undefined ? null : currentTurnId(db, session.id),
    epoch: root?.contextEpoch ?? session?.contextEpoch ?? 0,
    repoId: identity.id,
    repoIdentityDisplay: identity.normalizedIdentity,
    repoRoot: identity.root,
    model: seed.event.model,
    cwd: seed.event.cwd,
    config: seed.config,
    paths,
    db,
    sessionCreated: session !== undefined && sessionCreated,
    secretPaths: seed.secretPaths,
    remainingBudget: () => hookDeadlineMs(seed.event.agent, seed.eventName) - deps.elapsedMs(),
  };
  try {
    return await (await import('./injection/inject.js')).injectForHook(context);
  } catch (error) {
    appendLogQuietly(paths.hookLog, 'error', 'injection failed', {
      agent: seed.event.agent,
      event: seed.eventName,
      reason: errorCode(error),
    });
    return '';
  }
}

async function write(
  deps: CaptureDeps,
  paths: OboetePaths,
  identity: RepoIdentity,
  rows: RowDraft[],
  diagnostics: Diagnostic[],
  capturedAt: number,
  injection: InjectionSeed | undefined,
  deadlineMs: number,
): Promise<CaptureOutcome> {
  const remaining = (): number => deadlineMs - deps.elapsedMs();
  if (rows.length === 0 && diagnostics.length === 0) return { outcome: 'dropped', rows: 0 };

  // contracts/agents.md: below the spool reserve the database is not opened at all.
  if (remaining() >= SPOOL_RESERVE_MS) {
    const timeoutMs = Math.max(
      1,
      Math.min(BUSY_TIMEOUT_CEILING_MS, Math.floor(remaining() - SPOOL_RESERVE_MS)),
    );
    let opened: ReturnType<typeof openDatabase> | null = null;
    try {
      opened = openDatabase({ path: paths.db, timeoutMs, hook: true });
      // data-model: the hook never migrates, so an older file is left to the worker.
      if (opened.schemaBehind) {
        opened.db.close();
        opened = null;
      }
    } catch {
      // A missing or unopenable database is an availability problem, not a privacy one (R1): the
      // sanitized event goes to the spool below.
    }

    if (opened !== null) {
      try {
        const sessionExisted =
          injection === undefined ||
          readSession(opened.db, injection.event.agent, injection.event.native_session_id) !== undefined;
        // One read-then-write unit, one transaction (conventions "Database access"): a failure
        // half way through would otherwise leave rows behind that the spool then writes again
        // under the id of another turn (R7: the direct path keys by the ordinal it read).
        let stored: ReturnType<typeof storeRows>;
        try {
          stored = transactionImmediate(opened.db, () => {
            recognizePacks(opened.db, rows);
            return storeRows(opened.db, identity, rows, diagnostics, capturedAt);
          });
        } catch {
          // R1: a storage failure before the transaction commits writes the sanitized event to the
          // spool. Injection then sees no database and records index_unavailable in the hook log.
          const outcome = spoolAll(paths, identity, rows);
          return {
            ...outcome,
            stdout: await injectAfterCapture(deps, paths, identity, injection, undefined),
          };
        }
        const stdout = await injectAfterCapture(
          deps,
          paths,
          identity,
          injection,
          opened.db,
          !sessionExisted,
        );
        try {
          if (stored.trigger && remaining() >= SPAWN_MIN_REMAINING_MS && isLeaseFree(opened.db, Date.now())) {
            // R6: a hook spawns the detached worker only when the lease is free or stale. The lease
            // is compared against the real clock, which is the clock its heartbeats are written on.
            deps.spawnWorker();
          }
        } catch {
          // The rows are committed already, so a failed spawn must not lose the outcome or the pack:
          // it is best-effort and the next hook retries it (FR-002, R6).
        }
        return {
          outcome: rows.length === 0 ? 'dropped' : 'stored',
          rows: stored.inserted,
          stdout,
        };
      } finally {
        opened.db.close();
      }
    }
  }
  const outcome = spoolAll(paths, identity, rows);
  return {
    ...outcome,
    stdout: await injectAfterCapture(deps, paths, identity, injection, undefined),
  };
}

/**
 * The capture path itself, without the process wiring, so the deadline clock, the detector and the
 * worker spawn can be supplied by a test. Never throws for a payload reason: every branch ends in a
 * row, a spool file, a diagnostics counter or a log line, and the caller always exits 0 (FR-002).
 */
export async function captureEvent(deps: CaptureDeps, input: CaptureInput): Promise<CaptureOutcome> {
  const { paths } = input;
  // R12: the paused marker is read before stdin and before the database.
  if (isPaused(paths)) return { outcome: 'paused', rows: 0 };
  try {
    ensureDirectories(paths);
  } catch {
    // FR-002: an unwritable data directory is an availability problem. The database and the spool
    // are tried anyway, so the loss is counted and reported on stderr instead of raising here.
  }

  const capturedAt = deps.now();
  const deadlineMs = hookDeadlineMs(input.agent, input.eventName);
  const persist = (
    identity: RepoIdentity,
    rows: RowDraft[],
    entries: Diagnostic[],
    injection?: InjectionSeed,
  ): Promise<CaptureOutcome> =>
    write(deps, paths, identity, rows, entries, capturedAt, injection, deadlineMs);
  const stdin = input.readStdin();
  const diagnostics: Diagnostic[] = (input.priorFailures ?? []).map((code) => ({
    kind: 'pi_child_failed',
    agent: input.agent,
    messageCode: code,
  }));
  const kindFromName = EVENT_KIND_BY_NAME[input.eventName];
  const payloadHash = contentHash(stdin.text);

  // FR-006: an invocation whose handler carries no fixed selector keeps `unknown` provenance, and
  // its payload is never read as an agent's payload; doctor reports the counter.
  if (input.agent === 'unknown') {
    diagnostics.push({ kind: 'unknown_agent', agent: 'unknown', messageCode: input.eventName || 'none' });
    const rows =
      kindFromName === undefined
        ? []
        : [
            metadataRow({
              agent: 'unknown',
              nativeSessionId: UNKNOWN_SESSION,
              kind: kindFromName,
              eventName: input.eventName,
              capturedAt,
              payloadHash,
              payload: { failure_reason: 'unknown_agent', event: input.eventName },
            }),
          ];
    // FR-004: the repository comes from the directory the hook runs in, never from a payload.
    return persist(resolveRepoIdentity(process.cwd(), gitOptions(deps, deadlineMs)), rows, diagnostics);
  }

  const agent: AdapterAgent = input.agent;
  let payload: unknown;
  let parsed = true;
  try {
    payload = JSON.parse(stdin.text);
  } catch {
    parsed = false;
  }

  if (!parsed) {
    return captureUnparsed(deps, input, {
      agent,
      stdin,
      capturedAt,
      diagnostics,
      kindFromName,
      payloadHash,
    }, deadlineMs);
  }

  const adapted: AdapterOutput = adapt({ agent, eventName: input.eventName, payload, capturedAt });
  if (adapted.kind === 'unmapped') {
    if (adapted.reason === 'event_not_captured') {
      return { outcome: 'not_captured', rows: 0, reason: adapted.reason };
    }
    const sessionId = adapted.metadata.nativeSessionId;
    if (sessionId === null || kindFromName === undefined) {
      diagnostics.push({ kind: 'unreadable_payload', agent, messageCode: input.eventName || 'none' });
      return persist(resolveRepoIdentity(process.cwd(), gitOptions(deps, deadlineMs)), [], diagnostics);
    }
    const row = metadataRow({
      agent,
      nativeSessionId: sessionId,
      kind: kindFromName,
      eventName: input.eventName,
      capturedAt,
      payloadHash,
      payload: {
        failure_reason: adapted.reason,
        event: input.eventName,
        tool_name: adapted.metadata.toolName ?? undefined,
      },
    });
    return persist(resolveRepoIdentity(process.cwd(), gitOptions(deps, deadlineMs)), [row], diagnostics);
  }

  const events = adapted.events;
  const first = events[0];
  if (first === undefined) return { outcome: 'not_captured', rows: 0 };
  // FR-004: the payload's `cwd` is used as a directory to run git in, never as an identity.
  const identity = resolveRepoIdentity(first.cwd, gitOptions(deps, deadlineMs));

  const settings = readSettings(paths, identity.root);
  if (settings === null) {
    // R4: a malformed configuration is a classification failure, so the event keeps metadata only.
    const rows = events.map((event) => failedEventRow(event, 'config_malformed'));
    return persist(identity, rows, diagnostics);
  }
  const injection: InjectionSeed = {
    eventName: input.eventName,
    event: events[events.length - 1] as NormalizedEvent,
    config: settings.config,
    secretPaths: settings.secretPaths,
  };

  const fields = events.flatMap((event) => textFields(event));
  const detected = await runDetector(deps, {
    fields: fields.map((field) => field.read()),
    paths: adapted.contentForDetector.paths,
    identity,
    secretPaths: settings.secretPaths,
  }, deadlineMs);

  if (!detected.ok) {
    const rows = events.map((event) => failedEventRow(event, detected.reason));
    return persist(identity, rows, diagnostics, injection);
  }
  if (detected.pathRule === null) {
    if (detected.texts.length !== fields.length) {
      // The detector answered a shape this build does not understand, which is a detector failure.
      const rows = events.map((event) => failedEventRow(event, 'detector_error'));
      return persist(identity, rows, diagnostics, injection);
    }
    fields.forEach((field, index) => field.write(detected.texts[index] as string));
  } else {
    // R4: a path-rule hit stores metadata only, so no string of the event survives.
    for (const field of fields) field.write('');
  }

  return persist(
    identity,
    events.map((event) => eventRow(event, detected)),
    diagnostics,
    injection,
  );
}

/**
 * A payload that did not parse. Above the read bound that is the expected case (A7): the read part
 * goes through the detector and is kept as a `partial` row with the kind of the `--event` argument
 * and the session id and paths of a bounded prefix scan. Below the bound the payload is broken, so
 * only its metadata is stored.
 */
async function captureUnparsed(
  deps: CaptureDeps,
  input: CaptureInput,
  context: {
    agent: AdapterAgent;
    stdin: StdinRead;
    capturedAt: number;
    diagnostics: Diagnostic[];
    kindFromName: EventKind | undefined;
    payloadHash: string;
  },
  deadlineMs: number,
): Promise<CaptureOutcome> {
  const { paths } = input;
  const scanned = scanPartialPrefix(context.agent, context.stdin.text);
  // FR-004: the repository is derived from where the hook runs, because the payload is unreadable.
  const identity = resolveRepoIdentity(process.cwd(), gitOptions(deps, deadlineMs));

  if (scanned.nativeSessionId === null || context.kindFromName === undefined) {
    // A7: without a recoverable session id nothing is stored and a counter is incremented.
    context.diagnostics.push({
      kind: 'partial_without_session',
      agent: context.agent,
      messageCode: input.eventName || 'none',
    });
    return write(deps, paths, identity, [], context.diagnostics, context.capturedAt, undefined, deadlineMs);
  }

  const base = {
    agent: context.agent,
    nativeSessionId: scanned.nativeSessionId,
    kind: context.kindFromName,
    eventName: input.eventName,
    capturedAt: context.capturedAt,
    payloadHash: context.payloadHash,
  };
  const metadata: Record<string, unknown> = {
    event: input.eventName,
    tool_name: scanned.toolName ?? undefined,
    paths: scanned.paths,
  };

  if (!context.stdin.truncated) {
    const row = metadataRow({ ...base, payload: { ...metadata, failure_reason: 'payload_invalid' } });
    return write(deps, paths, identity, [row], context.diagnostics, context.capturedAt, undefined, deadlineMs);
  }

  const settings = readSettings(paths, identity.root);
  if (settings === null) {
    const row = metadataRow({ ...base, payload: { ...metadata, failure_reason: 'config_malformed' } });
    return write(deps, paths, identity, [row], context.diagnostics, context.capturedAt, undefined, deadlineMs);
  }

  const detected = await runDetector(deps, {
    fields: [context.stdin.text],
    paths: scanned.paths,
    identity,
    secretPaths: settings.secretPaths,
  }, deadlineMs);
  if (!detected.ok) {
    const row = metadataRow({ ...base, payload: { ...metadata, failure_reason: detected.reason } });
    return write(deps, paths, identity, [row], context.diagnostics, context.capturedAt, undefined, deadlineMs);
  }

  const prefix = detected.pathRule === null ? (detected.texts[0] ?? '') : null;
  const row = metadataRow({
    ...base,
    payload: detected.pathRule === null ? metadata : { ...metadata, path_rule: detected.pathRule },
    // A7: a partial row is never promoted and never injected, which `partial` enforces downstream;
    // a secret the detector found in the prefix still classifies the row as secret (FR-017).
    sensitivity: detected.sensitivity,
    classificationState: 'partial',
    truncated: 1,
  });
  row.content = prefix;
  row.contentHash = prefix === null ? null : contentHash(prefix);
  return write(deps, paths, identity, [row], context.diagnostics, context.capturedAt, undefined, deadlineMs);
}

type CaptureSettings = { config: OboeteConfig; secretPaths: string[] };

/** The user configuration and repository rules, or null when either one is malformed (R4). */
function readSettings(paths: OboetePaths, repoRoot: string): CaptureSettings | null {
  try {
    const config = loadConfig(paths);
    return {
      config,
      secretPaths: [...config.privacy.secret_paths, ...loadRepoRules(repoRoot).secretPaths],
    };
  } catch (error) {
    if (error instanceof ConfigError || error instanceof RepoConfigError) return null;
    throw error;
  }
}

async function runDetector(
  deps: CaptureDeps,
  input: { fields: string[]; paths: string[]; identity: RepoIdentity; secretPaths: string[] },
  deadlineMs: number,
): Promise<DetectorResult> {
  const cutoff = Math.floor(
    deadlineMs - deps.elapsedMs() - SPOOL_RESERVE_MS - ROW_BUILD_MARGIN_MS,
  );
  // Nothing left to run the detector in, and unscanned content is never stored (FR-018).
  if (cutoff <= 0) return { ok: false, reason: 'deadline' };
  try {
    return await deps.detect(
      {
        text: '',
        fields: input.fields,
        paths: input.paths,
        repoRoot: input.identity.root,
        secretPaths: input.secretPaths,
        // FR-016: the hook's own credentials are secrets whatever they look like (log.ts).
        credentialValues: credentialValues(process.env),
      },
      cutoff,
    );
  } catch {
    return { ok: false, reason: 'detector_error' };
  }
}

// -- process wiring ---------------------------------------------------------------------------

export type CaptureRuntime = { deps: CaptureDeps; readStdin: () => StdinRead };

function sleep(milliseconds: number): void {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, milliseconds);
}

/** One chunk of standard input; the seam a test replaces to drive the read bound exactly. */
export type StdinReader = (target: Buffer, length: number) => number;

const fromStandardInput: StdinReader = (target, length) => readSync(0, target, 0, length, null);

/**
 * Reads at most `STDIN_READ_BOUND` bytes and stops; the rest is never drained, so capture time does
 * not grow with the payload (A7, A14). A pipe with nothing ready yet answers EAGAIN, which is
 * waited out in short steps rather than spun on.
 */
export function readStdinBounded(read: StdinReader = fromStandardInput): StdinRead {
  const chunks: Buffer[] = [];
  const buffer = Buffer.allocUnsafe(64 * 1_024);
  let total = 0;
  let waits = 0;

  // One byte past the bound is read but never stored: a payload of exactly the bound was not cut,
  // so only a byte beyond it makes the row partial (A7).
  while (total <= STDIN_READ_BOUND) {
    let taken: number;
    try {
      taken = read(buffer, Math.min(buffer.length, STDIN_READ_BOUND + 1 - total));
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code === 'EAGAIN' && waits < STDIN_WAIT_LIMIT) {
        waits += 1;
        sleep(1);
        continue;
      }
      break;
    }
    if (taken === 0) break;
    chunks.push(Buffer.from(buffer.subarray(0, taken)));
    total += taken;
  }
  return {
    text: Buffer.concat(chunks).subarray(0, STDIN_READ_BOUND).toString('utf8'),
    truncated: total > STDIN_READ_BOUND,
  };
}

function defaultRuntime(): CaptureRuntime {
  const bundlePath = process.argv[1] ?? '';
  return {
    deps: {
      detect: (input, cutoffMs) => detectInWorker(input, { cutoffMs, workerScript: bundlePath }),
      now: () => Date.now(),
      // performance.now() counts from process start, which is where the budget is measured from.
      elapsedMs: () => performance.now(),
      spawnWorker: () => {
        const child = spawn(process.execPath, [bundlePath, 'observe'], {
          detached: true,
          stdio: 'ignore',
        });
        // A spawn failure arrives as an asynchronous 'error' event after the hook has returned, so
        // without a listener it would throw past the exit-0 contract (FR-002); the spawn is
        // best-effort, and the next hook retries it.
        child.on('error', () => {});
        child.unref();
      },
    },
    readStdin: () => readStdinBounded(),
  };
}

function option(values: Record<string, unknown>, name: string): string | undefined {
  const value = values[name];
  return typeof value === 'string' && value !== '' ? value : undefined;
}

async function runCaptureCommand(
  input: Omit<CaptureInput, 'readStdin'>,
  runtime: CaptureRuntime,
): Promise<number> {
  let outcome: CaptureOutcome;
  try {
    outcome = await captureEvent(runtime.deps, { ...input, readStdin: runtime.readStdin });
  } catch (error) {
    // FR-002: the agent is never blocked, so every failure ends as one log line and exit 0.
    appendLogQuietly(input.paths.hookLog, 'error', 'capture failed', {
      agent: input.agent,
      event: input.eventName,
      reason: errorCode(error),
    });
    return 0;
  }

  if (outcome.outcome !== 'paused') {
    appendLogQuietly(input.paths.hookLog, 'info', 'capture', {
      agent: input.agent,
      event: input.eventName,
      outcome: outcome.outcome,
      rows: outcome.rows,
    });
  }
  if (outcome.stdout !== undefined && outcome.stdout !== '') process.stdout.write(outcome.stdout);
  return 0;
}

/** `oboete hook --agent codex|claude-or-grok --event <name>` (contracts/cli.md); always exits 0. */
export async function runHook(argv: string[], runtime: Partial<CaptureRuntime> = {}): Promise<number> {
  const { values } = parseArgs({
    args: argv,
    strict: false,
    allowPositionals: true,
    options: { agent: { type: 'string' }, event: { type: 'string' } },
  });
  const paths = oboetePaths(resolveHome());
  return runCaptureCommand(
    {
      agent: resolveAgent(option(values, 'agent'), process.env),
      eventName: option(values, 'event') ?? '',
      paths,
    },
    { ...defaultRuntime(), ...runtime },
  );
}

/**
 * `oboete capture --agent pi --event <name> --invocation <id> [--prior-failures <codes>]`: Pi's
 * detached capture child. The acknowledgement is written before stdin is read and renamed on the
 * way out, which is how doctor tells a hung child from a failed spawn (data-model "Pi", A8).
 */
export async function runCapture(
  argv: string[],
  runtime: Partial<CaptureRuntime> = {},
): Promise<number> {
  // FR-002: an unanticipated bug in the Pi capture child must still leave the agent unblocked.
  if (testFault('pi-throw')) throw new Error('OBOETE_TEST_FAULT: pi-throw');
  const { values } = parseArgs({
    args: argv,
    strict: false,
    allowPositionals: true,
    options: {
      agent: { type: 'string' },
      event: { type: 'string' },
      invocation: { type: 'string' },
      'prior-failures': { type: 'string' },
    },
  });
  const paths = oboetePaths(resolveHome());
  const invocation = option(values, 'invocation');
  const started = invocation === undefined ? null : join(paths.piAck, `${invocation}.started`);

  if (started !== null) {
    try {
      ensureDirectories(paths);
      writeFileSync(started, '', { mode: 0o600 });
    } catch {
      // FR-007: the acknowledgement is diagnostics; capture continues without it.
    }
  }

  const code = await runCaptureCommand(
    {
      agent: resolveAgent(option(values, 'agent'), process.env),
      eventName: option(values, 'event') ?? '',
      paths,
      priorFailures: (option(values, 'prior-failures') ?? '')
        .split(',')
        .map((entry) => entry.trim())
        .filter((entry) => entry !== ''),
    },
    { ...defaultRuntime(), ...runtime },
  );

  if (started !== null) {
    try {
      if (existsSync(started)) renameSync(started, started.replace(/\.started$/, '.done'));
    } catch {
      // The worker records a `.started` file older than 30 s as `pi_child_hang` (data-model "Pi").
    }
  }
  return code;
}
