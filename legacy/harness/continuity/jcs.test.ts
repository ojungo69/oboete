import { test } from "node:test";
import assert from "node:assert/strict";
import { canonicalizeJson, decodeUtf8, parseIJson } from "../schema/jcs.ts";

/**
 * JCS そのものを実装している以上、「JCS になっている」ことを見る test が要る。
 * RFC 8785 の本文と付録から、この harness が扱う部分集合に効く性質を取っている。
 */

test("object のキーは UTF-16 code unit の昇順で並ぶ（RFC 8785 §3.2.3）", () => {
  assert.equal(canonicalizeJson({ b: 1, a: 2 }), '{"a":2,"b":1}');
  // 大文字は小文字より前（ASCII の順序であって辞書順ではない）
  assert.equal(canonicalizeJson({ a: 1, B: 2 }), '{"B":2,"a":1}');
  // 非 ASCII も code unit 順。a(0x61) < é(0xE9) < ċ(0x10B) < €(0x20AC)
  assert.equal(
    canonicalizeJson({ "€": 1, "é": 2, a: 3, "ċ": 4 }),
    '{"a":3,"é":2,"ċ":4,"€":1}',
  );
  // ここが JCS の肝。**code point 順ではなく UTF-16 code unit 順**なので、
  // 代理対（U+10384 = 0xD800,0xDF84）は U+FB33 より前に来る。code point 順に
  // 並べる実装とはここで bytes が食い違う（RFC 8785 §3.2.3）
  assert.equal(
    canonicalizeJson({ "\u{10384}": 1, "\uFB33": 2 }),
    '{"\u{10384}":1,"\uFB33":2}',
  );
  // 入れ子も同じ規則
  assert.equal(canonicalizeJson({ z: { y: 1, x: 2 } }), '{"z":{"x":2,"y":1}}');
});

test("配列の順序は保持する（並べ替えは値の側の責任）", () => {
  assert.equal(canonicalizeJson([3, 1, 2]), "[3,1,2]");
  assert.equal(canonicalizeJson({ a: [{ b: 1, a: 2 }] }), '{"a":[{"a":2,"b":1}]}');
});

test("空白を入れず、文字列と数値は ECMAScript の表記に従う", () => {
  assert.equal(canonicalizeJson({ a: 1, b: [1, 2] }), '{"a":1,"b":[1,2]}');
  assert.equal(canonicalizeJson("a\nb"), '"a\\nb"');
  assert.equal(canonicalizeJson("é"), '"é"'); // 非 ASCII はエスケープしない
  assert.equal(canonicalizeJson(1e21), "1e+21");
  assert.equal(canonicalizeJson(0.1), "0.1");
  assert.equal(canonicalizeJson(-0), "0"); // JCS は -0 を 0 として出す
  assert.equal(canonicalizeJson(null), "null");
  assert.equal(canonicalizeJson(true), "true");
});

test("JSON に無い値は受け付けない", () => {
  for (const bad of [Number.NaN, Number.POSITIVE_INFINITY, -Number.POSITIVE_INFINITY]) {
    assert.throws(() => canonicalizeJson(bad), /非有限/);
  }
  assert.throws(() => canonicalizeJson(() => 1), /JSON に無い型/);
  assert.throws(() => canonicalizeJson(1n), /JSON に無い型/);
  // undefined の property は落とさない。落とすと `{ a: 1 }` と同じ hash になり、
  // 組み立て損ねた契約値が「その欄は元々無かった」ものとして通る
  assert.equal(JSON.stringify({ a: 1, b: undefined }), '{"a":1}');
  assert.throws(() => canonicalizeJson({ a: 1, b: undefined }), /JSON に無い型/);
});

