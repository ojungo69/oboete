// 秘密が成果物へ出ない経路を、組み立てを子プロセスとして起動して確かめる。
// 同一プロセスで assembleFromFixtures を呼ぶ test では、実際に出力される file の中身と
// stdout / stderr を見られない（entrypoint の起動判定も含めて経路が違う）。
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { appendFileSync, cpSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test, { type TestContext } from "node:test";
import { assertNoSecretSubstrings } from "../assemble.ts";
import { SECRET_KEYS, SECRET_SUBTREES, collectSecretsOf } from "./verify.ts";

const HARNESS = fileURLToPath(new URL("../", import.meta.url));
const RAW = "claude-lifecycle-basic.jsonl";

// canary はどれも 16 文字以上。漏れた場合は成果物の検査（16-gram 警報）にも掛かる
const PROSE = "CANARY-PROSE-8f2b1c4d9e7a";
const CWD = "CANARY-CWD-3a7e5b1f2d8c0114";
const MSG = "CANARY-MSG-6d4c2a9f1b3e0227";
const EVENT = "CANARY-EVENT-1e9b7d3f5a2c";
const SCENARIO = "CANARY-SCENARIO-4b8e0c6a2f91";
const CODE = "CANARY-CODE-7f1a3e9d5b2c";
/** 綴りの検査を通ってしまう形。key 名に秘密を入れる経路はこちらが本命 */
const PLAIN_CODE = "CANARYCODE7f1a3e9d5b2c";

const node = (args: string[]) =>
  spawnSync(process.execPath, ["--experimental-strip-types", ...args], { encoding: "utf8" });

/**
 * harness ごと複製する。証拠置き場は module からの相対なので、複製と一緒に動く。
 * 複製は 2MB を超えるうえ、変異ゲートは test 一式を 98 回回すので、test ごとに片付ける
 */
function plantedTree(t: TestContext): { tmp: string; rawPath: string } {
  const tmp = mkdtempSync(join(tmpdir(), "evidence-secrets-"));
  t.after(() => rmSync(tmp, { recursive: true, force: true }));
  cpSync(HARNESS, join(tmp, "harness"), { recursive: true });
  const fixturesDir = join(tmp, "harness", "fixtures", "claude");
  const rawPath = join(fixturesDir, "raw", RAW);

  // (b) 観測記録の秘密欄へ仕込む
  const lines = readFileSync(rawPath, "utf8").split("\n");
  const planted = lines.map((line) => {
    if (line.trim() === "") return line;
    const rec = JSON.parse(line);
    rec.payload.cwd = `/home/someone/${CWD}`;
    if ("last_assistant_message" in rec.payload) rec.payload.last_assistant_message = MSG;
    return JSON.stringify(rec);
  });
  writeFileSync(rawPath, planted.join("\n"));

  // 記録を変えたので digest を取り直す。手で計算せず CLI から得る
  const out = node([join(tmp, "harness", "evidence", "normalize.ts"), rawPath]);
  assert.equal(out.status, 0, out.stderr);
  const { evidenceHash, captureRawHash } = JSON.parse(out.stdout);

  let prosePlanted = false;
  let eventPlanted = false;
  for (const name of readdirSync(fixturesDir).filter((n) => n.endsWith(".json"))) {
    const file = join(fixturesDir, name);
    const fixture = JSON.parse(readFileSync(file, "utf8"));
    for (const ref of fixture.evidence ?? []) {
      if (ref.path !== RAW) continue;
      ref.evidenceHash = evidenceHash;
      ref.captureRawHash = captureRawHash;
    }
    fixture.scenario = `${fixture.scenario} ${SCENARIO}`;
    // (a) fixture の散文へ仕込む。散文は成果物へ出ず、対応する code だけが出る
    if (!prosePlanted && fixture.limitations?.length) {
      fixture.limitations[0] = `${fixture.limitations[0]} ${PROSE}`;
      prosePlanted = true;
    }
    // 事象側の散文も別経路。こちらは cell の limitations へ出る欄なので別に仕込む
    for (const ev of fixture.observedEvents ?? []) {
      if (eventPlanted || !ev.limitations?.length) continue;
      ev.limitations[0] = `${ev.limitations[0]} ${EVENT}`;
      eventPlanted = true;
    }
    writeFileSync(file, JSON.stringify(fixture, null, 2));
  }
  assert.ok(prosePlanted, "散文 limitations を持つ fixture が無い");
  assert.ok(eventPlanted, "散文 limitations を持つ事象が無い");
  return { tmp, rawPath };
}

test("planted canaries never reach the matrix, stdout, or stderr", (t) => {
  const { tmp } = plantedTree(t);
  const outFile = join(tmp, "out.json");
  const run = node([join(tmp, "harness", "assemble.ts"), join(tmp, "harness", "fixtures", "claude"), outFile]);
  assert.equal(run.status, 0, `${run.stdout}\n${run.stderr}`);

  const matrix = readFileSync(outFile, "utf8");
  // 仕込みが効いていることを先に見る（記録側に無ければ、この test は何も守らない）
  assert.match(readFileSync(join(tmp, "harness", "fixtures", "claude", "raw", RAW), "utf8"), new RegExp(CWD));
  for (const [label, canary] of [["prose", PROSE], ["event", EVENT], ["scenario", SCENARIO], ["cwd", CWD], ["message", MSG]] as const) {
    assert.ok(!matrix.includes(canary), `${label} canary が matrix に出た`);
    assert.ok(!run.stdout.includes(canary), `${label} canary が stdout に出た`);
    assert.ok(!run.stderr.includes(canary), `${label} canary が stderr に出た`);
  }
  // 成果物が空でないこと（何も出さなければ canary も出ない）
  assert.match(matrix, /"evidenceKind"/);
});

test("failure messages carry neither capture contents nor absolute paths", (t) => {
  const { tmp, rawPath } = plantedTree(t);
  appendFileSync(rawPath, "\n");

  const run = node([join(tmp, "harness", "assemble.ts"), join(tmp, "harness", "fixtures", "claude"), join(tmp, "o.json")]);
  assert.notEqual(run.status, 0, "改竄した記録で組み立てが成功した");
  const said = `${run.stdout}\n${run.stderr}`;
  assert.match(said, /captureRawHash mismatch/);
  assert.match(said, new RegExp(RAW), "どの記録かは basename で言う");
  assert.ok(!said.includes(tmp), "失敗の説明に絶対 path が出た");
  for (const canary of [PROSE, EVENT, SCENARIO, CWD, MSG]) assert.ok(!said.includes(canary), "失敗の説明に記録の中身が出た");
});

// 棄却の診断も漏洩経路。schema が値を弾くとき、その値を message に載せると
// stderr から CI ログへ流れる（隣の fixtureId 検査が生値を避けているのと同じ理由）
test("schema diagnostics do not echo the rejected value", (t) => {
  const { tmp } = plantedTree(t);
  const dir = join(tmp, "harness", "fixtures", "claude");
  const file = join(dir, readdirSync(dir).filter((n) => n.endsWith(".json"))[0] as string);
  const fixture = JSON.parse(readFileSync(file, "utf8"));
  fixture.limitationCodes = [`${CODE}`];
  writeFileSync(file, JSON.stringify(fixture, null, 2));

  const run = node([join(tmp, "harness", "assemble.ts"), dir, join(tmp, "o.json")]);
  assert.notEqual(run.status, 0, "enum 外の値で組み立てが成功した");
  const said = `${run.stdout}\n${run.stderr}`;
  assert.ok(said.includes("value not in enum"), `enum の棄却が出ていない: ${said}`);
  assert.ok(!said.includes(CODE), "棄却した値そのものが診断に出た");
});

// schema より前に手書きの検証が走る欄がある。schema 側だけ直しても、そちらが
// 値をそのまま返していれば同じ漏洩が別の識別子で残る
test("hand-written fixture validation does not echo the rejected value either", (t) => {
  const { tmp } = plantedTree(t);
  const dir = join(tmp, "harness", "fixtures", "claude");
  const file = join(dir, readdirSync(dir).filter((n) => n.endsWith(".json"))[0] as string);
  const fixture = JSON.parse(readFileSync(file, "utf8"));
  // 値の側（kind / phase）と、key 名の側の両方。key 名は**英数字だけ**の綴りで置く:
  // 「形が安全なら出す」という直し方はこれを素通しする（key 名そのものが fixture の中身）
  fixture.observedEvents[0].kind = CODE;
  fixture.toolFailurePhasesObserved = [CODE];
  fixture[PLAIN_CODE] = 1;
  fixture.observedEvents[0][`x\n${CODE}`] = 1;
  writeFileSync(file, JSON.stringify(fixture, null, 2));

  const run = node([join(tmp, "harness", "assemble.ts"), dir, join(tmp, "o.json")]);
  assert.notEqual(run.status, 0, "不正な kind で組み立てが成功した");
  const said = `${run.stdout}\n${run.stderr}`;
  assert.ok(said.includes("capability.schema.json"), `手書き検証の説明が出ていない: ${said}`);
  assert.ok(!said.includes(CODE), "棄却した値または key 名が診断に出た");
  assert.ok(!said.includes(PLAIN_CODE), "英数字だけの未知 key 名が診断に出た");
});

// --- 警報そのものを直接見る ---
// 正しい実装では秘密が成果物へ届く経路が無いので、上の canary test は警報を殺しても落ちない。
// 警報は「他の防御が破れたとき最後に鳴るもの」なので、単体で鳴ることを別に確かめる。

test("a 16+ char secret substring in a generated string fails the build", () => {
  // 変数名も値も鍵らしくしない（gitleaks の generic-api-key が拾う）
  const material = "canary-canary-canary";
  assert.throws(() => assertNoSecretSubstrings({ cell: { note: `xx${material.slice(0, 16)}yy` } }, [material]), /16\+ character/);
  // 15 文字までは通す。窓を縮めると偽陽性で正常な組み立てが落ちる
  assert.doesNotThrow(() => assertNoSecretSubstrings({ cell: { note: `xx${material.slice(0, 15)}yy` } }, [material]));
  assert.doesNotThrow(() => assertNoSecretSubstrings({ cell: { note: material } }, []));
});

test("the fields the alarm watches cannot be narrowed at run time", () => {
  // export している一覧は複製。収集が見る集合は module の中にあるので、ここから消しても
  // 警報の材料は減らない（減らせると、その欄の値が成果物へ出ても誰も気づかない）
  const keys = SECRET_KEYS as string[];
  const subtrees = SECRET_SUBTREES as string[];
  const before = { keys: [...keys], subtrees: [...subtrees] };
  keys.splice(keys.indexOf("cwd"), 1);
  subtrees.splice(subtrees.indexOf("tool_input"), 1);
  try {
    const long = (tag: string) => `${tag}-0123456789abcdef`;
    const line = {
      event: "PreToolUse",
      at: "2026-01-01T00:00:00.000Z",
      payload: { cwd: long("cwd"), tool_input: { field: long("tool-input") } },
    };
    const found = collectSecretsOf(Buffer.from(`${JSON.stringify(line)}\n`, "utf8"));
    assert.ok(found.has(long("cwd")), "export した一覧から消しただけで欄が警報の材料から外れた");
    assert.ok(found.has(long("tool-input")), "export した一覧から消しただけで部分木が警報の材料から外れた");
  } finally {
    keys.splice(0, keys.length, ...before.keys);
    subtrees.splice(0, subtrees.length, ...before.subtrees);
  }
});

test("collectSecrets covers every secret-bearing field", () => {
  // 集合そのものから payload を組むと、欄を**外した**変異まで test が一緒に縮んで気づけない
  // （実測: 変異 M24「材料から cwd を外す」が生き残った）。そこで綴りはここに固定し、
  // 固定した並びと実装の集合が一致することを先に主張する。これで両方向が閉じる:
  // 欄を外せば下の deepEqual が落ち、欄を足せばここが落ちて test の更新を強制する
  const SECRET_FIELDS = ["prompt", "last_assistant_message", "cwd", "transcript_path", "agent_transcript_path"];
  const SUBTREE_FIELDS = ["tool_input", "tool_response"];
  assert.deepEqual([...SECRET_KEYS].sort(), [...SECRET_FIELDS].sort(), "秘密欄の集合が test の並びと違う");
  assert.deepEqual([...SECRET_SUBTREES].sort(), [...SUBTREE_FIELDS].sort(), "秘密部分木の集合が test の並びと違う");

  const long = (tag: string) => `${tag}-0123456789abcdef`;
  const payload: Record<string, unknown> = { hook_event_name: long("not-a-secret") };
  for (const key of SECRET_FIELDS) payload[key] = long(key);
  // 部分木は入れ子も配列の中まで辿ることを見る
  for (const key of SUBTREE_FIELDS) {
    payload[key] = { field: long(`${key}-field`), nested: [{ deep: long(`${key}-deep`) }] };
  }
  const line = { event: "PreToolUse", at: "2026-01-01T00:00:00.000Z", payload };
  const found = collectSecretsOf(Buffer.from(`${JSON.stringify(line)}\n`, "utf8"));
  for (const key of SECRET_FIELDS) {
    assert.ok(found.has(long(key)), `${key} が警報の材料に入っていない`);
  }
  for (const key of SUBTREE_FIELDS) {
    assert.ok(found.has(long(`${key}-field`)), `${key} 直下が警報の材料に入っていない`);
    assert.ok(found.has(long(`${key}-deep`)), `${key} の入れ子が警報の材料に入っていない`);
  }
  // 秘密でない欄まで材料にすると、正常な組み立てが偽陽性で落ちる
  assert.ok(!found.has(long("not-a-secret")));
});

// --- 秘密走査ゲートの「形」 ---
// 実際に走査が効くかは gitleaks を動かさないと分からないので、ここは best-effort。
// 見るのは「走査範囲を他人に選ばせていないか」だけ（信頼境界は full scan そのもの）

test("the secrets gate does not let an action pick which commits to scan", () => {
  const ci = readFileSync(new URL("../../.github/workflows/ci.yml", import.meta.url), "utf8");
  const start = ci.indexOf("\n  secrets:");
  // job が消えた・改名された場合に「走査していない」と誤診断しない。切り出しは次の job
  // 見出しまでで止める（後ろに job が増えると、その中身をこの job のせいにしてしまう）
  assert.notEqual(start, -1, "ci.yml に secrets job が無い");
  const rest = ci.slice(start + 1);
  const next = rest.slice(1).search(/\n {2}\S+:/);
  const job = next === -1 ? rest : rest.slice(0, next + 1);
  // gitleaks-action は pulls/:n/commits を pagination なしで呼び、その 1 ページ目の
  // 先頭と末尾だけを範囲にする。30 commit を超える PR では新しい側が丸ごと外れる
  // 綴りではなく **action として使っているか** を見る。ci.yml 側の説明文にも同じ語が出るので、
  // 語の有無で見ると自分の説明に引っかかる
  assert.ok(!/uses: gitleaks\//.test(job), "走査範囲を選ぶ action へ戻っている");
  assert.ok(!job.includes("--log-opts"), "走査範囲を絞る指定が入っている");
  assert.match(job, /gitleaks" detect .*--source \./s, "全履歴を走査していない");
  assert.match(job, /sha256sum -c -/, "取得した binary を検証していない");
});
