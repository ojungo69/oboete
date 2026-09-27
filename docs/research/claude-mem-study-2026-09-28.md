# claude-mem, compared mechanism by mechanism with oboete (2026-09-28)

> Status 2026-09-28: research input. The resetsAt unit fix is merged (#168); a native Windows launcher bug found here is issue #169. One owner question is open (要約); the other was a stale spec line, fixed in this PR (spec 7, `setup --yes` follows decision 28).

## 要約 (日本語)

- claude-mem（v13.28.0）を2回に分けて調べました。1回目は約190の仕組みを oboete と1つずつ比べました。2回目では、1回目で抜けていた分野を足しました。クラウドやスマホでの利用、インストール、他のエージェントとの連携、再起動や作り直しの扱いです。提案はどれも3つの点を別々の担当が確かめました。「oboete に本当に無いか」「claude-mem は本当にそう動くか」「oboete の方針に合うか」です。通ったものだけを推奨にしました。推奨は全部で24件です。
- oboete がすでに勝っている点は次のとおりです。
  - 記録は手元のファイルにすぐ書きます。裏で動くプログラムが止まっても、記録は消えません。書き込みにかかる時間は、oboete 自身の計測では一番遅い機械で約0.025秒です。仕様の仮の目標は0.020秒以下なので、まだ少し超えています。
  - 検索の正確さは、あなたのデータを使った oboete 自身の測定で 0.545 対 0.244 です。claude-mem の約2倍にあたります。ただしこの数字はまだ仮のものです。採点に使ったモデルが信頼できるかを確かめる前に測ったため、仕様でも「確定していない」と書いてあります。
  - 利用状況を外に送りません。エラーの本文も保存しません。
- 訂正があります。前回「対応エージェントの数が claude-mem より多い」と書いたのは誤りでした。claude-mem が対応しているのは Claude Code、Codex、Cursor、Windsurf、Antigravity、OpenCode、OpenClaw などです。oboete の7つとは組み合わせが違います。Pi と Grok Build は oboete だけが対応しています。Windsurf、OpenClaw、クラウド上の会話（cowork）は claude-mem だけが対応しています。
- 推奨①（すぐにできます）：oboete 自身の検索結果が、次の記録や要約の材料に戻らないようにします。今のままでは、取り消した決定や終わった課題が「新しい情報」として戻ってくるおそれがあります。
- 推奨②③：
  - 複数の端末で同期を始める前に、「バックアップから戻した端末」のルールを仕様に書きます。
  - 「忘れる」操作が、壊れたときに退避したデータベースの写しからも消すようにします。
- 推奨④（今回新しく加えました）：「このリポジトリやフォルダは記録しない」という設定の照合を、取りこぼしなく作ります。対象は、Windows の区切り文字、`~` の書き方、Mac の大文字と小文字、リンクされたフォルダです。これは切り替え前に必要な機能です（決定27）。
- 推奨⑤〜⑧：
  - 新しい会話の始めに渡す情報が長すぎると、「どこで作業が止まったか」が切り落とされます。大事な部分を先に残すように直します。
  - Claude が作業を分けて渡す「子の作業役（サブエージェント）」にも、あなたの決定と好みを渡します。
  - ある決定の「前後に何が起きていたか」を見られるようにします。
- 推奨（今回新しく加えました）：伏せ字にする規則を3つ足します。対象は、パスワードを含む接続文字列（postgres://ユーザー:パスワード@…）、Cookie、`Authorization: Token` です。今の規則では、これらがそのまま残ります。
- 不具合を2件見つけました。
  - 1件目はすでに直っています。claude-mem は、利用上限の「リセット時刻」が秒で来てもミリ秒で来ても読めるようにしています。oboete は常に1000倍していました。そのため単位が変わると、要約が約1週間止まるおそれがありました。この修正は main に入っています（PR #168）。
  - 2件目は Windows だけの不具合で、まだ直していません。要約に使うプログラム（codex など）の入り方によっては、診断で「ある」と表示されるのに、実際には起動できません。issue #169 に記録しました。
- Claude Code 自身のメモ（MEMORY.md）を止める設定は入れません。あなたが使い続けているからです。
- 取り入れないものは次のとおりです。
  - スマホやチャットへの通知、利用状況の送信
  - Windsurf と OpenClaw への対応（決めてある7つのエージェントの範囲外です）
  - 決定を守る安全の仕組みを弱めるもの
- 仕様の食い違いを1件直しました（質問ではありません）：
  - 質問なしで設定する `oboete setup --yes` は、仕様の文面では要約を「使わない」設定のままでした。これは決定28より前の書き方の残りです。
  - 決定28（契約中のサブスクを見つけたら既定で使う）が優先されるので、`--yes` も決定28に合わせました。サブスクを使うことと止め方は、これまでどおり画面に表示されます。
  - 変えたい場合は教えてください。
- 質問：
  - claude.ai/code のウェブ版では、今の oboete は何も記録していません。前回の続きも渡せません。
  - ウェブ版で記録するには、クラウドの環境設定に「書き込み専用の限定キー」を置く必要があります。これは、同期について決めてある方針（キーを環境設定や子プログラムに渡さない）を変えることになります。
  - ウェブ版でも記録したいですか。「はい」の場合は、まず小さな試験から始めます。入れ方と動き方を確かめる試験です。
  - 検索だけなら、予定している遠隔検索（段階6）で使えるかもしれません。ただし、ウェブ版から実際に使えるかはまだ確かめていません。

## Recommendations

Citations are prefixed with **claude-mem** or **oboete**.
- **claude-mem**: `~/.claude/plugins/marketplaces/thedotmack`, commit 7d03554.
- **oboete**: `oboete-main-ro`, origin/main at 4dc8f0d, unless marked **origin/main c64b6db**, which is the main branch after #165 and #168 merged.
- A branch prefix such as `m3/gates:` names an in-flight branch.

| # | Recommendation | Goal | Value | Cost | Milestone |
|---|---|---|---|---|---|
| 1 | Stop recording and curating oboete's own memory-tool output | 2 | high | S | now, before the M5 canary |
| 2 | Write a device-rewind rule before the first push | 1, 2 | high | S | decide in spec 5.8 now; build at M6 |
| 3 | Forget also reaches quarantined and rebuilding copies | 2 | high | S | M5 |
| 4 | Capture exclusion: prefix matching that cannot silently miss | 2, release | high | M | M5 (before cut-over, decision 27) |
| 5 | Short prompts in the Inject calibration | 2 | high | S | M4 |
| 6 | Fit the whole SessionStart packet once, dropping whole items | 1 | high | M | M4 |
| 7 | Inject into subagents (SubagentStart) | 2 | high | M | M4 |
| 8 | Anchored timeline with a before/after window | 3, 1 | high | M | M4 |
| 9 | `oboete repo alias` as a control op that travels | 1, 2 | med-high | S-M | M4 local, M6 travel |
| 10 | Three structural redaction rules (URI password, Cookie, `Authorization: Token`) | safety, release | medium | S | now |
| 11 | Classify a rejected API key by its vetted code | release | medium | S | now |
| 12 | Transcript import checkpoints each batch | 1 | medium | S | M4 |
| 13 | doctor keeps going when one store is broken | release | medium | S | now |
| 14 | setup wires every agent even if one fails | release | medium | S | now |
| 15 | Windows: kill the curator's whole process tree | Windows, quota | medium | S | now or M5 |
| 16 | Windows: detach the worker from the agent's console | Windows | medium | S | M5 |
| 17 | Batched `get` over claim uids, raw ids and imported docs | 3 | medium | S | M4 |
| 18 | Deterministic excerpt of oversized tool output for the curator | 2 | medium | M | M3/M4 |
| 19 | `kind` filter on search | 2, 3 | medium | S-M | M4 |
| 20 | `oboete inject` and the viewer show the real packet | 1 | medium | S | M4 |
| 21 | "This file has history" at read time | 2 | medium | M + op-format decision | M4 |
| 22 | Device enrollment by a one-time join code | release | medium | S | M6 |
| 23 | After a hub epoch change, relay replica ops and reset the pull cursor | 1 | medium | S | M6 |
| 24 | Strip claude-mem's context block from stored tool text | 2 | low-medium | S | now |

### 1. Stop recording and curating oboete's own memory-tool output
- **claude-mem:**
  - `isRecursiveMemoryTool` skips its own memory tools before any write (claude-mem `src/services/sqlite/tool-uses.ts:34-74,219-221`).
  - The cowork plugin excludes memory-tool calls from capture, as a guard against feedback loops (claude-mem `cowork/README.md:59-63`).
  - The shipped bundle does the same (claude-mem 13.28.0 `scripts/worker-service.cjs`, JFe/cQ).
- **oboete today:**
  - The PostToolUse hooks for Claude and Codex have no matcher (oboete `src/setup.rs:22-38`).
  - Capture stores the tool, its input and its output for every call (oboete `src/capture.rs:124-139`).
  - The curator window shows every tool line (oboete `src/curate.rs:347-363`).
  - The raw FTS index covers every event (oboete `src/consumer/fts.rs:95-110`).
  - No self-tool filter exists. `git grep -nE 'MCP_NAME|mcp__oboete|oboete (search|get|timeline)|mcp-search|isRecursive|memory tool' <branch> -- src/capture.rs src/curate.rs src/transcript.rs src/consumer/ src/hook.rs` finds only a comment on STRIP_BLOCKS (`src/hook.rs:21`) on origin/main c64b6db (which now contains m3/subs-anytime and m3/codex-limit), m3/gates-1, m3/gates, m3/digest-b2 and m2/manifest.
  - On m3/gates, an echoed old open item can come back as a current open item. The gates lower "tool result" drafts, but open items are delivered whatever their status (oboete `m3/gates:src/gates.rs:186-203,286-289`; `m3/gates:src/claims.rs:240-265`).
- **Proposal:**
  - In the PostToolUse arm of `capture::events`, recognize oboete's own tools:
    - names built per agent from `setup::MCP_NAME` (oboete `src/setup.rs:18-19`);
    - `oboete search|get|timeline` run through Bash;
    - claude-mem's mcp-search tools.
  - For those calls, keep the call record (tool name, gated query, returned uids and doc ids) and replace the output with an `omitted` marker.
  - Apply the same filter where transcript import rebuilds PostToolUse payloads (oboete `src/transcript.rs:113, 181-200`).
  - `curate` skips these lines under their own reason ("memory tool output"). It must not reuse the size marker, because M2 and B.2 keep size elision separate (oboete `docs/spec.md:1168`, `:1569`).
  - Fixture: a session searches a retracted decision and a finished open item. After curation, no current claim quotes either. The test must not claim "0 hits in raw" when the agent echoes the text in its reply (oboete `src/capture.rs:141-153`).
  - Fallback if raw must stay byte-complete: keep the output in raw and elide it only when the window is built (the cowork variant). That fixes curation but leaves the duplicate copies.
- **Value:** high. It stops retracted or finished items from coming back as current (goal 2), keeps duplicate memory text out of raw, and matches claude-mem.
- **Cost:** S.
- **Milestone:** now, before the M5 canary.
- **Verification:** held on existence, accuracy and fit in round 1 (the storage lane, plus the echo half of the cowork proposal).
  - The fit verifier noted that the forget argument is overstated. Forget by session or range already reaches these records, and claim forget leaves raw alone by design (oboete `docs/spec.md:635-637`).
  - Pattern secrets are already redacted in MCP output (oboete `src/mcp.rs:79-82`).

### 2. Write a device-rewind rule before the first push
- **claude-mem:** `dropStaleOriginDeviceIdSnapshots` runs before every flush. It dead-letters tombstones queued under a stale device id and re-snapshots live rows under the current id (claude-mem `src/services/sync/CloudSync.ts:1373-1431`, `:1493-1527`).
- **oboete today:**
  - Restore keeps the device id (oboete `src/backup.rs:351`; `src/raw.rs:1039,1046-1053`).
  - Restore then reuses seqs (oboete `src/backup.rs:425`). `next_seq` is MAX+1 (oboete `src/raw.rs:1205-1210`), and so is `op_seq` (`src/raw.rs:641-649`).
  - Records and ops are keyed by (device, seq) and (device, op_seq) (oboete `src/raw.rs:26`, `:50`).
  - The hub keys ops by origin device and seq (oboete `docs/spec.md:446`). It is idempotent by op id, with acks that carry the id and hash (`docs/spec.md:477-480`).
  - The rollback guard covers only a rolled-back hub (`docs/spec.md:504`).
  - A device span is a valid tombstone target (`docs/spec.md:632`).
- **Proposal:** add a rule to spec 5.8 now. After a restore that lands below what was already exported or pushed, do one of these:
  - (a) Take a new device id, as the copy path does (oboete `src/db.rs:272-310`). This conflicts with MUST-M15's "with its device id", so it is a spec decision.
  - (b) Fast-forward seq and op_seq past the highest value ever exported or pushed.
    - `src/backup.rs:67` is `Kind::top`, the current maximum, not an export high-water mark.
    - The export cursor is `src/backup.rs:198`, and restore sets aside the segments it would read (`src/backup.rs:389-401,422-425`).
    - So use the hub's acked high-water mark, or have backup keep its own high-water mark somewhere a restore cannot remove it.
  - Also: hub-protocol.md must say what the hub answers when a known op id arrives with a different hash. It must never be a silent re-ack. The donor refuses that case as `revision_hash_conflict` (see "Hub (M6)" under Engineering lessons).
  - Add both the stalled-push case and the misdirected-tombstone case to M4's restore fixtures.
- **Value:** high. Without the rule, two things go wrong:
  - A reused op_seq either never gets a matching ack, so the push stalls, or is deduplicated as the old op and lost.
  - An incoming tombstone for a pre-restore (device, seq) deletes an unrelated new record.
- **Cost:** S.
- **Milestone:** decide in spec 5.8 now; build at M6.
- **Verification:** held on all three in round 1. The fit verifier corrected the cursor source, as folded in above.

### 3. Forget also reaches quarantined and rebuilding copies
- **claude-mem:** bulk "forget a project" exists only in the paid Postgres backend (claude-mem `src/storage/postgres/data-deletion.ts:1-54`). Local SQLite has single-row delete only (claude-mem `src/services/worker/http/routes/DataRoutes.ts:120-124,333-383`). oboete's forget design already goes further.
- **oboete today:**
  - A damaged raw.db or knowledge.db is renamed to `*.quarantined-<ms>`, sidecars included (oboete `src/backup.rs:309-327`, `:521-535`).
  - Untrusted segments are set aside and "stay on disk" (`src/backup.rs:221-237`).
  - A failed rebuild leaves `knowledge.db.rebuilding-<ms>` behind (`src/worker.rs:476-507`).
  - None of these files is covered anywhere:
    - not in forget step 6, which covers backup segments only (`docs/spec.md:668`);
    - not in 6.3's list of limits (`docs/spec.md:690-716`);
    - not in the M4 grep (`docs/spec.md:818-819`).
  - So M4 fails on them, or passes only because the test home never quarantined anything.
- **Proposal:**
  - Forget step 6 deletes any such file that holds the scope. It deletes rather than rewrites, because a damaged SQLite file cannot be rewritten reliably. Decision 19 (no trash) supports this (`docs/spec.md:647-650`).
  - Otherwise, list the file as a 6.3 limit that forget prints and doctor names.
  - Add these files to the M4 grep list.
  - An automatic age-out may cover knowledge.db copies only, since they are derived. A quarantined raw.db, or a segment past raw's end, can hold the only copy of uncurated records. Those must never be discarded automatically (`docs/spec.md:685`).
- **Value:** high. It closes a real hole in the forget guarantee (goal 2).
- **Cost:** S.
- **Milestone:** M5.
- **Verification:** held on all three in round 1. The fit verifier narrowed the age-out to knowledge.db copies, as above.

### 4. Capture exclusion: prefix matching that cannot silently miss
- **claude-mem:**
  - `globToRegex` does its steps in this order: normalize backslashes, expand `~`, normalize the separators `homedir()` returned, escape, then substitute glob tokens (claude-mem `src/utils/project-filter.ts:6-27`).
  - The comment at `:7-11` records an earlier bug. Expanding `~` before normalizing turned a Windows pattern `~\projects\secret` into a literal that matched nothing, "so a configured exclusion silently stopped excluding".
  - Each pattern is compiled in its own try/catch. A bad one is logged and skipped, and the others still apply (`:40-50`, `:55-81`).
- **oboete today:**
  - Capture exclusion is decided but not built. Owner decision 16 lists it (`docs/spec.md:43`), and decision 27 requires it on the owner's machines from cut-over (`docs/spec.md:54`).
  - Spec 6.1's "Never record" row is `oboete capture exclude <repo or folder>`, setup, or `capture = false` in `.oboete.toml` (`docs/spec.md:619`). "The hook checks each event's repo key and working directory against the list before writing" (`docs/spec.md:625`).
  - No exclusion code exists in `src/capture.rs` or `src/config.rs` on main or the m3 branches (the round-2 existence verifier checked with grep).
  - Capture already canonicalizes the working directory: `cwd.canonicalize()` (`src/capture.rs:448`).
- **Proposal:** build it at M5 as the spec says, with repo keys and absolute folder prefixes, and no globs.
  - Normalize both separators. Expand a leading `~` only after separator normalization, which is the order claude-mem got wrong.
  - Canonicalize folder entries the same way capture canonicalizes the cwd. Otherwise an entry typed through a symlink (macOS `/var` → `/private/var`) never matches.
  - A folder that does not exist yet is kept and matched: canonicalize its longest existing ancestor and append the remaining components as typed. Canonicalize again at match time once more of it exists, so a symlink created later on the path still matches.
  - Compare on path components, so `/a/b` does not match `/a/bc`. Compare case-insensitively only where the volume is: Windows (NTFS, unless the folder carries the per-directory case-sensitive flag) and a macOS volume whose `pathconf(_PC_CASE_SENSITIVE)` is 0, asked of the entry's longest existing ancestor. A case-sensitive APFS volume compares exactly, so excluding `/work/Secret` does not stop capture under a distinct `/work/secret`.
  - Refuse a bad entry when `capture exclude` adds it: it must be absolute and non-empty.
    - Do not copy "log and skip". A skipped entry leaves its folder recorded, and after that only forget can remove it (`docs/spec.md:622`, `:627`).
    - The rest of the spec is fail-closed: "a session with a path that cannot be classified sends no content" (`docs/spec.md:457`).
  - Add a test for the separator/`~` order and for a symlinked entry. Add globs later, and only if someone asks for them.
- **Value:** high. It is a privacy prerequisite for the owner's switch (decision 27) and for public users.
- **Cost:** M.
- **Milestone:** M5 (Forget and safety), before cut-over.
- **Verification:** held on existence, accuracy and fit in round 2. The fit verifier supplied the three changes folded in above: refuse bad entries when they are added, canonicalize, and compare case-insensitively on macOS.

### 5. Short prompts in the Inject calibration
- **claude-mem:**
  - Per-prompt semantic injection is skipped for prompts under 20 characters. The client checks this (claude-mem `src/cli/handlers/session-init.ts:150-166`), and the server checks again before any score (claude-mem `src/services/worker/http/routes/SearchRoutes.ts:384-427`).
  - It is off by default (13.28.0 `scripts/worker-service.cjs`, `CLAUDE_MEM_SEMANTIC_INJECT:"false"`).
- **oboete today:**
  - The design has no length gate. It uses a full-text match plus a threshold (oboete `docs/spec.md:310-312`), calibrated on about 100 no-answer and 100 false-premise questions (`docs/spec.md:336-341`).
  - But the only query builders drop prompts under 15 characters (oboete `docs/eval/build_queries.py:46`; `docs/eval/build_english.py:23`).
  - The strata split only by language and by prompt versus agent (`docs/spec.md:1124`).
- **Proposal:**
  - Add a short-prompt stratum to the Inject no-answer and false-premise sets. It covers:
    - prompts of 3-14 characters, such as 認証のバグ直して;
    - prompts containing terms under 3 characters, which fall back to literal substrings (oboete `src/main.rs:102-104`, `src/mcp.rs:32-33`).
  - Keep gating on the threshold score, never on length.
- **Value:** high. Short, loaded prompts are common in Japanese, and the live hook will see them.
- **Cost:** S.
- **Milestone:** M4.
- **Verification:** held on all three in round 1. The fit verifier refined the mechanism: an 8-character Japanese prompt still produces trigrams, so the path that really changes is terms shorter than a trigram.

### 6. Fit the whole SessionStart packet once, dropping whole items
- **claude-mem:**
  - `fitContextToBudget` re-renders and drops whole items, cheapest first (claude-mem `src/services/context/ContextBudget.ts:9`, `:44-58`, `:66-89`).
  - The health warning is measured inside the budget (claude-mem `src/services/context/ContextBuilder.ts:249-283`).
  - Stats count what was delivered, not what was queried (`ContextBuilder.ts:262-303`).
- **oboete today:**
  - `render` drops whole sections, but it runs before the decisions exist (oboete `src/manifest.rs:104-159`).
  - `with_decisions` then inserts decisions and the digest into the already-capped text (oboete `src/consumer/manifest.rs:144-193`).
  - The hook cuts lines from the tail down to CAP 6,000 (oboete `src/hook.rs:318-322`). DECISIONS 15 × CLIP 400 comes to about 6,000 on its own (`src/consumer/manifest.rs:31-39`).
  - So on a repo with many decisions, the tail cut removes:
    - directives, todo and the last exchange;
    - files;
    - the "As of / not yet curated" line;
    - other sessions.
    It can also leave a heading with half its list.
  - Cursor's text is cut mid-string at 9,500 UTF-16 units (oboete `src/hook.rs:668-679`), because Cursor measures JS string length and drops the whole field above 10,000 (`docs/research/agent-adapters-2026-09-23.md:541`, `:547`). Every other agent is capped in code points: `manifest::cut` counts characters (`src/manifest.rs:108-115`, round-2 install lane).
  - All m3 branches are unchanged here.
- **Proposal:**
  - At M4, build every part once and fit the final gated and fenced text in the agent's own units. Cursor is the one agent whose limit is known to be in UTF-16 units (above), so its count stays `len_utf16`. Then no agent receives text past its truncation line.
  - Do not copy claude-mem's astral-character replacement (claude-mem `src/utils/bmp-safe.ts:1-16`). It guards against Claude Code cutting the instruction files it auto-loads (CLAUDE.md, AGENTS.md) at a UTF-16 boundary. oboete writes no memory into those files (a grep of `src/` for CLAUDE.md, AGENTS.md and .mdc finds no writer) and injects only through hook output (`src/hook.rs:236-243`). Both of its own cuts stop at char boundaries (`src/hook.rs:670-676`; `src/manifest.rs:108-115`).
  - Drop whole bullets in one fixed order (spec 4.9 already requires this, oboete `docs/spec.md:358`). Never leave a heading over a partial list.
  - Resume-critical parts outlive the decision index: risky git state, last failing command, todo, last exchange, and the as-of/backlog line. Drop the lowest-ranked decision bullets first, never the whole decisions section.
  - Health lines stay in the head that is never dropped, as the recording-failure line already does (oboete `src/hook.rs:180-193`). That includes MUST-M9's backlog line and 5.11's last-pull line. This part comes from a lane item that was not verified.
  - The per-agent cap and per-kind size come from the spec 1.5 settings, not from the single CAP.
  - The fit step returns what it dropped. That feeds M22's "manifest truncation per agent" (`docs/spec.md:1176`) and a local doctor line.
  - Record the drop order as Claude's call, which the owner can overrule.
- **Value:** high (goal 1).
- **Cost:** M.
- **Milestone:** M4.
- **Verification:** held on all three in round 1. The UTF-16 point rests on Cursor's documented limit. The round-2 install lane's surrogate-pair benefit (bmp-safe, not verified) was removed, because both of oboete's cuts already stop at char boundaries.

### 7. Inject into subagents (SubagentStart)
- **claude-mem:** a PreToolUse hook on `Task|Agent` fetches memories for the subagent's own prompt and prepends them (claude-mem `cowork/README.md:11-23`; `cowork/hooks/hooks.json:40-51`; `cowork/scripts/cmem-hook.mjs:438-454`).
- **oboete today:**
  - CLAUDE_EVENTS has no SubagentStart (oboete `src/setup.rs:22-30`). Injection runs only at SessionStart and at each agent's own injection point (oboete `src/hook.rs:207-220`).
  - The spec mentions subagents only for curation (`docs/spec.md:238`), and the plan marks even that as a ponytail gap (`docs/milestone-3-plan.md:259`).
  - SubagentStart context does reach a subagent. The ponytail plugin's hook says "SessionStart context is parent-thread only and never reaches subagents" and injects through SubagentStart for that reason (`~/.claude/plugins/marketplaces/ponytail/hooks/ponytail-subagent.js:2-6`).
- **Proposal:**
  - Add SubagentStart to CLAUDE_EVENTS. It injects a fenced, size-capped block from the precomputed packet: global preferences, current decisions and open items, and the manifest header.
  - Make it a new injection kind with its own on/off and size setting (decision 16).
  - Drop the PreToolUse-on-Task variant, for three reasons:
    - its context goes to the parent;
    - reaching the subagent would mean rewriting the owner's tool input;
    - per-prompt matching stays off until the Inject line passes (`docs/spec.md:339-341`).
- **Value:** high. The owner delegates heavily, and subagents now start without the owner's decisions (goal 2).
- **Cost:** M.
- **Milestone:** M4.
- **Verification:** held on all three in round 1 (the subagent half of the cowork proposal).
  - The round-2 research subagents started with context injected by that SubagentStart hook (`ponytail-subagent.js:2-6`), which directly confirms the route.
  - The round-2 remote lane rated the PreToolUse-on-Task variant low (not verified). Round 1 had already dropped that variant, so the two rounds do not conflict.

### 8. Anchored timeline with a before/after window
- **claude-mem:**
  - `timeline` resolves an anchor from an observation id, "S<n>", an ISO time or a query (claude-mem `src/services/worker/SearchManager.ts:746-872`).
  - `filterByDepth` slices a symmetric window around the anchor. When the anchor is missing, it silently falls back to the last item (claude-mem `src/services/worker/TimelineService.ts:10-37`).
  - Results are grouped by day (claude-mem `src/shared/timeline-formatting.ts:79`, used at `src/services/worker/SearchManager.ts:683`).
- **oboete today:**
  - MCP `TimelineArgs` has only all, repo and limit (oboete `src/mcp.rs:55-65`). Timeline lists sessions newest first (`src/search.rs:388-405`).
  - The building blocks exist:
    - claims carry a raw anchor (`docs/spec.md:260`);
    - raw can be read by seq range (`src/raw.rs:488`, `:494`, `:776`);
    - evidence is indexed by (device, seq) (`src/claims.rs:97-103`);
    - digests carry `through_device`/`through_seq` (`src/digest.rs:65-72`).
- **Proposal:**
  - `timeline` takes `around`, which can be a claim uid, `<device>:<seq>`, an imported doc id, or an ISO time.
  - It also takes capped `before`/`after` counts over raw seq order within the anchor's device and session. The window interleaves the session digest with the claims whose evidence falls in the span.
  - An unknown id returns an isError through `failed()`, never a silent fallback. Drop the fuzzy `query` mode: the agent can search first, then call timeline.
  - When the raw is not on this device (raw sync is off by default, `docs/spec.md:41`, `:449-450`), show that device's claims and digest over the span, labelled "raw not on this device".
  - Also:
    - enforce grants on direct ids (`docs/spec.md:568`);
    - fence the output (`docs/spec.md:372`);
    - label retracted and superseded claims;
    - keep session a read-side filter (`docs/spec.md:165`);
    - group by day only if dogfood shows long windows are hard to read.
  - `history=true` on timeline would be new spec surface; today it belongs to search (`docs/spec.md:368`).
- **Value:** high. It answers "why did we decide X" with the recorded context (goal 3) and shows "what was happening when work stopped" (goal 1).
- **Cost:** M. It is not in M4's build list (`docs/spec.md:1252-1260`), so it adds to scope.
- **Milestone:** M4.
- **Verification:** held on all three in round 1, for both the anchoring proposal and the depth-window proposal. The fit verifiers' trims are folded in above.

### 9. `oboete repo alias` as a control op that travels
- **claude-mem:** queries match `project IN (...) OR merged_into_project IN (...)` (claude-mem `src/services/context/ObservationCompiler.ts:78-101`, `:151-153`). A remap emits one rev-minted mutation across tables (claude-mem `src/services/sync/remap-outbox.ts:116-201`).
- **oboete today:**
  - The repo key is the origin URL, and otherwise the path (oboete `src/repo.rs:1-18`).
  - Design B filters on the exact key (`src/claims.rs:351`, `src/digest.rs:104`, `src/curate.rs:1087`, `src/search.rs:393,473`).
  - v1 moved path keys to their origin with `rekey_paths`. On main that runs only on the old oboete.db (`src/db.rs:352-397`; `src/mcp.rs:246,262`). So after cut-over, a repo that gains an origin loses its decisions from SessionStart.
  - The alias is already in the spec (`docs/spec.md:1460`; `docs/research/redesign-2026-09-24/constraints-synthesis.md:188`) but deferred (`docs/pr-c.md:28`).
  - The 5.4 list of ops that travel has no alias op (`docs/spec.md:435-443`).
- **Proposal:**
  - `oboete repo alias <from> <to>` is a control op in raw.db's op log, applied at read time: manifest, decisions, digests, search and supersedes candidates. No row is rewritten.
  - It travels with 5.4's control ops.
  - Alias automatically only a path key that becomes an origin key on the same checkout (v1 parity). Show an origin-to-origin change as a suggestion only, since the owner's settings are the only source of aliases (row 30-4).
  - Exclusion checks must match the whole alias group, on both the event side and the exclusion-list side (`docs/spec.md:452-458`). The hub applies aliases when it refuses excluded content, and the egress gate re-reads aliases together with the exclusion list.
  - A path-keyed `from` should not travel as-is, because folder paths stay on the device (`docs/spec.md:617-625`).
- **Value:** medium-high (goals 1 and 2; a regression from v1 for path-to-origin).
- **Cost:** S-M.
- **Milestone:** M4 locally, M6 for travel.
- **Verification:** held on all three in round 1's sync lane. The context lane's framing was refuted on existence (see Dropped). Both fit verdicts' caveats are folded in here.

### 10. Three structural redaction rules (URI password, Cookie, `Authorization: Token`)
- **claude-mem:**
  - The cowork hook's `clean(v, cap)` works in this order: strip private and system-tag regions, truncate, then redact (claude-mem `cowork/scripts/cmem-hook.mjs:163-168`).
  - Besides its credential regexes, it runs three structural detectors:
    - `KEYVALUE_RE` (`:124`): key=value with no minimum length;
    - `COOKIE_RE` (`:128`): Cookie, Set-Cookie and any Authorization header value of 4 or more characters;
    - `URI_RE` with `redactUriCredentials` (`:134-137`): redacts the userinfo only when it contains a password colon, so `ssh://git@github.com` survives.
  - These are applied at `:150-151`.
- **oboete today:** the coverage is broader overall. It has:
  - the bundled gitleaks rules plus oboete's own rules;
  - a scan of every stored byte;
  - a second pass at the egress gate;
  - a rescan when rules change;
  - an allowlist by value hash.
  But three shapes pass through (checked in `config/`):
  - A URI password: the only userinfo-in-URL rule is vendor-specific, `sidekiq-sensitive-url`, which matches only contribsys hosts (oboete `config/gitleaks.toml:2955-2957`).
  - A generic Cookie or Set-Cookie header: the only cookie rule is `gitlab-session-cookie` (`config/gitleaks.toml:2275`).
  - `Authorization: Token <x>` outside a curl line: `generic-bearer-token` needs the word "bearer" (`config/oboete-rules.toml:41-45`), and `generic-basic-auth` needs "basic" (`:48-52`).
  - A short `DB_PASSWORD=hunter2` also passes, because `generic-api-key` needs a value of 10-150 characters with entropy 3.5 (`config/gitleaks.toml:637-641`).
- **Proposal:**
  - Add three built-in rules to `config/oboete-rules.toml`:
    - `uri-userinfo-password`: the password part of `scheme://user:pass@host`, requiring a `:` inside the userinfo. The Rust regex engine has no lookaround, and the colon is enough to leave `ssh://git@github.com` alone.
    - `cookie-header`: the value of `Cookie:` and `Set-Cookie:`.
    - `authorization-token-scheme`: `Authorization: Token|token <value>`.
  - Each rule gets fixtures with split literals (MEMORY: test-secret-literals-must-be-split).
  - Do not add a short `password=x` rule yet. Built-in rules cannot be turned off, and an allowlist entry is "the SHA-256 of one exact value, never a pattern" (oboete `docs/spec.md:725`). So false positives such as `password=example` in docs could never be tuned away.
  - To size a candidate short-password rule, run a **dry run**, for example `redact::ranges` over a copy of raw in the dogfood user, and decide from the hit count. It cannot be the real rescan. A new rule changes the ruleset version (oboete `src/redact.rs:191`), and the rescan then tombstones each finding (oboete `src/consumer/rescan.rs:1-4`).
  - This is security scope: Claude writes it, and it is reviewed under rules/security.md.
- **Value:** medium. Connection strings, cookies and token headers are common in psql, redis-cli and `curl -v` output, and that output reaches the curator window (oboete `src/curate.rs:347-363`), which leaves the machine for a remote curator (`docs/spec.md:119`).
- **Cost:** S.
- **Milestone:** now. The rescan is already on main (`src/consumer/rescan.rs:1-4`), so there is no M5 dependency, contrary to the lane's milestone guess.
- **Verification:** held on existence, accuracy and fit in round 2.
  - The accuracy verifier counted 12 credential regexes, not 11. That does not change the proposal.
  - The fit verifier corrected two things: the rescan already exists, and the candidate-rule count must be a dry run. Both are folded in above.

### 11. Classify a rejected API key by its vetted code
- **claude-mem:** a typed error kind that includes `auth_invalid`, with a classifier per provider (claude-mem `src/services/worker/provider-errors.ts:1-73`; `ClaudeProvider.ts:66-176`; `GeminiProvider.ts:25-97`; `OpenRouterProvider.ts:73-176`).
- **oboete today:**
  - A 401 gets a flat 600 s outage cooldown (oboete `src/provider.rs:462`, `:31`).
  - `CallError` has no code field (`src/provider.rs:108-121`). The vetted code is only folded into the message (`src/provider.rs:633-636`), although KNOWN_CODES includes invalid_api_key, authentication_error and PERMISSION_DENIED (`src/provider.rs:794-817`).
  - A cooldown never counts toward the skip (`docs/milestone-3-plan.md:68`). So a revoked key re-sends each window every 10 minutes forever, and doctor only says "resting".
- **Proposal:**
  - Carry the vetted code on `CallError`. For a 401 or 403 carrying invalid_api_key, authentication_error or PERMISSION_DENIED, do one of two things:
    - back off by doubling up to 24 h, the 429 shape in `next_state` (`src/provider.rs:474-491`); or
    - put invalid_api_key alone on OWNER_HOLD, so doctor's existing line tells the owner (`src/setup.rs:1704-1706`).
  - Never key on a bare 401: OpenCode Go returns 401 for credit and monthly-limit errors too (`docs/spec.md:1626`).
  - Drop the proposal's second half ("treat a 400 with context_length_exceeded like a 413"). A 413 is not TooBig today; it counts toward the breaker (`src/provider.rs:389-410`; `cooldown_for` gives a 413 no cooldown at `:464`, so `next_state` counts it at `:481-482`), and `ceiling_hit` matches by equality (`src/budget.rs:58-97`). At most, keep 413s out of the breaker.
- **Value:** medium. The provider layer is an owner priority, and public users need to see that a key is bad. Egress is wasted, though not breached.
- **Cost:** S.
- **Milestone:** now.
- **Verification:** held on all three in round 1. The fit verifier corrected the egress rationale and part (2), as folded in above.

### 12. Transcript import checkpoints each batch
- **claude-mem:**
  - Ragtime runs one fresh session per file, with no checkpoint (claude-mem `ragtime/README.md:6-14`, `:29-63`).
  - Its transcript tailer saves the offset before it processes the lines (claude-mem `src/services/transcripts/watcher.ts:92-93` against `:99-102`; `state.ts:30`).
- **oboete today:**
  - The v1 import writes keys and a checkpoint in the same transaction (oboete `docs/spec.md:947-950`).
  - The transcript import's only guard is the per-session time cut (`docs/spec.md:987`, Claude's call, overrulable).
  - Raw records are keyed only by (device, seq) (`src/raw.rs:13-26`), and `oboete transcript` only prints fixtures (`src/transcript.rs:1-5`).
  - Cut-over step 2 depends on this import (`docs/spec.md:1002`), and the Transcript line allows at most one duplicated turn per session (`docs/spec.md:1178`).
