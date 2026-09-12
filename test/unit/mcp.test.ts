import { grantVisibility } from '../../src/db/queries.js';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { PassThrough, Readable } from 'node:stream';
import { test } from 'node:test';

import { contentHash, materialHash, memoryIdFor } from '../../src/db/identity.js';
import { openDatabase } from '../../src/db/open.js';
import { LEXICAL_NOTE, runGet, runSearch, runTimeline } from '../../src/memories-cli.js';
import { MCP_TOOLS, runMcp, type McpRuntime } from '../../src/mcp.js';
import { oboetePaths } from '../../src/paths.js';
import { resolveRepoIdentity, type RepoIdentity } from '../../src/repo-identity.js';
import { seedWorkBinding } from '../helpers/work.js';
import { cjkBigrams } from '../../src/retrieval/fts.js';
import { withTempHome } from '../helpers/home.js';

type Frame = Record<string, unknown>;

function insertRepo(db: DatabaseSync, identity: RepoIdentity): void {
  db.prepare(
    `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
     VALUES (?, ?, ?, ?, 1, 1)`,
  ).run(identity.id, identity.identityKind, identity.normalizedIdentity, identity.root);
}

function insertMemory(
  db: DatabaseSync,
  seed: { repoId: string; title: string; body: string; sensitivity?: string },
): string {
  const material = materialHash(seed.title, seed.body);
  const content = contentHash(seed.repoId, material);
  const id = memoryIdFor(content);
  db.prepare(
    `INSERT INTO memories
       (id, repo_id, type, title, body, concepts, cjk_bigrams, material_hash, content_hash,
        sensitivity, review_state, valid_from, created_at)
     VALUES (?, ?, 'discovery', ?, ?, '[]', ?, ?, ?, ?, 'unreviewed', 1, 1)`,
  ).run(
    id,
    seed.repoId,
    seed.title,
    seed.body,
    cjkBigrams(`${seed.title} ${seed.body}`),
    material,
    content,
    seed.sensitivity ?? 'eligible',
  );
  grantVisibility(db, id, { audience: 'project', repoId: seed.repoId }, 'migration', 1);
  return id;
}

/** Feeds the frames as one stdin stream and returns every stdout line parsed, in order. */
async function serve(
  cwd: string,
  lines: string[],
  overrides: Partial<McpRuntime> = {},
): Promise<{ status: number; frames: Frame[]; stdout: string; stderr: string }> {
  let stdout = '';
  let stderr = '';
  const status = await runMcp([], {
    cwd,
    input: Readable.from([`${lines.join('\n')}\n`]),
    ...overrides,
    writeOut: (text) => {
      stdout += text;
      overrides.writeOut?.(text);
    },
    writeError: (text) => {
      stderr += text;
      overrides.writeError?.(text);
    },
  });
  const frames = stdout
    .split('\n')
    .filter((line) => line !== '')
    .map((line) => JSON.parse(line) as Frame);
  return { status, frames, stdout, stderr };
}

const request = (id: number, method: string, params?: unknown): string =>
  JSON.stringify({ jsonrpc: '2.0', id, method, ...(params === undefined ? {} : { params }) });

const call = (id: number, name: string, args: unknown): string =>
  request(id, 'tools/call', { name, arguments: args });

async function withFixture(
  fn: (fixture: { home: string; repo: string; identity: RepoIdentity; db: DatabaseSync }) => Promise<void>,
): Promise<void> {
  await withTempHome(async (home) => {
    const repo = join(home, 'repos', 'current');
    mkdirSync(repo, { recursive: true });
    const identity = resolveRepoIdentity(repo);
    const { db } = openDatabase({ path: oboetePaths(home).db, timeoutMs: 2_000 });
    try {
      insertRepo(db, identity);
      await fn({ home, repo, identity, db });
    } finally {
      db.close();
    }
  });
}

test('initialize echoes a supported protocol version, initialized is silent, ping answers {}', async () => {
  await withFixture(async ({ repo }) => {
    const { status, frames } = await serve(repo, [
      request(1, 'initialize', {
        protocolVersion: '2025-11-25',
        capabilities: {},
        clientInfo: { name: 'claude-code', version: '2.1.258' },
      }),
      JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }),
      request(2, 'ping'),
    ]);
    assert.equal(status, 0);
    assert.equal(frames.length, 2);
    assert.deepEqual(frames[0], {
      jsonrpc: '2.0',
      id: 1,
      result: {
        protocolVersion: '2025-11-25',
        capabilities: { tools: { listChanged: false } },
        serverInfo: { name: 'oboete', version: OBOETE_VERSION },
      },
    });
    assert.deepEqual(frames[1], { jsonrpc: '2.0', id: 2, result: {} });
  });
});

