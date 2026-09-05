// DCO ゲートの純粋な判定を、通す側と落とす側の両方で固定する。
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

import { findUnsignedCommits } from "./dco-check.mjs";

function commit(overrides = {}) {
  return {
    sha: "1111111111111111111111111111111111111111",
    subject: "変更を追加",
    authorName: "Alice Example",
    authorEmail: "alice@example.com",
    committerEmail: "committer@example.com",
    body: "変更を追加\n\nSigned-off-by: Alice Example <alice@example.com>\n",
    ...overrides,
  };
}

test("すべての commit に一致する sign-off があれば空を返す", () => {
  const commits = [
    commit(),
    commit({
      sha: "2222222222222222222222222222222222222222",
      authorEmail: "ALICE@example.com",
    }),
  ];

  assert.deepEqual(findUnsignedCommits(commits), []);
});

test("未署名の commit だけを返す", () => {
  const unsigned = commit({
    sha: "2222222222222222222222222222222222222222",
    subject: "未署名の変更",
    body: "未署名の変更\n",
  });

  assert.deepEqual(findUnsignedCommits([commit(), unsigned]), [unsigned]);
});

test("sign-off の email が author と committer のどちらにも一致しなければ返す", () => {
  const mismatched = commit({
    body: "変更を追加\n\nSigned-off-by: Mallory Example <mallory@example.com>\n",
  });

  assert.deepEqual(findUnsignedCommits([mismatched]), [mismatched]);
});

test("複数の sign-off のうち 1 つが committer email と一致すれば通す", () => {
  const signed = commit({
    body: [
      "変更を追加",
      "",
      "Signed-off-by: Mallory Example <mallory@example.com>",
      "Signed-off-by: Committer Example <COMMITTER@example.com>",
      "",
    ].join("\n"),
  });

  assert.deepEqual(findUnsignedCommits([signed]), []);
});

test("bot を名乗る author email でも未署名なら落とす", () => {
  // `git commit --author` で誰でも名乗れる綴りなので、bot 名は免除の根拠にならない。
  const botEmails = [
    "dependabot[bot]@users.noreply.github.com",
    "49699333+dependabot[bot]@users.noreply.github.com",
    "github-actions[bot]@users.noreply.github.com",
  ];
  const commits = botEmails.map((authorEmail, index) =>
    commit({ sha: String(index + 1).repeat(40), authorEmail, body: "依存更新\n" }),
  );

  assert.deepEqual(findUnsignedCommits(commits), commits);
});

test("bot を名乗る author email でも署名があれば通す", () => {
  const signed = commit({
    authorEmail: "dependabot[bot]@users.noreply.github.com",
    body: "依存更新\n\nSigned-off-by: Dependabot <dependabot[bot]@users.noreply.github.com>\n",
  });

  assert.deepEqual(findUnsignedCommits([signed]), []);
});

