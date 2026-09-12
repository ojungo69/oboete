import { spawnSync } from 'node:child_process';
import { existsSync, realpathSync, statSync } from 'node:fs';
import { hostname, type as osType, cpus, release, arch } from 'node:os';
import { dirname, join, resolve } from 'node:path';

import { CAPTURE_DEADLINE_MS, INJECTION_DEADLINE_MS } from '../capture.js';
import { compareCodeUnits } from '../hash.js';
import { DEGRADED_SENTENCES } from '../injection/pack-format.js';
import type { ReportComputed } from './replay-evaluate.js';
import type { MeasureInput, RecallHit, Sample } from './replay.js';

export const READY_BOUND_MS = 300;
// Current-work capture and privacy checks share the session-start 1300 ms deadline.
export const PENDING_BOUND_MS = INJECTION_DEADLINE_MS;

export const HEADING = '## Fixture replay (T068)';
const SUMMARY_PENDING = DEGRADED_SENTENCES.summary_pending;

export type BoundRow = { sc: string; measured: string; bound: string; status: 'pass' | 'fail' };

export function repositoryRoot(): string {
  const fromArgv = process.argv[1] === undefined ? undefined : resolve(process.argv[1]);
  if (fromArgv !== undefined) {
    const dir = dirname(fromArgv);
    const parent = resolve(dir, '..');
    if (existsSync(join(parent, 'package.json')) && existsSync(join(dir, 'oboete.mjs'))) return parent;
  }
  let directory = process.cwd();
  for (;;) {
    if (existsSync(join(directory, 'package.json'))) return directory;
    const parent = dirname(directory);
    if (parent === directory) {
      throw new Error('could not find repository root: no package.json above cwd or the engine bundle');
    }
    directory = parent;
  }
}

/** The engine next to the launcher that was run. `dist/oboete.mjs` is a couple of kilobytes of
 *  launcher (src/launcher.mjs, issue #210), most of it comment, so its size says nothing about the
 *  build; a pre-split bundle has no sibling and is its own engine. `realpathSync` because a global
 *  install runs a symlinked bin and the engine sits beside the real file. */
export function enginePath(bundle: string): string {
  try {
    const engine = join(dirname(realpathSync(bundle)), 'engine.mjs');
    return existsSync(engine) ? engine : bundle;
  } catch {
    // `fileBytes` below renders a missing file as 0 bytes rather than throwing; rendering a report
    // is not where a vanished bundle should surface.
    return bundle;
  }
}

export function fileBytes(path: string): number {
  try {
    return statSync(path).size;
  } catch {
    return 0;
  }
}

export function percentile(values: number[], p: number): number {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const index = (p / 100) * (sorted.length - 1);
  const lo = Math.floor(index);
  const hi = Math.ceil(index);
  const left = sorted[lo];
  const right = sorted[hi];
  if (left === undefined) return 0;
  if (right === undefined || lo === hi) return left;
  return left + (right - left) * (index - lo);
}

export function ms(value: number): string {
  return value.toFixed(1);
}

function gitHead(cwd: string): string {
  const result = spawnSync('git', ['-C', cwd, 'rev-parse', '--short', 'HEAD'], { encoding: 'utf8' });
  return result.status === 0 ? result.stdout.trim() : 'unknown';
}

function mdTable(headers: string[], right: boolean[], rows: string[][]): string {
  const head = `| ${headers.join(' | ')} |`;
  const rule = `|${right.map((isRight) => (isRight ? '---:' : '---')).join('|')}|`;
  const body = rows.map((row) => `| ${row.join(' | ')} |`).join('\n');
  return `${head}\n${rule}\n${body}`;
}

function mdCell(text: string): string {
  const trimmed = text.trim();
  if (trimmed === '') return '—';
  return trimmed.replaceAll('|', String.raw`\|`);
}

function groupKey(sample: Sample): string {
  return `${sample.agent}\t${sample.event}`;
}

