// The single builder of every outbound observer request (T034).
// Sources: contracts/observer.md ("Batch composition and the outbound boundary", "Input", "Call
// policy" item 6), research.md R10, data-model.md destination_rules, spec FR-015, FR-020, SC-005,
// SC-006. Security-owned (plan.md "Structure Decision"): no other module assembles a request, and
// every field here states which rule admitted it.
import type { NearbyCandidate } from '../db/queries.js';
import { canonicalJson, contentHash } from '../events.js';
import { isAllowed, type DestinationRules } from '../privacy/egress.js';
import {
  payloadOf,
  toolInputOf,
  type RawEventRow,
  type SessionRow,
  type TurnRow,
} from '../worker/batches.js';
import { dominantScript } from './classify.js';
import {
  MAX_INPUT_CHARS, MAX_NEARBY_BODY, MAX_SOURCE_EVENT_IDS, MAX_TITLE,
  observerInputSchema, type ObserverInput,
} from './contract.js';

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
  checkpointContext?: ObserverInput['checkpoint_context'];
};

export type ObserverRequest = {
  input: ObserverInput;
  excerpted: boolean;
  dropped: DroppedRow[];
  coverage: SourcePortion[];
};

export type SourcePortion = {
  rowId: string;
  state: 'full' | 'partial' | 'omitted';
  start: number;
  end: number;
  total: number;
  sourceHash: string;
  text: string;
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
  return [event.text, event.output, event.error, event.fragment?.text, input?.command, input?.text]
    .filter((value): value is string => typeof value === 'string')
    .join('\n');
}

/** Admission happens before paging, including all metadata and normalized tool input. */
function collectObserverEvents(request: ObserverRequestInput): {
  dropped: DroppedRow[];
  events: ObserverEvent[];
} {
  const dropped: DroppedRow[] = [];
  const events: ObserverEvent[] = [];
  for (const row of request.rows) {
    const reason = refuse(request.rules, request.destination, row, request.repoId);
    if (reason !== null) {
      dropped.push({ rowId: row.id, reason });
      continue;
    }
    if (!OBSERVER_EVENT_KINDS.has(row.kind)) continue;
    const event = eventFor(row);
    if (event === null) continue;
    events.push({ ...event, captured_at: row.captured_at });
  }
  return { dropped, events };
}

/** The nearby memories the destination may receive, and the candidates the rule table refused. */
function collectNearbyMemories(request: ObserverRequestInput): {
  dropped: DroppedRow[];
  nearby: ObserverInput['nearby'];
} {
  const dropped: DroppedRow[] = [];
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
      title: candidate.title.slice(0, MAX_TITLE),
      body: candidate.body.slice(0, MAX_NEARBY_BODY),
      deleted: candidate.deleted,
      captured_at: candidate.source_captured_at ?? null,
    });
  }
  return { dropped, nearby };
}

/**
 * Assembles the one outbound request of a batch. Every field passes the same rule table, so a row
 * or a memory the destination may not receive is absent from the body rather than trimmed from it
 * later (contracts/observer.md, SC-006). Citations travel only inside the tool inputs of admitted
 * events; the request has no other place for a path.
 */
export function buildObserverRequest(request: ObserverRequestInput): ObserverRequest {
  const admitted = collectObserverEvents(request);
  const nearbyResult = collectNearbyMemories(request);
  const input: ObserverInput = {
    repo_ref: request.repoId,
    checkpoint_context: request.checkpointContext ?? { state: 'none' },
    session: { started_at: request.session.started_at ?? 0, turns: [] },
    events: [], free_summaries: {}, nearby: [], language_hint: 'other',
  };
  // A full prior checkpoint is mandatory context for replacement. Never clip it to fit sources.
  if (!fits(input)) input.checkpoint_context = { state: 'withheld' };
  const rows = new Map(request.rows.map((row) => [row.id, row]));
  const coverage: SourcePortion[] = [];
  let pageClosed = false;
  for (const event of admitted.events) {
    const text = canonicalJson(event);
    const sourceHash = `event-json-v1:${contentHash(text)}`;
    const row = rows.get(event.id)!;
    const start = row.processing_hash === sourceHash ? row.processing_offset ?? 0 : 0;
    const portion: SourcePortion = {
      rowId: event.id, state: 'omitted', start, end: start, total: text.length, sourceHash, text: '',
    };
    coverage.push(portion);
    if (pageClosed || input.events.length >= MAX_SOURCE_EVENT_IDS) continue;
    if (start === 0 && fits({ ...input, events: [...input.events, event] })) {
      input.events.push(event);
      Object.assign(portion, { state: 'full', end: text.length, text });
    } else if (input.events.length === 0) {
      const fragment = fitFragment(input, event, text, portion);
      if (fragment !== null) {
        input.events.push(fragment);
        Object.assign(portion, { state: 'partial', end: fragment.fragment!.end, text: fragment.fragment!.text });
      }
    }
    if (portion.state !== 'full') pageClosed = true;
  }
  // Summaries already occur as covered events. Repeating them here would bypass the range cap.
  const sentIds = new Set(input.events.map((event) => event.id));
  const turnIds = new Set(request.rows.filter((row) => sentIds.has(row.id)).map((row) => row.turn_id));
  for (const turn of request.turns) {
    if (!turnIds.has(turn.id)) continue;
    input.session.turns.push({ ordinal: turn.ordinal, started_at: turn.started_at ?? 0, ended_at: turn.ended_at });
    if (!fits(input)) input.session.turns.pop();
  }
  for (const candidate of nearbyResult.nearby) {
    input.nearby.push(candidate);
    if (!fits(input)) input.nearby.pop();
  }
  input.language_hint = dominantScript(input.events.map(textOf).join('\n'));
  return {
    input: observerInputSchema.parse(input),
    excerpted: coverage.some((portion) => portion.state !== 'full'),
    dropped: [...admitted.dropped, ...nearbyResult.dropped], coverage,
  };
}

function fits(input: ObserverInput): boolean {
  return JSON.stringify(input).length <= MAX_INPUT_CHARS;
}

/** Only a source that cannot fit by itself is split; ranges never discard the remaining text. */
function fitFragment(
  input: ObserverInput, event: ObserverEvent, text: string, portion: SourcePortion,
): ObserverEvent | null {
  let low = portion.start + 1;
  let high = text.length;
  let best: ObserverEvent | null = null;
  while (low <= high) {
    const middle = Math.floor((low + high) / 2);
    let end = middle;
    if (end < text.length && /[\uD800-\uDBFF]/u.test(text[end - 1]) && /[\uDC00-\uDFFF]/u.test(text[end])) end -= 1;
    const candidate: ObserverEvent = {
      id: event.id, kind: event.kind, captured_at: event.captured_at,
      fragment: {
        format: 'event-json-v1', source_hash: portion.sourceHash,
        start: portion.start, end, total: text.length, text: text.slice(portion.start, end),
      },
    };
    if (fits({ ...input, events: [candidate] })) {
      if (end > portion.start) best = candidate;
      low = middle + 1;
    } else high = middle - 1;
  }
  return best;
}
