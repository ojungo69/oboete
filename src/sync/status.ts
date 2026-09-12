// Local-only sync state (contracts/sync.md "Commands and health"): the configured space, consent
// drift, cursors, conflicts and withheld rows, read from `config.toml` and the local tables. This
// is the only sync module `oboete doctor` and MCP may import; it performs no I/O below the space
// directory and never touches a key or a bundle.
import type { DatabaseSync } from 'node:sqlite';

import { loadConfig, type SyncConfig } from '../config.js';
import { prepared } from '../db/statements.js';
import { compareCodeUnits, sha256Hex } from '../hash.js';
import type { OboetePaths } from '../paths.js';
import { canonicalJson } from './identity.js';

const CONSENT_TRANSPORT = 'file-bundle';
const CONSENT_ENCRYPTION = 'aes-256-gcm+hkdf-sha256 (oboete-sync-bundle/1)';

export class SyncError extends Error {
  constructor(readonly code: string, readonly detail: Record<string, unknown> = {}) {
    super(code);
    this.name = 'SyncError';
  }
}

/** The values consent is bound to, as `init` and `join` show them and `consentHashOf` hashes them. */
export type ConsentTuple = {
  transport: string; directory: string; directory_realpath: string; space_id: string; key_id: string;
  encryption: string; classes: string[]; network: string;
};

export function consentTupleOf(config: SyncConfig): ConsentTuple {
  return {
    transport: CONSENT_TRANSPORT, directory: config.directory, directory_realpath: config.directory_realpath,
    space_id: config.space_id, key_id: config.key_id, encryption: CONSENT_ENCRYPTION,
    classes: [...config.classes].sort(compareCodeUnits), network: 'no network',
  };
}

export function consentHashOf(config: SyncConfig): string {
  return sha256Hex(canonicalJson(consentTupleOf(config)));
}

export function loadSyncConfig(paths: OboetePaths): SyncConfig | null {
  return loadConfig(paths).sync ?? null;
}

/** Which consent fields drifted between `config.toml` and the recorded space; empty when consent holds. */
export function consentDrift(db: DatabaseSync, config: SyncConfig): string[] {
  const stored = prepared(db, 'SELECT * FROM sync_spaces WHERE space_id = ?').get(config.space_id);
  if (stored === undefined) return ['space'];
  if (consentHashOf(config) === String(stored.consent_hash)) return [];
  const changed: string[] = [];
  if (String(stored.directory) !== config.directory || String(stored.directory_realpath) !== config.directory_realpath) changed.push('directory');
  if (String(stored.key_id) !== config.key_id) changed.push('key_id');
  if (String(stored.classes_json) !== canonicalJson([...config.classes].sort(compareCodeUnits))) changed.push('classes');
  return changed.length === 0 ? ['consent'] : changed;
}

/**
 * How many rows of each open-ended listing `syncStatus` materializes. A peer's bundle may leave up
 * to a million origins withheld or unmapped in one pull, so the listings are a sample for the
 * operator to act on and `totals` carries the real count; `replicas` is left whole because
 * `BOUNDS.replicasPerSpace` already caps it at 32.
 */
const STATUS_LIST_LIMIT = 200;

export type SyncStatus = {
  configured: boolean; space_id?: string; directory?: string; classes?: string[]; replicas: { replica: string; snapshot_id: string; pulled_at: number }[];
  conflicts: { id: string; local_state: string; remote_state: string }[]; withheld_on_apply: { origin_id: string; kind: string; reason: string }[];
  unmapped_repos: { repo_key: string; identity_kind: string; normalized_identity: string }[];
  /** The full row count behind each listing above, which is capped at `STATUS_LIST_LIMIT`. */
  totals: { conflicts: number; withheld_on_apply: number; unmapped_repos: number };
};

function countOf(db: DatabaseSync, sql: string): number {
  return Number(prepared(db, sql).get()?.n ?? 0);
}

/** `oboete sync status`: local data only; never opens the space directory. */
export function syncStatus(db: DatabaseSync, paths: OboetePaths): SyncStatus {
  const config = loadSyncConfig(paths);
  const status: SyncStatus = { configured: config !== null, replicas: [], conflicts: [], withheld_on_apply: [], unmapped_repos: [],
    totals: { conflicts: 0, withheld_on_apply: 0, unmapped_repos: 0 } };
  if (config === null) return status;
  status.space_id = config.space_id;
  status.directory = config.directory;
  status.classes = [...config.classes];
  for (const row of prepared(db, 'SELECT replica_origin_id, snapshot_id, pulled_at FROM sync_cursors WHERE space_id = ? ORDER BY replica_origin_id').iterate(config.space_id)) {
    status.replicas.push({ replica: String(row.replica_origin_id), snapshot_id: String(row.snapshot_id), pulled_at: Number(row.pulled_at) });
  }
  for (const row of prepared(db, `SELECT id, local_state_json, remote_state_json FROM sync_conflicts WHERE status = 'open'
    ORDER BY created_at, id LIMIT ?`).all(STATUS_LIST_LIMIT)) {
    status.conflicts.push({ id: String(row.id), local_state: String(row.local_state_json ?? ''), remote_state: String(row.remote_state_json ?? '') });
  }
  status.totals.conflicts = countOf(db, "SELECT COUNT(*) AS n FROM sync_conflicts WHERE status = 'open'");
  for (const row of prepared(db, `SELECT origin_id, kind, withheld_reason FROM sync_origins WHERE withheld_reason IS NOT NULL
    ORDER BY origin_id LIMIT ?`).all(STATUS_LIST_LIMIT)) {
    status.withheld_on_apply.push({ origin_id: String(row.origin_id), kind: String(row.kind), reason: String(row.withheld_reason) });
  }
  status.totals.withheld_on_apply = countOf(db, 'SELECT COUNT(*) AS n FROM sync_origins WHERE withheld_reason IS NOT NULL');
  for (const row of prepared(db, `SELECT repo_key, identity_kind, normalized_identity FROM sync_repo_mappings WHERE local_repo_id IS NULL
    ORDER BY repo_key LIMIT ?`).all(STATUS_LIST_LIMIT)) {
    status.unmapped_repos.push({ repo_key: String(row.repo_key), identity_kind: String(row.identity_kind), normalized_identity: String(row.normalized_identity) });
  }
  status.totals.unmapped_repos = countOf(db, 'SELECT COUNT(*) AS n FROM sync_repo_mappings WHERE local_repo_id IS NULL');
  return status;
}
