import type { DatabaseSync } from 'node:sqlite';

import { openDatabase } from './db/open.js';
import { memoryScope } from './db/queries.js';
import {
  whyReport,
  type ItemReason,
  type WhyInjection,
  type WhyItem,
} from './injection/ledger.js';
import { DEGRADED_SENTENCES } from './injection/pack.js';
import {
  invalid,
  oneArgument,
  parseCommand,
  runtimeWith,
  type MemoryCliRuntime,
} from './memories-cli.js';
import { ensureDirectories, oboetePaths, resolveHome } from './paths.js';
import { resolveRepoIdentity } from './repo-identity.js';

const ITEM_SENTENCES: Record<ItemReason, string> = {
  below_threshold: 'Its relevance score was below the threshold.',
  budget: 'The character budget was already used by higher-ranked notes.',
  duplicate_in_conversation: 'It was already handed over earlier in this conversation.',
  stale_path: 'It cites a file that no longer exists at HEAD.',
  stale_commit: 'It cites a commit that is no longer reachable.',
  retired: 'It was retired by a newer note.',
  mmr_redundant: 'It repeats a note that was already selected.',
  pinned: 'It is pinned, so it is always included.',
  summary: 'It is the summary of the previous session.',
  not_delivered: 'It could not be handed over during this turn.',
  secret_detected: 'Its text carried a secret and was dropped.',
  directive: 'Its text read as an instruction to the agent and was dropped.',
};

const STALE_NOTE = ' (stale: path or commit no longer at HEAD)';
const MATCHED_PROMPT = 'It matched the prompt.';

type SessionRef = { id: string; agent: string; native_session_id: string };

function withWhyDatabase<T>(fn: (db: DatabaseSync) => T): T {
  const paths = oboetePaths(resolveHome());
  ensureDirectories(paths);
  const opened = openDatabase({ path: paths.db, timeoutMs: 2_000 });
  try {
    return fn(opened.db);
  } finally {
    opened.db.close();
  }
}

function asSession(row: Record<string, unknown>): SessionRef {
  return {
    id: String(row.id),
    agent: String(row.agent),
    native_session_id: String(row.native_session_id),
  };
}

/** Sessions of the current repository only: another repository's ledger is "not found". */
function findSession(db: DatabaseSync, repoId: string, given: string): SessionRef[] {
  const byId = db
    .prepare('SELECT id, agent, native_session_id FROM sessions WHERE id = ? AND repo_id = ?')
    .get(given, repoId);
  if (byId !== undefined) return [asSession(byId)];
  return db
    .prepare(
      'SELECT id, agent, native_session_id FROM sessions WHERE native_session_id = ? AND repo_id = ? ORDER BY agent, id',
    )
    .all(given, repoId)
    .map((row) => asSession(row));
}

function turnOrdinals(db: DatabaseSync, sessionId: string): Map<string, number> {
  const ordinals = new Map<string, number>();
  for (const row of db.prepare('SELECT id, ordinal FROM turns WHERE session_id = ?').all(sessionId)) {
    ordinals.set(String(row.id), Number(row.ordinal));
  }
  return ordinals;
}

function iso(at: number): string {
  return new Date(at).toISOString();
}

function reasonSentence(reason: ItemReason | null): string {
  return reason === null ? MATCHED_PROMPT : ITEM_SENTENCES[reason];
}

function includedLabel(item: WhyItem): string {
  if (item.title !== null && item.title !== '') return item.title;
  if (item.sourceKind === 'raw_activity') return 'raw activity';
  if (item.sourceKind === 'session_summary') return 'session summary';
  return item.sourceKind ?? 'memory';
}

function omittedLabel(item: WhyItem): string {
  if (item.title !== null && item.title !== '') return item.title;
  return item.sourceKind ?? 'memory';
}

function itemLine(label: string, item: WhyItem, rank: boolean): string {
  const stale = item.stale ? STALE_NOTE : '';
  const prefix = rank ? `${item.rank}. ` : '';
  return `    ${prefix}${label} — ${reasonSentence(item.reason)}${stale}`;
}

