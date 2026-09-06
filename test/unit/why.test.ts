import assert from 'node:assert/strict';
import type { DatabaseSync } from 'node:sqlite';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { after, test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import {
  confirmDelivery,
  createInjection,
  planItems,
  type LedgerItem,
  type NewInjection,
  type WhyAttempt,
} from '../../src/injection/ledger.js';
import { DEGRADED_SENTENCES } from '../../src/injection/pack.js';
import type { MemoryCliRuntime } from '../../src/memories-cli.js';
import { oboetePaths } from '../../src/paths.js';
import { resolveRepoIdentity } from '../../src/repo-identity.js';
import { runWhy } from '../../src/why.js';
import { withTempHome } from '../helpers/home.js';

const NOW = 1_800_000_000_000;
// A plain directory is a repository of kind "path"; the why command derives the repository from cwd.
const REPO_DIR = mkdtempSync(join(tmpdir(), 'oboete-why-repo-'));
after(() => rmSync(REPO_DIR, { recursive: true, force: true }));
const REPO = resolveRepoIdentity(REPO_DIR).id;
const SESSION = 's_why';
const NATIVE = 'native_why';
const TURN = 't_why_1';
const BODIES = {
  pin: 'PINNED_BODY_MUST_NOT_APPEAR in any why text.',
  hit: 'MATCHED_BODY_MUST_NOT_APPEAR in any why text.',
  budget: 'BUDGET_BODY_MUST_NOT_APPEAR in any why text.',
};

type Command = typeof runWhy;

async function run(
  command: Command,
  argv: string[],
): Promise<{ status: number; stdout: string; stderr: string }> {
  let stdout = '';
  let stderr = '';
  const status = await command(argv, {
    cwd: REPO_DIR,
    now: () => NOW,
    writeOut: (text) => {
      stdout += text;
    },
    writeError: (text) => {
      stderr += text;
    },
  } satisfies Partial<MemoryCliRuntime>);
  return { status, stdout, stderr };
}

function insertRepo(db: DatabaseSync): void {
  db.prepare(
    `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
     VALUES (?, 'remote', 'example.test/why', '/tmp/why', 1, 1)`,
  ).run(REPO);
}

function insertSession(
  db: DatabaseSync,
  session: { id: string; agent: 'claude' | 'grok' | 'codex' | 'pi'; native: string },
): void {
  db.prepare(
    `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, model,
       started_at, status, turn_count, context_epoch)
     VALUES (?, ?, ?, ?, ?, 'claude-opus-5', ?, 'active', 1, 0)`,
  ).run(session.id, REPO, session.agent, session.native, session.id, NOW - 10_000);
}

function insertTurn(db: DatabaseSync, id: string, sessionId: string, ordinal: number): void {
  db.prepare('INSERT INTO turns (id, session_id, ordinal, started_at) VALUES (?, ?, ?, ?)').run(
    id,
    sessionId,
    ordinal,
    NOW - 1_000,
  );
}

function insertMemory(db: DatabaseSync, seed: { id: string; title: string; body: string }): void {
  db.prepare(
    `INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash,
       content_hash, sensitivity, review_state, created_at)
     VALUES (?, ?, 'discovery', ?, ?, '', ?, ?, 'eligible', 'unreviewed', ?)`,
  ).run(seed.id, REPO, seed.title, seed.body, `material_${seed.id}`, `content_${seed.id}`, NOW);
}

function item(partial: LedgerItem): LedgerItem {
  return partial;
}

function seedMemories(db: DatabaseSync): void {
  insertMemory(db, { id: 'm_pin', title: 'Pinned note', body: BODIES.pin });
  insertMemory(db, { id: 'm_hit', title: 'Matched note', body: BODIES.hit });
  insertMemory(db, { id: 'm_budget', title: 'Budget note', body: BODIES.budget });
}

function injectionRow(overrides: Partial<NewInjection> = {}): NewInjection {
  return {
    repoId: REPO,
    sessionId: SESSION,
    conversationId: SESSION,
    turnId: TURN,
    kind: 'prompt',
    channel: 'claude:UserPromptSubmit',
    state: 'emitted',
    epoch: 0,
    packHash: 'hash-why',
    charBudget: 1_000,
    charsUsed: 400,
    degradedReason: null,
    createdAt: NOW,
    ...overrides,
  };
}

function seedPromptPack(db: DatabaseSync, id = 'inj-prompt'): string {
  const injectionId = createInjection(db, injectionRow({ id }));
  planItems(db, { id: injectionId, conversationId: SESSION, epoch: 0 }, [
    item({
      sourceKind: 'memory',
      memoryId: 'm_pin',
      rawEventId: null,
      decision: 'included',
      reason: 'pinned',
      rank: 1,
      stale: 0,
    }),
    item({
      sourceKind: 'memory',
      memoryId: 'm_hit',
      rawEventId: null,
      decision: 'included',
      reason: null,
      rank: 2,
      stale: 0,
    }),
    // The session summary is included without a rank, the way pack.ts records it.
    item({
      sourceKind: 'session_summary',
      memoryId: null,
      rawEventId: null,
      decision: 'included',
      reason: 'summary',
      rank: null,
      stale: 0,
    }),
    item({
      sourceKind: 'memory',
      memoryId: 'm_budget',
      rawEventId: null,
      decision: 'omitted',
      reason: 'budget',
      rank: null,
      stale: 0,
    }),
    item({
      sourceKind: 'memory',
      memoryId: null,
      rawEventId: null,
      decision: 'omitted',
      reason: 'stale_path',
      rank: null,
      stale: 1,
    }),
  ]);
  confirmDelivery(db, injectionId, NOW + 5);
  return injectionId;
}

function seedDegradedStart(db: DatabaseSync): void {
  const id = createInjection(
    db,
    injectionRow({
      id: 'inj-start',
      turnId: null,
      kind: 'session_start',
      channel: 'claude:SessionStart',
      state: 'omitted',
      packHash: null,
      charsUsed: 0,
      degradedReason: 'summary_pending',
      createdAt: NOW - 1_000,
    }),
  );
  planItems(db, { id, conversationId: SESSION, epoch: 0 }, []);
}

function seedDeferred(db: DatabaseSync, turnId = 't_why_2'): void {
  const id = createInjection(
    db,
    injectionRow({
      id: 'inj-grok',
      turnId,
      kind: 'grok_deferred',
      channel: 'grok:PreToolUse',
      state: 'emitted',
      packHash: 'hash-grok',
      createdAt: NOW + 1_000,
    }),
  );
  planItems(db, { id, conversationId: SESSION, epoch: 0 }, []);
  const attempts: WhyAttempt[] = [
    { tool_call_id: 'call_denied', execution: 'denied', delivery: 'pending', at: NOW + 2 },
    { tool_call_id: 'call_ok', execution: 'ran', delivery: 'delivered', at: NOW + 3 },
  ];
  db.prepare('UPDATE injections SET attempts_json = ?, delivery_count = ? WHERE id = ?').run(
    JSON.stringify(attempts),
    1,
    id,
  );
}

async function withSeeded(
  fn: (db: DatabaseSync) => void,
  runArgv: (home: string) => Promise<void>,
): Promise<void> {
  await withTempHome(async (home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 2_000 });
    try {
      insertRepo(opened.db);
      insertSession(opened.db, { id: SESSION, agent: 'claude', native: NATIVE });
      insertTurn(opened.db, TURN, SESSION, 1);
      seedMemories(opened.db);
      fn(opened.db);
    } finally {
      opened.db.close();
    }
    await runArgv(home);
  });
}

