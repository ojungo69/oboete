import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync, writeFileSync, mkdirSync, symlinkSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { newRoot } from "./scratch.ts";
import {
  NORMALIZATION_VERSION,
  captureCapturedAt,
  digestCapture,
  digestNormalized,
  digestRaw,
  normalizeCapture,
  resolveEvidencePath,
} from "./normalize.ts";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const rawDir = (cli: string) => join(repoRoot, "harness", "fixtures", cli, "raw");
const raw = (cli: string, name: string) => readFileSync(join(rawDir(cli), name));

/** NDJSON を byte 列にする。行の書き順をそのまま保つため文字列連結で作る */
const capture = (...lines: string[]) => Buffer.from(lines.map((l) => `${l}\n`).join(""), "utf8");

const SESSION = "0199a1b2-c3d4-7e5f-8a90-1b2c3d4e5f60";
const basic = (over: Record<string, unknown> = {}) =>
  JSON.stringify({
    event: "SessionStart",
    at: "2026-08-12T00:00:00.000Z",
    payload: { hook_event_name: "SessionStart", session_id: SESSION, source: "startup", ...over },
  });

test("normalizeCapture は byte 列を受け取り、string を拒否する", () => {
  // 公開 API が string を受けると、呼び出し側の readFileSync(..., "utf8") で
  // 不正 UTF-8 が U+FFFD へ置換され、壊れた記録が正常な digest を得る
  const bytes = capture(basic());
  assert.equal(typeof normalizeCapture(bytes), "string");
  // @ts-expect-error 型でも実行時でも string を拒否する
  assert.throws(() => normalizeCapture(bytes.toString("utf8")), /byte/);
});

test("不正な UTF-8 は棄却される", () => {
  const good = capture(basic());
  const broken = Buffer.concat([good.subarray(0, 5), Buffer.from([0xff, 0xfe]), good.subarray(5)]);
  assert.throws(() => normalizeCapture(broken));
});

test("重複したキーを持つ行は棄却される", () => {
  const dup = capture('{"event":"SessionStart","event":"Stop","payload":{}}');
  assert.throws(() => normalizeCapture(dup));
});

test("payload.unparsed を持つ行は棄却される", () => {
  // capture-hook.sh は hook の stdin を解釈できなかったとき {unparsed: raw} に包む。
  // 「取れなかった観測」を正常な payload として digest に混ぜない
  const un = capture('{"event":"Stop","at":"2026-08-12T00:00:00.000Z","payload":{"unparsed":"garbage"}}');
  assert.throws(() => normalizeCapture(un), /unparsed/);
});

test("空の観測記録は棄却される", () => {
  assert.throws(() => normalizeCapture(Buffer.from("", "utf8")), /0/);
  assert.throws(() => normalizeCapture(Buffer.from("\n\n\n", "utf8")), /0/);
});

test("空行以外で解釈できない行が 1 つでもあれば棄却される", () => {
  assert.throws(() => normalizeCapture(capture(basic(), "{ not json")));
});

test("event か payload を欠く行は棄却される", () => {
  assert.throws(() => normalizeCapture(capture('{"at":"x","payload":{}}')), /event/);
  assert.throws(() => normalizeCapture(capture('{"event":"Stop","at":"x"}')), /payload/);
  assert.throws(() => normalizeCapture(capture('{"event":"Stop","payload":[]}')), /payload/);
});

test("キーの書き順は digest を変えない", () => {
  const a = capture('{"event":"Stop","at":"t","payload":{"hook_event_name":"Stop","session_id":"s"}}');
  const b = capture('{"payload":{"session_id":"s","hook_event_name":"Stop"},"at":"t","event":"Stop"}');
  assert.equal(digestCapture(a), digestCapture(b));
});

test("キーの書き順は相関 token の番号も変えない", () => {
  // token は初出順に振るが、その「順」を入力の property 挿入順で決めると、
  // キーを整列する §1.4 と食い違って digest が書き順で変わる
  const a = capture('{"event":"Stop","payload":{"session_id":"A","turn_id":"B"}}');
  const b = capture('{"event":"Stop","payload":{"turn_id":"B","session_id":"A"}}');
  assert.equal(digestCapture(a), digestCapture(b));
  // 整列後の順（session_id < turn_id）で 1, 2 が振られる
  assert.match(normalizeCapture(a), /"session_id":"<id:1>"/);
  assert.match(normalizeCapture(a), /"turn_id":"<id:2>"/);
});

