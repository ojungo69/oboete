import { contentHash, materialHash } from '../db/identity.js';
import type { ToolInput } from '../events.js';
import {
  MAX_BODY,
  MAX_CITATION_LENGTH,
  MAX_COMMITS,
  MAX_OBSERVATIONS,
  MAX_PATHS,
  MAX_SOURCE_EVENT_IDS,
  shortenDisplayPath,
  trimObservation,
  type Observation,
  type ObserverOutput,
} from './contract.js';

export type FallbackEvent = {
  id: string;
  kind:
    | 'prompt'
    | 'tool_call'
    | 'tool_result'
    | 'tool_failure'
    | 'last_assistant_message'
    | 'compaction_summary';
  turn_index: number;
  tool_call_id?: string;
  tool_name?: string;
  input?: ToolInput;
  output?: string;
  error?: string;
  text?: string;
  is_error?: boolean;
  sensitivity: 'eligible' | 'local_only' | 'private' | 'secret';
  classification_state: 'done' | 'partial';
};

export type FallbackInput = {
  repoId: string;
  events: FallbackEvent[];
  nearby: { id: string; content_hash: string; deleted: boolean }[];
};

export type SuppressedObservation = { title: string; content_hash: string; target: string };
export type FallbackOutput = ObserverOutput & { suppressed: SuppressedObservation[] };

type Rule = 'change' | 'bugfix' | 'discovery' | 'decision';

export function firstLine(text: string): string {
  const end = text.search(/\r?\n/u);
  return end === -1 ? text : text.slice(0, end);
}

export function firstSentence(text: string): string {
  const end = text.search(/[.!?。！？\n]/u);
  if (end === -1) return text;
  return text[end] === '\n' ? text.slice(0, end).replace(/\r$/u, '') : text.slice(0, end + 1);
}

export function firstParagraph(text: string): string {
  const end = text.search(/\r?\n[\t ]*\r?\n/u);
  return end === -1 ? text : text.slice(0, end);
}

function visibleInput(event: FallbackEvent): ToolInput | undefined {
  if (event.input === undefined) return undefined;
  return event.classification_state === 'partial' ? { paths: event.input.paths } : event.input;
}

function visibleText(event: FallbackEvent, field: 'error' | 'output' | 'text'): string {
  return event.classification_state === 'partial' ? '' : (event[field] ?? '');
}

function unique(values: string[], limit: number): string[] {
  return [...new Set(values)].slice(0, limit);
}

function citationPaths(values: string[]): string[] {
  return unique(values.filter(Boolean), MAX_PATHS).map((path) => path.slice(0, MAX_CITATION_LENGTH));
}

function sourceIds(values: FallbackEvent[]): string[] {
  return unique(values.map((event) => event.id), MAX_SOURCE_EVENT_IDS);
}

function observation(
  rule: Rule,
  type: Observation['type'],
  title: string,
  body: string,
  concepts: Observation['concepts'],
  sources: FallbackEvent[],
  citations: Partial<Observation['citations']> = {},
): Observation {
  return trimObservation({
    type,
    title,
    body,
    concepts,
    citations: {
      files_read: citationPaths(citations.files_read ?? []),
      files_modified: citationPaths(citations.files_modified ?? []),
      commits: unique(citations.commits ?? [], MAX_COMMITS),
    },
    source_event_ids: sourceIds(sources),
    classification: { decision: 'add', target: null, reason: `rule:${rule}` },
  });
}

function eventsByTurn(events: FallbackEvent[]): [number, FallbackEvent[]][] {
  const turns = new Map<number, FallbackEvent[]>();
  for (const event of events) {
    const turn = turns.get(event.turn_index) ?? [];
    turn.push(event);
    turns.set(event.turn_index, turn);
  }
  return [...turns].sort(([left], [right]) => left - right);
}

function commitIds(turn: FallbackEvent[]): string[] {
  const calls = new Map(
    turn
      .filter((event) => event.kind === 'tool_call' && event.tool_call_id !== undefined)
      .map((event) => [event.tool_call_id as string, event]),
  );
  const commits: string[] = [];
  for (const event of turn) {
    if (event.kind !== 'tool_result' || event.tool_call_id === undefined) continue;
    const call = calls.get(event.tool_call_id);
    if (!visibleInput(call ?? event)?.command?.includes('git')) continue;
    // ponytail: commit ids are heuristic hex words in git output; use structured git events if false positives matter.
    commits.push(...(visibleText(event, 'output').match(/\b[0-9a-f]{7,40}\b/giu) ?? []));
  }
  return unique(commits.map((value) => value.toLowerCase()), MAX_COMMITS);
}

