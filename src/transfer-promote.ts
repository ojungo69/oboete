import type { DatabaseSync } from 'node:sqlite';
import { parseArgs } from 'node:util';

import { materialHash } from './db/identity.js';
import { openDatabase } from './db/open.js';
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
type PromotionArgs = { id: string; sourceWork: string; localWork: string; json: boolean };

function promotionArgs(argv: string[]): PromotionArgs | null {
  try {
    const { values, positionals } = parseArgs({ args: argv, allowPositionals: true, strict: true,
      options: { 'map-work': { type: 'string', multiple: true }, json: { type: 'boolean' } } });
    if (positionals.length !== 1 || values['map-work']?.length !== 1) return null;
    const entry = values['map-work'][0];
    const split = entry.lastIndexOf('=');
    const mapping = [entry.slice(0, split), entry.slice(split + 1)];
    if (split < 1 || mapping.some((id) => id.trim() === '' || id.length > 512)) return null;
    return { id: positionals[0], sourceWork: mapping[0], localWork: mapping[1], json: values.json === true };
  } catch { return null; }
}

function cleanCandidate(db: DatabaseSync, repoId: string, args: PromotionArgs) {
  const row = db.prepare(`SELECT r.payload_json, m.id, m.sensitivity FROM migration_records r
    JOIN memories m ON m.id = r.destination_memory_id AND m.repo_id = r.destination_repo_id
    JOIN work_items w ON w.id = ? AND w.repo_id = r.destination_repo_id
    WHERE r.id = ? AND r.destination_repo_id = ? AND r.record_kind = 'sharing_proposal'
      AND r.classification_state = 'clean' AND r.payload_json IS NOT NULL AND r.identity_domain = 'ordinary'
      AND m.review_state <> 'imported' AND m.deleted_at IS NULL AND m.sensitivity <> 'secret'
      AND m.type <> 'session_summary' AND m.valid_to IS NULL
      AND NOT EXISTS (SELECT 1 FROM memory_visibility v WHERE v.memory_id = m.id AND v.audience = 'personal')
      AND NOT EXISTS (SELECT 1 FROM migration_records p
        WHERE p.destination_memory_id = m.id AND p.identity_domain = 'personal_projection')`)
    .get(args.localWork, args.id, repoId);
  if (row === undefined) return null;
  const value: unknown = JSON.parse(String(row.payload_json));
  if (!migrationPayloadShape(value)) return null;
  const payload = value as NativeRecord;
  if (payload.kind !== 'sharing_proposal' || payload.origin_work_id !== args.sourceWork
    || payload.candidate_body.trim() === '') return null;
  return { memoryId: String(row.id), sensitivity: row.sensitivity as Sensitivity, payload };
}

function promote(db: DatabaseSync, args: PromotionArgs) {
  const identity = resolveRepoIdentity(process.cwd());
  const context = db.prepare('SELECT id FROM work_contexts WHERE repo_id = ? AND local_key = ?')
    .get(identity.id, identity.worktreeKey);
  if (context === undefined || verifiedRepoContext(db, identity.id, String(context.id))?.local_key !== identity.worktreeKey) return null;
  return transactionImmediate(db, () => {
    if (db.prepare('SELECT 1 FROM work_contexts WHERE id = ? AND repo_id = ? AND local_key = ?')
      .get(context.id, identity.id, identity.worktreeKey) === undefined) return null;
    const candidate = cleanCandidate(db, identity.id, args);
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
  });
}

export async function runImportPromote(argv: string[], io: Io): Promise<number> {
  const args = promotionArgs(argv);
  if (args === null) {
    io.writeError('Usage: oboete import promote <migration-record-id> --map-work <source-work>=<local-work> [--json]\n');
    return 2;
  }
  let db: DatabaseSync | undefined;
  let result: ReturnType<typeof promote> = null;
  try {
    if (/^[0-9a-f]{64}$/u.test(args.id)) {
      const options = { path: oboetePaths(resolveHome()).db, timeoutMs: 2_000 };
      const checked = openDatabase({ ...options, readOnly: true });
      checked.db.close();
      if (!checked.schemaBehind) {
        // 009 contracts/migration.md: unavailable promotion must not create or migrate a store.
        const opened = openDatabase({ ...options, hook: true });
        db = opened.db;
        if (!opened.schemaBehind) result = promote(db, args);
      }
    }
  } catch { /* 009 contracts/migration.md requires one fixed unavailable result. */ }
  finally { db?.close(); }
  if (result === null) {
    io.writeError('The migration record is unavailable in this scope.\n');
    return 1;
  }
  await io.writeOut(args.json ? `${JSON.stringify(result)}\n`
    : `Sharing proposal ${result.id} is ${result.state}. Use oboete share status to review the candidate.\n`);
  return 0;
}
