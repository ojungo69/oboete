import assert from 'node:assert/strict';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { Readable } from 'node:stream';
import { test } from 'node:test';

import { contentHash, materialHash, memoryIdFor } from '../../src/db/identity.js';
import { openDatabase } from '../../src/db/open.js';
import { LEXICAL_NOTE } from '../../src/memories-cli.js';
import { MCP_TOOLS, runMcp, type McpRuntime } from '../../src/mcp.js';
import { oboetePaths } from '../../src/paths.js';
import { resolveRepoIdentity, type RepoIdentity } from '../../src/repo-identity.js';
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
    writeOut: (text) => {
      stdout += text;
    },
    writeError: (text) => {
      stderr += text;
    },
    ...overrides,
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

test('tools/list returns the three tools of contracts/mcp.md with their input schemas', async () => {
  await withFixture(async ({ repo }) => {
    const { frames } = await serve(repo, [request(2, 'tools/list')]);
    assert.deepEqual(frames[0], { jsonrpc: '2.0', id: 2, result: { tools: MCP_TOOLS } });
    assert.deepEqual(
      MCP_TOOLS.map((tool) => tool.name),
      ['search', 'timeline', 'get'],
    );
    assert.deepEqual(MCP_TOOLS[0].inputSchema, {
      type: 'object',
      properties: {
        query: { type: 'string' },
        limit: { type: 'integer', minimum: 1, maximum: 50, default: 10 },
      },
      required: ['query'],
    });
    assert.deepEqual(MCP_TOOLS[2].inputSchema.required, ['id']);
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
