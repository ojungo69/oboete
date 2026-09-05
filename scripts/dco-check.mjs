#!/usr/bin/env node

import { execFile } from "node:child_process";
import { realpathSync } from "node:fs";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

const TRAILER_LINE = /^[A-Za-z0-9][A-Za-z0-9-]*:\s+\S.*$/;
// 名前は「`<` を含まず空白以外で始終する」と固定して backtracking を線形にする（旧 `(.+?)\s+<` は
// 作者が書ける 8,000 文字の空白で約 100 秒かかり、gate が timeout = 判定無しで落ちた）。
const SIGN_OFF_LINE = /^Signed-off-by:\s+([^\s<](?:[^<]*[^\s<])?)\s+<([^<>\s]+)>\s*$/i;
const execGit = promisify(execFile);

function trailers(body) {
  const lines = body.replaceAll("\r\n", "\n").split("\n");
  while (lines.length > 0 && lines.at(-1).trim() === "") lines.pop();

  const block = [];
  while (lines.length > 0 && lines.at(-1).trim() !== "") block.unshift(lines.pop());
  // git 自身の trailer 解釈と同じく、最終段落に trailer 以外の行が混じっていても block として扱う。
  // 全行が trailer であることを要求すると、`Refs #59` の直後に sign-off を置くこの repo の commit
  // 慣習が丸ごと落ちる（実測で確認済み）。1 行も trailer の形をしていない段落は block ではない。
  if (!block.some((line) => TRAILER_LINE.test(line))) return [];
  return block;
}

function hasMatchingSignOff(commit) {
  const acceptedEmails = [commit.authorEmail, commit.committerEmail].map((email) =>
    email.toLowerCase(),
  );
  return trailers(commit.body).some((line) => {
    const signOff = line.match(SIGN_OFF_LINE);
    return signOff !== null && acceptedEmails.includes(signOff[2].toLowerCase());
  });
}

// bot 用の免除は置かない。author email は commit する側が `--author` で自由に名乗れるので、
// email 一致だけで免除すると誰でも bot を騙って未署名 commit を通せる。免除が必要になったら、
// email ではなく pull_request_target が渡す actor login（GitHub 側が認証した値）に紐付ける。
export function findUnsignedCommits(commits) {
  return commits.filter((commit) => !hasMatchingSignOff(commit));
}

async function git(args) {
  try {
    // 既定の maxBuffer は 1 MiB。`%B` は全 commit の本文を出すので、commit 数の多い PR では
    // 超える。超えると git が殺されて exit 2 になり、「検査できなかった」と区別が付かなくなる。
    const { stdout } = await execGit("git", args, {
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
    });
    return stdout;
  } catch (error) {
    const detail = error?.stderr?.toString().trim() || error?.message;
    const suffix = detail ? `: ${detail}` : "";
    throw new Error(`git ${args[0]} failed${suffix}`);
  }
}

async function commitsInRange(range) {
  // NUL は commit metadata に入らないため、message 内の改行と衝突せず 6 field を区切れる。
  const format = "format:%H%x00%s%x00%an%x00%ae%x00%ce%x00%B";
  const output = await git(["log", "-z", `--format=${format}`, range]);
  if (output === "") return [];

  const fields = output.split("\0");
  if (fields.length % 6 !== 0) throw new Error("git log returned malformed commit data");

  const commits = [];
  for (let index = 0; index < fields.length; index += 6) {
    commits.push({
      sha: fields[index],
      subject: fields[index + 1],
      authorName: fields[index + 2],
      authorEmail: fields[index + 3],
      committerEmail: fields[index + 4],
      body: fields[index + 5],
    });
  }
  return commits;
}

async function main() {
  const [baseRef, headRef, ...extra] = process.argv.slice(2);
  if (!baseRef || !headRef || extra.length > 0) {
    console.error("usage: dco-check.mjs <base-ref> <head-ref>");
    process.exitCode = 2;
    return;
  }

  try {
    const mergeBase = (await git(["merge-base", baseRef, headRef])).trim();
    if (mergeBase === "") throw new Error("git merge-base returned no commit");

    const commits = await commitsInRange(`${mergeBase}..${headRef}`);
    if (commits.length === 0) throw new Error("no commits found between merge-base and head");

    const unsigned = findUnsignedCommits(commits);
    if (unsigned.length > 0) {
      console.error("DCO check FAILED: Signed-off-by が無い、または email が一致しない commit:");
      for (const commit of unsigned) {
        console.error(
          `  - ${commit.sha} ${commit.subject} (${commit.authorName} <${commit.authorEmail}>)`,
        );
      }
      process.exitCode = 1;
      return;
    }

    console.log(`DCO check OK: ${commits.length} commit(s)`);
  } catch (error) {
    console.error(`DCO check ERROR: ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 2;
  }
}

// 直接起動されたかどうかの判定。両側を realpath に落としてから比べる: `import.meta.url` は Node が
// 実体パスへ正規化するのに対し `process.argv[1]` は起動時の綴りのままなので、symlink を挟んだ経路で
// 起動すると一致せず、main() を呼ばないまま exit 0 で終わる。
//
// `import.meta.main` にも置き換えないこと。あれは Node 24.2 で入ったので、CONSTITUTION.md が下限に
// 置く Node 22.16 では undefined になり、同じく main() を呼ばない。
//
// どちらの取り違えも「検査した」と「検査しなかった」の区別を消す = ゲートとしては fail-open。
function isDirectInvocation(argv1, moduleUrl) {
  if (!argv1) return false;
  try {
    return realpathSync(argv1) === realpathSync(fileURLToPath(moduleUrl));
  } catch {
    // path を解決できないなら「起動されていない」と断定しない。import 側で余計に main() が
    // 動けば usage を出して exit 2 になり、そこで気づける。検査しないまま exit 0 で終わるほうが悪い。
    return true;
  }
}

if (isDirectInvocation(process.argv[1], import.meta.url)) await main();
