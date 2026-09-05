import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import type { CanonicalWorkStateV1, NormalizedContinuityEvent } from "../schema/continuity.ts";
import {
  finalizeAbandonedState,
  reduceTaskWorkState,
  stampIntakeEvidence,
  type IdempotencyLedger,
  type WorkStateRevisionEntryV1,
} from "./reference-model.ts";
import { diffPaths, projectOldShape, valueAtPath, type OldShapeOutcomeV1 } from "./old-shape-projection.ts";

/**
 * SC-003（旧形の入力に対する振る舞いは #35 / #39 / #43 / #44 の是正以外変わらない）の門。
 *
 * 「変わらない」を行数や issue 番号で語ると検証できない主張になるので、**変更前の実装の出力**を
 * committed baseline として持ち、この実装の出力と突き合わせる。差分は 1 つ残らず下の
 * `ALLOWED_DELTAS` に、case 名・event・JSON path・**値**・issue 番号まで書いてあるものだけを許す。
 * baseline の作り方と基準 sha は `old-shape-baseline.mjs` を参照。
 *
 * 意図した振る舞いの変更で許可表を書き直すときは、手で写さずに実測を出す:
 *   OLD_SHAPE_PARITY_DUMP=1 node --experimental-strip-types --test harness/continuity/old-shape-parity.test.ts
 *
 * **SC-003 の主張はこの corpus の範囲に閉じる**（黙って間引かない）。corpus が届いていない経路と、
 * その理由は次のとおり。ここに挙がっていない経路の証拠は `reference-model.test.ts` と `mutate.sh` が持つ。
 *
 * - 同じ session 内で `operationId` が衝突する 2 件を作る経路: start の再配送判定が derived id と
 *   native id の両方を見るので、同名を作ろうとすると `duplicate_operation_start` か `start_conflict`
 *   のどちらかに倒れる。状態側の衝突（復元）は `restored-collided-siblings-*` が通す。
 * - `schemaVersion` や `taskLineageId` が食い違う入力（`assertSameScope` が投げる経路）: 例外は
 *   還元結果を返さないので、この比較面では旧新の区別が付かない。
 */

interface ParityCase {
  readonly name: string;
  readonly description: string;
  readonly initialState: CanonicalWorkStateV1;
  readonly events: readonly NormalizedContinuityEvent[];
  readonly abandonEvent?: NormalizedContinuityEvent;
  readonly baseline: readonly (OldShapeOutcomeV1 & { readonly eventId: string })[];
}

interface ParityFixture {
  readonly sourceCommit: string;
  readonly intakeContext: Parameters<typeof stampIntakeEvidence>[1];
  readonly cases: readonly ParityCase[];
}

/** corpus が黙って縮まないための実数。case を消したら件数で落ちる */
const EXPECTED_CASES = 20;
const EXPECTED_STEPS = 29;

interface AllowedDelta {
  /** case 名 */
  readonly caseName: string;
  /** その step の eventId（放棄は `<eventId>#abandon`） */
  readonly eventId: string;
  /**
   * 変更前と食い違ってよい JSON path と、**そのときの値**。path だけを許すと
   * 「ここは何が起きてもよい」になり、記録が 2 件に増える退行を素通しする（#39 がまさにその形）。
   * ここに無い path が 1 つでもあれば落ちるし、ここにあるのに差分が出なくても落ちる。
   */
  readonly values: Readonly<Record<string, unknown>>;
  readonly issue: string;
  readonly why: string;
}

