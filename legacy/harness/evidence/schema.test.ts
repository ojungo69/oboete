// schema 側の検査を固定する。手書き検証との drift 防止で assemble.ts が schema へ委譲している
// 欄（evidence / highLevel / limitationCodes / observedEvents / scenarioId）は、schema を緩めた
// だけで matrix の cell に載る値の制約が消える。unit test は schema ファイルには届かないので
// （config-fix-needs-its-own-mutation）、schema そのものを対象にした test をここに置く。
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { validateFixture } from "../assemble.ts";
import * as capabilityModule from "../schema/capability.ts";
import { EVENT_KINDS, TOOL_FAILURE_PHASES } from "../schema/capability.ts";
import { SUPPORTED_KEYWORDS, validateAgainstSchema } from "../schema/validate.ts";
import { fixtureBase } from "./synthetic.ts";

const HEX = "a".repeat(64);
// ref が持つのは manifest 本体ではなく置き場からの相対 path。読むのは verify 側
const MANIFEST_PATH = "claude-lifecycle-basic.manifest.json";
const readSchema = (name: string): Record<string, unknown> =>
  JSON.parse(readFileSync(new URL(`../schema/${name}`, import.meta.url), "utf8"));

const ref = (extra: Record<string, unknown> = {}): Record<string, unknown> => ({
  path: "claude-lifecycle-basic.jsonl",
  evidenceHash: HEX,
  captureRawHash: HEX,
  normalizationVersion: 1,
  ...extra,
});

// schema の「どこが schema 位置か」は容器キーワードで決まる。値が任意名の器
// （properties / $defs / patternProperties）とそれ以外を分けないと、property 名を
// keyword と読んで偽陽性になる。
const NAMED_SCHEMA_MAPS = new Set(["properties", "$defs", "patternProperties"]);
const SCHEMA_VALUES = new Set(["items", "additionalProperties", "not", "if", "then", "else", "contains"]);
const SCHEMA_LISTS = new Set(["allOf", "anyOf", "oneOf", "prefixItems"]);

function collectKeywords(node: unknown, out: Set<string>): void {
  if (typeof node !== "object" || node === null || Array.isArray(node)) return;
  for (const [key, value] of Object.entries(node as Record<string, unknown>)) {
    out.add(key);
    if (NAMED_SCHEMA_MAPS.has(key)) {
      for (const child of Object.values(value as Record<string, unknown>)) collectKeywords(child, out);
    } else if (SCHEMA_VALUES.has(key)) {
      collectKeywords(value, out);
    } else if (SCHEMA_LISTS.has(key)) {
      for (const child of value as unknown[]) collectKeywords(child, out);
    }
  }
}

test("capability schema uses only supported keywords", () => {
  const found = new Set<string>();
  for (const name of ["capability.schema.json", "evidence-manifest.schema.json"]) {
    collectKeywords(readSchema(name), found);
  }
  // 走査自体が空振りしていないことを先に見る（歩き方を壊すと全件 pass になる）
  assert.ok(found.size > 15, `keyword が ${found.size} 件しか集まっていない`);
  assert.ok(SUPPORTED_KEYWORDS.length > 15, "SUPPORTED_KEYWORDS の取り込みに失敗している");
  const unsupported = [...found].filter((k) => !SUPPORTED_KEYWORDS.includes(k)).sort();
  assert.deepEqual(unsupported, [], `validate.ts が解釈しない keyword: ${unsupported.join(", ")}`);
});

// assemble.ts は schema と別に自前の集合を持ち、弾いたときの文言では schema を権威として
// 名指しする（assemble.ts:106 の「capability.schema.json lists」）。その 2 つがずれると、
// schema が載せている値を「schema に無い」と言って弾く／その逆が起きる。
// 対象は **実行時に成果物を左右する定数だけ**にする。読み手のいない定数を schema の写しと
// 突き合わせても、両側を一緒に書き換える編集は素通りする
// `raw` は schema の kind enum にだけあり、assemble.ts:106 が `ev.kind !== "raw"` で
// 集合の外に出して受ける sentinel。TS 側の EventKind には入れない、という取り決めをここに書く
const SCHEMA_ONLY_KINDS = ["raw"] as const;
const SCHEMA_ENUM_MIRRORS = [
  {
    // TS 側の出どころ。下の「登録漏れ」test がこの名前で capability.ts の export と照合する
    constants: ["EVENT_KINDS"],
    values: [...EVENT_KINDS, ...SCHEMA_ONLY_KINDS],
    path: "properties.observedEvents.items.properties.kind.enum",
  },
  {
    constants: ["TOOL_FAILURE_PHASES"],
    values: TOOL_FAILURE_PHASES,
    path: "properties.toolFailurePhasesObserved.items.enum",
  },
] as const;

