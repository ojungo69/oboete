import type { DatabaseSync, SQLOutputValue } from 'node:sqlite';
import { parseArgs } from 'node:util';

import { materialHash } from './db/identity.js';
import { DatabaseMissingError, SchemaAheadError, openDatabase } from './db/open.js';
import { grantVisibility } from './db/queries.js';
import { sha256Json } from './hash.js';
import { oboetePaths, resolveHome } from './paths.js';
import { strictest } from './privacy/classify.js';
import type { Sensitivity } from './privacy/egress.js';
import { verifiedRepoContext } from './privacy/source-context.js';
import { resolveRepoIdentity } from './repo-identity.js';
import { migrationPayloadShape, type NativeRecord } from './transfer-format.js';
import { transactionImmediate } from './worker/lease.js';

type Io = { writeOut(text: string): void | Promise<void>; writeError(text: string): void };
type PromotionArgs = { json: boolean } & ({ list: true } | { list: false; id: string; localWork: string });

function promotionArgs(argv: string[]): PromotionArgs | null {
  try {
    const { values, positionals } = parseArgs({ args: argv, allowPositionals: true, strict: true,
      options: { work: { type: 'string', multiple: true }, list: { type: 'boolean' }, json: { type: 'boolean' } } });
    if (values.list) return positionals.length === 0 && values.work === undefined
      ? { list: true, json: values.json === true } : null;
    if (positionals.length !== 1 || values.work?.length !== 1) return null;
    const localWork = values.work[0];
    if (localWork.trim() === '' || localWork.length > 512) return null;
    return { list: false, id: positionals[0], localWork, json: values.json === true };
  } catch { return null; }
}

const CANDIDATE_ROWS = `SELECT r.id, r.destination_memory_id AS memory, r.classification_state AS state,
    r.effect, r.promoted_proposal_id AS proposal, r.payload_json, m.sensitivity,
    (r.classification_state = 'clean' AND r.payload_json IS NOT NULL AND r.identity_domain = 'ordinary'
      AND r.effect = 'inserted'
      AND m.review_state <> 'imported' AND m.deleted_at IS NULL AND m.sensitivity <> 'secret'
      AND m.type <> 'session_summary' AND m.valid_to IS NULL
      AND NOT EXISTS (SELECT 1 FROM memory_visibility v WHERE v.memory_id = m.id AND v.audience = 'personal')
      AND NOT EXISTS (SELECT 1 FROM migration_records p
        WHERE p.destination_memory_id = m.id AND p.identity_domain = 'personal_projection')) AS eligible
    FROM migration_records r
    LEFT JOIN memories m ON m.id = r.destination_memory_id AND m.repo_id = r.destination_repo_id
    WHERE r.destination_repo_id = ? AND r.record_kind = 'sharing_proposal'`;

function cleanCandidate(row: Record<string, SQLOutputValue> | undefined) {
  if (row?.eligible !== 1) return null;
  const value: unknown = JSON.parse(String(row.payload_json));
  if (!migrationPayloadShape(value)) return null;
  const payload = value as NativeRecord;
  if (payload.kind !== 'sharing_proposal' || payload.candidate_body.trim() === '') return null;
  return { memoryId: String(row.memory), sensitivity: row.sensitivity as Sensitivity, payload };
}

