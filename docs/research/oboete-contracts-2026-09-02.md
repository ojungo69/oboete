# oboete エージェント契約 調査エビデンス (2026-09-02)

対象: Claude Code / Codex CLI / Grok Build CLI / Pi の hook・拡張契約、および Cloudflare Workers AI (observer LLM) と R2 (同期先)。M1 spec を書く前提資料。各主張は調査 1 本 + 敵対検証 2 本 (source fidelity / reproducibility) を通し、片方でも反証されたものは格下げして理由を明記した。

## 要約

1. Claude Code の注入上限は「200 行 / 25 KB」ではない。実際は `additionalContext` / `systemMessage` / plain stdout が **値ごとに 10,000 文字**、超過は切り捨てではなくファイル退避 + preview 差し替え。3 ページ (hooks / hooks-guide / tools-reference) を全文検索して「25 KB」「200 lines」は 0 件。
2. `PostCompact` は `compact_summary` (compact が生成済みの会話要約) を、`Stop` は `last_assistant_message` を hook stdin で無料で渡す。oboete が北極星に据えた「observer LLM 負担」の少なくとも一部は自前 LLM 呼び出し無しで賄える可能性がある — M1 のスコープ判断に直結する。
3. Claude Code で `UserPromptSubmit` から exit 2 するとユーザーの入力そのものが消える。また stdout が `{` で始まり `}` で終わると JSON として解釈され、パース失敗時 (v2.1.248 以降) は文脈が**無言で落ちる**。捕捉 hook は必ず exit 0、注入本文は `{` 始まりを避ける。
4. Codex 0.152.1 の rollout では tool 記録は `custom_tool_call` / `custom_tool_call_output`。`function_call` 前提の parser は現行セッションのツール活動をほぼ全部取りこぼす (実測で 142 対 12)。
5. Codex の hook は trust 済みでなければ**無言でスキップ**され exit 0 で正常終了する。`trusted_hash` は sha256(canonical JSON) でオフライン算出可能と実証済みなので、installer が自分で書ける。
6. Codex の `memories` は stage Stable だが **default_enabled: false**。「既定 ON」は本マシンのユーザー opt-in を既定と誤認したもの。installer が無条件に `features.memories = false` を書くのは誤り。
7. Grok 1.0.17 には turn 開始前の文脈注入経路が存在しない。`SessionStart` / `UserPromptSubmit` の additionalContext は捨てられ、`PreToolUse` の additionalContext は「呼び出しが走った後」に届き、deny されると消える。`Stop` の additionalContext は注入ではなく turn 継続の指示になる。
8. SessionEnd 系は 3 エージェントとも flush 先にできない。Claude Code は全 SessionEnd hook 共有 1.5 秒 (plugin 提供 hook は自分で予算を上げられない)、Codex は既定 1 秒・上限 3 秒で async 指定も同期に降格、Grok は各 1.5 秒 + キュー全体 0.5 秒。
9. Pi の拡張は完全に in-process、handler は逐次 `await` され timeout 機構が一切無い (`runner.js` に `setTimeout`/`race` が 0 件)。注入 handler が throw すると fail-open で文脈だけ黙って消える。`ctx.signal` は `session_start` / `before_agent_start` では undefined。
10. Workers AI `@cf/zai-org/glm-4.7-flash` は無料枠対象だが reasoning を抑制する手段が実測で無く、1 コール 11.9〜45.5 neurons (現実的スキーマで 45.45) = 実質 1 日 220 コール前後。R2 側は aws4fetch 1.0.15 以降が `.r2.cloudflarestorage.com` を hardcode 済みで推測不要、ただし単一キーへの書き込みは 1 req/s 制限で 429、aws4fetch は 429 を既定 10 回リトライする。

---

## Claude Code hooks (claude-code v2.1.258 / code.claude.com/docs/en/hooks)

### 結論

- hook イベントは 33 個で確定 [VERIFIED]。両検証者が独立に H3 見出しを列挙して同一集合を得た: `SessionStart`, `Setup`, `InstructionsLoaded`, `UserPromptSubmit`, `UserPromptExpansion`, `MessageDisplay`, `PreToolUse`, `PermissionRequest`, `PostToolUse`, `PostToolUseFailure`, `PostToolBatch`, `PermissionDenied`, `Notification`, `SubagentStart`, `SubagentStop`, `TaskCreated`, `TaskCompleted`, `Stop`, `StopFailure`, `TeammateIdle`, `ConfigChange`, `CwdChanged`, `DirectoryAdded`, `FileChanged`, `WorktreeCreate`, `WorktreeRemove`, `PreCompact`, `PostCompact`, `PreModelSwitch`, `PostModelSwitch`, `SessionEnd`, `Elicitation`, `ElicitationResult`。
- 「200 lines / 25 KB」という上限は存在しない [VERIFIED / 反証成立]。実際の上限は hook 出力文字列 10,000 文字で、しかも **合算ではなく値ごと**、超過は truncate ではなくセッションディレクトリへのファイル書き出し + パス付き preview 差し替え。調査側の「combined / truncated」という表現は repro 検証で反証されたので、oboete が SessionStart と UserPromptSubmit で 10,000 を分け合う必要は無い。
- plain stdout がモデルに届くのは `UserPromptSubmit` / `UserPromptExpansion` / `SessionStart` / `PostModelSwitch` の 4 イベントのみ [VERIFIED]。それ以外は debug log 行き。ただし `additionalContext` を受け付けるイベント集合はこれとは別で、`PreCompact` はどちらの形でも注入できない (調査側の「PreCompact は JSON 形式が必要」は反証)。
- 共通 stdin フィールドの「全イベントに存在する」は誤り [SECONDARY]。`permission_mode` は "Not all events receive this field"、`effort` は tool-use 文脈のイベントのみ、`prompt_id` は最初のユーザー入力まで不在かつ v2.1.196 以降。加えて調査側が verbatim として引いた JSON 例には実在しない `effort` キーが挿入されていた。oboete のスキーマはこれらを nullable にする必要がある。
- exit 2 でブロックされるイベントは 10 個ではなく 15 個 [SECONDARY]。調査側の列挙から `TeammateIdle` / `PreCompact` / `Elicitation` / `ElicitationResult` / `WorktreeCreate` が漏れていた。特に `PreCompact` は "Blocks compaction"、`WorktreeCreate` は "Any non-zero exit code causes worktree creation to fail" で「exit 2 以外は基本ブロックしない」という但し書きも成立しない。
- `transcript_path` は実在する `.jsonl` で Anthropic Messages API 形状の content block (`text` / `thinking` / `tool_use` / `tool_result`) を持つ [VERIFIED、ただし schema は非公式]。ただし docs は「turn の最終アシスタント文が要るなら transcript を読まず `last_assistant_message` を使え」と明示しており、調査側の「tail-read すればよい」という設計示唆はこれに反する。

### 設計への影響

- 注入ペイロードは値ごと 10,000 文字。SessionStart と UserPromptSubmit で予算を共有する必要は無いが、超過分はファイルに逃げて preview だけになるため、実質の設計目標は 1 値あたり数千文字。
- `SessionStart` / `UserPromptSubmit` は plain stdout でよい (JSON envelope は `continue` / `systemMessage` が要るときだけ)。ただし **stdout が `{` 始まり `}` 終わりだと JSON としてパースされ、失敗すると文脈が無言で消える**ので、注入本文の先頭に生 JSON やコードブロックを置かない。
- `UserPromptSubmit` から exit 2 を返すと「Blocks prompt processing and erases the prompt」。Node の未捕捉例外や `set -e` で 2 が返る経路を全部潰し、常に exit 0 する。これはスタイルではなく必須。
- 注入テキストは命令形ではなく事実文で書く。"Text framed as out-of-band system commands can trigger Claude's prompt-injection defenses" — 記憶を「常に X せよ」と書くとユーザーに晒されるだけで文脈にならない。
- resume 時、mid-session イベントの注入は再実行されず transcript から**そのまま replay** される。鮮度が要る注入は `SessionStart` (resume/fork でも再実行される) に置く。
- 書き込み側 hook を `async: true` にすると注入は「次の turn」に回り、さらに firing ごとに別プロセスが立ち dedup も無い。PostToolUse を async にすると同一 SQLite ファイルに N プロセスが同時に来る (claude-mem はこれを SessionStart で起動する常駐 worker に集約して回避している)。
- `SessionEnd` は flush 点にならない。全 SessionEnd hook で 1.5 秒共有、plugin 提供 hook の timeout は予算を引き上げられない (`CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS` は環境変数側の逃げ道)。claude-mem は SessionEnd hook を一切登録せず、async Stop で要約している。
- subagent でも hooks は走り `agent_id` / `agent_type` が付く。イベントスキーマにこの 2 つを optional で持つ。
- transcript を読むなら top-level `type` を **whitelist** する。実測で 13 種類あり `attachment` だけで全行の約半分を占める。

### 根拠

1. https://code.claude.com/docs/en/hooks §JSON output — "Hook output strings, including `additionalContext`, `systemMessage`, and plain stdout, are capped at 10,000 characters. Output that exceeds this limit is saved to a file and replaced with a preview and file path"
2. 同 §Add context for Claude — "When several hooks return `additionalContext` for the same event, Claude receives all of the values. If a value exceeds 10,000 characters, Claude Code writes the full text to a file in the session directory and passes Claude the file path with a short preview instead."
3. 同 §Exit code 0 — "The exceptions are `UserPromptSubmit`, `UserPromptExpansion`, `SessionStart`, and `PostModelSwitch`, where Claude Code adds plain-text stdout as context that Claude can see and act on."
4. 同 exit-code-2 表 — "UserPromptSubmit | Yes | Blocks prompt processing and erases the prompt" / "PreCompact | Yes | Blocks compaction" / "WorktreeCreate | Yes | Any non-zero exit code causes worktree creation to fail"
5. 同 stdout 解釈 — "Starts with `{` and ends with `}`: Claude Code parses it as JSON." / "On the events that add plain-text stdout as context, Claude Code doesn't add the text. Before v2.1.248, Claude Code treated that stdout as plain text."
6. 同 `transcript_path` 行 — "Path to conversation JSON. The transcript file is written asynchronously and may lag the in-memory conversation ... Hooks that need the final assistant text of the current turn should use `last_assistant_message` on Stop and SubagentStop instead of reading the transcript"
7. 同 §PostCompact — "PostCompact hooks receive `trigger` and `compact_summary`. The `compact_summary` field contains the conversation summary generated by the compact operation."
8. 同 `timeout` 行 — "Defaults: 600 for `command`, `http`, and `mcp_tool`; 30 for `prompt`; 60 for `agent`. ... `SessionEnd` hooks share a 1.5-second budget" / §SessionEnd — "Timeouts set on plugin-provided hooks don't raise the budget."
9. 同 §resume — "For mid-session events like `PostToolUse` or `UserPromptSubmit`, when you resume with `--continue` or `--resume`, Claude Code replays the saved text rather than re-running the hook for past turns ... `SessionStart` hooks run again on resume with `source` set to `\"resume\"`"
10. 同 §Limitations (async) — "Each execution creates a separate background process. There is no deduplication across multiple firings of the same async hook."
11. 同 `permission_mode` / `prompt_id` 行 — "Not all events receive this field. Check the JSON example in each hook event section" / "Absent until the first user input. Requires Claude Code v2.1.196 or later"
12. `/home/jura/.claude/plugins/cache/thedotmack/claude-mem/13.23.1/hooks/hooks.json` — `SessionStart` matcher `"startup|clear|compact"` (同期)、`UserPromptSubmit` (同期)、`PostToolUse` matcher `"*"` `"timeout": 120, "async": true`、`PreToolUse` matcher `"Read"` async、`Stop` `"timeout": 120, "async": true`、`SessionEnd` キー無し、全エントリ `"shell": "bash"`