function changes(events: FallbackEvent[]): Observation[] {
  const output: Observation[] = [];
  for (const [, turn] of eventsByTurn(events)) {
    const calls = turn.filter((event) => event.kind === 'tool_call');
    const modified = calls.filter((event) => {
      const input = visibleInput(event);
      return (
        event.tool_name === 'write' ||
        event.tool_name === 'edit' ||
        input?.lines_added !== undefined ||
        input?.lines_removed !== undefined
      );
    });
    const paths = [...new Set(modified.flatMap((event) => visibleInput(event)?.paths ?? []))];
    if (paths.length === 0) continue;
    const lines = calls.map((event) => {
      const input = visibleInput(event);
      const path = (input?.paths ?? []).map(shortenDisplayPath).join(', ');
      return `${event.tool_name ?? ''} ${path} (+${input?.lines_added ?? 0}/-${input?.lines_removed ?? 0})`;
    });
    const kept = lines.slice(0, 40);
    if (lines.length > kept.length) kept.push(`... (+${lines.length - kept.length} omitted)`);
    output.push(
      observation('change', 'change', paths.join(', '), kept.join('\n'), ['what-changed'], turn, {
        files_modified: paths,
        commits: commitIds(turn),
      }),
    );
  }
  return output;
}

function failedCall(turn: FallbackEvent[], failure: FallbackEvent): FallbackEvent | undefined {
  return turn.find(
    (event) => event.kind === 'tool_call' && event.tool_call_id === failure.tool_call_id,
  );
}

function retryAfter(
  turn: FallbackEvent[],
  failureIndex: number,
  toolName: string,
): { call: FallbackEvent; result?: FallbackEvent } | undefined {
  for (let index = failureIndex + 1; index < turn.length; index += 1) {
    const call = turn[index];
    if (call?.kind !== 'tool_call' || call.tool_name !== toolName) continue;
    const related =
      call.tool_call_id === undefined
        ? [call]
        : turn.filter((event) => event.tool_call_id === call.tool_call_id);
    const result = related.find(
      (event) => event.kind === 'tool_result' && event.classification_state === 'done',
    );
    const failed = related.some((event) => event.kind === 'tool_failure');
    if (result?.is_error === false || (result?.is_error !== true && !failed)) return { call, result };
  }
  return undefined;
}

function retryFirstLine(event: FallbackEvent): string {
  const input = visibleInput(event);
  if (input?.command !== undefined) return firstLine(input.command).slice(0, 200);
  if (input?.paths[0] !== undefined) return shortenDisplayPath(input.paths[0]).slice(0, 200);
  return firstLine(input?.text ?? '').slice(0, 200);
}

function failures(events: FallbackEvent[]): { bugfixes: Observation[]; discoveries: Observation[] } {
  const bugfixes: Observation[] = [];
  const discoveries: Observation[] = [];
  for (const [, turn] of eventsByTurn(events)) {
    for (const [index, failure] of turn.entries()) {
      if (failure.kind !== 'tool_failure') continue;
      const call = failedCall(turn, failure);
      const toolName = failure.tool_name ?? call?.tool_name ?? '';
      const error = firstLine(visibleText(failure, 'error'));
      const retry = retryAfter(turn, index, toolName);
      if (retry !== undefined) {
        bugfixes.push(
          observation(
            'bugfix',
            'bugfix',
            `${toolName}: ${error.slice(0, 80)}`,
            `${error.slice(0, 200)}\n${retryFirstLine(retry.call)}`,
            ['problem-solution'],
            [failure, retry.call, ...(retry.result === undefined ? [] : [retry.result])],
            { files_modified: visibleInput(retry.call)?.paths ?? [] },
          ),
        );
      } else {
        discoveries.push(
          observation(
            'discovery',
            'discovery',
            `${toolName}: ${error.slice(0, 80)}`,
            error.slice(0, 200),
            ['gotcha'],
            [...(call === undefined ? [] : [call]), failure],
            { files_read: visibleInput(call ?? failure)?.paths ?? [] },
          ),
        );
      }
    }
  }
  return { bugfixes, discoveries };
}

function decisions(events: FallbackEvent[]): Observation[] {
  return [...events]
    .map((event, index) => ({ event, index, text: (event.text ?? '').trim() }))
    .filter(
      ({ event, text }) =>
        (event.kind === 'last_assistant_message' || event.kind === 'compaction_summary') &&
        event.classification_state === 'done' &&
        text !== '',
    )
    .sort((left, right) => left.event.turn_index - right.event.turn_index || left.index - right.index)
    .map(({ event, text }) =>
      observation(
        'decision',
        'decision',
        firstSentence(text),
        firstParagraph(text).slice(0, MAX_BODY),
        ['why-it-exists'],
        [event],
      ),
    );
}

export function fallbackObserve(input: FallbackInput): FallbackOutput {
  const events = input.events.filter((event) => event.sensitivity !== 'secret');
  const failed = failures(events);
  const candidates = [...decisions(events), ...changes(events), ...failed.bugfixes, ...failed.discoveries];
  const observations: Observation[] = [];
  const suppressed: SuppressedObservation[] = [];

  for (const candidate of candidates) {
    const hash = contentHash(input.repoId, materialHash(candidate.title, candidate.body));
    const tombstone = input.nearby.find((row) => row.content_hash === hash && row.deleted);
    if (tombstone !== undefined) {
      suppressed.push({ title: candidate.title, content_hash: hash, target: tombstone.id });
      continue;
    }
    if (observations.length === MAX_OBSERVATIONS) continue;
    const active = input.nearby.find((row) => row.content_hash === hash && !row.deleted);
    observations.push({
      ...candidate,
      classification:
        active === undefined
          ? candidate.classification
          : { ...candidate.classification, decision: 'noop', target: active.id },
    });
  }
  return { observations, suppressed };
}
