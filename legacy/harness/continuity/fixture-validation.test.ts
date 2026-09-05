import { test } from "node:test";
import assert from "node:assert/strict";
import { validateFixture, assembleFromFixtures } from "../assemble.ts";
import type { CaptureFixture } from "../schema/capability.ts";
import { validateAgainstSchema, type JsonSchemaDocument } from "../schema/validate.ts";
import { readdirSync } from "node:fs";
import { readIJsonFile } from "../schema/jcs.ts";

const VERSION = "1.2.3-test";
const AT = "2026-08-16T00:00:00.000Z";

function base(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    fixtureId: "claude/high-level",
    cli: "claude",
    nativeVersion: VERSION,
    capturedAt: AT,
    scenario: "prompt delivery",
    scenarioId: "test.prompt-delivery",
    observedEvents: [{ kind: "session_started", at: AT }],
    toolFailurePhasesObserved: [],
    limitations: [],
    limitationCodes: [],
    rig: { isolated: true, internalRunMarker: true },
    ...overrides,
  };
}

test("highLevel の値は schema の enum で検査する", () => {
  // 型でない値: 素通りすると CapabilityEvidence.value に数値が載る
  assert.throws(
    () => validateFixture(base({ highLevel: { compactSingleDelivery: 1 } }), "f.json"),
    /expected type string, got integer/,
  );
  // enum 外の綴り違い
  assert.throws(
    () => validateFixture(base({ highLevel: { promptAwareInjection: "natvie" } }), "f.json"),
    /value not in enum/,
  );
  // highLevel 自体の未知キー。key 名は診断へ出さない（fixture の中身なので）ため、
  // 場所（どの object の何番目か）だけで確かめる
  assert.throws(
    () => validateFixture(base({ highLevel: { promptAwareInjecton: "native" } }), "f.json"),
    /\$\.highLevel: unknown property #1/,
  );
});

test("正しい highLevel は通り、値だけが cell に載る", () => {
  const f = validateFixture(
    base({ highLevel: { promptAwareInjection: "native", promptDeliveryBeforeModel: "native" } }),
    "f.json",
  ) as CaptureFixture;
  const m = assembleFromFixtures([f]);
  assert.equal(m.capabilities.promptAwareInjection.value, "native");
  // 注入が効いたことは応答本文への echo でしか分からず、正規化はそれを伏せる。
  // 導けない主張なので、証拠の有無に関わらず real-cli-e2e にはならない（FR-006c）
  assert.equal(m.capabilities.promptAwareInjection.evidenceKind, "source-test");
  assert.equal(m.capabilities.resumeDeliveryStrategy, "manual_only");
});

test("prompt 対は導けない主張なので tier が上がらない", () => {
  // §8 の synthesized tier は「1 つの実測が対を同時に証明した」ことを要求するが、
  // 対の両 cell は本文に依存する主張で、digest からは導けない。
  // 証拠を足しても tier は上がらない（証拠形式そのものを変える別 issue の担当）
  const f = validateFixture(
    base({ highLevel: { promptAwareInjection: "synthesized", promptDeliveryBeforeModel: "synthesized" } }),
    "f.json",
  ) as CaptureFixture;
  assert.equal(assembleFromFixtures([f]).capabilities.resumeDeliveryStrategy, "manual_only");
});

test("既存 fixture は capability.schema.json 全体に対して妥当（schema と手書き検証の drift 検出）", () => {
  const schema = readIJsonFile<JsonSchemaDocument>(new URL("../schema/capability.schema.json", import.meta.url));
  let checked = 0;
  for (const cli of ["claude", "codex"]) {
    const dir = new URL(`../fixtures/${cli}/`, import.meta.url);
    for (const name of readdirSync(dir)) {
      if (!name.endsWith(".json")) continue;
      const data = readIJsonFile(new URL(name, dir));
      assert.deepEqual(validateAgainstSchema(data, schema, schema), [], `${cli}/${name}`);
      checked++;
    }
  }
  assert.ok(checked >= 8, `fixture が見つかっていない (checked=${checked})`);
});

test("証拠を名指ししていない highLevel は real-cli-e2e として刻まない", () => {
  const declared = validateFixture(
    base({ highLevel: { sessionStartInjection: "native" } }),
    "f.json",
  ) as CaptureFixture;
  const cell = assembleFromFixtures([declared]).capabilities.sessionStartInjection;
  assert.equal(cell.value, "native");
  assert.equal(cell.evidenceKind, "source-test");
  assert.ok(cell.limitations.some((l) => /^unverified:/.test(l)));
  // 自動配送は有効にならない
  assert.equal(assembleFromFixtures([declared]).capabilities.resumeDeliveryStrategy, "manual_only");
});

test("形式は正しい 64 桁 hex でも、実在しない記録を指す fixture は組み立てを落とす", () => {
  const forged = validateFixture(
    base({
      evidence: [
        {
          path: "no-such-capture.jsonl",
          evidenceHash: "a".repeat(64),
          captureRawHash: "b".repeat(64),
          normalizationVersion: 1,
        },
      ],
    }),
    "f.json",
  ) as CaptureFixture;
  // `does not exist` は証拠置き場そのものが解決できないときの別経路。ここで許すと、
  // 個別の forge を拒めなくなっても「root が無い」で落ちて test が緑のままになる
  assert.throws(() => assembleFromFixtures([forged]), /cannot be resolved/);
});

test("evidence が空配列の fixture は schema でも組み立てでも棄却される", () => {
  assert.throws(() => validateFixture(base({ evidence: [] }), "f.json"), /minItems/);
});

test("commit された matrix も I-JSON として読める", () => {
  // CI の「Matrices are regenerated, not hand-edited」は `jq` で読む。`jq` は重複キーを
  // 後勝ちで潰すので、先頭に偽の値を差し込んで末尾に正規の値を残した matrix でも
  // diff が通ってしまう（厳格な parser と first-wins parser では別の値が見える）。
  // 契約データとして読める形であることを、ここで別途縛る
  const dir = new URL("../matrix/", import.meta.url);
  let checked = 0;
  for (const name of readdirSync(dir)) {
    if (!name.endsWith(".json")) continue;
    readIJsonFile(new URL(name, dir));
    checked++;
  }
  assert.ok(checked >= 2, `matrix が見つかっていない (checked=${checked})`);
});
