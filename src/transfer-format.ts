import { z } from 'zod';
import { repoSecretPaths } from './config.js';

export const EXPORT_FORMAT = 'oboete-export/1';
export const MAX_LINE_BYTES = 64 * 1024;
export const MAX_FILE_BYTES = 256 * 1024 * 1024;
/** data-model "memories": the stricter class wins on every merge. */
export const SENSITIVITY_RANK = { eligible: 0, local_only: 1, private: 2, secret: 3 } as const;
export type Sensitivity = keyof typeof SENSITIVITY_RANK;

const hash64 = z.string().regex(/^[0-9a-f]{64}$/u);
const timestamp = z.number().int().nonnegative();

export const headerSchema = z.looseObject({
  format: z.literal(EXPORT_FORMAT),
  exported_at: timestamp.optional(),
  repos: z.array(
    z.looseObject({
      id: z.string().min(1),
      identity_kind: z.enum(['remote', 'common_dir']),
      normalized_identity: z.string().min(1),
    }),
  ),
});

const sourceSchema = z.looseObject({
  citation_kind: z.enum(['file_read', 'file_modified', 'commit']).nullable(),
  citation_value: z.string().nullable(),
  source_agent: z.string().nullable(),
});

export const lineSchema = z.looseObject({
  id: z.string().min(1),
  repo_id: z.string().min(1),
  type: z.enum([
    'bugfix', 'feature', 'refactor', 'change', 'discovery', 'decision',
    'security_alert', 'security_note', 'session_summary',
  ]),
  title: z.string().nullable(),
  body: z.string().nullable(),
  concepts: z.string().nullable(),
  material_hash: hash64,
  content_hash: hash64,
  sensitivity: z.enum(['eligible', 'local_only', 'private', 'secret']),
  review_state: z.enum(['unreviewed', 'reviewed', 'imported']),
  degraded_reason: z.string().nullable(),
  source_session_id: z.string().nullable(),
  source_batch_id: z.string().nullable(),
  source_agent: z.string().nullable(),
  valid_from: timestamp.nullable(),
  valid_to: timestamp.nullable(),
  superseded_by: z.string().nullable(),
  pinned_at: timestamp.nullable(),
  pin_order: z.number().int().nullable(),
  deleted_at: timestamp.nullable(),
  created_at: timestamp.nullable(),
  sources: z.array(sourceSchema),
});
export type ExportLine = z.infer<typeof lineSchema>;

export type ImportResult = {
  applied: boolean; inserted: number; updated: number; tombstones: number; unchanged: number;
  rejected: { line: number; reason: string }[];
  duplicate?: boolean;
};
export type ImportOptions = {
  now: number; dryRun?: boolean; mapRepo?: Record<string, string>; mapWork?: Record<string, string>;
  mapContext?: Record<string, string>; mapProject?: Record<string, string>; mapProjectHash?: Record<string, string>;
  from?: 'claude-mem'; maxFileBytes?: number;
};

export const NATIVE_FORMAT = 'oboete-export/2';
export const MAX_NATIVE_LINE_BYTES = 4 * 1024 * 1024;
export const NATIVE_REVISION = 'memory-provenance-visibility-sharing/1';
export const CLAUDE_MEM_FORMAT = 'claude-mem-query-export';
export const CLAUDE_MEM_REVISION = '8bc631a71a487424b866756e43a6efa4574cc66b';
const originId = z.string().min(1).max(512);
const nullableId = originId.nullable();
const boundedText = z.string().max(2 * 1024 * 1024).nullable();
const nullableTime = timestamp.nullable();
const scopeKind = z.enum(['migration', 'observer', 'explicit_adoption', 'proposal_approval']);

export const nativeHeaderSchema = z.object({
  format: z.literal(NATIVE_FORMAT), revision: z.literal(NATIVE_REVISION), exported_at: timestamp,
  origin_id: z.string().regex(/^[0-9a-f]{32}$/u),
});

export const nativeRepoSchema = z.object({
  kind: z.literal('repo'), id: originId, identity_kind: z.enum(['remote', 'common_dir']),
  normalized_identity: z.string().min(1).max(16_384),
});