export function timingRows(
  samples: Sample[],
  boundFor: (sample: Sample) => number,
  requiredFraction = 0.99,
): { rows: string[][]; pass: boolean; worstGroup: string } {
  const groups = new Map<string, Sample[]>();
  for (const sample of samples) {
    const key = groupKey(sample);
    const list = groups.get(key) ?? [];
    list.push(sample);
    groups.set(key, list);
  }
  const rows: string[][] = [];
  let pass = samples.length > 0;
  let worstP99 = -1;
  let worstGroup = 'n/a';
  const rowOf = (labelAgent: string, labelEvent: string, group: Sample[]): string[] => {
    const values = group.map((sample) => sample.ms);
    const boundMs = Math.max(...group.map(boundFor));
    const p99 = percentile(values, 99);
    const under = group.filter((sample) => sample.ms <= boundFor(sample)).length;
    const passed = under / group.length >= requiredFraction && p99 <= boundMs;
    if (labelAgent !== 'all') {
      pass = pass && passed;
      if (p99 > worstP99) {
        worstP99 = p99;
        worstGroup = `${labelAgent}/${labelEvent} p99 ${ms(p99)} ms`;
      }
    }
    return [
      labelAgent,
      labelEvent,
      String(values.length),
      ms(percentile(values, 50)),
      ms(percentile(values, 95)),
      ms(p99),
      ms(Math.max(...values)),
      `${boundMs} ms`,
      statusOf(passed),
    ];
  };
  for (const [key, group] of [...groups.entries()].sort(([left], [right]) => compareCodeUnits(left, right))) {
    const [agent, event] = key.split('\t');
    rows.push(rowOf(agent ?? '', event ?? '', group));
  }
  if (samples.length > 0) rows.push(rowOf('all', '*', samples));
  return { rows, pass, worstGroup };
}

export function statusOf(pass: boolean): 'pass' | 'fail' {
  return pass ? 'pass' : 'fail';
}

/** A pending sample must carry the sentence in the pack belonging to its persisted injection. */
export function pendingSentence(
  input: { packs: { seq: number; text: string; injectionIds: string[] }[] },
  samples: Sample[],
): { hits: number; text: string } {
  const hits = samples.filter((sample) => {
    const pack = sample.injectionId === undefined ? '' :
      input.packs.find((entry) => entry.injectionIds.includes(sample.injectionId!))?.text ?? '';
    return pack.includes(SUMMARY_PENDING);
  }).length;
  return { hits, text: `${hits}/${samples.length} packs carry summary_pending` };
}

/** The share of recall probes whose fact came back; an empty set counts as a pass. */
export function recallRateOf(rows: RecallHit[]): number {
  return rows.length === 0 ? 1 : rows.filter((row) => row.hit).length / rows.length;
}

/** The capture, injection, session-start wait and size tables. */
function timingTables(
  input: MeasureInput,
  computed: ReportComputed,
) {
  const { injectionTiming, pending, pendingPass, readyPass } = computed;
  const captureTable = mdTable(
    ['Agent', 'Event', 'n', 'p50 ms', 'p95 ms', 'p99 ms', 'max ms', 'Bound', 'Status'],
    [false, false, true, true, true, true, true, false, false],
    timingRows(input.captureSamples, () => CAPTURE_DEADLINE_MS).rows,
  );
  const injectionTable = mdTable(
    ['Agent', 'Event', 'n', 'p50 ms', 'p95 ms', 'p99 ms', 'max ms', 'Bound', 'Status'],
    [false, false, true, true, true, true, true, false, false],
    injectionTiming.rows,
  );
  const waitRows: string[][] = [];
  const pushWait = (label: string, samples: Sample[], bound: number, sentence: string, status: string): void => {
    if (samples.length === 0) {
      waitRows.push(['all', label, '0', 'n/a', 'n/a', 'n/a', `${bound} ms`, sentence, status]);
      return;
    }
    const values = samples.map((sample) => sample.ms);
    waitRows.push([
      'all',
      label,
      String(values.length),
      ms(percentile(values, 50)),
      ms(percentile(values, 95)),
      ms(Math.max(...values)),
      `${bound} ms`,
      sentence,
      status,
    ]);
  };
  pushWait('ready', input.readySamples, READY_BOUND_MS, pendingSentence(input, input.readySamples).text,
    input.readySamples.length === 0 ? 'n/a' : statusOf(readyPass));
  pushWait('pending', input.pendingSamples, PENDING_BOUND_MS, pending.text, statusOf(pendingPass));
  const waitTable = mdTable(
    ['Agent', 'Path', 'n', 'p50 ms', 'p95 ms', 'max ms', 'Bound', 'summary_pending', 'Status'],
    [false, false, true, true, true, true, false, false, false],
    waitRows,
  );
  const sizeTable = mdTable(
    ['seq', 'Agent', 'Event', 'tag', 'FILL JSON bytes', 'wall ms', 'classification_state', 'truncated'],
    [true, false, false, false, true, true, false, true],
    input.sizeRows.map((row) => [
      String(row.seq),
      row.agent,
      row.event,
      row.tag,
      String(row.fillBytes),
      ms(row.ms),
      row.classification,
      String(row.truncated),
    ]),
  );
  return { captureTable, injectionTable, waitTable, sizeTable };
}

