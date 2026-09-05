import assert from 'node:assert/strict';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { PRESET_CATALOG } from '../../src/config.js';
import { openDatabase } from '../../src/db/open.js';
import { refreshWorkersAiCatalog } from '../../src/observer/catalog.js';
import { oboetePaths } from '../../src/paths.js';
import { runtimeStateGet } from '../../src/worker/purge.js';
import { withTempHome } from '../helpers/home.js';

const ENV = {
  OBOETE_CF_API_TOKEN: 'catalog-token',
  OBOETE_CF_ACCOUNT_ID: 'account-id',
};
const DAY = 24 * 60 * 60 * 1000;

async function withOpened(fn: (db: DatabaseSync) => void | Promise<void>): Promise<void> {
  await withTempHome(async (home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      await fn(opened.db);
    } finally {
      if (opened.db.isOpen) opened.db.close();
    }
  });
}

function requestedPage(input: string | URL | Request): number {
  return Number(new URL(String(input)).searchParams.get('page') ?? '1');
}

function catalogResponse(page = 1): Response {
  return new Response(
    JSON.stringify({
      success: true,
      result:
        page === 1
          ? [
              {
                name: PRESET_CATALOG['workers-ai'].defaultModel,
                properties: [{ property_id: 'price', value: [{ unit: 'per token', price: 0.1 }] }],
              },
              {
                name: '@cf/zai-org/glm-5.2',
                properties: [{ property_id: 'require_workers_paid', value: 'true' }],
              },
            ]
          : [],
      errors: [],
      messages: [],
    }),
    { status: 200, headers: { 'content-type': 'application/json' } },
  );
}

test('Workers AI catalog is parsed, cached for 24 hours, and then refreshed', async () => {
  await withOpened(async (db) => {
    const now = 1_757_000_000_000;
    let calls = 0;
    const fetchImpl: typeof fetch = async (input, init) => {
      calls += 1;
      const page = requestedPage(input);
      assert.equal(
        String(input),
        `https://api.cloudflare.com/client/v4/accounts/account-id/ai/models/search?per_page=100&page=${page}`,
      );
      assert.equal(new Headers(init?.headers).get('authorization'), 'Bearer catalog-token');
      assert.ok(init?.signal instanceof AbortSignal);
      return catalogResponse(page);
    };

    const first = await refreshWorkersAiCatalog(db, { env: ENV, now, fetchImpl });
    assert.deepEqual(first, {
      models: [PRESET_CATALOG['workers-ai'].defaultModel, '@cf/zai-org/glm-5.2'],
      defaultModelPresent: true,
      hasPaidOnlyModels: true,
      fetchedAt: now,
      fromCache: false,
    });
    assert.equal(calls, 2);

    const stored = runtimeStateGet(db, 'workers_ai_catalog');
    assert.ok(stored);
    assert.equal(stored.includes(ENV.OBOETE_CF_API_TOKEN), false);
    assert.deepEqual(JSON.parse(stored), {
      accountId: ENV.OBOETE_CF_ACCOUNT_ID,
      models: [PRESET_CATALOG['workers-ai'].defaultModel, '@cf/zai-org/glm-5.2'],
      defaultModelPresent: true,
      hasPaidOnlyModels: true,
      fetchedAt: now,
    });

    const cached = await refreshWorkersAiCatalog(db, { env: ENV, now: now + DAY - 1, fetchImpl });
    assert.deepEqual(cached, { ...first, fromCache: true });
    assert.equal(calls, 2);

    const refreshed = await refreshWorkersAiCatalog(db, {
      env: ENV,
      now: now + DAY + 60 * 60 * 1000,
      fetchImpl,
    });
    assert.equal(refreshed?.fetchedAt, now + DAY + 60 * 60 * 1000);
    assert.equal(refreshed?.fromCache, false);
    assert.equal(calls, 4);
  });
});

