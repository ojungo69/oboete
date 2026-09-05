import * as z from 'zod';

export const OBSERVATION_TYPES = [
  'bugfix',
  'feature',
  'refactor',
  'change',
  'discovery',
  'decision',
  'security_alert',
  'security_note',
] as const;

export const CONCEPTS = [
  'how-it-works',
  'why-it-exists',
  'what-changed',
  'problem-solution',
  'gotcha',
  'pattern',
  'trade-off',
] as const;

export const DECISIONS = ['add', 'update', 'delete', 'noop'] as const;

export const MAX_OBSERVATIONS = 20;
export const MAX_SOURCE_EVENT_IDS = 50;
export const MAX_CITATION_LENGTH = 512;
export const MAX_PATHS = 20;
export const MAX_COMMITS = 10;
export const MAX_TITLE = 120;
export const MAX_BODY = 2000;
export const MAX_REASON = 200;
export const MAX_INPUT_CHARS = 12_000;
/** A nearby memory is context, so its body enters the input as a stub (contracts/observer.md). */
export const MAX_NEARBY_BODY = 500;
/** A prompt is never dropped and never shortened below this (contracts/observer.md "Input"). */
export const MIN_PROMPT_TEXT = 200;
export const DISPLAY_PATH_TAIL = 60;

const observationTypeSchema = z.enum(OBSERVATION_TYPES);
const conceptSchema = z.enum(CONCEPTS);
const decisionSchema = z.enum(DECISIONS);
const observerEventKindSchema = z.enum([
  'prompt',
  'tool_call',
  'tool_result',
  'tool_failure',
  'last_assistant_message',
  'compaction_summary',
]);

const citationPathSchema = z.string().max(MAX_CITATION_LENGTH);
const commitIdSchema = z
  .string()
  .max(MAX_CITATION_LENGTH)
  .regex(/^[0-9a-f]{7,64}$/);

export const observerInputSchema = z
  .object({
    repo_ref: z.string(),
    session: z
      .object({
        started_at: z.number(),
        turns: z.array(
          z
            .object({
              ordinal: z.number(),
              started_at: z.number(),
              ended_at: z.number().nullable(),
            })
            .strict(),
        ),
      })
      .strict(),
    events: z.array(
      z
        .object({
          id: z.string(),
          kind: observerEventKindSchema,
          text: z.string().optional(),
          tool_name: z.string().optional(),
          input: z.unknown().optional(),
          output: z.string().optional(),
          error: z.string().optional(),
          is_error: z.boolean().optional(),
        })
        .strict(),
    ),
    free_summaries: z
      .object({
        last_assistant_message: z.string().optional(),
        compaction_summary: z.string().optional(),
      })
      .strict(),
    nearby: z.array(
      z
        .object({
          id: z.string(),
          type: z.string(),
          title: z.string(),
          body: z.string(),
          deleted: z.boolean(),
        })
        .strict(),
    ),
    language_hint: z.enum(['ja', 'en', 'other']),
  })
  .strict();

export const observationSchema = z
  .object({
    type: observationTypeSchema,
    title: z.string().min(1).max(MAX_TITLE),
    body: z.string().max(MAX_BODY),
    concepts: z
      .array(conceptSchema)
      .max(7)
      .refine((items) => new Set(items).size === items.length, {
        message: 'concepts must be unique',
      }),
    citations: z
      .object({
        files_read: z.array(citationPathSchema).max(MAX_PATHS),
        files_modified: z.array(citationPathSchema).max(MAX_PATHS),
        commits: z.array(commitIdSchema).max(MAX_COMMITS),
      })
      .strict(),
    source_event_ids: z.array(z.string()).min(1).max(MAX_SOURCE_EVENT_IDS),
    classification: z
      .object({
        decision: decisionSchema,
        target: z.string().nullable(),
        reason: z.string().max(MAX_REASON),
      })
      .strict(),
  })
  .strict();

export const observerOutputSchema = z
  .object({
    observations: z.array(observationSchema).max(MAX_OBSERVATIONS),
  })
  .strict();

/**
 * The shape a provider actually returns: the fields and their types must be right, but the caps of
 * contracts/observer.md ("Output budget and schema caps") are applied by `trimObservation` instead
 * of demanded of the model, because "every title is trimmed to 120 characters and every body to
 * 2,000 by a deterministic order" is a trim rule, not a rejection rule.
 */
const rawObservationSchema = observationSchema.extend({
  title: z.string().min(1),
  body: z.string(),
  classification: z
    .object({
      decision: decisionSchema,
      target: z.string().nullable(),
      reason: z.string(),
    })
    .strict(),
  citations: z
    .object({
      files_read: z.array(z.unknown()),
      files_modified: z.array(z.unknown()),
      commits: z.array(z.unknown()),
    })
    .strict(),
});

const rawOutputSchema = z
  .object({ observations: z.array(rawObservationSchema).max(MAX_OBSERVATIONS) })
  .strict();

