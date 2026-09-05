// rig が書く manifest を、実 CLI 無しで end-to-end に確かめる。
// stub CLI を CLAUDE_BIN として渡し、rig.sh の setup → claude-run → import を実際に走らせる。
// harness ごと複製して走らせるので、既定の証拠置き場（module からの相対）も複製側を指す。
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test, { after } from "node:test";
import { fileURLToPath } from "node:url";

const HARNESS = fileURLToPath(new URL("../", import.meta.url));
// rig を実 process として走らせる test は harness ごと複製する（1 回あたり数 MB）。変異ゲートは
// 変異 1 件ごとにこの file を回すので、置きっぱなしにすると CI の /tmp が数 GB 単位で埋まる
const SCRATCH = [];
after(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});
const LABEL = "rig-stub";
const VERSION = "9.9.9-stub (Stub Code)";

// 実 CLI の代わり。--version は 1 行返し、それ以外では hook 相当の観測記録を書く。
// 付属物は rig と同じ stem（.jsonl を外した形）で見る。記録側の名前で見ると、rig が
// 別名を消していても常に「無い」と読めてしまい、検査が空振りする
const stub = (extra = "", versionExtra = "") => `#!/usr/bin/env bash
set -eu
# --version も run_env 経由なので CAPTURE_FILE がある。隔離が効いているかをここで書き出す
# （run_env の外から呼ぶ変異では CAPTURE_FILE が無くなる）。STEM は記録本体を書く段で作る
if [ "\${1:-}" = --version ]; then
{ echo "HOME=\$HOME"; echo "CLAUDE_CONFIG_DIR=\${CLAUDE_CONFIG_DIR:-<unset>}"; echo "INTERNAL=\${AGENT_MEMORY_INTERNAL_RUN:-<unset>}"; echo "PWD=\$PWD"; echo "GITSYS=\${GIT_CONFIG_NOSYSTEM:-<unset>}"; echo "CAPTURE=\${CAPTURE_FILE:-<unset>}"; } > "\${CAPTURE_FILE:-/dev/null}.version-env"
${versionExtra}
printf '%s\\n' '${VERSION}'; exit 0; fi
STEM="\${CAPTURE_FILE%.jsonl}"; export STEM  # 子 shell から見えないと、目印を書かせる test が空振りする
# 前の run の終了コードが残ったまま今回の run が始まっていないか。run 中にしか観測できない
if [ -e "$STEM.exit" ]; then printf 'stale\\n' > "$STEM.stale-exit"; fi
# 資格情報が置かれるのは run の最中だけ（trap で消える）ので、ここでしか観測できない
if [ -e "\${CLAUDE_CONFIG_DIR:-/nonexistent}/.credentials.json" ]; then printf 'staged\\n' > "$STEM.staged-credential"; fi
${extra}
emit() { printf '{"event":"%s","at":"2026-01-01T00:00:00.000Z","payload":%s}\\n' "$1" "$2" >> "$CAPTURE_FILE"; }
emit SessionStart '{"hook_event_name":"SessionStart","source":"startup","session_id":"s-stub","cwd":"/w","transcript_path":"/w/t.jsonl"}'
emit UserPromptSubmit '{"hook_event_name":"UserPromptSubmit","prompt_id":"p-stub","prompt":"hello","session_id":"s-stub","cwd":"/w","transcript_path":"/w/t.jsonl"}'
emit Stop '{"hook_event_name":"Stop","prompt_id":"p-stub","last_assistant_message":"hi","session_id":"s-stub","cwd":"/w","transcript_path":"/w/t.jsonl"}'
emit SessionEnd '{"hook_event_name":"SessionEnd","prompt_id":"p-stub","reason":"other","session_id":"s-stub","cwd":"/w","transcript_path":"/w/t.jsonl"}'
`;

// group を抜けて lock の fd を握ったまま生き残る子。detach し切る前に run が終わると
// group kill が間に合ってしまうので、抜けたことを待ってから stub を終える（race のまま
// 置くと test が気まぐれに緑になる）
const ESCAPED_CHILD =
  'setsid sh -c \'printf x > "$STEM.detached"; sleep 20\' </dev/null >/dev/null 2>&1 &\n' +
  'for _ in 1 2 3 4 5 6 7 8 9 10; do [ -e "$STEM.detached" ] && break; sleep 0.2; done';

function rigRun({ stubExtra = "", stubVersionExtra = "", skipImport = false, skipRun = false, expectRunFailure = false, env: extraEnv = {} } = {}) {
  const tmp = mkdtempSync(join(tmpdir(), "rig-manifest-"));
  SCRATCH.push(tmp);
  cpSync(HARNESS, join(tmp, "harness"), { recursive: true });
  const stubPath = join(tmp, "stub-cli");
  writeFileSync(stubPath, stub(stubExtra, stubVersionExtra));
  chmodSync(stubPath, 0o755);

  const rig = join(tmp, "harness", "rig", "rig.sh");
  const base = join(tmp, "rig-base");
  // HOME は差し替える。実 HOME のままだと rig が開発者の Claude 資格情報を
  // 一時 rig へ複製する（trap で消えるが、SIGKILL で落ちれば残る）
  const env = { ...process.env, HOME: join(tmp, "home"), RIG_BASE: base, CLAUDE_BIN: stubPath, ...extraEnv };
  const sh = (...args) => spawnSync("bash", [rig, ...args], { encoding: "utf8", env });

  const setup = sh("setup");
  assert.equal(setup.status, 0, setup.stderr);
  if (skipRun) return { tmp, base, sh, setup, rawDir: join(tmp, "harness", "fixtures", "claude", "raw") };
  const run = sh("claude-run", LABEL, "hello");
  if (!expectRunFailure) assert.equal(run.status, 0, run.stderr);
  const rawDir = join(tmp, "harness", "fixtures", "claude", "raw");
  if (skipImport) return { tmp, base, sh, setup, rawDir, run };
  const imported = sh("import", "claude", LABEL, "self.stub");
  assert.equal(imported.status, 0, imported.stderr);

  return {
    tmp,
    base,
    sh,
    rawDir,
    ref: JSON.parse(imported.stdout),
    capture: join(base, "capture", `claude-${LABEL}.jsonl`),
    manifest: JSON.parse(readFileSync(join(rawDir, `claude-${LABEL}.manifest.json`), "utf8")),
  };
}

test("the rig writes a manifest and imports the capture byte-identically", () => {
  const { rawDir, ref, capture, manifest } = rigRun();

  assert.deepEqual(readFileSync(join(rawDir, `claude-${LABEL}.jsonl`)), readFileSync(capture));
  assert.equal(manifest.cliVersion, VERSION);
  assert.equal(manifest.cli, "claude");
  assert.equal(manifest.scenarioId, "self.stub");
  assert.equal(manifest.isolated, true);
  assert.equal(manifest.internalRunMarker, true);
  assert.equal(manifest.exitStatus, 0);
  assert.equal(manifest.recorderErrors, 0);
  assert.equal(manifest.capture, `claude-${LABEL}.jsonl`);
  // 出力した ref と manifest が同じ digest を指す（貼り間違いではなく機械で一致する）
  assert.equal(ref.evidenceHash, manifest.captureHash);
  assert.equal(ref.captureRawHash, manifest.captureRawHash);
  assert.match(ref.manifestHash, /^[a-f0-9]{64}$/);
});

