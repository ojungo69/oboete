#!/usr/bin/env node
// T067 — deterministic 1,000-event fixture generator (oboete M1, US3).
// Node >= 22.16, ESM, node:* only. Two runs must be byte-identical.
//
// Consumed later by scripts/fixtures/replay.mjs (T068). Do not change the line
// schema or the placeholder spellings without updating that script.
//
// Line schema (one JSON object per line):
//
// {
//   "seq": 1,                       // 1-based, strictly increasing
//   "agent": "claude" | "codex" | "grok" | "pi",
//   "event": "<native hook event name for that agent>",
//   "session": "<fixture-local session label, e.g. claude-01>",
//   "payload": { ...the native hook payload exactly as the agent sends it... },
//   "tags": {                       // all keys optional
//     "secret": "<id from test/corpus/secrets.jsonl>",
//     "directive": <0-based line index into test/corpus/directives.jsonl>,
//     "fact": { "id": "f-ja-03", "lang": "ja" | "en", "query": "<a later prompt that should recall it>", "expect": "<substring the injected pack must contain>" },
//     "lifecycle": "resume" | "compact" | "fork" | "clear",
//     "size": "at_bound" | "above_bound",
//     "recall": "<fact id>"         // on the prompt event that asks the query of that fact
//   }
// }
//
// Placeholders inside payload strings (replay expands them before piping to the hook):
//   __OBOETE_REPLAY_ROOT__  cwd, workspaceRoot, transcript_path/transcriptPath, absolute repo paths
//   __SECRET:<corpus id>__  replaced by the `text` field of that secrets.jsonl line
//   __DIRECTIVE:<index>__   replaced by the `phrase` of that directives.jsonl line
//   __FILL:<bytes>__        replaced by <bytes> bytes of JSON-safe ASCII (no " \ or controls)
//
// Byte count for size-tagged events (T068 must use this, not string.length):
//   After replacing __FILL:<n>__ with n bytes of JSON-safe ASCII in [A-Za-z0-9 .], and
//   leaving __OBOETE_REPLAY_ROOT__ as the literal 24-byte token (size events contain no
//   __SECRET: or __DIRECTIVE: tokens), Buffer.byteLength(JSON.stringify(payload)) equals
//   the bound: 1048576 (size=at_bound), 1048577 or 2097152 (size=above_bound).
//   JSON.stringify uses the default (no extra whitespace). Fill has no JSON metacharacters,
//   so one fill byte is one UTF-8 byte inside the JSON string. Replay should assert that
//   equality on FILL-only expansion, then substitute ROOT and pipe to the hook.
//
// Seed: mulberry32(0x0b0e7e43). Clock: Date.UTC(2026, 8, 6) plus 1000 ms per event.
// Never Date.now() / Math.random().

import { mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  ABOVE_ONE,
  ABOVE_TWO,
  AGENTS,
  AT_BOUND,
  FACTS,
  PI_INPUT_SOURCES,
  ROOT_PH,
  assertCoverage,
  countBy,
  directiveToken,
  expandFillOnly,
  secretToken,
} from './fixture-coverage.mjs';
const SEED = 0x0b0e7e43;
const BASE_MS = Date.UTC(2026, 8, 6);
const HERE = fileURLToPath(new URL('.', import.meta.url));
const REPO = join(HERE, '../..');
const OUT = join(REPO, 'test/fixtures/events-1000.jsonl');

const MODELS = {
  codex: 'gpt-5.6-sol',
  grok: undefined,
  pi: 'gpt-5.6-luna',
};

const TOOLS = {
  claude: ['read', 'write', 'edit', 'bash'],
  codex: ['bash-read', 'patch-add', 'patch-update', 'bash'],
  grok: ['read', 'write', 'edit', 'bash'],
  pi: ['read', 'write', 'edit', 'bash'],
};

const WORK = [
  { lang: 'en', prompt: 'Add a 50ms timeout to fetchJson in src/clients/http.ts and keep the existing retry.', file: 'src/clients/http.ts', cmd: 'git diff -- src/clients/http.ts', body: 'export async function fetchJson(url, ms = 50) {\n  return fetch(url, { signal: AbortSignal.timeout(ms) });\n}\n' },
  { lang: 'ja', prompt: 'src/db/open.ts の busy timeout は 150ms のままにして、ログ行だけ増やして。', file: 'src/db/open.ts', cmd: 'rg -n "BUSY_TIMEOUT" src/db/open.ts', body: 'const BUSY_TIMEOUT_CEILING_MS = 150;\n' },
  { lang: 'en', prompt: 'Replace the global mutex in src/worker/lease.ts with a per-repo queue.', file: 'src/worker/lease.ts', cmd: 'rg -n "lease" src/worker/lease.ts', body: 'export function acquireLease(repoId) {\n  return repoId;\n}\n' },
  { lang: 'ja', prompt: 'test/unit/capture.test.ts に stdin 切り詰めのケースを 1 本足して。', file: 'test/unit/capture.test.ts', cmd: 'npm test -- test/unit/capture.test.ts', body: "test('stdin stops at the read bound', () => {\n  assert.equal(262144, 262144);\n});\n" },
  { lang: 'en', prompt: 'Document the SessionStart source enum in docs/dev/conventions.md.', file: 'docs/dev/conventions.md', cmd: 'git status --short', body: 'SessionStart sources are agent-specific; do not invent values.\n' },
  { lang: 'ja', prompt: 'scripts/measure-cold-start.mjs の計測を 3 回平均にして。', file: 'scripts/measure-cold-start.mjs', cmd: 'node scripts/measure-cold-start.mjs', body: 'const samples = [1, 2, 3];\n' },
  { lang: 'en', prompt: 'Fix the path join in src/paths.ts so Windows separators never leak into the db.', file: 'src/paths.ts', cmd: 'rg -n "join" src/paths.ts', body: "import { join } from 'node:path';\n" },
  { lang: 'ja', prompt: 'README.md の Node 要件を 22.16 に合わせて直して。', file: 'README.md', cmd: 'sed -n "1,40p" README.md', body: 'oboete requires Node.js 22.16 or newer.\n' },
];

function mulberry32(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) >>> 0;
    t ^= t >>> 14;
    return t >>> 0;
  };
}

function loadJsonl(path) {
  return readFileSync(path, 'utf8')
    .trimEnd()
    .split('\n')
    .filter((line) => line !== '')
    .map((line) => JSON.parse(line));
}

function lineCount(text) {
  if (text === '') return 0;
  const n = text.split('\n').length - 1;
  return text.endsWith('\n') ? n : n + 1;
}

function grokEventName(pascal) {
  return pascal
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    .toLowerCase();
}

function grokTimestamp(ms, seq) {
  const iso = new Date(ms).toISOString().replace('Z', '');
  return `${iso}${String(seq).padStart(6, '0').slice(-6)}+00:00`;
}

function patchAdd(rel, content) {
  const body = content.endsWith('\n') ? content.slice(0, -1) : content;
  const plus = body.split('\n').map((line) => `+${line}`).join('\n');
  return `*** Begin Patch\n*** Add File: ${rel}\n${plus}\n*** End Patch`;
}