function promote(db: DatabaseSync, args: PromotionArgs) {
  const identity = resolveRepoIdentity(process.cwd());
  const context = db.prepare('SELECT id FROM work_contexts WHERE repo_id = ? AND local_key = ?')
    .get(identity.id, identity.worktreeKey);
  if (context === undefined || verifiedRepoContext(db, identity.id, String(context.id))?.local_key !== identity.worktreeKey) return null;
  const execute = () => {
    if (db.prepare('SELECT 1 FROM work_contexts WHERE id = ? AND repo_id = ? AND local_key = ?')
      .get(context.id, identity.id, identity.worktreeKey) === undefined) return null;
    if (args.list) {
      const total = Number(db.prepare(`SELECT COUNT(*) AS n FROM (${CANDIDATE_ROWS})`).get(identity.id)?.n ?? 0);
      const records = db.prepare(`${CANDIDATE_ROWS} ORDER BY r.id LIMIT 100`).all(identity.id).map((row) => ({
        id: String(row.id), memory: row.memory === null ? null : String(row.memory), state: String(row.state),
        effect: String(row.effect), promotable: cleanCandidate(row) !== null,
        proposal: row.proposal === null ? null : String(row.proposal),
      }));
      return { records, omitted: Math.max(0, total - records.length) };
    }
    if (db.prepare('SELECT 1 FROM work_items WHERE id = ? AND repo_id = ?').get(args.localWork, identity.id) === undefined) return null;
    const candidate = cleanCandidate(db.prepare(`${CANDIDATE_ROWS} AND r.id = ?`).get(identity.id, args.id));
    if (candidate === null) return null;
    const { memoryId, payload } = candidate;
    const title = payload.candidate_title;
    const body = payload.candidate_body;
    const now = Date.now();
    const id = `sp_${sha256Json(['migration-proposal-v1', memoryId, args.localWork, title, body])}`;
    grantVisibility(db, memoryId, { audience: 'work', repoId: identity.id, workId: args.localWork }, 'migration', now);
    db.prepare(`INSERT OR IGNORE INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id,
      candidate_title, candidate_body, candidate_material_hash, candidate_sensitivity, source_event_ids_json,
      basis, state, decision_channel, projected_memory_id, created_at, decided_at)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, '[]', 'inferred', 'pending', NULL, NULL, ?, NULL)`)
      .run(id, memoryId, identity.id, args.localWork, title, body, materialHash(title, body),
        strictest(candidate.sensitivity, payload.candidate_sensitivity), now);
    db.prepare('UPDATE migration_records SET promoted_proposal_id = ? WHERE id = ?').run(id, args.id);
    const proposal = db.prepare('SELECT state FROM sharing_proposals WHERE id = ?').get(id)!;
    return { id, state: String(proposal.state) };
  };
  if (!args.list) return transactionImmediate(db, execute);
  db.exec('BEGIN');
  try { return execute(); } finally { db.exec('ROLLBACK'); }
}

export async function runImportPromote(argv: string[], io: Io): Promise<number> {
  const args = promotionArgs(argv);
  if (args === null) {
    io.writeError('Usage: oboete import promote <migration-record-id> --work <local-work-id> [--json]\n'
      + '       oboete import promote --list [--json]\n');
    return 2;
  }
  let db: DatabaseSync | undefined;
  let result: ReturnType<typeof promote> = null;
  try {
    if (args.list || /^[0-9a-f]{64}$/u.test(args.id)) {
      const options = { path: oboetePaths(resolveHome()).db, timeoutMs: 2_000 };
      let opened = openDatabase({ ...options, readOnly: true });
      db = opened.db;
      if (!opened.schemaBehind && !args.list) {
        // 009 contracts/migration.md: unavailable promotion must not create or migrate a store.
        db.close();
        db = undefined;
        opened = openDatabase({ ...options, hook: true });
        db = opened.db;
      }
      if (!opened.schemaBehind) result = promote(db, args);
    }
  } catch (error) {
    if (!(error instanceof DatabaseMissingError || error instanceof SchemaAheadError)) throw error;
  }
  finally { db?.close(); }
  if (result === null) {
    io.writeError('The migration record is unavailable in this scope.\n');
    return 1;
  }
  if (result.records !== undefined) {
    await io.writeOut(args.json ? `${JSON.stringify(result)}\n` : result.records.map((row) =>
      `${row.id}  memory=${row.memory}  state=${row.state}  effect=${row.effect}  promotable=${row.promotable}  proposal=${row.proposal}\n`).join('')
      + (result.omitted > 0 ? `${result.omitted} more records omitted.\n` : ''));
  } else await io.writeOut(args.json ? `${JSON.stringify(result)}\n`
    : `Sharing proposal ${result.id} is ${result.state}. Use oboete share status to review the candidate.\n`);
  return 0;
}
