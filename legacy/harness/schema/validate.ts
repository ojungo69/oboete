// JSON Schema (draft 2020-12) の必要な部分だけを解釈する検証器。
//
// なぜ ajv を使わないか: harness は依存ゼロで動く前提（`node --experimental-strip-types` だけで
// 実行する）。加えて jsonDepth / objectKeys のような「文書全体の構造上限」は JSON Schema の
// 語彙では表現できず、どのみち独自に歩く必要がある。
//
// 対応キーワード: $ref, type, enum, const, required, properties, additionalProperties,
// items, minItems, maxItems, minLength, maxLength, pattern, minimum, maximum,
// oneOf, anyOf, allOf, if/then/else。これ以外が schema に現れたら「未対応」として
// *エラーにする*（黙って無視すると、書いたつもりの制約が効いていないことに気付けない）。
//
// schema 自体の書き間違い（dangling / 循環 $ref、不正な pattern）は issue ではなく throw する。
// データの不正とは層が違い、握り潰すと「制約が無いのに妥当」と誤認するため。
//
// ponytail: 自前実装の天井は「draft 2020-12 の一部しか解釈しない」こと。対応キーワードを
// 増やし続けるくらいなら ajv へ移す（vendor/codemem の lockfile に既に解決済みで、
// harness/phase1-static-scan.ts が vendor の node_modules を相対 import する前例もある）。
// 移さない理由は、ajv が vendor の推移的依存でしかなく vendor 更新で消え得ること、
// jsonDepth / stringUtf8Bytes などの構造上限は JSON Schema の語彙で表現できずどのみち
// 自前で歩く必要があること、ADR-003 G7 が harness の runtime 非依存を要求していること。

import { isDeepStrictEqual, types } from "node:util";

export interface ValidationIssue {
  path: string;
  message: string;
}

export interface JsonSchemaDocument {
  $defs?: Record<string, unknown>;
  [key: string]: unknown;
}

/** JSON Schema draft 2020-12 の型名。`type` の値はこれ以外を取らない */
const JSON_SCHEMA_TYPES = new Set([
  "string",
  "number",
  "integer",
  "boolean",
  "object",
  "array",
  "null",
]);

// 検証が見る集合は module の中に閉じる。export した Set は同じ process 内の別 module が `add`
// できるので、それを直接見ていると検証が受け付ける keyword を実行時に広げられる（未知の keyword は
// 「対応していない」で落とす設計なので、足された側は素通りする）。`ReadonlySet` は型の上の約束で
// しかなく、harness は型を剥がして走る
const SUPPORTED_KEYWORD_SET = new Set([
  "$ref",
  "$schema",
  "$id",
  "$defs",
  "$comment",
  "title",
  "description",
  "type",
  "enum",
  "const",
  "required",
  "properties",
  "additionalProperties",
  "items",
  "minItems",
  "maxItems",
  "minLength",
  "maxLength",
  "pattern",
  "minimum",
  "maximum",
  "oneOf",
  "anyOf",
  "allOf",
  "if",
  "then",
  "else",
]);

/** 検査の対象一覧。検証が見るのは上の集合なので、ここへ足しても受け付ける keyword は増えない */
export const SUPPORTED_KEYWORDS: readonly string[] = [...SUPPORTED_KEYWORD_SET];

