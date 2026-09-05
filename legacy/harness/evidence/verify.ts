/**
 * fixture が名指しした観測記録を読み直し、digest を再計算し、cell の主張を記録から導く。
 * 規則は specs/003-evidence-hash-normalization/data-model.md §4。
 *
 * ここが「自己申告で real-cli-e2e を名乗れる」経路を塞ぐ本体。失敗はすべて throw で、
 * 当該 cell を黙って下位の証拠強度へ落として続行しない（FR-005）。
 */
import { readFileSync } from "node:fs";
import {
  NORMALIZATION_VERSION,
  captureCapturedAt,
  digestNormalized,
  digestRaw,
  normalizeCapture,
  resolveEvidencePath,
} from "./normalize.ts";
import { decodeUtf8, parseIJson, readIJsonFile } from "../schema/jcs.ts";
import { validateAgainstSchema, type JsonSchemaDocument } from "../schema/validate.ts";
import type {
  CaptureFixture,
  EventKind,
  EvidenceRef,
  EvidenceSource,
  ObservedCapability,
  RunManifest,
} from "../schema/capability.ts";

const MANIFEST_SCHEMA = readIJsonFile(
  new URL("../schema/evidence-manifest.schema.json", import.meta.url),
) as JsonSchemaDocument;

/** harness が実装している manifest の版。違う版は推測で読まない */
export const MANIFEST_VERSION = 1;

/** 高位 cell のうち、観測記録から導ける主張を持つもの */
export type DerivableHighLevelKey = "subagentCapture" | "stableNativeSessionId";

/**
 * 導ける主張の一覧。**`deriveClaims` が書き込むキーと 1 対 1**にする。
 * ここに載っていない主張は、証拠があっても real-cli-e2e を名乗れない（§4.3 / FR-006c）。
 * 本文に依存する主張（tool_failed の phase・注入が効いたか）は、正規化が本文を伏せる以上
 * 再取得安定な digest と両立しないので、別の証拠形式で扱う。
 */
export const DERIVABLE_CAPTURE_KINDS: ReadonlySet<EventKind> = new Set<EventKind>([
  "session_started",
  "user_prompted",
  "session_ended",
  "tool_started",
  "tool_completed",
  "assistant_completed",
  "session_interrupted",
  "turn_completed",
]);

export const DERIVABLE_HIGH_LEVEL_KEYS: ReadonlySet<string> = new Set<string>([
  "subagentCapture",
  "stableNativeSessionId",
]);

/** 検証器の出力。この記録が支持する主張だけを持つ */
export interface VerifiedRef {
  index: number;
  path: string;
  /** manifest を伴わない ref は legacy 証拠で、real-cli-e2e の根拠にならない */
  manifestBacked: boolean;
  source: EvidenceSource;
  /** この記録に実在した hook 名。申告された sourceEvents の実在検査に使う */
  events: string[];
  /**
   * cell ごとに、**その値を実際に導いた** hook 名。`events` は記録に在るかしか言わないので、
   * 別の hook 由来だと申告しても通ってしまう（assistant_completed を SessionStart 由来と書く等）
   */
  captureSources: Partial<Record<EventKind, string[]>>;
  /** この記録の 1 行目の `at`。昇格した cell の verifiedAt はここから来る */
  capturedAt: string;
  /**
   * この記録の秘密欄にあった 16 文字以上の文字列。**成果物へ出す前の警報にだけ使う**。
   * 診断にも出力にも載せない（載せたら検査そのものが漏洩経路になる）。
   */
  secrets: string[];
  capture: Partial<Record<EventKind, ObservedCapability>>;
  highLevel: Partial<Record<DerivableHighLevelKey, ObservedCapability>>;
}

/** evidence root の差し替え口。**test 専用**で、production の入口は受け取らない */
export interface EvidenceContext {
  evidenceRoot?: string;
}

export class EvidenceVerificationError extends Error {}

function reject(fixtureId: string, message: string): never {
  throw new EvidenceVerificationError(`${fixtureId}: ${message}`);
}

/** 生の記録から秘密欄の文字列を拾う。正規化後には残らないのでここで取る */
export function collectSecretsOf(bytes: Uint8Array): Set<string> {
  const out = new Set<string>();
  for (const line of decodeUtf8(bytes, "capture").split("\n")) {
    if (line.trim() === "") continue;
    collectSecrets(parseIJson(line), false, out);
  }
  return out;
}

/** 正規化抜粋を行ごとの値に戻す。**導出は正規化後の情報だけを見る**（FR-006b の担保） */
interface NormalizedLine {
  event: string;
  payload: Record<string, unknown>;
}

