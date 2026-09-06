// The single builder of every outbound observer request (T034).
// Sources: contracts/observer.md ("Batch composition and the outbound boundary", "Input", "Call
// policy" item 6), research.md R10, data-model.md destination_rules, spec FR-015, FR-020, SC-005,
// SC-006. Security-owned (plan.md "Structure Decision"): no other module assembles a request, and
// every field here states which rule admitted it.
import type { NearbyCandidate } from '../db/queries.js';
import { isAllowed, type DestinationRules } from '../privacy/egress.js';
import {
  payloadOf,
  toolInputOf,
  type RawEventRow,
  type SessionRow,
  type TurnRow,
} from '../worker/batches.js';
import { dominantScript } from './classify.js';
import { excerptInput, observerInputSchema, type ObserverInput } from './contract.js';

/** The two destinations that produce a request; the fallback needs none (contracts/observer.md). */
export type ObserverDestination = 'remote_observer' | 'local_observer';

export type DropReason = 'sensitivity' | 'repository' | 'partial' | 'failed';
export type DroppedRow = { rowId: string; reason: DropReason };

export type ObserverRequestInput = {
  rows: RawEventRow[];
  session: SessionRow;
  turns: TurnRow[];
  destination: ObserverDestination;
  repoId: string;
  nearby: NearbyCandidate[];
  rules: DestinationRules;
};

export type ObserverRequest = {
  input: ObserverInput;
  excerpted: boolean;
  dropped: DroppedRow[];
};

type ObserverEvent = ObserverInput['events'][number];

// The kinds the contract's input schema names. A lifecycle row carries no content a summarizer can
// use, so it is not part of a request at all (data-model.md sessions.summary_state).
const OBSERVER_EVENT_KINDS: ReadonlySet<string> = new Set([
  'prompt',
  'tool_call',
  'tool_result',
  'tool_failure',
  'last_assistant_message',
  'compaction_summary',
]);

/** Why this row may not travel, or null when it may (FR-020: one rule table decides). */
function refuse(
  rules: DestinationRules,
  destination: ObserverDestination,
  row: RawEventRow,
  repoId: string,
): DropReason | null {
  // A7: a partial row hands metadata to the rule-based fallback and never text to a provider.
  if (row.classification_state === 'partial') return 'partial';
  // data-model.md raw_events: a failed classification is metadata only and is never summarized.
  if (row.classification_state === 'failed') return 'failed';
  // FR-044: the batch is one session of one repository, so a foreign row is a bug, not a filter.
  if (row.repo_id !== repoId) return 'repository';
  // FR-020: the seeded rule table is the only sensitivity decision on this path.
  if (!isAllowed(rules, destination, row.sensitivity, true)) return 'sensitivity';
  return null;
}

function eventFor(row: RawEventRow): ObserverEvent | null {
  const text = row.content ?? '';
  switch (row.kind) {
    case 'prompt':
    case 'last_assistant_message':
    case 'compaction_summary':
      return { id: row.id, kind: row.kind, text };
    case 'tool_result':
      return {
        id: row.id,
        kind: 'tool_result',
        output: text,
        is_error: payloadOf(row)?.is_error === true,
      };
    case 'tool_failure':
      return { id: row.id, kind: 'tool_failure', error: text };
    case 'tool_call': {
      // The normalized tool input is the only part of `payload_json` that travels; capture wrote it
      // after the detector ran (data-model.md raw_events, FR-018), and anything the schema does not
      // name stays on this machine.
      const payload = payloadOf(row);
      return {
        id: row.id,
        kind: 'tool_call',
        tool_name: typeof payload?.tool_name === 'string' ? payload.tool_name : 'other',
        input: toolInputOf(row),
      };
    }
    default:
      return null;
  }
}

function textOf(event: ObserverEvent): string {
  const input = event.input as { command?: string; text?: string } | undefined;
  return [event.text, event.output, event.error, input?.command, input?.text]
    .filter((value): value is string => typeof value === 'string')
    .join('\n');
}

/**
 * Assembles the one outbound request of a batch. Every field passes the same rule table, so a row
 * or a memory the destination may not receive is absent from the body rather than trimmed from it
 * later (contracts/observer.md, SC-006). Citations travel only inside the tool inputs of admitted
 * events; the request has no other place for a path.
 */
export function buildObserverRequest(request: ObserverRequestInput): ObserverRequest {
  const dropped: DroppedRow[] = [];
  const events: ObserverEvent[] = [];
  const freeSummaries: ObserverInput['free_summaries'] = {};

  for (const row of request.rows) {
    const reason = refuse(request.rules, request.destination, row, request.repoId);
    if (reason !== null) {
      dropped.push({ rowId: row.id, reason });
      continue;
    }
    if (!OBSERVER_EVENT_KINDS.has(row.kind)) continue;
    const event = eventFor(row);
    if (event === null) continue;
    events.push(event);
    // The summary text the agent supplies for free also fills its own field (contracts/observer.md
    // "Input"); it is the same admitted row, so it passed the same check.
    if (row.kind === 'last_assistant_message') {
      freeSummaries.last_assistant_message = row.content ?? '';
    }
    if (row.kind === 'compaction_summary') freeSummaries.compaction_summary = row.content ?? '';
  }

  const nearby: ObserverInput['nearby'] = [];
  for (const candidate of request.nearby) {
    // R10: the candidates are same-repository by construction; the check makes that an invariant
    // rather than an assumption of the caller.
    if (candidate.repo_id !== request.repoId) {
      dropped.push({ rowId: candidate.id, reason: 'repository' });
      continue;
    }
    if (!isAllowed(request.rules, request.destination, candidate.sensitivity, true)) {
      dropped.push({ rowId: candidate.id, reason: 'sensitivity' });
      continue;
    }
    nearby.push({
      id: candidate.id,
      type: candidate.type,
      title: candidate.title,
      body: candidate.body,
      deleted: candidate.deleted,
    });
  }

  const admittedText = [
    ...events.map(textOf),
    freeSummaries.last_assistant_message ?? '',
    freeSummaries.compaction_summary ?? '',
  ].join('\n');

  const built = observerInputSchema.parse({
    // R10: the repository travels as its opaque id, never as the normalized identity or a path.
    repo_ref: request.repoId,
    // No cwd and no agent: the producing agent is provenance only (FR-005, SC-006).
    session: {
      started_at: request.session.started_at ?? 0,
      turns: request.turns.map((turn) => ({
        ordinal: turn.ordinal,
        started_at: turn.started_at ?? 0,
        ended_at: turn.ended_at,
      })),
    },
    events,
    free_summaries: freeSummaries,
    nearby,
    // FR-014: the observer answers in the language of the content it was given.
    language_hint: dominantScript(admittedText),
  });

  // FR-015: 12,000 characters, and the caller records `excerpted` on the batch row.
  const excerpt = excerptInput(built);
  return { input: excerpt.input, excerpted: excerpt.excerpted, dropped };
}
