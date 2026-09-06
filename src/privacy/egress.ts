import type { DatabaseSync } from 'node:sqlite';

export type Destination = 'remote_observer' | 'local_observer' | 'injection' | 'sync';
export type Sensitivity = 'local_only' | 'eligible' | 'secret' | 'private';

const DESTINATIONS: Destination[] = ['remote_observer', 'local_observer', 'injection', 'sync'];
const SENSITIVITIES: Sensitivity[] = ['local_only', 'eligible', 'secret', 'private'];

export type DestinationRules = Map<
  Destination,
  { allowed: Set<Sensitivity>; sameRepoRequired: boolean }
>;

/**
 * A row an egress decision is made about. The producing agent is provenance only (FR-005, R10), so
 * it is absent here and no egress decision can read it.
 */
export type EgressRow = { sensitivity: Sensitivity; repoId: string };

export type EgressBlock = { row: EgressRow; reason: 'sensitivity' | 'repository' };

/** Reads the seeded `destination_rules` table. Callers read it once per process. */
export function loadDestinationRules(db: DatabaseSync): DestinationRules {
  const rules: DestinationRules = new Map();
  const rows = db
    .prepare('SELECT destination, sensitivity, allowed, same_repo_required FROM destination_rules')
    .all();

  for (const row of rows) {
    const destination = String(row.destination) as Destination;
    const sensitivity = String(row.sensitivity) as Sensitivity;
    if (!DESTINATIONS.includes(destination) || !SENSITIVITIES.includes(sensitivity)) continue;
    // A row that is not allowed carries no scope, so only allowed rows shape the entry.
    if (Number(row.allowed) !== 1) continue;

    const entry = rules.get(destination) ?? { allowed: new Set<Sensitivity>(), sameRepoRequired: false };
    entry.allowed.add(sensitivity);
    if (Number(row.same_repo_required) === 1) entry.sameRepoRequired = true;
    rules.set(destination, entry);
  }
  return rules;
}

/**
 * The single egress decision (FR-020, data-model "One function evaluates the table"): the table
 * must allow the class at this destination, and a destination that requires the same repository
 * only accepts a row of that repository.
 */
export function isAllowed(
  rules: DestinationRules,
  destination: Destination,
  sensitivity: Sensitivity,
  sameRepo: boolean,
): boolean {
  // FR-020: a secret row is never sent anywhere, whatever the table happens to say.
  if (sensitivity === 'secret') return false;
  const entry = rules.get(destination);
  if (entry === undefined || !entry.allowed.has(sensitivity)) return false;
  return !entry.sameRepoRequired || sameRepo;
}

/** Splits rows into the ones this destination may receive and the ones it may not, with the reason. */
export function filterEgress(
  rules: DestinationRules,
  destination: Destination,
  rows: readonly EgressRow[],
  targetRepoId: string,
): { allowed: EgressRow[]; blocked: EgressBlock[] } {
  const allowed: EgressRow[] = [];
  const blocked: EgressBlock[] = [];

  for (const row of rows) {
    const sameRepo = row.repoId === targetRepoId;
    if (isAllowed(rules, destination, row.sensitivity, sameRepo)) {
      allowed.push(row);
      continue;
    }
    // The class would pass for a row of the target repository, so the scope is what blocks it.
    const reason =
      !sameRepo && isAllowed(rules, destination, row.sensitivity, true) ? 'repository' : 'sensitivity';
    blocked.push({ row, reason });
  }

  return { allowed, blocked };
}