test('an unknown protocol version gets the latest legacy version the server implements', async () => {
  await withFixture(async ({ repo }) => {
    const { frames } = await serve(repo, [
      request(1, 'initialize', { protocolVersion: '2031-01-01', capabilities: {}, clientInfo: {} }),
    ]);
    assert.equal((frames[0].result as { protocolVersion: string }).protocolVersion, '2025-11-25');
  });
});

test('tools/list returns memory and scoped work tools with their input schemas', async () => {
  await withFixture(async ({ repo }) => {
    const { frames } = await serve(repo, [request(2, 'tools/list')]);
    assert.deepEqual(frames[0], { jsonrpc: '2.0', id: 2, result: { tools: MCP_TOOLS } });
    assert.deepEqual(
      MCP_TOOLS.map((tool) => tool.name),
      ['search', 'timeline', 'get', 'work_status', 'work_choose', 'sharing_status', 'sync_status'],
    );
    assert.deepEqual(MCP_TOOLS[0].inputSchema, {
      type: 'object',
      properties: {
        binding: { type: 'string', minLength: 1, maxLength: 128, description: 'An exact current-worktree binding ID' },
        history: { type: 'boolean', default: false, description: 'Deliberately include retained historical memories and checkpoints' },
        query: { type: 'string' },
        limit: { type: 'integer', minimum: 1, maximum: 50, default: 10 },
      },
      required: ['query'],
    });
    assert.deepEqual(MCP_TOOLS[2].inputSchema.required, ['id']);
  });
});

test('MCP sharing status is read-only and model-generated confirmation cannot approve a proposal', async () => {
  await withFixture(async ({ repo, db }) => {
    const result = await serve(repo, [call(1, 'sharing_status', {}),
      call(2, 'sharing_status', { confirmed: true }), call(3, 'sharing_update', { id: 'proposal', decision: 'approve', confirmed: true })]);
    assert.deepEqual((result.frames[0].result as { structuredContent: unknown }).structuredContent, { proposals: [], hasMore: false });
    assert.equal((result.frames[1].error as { code: number }).code, -32602);
    assert.equal((result.frames[2].error as { code: number }).code, -32602);
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM sharing_proposals').get()?.n, 0);
    assert.deepEqual(MCP_TOOLS.find((tool) => tool.name === 'sharing_status')?.annotations,
      { readOnlyHint: true, idempotentHint: true, openWorldHint: false });
  });
});

test('MCP sync_status is read-only local state and no MCP tool can push, pull or resolve', async () => {
  await withFixture(async ({ repo }) => {
    const result = await serve(repo, [call(1, 'sync_status', {}), call(2, 'sync_push', {}), call(3, 'sync_pull', {}), call(4, 'sync_resolve', { id: 'x', keep: 'y' })]);
    assert.deepEqual((result.frames[0].result as { structuredContent: unknown }).structuredContent,
      { configured: false, replicas: [], conflicts: [], withheld_on_apply: [], unmapped_repos: [], totals: { conflicts: 0, withheld_on_apply: 0, unmapped_repos: 0 } });
    for (const frame of result.frames.slice(1)) assert.equal((frame.error as { code: number }).code, -32602);
    assert.deepEqual(MCP_TOOLS.find((tool) => tool.name === 'sync_status')?.annotations,
      { readOnlyHint: true, idempotentHint: true, openWorldHint: false });
  });
});