type RawObservation = z.infer<typeof rawObservationSchema>;

export const observerOutputJsonSchema = z.toJSONSchema(observerOutputSchema);

export type ObserverInput = z.infer<typeof observerInputSchema>;
export type ObserverOutput = z.infer<typeof observerOutputSchema>;
export type Observation = z.infer<typeof observationSchema>;
export type ObservationType = z.infer<typeof observationTypeSchema>;
export type Decision = z.infer<typeof decisionSchema>;

const TOOL_EVENT_KINDS = new Set([
  'tool_call',
  'tool_result',
  'tool_failure',
]);

function serializedSize(value: unknown): number {
  return JSON.stringify(value).length;
}

function isToolEventKind(kind: ObserverInput['events'][number]['kind']): boolean {
  return TOOL_EVENT_KINDS.has(kind);
}

export function validateObserverOutput(
  raw: unknown,
  input: Pick<ObserverInput, 'events' | 'nearby'>,
):
  | { ok: true; output: ObserverOutput }
  | { ok: false; reason: 'unusable_output'; detail: string } {
  const received = rawOutputSchema.safeParse(raw);
  if (!received.success) {
    return {
      ok: false,
      reason: 'unusable_output',
      detail: z.prettifyError(received.error),
    };
  }

  // The caps are applied here; what is left over is a structural fault (a missing field, a wrong
  // type, an unknown key, an empty source_event_ids, more than 20 observations).
  const parsed = observerOutputSchema.safeParse({
    observations: received.data.observations.map(trimObservation),
  });
  if (!parsed.success) {
    return {
      ok: false,
      reason: 'unusable_output',
      detail: z.prettifyError(parsed.error),
    };
  }

  const eventIds = new Set(input.events.map((event) => event.id));
  for (const [index, observation] of parsed.data.observations.entries()) {
    for (const id of observation.source_event_ids) {
      if (!eventIds.has(id)) {
        return {
          ok: false,
          reason: 'unusable_output',
          detail: `observation ${index} source_event_ids ${id}`,
        };
      }
    }
  }

  const nearbyIds = new Set(input.nearby.map((row) => row.id));
  const observations = parsed.data.observations.map((observation) => {
    let { decision, target } = observation.classification;
    const { reason } = observation.classification;
    // contracts/observer.md: unknown nearby target is add, not an error
    if (target !== null && !nearbyIds.has(target)) {
      target = null;
      decision = 'add';
    }
    // contracts/observer.md: delete only with a reason
    if (decision === 'delete' && reason.length === 0) {
      decision = 'noop';
    }
    if (
      decision === observation.classification.decision &&
      target === observation.classification.target
    ) {
      return observation;
    }
    return {
      ...observation,
      classification: { decision, target, reason },
    };
  });

  return { ok: true, output: { observations } };
}

function trimBody(body: string): string {
  if (body.length <= MAX_BODY) return body;
  const lines = body.split('\n');
  for (let keep = lines.length - 1; keep >= 1; keep -= 1) {
    const suffix = `... (+${lines.length - keep} omitted)`;
    const next = `${lines.slice(0, keep).join('\n')}\n${suffix}`;
    if (next.length <= MAX_BODY) return next;
  }
  // Not even the first line fits, so it is cut by characters: a body of one long line must keep
  // its content, not become the omission marker alone (contracts/observer.md trim order).
  const suffix = `... (+${lines.length} omitted)`;
  const head = body.slice(0, Math.max(0, MAX_BODY - suffix.length - 1));
  return head === '' ? suffix.slice(0, MAX_BODY) : `${head}\n${suffix}`;
}

export function shortenDisplayPath(path: string): string {
  if (path.length <= DISPLAY_PATH_TAIL) return path;
  return `…${path.slice(-DISPLAY_PATH_TAIL)}`;
}

const COMMIT_ID = /^[0-9a-f]{7,64}$/;

function citationPaths(values: readonly unknown[]): string[] {
  const paths: string[] = [];
  for (const value of values) {
    if (typeof value !== 'string') continue;
    paths.push(value.slice(0, MAX_CITATION_LENGTH));
    if (paths.length === MAX_PATHS) break;
  }
  return paths;
}

/** A citation that is not a commit id (`HEAD`, a branch name) is dropped, never fatal. */
function commitCitations(values: readonly unknown[]): string[] {
  const commits: string[] = [];
  for (const value of values) {
    if (typeof value !== 'string') continue;
    const id = value.toLowerCase();
    if (!COMMIT_ID.test(id)) continue;
    commits.push(id);
    if (commits.length === MAX_COMMITS) break;
  }
  return commits;
}

export function trimObservation(observation: RawObservation): Observation {
  return {
    ...observation,
    title: observation.title.slice(0, MAX_TITLE),
    body: trimBody(observation.body),
    // A verbose reason is trimmed like a title or a body; it is not a structural fault.
    classification: {
      ...observation.classification,
      reason: observation.classification.reason.slice(0, MAX_REASON),
    },
    citations: {
      files_read: citationPaths(observation.citations.files_read),
      files_modified: citationPaths(observation.citations.files_modified),
      commits: commitCitations(observation.citations.commits),
    },
  };
}

