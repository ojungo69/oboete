// The one sha256 helper. docs/dev/conventions.md "Identifiers, hashes, time": sha256 over UTF-8
// with hex output, and a composite key hashed as `JSON.stringify([...parts])` so that no separator
// inside a part can collide with the separator between parts.
import { createHash } from 'node:crypto';

export function sha256Hex(text: string): string {
  return createHash('sha256').update(text, 'utf8').digest('hex');
}

export function sha256Json(parts: unknown[]): string {
  return sha256Hex(JSON.stringify(parts));
}