function readNormalized(text: string): NormalizedLine[] {
  const lines: NormalizedLine[] = [];
  for (const line of text.split("\n")) {
    if (line === "") continue;
    const value = JSON.parse(line) as NormalizedLine;
    lines.push(value);
  }
  return lines;
}

/**
 * 「欄が在る」の判定。正規化後の**非空文字列の伏せ字**だけを認める。型を緩めると、
 * `last_assistant_message: null`（伏せ字 `null`）も `: 123`（伏せ字 `<number>`）も
 * 「応答があった」の根拠になる
 */
const has = (line: NormalizedLine, key: string): boolean => line.payload[key] === "<string>";

/**
 * 成果物へ出てはいけない値の出どころ。入れ子も辿る。
 * export しているのは、網羅 test が固定した綴りの一覧とこの集合を突き合わせるため。
 * 集合から payload を組ませると、欄を**外した**変異まで test が一緒に縮んで気づけない
 * （変異 M24 が実際に生き残った）ので、比較する形にしてある
 */
const SECRET_KEY_SET = new Set([
  "prompt",
  "last_assistant_message",
  "cwd",
  "transcript_path",
  "agent_transcript_path",
]);
const SECRET_SUBTREE_SET = new Set(["tool_input", "tool_response"]);
// 収集が見るのは上の集合で、export するのはその複製。`ReadonlySet` は型の上の約束でしかなく、
// 型は実行前に剥がされるので、export した Set は同じ process の別 module が `delete` できる
// ——警報の対象を実行時に減らせると、その欄の値が成果物へ出ても誰も気づかない
export const SECRET_KEYS: readonly string[] = [...SECRET_KEY_SET];
export const SECRET_SUBTREES: readonly string[] = [...SECRET_SUBTREE_SET];

/** 警報用の材料を **正規化前の** 記録から集める（正規化は伏せてしまうため） */
function collectSecrets(value: unknown, inSubtree: boolean, out: Set<string>): void {
  if (typeof value === "string") {
    if (inSubtree && value.length >= 16) out.add(value);
    return;
  }
  if (Array.isArray(value)) {
    for (const item of value) collectSecrets(item, inSubtree, out);
    return;
  }
  if (typeof value !== "object" || value === null) return;
  for (const [key, v] of Object.entries(value)) {
    const secret = inSubtree || SECRET_KEY_SET.has(key) || SECRET_SUBTREE_SET.has(key);
    collectSecrets(v, secret, out);
  }
}
/**
 * 相関 token だけを返す。数値の session_id は正規化で `<number>` になるため、
 * 型だけ見ると別々の ID が同じ token として等値になる（run 全体で安定に見える）
 */
const CORRELATION_TOKEN = /^<(?:id|path):\d+>$/;
const tokenOf = (line: NormalizedLine, key: string): string | undefined => {
  const v = line.payload[key];
  return typeof v === "string" && CORRELATION_TOKEN.test(v) ? v : undefined;
};

/**
 * 正規化後に残る情報（event の種類と並び・欄の有無・識別子の等値関係・boolean）だけから
 * cell の値を導く。ここに無い主張は「導けない」ので real-cli-e2e を名乗れない（§4.3）。
 */