export const nativeMemorySchema = z.object({
  ...lineSchema.omit({ sources: true }).shape,
  kind: z.literal('memory'), id: originId, repo_id: originId,
  title: z.string().max(65_536).nullable(), body: z.string().max(65_536).nullable(), concepts: z.string().max(65_536).nullable(),
  identity_domain: z.enum(['ordinary', 'personal_projection']),
  work_id: nullableId, checkpoint_parent_id: nullableId,
  provenance_complete: z.union([z.literal(0), z.literal(1)]).nullable(),
  source_captured_at: nullableTime,
}).refine((row) => {
  if (row.concepts === null) return true;
  try {
    const values: unknown = JSON.parse(row.concepts);
    return Array.isArray(values) && values.length <= 10_000 && values.every((value) => typeof value === 'string');
  } catch { return false; }
}, { message: 'invalid_memory_concepts' });

export const nativeSourceSchema = z.object({
  kind: z.literal('source'), id: originId, memory_id: originId, raw_event_id: nullableId,
  source_memory_id: nullableId, source_context_id: nullableId,
  citation_kind: z.enum(['file_read', 'file_modified', 'commit']).nullable(),
  citation_value: boundedText, source_agent: boundedText,
  portion_start: nullableTime, portion_end: nullableTime, source_total: nullableTime,
  source_hash: hash64.nullable(), evidence: boundedText, captured_at: nullableTime,
  source_processed_at: nullableTime, capture_root: boundedText, source_paths_json: boundedText,
  context_only: z.union([z.literal(0), z.literal(1)]),
}).superRefine((source, context) => {
  const range = [source.portion_start, source.portion_end, source.source_total];
  if (range.some((part) => part !== null) && (range.some((part) => part === null)
    || source.portion_start! > source.portion_end! || source.portion_end! > source.source_total!)) {
    context.addIssue({ code: 'custom', message: 'invalid_source_range' });
  }
  if (source.source_paths_json !== null) {
    try {
      const paths: unknown = JSON.parse(source.source_paths_json);
      if (!Array.isArray(paths) || paths.length > 10_000 || !paths.every((path) => typeof path === 'string')) {
        context.addIssue({ code: 'custom', message: 'invalid_source_paths' });
      }
    } catch { context.addIssue({ code: 'custom', message: 'invalid_source_paths' }); }
  }
});

export const nativeVisibilitySchema = z.object({
  kind: z.literal('visibility'), id: originId, memory_id: originId,
  audience: z.enum(['work', 'project', 'personal']), repo_id: nullableId, work_id: nullableId,
  proposal_id: nullableId, grant_kind: scopeKind, created_at: timestamp,
}).refine((grant) => grant.audience === 'work'
  ? grant.repo_id !== null && grant.work_id !== null && grant.proposal_id === null
  : grant.audience === 'project' ? grant.repo_id !== null && grant.work_id === null && grant.proposal_id === null
    : grant.repo_id === null && grant.work_id === null && grant.proposal_id !== null,
{ message: 'invalid_visibility_shape' });

export const nativeProposalSchema = z.object({
  kind: z.literal('sharing_proposal'), id: originId, origin_memory_id: originId,
  origin_repo_id: originId, origin_work_id: originId,
  candidate_title: z.string().max(120), candidate_body: z.string().max(2000),
  candidate_material_hash: hash64, candidate_sensitivity: z.enum(['eligible', 'local_only', 'private', 'secret']),
  source_event_ids_json: z.string().max(32_768), basis: z.enum(['direct_declaration', 'inferred']),
  state: z.enum(['pending', 'approved', 'rejected']),
  decision_channel: z.enum(['automatic_direct', 'cli', 'viewer']).nullable(),
  projected_memory_id: nullableId, created_at: timestamp, decided_at: nullableTime,
  redacted: z.boolean(),
}).superRefine((proposal, context) => {
  const valid = proposal.state === 'pending'
    ? proposal.decision_channel === null && proposal.projected_memory_id === null && proposal.decided_at === null
    : proposal.decision_channel !== null && proposal.decided_at !== null
      && (proposal.state === 'approved' ? proposal.projected_memory_id !== null : proposal.projected_memory_id === null);
  if (!valid) context.addIssue({ code: 'custom', message: 'invalid_proposal_state' });
  if (proposal.candidate_sensitivity === 'secret' && !proposal.redacted) {
    context.addIssue({ code: 'custom', message: 'secret_proposal_text' });
  }
  try {
    const ids: unknown = JSON.parse(proposal.source_event_ids_json);
    if (!Array.isArray(ids) || ids.length > 50 || !ids.every((id) => typeof id === 'string' && id.length <= 512)
      || new Set(ids).size !== ids.length) context.addIssue({ code: 'custom', message: 'invalid_proposal_sources' });
  } catch { context.addIssue({ code: 'custom', message: 'invalid_proposal_sources' }); }
  if (proposal.redacted && (proposal.candidate_title !== '' || proposal.candidate_body !== ''
    || proposal.source_event_ids_json !== '[]')) context.addIssue({ code: 'custom', message: 'redacted_proposal_text' });
});

