// test が作る一時 directory の後始末を 1 箇所で持つ。変異ゲートは harness/evidence の test を
// 145 回以上回すので、置きっぱなしにすると /tmp が単調に増える（先例: rig-manifest.test.ts:13-18）。
//
// 片付けは `node:test` の after() ではなく process の出口に掛ける。after() を module 直下で
// 呼ぶと、この helper を import しただけの素の script でも runner が起動して
// `ℹ tests 0 …` を stdout に流す（実測）。後始末に runner を要求しないので、test でない
// 呼び出し元から使う日にも同じ保証が続く
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const ROOTS: string[] = [];

process.on("exit", () => {
  for (const root of ROOTS) rmSync(root, { recursive: true, force: true });
});

export const newRoot = (prefix = "evroot-"): string => {
  const root = mkdtempSync(join(tmpdir(), prefix));
  ROOTS.push(root);
  return root;
};