const ALLOWED_DELTAS: readonly AllowedDelta[] = [
  {
    caseName: "tool-lifecycle",
    eventId: "event-start",
    issue: "#35",
    why: "start の権威順序と turn 種別を状態に載せた。状態だけを渡された実装でも §4.3 の順序検査ができる。状態が動くので revision と contentHash も動く",
    values: {
      "contentHash": "1ef4ce39ea87a829ff7caa7d14cd66a217fd6ba8ae1ccfc27275f7307dd348c1",
      "history[0].contentHash": "1ef4ce39ea87a829ff7caa7d14cd66a217fd6ba8ae1ccfc27275f7307dd348c1",
      "history[0].revision": "bf7064b02573e0fa0d7743ab7fc9e166bb06c99f269cdb597193943b7a2c0eee",
      "state.pendingOperations[0].startIngestSeq": "9007199254740994",
      "state.pendingOperations[0].startTurnIdSource": "native",
      "state.stateRevision": "bf7064b02573e0fa0d7743ab7fc9e166bb06c99f269cdb597193943b7a2c0eee",
    },
  },
  {
    caseName: "tool-lifecycle",
    eventId: "event-start-redelivered",
    issue: "#35",
    why: "再配送そのものは状態を変えない。直前の start が書いた欄と hash がそのまま見えているだけ",
    values: {
      "contentHash": "1ef4ce39ea87a829ff7caa7d14cd66a217fd6ba8ae1ccfc27275f7307dd348c1",
      "history[0].contentHash": "1ef4ce39ea87a829ff7caa7d14cd66a217fd6ba8ae1ccfc27275f7307dd348c1",
      "history[0].revision": "bf7064b02573e0fa0d7743ab7fc9e166bb06c99f269cdb597193943b7a2c0eee",
      "state.pendingOperations[0].startIngestSeq": "9007199254740994",
      "state.pendingOperations[0].startTurnIdSource": "native",
      "state.stateRevision": "bf7064b02573e0fa0d7743ab7fc9e166bb06c99f269cdb597193943b7a2c0eee",
    },
  },
  {
    caseName: "tool-lifecycle",
    eventId: "event-terminal",
    issue: "#35 / #44",
    why: "閉じた terminal の指紋を残す。あとから届く別指紋の terminal を衝突として弾くため",
    values: {
      "contentHash": "6c959ab5a07b0fb5e911a85976cc8907f431ce5f561a8411090432756cd056de",
      "history[0].contentHash": "1ef4ce39ea87a829ff7caa7d14cd66a217fd6ba8ae1ccfc27275f7307dd348c1",
      "history[0].revision": "bf7064b02573e0fa0d7743ab7fc9e166bb06c99f269cdb597193943b7a2c0eee",
      "history[1].contentHash": "6c959ab5a07b0fb5e911a85976cc8907f431ce5f561a8411090432756cd056de",
      "history[1].revision": "fa42ded6a0bec3bd6a1146a777a421f0cd4f5a56178b0db8d2ad036c172571a5",
      "state.pendingOperations[0].startIngestSeq": "9007199254740994",
      "state.pendingOperations[0].startTurnIdSource": "native",
      "state.pendingOperations[0].terminalFingerprint": "fingerprint-terminal",
      "state.stateRevision": "fa42ded6a0bec3bd6a1146a777a421f0cd4f5a56178b0db8d2ad036c172571a5",
    },
  },
  {
    caseName: "tool-lifecycle",
    eventId: "event-terminal-orphan",
    issue: "#35 / #43 / #44",
    why: "相手の居ない terminal を状態に記録する。隔離は配送鍵を消費しないので、状態の記録が唯一の証跡",
    values: {
      "contentHash": "a001443e9cf26b0921e3a2ac2a81deec1f1060798fd7755736f9327150d4bb65",
      "diagnostics[1]": {"code": "dropped_evidence_recorded", "eventId": "event-terminal-orphan", "detail": "状態から落ちた証跡を記録: orphaned_terminal:event-terminal-orphan"},
      "history[0].contentHash": "1ef4ce39ea87a829ff7caa7d14cd66a217fd6ba8ae1ccfc27275f7307dd348c1",
      "history[0].revision": "bf7064b02573e0fa0d7743ab7fc9e166bb06c99f269cdb597193943b7a2c0eee",
      "history[1].contentHash": "6c959ab5a07b0fb5e911a85976cc8907f431ce5f561a8411090432756cd056de",
      "history[1].revision": "fa42ded6a0bec3bd6a1146a777a421f0cd4f5a56178b0db8d2ad036c172571a5",
      "history[2]": {"revision": "ca8acd10fed885f68c801e17d01077f45e565cd923da4d599ef69dda034ed24b", "contentHash": "a001443e9cf26b0921e3a2ac2a81deec1f1060798fd7755736f9327150d4bb65", "eventId": "event-terminal-orphan"},
      "state.droppedEvidence": [{"reason": "orphaned_terminal", "eventId": "event-terminal-orphan", "terminalFingerprint": "fingerprint-terminal-orphan", "adapterDeliveryId": "delivery-terminal-orphan", "recordedAt": "2026-08-16T00:00:04Z", "sensitivity": "private"}],
      "state.pendingOperations[0].startIngestSeq": "9007199254740994",
      "state.pendingOperations[0].startTurnIdSource": "native",
      "state.pendingOperations[0].terminalFingerprint": "fingerprint-terminal",
      "state.stateRevision": "ca8acd10fed885f68c801e17d01077f45e565cd923da4d599ef69dda034ed24b",
      "state.updatedAt": "2026-08-16T00:00:04Z",
    },
  },
  {
    caseName: "restored-orphan-terminal-redelivered",
    eventId: "event-terminal-orphan",
    issue: "#43",
    why: "同じ記録を、復元直後（pending が 1 件も無い状態）でも残す",
    values: {
      "contentHash": "f2b703e435e3c3fb29b65534c82ca2b3cda4f5144365f5c27a520753659d6384",
      "diagnostics[1]": {"code": "dropped_evidence_recorded", "eventId": "event-terminal-orphan", "detail": "状態から落ちた証跡を記録: orphaned_terminal:event-terminal-orphan"},
      "history[0]": {"revision": "7c42ad7216a30235af599dbe48356e33de438e37acb910e39efbb2130dc81dca", "contentHash": "f2b703e435e3c3fb29b65534c82ca2b3cda4f5144365f5c27a520753659d6384", "eventId": "event-terminal-orphan"},
      "state.droppedEvidence": [{"reason": "orphaned_terminal", "eventId": "event-terminal-orphan", "terminalFingerprint": "fingerprint-terminal-orphan", "adapterDeliveryId": "delivery-terminal-orphan", "recordedAt": "2026-08-16T00:00:04Z", "sensitivity": "private"}],
      "state.sensitivity": "private",
      "state.stateRevision": "7c42ad7216a30235af599dbe48356e33de438e37acb910e39efbb2130dc81dca",
      "state.updatedAt": "2026-08-16T00:00:04Z",
    },
  },
  {
    caseName: "restored-orphan-terminal-redelivered",
    eventId: "event-terminal-orphan-again",
    issue: "#39",
    why: "再起動で台帳が空に戻っても記録は 1 件のまま。2 件に増えないことをこの値で固定する",
    values: {
      "contentHash": "f2b703e435e3c3fb29b65534c82ca2b3cda4f5144365f5c27a520753659d6384",
      "history[0]": {"revision": "7c42ad7216a30235af599dbe48356e33de438e37acb910e39efbb2130dc81dca", "contentHash": "f2b703e435e3c3fb29b65534c82ca2b3cda4f5144365f5c27a520753659d6384", "eventId": "event-terminal-orphan"},
      "state.droppedEvidence": [{"reason": "orphaned_terminal", "eventId": "event-terminal-orphan", "terminalFingerprint": "fingerprint-terminal-orphan", "adapterDeliveryId": "delivery-terminal-orphan", "recordedAt": "2026-08-16T00:00:04Z", "sensitivity": "private"}],
      "state.sensitivity": "private",
      "state.stateRevision": "7c42ad7216a30235af599dbe48356e33de438e37acb910e39efbb2130dc81dca",
      "state.updatedAt": "2026-08-16T00:00:04Z",
    },
  },
  {
    caseName: "restored-collided-siblings-eviction",
    eventId: "event-start",
    issue: "#35 / #43",
    why: "上限退避で落ちた要素を記録する。同名の兄弟が並んでいても落ちるのは 1 件で、生き残った側の材料は消えない（T019 の 2 行目・3 行目）",
    values: {
      "contentHash": "dfa0910a26f78cc544d96688e68eced3f3ae7553e15b4d28ef147b7a741207fa",
      "diagnostics[1]": {"code": "dropped_evidence_recorded", "eventId": "event-start", "detail": "状態から落ちた証跡を記録: evicted:op-filled-0"},
      "history[0].contentHash": "dfa0910a26f78cc544d96688e68eced3f3ae7553e15b4d28ef147b7a741207fa",
      "history[0].revision": "c3699b501c82596a1411d6c3f5e208ef61aad0cce0781ace4c486622e1bcbc94",
      "state.droppedEvidence": [{"reason": "evicted", "operationId": "op-filled-0", "status": "succeeded", "eventId": "event-filled-0", "recordedAt": "2026-08-16T00:00:01Z", "sensitivity": "normal"}],
      "state.pendingOperations[255].startIngestSeq": "9007199254740994",
      "state.pendingOperations[255].startTurnIdSource": "native",
      "state.stateRevision": "c3699b501c82596a1411d6c3f5e208ef61aad0cce0781ace4c486622e1bcbc94",
    },
  },
  {
    caseName: "restored-cross-lineage-twin",
    eventId: "event-start",
    issue: "#35",
    why: "別 lineage に同名の双子が居ても、材料は自分の要素に載るので帰属が曖昧にならない",
    values: {
      "contentHash": "7321655bb2a65e6c60cbc5a34b82480772a2e74516eaaf2c3dfd11c8f1b06f02",
      "history[0].contentHash": "7321655bb2a65e6c60cbc5a34b82480772a2e74516eaaf2c3dfd11c8f1b06f02",
      "history[0].revision": "fca1c6af0cd964447cedd7826301a554be22bbe60963dbfdf9ca5a167d2775ca",
      "state.pendingOperations[1].startIngestSeq": "9007199254740994",
      "state.pendingOperations[1].startTurnIdSource": "native",
      "state.stateRevision": "fca1c6af0cd964447cedd7826301a554be22bbe60963dbfdf9ca5a167d2775ca",
    },
  },
  {
    caseName: "restored-cross-lineage-twin",
    eventId: "event-terminal",
    issue: "#35 / #44",
    why: "同上。閉じた側に指紋も残る",
    values: {
      "contentHash": "882d74c742b6b0eecf0505bc510624736627239e607393e0f2b860c3bcdb9c99",
      "history[0].contentHash": "7321655bb2a65e6c60cbc5a34b82480772a2e74516eaaf2c3dfd11c8f1b06f02",
      "history[0].revision": "fca1c6af0cd964447cedd7826301a554be22bbe60963dbfdf9ca5a167d2775ca",
      "history[1].contentHash": "882d74c742b6b0eecf0505bc510624736627239e607393e0f2b860c3bcdb9c99",
      "history[1].revision": "2ae795ee9c700a7e28ad55143611e3ee25f29c0364883fbec4c6bc7c4df7d78a",
      "state.pendingOperations[1].startIngestSeq": "9007199254740994",
      "state.pendingOperations[1].startTurnIdSource": "native",
      "state.pendingOperations[1].terminalFingerprint": "fingerprint-terminal",
      "state.stateRevision": "2ae795ee9c700a7e28ad55143611e3ee25f29c0364883fbec4c6bc7c4df7d78a",
    },
  },
  {
    caseName: "restored-start-redelivery-ledger-miss",
    eventId: "event-start-after-restore",
    issue: "#35",
    why: "**診断の detail だけ**が変わる。索引を引く文面から状態の要素を指す文面になった。判断・状態・台帳・hash はすべて一致している",
    values: {
      "diagnostics[0].detail": "operationId op-restored は既に記録済み（status: started）",
    },
  },
  {
    caseName: "restored-unmatched-terminal",
    eventId: "event-terminal",
    issue: "#43",
    why: "名乗った native ID の相手が居ない terminal も記録する。記録しないと隔離のたびに証跡が消える",
    values: {
      "contentHash": "fd198fffcb2c04d1ca8ec994091388fc3829b8ed830f9fa91562e24e18d5a7ca",
      "diagnostics[1]": {"code": "dropped_evidence_recorded", "eventId": "event-terminal", "detail": "状態から落ちた証跡を記録: orphaned_terminal:event-terminal"},
      "history[0]": {"revision": "0fce61e636389ffe596012e104753c29fc515cb51f6ea9bd05cbeec5bbd88b65", "contentHash": "fd198fffcb2c04d1ca8ec994091388fc3829b8ed830f9fa91562e24e18d5a7ca", "eventId": "event-terminal"},
      "state.droppedEvidence": [{"reason": "orphaned_terminal", "eventId": "event-terminal", "terminalFingerprint": "fingerprint-terminal", "adapterDeliveryId": "delivery-terminal", "recordedAt": "2026-08-16T00:00:03Z", "sensitivity": "private"}],
      "state.sensitivity": "private",
      "state.stateRevision": "0fce61e636389ffe596012e104753c29fc515cb51f6ea9bd05cbeec5bbd88b65",
      "state.updatedAt": "2026-08-16T00:00:03Z",
    },
  },
  {
    caseName: "restored-out-of-order-terminal",
    eventId: "event-start",
    issue: "#35",
    why: "同じ session の start が材料を要素に書く",
    values: {
      "contentHash": "1ef4ce39ea87a829ff7caa7d14cd66a217fd6ba8ae1ccfc27275f7307dd348c1",
      "history[0].contentHash": "1ef4ce39ea87a829ff7caa7d14cd66a217fd6ba8ae1ccfc27275f7307dd348c1",
      "history[0].revision": "bf7064b02573e0fa0d7743ab7fc9e166bb06c99f269cdb597193943b7a2c0eee",
      "state.pendingOperations[0].startIngestSeq": "9007199254740994",
      "state.pendingOperations[0].startTurnIdSource": "native",
      "state.stateRevision": "bf7064b02573e0fa0d7743ab7fc9e166bb06c99f269cdb597193943b7a2c0eee",
    },
  },
  {
    caseName: "restored-out-of-order-terminal",
    eventId: "event-terminal",
    issue: "#35",
    why: "その材料で順序を検査する。旧実装は側索引の同じ材料で同じ判定をしていた",
    values: {
      "contentHash": "151c2d29254fab853788b94e5c95d008f79ba1362388e9394a6c1a9df68a3c8a",
      "history[0].contentHash": "1ef4ce39ea87a829ff7caa7d14cd66a217fd6ba8ae1ccfc27275f7307dd348c1",
      "history[0].revision": "bf7064b02573e0fa0d7743ab7fc9e166bb06c99f269cdb597193943b7a2c0eee",
      "history[1].contentHash": "151c2d29254fab853788b94e5c95d008f79ba1362388e9394a6c1a9df68a3c8a",
      "history[1].revision": "40f60789ffca4456c082574ac79b8347460029bc0b80d289cbb63206a93df99f",
      "state.pendingOperations[0].startIngestSeq": "9007199254740994",
      "state.pendingOperations[0].startTurnIdSource": "native",
      "state.stateRevision": "40f60789ffca4456c082574ac79b8347460029bc0b80d289cbb63206a93df99f",
    },
  },
];