// path は文字列で持って歩く。`(s: any) => s.properties.…` の picker は、schema の形が変わった
// ときに undefined を読んだ TypeError か静かな undefined しか残さず、**どの段で外れたか**を
// 言わない。外れた segment を名指しできれば、直す側は schema を読み直さずに済む
function enumAt(schema: unknown, path: string): readonly string[] {
  let node: unknown = schema;
  const walked: string[] = [];
  for (const key of path.split(".")) {
    const here = walked.join(".") || "(root)";
    const descriptor =
      typeof node === "object" && node !== null && !Array.isArray(node)
        ? Object.getOwnPropertyDescriptor(node, key)
        : undefined;
    assert.ok(descriptor, `schema の ${here} に ${key} が無い（path: ${path}）`);
    node = descriptor.value;
    walked.push(key);
  }
  assert.ok(Array.isArray(node), `${path} が配列ではない`);
  return node as readonly string[];
}

void test("assemble が使う定数と schema の enum が一致する", () => {
  const schema = readSchema("capability.schema.json");
  const sorted = (xs: readonly string[]) => [...xs].sort();
  for (const { constants, values, path } of SCHEMA_ENUM_MIRRORS) {
    assert.deepEqual(sorted(values), sorted(enumAt(schema, path)), constants.join(" + "));
  }
});

void test("capability.ts の定数に、schema と突き合わせていないものが無い", () => {
  // 対象を手で並べると、次に定数が増えた日に黙って漏れる（hash-inputs-derive-from-structure）。
  // module の export から導いて、登録されていない文字列定数を名指しで落とす。
  // 突き合わせないと決めた定数がいずれ出たら、ここに理由を書いて除外する
  const exported = Object.entries(capabilityModule)
    .filter(([, value]) => Array.isArray(value) && value.every((x) => typeof x === "string"))
    .map(([name]) => name);
  assert.ok(exported.length > 0, "capability.ts から文字列定数を取り込めていない");
  const registered = new Set(SCHEMA_ENUM_MIRRORS.flatMap((m) => m.constants as readonly string[]));
  const missing = exported.filter((name) => !registered.has(name));
  assert.deepEqual(missing, [], `SCHEMA_ENUM_MIRRORS に登録されていない: ${missing.join(", ")}`);
});

void test("unknown source event is rejected", () => {
  // sourceEvents の enum は「閉じている」ことだけが値で、TS 側に読み手が無い。
  // 定数の写しではなく、未知の値が実際に弾かれることで縛る
  assert.throws(
    () =>
      validateFixture(
        fixtureBase({
          observedEvents: [
            { kind: "session_started", at: "2026-01-01T00:00:00.000Z", capability: "native", sourceEvents: ["Nope"] },
          ],
        }),
        "f.json",
      ),
    /enum/,
  );
});

test("the accepted keywords cannot be widened at run time", () => {
  // export しているのは複製で、検証が見る集合は module の中にある。型の `readonly` は実行前に
  // 剥がされるので、同じ process の別 module は export された配列を書き換えられる——それでも
  // 「対応していない keyword」は落ち続ける（広がると、その keyword の制約が黙って無効になる）
  const widened = SUPPORTED_KEYWORDS as string[];
  widened.push("unevaluatedProperties");
  try {
    const schema = { type: "object", unevaluatedProperties: false };
    assert.throws(
      () => validateAgainstSchema({}, schema, schema as never),
      /unsupported schema keyword/,
      "export した一覧へ足しただけで、検証が受け付ける keyword が広がった",
    );
  } finally {
    widened.pop();
  }
});

