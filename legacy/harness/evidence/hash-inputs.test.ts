// capabilityHashInputs は matrix の同一性の入力。ad-hoc な文字列連結に戻すと、欄の境界が
// 曖昧になって別の入力が同じ文字列になる（`a` + `@b` と `a@b` が区別できない）。
// 3 つの canonical な塊であることと、どの入力を動かしても列が動くことを固定する。
import assert from "node:assert/strict";
import test from "node:test";
import { canonicalizeJson } from "../schema/jcs.ts";
import { assembleWithRoot, fixtureBase, lifecycle, newRoot, putEvidence } from "./synthetic.ts";

const AT = "2026-08-12T00:00:00.000Z";

function inputsOf(opts: { manifest?: boolean; kind?: string } = {}): string[] {
  const root = newRoot();
  const ref = putEvidence(root, "backed", lifecycle("s1", "p1"), { manifest: opts.manifest ?? true });
  const m = assembleWithRoot(
    [
      fixtureBase({
        fixtureId: "claude/backed",
        observedEvents: [
          {
            kind: opts.kind ?? "session_started",
            at: AT,
            capability: "native",
            sourceEvents: [opts.kind === "user_prompted" ? "UserPromptSubmit" : "SessionStart"],
          },
        ],
        evidence: [ref],
      }),
    ],
    root,
  );
  return m.capabilities.capabilityHashInputs;
}

test("capabilityHashInputs stay three canonical blobs that react to every input", () => {
  const inputs = inputsOf();
  assert.equal(inputs.length, 3);
  // 連結された 1 本の文字列に戻すと、この形が崩れる
  for (const blob of inputs) {
    assert.equal(canonicalizeJson(JSON.parse(blob)), blob, "canonical な JSON でない塊がある");
  }
  assert.equal(inputs[0], '{"cli":"claude","nativeVersion":"1.2.3-test"}');
  // 証拠の素性が入る塊。manifestHash まで含む
  assert.match(inputs[1], /"fixtureId":"claude\/backed"/);
  // 欄を数え上げると落ちる。scenarioId と cliVersion まで入ることを見る
  assert.match(inputs[1], /"scenarioId":/);
  assert.match(inputs[1], /"cliVersion":/);
  // 畳んだ結果には capabilityHashInputs 自身が入らない（入れると自己参照で決まらない）
  assert.ok(!inputs[2].includes("capabilityHashInputs"));

  // 証拠の素性が変われば入力列も変わる（legacy と manifest 付きは別物）
  assert.notEqual(inputsOf({ manifest: false })[1], inputs[1]);
  // cell の値が変われば畳んだ塊も変わる
  assert.notEqual(inputsOf({ kind: "user_prompted" })[2], inputs[2]);
});