// 対応キーワードが取る値の形。名前が合っていても型が違えば制約は効かないので、
// 名前の集合と同じ場所で押さえる（未掲載のキーワードは形を問わない）
const isNumber = (v: unknown): boolean => typeof v === "number";
// 長さ・件数の上下限は JSON Schema 上「非負整数」。`minLength: -1` は常に真の no-op、
// `maxItems: 1.5` は 1 として振る舞う。どちらもデータに依らない誤記なので弾く
const isCount = (v: unknown): boolean => typeof v === "number" && Number.isInteger(v) && v >= 0;
const isSchemaLike = (v: unknown): boolean => isPlainObject(v) || typeof v === "boolean";
const KEYWORD_VALUE_KIND: Record<string, { name: string; check: (v: unknown) => boolean }> = {
  // 空配列や非文字列の要素も弾く。`type: []` / `enum: []` は何にも一致しない schema で、
  // 放っておくと「expected type , got string」のようにデータ側の誤りとして報告される
  type: {
    name: "a string or non-empty array of strings",
    check: (v) =>
      typeof v === "string" || (Array.isArray(v) && v.length > 0 && v.every((x) => typeof x === "string")),
  },
  enum: { name: "a non-empty array", check: (v) => Array.isArray(v) && v.length > 0 },
  required: { name: "an array of strings", check: (v) => Array.isArray(v) && v.every((x) => typeof x === "string") },
  properties: { name: "an object", check: isPlainObject },
  $defs: { name: "an object", check: isPlainObject },
  additionalProperties: { name: "a schema", check: isSchemaLike },
  items: { name: "a schema", check: isSchemaLike },
  if: { name: "a schema", check: isSchemaLike },
  then: { name: "a schema", check: isSchemaLike },
  else: { name: "a schema", check: isSchemaLike },
  // 空の oneOf / anyOf も同じく常に不一致（allOf は空なら「制約なし」で無害なので素通しでよい）
  oneOf: { name: "a non-empty array of schemas", check: (v) => Array.isArray(v) && v.length > 0 },
  anyOf: { name: "a non-empty array of schemas", check: (v) => Array.isArray(v) && v.length > 0 },
  allOf: { name: "an array of schemas", check: Array.isArray },
  minItems: { name: "a non-negative integer", check: isCount },
  maxItems: { name: "a non-negative integer", check: isCount },
  minLength: { name: "a non-negative integer", check: isCount },
  maxLength: { name: "a non-negative integer", check: isCount },
  minimum: { name: "a number", check: isNumber },
  maximum: { name: "a number", check: isNumber },
  pattern: { name: "a string", check: (v) => typeof v === "string" },
  $ref: { name: "a string", check: (v) => typeof v === "string" },
};

// 値を歩く関数の共通の打ち切り深さ。JSON の実用的な深さより十分大きく、
// スタックオーバーフローよりは十分小さい値
const MAX_WALK_DEPTH = 200;

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/**
 * プロトタイプ経由のキーを実データと取り違えないための own-key 判定。
 * `"constructor" in {}` は true になるため、`in` で書くと required が素通りし、
 * schema.properties の探索が `Object.prototype` の関数を schema として拾ってしまう。
 */
function own(obj: object, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(obj, key);
}

/** `{}` か `Object.create(null)` 由来のものだけを data object とみなす。 */
function isDataObject(v: unknown): v is Record<string, unknown> {
  if (!isPlainObject(v)) return false;
  const proto = Object.getPrototypeOf(v);
  return proto === Object.prototype || proto === null;
}

function typeOf(v: unknown): string {
  if (v === null) return "null";
  if (Array.isArray(v)) return "array";
  if (typeof v === "number") return Number.isInteger(v) ? "integer" : "number";
  return typeof v;
}

function typeMatches(actual: string, expected: string): boolean {
  if (expected === "number") return actual === "number" || actual === "integer";
  return actual === expected;
}

function resolveRef(ref: string, root: JsonSchemaDocument): unknown {
  const m = /^#\/\$defs\/([A-Za-z0-9_.-]+)$/.exec(ref);
  if (!m) throw new Error(`unsupported $ref form: ${ref}`);
  const defs = root.$defs;
  // `#/$defs/__proto__` は `?.[]` だと Object.prototype を拾い、制約ゼロの schema として通ってしまう
  if (!isPlainObject(defs) || !own(defs, m[1])) throw new Error(`dangling $ref: ${ref}`);
  return defs[m[1]];
}