export const nativeContextSchema = z.object({
  kind: z.literal('context'), id: originId, repo_id: originId, local_key: originId,
  root: z.string().max(16_384).nullable(), repo_secret_paths_json: z.string().max(32_768).nullable(),
  created_at: timestamp, last_seen_at: timestamp, redacted: z.boolean(),
}).refine((row) => !row.redacted || (row.root === null && row.repo_secret_paths_json === null), { message: 'redacted_context_text' })
  .refine((row) => {
    if (row.repo_secret_paths_json === null) return true;
    try { return repoSecretPaths(JSON.parse(row.repo_secret_paths_json)) !== null; } catch { return false; }
  }, { message: 'invalid_context_rules' });

export const nativeWorkSchema = z.object({
  kind: z.literal('work'), id: originId, repo_id: originId, origin_context_id: originId,
  purpose: z.string().max(300).nullable(), purpose_source_event_id: nullableId,
  purpose_sensitivity: z.enum(['eligible', 'local_only', 'private', 'secret']),
  state: z.enum(['active', 'dormant', 'completed']), created_at: timestamp, updated_at: timestamp,
  completed_at: nullableTime, current_checkpoint_memory_id: nullableId, redacted: z.boolean(),
}).refine((row) => (!row.redacted && row.purpose_sensitivity !== 'secret') || row.purpose === null,
{ message: 'redacted_work_purpose' });

export const nativeOriginSchema = z.object({
  kind: z.literal('migration_origin'), id: hash64, repo_id: nullableId, memory_id: nullableId,
  origin_json: z.string().max(4096), payload_hash: hash64, stored_payload_hash: hash64.nullable(),
  record_kind: z.enum(['memory', 'source', 'visibility', 'sharing_proposal', 'work', 'context', 'session', 'prompt', 'excluded']),
  payload: z.record(z.string(), z.unknown()).nullable(),
  classification_state: z.enum(['pending', 'clean', 'secret', 'not_applicable']),
}).refine((row) => (row.payload === null) === (row.stored_payload_hash === null)
  && (!['secret', 'not_applicable'].includes(row.classification_state) || row.payload === null), { message: 'invalid_origin_payload' });

export const nativeRecordSchema = z.discriminatedUnion('kind', [
  nativeRepoSchema, nativeMemorySchema, nativeSourceSchema, nativeVisibilitySchema, nativeProposalSchema,
  nativeContextSchema, nativeWorkSchema, nativeOriginSchema,
]);
export type NativeRecord = z.infer<typeof nativeRecordSchema>;
export type NativeMemory = z.infer<typeof nativeMemorySchema>;
export type NativeSource = z.infer<typeof nativeSourceSchema>;
export type NativeProposal = z.infer<typeof nativeProposalSchema>;

/** A foreign terminal label can only withhold its own payload, never authorize local content. */
export function migrationPayloadRedaction(value: unknown): 'secret' | 'deleted' | null {
  if (value === null || typeof value !== 'object') return null;
  const row = value as Record<string, unknown>;
  if ((row.kind === 'memory' && row.sensitivity === 'secret')
    || (row.kind === 'sharing_proposal' && row.candidate_sensitivity === 'secret')) return 'secret';
  if ((row.kind === 'memory' && row.deleted_at !== null && row.deleted_at !== undefined)
    || (row.kind === 'sharing_proposal' && row.redacted === true)) return 'deleted';
  return null;
}

export function migrationPayloadShape(value: unknown): boolean {
  if (value === null || typeof value !== 'object' || !('kind' in value)) return false;
  // External sessions/prompts are private supporting records, with no promotion/read authority.
  if (value.kind === 'session' || value.kind === 'prompt' || value.kind === 'excluded') {
    const record = value as Record<string, unknown>;
    return typeof record.id === 'string' && /^[0-9a-f]{64}$/u.test(record.id)
      && typeof record.external_payload === 'object' && record.external_payload !== null
      && !Array.isArray(record.external_payload);
  }
  return value.kind !== 'migration_origin' && nativeRecordSchema.safeParse(value).success;
}