test("欄の有無は digest を変える", () => {
  // 値を伏せても欄そのものを落とすと、未知の欄が増えたことが見えなくなる
  const a = capture('{"event":"Stop","payload":{"duration_ms":12}}');
  const b = capture('{"event":"Stop","payload":{}}');
  assert.notEqual(digestCapture(a), digestCapture(b));
  assert.match(normalizeCapture(a), /"duration_ms":"<number>"/);
});

test("__proto__ 欄の有無は保たれる", () => {
  // 中間 object を {} で作ると __proto__ の代入で欄が消え、有無が digest に出ない
  const a = capture('{"event":"Stop","payload":{"__proto__":{"x":1}}}');
  const b = capture('{"event":"Stop","payload":{}}');
  assert.notEqual(digestCapture(a), digestCapture(b));
  assert.match(normalizeCapture(a), /__proto__/);
});

test("配列の長さは保たれる", () => {
  const a = capture('{"event":"Stop","payload":{"background_tasks":[1,2]}}');
  const b = capture('{"event":"Stop","payload":{"background_tasks":[1]}}');
  assert.notEqual(digestCapture(a), digestCapture(b));
});

test("値は既定で伏せ字になり、null と boolean だけがそのまま残る", () => {
  const one = normalizeCapture(
    capture('{"event":"Stop","payload":{"a":1,"b":"x","c":"","d":null,"e":true,"f":false}}'),
  );
  assert.match(one, /"a":"<number>"/);
  assert.match(one, /"b":"<string>"/);
  assert.match(one, /"c":"<string:empty>"/);
  assert.match(one, /"d":null/);
  assert.match(one, /"e":true/);
  assert.match(one, /"f":false/);
});

test("payload 直下の 7 キーだけが verbatim になる", () => {
  const one = normalizeCapture(
    capture(
      '{"event":"PreToolUse","payload":{"tool_name":"Bash","reason":"other","tool_input":{"tool_name":"Nested","command":"secret"}}}',
    ),
  );
  assert.match(one, /"tool_name":"Bash"/);
  assert.match(one, /"reason":"other"/);
  // 深い階層の同名キーは verbatim にしない
  assert.match(one, /"tool_input":\{"command":"<string>","tool_name":"<string>"\}/);
});

test("入れ子の tool_input.prompt が違っても digest は変わらない", () => {
  // Agent tool へモデルが組み立てた引数とその echo。キー名だけで verbatim にすると
  // claude-subagent の digest が再取得で不安定になる
  const a = capture('{"event":"PreToolUse","payload":{"tool_name":"Agent","tool_input":{"prompt":"AAA"}}}');
  const b = capture('{"event":"PreToolUse","payload":{"tool_name":"Agent","tool_input":{"prompt":"BBB"}}}');
  assert.equal(digestCapture(a), digestCapture(b));
});

test("verbatim 値の RIG_INJECT marker は伏せられる", () => {
  const one = normalizeCapture(capture('{"event":"UserPromptSubmit","payload":{"prompt":"say RIG_INJECT_5f3a9 now"}}'));
  assert.match(one, /RIG_INJECT_<marker>/);
  assert.doesNotMatch(one, /5f3a9/);
});

test("verbatim 対象でも string 以外は伏せ字になる", () => {
  // 位置だけで verbatim を決めると、payload.reason に object を置いて本文を通せる
  const one = normalizeCapture(capture('{"event":"Stop","payload":{"reason":{"secret":"leak"}}}'));
  assert.doesNotMatch(one, /leak/);
});

test("識別子は値を出さずに等値関係だけ残す", () => {
  const one = normalizeCapture(
    capture(
      '{"event":"UserPromptSubmit","payload":{"session_id":"S","prompt_id":"P","cwd":"/home/someone/secret"}}',
      '{"event":"Stop","payload":{"session_id":"S","prompt_id":"P","cwd":"/home/someone/secret"}}',
    ),
  );
  assert.doesNotMatch(one, /someone/);
  assert.doesNotMatch(one, /"S"|"P"/);
  assert.equal((one.match(/<id:1>/g) ?? []).length, 2);
  assert.equal((one.match(/<path:1>/g) ?? []).length, 2);
});