test("fixture with evidence is rejected when the schema lacks it", () => {
  // schema の properties が KNOWN_KEYS の正本。evidence の定義を落とすと、この fixture が
  // 「unknown top-level key」で落ちるようになる
  validateFixture(fixtureBase({ evidence: [ref()] }), "f.json");
  assert.throws(() => validateFixture(fixtureBase({ bogusKey: 1 }), "f.json"), /unknown top-level key/);
});

test("manifest and manifestHash must appear together", () => {
  validateFixture(fixtureBase({ evidence: [ref({ manifest: MANIFEST_PATH, manifestHash: HEX })] }), "f.json");
  assert.throws(() => validateFixture(fixtureBase({ evidence: [ref({ manifest: MANIFEST_PATH })] }), "f.json"), /manifest/);
  assert.throws(() => validateFixture(fixtureBase({ evidence: [ref({ manifestHash: HEX })] }), "f.json"), /manifest/);
});

test("unknown limitation code is rejected", () => {
  assert.throws(() => validateFixture(fixtureBase({ limitations: ["x"], limitationCodes: ["nope"] }), "f.json"), /enum/);
  // 事象側の code は mergedLimits を通って cell の limitations に載るので、同じ検査が要る
  assert.throws(
    () =>
      validateFixture(
        fixtureBase({
          observedEvents: [
            {
              kind: "session_started",
              at: "2026-01-01T00:00:00.000Z",
              capability: "native",
              sourceEvents: ["SessionStart"],
              limitations: ["x"],
              limitationCodes: ["nope"],
            },
          ],
        }),
        "f.json",
      ),
    /enum/,
  );
});

test("empty evidence array is rejected", () => {
  assert.throws(() => validateFixture(fixtureBase({ evidence: [] }), "f.json"), /minItems|少なく|at least/);
});

test("provenance fields that reach the matrix are pattern-constrained", () => {
  // schema に pattern を書いても、検査が欄を選んで委譲していれば誰も読まない。
  // fixtureId は sourceFixtureId・evidenceSources へ、nativeVersion と capturedAt は
  // matrix と cell の verifiedAt へそのまま出るので、自由文と制御文字を通さない
  validateFixture(fixtureBase(), "f.json");
  for (const [label, override] of [
    ["制御文字入りの版", { nativeVersion: `1.0${String.fromCharCode(27)}[31m` }],
    ["絶対 path 風の fixtureId", { fixtureId: "/home/someone/secret/x" }],
    ["自由文の capturedAt", { capturedAt: "きのう" }],
    ["区切りが違う capturedAt", { capturedAt: "2026-08-12 11:00:00" }],
    ["13 月", { capturedAt: "2026-13-01T00:00:00.000Z" }],
    ["24 時", { capturedAt: "2026-08-12T24:00:00.000Z" }],
    ["60 分", { capturedAt: "2026-08-12T11:60:00.000Z" }],
    ["うるう秒", { capturedAt: "2026-08-12T11:00:60.000Z" }],
  ] as const) {
    assert.throws(() => validateFixture(fixtureBase(override), "f.json"), /does not match pattern/, label);
  }
});

test("timestamps that pass the pattern but do not exist on the calendar are rejected", () => {
  // pattern は桁数と範囲しか見ない。2 月 30 日と 4 月 31 日は綴りとしては通る
  for (const bad of ["2026-02-30T00:00:00.000Z", "2026-04-31T00:00:00.000Z", "2025-02-29T00:00:00.000Z"]) {
    assert.throws(() => validateFixture(fixtureBase({ capturedAt: bad }), "f.json"), /not a real instant/, bad);
  }
  // 事象側の at も同じ検査に載る（cell の verifiedAt へは出ないが、同じ自由文の経路）
  assert.throws(
    () =>
      validateFixture(
        fixtureBase({
          observedEvents: [
            { kind: "session_started", at: "2026-02-30T00:00:00.000Z", capability: "native", sourceEvents: ["SessionStart"] },
          ],
        }),
        "f.json",
      ),
    /not a real instant/,
  );
  // 通す側: うるう年の 2 月 29 日は実在する
  validateFixture(fixtureBase({ capturedAt: "2024-02-29T00:00:00.000Z" }), "f.json");
});
