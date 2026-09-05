import { test } from "node:test";
import assert from "node:assert/strict";
import {
  findNonJsonValues,
  findStructuralViolations,
  validateAgainstSchema,
  validateContractValue,
  type JsonSchemaDocument,
  type StructuralLimits,
} from "../schema/validate.ts";
import { CONTINUITY_LIMITS } from "../schema/continuity.ts";
import { readIJsonFile } from "../schema/jcs.ts";

const LIMITS = { jsonDepth: 4, stringUtf8Bytes: 16, arrayItems: 3, objectKeys: 3 };

const ROOT: JsonSchemaDocument = {
  $defs: {
    IsoTimestamp: {
      type: "string",
      pattern: "^\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}(\\.\\d{3})?Z$",
    },
    DecimalString: { type: "string", pattern: "^-?(0|[1-9][0-9]*)(\\.[0-9]+)?$" },
    Role: { type: "string", enum: ["primary", "side", "subagent"] },
    Binding: {
      type: "object",
      additionalProperties: false,
      required: ["role", "createdAt"],
      properties: {
        role: { $ref: "#/$defs/Role" },
        createdAt: { $ref: "#/$defs/IsoTimestamp" },
        score: { $ref: "#/$defs/DecimalString" },
      },
    },
    Command: {
      oneOf: [
        {
          type: "object",
          additionalProperties: false,
          required: ["kind", "attemptId"],
          properties: { kind: { const: "accept" }, attemptId: { type: "string" } },
        },
        {
          type: "object",
          additionalProperties: false,
          required: ["kind", "reason"],
          properties: { kind: { const: "dismiss" }, reason: { type: "string" } },
        },
      ],
    },
  },
};

