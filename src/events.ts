// Normalized events, event identity and conversation identity.
// Sources: specs/007-oboete-m1-alpha/contracts/agents.md ("Normalized events", "Agent identity",
// "Event identity and conversation identity"), research.md R7, data-model.md (raw_events, sessions).
import { z } from 'zod';

import { sha256Hex, sha256Json } from './hash.js';

export const AGENTS = ['claude', 'codex', 'grok', 'pi', 'unknown'] as const;

export const EVENT_KINDS = [
  'session_start',
  'prompt',
  'tool_call',
  'tool_result',
  'tool_failure',
  'turn_end',
  'session_end',
  'compaction_summary',
  'last_assistant_message',
  'probe',
] as const;

export const SESSION_START_SOURCES = [
  'startup',
  'resume',
  'clear',
  'compact',
  'fork',
  'new',
] as const;

export const INPUT_SOURCES = ['user', 'rpc', 'extension'] as const;

export const TOOL_NAMES = ['read', 'write', 'edit', 'bash', 'grep', 'glob', 'task', 'other'] as const;

/** A tool served over MCP, normalized to `mcp:<server>/<tool>` (contracts/agents.md). */
export const MCP_TOOL_NAME_PATTERN = /^mcp:[^/]+\/[^/]+$/;

/** The cap on one stored text. The hook's stdin read bound is smaller (A14, src/capture.ts). */
export const MAX_TEXT = 1_048_576;
/** A rendered tool input stays short: it is summarizer and pack material, not a transcript. */
export const MAX_TOOL_INPUT_TEXT = 20_000;
export const MAX_TOOL_INPUT_PATHS = 50;

export type AgentName = (typeof AGENTS)[number];
export type EventKind = (typeof EVENT_KINDS)[number];
export type SessionStartSource = (typeof SESSION_START_SOURCES)[number];
export type InputSource = (typeof INPUT_SOURCES)[number];
export type ToolName = (typeof TOOL_NAMES)[number] | `mcp:${string}/${string}`;

export function isToolName(value: string): value is ToolName {
  return (TOOL_NAMES as readonly string[]).includes(value) || MCP_TOOL_NAME_PATTERN.test(value);
}

const text = z.string().max(MAX_TEXT);
// An identifier that decides an event id must not be empty: an empty key would collapse unrelated
// events into one row (contracts/agents.md "Event identity", R7).
const identifier = z.string().min(1);
const toolNameSchema = z.custom<ToolName>(
  (value) => typeof value === 'string' && isToolName(value),
  { error: 'The tool name must be a normalized name or mcp:<server>/<tool>.' },
);

/**
 * The agent-neutral tool input the adapters produce. Strict and small: `raw_events.payload_json`
 * holds normalized fields only, never a raw payload (data-model.md raw_events).
 */
export const toolInputSchema = z.strictObject({
  paths: z.array(text).max(MAX_TOOL_INPUT_PATHS),
  command: text.optional(),
  text: z.string().max(MAX_TOOL_INPUT_TEXT).optional(),
  lines_added: z.int().optional(),
  lines_removed: z.int().optional(),
});

/**
 * The envelope every event carries. There is no repository field of any kind: oboete derives
 * repository identity from `cwd` and never accepts it from an agent or a payload (FR-004).
 */
export const envelopeSchema = z.strictObject({
  agent: z.enum(AGENTS),
  native_session_id: identifier,
  cwd: identifier,
  captured_at: z.int(),
  agent_id: identifier.optional(),
  agent_type: identifier.optional(),
  model: identifier.optional(),
});

const envelope = envelopeSchema.shape;
const promptRef = { prompt_id: identifier.optional() };

export const eventSchema = z.discriminatedUnion('kind', [
  z.strictObject({
    ...envelope,
    ...promptRef,
    kind: z.literal('session_start'),
    source: z.enum(SESSION_START_SOURCES),
  }),
  z.strictObject({
    ...envelope,
    ...promptRef,
    kind: z.literal('prompt'),
    text,
    input_source: z.enum(INPUT_SOURCES),
  }),
  z.strictObject({
    ...envelope,
    ...promptRef,
    kind: z.literal('tool_call'),
    tool_call_id: identifier,
    tool_name_native: identifier,
    tool_name: toolNameSchema,
    input: toolInputSchema,
  }),
  z.strictObject({
    ...envelope,
    ...promptRef,
    kind: z.literal('tool_result'),
    tool_call_id: identifier,
    output: text,
    is_error: z.boolean(),
  }),
  z.strictObject({
    ...envelope,
    ...promptRef,
    kind: z.literal('tool_failure'),
    tool_call_id: identifier,
    error: text,
  }),
  z.strictObject({
    ...envelope,
    ...promptRef,
    kind: z.literal('turn_end'),
    turn_index: z.int(),
    reason: text,
  }),
  z.strictObject({ ...envelope, ...promptRef, kind: z.literal('session_end'), reason: text }),
  z.strictObject({
    ...envelope,
    ...promptRef,
    kind: z.literal('compaction_summary'),
    text,
    // The native per-compaction value (Grok PostCompact.timestamp, Pi compactionEntry.id). Claude
    // Code and Codex have none, so the adapter passes '' and the event id becomes the key (A16).
    compaction_key: text,
  }),
  z.strictObject({ ...envelope, ...promptRef, kind: z.literal('last_assistant_message'), text }),
  z.strictObject({ ...envelope, ...promptRef, kind: z.literal('probe'), marker: text }),
]);