test('work MCP choices are scoped, persist selection and refuse a repeated new-work choice token', async () => {
  await withFixture(async ({ repo, identity, db, home }) => {
    db.prepare(`INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status)
      VALUES ('work-session', ?, 'codex', 'native-work', 'work-session', 'active')`).run(identity.id);
    const binding = seedWorkBinding(db, 'work-session');
    db.prepare('UPDATE work_contexts SET local_key = ?, root = ?').run(identity.worktreeKey, repo);
    db.prepare('UPDATE work_bindings SET work_id = NULL, candidates_json = ? WHERE id = ?')
      .run(JSON.stringify([`fixture-work:${identity.id}`]), binding);
    const { frames } = await serve(repo, [
      call(1, 'work_status', {}),
      call(2, 'work_choose', { binding, work: 'new' }),
      call(3, 'work_choose', { binding, work: 'new' }),
      call(4, 'work_status', {}),
    ]);
    const before = frames[0].result as { structuredContent: { bindings: { id: string; work_id: string | null }[] } };
    assert.equal(before.structuredContent.bindings[0].id, binding);
    assert.equal(before.structuredContent.bindings[0].work_id, null);
    assert.equal((frames[1].result as { isError?: boolean }).isError, undefined);
    assert.equal((frames[2].result as { isError?: boolean }).isError, true);
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM work_items').get()?.n, 2);
    const current = String(db.prepare('SELECT id FROM work_bindings WHERE closed_at IS NULL').get()?.id);
    assert.notEqual(current, binding);
    const foreign = join(home, 'other-work-root');
    mkdirSync(foreign);
    const denied = await serve(foreign, [
      call(5, 'work_choose', { binding: current, work: `fixture-work:${identity.id}` }),
      call(6, 'work_choose', { binding: 'missing', work: 'new' }),
      call(7, 'work_choose', { binding: current, source: 'source', work: 'new' }),
    ]);
    assert.deepEqual(denied.frames[0].result, denied.frames[1].result);
    assert.equal((denied.frames[2].error as { code: number }).code, -32602);
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM work_items').get()?.n, 2);
  });
});

test('CLI and MCP require an exact binding for ambiguous progress and keep historical inspection explicit', async () => {
  await withFixture(async ({ repo, identity, db }) => {
    db.prepare(`INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status)
      VALUES ('reader-session', ?, 'codex', 'native-reader', 'reader-session', 'active')`).run(identity.id);
    const binding = seedWorkBinding(db, 'reader-session');
    const firstWork = `fixture-work:${identity.id}`;
    db.prepare('UPDATE work_contexts SET local_key = ?, root = ?').run(identity.worktreeKey, repo);
    db.prepare(`INSERT INTO work_items (id, repo_id, origin_context_id, purpose, created_at, updated_at)
      VALUES ('second-work', ?, ?, 'Second investigation', 1, 1)`).run(identity.id, `fixture-context:${identity.id}`);
    const ids = [firstWork, 'second-work', null].map((workId, index) => {
      const id = insertMemory(db, { repoId: identity.id, title: `Progress ${index}`, body: `Checkpoint state ${index}.` });
      db.prepare("UPDATE memories SET type = 'session_summary', work_id = ?, source_session_id = 'reader-session' WHERE id = ?")
        .run(workId, id);
      if (workId !== null) {
        db.prepare('UPDATE work_items SET current_checkpoint_memory_id = ? WHERE id = ?').run(id, workId);
        db.prepare('UPDATE memories SET provenance_complete = 1 WHERE id = ?').run(id);
        db.prepare(`INSERT INTO memory_sources (memory_id, capture_root, source_paths_json, source_context_id)
          VALUES (?, ?, '[]', ?)`).run(id, repo, `fixture-context:${identity.id}`);
      }
      return id;
    });
    const { frames } = await serve(repo, [
      call(1, 'search', { query: 'Progress' }),
      call(2, 'get', { id: ids[0] }),
      call(3, 'get', { id: ids[0], binding }),
      call(4, 'get', { id: ids[1], binding }),
      call(5, 'get', { id: ids[2], history: true }),
      call(6, 'search', { query: 'Progress', binding }),
      call(7, 'timeline', { binding }),
      call(8, 'get', { id: ids[0], binding: 'foreign-binding' }),
    ]);
    const results = frames.map((frame) => frame.result as { isError?: boolean; structuredContent?: { id?: string;
      memories?: { id: string }[]; sessions?: { memory_ids: string[] }[]; selection?: { choices: unknown[] } } });
    assert.deepEqual(results[0].structuredContent?.memories, []);
    assert.equal(results[0].structuredContent?.selection?.choices.length, 2);
    assert.equal(results[1].isError, true);
    assert.equal(results[2].structuredContent?.id, ids[0]);
    assert.equal(results[3].isError, true);
    assert.equal(results[4].structuredContent?.id, ids[2]);
    assert.deepEqual(results[5].structuredContent?.memories?.map((item) => item.id), [ids[0]]);
    assert.deepEqual(results[6].structuredContent?.sessions?.[0].memory_ids, [ids[0]]);
    assert.equal(results[7].isError, true);

    for (const [command, args, status, expected] of [
      [runSearch, ['Progress', '--json'], 0, '"memories":[]'],
      [runGet, [ids[0], '--json'], 1, 'memory_not_found'],
      [runGet, [ids[0], '--json', '--binding', binding], 0, ids[0]],
      [runGet, [ids[1], '--json', '--binding', binding], 1, 'memory_not_found'],
      [runGet, [ids[2], '--json', '--history'], 0, ids[2]],
      [runTimeline, ['--json', '--binding', binding], 0, ids[0]],
    ] as const) {
      let stdout = '';
      assert.equal(await command([...args], { cwd: repo, writeOut: (text) => { stdout += text; }, writeError: () => {} }), status);
      assert.ok(stdout.includes(expected), stdout);
      if (command === runTimeline) assert.ok(!stdout.includes(ids[1]) && !stdout.includes(ids[2]), stdout);
    }
    db.prepare("UPDATE memories SET sensitivity = 'secret' WHERE id = ?").run(ids[2]);
    const secret = await serve(repo, [call(9, 'get', { id: ids[2], history: true })]);
    assert.equal((secret.frames[0].result as { isError: boolean }).isError, true);
  });
});

