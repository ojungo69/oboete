import { readdir, readFile, writeFile, mkdir, mkdtemp, rm } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { tmpdir } from "node:os";
import {
  EVENT_KINDS,
  TOOL_FAILURE_PHASES,
  emptyMatrix,
  resolveResumeDeliveryStrategy,
  type AdapterCapabilities,
  type CaptureFixture,
  type CompactionRecoveryStrategy,
  type EventKind,
  type EvidenceSource,
  type ObservedCapability,
  type ToolFailurePhase,
} from "./schema/capability.ts";
import { validateAgainstSchema, type JsonSchemaDocument } from "./schema/validate.ts";
import { canonicalizeJson, decodeUtf8, parseIJson, readIJsonFile } from "./schema/jcs.ts";
import { NORMALIZATION_VERSION, digestCapture, digestRaw, isRealInstant } from "./evidence/normalize.ts";
import {
  DERIVABLE_CAPTURE_KINDS,
  DERIVABLE_HIGH_LEVEL_KEYS,
  verifyEvidence,
  type DerivableHighLevelKey,
  type EvidenceContext,
  type VerifiedRef,
} from "./evidence/verify.ts";

// JSON Schema は「置いてあるだけ」にせず、キー集合の正本として実際に読む
const SCHEMA = readIJsonFile(new URL("./schema/capability.schema.json", import.meta.url)) as JsonSchemaDocument & { properties?: Record<string, any> };

// SCHEMA は起動時に 1 度読むだけなので、そこから引く集合も module 読み込み時に固める
const KNOWN_KEYS = new Set(Object.keys(SCHEMA.properties ?? {}));
const KNOWN_EVENT_KEYS = new Set(Object.keys(SCHEMA.properties?.observedEvents?.items?.properties ?? {}));

const EVENT_KIND_SET = new Set<string>(EVENT_KINDS);
const TOOL_FAILURE_PHASE_SET = new Set<string>(TOOL_FAILURE_PHASES);
const CLI_SET = new Set(["claude", "codex"]);

// 高位 cell の強さ。後勝ちで降格させないための順序（capture[kind] の native>synthesized と同じ考え）
const CAPABILITY_STRENGTH: Record<string, number> = { unsupported: 0, synthesized: 1, native: 2 };

