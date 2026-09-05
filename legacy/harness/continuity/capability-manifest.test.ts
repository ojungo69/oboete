import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import { validateContractValue, type JsonSchemaDocument } from "../schema/validate.ts";
import * as contract from "../schema/continuity.ts";
import { canonicalizeJson, readIJsonFile } from "../schema/jcs.ts";

/**
 * §13 は capability-scenarios.v1.json を「required scenario ID の唯一の権威」と定めているが、
 * 置いただけでは誰も読まない。ここが manifest に対する実行可能なゲート。
 *
 * §13 は `manifestHash` の不一致を preflight 失敗としながら算出方法を書いていないため、
 * 正規化規則は evidence/phase3-capability-scenario-manifest.md 側で定義してある。
 * この test はその規則どおりに再計算して照合する（手で埋めた値を宣言のまま放置しない）。
 */
const root = readIJsonFile<JsonSchemaDocument>(new URL("../schema/continuity.schema.json", import.meta.url));

/**
 * 版ごとにファイルが増える運用（§13 の bump は新しいファイルで足す。CI の
 * "Published manifest versions are immutable" が既存版の変更を拒否する）なので、
 * v1 を名指しで読むと後から足した版が誰にも検査されないまま通る。全部拾う。
 */
const schemaDir = new URL("../schema/", import.meta.url);
const manifestFiles = readdirSync(schemaDir)
  .filter((name) => /^capability-scenarios\.v\d+\.json$/.test(name))
  .sort();

const manifests = manifestFiles.map((file) => ({
  file,
  // 重複 property 名は `JSON.parse` だと後勝ちで潰れ、潰れた後の値からは hash も出てしまう。
  // RFC 8785 §3.1 は canonicalize の入力を I-JSON に限るので、読む段で落とす
  value: readIJsonFile<contract.CapabilityScenarioManifestV1>(new URL(file, schemaDir)),
}));

test("versioned manifest が 1 つ以上あり、ファイル名と manifestVersion が一致する", () => {
  assert.ok(manifests.length > 0, "capability-scenarios.vN.json が 1 つも無い");
  for (const { file, value } of manifests) {
    assert.equal(file, `capability-scenarios.v${value.manifestVersion}.json`);
  }
});

const evidence = readFileSync(
  new URL("../../evidence/phase3-capability-scenario-manifest.md", import.meta.url),
  "utf8",
);

/**
 * evidence に書いた正規化規則。値の側は「scenarios を scenarioId 昇順に並べ、hash 自身を
 * 除く」だけで、符号化は RFC 8785 JCS に任せる（正本 §22.6 が独自 canonical JSON を禁じている）。
 *
 * キーを手で並べないので、scenario に欄が増えたときも自動で hash の入力に入る。
 *
 * 「昇順」は **UTF-16 code unit 順**（JCS §3.2.3 が object のキーに使うのと同じ順序）。
 * scenarioId は正本も schema も `string` としか言っていないので、非 BMP の ID を使うと
 * Unicode scalar 順（＝ Rust の `str` の既定）と食い違い、同じ manifest から別の hash が出る。
 */
function canonicalManifestInput(
  value: contract.CapabilityScenarioManifestV1,
): Omit<contract.CapabilityScenarioManifestV1, "manifestHash"> {
  const { manifestHash: _excluded, ...rest } = value;
  return {
    ...rest,
    scenarios: [...value.scenarios].sort((a, b) =>
      a.scenarioId < b.scenarioId ? -1 : a.scenarioId > b.scenarioId ? 1 : 0,
    ),
  };
}

function computeManifestHash(value: contract.CapabilityScenarioManifestV1): string {
  return createHash("sha256")
    .update(canonicalizeJson(canonicalManifestInput(value)), "utf8")
    .digest("hex");
}

/** 版が増えても検査が漏れないように、全ての versioned manifest に同じ検査をかける */
function forEachManifest(
  fn: (manifest: contract.CapabilityScenarioManifestV1, file: string) => void,
): void {
  for (const { file, value } of manifests) fn(value, file);
}

/** `requiredFor` に必ず「まだ入っていない値」を 1 つ足すための候補（union の全値） */
const EXTRA_REQUIRED_FOR: contract.RequiredCapabilityScenarioV1["requiredFor"] = [
  "generic_phase3",
  "automatic_strategy",
  "tier_a",
];

