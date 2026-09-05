#!/usr/bin/env node
// license / NOTICE / attribution が欠けていないかを検査する。
//
// 目的は「配布物にライセンス表示が載らない」事故の検出であって、ライセンスの妥当性判断ではない。
// 判断は evidence/adr-004-licensing.md 側にある。
//
// usage: node harness/license-inclusion-check.mjs [repoRoot]

import { createHash } from "node:crypto";
import { readFileSync, existsSync, readdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = process.argv[2] ?? join(dirname(fileURLToPath(import.meta.url)), "..");
const failures = [];

// 見出しだけを見ると、本文を削ったファイルが検査を通ってしまう（許諾条項が消えても
// "Apache License / Version 2.0" の 2 行は残る）。全文の digest で固定する。
// LICENSE は apache.org の LICENSE-2.0.txt そのもの、vendor/codemem/LICENSE は
// upstream codemem の MIT 本文。意図して差し替えるときは、差分を確認してから更新する。
const CANONICAL_LICENSE_SHA256 = {
  LICENSE: "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30",
  "vendor/codemem/LICENSE": "ff32e354e7a84ddeda6b2cd2c43da707ce60a06eba568f54701aa25e225421da",
};

function sha256(text) {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

function read(path) {
  return readFileSync(join(root, path), "utf8");
}

function requireFile(path, why) {
  if (!existsSync(join(root, path))) failures.push(`missing ${path} (${why})`);
}

// attribution は本文が中身であって、ファイル名ではない。空にされても存在検査は通るので、
// 中身が消えたら落ちるだけの錨を置く。digest で固定しないのは、依存が変われば
// THIRD_PARTY_NOTICES.md が正当に変わるため（lockfile 更新のたびに再スキャンする運用）。
function requireContains(path, needles, why) {
  if (!existsSync(join(root, path))) return;
  const text = read(path);
  for (const needle of needles) {
    if (!text.includes(needle)) failures.push(`${path}: lost ${JSON.stringify(needle)} (${why})`);
  }
}

requireFile("LICENSE", "repository-wide grant");
requireFile("NOTICE", "Apache-2.0 §4(d) attribution");
requireFile("THIRD_PARTY_NOTICES.md", "third-party attribution");
requireFile("CONTRIBUTING.md", "inbound contribution terms");
requireFile("vendor/codemem/LICENSE", "vendored MIT snapshot must keep its own license");
requireFile("vendor/codemem/package.json", "vendored package metadata (下で license 欄を読む)");
requireFile("evidence/adr-004-licensing.md", "README と CONTRIBUTING が根拠として指す ADR");

requireContains(
  "NOTICE",
  ["Copyright", "The free-mem Authors", "THIRD_PARTY_NOTICES.md"],
  "Apache-2.0 §4(d) の attribution 本文",
);
requireContains(
  "THIRD_PARTY_NOTICES.md",
  ["## Dependency licenses", "Production only"],
  "依存スキャンの記載",
);
requireFile("vendor/codemem/VENDOR.md", "vendored snapshot の provenance（下で pin を読む）");

if (failures.length === 0) {
  const license = read("LICENSE");
  const licenseDigest = sha256(license);
  if (licenseDigest !== CANONICAL_LICENSE_SHA256.LICENSE) {
    failures.push(
      `LICENSE is not the canonical Apache-2.0 text (sha256 ${licenseDigest}, expected ${CANONICAL_LICENSE_SHA256.LICENSE})`,
    );
  }

  const vendorLicense = read("vendor/codemem/LICENSE");
  const vendorDigest = sha256(vendorLicense);
  if (vendorDigest !== CANONICAL_LICENSE_SHA256["vendor/codemem/LICENSE"]) {
    failures.push(
      `vendor/codemem/LICENSE is not the upstream MIT text (sha256 ${vendorDigest}, expected ${CANONICAL_LICENSE_SHA256["vendor/codemem/LICENSE"]})`,
    );
  }

  // 手で並べた錨は、その錨だけを貼った stub でも通る（検査が自分の literal を検査する形）。
  // vendored snapshot の記載は一次情報から導いて突き合わせる: 著作権者は MIT 本文から、
  // pin は VENDOR.md から取り、THIRD_PARTY_NOTICES.md が同じものを載せているか見る。
  const notices = read("THIRD_PARTY_NOTICES.md");

  const holder = /^Copyright \(c\) \d{4} (.+)$/m.exec(vendorLicense)?.[1]?.trim();
  if (!holder) {
    failures.push("vendor/codemem/LICENSE has no copyright line to attribute");
  } else if (!notices.includes(holder)) {
    failures.push(`THIRD_PARTY_NOTICES.md no longer credits ${holder} (vendor/codemem/LICENSE の著作権者)`);
  }

  const pin = /`([0-9a-f]{40})`/.exec(read("vendor/codemem/VENDOR.md"))?.[1];
  if (!pin) {
    failures.push("vendor/codemem/VENDOR.md records no 40-hex snapshot commit");
  } else if (!notices.includes(pin)) {
    failures.push(`THIRD_PARTY_NOTICES.md no longer records the vendored snapshot commit ${pin}`);
  }

  const readme = read("README.md");
  if (!/Apache License 2\.0/.test(readme)) {
    failures.push("README.md does not state the same license as LICENSE");
  }
  if (!/vendor\/codemem\/.*MIT|MIT snapshot/.test(readme)) {
    failures.push("README.md no longer records that vendor/codemem stays MIT");
  }

  const notice = read("NOTICE");
  if (!/vendor\/codemem/.test(notice)) {
    failures.push("NOTICE no longer points at the vendored snapshot's own license");
  }

  // vendored package は MIT のまま。Apache-2.0 へ書き換えられていないことを確認する
  const pkgDir = join(root, "vendor/codemem/packages");
  const pkgFiles = [join(root, "vendor/codemem/package.json")];
  if (existsSync(pkgDir)) {
    for (const name of readdirSync(pkgDir)) {
      const p = join(pkgDir, name, "package.json");
      if (existsSync(p)) pkgFiles.push(p);
    }
  }
  for (const file of pkgFiles) {
    const pkg = JSON.parse(readFileSync(file, "utf8"));
    const rel = file.slice(root.length + 1);
    // private workspace package は npm に出ないため license 欄が無くてよい（upstream の状態）。
    // 出るものは MIT を明示していること、出ないものも MIT 以外を名乗っていないことを見る。
    if (pkg.license !== undefined && pkg.license !== "MIT") {
      failures.push(`${rel}: vendored package must stay "MIT", got ${JSON.stringify(pkg.license)}`);
    } else if (pkg.license === undefined && pkg.private !== true) {
      failures.push(`${rel}: publishable vendored package has no license field (expected "MIT")`);
    }

    // `license: "MIT"` は metadata であって配布物ではない。MIT 本文は「すべての複製または
    // 実質的部分に含めること」を条件にしているので、公開される tarball に本文が要る。
    // npm は package ルートの LICENSE を `files` の指定に関わらず必ず tarball に入れるため、
    // ここではファイルの存在と本文の一致だけを見る（`npm pack` を CI で走らせなくてよい）。
    if (pkg.private === true || file === pkgFiles[0]) continue;
    const licensePath = join(dirname(file), "LICENSE");
    if (!existsSync(licensePath)) {
      failures.push(`${rel}: publishable package ships no LICENSE file (MIT 本文が tarball に載らない)`);
    } else if (readFileSync(licensePath, "utf8") !== vendorLicense) {
      failures.push(`${dirname(rel)}/LICENSE: does not match vendor/codemem/LICENSE`);
    }
  }
}

if (failures.length > 0) {
  console.error("license inclusion check FAILED:");
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log("license inclusion check OK");