/** JSON として表現できない値（undefined / NaN / Infinity / function / symbol / bigint / 循環）を弾く。 */
export function findNonJsonValues(
  value: unknown,
  path = "$",
  seen = new Set<object>(),
  depth = 1,
): ValidationIssue[] {
  const issues: ValidationIssue[] = [];
  // 上限を超えた時点で打ち切る。構造上限の検査より先に走るため、ここで落ちると
  // jsonDepth のエラーに到達できない（信頼境界が例外で死ぬ）
  if (depth > MAX_WALK_DEPTH) {
    issues.push({ path, message: `nesting deeper than ${MAX_WALK_DEPTH} (rejected before walking)` });
    return issues;
  }
  const t = typeof value;
  if (value === null || t === "string" || t === "boolean") return issues;
  if (t === "number") {
    if (!Number.isFinite(value as number)) issues.push({ path, message: `non-finite number: ${String(value)}` });
    return issues;
  }
  if (t === "undefined" || t === "function" || t === "symbol" || t === "bigint") {
    issues.push({ path, message: `non-JSON value of type ${t}` });
    return issues;
  }
  const obj = value as object;
  if (seen.has(obj)) {
    issues.push({ path, message: "circular reference" });
    return issues;
  }
  seen.add(obj);
  // Proxy は descriptor と実際の読み出しで別の値を返せる。「検証した形」と「保存する形」が
  // ずれる典型なので、中身を見る前に落とす
  if (types.isProxy(value)) {
    issues.push({ path, message: "Proxy is not JSON data" });
    return issues;
  }
  if (Array.isArray(value)) {
    // 添字と length 以外の own key（`a.meta = 1`・symbol）、穴、getter は JSON に出ない。
    // object 側を素の data object に限っているのと同じ理由で、配列もここで拒否する。
    // 走査は own key の数に比例させる（穴を 1 つずつ数えると Array(2**32-1) で停止する）
    let indices = 0;
    for (const key of Reflect.ownKeys(value)) {
      if (key === "length") continue;
      const index = typeof key === "string" ? Number(key) : Number.NaN;
      if (!Number.isInteger(index) || String(index) !== key || index < 0 || index >= value.length) {
        issues.push({ path, message: `array has non-index own key: ${String(key)}` });
        continue;
      }
      indices++;
      const descriptor = Object.getOwnPropertyDescriptor(value, key) as PropertyDescriptor;
      if (!("value" in descriptor)) {
        issues.push({ path: `${path}[${index}]`, message: "array element is a getter/setter" });
        continue;
      }
      issues.push(...findNonJsonValues(descriptor.value, `${path}[${index}]`, seen, depth + 1));
    }
    if (indices !== value.length) {
      issues.push({ path, message: `array has ${value.length - indices} holes (length ${value.length})` });
    }
  } else if (isDataObject(value)) {
    // getter は呼ばずに descriptor の値を読む。symbol キーと非 enumerable な property は
    // `Object.entries` が飛ばすので、`Reflect.ownKeys` と突き合わせて明示的に落とす
    for (const key of Reflect.ownKeys(value)) {
      const descriptor = Object.getOwnPropertyDescriptor(value, key) as PropertyDescriptor;
      if (typeof key === "symbol") {
        issues.push({ path, message: `object has symbol key: ${String(key)}` });
        continue;
      }
      if (!descriptor.enumerable) {
        issues.push({ path: `${path}.${key}`, message: "non-enumerable property is not JSON data" });
        continue;
      }
      if (!("value" in descriptor)) {
        issues.push({ path: `${path}.${key}`, message: "property is a getter/setter" });
        continue;
      }
      issues.push(...findNonJsonValues(descriptor.value, `${path}.${key}`, seen, depth + 1));
    }
  } else {
    // Date / Map / class instance など。toJSON で通ってしまう型を素通しすると
    // 「保存した形」と「検証した形」がずれるため、素の data object 以外は拒否する
    issues.push({ path, message: `unsupported object type: ${Object.prototype.toString.call(value)}` });
  }
  seen.delete(obj);
  return issues;
}

export interface StructuralLimits {
  jsonDepth: number;
  stringUtf8Bytes: number;
  arrayItems: number;
  objectKeys: number;
}