function deriveClaims(
  cli: "claude" | "codex",
  lines: NormalizedLine[],
): Pick<VerifiedRef, "events" | "capture" | "captureSources" | "highLevel"> {
  const events = lines.map((l) => l.event);
  const seen = (name: string): boolean => events.includes(name);
  const capture: Partial<Record<EventKind, ObservedCapability>> = {};
  const captureSources: Partial<Record<EventKind, string[]>> = {};
  const highLevel: Partial<Record<DerivableHighLevelKey, ObservedCapability>> = {};
  /** 値とその出どころを同時に置く。片方だけ書くと申告の照合が緩む */
  const derived = (kind: EventKind, value: ObservedCapability, from: string[]): void => {
    capture[kind] = value;
    captureSources[kind] = from;
  };

  if (seen("SessionStart")) derived("session_started", "native", ["SessionStart"]);
  if (seen("UserPromptSubmit")) derived("user_prompted", "native", ["UserPromptSubmit"]);
  if (seen("SessionEnd")) derived("session_ended", "native", ["SessionEnd"]);
  if (seen("PreToolUse")) derived("tool_started", "native", ["PreToolUse"]);
  if (seen("PostToolUse")) derived("tool_completed", "native", ["PostToolUse"]);

  const stops = lines.filter((l) => l.event === "Stop");
  // 値は伏せ字でよい。欄が在ることが「Stop から復元できる」の根拠
  if (stops.some((l) => has(l, "last_assistant_message"))) derived("assistant_completed", "synthesized", ["Stop"]);

  // Stop が無く SessionEnd がある = 中断。値ではなく並びから導く
  if (stops.length === 0 && seen("SessionEnd")) derived("session_interrupted", "synthesized", ["SessionEnd"]);

  // turn の対応付けは CLI で規則が違う。Claude は prompt_id の共有（turn_id は無い）、
  // Codex は turn_id の共有。Claude 規則へ統一すると Codex fixture 3 件が落ちる
  const prompts = lines.filter((l) => l.event === "UserPromptSubmit");
  const shares = (key: string): boolean => {
    const a = prompts.map((l) => tokenOf(l, key)).filter((t) => t !== undefined);
    const b = stops.map((l) => tokenOf(l, key)).filter((t) => t !== undefined);
    return a.length > 0 && b.length > 0 && a.some((t) => b.includes(t));
  };
  if (cli === "codex") {
    if (shares("turn_id")) derived("turn_completed", "native", ["UserPromptSubmit", "Stop"]);
    // 識別子の「在る」は has() ではなく相関 token で見る。has() は非空文字列の伏せ字
    // （`<string>`）だけを認めるので、`<id:N>` になる turn_id には当たらない
  } else if (shares("prompt_id") && !lines.some((l) => tokenOf(l, "turn_id") !== undefined)) {
    derived("turn_completed", "synthesized", ["UserPromptSubmit", "Stop"]);
  }

  if (seen("SubagentStop")) highLevel.subagentCapture = "native";

  // 相関 token のおかげで「run を通して同一か」が値を出さずに判定できる。
  // 欄が無い行を先に除くと、1 行だけ id を持つ記録が「run 全体で安定」と判定される
  const sessionTokens = lines.map((l) => tokenOf(l, "session_id"));
  if (sessionTokens.length >= 2 && new Set(sessionTokens).size === 1 && sessionTokens[0] !== undefined) {
    highLevel.stableNativeSessionId = "native";
  }

  return { events: [...new Set(events)].sort(), capture, captureSources, highLevel };
}

function verifyManifest(
  f: CaptureFixture,
  ref: EvidenceRef,
  ctx: EvidenceContext | undefined,
  computed: { evidenceHash: string; captureRawHash: string; capturedAt: string },
): RunManifest {
  const manifestPath = resolveEvidencePath(f.cli, ref.manifest as string, ctx?.evidenceRoot);
  let bytes: Uint8Array;
  try {
    bytes = readFileSync(manifestPath);
  } catch {
    reject(f.fixtureId, `manifest cannot be read: ${ref.manifest}`);
  }
  // parse の前に生 byte を照合する。壊れた manifest を parse しない
  if (digestRaw(bytes) !== ref.manifestHash) {
    reject(f.fixtureId, `manifestHash mismatch for ${ref.manifest}`);
  }
  let manifest: RunManifest;
  try {
    manifest = parseIJson<RunManifest>(decodeUtf8(bytes, "manifest"));
  } catch {
    reject(f.fixtureId, `manifest is not I-JSON: ${ref.manifest}`);
  }
  const issues = validateAgainstSchema(manifest, MANIFEST_SCHEMA, MANIFEST_SCHEMA);
  if (issues.length > 0) {
    reject(f.fixtureId, `manifest does not match the schema: ${issues.map((i) => `${i.path} ${i.message}`).join("; ")}`);
  }

  // §2.5 の照合表。1 項目でも違えば失敗
  const checks: Array<[string, boolean]> = [
    ["manifestVersion", manifest.manifestVersion === MANIFEST_VERSION],
    ["cli", manifest.cli === f.cli],
    ["cliVersion", manifest.cliVersion === f.nativeVersion],
    ["scenarioId", manifest.scenarioId === f.scenarioId],
    // 記録の 1 行目から導いた時刻に縛る。**fixture の capturedAt とは比べない**:
    // 1 つの fixture は複数の run を束ねるので、fixture 単位で縛ると 2 本目以降の
    // manifest が構造的に通らなくなる（claude/interrupt-and-hook-timeout は 5 本参照する）
    ["capturedAt", manifest.capturedAt === computed.capturedAt],
    ["capture", manifest.capture === ref.path],
    ["captureRawHash", manifest.captureRawHash === computed.captureRawHash],
    ["captureHash", manifest.captureHash === computed.evidenceHash],
    [
      "normalizationVersion",
      manifest.normalizationVersion === ref.normalizationVersion &&
        manifest.normalizationVersion === NORMALIZATION_VERSION,
    ],
    // 「fixture と一致」ではなく「true であること」。一致だけを見ると双方 false で通る
    ["isolated", manifest.isolated === true],
    ["internalRunMarker", manifest.internalRunMarker === true],
    ["recorderErrors", manifest.recorderErrors === 0],
  ];
  for (const [name, ok] of checks) {
    if (!ok) reject(f.fixtureId, `manifest ${name} does not match the fixture or the capture`);
  }
  return manifest;
}

