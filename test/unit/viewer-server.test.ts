import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { contentHash, materialHash, memoryIdFor } from '../../src/db/identity.js';
import { openDatabase } from '../../src/db/open.js';
import { oboetePaths } from '../../src/paths.js';
import { resolveRepoIdentity, type RepoIdentity } from '../../src/repo-identity.js';
import { cjkBigrams } from '../../src/retrieval/fts.js';
import { runView, startViewer, type ViewerHandle } from '../../src/viewer/server.js';
import { withTempHome } from '../helpers/home.js';

const NOW = 1_800_000_000_000;

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
     VALUES (?, ?, 'discovery', ?, ?, '[]', ?, ?, ?, ?, 'unreviewed', 1, ?)`,
  ).run(
    id,
    seed.repoId,
    seed.title,
    seed.body,
    cjkBigrams(`${seed.title} ${seed.body}`),
    material,
    content,
    seed.sensitivity ?? 'eligible',
    NOW,
  );
  return id;
}

type Fixture = {
  home: string;
  repo: string;
  identity: RepoIdentity;
  db: DatabaseSync;
  viewer: ViewerHandle;
  assets: string;
  /** Fetch against the viewer with the launch token, unless `token: null` is asked for. */
  api(path: string, init?: RequestInit & { token?: string | null }): Promise<Response>;
};

async function withViewer(fn: (fixture: Fixture) => Promise<void>): Promise<void> {
  await withTempHome(async (home) => {
    const repo = join(home, 'repos', 'current');
    mkdirSync(repo, { recursive: true });
    const assets = join(home, 'assets');
    mkdirSync(assets, { recursive: true });
    writeFileSync(join(assets, 'app.js'), 'console.log("viewer")');
    writeFileSync(join(assets, 'app.css'), 'body{}');
    const identity = resolveRepoIdentity(repo);
    const { db } = openDatabase({ path: oboetePaths(home).db, timeoutMs: 2_000 });
    insertRepo(db, identity);
    const viewer = await startViewer({ cwd: repo, assetsDir: assets, now: () => NOW });
    try {
      await fn({
        home,
        repo,
        identity,
        db,
        viewer,
        assets,
        api: async (path, init = {}) => {
          const { token = viewer.token, ...rest } = init;
          const headers = new Headers(rest.headers);
          if (token !== null) headers.set('authorization', `Bearer ${token}`);
          return await fetch(`${viewer.origin}${path}`, { ...rest, headers });
        },
      });
    } finally {
      await viewer.close();
      db.close();
    }
  });
}

test('the viewer binds 127.0.0.1 with a per-launch token and refuses any other host', async () => {
  await withViewer(async ({ viewer }) => {
    assert.match(viewer.origin, /^http:\/\/127\.0\.0\.1:\d+$/);
    assert.equal(viewer.url, `${viewer.origin}/?token=${viewer.token}`);
    assert.match(viewer.token, /^[0-9a-f]{32}$/);
  });
  await withTempHome(async (home) => {
    const repo = join(home, 'repo');
    mkdirSync(repo);
    await assert.rejects(
      startViewer({ cwd: repo, host: '0.0.0.0' }),
      /only the local machine/,
    );
  });
});

test('a request without the token is refused on every route; the page and the API accept it', async () => {
  await withViewer(async ({ api, viewer }) => {
    for (const path of ['/', '/api/memories', '/api/sessions', '/api/events', '/api/sessions/s1/why']) {
      const refused = await api(path, { token: null });
      assert.equal(refused.status, 401, path);
      const wrong = await api(path, { token: 'f'.repeat(32) });
      assert.equal(wrong.status, 401, path);
    }
    const page = await fetch(viewer.url);
    assert.equal(page.status, 200);
    assert.match(page.headers.get('content-type') ?? '', /text\/html/);
    const html = await page.text();
    assert.ok(html.includes('/assets/app.js'), html);
    assert.ok(!html.includes(viewer.token), 'the page never embeds the token');
    // The page script and stylesheet carry no data; the browser fetches them without a header.
    const script = await api('/assets/app.js', { token: null });
    assert.equal(script.status, 200);
    assert.equal(await script.text(), 'console.log("viewer")');
    assert.equal((await api('/assets/other.js', { token: null })).status, 401);
    const query = await fetch(`${viewer.origin}/api/memories?token=${viewer.token}`);
    assert.equal(query.status, 200);
  });
});

test('memories, sessions, search and why are read through the injection scope of the current repository', async () => {
  await withViewer(async ({ api, db, identity, home }) => {
    const visible = insertMemory(db, { repoId: identity.id, title: 'Busy timeout', body: 'Set busy_timeout to 2000 ms.' });
    insertMemory(db, { repoId: identity.id, title: 'Credential', body: 'busy secret', sensitivity: 'secret' });
    const otherRepo = join(home, 'repos', 'other');
    mkdirSync(otherRepo, { recursive: true });
    const other = resolveRepoIdentity(otherRepo);
    insertRepo(db, other);
    insertMemory(db, { repoId: other.id, title: 'Busy elsewhere', body: 'busy timeout of another repository' });
    db.prepare(
      `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, started_at, status, turn_count, context_epoch)
       VALUES ('s1', ?, 'codex', 'n1', 'c1', 1, 'ended', 0, 0)`,
    ).run(identity.id);

    const memories = (await (await api('/api/memories')).json()) as { memories: { id: string; sources: unknown[] }[]; repository: string };
    assert.deepEqual(memories.memories.map((memory) => memory.id), [visible]);
    assert.deepEqual(memories.memories[0].sources, []);
    assert.equal(memories.repository, identity.normalizedIdentity);

    const search = (await (await api('/api/search?q=busy%20timeout')).json()) as { memories: { id: string }[] };
    assert.deepEqual(search.memories.map((memory) => memory.id), [visible]);

    const sessions = (await (await api('/api/sessions')).json()) as { sessions: { id: string }[] };
    assert.deepEqual(sessions.sessions.map((session) => session.id), ['s1']);

    const why = (await (await api('/api/sessions/s1/why')).json()) as { injections: unknown[] };
    assert.deepEqual(why.injections, []);
  });
});

test('review, pin, unpin and delete mutate through the scope and need a same-origin request', async () => {
  await withViewer(async ({ api, db, identity, viewer }) => {
    const id = insertMemory(db, { repoId: identity.id, title: 'Pin me', body: 'A memory to pin.' });
    const hidden = insertMemory(db, { repoId: identity.id, title: 'Hidden', body: 'A secret memory.', sensitivity: 'secret' });
    const row = (): { review_state: string; pinned_at: number | null; deleted_at: number | null } =>
      db.prepare('SELECT review_state, pinned_at, deleted_at FROM memories WHERE id = ?').get(id) as never;

    const foreign = await api(`/api/memories/${id}/review`, { method: 'POST', headers: { origin: 'http://evil.example' } });
    assert.equal(foreign.status, 403);
    assert.equal(row().review_state, 'unreviewed');

    const noOrigin = await api(`/api/memories/${id}/review`, { method: 'POST' });
    assert.equal(noOrigin.status, 200);
    assert.equal(row().review_state, 'reviewed');

    const sameOrigin = await api(`/api/memories/${id}/pin`, { method: 'POST', headers: { origin: viewer.origin } });
    assert.equal(sameOrigin.status, 200);
    assert.equal(row().pinned_at, NOW);

    assert.equal((await api(`/api/memories/${id}/unpin`, { method: 'POST' })).status, 200);
    assert.equal(row().pinned_at, null);

    assert.equal((await api(`/api/memories/${hidden}/pin`, { method: 'POST' })).status, 404);
    assert.equal((await api('/api/memories/m_missing', { method: 'DELETE' })).status, 404);

    assert.equal((await api(`/api/memories/${id}`, { method: 'DELETE' })).status, 200);
    assert.equal(row().deleted_at, NOW);
    assert.equal((await api(`/api/memories/${id}/pin`, { method: 'POST' })).status, 404, 'a tombstone is gone');
  });
});

test('a change in the database reaches an open event stream within 2 seconds (SC-011)', async () => {
  await withViewer(async ({ api, db, identity }) => {
    const response = await api('/api/events');
    assert.equal(response.status, 200);
    assert.match(response.headers.get('content-type') ?? '', /text\/event-stream/);
    const reader = response.body!.getReader();
    const decoder = new TextDecoder();
    const started = Date.now();
    let received = '';
    // The first frame is the current version; the next must follow the insert.
    const readUntil = async (predicate: (text: string) => boolean): Promise<void> => {
      while (!predicate(received)) {
        const { value, done } = await reader.read();
        if (done) throw new Error('stream ended');
        received += decoder.decode(value);
      }
    };
    await readUntil((text) => text.includes('event: change'));
    const before = received.length;
    insertMemory(db, { repoId: identity.id, title: 'Fresh', body: 'A memory that just arrived.' });
    await readUntil((text) => text.slice(before).includes('event: change'));
    assert.ok(Date.now() - started < 2_000, `took ${Date.now() - started} ms`);
    await reader.cancel();
  });
});

test('oboete view prints the tokenized URL, opens the browser on --open and refuses a bad port', async () => {
  await withTempHome(async (home) => {
    const repo = join(home, 'repo');
    mkdirSync(repo);
    const assets = join(home, 'assets');
    mkdirSync(assets);
    writeFileSync(join(assets, 'app.js'), '');
    writeFileSync(join(assets, 'app.css'), '');
    let stdout = '';
    let opened: string | null = null;
    const controller = new AbortController();
    const running = runView(['--open'], {
      cwd: repo,
      assetsDir: assets,
      open: (url) => {
        opened = url;
        controller.abort();
      },
      signal: controller.signal,
      writeOut: (text) => {
        stdout += text;
      },
      writeError: () => {},
    });
    assert.equal(await running, 0);
    assert.match(stdout, /^http:\/\/127\.0\.0\.1:\d+\/\?token=[0-9a-f]{32}\n/);
    assert.equal(opened, stdout.trim());

    let stderr = '';
    const bad = await runView(['--port', '99999'], {
      cwd: repo,
      assetsDir: assets,
      writeOut: () => {},
      writeError: (text) => {
        stderr += text;
      },
    });
    assert.equal(bad, 2);
    assert.match(stderr, /--port/);
  });
});
