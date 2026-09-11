import type { DatabaseSync } from 'node:sqlite';

import { materialHash, memoryIdFor } from './db/identity.js';
import { getMemory, grantVisibility, memoryScope, type VisibilityGrant } from './db/queries.js';
import { compareCodeUnits, sha256Json } from './hash.js';
import type { ApplyInput, ApplyResult } from './observer/apply.js';
import { batchWork } from './observer/checkpoint.js';
import type { Observation } from './observer/contract.js';
import { strictest } from './privacy/classify.js';
import { detectSync } from './privacy/detect.js';
import type { Sensitivity } from './privacy/egress.js';
import { filterReadOutput, injectionPrivacy, type PrivacyLocation } from './privacy/provenance.js';
import { cjkBigrams } from './retrieval/fts.js';
import { payloadOf, type RawEventRow } from './worker/batches.js';
import { transactionImmediate } from './worker/lease.js';

type Proposal = {
  id: string; origin_memory_id: string; origin_repo_id: string; origin_work_id: string;
  candidate_title: string; candidate_body: string; candidate_material_hash: string;
  candidate_sensitivity: Sensitivity; source_event_ids_json: string;
  basis: 'direct_declaration' | 'inferred'; state: 'pending' | 'approved' | 'rejected';
  decision_channel: string | null; projected_memory_id: string | null; created_at: number; decided_at: number | null;
};
type Decision = { id: string; state: Proposal['state']; projectedMemoryId: string | null };
const decisionOf = (proposal: Proposal): Decision => ({ id: proposal.id, state: proposal.state,
  projectedMemoryId: proposal.projected_memory_id });

function readProposal(db: DatabaseSync, repoId: string, id: string): Proposal | undefined {
  return db.prepare('SELECT * FROM sharing_proposals WHERE id = ? AND origin_repo_id = ?')
    .get(id, repoId) as Proposal | undefined;
}

function declaration(row: RawEventRow): { title: string; body: string } | null {
  if (row.kind !== 'prompt' || row.classification_state !== 'done' || row.truncated !== 0 || row.sensitivity === 'secret'
    || payloadOf(row)?.input_source !== 'user') return null;
  const text = row.content ?? '';
  if (/[\r\n\u2028\u2029]/u.test(text)) return null;
  const matched = /^(Personal preference:|個人設定[:：])[ \t]*([^ \t\r\n][^\r\n]*)$/u.exec(text);
  const body = matched?.[2].trim();
  if (body === undefined || body === '' || body.length > 500) return null;
  return { title: matched![1].startsWith('個人設定') ? '個人設定' : 'Personal preference', body };
}

