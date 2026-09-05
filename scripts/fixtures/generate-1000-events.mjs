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

const ROOT_PH = '__OBOETE_REPLAY_ROOT__';
const SEED = 0x0b0e7e43;
const BASE_MS = Date.UTC(2026, 8, 6);
const AT_BOUND = 1_048_576;
const ABOVE_ONE = 1_048_577;
const ABOVE_TWO = 2_097_152;
const FILL_ALPHABET = 'The quick brown fox jumps over the lazy dog. ';
const AGENTS = ['claude', 'codex', 'grok', 'pi'];
const HERE = fileURLToPath(new URL('.', import.meta.url));
const REPO = join(HERE, '../..');
const OUT = join(REPO, 'test/fixtures/events-1000.jsonl');

const MODELS = {
  codex: 'gpt-5.6-sol',
  grok: undefined,
  pi: 'gpt-5.6-luna',
};

const PI_INPUT_SOURCES = ['interactive', 'rpc', 'extension'];

const TOOLS = {
  claude: ['read', 'write', 'edit', 'bash'],
  codex: ['bash-read', 'patch-add', 'patch-update', 'bash'],
  grok: ['read', 'write', 'edit', 'bash'],
  pi: ['read', 'write', 'edit', 'bash'],
};

const FACTS = [
  { id: 'f-ja-01', lang: 'ja', expect: '3847', statement: 'ローカルの oboete はポート 3847 で listen する。設定ファイルは config/oboete.local.toml。', query: 'oboete のローカル listen ポートは何番？' },
  { id: 'f-ja-02', lang: 'ja', expect: 'memories_v3', statement: '要約の保存先テーブルは memories_v3 に切り替えた。旧 memories は読み取り専用。', query: '要約は今どのテーブルに書いてる？' },
  { id: 'f-ja-03', lang: 'ja', expect: 'ENABLE_CJK_BIGRAM', statement: '日本語検索は機能フラグ ENABLE_CJK_BIGRAM が立っているときだけ bigram を足す。', query: '日本語検索の bigram を制御するフラグ名は？' },
  { id: 'f-ja-04', lang: 'ja', expect: 'yamada-keisuke', statement: 'dogfood アカウントの担当は yamada-keisuke。鍵のローテは彼が持つ。', query: 'dogfood アカウントの担当者は誰？' },
  { id: 'f-ja-05', lang: 'ja', expect: 'E-OBOETE-4419', statement: '検出器が切れたときのログコードは E-OBOETE-4419。doctor も同じコードを出す。', query: '検出器タイムアウトのログコードは？' },
  { id: 'f-ja-06', lang: 'ja', expect: '0007_fts_ja', statement: '次のマイグレーションファイル名は 0007_fts_ja.sql。FTS の日本語トークナイザを足す。', query: '次に入れるマイグレーションのファイル名は？' },
  { id: 'f-ja-07', lang: 'ja', expect: 'memory-box.internal.example', statement: '社内の memory ホストは memory-box.internal.example。VPN 内からのみ解決する。', query: '社内 memory ホストの名前は？' },
  { id: 'f-ja-08', lang: 'ja', expect: '--ink-sumi-900', statement: 'ビューアの本文色トークンは --ink-sumi-900。ライトテーマでもこの名前のまま。', query: 'ビューア本文の色トークン名は？' },
  { id: 'f-ja-09', lang: 'ja', expect: 'dogfood-jp-07', statement: '隔離ユーザのログイン名は dogfood-jp-07。ホームは /home/dogfood-jp-07。', query: '日本語 dogfood のログイン名は？' },
  { id: 'f-ja-10', lang: 'ja', expect: 'src/privacy/cjk-normalize.ts', statement: '全角英数の正規化は src/privacy/cjk-normalize.ts に閉じ込めた。他から呼ばない。', query: '全角英数の正規化はどのファイル？' },
  { id: 'f-ja-11', lang: 'ja', expect: '2026-11-18', statement: '無料枠のリセット日は 2026-11-18。それまでは allowance を使い切らない。', query: '無料枠のリセット日はいつ？' },
  { id: 'f-ja-12', lang: 'ja', expect: 'oboete-ja-packs', statement: '日本語パックの R2 バケット名は oboete-ja-packs。本番だけ。', query: '日本語パックのバケット名は？' },
  { id: 'f-ja-13', lang: 'ja', expect: '7f3e91c', statement: '直近で入れた正規化の修正コミットは 7f3e91c。revert するならこれ。', query: '正規化修正のコミットはどれ？' },
  { id: 'f-ja-14', lang: 'ja', expect: 'OBOETE_JA_HINT', statement: '日本語ヒントを出すときは環境変数 OBOETE_JA_HINT を 1 にする。', query: '日本語ヒントの環境変数名は？' },
  { id: 'f-ja-15', lang: 'ja', expect: 'compact-q-ja', statement: '日本語セッションの compact キュー名は compact-q-ja。英語とは分ける。', query: '日本語 compact のキュー名は？' },
  { id: 'f-ja-16', lang: 'ja', expect: 'kuromoji-lite-0.4.2', statement: '形態素のフォールバックは kuromoji-lite-0.4.2。本番の tokenizer ではない。', query: '形態素フォールバックのライブラリと版は？' },
  { id: 'f-ja-17', lang: 'ja', expect: '937', statement: 'セッション要約の最短間隔は 937 秒。それより短い再実行は捨てる。', query: 'セッション要約の最短間隔は何秒？' },
  { id: 'f-ja-18', lang: 'ja', expect: 'm1/p5-t067-replay', statement: 'replay 作業ブランチは m1/p5-t067-replay。main には直接載せない。', query: 'replay 作業ブランチ名は？' },
  { id: 'f-ja-19', lang: 'ja', expect: '第3水曜日', statement: '鍵のローテーションは毎月 第3水曜日。前倒ししない。', query: '鍵ローテは月のいつ？' },
  { id: 'f-ja-20', lang: 'ja', expect: 'turn-budget-ja-48', statement: '日本語ターンの文字予算キーは turn-budget-ja-48。英語キーと混ぜない。', query: '日本語ターン予算のキー名は？' },
  { id: 'f-en-01', lang: 'en', expect: '9124', statement: 'The sidecar listens on port 9124. Health checks must use that port, not 8080.', query: 'Which port does the sidecar listen on?' },
  { id: 'f-en-02', lang: 'en', expect: 'raw_events_epoch', statement: 'Epoch annotations live in the raw_events_epoch table, not on sessions.', query: 'Which table stores epoch annotations?' },
  { id: 'f-en-03', lang: 'en', expect: 'ENABLE_PACK_TRIM', statement: 'Pack trimming is gated by ENABLE_PACK_TRIM. Leave it off in dogfood.', query: 'What flag gates pack trimming?' },
  { id: 'f-en-04', lang: 'en', expect: 'Priya-Nair', statement: 'The isolated-user probe owner is Priya-Nair. Ping her before killing tmux.', query: 'Who owns the isolated-user probe?' },
  { id: 'f-en-05', lang: 'en', expect: 'src/injection/epoch-key.ts', statement: 'Compaction epoch keys are computed in src/injection/epoch-key.ts only.', query: 'Where is the compaction epoch key computed?' },
  { id: 'f-en-06', lang: 'en', expect: 'E-OBOETE-8801', statement: 'A stalled Pi child is reported as E-OBOETE-8801 in doctor and the hook log.', query: 'What error code is a stalled Pi child?' },
  { id: 'f-en-07', lang: 'en', expect: 'feat/replay-harness', statement: 'Land replay harness work on feat/replay-harness, never on m1/p5-t067 directly.', query: 'Which branch should the replay harness land on?' },
  { id: 'f-en-08', lang: 'en', expect: '271', statement: 'The spool reclaim interval is 271 seconds so it never aligns with the 300 ms hook.', query: 'How many seconds is the spool reclaim interval?' },
  { id: 'f-en-09', lang: 'en', expect: 'pack-cache.internal', statement: 'The pack cache host is pack-cache.internal. It is not reachable off VPN.', query: 'What is the pack cache hostname?' },
  { id: 'f-en-10', lang: 'en', expect: '--accent-ember-600', statement: 'The viewer accent token is --accent-ember-600. Do not invent a second accent.', query: 'What is the viewer accent token name?' },
  { id: 'f-en-11', lang: 'en', expect: 'dogfood-en-03', statement: 'The English dogfood login is dogfood-en-03. Home is /home/dogfood-en-03.', query: 'What is the English dogfood login name?' },
  { id: 'f-en-12', lang: 'en', expect: '2026-12-02', statement: 'The next catalog freeze is 2026-12-02. Do not bump models after that date.', query: 'When is the next catalog freeze?' },
  { id: 'f-en-13', lang: 'en', expect: 'oboete-en-packs', statement: 'English packs go to the oboete-en-packs bucket. Japanese has its own.', query: 'Which bucket holds English packs?' },
  { id: 'f-en-14', lang: 'en', expect: '4c2a18d', statement: 'The injection ledger fix is commit 4c2a18d. Revert that if duplicates return.', query: 'Which commit is the injection ledger fix?' },
  { id: 'f-en-15', lang: 'en', expect: 'OBOETE_EN_HINT', statement: 'Set OBOETE_EN_HINT=1 to print English retrieval traces on stderr.', query: 'Which env var enables English retrieval traces?' },
  { id: 'f-en-16', lang: 'en', expect: 'observe-q-en', statement: 'The English observer queue name is observe-q-en. Do not share it with Japanese.', query: 'What is the English observer queue name?' },
  { id: 'f-en-17', lang: 'en', expect: 'murmur-pack-2.1.0', statement: 'Fallback packing uses murmur-pack-2.1.0. Do not upgrade it in M1.', query: 'Which library version does fallback packing use?' },
  { id: 'f-en-18', lang: 'en', expect: '0008_epoch_index', statement: 'The next English-side migration is 0008_epoch_index.sql.', query: 'What is the next English-side migration file?' },
  { id: 'f-en-19', lang: 'en', expect: 'turn-budget-64', statement: 'The English turn character budget key is turn-budget-64.', query: 'What is the English turn budget key?' },
  { id: 'f-en-20', lang: 'en', expect: 'grok-fixture-luna', statement: 'Grok fixture sessions pin model alias grok-fixture-luna so windows stay stable.', query: 'Which model alias do Grok fixture sessions pin?' },
];

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