### 未確認・要フォロー

- ビルトインツールごとの `tool_response` の実スキーマ。docs は "The exact schema for both depends on the tool." としか書かず、`Write` の `{filePath, success}` 以外は列挙が無い。Read/Edit/Bash/Grep/Glob/Task を実セッションでダンプして確定する必要がある。
- `.jsonl` transcript のレコードスキーマが安定契約かどうか。hooks ページに "transcript format" の記述は 0 件で、両検証者のパースは再現であって仕様の約束ではない。`bridge-session` / `atis-latch` / `ai-title` / `queue-operation` などの内部型は文書化されていない。
- hook 出力が 10,000 文字を超えたときの preview 形式・退避先ファイルの命名・Claude が自動で読み戻すかどうかは未再現。
- workspace trust の初回 UX。trust ダイアログを受け入れるまで `~/.claude/settings.json` の hook すら保留される一方、`-p` / SDK セッションはダイアログを出さず trusted 扱い。installer 直後に記憶が取れない期間が生まれる。
- 管理者設定 `allowManagedHooksOnly` と `--settings '{"disableAllHooks": true}'` で oboete の hook が丸ごと無効化されうる。イベントログの完全性を前提にできない。
- `if` フィールド (permission rule 構文、tool イベント以外では絶対に発火しない)、`once`、`statusMessage`、`shell`、`type` の `http`/`mcp_tool`/`prompt`/`agent` は今回未評価。
- バージョン下限。少なくとも v2.1.191 / .195 / .196 / .202 / .214 / .234 / .248 / .251 に挙動差がある。本機は 2.1.258 なので全部満たすが、ユーザー環境では満たさない。
- `PostToolUse` は `EndConversation` 呼び出しでは発火しない。「全 tool call で発火」を前提にすると穴が空く。
- 引用衛生の注意: 調査側は `.../docs/en/hooks.html` を一次 URL として引いていたが、この URL は 404。正は `https://code.claude.com/docs/en/hooks`。

### 実測

- 本セッションの transcript `/home/jura/.claude/projects/-home-jura-projects-free-mem/0aea87bf-f141-4112-a843-af8afbe0d6aa.jsonl` を read-only でパース。3 回の独立計測で 174 / 288 / 319 行、いずれも JSON パース失敗 0。top-level type は 13 種 (319 行時点: `attachment` 153, `assistant` 48, `user` 36, `mode` 15, `bridge-session` 15, `last-prompt` 14, `atis-latch` 13, `ai-title` 13, `queue-operation` 4, `pr-link` 4, `file-history-snapshot` 2, `system` 1, `file-history-delta` 1)。`message.content` block は `tool_use` / `tool_result` / `thinking` / `text` / 素の文字列。
- hooks / hooks-guide / tools-reference を curl で raw 取得し、埋め込み Next.js RSC ペイロードを `json.loads` で展開して verbatim 照合 (WebFetch の要約を経由しない)。`25 KB` / `25KB` / `200 lines` は 3 ページとも 0 件。
- `claude --version` = 2.1.258。`~/.claude/settings.json` と claude-mem plugin の `hooks.json` を read-only で確認。
- hook を実際に登録・発火させる live probe は未実施 (本セッション自身の設定を変更しないため)。

---

## Codex CLI 0.152.1 hooks

### 結論