- **Proposal:**
  - Write a checkpoint (transcript file, byte offset) in the same transaction as each short batch of rows, and redact before taking the lock.
  - Do not use one transaction per session. Hooks append with `BEGIN IMMEDIATE` (`src/raw.rs:273`, `:289`) under a p95 ≤ 20 ms line (`docs/spec.md:186`), and large writes already go in batches (`src/raw.rs:942-944`).
  - The spec should also say which records count as a session's boundary. Records from an earlier transcript import do not count, so a rerun duplicates rather than skips.
  - Fixture: kill the import mid-session, rerun it, and check every entry is present exactly once.
- **Value:** medium (the cut-over's resume depends on it).
- **Cost:** S.
- **Milestone:** M4.
- **Verification:** held on all three in round 1. The fit verifier corrected the failure mode (duplicates, not skips) and ruled out the per-session transaction.

### 13. doctor keeps going when one store is broken
- **claude-mem:** each check runs independently and is classified ok, warn or fail. The exit is 1 only when a required check fails (claude-mem `src/npx-cli/commands/doctor.ts:89-225`).
- **oboete today:**
  - `knowledge::open(home)?` (oboete `src/setup.rs:1641`), `db::open(home)?` (`:1664`) and `providers_db::open(home)?` (`:1695`) each end the command.
  - So a broken knowledge.db hides the raw.db integrity line, the agent wiring and MCP.
  - `backup::doctor` already turns its errors into lines (`src/backup.rs:580-633`).
- **Proposal:** run each section in its own closure. An Err becomes "cannot read X: <err>" plus an unhealthy entry. Keep the `_raw` shared-lock guard alive inside the knowledge section.
- **Value:** medium. doctor is needed most exactly when a store is damaged.
- **Cost:** S.
- **Milestone:** now.
- **Verification:** held on all three in round 1.

### 14. setup wires every agent even if one fails
- **claude-mem:** the installer has three severities, ABORT, FAIL_LOUD_PER_IDE and WARN_CONTINUE, and no silent one (claude-mem `src/npx-cli/install/error-taxonomy.ts:1-58`; `error-reporter.ts:1-90`).
- **oboete today:** `for a in agents { wire(a, &cmd, remove)?; }` (oboete `src/setup.rs:73-75`), with claude first (`src/setup.rs:14`). One unreadable claude settings file leaves the other six agents unwired.
- **Proposal:**
  - Collect each agent's error, continue, print a summary and exit non-zero.
  - A `HookCommand::current` failure still stops the whole run.
  - Give each error type its own remedy instead of one "fix the file" line: `claude_mcp` shells out to the claude CLI (`src/setup.rs:538`, `:600`).
  - Give the M7 install script the same severities: abort on a download or checksum failure, and a per-agent failure otherwise.
  - Give each known install failure one line saying what to do: checksum, attestation, unsupported OS or architecture, install directory not writable, no network. A full taxonomy framework is not needed for one static binary (round-2 install lane, not verified).
- **Value:** medium (public release, decision 8).
- **Cost:** S.
- **Milestone:** now; the install-script part at M7.
- **Verification:** held on all three in round 1.

### 15. Windows: kill the curator's whole process tree
- **claude-mem:** shutdown snapshots descendants before SIGTERM, and treats a record as alive if the root or any descendant lives (claude-mem `src/supervisor/shutdown.ts:23-31`, `:74-80`). On Windows it runs `taskkill /T /F` (`shutdown.ts:293-300`).
- **oboete today:** on Unix, `own_group` plus `killpg` kill the whole group (oboete `src/provider.rs:1446-1466`). On Windows only the child is killed, as the code's own note says (`src/provider.rs:1456`). A survivor keeps using quota and keeps holding the pipe and the scratch directory (`src/provider.rs:1531-1540`).
- **Proposal:**
  - Put each child in a Job Object with KILL_ON_JOB_CLOSE, inside the shared `own_group`/`kill_tree`. That also covers the isolation probes (oboete `src/isolation.rs:109,143,329,340`) and the codex app-server read that #168 merged. windows-sys is already a dependency (`Cargo.toml:32-34`).
  - Accept the race between spawn and assignment, and comment it.
  - Do not claim the same parity on Unix, where a curator in its own group outlives a dead worker.
- **Value:** medium. The owner uses Windows native (decision 6, `docs/spec.md:33`).
- **Cost:** S.
- **Milestone:** now or M5.
- **Verification:** held on all three in round 1.

### 16. Windows: detach the worker from the agent's console
- **claude-mem:** Windows-specific spawn quoting, a boot-failure probe and a 2-minute respawn cooldown (claude-mem `src/services/infrastructure/ProcessManager.ts:407-421`, `437-516`; `src/services/worker-spawner.ts:23-70`, `170-173`).
- **oboete today:** `spawn_detached` detaches only on Unix (`process_group(0)`). On Windows it only clears handle inheritance (oboete `src/hook.rs:909-942`). The worker stays up to 30 minutes after the owner stops (`docs/milestone-3-plan.md:66`), which is exactly when the terminal gets closed.
- **Proposal:**
  - Set `creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)`. Still to be checked on the owner's Windows machine: that the worker survives the terminal closing, that no console window appears, and that the curator CLIs it spawns open none either.
  - Do not use `DETACHED_PROCESS`. Curator CLIs spawned without flags (oboete `src/isolation.rs:88-95`, `src/provider.rs:1100`) would then likely open visible consoles.
  - Drop the proposed boot-error file. doctor already prints the last worker failure (`src/setup.rs:1659-1661`), and only `lock(home)?` can fail before the lock is taken (`src/worker.rs:334`).
  - Do not copy claude-mem's time-based respawn cooldown. oboete's worker is lock-gated and opens no port, so a failed start costs one detached spawn per hook and never blocks the agent (round-2 lifecycle lane; see "Where oboete is already ahead").
- **Value:** medium (Windows cut-over, `docs/spec.md:998`).
- **Cost:** S.
- **Milestone:** M5.
- **Verification:** held on all three in round 1. The fit verifier removed the boot-error file.

### 17. Batched `get` over claim uids, raw ids and imported docs
- **claude-mem:**
  - `get_observations` and `get_tool_uses` take `ids: []`. An empty array is not rejected but answered with `[]` (the schemas set no minimum length and the handlers return early), so the endpoint cannot page through the whole table, as its comment intends (`:64-68`) (claude-mem `src/services/worker/http/routes/DataRoutes.ts:51-58,69-88,193-206,266-277`; `plugin/skills/mem-search/SKILL.md:96-121`).
  - The MCP instructions split an index step from a raw-body step: `get_tool_uses([IDs])` is used "ONLY when the summary is not enough" (claude-mem `src/servers/mcp-server.ts:446-451`).
- **oboete today:**
  - MCP `get` takes one id and reads v1's oboete.db (oboete `src/mcp.rs:47-52`, `:174-193`). A raw `<device>:<seq>` is reachable only from the CLI (`src/main.rs:222-255`).
  - Decision index lines carry no uid (`src/consumer/manifest.rs:157-162`).
  - knowledge.db already holds the active view, edges and evidence (`src/claims.rs:80-150`), and `raw.after` hides tombstoned records (`src/raw.rs:909-935`).
- **Proposal:**
  - `ids` is a non-empty array of at most about 20. It resolves:
    - claim uids, where a short prefix must resolve without ambiguity within the caller's grant and repo;
    - raw ids;
    - imported doc ids.
  - Report not-found per id without failing the batch.
  - Apply a total byte cap, or head/tail for raw bodies, since fields reach 64 KiB (`src/capture.rs:19`).
  - Every claim carries its status and superseded label. Imported docs are labelled "status unknown" (`docs/spec.md:332-333`, `:368`). Each record is fenced (`docs/spec.md:372`).
  - Index lines carry the short uid, which covers the dropped progressive-disclosure item.
- **Value:** medium (goal 3). It also exposes the raw evidence layer to agents.
- **Cost:** S.
- **Milestone:** M4.
- **Verification:** held on all three in round 1.

### 18. Deterministic excerpt of oversized tool output for the curator
- **claude-mem:**
  - Fields over 16k characters go to a one-shot, 30-second model compression, with head/tail truncation as the fallback (claude-mem `src/services/worker/field-optimizer.ts:83-132`; `src/sdk/prompts.ts:276-288`).
  - Before it drops an Edit's full-file payload, it checks that the diff replays (claude-mem `field-optimizer.ts:138-174`).
- **oboete today:**
  - The first tool output that does not fit a window is shown only as "(N bytes of output, seen, elided)" (oboete `src/curate.rs:192-197`, `:447-452`).
  - With the default `tool_output = full`, fields are kept up to 64 KB (`src/capture.rs:19`; `src/config.rs:603-611`). So any output over about 17.8 KB (the 5,000-token window, `src/curate.rs:18-21`) is invisible to the curator. The non-default 8 KB head-tail setting always fits, so today the default shows the curator less than the reduced one.
- **Proposal:**
  - Render a deterministic excerpt inside the marker: the head, the tail, and lines matching error/panic/failed/exit code or path:line. Pass it through `redact::outbound_range` (`src/redact.rs:356-387`). There is no model call.
  - Fill the remaining window budget, since an elided piece closes its window (`src/curate.rs:186-205`).
  - This needs a multi-segment Source. Today Source is one contiguous range (`src/curate.rs:71-78`), and the sentence uid assumes that (`src/curate.rs:587-596`).
  - Keep `structuredPatch`. It is the only quotable text of an Edit, because quotes never come from a tool's input (`docs/milestone-3-plan.md:260`). Drop only `originalFile`, and only once it is confirmed that the hook payload carries it (not verified). Record the drop.
- **Value:** medium. Failures and their causes live in big outputs (goal 2).
- **Cost:** M (raised from S by the fit verifier).
- **Milestone:** M3/M4.
- **Verification:** held on all three in round 1.

### 19. `kind` filter on search
- **claude-mem:** `resolveTypeFilters` reconciles an overloaded `type` parameter (claude-mem `src/services/worker/SearchManager.ts:312-348`).
- **oboete today:** search takes query, all, repo and limit (oboete `src/mcp.rs:28-45`; `src/main.rs:101-111`). The claim kinds exist (`src/claims.rs:43-51`), and an unknown repo is already an error rather than zero hits (`src/mcp.rs:110-135`, `:93-95`).
- **Proposal:**
  - Add an optional `kind` as its own parameter: a closed enum of the seven claim kinds plus `prompt`. An unknown value is refused.
  - Leave out `imported`, which is a speaker and provenance, not a kind (`docs/spec.md:260`, `:331-334`).
  - Add `raw` only if the M4 Raw measurement adopts raw chunks (`docs/spec.md:1167`).
  - The vector side needs a claim-kind column and a re-embed, because `vec_docs.kind` is only p/k (`src/embed.rs:97-103`). Remote search needs the same in the DO's FTS and Vectorize (`docs/spec.md:570`).
- **Value:** medium. "Decisions about X" and "lessons about X" map straight onto goal 2.
- **Cost:** S-M.
- **Milestone:** M4.
- **Verification:** held on all three in round 1. The fit verifier removed `imported` and `raw` and raised the vector cost.

### 20. `oboete inject` and the viewer show the real packet
- **claude-mem:** a live, debounced preview of the exact injected text (claude-mem `src/ui/viewer/hooks/useContextPreview.ts:16-135`).
- **oboete today:** `oboete inject` prints `hook::inject_text` (oboete `src/main.rs:91-96`, `:300-302`; `src/hook.rs:329-350`). The viewer's Context view reads v1 summaries through `inject::context` (`src/view.rs:317-320`; `src/inject.rs:10-55`), which is text a Design B session never receives (`docs/spec.md:334`).
- **Proposal:**
  - At M4, both use the hook's packet function.
  - `--prompt` reads its text from stdin or a hidden prompt, never from argv (`docs/spec.md:1364`).
  - Show each block against its per-kind size, plus counts and reasons for what was cut, not the cut text itself.
  - Say which path produced the result: the shortlist or a fallback (`docs/spec.md:313`).
- **Value:** medium.
- **Cost:** S.
- **Milestone:** M4.
- **Verification:** held on all three in round 1.

### 21. "This file has history" at read time
- **claude-mem:**
  - A PreToolUse hook on Read, marked async (claude-mem `plugin/hooks/hooks.json:62-71`), injects a timeline of past observations about the file.
  - It skips files newer than the newest observation, skips subagents, and queries both path forms (claude-mem `src/cli/handlers/file-context.ts:15-21,45-138,140-292`; `:263-270`; `:142-149`).
  - An atomic, fail-open dedupe gate stops repeats (claude-mem `src/cli/handlers/file-context-dedupe.ts:1-58,140-159`).
  - Paths are also extracted from shell read commands, fail-open and capped at 10 (claude-mem `src/cli/adapters/codex-file-context.ts:1,87-152`).
- **oboete today:**
  - Claude has no PreToolUse (oboete `src/setup.rs:22-30`). Grok's PreToolUse is the once-per-session injection point (`src/hook.rs:208-214`).
  - Files touched are extracted for the manifest only (`src/consumer/manifest.rs:438-475`), and claims have no file field.
  - `hookstate::claim` is atomic, fail-open and pruned after 7 days (`src/hookstate.rs:1-7`, `:14`, `:27-36`).
- **Proposal:**
  - Compute each claim's file set at the origin (the curation window's touched files) and carry it on the claim op, so it travels.
    - A locally derived `claim_files` table misses prompt-anchored decisions and every claim from another device (raw sync is off by default: `docs/spec.md:41`, `:433-450`).
    - This changes the op format and needs its own decision.
  - Normalize paths to the worktree root when the fact is recorded, not at query time. claude-mem instead matches several candidate forms at query time (`src/services/sqlite/observations/get.ts:7-64`).
  - The worker precomputes, per checkout, a map from each file to its current lessons, fixes, decisions and open items. Imported or retracted claims are never included.
  - A synchronous PreToolUse for Claude Code `Read|Edit|Write` injects at most about 5 fenced one-line claims, with a status check per uid.
  - Dedupe with `hookstate::claim(session, "file-<hash>-<newest seq>")`. Skip subagents. Do not copy the mtime skip: decisions stay valid after a file changes.
  - Agent coverage:
    - agy and Cursor pre-tool hooks block the tool on a bad response, so do not register them (`docs/research/agent-adapters-2026-09-23.md:21,64,523,535,556`);
    - Codex has no Read tool and no PreToolUse wiring (`src/setup.rs:32-39`);
    - Grok shares its existing PreToolUse.
    - So this covers Claude Code first, plus Grok.
  - Turn it on by default only after its own labelled read-event set passes (decision 29).
