import type { DatabaseSync } from 'node:sqlite';
import { lstatSync } from 'node:fs';
import { isAbsolute, relative, resolve, sep } from 'node:path';

import { loadConfig, loadRepoRules, repoSecretPaths } from '../config.js';
import { memoryScope, memoryVisibility, type NearbyCandidate, type TimelineMemory, type TimelineSession } from '../db/queries.js';
import { contentHash } from '../events.js';
import { credentialValues } from '../log.js';
import { memoryContexts, sourceContext, type SourceContext } from './source-context.js';
import { oboetePaths, resolveHome } from '../paths.js';
import { resolveRepoIdentity, type RepoIdentity } from '../repo-identity.js';
import { readWorkSelection } from '../work.js';
import { isAllowed, loadDestinationRules, type Sensitivity } from './egress.js';
import { detectSync } from './detect.js';
import type { RawEventRow } from '../worker/batches.js';

export type PrivacyLocation = { repoId: string; bindingId: string | null; home?: string;
  repoRoot?: string; contextKey?: string | null; workId?: string | null; history?: boolean };
export type RootCache = Map<string, RepoIdentity | null>;
export type SourceReference = { memoryId: string | null; rawEventId: string | null };
export type InjectionPrivacyGuard = PrivacyLocation & { sources: SourceReference[]; stamp: string };

/** Plaintext never enters the guard; only the digest and the engine's source IDs are persisted. */
export function injectionPrivacy(db: DatabaseSync, location: PrivacyLocation, references: SourceReference[],
  env: NodeJS.ProcessEnv = process.env, remainingBudget?: () => number) {
  const roots: RootCache = new Map();
  const base = readSourcePrivacy(db, location, null, env, roots, remainingBudget);
  if (base === null) return null;
  const rules = loadDestinationRules(db);
  const selectedWork = location.workId ?? db.prepare('SELECT work_id FROM work_bindings WHERE id = ?').get(location.bindingId)?.work_id ?? null;
  const scope = memoryScope(db, { repoId: location.repoId, destination: 'injection',
    workId: typeof selectedWork === 'string' ? selectedWork : null, history: location.history });
  const sources = references.map((reference) => {
    const memory = reference.memoryId === null ? undefined : db.prepare(`SELECT m.id, m.repo_id, m.title, m.body,
      m.content_hash, m.sensitivity, m.review_state, m.deleted_at, m.valid_to, m.work_id, m.provenance_complete,
      m.source_session_id, m.source_batch_id, m.checkpoint_parent_id, m.citations_head
      FROM memories m WHERE m.id = ? AND ${scope.where}`).get(reference.memoryId, ...scope.params);
    const visibility = memory === undefined ? [] : memoryVisibility(db, String(memory.id));
    const personal = visibility.some((grant) => grant.audience === 'personal'
      && grant.state === 'approved' && grant.projected_memory_id === memory?.id);
    const raw = reference.rawEventId === null ? undefined : db.prepare(`SELECT r.id, r.repo_id, r.session_id, r.kind,
      r.content, r.payload_json, r.sensitivity, r.classification_state, r.work_binding_id, b.work_id AS source_work_id
      FROM raw_events r LEFT JOIN work_bindings b ON b.id = r.work_binding_id WHERE r.id = ? AND r.repo_id = ?`)
      .get(reference.rawEventId, location.repoId);
    let contexts: SourceContext[] | null = null;
    if (memory !== undefined && memory.deleted_at === null && (memory.valid_to === null || location.history === true) && memory.review_state !== 'imported'
      && isAllowed(rules, 'injection', memory.sensitivity as Sensitivity, true)) {
      contexts = personal ? null : memoryContexts(db, memory as unknown as NearbyCandidate);
      // A reviewed/user-owned legacy fact can have no source links at all. Its text still passes
      // current local policy; an incomplete linked source or checkpoint never uses this case.
      if (memory.work_id === null && contexts?.length === 1 && contexts[0].root === null
        && db.prepare('SELECT 1 FROM memory_sources WHERE memory_id = ? AND raw_event_id IS NOT NULL LIMIT 1').get(memory.id) === undefined) {
        const current = db.prepare(`SELECT c.id, c.root FROM work_bindings b JOIN work_contexts c ON c.id = b.context_id
          WHERE b.id = ? AND c.repo_id = ?`).get(location.bindingId, location.repoId);
        if (current !== undefined) contexts = [{ root: String(current.root), contextId: String(current.id), paths: contexts[0].paths }];
        else if (location.repoRoot !== undefined) contexts = [{ root: location.repoRoot, contextId: null, paths: contexts[0].paths }];
      }
    } else if (raw !== undefined && raw.classification_state === 'done'
      && isAllowed(rules, 'injection', raw.sensitivity as Sensitivity, true)) contexts = [sourceContext(db, raw as unknown as RawEventRow)];
    const sourceWork = memory?.work_id ?? raw?.source_work_id;
    const sourceLocation = sourceWork != null && selectedWork != null && sourceWork !== selectedWork
      ? { ...location, bindingId: null, workId: null } : location;
    const sourceFree = personal && memory !== undefined && [memory.work_id, memory.source_session_id,
      memory.source_batch_id, memory.checkpoint_parent_id, memory.citations_head].every((value) => value === null)
      && db.prepare('SELECT 1 FROM memory_sources WHERE memory_id = ? LIMIT 1').get(memory.id) === undefined;
    const policies = sourceFree ? [base] : contexts?.map((context) => readSourcePrivacy(db, sourceLocation,
      context, env, roots, remainingBudget, memory === undefined ? undefined : String(memory.id))) ?? null;
    return { reference, memory, raw, visibility, personal, policies };
  });
  return { base, sources, stamp: contentHash(JSON.stringify([base.stamp, sources])) };
}

