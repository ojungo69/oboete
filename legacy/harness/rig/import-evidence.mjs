// rig が取った観測記録を証拠置き場へ byte 同一で持ち込み、run の素性を manifest として書く。
//
// digest は持ち込んだ**後**の byte から取る。持ち込む前に取ると、持ち込みで内容が変わっても
// 気づけない。byte 同一そのものは複製直後の突き合わせで別に見る。
// SHA-256 は harness/evidence/normalize.ts の実装だけを使う（sha256sum を別に呼ばない）。
import { copyFileSync, existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import { parseArgs } from "node:util";
import { fileURLToPath } from "node:url";
import { NORMALIZATION_VERSION, captureCapturedAt, digestCapture, digestRaw } from "../evidence/normalize.ts";
import { MANIFEST_VERSION } from "../evidence/verify.ts";
import { readIJsonFile } from "../schema/jcs.ts";
import { validateAgainstSchema } from "../schema/validate.ts";

const MANIFEST_SCHEMA = readIJsonFile(new URL("../schema/evidence-manifest.schema.json", import.meta.url));
const KNOWN_CLIS = new Set(["claude", "codex"]);
const SCENARIO_ID = /^[a-z0-9]+(?:[.-][a-z0-9]+)*$/;
const LABEL = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;
const PRINTABLE = /^[\x20-\x7e]+$/;

const die = (msg) => {
  console.error(`import-evidence: ${msg}`);
  process.exit(2);
};

/** `.version` は単一行として読む。複数行を返す CLI は黙って 1 行目を採らずに失敗させる */
function readCliVersion(path) {
  let text;
  try {
    text = readFileSync(path, "utf8");
  } catch {
    die(`cannot read the version file: ${basename(path)}`);
  }
  const line = text.replace(/\r?\n$/, "");
  if (line.includes("\n")) die("the CLI printed more than one line for --version");
  if (!PRINTABLE.test(line)) die("the CLI version is empty or not printable ASCII");
  return line;
}

function readExitStatus(path) {
  let text;
  try {
    text = readFileSync(path, "utf8");
  } catch {
    die("the run did not record an exit status");
  }
  // 縛るのは**綴り**そのもの。記録は `printf '%s\n' "$rc"` の 1 行なので、0 詰め（`042`）も
  // 前後の空白もありえない（来たなら書いたのは rig ではない）。trim してから見ると、それらが
  // 正規の綴りへ畳まれて通る。
  // **値の範囲**（`$?` は下位 8 bit しか持てない）は manifest schema の `maximum: 255` が見る。
  // 桁数の上限をここへ置いても、`042` を弾く 0 詰めの規則と schema の範囲で塞がっている分を
  // なぞるだけで、単独で拒める入力が 1 つも無い（実測で確認済み）。手で書いた manifest は
  // 取り込みを通らないので、範囲の門は検証側にしか置けない
  if (!/^(?:0|[1-9]\d*)\n$/.test(text)) die("the recorded exit status is not a plausible exit code");
  return Number(text);
}

// 未知の option は parseArgs 自身が弾く。素の例外は stack を吐くので die へ寄せる
let args;
try {
  ({ values: args } = parseArgs({
    args: process.argv.slice(2),
    options: { cli: { type: "string" }, label: { type: "string" }, "scenario-id": { type: "string" }, from: { type: "string" } },
  }));
} catch {
  die("usage: --cli <claude|codex> --label <label> --scenario-id <id> --from <capture-dir>");
}
const cli = args.cli;
const label = args.label;
const scenarioId = args["scenario-id"];
if (!KNOWN_CLIS.has(cli)) die("--cli must be claude or codex");
if (!LABEL.test(label ?? "")) die("--label must be a plain file-name token");
if (!SCENARIO_ID.test(scenarioId ?? "")) die("--scenario-id does not match the schema pattern");
if (!args.from) die("--from is required");

// fs や正規化の未捕捉例外は絶対 path 入りの stack を出す。分類済みの理由へ寄せる。
// 一時 file の名前が決まったあとに落ちることもある（複製と読み直しは try の外にあり、
// そこで落ちると途中まで書かれた .tmp が証拠置き場に残る）ので、片付けも通す
let cleanupStaged = () => {};
process.on("uncaughtException", (e) => {
  cleanupStaged();
  die(`import failed: ${e?.constructor?.name ?? "Error"}`);
});

const stem = `${cli}-${label}`;
const source = join(args.from, `${stem}.jsonl`);
// 置き場は module からの相対で固定する。差し替え口は作らない（test は harness ごと複製する）
const destDir = join(fileURLToPath(new URL("../fixtures/", import.meta.url)), cli, "raw");
const dest = join(destDir, `${stem}.jsonl`);
const manifestPath = join(destDir, `${stem}.manifest.json`);

if (!existsSync(source)) die(`no capture at ${stem}.jsonl`);

// **置き換える前に**入力を全部検証し、manifest まで組み立てる。1 つでも後段に残すと、
// そこで落ちたときには既に前の正しい証拠を壊しており、古い manifest が別の byte を指したまま残る
const sourceBytes = readFileSync(source);
const sourceRawHash = digestRaw(sourceBytes);
const sourceHash = digestCapture(sourceBytes);
// capturedAt は rig が別に持つ時刻ではなく、記録の 1 行目の at。こうすると
// captureRawHash がこの値まで縛る。**検証側と同じ関数**を通す（別実装にすると片方だけ緩む）
let capturedAt;
try {
  capturedAt = captureCapturedAt(sourceBytes);
} catch (e) {
  die(`the capture has no usable first line (${e.message})`);
}
const errorsFile = `${source}.errors`;
const recorderErrors = existsSync(errorsFile)
  ? readFileSync(errorsFile, "utf8").split("\n").filter((l) => l.trim() !== "").length
  : 0;

const manifest = {
  manifestVersion: MANIFEST_VERSION,
  cli,
  cliVersion: readCliVersion(join(args.from, `${stem}.version`)),
  scenarioId,
  capturedAt,
  isolated: true,
  internalRunMarker: true,
  exitStatus: readExitStatus(join(args.from, `${stem}.exit`)),
  recorderErrors,
  capture: basename(dest),
  captureRawHash: sourceRawHash,
  captureHash: sourceHash,
  normalizationVersion: NORMALIZATION_VERSION,
};

// 組み上げた manifest が**検証側の要求を満たすことまで**先に見る。ここを後回しにすると、
// 検証側が必ず棄却する manifest（schema 違反・記録器のエラーあり）を書くために、
// 前の正しい記録と manifest の対を壊してから失敗する
const issues = validateAgainstSchema(manifest, MANIFEST_SCHEMA, MANIFEST_SCHEMA);
// 診断は場所だけ。schema が棄却した値をそのまま出すと、その値が CI ログへ出る
if (issues.length > 0) die(`the manifest does not match the schema at: ${issues.map((i) => i.path).join(", ")}`);
// 検証側は recorderErrors === 0 を要求する。0 でない記録は証拠にならないので持ち込まない
if (manifest.recorderErrors !== 0) die("the recorder logged errors during this run; it cannot back a promotion");

// 置き場と同じ directory の一時 file へ両方そろえてから、rename 2 回で差し替える。
// dest を直接触ると、複製後の読み直しや manifest の書き込みで落ちたときに前の対が残らない。
// rename の間で落ちた場合だけは対が食い違うが、その形は digest が合わないので検証は
// fail closed になる。
// **直列化の範囲**: lock は RIG_BASE 単位で、置き場はその外で共有する。別々の RIG_BASE から
// 同じ label を同時に持ち込むと一時 file の名前が重なり、片方が他方の staged file を消して
// 持ち込みを失敗させうる（置いてある対は壊れない。対が入れ替わった形も上と同じく検証で落ちる）。
// 置き場を跨いで直列化するには置き場側の lock が要るが、この rig は 1 人が順に回す道具なので
// 取っていない
mkdirSync(destDir, { recursive: true });
// 退避が残っている = 前回の持ち込みが戻せずに落ちた。そのまま始めると下の rename が
// **唯一残った前の対**を上書きする。人が中身を確かめて片付けるまで、ここで止まる
if (existsSync(`${dest}.prev`)) {
  die("a previous record is still set aside for recovery; resolve it before importing again");
}
const stagedCapture = `${dest}.tmp`;
const stagedManifest = `${manifestPath}.tmp`;
const removeStaged = () => {
  // 塞がれた側が directory のこともある（そこで落ちると片付け自体が二次障害になる）
  for (const f of [stagedCapture, stagedManifest]) rmSync(f, { force: true, recursive: true });
  // 退避（`.prev`）はここでは消さない。戻せた経路では既に名前が無いので、消す対象が残っているのは
  // **戻せなかった**ときだけで、その瞬間 `dest` は空か新しい記録で、前の対はその退避にしか無い
};
// 分類済みの理由で落ちる経路と、未捕捉例外の経路で同じ片付けを通す
cleanupStaged = removeStaged;
/** 一時 file を残さずに落ちる。`die` は process.exit なので finally では片付かない */
const dieStaged = (msg) => {
  removeStaged();
  die(msg);
};

copyFileSync(source, stagedCapture);
const bytes = readFileSync(stagedCapture);
if (!sourceBytes.equals(bytes)) dieStaged("the copy is not byte-identical to the capture");
// 複製の側からも取り直して突き合わせる（byte 比較と digest の両方が一致して初めて同一）
const captureRawHash = digestRaw(bytes);
const captureHash = digestCapture(bytes);
if (captureRawHash !== sourceRawHash || captureHash !== sourceHash) {
  dieStaged("the copy does not digest to the capture");
}
// 置き換えそのものが失敗したときも、一時 file を証拠置き場へ残さない。
// 2 回の rename は 1 つの操作にできないので、先に古い記録を退避しておく。片方だけ入れ替わった
// 状態（新しい記録と古い manifest）で終わると、対としては digest が合わず検証で落ちるが、
// **その前にあった正しい対まで失われる**。退避があれば元へ戻せる
const previous = existsSync(dest) ? `${dest}.prev` : null;
if (previous) renameSync(dest, previous);
try {
  writeFileSync(stagedManifest, `${JSON.stringify(manifest, null, 2)}\n`);
  renameSync(stagedCapture, dest);
  renameSync(stagedManifest, manifestPath);
} catch (e) {
  if (previous) renameSync(previous, dest);
  // 失敗の説明に絶対 path を出さない。file system の error message は source と destination の
  // 絶対 path を含むので、そのまま出すと CI log へ実行環境の path が流れる
  dieStaged(`import failed while staging: ${e?.constructor?.name ?? "Error"}`);
}
if (previous) rmSync(previous, { force: true });

// fixture の evidence[] へそのまま貼れる形で出す（digest を手で写させない）
process.stdout.write(
  `${JSON.stringify(
    {
      path: basename(dest),
      evidenceHash: captureHash,
      captureRawHash,
      normalizationVersion: NORMALIZATION_VERSION,
      manifest: basename(manifestPath),
      manifestHash: digestRaw(readFileSync(manifestPath)),
    },
    null,
    2,
  )}\n`,
);
