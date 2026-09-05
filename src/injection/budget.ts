// Character budget for injection packs (FR-025, research.md R12 "Context window",
// contracts/agents.md "Injection policy shared by all agents"). No window is written here: the
// numbers are parsed from docs/research/context-windows.md, which R13 maintains, so a newly
// verified model reaches the budget by editing that document alone.
import windowsDoc from '../../docs/research/context-windows.md';
import type { AgentName } from '../events.js';

/** Tokens convert to characters at these rates (contracts/agents.md "Injection policy"). */
export const CHARS_PER_TOKEN: Record<'en' | 'cjk', number> = { en: 4, cjk: 1.5 };

/**
 * The per-value ceiling of each delivery channel (contracts/agents.md "Capture and injection per
 * agent"): the Claude Code and Grok Build hook channels clip at 10,000 characters, while Codex and
 * Pi deliver through the provider context, which has no separate ceiling.
 */
export const CHANNEL_CAPS: Record<AgentName, number | null> = {
  claude: 10_000,
  codex: null,
  grok: 10_000,
  pi: null,
  unknown: null,
};

/** How the document names each agent. `unknown` appears in no table, so its lane is blocked. */
const AGENT_LABELS: Record<AgentName, string | null> = {
  claude: 'Claude Code',
  codex: 'Codex CLI',
  grok: 'Grok Build',
  pi: 'Pi',
  unknown: null,
};

function tableCells(sectionTitle: string): string[][] {
  const section = windowsDoc.split(/^## /m).find((part) => part.startsWith(sectionTitle)) ?? '';
  return section
    .split('\n')
    .filter((line) => line.startsWith('|'))
    .map((line) => line.split('|').slice(1).map((cell) => cell.trim()));
}

function firstCode(cell: string | undefined): string | null {
  const match = /`([^`]+)`/.exec(cell ?? '');
  return match === null ? null : match[1];
}

function tokenCount(cell: string | undefined): number | null {
  const digits = (cell ?? '').replaceAll(',', '');
  return /^\d+$/.test(digits) ? Number(digits) : null;
}

/**
 * The alias rules of "Runtime id → catalog id", derived from the two ids the table shows instead of
 * from its prose: a runtime id that is a catalog id plus a trailing suffix defines that suffix as
 * the agent's strip rule (`[1m]` on Claude Code, `-build` on Grok Build).
 */
const ALIAS_SUFFIXES = new Map<string, string>();
for (const cells of tableCells('Runtime id')) {
  const runtime = firstCode(cells[1]);
  const catalog = firstCode(cells[2]);
  if (runtime === null || catalog === null) continue;
  if (runtime !== catalog && runtime.startsWith(catalog)) {
    ALIAS_SUFFIXES.set(cells[0], runtime.slice(catalog.length));
  }
}

const VERIFIED_WINDOWS = new Map<string, number>();
for (const cells of tableCells('Verified windows')) {
  const model = firstCode(cells[0]);
  const tokens = tokenCount(cells[1]);
  if (model !== null && tokens !== null) VERIFIED_WINDOWS.set(model, tokens);
}

const SMALLEST_WINDOWS = new Map<string, number>();
for (const cells of tableCells('Smallest verified window per agent')) {
  const tokens = tokenCount(cells[1]);
  if (tokens !== null && cells[0] !== undefined) SMALLEST_WINDOWS.set(cells[0], tokens);
}

/** The catalog id of a runtime id, after the agent's documented alias rule. */
export function normalizeModelId(agent: AgentName, runtimeId: string): string {
  const label = AGENT_LABELS[agent];
  const suffix = label === null ? undefined : ALIAS_SUFFIXES.get(label);
  const id = runtimeId.trim();
  if (suffix === undefined || id.length <= suffix.length || !id.endsWith(suffix)) return id;
  return id.slice(0, id.length - suffix.length);
}

export type DocumentedWindow = { tokens: number; known: boolean };

/**
 * The window the budget uses, or null when the agent has no verified window at all: FR-025 then
 * ships no injection lane for it rather than guessing a value (R12, R13 gate).
 */
export function documentedWindow(
  agent: AgentName,
  runtimeId: string | undefined,
): DocumentedWindow | null {
  const label = AGENT_LABELS[agent];
  const smallest = label === null ? undefined : SMALLEST_WINDOWS.get(label);
  if (smallest === undefined) return null;

  const id = runtimeId?.trim() ?? '';
  const verified = id === '' ? undefined : VERIFIED_WINDOWS.get(normalizeModelId(agent, id));
  // FR-025: an unreported or unlisted model falls back to the agent's smallest verified window and
  // the pack carries the window_unknown degraded line.
  return verified === undefined ? { tokens: smallest, known: false } : { tokens: verified, known: true };
}

export type BudgetInput = {
  agent: AgentName;
  model: string | undefined;
  channelCap: number | null;
  contextFraction: number;
  script: 'en' | 'cjk';
};

export type CharBudget = { chars: number; windowUnknown: boolean; blocked: boolean };

/** min(channel cap, context_fraction x window x characters per token) (contracts/agents.md). */
export function charBudget(input: BudgetInput): CharBudget {
  const window = documentedWindow(input.agent, input.model);
  if (window === null) return { chars: 0, windowUnknown: false, blocked: true };

  const fromWindow = input.contextFraction * window.tokens * CHARS_PER_TOKEN[input.script];
  const cap = input.channelCap ?? Number.POSITIVE_INFINITY;
  return {
    chars: Math.floor(Math.min(cap, fromWindow)),
    windowUnknown: !window.known,
    blocked: false,
  };
}