type WorkLabel = { id: string; purpose: string | null };

/** Internal proof only: public work labels never expose their source event IDs. */
export function workPurposeSources(db: DatabaseSync, repoId: string, works: readonly WorkLabel[]) {
  if (works.length === 0) return [];
  const rows = new Map(db.prepare(`SELECT id, purpose, purpose_source_event_id FROM work_items
    WHERE repo_id = ? AND id IN (${works.map(() => '?').join(', ')})`).all(repoId, ...works.map((work) => work.id))
    .map((row) => [String(row.id), row]));
  return works.map((work) => {
    const stored = rows.get(work.id);
    const rawEventId = work.purpose !== null && stored?.purpose === work.purpose
      && typeof stored.purpose_source_event_id === 'string' && stored.purpose_source_event_id !== ''
      ? stored.purpose_source_event_id : null;
    return { id: work.id, rawEventId };
  });
}
const PERSONAL_OUTPUT_KEYS = ['id', 'type', 'title', 'body', 'sensitivity', 'review_state',
  'valid_from', 'valid_to', 'pinned_at', 'pin_order', 'created_at', 'last_injected_at', 'score', 'reasons'] as const;
type PersonalOutput<T extends { id: string }> = Pick<T, Extract<keyof T, typeof PERSONAL_OUTPUT_KEYS[number]>>;
export type ReadOutput<T extends { id: string }> = T | PersonalOutput<T>;
export type PublicTimelineSession = Omit<TimelineSession, 'memories'> & { memories: ReadOutput<TimelineMemory>[] };

/** A read view has one source/policy snapshot across both its memory bodies and work labels. */
export async function filterReadOutput<M extends { id: string }, W extends WorkLabel>(db: DatabaseSync,
  location: PrivacyLocation, memories: M[], works: W[], detect = detectSync): Promise<{ memories: ReadOutput<M>[]; works: W[] }> {
  const withheld = () => ({ memories: [] as M[], works: works.map((row) => ({ ...row, purpose: null })) });
  try {
    const references: SourceReference[] = memories.map((row) => ({ memoryId: row.id, rawEventId: null }));
    const purposes = workPurposeSources(db, location.repoId, works);
    const workIndices = purposes.map(({ rawEventId }) => {
      if (rawEventId === null) return null;
      references.push({ memoryId: null, rawEventId });
      return references.length - 1;
    });
    const initial = injectionPrivacy(db, location, references);
    if (initial === null) return withheld();
    const allowed = async (value: unknown, parts: ReturnType<typeof readSourcePrivacy>[] | null): Promise<boolean> => {
      if (parts === null || parts.length === 0) return false;
      const strings: string[] = [];
      const text = typeof value === 'string' ? value : JSON.stringify(value, (_key, field: unknown) => {
        if (typeof field === 'string') strings.push(field);
        return field;
      });
      for (const part of parts) {
        if (part === null) return false;
        const fields = [...strings, ...part.detector.paths];
        const checked = await detect({ ...part.detector, text, fields });
        if (!checked.ok || checked.sensitivity === 'secret' || checked.privateRemoved > 0 || checked.text !== text
          || checked.texts.length !== fields.length || checked.texts.some((field, index) => field !== fields[index])) return false;
      }
      return true;
    };
    const kept: ReadOutput<M>[] = [];
    for (const [index, row] of memories.entries()) {
      const output = initial.sources[index].personal ? personalOutput(row) : row;
      if (await allowed(output, initial.sources[index].policies)) kept.push(output);
    }
    const labels: W[] = [];
    for (const [index, row] of works.entries()) {
      const source = workIndices[index];
      labels.push(row.purpose === null || (source !== null && await allowed(row.purpose, initial.sources[source].policies))
        ? row : { ...row, purpose: null });
    }
    return JSON.stringify(workPurposeSources(db, location.repoId, works)) === JSON.stringify(purposes)
      && injectionPrivacy(db, location, references)?.stamp === initial.stamp ? { memories: kept, works: labels } : withheld();
  } catch { return withheld(); }
}