function assertNoBodies(text: string): void {
  for (const body of Object.values(BODIES)) {
    assert.equal(text.includes(body), false, `why printed a memory body: ${body}`);
  }
}

test('why lists included and omitted items with trim and stale notes', async () => {
  await withSeeded(
    (db) => {
      seedPromptPack(db);
    },
    async () => {
      const result = await run(runWhy, [SESSION]);
      assert.equal(result.status, 0);
      assert.equal(result.stderr, '');
      assert.match(
        result.stdout,
        /prompt pack \(claude:UserPromptSubmit\) — emitted, epoch 0, turn 1, built /,
      );
      assert.match(result.stdout, /trimmed: 1 candidates omitted for budget/);
      assert.match(result.stdout, /included:/);
      assert.match(result.stdout, /1\. Pinned note — It is pinned, so it is always included\./);
      assert.match(result.stdout, /2\. Matched note — It matched the prompt\./);
      assert.match(result.stdout, /^ {4}session summary — It is the summary of the previous session\.$/m);
      assert.doesNotMatch(result.stdout, /null\./);
      assert.match(result.stdout, /omitted:/);
      assert.match(
        result.stdout,
        /Budget note — The character budget was already used by higher-ranked notes\./,
      );
      assert.match(result.stdout, /It cites a file that no longer exists at HEAD\./);
      assert.match(result.stdout, /stale: path or commit no longer at HEAD/);
      assertNoBodies(result.stdout + result.stderr);
    },
  );
});

test('why prints the degraded sentence for an omitted session-start pack', async () => {
  await withSeeded(
    (db) => {
      seedDegradedStart(db);
    },
    async () => {
      const result = await run(runWhy, [SESSION]);
      assert.equal(result.status, 0);
      assert.ok(
        result.stdout.includes(
          `degraded: ${DEGRADED_SENTENCES.summary_pending} (summary_pending)`,
        ),
        result.stdout,
      );
      assert.match(result.stdout, /session_start pack \(claude:SessionStart\) — omitted/);
      assertNoBodies(result.stdout + result.stderr);
    },
  );
});