function truncateFromEnd(
  root: ObserverInput,
  value: string | undefined,
  assign: (next: string) => void,
  floor = 0,
): boolean {
  if (value === undefined || value.length <= floor) return false;
  if (serializedSize(root) <= MAX_INPUT_CHARS) return false;
  let current = value;
  let changed = false;
  while (current.length > floor && serializedSize(root) > MAX_INPUT_CHARS) {
    const over = serializedSize(root) - MAX_INPUT_CHARS;
    current = current.slice(0, Math.max(floor, current.length - Math.max(1, over)));
    assign(current);
    changed = true;
  }
  return changed;
}

/**
 * Nothing above kept the input under MAX_INPUT_CHARS, so the cap is applied without preference:
 * the nearby titles, then the nearby rows, then the turn list, and last the event texts below
 * MIN_PROMPT_TEXT and the events themselves, oldest first. FR-015 is a cap and not a preference,
 * and an input over it reaches the provider whole.
 */
function lastResort(next: ObserverInput): boolean {
  const overBudget = (): boolean => serializedSize(next) > MAX_INPUT_CHARS;
  let cut = false;
  for (const row of next.nearby) {
    if (!overBudget()) return cut;
    cut =
      truncateFromEnd(next, row.title, (value) => {
        row.title = value;
      }) || cut;
  }
  while (overBudget() && next.nearby.length > 0) {
    next.nearby.pop();
    cut = true;
  }
  // Halved rather than dropped one by one: every check re-serializes the whole input.
  while (overBudget() && next.session.turns.length > 0) {
    next.session.turns.splice(0, Math.ceil(next.session.turns.length / 2));
    cut = true;
  }
  for (const event of next.events) {
    if (!overBudget()) return cut;
    cut =
      truncateFromEnd(next, event.text, (value) => {
        event.text = value;
      }) || cut;
  }
  while (overBudget() && next.events.length > 0) {
    next.events.shift();
    cut = true;
  }
  return cut;
}

/**
 * FR-015: the input is cut to MAX_INPUT_CHARS. contracts/observer.md ("Input") states the keep
 * order - free summaries first, then prompts, then tool inputs and outputs by recency - so the cut
 * runs the other way round: tool events, then the nearby memories, then the prompts (shortened, not
 * dropped, never below MIN_PROMPT_TEXT) and the free summaries last. When even that leaves the
 * input over the cap, `lastResort` spends the remaining preferences.
 */
export function excerptInput(
  input: ObserverInput,
): { input: ObserverInput; excerpted: boolean } {
  if (serializedSize(input) <= MAX_INPUT_CHARS) {
    return { input, excerpted: false };
  }

  const next = structuredClone(input);
  let excerpted = false;
  const overBudget = (): boolean => serializedSize(next) > MAX_INPUT_CHARS;

  // Verbatim tool material is the cheapest thing to lose, oldest first.
  while (overBudget()) {
    const index = next.events.findIndex((event) => isToolEventKind(event.kind));
    if (index === -1) break;
    next.events.splice(index, 1);
    excerpted = true;
  }

  // A nearby memory is context from another session. Its body is capped and then cut, but the row
  // stays so that `classification.target` can still name it.
  if (overBudget()) {
    for (const row of next.nearby) {
      if (row.body.length <= MAX_NEARBY_BODY) continue;
      row.body = row.body.slice(0, MAX_NEARBY_BODY);
      excerpted = true;
    }
    for (const row of next.nearby) {
      if (!overBudget()) break;
      excerpted =
        truncateFromEnd(next, row.body, (value) => {
          row.body = value;
        }) || excerpted;
    }
  }

  // The session's own words next, and a prompt keeps at least MIN_PROMPT_TEXT characters.
  const byKeepOrder = [
    ...next.events.filter((event) => event.kind !== 'prompt'),
    ...next.events.filter((event) => event.kind === 'prompt'),
  ];
  for (const event of byKeepOrder) {
    if (!overBudget()) break;
    excerpted =
      truncateFromEnd(
        next,
        event.text,
        (value) => {
          event.text = value;
        },
        event.kind === 'prompt' ? MIN_PROMPT_TEXT : 0,
      ) || excerpted;
  }

  // The free summaries are the highest thing in the keep order, so they are cut last of all.
  excerpted =
    truncateFromEnd(next, next.free_summaries.compaction_summary, (value) => {
      next.free_summaries.compaction_summary = value;
    }) || excerpted;
  excerpted =
    truncateFromEnd(next, next.free_summaries.last_assistant_message, (value) => {
      next.free_summaries.last_assistant_message = value;
    }) || excerpted;

  if (overBudget()) excerpted = lastResort(next) || excerpted;

  return { input: next, excerpted };
}