test("the rig reports usage and exit 2 for a wrong argument count", () => {
  // guard は with_lock より前、RIG_BASE を見る前に降りる。scratch tree も setup も要らない
  // （mutate.sh はこの file を 145 回以上回すので、1 件あたりの複製が丸ごと乗る）
  const cases = [
    [["import"], /usage: rig\.sh import <cli> <label> <scenario-id>/],
    [["import", "claude"], /usage: rig\.sh import <cli> <label> <scenario-id>/],
    [["import", "claude", "l", "s", "extra"], /usage: rig\.sh import <cli> <label> <scenario-id>/],
    [["claude-run"], /usage: rig\.sh claude-run <label> <prompt>/],
    [["codex-run"], /usage: rig\.sh codex-run <label> <prompt>/],
  ];
  for (const [argv, usage] of cases) {
    const result = spawnSync("bash", [join(HARNESS, "rig", "rig.sh"), ...argv], { encoding: "utf8" });
    assert.equal(result.status, 2, `${argv.join(" ")}: ${result.stderr}`);
    assert.match(result.stderr, usage, argv.join(" "));
  }
});

// 落とす側の case だけでは件数の guard が `-eq 2` でも `-ge 2` でも同じ結果になる。header は
// `[claude 追加引数...]` を約束し、実装は shift 2 のあと `"$@"` を測定対象へ渡すので、**通す側**も
// 測る。締めすぎた guard は、この test が無いと自分の suite からは緑のまま漏れる
test("the rig accepts trailing arguments after <label> <prompt>", () => {
  // 不正な label で require_label に降ろす。件数の guard を越えたことだけを見たいので、
  // lock も測定対象の CLI も踏まない（どちらも RIG_BASE の setup が要る）
  for (const sub of ["claude-run", "codex-run"]) {
    const argv = [sub, "bad/label", "prompt", "--extra"];
    const result = spawnSync("bash", [join(HARNESS, "rig", "rig.sh"), ...argv], { encoding: "utf8" });
    assert.equal(result.status, 2, `${argv.join(" ")}: ${result.stderr}`);
    assert.match(result.stderr, /label must be a plain file-name token/, argv.join(" "));
  }
});

test("a rig-produced manifest promotes the cell to real-cli-e2e", () => {
  const { tmp, ref, manifest } = rigRun();
  const dir = join(tmp, "harness", "fixtures", "rigtest");
  const at = "2026-01-01T00:00:00.000Z";
  const event = (kind, sourceEvents, capability = "native") => ({ kind, at, capability, sourceEvents });
  cpSync(join(tmp, "harness", "fixtures", "claude"), dir, { recursive: true, filter: (s) => !s.endsWith(".json") });
  writeFileSync(
    join(dir, "rig-stub.json"),
    JSON.stringify(
      {
        fixtureId: "claude/rig-stub",
        cli: "claude",
        nativeVersion: manifest.cliVersion,
        capturedAt: at,
        scenario: "stub lifecycle",
        scenarioId: manifest.scenarioId,
        observedEvents: [
          event("session_started", ["SessionStart"]),
          event("user_prompted", ["UserPromptSubmit"]),
          // Stop から復元する主張なので native ではなく synthesized
          event("assistant_completed", ["Stop"], "synthesized"),
          event("session_ended", ["SessionEnd"]),
        ],
        toolFailurePhasesObserved: [],
        limitations: [],
        limitationCodes: [],
        evidence: [ref],
        rig: { isolated: true, internalRunMarker: true },
      },
      null,
      2,
    ),
  );

  const out = join(tmp, "rigtest.json");
  const run = spawnSync(
    process.execPath,
    ["--experimental-strip-types", join(tmp, "harness", "assemble.ts"), dir, out],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 0, `${run.stdout}\n${run.stderr}`);
  const matrix = JSON.parse(readFileSync(out, "utf8"));
  for (const kind of ["session_started", "user_prompted", "assistant_completed", "session_ended"]) {
    assert.equal(matrix.capabilities.capture[kind].evidenceKind, "real-cli-e2e", `${kind} が昇格していない`);
  }
  // 昇格した cell に「manifest が無い」の caveat が残らない
  assert.ok(!JSON.stringify(matrix.capabilities.capture).includes("no manifest-backed evidence"));
});

test("a CLI that prints more than one version line is rejected", () => {
  const { base, sh } = rigRun();
  // 複数行を返す CLI で黙って 1 行目を採ると、manifest の cliVersion が本当の版と食い違う
  writeFileSync(join(base, "capture", `claude-${LABEL}.version`), "9.9.9-stub (Stub Code)\nwarning: update available\n");
  const again = sh("import", "claude", LABEL, "self.stub");
  assert.notEqual(again.status, 0, "複数行の版で持ち込みが成功した");
  assert.match(again.stderr, /more than one line/);
});

test("a rerun does not start with the previous run's exit status in place", () => {
  // SIGKILL などで新しい .exit が書かれないまま終わると、前回の成功が途中で切れた記録に付く。
  // stub は run 中に .exit を見つけたら痕跡を残すので、消し忘れはここで鳴る
  const { base, sh } = rigRun();
  assert.ok(existsSync(join(base, "capture", `claude-${LABEL}.exit`)), "1 回目で .exit が書かれていない");
  const again = sh("claude-run", LABEL, "hello");
  assert.equal(again.status, 0, again.stderr);
  assert.ok(
    !existsSync(join(base, "capture", `claude-${LABEL}.stale-exit`)),
    "前の run の .exit を持ったまま次の run が始まった",
  );
});

test("a capture is not replaced when a later input fails validation", () => {
  const { base, sh, rawDir } = rigRun();
  const stored = join(rawDir, `claude-${LABEL}.jsonl`);
  const before = readFileSync(stored);
  // 取り込み側の入力を 1 つ壊し、記録のほうは別物にする。検証を複製の後に置いていると、
  // 落ちた時点で保存済みの証拠は上書き済みで、古い manifest が別の byte を指したまま残る
  const capture = join(base, "capture", `claude-${LABEL}.jsonl`);
  writeFileSync(capture, `${readFileSync(capture, "utf8")}{"event":"Stop","at":"2026-01-02T00:00:00.000Z","payload":{"hook_event_name":"Stop"}}\n`);
  writeFileSync(join(base, "capture", `claude-${LABEL}.version`), "9.9.9-stub (Stub Code)\nwarning\n");
  const again = sh("import", "claude", LABEL, "self.stub");
  assert.notEqual(again.status, 0, "壊れた入力で持ち込みが成功した");
  assert.deepEqual(readFileSync(stored), before, "検証で落ちたのに保存済みの記録が置き換わった");
});

