// manifest と fixture・観測記録の照合表（data-model.md §2.5）を 1 項目ずつ反転する。
// 表の 1 行を落としても他の 10 行が通るので、まとめて 1 件の test にすると穴が見えない。
import assert from "node:assert/strict";
import test from "node:test";
import type { assembleFromFixtures } from "../assemble.ts";
import { assembleWithRoot, atTime, fixtureBase, lifecycle, newRoot, putEvidence, subagentRun } from "./synthetic.ts";

const AT = "2026-08-12T00:00:00.000Z";

/**
 * manifest 付きの証拠 1 件で組み立てる。`manifestOverrides` は manifest 側だけを、
 * `fixtureOverrides` は fixture 側だけを動かす。
 */
function build(
  manifestOverrides: Record<string, unknown> = {},
  fixtureOverrides: Record<string, unknown> = {},
): ReturnType<typeof assembleFromFixtures> {
  const root = newRoot();
  const ref = putEvidence(root, "backed", lifecycle("s1", "p1"), { manifest: true, manifestOverrides });
  const fixture = fixtureBase({
    fixtureId: "claude/backed",
    observedEvents: [{ kind: "session_started", at: AT, capability: "native", sourceEvents: ["SessionStart"] }],
    evidence: [ref],
    ...fixtureOverrides,
  });
  return assembleWithRoot([fixture], root);
}

test("an unmodified rig manifest promotes the cell", () => {
  const m = build();
  assert.equal(m.capabilities.capture.session_started.evidenceKind, "real-cli-e2e");
});

// 表の 11 項目。どれか 1 つでも照合を落とすと、その行の test だけが通ってしまう。
// test 名は literal で並べる（変異表との突き合わせが grep で効く形にするため）
const INVERSIONS: Array<[string, string, Record<string, unknown>]> = [
  ["manifest manifestVersion that disagrees is rejected", "manifestVersion", { manifestVersion: 2 }],
  ["manifest cli that disagrees is rejected", "cli", { cli: "codex" }],
  ["manifest cliVersion that disagrees is rejected", "cliVersion", { cliVersion: "0.0.0-other" }],
  ["manifest scenarioId that disagrees is rejected", "scenarioId", { scenarioId: "other.scenario" }],
  ["manifest capturedAt that disagrees is rejected", "capturedAt", { capturedAt: "2026-01-01T00:00:00.000Z" }],
  ["manifest capture that disagrees is rejected", "capture", { capture: "somewhere-else.jsonl" }],
  ["manifest captureRawHash that disagrees is rejected", "captureRawHash", { captureRawHash: "b".repeat(64) }],
  ["manifest captureHash that disagrees is rejected", "captureHash", { captureHash: "c".repeat(64) }],
  ["manifest normalizationVersion that disagrees is rejected", "normalizationVersion", { normalizationVersion: 2 }],
  ["manifest isolated that disagrees is rejected", "isolated", { isolated: false }],
  ["manifest recorderErrors that disagrees is rejected", "recorderErrors", { recorderErrors: 1 }],
];

for (const [testName, field, manifestOverrides] of INVERSIONS) {
  test(testName, () => {
    // 動的な RegExp を作らずに部分一致で見る（静的解析が RegExp の非リテラル引数を拾う）
    assert.throws(
      () => build(manifestOverrides),
      (e: unknown) => String(e).includes(`manifest ${field}`),
      `manifest ${field} の照合が効いていない`,
    );
  });
}

test("manifest internalRunMarker must be true, not merely equal to the fixture", () => {
  // 「fixture と一致」で見ていると、双方 false の組み合わせが通ってしまう。
  // 隔離 rig の外で取った記録を real-cli-e2e として載せる経路がそこに開く
  assert.throws(
    () => build({ internalRunMarker: false }, { rig: { isolated: true, internalRunMarker: false } }),
    /manifest internalRunMarker/,
  );
  assert.throws(() => build({ internalRunMarker: false }), /manifest internalRunMarker/);
});

