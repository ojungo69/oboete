import type { DatabaseSync } from 'node:sqlite';
import { PRESET_CATALOG, readCredentials } from '../config.js';
import { runtimeStateGet, runtimeStateSet } from '../worker/purge.js';

const CACHE_KEY = 'workers_ai_catalog';
const CACHE_MS = 24 * 60 * 60 * 1000;
const PAGE_SIZE = 100;
const MAX_PAGES = 20;
const REQUEST_TIMEOUT_MS = 10_000;
// The whole walk shares one budget so a slow endpoint cannot hold the worker for MAX_PAGES x 10 s.
const WALK_BUDGET_MS = 30_000;
type CatalogValue = {
  models: string[];
  defaultModelPresent: boolean;
  hasPaidOnlyModels: boolean;
  fetchedAt: number;
};
type CachedCatalog = CatalogValue & { accountId: string };
export type WorkersAiCatalog = CatalogValue & { fromCache: boolean };

function result(row: CachedCatalog, fromCache: boolean): WorkersAiCatalog {
  return {
    models: row.models,
    defaultModelPresent: row.defaultModelPresent,
    hasPaidOnlyModels: row.hasPaidOnlyModels,
    fetchedAt: row.fetchedAt,
    fromCache,
  };
}
function cachedCatalog(db: DatabaseSync): CachedCatalog | null {
  try {
    const row = JSON.parse(runtimeStateGet(db, CACHE_KEY) ?? 'null') as Partial<CachedCatalog> | null;
    return row !== null && Array.isArray(row.models) && row.models.every((model) => typeof model === 'string')
      && typeof row.defaultModelPresent === 'boolean' && typeof row.hasPaidOnlyModels === 'boolean'
      && typeof row.fetchedAt === 'number' && typeof row.accountId === 'string' ? row as CachedCatalog : null;
  } catch {
    return null;
  }
}

function paidOnly(model: unknown): boolean {
  if (typeof model !== 'object' || model === null) return false;
  const properties = (model as { properties?: unknown }).properties;
  if (!Array.isArray(properties)) return false;
  // R12/R13: model records expose property_id/value; price has no free-tier field.
  return properties.some((item) => {
    if (typeof item !== 'object' || item === null) return false;
    const property = item as { property_id?: unknown; value?: unknown };
    return (
      property.property_id === 'require_workers_paid' &&
      (property.value === true || property.value === 'true' || property.value === 1)
    );
  });
}

export async function refreshWorkersAiCatalog(
  db: DatabaseSync,
  { env, now, fetchImpl = fetch }: { env: NodeJS.ProcessEnv; now: number; fetchImpl?: typeof fetch },
): Promise<WorkersAiCatalog | null> {
  const credentials = readCredentials('workers-ai', env);
  if (!credentials.present) return null;
  const { accountId = '', token = '' } = credentials.values;
  const cached = cachedCatalog(db);
  const usableCached = cached?.accountId === accountId ? cached : null;
  if (usableCached !== null && now >= usableCached.fetchedAt && now - usableCached.fetchedAt < CACHE_MS) {
    return result(usableCached, true);
  }
  try {
    const rows: unknown[] = [];
    const walkDeadline = Date.now() + WALK_BUDGET_MS;
    for (let page = 1; ; page += 1) {
      if (page > MAX_PAGES) throw new Error('catalog page limit exceeded');
      const remaining = walkDeadline - Date.now();
      if (remaining <= 0) throw new Error('catalog walk budget exceeded');
      const response = await fetchImpl(
        `https://api.cloudflare.com/client/v4/accounts/${encodeURIComponent(accountId)}/ai/models/search?per_page=${PAGE_SIZE}&page=${page}`,
        {
          headers: { Authorization: `Bearer ${token}` },
          signal: AbortSignal.timeout(Math.min(REQUEST_TIMEOUT_MS, remaining)),
        },
      );
      if (!response.ok) throw new Error(`catalog HTTP ${response.status}`);
      const body: unknown = await response.json();
      if (typeof body !== 'object' || body === null || (body as { success?: unknown }).success !== true) {
        throw new Error('catalog response unsuccessful');
      }
      const pageRows = (body as { result?: unknown }).result;
      if (!Array.isArray(pageRows)) throw new Error('catalog result missing');
      rows.push(...pageRows);
      // Live models/search probe (2026-09-05): short pages and total_count do not imply exhaustion.
      if (pageRows.length === 0) break;
    }
    const models = rows.flatMap((model) => typeof model === 'object' && model !== null
      && typeof (model as { name?: unknown }).name === 'string'
      ? [(model as { name: string }).name] : []);
    const value: CachedCatalog = {
      accountId,
      models,
      defaultModelPresent: models.includes(PRESET_CATALOG['workers-ai'].defaultModel),
      hasPaidOnlyModels: rows.some(paidOnly),
      fetchedAt: now,
    };
    runtimeStateSet(db, CACHE_KEY, JSON.stringify(value), now);
    return result(value, false);
  } catch {
    return usableCached === null ? null : result(usableCached, true);
  }
}