test("every run initialisation clears the exit status file the run itself writes", () => {
  // 綴りを固定するのではなく、
  // **書く path と消す path が同じ**ことを突き合わせる: 綴りだけ見る検査は、両方が
  // 揃って間違っている状態（実際にそうなっていた）を素通しする
  const rig = readFileSync(join(HARNESS, "rig", "rig.sh"), "utf8");
  // run ごとに閉じて見る。ファイル全体で照合すると、2 つの run が同じ変数名を使う限り
  // 片方の消し忘れをもう片方の行が隠す
  const blocks = [...rig.matchAll(/^(claude|codex)_run\(\) \{$([\s\S]*?)^\}$/gm)];
  assert.equal(blocks.length, 2, "run 関数の数が変わった");
  for (const [, name, body] of blocks) {
    const written = [...body.matchAll(/printf '%s\\n' "\$rc" > "([^"]+)"/g)].map((m) => m[1]);
    assert.equal(written.length, 1, `${name}_run が終了コードを書く箇所数が変わった`);
    const removals = body.split("\n").filter((l) => l.includes("rm -f "));
    assert.ok(
      removals.some((l) => l.includes(`"${written[0]}"`)),
      `${name}_run が書く ${written[0]} を消している行が無い`,
    );
    assert.equal(body.split("\n").filter((l) => l.includes(': > "$capture"')).length, 1, `${name}_run の初期化が無い`);
  }
});

test("every measured launch is supervised and keeps the lock descriptor", () => {
  // stub で走らせるのは claude 経路だけなので、codex 経路はここで形として見る。
  // 測定対象の起動は 2 つ（--version と本実行）あり、どちらも group ごと畳む。
  // 片方だけ漏れると、その経路の残骸が lock を握って以後の run が止まる
  const rig = readFileSync(join(HARNESS, "rig", "rig.sh"), "utf8");
  const blocks = [...rig.matchAll(/^(claude|codex)_run\(\) \{$([\s\S]*?)^\}$/gm)];
  assert.equal(blocks.length, 2, "run 関数の数が変わった");
  for (const [, name, body] of blocks) {
    assert.equal(
      [...body.matchAll(/reap_group "\$(ver|run)_pid"/g)].length,
      2,
      `${name}_run の起動のうち監督されていないものがある`,
    );
    // fd を渡さないほうが lock は解放されるが、残骸の隣に次の provider の資格情報が置かれる
    assert.ok(!body.includes("9>&-"), `${name}_run が lock の fd を子から外している`);
    // `timeout` は既定で対象を自分の group へ移すので、これが無いと畳む先が空になる。
    // 起動は 2 つ（--version と本実行）あり、`assert.match` は最初の 1 つしか見ない
    assert.equal(
      [...body.matchAll(/timeout --foreground/g)].length,
      2,
      `${name}_run の起動のうち timeout が group を分けているものがある`,
    );
    // 引用を外すと word split が起き、`TERM -- 1 <program>` で timeout の引数を作り直せる
    assert.match(body, /--signal="\$RUN_SIGNAL"/, `${name}_run の signal knob が引用されていない`);
  }
});

test("an uncaught failure leaves no staged file in the evidence store", () => {
  // 複製と読み直しは try の外にあるので、ここで落ちると分類済みの片付けを通らない。
  // 置き場に同名の directory を置くと copyFileSync は EISDIR で落ちる
  const { sh, rawDir } = rigRun({ skipImport: true });
  const staged = join(rawDir, `claude-${LABEL}.jsonl.tmp`);
  mkdirSync(staged, { recursive: true });
  writeFileSync(join(staged, "leftover"), "x");
  const failed = sh("import", "claude", LABEL, "self.stub");
  assert.notEqual(failed.status, 0, "落ちるはずの取り込みが成功した");
  assert.ok(!existsSync(staged), "一時 file が証拠置き場に残った");
});

test("a knob with a space does not turn into another command", () => {
  // 値を 1 語に保っているのは `${VAR:+VAR="$VAR"}` の**内側の引用**。外すと word split が
  // 起き、env は分割後の 2 語目を起動する command と読むので、測定対象ではない program が走る
  const { base, run } = rigRun({ skipImport: true, env: { INJECT_MARKER: "two words" } });
  assert.equal(run.status, 0, run.stderr);
  assert.ok(
    existsSync(join(base, "capture", `claude-${LABEL}.jsonl`)),
    "knob に空白を入れただけで測定が撮れなくなった",
  );
});

test("a signal knob with spaces does not turn into another command", () => {
  // `--signal=$RUN_SIGNAL` を引用しないと word split が起き、`TERM -- 1 <program>` で
  // timeout の「-- duration command」を作り直せる。測定対象ではない program が、
  // 資格情報を置いた隔離環境の中で走る
  const tmp = mkdtempSync(join(tmpdir(), "rig-signal-"));
  SCRATCH.push(tmp);
  const marker = join(tmp, "ran");
  const prog = join(tmp, "prog");
  writeFileSync(prog, `#!/bin/sh\ntouch ${marker}\n`);
  chmodSync(prog, 0o755);
  rigRun({ skipImport: true, expectRunFailure: true, env: { RUN_SIGNAL: `TERM -- 1 ${prog}` } });
  assert.ok(!existsSync(marker), "signal knob の空白で測定対象ではない program が走った");
});

test("setup removes a credential no run owns any more", () => {
  // SIGKILL で trap が走らなければ資格情報は置きっぱなしになる。次に lock を握れた process は
  // 「走っている run はいない」ことを知っているので、そこで消せる
  const { base, sh } = rigRun({ skipRun: true });
  const left = join(base, "claude-config", ".credentials.json");
  writeFileSync(left, "{}");
  assert.equal(sh("setup").status, 0);
  assert.ok(!existsSync(left), "前の run が残した資格情報が rig に残り続けた");
});

test("the rig stages no credential when the test runs", () => {
  // run が終われば trap が消すので、外から見ても常に無い。stub に run 中を見させる
  const { base } = rigRun();
  assert.ok(
    !existsSync(join(base, "capture", `claude-${LABEL}.staged-credential`)),
    "test の run が実 HOME の資格情報を一時 rig へ複製した",
  );
});

test("a run reaps the processes the CLI leaves behind", () => {
  // 測定対象が子を残すのは普通のこと。残骸は lock の fd を持っているので、畳まないと
  // 以後の run と import が全部「別の run が掴んでいる」で止まる
  const { sh } = rigRun({ stubExtra: "sleep 20 </dev/null >/dev/null 2>&1 &", skipImport: true });
  const again = sh("claude-run", LABEL, "hello");
  assert.equal(again.status, 0, `2 回目の run が lock で止まった: ${again.stderr}`);
});

test("the version probe is supervised like the run itself", () => {
  // --version も測定対象の起動なので、そこで残った子も畳む。監督から漏れると、
  // その子が lock の fd を持ったまま残り、以後の run が全部止まる
  const { sh } = rigRun({ stubVersionExtra: "sleep 20 </dev/null >/dev/null 2>&1 &", skipImport: true });
  const again = sh("claude-run", LABEL, "hello");
  assert.equal(again.status, 0, `2 回目の run が lock で止まった: ${again.stderr}`);
});

test("a process that escapes the group is not released from the lock by the rig", () => {
  // `setsid` で group を抜けた process は畳めない。rig 側から fd を外さないので lock は
  // 握られたままになり、次の run は止まる。
  // **保証はここまで**: 残った process が自分で fd 9 を閉じれば次の run は始まり、
  // その run が置く別 provider の資格情報を同じ UID で読める（#95）。同一 UID で走らせる
  // 限りこれは閉じない
  // detach し切る前に run が終わると group kill が間に合ってしまうので、抜けたことを
  // 待ってから stub を終える（race のまま置くと test が気まぐれに緑になる）
  const { sh } = rigRun({ stubExtra: ESCAPED_CHILD, skipImport: true });
  const again = sh("claude-run", LABEL, "hello");
  assert.notEqual(again.status, 0, "group を抜けた残骸がいるのに次の run が始まった");
  assert.match(again.stderr, /another rig run holds/);
});