function patchUpdate(rel, oldText, newText) {
  return `*** Begin Patch\n*** Update File: ${rel}\n@@\n-${oldText}\n+${newText}\n*** End Patch`;
}

function cleanTags(tags) {
  if (tags === undefined || tags === null) return undefined;
  const out = {};
  for (const key of ['secret', 'directive', 'fact', 'lifecycle', 'size', 'recall']) {
    if (tags[key] !== undefined) out[key] = tags[key];
  }
  return Object.keys(out).length === 0 ? undefined : out;
}

function attachFill(payload, targetBytes, setFill) {
  let n = Math.max(0, targetBytes - 128);
  for (let i = 0; i < 12; i += 1) {
    const token = `__FILL:${n}__`;
    setFill(payload, token);
    const next = targetBytes - Buffer.byteLength(JSON.stringify(payload)) + token.length;
    if (next === n) break;
    n = next;
    if (n < 0) throw new Error(`fill underflow for target ${targetBytes}`);
  }
  setFill(payload, `__FILL:${n}__`);
  const got = Buffer.byteLength(JSON.stringify(expandFillOnly(payload)));
  if (got !== targetBytes) {
    throw new Error(`fill produced ${got} bytes, wanted ${targetBytes} (n=${n})`);
  }
  return n;
}

function createState() {
  const rng = mulberry32(SEED);
  return {
    rng,
    seq: 0,
    clock: BASE_MS,
    grokStamp: 0,
    events: [],
    sessionN: { claude: 0, codex: 0, grok: 0, pi: 0 },
    uuid() {
      const bytes = [];
      for (let i = 0; i < 16; i += 1) bytes.push(rng() & 0xff);
      bytes[6] = (bytes[6] & 0x0f) | 0x40;
      bytes[8] = (bytes[8] & 0x3f) | 0x80;
      const hex = bytes.map((b) => b.toString(16).padStart(2, '0')).join('');
      return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
    },
    hex(n) {
      let out = '';
      while (out.length < n) out += (rng() & 0xff).toString(16).padStart(2, '0');
      return out.slice(0, n);
    },
  };
}

function nextLabel(g, agent) {
  g.sessionN[agent] += 1;
  return `${agent}-${String(g.sessionN[agent]).padStart(2, '0')}`;
}

function newSession(g, agent, label = nextLabel(g, agent)) {
  const nativeId = g.uuid();
  return {
    agent,
    label,
    nativeId,
    transcript: `${ROOT_PH}/.oboete-replay/${agent}/${label}.jsonl`,
    model: MODELS[agent],
    promptId: null,
    turn: 0,
  };
}

function push(g, session, event, payload, tags) {
  g.seq += 1;
  g.clock += 1_000;
  const row = {
    seq: g.seq,
    agent: session.agent,
    event,
    session: session.label,
    payload,
  };
  const cleaned = cleanTags(tags);
  if (cleaned !== undefined) row.tags = cleaned;
  g.events.push(row);
  return row;
}

function beginTurn(g, session) {
  session.turn += 1;
  session.promptId = g.uuid();
}

function abs(rel) {
  return `${ROOT_PH}/${rel}`;
}

function claudeBase(session, eventName) {
  const payload = {
    session_id: session.nativeId,
    transcript_path: session.transcript,
    cwd: ROOT_PH,
    permission_mode: 'bypassPermissions',
    hook_event_name: eventName,
  };
  if (session.promptId !== null) payload.prompt_id = session.promptId;
  return payload;
}

function codexBase(session, eventName) {
  const payload = {
    session_id: session.nativeId,
    transcript_path: session.transcript,
    cwd: ROOT_PH,
    hook_event_name: eventName,
    model: session.model,
    permission_mode: 'bypassPermissions',
  };
  if (session.promptId !== null) payload.turn_id = session.promptId;
  return payload;
}

function grokBase(g, session, eventName) {
  const camel = grokEventName(eventName);
  g.grokStamp += 1;
  return {
    hookEventName: camel,
    sessionId: session.nativeId,
    cwd: ROOT_PH,
    workspaceRoot: `${ROOT_PH}/`,
    timestamp: grokTimestamp(BASE_MS + g.grokStamp * 1_000, g.grokStamp),
    transcriptPath: session.transcript,
    permissionMode: 'bypassPermissions',
    hook_event_name: eventName,
    session_id: session.nativeId,
    transcript_path: session.transcript,
    permission_mode: 'bypassPermissions',
  };
}

function piEnvelope(session, event, inner) {
  const envelope = {
    event,
    session_id: session.nativeId,
    cwd: ROOT_PH,
    payload: inner,
  };
  if (session.model !== undefined) envelope.model = session.model;
  if (session.promptId !== null) envelope.prompt_id = session.promptId;
  return envelope;
}

function emitSessionStart(g, session, source, tags) {
  const agent = session.agent;
  if (agent === 'claude') {
    const payload = claudeBase(session, 'SessionStart');
    payload.source = source;
    push(g, session, 'SessionStart', payload, tags);
    return;
  }
  if (agent === 'codex') {
    const payload = {
      session_id: session.nativeId,
      transcript_path: session.transcript,
      cwd: ROOT_PH,
      hook_event_name: 'SessionStart',
      model: session.model,
      permission_mode: 'bypassPermissions',
      source,
    };
    push(g, session, 'SessionStart', payload, tags);
    return;
  }
  if (agent === 'grok') {
    const payload = grokBase(g, session, 'SessionStart');
    payload.source = source;
    if (source === 'new') {
      delete payload.transcriptPath;
      delete payload.transcript_path;
    }
    push(g, session, 'SessionStart', payload, tags);
    return;
  }
  push(
    g,
    session,
    'session_start',
    piEnvelope(session, 'session_start', { type: 'session_start', reason: 'startup' }),
    tags,
  );
}

function emitPrompt(g, session, text, tags) {
  beginTurn(g, session);
  const agent = session.agent;
  if (agent === 'claude') {
    const payload = claudeBase(session, 'UserPromptSubmit');
    payload.prompt = text;
    push(g, session, 'UserPromptSubmit', payload, tags);
    return;
  }
  if (agent === 'codex') {
    const payload = codexBase(session, 'UserPromptSubmit');
    payload.prompt = text;
    push(g, session, 'UserPromptSubmit', payload, tags);
    return;
  }
  if (agent === 'grok') {
    const payload = grokBase(g, session, 'UserPromptSubmit');
    payload.promptId = session.promptId;
    payload.prompt = text;
    push(g, session, 'UserPromptSubmit', payload, tags);
    return;
  }
  push(
    g,
    session,
    'input',
    piEnvelope(session, 'input', {
      type: 'input',
      text,
      source: PI_INPUT_SOURCES[(session.turn - 1) % PI_INPUT_SOURCES.length],
    }),
    tags,
  );
}

function toolUseId(g, session) {
  if (session.agent === 'claude') return `toolu_01${g.hex(24)}`;
  if (session.agent === 'codex') return `exec-${g.uuid()}`;
  if (session.agent === 'grok') return `call-${g.uuid()}-0`;
  return `call_${g.hex(22)}|fc_${g.hex(48)}`;
}

