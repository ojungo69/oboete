// Storage half of the User Story 2 failure matrix. Sources: quickstart.md "Failure injection",
// contracts/agents.md (capture deadline, spool reserve, busy timeout, A14 read bound),
// FR-002, FR-003, R1, R6, A7, A14. Engine defects stay failing for T063.
import assert from 'node:assert/strict';
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import type { TestContext } from 'node:test';

import { CAPTURE_DEADLINE_MS, STDIN_READ_BOUND } from '../src/capture.js';
import {
  claudePayload,
  fixture,
  observeLog,
  rows,
  run,
  runHook,
  scenario,
  SELECTOR,
  spawnEngine,
  spoolFiles,
  type Place,
  type SpawnResult,
} from './helpers/fault.js';

const EVENT = 'PostToolUse';
const TOOL_OUTPUT = 'oboete probe repository\nsecond line\n';
// Bound the recovery harness: a recovered mid-session row remains queued work (FR-009).
// After SessionEnd, a second observe must exit naturally with 0 or 1 (contracts/cli.md:15),
// a session_end batch covering the recovered row, and run end reason=empty.
const OBSERVE_MS = 2_000;

// The capture-only event kinds of src/agents/claude.ts (SessionStart and UserPromptSubmit take
// the injection branch under INJECTION_DEADLINE_MS and print a pack, so they are not in this table).
const CLAUDE_CAPTURE_EVENTS = [
  'PreToolUse',
  'PostToolUse',
  'PostToolUseFailure',
  'Stop',
  'PostCompact',
  'SessionEnd',
] as const;

function hookLogLines(place: Place): string[] {
  const file = join(place.home, 'logs', 'hook.log');
  return existsSync(file) ? readFileSync(file, 'utf8').trimEnd().split('\n') : [];
}

function parsedSpool(place: Place): Record<string, unknown> {
  const names = spoolFiles(place);
  assert.equal(names.length, 1, `spool files: ${names.join(',')}`);
  return JSON.parse(readFileSync(join(place.spool, names[0] as string), 'utf8')) as Record<
    string,
    unknown
  >;
}

function recover(place: Place): SpawnResult {
  return spawnEngine(['observe'], {
    home: place.home,
    cwd: place.repo,
    timeoutMs: OBSERVE_MS,
  });
}

/** SQLite may create -wal/-shm while the db is 0o400; those files inherit 0o400. */
function chmodDbWritable(place: Place): void {
  chmodSync(place.db, 0o600);
  for (const extra of [`${place.db}-wal`, `${place.db}-shm`]) {
    if (existsSync(extra)) chmodSync(extra, 0o600);
  }
}

function assertRecovered(t: TestContext, place: Place, recovered: SpawnResult): void {
  t.diagnostic(
    `observe status=${recovered.status} signal=${recovered.signal} elapsed=${recovered.elapsedMs.toFixed(1)} ms`,
  );
  // The recovered row is queued work, so this run may still be up at the harness bound (signal, status null).
  assert.ok(recovered.status === null || recovered.status === 0 || recovered.status === 1, `observe exited ${recovered.status}`);
  const stored = rows(place, 'SELECT id, via_spool FROM raw_events');
  assert.equal(
    stored.length,
    1,
    `raw_events after observe: ${stored.length}; status=${recovered.status} signal=${recovered.signal} stderr=${recovered.stderr}`,
  );
  assert.equal(Number(stored[0]?.via_spool), 1);
  assert.deepEqual(spoolFiles(place), []);

  // Keep SessionEnd from spawning a detached worker, then release the harness-held lease so
  // the second observe performs the batch itself. The recovery worker may have been SIGTERMed.
  const now = Date.now();
  run(place,
    'UPDATE worker_lease SET owner_token = ?, pid = 1, started_at = ?, heartbeat_at = ? WHERE id = 1',
    ['storage-recovery-harness', now, now],
  );
  runHook(t, place, SELECTOR, 'SessionEnd', claudePayload(place.repo, 'SessionEnd'));
  run(place,
    'UPDATE worker_lease SET owner_token = NULL, pid = NULL, started_at = NULL, heartbeat_at = NULL WHERE id = 1',
  );
  const ended = recover(place);
  t.diagnostic(
    `ended observe status=${ended.status} signal=${ended.signal} elapsed=${ended.elapsedMs.toFixed(1)} ms`,
  );
  assert.equal(ended.signal, null, 'observe must exit on its own, not be killed by the harness bound');
  assert.ok(ended.status === 0 || ended.status === 1, `observe exited ${ended.status}: ${ended.stderr}`);
  const covered = rows(place,
    `SELECT r.id, b.trigger FROM raw_events r
     JOIN observation_batches b ON b.id = r.batch_id WHERE r.via_spool = 1`,
  );
  assert.equal(covered.length, stored.length);
  assert.equal(covered[0]?.id, stored[0]?.id);
  assert.equal(covered[0]?.trigger, 'session_end');
  assert.match(observeLog(place), /run end .*reason=empty/);
}