/** The recall misses and the SC verdict table. */
function recallTables(computed: ReportComputed, bounds: BoundRow[]) {
  const { misses } = computed;
  const missTable =
    misses.length === 0
      ? 'None.'
      : mdTable(
          ['fact id', 'lang', 'query', 'first failure', 'reason'],
          [false, false, false, false, false],
          misses.map((row) => [row.id, row.lang, mdCell(row.query), row.firstFailure ?? 'none',
            row.firstFailure === null ? 'none' : row.stages[row.firstFailure as keyof typeof row.stages].reason]),
        );
  const scTable = mdTable(
    ['SC', 'Measured', 'Bound', 'Status'],
    [false, false, false, false],
    bounds.map((row) => [row.sc, row.measured, row.bound, row.status]),
  );
  return { missTable, scTable };
}

/** The hook exits, the lifecycle checks, and the compaction summaries. */
function lifecycleTables(input: MeasureInput, computed: ReportComputed) {
  const { compactionSummaries, lifecycleRows } = computed;
  const hookExitTable =
    input.hookFailures.length === 0
      ? `All ${input.hookCount} capture and injection hooks exited 0 (none killed, none timed out).`
      : mdTable(
          ['seq', 'Agent', 'Event', 'status', 'stderr'],
          [true, false, false, false, false],
          input.hookFailures.map((row) => [
            String(row.seq),
            row.agent,
            row.event,
            row.status,
            mdCell(row.stderr),
          ]),
        );
  const lifecycleTable = mdTable(
    ['check', 'n', 'pass/fail', 'offending sessions'],
    [false, true, false, false],
    lifecycleRows.map((row) => [
      row.check,
      String(row.n),
      statusOf(row.pass),
      row.offenders.length === 0 ? '—' : row.offenders.join(', '),
    ]),
  );
  const compactSummaryTable =
    compactionSummaries.length === 0
      ? 'No `compaction_summary` rows.'
      : mdTable(
          ['agent', 'native_session_id', 'classification_state'],
          [false, false, false],
          compactionSummaries.map((row) => [
            String(row.agent),
            String(row.native_session_id),
            String(row.classification_state),
          ]),
        );
  return { hookExitTable, lifecycleTable, compactSummaryTable };
}

/** The heading and how this run was set up. */
function setupSection(input: MeasureInput, machine: string, cpu: string): string[] {
  return [
    HEADING,
    '',
    '### Setup',
    '',
    `- Date: ${input.startedAt}`,
    `- Machine: \`${machine}\`.`,
    `- CPU: \`${cpu}\`.`,
    `- Node: \`${process.execPath}\` (${process.version}).`,
    `- Commit: \`${gitHead(repositoryRoot())}\`.`,
    `- Bundle: \`${enginePath(input.bundle)}\`, ${fileBytes(enginePath(input.bundle))} bytes, run through \`${input.bundle}\`.`,
    `- Fixture: \`${input.fixturePath}\` (${input.lines.length} lines).`,
    `- \`OBOETE_HOME\`: \`${input.home}\`. Worker behavior uses this home's configuration; generation and delivery are scored separately below.`,
    `- Temporary git repository with one empty commit so \`HEAD\` exists. \`NODE_ENV=test\`.`,
    `- Worker RSS: Linux \`/proc/<pid>/status\` \`VmHWM\`, polled every 50 ms. Replay holds a fenced lease during capture and starts its own worker only when ended targets can drain; the child must exit before measurement. Automatic native-agent spawning is a separate qualification.`,
    '',
    'Commands executed:',
    '',
    '```bash',
    'npm run build',
    `node dist/oboete.mjs fixture replay ${input.fixturePath}`,
    '```',
    '',
    `Load average at the start of the run: \`${input.loadAtStart}\``,
    '',
  ];
}