function fillBytes(n) {
  if (n <= 0) return '';
  const unit = FILL_ALPHABET;
  const copies = Math.ceil(n / unit.length);
  return unit.repeat(copies).slice(0, n);
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

function walkExpand(value, expandString) {
  if (typeof value === 'string') return expandString(value);
  if (Array.isArray(value)) return value.map((item) => walkExpand(item, expandString));
  if (value !== null && typeof value === 'object') {
    const out = {};
    for (const [key, item] of Object.entries(value)) out[key] = walkExpand(item, expandString);
    return out;
  }
  return value;
}

function expandFillOnly(payload) {
  return walkExpand(payload, (text) =>
    text.replace(/__FILL:(\d+)__/g, (_, n) => fillBytes(Number(n))),
  );
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

function emitClaudeTool(g, session, spec, tagsPre, tagsPost) {
  const id = toolUseId(g, session);
  const pre = claudeBase(session, 'PreToolUse');
  pre.tool_use_id = id;
  const post = claudeBase(session, 'PostToolUse');
  post.tool_use_id = id;
  if (spec.kind === 'read') {
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
    return;
  }
  if (spec.kind === 'write') {
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
    return;
  }
  if (spec.kind === 'edit') {
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
    return;
  }
  if (spec.kind === 'bash') {
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

function emitGrokTool(g, session, spec, tagsPre, tagsPost) {
  const id = toolUseId(g, session);
  const pre = grokBase(g, session, 'PreToolUse');
  pre.toolUseId = id;
  pre.tool_use_id = id;
  pre.toolInputTruncated = false;
  if (spec.kind === 'read') {
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
    return;
  }
  if (spec.kind === 'write') {
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
    return;
  }
  if (spec.kind === 'edit') {
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
    return;
  }
  if (spec.kind === 'bash' || spec.kind === 'bash-fail') {
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
      output_file: `${session.transcript.replace(/updates\.jsonl$|[^/]+\.jsonl$/, '')}terminal/${id}.log`,
      total_bytes: Buffer.byteLength(spec.body),
    };
    post.toolResult = result;
    post.tool_response = result;
    push(g, session, 'PreToolUse', pre, tagsPre);
    push(g, session, 'PostToolUse', post, tagsPost);
    return;
  }
  if (spec.kind === 'deny') {
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
  emitPiTool(g, session, mapped, { ...(tagsPre ?? {}), ...(tagsPost ?? {}) });
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

function emitCompact(g, session) {
  const tags = { lifecycle: 'compact' };
  if (session.agent === 'claude') {
    emitSessionStart(g, session, 'compact', tags);
    const payload = claudeBase(session, 'PostCompact');
    payload.trigger = 'auto';
    payload.compact_summary = 'The session read project files and applied a small edit.';
    push(g, session, 'PostCompact', payload);
    return;
  }
  if (session.agent === 'codex') {
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
    return;
  }
  if (session.agent === 'grok') {
    const first = grokBase(g, session, 'PostCompact');
    first.source = 'auto';
    push(g, session, 'PostCompact', first, tags);
    const second = grokBase(g, session, 'PostCompact');
    second.source = 'auto';
    push(g, session, 'PostCompact', second);
    return;
  }
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

function assistantLine(work) {
  return work.lang === 'ja'
    ? `${work.file} を更新した。次はテストを回す。`
    : `Updated ${work.file}. Next step is to run the tests.`;
}

function emitWorkTurn(g, session, work, toolKind, extra = {}) {
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

  let promptText = extra.promptText ?? work.prompt;
  if (extra.secretPlace === 'prompt') promptText = `${promptText}\n${secretToken(extra.secret)}`;
  if (extra.directivePlace === 'prompt') {
    promptText = `${promptText}\n${directiveToken(extra.directive)}`;
  }
  if (extra.fact !== undefined && extra.factPlace === 'prompt') {
    promptText = `${promptText}\n${statementOf(extra.fact)}`;
  }
  emitPrompt(g, session, promptText, promptTags);

  if (extra.fail === 'claude-read') {
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
    return;
  }
  if (extra.fail === 'codex-bash') {
    emitTool(
      g,
      session,
      { kind: 'fail', cmd: 'false', error: 'Exit code 1\ncommand failed' },
      undefined,
      outputTags,
    );
    emitStop(g, session, assistantLine(work));
    return;
  }
  if (extra.fail === 'grok-deny') {
    emitTool(g, session, { kind: 'deny', cmd: 'echo perm-probe', body: '' }, undefined, outputTags);
    emitStop(g, session, assistantLine(work));
    return;
  }
  if (extra.fail === 'grok-exit') {
    emitTool(
      g,
      session,
      { kind: 'bash-fail', cmd: "bash -c 'echo boom >&2; exit 3'", body: 'boom\n' },
      undefined,
      outputTags,
    );
    emitStop(g, session, assistantLine(work));
    return;
  }
  if (extra.fail === 'grok-post-fail') {
    emitTool(
      g,
      session,
      { kind: 'fail', cmd: 'false', error: 'tool handler crashed' },
      undefined,
      outputTags,
    );
    emitStop(g, session, assistantLine(work));
    return;
  }
  if (extra.fail === 'pi-error') {
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
    return;
  }

  const spec = specFor(session.agent, kind, work);
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
  emitTool(
    g,
    session,
    spec,
    Object.keys(inputTags).length === 0 ? undefined : inputTags,
    Object.keys(outputTags).length === 0 ? undefined : outputTags,
  );
  emitStop(g, session, extra.assistant ?? assistantLine(work));
}

function secretToken(id) {
  return `__SECRET:${id}__`;
}

function directiveToken(index) {
  return `__DIRECTIVE:${index}__`;
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

function emitLifecycleBundle(g, agent) {
  const resume = newSession(g, agent);
  emitSessionStart(g, resume, defaultStartSource(agent));
  emitWorkTurn(g, resume, WORK[4], TOOLS[agent][0]);
  const resumeSource = agent === 'grok' ? 'load' : agent === 'pi' ? 'startup' : 'resume';
  emitSessionStart(g, resume, resumeSource, { lifecycle: 'resume' });
  emitWorkTurn(g, resume, WORK[5], TOOLS[agent][2]);
  emitCompact(g, resume);
  emitWorkTurn(g, resume, WORK[6], TOOLS[agent][3]);
  emitWorkTurn(g, resume, WORK[7], TOOLS[agent][0]);

  if (agent === 'claude') {
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
    return;
  }
  if (agent === 'codex') {
    // No fork source in the verified enum. /new: parent has no SessionEnd, child source=startup.
    const child = newSession(g, agent);
    emitSessionStart(g, child, 'startup', { lifecycle: 'clear' });
    emitWorkTurn(g, child, WORK[0], 'bash');
    emitWorkTurn(g, child, WORK[1], 'bash-read');
    emitWorkTurn(g, child, WORK[2], 'patch-add');
    emitWorkTurn(g, child, WORK[3], 'patch-update');
    emitEnd(g, child);
    emitEnd(g, resume);
    return;
  }
  if (agent === 'grok') {
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
    return;
  }
  emitEnd(g, resume, 'fork');
  const forkShutdown = g.events[g.events.length - 1];
  forkShutdown.tags = { ...(forkShutdown.tags ?? {}), lifecycle: 'fork' };
  const forked = newSession(g, agent);
  forked.transcript = resume.transcript;
  emitSessionStart(g, forked, 'startup');
  emitWorkTurn(g, forked, WORK[7], 'read');
  emitWorkTurn(g, forked, WORK[0], 'write');
  emitWorkTurn(g, forked, WORK[1], 'edit');
  emitWorkTurn(g, forked, WORK[2], 'bash');
  emitEnd(g, forked, 'new');
  const clearShutdown = g.events[g.events.length - 1];
  clearShutdown.tags = { ...(clearShutdown.tags ?? {}), lifecycle: 'clear' };
  const cleared = newSession(g, agent);
  emitSessionStart(g, cleared, 'startup');
  emitWorkTurn(g, cleared, WORK[0], 'bash');
  emitWorkTurn(g, cleared, WORK[1], 'read');
  emitWorkTurn(g, cleared, WORK[2], 'write');
  emitWorkTurn(g, cleared, WORK[3], 'edit');
  emitEnd(g, cleared);
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

function countBy(events, keyFn) {
  const out = {};
  for (const event of events) {
    const key = keyFn(event);
    out[key] = (out[key] ?? 0) + 1;
  }
  return out;
}

function promptTextOf(event) {
  if (event.event === 'UserPromptSubmit') return event.payload.prompt ?? '';
  if (event.event === 'input') return event.payload.payload?.text ?? '';
  return '';
}

function adapterOutputText(event) {
  const payload = event.payload;
  if (event.agent === 'claude' && event.event === 'PostToolUse') {
    const response = payload.tool_response;
    const tool = payload.tool_name;
    // Write/Edit adapter output is the path (`writtenPath`), not file content.
    if (tool === 'Write' || tool === 'Edit') {
      return typeof response?.filePath === 'string' ? response.filePath : '';
    }
    if (typeof response === 'string') return response;
    if (response === null || typeof response !== 'object') return '';
    if (typeof response.file?.content === 'string') return response.file.content;
    const stdout = typeof response.stdout === 'string' ? response.stdout : '';
    const stderr = typeof response.stderr === 'string' ? response.stderr : '';
    if ('stdout' in response || 'stderr' in response) return `${stdout}\n${stderr}`;
    return '';
  }
  if (event.agent === 'codex' && event.event === 'PostToolUse') {
    return typeof payload.tool_response === 'string' ? payload.tool_response : '';
  }
  if (event.agent === 'grok' && event.event === 'PostToolUse') {
    const result = payload.toolResult ?? {};
    if (typeof result.FileContent?.content === 'string') return result.FileContent.content;
    if (typeof result.EditsApplied?.tool_output_for_prompt === 'string') {
      return result.EditsApplied.tool_output_for_prompt;
    }
    if (typeof result.output_for_prompt === 'string') return result.output_for_prompt;
    return '';
  }
  if (event.agent === 'pi' && event.event === 'tool_result') {
    const blocks = payload.payload?.content;
    if (!Array.isArray(blocks)) return '';
    return blocks.map((block) => (typeof block?.text === 'string' ? block.text : '')).join('\n');
  }
  return '';
}

function coverage(events, secrets, directives) {
  const byAgent = countBy(events, (event) => event.agent);
  const byEvent = countBy(events, (event) => `${event.agent}:${event.event}`);
  const secretIds = new Set(
    secrets.filter((row) => row.secret !== null).map((row) => row.id),
  );
  const seenSecrets = new Set();
  const dirPrompt = new Set();
  const dirOutput = new Set();
  const factsPlanted = new Set();
  const factsRecalled = new Set();
  const plantSession = new Map();
  const recallSession = new Map();
  const lifecycle = { resume: new Set(), compact: new Set(), fork: new Set(), clear: new Set() };
  const sizes = [];
  const langs = { ja: 0, en: 0 };

  for (const event of events) {
    const tags = event.tags ?? {};
    if (tags.secret !== undefined) seenSecrets.add(tags.secret);
    if (tags.directive !== undefined) {
      const promptEvent =
        event.event === 'UserPromptSubmit' || event.event === 'input';
      if (promptEvent) dirPrompt.add(tags.directive);
      else dirOutput.add(tags.directive);
    }
    if (tags.fact !== undefined) {
      factsPlanted.add(tags.fact.id);
      plantSession.set(tags.fact.id, `${event.agent}:${event.session}`);
      if (tags.fact.lang === 'ja') langs.ja += 1;
      else langs.en += 1;
    }
    if (tags.recall !== undefined) {
      factsRecalled.add(tags.recall);
      recallSession.set(tags.recall, `${event.agent}:${event.session}`);
    }
    if (tags.lifecycle !== undefined) lifecycle[tags.lifecycle]?.add(event.agent);
    if (tags.size !== undefined) {
      const bytes = Buffer.byteLength(JSON.stringify(expandFillOnly(event.payload)));
      sizes.push({ agent: event.agent, seq: event.seq, tag: tags.size, bytes });
    }
  }

  const missingSecrets = [...secretIds].filter((id) => !seenSecrets.has(id));
  const negativeIds = secrets.filter((row) => row.secret === null).map((row) => row.id);
  const missingNegatives = negativeIds.filter((id) => !seenSecrets.has(id));
  const missingDir = [];
  for (let i = 0; i < directives.length; i += 1) {
    if (!dirPrompt.has(i) || !dirOutput.has(i)) missingDir.push(i);
  }
  const missingPlant = FACTS.filter((fact) => !factsPlanted.has(fact.id)).map((fact) => fact.id);
  const missingRecall = FACTS.filter((fact) => !factsRecalled.has(fact.id)).map((fact) => fact.id);
  const sameSessionRecall = [];
  for (const id of factsRecalled) {
    if (plantSession.get(id) === recallSession.get(id)) sameSessionRecall.push(id);
  }

  const required = {
    claude: ['SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'PostToolUseFailure', 'Stop', 'PostCompact', 'SessionEnd'],
    codex: ['SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'PostToolUseFailure', 'Stop', 'PostCompact', 'SessionEnd'],
    grok: ['SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'PostToolUseFailure', 'PermissionDenied', 'Stop', 'PostCompact', 'SessionEnd'],
    pi: ['session_start', 'input', 'tool_result', 'agent_settled', 'session_shutdown', 'session_compact'],
  };
  const missingKinds = [];
  for (const agent of AGENTS) {
    for (const name of required[agent]) {
      if ((byEvent[`${agent}:${name}`] ?? 0) === 0) missingKinds.push(`${agent}:${name}`);
    }
  }

  return {
    total: events.length,
    byAgent,
    byEvent,
    secrets: {
      required: secretIds.size,
      seen: seenSecrets.size,
      missing: missingSecrets,
      negatives: negativeIds.length,
      missingNegatives,
    },
    directives: {
      total: directives.length,
      prompt: dirPrompt.size,
      output: dirOutput.size,
      missing: missingDir,
    },
    facts: {
      planted: factsPlanted.size,
      recalled: factsRecalled.size,
      ja: langs.ja,
      en: langs.en,
      missingPlant,
      missingRecall,
      sameSessionRecall,
    },
    lifecycle: Object.fromEntries(
      Object.entries(lifecycle).map(([name, set]) => [name, [...set]]),
    ),
    sizes,
    missingKinds,
  };
}

function assertCoverage(events, secrets, directives, body) {
  const report = coverage(events, secrets, directives);
  const problems = [];
  if (report.total < 1000) problems.push(`only ${report.total} events`);
  for (const agent of AGENTS) {
    const n = report.byAgent[agent] ?? 0;
    if (n < 240) problems.push(`${agent} has ${n} events`);
  }
  if (report.secrets.missing.length > 0) {
    problems.push(`missing secrets ${report.secrets.missing.join(',')}`);
  }
  if (report.secrets.missingNegatives.length > 0) {
    problems.push(`missing negative secrets ${report.secrets.missingNegatives.join(',')}`);
  }
  if (report.directives.missing.length > 0) {
    problems.push(`directives not in both prompt and output: ${report.directives.missing.join(',')}`);
  }
  if (report.facts.ja < 20 || report.facts.en < 20) {
    problems.push(`facts ja=${report.facts.ja} en=${report.facts.en}`);
  }
  if (report.facts.missingPlant.length > 0) problems.push(`unplanted ${report.facts.missingPlant.join(',')}`);
  if (report.facts.missingRecall.length > 0) problems.push(`unrecalled ${report.facts.missingRecall.join(',')}`);
  if (report.facts.sameSessionRecall.length > 0) {
    problems.push(`recall in same session ${report.facts.sameSessionRecall.join(',')}`);
  }
  if (report.missingKinds.length > 0) problems.push(`missing kinds ${report.missingKinds.join(',')}`);
  for (const name of ['resume', 'compact', 'clear']) {
    if (report.lifecycle[name].length < 4) problems.push(`lifecycle ${name} agents=${report.lifecycle[name]}`);
  }
  if (!report.lifecycle.fork.includes('claude') || !report.lifecycle.fork.includes('grok') || !report.lifecycle.fork.includes('pi')) {
    problems.push(`lifecycle fork agents=${report.lifecycle.fork}`);
  }
  const at = report.sizes.filter((row) => row.tag === 'at_bound' && row.bytes === AT_BOUND);
  const above = report.sizes.filter((row) => row.tag === 'above_bound');
  if (at.length < 2) problems.push(`at_bound count ${at.length}`);
  if (above.length < 2) problems.push(`above_bound count ${above.length}`);
  if (!above.some((row) => row.bytes === ABOVE_ONE)) problems.push('missing 1048577');
  if (!above.some((row) => row.bytes === ABOVE_TWO)) problems.push('missing 2097152');
  for (const row of report.sizes) {
    if (row.tag === 'at_bound' && row.bytes !== AT_BOUND) {
      problems.push(`size seq ${row.seq} at_bound is ${row.bytes}`);
    }
    if (row.tag === 'above_bound' && row.bytes !== ABOVE_ONE && row.bytes !== ABOVE_TWO) {
      problems.push(`size seq ${row.seq} above_bound is ${row.bytes}`);
    }
  }
  for (const row of secrets) {
    if (row.secret !== null && body.includes(row.secret)) problems.push(`secret value leaked for ${row.id}`);
  }
  const factsById = new Map(FACTS.map((fact) => [fact.id, fact]));
  const grokTimestamps = [];
  const grokCompacts = {};
  const piSources = new Set();
  let prev = 0;
  for (const event of events) {
    if (event.seq !== prev + 1) problems.push(`seq gap at ${event.seq}`);
    prev = event.seq;
    JSON.parse(JSON.stringify(event));
    const tags = event.tags ?? {};
    const blob = JSON.stringify(event.payload);
    if (event.agent === 'claude' && Object.hasOwn(event.payload, 'model')) {
      problems.push(`claude payload has model at seq ${event.seq}`);
    }
    if (tags.fact !== undefined) {
      const expect = tags.fact.expect;
      if (typeof expect !== 'string' || expect === '' || !blob.includes(expect)) {
        problems.push(`fact ${tags.fact.id} expect missing from payload seq ${event.seq}`);
      }
      if (
        (event.event === 'PostToolUse' || event.event === 'tool_result') &&
        !adapterOutputText(event).includes(expect)
      ) {
        problems.push(`fact ${tags.fact.id} expect not in adapter output seq ${event.seq}`);
      }
    }
    if (tags.secret !== undefined) {
      const token = secretToken(tags.secret);
      if (!blob.includes(token)) {
        problems.push(`secret token missing from payload seq ${event.seq} id=${tags.secret}`);
      }
      if (event.event === 'PostToolUse' && !adapterOutputText(event).includes(token)) {
        problems.push(`secret ${tags.secret} not in adapter output seq ${event.seq}`);
      }
      if (event.event === 'tool_result' && !adapterOutputText(event).includes(token)) {
        const inputBlob = JSON.stringify(event.payload.payload?.input ?? {});
        const toolName = event.payload.payload?.toolName;
        if (!inputBlob.includes(token)) {
          problems.push(`secret ${tags.secret} not in pi input or content seq ${event.seq}`);
        } else if (toolName === 'write' || toolName === 'edit') {
          problems.push(`secret ${tags.secret} in pi ${toolName} input seq ${event.seq}`);
        }
      }
    }
    if (tags.directive !== undefined) {
      const token = directiveToken(tags.directive);
      if (!blob.includes(token)) {
        problems.push(`directive token missing from payload seq ${event.seq} index=${tags.directive}`);
      }
      if (event.event === 'PostToolUse' && !adapterOutputText(event).includes(token)) {
        problems.push(`directive ${tags.directive} not in adapter output seq ${event.seq}`);
      }
      if (event.event === 'tool_result' && !adapterOutputText(event).includes(token)) {
        const toolName = event.payload.payload?.toolName;
        if (toolName === 'write' || toolName === 'edit') {
          problems.push(`directive ${tags.directive} in pi ${toolName} input seq ${event.seq}`);
        }
      }
    }
    if (tags.recall !== undefined) {
      const fact = factsById.get(tags.recall);
      const prompt = promptTextOf(event);
      if (fact === undefined || !prompt.includes(fact.query)) {
        problems.push(`recall ${tags.recall} prompt missing query seq ${event.seq}`);
      }
    }
    if (event.agent === 'grok') {
      const ts = event.payload.timestamp;
      if (typeof ts !== 'string' || ts === '') {
        problems.push(`grok missing timestamp seq ${event.seq}`);
      } else {
        if (grokTimestamps.includes(ts)) problems.push(`duplicate grok timestamp ${ts} seq ${event.seq}`);
        if (grokTimestamps.length > 0 && ts <= grokTimestamps[grokTimestamps.length - 1]) {
          problems.push(`grok timestamp not increasing seq ${event.seq}`);
        }
        grokTimestamps.push(ts);
      }
      if (event.event === 'PostCompact') {
        grokCompacts[event.session] = (grokCompacts[event.session] ?? 0) + 1;
      }
      if (tags.lifecycle === 'fork') {
        const expected = `${ROOT_PH}/.oboete-replay/grok/${event.session}.jsonl`;
        if (event.payload.transcriptPath !== expected) {
          problems.push(`grok fork reuses transcript seq ${event.seq}`);
        }
      }
    }
    if (event.agent === 'pi' && event.event === 'input') {
      piSources.add(event.payload.payload?.source);
    }
  }
  if (!Object.values(grokCompacts).some((n) => n >= 2)) {
    problems.push('no grok session compact twice');
  }
  for (const source of PI_INPUT_SOURCES) {
    if (!piSources.has(source)) problems.push(`pi input source missing ${source}`);
  }
  const turnsBySession = {};
  for (const event of events) {
    if (event.event === 'UserPromptSubmit' || event.event === 'input') {
      turnsBySession[event.session] = (turnsBySession[event.session] ?? 0) + 1;
    }
  }
  for (const [label, turns] of Object.entries(turnsBySession)) {
    if (turns < 4 || turns > 12) problems.push(`session ${label} has ${turns} turns`);
  }
  if (problems.length > 0) throw new Error(`coverage failed:\n- ${problems.join('\n- ')}`);
  report.turnsBySession = turnsBySession;
  return report;
}

function generate() {
  const secrets = loadJsonl(join(REPO, 'test/corpus/secrets.jsonl'));
  const directives = loadJsonl(join(REPO, 'test/corpus/directives.jsonl'));
  const g = createState();

  const plants = { claude: [], codex: [], grok: [], pi: [] };
  const recalls = { claude: [], codex: [], grok: [], pi: [] };
  for (let i = 0; i < FACTS.length; i += 1) {
    const fact = FACTS[i];
    const plantAgent = AGENTS[i % 4];
    const recallAgent = i < 20 ? AGENTS[(i + 1) % 4] : plantAgent;
    plants[plantAgent].push(fact);
    recalls[recallAgent].push(fact);
  }

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

export { AT_BOUND, ABOVE_ONE, ABOVE_TWO, FILL_ALPHABET, ROOT_PH, expandFillOnly, fillBytes, generate };
