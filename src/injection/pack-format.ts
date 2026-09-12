// Pack framing shared by the builder and deferred delivery (FR-021).
import type { DegradedReason, ItemReason, LedgerItem } from './ledger.js';
import { PACK_FOOTER as FOOTER, PACK_HEADER as HEADER } from './recognize.js';

/**
 * One full sentence per reason code (contracts/agents.md "Pack format"): the code itself stays in
 * the ledger, `why` and doctor, and the reader of a pack sees plain language.
 */
export const DEGRADED_SENTENCES: Record<DegradedReason, string> = {
  summary_pending:
    'Some information for the selected work is still waiting to be processed. Its checkpoint and recent activity may be incomplete.',
  index_unavailable: 'The memory index could not be read this time, so some notes are missing.',
  empty: 'There is nothing recorded for this repository yet.',
  window_unknown:
    'The context window of this model is not documented yet, so a deliberately small amount of text was selected.',
  no_tool_call: 'This turn ran no tool, so these notes could not be handed over.',
  not_delivered:
    'These notes could not be handed over during this turn and stay available for the next one.',
  no_provider: 'No summarizer is configured, so these are rule-based notes.',
  unreachable: 'The summarizer could not be reached, so these are rule-based notes.',
  unusable_output: 'The summarizer returned an unusable answer, so these are rule-based notes.',
  language_mismatch:
    'The summarizer answered in another language than the content, so these are rule-based notes.',
  daily_cap: "Today's free summary quota is used up, so these are rule-based notes.",
  provider_exhausted: "The summarizer's free allowance is used up, so these are rule-based notes.",
  provider_paid:
    'The configured model is not on the free plan, so these are rule-based notes.',
  auth_failed: 'The summarizer rejected the credentials, so these are rule-based notes.',
  consent_changed:
    'The summarizer settings changed after consent was given, so these are rule-based notes.',
  model_alias: 'The configured model resolved to a different one, so these are rule-based notes.',
  timeout: 'The summarizer did not answer in time, so these are rule-based notes.',
  rule_based: 'These notes were written by the built-in rules rather than by a summarizer.',
};

const STALE_NOTES: Record<'stale_path' | 'stale_commit', string> = {
  stale_path: 'this file is no longer in the repository',
  stale_commit: 'this commit is not in the current history',
};

/** The control characters `canonicalLine` removed; a finished pack must not carry one back. */
export function hasControlCharacter(text: string): boolean {
  return /[\p{Cc}\p{Cf}]/u.test(text.replaceAll('\n', ''));
}

export type PackItem = LedgerItem & { lines: string[] };

/**
 * FR-021: every external string becomes one line before it is framed, so no title, path, remote or
 * body can produce an unprefixed line or a line of its own that starts with `{`.
 */
export function canonicalLine(value: string): string {
  return value
    .replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

function bodyLines(body: string): string[] {
  // The documented format is one line per paragraph; each paragraph is canonicalized on its own and
  // then framed, so a body can produce no unprefixed line (FR-021).
  return body
    .split(/\n\s*\n/)
    .map(canonicalLine)
    .filter((line) => line !== '')
    .map((line) => `> ${line}`);
}

/** FR-021: a pack that began with `{` would be parsed as JSON and dropped by Claude Code. */
export function guardLeadingBrace(text: string): string {
  return text.startsWith('{') ? ` ${text}` : text;
}

/**
 * R8: an identity that still carries `user:password@` never reaches a pack. Only the remote form
 * `host/path` can carry one; a machine-local identity is a file path and is left as it is.
 */
export function withoutUserinfo(identity: string): string {
  if (identity.startsWith('/') || /^[A-Za-z]:[\\/]/.test(identity)) return identity;
  const segments = identity.split('/');
  const at = segments[0].lastIndexOf('@');
  if (at === -1) return identity;
  segments[0] = segments[0].slice(at + 1);
  return segments.join('/');
}

function relativeTime(from: number, now: number): string {
  const minutes = Math.floor(Math.max(0, now - from) / 60_000);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes} minute${minutes === 1 ? '' : 's'} ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} hour${hours === 1 ? '' : 's'} ago`;
  const days = Math.floor(hours / 24);
  return days === 1 ? 'yesterday' : `${days} days ago`;
}

export function renderPack(input: {
  repositoryLine: string;
  blocks: readonly (readonly string[])[];
  degraded: DegradedReason | null;
}): string {
  const lines = [HEADER, input.repositoryLine];
  for (const block of input.blocks) lines.push(...block);
  if (input.degraded !== null) lines.push(`> degraded: ${DEGRADED_SENTENCES[input.degraded]}`);
  lines.push(FOOTER);
  return guardLeadingBrace(lines.join('\n'));
}

export type Citation = { kind: 'file_read' | 'file_modified' | 'commit'; value: string };

export type ActivityRow = { rawEventId: string; line: string };

export type PackMemory = {
  id: string;
  title: string;
  body: string;
  label: 'summary' | 'work checkpoint' | 'pinned' | 'related';
  reason: ItemReason | null;
  rank: number | null;
  scoreBm25?: number | null;
  scoreRrf?: number | null;
  scoreMmr?: number | null;
  createdAt?: number | null;
};

/** One memory's pack item, marked stale when a citation of its own no longer holds. */
export function memoryItem(
  memory: PackMemory,
  own: readonly Citation[],
  context: { commitsFresh: boolean; pathState: Map<string, boolean>; now: number },
): PackItem {
  const stale = own.find((citation) =>
    citation.kind === 'commit' ? !context.commitsFresh : context.pathState.get(citation.value) === false,
  );
  let staleReason: 'stale_path' | 'stale_commit' | null = null;
  if (stale !== undefined) staleReason = stale.kind === 'commit' ? 'stale_commit' : 'stale_path';
  const staleNote = staleReason === null ? '' : `; ${STALE_NOTES[staleReason]}`;
  const shown = stale ?? own[0];
  const note =
    memory.label !== 'related' || shown === undefined ? '' : ` [${canonicalLine(shown.value)}${staleNote}]`;
  const head =
    memory.label === 'summary'
      ? `> session summary (${relativeTime(memory.createdAt ?? context.now, context.now)}):`
      : `> ${memory.label}: ${canonicalLine(memory.title)}${note}`;
  return {
    sourceKind: memory.label === 'summary' || memory.label === 'work checkpoint' ? 'session_summary' : 'memory',
    memoryId: memory.id,
    rawEventId: null,
    decision: 'planned',
    // A stale citation is the more specific record; the memory is still injected, marked.
    reason: staleReason ?? memory.reason,
    rank: memory.rank,
    scoreBm25: memory.scoreBm25 ?? null,
    scoreRrf: memory.scoreRrf ?? null,
    scoreMmr: memory.scoreMmr ?? null,
    stale: stale === undefined ? 0 : 1,
    lines: [head, ...bodyLines(memory.body)],
  };
}

/** One recent raw event's pack item. */
export function activityItem(activity: ActivityRow): PackItem {
  return {
    sourceKind: 'raw_activity',
    memoryId: null,
    rawEventId: activity.rawEventId,
    decision: 'planned',
    reason: null,
    rank: null,
    stale: 0,
    lines: [`> recent activity: ${canonicalLine(activity.line)}`],
  };
}