/** The capture, injection and session-start tables with the text that reads them. */
function hookTimingSection(
  input: MeasureInput,
  computed: ReportComputed,
  tables: { captureTable: string; injectionTable: string; waitTable: string; sizeTable: string },
): string[] {
  const { readyMax, readyPass, pendingMax, pendingPass } = computed;
  const { captureTable, injectionTable, waitTable, sizeTable } = tables;
  return [
    '### SC-002 capture time',
    '',
    `Capture-only hooks (\`hookDeadlineMs\` ≠ \`INJECTION_DEADLINE_MS\`). Bound ${CAPTURE_DEADLINE_MS} ms. Row status is informational: p99 ≤ bound and ≥99% of samples ≤ bound. SC-002 is judged on the pooled capture sample.`,
    '',
    captureTable,
    '',
    '### Injection hooks',
    '',
    `Every (agent, event) group must pass: every ordinary injection and ready start is bounded by ${READY_BOUND_MS} ms. Pending session-start samples are classified by their exact persisted injection and evaluated below at ${PENDING_BOUND_MS} ms. Pi capture and explicit injection are measured separately.`,
    '',
    injectionTable,
    '',
    'Size-tagged events (FILL-only JSON byte length, then ROOT substituted). Stdin above the 256 KiB read bound is stored as `partial` / `truncated = 1`.',
    '',
    sizeTable,
    '',
    '### Session-start wait',
    '',
    `Ready and pending are read from the injection created by that start. Missing or ambiguous injection records invalidate timing evidence. Pending requires the summary-pending sentence in its correlated printed pack and wall time ≤ ${PENDING_BOUND_MS} ms; ready is bounded by ${READY_BOUND_MS} ms.`,
    '',
    waitTable,
    '',
    `Ready max ${ms(readyMax)} ms (n=${input.readySamples.length}, ${statusOf(readyPass)}). Pending max ${ms(pendingMax)} ms (n=${input.pendingSamples.length}, ${statusOf(pendingPass)}).`,
    '',
  ];
}

/** Worker memory, database growth, and the secret, directive and duplicate scans. */
function resourceSection(
  input: MeasureInput,
  computed: ReportComputed,
): string[] {
  const {
    dbBytesAfter, duplicateGroups, injectionItems, injections, leakedDirectives,
    leakedDirectivesEllipsis, leakedSecrets, memories, negativesUnredacted, perThousand,
    rawDirectiveRows, rawEvents, sc003, workerRssKb, workerRuns,
  } = computed;
  return [
    '### SC-003 worker memory and database growth',
    '',
    `- ${workerRuns}.`,
    `- Max VmHWM: ${workerRssKb} kB = ${(workerRssKb / 1024).toFixed(3)} MB (bound 150 MB, ${statusOf(sc003)}).`,
    `- \`memory.db\` + \`-wal\` before: ${input.dbBytesBefore} bytes; after: ${dbBytesAfter} bytes; delta ${dbBytesAfter - input.dbBytesBefore} bytes; ${Math.round(perThousand)} bytes per 1,000 events.`,
    `- Rows: raw_events=${rawEvents}, memories=${memories}, injections=${injections}, injection_items=${injectionItems}.`,
    '',
    '### SC-005 secret scan',
    '',
    leakedSecrets.length === 0
      ? `All ${input.maps.secretValues.length} non-null corpus secrets are absent from memory.db, memory.db-wal, spool/, logs/, and packs.`
      : `Leaked secret ids: ${leakedSecrets.join(', ')}.`,
    '',
    `Detector precision on the ${input.maps.negatives.length} \`secret = null\` negatives: ${negativesUnredacted} of their \`text\` values survived into \`memories\` unredacted (a redacted negative is a false positive, not a failure of this bound).`,
    '',
    '### Directive scan',
    '',
    leakedDirectives.length === 0
      ? `All ${input.maps.directives.length} directive phrases are absent from memories.title, memories.body, and packs.`
      : `Directive phrases in memories or packs (${leakedDirectives.length}): ${leakedDirectives.slice(0, 5).join(' | ')}${leakedDirectivesEllipsis}.`,
    '',
    `${rawDirectiveRows} raw_events.content rows still carry a directive phrase (allowed; they may remain in raw events and the spool).`,
    '',
    '### SC-010 duplicate injections',
    '',
    duplicateGroups.length === 0
      ? 'Zero `injection_items` rows with `decision = included` share the same `(conversation_id, context_epoch, memory_id)`.'
      : `${duplicateGroups.length} duplicate groups.`,
    '',
    `raw_events.id count ${rawEvents} vs lines piped ${input.lines.length}. Pi \`tool_result\` stores two kinds per line, so the id count can exceed the line count; a re-delivery would collapse onto an existing id.`,
    '',
  ];
}