test("a child that ignores SIGTERM does not wedge the rig", () => {
  // group に残った子が SIGTERM を無視すると lock を握ったままになり、以後の run が全部
  // 止まる。畳めるはずの残骸で可用性を失う形なので、猶予のあとに SIGKILL まで上げる
  const stubborn =
    'sh -c \'trap "" TERM; printf x > "$STEM.stubborn"; sleep 60\' </dev/null >/dev/null 2>&1 &\n' +
    'for _ in 1 2 3 4 5 6 7 8 9 10; do [ -e "$STEM.stubborn" ] && break; sleep 0.2; done';
  const { sh, base } = rigRun({ stubExtra: stubborn, skipImport: true });
  // 起動を待たずに次の run を試すと、子が居ない状態を測って検査が空振りする
  assert.ok(
    existsSync(join(base, "capture", `claude-${LABEL}.stubborn`)),
    "SIGTERM を無視する子が起動していない（検査が空振りする）",
  );
  const again = sh("claude-run", LABEL, "hello");
  assert.equal(again.status, 0, `SIGTERM を無視する残骸で次の run が止まった: ${again.stderr}`);
});

test("teardown does not pull the lock out from under a live run", () => {
  // teardown が lock を無視して消すと、setup が新しい .lock inode を作り、生きた測定対象の
  // 隣へ次の run が資格情報を置ける。直列化と資格情報の分離が同時に外れる
  const { sh, base } = rigRun({ stubExtra: ESCAPED_CHILD, skipImport: true });
  const removed = sh("teardown");
  assert.notEqual(removed.status, 0, "lock を握られたまま rig を消した");
  assert.match(removed.stderr, /another rig run holds/);
  assert.ok(existsSync(join(base, ".lock")), "lock を握られたまま .lock を消した");
});

test("an import that would write an unusable manifest replaces nothing", () => {
  // recorderErrors が 0 でない manifest は検証側が必ず棄却する。持ち込みが成功すると、
  // 保存済みの正しい対を壊したうえで、組み立てが必ず落ちる参照を返すことになる
  const { base, sh, rawDir } = rigRun();
  const stored = join(rawDir, `claude-${LABEL}.jsonl`);
  const storedManifest = join(rawDir, `claude-${LABEL}.manifest.json`);
  const before = { capture: readFileSync(stored), manifest: readFileSync(storedManifest) };
  writeFileSync(join(base, "capture", `claude-${LABEL}.jsonl.errors`), "2026-01-01T00:00:00Z\trecorder-failed rc=1 event=Stop\n");
  const again = sh("import", "claude", LABEL, "self.stub");
  assert.notEqual(again.status, 0, "記録器のエラーが残ったまま持ち込みが成功した");
  assert.deepEqual(readFileSync(stored), before.capture, "落ちたのに保存済みの記録が置き換わった");
  assert.deepEqual(readFileSync(storedManifest), before.manifest, "落ちたのに保存済みの manifest が置き換わった");
});

test("a CLI version the manifest schema rejects does not replace the stored evidence", () => {
  // 取り込み側は「1 行で printable」しか見ていない。manifest schema は 64 文字までなので、
  // 長い版を返す CLI は「持ち込みは成功、組み立ては必ず失敗」を作れてしまう
  const { base, sh, rawDir } = rigRun();
  const stored = join(rawDir, `claude-${LABEL}.jsonl`);
  const before = readFileSync(stored);
  writeFileSync(join(base, "capture", `claude-${LABEL}.version`), `${"9".repeat(70)}\n`);
  const again = sh("import", "claude", LABEL, "self.stub");
  assert.notEqual(again.status, 0, "schema が棄却する版で持ち込みが成功した");
  assert.match(again.stderr, /does not match the schema/);
  assert.deepEqual(readFileSync(stored), before, "落ちたのに保存済みの記録が置き換わった");
});

test("an exit status the rig could not have written is rejected", () => {
  // 綴りの検査（取り込み側）と範囲の検査（manifest schema）は別のものを見ている
  const { base, sh, rawDir } = rigRun();
  const before = readFileSync(join(rawDir, `claude-${LABEL}.manifest.json`));
  writeFileSync(join(base, "capture", `claude-${LABEL}.exit`), `${"9".repeat(30)}\n`);
  const again = sh("import", "claude", LABEL, "self.stub");
  assert.notEqual(again.status, 0, "終了コードでない数で持ち込みが成功した");
  assert.deepEqual(readFileSync(join(rawDir, `claude-${LABEL}.manifest.json`)), before);
  // 桁数だけでは足りない。記録するのは shell の `$?` なので 256 以上はありえない
  writeFileSync(join(base, "capture", `claude-${LABEL}.exit`), "300\n");
  const tooBig = sh("import", "claude", LABEL, "self.stub");
  assert.notEqual(tooBig.status, 0, "255 を超える終了コードで持ち込みが成功した");
  assert.deepEqual(readFileSync(join(rawDir, `claude-${LABEL}.manifest.json`)), before);
  // 逆に、範囲の検査だけでは 0 詰めが通る（`042` は値としては 42 でも rig はその綴りを書かない）
  writeFileSync(join(base, "capture", `claude-${LABEL}.exit`), "042\n");
  const padded = sh("import", "claude", LABEL, "self.stub");
  assert.notEqual(padded.status, 0, "0 詰めの終了コードで持ち込みが成功した");
  assert.deepEqual(readFileSync(join(rawDir, `claude-${LABEL}.manifest.json`)), before);
  // trim してから見ると、前後の空白や余分な改行が正規の綴りへ畳まれる。記録は 1 行そのもの
  writeFileSync(join(base, "capture", `claude-${LABEL}.exit`), " 42\n\n");
  const padded2 = sh("import", "claude", LABEL, "self.stub");
  assert.notEqual(padded2.status, 0, "空白と余分な改行を含む終了コードで持ち込みが成功した");
  assert.deepEqual(readFileSync(join(rawDir, `claude-${LABEL}.manifest.json`)), before);
});

test("a version probe that catches SIGTERM is still cut off", () => {
  // `timeout` は最初の signal のあと待ち続ける。捕まえる・無視する測定対象だと、時間制限を
  // 付けただけでは同じところで固まり、lock と資格情報を握ったままになる。
  // run が非ゼロで終わることだけでは足りない: 止めの signal が無くても、測定対象が自分で
  // 終わった時点で timeout は 124 を返す。**掛かった時間**がこのゲートの唯一の観測点で、
  // 止めの signal が飛ばなければ下の sleep が終わる 30 秒まで帰ってこない
  const started = Date.now();
  const { sh, run } = rigRun({
    stubVersionExtra: 'trap "" TERM; sleep 30',
    skipImport: true,
    expectRunFailure: true,
    env: { VERSION_TIMEOUT: "0.5", VERSION_KILL_AFTER: "0.5s" },
  });
  const elapsed = Date.now() - started;
  assert.notEqual(run.status, 0, "SIGTERM を無視する問い合わせのまま run が成功した");
  assert.ok(elapsed < 15_000, `時間制限のあとに止めの signal が飛んでいない (${elapsed}ms)`);
  const removed = sh("teardown");
  assert.equal(removed.status, 0, `問い合わせが lock を握ったままになった: ${removed.stderr}`);
});