function stdinAtReadBound(payload: Record<string, unknown>): string {
  const empty = JSON.stringify({ ...payload, wall: '' });
  const room = STDIN_READ_BOUND - empty.length;
  assert.ok(room > 0, `fixture payload is ${empty.length} bytes, already at or above the read bound`);
  const sent = JSON.stringify({ ...payload, wall: 'a'.repeat(room) });
  assert.equal(sent.length, STDIN_READ_BOUND);
  return sent;
}

function oversizedStdin(repo: string): string {
  const payload = claudePayload(repo);
  const content = `PROBE-${'N'.repeat(1_048_576)}`;
  const response = payload.tool_response as Record<string, unknown>;
  response.file = {
    filePath: `${repo}/README.md`,
    content,
    numLines: 1,
    startLine: 1,
    totalLines: 1,
  };
  const sent = JSON.stringify(payload);
  assert.ok(sent.length > 1_048_576, `stdin is ${sent.length} bytes`);
  assert.ok(sent.length > STDIN_READ_BOUND);
  return sent;
}

scenario('db-missing', (t: TestContext) => {
  const place = fixture();
  try {
    unlinkSync(place.db);

    runHook(t, place, SELECTOR, EVENT, claudePayload(place.repo));

    assert.equal(existsSync(place.db), false, 'a hook never migrates a missing database');
    const entry = parsedSpool(place);
    const row = entry.row as Record<string, unknown>;
    assert.equal(row.classification_state, 'done');
    assert.equal(row.content, TOOL_OUTPUT);
    const lines = hookLogLines(place);
    assert.equal(lines.length, 1);
    assert.match(lines[0] as string, /outcome=spooled/);

    // observe creates and migrates the database, then replays the spool (FR-003).
    assertRecovered(t, place, recover(place));
  } finally {
    place.cleanup();
  }
});

scenario('busy', (t: TestContext) => {
  const place = fixture();
  const holder = new DatabaseSync(place.db, { timeout: 2_000 });
  holder.exec('PRAGMA journal_mode = WAL');
  holder.exec('BEGIN IMMEDIATE');
  try {
    const elapsed = runHook(t, place, SELECTOR, EVENT, claudePayload(place.repo));
    // Busy timeout is min(150 ms, remaining - 40); the hook still has to exit inside the deadline.
    t.diagnostic(`busy hook elapsed ${elapsed.toFixed(1)} ms`);
    assert.equal(spoolFiles(place).length, 1);
    assert.equal(rows(place, 'SELECT id FROM raw_events').length, 0);

    holder.exec('ROLLBACK');
    holder.close();
    assertRecovered(t, place, recover(place));
  } finally {
    try {
      if (holder.isOpen) {
        holder.exec('ROLLBACK');
        holder.close();
      }
    } catch {
      // already rolled back
    }
    place.cleanup();
  }
});

scenario('corrupt', (t: TestContext) => {
  const place = fixture();
  try {
    const junk = Buffer.from('this is not a SQLite database\n');
    writeFileSync(place.db, junk);
    const before = Buffer.from(readFileSync(place.db));

    runHook(t, place, SELECTOR, EVENT, claudePayload(place.repo));

    assert.equal(spoolFiles(place).length, 1);
    assert.ok(before.equals(readFileSync(place.db)), 'corrupt bytes must be left untouched');

    const observed = recover(place);
    // any openDatabase throw returns 3 (src/worker/observe.ts); the spool file stays.
    assert.equal(observed.status, 3, `observe exited ${observed.status}: ${observed.stderr}`);
    assert.equal(spoolFiles(place).length, 1);
  } finally {
    place.cleanup();
  }
});

scenario('readonly', (t: TestContext) => {
  if (process.getuid?.() === 0) {
    t.skip('the root user writes into a file without write permission');
    return;
  }
  const place = fixture();
  try {
    chmodSync(place.db, 0o400);
    runHook(t, place, SELECTOR, EVENT, claudePayload(place.repo));
    // Fails at the transaction, not at open, so a -wal/-shm pair may appear: do not assert on those.
    assert.equal(spoolFiles(place).length, 1);
    assert.equal(rows(place, 'SELECT id FROM raw_events').length, 0);

    chmodDbWritable(place);
    assertRecovered(t, place, recover(place));
  } finally {
    place.cleanup();
  }
});

