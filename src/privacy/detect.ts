import { Worker, parentPort, workerData } from 'node:worker_threads';
import { isAbsolute, relative, resolve } from 'node:path';

import { lintSource } from '@secretlint/core';
import { rules as recommendedRules } from '@secretlint/secretlint-rule-preset-recommend';
import type { SecretLintCoreConfig } from '@secretlint/types';

import { credentialValues } from '../log.js';
import { testFault } from '../testing/faults.js';

const FILTER_COMMENTS_RULE = '@secretlint/secretlint-rule-filter-comments';
const AWS_RULE = '@secretlint/secretlint-rule-aws';
const RULE_ID_PREFIX = '@secretlint/secretlint-rule-';

/**
 * The recommend preset, registered rule by rule (R4). Two deviations from the preset defaults, both
 * because the defaults fail open on captured content:
 * - the comment filter is left out, because a `secretlint-disable` comment inside captured text
 *   would switch redaction off for the rest of the payload (FR-018 redacts before the first write);
 * - the AWS rule scans access key ids, which its `enableIDScanRule` option leaves off by default.
 */
const RECOMMENDED_SECRET_RULES: SecretLintCoreConfig = {
  rules: recommendedRules
    .filter((rule) => rule.meta.id !== FILTER_COMMENTS_RULE)
    .map((rule) => ({
      id: rule.meta.id,
      rule,
      options: rule.meta.id === AWS_RULE ? { enableIDScanRule: true } : undefined,
    })),
};

export type Redaction = { rule: string; count: number };

export type DetectorInput = {
  text: string;
  /**
   * Extra strings redacted separately in the same run and returned in `texts`, one per field. The
   * capture hook (T027) needs the redacted value of every event field back where it came from:
   * storing one redacted concatenation would lose which event each string belongs to.
   */
  fields?: string[];
  paths: string[];
  repoRoot: string | null;
  secretPaths: string[];
  /**
   * The values of oboete's own credential variables (log.ts credentialValues), redacted whatever
   * they look like: the environment, not the shape, is what makes them secrets (FR-016). Defaults
   * to this process's environment; the worker passes the one it was started with.
   */
  credentialValues?: string[];
};

export type DetectorResult =
  | {
      ok: true;
      text: string;
      /** The redacted `fields`, in order; empty when the caller passed none. */
      texts: string[];
      redactions: Redaction[];
      privateRemoved: number;
      sensitivity: 'local_only' | 'secret';
      pathRule: string | null;
    }
  | { ok: false; reason: 'deadline' | 'detector_error' };

/**
 * Removes every `<private>...</private>` span (FR-019). Nested tags are one span from the first
 * open tag to its matching close tag, and an unclosed tag removes everything to the end of the
 * text. The removed text is never part of the result.
 */
export function stripPrivate(text: string): { text: string; removed: number } {
  const tag = /<\s*(\/?)\s*private\s*>/gi;
  let depth = 0;
  let kept = '';
  let cursor = 0;
  let removed = 0;

  for (let match = tag.exec(text); match !== null; match = tag.exec(text)) {
    if (match[1] !== '/') {
      if (depth === 0) kept += text.slice(cursor, match.index);
      depth += 1;
      continue;
    }
    // A close tag with no open tag before it wraps nothing, so it is not a span.
    if (depth === 0) continue;
    depth -= 1;
    if (depth === 0) {
      cursor = tag.lastIndex;
      removed += 1;
    }
  }

  // FR-019: an unclosed tag removes the rest of the text rather than keeping it unreviewed.
  if (depth > 0) return { text: kept, removed: removed + 1 };
  return { text: kept + text.slice(cursor), removed };
}

/**
 * One compiled glob token. `one` consumes a single character (a literal, `?` or a `[...]` class,
 * which keeps its regular-expression body so its semantics are unchanged); the three star tokens
 * are the parts that may consume any number of characters.
 */
type GlobToken =
  | { kind: 'one'; test: (character: string) => boolean }
  | { kind: 'segmentStar' }
  | { kind: 'anyStar' }
  | { kind: 'anyDirectories' };

export type GlobMatcher = { test(path: string): boolean };