test('why prints deferred deliveries and each attempt', async () => {
  await withSeeded(
    (db) => {
      insertTurn(db, 't_why_2', SESSION, 2);
      seedDeferred(db);
    },
    async () => {
      const result = await run(runWhy, [SESSION]);
      assert.equal(result.status, 0);
      assert.match(result.stdout, /grok_deferred pack \(grok:PreToolUse\)/);
      assert.match(result.stdout, /deferred: delivered with tool calls \(1 deliveries\)/);
      assert.match(
        result.stdout,
        /attempt 1: call call_denied execution denied, delivery pending, at /,
      );
      assert.match(
        result.stdout,
        /attempt 2: call call_ok execution ran, delivery delivered, at /,
      );
      assertNoBodies(result.stdout + result.stderr);
    },
  );
});

test('why --turn selects that turn and treats a missing turn as empty', async () => {
  await withSeeded(
    (db) => {
      seedPromptPack(db);
      insertTurn(db, 't_why_2', SESSION, 2);
      const other = createInjection(
        db,
        injectionRow({
          id: 'inj-turn-2',
          turnId: 't_why_2',
          createdAt: NOW + 50,
          packHash: 'hash-turn-2',
        }),
      );
      planItems(db, { id: other, conversationId: SESSION, epoch: 0 }, [
        item({
          sourceKind: 'memory',
          memoryId: 'm_hit',
          rawEventId: null,
          decision: 'omitted',
          reason: 'below_threshold',
          rank: null,
          stale: 0,
        }),
      ]);
    },
    async () => {
      const turn1 = await run(runWhy, [SESSION, '--turn', '1']);
      assert.equal(turn1.status, 0);
      assert.match(turn1.stdout, /Pinned note/);
      assert.match(turn1.stdout, /turn 1/);
      assert.doesNotMatch(turn1.stdout, /turn 2/);
      assert.doesNotMatch(turn1.stdout, /below the threshold/);

      const missing = await run(runWhy, [SESSION, '--turn', '7']);
      assert.equal(missing.status, 0);
      assert.equal(missing.stderr, '');
      assert.equal(missing.stdout, `No injection was built for turn 7 of session ${SESSION}.\n`);
    },
  );
});

test('why resolves a native session id and rejects unknown or invalid input', async () => {
  await withSeeded(
    (db) => {
      seedPromptPack(db);
      insertSession(db, { id: 's_shared_claude', agent: 'claude', native: 'shared-native' });
      insertSession(db, { id: 's_shared_grok', agent: 'grok', native: 'shared-native' });
      db.prepare(
        `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
         VALUES ('r_other', 'remote', 'example.test/other', '/tmp/other', 1, 1)`,
      ).run();
      db.prepare(
        `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, started_at, status, turn_count, context_epoch)
         VALUES ('s_elsewhere', 'r_other', 'claude', 'n_elsewhere', 's_elsewhere', 1, 'active', 0, 0)`,
      ).run();
    },
    async () => {
      const byId = await run(runWhy, [SESSION]);
      const byNative = await run(runWhy, [NATIVE]);
      assert.equal(byId.status, 0);
      assert.equal(byNative.status, 0);
      assert.equal(byNative.stdout, byId.stdout);

      const unknown = await run(runWhy, ['no-such-session']);
      assert.equal(unknown.status, 1);
      assert.equal(unknown.stdout, '');
      assert.equal(unknown.stderr, 'Session no-such-session was not found.\n');

      const elsewhere = await run(runWhy, ['s_elsewhere']);
      assert.equal(elsewhere.status, 1);
      assert.equal(elsewhere.stderr, 'Session s_elsewhere was not found.\n');

      const badTurn = await run(runWhy, [SESSION, '--turn', 'x']);
      assert.equal(badTurn.status, 2);
      assert.equal(badTurn.stdout, '');
      assert.equal(badTurn.stderr.split('\n').filter((line) => line !== '').length, 1);

      const ambiguous = await run(runWhy, ['shared-native']);
      assert.equal(ambiguous.status, 2);
      assert.match(ambiguous.stderr, /Pass the oboete session id instead/);
      assert.match(ambiguous.stderr, /claude: s_shared_claude/);
      assert.match(ambiguous.stderr, /grok: s_shared_grok/);
    },
  );
});

test('why --json returns the ledger items and never prints a memory body', async () => {
  await withSeeded(
    (db) => {
      seedPromptPack(db);
    },
    async () => {
      const result = await run(runWhy, [SESSION, '--json']);
      assert.equal(result.status, 0);
      assert.equal(result.stderr, '');
      const parsed = JSON.parse(result.stdout) as {
        session: { id: string; agent: string; native_session_id: string };
        injections: { items: unknown[] }[];
      };
      assert.deepEqual(parsed.session, {
        id: SESSION,
        agent: 'claude',
        native_session_id: NATIVE,
      });
      assert.equal(parsed.injections.length, 1);
      assert.equal(parsed.injections[0].items.length, 5);
      assertNoBodies(result.stdout + result.stderr);
    },
  );
});

test('why prints an empty-session line when no pack was built', async () => {
  await withSeeded(
    () => {},
    async () => {
      const result = await run(runWhy, [SESSION]);
      assert.equal(result.status, 0);
      assert.equal(result.stdout, `No injection was built for session ${SESSION}.\n`);
    },
  );
});