/** 文書全体の構造上限。JSON Schema では表現できないので値を直接歩く。 */
export function findStructuralViolations(
  value: unknown,
  limits: StructuralLimits,
  path = "$",
  depth = 1,
): ValidationIssue[] {
  const issues: ValidationIssue[] = [];
  if (depth > MAX_WALK_DEPTH) {
    issues.push({ path, message: `nesting deeper than ${MAX_WALK_DEPTH} (rejected before walking)` });
    return issues;
  }
  if (depth > limits.jsonDepth) {
    issues.push({ path, message: `depth ${depth} exceeds jsonDepth ${limits.jsonDepth}` });
    return issues;
  }
  if (typeof value === "string") {
    const bytes = Buffer.byteLength(value, "utf8");
    if (bytes > limits.stringUtf8Bytes) {
      issues.push({ path, message: `string ${bytes}B exceeds stringUtf8Bytes ${limits.stringUtf8Bytes}` });
    }
    return issues;
  }
  if (Array.isArray(value)) {
    if (value.length > limits.arrayItems) {
      issues.push({ path, message: `array of ${value.length} exceeds arrayItems ${limits.arrayItems}` });
    }
    for (let i = 0; i < value.length; i++) {
      issues.push(...findStructuralViolations(value[i], limits, `${path}[${i}]`, depth + 1));
    }
    return issues;
  }
  if (isPlainObject(value)) {
    const keys = Object.keys(value);
    if (keys.length > limits.objectKeys) {
      issues.push({ path, message: `object with ${keys.length} keys exceeds objectKeys ${limits.objectKeys}` });
    }
    for (const k of keys) {
      // JSON の property name も文字列。値だけ測ると、長大なキーが 1 本だけの object が
      // stringUtf8Bytes を素通りする（キー数も 1 なので objectKeys にも掛からない）
      const keyBytes = Buffer.byteLength(k, "utf8");
      if (keyBytes > limits.stringUtf8Bytes) {
        issues.push({
          path: `${path}.<key ${keyBytes}B>`,
          message: `key ${keyBytes}B exceeds stringUtf8Bytes ${limits.stringUtf8Bytes}`,
        });
      }
      issues.push(...findStructuralViolations(value[k], limits, `${path}.${k}`, depth + 1));
    }
  }
  return issues;
}

/**
 * schema 文書そのものの誤りを、データとは独立に 1 度だけ検査する。
 *
 * データを歩きながら未対応キーワードを issue として積むと、anyOf / oneOf の分岐の中では
 * 「その分岐に一致しなかった」としか扱われず、別の分岐が一致した時点で丸ごと消える。
 * つまり `anyOf: [{properties:{x:{format:"date-time"}}}, {type:"object"}]` は黙って
 * 何でも通す schema になる。schema の誤りはデータに依らない欠陥なので throw する。
 */
function assertSchemaSupported(
  schema: unknown,
  root: JsonSchemaDocument,
  path: string,
  seenRefs: Set<string>,
): void {
  if (typeof schema === "boolean") return; // boolean schema は draft 2020-12 で合法
  if (!isPlainObject(schema)) {
    // 「データ側で報告する」では駄目。`anyOf: [42, {type:"string"}]` の 42 は分岐が
    // 一致した時点で issue ごと消え、参照されない $defs のスカラーに至っては誰も見ない
    throw new Error(`schema at ${path} must be an object or boolean, got ${typeof schema}`);
  }
  for (const key of Object.keys(schema)) {
    if (!SUPPORTED_KEYWORD_SET.has(key)) {
      throw new Error(`unsupported schema keyword at ${path}: ${key}`);
    }
    // 名前だけ合っていて型が違うキーワードは、制約が黙って無効化される
    // （`minLength: "3"` は数値比較に載らず素通り、`required: "kind"` は文字列を 1 文字ずつ
    //  必須プロパティ名として読む）。名前の誤記と同じくデータに依らない欠陥なので throw する
    const expected = KEYWORD_VALUE_KIND[key];
    if (expected && !expected.check(schema[key])) {
      throw new Error(`schema keyword ${key} at ${path} must be ${expected.name}`);
    }
  }
  // `type` の値は名前そのものが制約になる。`"strng"` はキーワード名としては正しいので
  // 上のループを通り、実行時はどの値にも一致しない = その欄の正当な値をすべて拒否する
  // schema になる。使われていない `$defs` なら誰も踏まないまま凍結される
  if (schema.type !== undefined) {
    const names = Array.isArray(schema.type) ? schema.type : [schema.type];
    for (const name of names) {
      if (typeof name !== "string" || !JSON_SCHEMA_TYPES.has(name)) {
        throw new Error(`invalid type name at ${path}: ${JSON.stringify(name)}`);
      }
    }
  }
  // pattern は値が文字列のときしか評価されない。`{type:"number", pattern:"["}` のような
  // 壊れた schema が「その値が文字列ではなかった」というだけで素通りするのを防ぐ
  if (typeof schema.pattern === "string") {
    try {
      new RegExp(schema.pattern);
    } catch {
      throw new Error(`invalid pattern at ${path}: ${schema.pattern}`);
    }
  }
  // 同じ $ref を 2 度歩かない。循環 schema でも preflight が止まるようにするための打ち切りで、
  // 循環そのものの検出は validateNode 側（refStack）が受け持つ
  if (typeof schema.$ref === "string" && !seenRefs.has(schema.$ref)) {
    seenRefs.add(schema.$ref);
    assertSchemaSupported(resolveRef(schema.$ref, root), root, schema.$ref, seenRefs);
  }
  // $defs も歩く。$ref から辿れる定義しか見ないと、参照が外れた定義の誤記だけが
  // 検査を免れ、「どの契約を検証したか」で schema の正しさが変わってしまう
  for (const key of ["properties", "$defs"] as const) {
    const map = schema[key];
    if (!isPlainObject(map)) continue;
    for (const [k, sub] of Object.entries(map)) {
      assertSchemaSupported(sub, root, `${path}.${key}.${k}`, seenRefs);
    }
  }
  for (const key of ["items", "additionalProperties", "if", "then", "else"] as const) {
    if (schema[key] !== undefined) assertSchemaSupported(schema[key], root, `${path}.${key}`, seenRefs);
  }
  for (const key of ["oneOf", "anyOf", "allOf"] as const) {
    const branches = schema[key];
    if (!Array.isArray(branches)) continue;
    branches.forEach((sub, i) => assertSchemaSupported(sub, root, `${path}.${key}[${i}]`, seenRefs));
  }
}

