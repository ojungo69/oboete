// Plaintext line schemas and bounds for `oboete-sync-snapshot/1` (contracts/sync.md). Payload
// field validation reuses the native v2 record schemas; the reference rules are sync's own and
// live in apply.ts. Security-owned: every line a reader parses passes these before anything else.
import { z } from 'zod';

import {
  nativeContextSchema, nativeMemorySchema, nativeProposalSchema, nativeSourceSchema, nativeVisibilitySchema,
  nativeWorkSchema,
} from '../transfer-format.js';
import { SNAPSHOT_FORMAT, SYNC_KINDS, type SyncKind } from './identity.js';

export const BOUNDS = {
  headerBytes: 65_536,
  lineBytes: 4 * 1024 * 1024,
  revisionLines: 1_000_000,
  parentsPerRevision: 64,
  headsPerOrigin: 64,
  revisionsPerOrigin: 4_096,
  replicasPerSpace: 32,
} as const;

export const SENSITIVITIES = ['eligible', 'local_only', 'private', 'secret'] as const;
const hex32 = z.string().regex(/^[0-9a-f]{32}$/u);
const hex64 = z.string().regex(/^[0-9a-f]{64}$/u);
/** `<creating replica origin_id>:<local identifier>`; the local part is opaque but bounded. */
export const originIdSchema = z.string().regex(/^[0-9a-f]{32}:[!-~]{1,479}$/u);
export const repoKeySchema = z.string().regex(/^(remote:[0-9a-f]{64}|[0-9a-f]{32}:common_dir:[0-9a-f]{64})$/u);

export const controlSchema = z.strictObject({
  tombstone: z.boolean(),
  sensitivity_floor: z.enum(SENSITIVITIES),
});
export type Control = z.infer<typeof controlSchema>;

export const naturalSchemas = {
  memory: z.discriminatedUnion('domain', [
    z.strictObject({ domain: z.literal('ordinary'), repo: repoKeySchema, material_hash: hex64 }),
    z.strictObject({ domain: z.literal('personal_projection'), projection_hash: hex64 }),
    z.strictObject({
      domain: z.literal('checkpoint'), repo: repoKeySchema, work: originIdSchema,
      parent: originIdSchema.nullable(), material_hash: hex64,
    }),
  ]),
  source: z.strictObject({ memory: originIdSchema, source_hash: hex64 }),
  visibility: z.strictObject({
    memory: originIdSchema, audience: z.enum(['work', 'project', 'personal']),
    repo: repoKeySchema.nullable(), work: originIdSchema.nullable(),
  }),
  sharing_proposal: z.strictObject({ candidate: hex64, origin_memory: originIdSchema }),
  context: z.strictObject({ repo: repoKeySchema, local_key: z.string().min(1).max(512) }),
  work: z.strictObject({ work: originIdSchema }),
} as const;
export type Natural = { [K in SyncKind]: z.infer<(typeof naturalSchemas)[K]> };

export const headerSchema = z.strictObject({
  format: z.literal(SNAPSHOT_FORMAT),
  space_id: hex32,
  replica_origin_id: hex32,
  snapshot_id: hex64,
  revision_lines: z.number().int().min(0).max(BOUNDS.revisionLines),
  heads: z.number().int().min(0).max(BOUNDS.revisionLines),
  revisions_sha256: hex64,
  withheld: z.strictObject({
    works: z.number().int().min(0), memories: z.number().int().min(0), sources: z.number().int().min(0),
    contexts: z.number().int().min(0), proposals: z.number().int().min(0),
  }),
  produced_at: z.number().int().min(0),
});
export type Header = z.infer<typeof headerSchema>;

/** Identity fields plus the two delivery fields; `natural` and `payload` are checked per kind. */
export const revisionLineSchema = z.strictObject({
  origin_id: originIdSchema,
  kind: z.enum(SYNC_KINDS),
  revision_id: hex64,
  author: hex32,
  parents: z.array(hex64).max(BOUNDS.parentsPerRevision),
  control: controlSchema,
  natural: z.record(z.string(), z.unknown()),
  payload_hash: hex64.nullable(),
  head: z.boolean(),
  payload: z.record(z.string(), z.unknown()).nullable(),
}).refine((line) => line.payload === null || line.payload_hash !== null, { message: 'payload_without_hash' })
  .refine((line) => new Set(line.parents).size === line.parents.length, { message: 'duplicate_parent' })
  .refine((line) => !line.parents.includes(line.revision_id), { message: 'self_parent' });
export type RevisionLine = z.infer<typeof revisionLineSchema>;

export const repoLineSchema = z.strictObject({
  kind: z.literal('repo'),
  origin_id: repoKeySchema,
  identity_kind: z.enum(['remote', 'common_dir']),
  normalized_identity: z.string().min(1).max(16_384),
});
export type RepoLine = z.infer<typeof repoLineSchema>;

/** Payload schemas in origin form: every entity reference field holds an origin id or repo key. */
export const payloadSchemas: Record<SyncKind, z.ZodType> = {
  memory: nativeMemorySchema,
  source: nativeSourceSchema,
  visibility: nativeVisibilitySchema,
  sharing_proposal: nativeProposalSchema,
  context: nativeContextSchema,
  work: nativeWorkSchema,
};

/**
 * Entity references per kind (contracts/sync.md "Payload references"): origin ids or repo keys
 * that must resolve. One list drives origin conversion on push, the reference closure and the
 * ownership check on apply. Everything else in a payload is metadata kept verbatim.
 */
export const ENTITY_REFERENCES: Record<SyncKind, readonly { field: string; kind: SyncKind | 'repo' }[]> = {
  memory: [
    { field: 'repo_id', kind: 'repo' }, { field: 'work_id', kind: 'work' },
    { field: 'checkpoint_parent_id', kind: 'memory' }, { field: 'superseded_by', kind: 'memory' },
  ],
  source: [
    { field: 'memory_id', kind: 'memory' }, { field: 'source_memory_id', kind: 'memory' },
    { field: 'source_context_id', kind: 'context' },
  ],
  visibility: [
    { field: 'memory_id', kind: 'memory' }, { field: 'repo_id', kind: 'repo' },
    { field: 'work_id', kind: 'work' }, { field: 'proposal_id', kind: 'sharing_proposal' },
  ],
  sharing_proposal: [
    { field: 'origin_memory_id', kind: 'memory' }, { field: 'origin_repo_id', kind: 'repo' },
    { field: 'origin_work_id', kind: 'work' }, { field: 'projected_memory_id', kind: 'memory' },
  ],
  context: [{ field: 'repo_id', kind: 'repo' }],
  work: [
    { field: 'repo_id', kind: 'repo' }, { field: 'origin_context_id', kind: 'context' },
    { field: 'current_checkpoint_memory_id', kind: 'memory' },
  ],
};