test('CLI and every MCP reader apply current path policy before formatting body or source fields', async () => {
  await withFixture(async ({ repo, identity, db }) => {
    db.prepare(`INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status)
      VALUES ('private-reader', ?, 'codex', 'private-reader', 'private-reader', 'active')`).run(identity.id);
    const binding = seedWorkBinding(db, 'private-reader');
    const workId = `fixture-work:${identity.id}`;
    const contextId = `fixture-context:${identity.id}`;
    db.prepare('UPDATE work_contexts SET local_key = ?, root = ?').run(identity.worktreeKey, repo);
    const id = insertMemory(db, { repoId: identity.id, title: 'Protected upload status', body: 'The protected deployment details remain outstanding.' });
    db.prepare(`UPDATE memories SET type = 'session_summary', work_id = ?, source_session_id = 'private-reader',
      provenance_complete = 1 WHERE id = ?`).run(workId, id);
    db.prepare('UPDATE work_items SET current_checkpoint_memory_id = ?').run(id);
    db.prepare(`INSERT INTO memory_sources (memory_id, capture_root, source_paths_json, source_context_id)
      VALUES (?, ?, '["protected/upload.ts"]', ?)`).run(id, repo, contextId);
    const before = await serve(repo, [call(1, 'get', { id, binding })]);
    assert.match(before.stdout, /protected deployment details/);
    writeFileSync(join(repo, '.oboete.toml'), '[privacy]\nsecret_paths = ["protected/**"]\n');
    const after = await serve(repo, [call(1, 'get', { id, binding }), call(2, 'get', { id, history: true }),
      call(3, 'search', { query: 'Protected', binding }), call(4, 'timeline', { binding }), call(5, 'work_status', { all: true })]);
    assert.doesNotMatch(after.stdout, /Protected upload status|protected deployment details|protected\/upload/);
    const status = spawnSync(process.execPath, [join(process.cwd(), 'dist/oboete.mjs'), 'work', 'status', '--json'],
      { cwd: repo, env: process.env, encoding: 'utf8' });
    assert.equal(status.status, 0, status.stderr);
    assert.doesNotMatch(status.stdout, /Protected upload status|protected deployment details|protected\/upload/);
    for (const [command, args] of [[runSearch, ['Protected', '--json']], [runGet, [id, '--json']], [runTimeline, ['--json']]] as const) {
      let output = '';
      await command([...args], { cwd: repo, writeOut: (text) => { output += text; }, writeError: () => {} });
      assert.doesNotMatch(output, /Protected upload status|protected deployment details|protected\/upload/);
    }
    let historical = '';
    await runGet([id, '--history', '--json'], { cwd: repo, writeOut: (text) => { historical += text; }, writeError: () => {} });
    assert.match(historical, /protected deployment details/, 'explicit local history remains an inspection surface');
  });
});