function emitClaudeRead(g, session, spec, pre, post, tagsPre, tagsPost) {
    pre.tool_name = 'Read';
    pre.tool_input = { file_path: abs(spec.file) };
    Object.assign(post, pre);
    post.hook_event_name = 'PostToolUse';
    post.tool_response = {
      type: 'text',
      file: {
        filePath: abs(spec.file),
        content: spec.body,
        numLines: lineCount(spec.body),
        startLine: 1,
        totalLines: lineCount(spec.body),
      },
    };
    post.duration_ms = 3;
    push(g, session, 'PreToolUse', pre, tagsPre);
    push(g, session, 'PostToolUse', post, tagsPost);
}

function emitClaudeWrite(g, session, spec, pre, post, tagsPre, tagsPost) {
    pre.tool_name = 'Write';
    pre.tool_input = { file_path: abs(spec.file), content: spec.body };
    Object.assign(post, pre);
    post.hook_event_name = 'PostToolUse';
    post.tool_response = {
      type: 'create',
      filePath: abs(spec.file),
      content: spec.body,
      structuredPatch: [],
      originalFile: null,
      userModified: false,
    };
    post.duration_ms = 10;
    push(g, session, 'PreToolUse', pre, tagsPre);
    push(g, session, 'PostToolUse', post, tagsPost);
}

function emitClaudeEdit(g, session, spec, pre, post, tagsPre, tagsPost) {
    pre.tool_name = 'Edit';
    pre.tool_input = {
      file_path: abs(spec.file),
      old_string: spec.oldText,
      new_string: spec.newText,
      replace_all: false,
    };
    Object.assign(post, pre);
    post.hook_event_name = 'PostToolUse';
    post.tool_response = {
      filePath: abs(spec.file),
      oldString: spec.oldText,
      newString: spec.newText,
      originalFile: `${spec.oldText}\n`,
      structuredPatch: [
        {
          oldStart: 1,
          oldLines: 1,
          newStart: 1,
          newLines: 1,
          lines: [`-${spec.oldText}`, `+${spec.newText}`],
        },
      ],
      userModified: false,
      replaceAll: false,
    };
    post.duration_ms = 5;
    push(g, session, 'PreToolUse', pre, tagsPre);
    push(g, session, 'PostToolUse', post, tagsPost);
}

function emitClaudeBash(g, session, spec, pre, post, tagsPre, tagsPost) {
    pre.tool_name = 'Bash';
    pre.tool_input = { command: spec.cmd, description: spec.description ?? 'Run command' };
    Object.assign(post, pre);
    post.hook_event_name = 'PostToolUse';
    post.tool_response = {
      stdout: spec.body,
      stderr: '',
      interrupted: false,
      isImage: false,
      noOutputExpected: false,
    };
    post.duration_ms = 41;
    push(g, session, 'PreToolUse', pre, tagsPre);
    push(g, session, 'PostToolUse', post, tagsPost);
}

function emitClaudeTool(g, session, spec, tagsPre, tagsPost) {
  const id = toolUseId(g, session);
  const pre = claudeBase(session, 'PreToolUse');
  pre.tool_use_id = id;
  const post = claudeBase(session, 'PostToolUse');
  post.tool_use_id = id;
  if (spec.kind === 'read') {
    emitClaudeRead(g, session, spec, pre, post, tagsPre, tagsPost);
    return;
  }
  if (spec.kind === 'write') {
    emitClaudeWrite(g, session, spec, pre, post, tagsPre, tagsPost);
    return;
  }
  if (spec.kind === 'edit') {
    emitClaudeEdit(g, session, spec, pre, post, tagsPre, tagsPost);
    return;
  }
  if (spec.kind === 'bash') {
    emitClaudeBash(g, session, spec, pre, post, tagsPre, tagsPost);
    return;
  }
  pre.tool_name = spec.toolName ?? 'Read';
  pre.tool_input = spec.toolInput ?? { file_path: abs(spec.file ?? 'missing.txt') };
  const fail = claudeBase(session, 'PostToolUseFailure');
  fail.tool_name = pre.tool_name;
  fail.tool_input = pre.tool_input;
  fail.tool_use_id = id;
  fail.error = spec.error;
  fail.is_interrupt = false;
  fail.duration_ms = 8;
  push(g, session, 'PreToolUse', pre, tagsPre);
  push(g, session, 'PostToolUseFailure', fail, tagsPost);
}

function emitCodexTool(g, session, spec, tagsPre, tagsPost) {
  const id = toolUseId(g, session);
  const pre = codexBase(session, 'PreToolUse');
  pre.tool_use_id = id;
  if (spec.kind === 'bash' || spec.kind === 'bash-read') {
    pre.tool_name = 'Bash';
    pre.tool_input = { command: spec.cmd };
  } else if (spec.kind === 'patch-add') {
    pre.tool_name = 'apply_patch';
    pre.tool_input = { command: patchAdd(spec.file, spec.body) };
  } else if (spec.kind === 'patch-update') {
    pre.tool_name = 'apply_patch';
    pre.tool_input = { command: patchUpdate(spec.file, spec.oldText, spec.newText) };
  } else {
    pre.tool_name = spec.toolName ?? 'Bash';
    pre.tool_input = spec.toolInput ?? { command: spec.cmd ?? 'false' };
    const fail = codexBase(session, 'PostToolUseFailure');
    fail.tool_use_id = id;
    fail.tool_name = pre.tool_name;
    fail.tool_input = pre.tool_input;
    fail.error = spec.error;
    push(g, session, 'PreToolUse', pre, tagsPre);
    push(g, session, 'PostToolUseFailure', fail, tagsPost);
    return;
  }
  const post = { ...pre, hook_event_name: 'PostToolUse' };
  if (spec.kind === 'bash' || spec.kind === 'bash-read') post.tool_response = spec.body;
  else if (spec.kind === 'patch-add') {
    post.tool_response = `Exit code: 0\nWall time: 0 seconds\nOutput:\nSuccess. Updated the following files:\nA ${spec.file}\n`;
  } else {
    post.tool_response = `Exit code: 0\nWall time: 0 seconds\nOutput:\nSuccess. Updated the following files:\nM ${spec.file}\n`;
  }
  push(g, session, 'PreToolUse', pre, tagsPre);
  push(g, session, 'PostToolUse', post, tagsPost);
}

function emitGrokRead(g, session, spec, pre, id, tagsPre, tagsPost) {
    pre.toolName = 'read_file';
    pre.tool_name = 'read_file';
    pre.toolInput = { target_file: spec.file };
    pre.tool_input = pre.toolInput;
    const post = grokBase(g, session, 'PostToolUse');
    post.toolName = pre.toolName;
    post.tool_name = pre.tool_name;
    post.toolUseId = id;
    post.tool_use_id = id;
    post.toolInput = pre.toolInput;
    post.tool_input = pre.toolInput;
    post.toolInputTruncated = false;
    post.toolResultTruncated = false;
    post.durationMs = 0;
    post.isBackgrounded = false;
    post.duration_ms = 0;
    const result = {
      type: 'ReadFile',
      FileContent: {
        content: `1→${spec.body}`,
        content_concise: `1→${spec.body}`,
        absolute_path: abs(spec.file),
        offset: null,
        raw_output: spec.body,
        total_lines: lineCount(spec.body),
      },
    };
    post.toolResult = result;
    post.tool_response = result;
    push(g, session, 'PreToolUse', pre, tagsPre);
    push(g, session, 'PostToolUse', post, tagsPost);
}