test("session 識別子が継続するかどうかは digest に現れる", () => {
  // すべて <string> にすると stableNativeSessionId が証拠から導けなくなる
  const same = capture(
    '{"event":"SessionStart","payload":{"session_id":"A"}}',
    '{"event":"Stop","payload":{"session_id":"A"}}',
  );
  const changed = capture(
    '{"event":"SessionStart","payload":{"session_id":"A"}}',
    '{"event":"Stop","payload":{"session_id":"B"}}',
  );
  assert.notEqual(digestCapture(same), digestCapture(changed));
});

test("token の番号はファイル単位で、id と path で別の空間を持つ", () => {
  const one = normalizeCapture(capture('{"event":"Stop","payload":{"session_id":"A","cwd":"/x"}}'));
  assert.match(one, /"session_id":"<id:1>"/);
  assert.match(one, /"cwd":"<path:1>"/);
  // 別ファイルの番号は影響し合わない（同じ入力なら同じ digest）
  const other = normalizeCapture(capture('{"event":"Stop","payload":{"session_id":"Z","cwd":"/y"}}'));
  assert.equal(one, other);
});

test("直列化は LF 区切りで最終行の後にも LF が付く", () => {
  const one = normalizeCapture(capture(basic(), basic()));
  assert.equal(one.split("\n").length, 3);
  assert.ok(one.endsWith("\n"));
  assert.doesNotMatch(one, /\r/);
});

test("at は落ちるが、それ以外の未知の top-level キーは伏せ字で残る", () => {
  const one = normalizeCapture(capture('{"event":"Stop","at":"2026-08-12T00:00:00.000Z","payload":{},"zz":"x"}'));
  assert.doesNotMatch(one, /"at"/);
  assert.match(one, /"zz":"<string>"/);
});

test("digestRaw は生 byte の SHA-256 で、正規化を掛けない", () => {
  const a = capture(basic());
  const b = Buffer.concat([a, Buffer.from("\n", "utf8")]);
  // 正規化後は同じでも生 byte は違う
  assert.equal(digestCapture(a), digestCapture(b));
  assert.notEqual(digestRaw(a), digestRaw(b));
  assert.match(digestRaw(a), /^[a-f0-9]{64}$/);
});

test("digestNormalized は正規化済み抜粋の SHA-256 を返す", () => {
  assert.equal(
    digestNormalized("normalized\n"),
    "5279fc33061aa06e246995c8f063a5869c8b31c77b12173e05cb3b5198d451cb",
  );
});

test("NORMALIZATION_VERSION は 1", () => {
  assert.equal(NORMALIZATION_VERSION, 1);
});

// --- 実データ（US2 の受け入れ） ---

test("同一 scenario の再取得は同じ digest になる", () => {
  // interrupt3 と interrupt4 は同じ prompt・同じ event 列で、session_id・時刻・
  // transcript path・cwd だけが違う
  assert.equal(
    digestCapture(raw("claude", "claude-interrupt3.jsonl")),
    digestCapture(raw("claude", "claude-interrupt4.jsonl")),
  );
});

test("投入した指示が違う 2 記録は別の digest になる", () => {
  // prompt を verbatim から外すと、この 2 件は衝突する
  assert.notEqual(
    digestCapture(raw("claude", "claude-tool-denied.jsonl")),
    digestCapture(raw("claude", "claude-tool-ok.jsonl")),
  );
});

test("観測が本当に同一な 2 記録は同じ digest になる", () => {
  // hook の timeout は hook event 列に一切現れない。過剰除外ではなく観測が同一
  assert.equal(
    digestCapture(raw("claude", "claude-hook-timeout.jsonl")),
    digestCapture(raw("claude", "claude-lifecycle-basic.jsonl")),
  );
});

test("committed raw 16 件の digest 分布は distinct 14 種・衝突 2 組", () => {
  const digests = new Map<string, string[]>();
  for (const cli of ["claude", "codex"]) {
    for (const f of readdirSync(rawDir(cli)).filter((x) => x.endsWith(".jsonl")).sort()) {
      const d = digestCapture(raw(cli, f));
      digests.set(d, [...(digests.get(d) ?? []), `${cli}/${f}`]);
    }
  }
  const total = [...digests.values()].flat().length;
  assert.equal(total, 16);
  assert.equal(digests.size, 14);
  const collisions = [...digests.values()].filter((v) => v.length > 1).map((v) => v.sort().join(" == ")).sort();
  assert.deepEqual(collisions, [
    "claude/claude-hook-timeout.jsonl == claude/claude-lifecycle-basic.jsonl",
    "claude/claude-interrupt3.jsonl == claude/claude-interrupt4.jsonl",
  ]);
});

