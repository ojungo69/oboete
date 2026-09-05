// Grok half of the User Story 2 failure matrix: the deferred delivery ledger under every way a
// tool call can end. Sources: quickstart.md "Failure injection", contracts/agents.md (Grok rows,
// FR-045, A15), src/injection/deferred.ts, test/contracts/grok/*.json (R13 probes 2026-09-03).
// No seam: every case is the real hook sequence through dist/oboete.mjs with GROK_SESSION_ID set.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import type { TestContext } from 'node:test';

import { hookDeadlineMs } from '../src/capture.js';
import { openDatabase } from '../src/db/open.js';
import { resolveRepoIdentity } from '../src/repo-identity.js';
import { claimLease } from '../src/worker/lease.js';
import { purgeExpiredEvents } from '../src/worker/purge.js';
import { fixture, rows, scenario, SELECTOR, spawnEngine, type Place } from './helpers/fault.js';

type Json = Record<string, unknown>;

// Grok reports no model, so every Grok pack is budgeted on the smallest verified window and
// carries window_unknown (contracts/agents.md "budget"); an omitted pack keeps that pack-time
// reason over not_delivered / no_tool_call (FR-028), which then show on the items instead.
const GROK_PACK_REASON = 'window_unknown';
const NATIVE = 'grok-fault-session';
const NOW = Date.now();
const FIXTURES = join(process.cwd(), 'test', 'contracts', 'grok');

function grokFixture(file: string, key: string): Json {
  const parsed = JSON.parse(readFileSync(join(FIXTURES, file), 'utf8')) as Json;
  const found = key.split('.').reduce<unknown>((value, part) => (value as Json)[part], parsed);
  assert.ok(found !== undefined, `${file} has no ${key}`);
  return structuredClone(found) as Json;
}

/** A recorded Grok payload re-addressed to this fixture's repository and session. */
function payload(place: Place, file: string, key: string, patch: Json = {}): Json {
  const body = grokFixture(file, key);
  body.sessionId = NATIVE;
  body.cwd = place.repo;
  body.workspaceRoot = `${place.repo}/`;
  return { ...body, ...patch };
}

/** An ended session with a summary memory, so a Grok turn has exactly one pack item to deliver. */
function seed(place: Place): void {
  const identity = resolveRepoIdentity(place.repo);
  const db = openDatabase({ path: place.db, timeoutMs: 5_000 }).db;
  try {
    db.prepare(
      `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
       VALUES (?, ?, ?, ?, ?, ?)`,
    ).run(identity.id, identity.identityKind, identity.normalizedIdentity, identity.root, NOW, NOW);
    db.prepare(
      `INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash,
         content_hash, sensitivity, review_state, created_at)
       VALUES ('m-summary', ?, 'session_summary', 'Previous session', ?, '', 'ms', 'cs',
         'eligible', 'unreviewed', ?)`,
    ).run(identity.id, 'The previous migration completed.', NOW - 2_000);
    db.prepare(
      `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, model,
         started_at, ended_at, status, turn_count, latest_summary_memory_id, summary_state)
       VALUES ('s-previous', ?, 'grok', 'previous-native', 's-previous', 'grok-4',
         ?, ?, 'ended', 1, 'm-summary', 'done')`,
    ).run(identity.id, NOW - 10_000, NOW - 1_000);
  } finally {
    db.close();
  }
}

/** One Grok hook: exit 0 inside its own deadline table (injection hooks get 1.3 s). */
function grokHook(t: TestContext, place: Place, event: string, body: Json): string {
  const result = spawnEngine(['hook', '--agent', SELECTOR, '--event', event], {
    home: place.home,
    cwd: place.repo,
    input: JSON.stringify(body),
    extraEnv: { GROK_SESSION_ID: NATIVE },
    timeoutMs: 5_000,
  });
  const deadline = hookDeadlineMs('grok', event);
  t.diagnostic(`grok ${event} took ${result.elapsedMs.toFixed(1)} ms (deadline ${deadline} ms)`);
  assert.equal(result.status, 0, `${event} exited ${result.status} signal=${result.signal}: ${result.stderr}`);
  assert.ok(result.elapsedMs < 2 * deadline, `${event} took ${result.elapsedMs.toFixed(1)} ms`);
  return result.stdout;
}

function envelope(stdout: string, event: string): string {
  const parsed = JSON.parse(stdout) as { hookSpecificOutput: { hookEventName: string; additionalContext: string } };
  assert.equal(parsed.hookSpecificOutput.hookEventName, event);
  assert.ok(parsed.hookSpecificOutput.additionalContext.startsWith('oboete memory context'));
  return parsed.hookSpecificOutput.additionalContext;
}