export async function filterMemoryOutput<T extends { id: string }>(db: DatabaseSync, location: PrivacyLocation, rows: T[]): Promise<ReadOutput<T>[]> {
  return (await filterReadOutput(db, location, rows, [])).memories;
}

/** A projection's public shape cannot acquire origin fields when a caller adds metadata. */
function personalOutput<T extends { id: string }>(row: T): PersonalOutput<T> {
  const fields = new Set<string>(PERSONAL_OUTPUT_KEYS);
  return Object.fromEntries(Object.entries(row).filter(([key]) => fields.has(key))) as PersonalOutput<T>;
}

/** Explicit local history can inspect retained state; personal rows still omit origin metadata. */
export function localHistoryOutput<T extends { id: string }>(db: DatabaseSync, rows: T[]): ReadOutput<T>[] {
  return rows.map((row) => memoryVisibility(db, row.id).some((grant) => grant.audience === 'personal') ? personalOutput(row) : row);
}

/** Session/turn references follow the filtered bodies, so omitted memories cannot leak via IDs. */
export async function filterTimelineOutput<W extends WorkLabel>(db: DatabaseSync, location: PrivacyLocation,
  sessions: TimelineSession[], works: W[] = []): Promise<{ sessions: PublicTimelineSession[]; works: W[] }> {
  const visible = await filterReadOutput(db, location, sessions.flatMap((session) => session.memories), works);
  const byId = new Map(visible.memories.map((memory) => [memory.id, memory]));
  const ids = new Set(byId.keys());
  return { works: visible.works, sessions: sessions.map((session) => ({ ...session,
    memories: session.memories.flatMap((memory) => byId.has(memory.id) ? [byId.get(memory.id)!] : []),
    memory_ids: session.memory_ids.filter((id) => ids.has(id)),
    turns: session.turns.map((turn) => ({ ...turn, memory_ids: turn.memory_ids.filter((id) => ids.has(id)) })),
  })) };
}

export function injectionPrivacyValid(db: DatabaseSync, guard: InjectionPrivacyGuard, remainingBudget?: () => number): boolean {
  try { return injectionPrivacy(db, guard, guard.sources, process.env, remainingBudget)?.stamp === guard.stamp; }
  catch { return false; }
}

function absent(root: string): boolean {
  try { lstatSync(root); return false; }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return true;
    throw error;
  }
}