test("実データの正規化出力に秘密が現れない", () => {
  for (const cli of ["claude", "codex"]) {
    for (const f of readdirSync(rawDir(cli)).filter((x) => x.endsWith(".jsonl"))) {
      const out = normalizeCapture(raw(cli, f));
      assert.doesNotMatch(out, /\/home\/|\/tmp\//, `${cli}/${f} に絶対 path`);
      assert.doesNotMatch(out, /RIG_INJECT_5f3a9/, `${cli}/${f} に marker の実値`);
      assert.doesNotMatch(out, /aa16b2026df287771/, `${cli}/${f} に agent_id の実値`);
    }
  }
});

// --- path 解決 ---

test("置き場の中の観測記録は解決できる", () => {
  const p = resolveEvidencePath("claude", "claude-lifecycle-basic.jsonl");
  assert.ok(p.endsWith("/harness/fixtures/claude/raw/claude-lifecycle-basic.jsonl"));
});

test("相対 path の .. は棄却される", () => {
  assert.throws(() => resolveEvidencePath("claude", "../../../etc/passwd"), /outside|traversal|\.\./i);
  assert.throws(() => resolveEvidencePath("claude", "a/../../b.jsonl"), /outside|traversal|\.\./i);
});

test("絶対 path は棄却される", () => {
  assert.throws(() => resolveEvidencePath("claude", "/etc/passwd"));
});

test("既知でない cli は棄却される", () => {
  assert.throws(() => resolveEvidencePath("../../etc", "passwd"));
  assert.throws(() => resolveEvidencePath("gemini", "x.jsonl"));
});

test("存在しないファイルは棄却される", () => {
  assert.throws(() => resolveEvidencePath("claude", "no-such-file.jsonl"));
});

test("通常ファイルでない参照は棄却される", () => {
  const root = newRoot("evroot-");
  mkdirSync(join(root, "adir"));
  assert.throws(() => resolveEvidencePath("claude", "adir", root));
});

test("置き場の外へ出る symlink は棄却される", () => {
  const base = newRoot("evlink-");
  const root = join(base, "raw");
  mkdirSync(root);
  writeFileSync(join(base, "outside.jsonl"), "x");
  symlinkSync(join(base, "outside.jsonl"), join(root, "escape.jsonl"));
  writeFileSync(join(root, "inside.jsonl"), "x");
  assert.throws(() => resolveEvidencePath("claude", "escape.jsonl", root), /outside|escape/i);
  assert.ok(resolveEvidencePath("claude", "inside.jsonl", root));
});

test("兄弟ディレクトリは前方一致で通り抜けない", () => {
  const base = newRoot("evsib-");
  const root = join(base, "raw");
  mkdirSync(root);
  mkdirSync(join(base, "raw-evil"));
  writeFileSync(join(base, "raw-evil", "x.jsonl"), "x");
  symlinkSync(join(base, "raw-evil", "x.jsonl"), join(root, "x.jsonl"));
  assert.throws(() => resolveEvidencePath("claude", "x.jsonl", root), /outside|escape/i);
});

test("失敗の説明に絶対 path は出ない", () => {
  const err = (() => {
    try {
      resolveEvidencePath("claude", "no-such-file.jsonl");
      return null;
    } catch (e) {
      return e as Error;
    }
  })();
  assert.ok(err);
  assert.doesNotMatch(err.message, /\/home\/|\/tmp\//);
});

// manifest の pattern は綴りしか当てないので、暦に無い日付はそのまま通り、
// verifiedAt として成果物へ出る。導出の側で落とす
test("a capture whose first line names a date that does not exist is rejected", () => {
  const line = (at: string) =>
    capture(JSON.stringify({ event: "SessionStart", at, payload: { hook_event_name: "SessionStart" } }));
  assert.equal(captureCapturedAt(line("2026-02-28T00:00:00.000Z")), "2026-02-28T00:00:00.000Z");
  for (const at of ["2026-02-30T00:00:00Z", "2026-04-31T00:00:00.000Z", "2027-02-29T00:00:00Z"]) {
    assert.throws(() => captureCapturedAt(line(at)), /not a real instant/, `${at} が通った`);
  }
});