/**
 * 「hash を計算している」だけでは、入力に含まれない欄を書き換えても通ってしまう。
 * §13 が防ぎたいのは scenario 集合の無言の書き換えなので、そこが効くことを見る。
 *
 * 変異は必ず元の値と違う形で作る（固定値の代入は、その値を既に持っている manifest で
 * no-op になる）。
 */
function assertHashIsSensitive(
  manifest: contract.CapabilityScenarioManifestV1,
  label: string,
): void {
  const [first, ...rest] = manifest.scenarios;
  assert.ok(first, `${label}: manifest が空`);
  const mutations: contract.CapabilityScenarioManifestV1[] = [
    { ...manifest, scenarios: [{ ...first, scenarioId: `${first.scenarioId}-x` }, ...rest] },
    { ...manifest, scenarios: [{ ...first, title: `${first.title} (x)` }, ...rest] },
    {
      ...manifest,
      scenarios: [{ ...first, appliesToAgents: [...first.appliesToAgents, "extra-agent"] }, ...rest],
    },
    {
      ...manifest,
      scenarios: [{ ...first, requiredFor: [...first.requiredFor, ...EXTRA_REQUIRED_FOR] }, ...rest],
    },
    { ...manifest, scenarios: rest },
    { ...manifest, manifestVersion: `${manifest.manifestVersion}-mutated` },
  ];
  for (const mutated of mutations) {
    assert.notEqual(computeManifestHash(mutated), manifest.manifestHash, label);
  }
  // 並べ替えは正規化で吸収される（同じ集合なら同じ hash）
  assert.equal(
    computeManifestHash({ ...manifest, scenarios: [...manifest.scenarios].reverse() }),
    manifest.manifestHash,
    label,
  );
}

test("manifest は CapabilityScenarioManifestV1 を満たす", () =>
  forEachManifest((manifest, file) => {
    assert.deepEqual(
      validateContractValue("CapabilityScenarioManifestV1", manifest, root, contract.CONTINUITY_LIMITS),
      [],
      file,
    );
  }));

test("manifestHash は記録した正規化規則で再計算できる（§13）", () =>
  forEachManifest((manifest, file) => {
    assert.equal(manifest.manifestHash, computeManifestHash(manifest), file);
  }));

test("manifestHash は scenario の変更で必ず変わる", () =>
  forEachManifest((manifest, file) => {
    assertHashIsSensitive(manifest, file);
  }));

test("hash の感度は manifest の中身に依らない（次の版でも成り立つ）", () => {
  // 変異に固定値を使っていると、その値を既に持っている manifest では変異が no-op になり
  // test 自身が偽陽性で落ちる。実データを 1 つ足しただけの合成 v2 でも成立することを見る
  const base = manifests[0]?.value;
  assert.ok(base, "manifest が無い");
  const synthetic: contract.CapabilityScenarioManifestV1 = {
    manifestVersion: "2",
    manifestHash: "",
    scenarios: [
      ...base.scenarios,
      {
        scenarioId: "synthetic-extra",
        title: "Synthetic extra scenario",
        appliesToAgents: ["claude", "codex"],
        requiredFor: ["generic_phase3", "automatic_strategy", "tier_a"],
      },
    ],
  };
  assertHashIsSensitive({ ...synthetic, manifestHash: computeManifestHash(synthetic) }, "synthetic v2");
});

test("scenarioId は重複しない（§13 の exact-set 照合が成立する前提）", () =>
  forEachManifest((manifest, file) => {
    const ids = manifest.scenarios.map((s) => s.scenarioId);
    assert.deepEqual([...new Set(ids)].sort(), [...ids].sort(), file);
    assert.ok(ids.length > 0, `${file}: manifest が空だと preflight が vacuously pass する`);
  }));

test("appliesToAgents は harness が matrix を持つ CLI だけを指す", () =>
  forEachManifest((manifest, file) => {
    // 対応する fixture / matrix が無い agent を書くと、その scenario は永久に
    // disposition が付かず、exact-set 照合が必ず落ちる（または黙って無視される）
    const known = new Set(["claude", "codex"]);
    for (const scenario of manifest.scenarios) {
      assert.ok(scenario.appliesToAgents.length > 0, `${scenario.scenarioId}: appliesToAgents が空`);
      // requiredFor が空だと「manifest に居るがどの preflight/Tier にも必須でない」scenario になり、
      // §13 の exact-set 照合を通ったまま何も要求しない。schema 側は正本どおり空を許すので
      // （addendum は `Array<...>` としか書いていない）、manifest のデータとしてここで弾く
      assert.ok(scenario.requiredFor.length > 0, `${scenario.scenarioId}: requiredFor が空`);
      for (const agent of scenario.appliesToAgents) {
        assert.ok(known.has(agent), `${scenario.scenarioId}: 未知の agent ${agent}`);
      }
    }
  }));

