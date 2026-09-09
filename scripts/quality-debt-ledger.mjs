// Shared pieces of the quality-debt record: the evidence paths, the ledger row predicates, and the
// ledger index used by both the local checks and the service calls.
import { readFileSync, writeFileSync } from 'node:fs';

export const evidence = 'docs/evidence/quality-debt-2026-09';
export const codacyIssues = 'https://app.codacy.com/api/v3/analysis/organizations/gh/ojungo69/repositories/oboete/issues';
export const codacyReasons = ['AcceptedUse', 'FalsePositive', 'NotExploitable', 'TestCode', 'ExternalCode'];

export function readJson(name) {
  return JSON.parse(readFileSync(`${evidence}/${name}`, 'utf8'));
}

export function writeLedger(ledger) {
  writeFileSync(`${evidence}/ledger.json`, `${JSON.stringify(ledger, null, 1)}\n`);
}

export function identity(row) {
  return `${row.service}:${row.id}`;
}

export function dispositionState(row) {
  return ['fixed', 'resolved', 'excluded'].includes(row?.state) ? row.state : 'open';
}

export function isConfirmed(row) {
  return Boolean(row?.confirmed?.trim?.());
}

/** A resolved row's reason is the one line posted on the service: non-blank, single-line, and a Codacy enum where needed. */
export function resolvedWithoutReason(row) {
  return row.state === 'resolved' && (typeof row.where !== 'string' || !row.where.trim() || /[\r\n]/.test(row.where)
    || (row.service === 'codacy' && !codacyReasons.includes(row.reason)));
}

/** Rules whose findings are read one by one (research R6); a fixed or resolved row of one needs a verdict. */
export function securityPopulation(row) {
  if (row.service === 'codacy') return row.rule === 'shellcheck_SC2024' || row.rule.startsWith('Semgrep_');
  return ['S8786', 'S4036', 'S8707'].includes(row.rule.split(':').at(-1));
}

export function indexLedger(ledger) {
  const index = new Map();
  for (const row of ledger) {
    const key = identity(row);
    if (!index.has(key)) index.set(key, []);
    index.get(key).push(row);
  }
  return index;
}

export function validateInventory(rows, ledger, inventory) {
  const ids = new Set(inventory.map(identity));
  const index = indexLedger(ledger);
  for (const row of rows) {
    const key = identity(row);
    // identity() would coerce a hand-edited numeric id to the same key as the inventory's string.
    if (typeof row.id !== 'string' || !ids.has(key)) throw new Error(`${key} not in inventory`);
    if (index.get(key).length > 1) throw new Error(`${key} duplicate in ledger`);
  }
}