/**
 * JSON Schema の文字列長は Unicode code point 数。String#length は UTF-16 code unit 数で絵文字を 2 と数える。
 * bound を超えた時点で打ち切るので返り値は min(実際の長さ, bound + 1)。長さの上限を検査するために
 * 長さに比例した確保をしない（信頼境界では findStructuralViolations のバイト数上限より前に走るため、
 * Array.from だと「64 バイト超で落とす値」の展開に 32MB 確保する、という逆転が起きる）
 */
const codePointLengthUpTo = (s: string, bound: number): number => {
  let n = 0;
  for (const _ of s) {
    n += 1;
    if (n > bound) break;
  }
  return n;
};

// JSON 上 -0 と 0 は同じ数（JSON.stringify(-0) === "0"）だが JSON.parse("-0") は -0 を返し、
// isDeepStrictEqual は別物と見る。放置すると `expected const 0, got 0` という読めない issue になる。
// ponytail: const object の奥に埋もれた -0 までは畳まない（`{a: -0}` vs `const {a: 0}` は今も
// 不一致で、読めない issue のまま）。契約に現れたら両辺を JSON round-trip して深く正規化する
const jsonEqual = (a: unknown, b: unknown): boolean =>
  isDeepStrictEqual(Object.is(a, -0) ? 0 : a, Object.is(b, -0) ? 0 : b);

// 一度歩いた schema object は覚えておく。schema は repo 内の静的 JSON で、同じ object を
// 値ごとに歩き直しても結論は変わらない。root ごとに覚えるのは、$ref の解決先が root 依存のため。
// 前提: 検査後に schema object を書き換えないこと（object 同一性で覚えるので、後から
// キーを足しても再検査されない）。この repo の schema は JSON.parse したまま凍結して使う
const schemaChecked = new WeakMap<object, JsonSchemaDocument>();

/** 同じ schema object を root ごとに一度だけ歩く */
function checkSchemaOnce(schema: unknown, root: JsonSchemaDocument, path: string): void {
  if (isPlainObject(schema) && schemaChecked.get(schema) === root) return;
  assertSchemaSupported(schema, root, path, new Set());
  if (isPlainObject(schema)) schemaChecked.set(schema, root);
}

/** schema 側を先に検査してから値を検査する。外から呼ぶのはこちら。 */
export function validateAgainstSchema(
  value: unknown,
  schema: unknown,
  root: JsonSchemaDocument,
  path = "$",
): ValidationIssue[] {
  checkSchemaOnce(schema, root, path);
  return validateNode(value, schema, root, path, [], 0);
}