test("a failure while staging leaves both stored files untouched", () => {
  // 検証を全部通ったあとでも書き込みは落ちうる（disk full・権限・race）。置き場を直接
  // 触っていると、そこで落ちた時点で前の対は失われている。manifest の一時 file の場所を
  // directory で塞いで、書き込み側だけを確実に失敗させる
  const { base, sh, rawDir } = rigRun();
  const stored = join(rawDir, `claude-${LABEL}.jsonl`);
  const storedManifest = join(rawDir, `claude-${LABEL}.manifest.json`);
  const before = { capture: readFileSync(stored), manifest: readFileSync(storedManifest) };
  mkdirSync(`${storedManifest}.tmp`);
  // 記録のほうは別物にしておく（置き換わったかどうかが byte で分かる形にする）
  const capture = join(base, "capture", `claude-${LABEL}.jsonl`);
  writeFileSync(
    capture,
    `${readFileSync(capture, "utf8")}{"event":"Stop","at":"2026-01-02T00:00:00.000Z","payload":{"hook_event_name":"Stop"}}\n`,
  );
  const again = sh("import", "claude", LABEL, "self.stub");
  assert.notEqual(again.status, 0, "書き込みに失敗したのに持ち込みが成功した");
  assert.deepEqual(readFileSync(stored), before.capture, "書き込みに失敗したのに記録が置き換わった");
  assert.deepEqual(readFileSync(storedManifest), before.manifest, "書き込みに失敗したのに manifest が置き換わった");
  // 落ちた経路でも一時 file を証拠置き場へ残さない（次の持ち込みが古い複製の隣で始まる）
  assert.ok(!existsSync(`${stored}.tmp`), "書き込みに失敗した run が一時 file を証拠置き場へ残した");
});

test("a diagnostic on stderr is not recorded as the version", () => {
  // stdout に何も出さず stderr に 1 行だけ出して 0 で帰る CLI がある。混ぜて記録していると、
  // その診断文が単一行として通り、cliVersion になる
  const { sh, base } = rigRun({
    stubVersionExtra: `printf 'warning: migrated your config\\n' >&2; exit 0`,
    skipImport: true,
  });
  const imported = sh("import", "claude", LABEL, "self.stub");
  assert.notEqual(imported.status, 0, `stderr の 1 行が版として通った: ${imported.stdout}`);
  assert.equal(readFileSync(join(base, "capture", `claude-${LABEL}.version`), "utf8"), "");
  assert.match(
    readFileSync(join(base, "capture", `claude-${LABEL}.version.err`), "utf8"),
    /warning: migrated your config/,
  );
});

test("a manifest that cannot be replaced puts the previous capture back", () => {
  // rename 2 回は 1 つの操作にできない。1 回目が通って 2 回目が落ちると、退避が無ければ
  // 「新しい記録と古い manifest」で終わり、その前にあった正しい対は戻せない
  const { base, sh, rawDir } = rigRun();
  const stored = join(rawDir, `claude-${LABEL}.jsonl`);
  const storedManifest = join(rawDir, `claude-${LABEL}.manifest.json`);
  const before = readFileSync(stored);
  rmSync(storedManifest);
  mkdirSync(storedManifest); // rename の宛先を directory で塞ぐ（2 回目だけ確実に落ちる）
  const capture = join(base, "capture", `claude-${LABEL}.jsonl`);
  writeFileSync(
    capture,
    `${readFileSync(capture, "utf8")}{"event":"Stop","at":"2026-01-02T00:00:00.000Z","payload":{"hook_event_name":"Stop"}}\n`,
  );
  const again = sh("import", "claude", LABEL, "self.stub");
  assert.notEqual(again.status, 0, "manifest を置き換えられないのに持ち込みが成功した");
  assert.deepEqual(readFileSync(stored), before, "manifest の置き換えに失敗したのに記録だけ入れ替わった");
  assert.ok(!existsSync(`${stored}.prev`), "退避が証拠置き場に残った");
  assert.ok(!existsSync(`${stored}.tmp`), "一時 file が証拠置き場に残った");
  // 失敗の説明に実行環境の path を出さない（file system の error message は絶対 path を含む）
  assert.ok(!again.stderr.includes(base), `失敗の説明に絶対 path が出た: ${again.stderr}`);
  assert.ok(!again.stderr.includes(rawDir), `失敗の説明に絶対 path が出た: ${again.stderr}`);
});

test("a restore that fails keeps the previous record beside the store", () => {
  // 退避を戻せなかった瞬間だけ、前の対は `.prev` にしか残っていない（`dest` は既に手放している）。
  // 片付けがそこまで消すと、手で戻す余地ごと消える。戻せた経路では退避に名前が無いので、
  // 消す対象が残っているのは常に「戻せなかった」ときだけ
  const { base, sh, rawDir } = rigRun();
  const stored = join(rawDir, `claude-${LABEL}.jsonl`);
  const storedManifest = join(rawDir, `claude-${LABEL}.manifest.json`);
  // 記録の側を directory にして、戻す rename（directory → 新しい記録の file）を確実に落とす
  rmSync(stored);
  mkdirSync(stored);
  writeFileSync(join(stored, "keep"), "previous\n");
  rmSync(storedManifest);
  mkdirSync(storedManifest); // 2 回目の rename を落として catch へ入れる
  const again = sh("import", "claude", LABEL, "self.stub");
  assert.notEqual(again.status, 0, "戻せなかったのに持ち込みが成功した");
  assert.equal(
    readFileSync(join(`${stored}.prev`, "keep"), "utf8"),
    "previous\n",
    "戻せなかった退避まで片付けが消した",
  );
  assert.ok(!again.stderr.includes(base), `失敗の説明に絶対 path が出た: ${again.stderr}`);
});

test("the version probe runs in the isolated environment, not the caller's", () => {
  // 隔離の約束は「実 HOME・実設定・実 plugin・実 repository を継承しない」。--version を
  // 素で起動していると、その 1 回だけ約束の外で測定対象が動く（実際に外れていた）。
  // 作業場所も見る: CLI は cwd から上へ設定を探すので、呼び出し元に居るだけで実設定に届く
  const { base } = rigRun({ skipImport: true });
  const seen = readFileSync(join(base, "capture", `claude-${LABEL}.version-probe.jsonl.version-env`), "utf8");
  assert.ok(seen.includes(`HOME=${join(base, "version-state", "home")}\n`), `版の問い合わせが隔離 HOME で走っていない: ${seen}`);
  assert.ok(seen.includes(`CLAUDE_CONFIG_DIR=${join(base, "version-state", "claude-config")}\n`), seen);
  assert.match(seen, /INTERNAL=1\n/, seen);
  assert.ok(seen.includes(`PWD=${join(base, "version-state", "workspace")}\n`), `版の問い合わせが呼び出し元の作業場所で走っている: ${seen}`);
  // HOME を差し替えても /etc/gitconfig は読まれる。測定対象が起動する git が host の
  // core.hooksPath や includes を拾うと、隔離の外の設定が記録を変える
  assert.match(seen, /GITSYS=1\n/, `測定対象の環境で system の git 設定が遮断されていない: ${seen}`);
  // 使い捨ての state を本実行と共有しない。共有すると、初回起動で設定を書く CLI が
  // 本実行を 2 回目の起動にしてしまう
  assert.ok(!existsSync(join(base, "version-state")), "問い合わせ用の state が残っている");
});