/**
 * fixture の全 ref を検証する。1 件でも通らなければ組み立て全体を失敗させる。
 * `evidence` を持たない fixture は空配列を返す（実 CLI 観測を名乗っていない）。
 */
export function verifyEvidence(f: CaptureFixture, ctx?: EvidenceContext): VerifiedRef[] {
  const refs = f.evidence;
  if (refs === undefined) return [];
  // schema の minItems: 1 と二重にする。schema を通さない経路が増えたときの穴を残さない
  if (!Array.isArray(refs) || refs.length === 0) {
    reject(f.fixtureId, "evidence must not be empty (an empty list is not 'all refs verified')");
  }

  // 同じ記録を 2 度名指しできると、昇格は manifest 付きの ref で決まる一方、公開する
  // evidenceSources は後勝ちで legacy 側になる（最高位 cell が manifest 無しの source を指す）
  const seenPaths = new Set<string>();
  for (const ref of refs) {
    if (seenPaths.has(ref.path)) reject(f.fixtureId, `evidence names ${ref.path} more than once`);
    seenPaths.add(ref.path);
  }

  return refs.map((ref, index) => {
    if (ref.normalizationVersion !== NORMALIZATION_VERSION) {
      reject(f.fixtureId, `unknown normalizationVersion ${ref.normalizationVersion} (this harness implements ${NORMALIZATION_VERSION})`);
    }
    const capturePath = resolveEvidencePath(f.cli, ref.path, ctx?.evidenceRoot);
    let bytes: Uint8Array;
    try {
      bytes = readFileSync(capturePath);
    } catch {
      reject(f.fixtureId, `evidence artifact cannot be read: ${ref.path}`);
    }
    // 生 byte を先に見る。正規化が伏せる差だけを変えた記録をここで落とす
    const captureRawHash = digestRaw(bytes);
    if (captureRawHash !== ref.captureRawHash) {
      reject(f.fixtureId, `captureRawHash mismatch for ${ref.path}`);
    }
    let normalized: string;
    try {
      normalized = normalizeCapture(bytes);
    } catch (e) {
      reject(f.fixtureId, `evidence artifact cannot be normalized: ${ref.path} (${(e as Error).message})`);
    }
    const evidenceHash = digestNormalized(normalized);
    if (evidenceHash !== ref.evidenceHash) {
      reject(f.fixtureId, `evidenceHash mismatch for ${ref.path}`);
    }

    // hook が名乗った event 名は settings.json の配線（= 我々が hook へ渡した argv）であって、
    // CLI の言い分ではない。配線を間違えると、CLI が送っていない event を「native で観測した」と
    // 名乗れる。payload の hook_event_name は CLI 自身が書くので、両方が一致した記録だけを根拠にする。
    // 失敗の説明に記録側の文字列は出さない（記録の中身で FR-015 を破れる）
    const lines = readNormalized(normalized);
    for (const [i, line] of lines.entries()) {
      if (line.payload["hook_event_name"] !== line.event) {
        reject(f.fixtureId, `capture ${ref.path} line ${i + 1}: the CLI payload does not confirm the recorded hook event`);
      }
    }

    const capturedAt = captureCapturedAt(bytes);
    const manifestBacked = ref.manifest !== undefined;
    if (manifestBacked) verifyManifest(f, ref, ctx, { evidenceHash, captureRawHash, capturedAt });

    return {
      index,
      path: ref.path,
      manifestBacked,
      capturedAt,
      source: {
        fixtureId: f.fixtureId,
        path: ref.path,
        evidenceHash,
        normalizationVersion: ref.normalizationVersion,
        manifestHash: ref.manifestHash ?? null,
        cliVersion: f.nativeVersion,
        scenarioId: f.scenarioId,
      },
      secrets: [...collectSecretsOf(bytes)],
      ...deriveClaims(f.cli, lines),
    };
  });
}