function validateNode(
  value: unknown,
  schema: unknown,
  root: JsonSchemaDocument,
  path: string,
  refStack: readonly string[],
  depth: number,
): ValidationIssue[] {
  // 再帰 schema（JsonValue など）では refStack が子降下で畳まれるため、値が深いだけで
  // 際限なく降りられる。RangeError で落ちる前に打ち切る。
  // issue ではなく throw にするのは、anyOf / oneOf の中では issue が「その分岐に一致しなかった」
  // に化けて原因が消えるため（circular $ref と同じ理由）。信頼境界の validateContractValue は
  // findNonJsonValues が同じ深さで先に弾いて早期 return するので、ここに深い値は到達しない
  if (depth > MAX_WALK_DEPTH) {
    throw new Error(`value nesting deeper than ${MAX_WALK_DEPTH} at ${path}`);
  }
  if (schema === true) return [];
  if (schema === false) return [{ path, message: "schema is false (nothing is valid here)" }];
  if (!isPlainObject(schema)) return [{ path, message: "schema must be an object or boolean" }];

  const issues: ValidationIssue[] = [];

  // draft 2020-12 では $ref に兄弟キーワードを併記できる（`{$ref, maxLength}` など）。
  // ここで return すると併記した制約が黙って落ちるので、参照先を検査してから残りも続ける
  if (typeof schema.$ref === "string") {
    // 同じ値に対して同じ $ref をもう一度たどったら、データを消費しない自己参照
    // （`Loop: {$ref: "#/$defs/Loop"}`）でスタックを食い潰す。診断可能なエラーにする。
    // CHILD_REF_STACK: properties / items / additionalProperties で子の値へ降りるときは
    // refStack を空に畳む。JsonValue のような再帰 schema は 1 段ごとに値を消費するので
    // 必ず終端し、ここで弾くべき「値が進まない循環」とは別物
    if (refStack.includes(schema.$ref)) {
      throw new Error(`circular $ref: ${[...refStack, schema.$ref].join(" -> ")}`);
    }
    issues.push(
      ...validateNode(value, resolveRef(schema.$ref, root), root, path, [...refStack, schema.$ref], depth),
    );
  }

  const actual = typeOf(value);

  if (schema.type !== undefined) {
    const expected = Array.isArray(schema.type) ? (schema.type as string[]) : [schema.type as string];
    if (!expected.some((t) => typeMatches(actual, t))) {
      issues.push({ path, message: `expected type ${expected.join("|")}, got ${actual}` });
      return issues; // 型が違う時点で以降の制約は意味を成さない
    }
  }

  if (Array.isArray(schema.enum) && !schema.enum.some((e) => jsonEqual(e, value))) {
    // 棄却した値そのものは出さない。診断は stderr から CI ログへ流れるので、
    // ここで echo すると fixture の中身を外へ出す経路になる（隣の検査が避けているのと同じ理由）
    issues.push({ path, message: "value not in enum" });
  }
  if (own(schema, "const") && !jsonEqual(schema.const, value)) {
    issues.push({ path, message: `expected const ${JSON.stringify(schema.const)}` });
  }

  if (typeof value === "string") {
    // ponytail: pattern は毎回コンパイルし、backtracking にも上限が無い。schema は repo 内の
    // 凍結された JSON でレビューを通るため今は許容している。外部由来の schema を受けるなら
    // pattern の事前検査（安全な部分集合への制限）か worker での実行が要る
    if (
      typeof schema.minLength === "number" &&
      codePointLengthUpTo(value, schema.minLength) < schema.minLength
    ) {
      issues.push({ path, message: `string shorter than minLength ${schema.minLength}` });
    }
    if (
      typeof schema.maxLength === "number" &&
      codePointLengthUpTo(value, schema.maxLength) > schema.maxLength
    ) {
      issues.push({ path, message: `string longer than maxLength ${schema.maxLength}` });
    }
    if (typeof schema.pattern === "string" && !new RegExp(schema.pattern).test(value)) {
      issues.push({ path, message: `string does not match pattern ${schema.pattern}` });
    }
  }

  if (typeof value === "number") {
    // NaN は < も > も false になるため、先に弾かないと minimum/maximum を素通りする
    if (!Number.isFinite(value)) {
      issues.push({ path, message: `non-finite number: ${String(value)}` });
    }
    if (typeof schema.minimum === "number" && value < schema.minimum) {
      issues.push({ path, message: `number below minimum ${schema.minimum}` });
    }
    if (typeof schema.maximum === "number" && value > schema.maximum) {
      issues.push({ path, message: `number above maximum ${schema.maximum}` });
    }
  }

  if (Array.isArray(value)) {
    if (typeof schema.minItems === "number" && value.length < schema.minItems) {
      issues.push({ path, message: `array shorter than minItems ${schema.minItems}` });
    }
    if (typeof schema.maxItems === "number" && value.length > schema.maxItems) {
      issues.push({ path, message: `array longer than maxItems ${schema.maxItems}` });
    }
    if (schema.items !== undefined) {
      // 子の値へ降りるので refStack は畳む。詳細は CHILD_REF_STACK の注記
      value.forEach((item, i) => issues.push(...validateNode(item, schema.items, root, `${path}[${i}]`, [], depth + 1)));
    }
  }

  if (isPlainObject(value)) {
    const props = isPlainObject(schema.properties) ? schema.properties : {};
    for (const req of (schema.required as string[] | undefined) ?? []) {
      if (!own(value, req)) issues.push({ path, message: `missing required property: ${req}` });
    }
    // key 名そのものが fixture の中身なので診断へ載せない。綴りを検査して「安全な形なら
    // 出す」でも足りない: 英数字だけの key 名に秘密を入れれば素通りする。場所は親 path と
    // その object の中での順番だけで示す
    let ordinal = 0;
    for (const [k, v] of Object.entries(value)) {
      ordinal += 1;
      if (own(props, k)) {
        issues.push(...validateNode(v, props[k], root, `${path}.${k}`, [], depth + 1));
      } else if (schema.additionalProperties === false) {
        issues.push({ path, message: `unknown property #${ordinal}` });
      } else if (schema.additionalProperties !== undefined) {
        issues.push(...validateNode(v, schema.additionalProperties, root, `${path}.${k}`, [], depth + 1));
      }
    }
  }

  if (Array.isArray(schema.allOf)) {
    for (const sub of schema.allOf) issues.push(...validateNode(value, sub, root, path, refStack, depth));
  }
  if (Array.isArray(schema.anyOf)) {
    const ok = schema.anyOf.some((sub) => validateNode(value, sub, root, path, refStack, depth).length === 0);
    if (!ok) issues.push({ path, message: "value matches none of anyOf" });
  }
  if (Array.isArray(schema.oneOf)) {
    // 2 件一致した時点で結果は確定する。variant の多い判別共用体で残りを回さない
    let matched = 0;
    for (const sub of schema.oneOf) {
      if (validateNode(value, sub, root, path, refStack, depth).length === 0 && ++matched === 2) break;
    }
    if (matched !== 1) {
      const got = matched === 2 ? "2 or more" : String(matched);
      issues.push({ path, message: `expected exactly 1 oneOf match, got ${got}` });
    }
  }

  // if/then/else: capability.schema.json §7.2（synthesized なら sourceEvents 必須）が使う。
  // `if` は「合致するか」の判定にだけ使い、その issue 自体は結果に混ぜない
  if (schema.if !== undefined) {
    const branch =
      validateNode(value, schema.if, root, path, refStack, depth).length === 0 ? schema.then : schema.else;
    if (branch !== undefined) {
      issues.push(...validateNode(value, branch, root, path, refStack, depth));
    }
  }

  return issues;
}

/** `$defs` の 1 つを名前で選び、JSON 妥当性・schema・構造上限をこの順で検査する。 */
export function validateContractValue(
  defName: string,
  value: unknown,
  root: JsonSchemaDocument,
  limits: StructuralLimits,
): ValidationIssue[] {
  // root ごと歩く。$defs のうち $ref から辿れるものしか見ないと、参照が外れた定義の誤記だけが
  // 検査を免れる。ここは値ごとに呼ばれるが memo が効くので root あたり 1 回で済む
  checkSchemaOnce(root, root, "$");
  const nonJson = findNonJsonValues(value);
  if (nonJson.length > 0) return nonJson;
  const defs = root.$defs;
  if (!isPlainObject(defs) || !own(defs, defName)) {
    return [{ path: "$", message: `unknown $defs entry: ${defName}` }];
  }
  const def = defs[defName];
  return [
    ...validateAgainstSchema(value, def, root, "$"),
    ...findStructuralViolations(value, limits),
  ];
}