test("a version probe that never returns does not hold the rig", () => {
  // 更新待ちなどで --version が固まると、時間制限が無ければ lock と資格情報を握ったまま
  // 帰らず、以後の run・import・teardown が全部止まる
  const { sh, run } = rigRun({
    stubVersionExtra: "sleep 30",
    skipImport: true,
    expectRunFailure: true,
    env: { VERSION_TIMEOUT: "1" },
  });
  assert.notEqual(run.status, 0, "固まった問い合わせのまま run が成功した");
  assert.match(run.stderr, /claude --version failed/, run.stderr);
  const again = sh("teardown");
  assert.equal(again.status, 0, `固まった問い合わせのあとで rig が握られたままになった: ${again.stderr}`);
});

test("a run that cannot take the lock leaves the holder's credentials alone", () => {
  // lock を取れずに降りる process まで資格情報を消すと、走っている run の認証を横から壊す
  const { sh, base } = rigRun({ stubExtra: ESCAPED_CHILD, skipImport: true });
  const staged = join(base, "claude-config", ".credentials.json");
  writeFileSync(staged, '{"token":"held-by-the-running-rig"}');
  const blocked = sh("claude-run", LABEL, "hello");
  assert.notEqual(blocked.status, 0, "lock を握られているのに次の run が始まった");
  assert.ok(existsSync(staged), "lock を取れなかった run が、握っている側の資格情報を消した");
  // 競合の説明に実行環境の絶対 path を出さない（FR-015）。置き場を決めたのは呼んだ側なので、
  // path を書いても分かることは増えない
  assert.match(blocked.stderr, /another rig run holds/);
  assert.ok(!blocked.stderr.includes(base), `lock 競合の説明に絶対 path が出た: ${blocked.stderr}`);
});

test("a measured run that catches SIGTERM is still cut off", () => {
  // 版問い合わせと同じ穴が測定側にもある。SIGTERM を捕まえる・無視する測定対象だと timeout は
  // 最初の signal のあと待ち続け、`wait` が帰らないので reap_group まで届かない——lock と
  // staged な資格情報を握ったまま rig が止まる。ここでも観測点は掛かった時間だけ
  const started = Date.now();
  const { sh } = rigRun({
    stubExtra: 'trap "" TERM; sleep 30',
    skipImport: true,
    env: { RUN_TIMEOUT: "0.5", RUN_KILL_AFTER: "0.5s" },
  });
  const elapsed = Date.now() - started;
  assert.ok(elapsed < 15_000, `測定の時間制限のあとに止めの signal が飛んでいない (${elapsed}ms)`);
  const removed = sh("teardown");
  assert.equal(removed.status, 0, `測定が lock を握ったままになった: ${removed.stderr}`);
});

test("setup refuses to rewrite the rig's state under a held lock", () => {
  // setup だけが lock を取らずに provider の設定を書き換えていた。走っている測定の足元で
  // hook の無い settings.json に差し替わっても、その記録の digest は合うので証拠として通る
  const { sh, base } = rigRun({ stubExtra: ESCAPED_CHILD, skipImport: true });
  const settings = join(base, "claude-config", "settings.json");
  writeFileSync(settings, '{"marker":"written-by-the-running-rig"}');
  const again = sh("setup");
  assert.notEqual(again.status, 0, "lock を握られているのに setup が通った");
  assert.match(readFileSync(settings, "utf8"), /written-by-the-running-rig/, "setup が走っている run の設定を書き換えた");
});

test("setup leaves the lock file in place", () => {
  // teardown 側で「あれば取る」にすると、無い瞬間を見た直後に run が作って握る隙間ができる。
  // run を通さずに setup だけで見る（run が作った file を見ても setup の検査にならない）
  const { base } = rigRun({ skipRun: true });
  assert.ok(existsSync(join(base, ".lock")), "setup が lock file を作っていない");
});

test("a version probe that fails is not accepted as the version behind the evidence", () => {
  // 非ゼロで終えた問い合わせでも、1 行だけ吐けば版として schema を通る。run を続けると
  // そのエラー行が cliVersion に載り、証拠が「この版で測った」と読める
  const { run, base } = rigRun({
    stubVersionExtra: `printf '%s\\n' '${VERSION}'; exit 7`,
    skipImport: true,
    expectRunFailure: true,
  });
  assert.notEqual(run.status, 0, `版の問い合わせが失敗したのに run が成功した: ${run.stdout}`);
  assert.match(run.stderr, /claude --version failed \(exit=7\)/, run.stderr);
  assert.ok(
    !existsSync(join(base, "capture", `claude-${LABEL}.exit`)),
    "版の問い合わせが失敗したのに測定を続けた",
  );
});

test("the rig does not print where it lives", () => {
  // FR-015 は診断出力に実行環境の絶対 path を載せることを禁じている。置き場を決めたのは
  // 呼んだ側なので、成功の報告に絶対 path を書いても分かることは増えない。
  // 失敗側（lock 競合・持ち込みの失敗）は別の test で見ているので、ここは成功側だけを見る
  const { setup, run, base } = rigRun({ skipImport: true });
  assert.ok(!setup.stdout.includes(base), `setup の報告に絶対 path が出た: ${setup.stdout}`);
  assert.ok(!run.stdout.includes(base), `run の報告に絶対 path が出た: ${run.stdout}`);
});

