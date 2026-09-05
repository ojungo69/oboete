#!/usr/bin/env node
// 公開 tarball に、bundle された第三者コードの notice が載っているかを検査する。
//
// build 済みかどうかで分岐しない。自分で install と build と pack を行い、実際の tarball の
// 中身だけを見る。「build 済みなら検査する」形にすると、build 順序に依存して黙って素通りする。
//
// 期待する依存名と、その license 本文の digest は harness/notice-baseline.json に
// **完全な集合として**固定してある。名指しの数件だけを要求する形は採らない: 生成が部分的に
// 退行して他が落ちても通ってしまうため（実測で `marked` 1 件の欠落が素通りすることを確認済み）。
// 本文を digest で固定するのは、非空かどうかしか見ないと本文が別物に差し替わっても通るため。
// 依存が正当に増減したとき・license 本文が変わったときは `--write-baseline` で再生成し、
// その差分を commit に載せてレビューする（harness/contract-hashes.json と同じ運用）。
//
// usage:
//   node harness/notice-inclusion-check.mjs                  # 検査
//   node harness/notice-inclusion-check.mjs --write-baseline # baseline を再生成

import {
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const NO_BUNDLED_DEPENDENCIES = "No third-party code is bundled in this artifact.";
const ENTRY_MARKER = "<!-- codemem:dependency -->";
const LICENSE_MARKER = "<!-- codemem:license-text -->";
const LICENSE_END_MARKER = "<!-- codemem:end-license-text -->";
const NOTICE_FILE_PATTERN = /^THIRD_PARTY_NOTICES.*\.md$/;

const PACKAGES = [
  { name: "codemem", directory: "packages/cli" },
  { name: "@codemem/core", directory: "packages/core" },
  { name: "@codemem/mcp", directory: "packages/mcp-server" },
  { name: "@codemem/server", directory: "packages/viewer-server" },
  // rollup build を通らない（出荷する JS が生成物ではなく commit 済みの原本）ため、module graph
  // 由来の notice を作れない。代わりに「何も bundle していない」と述べる notice を package 内に
  // 置いて同梱し、他と同じように tarball を検査する。原本が変わればその diff がレビューに載る
  // ので、bundle され始めた場合はそこで気付ける。詳細は evidence/adr-004-licensing.md。
  { name: "@codemem/opencode-plugin", directory: "packages/opencode-plugin" },
];

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const vendorRoot = join(repositoryRoot, "vendor", "codemem");
const baselinePath = join(repositoryRoot, "harness", "notice-baseline.json");

// license 本文がどこにあるかの定義はここ 1 箇所。digest を取る側と形の検査をする側で別々に
// 切り出していると、片方だけ直したときに「digest は一致するのに本文が空」のような食い違いが出る。
function licenseBody(text, from = 0) {
  const start = text.indexOf(LICENSE_MARKER, from);
  if (start === -1) return null;
  const bodyStart = text.indexOf("\n\n", start + LICENSE_MARKER.length);
  const end = text.indexOf(LICENSE_END_MARKER, start + LICENSE_MARKER.length);
  if (bodyStart === -1 || end === -1 || bodyStart >= end) return null;
  return { body: text.slice(bodyStart + 2, end).trim(), next: end + LICENSE_END_MARKER.length };
}

function licenseBodies(text) {
  const bodies = [];
  for (let found = licenseBody(text); found; found = licenseBody(text, found.next)) {
    bodies.push(found.body);
  }
  return bodies;
}

// `name@version` と、その license 本文の digest を対にして返す。名前だけを見る形にしないのは、
// 本文が壊れても・別物に差し替わっても「非空である」以上のことを言えないため（notice の中身が
// 正しいことがこのゲートの目的なので、形だけ合っていても意味が無い）。
//
// 鍵に version を入れるのは、rollup-plugin-license の dedupe 鍵に合わせるため。生成側で
// `multipleVersions: true` を指定すると同名・異 version が別 entry になるので、こちらも
// 名前だけを鍵にすると 2 件が 1 件へ潰れ、片方の欠落が見えなくなる。
export function dependencyDigests(text) {
  const entries = text.split(ENTRY_MARKER).slice(1);
  const result = {};
  for (const entry of entries) {
    const name = entry.match(/^- Name: `([^`]+)`$/m)?.[1];
    const version = entry.match(/^- Version: `([^`]+)`$/m)?.[1];
    if (!name || !version) continue;
    const body = licenseBody(entry)?.body ?? "";
    result[`${name}@${version}`] = createHash("sha256").update(body, "utf8").digest("hex");
  }
  return Object.fromEntries(Object.entries(result).sort(([left], [right]) => (left < right ? -1 : 1)));
}

// tarball を展開したディレクトリから notice ファイルを見つける。baseline との突き合わせで
// 「増えた」も「消えた」も落とすため、検査側で列挙する対象は baseline ではなく実物にする。
export function findNoticeFiles(packageDirectory) {
  const found = [];
  const walk = (directory, prefix) => {
    if (!existsSync(directory)) return;
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (entry.isDirectory()) walk(join(directory, entry.name), relative);
      else if (NOTICE_FILE_PATTERN.test(entry.name)) found.push(relative);
    }
  };
  walk(packageDirectory, "");
  return found.sort();
}

