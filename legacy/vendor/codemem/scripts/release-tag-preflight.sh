#!/usr/bin/env bash
set -euo pipefail

EXPECTED_BRANCH="${RELEASE_EXPECTED_BRANCH:-main}"
MAIN_REF="origin/${EXPECTED_BRANCH}"
TARGET_COMMIT="${RELEASE_TAG_COMMIT:-${GITHUB_SHA:-HEAD}}"
# ゲートは vendor snapshot の外（free-mem 側の harness/）にある。snapshot 単体を repository
# root として取り出した木では解決できず node がそこで失敗するが、それが正しい: 見つからないから
# といって検査を飛ばせば、このゲートが塞いでいる素通り経路が復活する。VENDOR.md のとおり
# snapshot は free-mem の中でだけ使う前提なので、この参照は満たされる。
NOTICE_REPOSITORY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

# CI を経ない手動 publish でも、実際の tarball に notice が無ければ tag を打たせない。
#
# 呼ぶのは tag の到達性が確定した後だけにする。ゲートは install と build を行う＝候補 commit の
# build script を実行するので、判定より先に走らせると、release workflow の preflight job
# (contents: write) の中で未検証のコードが動くことになる。成功で抜ける経路は下の 3 箇所しか
# 無いので、そのすべてをこの関数に集約する。
finish_pass() {
	node "${NOTICE_REPOSITORY_ROOT}/harness/notice-inclusion-check.mjs"

	# ゲートは build を行う＝`plugins/{claude,codex}/scripts/` の複製を作業ツリー上で作り直す。
	# HEAD 側が古い・欠けていてもここまでは通ってしまい、tag を打った source archive にだけ
	# 古い notice が残る。CI の check job と同じ検査をここでも行う。列挙の正本は
	# packages/cli/scripts/sync-hook-runtime.mjs の複製先。
	#
	# 作業ツリー全体ではなく対象 4 ファイルだけを見る。全体 clean の確認は下の local guard に
	# あるが、release branch から抜ける経路はそこを通らないため、ここで全体を見ると
	# 無関係な未コミット変更で落ちる。
	local drift
	drift="$(git -C "${NOTICE_REPOSITORY_ROOT}" status --porcelain --untracked-files=all -- \
		vendor/codemem/plugins/claude/scripts/hook-runtime.mjs \
		vendor/codemem/plugins/codex/scripts/hook-runtime.mjs \
		vendor/codemem/plugins/claude/scripts/THIRD_PARTY_NOTICES.hook-runtime.md \
		vendor/codemem/plugins/codex/scripts/THIRD_PARTY_NOTICES.hook-runtime.md)"
	if [[ -n "${drift}" ]]; then
		echo "Release tag preflight failed: committed hook-runtime copies do not match the build." >&2
		echo "${drift}" >&2
		echo "Run the build and commit the regenerated copies before tagging." >&2
		exit 1
	fi

	echo "$1"
	exit 0
}

git fetch origin "${EXPECTED_BRANCH}" --quiet
git fetch origin 'refs/heads/release/*:refs/remotes/origin/release/*' --quiet || true

main_commit="$(git rev-parse "${MAIN_REF}^{commit}")"
tag_commit="$(git rev-parse "${TARGET_COMMIT}^{commit}")"

matches_main=0
if git merge-base --is-ancestor "${tag_commit}" "${main_commit}"; then
	matches_main=1
fi

matching_release_branches=()
while IFS= read -r branch_ref; do
	[[ -z "${branch_ref}" ]] && continue
	if git merge-base --is-ancestor "${tag_commit}" "${branch_ref}"; then
		matching_release_branches+=("${branch_ref#origin/}")
	fi
done < <(git for-each-ref --format='%(refname:short)' 'refs/remotes/origin/release/*')

qualified_branch=""
if [[ "${matches_main}" -eq 1 ]]; then
	qualified_branch="${EXPECTED_BRANCH}"
elif [[ "${#matching_release_branches[@]}" -eq 1 ]]; then
	qualified_branch="${matching_release_branches[0]}"
fi

if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
	if [[ -z "${qualified_branch}" ]]; then
		echo "Release tag preflight failed: tag commit is not reachable from origin/${EXPECTED_BRANCH} or an origin/release/* branch." >&2
		echo "  tag commit:  ${tag_commit}" >&2
		echo "  main commit: ${main_commit}" >&2
		if [[ "${#matching_release_branches[@]}" -gt 1 ]]; then
			echo "  matching release branches: ${matching_release_branches[*]}" >&2
		fi
		echo "Tag only after the release commit is merged to ${EXPECTED_BRANCH} or a single release branch." >&2
		exit 1
	fi
	if [[ "${qualified_branch}" == "${EXPECTED_BRANCH}" && "${tag_commit}" != "${main_commit}" ]]; then
		finish_pass "Release tag preflight passed for commit ${tag_commit} on ${qualified_branch}."
	fi
	if [[ "${qualified_branch}" != "${EXPECTED_BRANCH}" ]]; then
		finish_pass "Release tag preflight passed for commit ${tag_commit} on ${qualified_branch}."
	fi
	if [[ "${tag_commit}" != "${main_commit}" ]]; then
		echo "Release tag preflight failed: local tag target is not origin/${EXPECTED_BRANCH} HEAD." >&2
		echo "  tag commit:  ${tag_commit}" >&2
		echo "  main commit: ${main_commit}" >&2
		echo "Tag from updated ${EXPECTED_BRANCH} after the release PR merge commit is at HEAD." >&2
		exit 1
	fi
	elif [[ -z "${qualified_branch}" ]]; then
	echo "Release tag preflight failed: local tag target is not on origin/${EXPECTED_BRANCH} or a single origin/release/* branch." >&2
	echo "  tag commit:  ${tag_commit}" >&2
	echo "  main commit: ${main_commit}" >&2
	if [[ "${#matching_release_branches[@]}" -gt 1 ]]; then
		echo "  matching release branches: ${matching_release_branches[*]}" >&2
	fi
	exit 1
fi

if [[ -z "${GITHUB_ACTIONS:-}" && "${RELEASE_SKIP_LOCAL_GUARDS:-0}" != "1" ]]; then
	current_branch="$(git branch --show-current || true)"
	if [[ -n "${qualified_branch}" && "${current_branch}" != "${qualified_branch}" ]]; then
		echo "Release tag preflight failed: current branch is '${current_branch}', expected '${qualified_branch}'." >&2
		exit 1
	fi

	if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
		echo "Release tag preflight failed: working tree is not clean." >&2
		exit 1
	fi
fi

finish_pass "Release tag preflight passed for commit ${tag_commit} on ${qualified_branch:-${EXPECTED_BRANCH}}."
