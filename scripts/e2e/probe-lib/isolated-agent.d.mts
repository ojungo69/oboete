// The part of `isolated-agent.mjs` the unit tests pin. `scripts/e2e` is outside the TypeScript
// program (`tsconfig.json` excludes it), so a `.ts` importer has nothing to check the call against
// without this. Keep it to what a `.ts` file imports; the harness itself needs no declarations.

/** The stem a run's ordered pair seeds its facts with. */
export function factStem(runId: string, from: string, to: string): string;

/** The three fact lines of one ordered pair, from that pair's stem. */
export function factSet(stem: string): [string, string, string];

/**
 * What the sending agent is asked: preserve the facts, and append them with one `printf`.
 * Throws a `TypeError` on anything but three non-empty strings, which the tuple states.
 */
export function buildFactSeedingPrompt(facts: readonly [string, string, string]): string;

/** What the receiving agent is asked: recall the fact lines already in its memory context. */
export function recallPrompt(agent: string, noCredentials: boolean): string;
