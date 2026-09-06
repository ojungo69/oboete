// Hook path: no heavy import. pack.ts and capture.ts both read the markers from here so the
// capture hook does not have to load the pack builder to recognize what it printed.
import { createHash } from 'node:crypto';
import type { DatabaseSync } from 'node:sqlite';

export const PACK_HEADER = 'oboete memory context';
export const PACK_FOOTER = 'end of oboete memory context';

/** `injections.pack_hash` (contracts/agents.md): the sha256 of the exact pack text. */
export function packHash(text: string): string {
  return createHash('sha256').update(text, 'utf8').digest('hex');
}

/**
 * FR-021: a pack that comes back through capture is recognized by its hash and removed from the
 * stored content, so it is never summarized as new activity. Only a whole pack, from a line that
 * starts with the header to the end of a line that starts with the footer, whose hash is in
 * `injections.pack_hash` is removed; a text that merely looks like a pack stays ordinary content
 * (the safe direction: a pack this database never issued is somebody's content, not oboete's).
 */
export function stripRecognizedPacks(
  db: DatabaseSync,
  text: string,
): { text: string; hashes: string[] } {
  if (!text.includes(PACK_HEADER)) return { text, hashes: [] };
  // ponytail: full scan of injections per candidate span, reached only when a text carries the
  // header; add an index on pack_hash when the table outgrows a few thousand rows.
  const known = db.prepare('SELECT 1 AS found FROM injections WHERE pack_hash = ? LIMIT 1');
  const issued = (span: string): string | null => {
    const hash = packHash(span);
    return known.get(hash) === undefined ? null : hash;
  };
  const hashes: string[] = [];
  let rest = text;
  let from = 0;
  for (;;) {
    const start = lineStart(rest, PACK_HEADER, from);
    if (start === -1) break;
    const match = issuedSpan(rest, start, issued);
    if (match === null) {
      from = start + PACK_HEADER.length;
      continue;
    }
    hashes.push(match.hash);
    rest = rest.slice(0, start) + rest.slice(match.end);
    from = start;
  }
  return { text: rest, hashes };
}

/** The next `marker` that begins a line, at or after `from`; -1 when there is none. */
function lineStart(text: string, marker: string, from: number): number {
  let at = text.indexOf(marker, from);
  while (at > 0 && text[at - 1] !== '\n') at = text.indexOf(marker, at + marker.length);
  return at;
}

/**
 * The first footer line after `start` that closes an issued pack. A memory about oboete itself can
 * quote the footer inside a pack line, so every footer is tried until the hash matches.
 */
function issuedSpan(
  text: string,
  start: number,
  issued: (span: string) => string | null,
): { end: number; hash: string } | null {
  let footer = lineStart(text, PACK_FOOTER, start + PACK_HEADER.length);
  while (footer !== -1) {
    const end = footer + PACK_FOOTER.length;
    const hash = issued(text.slice(start, end));
    if (hash !== null) return { end, hash };
    footer = lineStart(text, PACK_FOOTER, end);
  }
  return null;
}