// There is no unprivileged way to fill a disk and no seam, so this name is an ALIAS for the
// complete storage-write failure: memory.db read-only AND the spool directory chmod 0o500.
scenario('enospc', (t: TestContext) => {
  if (process.getuid?.() === 0) {
    t.skip('the root user writes into a directory without write permission');
    return;
  }
  const place = fixture();
  try {
    mkdirSync(place.spool, { recursive: true, mode: 0o700 });
    chmodSync(place.db, 0o400);
    chmodSync(place.spool, 0o500);

    const result = spawnEngine(['hook', '--agent', SELECTOR, '--event', EVENT], {
      home: place.home,
      cwd: place.repo,
      input: JSON.stringify(claudePayload(place.repo)),
      timeoutMs: 5_000,
    });
    assert.equal(result.status, 0, `the hook exited ${result.status}: ${result.stderr}`);
    assert.equal(result.stdout, '');
    assert.ok(
      result.elapsedMs < 2 * CAPTURE_DEADLINE_MS,
      `the hook took ${result.elapsedMs.toFixed(1)} ms`,
    );
    t.diagnostic(
      `hook --agent ${SELECTOR} --event ${EVENT} with enospc alias took ${result.elapsedMs.toFixed(1)} ms`,
    );
    assert.equal(rows(place, 'SELECT id FROM raw_events').length, 0);
    assert.deepEqual(spoolFiles(place), []);
    const lines = hookLogLines(place);
    assert.equal(lines.length, 1);
    assert.match(lines[0] as string, /outcome=dropped/);
    assert.match(result.stderr, /could not store 1 event/);

    chmodDbWritable(place);
    chmodSync(place.spool, 0o700);
    runHook(t, place, SELECTOR, EVENT, claudePayload(place.repo));
    assert.equal(rows(place, 'SELECT id FROM raw_events').length, 1);
    assert.deepEqual(spoolFiles(place), []);
  } finally {
    place.cleanup();
  }
});

scenario('oversized-payload', (t: TestContext) => {
  const place = fixture();
  try {
    const sent = oversizedStdin(place.repo);
    runHook(t, place, SELECTOR, EVENT, sent);

    const stored = rows(
      place,
      'SELECT classification_state, truncated, content FROM raw_events',
    );
    assert.equal(stored.length, 1);
    assert.equal(stored[0]?.classification_state, 'partial');
    assert.equal(Number(stored[0]?.truncated), 1);
    const content = stored[0]?.content;
    assert.equal(typeof content, 'string');
    assert.ok((content as string).length <= STDIN_READ_BOUND);
    assert.ok((content as string).length < sent.length, 'the stored prefix must be proper');
    assert.equal(sent.slice(0, (content as string).length), content);
    assert.deepEqual(spoolFiles(place), []);
  } finally {
    place.cleanup();
  }
});

scenario('detector-never-returns', (t: TestContext) => {
  const place = fixture();
  try {
    const elapsed = runHook(t, place, SELECTOR, EVENT, claudePayload(place.repo), {
      fault: 'detector-never-returns',
    });
    t.diagnostic(`detector-never-returns elapsed ${elapsed.toFixed(1)} ms`);

    const stored = rows(
      place,
      'SELECT classification_state, content, payload_json FROM raw_events',
    );
    assert.equal(stored.length, 1);
    assert.equal(stored[0]?.classification_state, 'failed');
    assert.equal(stored[0]?.content, null);
    const payload = JSON.parse(String(stored[0]?.payload_json)) as Record<string, unknown>;
    assert.equal(payload.failure_reason, 'deadline');
    assert.equal(
      String(stored[0]?.payload_json).includes('oboete probe repository'),
      false,
      'blanked detector fields must not leak the tool output into payload_json',
    );
  } finally {
    place.cleanup();
  }
});

scenario(
  'detector-never-returns',
  'detector-never-returns: slow detector + busy database wall time per event kind',
  (t: TestContext) => {
    const place = fixture();
    const holder = new DatabaseSync(place.db, { timeout: 2_000 });
    holder.exec('PRAGMA journal_mode = WAL');
    holder.exec('BEGIN IMMEDIATE');
    try {
      for (const kind of CLAUDE_CAPTURE_EVENTS) {
        const elapsed = runHook(
          t,
          place,
          SELECTOR,
          kind,
          stdinAtReadBound(claudePayload(place.repo, kind)),
          { fault: 'detector-never-returns' },
        );
        t.diagnostic(`${kind}: ${elapsed.toFixed(1)} ms`);
      }
    } finally {
      try {
        holder.exec('ROLLBACK');
        holder.close();
      } catch {
        // already closed
      }
      place.cleanup();
    }
  },
);