function emitGrokWrite(g, session, spec, pre, id, tagsPre, tagsPost) {
    pre.toolName = 'write';
    pre.tool_name = 'write';
    pre.toolInput = { file_path: spec.file, content: spec.body };
    pre.tool_input = pre.toolInput;
    const post = grokBase(g, session, 'PostToolUse');
    post.toolName = 'write';
    post.tool_name = 'write';
    post.toolUseId = id;
    post.tool_use_id = id;
    post.toolInput = pre.toolInput;
    post.tool_input = pre.toolInput;
    post.toolInputTruncated = false;
    post.toolResultTruncated = false;
    post.durationMs = 2;
    post.isBackgrounded = false;
    post.duration_ms = 2;
    const result = {
      type: 'SearchReplace',
      EditsApplied: {
        old_string: '',
        new_string: spec.body,
        tool_output_for_prompt: `The file ${abs(spec.file)} has been created.`,
        tool_output_for_prompt_concise: `The file ${abs(spec.file)} has been created.`,
        absolute_path: abs(spec.file),
        edits: { details: [] },
      },
    };
    post.toolResult = result;
    post.tool_response = result;
    push(g, session, 'PreToolUse', pre, tagsPre);
    push(g, session, 'PostToolUse', post, tagsPost);
}

function emitGrokEdit(g, session, spec, pre, id, tagsPre, tagsPost) {
    pre.toolName = 'search_replace';
    pre.tool_name = 'search_replace';
    pre.toolInput = { file_path: spec.file, old_string: spec.oldText, new_string: spec.newText };
    pre.tool_input = pre.toolInput;
    const post = grokBase(g, session, 'PostToolUse');
    post.toolName = 'search_replace';
    post.tool_name = 'search_replace';
    post.toolUseId = id;
    post.tool_use_id = id;
    post.toolInput = pre.toolInput;
    post.tool_input = pre.toolInput;
    post.toolInputTruncated = false;
    post.toolResultTruncated = false;
    post.durationMs = 0;
    post.isBackgrounded = false;
    post.duration_ms = 0;
    const result = {
      type: 'SearchReplace',
      EditsApplied: {
        old_string: spec.oldText,
        new_string: spec.newText,
        tool_output_for_prompt: `The file ${spec.file} has been updated successfully.`,
        tool_output_for_prompt_concise: `The file ${spec.file} has been updated.`,
        absolute_path: abs(spec.file),
        edits: { details: [] },
      },
    };
    post.toolResult = result;
    post.tool_response = result;
    push(g, session, 'PreToolUse', pre, tagsPre);
    push(g, session, 'PostToolUse', post, tagsPost);
}

function emitGrokBash(g, session, spec, pre, id, tagsPre, tagsPost) {
    pre.toolName = 'run_terminal_command';
    pre.tool_name = 'run_terminal_command';
    pre.toolInput = { command: spec.cmd, description: spec.description ?? 'Run command' };
    pre.tool_input = pre.toolInput;
    const post = grokBase(g, session, 'PostToolUse');
    post.toolName = pre.toolName;
    post.tool_name = pre.tool_name;
    post.toolUseId = id;
    post.tool_use_id = id;
    post.toolInput = pre.toolInput;
    post.tool_input = pre.toolInput;
    post.toolInputTruncated = false;
    post.toolResultTruncated = false;
    post.durationMs = 9;
    post.isBackgrounded = false;
    post.duration_ms = 9;
    const exit = spec.kind === 'bash-fail' ? 3 : 0;
    const result = {
      type: 'Bash',
      output: Array.from(Buffer.from(spec.body, 'utf8')),
      output_for_prompt: `exit: ${exit}\n${spec.body}`,
      exit_code: exit,
      command: spec.cmd,
      truncated: false,
      signal: null,
      timed_out: false,
      description: pre.toolInput.description,
      current_dir: ROOT_PH,
      output_file: `${session.transcript.slice(0, session.transcript.lastIndexOf('/') + 1)}terminal/${id}.log`,
      total_bytes: Buffer.byteLength(spec.body),
    };
    post.toolResult = result;
    post.tool_response = result;
    push(g, session, 'PreToolUse', pre, tagsPre);
    push(g, session, 'PostToolUse', post, tagsPost);
}

function emitGrokDenied(g, session, spec, pre, id, tagsPre, tagsPost) {
    pre.toolName = 'run_terminal_command';
    pre.tool_name = 'run_terminal_command';
    pre.toolInput = { command: spec.cmd, description: spec.description ?? 'Run command' };
    pre.tool_input = pre.toolInput;
    push(g, session, 'PreToolUse', pre, tagsPre);
    const denied = grokBase(g, session, 'PermissionDenied');
    denied.toolName = 'run_terminal_command';
    denied.tool_name = 'run_terminal_command';
    denied.toolUseId = id;
    denied.tool_use_id = id;
    denied.toolInput = pre.toolInput;
    denied.tool_input = pre.toolInput;
    denied.toolInputTruncated = false;
    push(g, session, 'PermissionDenied', denied, tagsPost);
}

function emitGrokTool(g, session, spec, tagsPre, tagsPost) {
  const id = toolUseId(g, session);
  const pre = grokBase(g, session, 'PreToolUse');
  pre.toolUseId = id;
  pre.tool_use_id = id;
  pre.toolInputTruncated = false;
  if (spec.kind === 'read') {
    emitGrokRead(g, session, spec, pre, id, tagsPre, tagsPost);
    return;
  }
  if (spec.kind === 'write') {
    emitGrokWrite(g, session, spec, pre, id, tagsPre, tagsPost);
    return;
  }
  if (spec.kind === 'edit') {
    emitGrokEdit(g, session, spec, pre, id, tagsPre, tagsPost);
    return;
  }
  if (spec.kind === 'bash' || spec.kind === 'bash-fail') {
    emitGrokBash(g, session, spec, pre, id, tagsPre, tagsPost);
    return;
  }
  if (spec.kind === 'deny') {
    emitGrokDenied(g, session, spec, pre, id, tagsPre, tagsPost);
    return;
  }
  pre.toolName = 'run_terminal_command';
  pre.tool_name = 'run_terminal_command';
  pre.toolInput = { command: spec.cmd ?? 'false', description: 'Failing call' };
  pre.tool_input = pre.toolInput;
  const fail = grokBase(g, session, 'PostToolUseFailure');
  fail.toolUseId = id;
  fail.tool_use_id = id;
  fail.toolName = pre.toolName;
  fail.tool_name = pre.tool_name;
  fail.toolInput = pre.toolInput;
  fail.tool_input = pre.toolInput;
  fail.error = spec.error;
  push(g, session, 'PreToolUse', pre, tagsPre);
  push(g, session, 'PostToolUseFailure', fail, tagsPost);
}