export type Envelope = z.infer<typeof envelopeSchema>;
export type ToolInput = z.infer<typeof toolInputSchema>;
export type NormalizedEvent = z.infer<typeof eventSchema>;
export type SessionStartEvent = Extract<NormalizedEvent, { kind: 'session_start' }>;
export type PromptEvent = Extract<NormalizedEvent, { kind: 'prompt' }>;
export type ToolCallEvent = Extract<NormalizedEvent, { kind: 'tool_call' }>;
export type ToolResultEvent = Extract<NormalizedEvent, { kind: 'tool_result' }>;
export type ToolFailureEvent = Extract<NormalizedEvent, { kind: 'tool_failure' }>;
export type TurnEndEvent = Extract<NormalizedEvent, { kind: 'turn_end' }>;
export type SessionEndEvent = Extract<NormalizedEvent, { kind: 'session_end' }>;
export type CompactionSummaryEvent = Extract<NormalizedEvent, { kind: 'compaction_summary' }>;
export type LastAssistantMessageEvent = Extract<NormalizedEvent, { kind: 'last_assistant_message' }>;
export type ProbeEvent = Extract<NormalizedEvent, { kind: 'probe' }>;

const ENVELOPE_KEYS: ReadonlySet<string> = new Set(Object.keys(envelope));

export function canonicalJson(value: unknown): string {
  if (value === null || typeof value !== 'object') return JSON.stringify(value) ?? 'null';
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  const entries = Object.entries(value as Record<string, unknown>)
    .filter(([, item]) => item !== undefined)
    .sort(([left], [right]) => (left < right ? -1 : 1));
  return `{${entries.map(([key, item]) => `${JSON.stringify(key)}:${canonicalJson(item)}`).join(',')}}`;
}

export function contentHash(value: string): string {
  return sha256Hex(value);
}

/**
 * The hash of the kind-specific fields alone. The envelope and the capture time stay out, so two
 * byte-identical deliveries of one event hash the same (contracts/agents.md "Event identity").
 */
export function eventContentHash(event: NormalizedEvent): string {
  const fields: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(event)) {
    if (!ENVELOPE_KEYS.has(key)) fields[key] = value;
  }
  return contentHash(canonicalJson(fields));
}

/**
 * The most specific stable key for an event, kind included in every form (R7). No delivery
 * counter and no timestamp, so a re-delivery always produces the same id. `turnOrdinal` is R7's
 * turn key for an event that carries no per-turn value of its own; capture supplies it from
 * `sessions.turn_count`, the one read that contracts/agents.md's "no database read" does not
 * cover, and a spooled event has to be replayed with the ordinal it had when it was captured, or
 * the direct path and the spool path give one event two ids and the re-delivery stops collapsing.
 * Accepted limits of the last form, all of them re-deliveries that do not collapse or events that
 * do: two byte-identical events collapse to one row inside one turn when either key exists and
 * session-wide when neither does; an event captured on the spool path (no ordinal, key '') that is
 * re-delivered later on the direct path (ordinal known) gets a second id, which is reachable for a
 * lifecycle event of an agent that supplies no `prompt_id`, Grok Build's `SessionStart` among them;
 * and a prompt with no `prompt_id` that is re-delivered opens the next turn, so it gets the next
 * ordinal and a second row. The alternative - no ordinal on a prompt - collapses two identical
 * prompts of one session ("continue" twice) into one row, which is the more frequent loss.
 */
export function eventIdKey(event: NormalizedEvent, turnOrdinal?: number): string[] {
  const base = ['v1', event.agent, event.native_session_id, event.kind];
  // A tool call and its result share the native call id, so the kind keeps them apart (R7).
  if (event.kind === 'tool_call' || event.kind === 'tool_result' || event.kind === 'tool_failure') {
    return [...base, event.tool_call_id];
  }
  // The agent's own prompt id identifies a prompt across re-deliveries and edits of the text.
  if (event.kind === 'prompt' && event.prompt_id !== undefined) {
    return [...base, event.prompt_id];
  }
  // The native per-compaction value where an agent has one; '' means it has none (A16).
  if (event.kind === 'compaction_summary' && event.compaction_key !== '') {
    return [...base, event.compaction_key];
  }
  return [...base, turnKey(event, turnOrdinal), eventContentHash(event)];
}