- 設定形式と event 一覧は確定 [VERIFIED]。`[[hooks.<Event>]]` (`MatcherGroup`, optional `matcher`) の中に `[[hooks.<Event>.hooks]]` (`HookHandlerConfig`)。イベントは 12 個: `PreToolUse`, `PermissionRequest`, `PostToolUse`, `PreCompact`, `PostCompact`, `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `SubagentStart`, `SubagentStop`, `Stop`, `Interrupt`。`hooks` feature は本当に default true (空 `CODEX_HOME` で確認)。
- stdin は 12 イベント全てで `cwd` / `hook_event_name` / `session_id` / `transcript_path` が `required` [VERIFIED]。`transcript_path` は nullable だが `codex exec` では実値が入る。`SessionStart` は `source` (enum `startup|resume|clear|compact`) を持ち、これが matcher 入力になる。
- `additionalContext` を出せるイベントは **5 個で閉じている** [SECONDARY / 調査側の「その他の非ブロック系イベントも」は反証]: `PreToolUse`, `PostToolUse`, `SessionStart`, `UserPromptSubmit`, `SubagentStart`。それ以外に `additionalContextLimit` を書くと警告付きで捨てられる。`Stop` の output schema に `hookSpecificOutput` は無く、`SessionEnd` に至っては output schema ファイル自体が存在しない (12 イベントに対し schema は 22 ファイル)。
- `additionalContextLimit` の既定は 2,500 tokens、超過分は `~/.codex` ではなく OS temp 配下 `hook_outputs/<thread_id>/<uuid>.txt` に退避され preview + 復元パスが付く [VERIFIED]。`0` は「spill 無効」= 実質無制限 (`if token_limit == 0 || ... { return text }`) なので、任意の大きな値を置くより 0 が正しい。
- rollout の tool 記録は現行版では `custom_tool_call` / `custom_tool_call_output` [VERIFIED / 調査側の `function_call` 前提は反証]。調査側が根拠にした rollout は `cli_version` 0.118.0 のもので、5 か月古い。両検証者が最新セッションで独立に確認し、比率は 142:12 と 5:0。`custom_tool_call_output.output` は文字列ではなく `{"type":"input_text","text":...}` の配列、`custom_tool_call.input` は JSON 引数ではなく JS プログラム。top-level には `world_state` と `inter_agent_communication_metadata` も出る。
- native `memories` は **default_enabled: false** [VERIFIED / 調査側の「stable, on by default」は反証]。本マシンの `~/.codex/config.toml` が明示的に `memories = true` と `[memories] generate_memories/use_memories = true` を持っているだけ。パイプライン自体の記述 (2 フェーズ、`~/.codex/memories/` 配下の artifact、session start の read path) は正しい。
- hook は trust されていなければ**無言でスキップ**される [VERIFIED / 調査側の live probe は `--dangerously-bypass-hook-trust` 付きだったため未証明だった]。同一 CODEX_HOME・同一 config でフラグの有無だけを変えた再現で、フラグ無しは exit 0・"OK" 出力・capture ディレクトリ空。`trusted_hash` は `sha256:` + canonical JSON のハッシュで、ユーザーの実 hash 3 件を手計算で 3/3 一致させ、事前計算した `[hooks.state]` 行だけで hook が発火することまで確認済み。

### 設計への影響

- installer の書き込み先は `config.toml` より `~/.codex/hooks.json` (または project の `.codex/hooks.json`) を第一候補にする。Codex は Claude Code と同じ JSON 形状でこれを読み、trust identity も TOML 版と収束する。ユーザーの 300 行超の `config.toml` を書き換えるリスクが消え、Claude Code 向けに出す hooks.json と実装経路が 1 本にまとまる。ただし `[hooks.state]` の trust 行だけは `config.toml` 側に書く必要がある。
- installer は hook を書いたら必ず対応する `[hooks.state."<abs config path>:<snake_case_event>:<group_idx>:<handler_idx>"] trusted_hash` を書く。preimage は `{"event_name": <snake_case>, "hooks": [{正規化済み handler}]}` を key ソート・compact separator で JSON 化したもの。正規化の落とし穴が 3 つ: `timeout` は設定値がそのまま preimage に入る (未設定なら既定 600、SessionEnd/Interrupt は 1。2026-09-04 に codex-cli 0.153.2 で hooks.json に `"timeout": 12` を置き、設定値 12 でハッシュした trust 行だけが発火し、既定 600 でハッシュした行は発火しないことを実測。当初の「正規化後の値」は timeout 未設定の hook でしか検証されていなかった)、`commandWindows` は非 Windows では None、`additionalContextLimit` は 2500 のとき preimage から除去、None 値のフィールドは丸ごと落とす。
- `[[hooks.SessionStart]]` には `matcher = "startup"` (または `"startup|resume"`) を必ず付ける。付けないと `/clear` と auto-compaction のたびに記憶ダイジェスト全体を再注入して 2,500 token 予算を焼く。
- `additionalContextLimit = 0` を使う。spill されたテキストは OS temp のパスになり、モデルが読みに行かなければ実質失われる。
- `SessionStart` hook は JSON envelope 無しの素の stdout でよい (`plain_stdout_becomes_model_context()` で確認済み)。JSON が要るのは `continue` / `systemMessage` を使うときだけ。
- rollout reader は `custom_tool_call` / `custom_tool_call_output` を主経路にし、`function_call*` は旧アーカイブ用の legacy branch に留める。未知の top-level type は握りつぶす。`world_state` は AGENTS.md 全文を抱えて巨大化しうるのでサイズガードを入れる。
- `SessionEnd` は既定 1 秒・上限 3 秒・`async` 指定は同期に降格される。detach した fire-and-forget 以外は置けない。`reason` はハードコードされた `"other"` 固定で、Claude Code と違い終了理由の情報を持たない。
- `features.memories = false` を無条件で書かない。既定は OFF なので新規ユーザーには書くだけ無駄で、ON のマシンはユーザーが自分で入れた設定。検出して重複を警告する。無効化するにしても粒度があり、注入だけ止めたいなら `[memories] use_memories = false`、LLM コストだけ止めたいなら `generate_memories = false`。
- `SubagentStop` は `transcript_path` とは別に `agent_transcript_path` を持ち、`PreToolUse`/`PostToolUse`/`UserPromptSubmit` は optional の `agent_id`/`agent_type` を持つ。sub-agent の turn を取りこぼすか二重計上するかはここで決まる (本ユーザーは `multi_agent = true`)。
- 企業環境では `requirements.toml` の `allow_managed_hooks_only = true` で user/project/session の hook が全部無視される。post-install の検証は「ファイルを書けたか」ではなく「実際に発火したか」で行う。

### 根拠

1. `codex-rs/config/src/hook_config.rs` @ `rust-v0.152.1` — `pub struct MatcherGroup { pub matcher: Option<String>, pub hooks: Vec<HookHandlerConfig> }` / `pub enum HookHandlerConfig { #[serde(rename = "command")] Command { command: String, command_windows, timeout, async, statusMessage, additionalContextLimit }, ... }`
2. 同 — `pub fn into_matcher_groups(mut self) -> [(HookEventName, Vec<MatcherGroup>); 12]` (12 で固定)
3. `codex-rs/features/src/lib.rs` @ `rust-v0.152.1` — `FeatureSpec { id: Feature::CodexHooks, key: "hooks", stage: Stage::Stable, default_enabled: true }` / `FeatureSpec { id: Feature::MemoryTool, key: "memories", stage: Stage::Stable, default_enabled: false }`
4. `codex-rs/hooks/src/engine/discovery.rs` — `let additional_context_limit = if matches!(event_name, HookEventName::PreToolUse | HookEventName::PostToolUse | HookEventName::SessionStart | HookEventName::UserPromptSubmit | HookEventName::SubagentStart) { ... } else { ... "this event cannot emit additionalContext" ... None }`
5. 同 — `if enabled && (source.bypass_hook_trust || matches!(trust_status, HookTrustStatus::Managed | HookTrustStatus::Trusted))`
6. 同 — `/// Normalizes hook timeouts. SessionEnd and Interrupt default to one second and are capped at three seconds; all other hooks keep the standard ten-minute default.` / `let runs_async = r#async && event_name != HookEventName::SessionEnd;`
7. 同 — `fn load_hooks_json(config_folder: Option<&Path>, ...) { let source_path = config_folder?.join("hooks.json"); ... let parsed: HooksFile = serde_json::from_str(&contents) ... }` およびコメント "Hash a normalized, config-derived identity instead of source text so equivalent hooks from config TOML and hooks.json converge on the same trust identity."
8. `codex-rs/hooks/src/output_spill.rs` — `pub(crate) const DEFAULT_HOOK_OUTPUT_TOKEN_LIMIT: usize = 2_500;` / `let token_limit = limit.token_limit; if token_limit == 0 || approx_token_count(&text) <= token_limit { return text; }` / `output_dir: ...std::env::temp_dir()...join(HOOK_OUTPUTS_DIR).join(thread_id.to_string())`
9. `codex-rs/hooks/schema/generated/session-start.command.input.schema.json` — `"source": {"enum": ["startup","resume","clear","compact"], "type":"string"}` (required)
10. `codex-rs/hooks/src/events/session_end.rs` — `pub(crate) const SESSION_END_DEFAULT_TIMEOUT_SEC: u64 = 1;` / `pub(crate) const SESSION_END_MAX_TIMEOUT_SEC: u64 = 3;` / `const SESSION_END_REASON: &str = "other";`
11. `codex-rs/hooks/src/events/session_start.rs` — テスト `fn plain_stdout_becomes_model_context()` および `fn continue_false_preserves_context_for_later_turns()`
12. `codex-rs/hooks/src/events/pre_tool_use.rs` — `fn additional_context_is_recorded()` (`permissionDecision: "deny"` でも `additional_contexts_for_model: vec![AdditionalContext { text: "nope", ... }]`)
13. https://github.com/openai/codex/issues/19385 — title "Support additionalContext in PreToolUse hooks or clarify Claude-style hook parity", `"state":"closed"`, `"state_reason":"completed"`, `"closed_at":"2026-08-04T04:30:46Z"`、本文は `codex-cli 0.124.0` に対する報告
14. `codex-rs/memories/README.md` — "owns the read path: memory developer-instruction injection" / "The pipeline is triggered when a root session starts, and only if: ... the memory feature is enabled"
15. `codex-rs/config/src/types.rs` `MemoriesToml` — `/// When \`true\`, external context sources mark the thread \`memory_mode\` as \`"polluted"\`. pub disable_on_external_context` / `use_memories` / `generate_memories` / `extract_model` / `consolidation_model`
16. `docs/config.md` @ `rust-v0.152.1` §Lifecycle hooks — "Admins can set top-level `allow_managed_hooks_only = true` in `requirements.toml` to ignore user, project, and session hook configs"

### 未確認・要フォロー

- rollout の flush タイミング。`PostToolUse` hook の中で `transcript_path` を読んだとき、直前の tool 呼び出しが既にディスクに落ちている保証はソース上見つからなかった。PostToolUse hook が自分の `tool_use_id` を transcript から grep する probe で安価に確定できる。
- 対話 TUI 経路と plugin 経路の hook trust。今回の確証は `codex exec` + fresh CODEX_HOME の 1 データポイントのみ。ユーザーの実 config には `hooks.json` 由来の `trusted_hash` 行が存在する一方、`plugin_hooks` は 0.152.1 で feature state "removed"。
- `[features] memories = false` が read path 注入まで完全に抑止するか。README の gating からの推論のみで live 未検証。`use_memories = false` の方が確実な梃子。
- `disable_on_external_context` の相互作用。外部 context source があると thread が `polluted` になり Codex 側が記憶生成を止める可能性があり、oboete の注入がこれを踏むかは未検証。
- `codex exec` の rollout が native memories パイプラインの対象になるか (README の "allowed interactive session sources" 記述からは対象外の可能性)。
- `PermissionRequest` / `PreCompact` / `PostCompact` / `Interrupt` の stdin/output schema は存在確認のみで内容未精査。
- hooks は `docs/` にほぼ文書化されていない (`docs/hooks.md` は存在せず、`docs/config.md` の記述は admin スイッチ 4 行のみ)。ソースが唯一の正本。

### 実測

- 複数回の live probe を実施 (いずれも実 `~/.codex` は非書き込み、auth.json は throwaway CODEX_HOME にコピーして実行後削除)。
- `CODEX_HOME=<temp> codex exec --dangerously-bypass-hook-trust --skip-git-repo-check "Run the shell command 'echo hello-world' ..."` で `SessionStart → UserPromptSubmit → PreToolUse → PostToolUse → Stop → SessionEnd` が全て発火し、各 stdin JSON を捕捉。`transcript_path` は `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`、`permission_mode` は `"bypassPermissions"`。
- 検証側は `--dangerously-bypass-hook-trust` **無し**で同一 config を再実行し、hook が 1 つも発火せず exit 0 することを確認。さらに手計算した `[hooks.state] trusted_hash` を置いた config では bypass フラグ無しで発火することを確認 (ユーザーの実 hash 3 件も 3/3 一致)。
- `SessionStart` hook が `{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"The project codeword is ZUCCHINI-7741."}}` を返す probe で `codex exec` の応答が `ZUCCHINI-7741` になり、additionalContext がモデルに実際に届くことを確認。
- 空の `CODEX_HOME` で `codex features list` → `hooks stable true` / `memories stable false`。実 home では `memories stable true`。
- `[[hooks.SessionEnd.hooks]] timeout = 30` を置くと `warning: clamping SessionEnd hook timeout to 3s in .../config.toml`。
- 最新 rollout (2026-09-02) の実測: top-level `{'session_meta':1,'event_msg':529,'response_item':535,'world_state':8,'turn_context':4,'inter_agent_communication_metadata':4}`、payload `{'message':29,'reasoning':194,'custom_tool_call':142,'custom_tool_call_output':142,'function_call':12,'function_call_output':12,'agent_message':4}`。

---

## Grok Build CLI 1.0.17 hooks

### 結論

- hook イベントは 15 個 + `SubagentEnd` は `SubagentStop` の別名 [VERIFIED]。`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PermissionDenied`, `Stop`, `StopFailure`, `StopCancelled`, `Notification`, `SubagentStart`, `SubagentStop`, `PreCompact`, `PostCompact`, `SessionEnd`。バイナリの strings でも同一集合を確認。
- `~/.claude/settings.json` (global は常時 trusted、project は folder trust 必須) を互換層として読み、`~/.grok/hooks/*.json` と**加算的に**マージする [VERIFIED]。さらに調査側が触れていない経路として **Claude Code の plugin hooks** (`~/.claude/plugins/**`) と `~/.cursor/hooks.json` も発見される。空 `GROK_HOME` でも `grok inspect --json` が `/home/jura/.claude` 由来の hook 14 件 + plugin 由来 8 件を列挙した。
- **`SessionStart` と `UserPromptSubmit` の `additionalContext` はモデルに届かない** [VERIFIED、両検証者が独立の統制付き実験で再現]。同一 hook ファイル内に 3 イベント分の handler を置き、全て発火したこと (stdin ダンプ) を確認した上で、モデルに届いたのは `PreToolUse` (または `PostToolUse`) のマーカーのみ。既存の `~/.grok/hooks/claude-mem.json` の `SessionStart` "context" hook はこのバージョンで実質 no-op。
- `Stop` は注入チャネルではない [SECONDARY / 調査側の「PreToolUse/PostToolUse/Stop で注入できる」は反証]。`Stop` の `additionalContext` は「エージェントを働き続けさせる」フィードバックで、1 turn 8 回の継続で強制終了され、session 終了時の Stop では decision output ごと破棄される。
- `PreToolUse` の `additionalContext` は呼び出しの**後**に届く [VERIFIED、調査側の記載漏れ]。"It arrives after the call has run — never before — with the results of the batch the call belongs to ... A `deny` drops all of it"。`SessionStart` が無効なことと合わせると、Grok には turn 開始前に文脈を入れる経路が存在しない。
- native memory (`GROK_MEMORY`) は experimental・既定無効・`~/.grok/memory/` 配下の Markdown + SQLite FTS5 (vec0 は embedding model 設定時のみ)、first turn に独自注入 [VERIFIED]。ただし「hook から feed できない」という主張は根拠が無く反証済み (Markdown ファイルなので書き込み自体は可能)。
- `GROK_HOME` は Grok 自身のツリーしか切り替えない [SECONDARY / 「entire config tree を安全に隔離できる」は反証]。Claude / Cursor 互換層は `$HOME` 基準で解決されるため、`GROK_HOME` を temp に向けた probe でもユーザーの実 `~/.claude/settings.json` の hook (git-ai checkpoint, gitnexus, sonar-secrets 等) が実行されていた。ランチャ `~/.local/bin/grok` も `$HOME/.grok/env` を source する。

### 設計への影響

- oboete の Grok 側注入は `PreToolUse` か `PostToolUse` に置くしかなく、しかも「最初の tool 呼び出しが完了した後」に届く。tool を 1 度も呼ばない turn には何も届かない。これは Grok レーンのスコープ判断そのものなので、(a) 事後注入で妥協するか、(b) AGENTS.md / project rules のような非 hook 面を使うかを明示的に決める必要がある。
- hook の既定 timeout は **5 秒**。例外は `Stop`/`SubagentStop`/`PostToolUse` の 600 秒と `UserPromptSubmit` の 30 秒、`SessionEnd` の 1.5 秒 (キュー全体で 0.5 秒、`GROK_SESSION_END_HOOKS_TIMEOUT_MS` で最大 60 秒)。SQLite を開いて整形する注入 hook は明示的に `timeout` を書かないと殺される。失敗は全て fail-open で無言。
- `PreToolUse` チェーンはどれかの handler が `deny` を返した時点で止まる。ユーザーの既存 guard hook が deny すると oboete の handler は走らない。first-tool-call の PreToolUse を唯一の注入経路にしない。
- `Stop` は 1 セッションで 2 回発火する (turn 終了の `reason: "end_turn"` と、session 終了の `"channel_closed"` / `"shutdown"`)。`reason == "end_turn"` で分岐しないと最終 turn を二重記録し、promptId を持たない幽霊 turn も記録する。
- `Stop` は `lastAssistantMessage` (32,768 文字で clip) を渡すので、turn 捕捉に `updates.jsonl` のパースは要らない。これは「agent ごとに transcript 形式を分岐する」という設計項目のかなりの部分を消す。
- headless の `SessionStart.source` は `"new"`。Claude Code / Codex の `"startup"` をそのまま matcher に書くと発火しない。
- matcher は Claude 名を別名として受け付ける (`Bash` → `run_terminal_command`, `Read` → `read_file`, `Edit|Write|MultiEdit` → `search_replace`, `Task` → `spawn_subagent`) が、payload の `toolName` は Grok のネイティブ名。イベント正規化層でマッピングする。MCP は `server__tool` 形式で来るので `use_tool` にマッチさせても何も取れない。
- エージェント識別は `GROK_HOOK_EVENT` / `GROK_SESSION_ID` を先に見る。`CLAUDE_PROJECT_DIR` は Grok 自身が全 hook にセットするので、これで Claude Code を判定すると必ず誤判定する。
- `~/.claude/settings.json` と `~/.grok/hooks/*.json` は加算的にマージされ、さらに Claude plugin hooks も走る。oboete が Claude Code plugin として配布されると Grok でも二重発火しうるので、書き込みは冪等に。`[compat.claude] hooks = false` / `GROK_CLAUDE_HOOKS_ENABLED` が逃げ道。
- 統合テストは `GROK_HOME` だけでなく `HOME` も切り替える (または `GROK_CLAUDE_HOOKS_ENABLED=0`)。そうしないとユーザーの実 hook を throwaway repo に対して実行してしまう。

### 根拠

1. `~/.grok/docs/user-guide/10-hooks.md` Hook Events 表 (15 行) および直後の "`SubagentEnd` is accepted as an alias for `SubagentStop`."
2. 同 Hook Locations 表 — "Global | `~/.claude/settings.json` (and `settings.local.json`) | Always | Claude Code compatibility (configurable)" / "Project | `<project>/.claude/settings.json` ... | Requires trust"
3. 同 — "**Additive across layers.** Every layer's hooks run; a lower-priority layer adds hooks but never replaces another layer's block. A hook defined identically in more than one layer is deduplicated, keeping the highest-authority copy."
4. 同 — "For events like `SessionStart` or `Notification`, stdout is ignored. Just exit 0 on success." / "One current limit: stdout of an allowing hook is discarded (no `additionalContext`)."
5. 同 (PreToolUse) — "`additionalContext` is a note for the model. It arrives after the call has run — never before — with the results of the batch the call belongs to ... A `deny` drops all of it, since the call never runs"
6. 同 (Stop Decision Control) — "**Non-error feedback**: ... Also keeps the agent working, but is surfaced as hook feedback rather than a hook error." / "After **8 continuations** ... the gate is overridden and the turn ends" / "A separate Stop also fires at session end (`reason: \"channel_closed\"` or `\"shutdown\"`); its decision output is parsed but ignored"
7. 同 — "**timeout**: Seconds before killing the hook (default: 5, or 600 for `Stop`/`SubagentStop`/`PostToolUse` gates)." / "Teardown gives the whole queue of turn-end reports half a second, and each `SessionEnd` hook is then bounded by its own timeout (default 1.5s; set `GROK_SESSION_END_HOOKS_TIMEOUT_MS` ... capped at 60s)."
8. 同 "How a Hook Resolves" step 2 — "Handlers in the selected groups run in config order ... until one returns `deny` (which stops the chain). ... a `PreToolUse` `updatedInput` is applied only after all handlers finish"
9. 同 — "Text over 10,000 characters is clipped, the same ceiling `Stop` feedback carries." / "The block reason and `additionalContext` are clipped at 10,000 characters ... A replacement gets 64 K characters."
10. 同 (Stop input) — "`lastAssistantMessage` carries the text of the agent's final response this turn, so hooks can act on it without parsing the transcript."
11. 同 runner-injected 変数表 — `GROK_HOOK_EVENT` / `GROK_HOOK_NAME` / `GROK_SESSION_ID` / `GROK_WORKSPACE_ROOT` / "`CLAUDE_PROJECT_DIR` | Absolute path to the workspace root. A Claude Code-compatible alias for `GROK_WORKSPACE_ROOT`, set for every hook." / "These variables are **reserved**. Any values you attempt to set for them via the `env` field in your hook JSON are stripped at load time"
12. 同 — "Grok maps Claude-style tool names to its own ... `Bash` → `run_terminal_command`, `Read` → `read_file` ... A matcher keeps its original name too" / "MCP calls ... appear as the qualified `server__tool` name (e.g. `linear__save_issue`), so match on that, not the dispatcher name."
13. `~/.grok/docs/user-guide/13-memory.md` — "Memory is experimental and disabled by default." / "Memory is stored as Markdown files under `~/.grok/memory/`" / "**FTS5** provides the default full-text search ... **vec0** adds vector search ... when an embedding model is configured" / "The default embedding model is unset, so memory starts in full-text-only mode."
14. `~/.grok/docs/user-guide/26-config-reference.md` — "| `compat.claude.hooks` | `boolean` | ... | Scan Claude hooks. Also GROK_CLAUDE_HOOKS_ENABLED. |"
15. `~/.grok/README.md` — "Claude Code plugins can provide skills (`skills/`), commands (`commands/`), agents (`agents/`), hooks (`hooks/hooks.json`) ... All component types are discovered and used by Grok at runtime."

### 未確認・要フォロー

- 同一 event+matcher に**非同一**の handler が複数レイヤから登録された場合の総合的な実行順序。docs は「加算的、全部走る」「byte-identical は重複排除」までしか保証していない。handler レベルの「config order」は明示されているが、レイヤ間の順序は未公表。
- `PermissionDenied` / `SubagentStart` / `SubagentStop` / `PreCompact` / `PostCompact` / `StopFailure` / `StopCancelled` の payload は docs のみで live 未確認。
- `[compat.claude] hooks = false` / `GROK_CLAUDE_HOOKS_ENABLED=0` が実際に compat scan を抑止するかは未検証。
- hook stdout の生バイト上限 (バイナリに `total_bytes` / `max_bytes` / `hook output truncated` の文字列があるが値は非公開)。上限で切られると JSON が途中で切れて malformed → fail-open で無言に落ちる。
- resume 時の `SessionStart` に `transcriptPath` が入るか。`source: "new"` での不在しか観測していない。
- `transcriptPath` / `transcript_path` は **hooks doc に一切記載が無い** (grep で user-guide 全体でも `25-status-line.md` の 2 箇所のみ、しかもそれは status line 用 stdin の説明)。実在は再現済みだが仕様上の保証は無い扱いにする。
- headless で `Notification` が 1 度も発火しなかった。「headless は特別扱い不要」は言い過ぎ。
- hook は `/hooks` モーダル・enable/disable API・`disabled-hooks` ファイルで個別に無効化できる。「インストール済み」= 「動作中」ではない。`/hooks-list` が実際にロードされたものを出す。
- native memory はセッション終了時に LLM 呼び出し無しでメタデータ要約を自動保存し、workspace ディレクトリを git `origin` remote のハッシュで keying する (clone / worktree は同一メモリを共有)。auto-compaction 後にも再注入する。oboete の identity 設計と衝突/参考になる点。
- `14-headless-mode.md` の File Locations 表は `sessions/` を "Session transcripts (SQLite)" と書いているが、実体はセッションごとの `updates.jsonl` / `chat_history.jsonl` / `events.jsonl` / `rewind_points.jsonl` + `session_search.sqlite`。表を信用しない。

### 実測

- `GROK_HOME` を scratchpad に向け、throwaway git repo を `--cwd` にして `grok -p ... --always-approve --output-format json` を複数回実行 (auth.json はコピー、実行後削除。実 `~/.grok` は非書き込み)。
- 6 イベント登録の run で `SessionStart` x1, `UserPromptSubmit` x1, `PostToolUse` x1, `Stop` x2 (`end_turn` → `shutdown`), `SessionEnd` x1 が発火、`Notification` は 0。
- マーカー実験: `SessionStart` / `UserPromptSubmit` / `PreToolUse` (別 run では PostToolUse) の各 handler が固有マーカーを `hookSpecificOutput.additionalContext` で返し、全 handler の発火を stdin ダンプで確認した上でモデルに echo させたところ、届いたのは PreToolUse/PostToolUse のマーカーのみ。SessionStart / UserPromptSubmit のマーカーはセッションの全ファイルを grep しても 0 件。
- サイズ実験: 約 29,000 文字の additionalContext を PostToolUse から返し、offset 9,000 のマーカーは届き offset 約 29,011 のマーカーは届かず。10,000 文字 clip がエラー無しの静かな切り捨てであることを確認。
- compat 実験: `GROK_FOLDER_TRUST=0` + project `.claude/settings.json` の hook + global `~/.grok/hooks/*.json` の hook で、同一 turn に両方のマーカーが `<system-reminder> Context from PreToolUse hook 'global/...'` / `'project/settings:...'` として provenance 付きで届いた。
- 空 `GROK_HOME` での `grok inspect --json` が `vendor: "claude"` の hook 14 件 (`source.path: /home/jura/.claude`) と Claude plugin 由来 8 件を discovery。`dbg.log` に `path=/home/jura/.claude/settings.json` の警告行も残った (= `GROK_HOME` は隔離にならない)。
- 調査側が「説明不能なマーカー」として open point に挙げた文字列は、同 turn の `grep` tool 結果に probe スクリプト自身の中身が含まれていたためと判明 (hallucination ではない)。

---

## Pi 0.84.4 extension API (@earendil-works/pi-coding-agent)

### 結論

- パッケージ名は `@earendil-works/pi-coding-agent` [VERIFIED]。`@earendil-works/pi` は npm に存在しない (E404)。installed 0.84.4 = npm latest なのでリリース済みのドリフト無し。`engines: { "node": ">=22.19.0" }`。
- 拡張は `~/.pi/agent/extensions/*.ts` (global) / `.pi/extensions/*.ts` (project) から jiti でロードされ、default export の factory `(pi: ExtensionAPI) => {...}` は sync/async どちらも可 [VERIFIED]。async なら `session_start` / `resources_discover` / provider 登録の前に await される。
- `pi.on()` のイベントは 36 個で確定 [VERIFIED]。`types.d.ts` の overload を両検証者が独立に列挙して一致。設計が仮定していた `before_agent_start` / `tool_execution_end` / `session_shutdown` は 3 つとも実在し、`before_agent_start` は prompt 送信後・agent loop 前のフックである点も正しい。
- 文脈注入は `before_agent_start` の戻り値 `{message, systemPrompt}` [VERIFIED、live で end-to-end 確認済み]。`pi.appendEntry()` は `type: "custom"` の `CustomEntry` を書き、`sessionEntryToContextMessages` が `[]` を返すため LLM には**絶対に届かない** (機構レベルで確認)。注入メッセージは実際の provider payload では **user prompt の後ろ**に `role: "user"` + `cache_control: ephemeral` として並ぶ。
- 拡張は完全に in-process、sandbox 無し、**handler timeout も無い** [VERIFIED、コードレベル]。`dist/core/extensions/runner.js` に `setTimeout` / `AbortSignal.timeout` / `Promise.race` が 0 件で、全 dispatch が `for (const handler of handlers) { await handler(event, ctx) }`。ハングした handler を強制終了する機構が存在しない。例外は catch されるが、注入 handler が throw すると文脈だけが**無言で消える** (fail-open)。
- tool イベントの購読設計は要修正 [SECONDARY / 調査側の「tool_execution_end だけでは args と result の両方を取れないので複数購読が必要」は半分反証]。`tool_execution_end` に args が無いのは正しいが、`tool_result` は `input` (args) と `content` (結果) と `isError` と `usage` を**全部**持つので、単一購読で足りる。

### 設計への影響

- oboete の Pi 拡張は `~/.pi/agent/extensions/oboete.ts` に **global で**入れる。project-local の `.pi/extensions` は project trust が解決するまでロードされず、`-p` / `--mode json` / `--mode rpc` では trust プロンプト自体が出ないので (既定 `defaultProjectTrust: "ask"`)、CI/スクリプト実行で黙って何も捕捉しない。
- 配布は手置きではなく `pi install npm:@oboete/pi` (settings.json の `packages` 配列) を検討する。project settings に入れれば trust 後に起動時自動インストールされる。他 3 エージェントの installer 物語と最も近い経路。
- tool 記録は `tool_result` 一本で足りる。`toolName` / `input` / `content` / `isError` / `usage` が揃う。
- turn 終了の捕捉点は `turn_end` ではなく `agent_settled`。実測で 1 プロンプト・2 LLM ターンの run において `turn_end` は 2 回、`agent_end` / `agent_settled` は 1 回。`context` は LLM 呼び出しごとに発火する。
- 「ユーザーがプロンプトを送った」の正しいフックは `input` (`before_agent_start` より前、skill/template 展開前)。`source: "interactive" | "rpc" | "extension"` を持つので、拡張が生成した入力や RPC 駆動の入力を人間の入力として記録してしまう汚染を避けられる。
- 注入タイミングの選択肢は `before_agent_start` (毎プロンプト) だけではない。`pi.sendMessage(..., {deliverAs: "nextTurn"})` は `session_start` から「最初のユーザー turn に 1 度だけ」の注入ができ、`context` イベントは LLM 呼び出しごとに全メッセージ配列の deep copy を受け取って置換できる。session 単位注入か prompt 単位注入かは M1 で決める判断事項。
- 拡張内で blocking I/O をしない。timeout が無く逐次 await なので、SQLite ロック待ちがそのまま Pi のプロンプト処理を止める。detach した子プロセスに投げる。その子プロセスは `PI_SESSION_ID` / `PI_SESSION_FILE` を継承環境から読める (bash tool 経由の子プロセスに Pi が注入する)。
- 注入失敗は fail-open で無言。SQLite 読み出しが throw すると記憶ゼロのまま普通に応答が返り、ユーザーにも session にも痕跡が残らない。oboete 側で明示的な劣化マーカーを出すか決める。
- `ctx.signal` は `session_start` / `before_agent_start` / `agent_settled` / `session_shutdown` では undefined。oboete が最も使いたい 2 フックでこれを timebox に使えないので、自前の `AbortController` が要る。
- `--no-extensions` / `-ne` で拡張 discovery を全部切れる (明示 `-e` パスは残る)。イベントログの完全性を前提にしない。
- session ファイルパスは `ctx.sessionManager.getSessionFile()` から取るが、`string | undefined` であり、かつ返ってきたパスのファイルがまだ存在しないことがある (実測: API key 不在で中断した run では sessions ディレクトリだけ作られ .jsonl は 0 件)。

### 根拠

1. `<node_modules>/@earendil-works/pi-coding-agent/package.json` — `"name": "@earendil-works/pi-coding-agent", "version": "0.84.4"`, `"engines": { "node": ">=22.19.0" }`; `npm view @earendil-works/pi` → E404
2. `docs/extensions.md` §Extension Locations — `~/.pi/agent/extensions/*.ts` (Global) / `.pi/extensions/*.ts` (Project-local) 他、および "Project-local `.pi/extensions` entries load only after the project is trusted."
3. 同 — "Extensions are loaded via [jiti](...), so TypeScript works without compilation." / "If the factory returns a `Promise`, pi awaits it before continuing startup."; `dist/core/extensions/loader.js` — `const jiti = createJiti(import.meta.url, {...})` / `await jiti.import(extensionPath, { default: true })`
4. `dist/core/extensions/types.d.ts` の `on(event: ...)` overload 36 本 (catch-all 無し)
5. 同 — `interface ToolExecutionEndEvent { type; toolCallId; toolName; result: any; isError: boolean }` と `interface ToolResultEventBase { type: "tool_result"; toolCallId; input: Record<string, unknown>; content: (TextContent|ImageContent)[]; isError: boolean; usage?: Usage }`
6. 同 — `interface BeforeAgentStartEventResult { message?: Pick<CustomMessage, "customType"|"content"|"display"|"details">; /** Replace the system prompt for this turn. If multiple extensions return this, they are chained. */ systemPrompt?: string }`
7. `dist/core/session-manager.d.ts` — `CustomEntry` のコメント "For injecting content into context, see CustomMessageEntry."; `dist/core/session-manager.js` `sessionEntryToContextMessages` が `type: "custom"` に対して `[]` を返す
8. `docs/security.md` — "Pi does not include a built-in sandbox. ... Extensions are TypeScript modules that run with the same permissions."
9. `dist/core/extensions/runner.js` — `async emit(event)` 内の `const handlerResult = await handler(event, ctx);` を含む逐次ループと per-handler try/catch (`emitError`)。`setTimeout` / `race` の出現 0 件。`ui_prompt_start` / `ui_prompt_end` のみ `queueMicrotask(() => { void this.emit(event); })`
10. `docs/session-format.md` — "Sessions are stored as JSONL (JSON Lines) files. Each line is a JSON object with a `type` field. Session entries form a tree structure via `id`/`parentId` fields" / `~/.pi/agent/sessions/--<path>--/<timestamp>_<uuid>.jsonl`
11. `docs/extensions.md` Mode Behavior 表 — "| Print (`-p`) | `\"print\"` | `false` | Extensions run but can't prompt |"
12. `docs/environment-variables.md` — "PI_CODING_AGENT_DIR | Override the config directory; default is ~/.pi/agent" / "PI_CODING_AGENT_SESSION_DIR | Override session storage; overridden by --session-dir" / §Shell Tool Session Environment "PI_SESSION_ID | Current session ID", "PI_SESSION_FILE | Absolute path to the current session JSONL file; unset for ephemeral sessions", `AI_AGENT=pi`, `PI_CODING_AGENT=true`
13. `types.d.ts` — `InputEvent { type: "input"; text; images?; source: InputSource; streamingBehavior? }` / `type InputSource = "interactive" | "rpc" | "extension"` / `SessionShutdownEvent { type; reason: "quit"|"reload"|"new"|"resume"|"fork"; targetSessionFile? }`
14. `docs/extensions.md` §context — "Fired before each LLM call. Modify messages non-destructively." / `pi.sendMessage` options `deliverAs` — "`\"nextTurn\"` - Queued for next user prompt."
15. `docs/security.md` — "Non-interactive modes (`-p`, `--mode json`, and `--mode rpc`) do not show a trust prompt. Without an applicable saved trust decision, `defaultProjectTrust: \"ask\"` and `\"never\"` ignore such resources"
16. `docs/packages.md` — "pi install npm:@foo/bar@1.0.0 ..." / "Project settings can be shared with your team, and pi installs any missing packages automatically on startup after the project is trusted."
17. `pi --help` — "--no-extensions, -ne  Disable extension discovery (explicit -e paths still work)" / "-p, --single <PROMPT>  Single-turn prompt."

### 未確認・要フォロー

- `pi -p --no-session` が固着する (同一コマンドから `--no-session` を外すと即座に 401 で終了するのに、付けると 60 秒 timeout で exit 124、`session_start` すら発火しない)。再現するが原因未特定。summarizer worker が `pi -p --no-session` を叩く設計なら先に潰す。
- `/reload` によるホットリロード、複数拡張の `systemPrompt` / `tool_result` チェーン順は未検証 (docs は「ロード順」とだけ言う)。
- `pi.registerTool()` によるカスタムツール登録は未検証 (oboete の hook 設計には不要)。
- GitHub の `earendil-works/pi` main branch との差分は取っていない。ただし installed 0.84.4 = npm latest なので、リリース済みの範囲ではドリフト無し。
- `--mode json` / `--mode rpc` は `-p` とは別の非対話モード。`ctx.hasUI = false` は同じだが、trust 挙動と拡張ロードは別途確認が要る。
- 引用衛生の注意: 調査側の "Lifecycle Overview diagram" 引用はページに存在しない文字列 (ASCII 図の言い換え)、`getSessionId()` / `getSessionFile()` の出典も `docs/extensions.md` の `ctx.sessionManager` 節ではなく `dist/core/session-manager.d.ts`。M1 spec ではこの 2 点を正しい出典に差し替える。

### 実測

- 検証側が独立に live probe を実施 (`PI_CODING_AGENT_DIR` を scratchpad に向け、`PI_OFFLINE=1`、dummy API key。実 `~/.pi` は非改変)。調査側の probe は API key 不在で `before_agent_start` に到達できなかったが、`dist/core/agent-session.js` が `hasConfiguredAuth` を `emitBeforeAgentStart` の**前**にチェックしているため、dummy key を入れるだけでライフサイクル全体を通せると判明。
- 実観測イベント順 (1 プロンプト・1 tool 呼び出し・2 LLM ターン): `session_start → resources_discover → input → before_agent_start → agent_start → turn_start → message_start → message_end → context → before_provider_headers → before_provider_request → after_provider_response → message_start/update/end → tool_execution_start → tool_call → tool_execution_update → tool_result → tool_execution_end → message_start/end → turn_end → [turn 2] → agent_end → agent_settled → session_shutdown(reason=quit)`。
- 注入の end-to-end 確認: `before_agent_start` が `{message:{customType:"oboete", content:"INJECTED_MARKER_XXX", display:true}, systemPrompt: e.systemPrompt + "SYSPROMPT_MARKER_YYY"}` を返し、`before_provider_request` で覗いた実 payload に両マーカーが存在。session JSONL には `{"type":"custom_message","customType":"oboete","content":"INJECTED_MARKER_XXX",...}`。
- `pi.appendEntry("oboete-state", {v:"ENTRY_MARKER_ZZZ"})` は JSONL に `{"type":"custom","customType":"oboete-state","data":{...}}` を書いたが provider payload には出ず (`entry=false`)。
- 実 payload での注入位置: `messages[0]` = role user (ユーザーのプロンプト)、`messages[1]` = `{"role":"user","content":[{"type":"text","text":"MEMORY_INJECT_MARKER_...","cache_control":{"type":"ephemeral"}}]}`。`context` イベント時点では `role: "custom"` だが wire では `user` に平坦化される。
- モック provider (`pi.registerProvider` + ローカル SSE サーバ) を使い、実際に `bash` tool を呼ばせて `tool_call` / `tool_result` / `tool_execution_*` の payload キーを実観測。`tool_result :: keys=[type,toolName,toolCallId,input,content,details,isError,usage]`、`tool_execution_end :: keys=[type,toolCallId,toolName,result,isError]`。

---

## Cloudflare Workers AI @cf/zai-org/glm-4.7-flash (observer LLM)

### 結論

- `@cf/zai-org/glm-4.7-flash` はカタログに存在し、context window 131,072、function calling / reasoning あり、$0.0605/M input・$0.40/M output、neuron 換算 5,500 / 36,400 [VERIFIED]。**paid 必須リストには入っていない**ので 10,000 neurons/day の無料枠対象。paid 必須は `kimi-k2.6`, `kimi-k2.7-code`, `glm-5.2`, `glm-5.3`, `glm-5.3-flash`, `deepseek-v4-flash-0731`, `deepseek-v4-pro-0813` の 7 つ。
- `POST /accounts/{id}/ai/run/{model}` と `POST /accounts/{id}/ai/v1/chat/completions` の両方が `messages` + `response_format: {type:"json_schema", ...}` を受け付け、スキーマ準拠の JSON を返す [VERIFIED]。しかも準拠は偶然ではない: 敵対的スキーマ (捏造キー名 + enum + ネスト配列 + `additionalProperties:false` + `strict:true`) でも型・enum 所属・ネストキーが全て守られた = 実際に文法制約デコードが効いている。ただし glm-4.7-flash は**公式の JSON Mode 対応モデル一覧 (9 モデル) に入っていない**ので、契約としては未保証。
- neuron 消費の見積もりは調査側の数値を採用できない [SECONDARY / 反証成立]。調査報告自体が summary で 515–788、claim 本文で 420–788 と自己矛盾しており、独立実行では 322 / 411 / 448 / 453 / 1084 / 1241 completion tokens、11.92 / 15.18 / 16.53 / 16.69 / 39.62 / 45.45 neurons と両側に外れた。現実的な observer スキーマ (`entries[]` of `{kind:enum, text, confidence}` + `session_topic`、入力 51 tokens) で **45.45 neurons** = 無料枠は実質 **1 日約 220 コール**。
- reasoning は抑制できない [VERIFIED、negative]。model ページに文書化されている `reasoning_effort: "low"` と `chat_template_kwargs: {thinking: {type:"disabled"}}` を両方試して HTTP 200 で受理されるが reasoning 長は変わらない (1683 / 1507 文字)。`reasoning` と `reasoning_content` は同一文字列の重複。
- 実際の失敗モードは「JSON Mode couldn't be met」ではない [VERIFIED、新規発見]。`max_completion_tokens` で出力を絞ると reasoning が予算を食い尽くし、**HTTP 200 / success:true / `content: null` / `finish_reason: "length"` のまま課金**される。`JSON Mode couldn't be met` という文字列は Workers AI の errors 表に存在しない。
- neuron 消費は body の `usage.neurons` と `cf-ai-neurons` レスポンスヘッダの両方で返る [VERIFIED] が、それをクライアント側で合算しても**アカウント日次残量にはならない** [SECONDARY / 「ローカルで日次上限を強制できる」は反証]。10,000/day はアカウント単位で、同一アカウントの Workers や Wrangler local mode の推論も算入される。

### 設計への影響

- 既定 observer は glm-4.7-flash のままでよいが、**コスト前提を約 2 倍厳しく引き直す**。1 コール ~45 neurons、無料枠 ~220 コール/日。turn ごとに 1 コールする設計は成立しない可能性が高く、複数 turn をまとめて 1 コールにするバッチ化か、reasoning しない安価なモデル (`@cf/ibm-granite/granite-4.0-h-micro` 1542/10158、`@cf/meta/llama-3.2-3b-instruct` 4625/30475) への切り替えを M1 で判断する。平均ではなく裾で予算を組む (322 → 1241 tokens の分散)。
- `max_completion_tokens` をコスト制御に使わない。null 結果を満額で買うだけになる。パース層は `content == null` と `finish_reason == "length"` を第一級の失敗として扱う。
- 失敗検知はエラーコードで行う: `3036` / 429 = 日次 10k 枯渇 (**リトライ禁止**、これが権威ある停止信号)、`5035` / 403 = paid プラン必須モデル (無料モデルが有料化された瞬間の検知信号。カタログ polling より安く確実)、`3006` / 413 = リクエスト過大、`3040` / 429 と `3007` / 408 = 一時的 (リトライ可)。429 を一括りにすると死んだ予算を叩き続ける。
- モデルの有料化検知は「価格エントリの形が変わったら」ではなく、カタログの `require_workers_paid` プロパティを読むか、`5035` を捕まえる。加えて **id を pin しても価格は pin されない**: `kimi-k2.5` → `kimi-k2.6` (より高価かつ paid 必須) への無言 alias の前例がある。レスポンス body が返す `"model"` を要求 id と毎回突き合わせて不一致でアラームするのが最も安い防御。
- `usage.neurons` の合算は**ペース配分用の推定値**として使い、日次リセットは 00:00 UTC。権威ある停止は `3036`。残量を返す API は存在しない。
- レート制限は制約にならない (Text Generation 既定 300 req/min、frontier の 20 req/min 表に glm-4.7-flash は入っていない)。
- JSON Mode はストリーミング非対応。detached background worker なので実害は無いはずだが記録しておく。
- `/ai/run` はこのモデルで **OpenAI 形状の body** を返す (`result.response` は null、テキストは `result.choices[0].message.content`)。旧来の `{result:{response}}` 前提のクライアントは無言で null を掴む。両エンドポイントは `d.get('result', d)` 相当の 1 行で吸収でき、OpenAI 互換パスの fallback を残すコストはほぼゼロ。
- `reasoning` / `reasoning_content` は同一内容なので両方を永続化しない。
- glm-4.7-flash には cached input 価格が存在しない (glm-5.3-flash 等にはある) ので、observer の固定 system prompt を毎回フル価格で払う。`prompt_tokens_details.cached_tokens` は常に 0 で飾り。

### 根拠

1. https://developers.cloudflare.com/workers-ai/platform/pricing/index.md (Last updated Aug 28, 2026) — "Some models require a paid billing method. This applies to `@cf/moonshotai/kimi-k2.6`, `@cf/moonshotai/kimi-k2.7-code`, `@cf/zai-org/glm-5.2`, `@cf/zai-org/glm-5.3`, `@cf/zai-org/glm-5.3-flash`, `@cf/deepseek-ai/deepseek-v4-flash-0731`, and `@cf/deepseek-ai/deepseek-v4-pro-0813`."
2. 同 — "| @cf/zai-org/glm-4.7-flash | $0.060 per M input tokens  $0.400 per M output tokens | 5500 neurons per M input tokens  36400 neurons per M output tokens |" / "Our free allocation allows anyone to use a total of **10,000 Neurons per day at no charge**." / "All limits reset daily at 00:00 UTC."
3. https://developers.cloudflare.com/workers-ai/models/glm-4.7-flash/index.md — "| Context Window | 131,072 tokens | Function calling | Yes | Reasoning | Yes |"、Input parameters に `reasoning_effort` (enum low/medium/high), `chat_template_kwargs`, `response_format`
4. https://developers.cloudflare.com/workers-ai/features/json-mode/index.md (Last updated Apr 21, 2026) — 対応モデル 9 件の列挙 (glm-4.7-flash は不在) / "Note that Workers AI can't guarantee that the model responds according to the requested JSON Schema. ... If that's the case, then an error `JSON Mode couldn't be met` is returned and must be handled." / "JSON Mode currently doesn't support streaming."
5. https://developers.cloudflare.com/workers-ai/platform/errors/index.md (Last updated Jul 29, 2026) — "| Account limited | 3036 | 429 | You have used up your daily free allocation of 10,000 neurons. ... |" / "| Model requires Workers Paid plan | 5035 | 403 | ... |" / "| Request too large | 3006 | 413 |" / "| Out of capacity | 3040 | 429 |" / "| Timeout | 3007 | 408 |"
6. https://developers.cloudflare.com/workers-ai/platform/limits/index.md (Last updated Aug 7, 2026) — "### Text Generation * 300 requests per minute"、"#### Frontier models" 表は kimi-k2.6 / kimi-k2.7-code / glm-5.2 の 3 行のみ / "Note that model inferences in local mode using Wrangler will also count towards these limits."
7. https://developers.cloudflare.com/workers-ai/changelog/index.md — 2026-02-13 "@cf/zai-org/glm-4.7-flash is now available on Workers AI!" / 2026-05-08 "On May 30, 2026, requests to @cf/moonshotai/kimi-k2.5 will be automatically aliased to @cf/moonshotai/kimi-k2.6, which has a higher price."
8. https://developers.cloudflare.com/workers-ai/configuration/open-ai-compatibility/index.md — 全文に `response_format` / `json_schema` の記載が 0 件
9. live: `GET /accounts/{id}/ai/models/search?search=glm` — glm-5.2 / glm-5.3 / glm-5.3-flash に `require_workers_paid = "true"`、glm-4.7-flash には無し
10. live: `POST /ai/run/@cf/zai-org/glm-4.7-flash` + `"max_completion_tokens": 200` — `HTTP/2 200`, `cf-ai-neurons: 7.50`, `finish_reason: length`, `content: None`

### 未確認・要フォロー

- テストアカウントの Workers プラン (Free / Paid) を確定していない。live probe の成功は「無料枠で通った」証拠にはならず (paid 必須モデルは Free でのみ弾かれる)、無料枠適合の結論はドキュメント根拠のみ。`@cf/zai-org/glm-5.3-flash` へ 1 コール投げて `5035`/403 が返るかを見れば確定する (未実施)。
- 「2026-07-28 に一部モデルが有料化された」という設計ナラティブ上の日付は確認できない [UNKNOWN]。Workers AI の changelog は 2026-06-16 以降更新が止まっており (pricing ページは Aug 28 更新)、少なくとも 6 モデルが changelog に載らないまま価格表に追加されている。changelog は監視ソースとして使えない。日付自体は設計から落とすべき。
- `JSON Mode couldn't be met` エラーパスは 1 度も観測できていない。エラー表にも文字列が無いため、実在するのか、どのステータス/コードで返るのかが不明。
- 現実的な観測ペイロード (実会話 turn を含む数千トークンの prompt) でのコスト再測定が未了。今回の最大でも入力 51 トークン。
- AI Gateway 経由 / prepaid credits 経由でのヘッダ・課金挙動は未検証。
- reasoning を止める第三の手段 (未文書化パラメータ等) の有無。

### 実測

- 認証付き REST 呼び出しを計 10 回程度 (調査 6 + 検証側の独立 4 + 追加 3)。account_id `486f92f733bf7fcf7aa94fa4738eeeea`、`X-Auth-Email` / `X-Auth-Key` (キーはシェル変数経由、出力に露出無し)。
- `GET /ai/models/search?search=glm-4.7` は HTTP 500 (`{"code":1000,"message":"Server Error"}`)、`?search=glm` は成功。カタログ検索は完全一致 model 名では信頼できない。
- `/ai/run` 2 回 + `/ai/v1/chat/completions` 2 回 (調査)、`/ai/run` 3 回 (検証 A)、`/ai/run` 4 回 (検証 B) をスキーマ付きで実行。全て HTTP 200・スキーマ準拠。
- 敵対的スキーマ `{qzx_level:integer, frobnitz:enum[alpha,beta], widgets:array<{k:string,v:number}>}` (`additionalProperties:false`, `strict:true`) → `{"qzx_level":0,"frobnitz":"beta","widgets":[{"k":"wrangler.toml","v":-1}]}`。
- 現実的 observer スキーマ (`entries[]` + `session_topic`, 入力 51 tokens) → `cf-ai-neurons: 45.45`, `completion_tokens: 1241`, reasoning 4687 文字。
- `max_completion_tokens: 200` → 200 / success:true / `content: None` / `finish_reason: length` / 7.50 neurons 課金。
- `reasoning_effort: "low"` → 448 tokens / reasoning 1683 文字 / 16.53 neurons。`chat_template_kwargs: {"thinking": {"type":"disabled"}}` → 411 tokens / reasoning 1507 文字 / 15.18 neurons。どちらも無効。

---

## Cloudflare R2 S3 API + aws4fetch (同期先)

### 結論

- エンドポイントは `https://<ACCOUNT_ID>.r2.cloudflarestorage.com`、region は `auto` (空文字と `us-east-1` は alias) [VERIFIED]。ただしこれは**完全ではない**: jurisdiction 付きバケットは `<ACCOUNT_ID>.eu.` / `.us.` / `.fedramp.` 専用ホストからしかアクセスできず、多くの S3 クライアントは複数エンドポイントを持てない。
- SigV4 資格情報は R2 API token からのみ得られる: Access Key ID = token の `id`、Secret Access Key = token `value` の SHA-256 [VERIFIED]。Global API Key で SigV4 は署名できない [SECONDARY — 否定を明言した公式文はないが、(a) S3 access key の定義が token id + SHA-256 であること、(b) Global API Key は 1 ユーザー 1 個の legacy スキームであること、(c) "Object-level tokens are only supported by the S3-compatible API, which authenticates with AWS Signature Version 4 (SigV4)" の 3 点で機構的に裏付け]。
- bucket スコープの Object Read & Write / Object Read トークンは可能で、これが BYO バケットの最小権限 [VERIFIED]。プログラム発行も可能で、エンドポイントは汎用の `POST /client/v4/user/tokens`、権限グループは `Workers R2 Storage Bucket Item Write` / `... Item Read`、Access Policy の resource key は `com.cloudflare.edge.r2.bucket.<ACCOUNT_ID>_<JURISDICTION>_<BUCKET_NAME>` (jurisdiction 無しは `default`)。
- 無料枠は storage 10 GB-month、Class A 100 万 req/月、Class B 1000 万 req/月、egress 無料 [VERIFIED]。ただし **delete は Class A にも Class B にも属さない** [調査側の「PUT/POST/LIST/DELETE が Class A」は反証] ので、古い diff/snapshot の刈り取りは実質無料。無料枠は Standard storage のみで Infrequent Access には適用されない。
- **Cloudflare は aws4fetch + R2 の公式 example ページを持っている** [VERIFIED / 調査側の「そんな例は存在しないので手書きするしかない」は反証]。しかも Cloudflare 自身の ListBuckets / ListObjectsV2 例は service も region も渡さない。aws4fetch のソースには `if (hostname.endsWith('.r2.cloudflarestorage.com')) return ['s3', 'auto']` というハードコード分岐があり、公開 npm 1.0.20 にも入っている。資格情報不要でオフライン再現済み。ただしこの分岐は **1.0.15 以降にしか存在しない** (1.0.12 / 1.0.14 では 0 件)。
- 条件付き PUT は `If-Match` も `If-None-Match` も PutObject で公式サポート [VERIFIED / `If-None-Match` の SECONDARY は互換性表の PutObject 行 verbatim で格上げ]。バージョニング・タグ・bucket policy・ACL・Object Lock・Public Access Block・SSE-KMS は未実装。ただし `PutBucketLifecycleConfiguration` / `GetBucketLifecycleConfiguration` は**実装済み** (未実装なのは legacy の `PutBucketLifecycle` のみ)。

### 設計への影響

- 「手書きで組む / 未検証」の但し書きを外す。`new AwsClient({accessKeyId, secretAccessKey})` + `client.fetch(url, {method, body})` でよい。`service:'s3', region:'auto'` を明示するのは保険として無害だが、必須の回避策ではない。**aws4fetch は >= 1.0.15 に pin する** (下回ると service='' / region='us-east-1' で署名し 403 SignatureDoesNotMatch)。
- 設定にはアカウント id だけでなく**エンドポイント host か jurisdiction** を持つ。EU/US/FedRAMP バケットは既定ホストから到達できず、token の Access Policy 文字列にも同じ jurisdiction が埋まる。
- オンボーディングの前提条件が 1 段増える: **「R2 を購入/有効化していないと API token を作れない」**。「無料で cmem pro 相当」を掲げる以上、ここが最大の離脱点。CLI のエラーメッセージも「トークンが不正」ではなく「R2 未有効化」を別扱いにする。本ユーザーのアカウントは既に通過済みなのでローカルテストでは再現しない。
- token 種別を明示する: User API token は「ユーザーがアカウントから外れると無効化」される。長寿命のバックグラウンド同期には Account API token が向くが、作成には Super Administrator 権限が要る。
- 初回 push は `If-None-Match: *`、上書きは前回の GET/PUT が返した ETag を**二重引用符ごとバイト一致で** `If-Match` に入れる (Cloudflare の例は `'"29d911..."'` と引用符を含む。外すと毎回 412)。
- **同一キーへの書き込みは 1 秒 1 回で 429**。固定 manifest キーに対する楽観的並行制御ループはこの制限に直撃する。しかも aws4fetch は 429 を既定 `retries: 10` で指数バックオフ再試行 (最大 ~25.6 秒) するため、短命な hook プロセスがログ無しで数十秒ブロックされうる。`retries` を明示的に下げるか 0 にし、diff チェーンのホットパスを単一キーに置かない。再試行時は body を再送するのでストリーム body は使えない。
- 古い diff/snapshot の期限切れは `PutBucketLifecycleConfiguration` でサーバ側に押し付ける。クライアント GC は書かない。delete 自体も課金クラス外。
- R2 は list-after-write を含めて強整合なので、書いた直後の読み戻しポーリングや結果整合の調停コードは一切不要。逆に **IAM は結果整合で新規トークンが最大 1 分効かない**ので、トークンを貼った直後の検証は再試行付きにする。
- キーに世代/系譜を埋める設計は key length 1,024 バイト、custom metadata 8,192 バイトの予算内で組む。単一 PUT は 4.995 GiB までで、それ以上は multipart 必須 (part 最小 5 MiB / 最大 5 GiB / 最大 10,000)。
- Class A 予算の実態は見出し数字より厳しい: ListObjectsV2 の**ページごと**、multipart の**パートごと**が別々の Class A。毎回の pull で diff チェーンを LIST するより、単一のポインタオブジェクトを GetObject (Class B) する設計が安い。
- aws4fetch は S3 に対し既定で `X-Amz-Content-Sha256: UNSIGNED-PAYLOAD` を付けるので署名は body を保護しない。age の AEAD があるので実害は無いが、意図的な選択として記録する。必要なら Content-MD5 / CRC64NVME 等を明示的に付けられる。
- 将来 presigned URL を使うなら注意: Cloudflare の aws4fetch example ページは「Content-Type を署名すれば制限できる」と書いているが、実出力は `X-Amz-SignedHeaders=host` のみで Content-Type は署名されない (aws4fetch の `UNSIGNABLE_HEADERS` に含まれるため)。`aws: { signQuery: true, allHeaders: true }` が必要。

### 根拠

1. https://developers.cloudflare.com/r2/api/s3/api/ (Last updated Jul 31, 2026) — "The API is available via the `https://<ACCOUNT_ID>.r2.cloudflarestorage.com` endpoint." / "When using the S3 API, the region for an R2 bucket is `auto`. For compatibility with tools that do not allow you to specify a region, an empty value and `us-east-1` will alias to the `auto` region."
2. 同、PutObject 行 — "| ✅ PutObject | ✅ Conditional Operations: ✅ If-Match ✅ If-Modified-Since ✅ If-None-Match ✅ If-Unmodified-Since ...|"、ListObjectsV2 行 — "✅ list-type ✅ continuation-token ✅ delimiter ✅ encoding-type ✅ fetch-owner ✅ max-keys ✅ prefix ✅ start-after"、実装済み bucket 操作に `PutBucketLifecycleConfiguration` / `GetBucketLifecycleConfiguration`
3. https://developers.cloudflare.com/r2/api/tokens/ (Last updated Aug 18, 2026) — "Access Key ID: The `id` of the API token. Secret Access Key: The SHA-256 hash of the API token `value`." / "You must purchase R2 before you can generate an API token." / "(Optional) If you select the **Object Read and Write** or **Object Read** permissions, you can scope your token to a set of buckets."
4. 同 — "Buckets created with jurisdictions must be accessed via jurisdiction-specific endpoints: European Union (EU): `https://<ACCOUNT_ID>.eu.r2.cloudflarestorage.com` ..." / "Jurisdictional buckets can only be accessed via the corresponding jurisdictional endpoint. Most S3 clients will not let you configure multiple `endpoints`"
5. 同 — "- **Create Account API token** ... Only users with the Super Administrator role can view or create them. ... - **Create User API token** ... become inactive if your user is removed from the account."
6. https://developers.cloudflare.com/r2/pricing/ — "Storage: 10 GB-month per month / Class A Operations: 1 million requests per month / Class B Operations: 10 million requests per month / Egress (data transfer to Internet): Free"、Class A 列挙 (`ListBuckets`, `PutObject`, `UploadPart`, `ListParts`, `PutBucketLifecycleConfiguration` 等) と Class B 列挙 (`HeadObject`, `GetObject` 等) のいずれにも `DeleteObject` は不在 / "The free tier only applies to Standard storage, and does not apply to Infrequent Access storage."
7. https://developers.cloudflare.com/r2/platform/limits/ — "| Maximum concurrent writes to the same object name (key) | 1 per second [^5] |" 脚注 "Concurrent writes to the same object name (key) at a higher rate return HTTP 429 (rate limited) responses." / "| Object key length | 1,024 bytes |" / "| Object metadata size | 8,192 bytes |" / "| Maximum upload size | 5 GiB (single-part) ... |" 脚注 "The max upload size is 5 MiB less than 5 GiB, so 4.995 GiB."
8. https://developers.cloudflare.com/r2/reference/consistency/ — "| Object listing: List the objects in a bucket | Strongly consistent ... |" / "| IAM: Adding/removing R2 Storage permissions | Eventually consistent: A new or updated API key may take up to a minute to have permissions reflected globally |"
9. https://developers.cloudflare.com/r2/examples/aws/aws4fetch/ (Last updated Apr 21, 2026) — `import { AwsClient } from "aws4fetch";` / ``const R2_URL = `https://${ACCOUNT_ID}.r2.cloudflarestorage.com`;`` / `const client = new AwsClient({ accessKeyId: ACCESS_KEY_ID, secretAccessKey: SECRET_ACCESS_KEY });` / `const ListBucketsResult = await client.fetch(R2_URL);`
10. https://raw.githubusercontent.com/mhart/aws4fetch/master/src/main.js — `function guessServiceRegion(url, headers) { ... if (hostname.endsWith('.r2.cloudflarestorage.com')) { return ['s3', 'auto'] }` / `this.retries = retries != null ? retries : 10 // Up to 25.6 secs` / `if (res.status < 500 && res.status !== 429) { return res }` / `if (this.service === 's3' && !this.signQuery && !this.headers.has('X-Amz-Content-Sha256')) { this.headers.set('X-Amz-Content-Sha256', 'UNSIGNED-PAYLOAD') }`
11. https://developers.cloudflare.com/r2/examples/aws/custom-header/ — `custom_headers = {'If-Match' : '"29d911f495d1ba7cb3a4d7d15e63236a"'}` (内側の二重引用符に注意) / "# Note that boto3 will throw an exception if the precondition failed."
12. https://developers.cloudflare.com/r2/api/s3/presigned-urls/ — "R2 supports presigned URLs for the following HTTP methods: GET, HEAD, PUT, DELETE" / "POST (multipart form uploads via HTML forms) is not currently supported." / "**Expiry**: Timeout from 1 second to 7 days (604,800 seconds)" / "Presigned URLs work with the S3 API domain ... and cannot be used with custom domains."
13. https://developers.cloudflare.com/r2/api/s3/temporary-credentials/ — "They authenticate with AWS Signature Version 4, the same as a long-lived token, but include a session token and expire automatically. The session token is sent with every request via the `X-Amz-Security-Token` header"
14. live: `GET /client/v4/accounts/486f92f733bf7fcf7aa94fa4738eeeea/r2/buckets` → `agentic-inbox`, `gyosu-zukan-media`, `gyosu-zukan-staging-media`, `pcipher-db-backups`, `reservation-line-homepage-staging-storage`, `reservation-line-homepage-storage`

### 未確認・要フォロー

- Global API Key で SigV4 署名できないことを明言した公式文は見つかっていない [SECONDARY のまま]。機構的裏付けは十分だが、公式の否定文ではない。
- `POST /client/v4/user/tokens` で bucket スコープ Object Read & Write トークンを 1 コールで発行できるかは、タスクの「トークンを作るな」制約により live 未検証 (ドキュメント上は可能)。
- temporary credentials (親トークンの secret で JWT をローカル署名して短命資格を作る) は未評価。hook が毎プロンプト走る設計で長寿命の書き込み可能 secret を撒くか、15 分の prefix スコープ資格にするかは M1 の判断事項。aws4fetch は `sessionToken` を native サポート。
- multipart / lifecycle / presigned のいずれも live 実行していない (R2 API token 未作成のため SigV4 呼び出しはゼロ)。
- `~/.grok` 等と同様、この調査でも並列エージェントが共有 scratchpad で衝突した記録がある (別エージェントに作業ディレクトリを上書きされた)。今後の probe は PID 付きディレクトリを使う。

### 実測

- read-only の Cloudflare REST 呼び出しのみ 4 回 (調査 2 + 検証側の独立再現 2)。`GET /client/v4/accounts` → 単一アカウント `486f92f733bf7fcf7aa94fa4738eeeea`、`GET /accounts/{id}/r2/buckets` → success: true、既存 6 バケット (oboete 用は無し)。作成・書き込みは一切なし。この成功自体が「このアカウントでは R2 が既に購入/有効化済み」を示すので、購入ゲートはローカル再現しない。
- SigV4 の live 呼び出しはゼロ (R2 API token を作らない制約)。代わりに aws4fetch の署名を**オフラインで再現**: Node v24.16.0 + ダミー資格情報で `PUT https://ACCOUNTID.r2.cloudflarestorage.com/my-bucket/db.age` に service/region を渡さずに署名 → `Credential=FAKEKEYID/20260902/auto/s3/aws4_request`、`x-amz-content-sha256: UNSIGNED-PAYLOAD`。`https://ACCOUNTID.eu.r2.cloudflarestorage.com/...` でも `service/region: s3 auto`。
- aws4fetch のバージョン境界を npm tarball を落として grep で確定: `1.0.12` 0 件 / `1.0.14` 0 件 / `1.0.15` 3 件 / `1.0.16` 3 件 / `1.0.17` 4 件 / `1.0.20` 4 件。npm dist-tags latest = 1.0.20。
- Cloudflare の presigned example をそのまま再現すると実出力は `X-Amz-SignedHeaders=host` (ドキュメントの `content-type%3Bhost` と不一致)。`allHeaders: true` を足すとドキュメント通りになることを確認。
- ドキュメントは全て `developers.cloudflare.com/.../index.md` または `raw.githubusercontent.com/cloudflare/cloudflare-docs` の生 markdown で取得し、WebFetch の要約層を経由せず verbatim 照合した。

---

## 次に確認すべきこと

M1 spec を書く前に閉じるべき UNKNOWN を、ブロッキング度の高い順に挙げる。

1. **observer LLM は本当に要るのか (最上位のスコープ判断)**。Claude Code の `PostCompact.compact_summary` は compact が生成済みの会話要約を hook stdin で無料で渡し、`Stop.last_assistant_message` / Grok の `lastAssistantMessage` も無料。一方 Workers AI の実測は 1 コール ~45 neurons = 無料枠 ~220 コール/日。「自前で要約する」前提を維持するのか、無料で来る要約を主経路にして LLM 呼び出しを例外扱いにするのかを先に決めないと、M1 の主要コンポーネントが変わる。
2. **Grok レーンの注入をどうするか**。Grok には turn 開始前の注入経路が存在しない (`SessionStart`/`UserPromptSubmit` は破棄、`PreToolUse` は tool 実行後・deny で消滅、`Stop` は turn を継続させてしまう)。(a) 「最初の tool 結果と一緒に届く」で妥協する、(b) AGENTS.md / project rules など非 hook 面を使う、(c) Grok を M1 スコープから外す、のいずれか。
3. **SQLite 書き込みの直列化点**。Claude Code の async hook は firing ごとに別プロセスで dedup 無し、Codex/Pi も同様に短命プロセスから来る。claude-mem は SessionStart で常駐 worker を起こして集約しているが、oboete の clean-slate 決定は「daemon 無し・SQLite 1 ファイル」。この矛盾の解き方 (WAL + busy_timeout で押し切るか、単一 writer プロセスを認めるか) が未決。
4. **各エージェントの `tool_response` / tool payload の実スキーマ**。Claude Code は "depends on the tool" としか書かれておらず、Codex は `custom_tool_call.input` が JS プログラム、Grok は `toolName` がネイティブ名。イベント正規化層の形が決まらないので、Read/Edit/Write/Bash について 4 エージェント分の実キャプチャが要る。
5. **Codex の rollout flush タイミング**。`PostToolUse` hook の中で `transcript_path` を読んだとき直前の tool 呼び出しが既に永続化されているかは、ソース上保証が見つからなかった。安価な probe (hook が自分の `tool_use_id` を transcript から grep) で確定できる。
6. **Codex の hook trust を installer が非対話で完結できるか**。`trusted_hash` の算出は再現済みだが、確証は `codex exec` + fresh CODEX_HOME の 1 データポイントのみ。対話 TUI 経路と `hooks.json` 経路 (ユーザーの実 config には該当 trust 行が存在する) は未検証。
7. **R2 のオンボーディング最短経路**。「R2 購入必須」→「token 発行」→「IAM 反映まで最大 1 分」の 3 段があり、さらに jurisdiction バケットの可能性がある。`POST /client/v4/user/tokens` で bucket スコープ token を 1 コール発行できるかは未 live 検証 (今回はトークン作成禁止だった)。長寿命 secret か temporary credentials かの判断もここに紐づく。
8. **Pi の `pi -p --no-session` 固着**。再現するが原因未特定。summarizer worker を `pi -p` で回す設計なら先に切り分ける。
9. **transcript / session ファイル形式の安定性**。Claude Code の `.jsonl` レコードスキーマは公式に文書化されておらず (hooks ページに "transcript format" 0 件)、Grok の `transcriptPath` は hooks doc に記載が無く、Grok の docs は session 保存形式を SQLite と誤記している。どこまで依存してよいかの線引きが未決。幸い Claude Code / Grok とも Stop 系で最終応答テキストを直接もらえるので、transcript 依存を捨てられる可能性は高い。
10. **有料化・非推奨の監視方法**。Workers AI の changelog は 2026-06-16 で更新が止まっており監視ソースにならない。`require_workers_paid` プロパティの読み取り、`5035`/403 の捕捉、レスポンス body の `"model"` と要求 id の突き合わせ (kimi-k2.5 → k2.6 の無言 alias 前例あり) のどれを採るか。加えて「2026-07-28 に有料化」という設計ナラティブ上の日付は一次ソースで確認できず、記述から落とすべき。