test("対になっていない代理は拒否する（RFC 8785 §3.2.2.2）", () => {
  // `JSON.stringify` は well-formed 化してエスケープを返すだけなので、そのままだと
  // 「TS では hash が出るが、準拠した実装は計算を拒否する」状態になる
  assert.equal(JSON.stringify("\ud800"), '"\\ud800"');
  for (const bad of ["\ud800", "a\udfff", "\ud800a", "\udc00\ud800"]) {
    assert.throws(() => canonicalizeJson(bad), /代理/, JSON.stringify(bad));
    assert.throws(() => canonicalizeJson({ [bad]: 1 }), /代理/, `key ${JSON.stringify(bad)}`);
    assert.throws(() => canonicalizeJson({ a: [bad] }), /代理/, `nested ${JSON.stringify(bad)}`);
  }
  // 正しい代理対は通る
  assert.equal(canonicalizeJson("\u{10384}"), '"\u{10384}"');
  assert.equal(canonicalizeJson({ "\u{10384}": 1 }), '{"\u{10384}":1}');
});

test("キー順が違うだけの object は同じ bytes になる", () => {
  const a = { x: 1, y: { p: 1, q: 2 }, z: [1, 2] };
  const b = { z: [1, 2], y: { q: 2, p: 1 }, x: 1 };
  assert.equal(canonicalizeJson(a), canonicalizeJson(b));
  // 配列の順序が違えば別物
  assert.notEqual(canonicalizeJson({ z: [1, 2] }), canonicalizeJson({ z: [2, 1] }));
});

test("重複した property 名を持つ JSON は読まない（RFC 8785 §3.1 / RFC 7493 §2.3）", () => {
  // `JSON.parse` は後勝ちで潰すだけ。潰れた値は canonicalize できてしまう
  assert.deepEqual(JSON.parse('{"a":1,"a":2}'), { a: 2 });

  for (const bad of [
    '{"a":1,"a":2}',
    '{"a":{"k":1,"k":2}}',
    '{"a":[{"k":1,"k":2}]}',
    '{"a":[1,{"b":[{"k":{},"k":[]}]}]}',
    '{"\u0061b":1,"ab":2}', // エスケープ違いでも同じ property 名
  ]) {
    assert.throws(() => parseIJson(bad), /property 名が重複/, bad);
  }
});

test("重複していない JSON はそのまま読める", () => {
  assert.deepEqual(parseIJson('{"a":1,"b":2}'), { a: 1, b: 2 });
  // 別々の object に同じ名前が出るのは重複ではない
  assert.deepEqual(parseIJson('{"a":{"k":1},"b":{"k":2}}'), { a: { k: 1 }, b: { k: 2 } });
  assert.deepEqual(parseIJson('[{"k":1},{"k":2}]'), [{ k: 1 }, { k: 2 }]);
  // 値の側の文字列は property 名ではない。`"` や `{` を含んでも誤検出しない
  assert.deepEqual(parseIJson('{"a":"\\"k\\":1,\\"k\\":2","k":3}'), { a: '"k":1,"k":2', k: 3 });
  assert.deepEqual(parseIJson('{"a":"}{[,","b":"\\\\"}'), { a: "}{[,", b: "\\" });
  // 整形されていても位置を見失わない
  assert.deepEqual(parseIJson('{\n  "a" : 1,\n  "b" : [ 1, 2 ]\n}'), { a: 1, b: [1, 2] });
  assert.throws(() => parseIJson('{\n  "a" : 1,\n  "a" : 2\n}'), /property 名が重複/);
});

test("疎な配列は canonicalize しない（穴は JSON に無い）", () => {
  // `map` は穴を飛ばすので `[,]` のような JSON でない bytes が出る。`JSON.stringify` は
  // 穴を null にするが、JCS の入力として渡された時点で値が確定していないので落とす
  assert.equal(JSON.stringify(Array(2)), "[null,null]");
  assert.throws(() => canonicalizeJson(Array(2)), /穴のある配列/);
  assert.throws(() => canonicalizeJson([1, , 3]), /穴のある配列/);
  assert.throws(() => canonicalizeJson({ a: [1, , 3] }), /穴のある配列/);
  // 穴でない undefined は配列でも同じ扱い
  assert.throws(() => canonicalizeJson([undefined]), /JSON に無い型/);
});