type Attempt = { tool_call_id: string; execution: string; delivery: string };
type Ledger = {
  state: string;
  degraded_reason: string | null;
  delivery_count: number;
  attempts: Attempt[];
  items: { memory_id: string | null; decision: string; reason: string | null }[];
};

/** The one grok_deferred ledger row of the fixture and its items, in a comparable shape. */
function ledger(place: Place): Ledger {
  const found = rows(
    place,
    `SELECT id, state, degraded_reason, delivery_count, attempts_json FROM injections WHERE kind = 'grok_deferred'`,
  );
  assert.equal(found.length, 1, `grok_deferred rows: ${found.length}`);
  const row = found[0] as Json;
  // node:sqlite rows have a null prototype; copy them so deepEqual compares values only.
  const items = rows(
    place,
    `SELECT memory_id, decision, reason FROM injection_items WHERE injection_id = '${String(row.id)}' ORDER BY memory_id, decision`,
  ).map((item) => ({
    memory_id: item.memory_id as string | null,
    decision: String(item.decision),
    reason: item.reason as string | null,
  }));
  return {
    state: String(row.state),
    degraded_reason: row.degraded_reason === null ? null : String(row.degraded_reason),
    delivery_count: Number(row.delivery_count),
    // `at` is a wall-clock stamp; the outcome pair is what the matrix asserts.
    attempts: (JSON.parse(String(row.attempts_json ?? '[]')) as (Attempt & { at?: number })[]).map(
      ({ tool_call_id, execution, delivery }) => ({ tool_call_id, execution, delivery }),
    ),
    items,
  };
}

function attempt(id: string, execution: string, delivery: string): Attempt {
  return { tool_call_id: id, execution, delivery };
}

function preToolUse(place: Place, id: string): Json {
  return payload(place, 'run_terminal_command.json', 'events.PreToolUse', { toolUseId: id });
}

function postToolUse(place: Place, id: string): Json {
  return payload(place, 'run_terminal_command.json', 'events.PostToolUse', { toolUseId: id });
}

function start(t: TestContext, place: Place): void {
  const stdout = grokHook(t, place, 'SessionStart', payload(place, 'session-start-resume.json', 'A'));
  assert.equal(stdout, '', 'Grok gets nothing at session start: delivery is deferred to a tool call (FR-045)');
}

function includedOnce(book: Ledger, memoryId: string): void {
  const included = book.items.filter((item) => item.memory_id === memoryId && item.decision === 'included');
  assert.equal(included.length, 1, `${memoryId} included ${included.length} times`);
}

scenario('grok-success', (t: TestContext) => {
  const place = fixture();
  try {
    seed(place);
    start(t, place);
    const pack = envelope(grokHook(t, place, 'PreToolUse', preToolUse(place, 'call-1')), 'PreToolUse');
    assert.ok(pack.includes('previous migration'), pack);
    assert.equal(grokHook(t, place, 'PostToolUse', postToolUse(place, 'call-1')), '');

    const book = ledger(place);
    assert.equal(book.state, 'emitted');
    assert.equal(book.degraded_reason, GROK_PACK_REASON);
    assert.equal(book.delivery_count, 1);
    assert.deepEqual(book.attempts, [attempt('call-1', 'ran', 'delivered')]);
    assert.deepEqual(book.items, [{ memory_id: 'm-summary', decision: 'included', reason: 'summary' }]);

    // The ledger outlives the raw events it was built from (rule 2): backdate them and purge under
    // a lease; a summarizable row with content waits for a batch (FR-008), lifecycle rows go.
    const db = new DatabaseSync(place.db, { timeout: 5_000 });
    try {
      db.prepare('UPDATE raw_events SET expires_at = ?').run(NOW - 1);
      const token = claimLease(db, { pid: process.pid, now: NOW });
      assert.ok(token !== null, 'the test must hold the worker lease to purge');
      const purged = purgeExpiredEvents(db, token, NOW);
      assert.ok(purged.deleted > 0, 'the session had raw events to purge');
      assert.equal(purged.leaseLost, false);
      assert.equal(db.prepare("SELECT COUNT(*) AS n FROM raw_events WHERE kind = 'session_start'").get()?.n, 0);
    } finally {
      db.close();
    }
    assert.deepEqual(ledger(place), book);
  } finally {
    place.cleanup();
  }
});