test("both runs report the capture by name, not by where it lives", () => {
  // stub で走らせるのは claude 経路だけなので、codex 経路はここで形として見る。
  // 2 つの run は同じ形で報告する。片方だけ絶対 path へ戻す変更が入ると、実行しない側の
  // FR-015 違反は誰も見ないまま出荷される
  const rig = readFileSync(join(HARNESS, "rig", "rig.sh"), "utf8");
  const blocks = [...rig.matchAll(/^(claude|codex)_run\(\) \{$([\s\S]*?)^\}$/gm)];
  assert.equal(blocks.length, 2, "run 関数の数が変わった");
  for (const [, name, body] of blocks) {
    assert.match(body, /echo "captured: \$\{capture##\*\/\}/, `${name}_run の報告が名前だけになっていない`);
  }
});

test("teardown that cannot take the lock removes nothing", () => {
  // 「あれば取る」にすると、無い瞬間を見た直後に run が作った base を lock 無しで消せる。
  // その順序は race なので直接は再現できない。**lock を取れないなら成功を報告しない**ほうを
  // 固定する: 置き場を作れなければ lock も取れないので、そこで降りる
  // 権限では止められない（root では 0500 の親にも作れる）。file の下に directory は作れないので、
  // UID に依らず「置き場を作れない」状態になる
  const tmp = mkdtempSync(join(tmpdir(), "rig-teardown-"));
  SCRATCH.push(tmp);
  const blocker = join(tmp, "not-a-directory");
  writeFileSync(blocker, "");
  const done = spawnSync("bash", [join(HARNESS, "rig", "rig.sh"), "teardown"], {
    encoding: "utf8",
    env: { ...process.env, RIG_BASE: join(blocker, "rig-base") },
  });
  assert.notEqual(done.status, 0, `lock を取れないのに teardown が消したと報告した: ${done.stdout}`);
});

// 測定用 workspace を作る git が実環境から切り離されているか。漏れると operator の hook が
// 隔離したはずの測定の中で走り、それでも manifest は isolated: true を書く
test("the measured workspace is built without the operator's environment", () => {
  const tmp = mkdtempSync(join(tmpdir(), "rig-gitenv-"));
  SCRATCH.push(tmp);
  const hooks = join(tmp, "hooks");
  mkdirSync(hooks, { recursive: true });
  const preCommit = join(hooks, "pre-commit");
  writeFileSync(preCommit, "#!/bin/sh\nexit 1\n");
  chmodSync(preCommit, 0o755);
  const home = join(tmp, "home");
  mkdirSync(home, { recursive: true });
  writeFileSync(join(home, ".gitconfig"), `[core]\n\thooksPath = ${hooks}\n`);
  const template = join(tmp, "template");
  mkdirSync(template, { recursive: true });
  writeFileSync(join(template, "MARKER"), "leaked\n");
  const foreign = join(tmp, "foreign.git");
  const base = join(tmp, "base");
  // 設定の入口を一度に全部渡す。塞いでいるのは 1 つ（渡す環境を決めること）なので、
  // どれか 1 つでも通れば同じ穴が開く
  const done = spawnSync("bash", [join(HARNESS, "rig", "rig.sh"), "setup"], {
    encoding: "utf8",
    env: {
      ...process.env,
      HOME: home,
      RIG_BASE: base,
      GIT_DIR: foreign,
      GIT_TEMPLATE_DIR: template,
      GIT_CONFIG_COUNT: "1",
      GIT_CONFIG_KEY_0: "core.hooksPath",
      GIT_CONFIG_VALUE_0: hooks,
      GIT_CONFIG_PARAMETERS: `'core.hooksPath=${hooks}'`,
    },
  });
  assert.equal(done.status, 0, `operator の環境から渡した hook が隔離した commit を止めた: ${done.stderr}`);
  assert.ok(existsSync(join(base, "workspace", ".git")), "measured workspace の repository が別の場所に作られた");
  assert.ok(!existsSync(foreign), "GIT_DIR の指す repository を触った");
  assert.ok(!existsSync(join(base, "workspace", ".git", "MARKER")), "operator の template が .git へ複製された");
});

test("the measured workspace carries none of git's default hooks", () => {
  // `--template=` を外すと git 自身の既定 template（`hooks/*.sample`）が .git に並ぶ。
  // 環境を切り離しても既定は残るので、空の template は別に明示する
  const { base } = rigRun({ skipRun: true });
  const dir = join(base, "workspace", ".git", "hooks");
  assert.deepEqual(existsSync(dir) ? readdirSync(dir) : [], [], "測定用 workspace の .git に hook が並んだ");
});

test("neither setup nor teardown touches a base the rig did not create", () => {
  // RIG_BASE を決めるのは呼んだ側で、teardown は中身ごと消す。既にある directory をそのまま
  // 扱うと、打ち間違い 1 つで無関係な木が消え、権限まで変わる
  const tmp = mkdtempSync(join(tmpdir(), "rig-foreign-"));
  SCRATCH.push(tmp);
  const foreign = join(tmp, "not-a-rig");
  mkdirSync(foreign);
  writeFileSync(join(foreign, "keep"), "mine\n");
  chmodSync(foreign, 0o755);
  const rigCmd = (cmd) =>
    spawnSync("bash", [join(HARNESS, "rig", "rig.sh"), cmd], {
      encoding: "utf8",
      env: { ...process.env, RIG_BASE: foreign },
    });
  const down = rigCmd("teardown");
  assert.notEqual(down.status, 0, "rig が作っていない置き場を teardown が消したと報告した");
  assert.ok(existsSync(join(foreign, "keep")), "rig が作っていない置き場の中身が消えた");
  const up = rigCmd("setup");
  assert.notEqual(up.status, 0, "rig が作っていない置き場を setup が使った");
  assert.equal(statSync(foreign).mode & 0o777, 0o755, "rig が作っていない置き場の権限を変えた");
});

test("an import does not start while a previous record is set aside", () => {
  // 退避が残っている = 前回が戻せずに落ちた。そのまま始めると、唯一残った前の対を上書きする
  const { sh, rawDir } = rigRun();
  const stored = join(rawDir, `claude-${LABEL}.jsonl`);
  const storedManifest = join(rawDir, `claude-${LABEL}.manifest.json`);
  const before = { capture: readFileSync(stored), manifest: readFileSync(storedManifest) };
  writeFileSync(`${stored}.prev`, "the last good capture\n");
  const again = sh("import", "claude", LABEL, "self.stub");
  assert.notEqual(again.status, 0, "復旧待ちの退避があるのに持ち込みが始まった");
  assert.equal(
    readFileSync(`${stored}.prev`, "utf8"),
    "the last good capture\n",
    "復旧待ちの退避を上書きした",
  );
  assert.deepEqual(readFileSync(stored), before.capture, "退避が残っているのに記録が置き換わった");
  assert.deepEqual(readFileSync(storedManifest), before.manifest, "退避が残っているのに manifest が置き換わった");
});

test("a label the importer would refuse never starts a measured run", () => {
  // 取り込みが必ず落とす綴りで測定を始めると、CLI の起動 1 回分がまるごと捨てになる
  const { base, sh } = rigRun({ skipRun: true });
  const bad = sh("claude-run", "my run", "hello");
  assert.notEqual(bad.status, 0, `import が落とす label で測定が走った: ${bad.stdout}`);
  assert.deepEqual(readdirSync(join(base, "capture")), [], "捨てるしかない記録が残った");
});

test("both runs check the label before starting a measured run", () => {
  // stub で走らせるのは claude 経路だけなので、codex 経路はここで形として見る
  const rig = readFileSync(join(HARNESS, "rig", "rig.sh"), "utf8");
  const blocks = [...rig.matchAll(/^(claude|codex)_run\(\) \{$([\s\S]*?)^\}$/gm)];
  assert.equal(blocks.length, 2, "run 関数の数が変わった");
  for (const [, name, body] of blocks) {
    assert.match(body, /require_label "\$label"/, `${name}_run が label を検査していない`);
  }
});

test("run_env refuses a CLI it has no isolated config for", () => {
  // 設定 dir を渡さずに起動すると、測定対象は既定の場所（= 実環境）を見に行く。隔離の外で
  // 測ったのに、記録も manifest も同じ形で残る。rig.sh の外から run_env だけを呼ぶ口は
  // 無いので、ここは形として見る
  const rig = readFileSync(join(HARNESS, "rig", "rig.sh"), "utf8");
  const body = rig.match(/^run_env\(\) \{[\s\S]*?^\}$/m)?.[0];
  assert.ok(body, "run_env が見つからない");
  assert.match(body, /\*\)[^\n]*exit 2/, "run_env に既定分岐が無い（知らない cli を隔離無しで起動する）");
});

