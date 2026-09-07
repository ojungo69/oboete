// `oboete view`: the local viewer (spec FR-037, FR-038; research R9). Security-owned: the server
// binds 127.0.0.1 only, every request carries the per-launch token, mutating routes check the
// `Origin` header, and every read and write goes through the injection scope of db/queries.ts for
// the repository of the working directory -- the viewer shows exactly what a local agent may see.
// Hono and its Node adapter are loaded here, off the hook path (plan.md Complexity Tracking 1).
import { spawn } from 'node:child_process';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

import { openDatabase } from './../db/open.js';
import {
  getMemory,
  listMemories,
  memoryScope,
  memorySources,
  setPinned,
  setReviewed,
  timeline,
  tombstone,
  type MemoryScope,
} from './../db/queries.js';
import { whyReport } from './../injection/ledger.js';
import { LEXICAL_NOTE, searchMemories } from './../memories-cli.js';
import { ensureDirectories, oboetePaths, resolveHome, type OboetePaths } from './../paths.js';
import { resolveRepoIdentity } from './../repo-identity.js';

const LOOPBACK_HOSTS = new Set(['127.0.0.1', '::1', 'localhost']);
const DATABASE_TIMEOUT_MS = 2_000;
const LIST_LIMIT = 200;
const SEARCH_LIMIT = 50;
/** research R9: change detection by polling `PRAGMA data_version`; SC-011 allows 2 s end to end. */
const POLL_MS = 500;

export type ViewerOptions = {
  cwd: string;
  host?: string;
  port?: number;
  /** Where `app.js` and `app.css` live; the build writes them next to the engine bundle. */
  assetsDir?: string;
  now?: () => number;
};

export type ViewerHandle = {
  origin: string;
  token: string;
  url: string;
  close(): Promise<void>;
};

export type ViewRuntime = ViewerOptions & {
  open(url: string): void;
  signal?: AbortSignal;
  writeOut(text: string): void;
  writeError(text: string): void;
};