test('Workers AI catalog aggregates short pages until an empty page', async () => {
  await withOpened(async (db) => {
    const now = 1_757_000_000_000;
    const pages = [
      { result: [{ name: '@cf/example/first-page-model', properties: [] }] },
      {
        result: [
          { name: PRESET_CATALOG['workers-ai'].defaultModel, properties: [] },
          {
            name: '@cf/example/paid-only-model',
            properties: [{ property_id: 'require_workers_paid', value: true }],
          },
        ],
      },
      { result: [] },
      { result: null },
    ];
    const requestedPages: number[] = [];
    const signals: unknown[] = [];
    const fetchImpl: typeof fetch = async (input, init) => {
      const url = new URL(String(input));
      const page = requestedPage(url);
      requestedPages.push(page);
      signals.push(init?.signal);
      return new Response(
        JSON.stringify({ success: true, ...pages[page - 1], errors: [], messages: [] }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      );
    };

    const result = await refreshWorkersAiCatalog(db, { env: ENV, now, fetchImpl });

    assert.deepEqual(requestedPages, [1, 2, 3]);
    // Each request carries its own AbortSignal.timeout rather than one shared budget.
    assert.equal(new Set(signals).size, 3);
    assert.ok(signals.every((signal) => signal instanceof AbortSignal));
    assert.deepEqual(result, {
      models: [
        '@cf/example/first-page-model',
        PRESET_CATALOG['workers-ai'].defaultModel,
        '@cf/example/paid-only-model',
      ],
      defaultModelPresent: true,
      hasPaidOnlyModels: true,
      fetchedAt: now,
      fromCache: false,
    });
  });
});

for (const hasCache of [false, true]) {
  test(`an endless Workers AI catalog stops at 20 requests and returns ${hasCache ? 'the stale cache' : 'null'}`, async () => {
    await withOpened(async (db) => {
      const cached = hasCache
        ? await refreshWorkersAiCatalog(db, {
            env: ENV,
            now: 1,
            fetchImpl: async (input) => catalogResponse(requestedPage(input)),
          })
        : null;
      const stored = runtimeStateGet(db, 'workers_ai_catalog');
      const fullPage = Array.from({ length: 100 }, (_, index) => ({
        name: `@cf/example/model-${index}`,
        properties: [],
      }));
      let calls = 0;
      const result = await refreshWorkersAiCatalog(db, {
        env: ENV,
        now: DAY + 2,
        fetchImpl: async () => {
          calls += 1;
          // Fail an extra request so a missing bound cannot hang this test.
          assert.ok(calls <= 20, 'catalog exceeded the page limit');
          return new Response(JSON.stringify({ success: true, result: fullPage }));
        },
      });

      assert.equal(calls, 20);
      assert.deepEqual(result, cached === null ? null : { ...cached, fromCache: true });
      assert.equal(runtimeStateGet(db, 'workers_ai_catalog'), stored);
    });
  });
}

test('changing the Workers AI account bypasses the shared cache entry', async () => {
  await withOpened(async (db) => {
    const now = 1_757_000_000_000;
    await refreshWorkersAiCatalog(db, {
      env: ENV,
      now,
      fetchImpl: async (input) => catalogResponse(requestedPage(input)),
    });
    let calls = 0;
    const fetchImpl: typeof fetch = async (input) => {
      calls += 1;
      assert.match(String(input), /accounts\/other-account\/ai\/models\/search/);
      return catalogResponse(requestedPage(input));
    };
    const result = await refreshWorkersAiCatalog(db, {
      env: { ...ENV, OBOETE_CF_ACCOUNT_ID: 'other-account' },
      now: now + 1,
      fetchImpl,
    });
    assert.equal(calls, 2);
    assert.equal(result?.fromCache, false);
    assert.equal(JSON.parse(runtimeStateGet(db, 'workers_ai_catalog') ?? '{}').accountId, 'other-account');
  });
});

test('a fetch failure returns the stale cached catalog without throwing', async () => {
  await withOpened(async (db) => {
    const now = 1_757_000_000_000;
    await refreshWorkersAiCatalog(db, {
      env: ENV,
      now,
      fetchImpl: async (input) => catalogResponse(requestedPage(input)),
    });
    let calls = 0;
    const fetchImpl: typeof fetch = async () => {
      calls += 1;
      throw new Error('offline');
    };

    const result = await refreshWorkersAiCatalog(db, {
      env: ENV,
      now: now + DAY + 1,
      fetchImpl,
    });
    assert.equal(calls, 1);
    assert.equal(result?.fetchedAt, now);
    assert.equal(result?.fromCache, true);
  });
});

test('an HTTP failure without a cache returns null', async () => {
  await withOpened(async (db) => {
    const result = await refreshWorkersAiCatalog(db, {
      env: ENV,
      now: 1,
      fetchImpl: async () => new Response('unavailable', { status: 503 }),
    });
    assert.equal(result, null);
  });
});

test('missing Workers AI credentials returns null without fetching', async () => {
  await withOpened(async (db) => {
    let calls = 0;
    const fetchImpl: typeof fetch = async () => {
      calls += 1;
      return catalogResponse();
    };
    const result = await refreshWorkersAiCatalog(db, {
      env: { OBOETE_CF_API_TOKEN: 'token-without-account' },
      now: 1,
      fetchImpl,
    });
    assert.equal(result, null);
    assert.equal(calls, 0);
    assert.equal(runtimeStateGet(db, 'workers_ai_catalog'), undefined);
  });
});