test("添字以外の property を持つ配列は canonicalize しない", () => {
  const tagged: unknown[] & { metadata?: string } = [1];
  tagged.metadata = "lost";
  assert.equal(JSON.stringify(tagged), "[1]"); // 付けた欄は消える
  assert.throws(() => canonicalizeJson(tagged), /length 以外の own key/);
  const symbolTagged = [1];
  (symbolTagged as unknown as Record<symbol, number>)[Symbol("s")] = 1;
  assert.throws(() => canonicalizeJson(symbolTagged), /length 以外の own key/);

  // 件数だけ数えると、穴が空けた枠に別の key が収まって素通りする。しかも `Array.from` は
  // 差し替えられた `Symbol.iterator` を呼ぶので、実在しない要素の bytes まで出せる
  const forged = Array(1);
  (forged as unknown as Record<symbol, unknown>)[Symbol.iterator] = function* () {
    yield 7;
  };
  assert.throws(() => canonicalizeJson(forged), /length 以外の own key/);

  // prototype 側の添字で穴を埋めた配列も落とす（own property でない値は JSON に出ない）
  const proto: unknown[] = Object.create(Array.prototype) as unknown[];
  proto[0] = "inherited";
  assert.throws(
    () => canonicalizeJson(Object.setPrototypeOf(Array(1), proto) as unknown[]),
    /穴のある配列/,
  );

  // 穴は own key の数で判定する（添字を 1 つずつ見ると Array(2**32-1) で止まらない）
  assert.throws(() => canonicalizeJson(Array(2 ** 32 - 1)), /穴のある配列/);

  assert.equal(canonicalizeJson([1, 2]), "[1,2]");
});

test("Proxy は canonicalize しない（読むたびに値を変えられる）", () => {
  let reads = 0;
  const proxy = new Proxy({ a: 0 }, { get: (t, k, r) => (k === "a" ? ++reads : Reflect.get(t, k, r)) });
  // descriptor は target のものが見えるので、getter 検査だけでは見抜けない
  assert.equal("value" in (Object.getOwnPropertyDescriptor(proxy, "a") as PropertyDescriptor), true);
  assert.throws(() => canonicalizeJson(proxy), /Proxy/);
  assert.throws(() => canonicalizeJson(new Proxy([1], {})), /Proxy/);
  assert.equal(reads, 0);
});

test("I-JSON は代理と noncharacter を文字列に許さない（RFC 7493 §2.1）", () => {
  // `JSON.parse` はどちらも通す
  assert.deepEqual(JSON.parse('{"a":"\uDEAD"}'), { a: "\uDEAD" });

  for (const bad of [
    '{"a":"\uD800"}',
    '{"a":["\uDEAD"]}',
    '{"\uDEAD":1}',
    '["a\uDC00"]',
  ]) {
    assert.throws(() => parseIJson(bad), /対になっていない代理/, bad);
  }

  // noncharacter も I-JSON では不正（面ごとの FFFE/FFFF と FDD0-FDEF）
  for (const bad of [
    '{"a":"\uFFFE"}',
    '{"a":"\uFFFF"}',
    '{"a":"\uFDD0"}',
    '{"\uFFFE":1}',
    '{"a":"\uDBFF\uDFFF"}', // U+10FFFF（代理対としては正しいが noncharacter）
  ]) {
    assert.throws(() => parseIJson(bad), /noncharacter/, bad);
  }

  // 正しい代理対は通る（RFC 7493 §2.1 が legal と書いている例そのもの）
  assert.deepEqual(parseIJson('{"a":"\uD800\uDEAD"}'), { a: "\uD800\uDEAD" });
});