/** Runs inside the caller's transaction. Repeated approval cannot resurrect deleted content. */
function approveProjection(db: DatabaseSync, proposal: Proposal, sensitivity: Sensitivity,
  channel: 'automatic_direct' | 'cli' | 'viewer', now: number): Decision | null {
  if (sensitivity === 'secret') return null;
  const hash = sha256Json(['personal-projection-v1', proposal.candidate_title, proposal.candidate_body]);
  const id = memoryIdFor(hash);
  const prior = db.prepare('SELECT * FROM memories WHERE content_hash = ?').get(hash);
  if (prior !== undefined && (prior.title !== proposal.candidate_title || prior.body !== proposal.candidate_body
    || prior.material_hash !== proposal.candidate_material_hash || prior.deleted_at !== null || prior.valid_to !== null || prior.review_state === 'imported'
    || prior.sensitivity === 'secret' || prior.work_id !== null || prior.source_session_id !== null
    || prior.source_batch_id !== null || prior.checkpoint_parent_id !== null || prior.citations_head !== null
    || db.prepare('SELECT 1 FROM memory_sources WHERE memory_id = ? LIMIT 1').get(prior.id) !== undefined)) return null;
  const projectionId = prior === undefined ? id : String(prior.id);
  if (prior === undefined) db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, concepts,
    cjk_bigrams, material_hash, content_hash, sensitivity, review_state, valid_from, created_at)
    VALUES (?, ?, 'decision', ?, ?, '[]', ?, ?, ?, ?, 'reviewed', ?, ?)`)
    .run(id, proposal.origin_repo_id, proposal.candidate_title, proposal.candidate_body,
      cjkBigrams(`${proposal.candidate_title} ${proposal.candidate_body}`), proposal.candidate_material_hash,
      hash, sensitivity, now, now);
  else db.prepare('UPDATE memories SET sensitivity = ? WHERE id = ?')
    .run(strictest(sensitivity, prior.sensitivity as Sensitivity), projectionId);
  db.prepare(`UPDATE sharing_proposals SET state = 'approved', decision_channel = ?,
    projected_memory_id = ?, decided_at = ? WHERE id = ? AND state = 'pending'`)
    .run(channel, projectionId, now, proposal.id);
  grantVisibility(db, projectionId, { audience: 'personal', proposalId: proposal.id }, 'proposal_approval', now);
  return { id: proposal.id, state: 'approved', projectedMemoryId: projectionId };
}

/** Accepted batch identity owns all targets; the model only requests an audience. */
export function recordObservationSharing(db: DatabaseSync, input: ApplyInput, applied: ApplyResult['applied'],
  prepared: readonly { index: number; observation: Observation }[]): void {
  if (applied.every((item) => item.memoryId === null || item.decision === 'delete')) return;
  const work = batchWork(db, input);
  if (work === undefined) throw new Error('sharing_context_changed');
  for (const item of applied) {
    if (item.memoryId === null || item.decision === 'delete') continue;
    const observation = prepared.find((candidate) => candidate.index === item.index)?.observation;
    if (observation === undefined) throw new Error('sharing_context_changed');
    const sourceIds = [...new Set(observation.source_event_ids)];
    const rows = sourceIds.map((id) => db.prepare('SELECT * FROM raw_events WHERE id = ?').get(id) as RawEventRow | undefined);
    if (sourceIds.length === 0 || sourceIds.length > 50 || rows.some((row) => row === undefined
      || row.repo_id !== input.repoId || row.session_id !== input.sessionId || row.work_binding_id !== work.work_binding_id)) {
      throw new Error('sharing_source_changed');
    }
    const origin = db.prepare(`SELECT * FROM memories WHERE id = ? AND repo_id = ? AND type <> 'session_summary'
      AND deleted_at IS NULL AND review_state <> 'imported' AND sensitivity <> 'secret'`)
      .get(item.memoryId, input.repoId);
    if (origin === undefined || (origin.valid_to !== null && item.historical === undefined)) continue;
    if ((item.decision === 'update' || item.historical?.reason === 'capture_time_order')
      && observation.classification.target !== null && input.nearby.some((row) => row.id === observation.classification.target)) {
      for (const grant of db.prepare(`SELECT audience, repo_id, work_id FROM memory_visibility WHERE memory_id = ?
        AND audience IN ('work', 'project') AND repo_id = ?`).all(observation.classification.target, input.repoId)) {
        const inherited: VisibilityGrant = grant.audience === 'work'
          ? { audience: 'work', repoId: String(grant.repo_id), workId: String(grant.work_id) }
          : { audience: 'project', repoId: String(grant.repo_id) };
        grantVisibility(db, item.memoryId, inherited, 'observer', input.now);
      }
    }
    grantVisibility(db, item.memoryId, { audience: 'work', repoId: input.repoId, workId: work.work_id }, 'observer', input.now);
    if (input.fallbackReason !== null) continue;
    if (observation.visibility === 'project') {
      grantVisibility(db, item.memoryId, { audience: 'project', repoId: input.repoId }, 'observer', input.now);
    }
    if (observation.visibility !== 'personal_proposal' || origin.valid_to !== null) continue;
    const direct = observation.source_event_ids.length === 1 ? declaration(rows[0]!) : null;
    const portions = input.coverage?.filter((portion) => portion.rowId === sourceIds[0]) ?? [];
    const full = sourceIds.length === 1 && portions.length === 1 && portions[0].state === 'full'
      && portions[0].start === 0 && portions[0].end === portions[0].total;
    const processed = rows.length === 1 && rows[0]!.batch_id === input.batchId && db.prepare(`SELECT 1
      FROM observation_batch_sources WHERE batch_id = ? AND raw_event_id = ? AND outcome = 'processed'`)
      .get(input.batchId, sourceIds[0]) !== undefined;
    const automatic = full && processed && direct !== null && rows[0]!.processing_state === 'processed'
      && origin.provenance_complete === 1
      && observation.body.trim() === direct.body;
    const title = automatic ? direct.title : observation.title;
    const body = automatic ? direct.body : observation.body;
    if (body.trim() === '' || body.length > 2_000) continue;
    const material = materialHash(title, body);
    const id = `sp_${sha256Json(['sharing-proposal-v1', item.memoryId, sourceIds.sort(compareCodeUnits), title, body])}`;
    const sensitivity = strictest(origin.sensitivity as Sensitivity, ...rows.map((row) => row!.sensitivity));
    db.prepare(`INSERT OR IGNORE INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id,
      candidate_title, candidate_body, candidate_material_hash, candidate_sensitivity, source_event_ids_json,
      basis, state, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?)`)
      .run(id, item.memoryId, input.repoId, work.work_id, title, body, material, sensitivity,
        JSON.stringify(sourceIds), automatic ? 'direct_declaration' : 'inferred', input.now);
    const proposal = readProposal(db, input.repoId, id)!;
    if (automatic && proposal.state === 'pending' && proposal.candidate_title === title
      && proposal.candidate_body === body && proposal.candidate_material_hash === material) {
      approveProjection(db, proposal, sensitivity, 'automatic_direct', input.now);
    }
  }
}

function approvalLocation(location: PrivacyLocation, proposal: Proposal): PrivacyLocation {
  return { ...location, workId: proposal.origin_work_id };
}

/** Candidate and origin text share one privacy check; decisions additionally fence its snapshot. */
async function checkedProposal(db: DatabaseSync, location: PrivacyLocation, proposal: Proposal, detect: typeof detectSync) {
  if (proposal.candidate_body.length > 2_000 || proposal.candidate_title.length > 120
    || materialHash(proposal.candidate_title, proposal.candidate_body) !== proposal.candidate_material_hash) return null;
  const scope = memoryScope(db, { repoId: location.repoId, workId: proposal.origin_work_id, destination: 'injection' });
  const origin = getMemory(db, proposal.origin_memory_id, scope);
  if (origin === null || origin.type === 'session_summary' || origin.repo_id !== location.repoId) return null;
  const visible = await filterReadOutput(db, approvalLocation(location, proposal),
    [{ ...origin, candidate_title: proposal.candidate_title, candidate_body: proposal.candidate_body }], [], detect);
  return visible.memories.length === 0 ? null : origin;
}

export async function sharingStatus(db: DatabaseSync, location: PrivacyLocation) {
  const candidates = db.prepare(`SELECT * FROM sharing_proposals WHERE origin_repo_id = ?
    ORDER BY state = 'pending' DESC, created_at DESC, id LIMIT 51`)
    .all(location.repoId) as unknown as Proposal[];
  const proposals = [];
  const guards = [];
  for (const candidate of candidates.slice(0, 50)) {
    const sourceLocation = approvalLocation(location, candidate);
    const references = [{ memoryId: candidate.origin_memory_id, rawEventId: null }];
    const initial = injectionPrivacy(db, sourceLocation, references);
    if (initial !== null && await checkedProposal(db, location, candidate, detectSync) !== null) {
      proposals.push(candidate);
      guards.push({ candidate, sourceLocation, references, stamp: initial.stamp });
    }
  }
  if (guards.some(({ candidate, sourceLocation, references, stamp }) =>
    JSON.stringify(readProposal(db, location.repoId, candidate.id)) !== JSON.stringify(candidate)
    || injectionPrivacy(db, sourceLocation, references)?.stamp !== stamp)) return { proposals: [], hasMore: candidates.length > 50 };
  return { proposals, hasMore: candidates.length > 50 };
}

export async function decideSharing(db: DatabaseSync, location: PrivacyLocation,
  input: { id: string; decision: 'approve' | 'reject'; channel: 'cli' | 'viewer'; now: number },
  detect: typeof detectSync = detectSync): Promise<Decision | null> {
  const proposal = readProposal(db, location.repoId, input.id);
  if (proposal === undefined) return null;
  if (proposal.state !== 'pending') return proposal.state === (input.decision === 'approve' ? 'approved' : 'rejected')
    ? decisionOf(proposal) : null;
  if (input.decision === 'reject') return transactionImmediate(db, () => {
    const changed = db.prepare(`UPDATE sharing_proposals SET state = 'rejected', decision_channel = ?, decided_at = ?
      WHERE id = ? AND origin_repo_id = ? AND state = 'pending'`).run(input.channel, input.now, input.id, location.repoId);
    return Number(changed.changes) === 1 ? { id: input.id, state: 'rejected', projectedMemoryId: null } : null;
  });
  const references = [{ memoryId: proposal.origin_memory_id, rawEventId: null }];
  const sourceLocation = approvalLocation(location, proposal);
  const initial = injectionPrivacy(db, sourceLocation, references);
  if (initial === null) return null;
  const origin = await checkedProposal(db, location, proposal, detect);
  if (origin === null) return null;
  return transactionImmediate(db, () => {
    const current = readProposal(db, location.repoId, input.id);
    if (current === undefined || JSON.stringify(current) !== JSON.stringify(proposal)
      || injectionPrivacy(db, sourceLocation, references)?.stamp !== initial.stamp) return null;
    return approveProjection(db, current, strictest(origin.sensitivity, current.candidate_sensitivity), input.channel, input.now);
  });
}

export function knowledgeForAdoption(db: DatabaseSync, location: PrivacyLocation, id: string) {
  const workId = location.workId ?? db.prepare('SELECT work_id FROM work_bindings WHERE id = ?').get(location.bindingId)?.work_id;
  if (typeof workId !== 'string' || db.prepare("SELECT 1 FROM work_items WHERE id = ? AND repo_id = ? AND state = 'active'")
    .get(workId, location.repoId) === undefined) return null;
  const origin = getMemory(db, id, memoryScope(db, { repoId: location.repoId, workId, destination: 'injection' }));
  if (origin === null || origin.repo_id !== location.repoId || origin.type === 'session_summary'
    || db.prepare("SELECT 1 FROM memory_visibility WHERE memory_id = ? AND audience = 'work' AND repo_id = ? AND work_id = ?")
      .get(id, location.repoId, workId) === undefined) return null;
  return origin;
}

export async function adoptKnowledge(db: DatabaseSync, location: PrivacyLocation, id: string, now: number,
  detect: typeof detectSync = detectSync): Promise<boolean> {
  const origin = knowledgeForAdoption(db, location, id);
  if (origin === null) return false;
  const references = [{ memoryId: id, rawEventId: null }];
  const initial = injectionPrivacy(db, location, references);
  if (initial === null || (await filterReadOutput(db, location, [origin], [], detect)).memories.length === 0) return false;
  return transactionImmediate(db, () => {
    if (injectionPrivacy(db, location, references)?.stamp !== initial.stamp) return false;
    grantVisibility(db, id, { audience: 'project', repoId: location.repoId }, 'explicit_adoption', now);
    return true;
  });
}
