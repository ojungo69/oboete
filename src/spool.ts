// The overflow spool: one sanitized file per row the hook could not store, written to a temporary
// name and renamed. Sources: research.md R6 ("Spool entries are one sanitized file per event
// (write-then-rename)"), data-model.md "Spool entry", spec FR-002 and FR-003.
import { randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { z } from 'zod';

import { AGENTS, EVENT_KINDS } from './events.js';
import type { OboetePaths } from './paths.js';

const identifier = z.string().min(1);
const timestamp = z.int();

/**
 * One spool file: the `raw_events` row capture could not write plus the parent rows it would have
 * upserted. The parents travel with the row because a hook that could not write `raw_events` could
 * not write `repos`, `sessions` or `turns` either, and the foreign keys refuse the row without
 * them. A spool file is read back by the worker, so it is validated like any other input; a file
 * that does not match is quarantined rather than trusted into the database (R4 fails closed on
 * what it cannot classify).
 */
export const spoolEntrySchema = z.strictObject({
  repo: z.strictObject({
    id: identifier,
    identity_kind: z.enum(['remote', 'common_dir']),
    normalized_identity: identifier,
    display_root: z.string().nullable().default(null),
  }),
  session: z.strictObject({
    id: identifier,
    repo_id: identifier,
    agent: z.enum(AGENTS),
    native_session_id: identifier,
    conversation_id: identifier,
    model: z.string().nullable().default(null),
    started_at: timestamp.nullable().default(null),
    status: z.enum(['active', 'ended']),
  }),
  row: z.strictObject({
    id: identifier,
    repo_id: identifier,
    session_id: identifier,
    turn_id: z.string().nullable().default(null),
    agent: z.enum(AGENTS),
    kind: z.enum(EVENT_KINDS),
    content: z.string().nullable().default(null),
    truncated: z.int().nullable().default(null),
    payload_json: z.string().nullable().default(null),
    content_hash: z.string().nullable().default(null),
    sensitivity: z.enum(['local_only', 'eligible', 'secret', 'private']),
    classification_state: z.enum(['pending', 'done', 'partial', 'failed']),
    captured_at: timestamp,
    expires_at: timestamp,
  }),
});

export type SpoolEntry = z.infer<typeof spoolEntrySchema>;

/**
 * Writes one already redacted entry to `spool/<captured_at>-<row id>.json`. The detector has run
 * before this call on every path that reaches it (FR-018), and an event whose detector run failed
 * keeps metadata only, so a spool file never holds unsanitized content.
 */
export function writeSpoolEntry(paths: OboetePaths, entry: SpoolEntry): void {
  const target = join(paths.spool, `${entry.row.captured_at}-${entry.row.id}.json`);
  // The temporary name carries a fresh uuid so two hooks writing the same event cannot interleave,
  // and it is not `.json`, so a half-written file is never listed as an entry.
  const temporary = `${target}.${randomUUID()}.tmp`;
  writeFileSync(temporary, JSON.stringify(entry), { mode: 0o600 });
  renameSync(temporary, target);
}

/** The entries in name order, which is capture order because the name starts with `captured_at`. */
export function listSpool(paths: OboetePaths): string[] {
  if (!existsSync(paths.spool)) return [];
  return readdirSync(paths.spool)
    .filter((name) => name.endsWith('.json'))
    .sort();
}

/** One entry, or null when it is gone or does not match the schema. */
export function readSpoolEntry(paths: OboetePaths, name: string): SpoolEntry | null {
  let raw: unknown;
  try {
    raw = JSON.parse(readFileSync(join(paths.spool, name), 'utf8'));
  } catch {
    return null;
  }
  const parsed = spoolEntrySchema.safeParse(raw);
  return parsed.success ? parsed.data : null;
}

/** Removing an entry that is already gone is not an error: recovery must be idempotent (FR-003). */
export function removeSpoolEntry(paths: OboetePaths, name: string): void {
  rmSync(join(paths.spool, name), { force: true });
}

/** A file the worker could not read is moved aside instead of being read into the database (R6). */
export function quarantineSpoolEntry(paths: OboetePaths, name: string): void {
  mkdirSync(paths.spoolFailed, { recursive: true, mode: 0o700 });
  renameSync(join(paths.spool, name), join(paths.spoolFailed, name));
}