test('work and selection metadata cannot expose a value that becomes a configured credential', async () => {
  await withFixture(async ({ repo, identity, db }) => {
    db.prepare(`INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status)
      VALUES ('metadata-session', ?, 'codex', 'metadata-native', 'metadata-session', 'active')`).run(identity.id);
    seedWorkBinding(db, 'metadata-session');
    db.prepare('UPDATE work_contexts SET local_key = ?, root = ?').run(identity.worktreeKey, repo);
    const privatePurpose = 'A formerly ordinary purpose becomes a credential';
    db.prepare(`INSERT INTO work_items (id, repo_id, origin_context_id, purpose, created_at, updated_at)
      VALUES ('private-purpose', ?, ?, ?, 1, 1)`).run(identity.id, `fixture-context:${identity.id}`, privatePurpose);
    const before = process.env.OBOETE_OPENROUTER_API_KEY;
    try {
      process.env.OBOETE_OPENROUTER_API_KEY = privatePurpose;
      const result = await serve(repo, [call(1, 'work_status', { all: true }), call(2, 'search', { query: 'missing' }), call(3, 'timeline', {})]);
      assert.ok(!result.stdout.includes(privatePurpose));
      assert.ok(result.stdout.includes('private-purpose'), 'opaque choices remain usable');
      let output = '';
      await runSearch(['missing', '--json'], { cwd: repo, writeOut: (text) => { output += text; }, writeError: () => {} });
      assert.ok(!output.includes(privatePurpose));
    } finally {
      if (before === undefined) delete process.env.OBOETE_OPENROUTER_API_KEY;
      else process.env.OBOETE_OPENROUTER_API_KEY = before;
    }
  });
});

test('a running MCP refuses an old worktree generation after its directory is recreated', async () => {
  await withFixture(async ({ repo, identity, db }) => {
    db.prepare(`INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status)
      VALUES ('long-lived', ?, 'codex', 'long-lived-native', 'long-lived', 'active')`).run(identity.id);
    const binding = seedWorkBinding(db, 'long-lived');
    db.prepare('UPDATE work_contexts SET local_key = ?, root = ?').run(identity.worktreeKey, repo);
    const input = new PassThrough();
    let ready!: () => void;
    const first = new Promise<void>((resolve) => { ready = resolve; });
    const running = serve(repo, [], { input, writeOut: (text) => { if (JSON.parse(text).id === 1) ready(); } });
    input.write(`${call(1, 'work_status', {})}\n`);
    await first;
    rmSync(repo, { recursive: true });
    mkdirSync(repo);
    input.end(`${call(2, 'work_choose', { binding, work: 'new' })}\n`);
    const result = await running;
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM work_items').get()?.n, 1);
    assert.ok(result.frames[1].error !== undefined || (result.frames[1].result as { isError?: boolean })?.isError === true);
  });
});

test('search returns a text rendering and the structured memories of the current repository only', async () => {
  await withFixture(async ({ repo, identity, db, home }) => {
    const hit = insertMemory(db, { repoId: identity.id, title: 'SQLite busy timeout', body: 'Set busy_timeout to 2000 ms.' });
    // Injection scope: private rows stay on this machine and are readable by a local agent; secret rows never are.
    const local = insertMemory(db, { repoId: identity.id, title: 'Private busy timeout note', body: 'busy timeout on this machine', sensitivity: 'private' });
    insertMemory(db, { repoId: identity.id, title: 'Secret busy timeout', body: 'busy timeout credential', sensitivity: 'secret' });
    const otherRepo = join(home, 'repos', 'other');
    mkdirSync(otherRepo, { recursive: true });
    const other = resolveRepoIdentity(otherRepo);
    insertRepo(db, other);
    insertMemory(db, { repoId: other.id, title: 'SQLite busy timeout elsewhere', body: 'busy timeout in another repository' });

    const { frames, stderr } = await serve(repo, [call(3, 'search', { query: 'sqlite busy timeout', limit: 5 })]);
    assert.equal(stderr, '');
    const result = frames[0].result as {
      content: { type: string; text: string }[];
      structuredContent: { memories: { id: string; title: string; score: number; stale: boolean; citations: unknown[] }[]; degraded: null };
      isError?: boolean;
    };
    assert.equal(result.isError, undefined);
    assert.equal(result.content[0].type, 'text');
    assert.ok(result.content[0].text.includes('SQLite busy timeout'), result.content[0].text);
    assert.deepEqual(
      result.structuredContent.memories.map((memory) => memory.id).sort(),
      [hit, local].sort(),
    );
    assert.equal(typeof result.structuredContent.memories[0].score, 'number');
    assert.equal(result.structuredContent.memories[0].stale, false);
    assert.deepEqual(result.structuredContent.memories[0].citations, []);
    assert.equal(result.structuredContent.degraded, null);
  });
});

test('an empty search says that M1 search is lexical', async () => {
  await withFixture(async ({ repo }) => {
    const { frames } = await serve(repo, [call(3, 'search', { query: 'nothing here' })]);
    const result = frames[0].result as { content: { text: string }[]; structuredContent: { memories: unknown[] } };
    assert.deepEqual(result.structuredContent.memories, []);
    assert.ok(result.content[0].text.includes(LEXICAL_NOTE), result.content[0].text);
  });
});