export function inspectPackageDirectory(packageName, packageDirectory, baseline) {
  const expected = baseline[packageName];
  if (!expected) return { failures: [`${packageName}: no baseline entry`], counts: [] };

  const failures = [];
  const counts = [];
  const found = findNoticeFiles(packageDirectory);
  const expectedPaths = Object.keys(expected).sort();

  // 両側が空だと以下の比較は 1 件も回らず、「検査して通った」と「検査対象が無かった」の区別が
  // 消える。生成が全滅した状態で --write-baseline を回すとその空 baseline が固定されてしまうので、
  // 公開する package は必ず notice を 1 件以上同梱する、を明示の不変条件にする。
  if (expectedPaths.length === 0) {
    failures.push(
      `${packageName}: baseline lists no notice file (regenerate the baseline from a working build)`,
    );
  }
  if (found.length === 0) failures.push(`${packageName}: the tarball ships no notice file`);

  for (const path of expectedPaths) {
    if (!found.includes(path)) failures.push(`${packageName}: missing ${path}`);
  }
  for (const path of found) {
    if (!expectedPaths.includes(path)) {
      failures.push(`${packageName}: unexpected notice file ${path} (regenerate the baseline)`);
    }
  }

  for (const path of expectedPaths) {
    if (!found.includes(path)) continue;
    const text = readFileSync(join(packageDirectory, ...path.split("/")), "utf8");
    if (text.trim() === "") {
      failures.push(`${packageName}: ${path} is empty`);
      continue;
    }

    const entryCount = text.split(ENTRY_MARKER).length - 1;
    const licenseCount = text.split(LICENSE_MARKER).length - 1;
    const bodies = licenseBodies(text);
    counts.push({ packageName, path, entries: entryCount });

    if (entryCount !== licenseCount) {
      failures.push(
        `${packageName}: ${path} has ${entryCount} entries but ${licenseCount} license text fields`,
      );
    }
    if (bodies.length !== licenseCount || bodies.some((body) => body === "")) {
      failures.push(`${packageName}: ${path} has a missing or empty license text body`);
    }

    // 0 件の成果物にもファイルを出し、「bundle されていない」と明記させる。ファイルが無いことを
    // 0 件と読む形にすると、生成が壊れた場合と区別できない。
    if (Object.keys(expected[path]).length === 0 && !text.includes(NO_BUNDLED_DEPENDENCIES)) {
      failures.push(`${packageName}: ${path} does not state that no third-party code is bundled`);
    }

    const actual = dependencyDigests(text);
    const actualNames = Object.keys(actual);
    const expectedNames = Object.keys(expected[path]);
    const missing = expectedNames.filter((name) => !actualNames.includes(name));
    const added = actualNames.filter((name) => !expectedNames.includes(name));
    if (missing.length > 0) {
      failures.push(`${packageName}: ${path} is missing bundled dependencies: ${missing.join(", ")}`);
    }
    if (added.length > 0) {
      failures.push(
        `${packageName}: ${path} has dependencies not in the baseline: ${added.join(", ")} (regenerate the baseline)`,
      );
    }
    const changed = expectedNames.filter(
      (name) => actual[name] !== undefined && actual[name] !== expected[path][name],
    );
    if (changed.length > 0) {
      failures.push(
        `${packageName}: ${path} has changed license text for: ${changed.join(", ")} (regenerate the baseline after reviewing the new text)`,
      );
    }
    if (actualNames.length !== entryCount) {
      failures.push(
        `${packageName}: ${path} has ${entryCount} entries but ${actualNames.length} name fields`,
      );
    }
  }

  return { failures, counts };
}