test("the workspace's git does not come from the caller's path", () => {
  // 隔離は環境を畳むが、畳む**前**の PATH で git を探していると、呼び出し元が先頭に置いた
  // program が「隔離した」workspace を作る。それでも manifest は isolated: true を書く
  const fake = mkdtempSync(join(tmpdir(), "rig-fakegit-"));
  SCRATCH.push(fake);
  const marker = join(fake, "used");
  writeFileSync(join(fake, "git"), `#!/usr/bin/env bash\nprintf x > ${JSON.stringify(marker)}\nexit 0\n`);
  chmodSync(join(fake, "git"), 0o755);

  const { base } = rigRun({ skipRun: true, env: { PATH: `${fake}:${process.env.PATH}` } });

  assert.equal(existsSync(marker), false, "呼び出し元の PATH に置いた git が使われた");
  assert.ok(existsSync(join(base, "workspace", ".git")), "workspace が git repository になっていない");
});

test("a failed version probe leaves the previous capture in place", () => {
  // 版の問い合わせは測定の**前**に落ちうる。そこで前の run の記録を消していると、
  // 始まってもいない測定のために、取り込み前の記録が復旧不能に失われる
  const { base, sh } = rigRun({
    skipImport: true,
    // 問い合わせも測定対象の起動なので hook が動きうる。記録先を本実行と共有していると、
    // その行が**前の run の記録**へ書き足される——落ちる問い合わせでは消す段まで進まないので、
    // 取り込み前の記録がそのまま汚れて残る。ここが分離を観測できる唯一の入力
    stubVersionExtra:
      `printf '%s\\n' '{"event":"SessionStart","at":"2026-01-01T00:00:00.000Z","payload":{"hook_event_name":"SessionStart","forged":"version-probe"}}' >> "$CAPTURE_FILE"
` +
      'if [ -e "$(dirname "${CAPTURE_FILE}")/fail-version" ]; then echo boom >&2; exit 3; fi',
  });
  const stem = join(base, "capture", `claude-${LABEL}`);
  const before = readFileSync(`${stem}.jsonl`);
  assert.ok(before.length > 0, "1 回目の記録が空");
  const version = readFileSync(`${stem}.version`, "utf8");

  writeFileSync(join(base, "capture", "fail-version"), "");
  const second = sh("claude-run", LABEL, "hello");

  assert.notEqual(second.status, 0, "落ちた問い合わせで run が成功している");
  assert.deepEqual(readFileSync(`${stem}.jsonl`), before, "前の記録が失われた");
  assert.ok(!readFileSync(`${stem}.jsonl`, "utf8").includes("version-probe"), "問い合わせが書いた行が前の記録に混ざった");
  assert.ok(existsSync(`${stem}.exit`), "前の run の終了コードが消えた");
  assert.equal(readFileSync(`${stem}.version`, "utf8"), version, "前の版が上書きされた");
});

test("both runs replace the previous capture only after the version probe", () => {
  // stub で走らせるのは claude 経路だけなので、codex 経路はここで形として見る
  const rig = readFileSync(join(HARNESS, "rig", "rig.sh"), "utf8");
  const blocks = [...rig.matchAll(/^(claude|codex)_run\(\) \{$([\s\S]*?)^\}$/gm)];
  assert.equal(blocks.length, 2, "run 関数の数が変わった");
  for (const [, name, body] of blocks) {
    const probe = body.indexOf('[ "$ver_rc" -eq 0 ]');
    const wipe = body.indexOf(': > "$capture"');
    assert.ok(probe >= 0 && wipe >= 0, `${name}_run の問い合わせか記録の初期化が見つからない`);
    assert.ok(wipe > probe, `${name}_run が問い合わせより先に前の記録を消している`);
  }
});

test("the rig does not run shell functions the caller exported", () => {
  // export した関数は環境で引き継がれ、PATH より先に選ばれる。隔離を作る側の command が
  // それだと、workspace を作る git も環境を畳む env も呼び出し元の code になる
  const tmp = mkdtempSync(join(tmpdir(), "rig-fn-"));
  SCRATCH.push(tmp);
  const marker = join(tmp, "ran");
  const body = `() { printf x > ${JSON.stringify(marker)}; }`;
  const base = join(tmp, "base");
  const done = spawnSync("bash", [join(HARNESS, "rig", "rig.sh"), "setup"], {
    encoding: "utf8",
    // 隔離を作る側が使う command を一度に全部差し替える（塞いでいるのは 1 つなので、
    // どれか 1 つでも通れば同じ穴が開く）
    env: {
      ...process.env,
      HOME: join(tmp, "home"),
      RIG_BASE: base,
      "BASH_FUNC_git%%": body,
      "BASH_FUNC_env%%": body,
      "BASH_FUNC_sed%%": body,
    },
  });

  assert.equal(done.status, 0, done.stderr);
  assert.equal(existsSync(marker), false, "呼び出し元が export した関数が rig の中で走った");
  assert.ok(existsSync(join(base, "workspace", ".git")), "workspace が git repository になっていない");
});

test("the env that folds the environment away does not come from the caller's path", () => {
  // `env -i` は環境を畳む側そのもの。これを呼び出し元の PATH から取ると、畳んだつもりの
  // 起動が呼び出し元の program になる（それでも manifest は isolated: true を書く）
  const fake = mkdtempSync(join(tmpdir(), "rig-fakeenv-"));
  SCRATCH.push(fake);
  const marker = join(fake, "used");
  // 目印を置いてから本物へ渡す。置き換えたことだけが観測でき、rig は普段どおり動く
  writeFileSync(join(fake, "env"), `#!/bin/bash\nprintf x >> ${JSON.stringify(marker)}\nexec /usr/bin/env "$@"\n`);
  chmodSync(join(fake, "env"), 0o755);

  // setup（workspace を作る git）と run（測定対象）の両方を通す
  rigRun({ skipImport: true, env: { PATH: `${fake}:${process.env.PATH}` } });

  assert.equal(existsSync(marker), false, "呼び出し元の PATH に置いた env が使われた");
});

test("both runs invalidate the previous capture before publishing the new version", () => {
  // stub で走らせるのは claude 経路だけなので、codex 経路はここで形として見る。
  // 逆順だと、その間に落ちた瞬間だけ「前の記録 + 新しい版」が揃い、取り込みが前の run の
  // 記録を新しい版で測ったことにできる
  const rig = readFileSync(join(HARNESS, "rig", "rig.sh"), "utf8");
  const blocks = [...rig.matchAll(/^(claude|codex)_run\(\) \{$([\s\S]*?)^\}$/gm)];
  assert.equal(blocks.length, 2, "run 関数の数が変わった");
  for (const [, name, body] of blocks) {
    const wipe = body.indexOf(': > "$capture"');
    const publish = body.indexOf('mv "$stem.version.new"');
    assert.ok(wipe >= 0 && publish >= 0, `${name}_run の記録の初期化か版の公開が見つからない`);
    assert.ok(wipe < publish, `${name}_run が前の記録を無効にする前に新しい版を公開している`);
  }
});

test("the workspace's git is not handed the coordination lock", () => {
  // 測定対象には fd を渡す（残骸が lock を握って次の run を止めるのが望ましい）が、rig 自身の
  // 道具は別。git は commit のあと維持作業を切り離すことがあり、その子が握ったまま残ると、
  // setup の直後の run が理由の分からない「別の run が走っている」で止まる
  const rig = readFileSync(join(HARNESS, "rig", "rig.sh"), "utf8");
  const body = rig.match(/^git_iso\(\) \{[\s\S]*?^\}$/m)?.[0];
  assert.ok(body, "git_iso が見つからない");
  assert.match(body, /9>&-/, "workspace を作る git に lock の fd を渡している");
});