/** The agent's own per-turn value, else the turn ordinal, else nothing at all (R7, A16). */
function turnKey(event: NormalizedEvent, turnOrdinal: number | undefined): string {
  if (event.prompt_id !== undefined) return event.prompt_id;
  return turnOrdinal === undefined ? '' : String(turnOrdinal);
}

export function eventId(event: NormalizedEvent, turnOrdinal?: number): string {
  return sha256Json(eventIdKey(event, turnOrdinal));
}

export type ConversationDecision = 'reuse_root' | 'new_root';

// Per session-start source, either a fixed decision or "whichever the native session id says".
// Sources: contracts/agents.md "Event identity and conversation identity" and the 2026-09-03 R13
// probes. `resume` reuses only a session id oboete already knows, because Grok's --fork-session
// also reports source `load` (mapped to `resume`) but with a new session id, and Pi's resume is
// recognized by id continuity alone.
const CONVERSATION_RULES: Record<
  SessionStartSource | 'none',
  ConversationDecision | 'by_session_id'
> = {
  startup: 'by_session_id',
  clear: 'by_session_id',
  resume: 'by_session_id',
  // Codex fires no SessionStart on /new (A18): a hook without a source is placed by its id alone.
  none: 'by_session_id',
  compact: 'reuse_root',
  fork: 'new_root',
  new: 'new_root',
};

/**
 * Whether a session continues its root conversation or starts a new one. Turning `reuse_root` into
 * an actual `conversation_id` is capture's work, not this module's.
 */
export function conversationPolicy(input: {
  agent: AgentName;
  source: SessionStartSource | undefined;
  nativeSessionIdKnown: boolean;
}): ConversationDecision {
  // An unresolved agent never joins an existing conversation: the selector failed, so its session
  // id means nothing (FR-006, contracts/agents.md "Agent identity").
  if (input.agent === 'unknown') return 'new_root';
  const rule = CONVERSATION_RULES[input.source ?? 'none'];
  if (rule !== 'by_session_id') return rule;
  return input.nativeSessionIdKnown ? 'reuse_root' : 'new_root';
}

function summarizableContent(event: NormalizedEvent): string {
  switch (event.kind) {
    case 'prompt':
    case 'compaction_summary':
    case 'last_assistant_message':
      return event.text;
    case 'tool_call':
      return [event.input.command ?? '', event.input.text ?? '', ...event.input.paths].join('');
    case 'tool_result':
      return event.output;
    case 'tool_failure':
      return event.error;
    default:
      // Lifecycle kinds carry no content and never count (data-model.md sessions.summary_state).
      return '';
  }
}

/** Whether this event counts towards a session having content to summarize. */
export function isSummarizable(event: NormalizedEvent): boolean {
  return summarizableContent(event).trim() !== '';
}

// Native tool name per agent, from the committed fixtures under test/contracts/<agent>/ and the
// verified alias table of docs/research/oboete-contracts-2026-09-02.md. A name that is not listed
// is `other`; the adapters (T028) refine what the name alone cannot say, such as a Codex
// apply_patch that adds a file (write) rather than editing one.
const NATIVE_TOOL_NAMES: Record<AgentName, Readonly<Record<string, ToolName>>> = {
  claude: {
    Read: 'read',
    Write: 'write',
    Edit: 'edit',
    MultiEdit: 'edit',
    Bash: 'bash',
    Grep: 'grep',
    Glob: 'glob',
    Task: 'task',
  },
  // Codex has no read tool: reads arrive as Bash commands, writes and edits as apply_patch.
  codex: { Bash: 'bash', apply_patch: 'edit' },
  grok: {
    read_file: 'read',
    write: 'write',
    search_replace: 'edit',
    run_terminal_command: 'bash',
    spawn_subagent: 'task',
  },
  pi: { read: 'read', write: 'write', edit: 'edit', bash: 'bash', grep: 'grep', glob: 'glob' },
  unknown: {},
};

// How each agent spells an MCP tool. Pi is absent: no probe has shown Pi's MCP naming, and an
// invented pattern would mislabel an extension tool, so Pi's unlisted names stay `other`.
const MCP_TOOL_PATTERNS: Partial<Record<AgentName, RegExp>> = {
  claude: /^mcp__(.+?)__(.+)$/,
  codex: /^mcp__(.+?)__(.+)$/,
  grok: /^(.+?)__(.+)$/,
};

export function normalizeToolName(agent: AgentName, native: string): ToolName {
  const table = NATIVE_TOOL_NAMES[agent];
  // The native name comes from an agent payload, so an inherited key such as `constructor` must
  // not be read as a mapping (FR-004: an event never dictates what oboete records).
  const mapped = Object.hasOwn(table, native) ? table[native] : undefined;
  if (mapped !== undefined) return mapped;
  const match = MCP_TOOL_PATTERNS[agent]?.exec(native);
  if (match) {
    const name = `mcp:${match[1]}/${match[2]}`;
    // A server or tool name carrying a slash would forge a normalized name, so it stays `other`.
    if (isToolName(name)) return name;
  }
  return 'other';
}