function emitPiTool(g, session, spec, tags) {
  const id = toolUseId(g, session);
  const inner = {
    type: 'tool_result',
    toolName: spec.piName,
    toolCallId: id,
    input: spec.input,
    content: [{ type: 'text', text: spec.body }],
    isError: spec.isError === true,
  };
  if (spec.details !== undefined) inner.details = spec.details;
  push(g, session, 'tool_result', piEnvelope(session, 'tool_result', inner), tags);
}

function emitTool(g, session, spec, tagsPre, tagsPost) {
  if (session.agent === 'claude') {
    emitClaudeTool(g, session, spec, tagsPre, tagsPost);
    return;
  }
  if (session.agent === 'codex') {
    emitCodexTool(g, session, spec, tagsPre, tagsPost);
    return;
  }
  if (session.agent === 'grok') {
    emitGrokTool(g, session, spec, tagsPre, tagsPost);
    return;
  }
  const mapped = mapPiSpec(spec);
  emitPiTool(g, session, mapped, { ...tagsPre, ...tagsPost });
}

function mapPiSpec(spec) {
  if (spec.kind === 'read') {
    return { piName: 'read', input: { path: spec.file }, body: spec.body, isError: false };
  }
  if (spec.kind === 'write') {
    return {
      piName: 'write',
      input: { path: spec.file, content: spec.body },
      body: `Successfully wrote ${Buffer.byteLength(spec.body)} bytes to ${spec.file}`,
      isError: false,
    };
  }
  if (spec.kind === 'edit') {
    return {
      piName: 'edit',
      input: { path: spec.file, edits: [{ oldText: spec.oldText, newText: spec.newText }] },
      body: `Successfully replaced 1 block(s) in ${spec.file}.`,
      details: { diff: `-1 ${spec.oldText}\n+1 ${spec.newText}` },
      isError: false,
    };
  }
  if (spec.kind === 'fail' || spec.isError) {
    return {
      piName: spec.piName ?? 'read',
      input: spec.input ?? { path: spec.file ?? 'missing.txt' },
      body: spec.error ?? spec.body,
      isError: true,
    };
  }
  return { piName: 'bash', input: { command: spec.cmd }, body: spec.body, isError: false };
}

function specFor(agent, kind, work) {
  if (kind === 'read' || kind === 'bash-read') {
    return {
      kind: agent === 'codex' ? 'bash-read' : 'read',
      file: work.file,
      cmd: `cat ${work.file}`,
      body: work.body,
    };
  }
  if (kind === 'write' || kind === 'patch-add') {
    return {
      kind: agent === 'codex' ? 'patch-add' : 'write',
      file: work.file,
      body: work.body,
    };
  }
  if (kind === 'edit' || kind === 'patch-update') {
    const oldText = work.oldText ?? 'alpha';
    const newText = work.newText ?? 'beta';
    return {
      kind: agent === 'codex' ? 'patch-update' : 'edit',
      file: work.file,
      oldText,
      newText,
      body: work.body,
    };
  }
  return {
    kind: 'bash',
    cmd: work.cmd,
    description: 'Run developer command',
    body: work.stdout ?? 'ok\n',
  };
}

function emitStop(g, session, text) {
  const agent = session.agent;
  if (agent === 'claude') {
    const payload = claudeBase(session, 'Stop');
    payload.stop_hook_active = false;
    payload.last_assistant_message = text;
    push(g, session, 'Stop', payload);
    return;
  }
  if (agent === 'codex') {
    push(g, session, 'Stop', codexBase(session, 'Stop'));
    return;
  }
  if (agent === 'grok') {
    const payload = grokBase(g, session, 'Stop');
    payload.promptId = session.promptId;
    payload.reason = 'end_turn';
    payload.stopHookActive = false;
    payload.lastAssistantMessage = text;
    payload.backgroundTasks = [];
    payload.sessionCrons = [];
    push(g, session, 'Stop', payload);
    return;
  }
  push(
    g,
    session,
    'agent_settled',
    piEnvelope(session, 'agent_settled', { type: 'agent_settled', text }),
  );
}

function emitEnd(g, session, reason) {
  const agent = session.agent;
  if (agent === 'claude') {
    const payload = claudeBase(session, 'SessionEnd');
    payload.reason = reason ?? 'prompt_input_exit';
    push(g, session, 'SessionEnd', payload);
    return;
  }
  if (agent === 'codex') {
    const payload = codexBase(session, 'SessionEnd');
    payload.reason = reason ?? 'stop';
    push(g, session, 'SessionEnd', payload);
    return;
  }
  if (agent === 'grok') {
    const payload = grokBase(g, session, 'SessionEnd');
    payload.reason = reason ?? 'shutdown';
    push(g, session, 'SessionEnd', payload);
    return;
  }
  push(
    g,
    session,
    'session_shutdown',
    piEnvelope(session, 'session_shutdown', {
      type: 'session_shutdown',
      reason: reason ?? 'quit',
    }),
  );
}

function emitClaudeCompact(g, session, tags) {
    emitSessionStart(g, session, 'compact', tags);
    const payload = claudeBase(session, 'PostCompact');
    payload.trigger = 'auto';
    payload.compact_summary = 'The session read project files and applied a small edit.';
    push(g, session, 'PostCompact', payload);
}

function emitCodexCompact(g, session, tags) {
    const post = {
      session_id: session.nativeId,
      turn_id: session.promptId ?? g.uuid(),
      transcript_path: session.transcript,
      cwd: ROOT_PH,
      hook_event_name: 'PostCompact',
      model: session.model,
      trigger: 'manual',
    };
    push(g, session, 'PostCompact', post, tags);
    emitSessionStart(g, session, 'compact');
}

function emitGrokCompact(g, session, tags) {
    const first = grokBase(g, session, 'PostCompact');
    first.source = 'auto';
    push(g, session, 'PostCompact', first, tags);
    const second = grokBase(g, session, 'PostCompact');
    second.source = 'auto';
    push(g, session, 'PostCompact', second);
}

function emitPiCompact(g, session, tags) {
  const id = g.hex(8);
  push(
    g,
    session,
    'session_compact',
    piEnvelope(session, 'session_compact', {
      type: 'session_compact',
      compactionEntry: {
        type: 'compaction',
        id,
        summary: 'Read project files and continued the current turn.',
        timestamp: new Date(g.clock + 1_000).toISOString(),
      },
      reason: 'threshold',
      willRetry: false,
    }),
    tags,
  );
}

function emitCompact(g, session) {
  const tags = { lifecycle: 'compact' };
  if (session.agent === 'claude') {
    emitClaudeCompact(g, session, tags);
    return;
  }
  if (session.agent === 'codex') {
    emitCodexCompact(g, session, tags);
    return;
  }
  if (session.agent === 'grok') {
    emitGrokCompact(g, session, tags);
    return;
  }
  emitPiCompact(g, session, tags);
}

function assistantLine(work) {
  return work.lang === 'ja'
    ? `${work.file} を更新した。次はテストを回す。`
    : `Updated ${work.file}. Next step is to run the tests.`;
}