test("binary64 で表せない数を含む JSON は読まない（RFC 7493 §2.2）", () => {
  // `JSON.parse` は範囲外の数を Infinity にする。そこから先は比較も hash も意味を失う
  assert.equal(JSON.parse('{"a":1e400}').a, Number.POSITIVE_INFINITY);

  for (const bad of ['{"a":1e400}', '{"a":-1e400}', '{"a":[1e400]}', '{"a":{"b":1e400}}']) {
    assert.throws(() => parseIJson(bad), /binary64 で表せない/, bad);
  }
  // 丸められる整数リテラルも拒否する。`JSON.parse` が黙って値を変えるので、ファイルの文字列と
  // 読んだ値がずれる（正本 §22.6 はこの大きさの値を decimal string で書けと定めている）
  assert.equal(JSON.parse('{"a":9007199254740993}').a, 9007199254740992);
  for (const bad of [
    '{"a":9007199254740993}',
    '{"a":-9007199254740993}',
    '{"a":[123456789012345678901234567890]}',
  ]) {
    assert.throws(() => parseIJson(bad), /正規形でない/, bad);
  }
  // 小数・指数で書いた同じ値も落とす（整数の綴りだけ見ると素通りする）
  for (const bad of ['{"a":9007199254740993.0}', '{"a":9.007199254740993e15}', '{"a":1.0000000000000001}']) {
    assert.throws(() => parseIJson(bad), /正規形でない/, bad);
  }
  // 指数だけが極端なゼロ。JSON として妥当で値は 0 なので、落とさず・溢れさせずに通す
  assert.deepEqual(parseIJson('{"a":0e9007199254740993,"b":0e-9007199254740993,"c":-0e5,"d":0.0e999999999}'), {
    a: 0,
    b: 0,
    c: -0,
    d: 0,
  });
  // 0 に丸められる非ゼロの綴りは通さない（`0` と書くべきで、書かれた値とは違う）
  assert.throws(() => parseIJson('{"a":1e-9007199254740993}'), /正規形でない/);

  // 同じ double を指す別の綴り（厳密な 10 進展開）も、正規形でないので通さない。
  // 値としては 0.1 と同じだが bytes が違い、JCS の canonical 表記とも食い違う
  assert.throws(
    () => parseIJson('{"a":0.1000000000000000055511151231257827021181583404541015625}'),
    /正規形でない/,
  );
  // 2 進で正確に表せない値でも、最短表記が同じ値に戻るなら綴りは保たれている。
  // 桁数で切ると `3.141592653589793` のような正当な double まで落ちる
  assert.deepEqual(
    parseIJson('{"a":1.10,"b":1.5e300,"c":0.1,"d":3.141592653589793,"e":2.2250738585072014e-308,"f":0.30000000000000004,"g":5e-324}'),
    {
      a: 1.1,
      b: 1.5e300,
      c: 0.1,
      d: 3.141592653589793,
      e: 2.2250738585072014e-308,
      f: 0.30000000000000004,
      g: 5e-324,
    },
  );

  // 見るのは**整数として書かれた綴り**だけ。`1e21` は 2**53 より大きいが正確に表せるので通す
  assert.deepEqual(parseIJson('{"a":1e21}'), { a: 1e21 });
  assert.deepEqual(parseIJson('{"a":9007199254740992}'), { a: 9007199254740992 });
  assert.deepEqual(parseIJson('{"a":"9007199254740993"}'), { a: "9007199254740993" }); // 文字列は対象外
  // 正確に表せる範囲はそのまま通る
  assert.deepEqual(parseIJson('{"a":9007199254740991,"b":-0,"c":0.1,"d":-12,"e":1.0e2}'), {
    a: 9007199254740991,
    b: -0,
    c: 0.1,
    d: -12,
    e: 100,
  });
});