/** Per-call caches expire before the next send; a reused path is never mistaken for a removal. */
export function readSourcePrivacy(db: DatabaseSync, location: PrivacyLocation, context: SourceContext | null,
  env: NodeJS.ProcessEnv = process.env, roots: RootCache = new Map(), remainingBudget?: () => number,
  projectMemoryId?: string) {
  const config = loadConfig(oboetePaths(location.home ?? resolveHome(env)));
  const rules = db.prepare('SELECT * FROM destination_rules ORDER BY destination, sensitivity').all();
  const detector = { repoRoot: null as string | null, paths: [] as string[],
    secretPaths: [...config.privacy.secret_paths], credentialValues: credentialValues(env) };
  const policyRoots = new Set<string>();
  let binding = location.bindingId === null ? undefined : db.prepare(`SELECT b.id, b.work_id, b.context_id,
    b.reason, b.closed_at, c.repo_id, c.local_key, c.root, c.repo_secret_paths_json,
    w.origin_context_id, w.state, w.current_checkpoint_memory_id FROM work_bindings b
    JOIN work_contexts c ON c.id = b.context_id JOIN sessions s ON s.id = b.session_id
    LEFT JOIN work_items w ON w.id = b.work_id WHERE b.id = ? AND s.repo_id = ? AND c.repo_id = ?`)
    .get(location.bindingId, location.repoId, location.repoId);
  if (location.bindingId === null && location.repoRoot !== undefined) {
    if (location.contextKey == null) return null;
    const current = db.prepare('SELECT id, repo_secret_paths_json FROM work_contexts WHERE repo_id = ? AND local_key = ?')
      .get(location.repoId, location.contextKey);
    const work = db.prepare('SELECT id, origin_context_id, state, current_checkpoint_memory_id FROM work_items WHERE id = ? AND repo_id = ?')
      .get(location.workId ?? null, location.repoId);
    binding = { id: null, context_id: current?.id ?? null, root: location.repoRoot, local_key: location.contextKey,
      repo_id: location.repoId, repo_secret_paths_json: current?.repo_secret_paths_json ?? null,
      work_id: work?.id ?? null, origin_context_id: work?.origin_context_id ?? null,
      state: work?.state ?? null, current_checkpoint_memory_id: work?.current_checkpoint_memory_id ?? null };
  }
  const verify = (root: string, key?: string): RepoIdentity | null => {
    if (!roots.has(root)) {
      if (remainingBudget !== undefined && remainingBudget() <= 0) return null;
      roots.set(root, absent(root) ? null : resolveRepoIdentity(root, { budgetMs: remainingBudget?.() }));
    }
    const identity = roots.get(root);
    return identity?.id === location.repoId && identity.worktreeKey !== null
      && (key === undefined || identity.worktreeKey === key) ? identity : null;
  };
  if (location.bindingId !== null && binding === undefined) return null;
  if (binding !== undefined) {
    const current = verify(String(binding.root), String(binding.local_key));
    if (current === null) return null;
    detector.repoRoot = current.root;
    policyRoots.add(current.root);
    detector.secretPaths.push(...loadRepoRules(current.root).secretPaths);
  }
  let origin: Record<string, unknown> | undefined;
  if (context !== null) {
    if (context.root === null) return null;
    origin = context.contextId === null ? undefined : db.prepare(`SELECT id, repo_id, local_key, root,
      repo_secret_paths_json FROM work_contexts WHERE id = ? AND repo_id = ?`).get(context.contextId, location.repoId);
    if (context.contextId !== null && origin === undefined) return null;
    let live = verify(context.root, typeof origin?.local_key === 'string' ? origin.local_key : undefined);
    // Git worktree move preserves the administration directory's generation. Only a missing old
    // path may follow that same generation to its verified current root; a replaced path cannot.
    if (live === null && absent(context.root) && typeof origin?.root === 'string' && typeof origin.local_key === 'string') {
      live = verify(origin.root, origin.local_key);
    }
    if (live !== null) {
      detector.repoRoot = live.root;
      policyRoots.add(live.root);
      detector.secretPaths.push(...loadRepoRules(live.root).secretPaths);
    } else {
      const projectGrant = projectMemoryId !== undefined && db.prepare(`SELECT 1 FROM memory_visibility v
        JOIN memories m ON m.id = v.memory_id WHERE v.memory_id = ? AND v.audience = 'project'
        AND v.repo_id = ? AND m.repo_id = v.repo_id`).get(projectMemoryId, location.repoId) !== undefined;
      if (!absent(context.root) || origin?.root !== context.root || binding === undefined
        || (binding.work_id === null && !projectGrant)) return null;
      const retained = typeof origin.repo_secret_paths_json === 'string'
        ? repoSecretPaths(JSON.parse(origin.repo_secret_paths_json)) : null;
      if (retained === null) return null;
      const resumed = verify(String(binding.root), String(binding.local_key));
      const selected = projectGrant || binding.origin_context_id === binding.context_id || db.prepare(`SELECT 1 FROM work_bindings
        WHERE work_id = ? AND context_id = ? AND reason = 'explicit' LIMIT 1`).get(binding.work_id, binding.context_id) !== undefined;
      if (resumed === null || !selected) return null;
      detector.repoRoot = resumed.root;
      policyRoots.add(resumed.root);
      detector.secretPaths.push(...retained, ...loadRepoRules(resumed.root).secretPaths);
    }
    if (context.paths === null && detector.secretPaths.length > 0) return null;
    for (const path of context.paths ?? []) {
      detector.paths.push(path);
      const inside = relative(resolve(context.root), resolve(context.root, path));
      if (inside !== '..' && !inside.startsWith(`..${sep}`) && !isAbsolute(inside)) {
        detector.paths.push(inside);
        for (const root of policyRoots) detector.paths.push(resolve(root, inside));
      }
    }
  }
  detector.secretPaths = [...new Set(detector.secretPaths)].sort();
  detector.paths = [...new Set(detector.paths)].sort();
  const selection = location.bindingId === null && location.repoRoot !== undefined
    ? readWorkSelection(db, { repoId: location.repoId, contextKey: location.contextKey ?? null }) : null;
  return { detector, stamp: contentHash(JSON.stringify([detector, rules, context, origin, binding, selection])) };
}

export { memoryContexts, sourceContext };
export type { SourceContext };