function renderInjection(injection: WhyInjection, ordinals: Map<string, number>): string {
  const channel = injection.channel ?? '';
  const bits = [`epoch ${injection.contextEpoch}`];
  if (injection.turnId !== null) {
    const ordinal = ordinals.get(injection.turnId);
    if (ordinal !== undefined) bits.push(`turn ${ordinal}`);
  }
  if (injection.createdAt !== null) bits.push(`built ${iso(injection.createdAt)}`);
  if (injection.emittedAt !== null) bits.push(`delivered ${iso(injection.emittedAt)}`);
  const lines = [
    `${injection.kind} pack (${channel}) — ${injection.state}, ${bits.join(', ')}`,
  ];

  const used = injection.charsUsed ?? 0;
  const budget = injection.charBudget ?? 0;
  const trimmed = injection.items.filter(
    (item) => item.decision === 'omitted' && item.reason === 'budget',
  ).length;
  let budgetLine = `  budget: ${used} of ${budget} characters`;
  if (trimmed > 0) budgetLine += `; trimmed: ${trimmed} candidates omitted for budget`;
  lines.push(budgetLine);

  if (injection.degradedReason !== null) {
    const sentence = DEGRADED_SENTENCES[injection.degradedReason];
    lines.push(`  degraded: ${sentence} (${injection.degradedReason})`);
  }

  if (injection.deferred) {
    lines.push(
      `  deferred: delivered with tool calls (${injection.deliveryCount} deliveries)`,
    );
    injection.attempts.forEach((attempt, index) => {
      lines.push(
        `    attempt ${index + 1}: call ${attempt.tool_call_id} execution ${attempt.execution}, delivery ${attempt.delivery}, at ${iso(attempt.at)}`,
      );
    });
  }

  const included = injection.items.filter((item) => item.decision === 'included');
  const omitted = injection.items.filter((item) => item.decision === 'omitted');
  if (included.length > 0) {
    lines.push('  included:');
    for (const item of included) lines.push(itemLine(includedLabel(item), item, true));
  }
  if (omitted.length > 0) {
    lines.push('  omitted:');
    for (const item of omitted) lines.push(itemLine(omittedLabel(item), item, false));
  }
  return lines.join('\n');
}

function parseTurn(value: unknown, runtime: MemoryCliRuntime): number | null {
  if (typeof value !== 'string' || !/^\d+$/u.test(value)) {
    invalid(runtime, '--turn must be a non-negative integer.');
    return null;
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed)) {
    invalid(runtime, '--turn must be a non-negative integer.');
    return null;
  }
  return parsed;
}

/** `oboete why <session-id> [--turn N] [--json]`: the injection ledger for one session (FR-028, FR-045). */
export async function runWhy(
  argv: string[],
  overrides: Partial<MemoryCliRuntime> = {},
): Promise<number> {
  const runtime = runtimeWith(overrides);
  const parsed = parseCommand(
    argv,
    { turn: { type: 'string' }, json: { type: 'boolean' } },
    runtime,
  );
  if (parsed === null) return 2;
  const givenId = oneArgument(parsed.positionals, 'why', runtime);
  if (givenId === null) return 2;
  const json = parsed.values.json === true;
  let turn: number | undefined;
  if (parsed.values.turn !== undefined) {
    const parsedTurn = parseTurn(parsed.values.turn, runtime);
    if (parsedTurn === null) return 2;
    turn = parsedTurn;
  }

  const repoId = resolveRepoIdentity(runtime.cwd).id;
  return withWhyDatabase((db) => {
    const scope = memoryScope(db, { repoId, destination: 'injection' });
    const matches = findSession(db, repoId, givenId);
    if (matches.length === 0) {
      runtime.writeError(`Session ${givenId} was not found.\n`);
      return 1;
    }
    if (matches.length > 1) {
      const list = matches.map((row) => `  ${row.agent}: ${row.id}`).join('\n');
      return invalid(
        runtime,
        `${givenId} matches more than one session. Pass the oboete session id instead:\n${list}`,
      );
    }

    const session = matches[0];
    const injections =
      turn === undefined
        ? whyReport(db, session.id, scope)
        : whyReport(db, session.id, scope, turn);
    if (json) {
      runtime.writeOut(`${JSON.stringify({ session, injections })}\n`);
      return 0;
    }
    if (injections.length === 0) {
      runtime.writeOut(
        turn === undefined
          ? `No injection was built for session ${session.id}.\n`
          : `No injection was built for turn ${turn} of session ${session.id}.\n`,
      );
      return 0;
    }

    const ordinals = turnOrdinals(db, session.id);
    runtime.writeOut(`${injections.map((injection) => renderInjection(injection, ordinals)).join('\n\n')}\n`);
    return 0;
  });
}