function fail(msg: string): never {
  console.error(msg);
  process.exit(1);
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** Minimal CaptureFixture validation (no ajv; required keys + enum checks). */
export function validateFixture(data: unknown, fileName: string): CaptureFixture {
  const errs: string[] = [];
  if (!isObject(data)) {
    throw new Error(`${fileName}: not a JSON object`);
  }

  const req = [
    "fixtureId",
    "cli",
    "nativeVersion",
    "capturedAt",
    "scenario",
    "scenarioId",
    "observedEvents",
    "toolFailurePhasesObserved",
    "limitations",
    "limitationCodes",
    "rig",
  ] as const;
  for (const k of req) {
    if (!(k in data)) errs.push(`missing required key: ${k}`);
  }

  if (typeof data.fixtureId !== "string" || data.fixtureId.length === 0) {
    errs.push("fixtureId must be non-empty string");
  }
  if (typeof data.cli !== "string" || !CLI_SET.has(data.cli)) {
    errs.push('cli must be "claude" | "codex"');
  }
  if (typeof data.nativeVersion !== "string" || data.nativeVersion.length === 0) {
    errs.push("nativeVersion must be non-empty string");
  }
  if (typeof data.capturedAt !== "string" || data.capturedAt.length === 0) {
    errs.push("capturedAt must be non-empty string");
  }
  if (typeof data.scenario !== "string" || data.scenario.length === 0) {
    errs.push("scenario must be non-empty string");
  }
  if (typeof data.scenarioId !== "string" || data.scenarioId.length === 0) {
    errs.push("scenarioId must be non-empty string");
  }

  if (!Array.isArray(data.observedEvents)) {
    errs.push("observedEvents must be array");
  } else {
    data.observedEvents.forEach((ev, i) => {
      if (!isObject(ev)) {
        errs.push(`observedEvents[${i}] must be object`);
        return;
      }
      if (typeof ev.kind !== "string") {
        errs.push(`observedEvents[${i}].kind must be string`);
      } else if (ev.kind !== "raw" && !EVENT_KIND_SET.has(ev.kind)) {
        errs.push(`observedEvents[${i}].kind is not one of the kinds capability.schema.json lists`);
      }
      if ("at" in ev && typeof ev.at !== "string") {
        errs.push(`observedEvents[${i}].at must be string if present`);
      }
      if ("capability" in ev && ev.capability !== "native" && ev.capability !== "synthesized") {
        errs.push(`observedEvents[${i}].capability must be "native" | "synthesized"`);
      }
      if (ev.capability === "synthesized" && (!Array.isArray(ev.sourceEvents) || ev.sourceEvents.length === 0)) {
        errs.push(`observedEvents[${i}]: synthesized requires non-empty sourceEvents (§7.2)`);
      }
      // 散文とコードを位置対応させる。散文を足してコードを足し忘れた状態をここで落とす
      // （成果物へ出るのはコードだけなので、対応が崩れると caveat が黙って消える）
      const proseCount = Array.isArray(ev.limitations) ? ev.limitations.length : 0;
      const codeCount = Array.isArray(ev.limitationCodes) ? ev.limitationCodes.length : 0;
      if (proseCount !== codeCount) {
        errs.push(`observedEvents[${i}]: limitationCodes must line up 1:1 with limitations (${proseCount} vs ${codeCount})`);
      }
    });
  }

  if (!Array.isArray(data.toolFailurePhasesObserved)) {
    errs.push("toolFailurePhasesObserved must be array");
  } else {
    data.toolFailurePhasesObserved.forEach((p, i) => {
      if (typeof p !== "string" || !TOOL_FAILURE_PHASE_SET.has(p)) {
        errs.push(`toolFailurePhasesObserved[${i}] is not one of the phases capability.schema.json lists`);
      }
    });
  }

  if (!Array.isArray(data.limitations)) {
    errs.push("limitations must be array");
  } else if (!data.limitations.every((x) => typeof x === "string")) {
    errs.push("limitations items must be strings");
  }
  if (!Array.isArray(data.limitationCodes)) {
    errs.push("limitationCodes must be array");
  } else if (Array.isArray(data.limitations) && data.limitationCodes.length !== data.limitations.length) {
    errs.push(
      `limitationCodes must line up 1:1 with limitations (${data.limitations.length} vs ${data.limitationCodes.length})`,
    );
  }

  // schema 側を正本にして **fixture 全体**を検査する。欄を選んで委譲すると、選ばれなかった
  // 欄の制約が誰にも読まれないまま残る（fixtureId / nativeVersion / capturedAt の pattern が
  // 実際にそうなっていた: schema には書いたのに検査に載らず、制御文字と任意の自由文が
  // sourceFixtureId・verifiedAt として公開 matrix へ出せた）
  for (const issue of validateAgainstSchema(data, SCHEMA, SCHEMA)) {
    errs.push(`${issue.path}: ${issue.message}`);
  }
  // pattern は桁数しか見ない。`2026-99-99T99:99:99Z` は範囲を絞っても 2 月 30 日が残るので、
  // 暦として実在する瞬間かを別に確かめる（continuity 側 reference-model.ts の isRealInstant と同型。
  // 部分系どうしを import で結ばないぶん、この 3 行は意図的な重複）
  // fixtureId は公開 matrix の帰属（fixtureIds / sourceFixtureId / evidenceSources）になる。
  // 書式だけ見ていると cli:"claude" の観測を codex/... へ付け替えられる
  // 診断に data.cli の生値を混ぜない。schema を通る前の値なので改行を入れて CI log の
  // 行を偽装でき、fixture に紛れた秘密も stderr へ出る
  if (typeof data.fixtureId === "string" && typeof data.cli === "string" && CLI_SET.has(data.cli)) {
    if (!data.fixtureId.startsWith(`${data.cli}/`)) errs.push("fixtureId must be prefixed with its own cli");
  }

  const instants: Array<[string, unknown]> = [["capturedAt", data.capturedAt]];
  if (Array.isArray(data.observedEvents)) {
    data.observedEvents.forEach((ev, i) => {
      if (isObject(ev)) instants.push([`observedEvents[${i}].at`, ev.at]);
    });
  }
  for (const [label, value] of instants) {
    if (typeof value === "string" && !isRealInstant(value)) {
      errs.push(`${label}: not a real instant on the calendar`);
    }
  }

  if (!isObject(data.rig)) {
    errs.push("rig must be object");
  } else {
    // 隔離 rig 外で取った capture を real-cli-e2e として matrix に載せない
    if (data.rig.isolated !== true) errs.push("rig.isolated must be true (隔離 rig 外の capture は採用しない)");
    if (typeof data.rig.internalRunMarker !== "boolean") {
      errs.push("rig.internalRunMarker must be boolean");
    }
  }

  // JSON Schema と手書き検証の drift 防止: schema が知らないキーは弾く。
  // key 名は fixture の中身なので診断へ載せない（何番目か、だけで場所は足りる）
  for (const [n, k] of Object.keys(data).entries()) {
    if (!KNOWN_KEYS.has(k)) errs.push(`unknown top-level key #${n + 1} (capability.schema.json 未定義)`);
  }
  if (Array.isArray(data.observedEvents)) {
    for (const [i, ev] of data.observedEvents.entries()) {
      if (!isObject(ev)) continue;
      for (const [n, k] of Object.keys(ev).entries()) {
        if (!KNOWN_EVENT_KEYS.has(k)) errs.push(`observedEvents[${i}]: unknown key #${n + 1} (schema 未定義)`);
      }
    }
  }

  // throw にしておく（呼び出し側の loadFixtures が exit へ変換する）。
  // process.exit だと不正 fixture の棄却をテストから確認できない
  if (errs.length > 0) {
    throw new Error(`${fileName}: ${errs.join("; ")}`);
  }

  return data as unknown as CaptureFixture;
}

export interface AssembledMatrix {
  cli: "claude" | "codex";
  nativeVersion: string;
  generatedAt: string;
  fixtureCount: number;
  fixtureIds: string[];
  fixtureLimitations: string[]; // fixture 単位の caveat（cell に紐づかないもの）
  // どの観測記録が cell を裏付けたか。cell 側は evidenceRefs で添字を持つ
  evidenceSources: EvidenceSource[];
  capabilities: AdapterCapabilities;
}

function dedupe(xs: string[]): string[] {
  return [...new Set(xs)];
}

/** 成果物に現れる全文字列を集める（キー名は生成側が決めるので値だけ見る） */
function collectStrings(value: unknown, out: string[]): void {
  if (typeof value === "string") {
    out.push(value);
    return;
  }
  if (Array.isArray(value)) {
    for (const item of value) collectStrings(item, out);
    return;
  }
  if (typeof value === "object" && value !== null) {
    for (const v of Object.values(value)) collectStrings(v, out);
  }
}

const SECRET_WINDOW = 16;

/**
 * 参照した観測記録の秘密欄から取った 16 文字以上の部分文字列が成果物に現れたら失敗させる。
 * **一致した文字列そのものは診断に出さない**（出したら検査が漏洩経路になる）。
 */
export function assertNoSecretSubstrings(artifact: unknown, secrets: string[]): void {
  if (secrets.length === 0) return;
  // 窓は**成果物の側**から作る。秘密の側から作ると、記録に置いた 1 本の長大な値が
  // そのまま Set の大きさになり、検査自体を落とせる（成果物の大きさはこちらが決める）。
  // 16 文字以上の共通部分文字列があれば、必ずどこかで長さ 16 の窓が両側で一致する
  const windows = new Set<string>();
  const strings: string[] = [];
  collectStrings(artifact, strings);
  for (const s of strings) {
    for (let i = 0; i + SECRET_WINDOW <= s.length; i++) windows.add(s.slice(i, i + SECRET_WINDOW));
  }
  if (windows.size === 0) return;
  for (const secret of secrets) {
    for (let i = 0; i + SECRET_WINDOW <= secret.length; i++) {
      if (windows.has(secret.slice(i, i + SECRET_WINDOW))) {
        throw new Error(
          "generated output carries a 16+ character substring of a referenced capture's secret field " +
            "(the free-text path into the matrix is open again — see data-model.md §5.3)",
        );
      }
    }
  }
}

/**
 * 組み立ての本体。
 *
 * `ctx` は evidence root の差し替え口で、**test 専用**。production の入口 `runAssemble` は
 * これを受け取らないので、CLI 引数・fixture の値・環境変数のどれからも root は動かない。
 */
/**
 * 対が成立する条件。「同じ fixture」では足りない（1 つの fixture が複数の run を束ねる）。
 * 両 cell を裏付けた記録に同じものが 1 件でもあることを要求する。
 * 呼び出し側の経路は現在到達しないので、述語そのものを直接 test で固定する
 */
export const shareRef = (a: number[], b: number[]): boolean => a.some((i) => b.includes(i));

export function assembleFromFixtures(fixtures: CaptureFixture[], ctx?: EvidenceContext): AssembledMatrix {
  // この関数のガードは fail ではなく throw にする。process.exit だとテストから確認できず、
  // --test 実行中に踏むとスイート全体が途中で死ぬ。CLI 側は catch して同じ終了コードを返す
  if (fixtures.length === 0) {
    throw new Error("no valid fixtures to assemble");
  }
  // fixtureId は cell 間の「同一の実測か」の照合キー（capability.ts sameEvidenceSource）。
  // 重複したまま通すと別 run 同士を同一実測と誤認する
  const seenIds = new Set<string>();
  for (const f of fixtures) {
    if (seenIds.has(f.fixtureId)) throw new Error(`duplicate fixtureId: ${f.fixtureId}`);
    seenIds.add(f.fixtureId);
  }

  // 一意と分かってから fixtureId で正規化する。同格な観測どうしの勝ち負けが「どの順で
  // 渡したか」で変わると、matrix の provenance が readdir の並びや呼び出し側の都合で入れ替わる。
  // ponytail: 同格の勝者は fixtureId 順で決まるだけで、新しい観測を優先はしない。
  // verifiedAt を「最後に確認した時刻」として読ませたくなったら畳み込みに recency 規則が要る
  fixtures = [...fixtures].sort((a, b) => (a.fixtureId < b.fixtureId ? -1 : 1));

  const cli = fixtures[0].cli;
  const nativeVersion = fixtures[0].nativeVersion;
  for (const f of fixtures) {
    if (f.cli !== cli) {
      throw new Error(`cli mismatch: ${fixtures[0].fixtureId}=${cli} vs ${f.fixtureId}=${f.cli}`);
    }
    if (f.nativeVersion !== nativeVersion) {
      throw new Error(
        `version-pin violation: ${fixtures[0].fixtureId}=${nativeVersion} vs ${f.fixtureId}=${f.nativeVersion}`,
      );
    }
  }

  // 名指しされた観測記録を全件読み直して digest を再計算する。fixture の申告値は信用しない。
  // 1 件でも通らなければここで throw する（当該 cell を黙って source-test へ落とさない）
  const verifiedByFixture = new Map<string, VerifiedRef[]>();
  for (const f of fixtures) verifiedByFixture.set(f.fixtureId, verifyEvidence(f, ctx));

  // 申告した hook 名が観測記録に実在するか。正しい raw と digest のまま主張だけ足す経路を塞ぐ
  for (const f of fixtures) {
    const refs = verifiedByFixture.get(f.fixtureId) ?? [];
    if (refs.length === 0) continue;
    const observed = new Set(refs.flatMap((r) => r.events));
    for (const [i, ev] of f.observedEvents.entries()) {
      for (const name of ev.sourceEvents ?? []) {
        if (!observed.has(name)) {
          throw new Error(`${f.fixtureId}: observedEvents[${i}].sourceEvents names ${name}, which no referenced capture contains`);
        }
      }
    }
  }

  // 証拠の表は fixtureId → path の昇順で一意化する。cell からは添字で参照する
  const sourceKey = (s: EvidenceSource): string => `${s.fixtureId}\u0000${s.path}`;
  const sourceMap = new Map<string, EvidenceSource>();
  for (const refs of verifiedByFixture.values()) {
    for (const r of refs) sourceMap.set(sourceKey(r.source), r.source);
  }
  const evidenceSources = [...sourceMap.values()].sort((a, b) =>
    sourceKey(a) < sourceKey(b) ? -1 : sourceKey(a) > sourceKey(b) ? 1 : 0,
  );
  const sourceIndex = new Map(evidenceSources.map((s, i) => [sourceKey(s), i]));

  /** 遅いほうの時刻。読めない値が来たら文字列比較に落とす（判定を止めない） */
  const laterInstant = (a: string, b: string): string => {
    const [x, y] = [Date.parse(a), Date.parse(b)];
    if (Number.isNaN(x) || Number.isNaN(y)) return a > b ? a : b;
    return x >= y ? a : b;
  };

  interface Promotion {
    evidenceKind: "real-cli-e2e" | "source-test";
    evidenceRefs: number[];
    /** 昇格したときだけ、根拠になった記録の時刻。昇格していなければ null */
    verifiedAt: string | null;
  }
  const UNVERIFIED = "unverified: no manifest-backed evidence";

  /**
   * cell ごとの昇格判定（data-model.md §4.2）。**判定の順序が効く**:
   * 種別（導けるか）を先に見ないと、導出値が存在しない主張は supporting が必ず空になり、
   * source-test に留まるべき既存 fixture が組み立て全体を落とす。
   */
  const promoteCell = (
    f: CaptureFixture,
    derivable: boolean,
    derive: (r: VerifiedRef) => { value: ObservedCapability | undefined; sources: readonly string[] },
    declared: ObservedCapability,
    label: string,
    claimedEvents: readonly string[] = [],
  ): Promotion => {
    const refs = verifiedByFixture.get(f.fixtureId) ?? [];
    if (!derivable || refs.length === 0) {
      return { evidenceKind: "source-test", evidenceRefs: [], verifiedAt: null };
    }
    // fixture は複数の run の和集合を表す。どれか 1 件が同じ値を導けば成立する
    // （全 ref が全主張を支持することは要求しない。要求すると既存 fixture が全件落ちる）
    const supporting = refs.filter((r) => derive(r).value === declared);
    if (supporting.length === 0) {
      throw new Error(`${f.fixtureId}: ${label} claims "${declared}" but no referenced capture derives it`);
    }
    // 昇格の根拠は「値を導き、申告した hook 名がその導出に**実際に使われた**」1 本であること。
    // 「記録に在る」で足りるとすると、assistant_completed を SessionStart 由来と書くような
    // 出どころの偽装が通る。fixture 全体の和集合で足りるとすると、申告した hook が別の run に
    // しか無い組み合わせが通る（和集合の実在検査は別に残してある）
    const backed = supporting.filter(
      (r) => r.manifestBacked && claimedEvents.every((n) => derive(r).sources.includes(n)),
    );
    const chosen = backed.length > 0 ? backed : supporting;
    return {
      // manifest を伴わない legacy 証拠だけなら source-test に留める。
      // digest は「記録が申告どおりか」しか言わず、「実 CLI を隔離 rig で動かした」は
      // 取得側が書いた manifest でしか裏付けられない
      evidenceKind: backed.length > 0 ? "real-cli-e2e" : "source-test",
      evidenceRefs: chosen.map((r) => sourceIndex.get(sourceKey(r.source)) ?? -1).sort((a, b) => a - b),
      // 公開する時刻も記録側から取る。fixture の自己申告を verifiedAt に載せない。
      // 文字列比較では並ばない（小数秒の桁数が違うと `.1Z` < `Z` になる）ので時刻で比べる
      verifiedAt: backed.length > 0 ? backed.map((r) => r.capturedAt).reduce(laterInstant) : null,
    };
  };

  const capabilities = emptyMatrix(nativeVersion);
  const phaseSet = new Set<ToolFailurePhase>();
  const fixtureLimitations: string[] = [];
  const HIGH_LEVEL_KEYS = [
    "sessionStartInjection",
    "promptAwareInjection",
    "promptDeliveryBeforeModel",
    "compactSingleDelivery",
    "trueSessionEnd",
    "subagentCapture",
    "stableNativeSessionId",
  ] as const;
  // schema に cell を足して HIGH_LEVEL_KEYS に足し忘れると、fixture 検証は通るのに
  // matrix では unknown のまま黙って落ちる。top-level / observedEvents と同じ drift 検査を掛ける
  const schemaHighLevelKeys = Object.keys(SCHEMA.properties?.highLevel?.properties ?? {});
  const foldedKeys = new Set<string>([...HIGH_LEVEL_KEYS, "compactionRecoveryStrategy"]);
  const unfolded = schemaHighLevelKeys.filter((k) => !foldedKeys.has(k));
  if (unfolded.length > 0) {
    throw new Error(`highLevel key not folded into the matrix (HIGH_LEVEL_KEYS 未登録): ${unfolded.join(", ")}`);
  }

  type HighLevelObservation = { fixture: CaptureFixture; value: ObservedCapability };
  const highLevelObs: Partial<Record<(typeof HIGH_LEVEL_KEYS)[number], HighLevelObservation[]>> = {};
  const compactionObs: { fixtureId: string; value: CompactionRecoveryStrategy }[] = [];

  for (const f of fixtures) {
    for (const ev of f.observedEvents) {
      if (ev.kind === "raw") continue;
      const kind = ev.kind as EventKind;
      const prev = capabilities.capture[kind];
      // native は synthesized に降格させない（別 fixture で native 観測済みなら維持）。
      // 値が上がらないなら、証跡が良くなるときだけ書き換える。hash 付きの実測を hash 無しの
      // 自己申告で潰さないだけでなく、完全に同格な観測（同じ値・どちらも hash 付き）でも
      // 後勝ちにしない（勝者は入り口で正規化した fixtureId 順で決まる）
      const evValue = ev.capability ?? "native";
      const upgrades = (CAPABILITY_STRENGTH[evValue] ?? 0) > (CAPABILITY_STRENGTH[prev.value] ?? -1);
      const promotion = promoteCell(
        f,
        DERIVABLE_CAPTURE_KINDS.has(kind),
        (r) => ({ value: r.capture[kind], sources: r.captureSources[kind] ?? [] }),
        evValue,
        kind,
        ev.sourceEvents ?? [],
      );
      // 廃止した欄を読むと常に false になり、証跡の優劣が黙って消える。再計算の結果で比べる
      const improvesEvidence =
        promotion.evidenceKind === "real-cli-e2e" && prev.evidenceKind !== "real-cli-e2e";
      const keepPrev =
        (prev.value === "native" && evValue === "synthesized") || (!upgrades && !improvesEvidence);
      // limitations / sourceEvents は上書きせず統合する（後勝ちで caveat を消さない）
      // 散文ではなくコードを載せる。自由文を matrix へ転記すると raw の実値が漏れる
      const mergedLimits = dedupe([
        ...(prev.value === "unknown" ? [] : prev.limitations),
        ...(ev.limitationCodes ?? []),
      ]);
      // real-cli-e2e の cell には、**自分の昇格が real-cli-e2e だった側**の hook 名だけを載せる。
      // 証拠を持たない fixture は上の実在検査（refs.length === 0 で skip）を通らないので、
      // ここで足すとどの記録にも無い hook 名を実測済み cell が主張する。保持側・選択側の
      // 両方に効かせる（片方だけだと同じ欠陥が逆向きに残る）
      const finalKind = keepPrev ? prev.evidenceKind : promotion.evidenceKind;
      const backedOnly = (backed: boolean, names: readonly string[]): readonly string[] =>
        finalKind === "real-cli-e2e" && !backed ? [] : names;
      const mergedSources = dedupe([
        ...backedOnly(prev.evidenceKind === "real-cli-e2e", prev.value === "unknown" ? [] : prev.sourceEvents),
        ...backedOnly(promotion.evidenceKind === "real-cli-e2e", ev.sourceEvents ?? []),
      ]);
      if (keepPrev) {
        capabilities.capture[kind] = { ...prev, limitations: mergedLimits, sourceEvents: mergedSources };
        continue;
      }
      const coverage = ev.coverage ?? (prev.value !== "unknown" ? prev.coverage : undefined);
      // 証拠強度は再計算の結果で決まる（promoteCell）。fixture の申告や rig.isolated の
      // 自己申告では上がらない。sourceFixtureId / verifiedAt は「値か証跡を最後に進めた
      // fixture」で、高位 cell のような強度順の選抜まではしていない
      // （capture cell は tier 判定に入らないため）
      const verified = promotion.evidenceKind === "real-cli-e2e";
      capabilities.capture[kind] = {
        value: evValue,
        sourceEvents: mergedSources,
        nativeVersion,
        evidenceKind: promotion.evidenceKind,
        verifiedAt: promotion.verifiedAt ?? f.capturedAt,
        // 裏付けの無い状態で一度書いた caveat は、裏付いた時点で消す
        // （残すと「実測に紐付いた」と「自己申告」を同じ cell が同時に主張する）
        limitations: verified
          ? mergedLimits.filter((l) => l !== UNVERIFIED)
          : dedupe([...mergedLimits, UNVERIFIED]),
        sourceFixtureId: f.fixtureId,
        ...(promotion.evidenceRefs.length > 0 ? { evidenceRefs: promotion.evidenceRefs } : {}),
        ...(coverage !== undefined ? { coverage } : {}),
      };
    }
    for (const p of f.toolFailurePhasesObserved) {
      phaseSet.add(p);
    }
    // 散文ではなくコードを載せる（§5.3）。fixtureId は schema の pattern で閉じてある
    for (const c of f.limitationCodes ?? []) fixtureLimitations.push(`[${f.fixtureId}] ${c}`);

    // 高位 cell は観測を集めるだけにして、畳むのは全 fixture を読んだ後（下の 1 パス）。
    // cell ごとに後勝ちで畳むと fixture の並び順で結果が変わる:
    // 補強証拠を足すと prompt 経路の対が壊れ、否定的な実測は先に来たか後に来たかで消える
    const hl = f.highLevel;
    if (hl) {
      for (const key of HIGH_LEVEL_KEYS) {
        const v = hl[key];
        if (v) (highLevelObs[key] ??= []).push({ fixture: f, value: v });
      }
      if (hl.compactionRecoveryStrategy) {
        compactionObs.push({ fixtureId: f.fixtureId, value: hl.compactionRecoveryStrategy });
      }
    }
  }

  // compactionRecoveryStrategy は capability cell ではなく単独の enum なので強度順が無い。
  // 割れたまま片方を採ると並び順で結果が変わるため、割れたら「不明」に落として理由を残す
  const compactionValues = dedupe(compactionObs.map((o) => o.value));
  if (compactionValues.length === 1) {
    capabilities.compactionRecoveryStrategy = compactionObs[0].value;
  } else if (compactionValues.length > 1) {
    fixtureLimitations.push(
      `compactionRecoveryStrategy: conflicting observations (${compactionObs
        .map((o) => `${o.fixtureId}=${o.value}`)
        .join(", ")}) — left null`,
    );
  }

  for (const key of HIGH_LEVEL_KEYS) {
    const obs = highLevelObs[key];
    if (!obs) continue;
    // 実測どうしが割れたら、どちらが正しいか harness には決められない。矛盾したまま
    // 自動配送を有効化しないよう証跡を落とす（isProven が false = fail closed）。
    // native と synthesized の食い違いも矛盾に含める: 同一 exact version について
    // 「CLI 自前で起きた」と「こちらで合成した」は両立せず、しかもこの割れは §8 の
    // native_prompt_gate の成立条件そのもの（値だけ native を採って証跡を残すと、
    // synthesized と実測した run があるのに最上位 tier が通る）
    const contradicted = obs.some((o) => o.value !== obs[0].value);
    const derivable = DERIVABLE_HIGH_LEVEL_KEYS.has(key);
    const promotionOf = (o: HighLevelObservation): Promotion =>
      promoteCell(
        o.fixture,
        // 申告値まで見る。この 2 cell の述語は `native` しか出さない（deriveClaims 参照）ので、
        // key だけで「導ける」と決めると、synthesized や unsupported という**正当な**観測が、
        // 証拠を足した途端に「導けるのに支持が無い」と読まれて組み立てを落とす
        derivable && o.value === "native",
        (r) => ({ value: r.highLevel[key as DerivableHighLevelKey], sources: [] }),
        o.value,
        key,
      );
    // 値は最も強い観測を採る。同強度なら裏付けのある方を優先する
    // （並び順で自己申告が実測の証跡を捨てるのを防ぐ）
    const rank = (o: HighLevelObservation): number =>
      CAPABILITY_STRENGTH[o.value] * 2 + (promotionOf(o).evidenceKind === "real-cli-e2e" ? 1 : 0);
    const best = obs.reduce((a, b) => (rank(b) > rank(a) ? b : a));
    // 導けない主張（本文に依存するもの）は証拠があっても real-cli-e2e を名乗らせない。
    // 「穴があると説明文に書く」では塞がらないため（FR-006c）
    const promotion = promotionOf(best);
    const verified = !contradicted && promotion.evidenceKind === "real-cli-e2e";
    capabilities[key] = {
      value: best.value,
      sourceEvents: [],
      nativeVersion,
      evidenceKind: contradicted ? null : promotion.evidenceKind,
      verifiedAt: contradicted ? null : (promotion.verifiedAt ?? best.fixture.capturedAt),
      limitations: dedupe([
        ...obs.map((o) => `observed ${o.value} in ${o.fixture.fixtureId}`),
        ...(contradicted ? ["conflicting observations: evidence dropped"] : []),
        ...(!contradicted && !verified ? [UNVERIFIED] : []),
      ]),
      sourceFixtureId: best.fixture.fixtureId,
      ...(!contradicted && promotion.evidenceRefs.length > 0
        ? { evidenceRefs: promotion.evidenceRefs }
        : {}),
    };
  }

  // addendum §8 の synthesized tier は「1 つの実測が prompt 経路の両 cell を同時に証明した」
  // ことを要求する。cell ごとに後勝ちで畳むと、別 fixture の等強度の観測が片側だけ
  // provenance を差し替え、実在する対が消える（証拠を足したのに tier が下がる）。
  // 畳んだ後に、対を証明した fixture があればその provenance へ揃え直す。
  // 「1 つの実測が対を同時に証明した」の拘束は、fixture が同じことでは満たされない
  // （1 つの fixture が複数の run を束ねられるため）。**両 cell を裏付けた記録に
  // 同じものが 1 件でもある**ことを要求する
  const pairKeys = ["promptAwareInjection", "promptDeliveryBeforeModel"] as const;
  const pairFixture = fixtures.findLast((f) => {
    if (f.highLevel?.promptAwareInjection !== "synthesized") return false;
    if (f.highLevel?.promptDeliveryBeforeModel !== "synthesized") return false;
    const refsFor = (key: (typeof pairKeys)[number]): number[] =>
      promoteCell(
        f,
        DERIVABLE_HIGH_LEVEL_KEYS.has(key),
        (r) => ({ value: r.highLevel[key as DerivableHighLevelKey], sources: [] }),
        "synthesized",
        key,
      ).evidenceRefs;
    const a = refsFor("promptAwareInjection");
    const b = refsFor("promptDeliveryBeforeModel");
    return shareRef(a, b);
  });
  if (pairFixture) {
    for (const key of pairKeys) {
      const cell = capabilities[key];
      // native に昇格した cell や、矛盾で証跡を落とした cell には触らない。
      // ponytail: 片側だけ別 fixture で native になると、対を証明した fixture が残っていても
      // §8 の synthesized 対は成立せず tier が下がる（下がる向きなので fail closed）。
      // 直すには matrix に「この run が対を証明した」機械可読な欄を足すか、§8 の
      // 「両方 synthesized」を「native は上位互換」と読み替えるかで、どちらも凍結済み
      // contract の変更。現行 fixture では両 cell とも unknown で到達しない → issue #19
      if (cell.value !== "synthesized" || cell.evidenceKind === null) continue;
      const pairPromotion = promoteCell(
        pairFixture,
        DERIVABLE_HIGH_LEVEL_KEYS.has(key),
        (r) => ({ value: r.highLevel[key as DerivableHighLevelKey], sources: [] }),
        "synthesized",
        key,
      );
      // 対を証明した記録が manifest を伴わなければ、ここでも real-cli-e2e にはしない
      if (pairPromotion.evidenceKind !== "real-cli-e2e") continue;
      capabilities[key] = {
        ...cell,
        evidenceKind: "real-cli-e2e",
        verifiedAt: pairPromotion.verifiedAt ?? pairFixture.capturedAt,
        sourceFixtureId: pairFixture.fixtureId,
        evidenceRefs: pairPromotion.evidenceRefs,
        // 裏付いた証跡へ差し替えたので「裏付けが無い」caveat は残さない（残すと記録が自己矛盾する）
        limitations: dedupe([
          ...cell.limitations.filter((l) => l !== UNVERIFIED),
          `prompt pair proven together in ${pairFixture.fixtureId}`,
        ]),
      };
    }
  }

  capabilities.toolFailurePhases = TOOL_FAILURE_PHASES.filter((p) => phaseSet.has(p));
  capabilities.toolFailurePhasesUntested = TOOL_FAILURE_PHASES.filter((p) => !phaseSet.has(p));

  // capability hash の入力。addendum §8 は「exact version + §13 の manifest hash +
  // 記録された scenario disposition / evidence hash」の SHA-256 と定めている。
  // fixture の同一性だけでは、disposition が変わっても入力列が変わらない
  // （evidenceHash を持たない現行 fixture では disposition に完全に無反応になる）。
  // fixture 順に依らないよう sort する（同じ証拠集合なら同じ入力列になる）。
  // ponytail: §13 の manifest hash はまだ無い（Task 5 で入る）。入る場所はこの配列
  capabilities.resumeDeliveryStrategy = resolveResumeDeliveryStrategy(capabilities);
  // 欄を数え上げると取りこぼす（2 ラウンド続けて欄を落とした: disposition 全部 → coverage）。
  // capabilityHashInputs 自身だけ外して、畳んだ結果を丸ごと 1 行にする。
  // resumeDeliveryStrategy を先に代入するのは、tier そのものも入力に含めるため
  // （cell の値が同じでも「1 つの run が対を証明したか」で tier は変わる = §8 の区別）
  const { capabilityHashInputs: _unused, ...folded } = capabilities;
  // ad-hoc な文字列連結をやめて JCS で canonical 化する。欄が増えると連結の区切りが
  // 曖昧になり、別の入力が同じ文字列になる（`a@b` と `a` + `@b` が区別できない）
  capabilities.capabilityHashInputs = [
    canonicalizeJson({ cli, nativeVersion }),
    // 欄を数え上げると取りこぼす（scenarioId と cliVersion が実際に落ちていて、
    // 公開 provenance を書き換えても capability hash が動かなかった）。丸ごと canonical 化する
    canonicalizeJson(evidenceSources),
    canonicalizeJson(folded),
  ];

  // 設計側の閉じ方は「自由文を成果物へ出さない」こと（散文の limitations と scenario を
  // matrix へ載せない）。これはその設計が破れたときの**警報**で、信頼境界ではない。
  // 部分文字列の照合は下限より短い秘密を原理的に取りこぼすため、MUST の担保にはできない
  assertNoSecretSubstrings(
    { fixtureIds: fixtures.map((f) => f.fixtureId), fixtureLimitations, evidenceSources, capabilities },
    [...verifiedByFixture.values()].flatMap((refs) => refs.flatMap((r) => r.secrets)),
  );

  return {
    cli,
    nativeVersion,
    generatedAt: new Date().toISOString(),
    fixtureCount: fixtures.length,
    fixtureIds: fixtures.map((f) => f.fixtureId),
    fixtureLimitations,
    evidenceSources,
    capabilities,
  };
}

async function loadFixtures(fixturesDir: string): Promise<CaptureFixture[]> {
  let names: string[];
  try {
    names = await readdir(fixturesDir);
  } catch (e) {
    fail(`cannot read fixturesDir: ${fixturesDir}: ${String(e)}`);
  }

  const jsonFiles = names.filter((n) => n.endsWith(".json")).sort();
  if (jsonFiles.length === 0) {
    fail(`no *.json fixtures in ${fixturesDir}`);
  }

  const fixtures: CaptureFixture[] = [];
  for (const name of jsonFiles) {
    const path = join(fixturesDir, name);
    let raw: string;
    try {
      raw = decodeUtf8(await readFile(path), path);
    } catch (e) {
      fail(`${name}: read failed: ${String(e)}`);
    }
    let data: unknown;
    try {
      data = parseIJson(raw);
    } catch (e) {
      fail(`${name}: invalid JSON: ${String(e)}`);
    }
    try {
      fixtures.push(validateFixture(data, name));
    } catch (e) {
      fail(e instanceof Error ? e.message : String(e));
    }
  }
  return fixtures;
}

async function runAssemble(fixturesDir: string, outFile: string): Promise<void> {
  const fixtures = await loadFixtures(fixturesDir);
  const assembled = assembleFromFixtures(fixtures);
  await writeFile(outFile, JSON.stringify(assembled, null, 2) + "\n", "utf8");
  console.log(`wrote ${outFile} (${assembled.fixtureCount} fixtures, ${assembled.cli} ${assembled.nativeVersion})`);
}

function assert(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Error(`assert failed: ${msg}`);
}

/**
 * 自己 test。**作り物の hash では昇格しない**ことと、本物の観測記録 + manifest では
 * 昇格することの両方を見る。証拠は mkdtemp した置き場へ実際に書き、
 * `evidenceRoot`（test 専用の差し替え口）を渡して end-to-end で組み立てる。
 */
async function selfTest(): Promise<void> {
  const dir = await mkdtemp(join(tmpdir(), "harness-self-test-"));
  try {
    const v = "1.2.3-test";
    const at1 = "2026-08-12T00:00:00.000Z";
    const at2 = "2026-08-12T01:00:00.000Z";
    const root = join(dir, "raw");
    await mkdir(root);
    const ctx = { evidenceRoot: root };

    type Line = { event: string; at: string; payload: Record<string, unknown> };
    const lifecycle = (session: string, prompt: string): Line[] => [
      { event: "SessionStart", at: at1, payload: { hook_event_name: "SessionStart", session_id: session, source: "startup", cwd: "/w" } },
      { event: "UserPromptSubmit", at: at1, payload: { hook_event_name: "UserPromptSubmit", session_id: session, prompt_id: prompt, prompt: "hello", cwd: "/w" } },
      { event: "Stop", at: at1, payload: { hook_event_name: "Stop", session_id: session, prompt_id: prompt, last_assistant_message: "hi", cwd: "/w" } },
      { event: "SessionEnd", at: at1, payload: { hook_event_name: "SessionEnd", session_id: session, prompt_id: prompt, reason: "other", cwd: "/w" } },
    ];

    /** 観測記録（と任意で manifest）を置き場へ書き、fixture が名指しする ref を返す */
    const putEvidence = async (
      label: string,
      lines: Line[],
      opts: { manifest?: boolean; scenarioId?: string } = {},
    ) => {
      const bytes = Buffer.from(lines.map((l) => JSON.stringify(l)).join("\n") + "\n", "utf8");
      await writeFile(join(root, `${label}.jsonl`), bytes);
      const ref: Record<string, unknown> = {
        path: `${label}.jsonl`,
        evidenceHash: digestCapture(bytes),
        captureRawHash: digestRaw(bytes),
        normalizationVersion: NORMALIZATION_VERSION,
      };
      if (!opts.manifest) return ref;
      const manifest = {
        manifestVersion: 1,
        cli: "claude",
        cliVersion: v,
        scenarioId: opts.scenarioId ?? "self.test",
        capturedAt: at1,
        isolated: true,
        internalRunMarker: true,
        exitStatus: 0,
        recorderErrors: 0,
        capture: `${label}.jsonl`,
        captureRawHash: digestRaw(bytes),
        captureHash: digestCapture(bytes),
        normalizationVersion: NORMALIZATION_VERSION,
      };
      const mBytes = Buffer.from(JSON.stringify(manifest), "utf8");
      await writeFile(join(root, `${label}.manifest.json`), mBytes);
      ref.manifest = `${label}.manifest.json`;
      ref.manifestHash = digestRaw(mBytes);
      return ref;
    };

    const base = {
      cli: "claude" as const,
      nativeVersion: v,
      observedEvents: [],
      toolFailurePhasesObserved: [],
      limitations: [],
      limitationCodes: [],
      rig: { isolated: true, internalRunMarker: true },
    };

    // --- 証拠を持たない fixture は値だけ載り、証拠強度は上がらない ---
    const f1 = {
      ...base,
      fixtureId: "claude/lifecycle-basic",
      capturedAt: at1,
      scenario: "session start + prompt",
      scenarioId: "self.lifecycle-basic",
      observedEvents: [
        { kind: "session_started" as const, at: at1 },
        { kind: "user_prompted" as const, at: at1 },
        { kind: "raw" as const, raw: { note: "ignored" } },
      ],
    };
    const f2 = {
      ...base,
      fixtureId: "claude/tool-fail",
      capturedAt: at2,
      scenario: "tool lifecycle + executed fail",
      scenarioId: "self.tool-fail",
      observedEvents: [
        { kind: "tool_started" as const, at: at2 },
        { kind: "tool_completed" as const, at: at2 },
        { kind: "tool_failed" as const, at: at2 },
      ],
      toolFailurePhasesObserved: ["executed" as const, "permission_denied" as const],
      limitations: ["schema_invalid not observed"],
      limitationCodes: ["failure-phases-not-reached"],
    };

    await writeFile(join(dir, "lifecycle-basic.json"), JSON.stringify(f1), "utf8");
    await writeFile(join(dir, "tool-fail.json"), JSON.stringify(f2), "utf8");

    const fixtures = await loadFixtures(dir);
    const assembled = assembleFromFixtures(fixtures, ctx);

    assert(assembled.cli === "claude", "cli");
    assert(assembled.nativeVersion === v, "nativeVersion");
    assert(assembled.fixtureCount === 2, "fixtureCount");
    assert(assembled.fixtureIds.includes("claude/lifecycle-basic"), "id1");
    assert(assembled.fixtureIds.includes("claude/tool-fail"), "id2");
    assert(assembled.evidenceSources.length === 0, "証拠を名指ししていないので表は空");

    const cap = assembled.capabilities;
    for (const kind of ["session_started", "user_prompted"] as EventKind[]) {
      assert(cap.capture[kind].value === "native", `${kind} native`);
      assert(cap.capture[kind].verifiedAt === at1, `${kind} verifiedAt`);
      // 証拠を名指ししていない capture は値だけ載せ、real-cli-e2e とは刻まない
      assert(cap.capture[kind].evidenceKind === "source-test", `${kind} evidenceKind`);
      assert(
        cap.capture[kind].limitations.some((l) => l.startsWith("unverified:")),
        `${kind} caveat`,
      );
      assert(cap.capture[kind].nativeVersion === v, `${kind} nativeVersion`);
    }
    for (const kind of ["tool_started", "tool_completed", "tool_failed"] as EventKind[]) {
      assert(cap.capture[kind].value === "native", `${kind} native`);
      assert(cap.capture[kind].verifiedAt === at2, `${kind} verifiedAt`);
    }
    for (const kind of EVENT_KINDS) {
      if (
        kind === "session_started" ||
        kind === "user_prompted" ||
        kind === "tool_started" ||
        kind === "tool_completed" ||
        kind === "tool_failed"
      ) {
        continue;
      }
      assert(cap.capture[kind].value === "unknown", `${kind} stays unknown`);
    }

    assert(
      JSON.stringify(cap.toolFailurePhases) === JSON.stringify(["executed", "permission_denied"]),
      "toolFailurePhases union ordered",
    );
    assert(cap.sessionStartInjection.value === "unknown", "sessionStartInjection unknown");
    assert(cap.promptAwareInjection.value === "unknown", "promptAwareInjection unknown");
    assert(cap.promptDeliveryBeforeModel.value === "unknown", "promptDeliveryBeforeModel unknown");
    assert(cap.compactSingleDelivery.value === "unknown", "compactSingleDelivery unknown");
    assert(cap.resumeDeliveryStrategy === "manual_only", "no proof ⇒ manual_only");
    assert(cap.capabilityHashInputs.length === 3, "capabilityHashInputs は 3 つの canonical な塊");
    assert(cap.capabilityHashInputs[0] === `{"cli":"claude","nativeVersion":"${v}"}`, "cli と version が先頭");
    assert(cap.trueSessionEnd.value === "unknown", "trueSessionEnd unknown");
    assert(cap.subagentCapture.value === "unknown", "subagentCapture unknown");
    assert(cap.stableNativeSessionId.value === "unknown", "stableNativeSessionId unknown");

    // self-test は entry point が動いていることの門なので、決定的な 2 件だけ置く。
    // 個々の棄却規則は harness/evidence/promotion.test.ts が網羅していて、変異ゲート
    // （harness/evidence/mutate.sh）の 54 件もそちらで殺している

    // --- 作り物の hash では昇格しない（本 issue が塞ぐ経路そのもの） ---
    const forged = {
      ...base,
      fixtureId: "claude/forged",
      capturedAt: at1,
      scenario: "forged evidence",
      scenarioId: "self.forged",
      observedEvents: [{ kind: "session_started" as const, at: at1 }],
      evidence: [
        {
          path: "no-such-capture.jsonl",
          evidenceHash: "a".repeat(64),
          captureRawHash: "b".repeat(64),
          normalizationVersion: NORMALIZATION_VERSION,
        },
      ],
    };
    let rejected = false;
    try {
      assembleFromFixtures([forged as unknown as CaptureFixture], ctx);
    } catch {
      rejected = true;
    }
    assert(rejected, "実在しない観測記録を指す 64 桁 hex は棄却される");

    // --- manifest 付きなら昇格する（positive control） ---
    const backedRef = await putEvidence("backed", lifecycle("S2", "P2"), { manifest: true, scenarioId: "self.backed" });
    const backed = {
      ...base,
      fixtureId: "claude/backed",
      capturedAt: at1,
      scenario: "manifest backed",
      scenarioId: "self.backed",
      observedEvents: [
        { kind: "session_started" as const, at: at1 },
        { kind: "assistant_completed" as const, at: at1, capability: "synthesized" as const, sourceEvents: ["Stop"] },
      ],
      evidence: [backedRef],
    } as unknown as CaptureFixture;
    const backedOut = assembleFromFixtures([backed], ctx);
    assert(
      backedOut.capabilities.capture.session_started.evidenceKind === "real-cli-e2e",
      "manifest 付きの証拠から再計算が通れば real-cli-e2e",
    );
    assert(
      backedOut.capabilities.capture.assistant_completed.evidenceKind === "real-cli-e2e",
      "Stop の欄から導いた synthesized も昇格する",
    );
    assert(
      JSON.stringify(backedOut.capabilities.capture.session_started.evidenceRefs) === "[0]",
      "cell は裏付けた記録を添字で指す",
    );
    assert(
      !backedOut.capabilities.capture.session_started.limitations.some((l) => l.startsWith("unverified:")),
      "裏付いた cell に自己申告の caveat は残さない",
    );

    // 実測どうしが矛盾したら、値は強いほうを残しても証跡は落として自動配送を止める。
    // 並び順で結果が変わらないこと（否定的な観測が先でも後でも同じ）まで確認する
    const hlBase = { ...base };
    const ssNative = {
      ...hlBase,
      fixtureId: "claude/ss-native",
      capturedAt: at1,
      scenario: "session start native",
      scenarioId: "self.ss-native",
      highLevel: { sessionStartInjection: "native" as const },
    };
    const ssUnsupported = {
      ...hlBase,
      fixtureId: "claude/ss-unsupported",
      capturedAt: at2,
      scenario: "session start unsupported",
      scenarioId: "self.ss-unsupported",
      highLevel: { sessionStartInjection: "unsupported" as const },
    };
    for (const [label, order] of [
      ["positive first", [ssNative, ssUnsupported]],
      ["negative first", [ssUnsupported, ssNative]],
    ] as const) {
      const conflicted = assembleFromFixtures([...order], ctx).capabilities;
      assert(conflicted.sessionStartInjection.value === "native", `${label}: keeps the stronger value`);
      assert(conflicted.sessionStartInjection.evidenceKind === null, `${label}: drops the proof`);
      assert(conflicted.resumeDeliveryStrategy === "manual_only", `${label}: must not enable delivery`);
    }

    // 3 つ目の肯定的な観測で証跡が復活しない（矛盾は打ち消せない）
    const reasserted = assembleFromFixtures([
      ssUnsupported,
      ssNative,
      { ...ssNative, fixtureId: "claude/ss-native-2", capturedAt: at2 },
    ], ctx).capabilities;
    assert(reasserted.sessionStartInjection.evidenceKind === null, "contradiction is sticky");

    // native と synthesized の割れも矛盾として扱う（§8 native_prompt_gate の成立条件そのもの）
    const pdNative = {
      ...hlBase,
      fixtureId: "claude/pd-native",
      capturedAt: at1,
      scenario: "pre-model delivery native",
      scenarioId: "self.pd-native",
      highLevel: { promptDeliveryBeforeModel: "native" as const },
    };
    const pdSynth = {
      ...hlBase,
      fixtureId: "claude/pd-synth",
      capturedAt: at2,
      scenario: "pre-model delivery synthesized",
      scenarioId: "self.pd-synth",
      highLevel: { promptDeliveryBeforeModel: "synthesized" as const },
    };
    for (const order of [
      [pdNative, pdSynth],
      [pdSynth, pdNative],
    ]) {
      const split = assembleFromFixtures([...order], ctx).capabilities;
      assert(split.promptDeliveryBeforeModel.evidenceKind === null, "native/synthesized split drops the proof");
      assert(split.resumeDeliveryStrategy === "manual_only", "split must not reach native_prompt_gate");
    }

    // 同格な観測（同じ値・どちらも裏付け無し）なら fixtureId 順で決める。
    // 後勝ちだと「どの run がこの cell を証明したか」が file 名で入れ替わる
    const capA = {
      ...hlBase,
      fixtureId: "claude/cap-a",
      capturedAt: at1,
      scenario: "capture a",
      scenarioId: "self.cap-a",
      observedEvents: [{ kind: "session_started" as const, at: at1 }],
    };
    const capB = {
      ...capA,
      fixtureId: "claude/cap-b",
      capturedAt: at2,
      scenarioId: "self.cap-b",
      observedEvents: [{ kind: "session_started" as const, at: at2 }],
    };
    for (const order of [
      [capA, capB],
      [capB, capA],
    ]) {
      const cell = assembleFromFixtures([...order], ctx).capabilities.capture.session_started;
      assert(cell.sourceFixtureId === "claude/cap-a", "同格な観測は fixtureId 順で決める");
    }

    // disposition だけ変えたら capability hash の入力列も変わること
    {
      const hashBase = {
        ...hlBase,
        fixtureId: "claude/hash-inputs",
        capturedAt: at1,
        scenario: "disposition sensitivity",
        scenarioId: "self.hash-inputs",
        highLevel: { sessionStartInjection: "native" as const },
      };
      const flipped = { ...hashBase, highLevel: { sessionStartInjection: "unsupported" as const } };
      const a = assembleFromFixtures([hashBase], ctx).capabilities.capabilityHashInputs;
      const b = assembleFromFixtures([flipped], ctx).capabilities.capabilityHashInputs;
      assert(a[1] === b[1], "証拠の同一性は変えていない");
      assert(a.join() !== b.join(), "disposition が変われば capability hash の入力も変わる");
      // 欄の数え上げで落としやすいもの: coverage（出荷済み matrix に実在する）と、
      // cell の値が同じでも変わりうる tier
      const cov = {
        ...hashBase,
        highLevel: undefined,
        observedEvents: [{ kind: "session_started" as const, at: at1, coverage: 0.5 }],
      };
      const cov9 = { ...cov, observedEvents: [{ kind: "session_started" as const, at: at1, coverage: 0.9 }] };
      assert(
        assembleFromFixtures([cov], ctx).capabilities.capabilityHashInputs.join() !==
          assembleFromFixtures([cov9], ctx).capabilities.capabilityHashInputs.join(),
        "coverage が変われば capability hash の入力も変わる",
      );
      assert(
        a.some((s) => s.includes('"resumeDeliveryStrategy"')),
        "tier そのものも capability hash の入力に含める",
      );
      // 証拠が変われば入力も変わる（欄の境界は JCS が決めるので連結の曖昧さが無い）
      const withEvidence = assembleFromFixtures([backed], ctx).capabilities.capabilityHashInputs;
      assert(withEvidence[1] !== a[1], "証拠の集合が変われば入力も変わる");
      assert(withEvidence[1].includes(backedRef.evidenceHash as string), "manifest hash と digest が入力に入る");
    }

    // compactionRecoveryStrategy が割れたら、片方を採らず null にして理由を残す
    const crBase = {
      ...hlBase,
      fixtureId: "claude/cr-native",
      capturedAt: at1,
      scenario: "compaction recovery native",
      scenarioId: "self.cr-native",
      highLevel: { compactionRecoveryStrategy: "native_pre_and_post" as const },
    };
    const crOther = {
      ...crBase,
      fixtureId: "claude/cr-other",
      capturedAt: at2,
      scenarioId: "self.cr-other",
      highLevel: { compactionRecoveryStrategy: "unsupported" as const },
    };
    for (const order of [
      [crBase, crOther],
      [crOther, crBase],
    ]) {
      const cr = assembleFromFixtures([...order], ctx);
      assert(cr.capabilities.compactionRecoveryStrategy === null, "conflicting compaction strategy left null");
      assert(
        cr.fixtureLimitations.some((l) => l.startsWith("compactionRecoveryStrategy: conflicting")),
        "conflict recorded",
      );
    }
    assert(
      assembleFromFixtures([crBase], ctx).capabilities.compactionRecoveryStrategy === "native_pre_and_post",
      "single observation still lands",
    );

    // production の入口は evidence root を受け取らない（fixture の値や引数で置き場を動かせない）
    assert(runAssemble.length === 2, "runAssemble は fixturesDir と outFile だけを取る");

    console.log("PASS");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

// import されたとき（テストから validateFixture を呼ぶ場合）は CLI を起動しない
const invokedDirectly = process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) {
  const args = process.argv.slice(2);
  if (args[0] === "--self-test") {
    selfTest().catch((e) => {
      console.error(String(e));
      process.exit(1);
    });
  } else if (args.length === 2) {
    runAssemble(args[0], args[1]).catch((e) => {
      console.error(String(e));
      process.exit(1);
    });
  } else {
    fail(
      "usage: node --experimental-strip-types harness/assemble.ts <fixturesDir> <outFile>\n" +
        "       node --experimental-strip-types harness/assemble.ts --self-test",
    );
  }
}