/** `## manifestVersion N` の節から、その版に属する scenarioId 集合と記録された hash を取る */
function evidenceEntry(version: string): { ids: string[]; hash: string | undefined } {
  const section = evidence
    .split(/^## /m)
    .find((s) => s.startsWith(`manifestVersion ${version} `) || s.startsWith(`manifestVersion ${version}\n`));
  assert.ok(section, `evidence に manifestVersion ${version} の節が無い`);
  return {
    // ID の形は正本も schema も `string` としか言っていない。表の 1 列目の backtick の
    // 中身をそのまま取る（kebab-case を暗黙の契約にすると、正当な ID で偽陽性になる）。
    // 区切り行より後ろだけを見る——見出し行の 1 列目も `scenarioId` という backtick なので
    ids: [...(section.split(/^\|[-|\s]+\|$/m)[1] ?? "").matchAll(/^\| `([^`]+)` \|/gm)].map(
      (m) => m[1] as string,
    ),
    hash: /^manifestHash: `([0-9a-f]{64})`$/m.exec(section)?.[1],
  };
}

test("evidence が版ごとの scenario 集合を固定している（§13 の version bump 要件）", () =>
  forEachManifest((manifest, file) => {
    // §13「Adding or removing a required scenario requires a manifest version bump
    // recorded in the evidence file」。ID の出現を数えるだけだと、scenario を 1 つ削って
    // hash を再計算し、evidence の表も直して version は据え置き——が素通りする。
    // 版の節を「その版の集合そのもの」として exact-set で照合し、hash も版に紐付ける。
    // ただし manifest・hash・evidence は同じ変更の中で一緒に書き換えられるので、この test
    // だけでは版の据え置きを塞げない。「公開済みの版のファイルは変更しない」は CI の
    // Published manifest versions are immutable が origin/main を基準に見る
    const entry = evidenceEntry(manifest.manifestVersion);
    assert.deepEqual(
      entry.ids.slice().sort(),
      manifest.scenarios.map((s) => s.scenarioId).sort(),
      file,
    );
    assert.equal(entry.hash, manifest.manifestHash, file);
    assert.equal(entry.hash, computeManifestHash(manifest), file);
  }));

test("scenarios の並べ替えは UTF-16 code unit 順（言語をまたいで同じ hash にするため）", () => {
  // U+10384 は UTF-16 では 0xD800,0xDF84 なので U+FB33 より前、Unicode scalar 順では後ろ。
  // どちらの実装も自然なので、どちらを使うかを test で固定しておく（現行 v1 は ASCII のみ）
  const scenario = (scenarioId: string): contract.RequiredCapabilityScenarioV1 => ({
    scenarioId,
    title: "t",
    appliesToAgents: ["claude"],
    requiredFor: ["generic_phase3"],
  });
  const manifest: contract.CapabilityScenarioManifestV1 = {
    manifestVersion: "9",
    manifestHash: "",
    scenarios: [scenario("\uFB33"), scenario("\u{10384}")],
  };
  assert.ok("\u{10384}" < "\uFB33", "JS の `<` は UTF-16 code unit 比較");
  assert.ok(
    "\u{10384}".codePointAt(0)! > "\uFB33".codePointAt(0)!,
    "code point 順なら逆になる（順序規則を決めないと言語間で hash が割れる）",
  );
  // 同じ集合を逆順で渡しても、正規化後の先頭は UTF-16 順の小さい方になる
  for (const scenarios of [manifest.scenarios, [...manifest.scenarios].reverse()]) {
    assert.equal(
      computeManifestHash({ ...manifest, scenarios }),
      computeManifestHash(manifest),
    );
  }
  // 実際に hash の入力になる並びを見る（test 側で並べ替えを書き直さない）
  assert.equal(canonicalManifestInput(manifest).scenarios[0]?.scenarioId, "\u{10384}");
});