- **Value:** medium. claude-mem ships this on by default, and it surfaces lessons where they matter (goal 2).
- **Cost:** M, plus the op-format decision.
- **Milestone:** M4.
- **Verification:** held on all three in round 1. The fit verifier found the two defects fixed above. The local extraction proposal is under "Considered and not adopted". Round 2's shell-read extraction survives only for the manifest (see Engineering lessons).

### 22. Device enrollment by a one-time join code
- **claude-mem:** the cloud-sync skill never prints the token or puts it in argv. It writes settings through a quoted heredoc, with mode 0600 (claude-mem `plugin/skills/cloud-sync/SKILL.md:16-18`, `:52-78`).
- **oboete today:**
  - `oboete hub device add` mints a token, and the DO keeps its SHA-256 (oboete `docs/spec.md:543`). Tokens live in a user-only file, never in argv or env (`docs/spec.md:545`).
  - How the token reaches the new device is not specified. "Device enrollment" is listed as security scope (`docs/spec.md:1307`), and approval-code machinery already exists for the MCP login (`docs/spec.md:563`).
- **Proposal:**
  - `device add` prints a short-lived, single-use join code.
  - The new device runs `oboete hub join`, reads the code from a hidden prompt, exchanges it over TLS, and writes the token with mode 0600.
  - doctor and status show only a hash prefix.
  - The spec must say how the first device gets its token at `hub deploy`.
