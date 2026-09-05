// 出荷する matrix が fixture から機械的に導けることを確かめる。
//
// CI が `diff <(jq …) <(jq …)` でやっていた検査を置き換えたもの。あの形は 2 つ素通しする:
// `jq` は object の重複キーを後勝ちで潰すので、潰れる側に何を書いても比較は一致し、
// process substitution の中の `jq` が落ちても終了状態は `diff` のものしか返らない。
// ここでは repo の I-JSON parser で読むので、重複キーはその場で棄却される。
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { decodeUtf8, parseIJson } from "../schema/jcs.ts";
import { newRoot } from "./scratch.ts";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const CLIS = ["claude", "codex"] as const;

/**
 * 生成時刻だけは実行ごとに変わるので値の比較から外す。ただし**外すなら検査する**:
 * 丸ごと落として比べると、この欄に絶対 path でも秘密でも入れた matrix がゲートを通る。
 * `new Date().toISOString()` が出す形（UTC・小数 3 桁）に固定し、往復で正準性を見る
 */
function assertGeneratedAt(value: unknown, cli: string): string {
  assert.equal(typeof value, "string", `${cli}: generatedAt が文字列でない`);
  const at = value as string;
  assert.match(at, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/, `${cli}: generatedAt の形が違う`);
  assert.equal(new Date(at).toISOString(), at, `${cli}: generatedAt が正準な UTC 表記でない`);
  return at;
}

/** 出荷物は byte で比べる。構造だけ比べると、書き方の違いに何かを隠せる */
function readMatrixText(path: string): { text: string; value: Record<string, unknown> } {
  const text = decodeUtf8(readFileSync(path), path);
  return { text, value: parseIJson<Record<string, unknown>>(text) };
}

const assertNoDrift = (cli: (typeof CLIS)[number]): void => {
  const out = join(newRoot("matrix-drift-"), `${cli}.json`);
  const run = spawnSync(
    process.execPath,
    [
      "--experimental-strip-types",
      join(repoRoot, "harness", "assemble.ts"),
      join(repoRoot, "harness", "fixtures", cli),
      out,
    ],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 0, `${run.stdout}\n${run.stderr}`);

  const shipped = readMatrixText(join(repoRoot, "harness", "matrix", `${cli}.json`));
  const fresh = readMatrixText(out);
  const shippedAt = assertGeneratedAt(shipped.value.generatedAt, cli);
  assertGeneratedAt(fresh.value.generatedAt, cli);

  // 出荷側の時刻を組み立て直後の結果へ移してから、書き出しと同じ形へ直して byte で比べる
  fresh.value.generatedAt = shippedAt;
  assert.equal(
    `${JSON.stringify(fresh.value, null, 2)}\n`,
    shipped.text,
    `${cli}: 出荷している matrix が fixture から導けない（手で編集された）`,
  );
};

// test 名は literal で並べる（変異表との突き合わせが grep で効く形にするため）
test("the shipped claude matrix is what the fixtures assemble to", () => assertNoDrift("claude"));
test("the shipped codex matrix is what the fixtures assemble to", () => assertNoDrift("codex"));