test("未知のプロパティを拒否する（additionalProperties: false）", () => {
  const issues = validateAgainstSchema(
    { role: "primary", createdAt: "2026-08-16T00:00:00.000Z", extra: 1 },
    ROOT.$defs!.Binding,
    ROOT,
  );
  assert.equal(issues.length, 1);
  // key 名は載せない。位置だけで場所は足りる
  assert.match(issues[0].message, /unknown property #3/);
});

test("required の欠落を拒否する", () => {
  const issues = validateAgainstSchema({ role: "primary" }, ROOT.$defs!.Binding, ROOT);
  assert.match(issues[0].message, /missing required property: createdAt/);
});

test("enum 外の値を拒否する", () => {
  const issues = validateAgainstSchema(
    { role: "owner", createdAt: "2026-08-16T00:00:00.000Z" },
    ROOT.$defs!.Binding,
    ROOT,
  );
  assert.match(issues[0].message, /not in enum/);
});

test("ISO timestamp でない文字列を拒否する", () => {
  for (const bad of ["2026-08-16", "2026-08-16 00:00:00Z", "2026-08-16T00:00:00+09:00", ""]) {
    const issues = validateAgainstSchema(
      { role: "primary", createdAt: bad },
      ROOT.$defs!.Binding,
      ROOT,
    );
    assert.ok(issues.length > 0, `accepted bad timestamp: ${bad}`);
  }
  const ok = validateAgainstSchema(
    { role: "primary", createdAt: "2026-08-16T00:00:00Z" },
    ROOT.$defs!.Binding,
    ROOT,
  );
  assert.deepEqual(ok, []);
});

test("decimal string でない score を拒否する（数値も拒否する）", () => {
  for (const bad of ["1.", ".5", "01", "1e3", "abc"]) {
    const issues = validateAgainstSchema(
      { role: "side", createdAt: "2026-08-16T00:00:00Z", score: bad },
      ROOT.$defs!.Binding,
      ROOT,
    );
    assert.ok(issues.length > 0, `accepted bad decimal: ${bad}`);
  }
  const numeric = validateAgainstSchema(
    { role: "side", createdAt: "2026-08-16T00:00:00Z", score: 0.5 },
    ROOT.$defs!.Binding,
    ROOT,
  );
  assert.match(numeric[0].message, /expected type string/);
});

test("discriminated union はちょうど 1 variant に一致しなければならない", () => {
  assert.deepEqual(validateAgainstSchema({ kind: "accept", attemptId: "a1" }, ROOT.$defs!.Command, ROOT), []);
  const wrongShape = validateAgainstSchema({ kind: "accept", reason: "x" }, ROOT.$defs!.Command, ROOT);
  assert.match(wrongShape[0].message, /expected exactly 1 oneOf match, got 0/);
});

test("oneOf は 2 件一致した時点で打ち切るが、不一致として報告する", () => {
  const schema = { oneOf: [{ type: "string" }, { type: "string" }, { type: "string" }] };
  const issues = validateAgainstSchema("x", schema, ROOT);
  assert.match(issues[0].message, /expected exactly 1 oneOf match, got 2 or more/);
});

test("未対応の schema キーワードは黙って無視せずエラーにする", () => {
  assert.throws(
    () => validateAgainstSchema("x", { type: "string", multipleOf: 2 }, ROOT),
    /unsupported schema keyword at \$: multipleOf/,
  );
});

test("キーワードの値の型違いも検出する（制約が黙って無効化される）", () => {
  assert.throws(
    () => validateAgainstSchema("a", { type: "string", minLength: "3" }, ROOT),
    /schema keyword minLength at \$ must be a non-negative integer/,
  );
  // required: "kind" は文字列を 1 文字ずつ必須プロパティ名として読み、無関係な issue を出す
  assert.throws(
    () => validateAgainstSchema({}, { type: "object", required: "kind" }, ROOT),
    /schema keyword required at \$ must be an array of strings/,
  );
});

test("再帰 schema でも深い値でスタックを割らずに issue で返す", () => {
  // 子降下で refStack を畳むため、再帰 schema では値の深さが唯一の歯止めになる
  const root: JsonSchemaDocument = {
    $defs: { J: { anyOf: [{ type: "string" }, { type: "object", additionalProperties: { $ref: "#/$defs/J" } }] } },
  };
  let deep: unknown = "leaf";
  for (let i = 0; i < 3000; i++) deep = { a: deep };
  assert.throws(() => validateAgainstSchema(deep, { $ref: "#/$defs/J" }, root), /value nesting deeper than 200/);
  // 信頼境界では JSON 妥当性検査が先に同じ深さで弾くので、例外は外まで出ない
  assert.match(validateContractValue("J", deep, root, LIMITS)[0].message, /nesting deeper than 200/);
});

test("壊れた pattern は、値が文字列でなくても検出する", () => {
  // pattern の評価は値が文字列のときだけ走る。schema の誤りは値に依らない欠陥なので
  // 「たまたま数値だったから素通り」を許さない
  assert.throws(
    () => validateAgainstSchema(1, { type: "number", pattern: "[" }, ROOT),
    /invalid pattern at \$: \[/,
  );
});

test("schema の位置にスカラーが入っていたら preflight で弾く", () => {
  // データ側の issue にすると、一致した分岐に飲まれて消える
  assert.throws(
    () => validateAgainstSchema("ok", { anyOf: [42, { type: "string" }] }, ROOT),
    /schema at \$\.anyOf\[0\] must be an object or boolean, got number/,
  );
  // boolean schema は合法なので通す
  assert.deepEqual(validateAgainstSchema("ok", { anyOf: [true, { type: "number" }] }, ROOT), []);
});

test("$ref から辿れない $defs の誤記も検出する", () => {
  const root: JsonSchemaDocument = {
    $defs: { Used: { type: "string" }, Unused: { type: "string", format: "date-time" } },
  };
  assert.throws(
    () => validateAgainstSchema("x", root, root),
    /unsupported schema keyword at \$\.\$defs\.Unused: format/,
  );
});

test("minLength / maxLength は Unicode code point で数える", () => {
  // "😀".length は UTF-16 code unit 数で 2。code point では 1 文字
  assert.deepEqual(validateAgainstSchema("😀", { type: "string", maxLength: 1 }, ROOT), []);
  assert.match(
    validateAgainstSchema("😀😀", { type: "string", maxLength: 1 }, ROOT)[0].message,
    /longer than maxLength 1/,
  );
});

test("dangling $ref は例外にする", () => {
  assert.throws(() => validateAgainstSchema({}, { $ref: "#/$defs/Nope" }, ROOT), /dangling \$ref/);
});

test("JSON にできない値を拒否する", () => {
  assert.match(findNonJsonValues({ a: undefined })[0].message, /non-JSON value of type undefined/);
  assert.match(findNonJsonValues({ a: Number.NaN })[0].message, /non-finite number/);
  assert.match(findNonJsonValues({ a: () => 1 })[0].message, /non-JSON value of type function/);
  assert.match(findNonJsonValues({ a: 1n })[0].message, /non-JSON value of type bigint/);
  assert.match(findNonJsonValues({ a: new Date() })[0].message, /unsupported object type/);
  const cyclic: Record<string, unknown> = {};
  cyclic.self = cyclic;
  assert.match(findNonJsonValues(cyclic)[0].message, /circular reference/);
  assert.deepEqual(findNonJsonValues({ a: [1, "x", null, { b: true }] }), []);
});

test("構造上限（深さ・文字列バイト・要素数・キー数）を拒否する", () => {
  assert.match(
    findStructuralViolations({ a: { b: { c: { d: 1 } } } }, LIMITS)[0].message,
    /exceeds jsonDepth/,
  );
  assert.match(findStructuralViolations({ s: "あ".repeat(6) }, LIMITS)[0].message, /exceeds stringUtf8Bytes/);
  assert.match(findStructuralViolations({ a: [1, 2, 3, 4] }, LIMITS)[0].message, /exceeds arrayItems/);
  assert.match(findStructuralViolations({ a: 1, b: 2, c: 3, d: 4 }, LIMITS)[0].message, /exceeds objectKeys/);
  assert.deepEqual(findStructuralViolations({ a: ["ok"] }, LIMITS), []);
});

test("validateContractValue は JSON 妥当性を schema より先に見る", () => {
  const issues = validateContractValue("Binding", { role: undefined }, ROOT, LIMITS);
  assert.equal(issues.length, 1);
  assert.match(issues[0].message, /non-JSON value/);
});

test("validateContractValue は未知の $defs 名を拒否する", () => {
  const issues = validateContractValue("Nope", {}, ROOT, LIMITS);
  assert.match(issues[0].message, /unknown \$defs entry: Nope/);
});

test("validateContractValue は schema 通過後に構造上限も見る", () => {
  const issues = validateContractValue(
    "Binding",
    { role: "primary", createdAt: "2026-08-16T00:00:00.000Z" },
    ROOT,
    { ...LIMITS, stringUtf8Bytes: 4 },
  );
  assert.ok(issues.some((i) => /exceeds stringUtf8Bytes/.test(i.message)));
});

// --- プロトタイプ経由のキーを実データと取り違えない（PR #18 レビュー指摘） ---

test("required は継承プロパティを「有る」と数えない", () => {
  const schema = { type: "object", required: ["constructor"], properties: {} };
  const issues = validateAgainstSchema({}, schema, ROOT);
  assert.ok(issues.some((i) => /missing required property: constructor/.test(i.message)));
});

test("properties に無い継承名のキーは additionalProperties: false で弾かれる", () => {
  const schema = {
    type: "object",
    additionalProperties: false,
    properties: { role: { $ref: "#/$defs/Role" } },
  };
  for (const key of ["constructor", "toString", "hasOwnProperty"]) {
    const issues = validateAgainstSchema({ [key]: 1 }, schema, ROOT);
    assert.ok(issues.some((i) => i.message === "unknown property #1"), key);
  }
});

test("$defs に無い継承名は $ref でも contract 名でも解決されない", () => {
  assert.throws(
    () => validateAgainstSchema({}, { $ref: "#/$defs/__proto__" }, ROOT),
    /dangling \$ref/,
  );
  const issues = validateContractValue("toString", { any: "thing" }, ROOT, LIMITS);
  assert.match(issues[0].message, /unknown \$defs entry: toString/);
});

test("format は未対応キーワードとして拒否する（黙って無視しない）", () => {
  assert.throws(
    () => validateAgainstSchema("not-a-date", { type: "string", format: "date-time" }, ROOT),
    /unsupported schema keyword at \$: format/,
  );
});

test("$ref に併記した兄弟キーワードも評価する（draft 2020-12）", () => {
  const schema = { $ref: "#/$defs/Role", enum: ["primary"] };
  assert.deepEqual(validateAgainstSchema("primary", schema, ROOT), []);
  // $ref は通るが enum で落ちる: 早期 return だと素通りしていた
  const issues = validateAgainstSchema("side", schema, ROOT);
  assert.equal(issues.length, 1);
  assert.match(issues[0].message, /value not in enum/);
  // $ref 側で落ちる場合も両方報告する
  assert.ok(validateAgainstSchema(1, schema, ROOT).some((i) => /expected type string/.test(i.message)));
});

// --- /code-review の指摘（PR #18）で塞いだ穴の回帰テスト ---

test("__proto__ という名前のデータキーも additionalProperties: false で弾かれる", () => {
  const schema = { type: "object", additionalProperties: false, properties: { role: { type: "string" } } };
  const value = JSON.parse('{"role":"primary","__proto__":{"smuggled":"payload"}}');
  const issues = validateAgainstSchema(value, schema, ROOT);
  assert.ok(issues.some((i) => i.message === "unknown property #2"));
});

test("循環 $ref はスタックオーバーフローでなく診断可能なエラーにする", () => {
  const root: JsonSchemaDocument = { $defs: { Loop: { $ref: "#/$defs/Loop" } } };
  assert.throws(() => validateAgainstSchema("x", { $ref: "#/$defs/Loop" }, root), /circular \$ref/);
});

test("再帰 schema（JsonValue）は循環扱いにしない — 1 段ごとに値を消費するため", () => {
  const root: JsonSchemaDocument = {
    $defs: {
      JsonValue: {
        oneOf: [
          { type: "string" },
          { type: "array", items: { $ref: "#/$defs/JsonValue" } },
          { type: "object", additionalProperties: { $ref: "#/$defs/JsonValue" } },
        ],
      },
    },
  };
  const schema = { $ref: "#/$defs/JsonValue" };
  assert.deepEqual(validateAgainstSchema({ a: ["x", { b: "y" }] }, schema, root), []);
  assert.ok(validateAgainstSchema({ a: [1] }, schema, root).length > 0, "中身の型違反は拾う");
});

test("NaN は minimum/maximum を素通りしない", () => {
  const issues = validateAgainstSchema(NaN, { type: "number", minimum: 0, maximum: 1 }, ROOT);
  assert.ok(issues.some((i) => /non-finite number/.test(i.message)));
  assert.deepEqual(validateAgainstSchema(0.5, { type: "number", minimum: 0, maximum: 1 }, ROOT), []);
});

test("const / enum の比較はキー順に依存しない", () => {
  assert.deepEqual(validateAgainstSchema({ b: 2, a: 1 }, { const: { a: 1, b: 2 } }, ROOT), []);
  assert.equal(validateAgainstSchema({ a: 1 }, { const: { a: 1, b: 2 } }, ROOT).length, 1);
  assert.deepEqual(validateAgainstSchema({ b: 2, a: 1 }, { enum: [{ a: 1, b: 2 }] }, ROOT), []);
});

test("if / then / else を評価する", () => {
  const schema = {
    type: "object",
    properties: { capability: { type: "string" }, sourceEvents: { type: "array" } },
    if: { required: ["capability"], properties: { capability: { const: "synthesized" } } },
    then: { required: ["sourceEvents"], properties: { sourceEvents: { minItems: 1 } } },
    else: { required: ["capability"] },
  };
  // if 成立 → then が効く
  assert.ok(
    validateAgainstSchema({ capability: "synthesized" }, schema, ROOT).some((i) =>
      /missing required property: sourceEvents/.test(i.message),
    ),
  );
  assert.deepEqual(
    validateAgainstSchema({ capability: "synthesized", sourceEvents: ["x"] }, schema, ROOT),
    [],
  );
  // if 不成立 → else が効く。if 自体の不一致は issue にしない
  assert.ok(validateAgainstSchema({}, schema, ROOT).some((i) => /missing required property: capability/.test(i.message)));
});

test("JSON に無い配列の状態を妥当性検査で見逃さない", () => {
  // JSON.stringify は hole を null にするため「検証した形」と「保存する形」がずれる
  assert.ok(findNonJsonValues([1, , 3]).some((i) => /1 holes \(length 3\)/.test(i.message)));

  // 添字以外の own key と getter も JSON に出ない（消えた欄が検証済みとして通る）
  const tagged: unknown[] & { audit?: unknown } = [1];
  tagged.audit = { shouldNotDisappear: true };
  assert.equal(JSON.stringify(tagged), "[1]");
  assert.ok(findNonJsonValues(tagged).some((i) => /non-index own key: audit/.test(i.message)));

  const accessor: unknown[] = [];
  Object.defineProperty(accessor, "0", { enumerable: true, get: () => 1 });
  Object.defineProperty(accessor, "length", { value: 1 });
  assert.ok(findNonJsonValues(accessor).some((i) => /getter\/setter/.test(i.message)));

  // prototype 側の添字で穴を埋めても own property ではない（値は読めるのに JSON には出ない）
  const proto: unknown[] = Object.create(Array.prototype) as unknown[];
  proto[0] = "inherited";
  const inherited = Object.setPrototypeOf(Array(1), proto) as unknown[];
  assert.equal(inherited[0], "inherited");
  assert.ok(findNonJsonValues(inherited).some((i) => /1 holes \(length 1\)/.test(i.message)));

  assert.deepEqual(findNonJsonValues([1, "a", { b: [2] }]), []);

  // 穴は 1 件にまとめる。1 つずつ issue にすると Array(2**32-1) で検証側が停止する
  assert.equal(findNonJsonValues(Array(500_000)).length, 1);

  // object 側も getter を呼ばず、symbol と非 enumerable を落とす
  let reads = 0;
  const changing = Object.defineProperty({}, "a", {
    enumerable: true,
    get: () => (++reads <= 1 ? "safe" : "changed"),
  });
  assert.ok(findNonJsonValues(changing).some((i) => /getter\/setter/.test(i.message)));
  assert.equal(reads, 0, "検証は getter を呼ばない");
  const hidden = Object.defineProperty({}, "hidden", { value: "lost" });
  assert.equal(JSON.stringify(hidden), "{}");
  assert.ok(findNonJsonValues(hidden).some((i) => /non-enumerable/.test(i.message)));
  assert.ok(
    findNonJsonValues({ [Symbol("s")]: 1 }).some((i) => /symbol key/.test(i.message)),
  );

  // Proxy は descriptor と実際の読み出しで別の値を返せる
  const proxy = new Proxy({ a: 0 }, { get: () => Math.floor(1) });
  assert.ok(findNonJsonValues(proxy).some((i) => /Proxy/.test(i.message)));
});

test("anyOf / oneOf の分岐にある schema 誤記を飲み込まない", () => {
  // 他の分岐が一致しても、書き間違えた分岐は表に出す
  assert.throws(
    () => validateAgainstSchema("hello", { anyOf: [{ type: "string", multipleOf: 2 }, { type: "string" }] }, ROOT),
    /unsupported schema keyword at \$\.anyOf\[0\]: multipleOf/,
  );
  // oneOf の不一致メッセージだけになって原因が消えないこと
  assert.throws(
    () =>
      validateAgainstSchema(
        { kind: "accept", attemptId: "a1" },
        { oneOf: [{ type: "object", minLenght: 3 }, { type: "number" }] },
        ROOT,
      ),
    /unsupported schema keyword at \$\.oneOf\[0\]: minLenght/,
  );
});

test("分岐の奥（properties の下）に隠れた誤記も、データに依らず検出する", () => {
  // データ次第で分岐が一致すると issue 方式では丸ごと消えるため、schema 検査は値と独立に走らせる
  const schema = { anyOf: [{ properties: { x: { format: "date-time" } } }, { type: "object" }] };
  assert.throws(
    () => validateAgainstSchema({ x: "bad" }, schema, ROOT),
    /unsupported schema keyword at \$\.anyOf\[0\]\.properties\.x: format/,
  );
  // items / additionalProperties / then の下も同じ
  assert.throws(
    () => validateAgainstSchema([], { items: { uniqueItems: true } }, ROOT),
    /unsupported schema keyword at \$\.items: uniqueItems/,
  );
  assert.throws(
    () => validateAgainstSchema({}, { if: { type: "object" }, then: { dependentRequired: {} } }, ROOT),
    /unsupported schema keyword at \$\.then: dependentRequired/,
  );
});

test("object の key も stringUtf8Bytes の対象にする", () => {
  const longKey = "k".repeat(200);
  const issues = findStructuralViolations({ [longKey]: 1 }, { ...LIMITS, stringUtf8Bytes: 64 });
  assert.ok(issues.some((i) => /key 200B exceeds stringUtf8Bytes 64/.test(i.message)));
  // 値側の判定を壊していないこと
  assert.deepEqual(findStructuralViolations({ ok: "short" }, { ...LIMITS, stringUtf8Bytes: 64 }), []);
});

test("深すぎるネストは例外でなく issue で返す（信頼境界を落とさない）", () => {
  let deep: Record<string, unknown> = { n: 1 };
  for (let i = 0; i < 5000; i++) deep = { n: deep };
  const issues = validateContractValue("Binding", deep, ROOT, LIMITS);
  assert.ok(issues.some((i) => /nesting deeper than/.test(i.message)));
  // findStructuralViolations 単体でも同じ（循環値でスタックを食い潰さない）
  assert.ok(findStructuralViolations(deep, { ...LIMITS, jsonDepth: 100000 }).some((i) => /nesting deeper than/.test(i.message)));
});

test("$defs 名にハイフン・ドットを使える", () => {
  const root: JsonSchemaDocument = { $defs: { "Iso-Timestamp": { type: "string" } } };
  assert.deepEqual(validateAgainstSchema("x", { $ref: "#/$defs/Iso-Timestamp" }, root), []);
  assert.equal(validateAgainstSchema(1, { $ref: "#/$defs/Iso-Timestamp" }, root).length, 1);
});

test("$defs の誤記は validateContractValue（本番の入口）からも検出する", () => {
  // 入口が渡すのは root ではなく $defs の 1 つ。root を歩かないと、参照が外れた定義の
  // 誤記だけが検査を免れ、「どの契約を検証したか」で schema の正しさが変わる
  const root: JsonSchemaDocument = {
    $defs: { Used: { type: "string" }, Unused: { type: "object", required: "kind" } },
  };
  assert.throws(
    () => validateContractValue("Used", "x", root, LIMITS),
    /schema keyword required at \$\.\$defs\.Unused must be an array of strings/,
  );
});

test("常に不一致になる schema（空の type / enum / anyOf）も誤記として弾く", () => {
  for (const bad of [{ type: [] }, { type: [1, 2] }, { enum: [] }, { anyOf: [] }, { oneOf: [] }]) {
    assert.throws(() => validateAgainstSchema("x", bad, ROOT), /must be a (string or )?non-empty/);
  }
  // allOf: [] は「制約なし」で無害なので通す
  assert.deepEqual(validateAgainstSchema("x", { allOf: [] }, ROOT), []);
});

test("長さ制限の検査は文字列長に比例した確保をしない", () => {
  // Array.from(s) だと「64 バイト超で落とす値」の展開に数百 MB 確保する逆転が起きていた
  const huge = "a".repeat(32 * 1024 * 1024);
  const before = process.memoryUsage().heapUsed;
  assert.equal(validateAgainstSchema(huge, { type: "string", maxLength: 10 }, ROOT).length, 1);
  assert.deepEqual(validateAgainstSchema(huge, { type: "string", minLength: 10 }, ROOT), []);
  assert.ok(process.memoryUsage().heapUsed - before < 64 * 1024 * 1024, "長さ検査で大きく確保しない");
  // code point 単位の意味は保つ（絵文字 1 文字 = 長さ 1）
  assert.equal(validateAgainstSchema("😀😀", { type: "string", maxLength: 2 }, ROOT).length, 0);
  assert.equal(validateAgainstSchema("😀😀", { type: "string", maxLength: 1 }, ROOT).length, 1);
  assert.equal(validateAgainstSchema("😀😀", { type: "string", minLength: 3 }, ROOT).length, 1);
});

test("長さ・件数の上下限は非負整数だけ受け付ける", () => {
  // `minLength: -1` は常に真の no-op、`maxItems: 1.5` は 1 として振る舞う
  for (const bad of [{ minLength: -1 }, { maxLength: 1.5 }, { minItems: -1 }, { maxItems: 1.5 }]) {
    assert.throws(() => validateAgainstSchema("x", bad, ROOT), /must be a non-negative integer/);
  }
  // minimum / maximum は負数も小数も正当
  assert.deepEqual(validateAgainstSchema(-0.5, { minimum: -1.5, maximum: 0 }, ROOT), []);
});

/**
 * §10 の上限は 12 個あるが、`findStructuralViolations` が測るのは 4 個だけ。
 * 残りは「addendum の数値を凍結しただけで、まだ誰も検査していない」状態にある。
 *
 * 数値を宣言したまま検査を持たないと、その値は黙って守られなくなる。ここで
 * 「強制済み」と「未強制（追跡先つき）」に分けて全キーを網羅させ、上限を足したのに
 * どちらにも入れなかった場合に落ちるようにする。
 */
test("CONTINUITY_LIMITS のキーは強制済みか、未強制として明示されているかのどちらか", () => {
  // 型でも縛る。StructuralLimits に欄を足したらここが型エラーになる
  const enforced: readonly (keyof StructuralLimits & keyof typeof CONTINUITY_LIMITS)[] = [
    "jsonDepth",
    "stringUtf8Bytes",
    "arrayItems",
    "objectKeys",
  ];
  // schema 側の制約として書かれているもの（構造の walk ではなく JSON Schema が見る）
  const enforcedBySchema = ["rankedCandidates"];
  // 未強制。Task 5 の runtime validator で塞ぐ（#32）
  const deferred = [
    "hintTokens",
    "fullCapsuleTokens",
    "promptMemoryTokens",
    "combinedTokens",
    "absoluteTokens",
    "capsulePayloadBytes",
    "wrapperBytes",
  ];

  const all = [...enforced, ...enforcedBySchema, ...deferred];
  assert.deepEqual([...all].sort(), Object.keys(CONTINUITY_LIMITS).sort());
  assert.equal(new Set(all).size, all.length);

  // 「強制済み」が名ばかりでないことを、上限超過が実際に issue になることで見る
  const tiny = { jsonDepth: 2, stringUtf8Bytes: 4, arrayItems: 1, objectKeys: 1 };
  for (const [value, key] of [
    [{ a: { b: { c: 1 } } }, "jsonDepth"],
    ["abcde", "stringUtf8Bytes"],
    [[1, 2], "arrayItems"],
    [{ a: 1, b: 2 }, "objectKeys"],
  ] as const) {
    const issues = findStructuralViolations(value, tiny);
    assert.ok(
      issues.some((i) => i.message.includes(key)),
      `${key}: ${JSON.stringify(issues)}`,
    );
  }

  // 「未強制」も名ばかりでないことを見る。32KiB を超える capsule が素通りする現状を固定し、
  // #32 を実装したらこの assert が落ちて更新を強制する
  const oversized = { warnings: Array.from({ length: 5 }, () => "x".repeat(8192)) };
  assert.ok(Buffer.byteLength(JSON.stringify(oversized), "utf8") > CONTINUITY_LIMITS.capsulePayloadBytes);
  assert.deepEqual(findStructuralViolations(oversized, CONTINUITY_LIMITS), []);

});

test("schema の type 名の誤記は preflight で落とす（#23）", () => {
  // `strng` はキーワード名としては正しいので、名前と値の種別の検査は通る。
  // 実行時はどの値にも一致しないため、その欄の正当な値をすべて拒否する schema になる
  for (const bad of ["strng", "String", "int", ""]) {
    assert.throws(
      () => validateAgainstSchema("x", { type: bad }, ROOT),
      /invalid type name/,
      bad,
    );
    // 配列形も同じ。1 つでも誤記があれば落とす
    assert.throws(() => validateAgainstSchema("x", { type: ["string", bad] }, ROOT), /invalid type name/, bad);
  }
  // 参照されていない $defs の中でも検査される（踏まないまま凍結されるのを防ぐ）。
  // 実際の使われ方と同じく、文書そのものを schema として渡したときに $defs が歩かれる
  const doc = { $defs: { Unused: { type: "strng" } } } as unknown as JsonSchemaDocument;
  assert.throws(() => validateAgainstSchema(undefined, doc, doc), /invalid type name/);
  // 正当な型名は 7 つとも通る
  for (const ok of ["string", "number", "integer", "boolean", "object", "array", "null"]) {
    assert.doesNotThrow(() => validateAgainstSchema(null, { type: ok }, ROOT), ok);
  }
  assert.doesNotThrow(() => validateAgainstSchema("x", { type: ["string", "null"] }, ROOT));
});

test("rankedCandidates は schema 側で強制されていて、上限は定数と一致する", () => {
  // §10 の「candidates 5」は ResumeSelectionDecisionV1.rankedCandidates の maxItems として
  // 書かれている。リテラルで書いてあるだけだと定数と黙ってずれるので、両者を突き合わせる
  const root = readIJsonFile<JsonSchemaDocument>(new URL("../schema/continuity.schema.json", import.meta.url));
  const defs = (root.$defs ?? {}) as Record<string, Record<string, unknown>>;
  const decision = defs.ResumeSelectionDecisionV1 as { properties: Record<string, { maxItems?: number }> };
  assert.equal(decision.properties.rankedCandidates?.maxItems, CONTINUITY_LIMITS.rankedCandidates);

  // 名ばかりでないことを、上限ちょうどと 1 つ超過で見る
  const candidate = {
    checkpointId: "c1",
    checkpointRevision: "r1",
    taskLineageId: "t1",
    score: 0.5,
    reasonCodes: [],
  };
  const decisionValue = (count: number) => ({
    schemaVersion: 1,
    decisionId: "d1",
    sessionId: "s1",
    boundary: "session_start",
    mode: "smart",
    strategy: "automatic",
    datasetVersion: "v1",
    profileId: "p1",
    capabilityHash: "h1",
    action: "none",
    reasonCodes: [],
    rankedCandidates: Array.from({ length: count }, () => candidate),
    confidenceBand: "none",
    decidedAt: "2026-08-16T00:00:00Z",
  });
  const issuesAt = (count: number) =>
    validateContractValue("ResumeSelectionDecisionV1", decisionValue(count), root, CONTINUITY_LIMITS)
      .map((i) => `${i.path} ${i.message}`)
      .filter((m) => /rankedCandidates/.test(m) && /maxItems|array of/.test(m));
  assert.deepEqual(issuesAt(CONTINUITY_LIMITS.rankedCandidates), []);
  assert.equal(issuesAt(CONTINUITY_LIMITS.rankedCandidates + 1).length > 0, true);
});

test("sequence と watermark は decimal string でなければ通らない（正本 §22.6）", () => {
  // §22.6:「server seq、device seq、epoch は JavaScript safe integer を超えても壊れない
  // decimal string として wire へ出す」。string 型のままだと ordering の権威が任意の文字列になる
  const schema = readIJsonFile<JsonSchemaDocument>(
    new URL("../schema/continuity.schema.json", import.meta.url),
  );
  const event = {
    eventId: "e1",
    canonicalFingerprint: "f1",
    kind: "prompt",
    ingestSeq: "12",
    occurredAt: "2026-08-16T00:00:00Z",
    sessionId: "s1",
    turnIdSource: "unavailable",
    sourceAgent: "codex",
    provenance: { sourceAgentVersion: "1.0.0", evidenceKind: "synthesized", captureMethod: "hook" },
    payload: {},
  };
  assert.deepEqual(
    validateContractValue("NormalizedContinuityEvent", event, schema, CONTINUITY_LIMITS),
    [],
  );
  // safe integer を超える値は通る（decimal string にしている理由そのもの）
  assert.deepEqual(
    validateContractValue(
      "NormalizedContinuityEvent",
      { ...event, ingestSeq: "9007199254740993000" },
      schema,
      CONTINUITY_LIMITS,
    ),
    [],
  );
  for (const bad of ["not-a-decimal", "", "12.5", "-1", "007", "1e3", " 12", "12 "]) {
    const issues = validateContractValue(
      "NormalizedContinuityEvent",
      { ...event, ingestSeq: bad },
      schema,
      CONTINUITY_LIMITS,
    );
    assert.ok(issues.length > 0, `ingestSeq ${JSON.stringify(bad)} が通った`);
  }
});