test('get answers not found as a tool error for a missing id and for an out-of-boundary id', async () => {
  await withFixture(async ({ repo, identity, db, home }) => {
    const visible = insertMemory(db, { repoId: identity.id, title: 'Visible', body: 'A visible memory.' });
    const secret = insertMemory(db, { repoId: identity.id, title: 'Hidden', body: 'A secret memory.', sensitivity: 'secret' });
    const otherRepo = join(home, 'repos', 'other');
    mkdirSync(otherRepo, { recursive: true });
    const other = resolveRepoIdentity(otherRepo);
    insertRepo(db, other);
    const elsewhere = insertMemory(db, { repoId: other.id, title: 'Elsewhere', body: 'A memory of another repository.' });
    const { frames } = await serve(repo, [
      call(4, 'get', { id: visible }),
      call(5, 'get', { id: secret }),
      call(6, 'get', { id: elsewhere }),
      call(7, 'get', { id: 'm_missing' }),
    ]);
    const found = frames[0].result as { structuredContent: { id: string; title: string; sources: unknown[] }; isError?: boolean };
    assert.equal(found.isError, undefined);
    assert.equal(found.structuredContent.id, visible);
    assert.deepEqual(found.structuredContent.sources, []);
    for (const frame of frames.slice(1)) {
      assert.deepEqual(frame.result, { content: [{ type: 'text', text: 'not found' }], isError: true });
    }
  });
});

test('timeline returns the sessions of the current repository', async () => {
  await withFixture(async ({ repo, identity, db }) => {
    db.prepare(
      `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, started_at, status, turn_count, context_epoch)
       VALUES ('s1', ?, 'codex', 'n1', 'c1', 1, 'ended', 0, 0)`,
    ).run(identity.id);
    const { frames } = await serve(repo, [call(7, 'timeline', {})]);
    const result = frames[0].result as { content: { text: string }[]; structuredContent: { sessions: { id: string }[] } };
    assert.deepEqual(result.structuredContent.sessions.map((session) => session.id), ['s1']);
    assert.ok(result.content[0].text.includes('s1'));
  });
});

test('a repo argument, an unknown tool and invalid arguments are -32602 protocol errors', async () => {
  await withFixture(async ({ repo }) => {
    const { frames } = await serve(repo, [
      call(8, 'search', { query: 'x', repo: '/elsewhere' }),
      call(9, 'forget', {}),
      call(10, 'search', { query: 'x', limit: 0 }),
      call(11, 'search', {}),
      request(12, 'tools/call', { arguments: {} }),
      call(16, 'search', { query: 'x'.repeat(4097) }),
    ]);
    assert.equal(frames.length, 6);
    for (const frame of frames) {
      const error = frame.error as { code: number; message: string; data?: unknown };
      assert.equal(error.code, -32602, JSON.stringify(frame));
    }
    assert.match(String((frames[0].error as { data?: string }).data), /working directory/);
  });
});

test('server/discover and other unknown methods are -32601; a broken line is -32700', async () => {
  await withFixture(async ({ repo }) => {
    const { frames } = await serve(repo, [
      request(13, 'server/discover'),
      request(14, 'resources/list'),
      '{"jsonrpc":"2.0","id":15,"method":',
      JSON.stringify({ jsonrpc: '2.0', method: 'notifications/cancelled', params: {} }),
      JSON.stringify({ jsonrpc: '2.0', id: 17, method: 'ping', params: { pad: 'x'.repeat(1_048_576) } }),
    ]);
    assert.equal(frames.length, 4);
    assert.equal((frames[3].error as { code: number }).code, -32600);
    assert.equal(frames[3].id, null);
    assert.equal((frames[0].error as { code: number }).code, -32601);
    assert.equal(frames[0].id, 13);
    assert.equal((frames[1].error as { code: number }).code, -32601);
    assert.equal((frames[2].error as { code: number }).code, -32700);
    assert.equal(frames[2].id, null);
  });
});

test('the server refuses arguments and prints nothing but frames on stdout', async () => {
  await withFixture(async ({ repo }) => {
    let stderr = '';
    const status = await runMcp(['--repo', '/x'], {
      cwd: repo,
      input: Readable.from([]),
      writeOut: () => {
        throw new Error('nothing may reach stdout');
      },
      writeError: (text) => {
        stderr += text;
      },
    });
    assert.equal(status, 2);
    assert.notEqual(stderr, '');
  });
});