function pageHtml(): string {
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>oboete memory viewer</title>
<link rel="stylesheet" href="/assets/app.css">
</head>
<body>
<main id="app"></main>
<noscript><p>The memory viewer needs JavaScript enabled in this browser.</p></noscript>
<script type="module" src="/assets/app.js"></script>
</body>
</html>
`;
}

/** The browser launcher of contracts/cli.md `--open`; never through a shell. */
function openBrowser(url: string): void {
  let command = 'xdg-open';
  let args = [url];
  if (process.platform === 'darwin') command = 'open';
  else if (process.platform === 'win32') [command, args] = ['cmd', ['/c', 'start', '', url]];
  const child = spawn(command, args, { detached: true, stdio: 'ignore' });
  child.on('error', () => {
    // The URL is already printed; a machine without a launcher is not an error of the viewer.
  });
  child.unref();
}

function defaultAssetsDir(): string {
  return join(dirname(fileURLToPath(import.meta.url)), 'viewer');
}

function sameToken(expected: string, given: string | undefined): boolean {
  if (given === undefined) return false;
  const a = Buffer.from(expected);
  const b = Buffer.from(given);
  // Byte lengths, not character counts: a multibyte token of the same length would throw here.
  return a.length === b.length && timingSafeEqual(a, b);
}

export async function startViewer(options: ViewerOptions): Promise<ViewerHandle> {
  const host = options.host ?? '127.0.0.1';
  if (!LOOPBACK_HOSTS.has(host)) {
    throw new Error(`The viewer serves only the local machine; ${host} is not a loopback address (FR-038).`);
  }
  const now = options.now ?? Date.now;
  const assetsDir = options.assetsDir ?? defaultAssetsDir();
  const token = randomBytes(16).toString('hex');
  const paths = oboetePaths(resolveHome());
  ensureDirectories(paths);
  const identity = resolveRepoIdentity(options.cwd);

  const { Hono } = await import('hono');
  const { streamSSE } = await import('hono/streaming');
  const { serve } = await import('@hono/node-server');

  const withDatabase = <T>(fn: (db: DatabaseSync, scope: MemoryScope) => T): T => {
    const opened = openDatabase({ path: paths.db, timeoutMs: DATABASE_TIMEOUT_MS });
    try {
      return fn(opened.db, memoryScope(opened.db, { repoId: identity.id, destination: 'injection' }));
    } finally {
      opened.db.close();
    }
  };
  const searchContext = (paths2: OboetePaths) => ({ repoId: identity.id, paths: paths2 });

  const app = new Hono();
  let origin = '';

  // FR-038: the token gates the page and every API route; the query form exists for the page URL
  // and for EventSource, which cannot send a header. The page script and stylesheet carry no data
  // and are fetched by the browser without a header, so they are served on the bind alone.
  app.get('/assets/:name{app\\.(js|css)}', (c) => {
    const name = c.req.param('name');
    const file = join(assetsDir, name);
    if (!existsSync(file)) return c.text('The viewer assets were not built.', 404);
    return c.body(readFileSync(file), 200, {
      'content-type': name.endsWith('.css') ? 'text/css; charset=utf-8' : 'text/javascript; charset=utf-8',
    });
  });
  app.get('/favicon.ico', (c) => c.body(null, 204));
  app.use('*', async (c, next) => {
    const header = c.req.header('authorization');
    const bearer = header?.startsWith('Bearer ') ? header.slice('Bearer '.length) : undefined;
    if (!sameToken(token, bearer ?? c.req.query('token'))) {
      return c.text('This viewer needs the token printed by `oboete view`.', 401);
    }
    // A cross-site page can carry the token only by guessing it, and it cannot read responses; the
    // Origin check closes the remaining write path (research R9).
    if (c.req.method !== 'GET' && c.req.method !== 'HEAD') {
      const requestOrigin = c.req.header('origin');
      if (requestOrigin !== undefined && requestOrigin !== origin) {
        return c.text('Requests that change memories must come from the viewer itself.', 403);
      }
    }
    await next();
  });

  app.get('/', (c) => c.html(pageHtml()));

  app.get('/api/memories', (c) =>
    withDatabase((db, scope) =>
      c.json({
        repository: identity.normalizedIdentity,
        memories: listMemories(db, scope, { limit: LIST_LIMIT }).map((memory) => ({
          ...memory,
          sources: memorySources(db, memory.id),
        })),
      }),
    ),
  );
  app.get('/api/search', (c) => {
    const query = c.req.query('q')?.trim() ?? '';
    if (query === '') return c.json({ memories: [], note: LEXICAL_NOTE });
    return withDatabase((db) =>
      c.json({
        memories: searchMemories(db, { ...searchContext(paths), query, limit: SEARCH_LIMIT }),
        note: LEXICAL_NOTE,
      }),
    );
  });
  app.get('/api/sessions', (c) =>
    withDatabase((db) => c.json({ sessions: timeline(db, identity.id, { limit: SEARCH_LIMIT }) })),
  );
  app.get('/api/sessions/:id', (c) =>
    withDatabase((db) =>
      c.json({ sessions: timeline(db, identity.id, { sessionId: c.req.param('id'), limit: 1 }) }),
    ),
  );
  app.get('/api/sessions/:id/why', (c) =>
    withDatabase((db, scope) => c.json({ injections: whyReport(db, c.req.param('id'), scope) })),
  );

  const mutate = (
    fn: (db: DatabaseSync, scope: MemoryScope, id: string) => boolean,
  ): ((c: { req: { param(name: 'id'): string }; json: (body: unknown, status?: 200 | 404) => Response }) => Response) =>
    (c) =>
      withDatabase((db, scope) => {
        const id = c.req.param('id');
        if (!fn(db, scope, id)) return c.json({ error: 'memory_not_found', id }, 404);
        const memory = getMemory(db, id, scope);
        return c.json({ memory: memory === null ? null : { ...memory, sources: memorySources(db, id) } });
      });
  app.post('/api/memories/:id/review', mutate((db, scope, id) => setReviewed(db, { id, scope })));
  app.post('/api/memories/:id/pin', mutate((db, scope, id) => {
    const pinnedAt = now();
    const last = db.prepare('SELECT COALESCE(MAX(pin_order), 0) AS n FROM memories').get() as { n: number };
    return setPinned(db, { id, scope, pinnedAt, pinOrder: Number(last.n) + 1 });
  }));
  app.post('/api/memories/:id/unpin', mutate((db, scope, id) => setPinned(db, { id, scope, pinnedAt: null, pinOrder: null })));
  app.delete('/api/memories/:id', mutate((db, scope, id) => tombstone(db, { id, scope, deletedAt: now() })));

  // `PRAGMA data_version` counts commits by other connections as seen from one connection, so the
  // stream keeps its own connection open for as long as the browser listens.
  app.get('/api/events', (c) =>
    streamSSE(c, async (stream) => {
      const opened = openDatabase({ path: paths.db, timeoutMs: DATABASE_TIMEOUT_MS });
      try {
        const read = opened.db.prepare('PRAGMA data_version');
        let last = -1;
        let id = 0;
        while (!stream.aborted && !stream.closed) {
          const version = Number((read.get() as { data_version: number }).data_version);
          if (version !== last) {
            last = version;
            await stream.writeSSE({ event: 'change', data: String(version), id: String(id++) });
          }
          await delay(POLL_MS);
        }
      } finally {
        opened.db.close();
      }
    }),
  );

  const server = await new Promise<ReturnType<typeof serve>>((resolve, reject) => {
    const instance = serve({ fetch: app.fetch, hostname: host, port: options.port ?? 0 }, (info) => {
      const address = info.family === 'IPv6' ? `[${info.address}]` : info.address;
      origin = `http://${address}:${info.port}`;
      resolve(instance);
    });
    instance.once('error', reject);
  });

  return {
    origin,
    token,
    url: `${origin}/?token=${token}`,
    close: () =>
      new Promise<void>((resolve) => {
        // Open event streams would keep close() waiting; the http.Server of the adapter can drop them.
        (server as { closeAllConnections?: () => void }).closeAllConnections?.();
        server.close(() => resolve());
      }),
  };
}