scenario('grok-exec-failure', (t: TestContext) => {
  const place = fixture();
  try {
    seed(place);
    start(t, place);
    envelope(grokHook(t, place, 'PreToolUse', preToolUse(place, 'call-1')), 'PreToolUse');
    // exit_code 3: the call ran and failed, which still delivered the pack (FR-045 rule 3).
    const failed = payload(place, 'posttooluse-failure.json', 'PostToolUse', { toolUseId: 'call-1' });
    assert.equal(grokHook(t, place, 'PostToolUse', failed), '');

    const book = ledger(place);
    assert.equal(book.state, 'emitted');
    assert.equal(book.delivery_count, 1);
    assert.deepEqual(book.attempts, [attempt('call-1', 'failed', 'delivered')]);
    assert.deepEqual(book.items, [{ memory_id: 'm-summary', decision: 'included', reason: 'summary' }]);
  } finally {
    place.cleanup();
  }
});

scenario('grok-oboete-deny', (t: TestContext) => {
  const place = fixture();
  try {
    seed(place);
    start(t, place);
    envelope(grokHook(t, place, 'PreToolUse', preToolUse(place, 'call-1')), 'PreToolUse');
    // Grok's own permission rule denied call-1 after the pack was attached: nothing was delivered.
    const denied = payload(place, 'permission-denied.json', 'permissionRule', { toolUseId: 'call-1' });
    assert.equal(grokHook(t, place, 'PermissionDenied', denied), '');
    envelope(grokHook(t, place, 'PreToolUse', preToolUse(place, 'call-2')), 'PreToolUse');
    assert.equal(grokHook(t, place, 'PostToolUse', postToolUse(place, 'call-2')), '');

    const book = ledger(place);
    assert.equal(book.state, 'emitted');
    assert.equal(book.delivery_count, 1, 'only the call that ran delivered the pack');
    assert.deepEqual(book.attempts, [
      attempt('call-1', 'denied', 'dropped'),
      attempt('call-2', 'ran', 'delivered'),
    ]);
    includedOnce(book, 'm-summary');
  } finally {
    place.cleanup();
  }
});

scenario('grok-other-handler-deny', (t: TestContext) => {
  const place = fixture();
  try {
    seed(place);
    start(t, place);
    envelope(grokHook(t, place, 'PreToolUse', preToolUse(place, 'call-1')), 'PreToolUse');
    // A sibling handler stopped the chain: neither PostToolUse nor PermissionDenied ever fires for
    // call-1, and the turn ends. Stop must arrive before any purge, or this degrades into no_tool_call.
    assert.equal(grokHook(t, place, 'Stop', payload(place, 'stop-end-turn.json', 'end_turn')), '');

    const book = ledger(place);
    assert.equal(book.state, 'omitted');
    assert.equal(book.degraded_reason, GROK_PACK_REASON);
    assert.equal(book.delivery_count, 0);
    assert.deepEqual(book.attempts, [attempt('call-1', 'pending', 'dropped')]);
    assert.ok(
      book.items.every((item) => item.decision === 'omitted' && item.reason === 'not_delivered'),
      JSON.stringify(book.items),
    );
  } finally {
    place.cleanup();
  }
});

scenario('grok-parallel-batch', (t: TestContext) => {
  const place = fixture();
  try {
    seed(place);
    start(t, place);
    // Both PreToolUse hooks run before either call finishes (R13 probe): each call carries the
    // pack, both deliveries are counted (A15), the shared item is included once.
    envelope(grokHook(t, place, 'PreToolUse', preToolUse(place, 'call-a')), 'PreToolUse');
    envelope(grokHook(t, place, 'PreToolUse', preToolUse(place, 'call-b')), 'PreToolUse');
    assert.equal(grokHook(t, place, 'PostToolUse', postToolUse(place, 'call-a')), '');
    assert.equal(grokHook(t, place, 'PostToolUse', postToolUse(place, 'call-b')), '');

    const book = ledger(place);
    assert.equal(book.state, 'emitted');
    assert.equal(book.delivery_count, 2);
    assert.deepEqual(book.attempts, [
      attempt('call-a', 'ran', 'delivered'),
      attempt('call-b', 'ran', 'delivered'),
    ]);
    includedOnce(book, 'm-summary');
  } finally {
    place.cleanup();
  }
});

scenario('grok-no-tool', (t: TestContext) => {
  const place = fixture();
  try {
    seed(place);
    start(t, place);
    assert.equal(grokHook(t, place, 'Stop', payload(place, 'stop-end-turn.json', 'end_turn')), '');

    const before = ledger(place);
    assert.equal(before.state, 'omitted');
    assert.equal(before.degraded_reason, GROK_PACK_REASON);
    assert.equal(before.delivery_count, 0);
    assert.deepEqual(before.attempts, []);
    assert.deepEqual(before.items, [{ memory_id: 'm-summary', decision: 'omitted', reason: 'not_delivered' }]);
  } finally {
    place.cleanup();
  }
});