function run(command, args, cwd) {
  console.log(`> ${command} ${args.join(" ")}`);
  const result = spawnSync(command, args, { cwd, stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} ${args.join(" ")} exited with ${result.status}`);
}

function pack(packageSpec, temporaryRoot) {
  const packageTemp = join(temporaryRoot, packageSpec.name.replaceAll(/[@/]/g, "_"));
  const tarballDirectory = join(packageTemp, "tarball");
  const extractDirectory = join(packageTemp, "extract");
  mkdirSync(tarballDirectory, { recursive: true });
  mkdirSync(extractDirectory, { recursive: true });

  run(
    "corepack",
    ["pnpm", "pack", "--pack-destination", tarballDirectory],
    join(vendorRoot, packageSpec.directory),
  );
  const tarballs = readdirSync(tarballDirectory).filter((name) => name.endsWith(".tgz"));
  if (tarballs.length !== 1) {
    throw new Error(`${packageSpec.name}: expected one tarball, found ${tarballs.length}`);
  }
  run("tar", ["-xzf", join(tarballDirectory, tarballs[0]), "-C", extractDirectory], repositoryRoot);

  const packageDirectory = join(extractDirectory, "package");
  if (!existsSync(packageDirectory)) throw new Error(`${packageSpec.name}: tarball has no package/ directory`);
  return packageDirectory;
}

function buildAndPack(temporaryRoot) {
  run("corepack", ["pnpm", "install", "--frozen-lockfile"], vendorRoot);

  // build 前に生成先を空にする。viewer-server/static も cli の dist も emptyOutDir: false なので、
  // 生成が止まったり対象から外れたりしても前回のファイルが残り、古い bundle と古い notice を
  // この検査が受理してしまう——塞ごうとしている fail-open そのものが検査側に生える。notice だけを
  // 消す形では、notice に載らない古い JS が tarball に残る経路が閉じない。clean checkout の CI では
  // 起きないが、同じ作業ツリーで繰り返す release preflight と prepublishOnly では起きる。
  run("corepack", ["pnpm", "run", "clean"], vendorRoot);

  run("corepack", ["pnpm", "-r", "run", "build"], vendorRoot);

  return PACKAGES.map((spec) => ({ spec, directory: pack(spec, temporaryRoot) }));
}

function writeBaseline() {
  const temporaryRoot = mkdtempSync(join(tmpdir(), "codemem-notices-"));
  try {
    const packed = buildAndPack(temporaryRoot);
    const baseline = {};
    for (const { spec, directory } of packed) {
      baseline[spec.name] = {};
      for (const path of findNoticeFiles(directory)) {
        baseline[spec.name][path] = dependencyDigests(
          readFileSync(join(directory, ...path.split("/")), "utf8"),
        );
      }
    }
    writeFileSync(baselinePath, `${JSON.stringify(baseline, null, 2)}\n`);
    console.log(`baseline written: ${baselinePath}`);
  } finally {
    rmSync(temporaryRoot, { recursive: true, force: true });
  }
}

function main() {
  // 綴り間違いを黙って「通常の検査」として扱わない。ここが最初に走ることで、`--help` のような
  // 未知の引数で起動するだけで「main() に到達したか」を build 抜きで確かめられる（末尾の
  // 起動判定が壊れると exit 0・無出力になるので、test はその差を見ている）。
  const unknown = process.argv.slice(2).filter((argument) => argument !== "--write-baseline");
  if (unknown.length > 0) {
    console.error(`unknown option: ${unknown.join(" ")}`);
    console.error("usage: notice-inclusion-check.mjs [--write-baseline]");
    process.exitCode = 2;
    return;
  }

  if (process.argv.includes("--write-baseline")) {
    writeBaseline();
    return;
  }

  const failures = [];
  const counts = [];
  const temporaryRoot = mkdtempSync(join(tmpdir(), "codemem-notices-"));

  try {
    const baseline = JSON.parse(readFileSync(baselinePath, "utf8"));
    for (const packageSpec of PACKAGES) {
      if (!baseline[packageSpec.name]) failures.push(`${packageSpec.name}: no baseline entry`);
    }

    for (const { spec, directory } of buildAndPack(temporaryRoot)) {
      const result = inspectPackageDirectory(spec.name, directory, baseline);
      failures.push(...result.failures);
      counts.push(...result.counts);
    }
  } catch (error) {
    failures.push(error instanceof Error ? (error.stack ?? error.message) : String(error));
  } finally {
    rmSync(temporaryRoot, { recursive: true, force: true });
  }

  for (const notice of counts) {
    console.log(`${notice.packageName} ${notice.path}: ${notice.entries} entries`);
  }

  if (failures.length > 0) {
    console.error("third-party notice inclusion check FAILED:");
    for (const failure of failures) console.error(`  - ${failure}`);
    console.error("依存が正当に増減したなら --write-baseline で再生成し、差分を commit に載せること。");
    process.exitCode = 1;
    return;
  }
  console.log("third-party notice inclusion check OK");
}

// 直接起動されたかどうかの判定。両側を realpath に落としてから比べる: `import.meta.url` は Node が
// 実体パスへ正規化するのに対し `process.argv[1]` は起動時の綴りのままなので、symlink を挟んだ経路で
// 起動すると一致せず、main() を呼ばないまま exit 0 で終わる。
//
// `import.meta.main` にも置き換えないこと。あれは Node 24.2 で入ったので、engines の `>=24` を
// 満たす 24.0 / 24.1 では undefined になり、同じく main() を呼ばない。
//
// どちらの取り違えも「検査した」と「検査しなかった」の区別を消す = ゲートとしては fail-open。
export function isDirectInvocation(argv1, moduleUrl) {
  if (!argv1) return false;
  try {
    return realpathSync(argv1) === realpathSync(fileURLToPath(moduleUrl));
  } catch {
    return false;
  }
}

if (isDirectInvocation(process.argv[1], import.meta.url)) main();