/** `oboete view [--port N] [--open]`: foreground until interrupted (contracts/cli.md). */
export async function runView(argv: string[], overrides: Partial<ViewRuntime> = {}): Promise<number> {
  const runtime: ViewRuntime = {
    cwd: process.cwd(),
    open: openBrowser,
    writeOut: (text) => {
      process.stdout.write(text);
    },
    writeError: (text) => {
      process.stderr.write(text);
    },
    ...overrides,
  };
  let port = 0;
  let open: boolean;
  try {
    const { values } = parseArgs({
      args: argv,
      allowPositionals: false,
      strict: true,
      options: { port: { type: 'string' }, open: { type: 'boolean' } },
    });
    if (values.port !== undefined) {
      port = Number(values.port);
      if (!/^\d+$/u.test(values.port) || port > 65_535) throw new Error('--port must be an integer from 0 to 65535.');
    }
    open = values.open === true;
  } catch (error) {
    runtime.writeError(`${error instanceof Error ? error.message : String(error)}\n`);
    return 2;
  }

  let viewer: ViewerHandle;
  try {
    viewer = await startViewer({ ...runtime, port });
  } catch (error) {
    runtime.writeError(`${error instanceof Error ? error.message : String(error)}\n`);
    return 2;
  }
  runtime.writeOut(`${viewer.url}\n`);
  if (open) runtime.open(viewer.url);

  await new Promise<void>((resolve) => {
    const stop = (): void => {
      resolve();
    };
    if (runtime.signal !== undefined) {
      if (runtime.signal.aborted) stop();
      else runtime.signal.addEventListener('abort', stop, { once: true });
    } else {
      process.once('SIGINT', stop);
      process.once('SIGTERM', stop);
    }
  });
  await viewer.close();
  return 0;
}