test("a manifest exit status no shell could have reported is rejected", () => {
  // 取り込み側は綴りと値の両方を見るが、**手で書いた manifest は取り込みを通らない**。
  // 検証側が同じ範囲を要求していないと、hash を取り直しただけの manifest が照合を全部通って
  // 昇格する。11 項目の照合に exitStatus は無い（突き合わせる相手が無い）ので、範囲がここの唯一の門
  assert.throws(() => build({ exitStatus: 300 }), /schema/);
});

// 1 つの fixture は複数の run を束ねる（claude/interrupt-and-hook-timeout は 5 本参照する）。
// capturedAt を fixture 単位で縛ると、2 本目以降の manifest が構造的に通らなくなる
test("a fixture that references two runs carries a manifest for each", () => {
  const root = newRoot();
  const first = putEvidence(root, "run-1", lifecycle("s1", "p1"), { manifest: true });
  const second = putEvidence(root, "run-2", atTime(lifecycle("s2", "p2"), "2026-08-13T09:30:00.000Z"), {
    manifest: true,
  });
  const m = assembleWithRoot(
    [
      fixtureBase({
        fixtureId: "claude/two-runs",
        observedEvents: [{ kind: "session_started", at: AT, capability: "native", sourceEvents: ["SessionStart"] }],
        evidence: [first, second],
      }),
    ],
    root,
  );
  assert.equal(m.capabilities.capture.session_started.evidenceKind, "real-cli-e2e");
  // 公開する時刻は fixture の申告ではなく、根拠になった記録の側から来る
  assert.equal(m.capabilities.capture.session_started.verifiedAt, "2026-08-13T09:30:00.000Z");
});

// 遅いほうを選ぶ判定は「文字列の大小」ではない。小数秒の桁数が違うと
// `…:00.1Z` < `…:00Z` になり、文字列比較は早いほうを選んでしまう
test("verifiedAt picks the later instant, not the larger string", () => {
  const root = newRoot();
  const late = putEvidence(root, "run-late", atTime(lifecycle("s1", "p1"), "2026-08-13T09:30:00.1Z"), {
    manifest: true,
  });
  const early = putEvidence(root, "run-early", atTime(lifecycle("s2", "p2"), "2026-08-13T09:30:00Z"), {
    manifest: true,
  });
  const m = assembleWithRoot(
    [
      fixtureBase({
        fixtureId: "claude/fractional",
        observedEvents: [{ kind: "session_started", at: AT, capability: "native", sourceEvents: ["SessionStart"] }],
        evidence: [late, early],
      }),
    ],
    root,
  );
  assert.equal(m.capabilities.capture.session_started.verifiedAt, "2026-08-13T09:30:00.1Z");
});

// `Date.parse` はミリ秒より細かい桁を捨てる。捨てられる桁を schema が許すと、別々の
// 瞬間として書かれた 2 本が同じ epoch になり、どちらが後かは ref の並び順で決まる
test("a timestamp finer than a millisecond is not accepted at all", () => {
  const root = newRoot();
  const ref = putEvidence(root, "sub-ms", atTime(lifecycle("s1", "p1"), "2026-08-13T09:30:00.0001Z"), {
    manifest: true,
  });
  assert.throws(
    () =>
      assembleWithRoot(
        [
          fixtureBase({
            fixtureId: "claude/sub-ms",
            observedEvents: [{ kind: "session_started", at: AT, capability: "native", sourceEvents: ["SessionStart"] }],
            evidence: [ref],
          }),
        ],
        root,
      ),
    /schema/,
  );
});

test("manifest capturedAt must come from the capture, not from the fixture", () => {
  const root = newRoot();
  const ref = putEvidence(root, "late", atTime(lifecycle("s1", "p1"), "2026-08-13T09:30:00.000Z"), {
    manifest: true,
    manifestOverrides: { capturedAt: AT },
  });
  assert.throws(
    () =>
      assembleWithRoot(
        [
          fixtureBase({
            fixtureId: "claude/late",
            observedEvents: [{ kind: "session_started", at: AT, capability: "native", sourceEvents: ["SessionStart"] }],
            evidence: [ref],
          }),
        ],
        root,
      ),
    (e: unknown) => String(e).includes("manifest capturedAt"),
  );
});

