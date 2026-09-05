import type { DetectorResult } from './detect.js';
import type { Sensitivity } from './egress.js';

// docs/dev/conventions.md "Sensitivity and egress": secret > private > local_only > eligible.
const STRICTNESS: Record<Sensitivity, number> = {
  eligible: 0,
  local_only: 1,
  private: 2,
  secret: 3,
};

/**
 * The worker's promotion rule (FR-017). The table, in order:
 *
 * | current                | classification state | detector      | result    |
 * |------------------------|----------------------|---------------|-----------|
 * | private or secret      | any                  | any           | unchanged |
 * | any                    | partial or failed    | any           | unchanged |
 * | any                    | done                 | failed        | unchanged |
 * | any                    | done                 | secret found  | secret    |
 * | local_only or eligible | done                 | clean         | eligible  |
 *
 * A row is never made less strict by anything but a complete, successful, clean detector run. The
 * stricter of two classes is `strictest`, which the apply step uses for a memory's sensitivity.
 */
export function promoteSensitivity(
  current: Sensitivity,
  detector: DetectorResult,
  classificationState: 'done' | 'partial' | 'failed',
): Sensitivity {
  // A class the capture step decided stays as it was recorded.
  if (current === 'private' || current === 'secret') return current;
  // A7: a partial row and a failed row are never promoted.
  if (classificationState !== 'done') return current;
  // R4: a detector failure fails closed, so it never promotes either.
  if (!detector.ok) return current;
  if (detector.sensitivity === 'secret') return 'secret';
  return 'eligible';
}

/** The stricter class of the lattice, used wherever several sources decide one row (R10). */
export function strictest(first: Sensitivity, ...rest: Sensitivity[]): Sensitivity {
  // The first class is required so an empty source list cannot silently yield the loosest class.
  let strictestValue: Sensitivity = first;
  for (const value of rest) {
    if (STRICTNESS[value] > STRICTNESS[strictestValue]) strictestValue = value;
  }
  return strictestValue;
}

export type ImportedDecision =
  | { decision: 'retry' }
  | { decision: 'unreviewed' | 'secret'; title: string; body: string };

/**
 * R12 "Export/import": an imported row is quarantined (`review_state = imported`) until the worker
 * has run the detector on its title and body and the directive check on both. A clean, complete run
 * without a directive releases it as `unreviewed`; a secret finding or a directive tombstones it as
 * `secret`; a detector that did not finish decides nothing, and the row stays quarantined for the
 * next run (fail closed, R4). The texts returned are the detector's, so what the detector removed
 * (a `<private>` span, a redacted secret) is what gets stored (FR-018, FR-019).
 */
export function reclassifyImportedRow(
  title: DetectorResult,
  body: DetectorResult,
  directive: boolean,
): ImportedDecision {
  if (!title.ok || !body.ok) return { decision: 'retry' };
  const decision =
    title.sensitivity === 'secret' || body.sensitivity === 'secret' || directive ? 'secret' : 'unreviewed';
  return { decision, title: title.text, body: body.text };
}
