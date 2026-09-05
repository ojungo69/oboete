#!/usr/bin/env node
/**
 * 旧形 corpus と、その corpus を**この branch の前の実装**に通した結果（baseline）を生成する。
 *
 * 生成物 `harness/fixtures/continuity/old-shape-parity.json` は入力と旧結果を両方含む committed
 * artifact で、`old-shape-parity.test.ts` がこの branch の実装と突き合わせる。実行時に git を
 * 引かないのは、この PR が main に入った時点で `origin/main` が変更後の実装になり、
 * 「main と比べる」門が自分で自分を無効化するため。基準は sha で固定する。
 *
 * 使い方:
 *   node harness/continuity/old-shape-baseline.mjs                  # 固定 sha で再生成
 *   node harness/continuity/old-shape-baseline.mjs --output <path>  # 別の場所へ出す（CI の再生成検査）
 *
 * 基準を動かすときは下の `PINNED_SOURCE_COMMIT` を書き換える（flag にしない: 基準の変更が
 * commit の差分に出ることそのものが狙いなので、実行時に差し替えられる口は要らない）。
 */
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { CONTINUITY_LIMITS } from "../schema/continuity.ts";

/** この branch と origin/main の merge-base。「変更前の実装」の定義であり、勝手に動かさない */
const PINNED_SOURCE_COMMIT = "d517a8b49988b1109d56c1868edd8e0f5a1c85b5";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = join(HERE, "..", "..");
const FIXTURE_PATH = join(REPO_ROOT, "harness", "fixtures", "continuity", "tool-lifecycle-reduction.json");
const OUTPUT_PATH = join(REPO_ROOT, "harness", "fixtures", "continuity", "old-shape-parity.json");

// CI の再生成検査は committed fixture を書き換えずに突き合わせたいので、出力先を移せるようにする
const outputIndex = process.argv.indexOf("--output");
const outputPath = outputIndex === -1 ? OUTPUT_PATH : process.argv[outputIndex + 1];

// --- 旧形 corpus -----------------------------------------------------------
// 「旧形」= 新しい任意欄（startIngestSeq / startTurnIdSource / terminalFingerprint /
// droppedEvidence / adapterDeliveryId の記録側）を 1 つも持たない状態。復元された状態は
// 必ずこの形をしているので、event の種類ごとに 1 件ずつ通す。

const fixture = JSON.parse(readFileSync(FIXTURE_PATH, "utf8"));
const { intakeContext } = fixture;
const [startTemplate, , terminalTemplate] = fixture.events;

/** 旧形の PendingOperation。凍結 schema の必須欄だけを持ち、新しい任意欄は 1 つも無い */
function oldShapePending(overrides = {}) {
  const { correlation: correlationOverrides, ...rest } = overrides;
  return {
    operationId: "op-restored",
    correlation: {
      operationId: "op-restored",
      startEventId: "event-start-before-restore",
      nativeOperationId: "toolu_1",
      operationMatchKey: "match-key-1",
      sessionId: "session-1",
      taskLineageId: "lineage-1",
      turnId: "turn-1",
      toolName: "Bash",
      canonicalInputHash: "input-hash-1",
      ...correlationOverrides,
    },
    kind: "tool",
    description: "restored pending operation",
    status: "started",
    replayPolicy: "never_auto",
    sourceEventIds: ["event-start-before-restore"],
    startedAt: "2026-08-16T00:00:01Z",
    sensitivity: "normal",
    ...rest,
  };
}

function restoredState(pendings) {
  return { ...fixture.initialState, pendingOperations: pendings };
}

function event(template, overrides = {}) {
  const { operation: operationOverrides, provenance: provenanceOverrides, ...rest } = overrides;
  const merged = { ...template, ...rest };
  if (operationOverrides === undefined) {
    // 明示的に undefined を渡した場合は operation を落とす（非 operation 系の kind）
    if ("operation" in overrides) delete merged.operation;
  } else {
    merged.operation = { ...template.operation, ...operationOverrides };
  }
  if (provenanceOverrides !== undefined) {
    merged.provenance = { ...template.provenance, ...provenanceOverrides };
  }
  return merged;
}

/**
 * 上限（§10 `arrayItems`）まで埋まった旧形の状態。手で 256 件を JSON に書かずに組み立てる。
 * index 0 は確定済み（退避の相手）、index 1 は index 0 と `operationId` が衝突する started。
 * 凍結 schema は `operationId` に一意性を課さないので、復元した状態はこの形を取り得る。
 */
