// notice 生成そのもののテスト。中身は rollup-plugin-license が集めた依存の一覧なので、
// 収集の設定が変わると生成物が黙って痩せる。実際に rollup を回して確かめる。
//
// vendor/codemem の node_modules が要る（rollup と rollup-plugin-license）。CI では
// notices job がゲートを走らせた後——つまり install 済みの状態——で実行する。
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const repositoryRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const vendorRoot = join(repositoryRoot, "vendor", "codemem");

// rollup は rollup-plugin-license の peer で、pnpm の厳密な配置では vendor 直下に無い。
// plugin の解決位置を起点にすれば版を決め打ちせずに辿れる。
const vendorRequire = createRequire(join(vendorRoot, "package.json"));
const pluginRequire = createRequire(vendorRequire.resolve("rollup-plugin-license"));

const { rollup } = await import(pathToFileURL(pluginRequire.resolve("rollup")).href);
const licenseNoticePlugin = (
  await import(pathToFileURL(join(vendorRoot, "scripts", "license-notice-plugin.mjs")).href)
).default;

function writePackage(root, directory, name, version) {
  const packageDirectory = join(root, directory, "node_modules", name);
  mkdirSync(packageDirectory, { recursive: true });
  writeFileSync(
    join(packageDirectory, "package.json"),
    `${JSON.stringify({ name, version, main: "index.js", license: "MIT" })}\n`,
  );
  writeFileSync(join(packageDirectory, "index.js"), `export const value = "${directory}";\n`);
  writeFileSync(join(packageDirectory, "LICENSE"), `MIT License text for ${name}@${version}\n`);
  return join(packageDirectory, "index.js");
}

// rollup-plugin-license は生成時に cwd の package.json を読むので、fixture 側にも置いて
// そこへ移動してから回す（node:test は 1 ファイル内を順に実行するので chdir は競合しない）。
async function generate(t, packages) {
  const root = mkdtempSync(join(tmpdir(), "license-notice-plugin-"));
  const previousCwd = process.cwd();
  t.after(() => {
    process.chdir(previousCwd);
    rmSync(root, { recursive: true, force: true });
  });
  writeFileSync(
    join(root, "package.json"),
    `${JSON.stringify({ name: "notice-fixture", version: "0.0.0", private: true })}\n`,
  );
  const entries = packages.map(({ directory, name, version }) =>
    writePackage(root, directory, name, version),
  );
  const entry = join(root, "entry.js");
  writeFileSync(
    entry,
    entries
      .map((path, index) => `export { value as value${index} } from ${JSON.stringify(path)};`)
      .join("\n"),
  );

  const outFile = join(root, "THIRD_PARTY_NOTICES.md");
  process.chdir(root);
  const bundle = await rollup({ input: entry, plugins: [licenseNoticePlugin({ outFile })] });
  await bundle.generate({ format: "es" });
  await bundle.close();
  return readFileSync(outFile, "utf8");
}

// 収集側の dedupe 鍵は既定では package 名だけ。`multipleVersions: true` を外すと同名・異 version が
// 1 件へ潰れ、落ちた方の version と license 本文が notice から消える（実測: 2 entries → 1 entry）。
// baseline は `name@version` を鍵にしているので、この設定が外れると欠落が比較にも映らない。
test("同名・異 version の依存を両方 notice に出す", async (t) => {
  const notice = await generate(t, [
    { directory: "a", name: "dup-pkg", version: "1.0.0" },
    { directory: "b", name: "dup-pkg", version: "2.0.0" },
  ]);
  assert.equal(notice.split("<!-- codemem:dependency -->").length - 1, 2);
  assert.match(notice, /- Version: `1\.0\.0`/);
  assert.match(notice, /- Version: `2\.0\.0`/);
  assert.match(notice, /MIT License text for dup-pkg@1\.0\.0/);
  assert.match(notice, /MIT License text for dup-pkg@2\.0\.0/);
});

// 0 件でもファイルを出し、0 件であると述べる。ゲートはファイルの不在を失敗として扱うので、
// 生成側がここで黙ってファイルを作らないと、その package の検査は落ちる側に倒れる。
test("bundle する第三者コードが無くても notice を出す", async (t) => {
  const notice = await generate(t, []);
  assert.match(notice, /No third-party code is bundled in this artifact\./);
});