function tokenize(glob: string): GlobToken[] {
  const tokens: GlobToken[] = [];
  // A rule without a slash matches the file name at any depth, as it does in .gitignore.
  if (!glob.includes('/')) tokens.push({ kind: 'anyDirectories' });
  for (let index = 0; index < glob.length; index += 1) {
    const character = glob[index] as string;
    if (character === '*') {
      if (glob[index + 1] === '*') {
        // `**/` is zero or more directories, so `a/**/b` matches `a/b` as well as `a/x/b`.
        if (glob[index + 2] === '/') {
          tokens.push({ kind: 'anyDirectories' });
          index += 2;
        } else {
          tokens.push({ kind: 'anyStar' });
          index += 1;
        }
      } else {
        tokens.push({ kind: 'segmentStar' });
      }
      continue;
    }
    if (character === '?') {
      tokens.push({ kind: 'one', test: (candidate) => candidate !== '/' });
      continue;
    }
    if (character === '[') {
      const end = glob.indexOf(']', index + 1);
      if (end !== -1) {
        const body = glob.slice(index + 1, end).replace(/^[!^]/, '^');
        const expression = new RegExp(`^[${body}]$`);
        tokens.push({ kind: 'one', test: (candidate) => expression.test(candidate) });
        index = end;
        continue;
      }
    }
    tokens.push({ kind: 'one', test: (candidate) => candidate === character });
  }
  return tokens;
}

/**
 * Compiles one `.oboete.toml` path rule with gitignore semantics, because that is the form the
 * rules are written in: a pattern without a slash matches the file name at any depth, `**` crosses
 * directories (`**\/x` matches `x` at the root too), and `*` and `?` never cross one.
 *
 * The rule is repository-supplied input (R4), so it is not compiled into one regular expression:
 * several wildcards in one expression make a non-matching path take exponential time, which would
 * hang the capture hook. Each token instead sweeps the path once and marks the positions it can
 * reach, so a match costs the length of the rule times the length of the path.
 */
export function compileGlob(glob: string): GlobMatcher {
  const tokens = tokenize(glob);
  return {
    test(path: string): boolean {
      let reached = new Uint8Array(path.length + 1);
      let next = new Uint8Array(path.length + 1);
      reached[0] = 1;
      for (const token of tokens) {
        next.fill(0);
        let any = false;
        if (token.kind === 'one') {
          for (let at = 0; at < path.length; at += 1) {
            if (reached[at] === 1 && token.test(path[at] as string)) {
              next[at + 1] = 1;
              any = true;
            }
          }
        } else {
          // `open` is "some earlier reachable position can still slide to here"; a `*` stops at a
          // separator, `**` does not, and `**/` lands only just after one.
          let open = false;
          for (let at = 0; at <= path.length; at += 1) {
            const afterSeparator = open && at > 0 && path[at - 1] === '/';
            if (reached[at] === 1) open = true;
            const here =
              token.kind === 'anyDirectories' ? reached[at] === 1 || afterSeparator : open;
            if (here) {
              next[at] = 1;
              any = true;
            }
            if (token.kind === 'segmentStar' && path[at] === '/') open = false;
          }
        }
        if (!any) return false;
        [reached, next] = [next, reached];
      }
      return reached[path.length] === 1;
    },
  };
}

/** FR-039: a rule is written with forward slashes, so a Windows path is compared in that form. */
function withForwardSlashes(value: string): string {
  return value.replace(/\\/g, '/');
}

/**
 * Why a rule cannot be compiled, or null when it can. A bracket expression is compiled to a
 * regular expression, so a body that is invalid there (`[z-a]`, say) throws -- and it throws
 * inside the detector, where the blanket catch turns every event carrying a path into
 * `detector_error`. The rule is therefore compiled once where the configuration is read, in the
 * form `matchSecretPath` will compile it, so a malformed rule is refused with its own reason
 * instead of disabling detection for everything (research.md R4: a malformed config fails closed).
 */