function filledOldShapePendings(limit) {
  return Array.from({ length: limit }, (_, index) => {
    const id = index === 1 ? "op-filled-0" : `op-filled-${index}`;
    return oldShapePending({
      operationId: id,
      status: index === 0 ? "succeeded" : "started",
      sourceEventIds: [`event-filled-${index}`],
      correlation: {
        operationId: id,
        startEventId: `event-filled-${index}`,
        // native ID は本物の呼び出しごとに一意（同じ値を並べると再配送に見える）
        nativeOperationId: `toolu_filled_${index}`,
      },
    });
  });
}

const CASES = [
  {
    name: "tool-lifecycle",
    description: "変更前から committed だった parity fixture の入力そのもの（start / 再配送 / terminal / 孤児 terminal）",
    initialState: fixture.initialState,
    events: fixture.events,
  },
  {
    name: "restored-native-id-terminal",
    description: "復元した旧形 pending（順序材料なし）に rule 1 の terminal が届く",
    initialState: restoredState([oldShapePending()]),
    events: [terminalTemplate],
  },
  {
    name: "restored-match-key-terminal",
    description: "同じ形を rule 2（matchKey 照合）で閉じる。nativeOperationId を名乗らない adapter",
    initialState: restoredState([oldShapePending({ correlation: { nativeOperationId: undefined } })]),
    events: [event(terminalTemplate, { operation: { nativeOperationId: undefined } })],
  },
  {
    name: "restored-terminal-redelivered",
    description: "旧形 pending を閉じた terminal が同じ配送 ID で再送される",
    initialState: restoredState([oldShapePending()]),
    events: [
      terminalTemplate,
      event(terminalTemplate, { eventId: "event-terminal-again", ingestSeq: "9007199254740998" }),
    ],
  },
  {
    name: "restored-unknown-terminal",
    description: "旧形 pending に successful を持たない terminal が届き unknown に倒れる",
    initialState: restoredState([oldShapePending()]),
    events: [event(terminalTemplate, { successful: undefined })],
  },
  {
    name: "restored-non-operation-event",
    description: "旧形 pending を抱えたまま operation 系でない event が届く",
    initialState: restoredState([oldShapePending()]),
    events: [
      event(startTemplate, {
        eventId: "event-assistant",
        adapterDeliveryId: "delivery-assistant",
        canonicalFingerprint: "fingerprint-assistant",
        kind: "assistant_completed",
        ingestSeq: "9007199254740999",
        operation: undefined,
      }),
    ],
  },
  {
    name: "restored-adapter-progress",
    description:
      "adapter 固有の kind が `phase: \"progress\"` の operation envelope を持って届く。" +
      "既知の phase 表にも非 operation 表にも無い kind なので envelope の欄検査だけを通り、" +
      "start でも terminal でもない経路に入る（`operation === undefined` と同じ分岐の片割れ）",
    initialState: restoredState([oldShapePending()]),
    events: [
      event(startTemplate, {
        eventId: "event-adapter-progress",
        adapterDeliveryId: "delivery-adapter-progress",
        canonicalFingerprint: "fingerprint-adapter-progress",
        kind: "adapter_progress",
        ingestSeq: "9007199254740998",
        operation: { phase: "progress" },
      }),
    ],
  },
  {
    name: "restored-orphan-terminal-redelivered",
    description:
      "相手の居ない terminal が同じ配送 ID で 2 度届く。**隔離は配送鍵を消費しない**ので" +
      "台帳は 2 通目を止めない。止められるのは記録側の鍵だけ（#39）",
    initialState: fixture.initialState,
    events: [
      fixture.events[3],
      event(fixture.events[3], { eventId: "event-terminal-orphan-again" }),
    ],
  },
  {
    name: "restored-abandonment",
    description: "旧形 pending を抱えた状態で session が終わる（放棄経路）",
    initialState: restoredState([oldShapePending()]),
    events: [],
    abandonEvent: event(startTemplate, {
      eventId: "event-abandon",
      adapterDeliveryId: "delivery-abandon",
      canonicalFingerprint: "fingerprint-abandon",
      kind: "session_ended",
      ingestSeq: "9007199254741000",
      operation: undefined,
    }),
  },

  // --- 同名 operationId の兄弟（T019 の 5 件が名指しした欠陥の経路） ------------
  // 旧実装は順序材料を `operationId` を鍵にした側索引で持っていたので、同名の兄弟が
  // 並ぶと帰属を判別できなかった。復元された状態は側索引を持たないので、旧形入力で
  // その欠陥がどこまで観測できるかを実測するのがこの群。

  {
    name: "restored-collided-siblings-terminal",
    description:
      "復元した状態に `operationId` が衝突する兄弟が並ぶ（open 1 件・確定済み 1 件）。" +
      "凍結 schema は一意性を課さないので、この形は schema 妥当なまま復元されうる",
    initialState: restoredState([
      oldShapePending({
        operationId: "op-collide",
        correlation: { operationId: "op-collide", nativeOperationId: undefined },
      }),
      oldShapePending({
        operationId: "op-collide",
        status: "succeeded",
        correlation: { operationId: "op-collide", nativeOperationId: undefined },
      }),
    ]),
    events: [event(terminalTemplate, { operation: { nativeOperationId: undefined } })],
  },
  {
    name: "restored-collided-siblings-eviction",
    description:
      "上限まで埋まった旧形の状態に新しい start が届く。退避（#43 の reason `evicted`）と、" +
      "同名の兄弟をまとめて落とさないこと（T019 の 2 行目・3 行目）を同時に通す",
    initialState: restoredState(filledOldShapePendings(CONTINUITY_LIMITS.arrayItems)),
    events: [startTemplate],
  },
  {
    name: "restored-cross-lineage-twin",
    description:
      "別 lineage に同名の双子が居る状態で start と terminal が届く。旧実装の側索引は " +
      "lineage で絞らない `operationId` 鍵だったので、双子が材料の帰属を曖昧にしうる",
    initialState: restoredState([
      oldShapePending({
        operationId: "op-twin",
        correlation: {
          operationId: "op-twin",
          taskLineageId: "lineage-OTHER",
          nativeOperationId: "toolu_twin",
          startEventId: "event-twin",
        },
        sourceEventIds: ["event-twin"],
      }),
    ]),
    events: [startTemplate, terminalTemplate],
  },

  // --- 台帳と隔離の分岐（早期 return / commit / quarantine） -------------------

  {
    name: "restored-start-redelivery-ledger-miss",
    description:
      "復元で台帳だけが空に戻った状態へ start が再配送される。台帳では止まらず、" +
      "状態側の identity 照合で重複と判定される経路",
    initialState: restoredState([
      oldShapePending({
        operationId: "op-restored",
        correlation: { operationId: "op-restored", startEventId: "event-start" },
      }),
    ]),
    events: [event(startTemplate, { eventId: "event-start-after-restore", ingestSeq: "9007199254741001" })],
  },
  {
    name: "restored-start-conflict",
    description:
      "同じ native ID を名乗りながら入力 hash が違う start。再配送ではなく corruption として" +
      "隔離する（隔離は配送鍵を消費しないので訂正版が後から効く）",
    initialState: restoredState([oldShapePending()]),
    events: [
      event(startTemplate, {
        eventId: "event-start-corrupt",
        ingestSeq: "9007199254741002",
        operation: { canonicalInputHash: "input-hash-CORRUPT" },
      }),
    ],
  },
  {
    name: "restored-delivery-conflict",
    description:
      "同じ配送 ID で source hash が違う event。台帳の重複判定より衝突検査が先に立つ（§4.3）",
    initialState: restoredState([oldShapePending()]),
    events: [
      terminalTemplate,
      event(terminalTemplate, {
        eventId: "event-terminal-different-source",
        canonicalFingerprint: "fingerprint-terminal-DIFFERENT",
        ingestSeq: "9007199254741003",
      }),
    ],
  },
  {
    name: "restored-fingerprint-fallback",
    description:
      "配送 ID を持たない adapter。§8.2 の第二 authority（canonical fingerprint）が鍵になり、" +
      "同じ指紋の 2 通目が重複として止まる",
    initialState: restoredState([oldShapePending()]),
    events: [
      event(terminalTemplate, { adapterDeliveryId: undefined }),
      event(terminalTemplate, {
        eventId: "event-terminal-no-delivery-again",
        adapterDeliveryId: undefined,
        ingestSeq: "9007199254741004",
      }),
    ],
  },

  // --- 候補選びが割れる経路 ---------------------------------------------------

  {
    name: "restored-ambiguous-terminal",
    description: "同じ native ID の open な兄弟が 2 件。rule 1 の候補が割れるのでどちらも閉じない",
    initialState: restoredState([
      oldShapePending({
        operationId: "op-ambiguous-a",
        correlation: { operationId: "op-ambiguous-a", startEventId: "event-ambiguous-a" },
        sourceEventIds: ["event-ambiguous-a"],
      }),
      oldShapePending({
        operationId: "op-ambiguous-b",
        correlation: { operationId: "op-ambiguous-b", startEventId: "event-ambiguous-b" },
        sourceEventIds: ["event-ambiguous-b"],
      }),
    ]),
    events: [terminalTemplate],
  },
  {
    name: "restored-unmatched-terminal",
    description:
      "一致しない native ID を名乗る terminal。名乗っている以上 matchKey へは落ちず、" +
      "相手が居ないまま unmatched に倒れる",
    initialState: restoredState([oldShapePending()]),
    events: [event(terminalTemplate, { operation: { nativeOperationId: "toolu_OTHER" } })],
  },
  {
    name: "restored-out-of-order-terminal",
    description:
      "同じ session で start を受けたあと、その start より前の連番の terminal が届く。" +
      "旧実装は側索引の材料で、この実装は要素の材料で同じ検査をする",
    initialState: fixture.initialState,
    events: [
      event(startTemplate, { ingestSeq: "9007199254740994" }),
      event(terminalTemplate, { ingestSeq: "9007199254740990" }),
    ],
  },
  {
    name: "restored-settled-sibling-fingerprint",
    description:
      "確定済みの pending へ別の指紋の terminal が届く。旧形の状態は `terminalFingerprint` を" +
      "持たないので #44 の衝突検査は発動しない（FR-012 の fail-closed でない側の確認）",
    initialState: restoredState([oldShapePending({ status: "succeeded" })]),
    events: [
      event(terminalTemplate, {
        eventId: "event-terminal-second",
        adapterDeliveryId: "delivery-terminal-second",
        canonicalFingerprint: "fingerprint-terminal-second",
        ingestSeq: "9007199254741005",
      }),
    ],
  },
];

