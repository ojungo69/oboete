// The one sha256 helper. docs/dev/conventions.md "Identifiers, hashes, time": sha256 over UTF-8
// with hex output, and a composite key hashed as `JSON.stringify([...parts])` so that no separator
// inside a part can collide with the separator between parts.
import { createHash } from 'node:crypto';

/**
 * The default sort order for strings (UTF-16 code units), spelled out. Deterministic across
 * locales, which localeCompare is not; used wherever the order feeds a hash or a file sequence.
 */
export function compareCodeUnits(left: string, right: string): number {
  if (left < right) return -1;
  return left > right ? 1 : 0;
}

export function sha256Hex(text: string): string {
  return createHash('sha256').update(text, 'utf8').digest('hex');
}

export function sha256Json(parts: unknown[]): string {
  return sha256Hex(JSON.stringify(parts));
}
