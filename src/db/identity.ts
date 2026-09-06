import { sha256Json } from '../hash.js';

/**
 * The normalized title and the normalized body of amendment A13: Unicode NFKC, whitespace runs
 * (newlines included) collapsed to one space, trimmed, lowercased. Two texts that differ only in
 * spacing, casing or Unicode form are the same content (FR-035).
 */
export function normalizeForIdentity(text: string): string {
  return text.normalize('NFKC').replace(/\s+/gu, ' ').trim().toLowerCase();
}

/**
 * Repository-independent content identity, kept on tombstones (data-model "memories").
 * The observation type is deliberately not an input (A13, FR-035): a deleted memory must not
 * return under another type.
 */
export function materialHash(title: string, body: string): string {
  return sha256Json([normalizeForIdentity(title), normalizeForIdentity(body)]);
}

/** The identity of the same content inside one repository (FR-044 keeps that identity per repo). */
export function contentHash(repoId: string, material: string): string {
  return sha256Json([repoId, material]);
}

/** `memories.id` = `'m_' + content_hash.slice(0, 24)` (docs/dev/conventions.md). */
export function memoryIdFor(hash: string): string {
  return `m_${hash.slice(0, 24)}`;
}