/** Fact recall, the lifecycle checks, hook exits, and the bounds table. */
function recallSection(
  input: MeasureInput,
  computed: ReportComputed,
  tables: { missTable: string; scTable: string; hookExitTable: string; lifecycleTable: string; compactSummaryTable: string },
): string[] {
  const { failed, recallEn, recallJa } = computed;
  const { missTable, scTable, hookExitTable, lifecycleTable, compactSummaryTable } = tables;
  return [
    '### SC-009 fact recall',
    '',
    `Japanese ${(recallRateOf(recallJa) * 100).toFixed(1)}% (${recallJa.filter((row) => row.hit).length}/${recallJa.length}); English ${(recallRateOf(recallEn) * 100).toFixed(1)}% (${recallEn.filter((row) => row.hit).length}/${recallEn.length}); overall ${(recallRateOf([...recallJa, ...recallEn]) * 100).toFixed(1)}% (${[...recallJa, ...recallEn].filter((row) => row.hit).length}/${(recallJa.length + recallEn.length)}). Bound ≥ 90%. Availability counts a confirmed current delivery or a confirmed prior delivery in the same conversation and epoch. Receiving-agent answer quality is not measured here.`,
    '',
    'Stages (fixture replay never runs a receiving agent):',
    '',
    mdTable(['stage', 'pass', 'fail', 'pending', 'partial', 'not run'], [false, true, true, true, true, true],
      Object.entries(computed.stageCounts).map(([name, counts]) => [name, ...['pass', 'fail', 'pending', 'partial', 'not_run']
        .map((status) => String(counts[status] ?? 0))])),
    '',
    'Misses:',
    '',
    missTable,
    '',
    '### Lifecycle',
    '',
    'Each `tags.lifecycle` sequence checked against contracts/agents.md. `fork`: the forked session\'s `conversation_id` differs from the preceding session of that agent. `resume`: that SessionStart created no `injections` row and printed no pack. `compact`: at least one detector-clean (`classification_state = done`) `compaction_summary` row exists, and the conversation\'s `context_epoch` equals that row count (Claude\'s PostCompact + SessionStart(compact) pair counts once, A16). `clear`: a new session id and a new conversation. A compaction hook that misses the detector deadline stores a `failed` row and by A16 opens no epoch; 0/0 fails this check.',
    '',
    lifecycleTable,
    '',
    '`compaction_summary` rows:',
    '',
    compactSummaryTable,
    '',
    '### Hook exits',
    '',
    'Capture and injection hooks (including Pi `inject`). Bound: every process exits 0; a non-zero status, a kill signal, or a spawn timeout is a contract violation (contracts/cli.md, FR-002). Wall time of these hooks stays in the timing tables above.',
    '',
    hookExitTable,
    '',
    '### Bounds',
    '',
    scTable,
    '',
    failed
      ? 'One or more measured bounds failed. The numbers above are the run, not a softened reading.'
      : 'Every listed bound passed on this run.',
  ];
}