// ゲートの「形」は unit test が届かない層（workflow の trigger と job 名）で決まっているので、
// そこが崩れたら落ちるものをここに 1 つ置く。security control ではなく、後から自分で壊さない
// ための回帰検査。
//
// GitHub が required status check を照合する名前は job の `name`、無ければ job id。どちらの
// 綴りでも `dco` という check を作れるので両方拾う。indent は固定しない（4 space でも valid）。
const DCO_CHECK_NAME = /^\s+(?:["']?dco["']?\s*:|name\s*:\s*["']?dco["']?)\s*(?:#.*)?$/gm;
// job 名を式で組み立てられると、静的には `dco` を作る job を数え切れない。保守的に拒否する。
const DYNAMIC_JOB_NAME = /^\s+name\s*:.*\$\{\{/gm;

// `  dco:` から、同じ indent の次の key までを 1 job の本文として切り出す。job を跨いだ検査は
// 「checker はどこかにある」「skip 制御はどこにも無い」を別々に見てしまい、checker を別 job へ
// 移して `dco` を空の job に差し替える変異を通す。
function jobBlock(workflow, jobId) {
  const header = workflow.match(new RegExp(`^( +)["']?${jobId}["']?\\s*:\\s*$`, "m"));
  if (header === null) return null;
  const from = header.index + header[0].length;
  const rest = workflow.slice(from);
  const next = rest.match(new RegExp(`^ {0,${header[1].length}}\\S`, "m"));
  return next === null ? rest : rest.slice(0, next.index);
}

test("dco check は 1 経路だけで、base branch 側から走る", () => {
  const workflowDir = fileURLToPath(new URL("../.github/workflows", import.meta.url));
  const workflows = readdirSync(workflowDir)
    .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))
    .map((name) => [name, readFileSync(join(workflowDir, name), "utf8")]);

  // ファイル数ではなく producer 数を数える。required check は同じ名前の check run を全件見るので、
  // 生産者が増えると、そちらが失敗・取り消しになるだけで merge が止まる。どの run が判定したのかも
  // 辿れなくなる。同じ file の中に 2 つあっても等しく駄目。
  const producers = workflows.flatMap(([name, text]) =>
    [...text.matchAll(DCO_CHECK_NAME)].map(() => name),
  );
  assert.deepEqual(producers, ["dco.yml"]);
  assert.deepEqual(
    workflows.flatMap(([name, text]) => [...text.matchAll(DYNAMIC_JOB_NAME)].map(() => name)),
    [],
  );

  const [, workflow] = workflows.find(([name]) => name === "dco.yml");
  // PR 側の tree から実行すると、PR が自分を検査する workflow と checker を書き換えられる。
  assert.match(workflow, /^ +pull_request_target\s*:\s*$/m);
  assert.doesNotMatch(workflow, /^ +pull_request\s*:\s*$/m);
  // retarget (edited) を落とすと、main へ向いた PR が検査されないまま残る。
  assert.match(workflow, /types\s*:.*edited/);
  // 取り消された run は required check を満たさないので、concurrency は merge を止めてしまう。
  // workflow 直下でも job の中でも同じなので、indent と引用符を許して両方を拒否する。
  assert.doesNotMatch(workflow, /^\s*["']?concurrency["']?\s*:/m);

  const dco = jobBlock(workflow, "dco");
  assert.ok(dco, "dco job が見つからない");
  // checkout に ref を渡すと base ではなく PR head を取り出してしまう。
  assert.doesNotMatch(dco, /^\s+["']?ref["']?\s*:/m);
  // job を skip させれば、検査せずに成功した check が出る。`needs` も同じで、依存先が失敗すると
  // この job は skip され、required check としては成功と同じ扱いになる。`defaults` / `shell` /
  // `working-directory` は run step の実行そのものを差し替えられる（`shell: "true {0}"` など）。
  assert.doesNotMatch(
    dco,
    /^\s+["']?(?:if|continue-on-error|needs|defaults|shell|working-directory)["']?\s*:/m,
  );
  // 行全体で固定する。`run: echo node scripts/dco-check.mjs ...` でも部分一致は通ってしまう。
  assert.match(dco, /^ +run: node scripts\/dco-check\.mjs "\$BASE_REF" "\$HEAD_REF"$/m);
});

test("本文中の引用や行途中にある Signed-off-by は trailer として扱わない", () => {
  const quoted = commit({
    sha: "2222222222222222222222222222222222222222",
    body: "説明\n\n> Signed-off-by: Alice Example <alice@example.com>\n",
  });
  const inline = commit({
    sha: "3333333333333333333333333333333333333333",
    body: "説明\n\n例: Signed-off-by: Alice Example <alice@example.com>\n",
  });

  assert.deepEqual(findUnsignedCommits([quoted, inline]), [quoted, inline]);
});

test("`<` を持たない長い空白列の sign-off でも判定が一瞬で終わる（backtracking の回帰）", () => {
  const padded = commit({ body: "Refs: #1\nSigned-off-by: " + " ".repeat(32_000) + "x\n" });
  const started = performance.now();
  assert.deepEqual(findUnsignedCommits([padded]), [padded]);
  assert.ok(performance.now() - started < 2_000);
});

// 上の 8 件は純関数だけを見る。実測では、その 8 件が全部通ったまま CLI 経路が `Refs #59` +
// sign-off の commit を落としていた（trailer block の判定違い）。実物を子プロセスとして
// 起動し、通す側・落とす側・fail-closed の 3 方向を固定する。
const script = fileURLToPath(new URL("./dco-check.mjs", import.meta.url));

// 実行者の global / system config を継承すると、`commit.template` や `core.hooksPath` で
// message や commit そのものが変わり、test の結果が実行環境に依存する。checker が中で呼ぶ
// `git log` にも同じ隔離が要るので、子プロセス両方へ渡す。
const GIT_ENV = { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_SYSTEM: "/dev/null" };

function run(cwd, args) {
  return spawnSync(process.execPath, [script, ...args], { cwd, encoding: "utf8", env: GIT_ENV });
}

function git(cwd, args) {
  const result = spawnSync("git", args, { cwd, encoding: "utf8", env: GIT_ENV });
  assert.equal(result.status, 0, `git ${args.join(" ")}: ${result.stderr}`);
  return result.stdout.trim();
}

function repository(t) {
  const root = mkdtempSync(join(tmpdir(), "dco-check-repo-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  git(root, ["init", "-q", "-b", "main"]);
  git(root, ["config", "user.name", "Alice Example"]);
  git(root, ["config", "user.email", "alice@example.com"]);
  git(root, ["config", "commit.gpgsign", "false"]);
  return root;
}

function commitFile(root, name, message, { addSignOff = true } = {}) {
  writeFileSync(join(root, name), `${name}\n`);
  git(root, ["add", name]);
  git(root, ["commit", "-q", ...(addSignOff ? ["-s"] : []), "-m", message]);
}

test("実スクリプト: PR の commit が全部署名済みなら通る", (t) => {
  const root = repository(t);
  commitFile(root, "base.txt", "base commit");
  const base = git(root, ["rev-parse", "HEAD"]);
  git(root, ["checkout", "-q", "-b", "feature"]);
  commitFile(root, "a.txt", "signed commit");
  // `git commit -s` は trailer らしくない最終段落の後に空行を足すので、この形にはならない。
  // agent や人が message 全体を書くときにだけ `Refs #59` の直下に sign-off が並び、そちらが
  // 落ちていた。message をそのまま渡して再現する。
  commitFile(
    root,
    "b.txt",
    "refs style commit\n\nRefs #59\nSigned-off-by: Alice Example <alice@example.com>\n",
    { addSignOff: false },
  );

  const result = run(root, [base, "HEAD"]);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /DCO check OK: 2 commit\(s\)/);
});

test("実スクリプト: 未署名 commit が 1 件でもあれば落ちる", (t) => {
  const root = repository(t);
  commitFile(root, "base.txt", "base commit");
  const base = git(root, ["rev-parse", "HEAD"]);
  git(root, ["checkout", "-q", "-b", "feature"]);
  commitFile(root, "a.txt", "signed commit");
  commitFile(root, "b.txt", "unsigned commit", { addSignOff: false });

  const result = run(root, [base, "HEAD"]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /DCO check FAILED/);
  assert.match(result.stderr, /unsigned commit/);
});

// 検査できなかったことを成功として返さない。ここが緩むと、ゲートは「何も見ていない」まま緑になる。
test("実スクリプト: 検査対象を確定できない場合は fail-closed", (t) => {
  const root = repository(t);
  commitFile(root, "base.txt", "base commit");

  assert.equal(run(root, []).status, 2, "引数不足");
  assert.equal(run(root, ["HEAD"]).status, 2, "引数不足");
  assert.equal(run(root, ["HEAD", "HEAD", "extra"]).status, 2, "余分な引数");
  assert.equal(run(root, ["does-not-exist", "HEAD"]).status, 2, "解決できない ref");
  assert.equal(run(root, ["HEAD", "HEAD"]).status, 2, "範囲が空");
});

// 起動判定が path を解決できないとき、`false` を返すと検査せず exit 0 で終わる。argv[1] を
// 存在しない path にして import し、握り潰さず main() へ届くことを固定する。
test("実スクリプト: 起動判定が path を解決できなくても main() に到達する", (t) => {
  const root = mkdtempSync(join(tmpdir(), "dco-check-argv-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const source = `process.argv[1] = ${JSON.stringify(join(root, "missing.mjs"))};
await import(${JSON.stringify(pathToFileURL(script).href)});`;

  const result = spawnSync(process.execPath, ["--input-type=module", "-e", source], {
    cwd: root,
    encoding: "utf8",
  });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /usage: dco-check\.mjs/);
});

// 起動判定が壊れると、ゲートは何も検査せず exit 0 で終わる。symlink 経由でも main() に届くこと。
test("実スクリプト: symlink 経由で起動しても main() に到達する", (t) => {
  const root = mkdtempSync(join(tmpdir(), "dco-check-link-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const link = join(root, "linked-dco-check.mjs");
  symlinkSync(script, link);

  const result = spawnSync(process.execPath, [link], { cwd: root, encoding: "utf8" });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /usage: dco-check\.mjs/);
});