function workTurnKind(toolKind, extra) {
  let kind = toolKind;
  if (extra.secretPlace === 'input') kind = 'bash';
  if (
    extra.secretPlace === 'output' ||
    extra.directivePlace === 'output' ||
    extra.factPlace === 'output'
  ) {
    // Write/edit results are a path or a fixed success string. Adapter output is
    // tool_response / output_for_prompt / Pi content, so force a tool that stores spec.body there.
    if (kind !== 'bash' && kind !== 'bash-read' && kind !== 'read') kind = 'read';
  }
  return kind;
}

function workTurnTags(extra) {
  const promptTags = {};
  const outputTags = {};
  const inputTags = {};
  if (extra.secretPlace === 'prompt') promptTags.secret = extra.secret;
  if (extra.secretPlace === 'output') outputTags.secret = extra.secret;
  if (extra.secretPlace === 'input') inputTags.secret = extra.secret;
  if (extra.directivePlace === 'prompt') promptTags.directive = extra.directive;
  if (extra.directivePlace === 'output') outputTags.directive = extra.directive;
  if (extra.fact !== undefined && extra.factPlace === 'prompt') promptTags.fact = extra.fact;
  if (extra.fact !== undefined && extra.factPlace === 'output') outputTags.fact = extra.fact;
  if (extra.recall !== undefined) promptTags.recall = extra.recall;
  return { promptTags, outputTags, inputTags };
}

function emitWorkPrompt(g, session, work, extra, promptTags) {
  let promptText = extra.promptText ?? work.prompt;
  if (extra.secretPlace === 'prompt') promptText = `${promptText}\n${secretToken(extra.secret)}`;
  if (extra.directivePlace === 'prompt') {
    promptText = `${promptText}\n${directiveToken(extra.directive)}`;
  }
  if (extra.fact !== undefined && extra.factPlace === 'prompt') {
    promptText = `${promptText}\n${statementOf(extra.fact)}`;
  }
  emitPrompt(g, session, promptText, promptTags);
}

function emitClaudeReadFailure(g, session, work, outputTags) {
  emitTool(
      g,
      session,
      {
        kind: 'fail',
        toolName: 'Read',
        file: 'missing-probe-file-does-not-exist.txt',
        error: `File does not exist. Note: your current working directory is ${ROOT_PH}.`,
      },
      undefined,
      outputTags,
    );
  emitStop(g, session, assistantLine(work));
}

function emitCodexBashFailure(g, session, work, outputTags) {
  emitTool(
      g,
      session,
      { kind: 'fail', cmd: 'false', error: 'Exit code 1\ncommand failed' },
      undefined,
      outputTags,
    );
  emitStop(g, session, assistantLine(work));
}

function emitGrokDeniedTurn(g, session, work, outputTags) {
  emitTool(g, session, { kind: 'deny', cmd: 'echo perm-probe', body: '' }, undefined, outputTags);
  emitStop(g, session, assistantLine(work));
}

function emitGrokExitFailure(g, session, work, outputTags) {
  emitTool(
      g,
      session,
      { kind: 'bash-fail', cmd: "bash -c 'echo boom >&2; exit 3'", body: 'boom\n' },
      undefined,
      outputTags,
    );
  emitStop(g, session, assistantLine(work));
}

function emitGrokPostFailure(g, session, work, outputTags) {
  emitTool(
      g,
      session,
      { kind: 'fail', cmd: 'false', error: 'tool handler crashed' },
      undefined,
      outputTags,
    );
  emitStop(g, session, assistantLine(work));
}

function emitPiFailure(g, session, work, outputTags) {
  emitTool(
      g,
      session,
      {
        kind: 'fail',
        piName: 'read',
        file: 'missing.txt',
        error: 'ENOENT: no such file or directory, open missing.txt',
        isError: true,
      },
      undefined,
      outputTags,
    );
  emitStop(g, session, assistantLine(work));
}

function applyWorkTurnExtras(spec, extra) {
  if (extra.secretPlace === 'output') spec.body = `${spec.body}\n${secretToken(extra.secret)}`;
  if (extra.secretPlace === 'input') {
    spec.cmd = `${spec.cmd} # ${secretToken(extra.secret)}`;
  }
  if (extra.directivePlace === 'output') {
    spec.body = `${spec.body}\n${directiveToken(extra.directive)}`;
  }
  if (extra.fact !== undefined && extra.factPlace === 'output') {
    spec.body = `${spec.body}\n${statementOf(extra.fact)}`;
  }
}

function emitWorkTurn(g, session, work, toolKind, extra = {}) {
  const kind = workTurnKind(toolKind, extra);
  const { promptTags, outputTags, inputTags } = workTurnTags(extra);
  emitWorkPrompt(g, session, work, extra, promptTags);

  if (extra.fail === 'claude-read') {
    emitClaudeReadFailure(g, session, work, outputTags);
    return;
  }
  if (extra.fail === 'codex-bash') {
    emitCodexBashFailure(g, session, work, outputTags);
    return;
  }
  if (extra.fail === 'grok-deny') {
    emitGrokDeniedTurn(g, session, work, outputTags);
    return;
  }
  if (extra.fail === 'grok-exit') {
    emitGrokExitFailure(g, session, work, outputTags);
    return;
  }
  if (extra.fail === 'grok-post-fail') {
    emitGrokPostFailure(g, session, work, outputTags);
    return;
  }
  if (extra.fail === 'pi-error') {
    emitPiFailure(g, session, work, outputTags);
    return;
  }

  const spec = specFor(session.agent, kind, work);
  applyWorkTurnExtras(spec, extra);
  emitTool(
    g,
    session,
    spec,
    Object.keys(inputTags).length === 0 ? undefined : inputTags,
    Object.keys(outputTags).length === 0 ? undefined : outputTags,
  );
  emitStop(g, session, extra.assistant ?? assistantLine(work));
}

function factTag(fact) {
  return { id: fact.id, lang: fact.lang, query: fact.query, expect: fact.expect };
}

function statementOf(tag) {
  const row = FACTS.find((fact) => fact.id === tag.id);
  if (row === undefined) throw new Error(`unknown fact ${tag.id}`);
  return row.statement;
}

function defaultStartSource(agent) {
  return agent === 'grok' ? 'new' : 'startup';
}

function emitSmokeSession(g, agent) {
  const session = newSession(g, agent);
  emitSessionStart(g, session, defaultStartSource(agent));
  const workA =
    agent === 'codex' || agent === 'pi'
      ? WORK[1]
      : WORK[0];
  const workB = agent === 'codex' || agent === 'pi' ? WORK[3] : WORK[2];
  emitWorkTurn(g, session, workA, TOOLS[agent][0]);
  emitWorkTurn(g, session, workB, TOOLS[agent][1]);
  emitWorkTurn(g, session, WORK[4], TOOLS[agent][2]);
  emitWorkTurn(g, session, WORK[5], TOOLS[agent][3]);
  emitEnd(g, session);
  return session;
}