test("素の object でない値は canonicalize しない", () => {
  // enumerable な own property が無いので、通すと値と無関係な `{}` の bytes が出る
  assert.equal(JSON.stringify(new Map([["x", 1]])), "{}");
  for (const bad of [new Map([["x", 1]]), new Set([1]), new Date(0), Object(1), Object("a")]) {
    assert.throws(() => canonicalizeJson(bad), /素の object でない/, String(bad));
  }
  class Capsule {
    id = "x";
  }
  assert.throws(() => canonicalizeJson(new Capsule()), /できない: Capsule/);
  assert.throws(() => canonicalizeJson({ a: new Date(0) }), /素の object でない/);
  // `Object.entries` が飛ばす own property を持つ object も落とす（`{}` として hash されるため）
  assert.equal(JSON.stringify(Object.defineProperty({}, "x", { value: 1 })), "{}");
  assert.throws(
    () => canonicalizeJson(Object.defineProperty({}, "x", { value: 1 })),
    /非 enumerable/,
  );
  assert.throws(() => canonicalizeJson({ [Symbol("s")]: 1 }), /symbol キー/);

  // `{}` と `Object.create(null)` 由来は通る（JSON.parse が返すのはこの 2 つ）
  assert.equal(canonicalizeJson({ a: 1 }), '{"a":1}');
  assert.equal(canonicalizeJson(Object.assign(Object.create(null), { a: 1 })), '{"a":1}');
});

test("UTF-8 として不正な bytes は置換せずに落とす（RFC 7493 §2.1）", () => {
  // `Buffer.toString("utf8")` / TextDecoder の既定は不正 byte を U+FFFD に置換する。
  // 置換された値から hash を出すと、元の bytes を拒否する実装と食い違う
  const broken = new Uint8Array([0x7b, 0x22, 0x61, 0x22, 0x3a, 0x22, 0xff, 0x22, 0x7d]); // {"a":"\xff"}
  assert.equal(Buffer.from(broken).toString("utf8"), '{"a":"\ufffd"}');
  assert.throws(() => decodeUtf8(broken, "t.json"), /UTF-8 として不正/);
  // BOM は既定の TextDecoder が黙って剥がす。剥がさずに落とす（同じ bytes を読む他の実装は
  // JSON として拒否するので、こちらだけ通ると契約ファイルが「読める」ことにならない）
  const withBom = new Uint8Array([0xef, 0xbb, 0xbf, 0x7b, 0x7d]); // BOM + {}
  assert.equal(new TextDecoder("utf-8").decode(withBom), "{}");
  assert.throws(() => decodeUtf8(withBom, "t.json"), /BOM/);
  // 正しい UTF-8 はそのまま
  assert.equal(decodeUtf8(new TextEncoder().encode('{"a":"é"}'), "t.json"), '{"a":"é"}');
});

test("canonicalize も I-JSON の文字列だけを受ける（parseIJson と同じ範囲）", () => {
  // JCS §3.1 は入力を I-JSON に限るので、noncharacter を含む値から bytes を出すべきでない
  assert.throws(() => canonicalizeJson("\uFFFE"), /noncharacter/);
  assert.throws(() => canonicalizeJson({ "\uFDD0": 1 }), /noncharacter/);
  assert.throws(() => canonicalizeJson({ a: ["\uFFFF"] }), /noncharacter/);
  assert.equal(canonicalizeJson({ "\u{10384}": "é" }), '{"\u{10384}":"é"}');
});

test("getter を持つ object は canonicalize しない（hash が決定的でなくなる）", () => {
  let calls = 0;
  const counter = {
    get a() {
      return ++calls;
    },
  };
  // `JSON.stringify` は getter を呼ぶので、同じ object から違う bytes が出る
  assert.equal(JSON.stringify(counter), '{"a":1}');
  assert.equal(JSON.stringify(counter), '{"a":2}');
  assert.throws(() => canonicalizeJson(counter), /getter\/setter/);
  assert.equal(calls, 2, "canonicalizeJson は getter を呼ばない");

  // 配列の添字も同じ（`assertPlainArray` は key の同一性だけ見ていた）
  let reads = 0;
  const indexed: unknown[] = [];
  Object.defineProperty(indexed, "0", { enumerable: true, get: () => ++reads });
  Object.defineProperty(indexed, "length", { value: 1 });
  assert.equal(JSON.stringify(indexed), "[1]");
  assert.equal(JSON.stringify(indexed), "[2]");
  assert.throws(() => canonicalizeJson(indexed), /getter\/setter/);
  assert.equal(reads, 2, "canonicalizeJson は添字の getter も呼ばない");
});
