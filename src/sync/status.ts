// Local-only sync state (contracts/sync.md "Commands and health"): the configured space, consent
// drift, cursors, conflicts and withheld rows, read from `config.toml` and the local tables. This
// is the only sync module `oboete doctor` and MCP may import; it performs no I/O below the space
// directory and never touches a key or a bundle.
import type { DatabaseSync } from 'node:sqlite';

import { loadConfig, type SyncConfig } from '../config.js';
import { prepared } from '../db/statements.js';
import { sha256Hex } from '../hash.js';
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

export function consentTupleOf(config: SyncConfig): Record<string, unknown> {
  return {
    transport: CONSENT_TRANSPORT, directory: config.directory, directory_realpath: config.directory_realpath,
    space_id: config.space_id, key_id: config.key_id, encryption: CONSENT_ENCRYPTION,
    classes: [...config.classes].sort(), network: 'no network',
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
  if (String(stored.classes_json) !== canonicalJson([...config.classes].sort())) changed.push('classes');
  return changed.length === 0 ? ['consent'] : changed;
}

export type SyncStatus = {
  configured: boolean; space_id?: string; directory?: string; classes?: string[]; replicas: { replica: string; snapshot_id: string; pulled_at: number }[];
  conflicts: { id: string; local_state: string; remote_state: string }[]; withheld_on_apply: { origin_id: string; kind: string; reason: string }[];
  unmapped_repos: { repo_key: string; identity_kind: string; normalized_identity: string }[];
};

/** `oboete sync status`: local data only; never opens the space directory. */
export function syncStatus(db: DatabaseSync, paths: OboetePaths): SyncStatus {
  const config = loadSyncConfig(paths);
  const status: SyncStatus = { configured: config !== null, replicas: [], conflicts: [], withheld_on_apply: [], unmapped_repos: [] };
  if (config === null) return status;
  status.space_id = config.space_id;
  status.directory = config.directory;
  status.classes = [...config.classes];
  for (const row of prepared(db, 'SELECT replica_origin_id, snapshot_id, pulled_at FROM sync_cursors WHERE space_id = ? ORDER BY replica_origin_id').iterate(config.space_id)) {
    status.replicas.push({ replica: String(row.replica_origin_id), snapshot_id: String(row.snapshot_id), pulled_at: Number(row.pulled_at) });
  }
  for (const row of prepared(db, "SELECT id, local_state_json, remote_state_json FROM sync_conflicts WHERE status = 'open' ORDER BY created_at, id").iterate()) {
    status.conflicts.push({ id: String(row.id), local_state: String(row.local_state_json ?? ''), remote_state: String(row.remote_state_json ?? '') });
  }
  for (const row of prepared(db, 'SELECT origin_id, kind, withheld_reason FROM sync_origins WHERE withheld_reason IS NOT NULL ORDER BY origin_id').iterate()) {
    status.withheld_on_apply.push({ origin_id: String(row.origin_id), kind: String(row.kind), reason: String(row.withheld_reason) });
  }
  for (const row of prepared(db, 'SELECT repo_key, identity_kind, normalized_identity FROM sync_repo_mappings WHERE local_repo_id IS NULL ORDER BY repo_key').iterate()) {
    status.unmapped_repos.push({ repo_key: String(row.repo_key), identity_kind: String(row.identity_kind), normalized_identity: String(row.normalized_identity) });
  }
  return status;
}
