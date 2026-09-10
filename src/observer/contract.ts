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

const checkpointContextSchema = z.discriminatedUnion('state', [
  z.object({ state: z.literal('none') }).strict(),
  z.object({ state: z.literal('withheld') }).strict(),
  z.object({ state: z.literal('provided'), id: z.string(), title: z.string().max(MAX_TITLE),
    body: z.string().max(MAX_BODY) }).strict(),
]);

const checkpointSources = z.array(z.string()).min(1).max(MAX_SOURCE_EVENT_IDS);
const checkpointReason = z.string().min(1).max(MAX_REASON).refine((text) => text.trim() !== '');
const checkpointItems = z.array(z.string().min(1).max(500)).max(20);
const checkpointChoiceSchema = z.discriminatedUnion('decision', [
  z.object({ decision: z.literal('unchanged'), source_event_ids: checkpointSources, reason: checkpointReason }).strict(),
  z.object({ decision: z.literal('replace'), purpose: z.string().min(1).max(MAX_TITLE),
    constraints: checkpointItems, decisions: checkpointItems, outstanding: checkpointItems,
    source_event_ids: checkpointSources, reason: checkpointReason }).strict(),
]);
export type CheckpointChoice = z.infer<typeof checkpointChoiceSchema>;

/** One canonical display form; checkpoints are never silently trimmed or parsed back from text. */
export function checkpointText(choice: Extract<CheckpointChoice, { decision: 'replace' }>) {
  const japanese = /[\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Han}]/u.test(choice.purpose);
  const headings = japanese ? ['目的', '制約', '決定', '未完了'] : ['Purpose', 'Constraints', 'Decisions', 'Outstanding'];
  const list = (items: string[]) => items.length === 0 ? (japanese ? '記録なし' : 'None recorded.')
    : items.map((item) => `- ${item}`).join('\n');
  return { title: choice.purpose, body: [
    `${headings[0]}\n${choice.purpose}`, `${headings[1]}\n${list(choice.constraints)}`,
    `${headings[2]}\n${list(choice.decisions)}`, `${headings[3]}\n${list(choice.outstanding)}`,
  ].join('\n\n') };
}

export const checkpointSchema = checkpointChoiceSchema.refine((choice) => choice.decision === 'unchanged'
  || (choice.purpose.trim() !== '' && checkpointText(choice).body.length <= MAX_BODY),
{ message: 'checkpoint must fit completely' });

export const observerInputSchema = z
  .object({
    repo_ref: z.string(),
    checkpoint_context: checkpointContextSchema,
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
          captured_at: z.number().nullable().optional(),
          fragment: z.object({
            format: z.literal('event-json-v1'), source_hash: z.string(),
            start: z.number().int().nonnegative(), end: z.number().int().positive(),
            total: z.number().int().positive(), text: z.string(),
          }).strict().optional(),
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
          captured_at: z.number().nullable().optional(),
        })
        .strict(),
    ),
    language_hint: z.enum(['ja', 'en', 'other']),
  })
  .strict();

export const observationSchema = z
  .object({
    type: observationTypeSchema,
    visibility: z.enum(['work', 'project', 'personal_proposal']),
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
    checkpoint: checkpointSchema,
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
  .object({ observations: z.array(rawObservationSchema).max(MAX_OBSERVATIONS), checkpoint: checkpointSchema })
  .strict();

type RawObservation = z.infer<typeof rawObservationSchema>;

export const observerOutputJsonSchema = z.toJSONSchema(observerOutputSchema);

export type ObserverInput = z.infer<typeof observerInputSchema>;
export type ObserverOutput = z.infer<typeof observerOutputSchema>;
export type Observation = z.infer<typeof observationSchema>;
export type ObservationType = z.infer<typeof observationTypeSchema>;
export type Decision = z.infer<typeof decisionSchema>;


function normalizeClassification(observation: Observation, nearbyIds: Set<string>): Observation {
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
}

function foreignSourceId(observation: Observation, eventIds: Set<string>): string | null {
  for (const id of observation.source_event_ids) {
    if (!eventIds.has(id)) return id;
  }
  return null;
}

function foreignSourceDetail(observations: Observation[], eventIds: Set<string>): string | null {
  for (const [index, observation] of observations.entries()) {
    const id = foreignSourceId(observation, eventIds);
    if (id !== null) return `observation ${index} source_event_ids ${id}`;
  }
  return null;
}

type ParsedObserverOutput =
  | { ok: true; output: ObserverOutput }
  | { ok: false; reason: 'unusable_output'; detail: string };

function parseObserverOutput(raw: unknown): ParsedObserverOutput {
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
    checkpoint: received.data.checkpoint,
  });
  if (!parsed.success) {
    return {
      ok: false,
      reason: 'unusable_output',
      detail: z.prettifyError(parsed.error),
    };
  }
  return { ok: true, output: parsed.data };
}

export function validateObserverOutput(
  raw: unknown,
  input: Pick<ObserverInput, 'events' | 'nearby'> & Partial<Pick<ObserverInput, 'checkpoint_context'>>,
):
  | { ok: true; output: ObserverOutput }
  | { ok: false; reason: 'unusable_output'; detail: string } {
  const parsed = parseObserverOutput(raw);
  if (!parsed.ok) return parsed;

  const eventIds = new Set(input.events.map((event) => event.id));
  const detail = foreignSourceDetail(parsed.output.observations, eventIds);
  if (detail !== null) {
    return {
      ok: false,
      reason: 'unusable_output',
      detail,
    };
  }

  const checkpoint = parsed.output.checkpoint;
  if (checkpoint.source_event_ids.some((id) => !eventIds.has(id))
    || (input.checkpoint_context?.state === 'withheld' && checkpoint.decision !== 'unchanged')) {
    return { ok: false, reason: 'unusable_output', detail: 'checkpoint context or source was not admitted' };
  }

  const nearbyIds = new Set(input.nearby.map((row) => row.id));
  const observations = parsed.output.observations.map((observation) =>
    normalizeClassification(observation, nearbyIds),
  );

  return { ok: true, output: { observations, checkpoint } };
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