// --- 変更前の実装を読み込む -------------------------------------------------
// **基準 commit の `harness/` を丸ごと取り出して、その中から読む**。還元器のソースだけを
// 作業ツリーの隣に置くと、相対 import (`../schema/*.ts`) が **HEAD 側**の schema を解決する。
// 「基準 commit の実装」と名乗りながら中身は旧還元器 + 新 schema の混成になり、正規化・上限・
// kind 語彙・phase 表を後から変えたときに、固定したはずの基準が黙って動く。
const scratch = mkdtempSync(join(tmpdir(), "old-shape-baseline-"));
try {
  const archivePath = join(scratch, "pinned.tar");
  execFileSync("git", ["archive", "--output", archivePath, PINNED_SOURCE_COMMIT, "harness"], { cwd: REPO_ROOT });
  execFileSync("tar", ["-xf", archivePath, "-C", scratch]);
  const oldModulePath = join(scratch, "harness", "continuity", "reference-model.ts");
  const old = await import(pathToFileURL(oldModulePath).href);
  const { projectOldShape } = await import(pathToFileURL(join(HERE, "old-shape-projection.ts")).href);

  // `{ nativeOperationId: undefined }` のような「欄を消す」上書きは、値が undefined の key として
  // 残る。committed fixture は JSON なので、走らせる前に JSON の形へ落として欄ごと消す
  const cases = CASES.map((raw) => JSON.parse(JSON.stringify(raw))).map((testCase) => {
    let state = testCase.initialState;
    let history = [];
    let ledger = new Map();
    // 変更前の snapshot は状態の外に `operationStarts` を持っていた。**状態だけを渡された**
    // 実装ではこれが空になる（それが #35 の欠陥そのもの）ので、復元を再現するなら空で始めるのが
    // 正しい。同じ session 内で start を受けた case では、旧実装がここを埋めながら進む
    let operationStarts = new Map();
    const baseline = [];
    for (const raw of testCase.events) {
      const stamped = old.stampIntakeEvidence(raw, intakeContext).event;
      const result = old.reduceTaskWorkState({ state, history, operationStarts }, stamped, ledger);
      state = result.snapshot.state;
      history = result.snapshot.history;
      operationStarts = result.snapshot.operationStarts;
      ledger = result.ledger;
      baseline.push({
        eventId: stamped.eventId,
        ...projectOldShape({
          outcome: result.outcome,
          contentHash: result.contentHash,
          diagnostics: result.diagnostics,
          state,
          history,
          ledger,
        }),
      });
    }
    if (testCase.abandonEvent !== undefined) {
      const stamped = old.stampIntakeEvidence(testCase.abandonEvent, intakeContext).event;
      const result = old.finalizeAbandonedState(state, stamped, ledger);
      baseline.push({
        eventId: `${stamped.eventId}#abandon`,
        ...projectOldShape({
          outcome: result.outcome,
          diagnostics: result.diagnostics,
          state: result.state,
          history,
          ledger: result.ledger,
        }),
      });
    }
    return { ...testCase, baseline };
  });

  const output = {
    fixtureId: "old-shape-parity",
    description:
      "旧形（新しい任意欄を持たない）状態と event を、この branch の変更前の実装に通した結果。" +
      "SC-003 の「旧形入力に対する振る舞いは 4 件の是正以外変わらない」を機械的に検証するための基準。",
    sourceCommit: PINNED_SOURCE_COMMIT,
    generatedBy: "harness/continuity/old-shape-baseline.mjs",
    intakeContext,
    cases,
  };
  writeFileSync(outputPath, `${JSON.stringify(output, null, 2)}\n`);
  const events = cases.reduce((sum, testCase) => sum + testCase.baseline.length, 0);
  console.log(`${outputPath} を生成した: ${cases.length} case / ${events} step（基準 ${PINNED_SOURCE_COMMIT}）`);
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
