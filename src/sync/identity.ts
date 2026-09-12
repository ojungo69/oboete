// Sync identity (contracts/sync.md "Plaintext: header and revision log"). Every replica must
// compute these ids identically, so all hashing goes through one canonical JSON: members sorted
// by code unit at every depth, arrays in order, no whitespace. Security-owned with the rest of
// sync: a reader recomputes every id and never trusts one it was sent.
import { compareCodeUnits, sha256Hex } from '../hash.js';

export const REVISION_ID_DOMAIN = 'oboete-record-revision/1';
export const SNAPSHOT_FORMAT = 'oboete-sync-snapshot/1';
export const REVISION_FORMAT = 'oboete-sync-revision/1';
export const SYNC_KINDS = ['memory', 'source', 'visibility', 'sharing_proposal', 'work', 'context'] as const;
export type SyncKind = (typeof SYNC_KINDS)[number];
export type Sensitivity = 'eligible' | 'local_only' | 'private' | 'secret';
export type Control = { tombstone: boolean; sensitivity_floor: Sensitivity };

export type RevisionIdentity = {
  origin_id: string;
  kind: SyncKind;
  author: string;
  parents: readonly string[];
  control: Control;
  natural: Record<string, unknown>;
  payload_hash: string | null;
};

export function canonicalJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  if (value !== null && typeof value === 'object') {
    const members = Object.entries(value as Record<string, unknown>)
      .filter(([, member]) => member !== undefined)
      .sort(([left], [right]) => compareCodeUnits(left, right))
      .map(([key, member]) => `${JSON.stringify(key)}:${canonicalJson(member)}`);
    return `{${members.join(',')}}`;
  }
  return JSON.stringify(value);
}

export function payloadHash(payload: unknown): string {
  return sha256Hex(canonicalJson(payload));
}

export function revisionId(identity: RevisionIdentity): string {
  return sha256Hex(canonicalJson([
    REVISION_ID_DOMAIN, identity.origin_id, identity.kind, identity.author,
    [...identity.parents].sort(compareCodeUnits), identity.control, identity.natural, identity.payload_hash,
  ]));
}

export function snapshotId(spaceId: string, replicaOriginId: string, revisionsSha256: string): string {
  return sha256Hex(canonicalJson([SNAPSHOT_FORMAT, spaceId, replicaOriginId, revisionsSha256]));
}
