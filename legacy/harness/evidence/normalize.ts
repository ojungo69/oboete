/**
 * 観測記録の正規化と digest。取得側（harness/rig/rig.sh）と検証側（harness/assemble.ts）が
 * 共有する唯一の実装で、module としても CLI としても使う。
 * 規則は specs/003-evidence-hash-normalization/data-model.md §1。
 *
 * 二重実装を作らない。shell から TypeScript の関数を呼ぶ手段が無いので、
 * 同じファイルを import と `node <file>` の両方から使う。
 */
import { createHash } from "node:crypto";
import { lstatSync, readFileSync, realpathSync } from "node:fs";
import { basename, isAbsolute, join, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { canonicalizeJson, decodeUtf8, parseIJson } from "../schema/jcs.ts";

/** 現在の正規化規則の版。data-model.md §1 のいずれかを変えたら上げる */
export const NORMALIZATION_VERSION = 1;

/**
 * payload **直下**でだけ verbatim にするキー。実測で「取得のたびに変わらず、
 * 観測の内容を決める」ことを確認したものだけ（data-model.md §1.3(a)）。
 * 深い階層の同名キー（tool_input.prompt など）はモデルが組み立てた引数なので対象外。
 */
const VERBATIM_PAYLOAD_KEYS = new Set([
  "hook_event_name",
  "tool_name",
  "source",
  "reason",
  "permission_mode",
  "agent_type",
  "prompt",
]);

/** 値は出さずに等値関係だけ残すキー。番号空間は id と path で分ける */
const ID_KEYS = new Set(["session_id", "prompt_id", "turn_id", "tool_use_id", "agent_id"]);
const PATH_KEYS = new Set(["transcript_path", "agent_transcript_path", "cwd"]);

/** 証拠置き場を持つ CLI。path の一部になるので呼び出し側任せにしない */
const KNOWN_CLIS = new Set(["claude", "codex"]);

/** rig が投入する marker。値そのものは秘密なので伏せる */
const INJECT_MARKER = /RIG_INJECT_[A-Za-z0-9_]+/g;

type JsonValue = string | number | boolean | null | JsonValue[] | { [k: string]: JsonValue };

/** JCS と同じ UTF-16 コードユニット昇順。走査順を直列化と揃えるために使う */
const byUtf16 = (a: string, b: string): number => (a < b ? -1 : a > b ? 1 : 0);

/** 失敗の理由は分類して返す。観測記録の中身と絶対 path は載せない */
export class EvidenceError extends Error {}

function fail(message: string): never {
  throw new EvidenceError(message);
}

/** 初出順の局所 token。表はファイル単位で、id と path が別の番号空間を持つ */
function makeTokenTable(): { id: (v: string) => string; path: (v: string) => string } {
  const seen = { id: new Map<string, string>(), path: new Map<string, string>() };
  const take = (kind: "id" | "path", value: string): string => {
    const table = seen[kind];
    const hit = table.get(value);
    if (hit !== undefined) return hit;
    const token = `<${kind}:${table.size + 1}>`;
    table.set(value, token);
    return token;
  };
  return { id: (v) => take("id", v), path: (v) => take("path", v) };
}

/**
 * 既定の扱い。値は伏せるがキーと構造は必ず残す。
 * 欄を落とすと、未知の欄が増えたことが digest に現れなくなる（FR-010）。
 */
function redact(value: JsonValue): JsonValue {
  if (value === null) return null;
  if (typeof value === "boolean") return value;
  if (typeof value === "number") return "<number>";
  if (typeof value === "string") return value === "" ? "<string:empty>" : "<string>";
  if (Array.isArray(value)) {
    // 長さを保つ。長さだけが違う 2 記録を同じ digest にしない
    const out: JsonValue[] = [];
    for (const item of value) out.push(redact(item));
    return out;
  }
  // 中間 object は Object.create(null) で作る。{} だと __proto__ の代入で欄が消える
  const out = Object.create(null) as { [k: string]: JsonValue };
  for (const key of Object.keys(value).sort(byUtf16)) out[key] = redact(value[key] as JsonValue);
  return out;
}

/** verbatim にする文字列。marker だけは伏せる */
function verbatim(value: string): string {
  return value.replace(INJECT_MARKER, "RIG_INJECT_<marker>");
}

function normalizePayload(
  payload: { [k: string]: JsonValue },
  tokens: ReturnType<typeof makeTokenTable>,
): JsonValue {
  const out = Object.create(null) as { [k: string]: JsonValue };
  // 整列した後の順で走る。入力の property 順で token を振ると、
  // 書き順だけが違う 2 記録の digest が変わる（§1.3(b)）
  for (const key of Object.keys(payload).sort(byUtf16)) {
    const value = payload[key] as JsonValue;
    // verbatim と token は string のときだけ。位置だけで決めると、
    // payload.reason に object を置いて本文を通せる
    // 空文字は識別子として扱わない。token を振ると、実体の無い空 ID どうしが
    // 「run を通して同じ」「turn を共有した」の根拠になる
    if (typeof value === "string" && value !== "") {
      if (VERBATIM_PAYLOAD_KEYS.has(key)) {
        out[key] = verbatim(value);
        continue;
      }
      if (ID_KEYS.has(key)) {
        out[key] = tokens.id(value);
        continue;
      }
      if (PATH_KEYS.has(key)) {
        out[key] = tokens.path(value);
        continue;
      }
    }
    out[key] = redact(value);
  }
  return out;
}

function isDataObject(v: unknown): v is { [k: string]: JsonValue } {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** 不正 UTF-8 の位置を診断へ出す。中身は出さない */
function firstInvalidUtf8Offset(bytes: Uint8Array): number {
  const lossy = new TextDecoder("utf-8", { fatal: false, ignoreBOM: true }).decode(bytes);
  const at = lossy.indexOf("�");
  return at < 0 ? bytes.length : Buffer.byteLength(lossy.slice(0, at), "utf8");
}

/**
 * 観測記録の byte 列から正規化抜粋を作る。
 *
 * 入力を string にしない。呼び出し側が readFileSync(..., "utf8") で先に復号できると、
 * Node が不正 byte を U+FFFD へ置換した結果に digest が付いてしまう。
 */
export function normalizeCapture(bytes: Uint8Array): string {
  if (!ArrayBuffer.isView(bytes)) {
    fail("normalizeCapture takes raw bytes (Uint8Array), not a decoded string");
  }
  let text: string;
  try {
    text = decodeUtf8(bytes, "capture");
  } catch {
    fail(`capture is not valid UTF-8 (first invalid byte at offset ${firstInvalidUtf8Offset(bytes)})`);
  }

  const tokens = makeTokenTable();
  const lines: string[] = [];
  const source = text.split("\n");
  for (const [index, rawLine] of source.entries()) {
    const lineNo = index + 1;
    if (rawLine.trim() === "") continue;
    let parsed: unknown;
    try {
      parsed = parseIJson(rawLine);
    } catch {
      // 元の例外は行の中身を含み得る。行番号までにする
      fail(`capture line ${lineNo} is not I-JSON (duplicate key, or not JSON)`);
    }
    if (!isDataObject(parsed)) fail(`capture line ${lineNo} is not a JSON object`);
    if (typeof parsed.event !== "string") fail(`capture line ${lineNo} has no string "event"`);
    if (!isDataObject(parsed.payload)) fail(`capture line ${lineNo} has no object "payload"`);
    // capture-hook.sh は hook の stdin を解釈できなかったとき {unparsed: raw} に包む。
    // 取れなかった観測を正常な payload として digest に混ぜない
    if (Object.hasOwn(parsed.payload, "unparsed")) {
      fail(`capture line ${lineNo} carries payload.unparsed (the recorder could not read this hook)`);
    }

    const out = Object.create(null) as { [k: string]: JsonValue };
    for (const key of Object.keys(parsed).sort(byUtf16)) {
      if (key === "at") continue; // 時刻は取得のたびに変わる
      if (key === "event") {
        out.event = verbatim(parsed.event);
      } else if (key === "payload") {
        out.payload = normalizePayload(parsed.payload, tokens);
      } else {
        // 未知の top-level キーは伏せ字で残す。有無そのものは digest に出す
        out[key] = redact(parsed[key] as JsonValue);
      }
    }
    lines.push(canonicalizeJson(out));
  }

  if (lines.length === 0) fail("capture has 0 usable lines");
  return `${lines.join("\n")}\n`;
}

/**
 * 記録が始まった時刻。**1 行目の `at` であって、rig が別に持つ値ではない**。
 * こうすると captureRawHash がこの時刻まで縛る。取り込み側と検証側で二重実装すると
 * 片方だけ緩むので、両者ともこの 1 本を通す。
 */
export function captureCapturedAt(bytes: Uint8Array): string {
  let text: string;
  try {
    text = decodeUtf8(bytes, "capture");
  } catch {
    fail(`capture is not valid UTF-8 (first invalid byte at offset ${firstInvalidUtf8Offset(bytes)})`);
  }
  const first = text.split("\n").find((l) => l.trim() !== "");
  if (first === undefined) fail("capture has 0 usable lines");
  let parsed: unknown;
  try {
    parsed = parseIJson(first);
  } catch {
    fail("capture line 1 is not I-JSON (duplicate key, or not JSON)");
  }
  if (!isDataObject(parsed) || typeof parsed.at !== "string") fail('capture line 1 has no string "at"');
  // 暦として実在する瞬間かをここで見る。manifest 側の pattern は綴りしか当てないので、
  // 2026-02-30 のような値がそのまま verifiedAt として成果物へ出る
  if (!isRealInstant(parsed.at)) fail('capture line 1 "at" is not a real instant on the calendar');
  return parsed.at;
}

/** 綴りは schema の pattern が当てる。ここは「その日付が実在するか」だけを見る */
export function isRealInstant(value: string): boolean {
  const seconds = value.slice(0, 19);
  const parsed = Date.parse(`${seconds}Z`);
  return Number.isFinite(parsed) && new Date(parsed).toISOString().startsWith(seconds);
}

const sha256 = (data: Uint8Array | string): string =>
  createHash("sha256").update(data).digest("hex");

/**
 * 正規化済み抜粋の SHA-256。引数は normalizeCapture の出力でなければならない。
 * 正規化結果を既に持つ呼び出し側が同じ byte 列を二度処理しないための入口。
 */
export function digestNormalized(normalized: string): string {
  return sha256(Buffer.from(normalized, "utf8"));
}

/** 正規化抜粋の SHA-256。再取得しても変わらない側 */
export function digestCapture(bytes: Uint8Array): string {
  return digestNormalized(normalizeCapture(bytes));
}

/** 生 byte の SHA-256。「この記録そのもの」への結び付けに使う */
export function digestRaw(bytes: Uint8Array): string {
  if (!ArrayBuffer.isView(bytes)) fail("digestRaw takes raw bytes (Uint8Array)");
  return sha256(bytes);
}

/** 既定の証拠置き場。module 位置から導くので呼び出し側の cwd に依存しない */
function defaultEvidenceRoot(cli: string): string {
  return join(fileURLToPath(new URL("../fixtures/", import.meta.url)), cli, "raw");
}

/**
 * 証拠置き場の中だけを解決する（data-model.md §3）。
 *
 * `root` は **test 専用の差し替え口**。production の経路は必ず省略する
 * （CLI 引数・fixture の値・環境変数から root が動かないことを test で固定している）。
 */
export function resolveEvidencePath(cli: string, relPath: string, root?: string): string {
  if (!KNOWN_CLIS.has(cli)) fail("unknown cli for the evidence root");
  if (typeof relPath !== "string" || relPath === "") fail("evidence path must be a non-empty string");
  if (isAbsolute(relPath)) fail("evidence path must be relative to the evidence root");
  if (relPath.split(/[\\/]/).includes("..")) fail("evidence path must not contain '..'");

  let realRoot: string;
  try {
    realRoot = realpathSync(root ?? defaultEvidenceRoot(cli));
  } catch {
    fail("the evidence root does not exist");
  }
  let candidate: string;
  try {
    candidate = realpathSync(join(realRoot, relPath));
  } catch {
    fail(`evidence artifact cannot be resolved: ${basename(relPath)}`);
  }
  // 「root で始まる」ではなく「root + 区切りで始まる」。raw と raw-evil を通さない
  if (!candidate.startsWith(realRoot + sep)) {
    fail(`evidence path resolves outside the evidence root: ${basename(relPath)}`);
  }
  if (!lstatSync(candidate).isFile()) {
    fail(`evidence path is not a regular file: ${basename(relPath)}`);
  }
  return candidate;
}

// --- CLI ---
//
// 引数で渡した path はそのまま読む（resolveEvidencePath は通さない）。rig は置き場の外
// $RIG_BASE/capture/ から呼ぶ。置き場の制約は fixture の申告値に対して assemble 側で掛かる。

const USAGE =
  "usage: node --experimental-strip-types harness/evidence/normalize.ts <capture-file>\n" +
  "       node --experimental-strip-types harness/evidence/normalize.ts --raw <file>";

function main(argv: string[]): number {
  const rawMode = argv[0] === "--raw";
  const args = rawMode ? argv.slice(1) : argv;
  if (args.length !== 1 || args[0].startsWith("--")) {
    process.stderr.write(`${USAGE}\n`);
    return 2;
  }
  let bytes: Uint8Array;
  try {
    bytes = readFileSync(args[0]);
  } catch {
    process.stderr.write(`cannot read ${basename(args[0])}\n`);
    return 2;
  }
  try {
    const result = rawMode
      ? { rawHash: digestRaw(bytes) }
      : {
          evidenceHash: digestCapture(bytes),
          captureRawHash: digestRaw(bytes),
          normalizationVersion: NORMALIZATION_VERSION,
        };
    process.stdout.write(`${JSON.stringify(result)}\n`);
    return 0;
  } catch (e) {
    // EvidenceError の message は行番号と basename までで、中身と絶対 path を含まない
    process.stderr.write(`${e instanceof EvidenceError ? e.message : "capture cannot be normalized"}\n`);
    return 2;
  }
}

// import されたときは CLI を起動しない
const invokedDirectly =
  process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) process.exit(main(process.argv.slice(2)));