function emitSeedSession(g, agent, facts) {
  const session = newSession(g, agent);
  emitSessionStart(g, session, defaultStartSource(agent));
  for (let i = 0; i < facts.length; i += 1) {
    const fact = facts[i];
    const work = WORK[(i + (fact.lang === 'ja' ? 1 : 0)) % WORK.length];
    const toolKind = TOOLS[agent][i % TOOLS[agent].length];
    const factPlace = i % 2 === 0 ? 'prompt' : 'output';
    emitWorkTurn(g, session, work, toolKind, { fact: factTag(fact), factPlace });
  }
  emitEnd(g, session);
  return session;
}

function emitRecallSession(g, agent, recalls, extras = {}) {
  const secrets = extras.secrets ?? [];
  const dirPrompts = extras.dirPrompts ?? [];
  const dirOutputs = extras.dirOutputs ?? [];
  const n = Math.max(
    recalls.length,
    secrets.length,
    dirPrompts.length + dirOutputs.length,
    4,
  );
  const turns = [];
  for (let i = 0; i < n; i += 1) {
    const extra = {};
    const fact = recalls[i];
    if (fact !== undefined) {
      extra.promptText = fact.query;
      extra.recall = fact.id;
    }
    if (i < secrets.length) {
      extra.secret = secrets[i].id;
      extra.secretPlace = secrets[i].place;
    }
    if (i < dirPrompts.length) {
      extra.directive = dirPrompts[i];
      extra.directivePlace = 'prompt';
    } else if (i - dirPrompts.length < dirOutputs.length) {
      extra.directive = dirOutputs[i - dirPrompts.length];
      extra.directivePlace = 'output';
    }
    turns.push(extra);
  }
  for (let offset = 0; offset < turns.length; offset += 12) {
    const chunk = turns.slice(offset, offset + 12);
    const session = newSession(g, agent);
    emitSessionStart(g, session, defaultStartSource(agent));
    for (let i = 0; i < chunk.length; i += 1) {
      emitWorkTurn(
        g,
        session,
        WORK[(offset + i) % WORK.length],
        TOOLS[agent][(offset + i) % TOOLS[agent].length],
        chunk[i],
      );
    }
    emitEnd(g, session);
  }
}

function emitFailureTurn(g, agent) {
  const session = newSession(g, agent);
  emitSessionStart(g, session, defaultStartSource(agent));
  const work = WORK[0];
  if (agent === 'claude') emitWorkTurn(g, session, work, 'read', { fail: 'claude-read' });
  else if (agent === 'codex') emitWorkTurn(g, session, work, 'bash', { fail: 'codex-bash' });
  else if (agent === 'grok') {
    emitWorkTurn(g, session, work, 'bash', { fail: 'grok-deny' });
    emitWorkTurn(g, session, work, 'bash', { fail: 'grok-exit' });
    emitWorkTurn(g, session, work, 'bash', { fail: 'grok-post-fail' });
  } else emitWorkTurn(g, session, work, 'read', { fail: 'pi-error' });
  emitWorkTurn(g, session, WORK[1], TOOLS[agent][0]);
  emitWorkTurn(g, session, WORK[2], TOOLS[agent][1]);
  if (agent !== 'grok') emitWorkTurn(g, session, WORK[3], TOOLS[agent][2]);
  emitEnd(g, session);
}

function emitClaudeLifecycle(g, agent, resume) {
    const forked = newSession(g, agent);
    forked.transcript = resume.transcript;
    emitSessionStart(g, forked, 'fork', { lifecycle: 'fork' });
    emitWorkTurn(g, forked, WORK[7], 'read');
    emitWorkTurn(g, forked, WORK[0], 'write');
    emitWorkTurn(g, forked, WORK[1], 'edit');
    emitWorkTurn(g, forked, WORK[2], 'bash');
    emitEnd(g, forked);
    emitEnd(g, resume);
    const cleared = newSession(g, agent);
    emitSessionStart(g, cleared, 'clear', { lifecycle: 'clear' });
    emitWorkTurn(g, cleared, WORK[0], 'bash');
    emitWorkTurn(g, cleared, WORK[1], 'read');
    emitWorkTurn(g, cleared, WORK[2], 'write');
    emitWorkTurn(g, cleared, WORK[3], 'edit');
    emitEnd(g, cleared);
}

function emitCodexLifecycle(g, agent, resume) {
    // No fork source in the verified enum. /new: parent has no SessionEnd, child source=startup.
    const child = newSession(g, agent);
    emitSessionStart(g, child, 'startup', { lifecycle: 'clear' });
    emitWorkTurn(g, child, WORK[0], 'bash');
    emitWorkTurn(g, child, WORK[1], 'bash-read');
    emitWorkTurn(g, child, WORK[2], 'patch-add');
    emitWorkTurn(g, child, WORK[3], 'patch-update');
    emitEnd(g, child);
    emitEnd(g, resume);
}

function emitGrokLifecycle(g, agent, resume) {
    const forked = newSession(g, agent);
    emitSessionStart(g, forked, 'load', { lifecycle: 'fork' });
    emitWorkTurn(g, forked, WORK[7], 'read');
    emitWorkTurn(g, forked, WORK[0], 'write');
    emitWorkTurn(g, forked, WORK[1], 'edit');
    emitWorkTurn(g, forked, WORK[2], 'bash');
    emitEnd(g, forked);
    const cleared = newSession(g, agent);
    emitSessionStart(g, cleared, 'new', { lifecycle: 'clear' });
    emitWorkTurn(g, cleared, WORK[0], 'bash');
    emitWorkTurn(g, cleared, WORK[1], 'read');
    emitWorkTurn(g, cleared, WORK[2], 'write');
    emitWorkTurn(g, cleared, WORK[3], 'edit');
    emitEnd(g, cleared);
    emitEnd(g, resume);
}

function emitPiLifecycle(g, agent, resume) {
  emitEnd(g, resume, 'fork');
  const forkShutdown = g.events[g.events.length - 1];
  forkShutdown.tags = { ...forkShutdown.tags, lifecycle: 'fork' };
  const forked = newSession(g, agent);
  forked.transcript = resume.transcript;
  emitSessionStart(g, forked, 'startup');
  emitWorkTurn(g, forked, WORK[7], 'read');
  emitWorkTurn(g, forked, WORK[0], 'write');
  emitWorkTurn(g, forked, WORK[1], 'edit');
  emitWorkTurn(g, forked, WORK[2], 'bash');
  emitEnd(g, forked, 'new');
  const clearShutdown = g.events[g.events.length - 1];
  clearShutdown.tags = { ...clearShutdown.tags, lifecycle: 'clear' };
  const cleared = newSession(g, agent);
  emitSessionStart(g, cleared, 'startup');
  emitWorkTurn(g, cleared, WORK[0], 'bash');
  emitWorkTurn(g, cleared, WORK[1], 'read');
  emitWorkTurn(g, cleared, WORK[2], 'write');
  emitWorkTurn(g, cleared, WORK[3], 'edit');
  emitEnd(g, cleared);
}