const fixture = JSON.parse(
  readFileSync(new URL("../fixtures/continuity/old-shape-parity.json", import.meta.url), "utf8"),
) as ParityFixture;

test("旧形の入力に対する振る舞いは、許可した差分以外は変更前と一致する（SC-003）", () => {
  assert.equal(fixture.cases.length, EXPECTED_CASES, "corpus の case 数が変わっている");
  const steps = fixture.cases.reduce((sum, testCase) => sum + testCase.baseline.length, 0);
  assert.equal(steps, EXPECTED_STEPS, "corpus の step 数が変わっている");

  const usedDeltas = new Set<AllowedDelta>();
  const unexplained: string[] = [];
  // 意図した振る舞いの変更のあとに allowlist を手で書き写すと転記を間違える。実測を出す
  const dump: unknown[] = [];

  for (const testCase of fixture.cases) {
    let snapshot = { state: testCase.initialState, history: [] as readonly WorkStateRevisionEntryV1[] };
    let ledger: IdempotencyLedger = new Map();
    const actual: (OldShapeOutcomeV1 & { eventId: string })[] = [];

    for (const raw of testCase.events) {
      const event = stampIntakeEvidence(raw, fixture.intakeContext).event;
      const result = reduceTaskWorkState(snapshot, event, ledger);
      snapshot = result.snapshot;
      ledger = result.ledger;
      actual.push({
        eventId: event.eventId,
        ...projectOldShape({
          outcome: result.outcome,
          contentHash: result.contentHash,
          diagnostics: result.diagnostics,
          state: result.snapshot.state as unknown as Record<string, unknown>,
          history: result.snapshot.history,
          ledger,
        }),
      });
    }

    if (testCase.abandonEvent !== undefined) {
      const event = stampIntakeEvidence(testCase.abandonEvent, fixture.intakeContext).event;
      const result = finalizeAbandonedState(snapshot.state, event, ledger);
      actual.push({
        eventId: `${event.eventId}#abandon`,
        ...projectOldShape({
          outcome: result.outcome,
          diagnostics: result.diagnostics,
          state: result.state as unknown as Record<string, unknown>,
          history: snapshot.history,
          ledger: result.ledger,
        }),
      });
    }

    assert.equal(
      actual.length,
      testCase.baseline.length,
      `${testCase.name}: step 数が baseline と違う（event が黙って落ちている）`,
    );

    for (const [index, expected] of testCase.baseline.entries()) {
      const produced = actual[index] as OldShapeOutcomeV1 & { eventId: string };
      assert.equal(produced.eventId, expected.eventId, `${testCase.name}: step ${index} の event が違う`);
      const { eventId: _e1, ...before } = expected;
      const { eventId: _e2, ...after } = produced;
      const differing = diffPaths(before, after);
      const describe = (paths: readonly string[]): string =>
        paths.map((path) => `${path}=${JSON.stringify(valueAtPath(after, path))}`).join(", ");
      if (differing.length === 0) continue;
      if (process.env.OLD_SHAPE_PARITY_DUMP === "1") {
        dump.push({
          caseName: testCase.name,
          eventId: expected.eventId,
          values: Object.fromEntries(differing.map((path) => [path, valueAtPath(after, path)])),
        });
      }
      const allowed = ALLOWED_DELTAS.find(
        (entry) => entry.caseName === testCase.name && entry.eventId === expected.eventId,
      );
      if (allowed === undefined) {
        unexplained.push(`${testCase.name} / ${expected.eventId}: ${describe(differing)}`);
        continue;
      }
      usedDeltas.add(allowed);
      const extra = differing.filter((path) => !(path in allowed.values));
      if (extra.length > 0) unexplained.push(`${testCase.name} / ${expected.eventId}: ${describe(extra)}`);
      for (const [path, value] of Object.entries(allowed.values)) {
        if (!differing.includes(path)) {
          unexplained.push(`${testCase.name} / ${expected.eventId}: 許可したのに差分が無い path ${path}`);
          continue;
        }
        try {
          assert.deepEqual(valueAtPath(after, path), value);
        } catch {
          unexplained.push(`${testCase.name} / ${expected.eventId}: ${describe([path])} は許可した値と違う`);
        }
      }
    }
  }

  if (dump.length > 0) console.log(JSON.stringify(dump, null, 2));
  assert.deepEqual(unexplained, [], "許可していない振る舞いの差分がある");
  // 締めすぎた門は自分の test からは漏れる。使われなかった許可は「もう起きない差分」なので、
  // 残すと allowlist が実際より広く見える
  const unused = ALLOWED_DELTAS.filter((entry) => !usedDeltas.has(entry));
  assert.deepEqual(unused.map((entry) => `${entry.caseName} / ${entry.eventId}`), []);
});

/**
 * 基準がここで止まっていることの検査。**これは自己申告であって provenance の証明ではない**——
 * fixture はただの JSON なので、手で書き換えて `contract-hashes.json` を作り直せばこの test は
 * 通る。生成物であることは CI の "Old-shape parity baseline is regenerated from the pinned commit"
 * が見る（基準 commit から `--output` で作業ツリー外へ生成し直し、committed fixture と `diff`）。
 * ここが見るのは「基準 sha を黙って動かさないこと」だけ。
 */
test("baseline の基準 sha が分岐点のまま動いていない", () => {
  assert.equal(fixture.sourceCommit, "d517a8b49988b1109d56c1868edd8e0f5a1c85b5");
});