- **Value:** medium (public-release safety).
- **Cost:** S.
- **Milestone:** M6.
- **Verification:** held on all three in round 1.

### 23. After a hub epoch change, relay replica ops and reset the pull cursor
- **claude-mem:** `handleEpoch` resets the cursor to 0, discards the stale page, and requeues native rows but not replica rows (claude-mem `src/services/sync/SyncApply.ts:356-413`, `:426-436`). So a lost device's history never returns to a rebuilt hub.
- **oboete today:** the rollback guard pushes tombstones first on a new epoch (oboete `docs/spec.md:504`), and re-seeding uses each device's own ops only (`docs/spec.md:503`). No pull-cursor reset is stated.
- **Proposal:** hub-protocol.md says that on a new epoch the device does the following:
  1. resets its pull cursor;
  2. pushes tombstones and withdrawals first;
  3. re-pushes its own ops, and relays the replica ops it holds for other devices. Relayed ops are deduplicated by origin device, seq and hash, and are sent only after the deny-list (`docs/spec.md:501`) and the send-time exclusion check (`docs/spec.md:453-467`).
  - The hub accepts relayed ops only after a new epoch, and only for keys it does not have.
  - A relay must be authorized, because the stored hash is not an origin signature: with the token binding under Hub (Engineering lessons), device A's token cannot push device B's ops, and exempting relays without proof would let A forge ops for B. So relaying runs only under an owner-issued, epoch-scoped recovery grant (for example `oboete hub recover` mints a one-epoch relay token on one device). The DO records relayed ops as relayed, with the relaying device, and accepts them only for origins that have not pushed in the new epoch; once an origin pushes, its own ops win and the grant no longer covers it.
  - Relaying needs other devices' op envelopes kept byte for byte. The spec does not say so yet: open question 17 (`docs/spec.md:1643`) only leaves open where the kept op log lives, so this requirement is added when that question is settled.
  - M4's wipe test (`docs/spec.md:506`) adds a relay case.
- **Value:** medium (goal 1 when a device is lost).
- **Cost:** S.
- **Milestone:** M6.
- **Verification:** held on all three in round 1.

### 24. Strip claude-mem's context block from stored tool text
- **claude-mem:** strips a fixed tag list that includes `claude-mem-context` (claude-mem `src/utils/tag-stripping.ts:4-46`), and writes such blocks into instruction files (claude-mem `src/utils/claude-md-utils.ts:57-75`).
- **oboete today:** `STRIP_BLOCKS` is ide_opened_file, hook_context and private (oboete `src/hook.rs:20-24`). It is used by capture (`src/capture.rs:338-355`) and by `redact::hidden`, which the curator's Prepared uses (`src/redact.rs:385-410`; `src/curate.rs:335-398`). A live block exists at `/home/jura/projects/wt-minimo-bidir-sync/AGENTS.md:1`.
- **Proposal:**
  - Add only `claude-mem-context`, with a test on the real AGENTS.md shape.
  - Do not add `oboete-memory`. oboete's own source contains that pair (oboete `src/manifest.rs:167-172`, `src/hook.rs:1126`), so capture would cut lines out of raw whenever an agent read oboete's code.
  - No separate curate change is needed.
- **Value:** low-medium. The gates already stop file content from becoming decided (oboete `docs/spec.md:274`, `:743`). The block is also rare, since claude-mem's folder CLAUDE.md is off by default (claude-mem `src/shared/SettingsDefaultsManager.ts:232`).
- **Cost:** S.
- **Milestone:** now.
- **Verification:** held on all three in round 1. The fit verifier trimmed it to the one tag.

## Engineering lessons

Labels:
- **not verified**: the item had no adversarial verdicts.
- **salvaged**: the item comes from a dropped or not-adopted proposal, and rests on the evidence of the verifier named.
- **round-2 lane**: the item is from a round-2 lane's gap analysis, not verified.

**provider.rs / curator spawning**
- **resetsAt unit guard: confirmed, and already fixed.**
  - claude-mem's `normalizeResetTimeMs` treats any value below 1e12 as seconds and multiplies it, and keeps larger values as milliseconds (claude-mem `src/services/worker/RateLimitStore.ts:153-156`).
  - oboete's claude and codex rest code multiplied by 1000 unconditionally. By round 1's analysis, a millisecond value would then clamp to the maximum rest, taking claude or codex out of the chain for about a week on a mere warning.
  - PR #168, merged to origin/main as c64b6db, adds `reset_ms` with the same 1e12 line (oboete origin/main c64b6db `src/provider.rs:1304-1310`). It is used by `claude_rest` (`:1328`) and `codex_rest` (`:1534`), and the test `a_reset_is_read_in_seconds_or_milliseconds` checks both units (`:1969-1975`).
  - Nothing is left to do.