// 申告した hook 名が「どれかの記録に在る」だけでは足りない。裏付けのある 1 本が
// 値も導き、申告した hook 名も全部持っていること
test("a claimed source event that lives only in an unbacked capture does not promote", () => {
  const root = newRoot();
  const backed = putEvidence(root, "backed", lifecycle("s1", "p1"), { manifest: true });
  const legacy = putEvidence(root, "legacy", subagentRun("s2"));
  const m = assembleWithRoot(
    [
      fixtureBase({
        fixtureId: "claude/split",
        observedEvents: [
          {
            kind: "session_started",
            at: AT,
            capability: "native",
            sourceEvents: ["SessionStart", "SubagentStop"],
          },
        ],
        evidence: [backed, legacy],
      }),
    ],
    root,
  );
  assert.equal(m.capabilities.capture.session_started.evidenceKind, "source-test");
});

// 証拠を持たない fixture は「申告した hook 名が記録に実在するか」の検査を素通りする
// （refs が空なので飛ばされる）。cell の統合でその名前を足すと、real-cli-e2e の cell が
// どの記録にも無い hook 名を主張する
// 統合は「先に見たほう」と「後から来たほう」の 2 経路あり、裏付けの無い側は
// どちらから来ても落とす。片方向だけ試すと、もう片方の filter を外しても緑のままになる
const promotedSourceEvents = (backedId: string, claimId: string): readonly string[] => {
  const root = newRoot();
  const ref = putEvidence(root, "backed", lifecycle("s1", "p1"), { manifest: true });
  const event = (sourceEvents: string[]) => ({
    kind: "session_started",
    at: AT,
    capability: "native",
    sourceEvents,
  });
  const m = assembleWithRoot(
    [
      fixtureBase({ fixtureId: backedId, observedEvents: [event(["SessionStart"])], evidence: [ref] }),
      fixtureBase({ fixtureId: claimId, observedEvents: [event(["SessionStart", "SubagentStop"])] }),
    ],
    root,
  );
  const cell = m.capabilities.capture.session_started;
  assert.equal(cell.evidenceKind, "real-cli-e2e");
  return cell.sourceEvents;
};

test("an unbacked fixture cannot add a source event to a promoted cell", () => {
  // 裏付けのある側が先（統合では保持側になる）
  assert.deepEqual(promotedSourceEvents("claude/a-backed", "claude/b-claim"), ["SessionStart"]);
});

test("an unbacked fixture seen before the backed one cannot add a source event either", () => {
  // 裏付けの無い側が先。ここが漏れると、同じ欠陥が「先に読まれた fixture」の側に残る
  assert.deepEqual(promotedSourceEvents("claude/b-backed", "claude/a-claim"), ["SessionStart"]);
});

// 「記録に在る」で足りると、値を導いた hook とは別の hook を出どころとして申告できる。
// 記録・digest・manifest はすべて本物のまま、公開する provenance だけが偽になる経路
test("a source event that did not derive the value does not back the promotion", () => {
  const root = newRoot();
  const ref = putEvidence(root, "backed", lifecycle("s1", "p1"), { manifest: true });
  const claim = (sourceEvents: string[]) =>
    assembleWithRoot(
      [
        fixtureBase({
          fixtureId: "claude/misattributed",
          observedEvents: [
            { kind: "assistant_completed", at: AT, capability: "synthesized", sourceEvents },
          ],
          evidence: [ref],
        }),
      ],
      root,
    ).capabilities.capture.assistant_completed;
  // Stop から導いた値。SessionStart は同じ記録に在るが、この値は導いていない
  assert.equal(claim(["Stop"]).evidenceKind, "real-cli-e2e");
  assert.equal(claim(["Stop", "SessionStart"]).evidenceKind, "source-test");
});