export function globRuleError(rule: string): string | null {
  try {
    compileGlob(withForwardSlashes(rule));
    return null;
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
}

/**
 * The repository path rule that this path matches, or null. The path is tested in its
 * repository-relative form (when it lies inside the repository) and in its raw form; a match makes
 * the whole event a path-rule hit, which is stored as metadata only (R4).
 */
export function matchSecretPath(
  pathValue: string,
  rules: string[],
  repoRoot: string | null,
): string | null {
  if (rules.length === 0) return null;

  const candidates = [withForwardSlashes(pathValue)];
  if (repoRoot !== null) {
    const inside = relative(resolve(repoRoot), resolve(pathValue));
    // A path outside the repository has no repository-relative form to compare.
    if (inside !== '' && !inside.startsWith('..') && !isAbsolute(inside)) {
      candidates.push(withForwardSlashes(inside));
    }
  }

  for (const rule of rules) {
    const pattern = compileGlob(withForwardSlashes(rule));
    if (candidates.some((candidate) => pattern.test(candidate))) return rule;
  }
  return null;
}

type Span = { start: number; end: number; rule: string };

// The characters a secret is made of. A reported range that ends inside such a run is extended to
// the end of the run (see redactSecrets).
const SECRET_CHARACTER = /[A-Za-z0-9+/=_-]/;

const CANDIDATE_PATTERNS = [
  /(?:api[_-]?key|token|secret|password|passwd|pwd|auth|credential|bearer)\s*[:=]\s*["']?([A-Za-z0-9+/=_.-]{16,})/gi,
  /\bBearer\s+([A-Za-z0-9+/=_.-]{16,})/g,
];

const UUID = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

function shannonEntropy(value: string): number {
  const counts = new Map<string, number>();
  for (const character of value) counts.set(character, (counts.get(character) ?? 0) + 1);
  let bits = 0;
  for (const count of counts.values()) {
    const probability = count / value.length;
    bits -= probability * Math.log2(probability);
  }
  return bits;
}

/**
 * R4: entropy decides only on candidates a credential-shaped pattern captured, and never on a bare
 * word, a UUID or a path.
 */
function isHighEntropySecret(value: string): boolean {
  if (value.length < 32) return false;
  if (value.includes('/') || value.includes('\\')) return false;
  if (UUID.test(value)) return false;
  if (/^[A-Za-z]+$/.test(value)) return false;
  if (/^[0-9a-fA-F]+$/.test(value)) return shannonEntropy(value) >= 3;
  return shannonEntropy(value) >= 4;
}

function entropySpans(text: string): Span[] {
  const spans: Span[] = [];
  for (const pattern of CANDIDATE_PATTERNS) {
    pattern.lastIndex = 0;
    for (let match = pattern.exec(text); match !== null; match = pattern.exec(text)) {
      const value = match[1] as string;
      if (!isHighEntropySecret(value)) continue;
      const start = match.index + match[0].lastIndexOf(value);
      spans.push({ start, end: start + value.length, rule: 'entropy' });
    }
  }
  return spans;
}

function replaceSpans(text: string, spans: Span[]): { text: string; hits: Redaction[] } {
  const sorted = [...spans].sort(
    (left, right) => left.start - right.start || right.end - left.end || left.rule.localeCompare(right.rule),
  );

  // Two rules can report overlapping ranges, and one marker must replace one region.
  const merged: Span[] = [];
  for (const span of sorted) {
    const last = merged[merged.length - 1];
    if (last !== undefined && span.start <= last.end) last.end = Math.max(last.end, span.end);
    else merged.push({ ...span });
  }

  // The pieces between the spans are collected in one pass, so every offset is read from the
  // original text and a payload with thousands of hits still costs one copy.
  const counts = new Map<string, number>();
  const pieces: string[] = [];
  let cursor = 0;
  for (const span of merged) {
    pieces.push(text.slice(cursor, span.start), `[REDACTED:${span.rule}]`);
    cursor = span.end;
    counts.set(span.rule, (counts.get(span.rule) ?? 0) + 1);
  }
  pieces.push(text.slice(cursor));
  const redacted = pieces.join('');

  const hits = [...counts.entries()]
    .map(([rule, count]) => ({ rule, count }))
    .sort((left, right) => left.rule.localeCompare(right.rule));
  return { text: redacted, hits };
}

/**
 * FR-018: replaces every secret with `[REDACTED:<rule>]`, first the ranges the secretlint rules
 * report and then the gated entropy candidates. Deterministic and idempotent.
 */
export async function redactSecrets(
  text: string,
  options?: { rules?: SecretLintCoreConfig },
): Promise<{ text: string; hits: Redaction[] }> {
  const linted = await lintSource({
    source: { filePath: 'oboete-event.txt', content: text, contentType: 'text', ext: '.txt' },
    options: { config: options?.rules ?? RECOMMENDED_SECRET_RULES, noPhysicFilePath: true },
  });

  const ruleSpans = linted.messages.map((message) => {
    // Some preset rules report the offset of the whole match with the length of the captured value
    // only (the AWS secret key and the npm token rules do), which would leave the tail of the
    // secret in the text. The end is extended over the rest of the secret's characters.
    let end = message.range[1];
    while (end < text.length && SECRET_CHARACTER.test(text[end] as string)) end += 1;
    return { start: message.range[0], end, rule: message.ruleId.replace(RULE_ID_PREFIX, '') };
  });

  const afterRules = replaceSpans(text, ruleSpans);
  const afterEntropy = replaceSpans(afterRules.text, entropySpans(afterRules.text));
  return {
    text: afterEntropy.text,
    hits: [...afterRules.hits, ...afterEntropy.hits].sort((left, right) =>
      left.rule.localeCompare(right.rule),
    ),
  };
}

/** FR-016: a configured credential value is redacted wherever it appears, whatever it looks like. */
function redactCredentials(
  value: string,
  credentials: readonly string[],
): { text: string; hits: Redaction[] } {
  let text = value;
  let count = 0;
  for (const credential of credentials) {
    const parts = text.split(credential);
    count += parts.length - 1;
    text = parts.join('[REDACTED:oboete-credential]');
  }
  return { text, hits: count === 0 ? [] : [{ rule: 'oboete-credential', count }] };
}

/** One entry per rule with the counts of every text summed, ordered by rule name. */
function mergeRedactions(hits: Redaction[]): Redaction[] {
  const counts = new Map<string, number>();
  for (const hit of hits) counts.set(hit.rule, (counts.get(hit.rule) ?? 0) + hit.count);
  return [...counts.entries()]
    .map(([rule, count]) => ({ rule, count }))
    .sort((left, right) => left.rule.localeCompare(right.rule));
}

/**
 * The detector in the R4 order: private strip, repository path rules, secretlint plus gated
 * entropy. Any failure fails closed, and the unsanitized text is never part of the result.
 * `options` exists so a test can supply a rule set; capture calls this with one argument.
 */
export async function detectSync(
  input: DetectorInput,
  options?: { rules?: SecretLintCoreConfig },
): Promise<DetectorResult> {
  try {
    const stripped = stripPrivate(input.text);
    const strippedFields = (input.fields ?? []).map((field) => stripPrivate(field));
    let privateRemoved = stripped.removed;
    for (const field of strippedFields) privateRemoved += field.removed;

    for (const path of input.paths) {
      const pathRule = matchSecretPath(path, input.secretPaths, input.repoRoot);
      // R4: a path-rule hit stores metadata only, so the content does not travel any further.
      if (pathRule !== null) {
        return {
          ok: true,
          text: '',
          texts: [],
          redactions: [],
          privateRemoved,
          sensitivity: 'secret',
          pathRule,
        };
      }
    }

    const credentials = input.credentialValues ?? credentialValues(process.env);
    // The configured credentials go first: a rule that redacted part of one would otherwise leave
    // the rest unmatched. A caller that passes fields only (the hook does) must not pay for linting
    // an empty string.
    const redactAll = async (value: string): Promise<{ text: string; hits: Redaction[] }> => {
      if (value === '') return { text: '', hits: [] };
      const known = redactCredentials(value, credentials);
      const found = await redactSecrets(known.text, options);
      return { text: found.text, hits: [...known.hits, ...found.hits] };
    };
    const redacted = await redactAll(stripped.text);
    const hits = [...redacted.hits];
    const texts: string[] = [];
    for (const field of strippedFields) {
      const redactedField = await redactAll(field.text);
      texts.push(redactedField.text);
      hits.push(...redactedField.hits);
    }

    return {
      ok: true,
      text: redacted.text,
      texts,
      redactions: mergeRedactions(hits),
      privateRemoved,
      // FR-017: local_only by default; a rule or entropy hit classifies the row as secret here,
      // and promotion to eligible is the worker's decision (privacy/classify.ts).
      sensitivity: hits.length > 0 ? 'secret' : 'local_only',
      pathRule: null,
    };
  } catch {
    // R4: a detector or configuration failure is a classification failure, never a stored payload.
    return { ok: false, reason: 'detector_error' };
  }
}

/**
 * Runs the detector in a `worker_threads` Worker and gives up at `cutoffMs`, so the hook's wall
 * time stays bounded even when the detector never returns (contracts/agents.md hook SLAs). A
 * terminated run is a detector failure with reason `deadline`.
 */
export function detectInWorker(
  input: DetectorInput,
  options: { cutoffMs: number; workerScript: string },
): Promise<DetectorResult> {
  return new Promise((settle) => {
    // R6 reserves the hook's stderr for the count of unstored events. The worker thread prints its
    // own warnings (Node 22.16 warns when node:sqlite loads) unless its streams are kept apart, so
    // they are piped here and dropped.
    const worker = new Worker(options.workerScript, {
      workerData: { role: 'oboete-detector', input },
      stdout: true,
      stderr: true,
    });
    worker.stdout.resume();
    worker.stderr.resume();

    let done = false;
    const finish = (result: DetectorResult): void => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      void worker.terminate();
      settle(result);
    };

    const timer = setTimeout(() => finish({ ok: false, reason: 'deadline' }), options.cutoffMs);
    worker.on('message', (message: DetectorResult) => finish(message));
    worker.on('error', () => finish({ ok: false, reason: 'detector_error' }));
    // An exit without a message means the worker died before it could answer.
    worker.on('exit', () => finish({ ok: false, reason: 'detector_error' }));
  });
}

/** The entry the engine bundle runs when it is started as the detector Worker (see src/cli.ts). */
export async function detectorWorkerMain(): Promise<void> {
  if (testFault('detector-never-returns')) {
    // Stay alive and silent so detectInWorker's cutoff terminates the thread. An idle timer, not an
    // unresolved promise: with an empty event loop the Worker would exit and the 'exit' listener
    // would answer `detector_error` instead of `deadline`.
    setInterval(() => {}, 1_000);
    return;
  }
  const { input } = workerData as { input: DetectorInput };
  parentPort?.postMessage(await detectSync(input));
}
