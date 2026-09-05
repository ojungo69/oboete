#!/usr/bin/env node
/**
 * 契約ファイルの SHA-256 を出力する（plan Task 4「Emit schema/fixture SHA-256 values for
 * TypeScript/Rust parity」）。
 *
 * Rust 側が同じ契約を読んでいることを確かめる手段が版番号しかないと、drift した複製を
 * 読んでいても気付けない。ここで出すのは**生バイト**の hash なので、整形の違いも検出する。
 *
 * 再生成:
 *   node harness/contract-hashes.mjs > harness/contract-hashes.json
 *
 * CI の "Contract hashes are regenerated" が、この出力と commit された JSON を突き合わせる。
 */
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

// `URL.pathname` は Windows で `/C:/...`、空白を含む path で `%20` になる。
// 実在する path を得るには `fileURLToPath` を通す
const harnessDir = fileURLToPath(new URL(".", import.meta.url)).replace(/[/\\]$/, "");

/**
 * 契約として凍結しているもの。`.json`（schema・manifest・組み立て済み fixture）と
 * `.jsonl`（生の捕捉ログ）を対象にする。`.ts` は TypeScript 実装であって契約データでは
 * ないので入れない（Rust 側が読むのは JSON だけ）。
 */
const ROOTS = ["schema", "fixtures"];

function walk(dir) {
  const out = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) out.push(...walk(full));
    else if (entry.endsWith(".json") || entry.endsWith(".jsonl")) out.push(full);
  }
  return out;
}

const files = ROOTS.flatMap((root) => walk(join(harnessDir, root)))
  .map((full) => relative(harnessDir, full))
  .sort();

const hashes = {};
for (const file of files) {
  hashes[file] = createHash("sha256").update(readFileSync(join(harnessDir, file))).digest("hex");
}

process.stdout.write(`${JSON.stringify({ algorithm: "sha256", files: hashes }, null, 2)}\n`);
