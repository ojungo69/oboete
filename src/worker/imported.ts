import type { DatabaseSync } from 'node:sqlite';

import { rejectsDirectives } from '../observer/classify.js';
import { reclassifyImportedRow } from '../privacy/classify.js';
import type { DetectorResult } from '../privacy/detect.js';
import { cjkBigrams } from '../retrieval/fts.js';
import { assertLease, transactionImmediate } from './lease.js';

/** Imported rows per fenced write: two detector runs each, so the lease's `now` stays fresh. */
const RECLASSIFY_LIMIT = 50;
/**
 * R12 "Export/import": every quarantined row (`review_state = imported`) goes through the detector
 * and the directive check; the decision table is privacy/classify.ts reclassifyImportedRow. The
 * writes are one fenced transaction so a lost lease changes nothing. A tombstoned row keeps its
 * hashes (FR-035) and loses the secret (FR-018).
 */
export async function reclassifyImported(
  db: DatabaseSync,
  token: string,
  now: () => number,
  detect: (text: string) => Promise<DetectorResult>,
): Promise<{ examined: number; leaseLost: boolean }> {
  const select = db.prepare(
    `SELECT id, title, body FROM memories
     WHERE review_state = 'imported' AND deleted_at IS NULL AND id > ? ORDER BY id LIMIT ?`,
  );
  // The CJK index is a column of its own (0002_memory_search.sql), so it follows the text here.
  const release = db.prepare(
    `UPDATE memories SET review_state = 'unreviewed', title = ?, body = ?, cjk_bigrams = ?
     WHERE id = ? AND review_state = 'imported'`,
  );
  const tombstone = db.prepare(
    `UPDATE memories SET sensitivity = 'secret', deleted_at = ?, title = ?, body = ?, cjk_bigrams = ?
     WHERE id = ? AND review_state = 'imported'`,
  );
  let examined = 0;
  // Keyset pages: a row the detector could not finish stays quarantined and is passed over, so
  // the pass ends even when the detector keeps failing on it (the next run tries it again).
  let after = '';
  for (;;) {
    const rows = select
      .all(after, RECLASSIFY_LIMIT)
      .map((row) => ({ id: String(row.id), title: String(row.title ?? ''), body: String(row.body ?? '') }));
    if (rows.length === 0) return { examined, leaseLost: false };
    examined += rows.length;
    after = rows.at(-1)?.id ?? after;

    const decided: { id: string; decision: 'unreviewed' | 'secret'; title: string; body: string }[] = [];
    for (const row of rows) {
      const directive = rejectsDirectives(row.title) !== null || rejectsDirectives(row.body) !== null;
      const verdict = reclassifyImportedRow(await detect(row.title), await detect(row.body), directive);
      if (verdict.decision === 'retry') continue;
      decided.push({ id: row.id, ...verdict });
    }

    // The clock is read after the detector ran, so the lease heartbeat this write leaves is current.
    const at = now();
    const leaseLost = transactionImmediate(db, () => {
      if (!assertLease(db, token, at)) {
        db.exec('ROLLBACK');
        return true;
      }
      for (const row of decided) {
        const bigrams = cjkBigrams(`${row.title} ${row.body}`);
        if (row.decision === 'unreviewed') release.run(row.title, row.body, bigrams, row.id);
        else tombstone.run(at, row.title, row.body, bigrams, row.id);
      }
      return false;
    });
    if (leaseLost) return { examined, leaseLost: true };
    if (rows.length < RECLASSIFY_LIMIT) return { examined, leaseLost: false };
  }
}