- **Windows `.cmd`-only launcher** (salvaged from round-1 Dropped #1, part (b); confirmed by both its existence and fit verifiers; issue #169).
  - `on_path` counts `codex.cmd` as present (oboete `src/setup.rs:1898-1910`), and doctor prints "on PATH" (`src/setup.rs:1869-1874`).
  - But the curator spawns the bare name (`src/provider.rs:1100`), and Rust on Windows appends only `.exe` (Rust 1.98.1 std, `library/std/src/sys/process/windows.rs:461-516`, read in the installed toolchain under `$(rustc --print sysroot)/lib/rustlib/src/rust/`), so the spawn fails (`src/provider.rs:1481-1486`).
  - Fix: `on_path` returns the name that matched, and the curator spawns that name. Rust escapes `.cmd` batch lines itself (same file, `:294-315`), so `where.exe` is not needed. Alternatively, doctor marks the provider as not runnable.
  - The same code is on m3/gates, m3/digest-b2 and m3/codex-limit.
- **One recorded real `rate_limit_event`** (salvaged from Dropped #26, fit verifier).
  - Handwritten test JSON can drift from what the CLI sends, as the seconds/milliseconds fix shows.
  - Add one redacted `rate_limit_event` from an ordinary call as a test fixture. The real shape is already recorded in `docs/spike/curator-isolation.md`, Appendix C item 1 (per the fit verifier).
  - Do not try to record a limit-hit result: Claude decision C1 rests the curator before any limit is reached, so recording one would burn the owner's quota.
- **Curator tool attempts** (round-2 lane, not verified, low).
  - claude-mem appends every denied observer tool attempt to an NDJSON audit log (claude-mem `src/utils/observer-audit.ts:94-102`, called from `src/sdk/hardened-options.ts:127-149`).
  - oboete discards the answer with a fixed message and records that message in `provider_calls` (oboete `src/provider.rs:1246-1283`, `:393-397`; `src/providers_db.rs:21-40`). It does not name the tool, and doctor does not count these calls.
  - Add the `tool_use.name` values, each validated against `^[A-Za-z_][A-Za-z0-9_]{0,63}$` (otherwise recorded as "other"), to that detail. Never record the tool input. Add one doctor line with the count over 7 days and the last window.
- **Embedding line in doctor** (salvaged from Dropped #23, fit verifier).
  - Build spec 7.6's line ("rows waiting for embedding, with the embedder's last error, and whether a worker runs now", oboete `docs/spec.md:1036`) at M4, with no new stored state:
    - log embedding calls in the `provider_calls` ledger (`src/providers_db.rs:22-38`);
    - count waiting rows live;
    - check the worker lock live (`src/worker.rs:84-115`).

**curate.rs**
- **Tie the prompt's lists to the constants** (salvaged from Dropped #27; held on existence and fit).
  - claude-mem's `type_guidance` prose drifted from its type registry: it described "6 options" and never listed two types (claude-mem `CHANGELOG.md:875`).
  - In oboete, the prompt prose lists kinds, statuses and speakers by hand (oboete `src/curate.rs:1234-1239`). Only the schema's kind enum uses `claims::KINDS` (`src/curate.rs:1269`). The status and speaker enums are literals (`:1270-1273`), and `claims::STATUSES` is private (`src/claims.rs:54`).
  - An older 6-entry `curate::KINDS` still exists (`src/curate.rs:644`, used from `src/import.rs:283`).
  - Add a test that every kind, status and speaker the gates accept appears in `curate::prompt()`. Scope is excluded, because the prompt says only "scope: repo" by design (`docs/spec.md:270`).
  - Better: build the schema's status and speaker lists from constants.
- **Line-wrapped base64** (low): join 76-character base64 lines in `base64_runs` only if dogfood raw shows one (oboete `src/capture.rs:381`).

**hook.rs / capture / manifest**
- **ANSI/C0 stripping** (low): strip escape sequences from injected text in `consumer::manifest::one_line` (oboete `src/consumer/manifest.rs:480-494`, `:611-616`), as claude-mem does for agy (claude-mem `src/cli/adapters/antigravity-cli.ts:126-127`).
- **apply_patch parsing** (low): read `*** ... File:` markers only between `*** Begin/End Patch` and outside hunk bodies (oboete `src/consumer/manifest.rs:438-475`). claude-mem has the same weakness (`src/services/transcripts/processor.ts:201-209,302-321`).
- **Files read through the shell, for the manifest only** (salvaged from the not-adopted codex-file-context item, fit verifier).
  - The manifest's files come only from `file_path`, `notebook_path` and `target_file`, plus Codex apply_patch lines (oboete `src/consumer/manifest.rs:438-460`). Reads done with `cat` or `sed -n` in a Bash call are missing.
  - Add a small token scan for read commands:
    - fail-open;
    - existence checked at capture time and stored in the tool body, so replay stays deterministic;
    - cap of 10;
    - feeds the manifest's `file` facts only.
  - Do not feed spec 5.5's touch set from it. That touch set (`docs/spec.md:457`) has to be a separate, fail-closed classifier at PR-H:
    - an unclassifiable path marks the session unclassifiable;
    - directories count;
    - there is no cap;
    - paths map to repo keys.
- **An owner-visible line for recording failure** (salvaged from Dropped #25, fit verifier).
  - The recording-failure line reaches only the model, as additionalContext (oboete `src/failure.rs:233-237`; `src/hook.rs:166-186`, `:238-243`).
  - Check each agent's hook documentation for a user-visible field first. This is not verified for Claude Code or Codex.
  - Show only recording failure and OWNER_HOLD (`src/setup.rs:1703-1707`). Never show overdue windows: they are normal after a break, because the next hook starts the worker (`src/setup.rs:1719-1720`; `docs/milestone-3-plan.md:66`).
  - No promo or community lines.
- **Stop re-entry** (round-2 lane, not verified, low).
  - claude-mem skips a summary when `stopHookActive` is set (claude-mem `src/cli/handlers/summarize.ts:56-114`). oboete's Stop path does not read `stop_hook_active` (oboete `src/capture.rs:142-155`).
  - Add one capture test: a re-entered Stop with the same last message yields one reply record.
- **An unknown agent is loud** (round-2 lane, not verified, low).
  - An agent name outside AGENTS returns Ok with no message (oboete `src/hook.rs:82-89`).
  - Return an error, so main's fail-open path logs it.
  - Do not copy claude-mem's raw-adapter default, which turns typos into generic parsing (claude-mem `src/cli/adapters/index.ts:9-19`).
- **OpenCode capture kill timer** (round-2 lane, not verified, low): `send()` spawns with no kill timer (oboete `src/opencode.js:24-35`). Add the 5 s kill that Pi's extension has (`src/pi.ts:19-30`).
- **Conditional backfill** (low): only if the gaps table shows persistent shortfalls, backfill at SessionEnd with a byte offset per transcript that resets to 0 when the file shrinks (claude-mem `src/services/transcripts/watcher.ts:74-77`; oboete `src/consumer/gaps.rs:1-3`).

**Adapters (M7-M8)**
- **OpenCode v1 line** (salvaged from Dropped #24; its existence verifier confirmed the v1 gap is not planned, and its fit verifier held).
  - oboete's plugin exports only the v2 `setup()` (oboete `src/opencode.js:13-15`), and setup installs it with no version check (`src/setup.rs:93-100`).
  - The v1 line is still published (`docs/research/agent-adapters-2026-09-23.md:327`, `:494`), and claude-mem targets its hooks (claude-mem `src/integrations/opencode-plugin/index.ts:32-45`).
  - Add `server()` to the same default export. Include `experimental.chat.system.transform` for injection (`agent-adapters-2026-09-23.md:461`).
  - Extend the existing Node harness (oboete `src/setup.rs:2085-2142`) with v1 payloads, rather than adding a grep test.
  - Minimum version: setup and doctor detect v1 and say it is unsupported.
  - Value: medium (public release). Cost: M.

**search.rs / mcp.rs / viewer (M4)**
- **since/until on the vector side** (salvaged from round-1 Dropped #7): make `ts` a metadata column filtered inside the KNN query. The deeper-fetch loop stops at sqlite-vec's k cap of 4,096 (oboete `src/embed.rs:106-129`; vec0 schema `src/db.rs:513-516`). Test with more than 4,096 older on-topic vectors.
- **Search fallback** (low): when search falls back to full-text, add one line to the tool result (oboete `src/search.rs:114`) and a local daily counter shown in doctor.
- **Paging hints** (low): fetch limit+1 and end with "more results: narrow with since/until/kind" (claude-mem `src/services/worker/PaginationHelper.ts:97-112`). Raw hits show the stored size and `original_bytes`.
- **Default-value test** (low): check that each documented MCP default and cap is what the handler applies (oboete `src/mcp.rs:42-44`, `:62-63`, `:155`, `:210`). claude-mem documents 500 but applies 20 (claude-mem `src/services/worker/knowledge/CorpusBuilder.ts:26-144`).
- **Embedding failures** (low): give the M4 embedding consumer timed backoff after consecutive failed batches (claude-mem `src/services/sync/ChromaSync.ts:131-140,1158-1200`).
- **Viewer status line** (low): build it from `worker::running`, pending windows and provider rests (oboete `src/worker.rs:465`; `src/setup.rs:1702-1735`).

**Stores / migrations**
- **Where to check `user_version`** (salvaged from round-1 Dropped #10), when spec 7.3 step 2 is built:
  - Not in `knowledge::open`: readers hold raw's shared lock (oboete `src/main.rs:505-507`), and rebuild takes it exclusively (`src/raw.rs:250`).
  - Not in `raw::open`: hooks append through it (`src/raw.rs:172`).
  - The worker checks under its own lock, and readers treat a mismatch as "not built yet".
  - The round-2 lifecycle lane adds one point: knowledge.db has no version today, and `CREATE TABLE IF NOT EXISTS` never upgrades a derived table (oboete `src/knowledge.rs:14-31`; `src/claims.rs:81-149`). Stamp it and rebuild on mismatch through `worker::rebuild` (`src/worker.rs:475-479`). This is the structural answer to claude-mem's `carryLiveColumnsOntoNewTable` patch (claude-mem `src/services/sqlite/SessionStore.ts:1194-1211`).
- **doctor FTS check** (low; oboete's own gap, not a claude-mem mechanism): knowledge.db gets only a `quick_check` when it is opened for the worker (oboete `src/backup.rs:527`), and doctor's full `integrity_check` covers raw.db only (`src/backup.rs:623-630`). Run the FTS5 `integrity-check` command on `raw_fts` and `claims_fts` in doctor and offer `oboete rebuild` (`src/main.rs:70-72`). claude-mem has an FTS rebuild (claude-mem `src/services/sqlite/SessionStore.ts:3705-3716`) and, by grep, no FTS check.

**Hub (M6)**
- **Hash what was actually sent** (salvaged from round-1 Dropped #12): compute the op hash over the bytes actually sent after the egress gate, not over the stored `ops.body`. The gate can rewrite a string literal (oboete `docs/spec.md:719`; `src/redact.rs:995-1005`). Put golden vectors in the Worker's own JS tests.
- **The donor's same-revision rule** (from Dropped #20's accuracy verdict).
  - The donor refuses a push whose `entity_rev` matches the stored one but whose hash differs, as `revision_hash_conflict`. It acks only on an exact hash match (claude-mem `services/sync-api/src/store.ts:344-351`; `workers/sync-hub/src/do/SyncHub.ts:410-412`).
  - `canonicalJson` normalizes `-0` to 0 rather than refusing it (`services/sync-api/src/canonical-content.ts:74`).
  - oboete does not port the per-entity rev check (oboete `docs/spec.md:406`). Rec 2 still needs hub-protocol.md to state the answer for a known op id with a different hash.
- **Bind each device token to its origin device id** (round-2 lane, not verified, low).
  - When a token first pushes, the DO records that token's origin device id, and refuses ops from any other origin with 403. The push loop selects only ops from its own origin. The only exception is Recommendation 23's relay, under its epoch-scoped recovery grant.
  - Keep a retired id in meta, so that id's unsynced ops can still be pushed.
  - claude-mem's single identity source is enforced by convention only (claude-mem `src/services/sync/SyncApply.ts:16-20`).
- **Free-plan refusal is a wait** (oboete's own need, salvaged from Dropped #29's fit verifier; not a claude-mem mechanism).
  - The spec says only that operations fail once a Free limit is exceeded (oboete `docs/spec.md:420-422`). Hub spike item 7 needs a throttled first push to resume across days (`docs/research/redesign-2026-09-24/hub-platform.md:140-142`).
  - Treat the refusal as a pending wait until 00:00 UTC, the same pattern as curation waits (`docs/spec.md:246`).
  - Count rows actually written, because index and FTS rows count too. Label the DO's count a lower bound, since the limits apply to the whole account.
  - This does not apply to the owner, whose account is Workers Paid (`docs/spec.md:65`, `:418`).
- **Web sessions: an owner question and a spike** (salvaged from Dropped #19 and not-adopted remote-mcp-recall, both fit verifiers).
  - Today a claude.ai/code session records nothing and gets no manifest:
    - setup wires only the user-level `~/.claude/settings.json` (oboete `src/setup.rs:514-519`, `:727-729`);
    - the lane's reading of code.claude.com's cloud-session docs is that only a repo's `.claude/settings.json` hooks and `.mcp.json` reach the session;
    - no spec text mentions claude.ai/code (existence verifier's grep).
  - Recording there needs a token in the cloud environment. That changes owner-settled section 5 (decision 18, `docs/spec.md:45`; the no-env rule at `:545`), so it goes to the owner first (the question in 要約).
  - Then run a spike covering:
    - the install path;
    - whether the proxy rewrites the clone's origin URL, which `src/repo.rs` keys on;
    - a token and device model that still passes the egress gate's hub read (`docs/spec.md:459`).
  - When hub spike item 8 runs (`hub-platform.md:143-146`), also connect once from a claude.ai/code session and write down whether the connector appears.
- **Hub OAuth redirects** (round-2 lane, not verified, low): in the M6 security review, validate `redirect_uri` and any `next` parameter against the exact origin and path, and refuse protocol-relative paths. claude-mem does this for its installer pairing (claude-mem `src/npx-cli/commands/install.ts:1372-1389`, `:1449-1452`).
- **Smaller hub items** (low):
  - Carry the hub head in push and status responses, so a head above the local pull cursor triggers an immediate pull. The donor's push and status answers already carry `head_seq` (claude-mem `workers/sync-hub/src/do/SyncHub.ts:482`, `:551-561`). oboete pulls at start, before a packet refresh, and then on an interval (oboete `docs/spec.md:528-537`).
  - Make seq an `INTEGER PRIMARY KEY AUTOINCREMENT`, so a pull cursor is a plain rowid range. The donor keeps seq as text and finds a cursor's row by a binary search over rowid, which holds only because rows are never deleted except by a reset (claude-mem `workers/sync-hub/src/do/SyncHub.ts:812-892`). On a tombstone, oboete's hub removes the payload (oboete `docs/spec.md:497`): blank it rather than delete the row.
  - doctor sends one bogus-token request and expects 401 (claude-mem `workers/sync-hub/src/control-plane-probe.ts:104-203`).
  - Authenticate by looking up the token's hash. claude-mem compares internal secrets with `===` (claude-mem `workers/sync-hub/src/index.ts:305-426`).
  - Give the hub's excluded-repo refusal (oboete `docs/spec.md:458`, which names no status code) a code other than the auth failure's. The donor answers auth failures with 403 (claude-mem `workers/sync-hub/src/index.ts:390-396`), so a shared 403 would read as a bad token.

**Install, uninstall and docs (M7)**
- "Other MCP clients" README section, saying in plain words that nothing is recorded there (claude-mem `src/services/integrations/McpIntegrations.ts:103-104`).
- A line saying that `smart_*` code navigation is not part of oboete. claude-mem's MCP server offers `smart_search`, `smart_unfold` and `smart_outline` (claude-mem `src/servers/mcp-server.ts:701`, `:738`, `:782`), so users moving over will look for them.
- setup's closing text says when memory first appears, as claude-mem's installer does ("Memory injection starts on your second session in a project.", claude-mem `src/npx-cli/commands/install.ts:2590-2593`).
- A release job check that the tag equals the Cargo.toml version. oboete has no release job yet (origin/main has only `ci.yml` and `open-code-review.yml` under `.github/workflows/`). claude-mem publishes on any `v*` tag without such a check (claude-mem `.github/workflows/npm-publish.yml:3-6`, `:24`) and keeps its versions in step by a manual check in its version-bump skill (`plugin/skills/version-bump/SKILL.md:31`).
- **Uninstall note** (round-2 lane, not verified, low): `setup --remove` prints nothing about the data it kept (oboete `src/setup.rs:74-79` prints only on install). Print the home path, the delete command, and the `.oboete.bak` files left behind, as claude-mem does (claude-mem `src/npx-cli/commands/uninstall.ts:394-400`).
- **Progress line** (round-2 lane, not verified, low): on a TTY only, one rewritten progress line for the bge-m3 download, the update download and transcript import (claude-mem's heartbeat: `src/npx-cli/commands/install.ts:119-135`).
- **Status line** (round-2 lane, not verified, low, M7 or later): `oboete status --line` prints records for this repo, the curation backlog and the recording-failed flag. On any error it prints an empty line and exits 0 (claude-mem `plugin/scripts/statusline-counts.js:1-40`).

## Where oboete is already ahead

Sources of the measurements:
- **hook p95**: oboete's own milestone-2 measurement (Task 12), quoted in `src/capture.rs:15-18`. It is 25.5 ms on Windows, with a 25.2 ms floor on the iMac. This is still above the spec's provisional target of 20 ms or less (`docs/spec.md:186`).
- **nDCG@10**: oboete's own evaluation on the owner's data, `docs/spec.md:364`. The spec calls the 0.545 provisional: it was judged by claude-sonnet-5 before the judge's trust condition was measured, and it is judged again if gate B3 fails (`docs/spec.md:1139-1140`). Neither number comes from the lanes.

- **Capture path**
  - Hooks append straight to raw.db with no worker RPC, and fail open with exit 0 (oboete `src/hook.rs:106-120`; `src/main.rs:287-292`).
  - So claude-mem's worker-unavailable classifier, dual runtime fallback, async hooks and SessionEnd replay queue are unnecessary (`src/hook.rs:106-142`).
  - A failed write is not silent: the next SessionStart says recording has failed since T (`docs/spec.md:218`). claude-mem's cowork hook instead swallows every error, and can exit before an un-awaited request finishes (claude-mem `cowork/scripts/cmem-hook.mjs:392-395`, `:516-526`; round-2 remote lane).
  - Capture goes to a durable local file (raw.db, synchronous=FULL, `docs/spec.md:214-219`), so claude-mem's retry spool for failed sends is not needed (round-2 remote lane).
  - No circuit breaker is needed (`openclaw-circuit-breaker`, round-2 adapters lane). claude-mem's OpenClaw plugin opens a breaker for 30 s after 3 failed worker calls (claude-mem `openclaw/src/index.ts:243-299`). oboete's hooks make no worker call: a hook writes raw.db, then starts the worker, and a failed start is only logged (oboete `src/hook.rs:141-145`). The one unbounded wait is the OpenCode kill-timer lesson under Engineering lessons.
- **Agent coverage**
  - oboete covers seven agents: Claude Code, Codex, Grok Build, agy, OpenCode, Pi and Cursor (oboete `src/setup.rs:14`).
  - That is a **different** set from claude-mem's, not a larger one:
    - claude-mem's adapters are claude-code, codex, cursor, windsurf and antigravity, plus a raw fallback (claude-mem `src/cli/adapters/index.ts:9-19`);
    - it adds an OpenCode plugin (`src/integrations/opencode-plugin/`), an OpenClaw installer (`src/services/integrations/OpenClawInstaller.ts`) and cowork for cloud sessions (`cowork/`).
  - Its Grok integration is a transcript schema for "Grok Bot" exports (claude-mem `src/services/integrations/GrokBotInstaller.ts:7-10`), not the Grok Build CLI.
  - Only oboete covers Pi and Grok Build. Only claude-mem covers Windsurf, OpenClaw and cloud sessions.
  - Each agent has its own output shape and limit (oboete `src/hook.rs:237-244`, `:668-680`).
  - agy findings were taken from claude-mem and corrected where it was wrong (`docs/research/agent-adapters-2026-09-23.md:59,81,99,109`).
  - Cursor SessionEnd backfills missed turns (`src/hook.rs:494-520`). Session ids are hashed before they become paths (`src/hookstate.rs:20-26`).
  - Once-per-session injection is an atomic claim on the session (`src/hook.rs:208-214`). claude-mem instead uses a 2-second window keyed on the prompt text (claude-mem `openclaw/src/index.ts:693-702`; round-2 adapters lane).
- **Install and settings**
  - A single binary with its absolute path baked in and SQLite bundled (`src/setup.rs:150-172`; `Cargo.toml:15`). There is no Bun, npm or uv to install, probe or verify (`docs/spec.md:856`).
  - Settings writes refuse an unparseable file, keep a one-time `.oboete.bak`, and keep key order (oboete `src/setup.rs:353-376`, `:396-475`; round-2 install lane). claude-mem falls back to `{}` on a parse failure (claude-mem `src/npx-cli/commands/install.ts:853-856`).
  - A hook entry is judged to be oboete's by parsing its command, not by matching a value (oboete `src/setup.rs:252-276`). So claude-mem's uninstall false positive cannot happen (claude-mem `src/npx-cli/commands/uninstall.ts:132-170`).
  - No install marker is needed (`install-marker-versioning`, round-2 install lane). claude-mem writes a `.install-version` marker to skip a slow Bun dependency install (claude-mem `src/npx-cli/install/setup-runtime.ts:508-533`). oboete has no dependency install (`docs/spec.md:856`), and a repeated setup just prints "already up to date" (`src/setup.rs:121-126`).
  - `openclaw-installer` (round-2 adapters lane): claude-mem's OpenClaw installer fills only unset fields (claude-mem `src/services/integrations/OpenClawInstaller.ts:92-126`) but stops on a broken openclaw.json with a raw parse error (`:74-84`). oboete's setup already keeps fields it does not own (oboete `src/setup.rs:1174-1175`) and refuses a JSONC or empty config with a message (`src/setup.rs:1143-1157`; test at `:1971`). OpenClaw itself: see "Considered and not adopted".
- **Curation**
  - Curator isolation is checked on every claude call's init event, and the answer is thrown away if any tool, MCP server or plugin appears (oboete `src/provider.rs:1251-1290`). That catches a newly added CLI tool by what it can do, not by a hand-kept list. claude-mem admits that on the CLI path its deny list is the only layer (claude-mem `src/sdk/hardened-options.ts:34-38`).
  - Windows are stateless and bounded (`src/curate.rs:18-21`, `:1222`), with hard deadlines on CLI and HTTP calls (`src/provider.rs:1474-1535`, `:578-579`), so no recycling and no pacer are needed.
  - Curator calls are one-shots with no session to resume (`docs/spec.md:700`). So claude-mem's dual session-id architecture and prompt-to-session repairs have nothing to guard (round-2 remote lane).
  - No queue service is needed (`server-beta-worker-split`, round-2 remote lane). claude-mem's server beta splits an HTTP service from a BullMQ generation worker and makes jobs idempotent by content-addressed ids (claude-mem `docs/server.md:34`, `:92-94`). oboete gets the same two guarantees without a queue: hooks never wait on AI (`docs/spec.md:187`), a claim seen by two windows is committed once (`:241`), and the checkpoint moves in the same transaction as the window's knowledge (`:243`).
  - Explicit memory. claude-mem's manual save (`POST /api/memory/save`) stores the text as a "discovery" observation of one project (claude-mem `src/services/worker/http/routes/MemoryRoutes.ts:26-57`). Such rows have no concepts, so they never pass the SessionStart mode filter; only the Grok Bot index reads them (claude-mem `src/services/context/ObservationCompiler.ts:59-62`; `src/services/integrations/GrokBotIndexWriter.ts:203`). oboete's `oboete pref add` (`m3/gates:src/main.rs:57-59`, `:121-126`) stores the owner's words whole, through the same redaction gate, as a decided global preference (`m3/gates:src/claims.rs:143-179`; `m3/gates:src/capture.rs:246-258`), which spec 3.3 makes the only route to global scope besides the viewer (`docs/spec.md:270`). A repo-level decision needs no separate command: the owner's prompt is captured, and a verbatim user quote is what makes a claim decided (`docs/spec.md:267`).
  - Skip reasons are a typed enum, not string prefixes (`src/provider.rs:52-67`).
  - Cooldowns end at the provider's own reset (`src/provider.rs:449-491`) and are kept in providers.db across runs. A single worker lock makes re-probes single-flight (`src/worker.rs:84-116`).
  - Error bodies are never stored (`src/provider.rs:794-850`; test `:2352`), which is stronger than claude-mem's denylist scrubber.
  - A strict JSON schema: an invalid answer moves down the chain instead of being classified from prose (`src/provider.rs:568-573`, `:318-352`).
  - Long sessions are paged, never middle-dropped (`src/curate.rs:192-196`; `docs/spec.md:1168`). claude-mem drops the middle (claude-mem `src/server/generation/ProviderObservationGenerator.ts:64-131`).
- **Storage, identity and privacy**
  - The repo key is the origin URL, and a linked worktree maps to its main repository (oboete `src/repo.rs:1-18`). So memory never orphans under a worktree name, and claude-mem's merged-worktree adoption is not needed (claude-mem `src/services/infrastructure/WorktreeAdoption.ts:142-150`; round-2 lifecycle lane).
  - The same key replaces automatic project naming (`auto-project-naming`, round-2 remote lane). claude-mem's cowork plugin names a project from the last folder name, cut to 40 characters, with a fixed list of generic folders sent to one shared bucket (claude-mem `cowork/scripts/cmem-hook.mjs:72-82`). Two long folder names can collide there; an origin URL cannot. Whether a cloud clone keeps the same origin URL is part of the web-sessions spike under Hub.
  - raw.db migrations only add columns or tables, and knowledge.db is derived (`docs/spec.md:928-933`). So the failure class behind claude-mem's `carryLiveColumnsOntoNewTable` (#3849, #3890) cannot occur (claude-mem `src/services/sqlite/SessionStore.ts:1188-1211`).
  - The device id is minted in the same transaction as the store it labels, and open fails without it (oboete `src/db.rs:275-314`; `src/raw.rs:196-200`; round-2 lifecycle lane).
  - Nested `<private>` handling, where an unclosed tag hides the rest (oboete `src/hook.rs:681-749`). An all-private turn records nothing (`src/capture.rs:107-112`).
  - Fields are cut to 64 KB head+tail, redacted before the cut, and the stored JSON stays valid (`src/capture.rs:19`; `src/redact.rs:569-620`).
  - Images and base64 are replaced at capture, by shape and by length (`src/capture.rs:357-400`).
  - Claim uids come from the evidence sentence (`src/claims.rs:72-79`), so claude-mem's title+narrative hash collision cannot happen.
  - The repo is an immutable label on append-only records (`src/raw.rs:1-3`, `:13-27`): no reparenting triggers, no foreign keys, no orphan repair.
  - knowledge.db is derived and rebuilt from raw when damaged (`src/backup.rs:525-540`).
  - The FTS query is quoted per trigram, and LIKE terms are escaped (`src/search.rs:170-212`).
  - Forget is designed local-first with bulk scopes (`docs/spec.md:632`, `:651-673`). claude-mem offers bulk forget only in its paid cloud.
- **Delivery and search**
  - One hybrid search entry point with RRF (oboete `src/search.rs:96-172`): nDCG@10 0.545 against claude-mem's 0.244 (source above; provisional until gate B3, `docs/spec.md:1140`).
  - Japanese search works. claude-mem's own tracker says its unicode61 FTS cannot segment CJK, so a Japanese phrase search returns 0 hits even when rows exist (claude-mem `plans/25-search-read-path-fts-cjk.md:6-8`, issue #3982). oboete's FTS tables use the trigram tokenizer, which indexes CJK by character (oboete `src/search.rs:1-7`; `src/consumer/fts.rs:21`; `src/claims.rs:118`), and a two-character Japanese query is tested (`src/search.rs:931-961`).
  - MCP search takes `repo`, `all` and a limit up to 100, alongside `get` and `timeline` (oboete `src/mcp.rs:36-44`, `:142-210`). claude-mem's OpenCode tool is fixed at 10 results (claude-mem `src/integrations/opencode-plugin/index.ts:284-309`; round-2 adapters lane).
  - Injection is fenced as data (`src/manifest.rs:165-174`).
  - The last reply is captured into raw at Stop (`src/capture.rs:144-154`), not read from another product's transcript at read time.
  - Compaction re-injection uses the same path, with no double injection on resume (`src/setup.rs:31-33`; `src/hook.rs:202-220`).
- **Operations**
  - No network port and no sidecar processes (oboete `docs/spec.md:801`). The worker lock is a kernel flock released by the OS on exit (`src/worker.rs:84-118`).
  - So claude-mem's spawn lock, ghost-port reclaim, Windows respawn cooldown and restart-successor handoff are not needed: the next hook simply starts a worker (claude-mem `src/services/worker-spawner.ts:85-234`; `src/services/worker-shutdown.ts:158-189`; round-2 lifecycle lane).
  - The process group is killed before the child is reaped, so pid reuse cannot hit another process (`src/provider.rs:1457-1467`).
  - Crash or clean outcome, ordered by generation (`src/worker.rs:176-186`, `:408-446`).
  - The curator environment is an allow-list, not a denylist (`src/provider.rs:1195-1218`). claude-mem strips named foreign-Python variables instead (round-2 install lane).
  - Keys are stored as file paths (`src/config.rs:63`, `:133-147`).
  - The viewer binds 127.0.0.1 with a per-run token and a Host check (`src/view.rs:91`, `:101`, `:205-213`).
  - No telemetry (`docs/spec.md:1049`).
  - Money limits fail closed (`src/provider.rs:273-274`; `src/budget.rs:60-66`).
  - The hub design looks up each device token's hash in the DO on every request, so a revoked token stops at the next request (`docs/spec.md:543`). claude-mem's verdict cache can honour a revoked token for up to 900 s (claude-mem `services/sync-api/src/auth.ts:97-104`; `env.ts:11-13`; round-2 remote lane).

## Considered and not adopted

**Refuted on fit (verified), round 1.** I agree with each verdict.
- **Local `claim_files` table plus a `file:` search filter** (curation lane).
  - Claim ops carry no files (oboete `src/claims.rs:22-40`), and raw does not travel by default (`docs/spec.md:41`, `:450`). So claims from other devices would never map to files.
  - `claims_fts` already matches paths named in a claim's body or quotes (`src/claims.rs:118`), and `raw_fts` indexes `file_path` (`src/consumer/fts.rs:16-47`).
  - The working variant, with files computed at the origin and carried on the op, is inside Recommendation 21.
- **Contentless FTS5 for `raw_fts`.** Queries shorter than a trigram fall back to `LIKE` on the column (oboete `src/search.rs:174-209`), which returns NULL in a contentless table. That breaks the shipped 設計 test (`src/search.rs:931-961`).
- **SQLite pragma baseline** (journal_size_limit, auto_vacuum).
  - Forget step 5 already specifies `secure_delete`, `wal_checkpoint(TRUNCATE)` and optimize (oboete `docs/spec.md:665-666`).
  - The worker exits when idle (`src/worker.rs:136-139`), which removes the -wal file.
  - INCREMENTAL auto_vacuum frees nothing unless something calls it, and it costs time on every hook write.
- **Generalized poison-op quarantine.**
  - The only permanent refusal, 413, is already dead-lettered (oboete `docs/spec.md:449`).
  - An excluded-repo refusal is reversible (`docs/spec.md:470`), and version skew parks ops rather than dropping them (`docs/spec.md:492`).
  - A hash conflict is a device fault, which Recommendation 2 handles.
- **Re-validating synced claim ops against the gates.**
  - A supersedes-must-exist check breaks convergence when sync arrives out of order (oboete `src/consumer/claims.rs:33-50`; `docs/spec.md:473-492`).
  - Quotes are already re-checked verbatim (`src/consumer/claims.rs:185-204`).
  - A leaked token is outside what payload checks can stop (`docs/spec.md:557`).
  - Optional residual: a 2-3 line check that rejects `scope=global` unless the recipe is `oboete pref add`, because main's pre-gates curator allowed global scope (`src/curate.rs:1041`, `:1274`).
- **Folding `merged_into_project` into the current claude-mem import.**
  - Imported repos are `claude-mem:<project>` and match no oboete key yet (oboete `src/import.rs:4-5`, `:276-278`).
  - claude-mem never rewrites `project`; it ORs the two names.
  - At M7's name-to-key mapping, read `COALESCE(merged_into_project, project)`, fold the `parent/<wt>` composite to `parent`, map the result through `oboete repo alias`, and add one import test with both cases. The round-2 lifecycle lane gives the same steps (not verified).
- **Weekly timeline.**
  - A list with no AI can only print the curator's English bodies (decision 29, `docs/spec.md:56`).
  - Old digests go stale by rule (`docs/spec.md:280`; `src/digest.rs:88-89`).
  - Search with since/until already covers the need (`docs/spec.md:363-370`).

**Refuted on fit (verified), round 2.** I agree with each verdict.
- **Remote MCP recall: amend spec 5.13 and add an M6 acceptance check for web sessions.**
  - claude-mem's recall link is read-only by construction: search, context and recent only (claude-mem `src/server/mcp/recall-mcp-server.ts:1-33`, `:140-161`), mounted on POST and GET only (`src/server/routes/v1/ServerV1PostgresRoutes.ts:1081-1082`).
  - The fit verifier found that oboete's plan already covers the same ground:
    - a web session has no local oboete, so it already falls under 5.13 (oboete `docs/spec.md:561`);
    - the tool set is already read-only (`:568`);
    - the acceptance check repeats hub spike item 8 (`hub-platform.md:143-146`);
    - the proposed latency measurement from Anthropic's region cannot be made, because Anthropic does not publish the region (`docs/spec.md:566`).
  - The one surviving piece, connecting once from claude.ai/code during item 8, is in Engineering lessons under Hub.
- **Shell-read paths feeding both the manifest and the touch set** (codex-file-context-extraction).
  - claude-mem's extractor is fail-open and capped at 10 (claude-mem `src/cli/adapters/codex-file-context.ts:87-152`).
  - Spec 5.5's touch set must fail closed (oboete `docs/spec.md:457`; row 30-20 at `:1476`). A fail-open extractor would make an unparseable session look clean and send its content.
  - The manifest-only half is in Engineering lessons.
- **Worker restart handoff / self-exit when its binary changes** (round-1 item and round-2 lifecycle proposal, merged).
  - claude-mem's dying worker spawns its own successor once the port is free (claude-mem `src/services/worker-shutdown.ts:158-189`). oboete has no resident daemon to hand off: the next hook starts a worker (oboete `src/hook.rs:897-906`).
  - Round 2 proposed that the worker exit when the `current_exe()` metadata changes. The fit verifier refuted this:
    - only `oboete update` replaces the binary (oboete `docs/spec.md:912`; the 0.1 rule at `:69`);
    - update step 0 must stop the worker before the backup and the swap (`:918-921`, `:931`), which exe detection cannot do;
    - on Linux, a replaced binary's `current_exe()` reads "(deleted)", so `metadata()` fails instead of showing a change.
  - At M7, use an exit-request flag polled like `restore_requested` (oboete `src/worker.rs:121`, `:299-321`; `src/backup.rs:481-494`).
- **Keep `CLAUDE_CODE_GIT_BASH_PATH` and `CLAUDE_CODE_OAUTH_TOKEN` in the curator environment.**
  - claude-mem's sanitizer does preserve both names (claude-mem `src/supervisor/env-sanitizer.ts:28-30`). But its EnvManager then deletes any inherited OAuth token (Issue #2215), and injects only a fresh token from the keychain (per the fit verifier: `src/shared/EnvManager.ts:42`, `:213-217`, `:245`).
  - Spec 6.4 keeps every `CLAUDE_CODE_*` name off the allow-list (oboete `docs/spec.md:728`).
  - The Git Bash question is already open and scheduled for M5's spawned-curator test (`docs/spec.md:1630`).
  - Residual: that test should include a Git for Windows install outside the standard paths. claude-mem's preflight checks the environment variable, then the two Program Files paths (claude-mem `src/npx-cli/utils/windows-git-bash-preflight.ts:31-39,74-79`, per the fit verifier).

**Out of scope or excluded by owner decision (not verified).**
- **Disabling Claude Code's auto-memory** (claude-mem `src/npx-cli/commands/install.ts:246-301`): the owner keeps MEMORY.md in use. If public users ask for it later, add a `setup --advanced` question that defaults to leaving it on, with a provenance record so `--remove` clears only what oboete set. Not measured: how much of the context window MEMORY.md takes next to oboete's packet. That is low value while the owner keeps MEMORY.md; measure it only if Rec 6's packet budget runs short.
- **Windsurf adapter and the OpenClaw gateway** (claude-mem `src/cli/adapters/windsurf.ts`; `openclaw/src/index.ts:620-876`): outside the seven agents (oboete `docs/spec.md:62`). Widening the list is the owner's decision.
- **OpenClaw live observation feed to chat channels, and plan/billing telemetry**: decision 21's network list allows no such destination (oboete `docs/spec.md:1049-1055`).
- **OpenClaw multi-alias session merging** (claude-mem `openclaw/src/index.ts:662-680`, which folds a gateway's session key, conversation id and channel id into one session): OpenClaw is outside the seven agents. Whether any of the seven changes its session id mid-session was not checked.
- **OpenClaw context TTL cache**: spec 4.1/4.2's refresh on Stop plus a status check per uid is fresher (`docs/spec.md:302-311`).
- **Sharing one credential between a local install and the cloud** (claude-mem `cowork/scripts/cmem-hook.mjs:35-68`): per-device revocable tokens exist exactly to avoid this (`docs/spec.md:543-545`).
- **Deferred cmem.ai sign-in offer, installer OAuth pairing, pricing and trial pages, and the Docker/Postgres server runtime** (claude-mem `src/npx-cli/commands/install.ts:1842-1867`, `:1436-1461`, `:991-1047`): oboete has no vendor account or hosted server (decision 18, `docs/spec.md:45`, `:412`). The redirect-validation lesson is kept under Hub.
- **Per-user advisory locks with a projection lease, the WebSocket speed layer with its fan-out sizing, and the auth verdict cache** (claude-mem `services/sync-api/src/store.ts:197-203`, `:263-292`; `auth.ts:97-104`):
  - a single-owner DO serializes writes (`docs/spec.md:379-382`);
  - the first release polls (`:384`), and a wake-only WebSocket comes only if M5 misses its line (`:385`);
  - there is no external verifier to cache (`:399`).
- **Stateless-first MCP fallback and the server parity map**: oboete has one remote endpoint and one hub, whose reference is the fake hub (`docs/spec.md:588-591`).
- Declarative transcript-schema watcher: all 7 agents have code adapters (`docs/spec.md:1308-1310`).
- Writing memory into AGENTS.md: memory is data, never instructions (`docs/spec.md:740-747`).
- Grok Bot discovery and write-back into its memory files (claude-mem `src/services/integrations/GrokBotIndexWriter.ts:257-270`): not one of the 7 agents, and it bypasses forget.
- Telegram alerts and wrap-ups (claude-mem `src/services/integrations/TelegramNotifier.ts:44`; settings at `src/shared/SettingsDefaultsManager.ts:143-145`): a new egress path serving no goal.
- `$TIER:` model aliases (claude-mem `src/services/worker/model-aliases.ts:4-14`): each chain entry already names its model.
- Knowledge-agent corpora: spec 6.5 rejects long-lived sessions.
- Tree-sitter `smart_*` code navigation.
- Custom modes and taxonomies (claude-mem `src/services/domain/ModeManager.ts:85-111`; `plugin/modes/`): they would weaken the gates.
- XML prose salvage (claude-mem `src/sdk/parser.ts:142-160`): it bypasses the verbatim-evidence gates.
- Per-agent memory isolation (claude-mem `src/services/worker/SearchManager.ts:298-307`): it conflicts with goal 1.
- Token-economics footer (claude-mem `src/services/context/sections/FooterRenderer.ts:17-29`), context welcome hint, parallel human and agent renderers, log console.
- All telemetry (decision 21, `docs/spec.md:1049`).
- Remote "Observation TV" viewer (decision 19, `docs/spec.md:803`).
- Projection lease, kill switch, Discord watchdog (not ported, spec 5.2, `docs/spec.md:400-402`).
- Product skills that are not memory features:
  - cost report, babysit, design-is, do/make-plan/pathfinder, learn-codebase, oh-my-issues, wowerpoint;
  - standup chat room, ccs-align rules walker;
  - what-the, a plain-English explainer (claude-mem `plugin/skills/what-the/SKILL.md:1-3`).

**Considered after the second critic (not verified).**
- **Endless Mode** (claude-mem `plans/2026-07-17-endless-mode-v1.md:1-31`; a plan marked "Ready to build", not shipped: a grep of claude-mem `src` for "endless" and "bottle" finds only two unrelated comments).
  - After a compaction or a resume, it would write a file (the "bottle") with the session's user and assistant messages verbatim from the transcript and claude-mem's observations in place of tool traffic, then inject a pointer telling the model to read it and continue (`plans/2026-07-17-endless-mode-v1.md:24-37`, `:98`).
  - oboete's answer to the same problem is spec 4.7: after compaction, open items and current claims are re-injected, with no double injection on resume (oboete `docs/spec.md:345-346`), through the same SessionStart path (`src/hook.rs:202-220`). The manifest already carries the last prompt and reply and the todo list (`docs/spec.md:357`), and Rec 6 keeps those resume-critical parts when the packet is cut.
  - Not adopted now. A pointer that tells the model what to do is an instruction, while oboete injects memory as fenced data (`docs/spec.md:740-743`), and a verbatim copy of the conversation in a separate file would be one more place forget has to reach (compare Rec 3).
  - An M4 dogfood question instead: after a compaction in a long session, does the agent lose its plan even with Rec 6's packet? If it does, weigh adding the session's recent prompts (the owner's words only) to the post-compaction packet within Rec 6's budget.

## Dropped in verification

Each of these was refuted on existence or accuracy. For round-2 items, I also say what the verdict refuted, and where anything that survived went.

**Round 1**
1. **Codex plugin-route installer** (existence): oboete already registers with Codex's trust bookkeeping and MCP (oboete `src/setup.rs:762-830`, `:873-937`, `:619-659`). Part (b) is salvaged in Engineering lessons. The fit verifier noted that trust hashes checked only against Codex 0.155 (`src/setup.rs:916-947`; `docs/m1.md:265`) are fragile for public release; re-examine them at M7.
2. **Curation-health ledger at SessionStart** (existence): doctor already prints OWNER_HOLD with a remedy, isolation results and overdue windows (oboete `src/setup.rs:1674-1725`).
3. **Quota cooldown kept apart from failures** (existence): the Skip enum and D11 already separate waits from failures (oboete `src/provider.rs:474-490`; `src/curate.rs:805-830`).
4. **Sync health in the same warning slot** (existence): doctor keeps the hub probe and curator status as separate lines (oboete `docs/spec.md:1032-1035`).
   - Note for 2-4: MUST-M9 already requires a backlog line at SessionStart (oboete `docs/research/redesign-2026-09-24/improvements-synthesis.md:138-142`). When it is built:
     - show a category, never provider text;
     - open providers.db read-only;
     - never tell the agent to run `oboete resume`, since OWNER_HOLD is also the web-search safety stop (`src/provider.rs:1400-1408`).
5. **Progressive disclosure** (existence): planned in spec 4.4 (`docs/spec.md:326`, `docs/milestone-3-plan.md:317`). The missing uid handle is covered by Recommendation 17.
6. **Project merge continuity** (existence): `oboete repo alias` is already in the spec (`docs/spec.md:1460`). The sync-lane framing survived as Recommendation 9.
7. **90-day recency window** (existence): since/until is already planned on every surface, with one ranked entry point. The 4,096-cap lesson is salvaged.
8. **Injection guard for MCP results** (existence): the curator's system prompt already says session text is data (oboete `src/provider.rs:979-985`), and spec 4.10 already requires MCP output to be fenced as data (`docs/spec.md:372`). When building it, fence inside `text()` (`src/mcp.rs:73-77`) with an MCP-specific first line, not `manifest::fenced`'s checkout wording (`src/manifest.rs:165-173`).
9. **Retention with VACUUM** (existence): retention expiry and forget step 5 are specified (oboete `docs/spec.md:682-688`, `:663-666`). If disk reclaim is added later, set INCREMENTAL auto_vacuum on raw.db when it is created, rather than running an idle full VACUUM.
10. **`user_version` stamping** (existence): spec 7.3 already requires it (oboete `docs/spec.md:917-935`). The lesson on where to check it is salvaged.
11. **Two parallel schemas** (existence): v1 retirement is already planned (oboete `docs/spec.md:944-990`). Still, `eval`, `reindex` and the CLI search/get/timeline call `db::open` with no scheduled move (`src/main.rs:336`, `:372`, `:390`, `:439`, `:475`). Add them to the M4 move.
12. **Canonical JSON envelope** (existence, accuracy and fit): the op-hash lesson is salvaged.
13. **SQLite-as-outbox push drain** (existence and fit): spec 5.6 already covers it, and one checkpoint cannot express held, dead-lettered or out-of-order ops. Tombstones are `records` rows (oboete `src/raw.rs:13-17`).
14. **Exact multiset ack check** (existence and fit): spec 5.6 already takes this guard (oboete `docs/spec.md:476-480`). Keep the fake hub's misbehaviour modes as acceptance tests.
15. **409 for the same id with a different hash** (existence): per-op idempotency is planned (oboete `docs/spec.md:475`). The missing answer is kept inside Recommendation 2.
16. **Plain "how it works" README** (existence and fit): spec 7.7 plans a Japanese README plus MUST-M23's docs, with a plainness test (oboete `docs/spec.md:1065-1072`).
17. **Cloud-sync 401/403 pause plus dead-letter** (accuracy): claude-mem does dead-letter tombstones (claude-mem `src/services/sync/CloudSync.ts:1516-1534`, `:1290-1327`). The 401/403 hourly-pause half was confirmed accurate (`CloudSync.ts:95`, `:802-812`), and the fit verifier supported it. Re-propose that half alone at M6.
18. **Installer UX with live pricing** (accuracy): per-option pricing was removed in claude-mem 13.21.0 (claude-mem `CHANGELOG.md:534-552`; `src/npx-cli/cmem-pro-costs.ts:1-4`). The spec conflict the fit verifier found is handled in #22 below.

**Round 2**

19. **Cowork-style memory for cloud sessions (HTTP hook shims)** (accuracy; fit also refuted).
    - Accuracy refuted a narrow point. The Task/Agent handler never posts to `/api/hooks/ingest`: it only fetches and injects (claude-mem `cowork/scripts/cmem-hook.mjs:438-454`). The verifier said this does not change the proposal.
    - The fit verdict is the substantive one:
      - a capture-only token with no read leaves the egress gate sending nothing (oboete `docs/spec.md:459`);
      - a device id per session cannot be bound to a fixed token;
      - Layer A changes owner-settled section 5 (`:545`, decision 18 at `:45`);
      - the teleport interim misreads the transcript import, which runs once at setup (`:980`).
    - The need survives as the question in 要約 and as the web-sessions lesson under Hub.
20. **Content-addressed sync ops, "port as planned"** (accuracy).
    - The claim said a same-revision resend is acked without re-validation. In fact the donor compares hashes and refuses on a mismatch (claude-mem `services/sync-api/src/store.ts:344-351`). It also normalizes `-0` rather than rejecting it (`canonical-content.ts:74`).
    - The proposal was "none", and oboete does not port the rev check (oboete `docs/spec.md:406`). The corrected behaviour is recorded under Hub.
21. **Post-install module verification** (existence): it is already planned. Setup ends with a canary round trip (oboete `docs/spec.md:901-904`), and update step 6 runs a self-check (`:931-932`).
22. **Non-interactive setup resolves provider defaults** (existence).
    - The verdict refuted "setup aborts instead of defaulting". That is true: `setup --yes` takes defaults (oboete `docs/spec.md:905`). But it did not address the item's actual point.
    - The actual point is that line 905's "tier none" contradicts decision 28, which turns subscriptions on by default when setup finds them (`docs/spec.md:55`). I checked both lines on origin/main c64b6db, and the fit verifier held.
    - Decision 28 replaced decision 21's opt-in, and decisions win over section text, so the stale "tier none" at spec 7 was fixed in the PR that adds this note.
23. **Dependency health snapshot** (existence; fit also refuted): persisted cooldowns with remedies already exist (oboete `src/providers_db.rs:169-238`; `src/setup.rs:1713-1724`), and spec 7.6 already specifies the embedding line (`docs/spec.md:1036`). The no-new-state way to build it is under Engineering lessons.
24. **OpenCode hook wiring** (existence).
    - The v2 wiring is already correct and was verified live (oboete `src/opencode.js:71-135`).
    - The verifier confirmed that the proposal's real content, v1 support, is neither built nor planned. That part is salvaged under Adapters, with the fit verifier's three fixes.
25. **Context banner as a user-visible message** (accuracy; fit also refuted).
    - The cited `user-message.ts` handler is wired to no hook. The live SessionStart banner is `context.ts`, and it carries a promo line (claude-mem `src/cli/handlers/context.ts:143-145`, per the verifier).
    - The fit verdict found that the "overdue windows" trigger fires in the normal state after a break.
    - The narrower recording-failure line is salvaged under Engineering lessons.
26. **Parse recorded real limit events in tests** (accuracy; fit also refuted).
    - The accuracy verdict refuted "claude-mem uses recorded fixtures", which the lesson did not claim. The lesson's cited fact holds: claude-mem's `usage_limit_hit` guard never matched the SDK's real shape (claude-mem `CHANGELOG.md:435-439`, which I opened).
    - The fit verifier's point 4 checked an older claude-mem checkout (`/home/jura/projects/claude-mem`, 775781c0), so it missed that entry. Its points 1-3 stand:
      - no limit-hit result can be recorded without burning quota;
      - the real `rate_limit_event` shape is already in the spike notes;
      - doctor already prints each rest.
    - One recorded ordinary event is salvaged.
27. **Tie the curator prompt's lists to the constants** (accuracy).
    - The verdict refuted "claude-mem ties its prose to its registry", which the lesson did not claim. The verifier confirmed the drift the lesson cites (claude-mem `CHANGELOG.md:875`).
    - Existence and fit held. The test is salvaged under curate.rs.
28. **Allowlist-first routing for hub endpoints** (accuracy; fit also refuted).
    - The accuracy verdict read the oboete proposal as a claim about claude-mem. Its own quote shows that the middleware is allowlist-first ("default-denied except an exact-match allowlist", claude-mem `src/services/worker/http/middleware.ts:99-130`).
    - The fit verdict independently refutes the proposal:
      - the donor hub already matches exact paths (claude-mem `workers/sync-hub/src/index.ts:1085-1091`);
      - the admin routes are not ported (oboete `docs/spec.md:404`);
      - every route is token-gated (`:543`).
    - Nothing is salvaged.
29. **Hub counts Free-plan usage** (existence and accuracy).
    - The existence verdict argued about Claude subscription usage, which is a different thing from the hub's Cloudflare Free plan.
    - The accuracy verdict is right: `watchdog.ts` is an hourly monitor of DO cost with a kill switch (claude-mem `workers/sync-hub/src/watchdog.ts:2-53`). That is what spec 5.2 already declines to port. It is not a daily counter.
    - So claude-mem does not back this lesson. oboete's own gap, that the spec never says how a Free-limit refusal is treated, is salvaged under Hub.

## Method

- **Sources.**
  - claude-mem: `~/.claude/plugins/marketplaces/thedotmack` at 7d03554 (v13.28.0).
  - oboete: `oboete-main-ro` at 4dc8f0d. Since then, #165 and #168 merged to origin/main (c64b6db), and the resetsAt lines were checked there.
  - In-flight branches were read with `git show`.
- **Round 1** (reported by round 1; the critic could not re-check these numbers from its data):
  - 8 lanes compared about 190 claude-mem mechanisms with oboete.
  - 49 proposals went to adversarial verification on three lenses: existence (oboete really lacks it), accuracy (claude-mem really does this), and fit (it serves the owner's goals without breaking a decision).
  - 23 held on all three, which merged into 22 recommendations. 18 were dropped.
- **Critic round.** One completeness critic reviewed the round-1 note. It found:
  - (1) 17 lane items dropped without a word;
  - (2) claims the data could not back: the verification counts, "more agents than claude-mem", measurement sources, the Free-plan lesson, and two lessons with no lane feature;
  - (3) claude-mem areas that no lane covered.
- **Round 2.** Four new lanes covered 58 features:
  - remote (cloud, web and mobile sessions, hosted recall, server parity): 17;
  - install (install, uninstall and environment): 19;
  - adapters (agent integrations round 1 did not compare, and hook handlers): 15;
  - lifecycle (worktrees, restarts, migrations, identity, curator audit): 7.
- **Round-2 verification.** Each lane's gap analysis sent its strongest proposals to verification, together with the five round-1 lessons that had no verdicts. That is 12 proposals plus 5 lessons, and 51 verdicts in all.
  - Existence: 12 held, 5 refuted.
  - Accuracy: 10 held, 7 refuted.
  - Fit: 8 held, 9 refuted.
  - 2 of the 12 proposals held on all three: Recommendations 4 and 10. None of the 5 lessons did.
  - Where the verdicts went: 11 items went to Dropped (#19-#29) and 4 fit-only refutations went to "Considered and not adopted". The rest of each lane's analysis appears as "round-2 lane, not verified" lessons, as "already ahead" entries, or as out-of-scope items.
- **Placement rule.**
  - Refuted on existence or accuracy: Dropped, even when the verdict addressed a different claim than the item made.
  - Refuted on fit: Considered and not adopted. I agreed with every fit verdict, so I overrode none.
  - Surviving pieces rest on the verifier's evidence and are labelled "salvaged".
- **Verdicts that addressed a different claim than the item** (audit note, not an override):
  - nonint-provider-defaults (existence);
  - opencode-hook-fix (existence, which confirmed the real gap);
  - real-limit-events (accuracy; and fit point 4, which read an older checkout);
  - prompt-lists (accuracy);
  - allowlist-routing (accuracy);
  - free-plan-limits (existence).
- **Corrections to round 1:**
  - "7 agents, more than claude-mem" is replaced by the actual overlap. I checked claude-mem's adapter index, integrations and GrokBot schema, and oboete's `setup.rs:14`.
  - The measurements now name their source, oboete's own docs, and the hook p95 is no longer described as within target.
  - The resetsAt item moved from "not verified" to confirmed and fixed. I checked it at claude-mem `RateLimitStore.ts:153-156` and oboete origin/main c64b6db.
  - The "Other active sessions" lesson was removed, because it had no claude-mem source. The ANSI/C0 lesson was checked (`antigravity-cli.ts:126-127`).
  - The SubagentStart route (Rec 7) is confirmed first-hand: the research subagents received the ponytail plugin's SubagentStart-injected context (`~/.claude/plugins/marketplaces/ponytail/hooks/ponytail-subagent.js:2-6`).
  - The round-1 branches m3/subs-anytime and m3/codex-limit are now merged (#165, #168).
- **Where each critic item went:**
  - cowork-plugin → Dropped #19, the owner question, the web-sessions lesson, and "already ahead" for the spool.
  - worktree-adoption → already ahead (origin-URL key), plus the M7 import step in "Considered".
  - guarded-restart-successor-handoff → "Considered" (restart handoff / self-exit) and "already ahead".
  - migration-column-carry-forward → already ahead (raw.db add-only), plus the knowledge.db version note.
  - identity-triad-defense-in-depth and auth-token-user-binding → already ahead (device id in its store), plus the token-binding lesson under Hub.
  - installer-ux-evolution: pricing → Dropped #18; device-code OAuth → "Considered" plus the redirect lesson; the restart page is not needed: claude-mem's `GET /restart` page restarts its worker and waits for a new pid (claude-mem `CHANGELOG.md:618-630`, `:641-650`), and oboete has no resident worker or port to restart (oboete `docs/spec.md:801`); the next hook starts one (`src/hook.rs:897-906`).
  - timeline-day-file-grouped-rendering → Rec 8 (group by day only if needed; claude-mem `timeline-formatting.ts:79`).
  - pagination-path-stripping-privacy → not adopted. claude-mem strips project paths from results (claude-mem `src/services/worker/PaginationHelper.ts:16-51`). oboete's manifest already shows a path under the call's cwd relative to it (oboete `src/consumer/manifest.rs:436-438`, `:459-470`).
  - tool-uses-index-vs-batch-split → Rec 17.
  - rowid-binary-search-cursor → the Hub lesson "make seq an INTEGER PRIMARY KEY". The donor's binary search is in `workers/sync-hub/src/do/SyncHub.ts:812-892`, outside the `src` an earlier grep searched.
  - two-lane-sync-poll-plus-advisory-ws and advisory-websocket-fanout-sizing → "Considered" (WebSocket speed layer, `docs/spec.md:384-385`).
  - open-core-ip-boundary → already specified: the port takes only published code and re-verifies each ported commit (oboete `docs/spec.md:1064`; claude-mem `docs/ip-boundary.md:1-12`).
  - chroma-backfill-concurrency-guard → not needed. claude-mem caps concurrent project backfills (claude-mem `src/services/sync/ChromaSync.ts:1158`, `:1206-1212`). oboete's embedding consumer runs inside the single locked worker (oboete `src/worker.rs:84-118`).
  - idempotent-marker-gated-migrations → not needed. claude-mem gates each migration on a `schema_versions` row (claude-mem `src/services/sqlite/SessionStore.ts:267-285`). oboete's forward-only migrations run one transaction per file (`docs/spec.md:928-933`).
  - windows-gitbash-preflight-replica → "Considered" (curator environment) and the M5 test case.
  - auto-memory → "Considered".
  - curator audit log → lesson.
  - project-exclusion globs → Rec 4.
  - recall MCP → "Considered", plus the Hub lesson.
  - OpenCode plugin → Dropped #24 and the salvaged v1 lesson.
  - Windsurf and OpenClaw → "Considered".
  - bmp-safe → Rec 6.
  - statusline → lesson.
  - dependency preflight and health → Dropped #23 and the lesson.
  - user-message → Dropped #25.
  - summarize → already ahead (a digest is made once per session and repo, after the session ended or went idle, and only when it has claims: oboete `m3/digest-b2:src/digest.rs:133-135`, `:171`, `:183`), plus the Stop re-entry test.
  - file-edit handler → not needed. It turns Cursor's afterFileEdit and Windsurf's post_write_code into a synthetic `write_file` observation (claude-mem `src/cli/handlers/file-edit.ts:30-41`; `src/services/integrations/CursorHooksInstaller.ts:160-161`; `WindsurfHooksInstaller.ts:111`). oboete takes Cursor's edits from postToolUse, which already carries them (oboete `docs/research/agent-adapters-2026-09-23.md:535`), and Windsurf is out of scope.
  - what-the → "Considered" (product skill).
  - services/sync-api, server-parity-map and SESSION_ID_ARCHITECTURE → round-2 remote lane.
  - `docs/security.md` → out of scope. Its seven lines cover the server beta's API-key auth and its Redis queue (claude-mem `docs/security.md:1-7`), a runtime oboete does not have (decision 18, oboete `docs/spec.md:45`).
- **Second critic round.** A second completeness critic reviewed this note. Where its items went:
  - Six "salvaged from Dropped #N" labels pointed at the old numbering; they now name #23-#27 and #29.
  - The five round-2 items that had no line are under "Where oboete is already ahead": `auto-project-naming`, `server-beta-worker-split`, `install-marker-versioning`, `openclaw-circuit-breaker` and `openclaw-installer`.
  - Uncovered areas: Endless Mode → "Considered after the second critic"; the CJK FTS defect → already ahead (Japanese search); manual memory save → already ahead (explicit memory).
  - Rec 6's surrogate-pair benefit was removed; the uncited claims it listed now cite a file and line, or are worded as a to-do.
  - Also corrected while checking: owner decision 18 is spec line 45, not 41; raw sync's default is line 41, not 40; the seven agents are line 62, not 63 (all at 4dc8f0d).
  - Not reviewed one by one: claude-mem `plans/*` other than the two above (most match covered lanes by title), and the top-level `SECURITY.md` (a vulnerability-reporting policy) and `RECEIPT-JOIN.md` (a join contract between tool uses and expense tallies), neither a memory mechanism.
  - tests/fixtures → Dropped #26: claude-mem's rate-limit tests use hand-written objects, not recorded fixtures.