function emitLifecycleBundle(g, agent) {
  const resume = newSession(g, agent);
  emitSessionStart(g, resume, defaultStartSource(agent));
  emitWorkTurn(g, resume, WORK[4], TOOLS[agent][0]);
  let resumeSource;
  if (agent === 'grok') resumeSource = 'load';
  else if (agent === 'pi') resumeSource = 'startup';
  else resumeSource = 'resume';
  emitSessionStart(g, resume, resumeSource, { lifecycle: 'resume' });
  emitWorkTurn(g, resume, WORK[5], TOOLS[agent][2]);
  emitCompact(g, resume);
  emitWorkTurn(g, resume, WORK[6], TOOLS[agent][3]);
  emitWorkTurn(g, resume, WORK[7], TOOLS[agent][0]);

  if (agent === 'claude') emitClaudeLifecycle(g, agent, resume);
  else if (agent === 'codex') emitCodexLifecycle(g, agent, resume);
  else if (agent === 'grok') emitGrokLifecycle(g, agent, resume);
  else emitPiLifecycle(g, agent, resume);
}

function emitSizeEvent(g, agent, target, sizeTag) {
  const session = newSession(g, agent);
  emitSessionStart(g, session, defaultStartSource(agent));
  beginTurn(g, session);
  if (agent === 'claude') {
    const payload = claudeBase(session, 'UserPromptSubmit');
    attachFill(payload, target, (object, token) => {
      object.prompt = token;
    });
    push(g, session, 'UserPromptSubmit', payload, { size: sizeTag });
  } else if (agent === 'codex') {
    const payload = codexBase(session, 'UserPromptSubmit');
    attachFill(payload, target, (object, token) => {
      object.prompt = token;
    });
    push(g, session, 'UserPromptSubmit', payload, { size: sizeTag });
  } else if (agent === 'grok') {
    const payload = grokBase(g, session, 'UserPromptSubmit');
    payload.promptId = session.promptId;
    attachFill(payload, target, (object, token) => {
      object.prompt = token;
    });
    push(g, session, 'UserPromptSubmit', payload, { size: sizeTag });
  } else {
    const envelope = piEnvelope(session, 'input', { type: 'input', text: '', source: 'interactive' });
    attachFill(envelope, target, (object, token) => {
      object.payload.text = token;
    });
    push(g, session, 'input', envelope, { size: sizeTag });
  }
  emitStop(g, session, 'Large payload captured.');
  emitWorkTurn(g, session, WORK[0], TOOLS[agent][0]);
  emitWorkTurn(g, session, WORK[1], TOOLS[agent][1]);
  emitWorkTurn(g, session, WORK[2], TOOLS[agent][2]);
  emitEnd(g, session);
}

function emitFillerSession(g, agent, turns) {
  const session = newSession(g, agent);
  emitSessionStart(g, session, defaultStartSource(agent));
  for (let i = 0; i < turns; i += 1) {
    const work = WORK[(g.rng() + i) % WORK.length];
    emitWorkTurn(g, session, work, TOOLS[agent][i % TOOLS[agent].length]);
  }
  emitEnd(g, session);
}

function createFactPlans() {
  const plants = { claude: [], codex: [], grok: [], pi: [] };
  const recalls = { claude: [], codex: [], grok: [], pi: [] };
  for (let i = 0; i < FACTS.length; i += 1) {
    const fact = FACTS[i];
    const plantAgent = AGENTS[i % 4];
    const recallAgent = i < 20 ? AGENTS[(i + 1) % 4] : plantAgent;
    plants[plantAgent].push(fact);
    recalls[recallAgent].push(fact);
  }
  return { plants, recalls };
}

function createCorpusPlans(secrets, directives) {
  const secretRows = secrets.filter((row) => row.secret !== null);
  const negativeRows = secrets.filter((row) => row.secret === null);
  const secretPlan = { claude: [], codex: [], grok: [], pi: [] };
  const places = ['prompt', 'input', 'output'];
  for (let i = 0; i < secretRows.length; i += 1) {
    secretPlan[AGENTS[i % 4]].push({ id: secretRows[i].id, place: places[i % 3] });
  }
  for (let i = 0; i < negativeRows.length; i += 1) {
    secretPlan[AGENTS[i % 4]].push({ id: negativeRows[i].id, place: places[i % 3] });
  }
  const dirPromptPlan = { claude: [], codex: [], grok: [], pi: [] };
  const dirOutputPlan = { claude: [], codex: [], grok: [], pi: [] };
  for (let i = 0; i < directives.length; i += 1) {
    dirPromptPlan[AGENTS[i % 4]].push(i);
    dirOutputPlan[AGENTS[(i + 2) % 4]].push(i);
  }
  return { secretPlan, dirPromptPlan, dirOutputPlan };
}

function generate() {
  const secrets = loadJsonl(join(REPO, 'test/corpus/secrets.jsonl'));
  const directives = loadJsonl(join(REPO, 'test/corpus/directives.jsonl'));
  const g = createState();

  const { plants, recalls } = createFactPlans();
  const { secretPlan, dirPromptPlan, dirOutputPlan } = createCorpusPlans(secrets, directives);

  for (const agent of AGENTS) emitSmokeSession(g, agent);

  for (const agent of AGENTS) {
    const list = plants[agent];
    emitSeedSession(g, agent, list.slice(0, 5));
    emitSeedSession(g, agent, list.slice(5));
  }

  for (const agent of AGENTS) {
    emitRecallSession(g, agent, recalls[agent], {
      secrets: secretPlan[agent],
      dirPrompts: dirPromptPlan[agent],
      dirOutputs: dirOutputPlan[agent],
    });
  }

  for (const agent of AGENTS) emitFailureTurn(g, agent);
  for (const agent of AGENTS) emitLifecycleBundle(g, agent);

  emitSizeEvent(g, 'claude', AT_BOUND, 'at_bound');
  emitSizeEvent(g, 'grok', AT_BOUND, 'at_bound');
  emitSizeEvent(g, 'codex', ABOVE_ONE, 'above_bound');
  emitSizeEvent(g, 'pi', ABOVE_TWO, 'above_bound');

  const minTurns = 4;
  const maxTurns = 12;
  let guard = 0;
  while (guard < 80) {
    guard += 1;
    const byAgent = countBy(g.events, (event) => event.agent);
    const short = AGENTS.filter((agent) => (byAgent[agent] ?? 0) < 250);
    if (g.events.length >= 1000 && short.length === 0) break;
    const agent = short[0] ?? AGENTS[g.events.length % 4];
    const turns = minTurns + (g.rng() % (maxTurns - minTurns + 1));
    emitFillerSession(g, agent, turns);
  }

  const lines = g.events.map((event) => JSON.stringify(event));
  const body = `${lines.join('\n')}\n`;
  const report = assertCoverage(g.events, secrets, directives, body);
  mkdirSync(dirname(OUT), { recursive: true });
  writeFileSync(OUT, body);
  return { report, bytes: Buffer.byteLength(body), path: OUT };
}

function invokedDirectly() {
  if (process.argv[1] === undefined) return false;
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

if (invokedDirectly()) {
  const result = generate();
  process.stdout.write(
    `${result.path} ${result.report.total} events ${result.bytes} bytes ${JSON.stringify(result.report.byAgent)}\n`,
  );
}

export { generate };