/** The evidence section, in the order the document reads. */
function reportMarkdown(
  input: MeasureInput,
  computed: ReportComputed,
  tables: { captureTable: string; injectionTable: string; waitTable: string; sizeTable: string; missTable: string; scTable: string; hookExitTable: string; lifecycleTable: string; compactSummaryTable: string },
): string {
  const cpu = cpus()[0]?.model ?? 'unknown';
  const machine = `${osType()} ${hostname()} ${release()} ${arch()}`;
  const { captureTable, injectionTable, waitTable, sizeTable, missTable, scTable, hookExitTable, lifecycleTable, compactSummaryTable } = tables;
  return [
    ...setupSection(input, machine, cpu),
    ...hookTimingSection(input, computed, { captureTable, injectionTable, waitTable, sizeTable }),
    ...resourceSection(input, computed),
    ...recallSection(input, computed, { missTable, scTable, hookExitTable, lifecycleTable, compactSummaryTable }),
  ].join('\n');
}

/** The timing, worker and growth halves of the machine report. */
function timingJson(
  input: MeasureInput,
  computed: ReportComputed,
): Record<string, unknown> {
  const {
    captureP99, captureUnder, captureValues, dbBytesAfter, injectionItems, injections, memories,
    pending, pendingMax, pendingPass, perThousand, rawEvents, readyMax, readyPass, sc002, sc003,
    workerRssKb,
  } = computed;
  return {
    capture: { n: captureValues.length, p99: captureP99, under: captureUnder, pass: sc002 },
    injection: { n: input.injectionSamples.length, samples: input.injectionSamples },
    sessionStart: {
      ready: { n: input.readySamples.length, max: readyMax, pass: readyPass },
      pending: { n: input.pendingSamples.length, max: pendingMax, summaryPending: pending.hits, pass: pendingPass },
    },
    worker: {
      observeRuns: input.observeRuns,
      hookWorkerRuns: input.hookWorkerRuns,
      hookWorkerRssKb: input.hookWorkerRssKb,
      rssKb: workerRssKb,
      pass: sc003,
    },
    growth: {
      before: input.dbBytesBefore,
      after: dbBytesAfter,
      perThousand,
      rawEvents,
      memories,
      injections,
      injectionItems,
    },
  };
}

/** The same evidence as machine-readable JSON. */
function reportJson(
  input: MeasureInput,
  computed: ReportComputed,
  bounds: BoundRow[],
): Record<string, unknown> {
  const {
    duplicateGroups, failed, hooksPass, leakedDirectives, leakedSecrets, lifecycleRows, misses,
    negativesUnredacted, rawDirectiveRows, rawEvents, recallEn, recallJa, sc009, sc010,
  } = computed;
  return {
    startedAt: input.startedAt,
    repoId: input.repoId,
    startSamples: input.startSamples,
    lines: input.lines.length,
    ...timingJson(input, computed),
    secrets: { leaked: leakedSecrets, negativesUnredacted },
    directives: { leaked: leakedDirectives.length, rawRows: rawDirectiveRows },
    recall: {
      ja: recallRateOf(recallJa),
      en: recallRateOf(recallEn),
      overall: recallRateOf([...recallJa, ...recallEn]),
      misses: misses.map((row) => ({ id: row.id, query: row.query })),
      pass: sc009,
      currentDelivery: computed.recallTraces.filter((row) => row.availability === 'current_delivery').length,
      priorDelivery: computed.recallTraces.filter((row) => row.availability === 'prior_delivery').length,
      stageCounts: computed.stageCounts,
      probes: computed.recallTraces.map((trace) => ({ id: trace.id, lang: trace.lang, query: trace.query,
        factSeq: trace.factSeq, querySeq: trace.querySeq, hit: trace.hit, availability: trace.availability,
        stages: trace.stages, firstFailure: trace.firstFailure })),
    },
    duplicates: { groups: duplicateGroups.length, rawEvents, lines: input.lines.length, pass: sc010 },
    hooks: { n: input.hookCount, failures: input.hookFailures.length, pass: hooksPass },
    lifecycle: lifecycleRows,
    bounds,
    failed,
  };
}

/** The evidence section and its machine form, from what computeReport measured. */
export function renderReport(
  input: MeasureInput,
  computed: ReportComputed,
  bounds: BoundRow[],
): { markdown: string; json: Record<string, unknown>; failed: boolean } {
  const timing = timingTables(input, computed);
  const finding = { ...recallTables(computed, bounds), ...lifecycleTables(input, computed) };
  return {
    markdown: reportMarkdown(input, computed, { ...timing, ...finding }),
    json: reportJson(input, computed, bounds),
    failed: computed.failed,
  };
}
