# M1 amendments A1-A16: decision record (2026-09-03)

**Decision authority**: the owner delegated every technical decision on 2026-09-03 with one
criterion: the finished oboete must be better than claude-mem in usability, stability, and
features, never "why not just use claude-mem?". Decisions below were made by Claude Code under
that delegation after an adversarial multi-agent comparison against claude-mem (workflow
`oboete-m1-vs-claude-mem`, run `wf_c9ccd47d-791`, 2026-09-03: 5 amendment evaluators, 3 attack
lenses, 3 refuters per finding, 18 of 29 findings survived, opus synthesis). The synthesis report
is appended verbatim below.

## Decisions

| id | decision | applied where |
|---|---|---|
| A1 | approved | CONSTITUTION Principle II wording |
| A2 | approved with change: session-start wait is **1 s**, not 8 s | CONSTITUTION Principle IV; spec FR-024, User Story 1 scenario 3, Assumptions; plan; contracts/agents.md |
| A3 | approved | CONSTITUTION Principle VI allow-list |
| A4 | approved as a MINOR amendment (3.0.0 → 3.1.0) | CONSTITUTION Principle VI and Product Constraints; spec FR-039 and Assumptions |
| A5 | approved | CONSTITUTION Product Constraints |
| A6 | approved | CONSTITUTION Development Workflow |
| A7 | approved with change: payloads above the 1 MB read bound keep a **redacted, detector-processed prefix** marked truncated (`classification_state = partial`); partial rows contribute only metadata to the rule-based summarizer, never text to any provider, and are never injected | spec edge case; research R4; data-model; contracts/agents.md |
| A8 | approved (conditional on the Pi error-surface probe); **triggered 2026-09-03** (R13: extension throws reach stderr only) | spec FR-007 (applied 2026-09-03) |
| A9 | approved | CONSTITUTION Principle II (loopback foreground viewer) |
| A10 | approved | CONSTITUTION Product Constraints (Pi child-process capture) |
| A11 | approved | spec User Story 2 scenario 2 wording |
| A12 | approved | spec FR-024, FR-026, User Story 1 scenario 2, SC-010 (context epoch) |
| A13 | approved with change: content identity is (normalized title, normalized body) **without type** | spec FR-035; contracts/observer.md; data-model; research |
| A14 | approved (conditional): same treatment as A7 (prefix kept); a measured bound under 200 KB is escalated to the owner; not yet evaluated (detector probe runs after T025) | plan amendment table |
| A15 | approved (conditional): default = accept counted per-call duplicates inside one parallel batch; never exclude delivery; **triggered 2026-09-03** (R13: Grok delivers once per call) | plan amendment table |
| A16 | approved (conditional): default = `PostCompact` event id as the epoch key with the documented ordering limit; **triggered 2026-09-03 for Claude Code and Codex** (no per-compaction id; Claude Code also runs `SessionStart source = compact` before `PostCompact`); Grok Build and Pi pass | plan amendment table; contracts/agents.md |
| A17 | owner decision 2026-09-03 (isolated-user setup): remote-preset credentials are one variable per preset, `OBOETE_<PRESET>_API_KEY` (`OBOETE_NIM_API_KEY`, `OBOETE_OPENROUTER_API_KEY`, `OBOETE_GEMINI_API_KEY`, `OBOETE_ANTHROPIC_API_KEY`), replacing the single `OBOETE_PROVIDER_API_KEY`; lets the R13 provider probe hold every preset's key at once and keeps a key bound to its host | contracts/cli.md, contracts/observer.md, docs/research/isolated-user-setup.md |

## R13 outcome (T011, 2026-09-03)

Evidence: `docs/research/oboete-contracts-probes.md` (section "R13 evaluation"). Decisions taken by
Claude Code under the 2026-09-03 delegation (claude-mem yardstick); the owner may override any of them.

| id | trigger observed | decision |
|---|---|---|
| A8 | Pi: an extension throw is printed to stderr only; the session JSONL has no error record; the session continues | default applied to spec FR-007: in-memory counters handed to the next child spawn plus the `oboete doctor` wiring probe |
| A15 | Grok Build: `additionalContext` attached to two calls of one parallel batch reaches the model once per call (twice in the transcript) | default applied: per-call duplicates inside one batch are accepted and counted in `why` and SC-010; delivery is never excluded |
| A16 | Claude Code: `PostCompact` carries no per-compaction id (only `compact_summary`), and `SessionStart source = compact` fires about 24 ms before `PostCompact`. Codex: `PostCompact` carries `turn_id` and `trigger` only; ordering is fine. Grok Build (`timestamp`) and Pi (`compactionEntry.id`) pass both conditions | default applied to Claude Code and Codex: the epoch key is the `PostCompact` event id (byte-identical same-turn compactions collapse). Documented ordering limit on Claude Code: the `SessionStart source = compact` hook opens the new epoch itself (it carries `source = compact`) and `PostCompact` only confirms it |
| A14 | not evaluated: the detector does not exist yet (after T025); hook runners cap delivered tool results at about 31 KB (Claude Code), 5 KB (Codex), 165-190 KB (Grok Build), so the 1 MB path is reachable only through prompts and transcripts | pending |
| A19 (new, owner) | owner statement 2026-09-04: "Anthropic API は使いません" | the `anthropic` observer preset is removed from M1 and from the constitution's preset list (3.1.0 → 3.2.0); `OBOETE_ANTHROPIC_API_KEY` dropped from contracts/cli.md; provider probe removed; Complexity Tracking row 15 closed. No R13 row stays blocked on credentials |
| A20 (new) | 2026-09-04 の dogfood で `session_summary.request` が 200 文字で切れ、observer も exact string を言い換えたため SC-001 / SC-004 が停止した | developer の exact string を決定論的な row に残すため `request` 上限を 1,000 文字へ拡大し（`next_steps` は 200 文字のまま）、observer system prompt に identifiers、tokens、codes、file names、error text と exact 指定文字列の翻訳・言い換え禁止を追加する。body のトリム順は「3 つの list 行を各 5 件まで削る → `request` を 200 文字まで戻す → なお超過する場合のみ list をさらに空にする」とし、長い prompt でも learned（適用された observation の title）が body に残る |
| A14 outcome (2026-09-04, T025 measurement) | the full detector (secretlint recommend rules + gated entropy, single-pass replacement) finishes a clean 1 MB payload in about 11 ms but a secret-dense 1 MB payload in 406-665 ms on Node 22/24, above the 240 ms hook cutoff (300 ms deadline minus the 40 ms spool reserve and 20 ms row margin) | A14 default applied: the hook read bound becomes 256 KiB (`STDIN_READ_BOUND` in src/capture.ts) with the A7 partial-row treatment for the rest; 256 KiB keeps the secret-dense worst case near 100-170 ms and stays above the 200 KB escalation floor, and the agents' own runners already cap tool results at 5-190 KB (R13 "unread stdin" row), so only oversized tool inputs are affected. Spec text "1 MB" in contracts/agents.md, cli.md, data-model.md and research R4 reads as this bound |
| A18 (new) | Codex: `/new` in the TUI ends the session (`SessionEnd`) but fires no `SessionStart` with `source = clear`; `startup`, `resume`, `compact` are verified | FR-024 on Codex: a cleared session is detected by the session id changing on the next hook (`UserPromptSubmit`); the session-start pack is injected there through `additionalContext`, one prompt later than on the other agents. Not blocked: the same pack reaches the model on the first prompt of the new session |
| A21 (new) | Codex (codex-cli 0.153.0) fires its `SessionStart` hooks lazily, at the start of the next turn: after a TUI `/compact`, `PostCompact` fires at once and `SessionStart source = compact` about 200 ms before the next turn's `UserPromptSubmit` (isolated-user hook log 2026-09-05: 04:01:27.744Z, 04:01:30.180Z, 04:01:30.375Z; in run 2026-09-05T06-02-58-033Z no SessionStart arrived in 180 s because no prompt followed the /compact); after `/new` the parent gets no `SessionEnd` (0 in both runs, it arrives at `/quit`) and the new session's `SessionStart source = startup` fires about 200 ms before its first `UserPromptSubmit` (06:08:25.536Z, 06:08:25.734Z, pack delivered on `codex:SessionStart`, recall answered from memory). The A18 observation of Phase 1 (probe D quit right after `/new`) and the lifecycle run 2026-09-05T03-30-44-818Z (/quit 0.6 s after PostCompact) were this lazy firing seen through a session that ended before its next turn | FR-024 on Codex keeps `SessionStart(compact)` and the new session's `SessionStart(startup)` as the delivery points; as a fallback the epoch's session-start pack is delivered on the first `UserPromptSubmit` whose conversation has no session-start injection for its current `context_epoch` — the A18 channel generalised from "a session was just created" to "this epoch has no pack" — so a spooled SessionStart or a CLI build that skips the lazy hook still gets the pack, one prompt late (verified 2026-09-05: the epoch-1 pack of the raced run arrived on `codex:UserPromptSubmit`). An omitted pack counts as delivered for that epoch, so a repository with nothing to inject is not retried on every prompt. `hasSessionStart` (raw_events based, Phase 2 follow-up 14) is deleted with it. Harness rule: a Codex hook that belongs to the next turn is observed by sending that turn, never by waiting |

Blocked rows after T011: none on credentials (NIM, OpenRouter, Gemini and Workers AI passed on
2026-09-04 once `~/.oboete-credentials` held their keys; the Anthropic preset was removed by the owner,
A19); detector 1 MB
(after T025); bundle cold start and installed size (after T012); legacy MCP server against Claude Code
(needs the T077 server).

## Additional M1 changes adopted from the comparison

1. Session-start wait 1 s (A2).
2. Redacted prefix retained above 1 MB (A7, A14).
3. Type removed from content identity (A13).
4. `oboete setup` never dead-ends on missing credentials: it prints the Cloudflare free-account and Workers AI token steps with URLs and offers the local `ollama` preset, the new `agent-cli` preset, or continuing without a provider (rule-based).
5. Packs carry plain-language degraded lines; reason codes stay in the ledger, doctor, and `why`.
6. New optional observer preset `agent-cli`: the worker runs the already-authenticated `claude -p`, `codex exec`, or `grok -p` for summarization (no new credentials, no daily cap of oboete's own; consumes the developer's subscription, shown on the consent screen). Not the default. Headless JSON output per CLI is an R13 probe; failure only disables the preset.
7. Lexical-only search is disclosed in empty `search` results, in `doctor`, and in the README; semantic search stays M2.
8. `oboete view --open` opens the browser; `setup` and `doctor` print the launch line.
9. Near the daily cap (10 or fewer calls left) ten-turn batches go to the fallback and the remaining calls are reserved for session-end batches, so the summary that is injected first degrades last.

## Known gaps versus claude-mem after M1 (deliberate, disclosed)

- No semantic (vector) search until M2.
- No resident viewer; `oboete view` runs in the foreground.
- No cross-repository search (owner decision in Clarifications).
- No folder context files, knowledge agents, in-viewer settings, or device sync (M2 or later).

---

# oboete と claude-mem の最終判定（M1 計画に対する持ち主向け報告）

## 結論

計画どおりに M1 を作り終えた時点の oboete は、**プライバシー、注入の説明可能性、障害時の正直さ、書き出しと持ち運び、そして Pi と Grok Build CLI という二つのエージェントへの対応**では claude-mem より明確に優れており、**費用の考え方と対応エージェントの広さ**ではおおむね同等ですが、**検索の質（意味検索がなく語の一致だけである点）、無料枠で作られる記憶の質と鮮度、毎ターンの体感速度、初回セットアップの手間**の四点では claude-mem に劣ります。とくに検索については、claude-mem が無料の本体機能としてすでに意味検索（Chroma によるベクトル検索と全文検索の併用）を積んでいることが公式資料で確認できており（https://docs.claude-mem.ai/architecture/search-architecture ）、これは M1 の設計上どうしても埋まりません。したがって現状の計画のままでは、言い換えた言葉で検索した利用者が「それなら claude-mem でよいのでは」と言う余地が残ります。ただし、この報告の第四節に挙げた六件（八秒待ちの短縮、巨大出力の丸ごと破棄の廃止、削除記憶の復活穴の封じ、セットアップの資格情報行き止まりの解消、注入文の内部用語の平文化、既にログイン済みのエージェントを要約に使う無料プリセットの追加）を実装開始前に計画へ反映すれば、劣る軸は「意味検索が M2 まで無い」ことだけに絞り込め、それ以外はすべて同等以上になります。この一点は正直に既知の穴として表示する方針を取るべきであり、隠すべきではありません。

---

## claude-mem との比較表

| 軸 | claude-mem（無料 / Pro） | oboete M1（計画どおりの場合） | 判定 |
|---|---|---|---|
| 対応エージェント数 | 導入画面で Claude Code、Cursor、Windsurf、OpenCode、Codex CLI、Antigravity CLI、Grok Bot を列挙。Codex は本物のフック取得に対応済み（リポジトリに `codex-hooks.json` を生成する仕組みが実在）。一方で Pi の対応は提案が取り下げられ未搭載（PR は未マージのまま閉鎖）、Grok は「Grok Bot にはホスト側フックが無い」と明記。https://docs.claude-mem.ai/installation ／ https://github.com/thedotmack/claude-mem/pull/2532 | Claude Code、Codex CLI、Grok Build、Pi の四種すべてで取得と注入の両方を行う（Grok Build のみ最初の道具呼び出しに合わせた遅延配送） | 同等（数の広さでは劣り、Pi と Grok Build CLI では優る） |
| セットアップ | `npx claude-mem install` の一行。`--provider claude` を選べば既存の Claude 契約をそのまま使い、新しい登録も鍵も不要。`--provider host` は鍵なしで既にログイン済みのエージェントを使う。https://docs.claude-mem.ai/installation | `oboete setup` は四エージェントを自動検出し、フックが本当に発火するか実際に試して確認するところは優れているが、既定の要約先を使うには利用者が自分で Cloudflare の無料アカウントを作り、二つの資格情報を環境変数に入れる必要がある | 劣（第四節の修正四で解消可能） |
| 毎ターンの遅延 | 道具呼び出しごとの記録は平均 8 ミリ秒、上位 1 パーセントでも 30 ミリ秒。セッション開始時の文脈注入は平均 45 ミリ秒、上位 1 パーセントで 250 ミリ秒。常駐サービスが温まっているため。https://docs.claude-mem.ai/hooks-architecture | 取得は 300 ミリ秒以内という予算（常駐しない設計のため毎回プロセスを起動する）。セッション開始の注入は要約が出来ていれば 300 ミリ秒、まだのときは最大 8 秒待つ | 劣（8 秒待ちは第四節の修正一で 1 秒に短縮すべき） |
| 記憶の質 | 道具呼び出しのたびに記録し、その都度まとめる。無料経路では利用者自身の Claude 契約のモデル（高品質）を使え、claude-mem 独自の一日あたり呼び出し上限は設けていない。https://docs.claude-mem.ai/installation ／ https://docs.claude-mem.ai/hooks-architecture | 十ターンごとと終了時にまとめて一回だけ呼び出し、無料枠の小さなモデルを使い、一日 150 回で打ち切って規則ベースに落ちる | 劣（第四節の修正六で同等以上にできる） |
| 検索の質 | ベクトル検索（Chroma）と全文検索を組み合わせた意味検索が無料の本体機能。言い換えた質問でも当たる。https://docs.claude-mem.ai/architecture/search-architecture | 語の一致のみ（日本語は二文字単位で確実に引ける工夫あり）。意味検索は M2 送り | 劣（M1 では埋まらない。正直に表示する） |
| 注入の挙動 | 利用者に見えない形で静かに差し込む。何が入って何が落ちたかの台帳は公開資料に見当たらない。https://docs.claude-mem.ai/hooks-architecture | 差し込んだ文章に印を付け、入れたもの・落としたもの・その理由・劣化状態をすべて記録し、`oboete why` で後から確認できる。同じ記憶を同じ会話で二度入れない | 優 |
| プライバシー | 既定の保護は利用者が手で `<private>` と書いた範囲だけで、鍵や合言葉の自動検出は行わない（自動伏せ字の提案は未マージのまま、しかも既定では無効）。Pro は観測記録をすべて雲の上へ同期する。https://docs.claude-mem.ai/usage/private-tags ／ https://github.com/thedotmack/claude-mem/pull/2616 ／ https://cmem.ai/pricing | 保存する前に必ず検出器を通し、判定できなければ安全側に倒す。既定は端末内のみで、外へ出せるのは検査を通った行だけ。外部へ送る前に宛先と費用区分を表示して同意を取る | 優 |
| 費用と上限 | 無料経路は追加費用ゼロだが利用者自身の契約枠を消費する。Pro は月額 30 ドルで、要約を自分の契約外で回す機能と機器間同期が付く。https://cmem.ai/pricing | 契約不要で恒久的に無料。ただし無料枠ゆえに一日 150 回の上限と小さなモデルという制約が付く | 同等（修正六を入れれば優） |
| ビューア | 常駐サービスが常に画面を提供するので、いつでもブラウザを開けば見られる。設定画面もブラウザ内にある。公開資料に認証の記載は無く、固定の番号の口を待ち受け続ける。https://docs.claude-mem.ai/configuration | `oboete view` を実行している間だけ開き、起動ごとの合言葉が要る。設定変更はコマンド側 | 同等（手軽さで劣り、安全性で優る） |
| 障害時の挙動 | 失敗しても本体を止めない設計。ただし自動復旧は「記憶が二重に作られる不具合」のために無効化されており、取りこぼしの再処理は利用者が手で命令を打つ必要がある。https://docs.claude-mem.ai/usage/manual-recovery | 書けなければ一時退避して後から重複なく取り込み、作業役は一台に一つと機械的に保証し、`oboete doctor` が壊れている箇所ごとに「今何が失われているか」と「直す手順」を示す | 優（設計上。実装後の検証で確かめる必要あり） |
| 移行・書き出し | 書き出しと取り込みは補助スクリプトとして提供。文書には「書き出したファイルは平文なので、共有前に鍵などが混じっていないか自分で確認せよ」と注意書きがある。https://docs.claude-mem.ai/usage/export-import | `oboete export` と `oboete import` が正式な機能で、機密度・出どころ・リポジトリの識別子を保ったまま移せる。取り込み時に機密度が緩む方向へは決して動かず、削除の記録が常に優先される | 優 |

---

## A1 から A16 の決定

承認の基準は一つだけです。すなわち「その決定によって oboete が claude-mem より悪くなるか」で判断し、悪くなるものは条件付き承認として代わりの既定値をここで決めておきました。

| 記号 | 決定 | 利用者への影響（一行） | 条件付きの場合に事前に決めておく既定値 |
|---|---|---|---|
| A1 | 承認 | 利用者から見える変化はなく、むしろ全部を一つの塊に固めるとフックの起動が重くなるため、この文言修正が体感速度を守ります（claude-mem 自身も用途別に分けて出力しています。https://github.com/thedotmack/claude-mem ）。 | — |
| A2 | 条件付き承認 | このままだと直前のセッションを終えた直後に別のエージェントを開いた場合、画面が最大 8 秒固まったように見えます。claude-mem は待たずに一瞬で開始します（https://docs.claude-mem.ai/hooks-architecture ）。 | 取得側の 300 ミリ秒はそのまま承認し、**セッション開始時の待ち時間の上限は 8 秒ではなく 1 秒**とします。1 秒で要約が揃わなければ、既に決まっているとおり直近の生の活動記録に「要約作成中」の印を付けて即座に渡します。要約には外部への通信が挟まるため、8 秒待っても間に合わないことが多く、待ち時間はほぼ苦痛だけを増やします。 |
| A3 | 承認 | 検出処理が速くなることで「保存前に必ず鍵を伏せる」という oboete の既定の保護が成立します。claude-mem には自動検出そのものがありません（https://docs.claude-mem.ai/usage/private-tags ）。 | — |
| A4 | 承認 | 保存場所は `~/.oboete/` の一箇所で、環境変数一つで移動できます。claude-mem も同じ方式です（https://docs.claude-mem.ai/installation ）。 | — |
| A5 | 承認 | 利用者の既存設定ファイルを丸ごと書き換えずに済むため安全です。ただし提出された評価にあった「claude-mem は Codex のフックを見つけていない」という説明は**誤り**で、claude-mem は既に Codex 用のフック取得を搭載しています（https://github.com/thedotmack/claude-mem/issues/3651 ）。優位ではなく同点だと理解してください。 | — |
| A6 | 承認 | 内部の検証手順の一語修正で、利用者には見えません。 | — |
| A7 | 条件付き承認 | 現案のままだと巨大な道具出力（大きな試験記録や広い検索結果など）について「何かを実行した」という記録しか残らず、中身が永久に失われます。claude-mem はまったく同じ設計を一度入れて撤回しました（https://github.com/thedotmack/claude-mem/issues/2217 ）。 | 1 メガバイトという読み取り上限は承認します。ただし**丸ごと捨てるのをやめ、検出器を通し終えた（伏せ字済みの）先頭部分を「途中で切った」という印を付けて保存**します。全体を解析できなくても、既に設計にある「必要な識別情報だけを走査して拾う」やり方で足ります。 |
| A8 | 承認 | Pi の取得が壊れても Pi 自体は止まらず、`oboete doctor` が「Pi の取得が失敗しています」と理由と直し方を出します。claude-mem には Pi 対応そのものがありません（https://github.com/thedotmack/claude-mem/pull/2532 ）。 | — |
| A9 | 承認 | ビューアを見ている間だけ口が開き、閉じれば誰も届きません。claude-mem は常時開いたままで、公開資料に認証の記載がありません（https://docs.claude-mem.ai/configuration ）。 | — |
| A10 | 承認 | 保存処理が詰まっても Pi の返答が凍りません。 | — |
| A11 | 承認 | ごく稀に無料枠の呼び出しが一回余分に消えることはありますが、同じ記憶が二件できることはありません。claude-mem は逆に、記憶が二重に作られる問題のために自動復旧を止めています（https://docs.claude-mem.ai/usage/manual-recovery ）。 | — |
| A12 | 承認 | 長いセッションが自動圧縮された後にもう一度記憶が渡されます。同じ文が二度見えても仕様どおりで、不具合ではありません。 | — |
| A13 | 条件付き承認 | 現案のままだと、消したはずの記憶が「種類のラベルだけ違う」形で復活し得ます。claude-mem は識別子が狭すぎて重複を取り逃す問題を繰り返しています（https://github.com/thedotmack/claude-mem/issues/3163 ）。 | **同一性の判定から「種類」を外し、正規化した題名と本文だけで決めます**（計画書が併記していた持ち主向けの代替案を採用します）。利用者にとっての「同じ内容」は文章そのものであり、内部のラベルではありません。 |
| A14 | 条件付き承認 | 検出器の速度検証が失敗した場合に発動する条件付きの項目です。現案のままだと、ごく普通の大きさの出力まで中身がゼロになりかねません。 | **A7 と同じ扱いに統一**します。すなわち測定で決まった上限を超えた分も、検出器を通し終えた先頭部分は印を付けて保存します。もし測定された上限が極端に小さい場合（目安として 200 キロバイト未満）は、単なる数値調整では済まない兆候ですので、実装を進める前にもう一度こちらへ判断を求めます。 |
| A15 | 条件付き承認 | Grok Build の検証結果しだいで発動します。除外を選ぶと「道具を同時に複数使ったターンだけ、理由もわからず記憶が届かない」という説明のつかない欠落になります。 | **重複を許容する案を既定**とします。同じ記憶がそのターン内で二度届くことを認め、その重複は台帳と成功基準の側で明示的に数えます。配送の除外は選びません。 |
| A16 | 条件付き承認 | 圧縮の検証結果しだいで発動します。除外を選ぶと、そのエージェントでは一度圧縮が起きた後は二度と記憶が入らなくなり、使うほど「何も覚えていない」状態になります。 | **記録した出来事の識別子をそのまま区切りの鍵として使う案を既定**とします。失うのは「まったく同じ圧縮が同じターンで二回起きたとき、それを一回として数える」という実害のない精度だけです。 |

---

## M1 の範囲で直すべき点

### 実装を始める前に、計画と仕様の文面を直しておくもの

1. **セッション開始時の待ち時間を 8 秒から 1 秒へ短縮する。**
   変更箇所は、`spec.md` の利用者物語 1 の受け入れ場面 3 と機能要件 FR-024、`plan.md` の性能目標および複雑さ管理表の第 2 項と修正表の A2 行、`contracts/agents.md` の「Hook process rules and SLAs」節と Pi の行、`CONSTITUTION.md` の原則 IV です。一行の変更は「最大 8 秒待つ」を「最大 1 秒待つ」に置き換えることです。理由は、要約には外部通信が挟まるため 8 秒待っても間に合わないことが多く、待ち時間が体感速度の劣化にしかならないためです（claude-mem は待ちません。https://docs.claude-mem.ai/hooks-architecture ）。

2. **1 メガバイトを超える出力を丸ごと捨てるのをやめ、先頭部分を印付きで残す。**
   変更箇所は、`spec.md` の「Edge Cases」節の該当項目と機能要件 FR-002、`plan.md` の複雑さ管理表の第 16 項と修正表の A7 および A14 行です。一行の変更は「内容を落として記録だけ残す」を「検出器を通し終えた先頭部分を『途中で切った』印付きで保存する」に置き換えることです。claude-mem はまったく同じ設計を「永久にデータを失う税金のようなもの」として撤回済みです（https://github.com/thedotmack/claude-mem/issues/2217 ）。

3. **削除した記憶が種類ラベルの違いだけで復活する穴を塞ぐ。**
   変更箇所は、`spec.md` の機能要件 FR-035、`contracts/observer.md` の「Worker rules」節にある同一性の定義、`research.md` の同一性に関する記述です。一行の変更は、同一性の計算から「種類」を外し、正規化した題名と本文だけで決めることです。

4. **セットアップが資格情報の行き止まりにならないようにする。**
   変更箇所は、`contracts/cli.md` の `oboete setup` の行と `quickstart.md` です。一行の変更は、既定の要約先の資格情報が見つからないときに、（一）Cloudflare の無料アカウント作成と権限付き符号の発行手順を短い案内と URL 付きで表示し、（二）その場で「端末内のモデルを使う」または「資格情報なしで続ける（規則ベース）」を選べるようにすることです。claude-mem は新しい登録をまったく必要としない経路を持っているため、ここを放置すると初回の印象で負けます（https://docs.claude-mem.ai/installation ）。

5. **差し込む文章に内部用語をそのまま出さない。**
   変更箇所は、`contracts/agents.md` の「Pack format」節にある劣化理由の行と、`contracts/observer.md` の劣化理由の優先順位表です。一行の変更は、内部の符号（`daily_cap` などの語）はそのまま台帳と診断用に残しつつ、差し込む文章の側だけは「本日の無料要約枠を使い切ったため、簡易版の記録を表示しています」のような普通の言葉に置き換える対応表を持つことです。

6. **既にログイン済みのエージェントを要約に使う、資格情報不要のプリセットを一つ足す。**
   変更箇所は、`contracts/observer.md` の「Provider presets」表と `spec.md` の機能要件 FR-012 のプリセット一覧です。一行の変更は、作業役から `claude -p` や `codex exec` や `grok -p` を呼び出して要約させる選択肢を一件加えることです（既定は変えません）。claude-mem は同じものを `--provider host`（鍵不要）として持っており、これが無料枠の品質差とセットアップの手間の両方を一度に埋めます（https://docs.claude-mem.ai/installation ）。なお、この経路を使うと利用者自身の契約枠を消費しますので、外部送信の同意画面には宛先としてそのエージェントの提供元を表示する必要があります。

7. **検索が語の一致だけであることを正直に表示する。**
   変更箇所は、`spec.md` の前提条件節、`quickstart.md`、`contracts/cli.md` の `oboete search` と `oboete doctor` の行です。一行の変更は、検索結果が空のときと診断の出力に「現在は語の一致で検索しています。意味の近さによる検索は次の段階で対応します」と明記することです。これは機能の穴を塞ぐものではなく、隠さないための措置です。

8. **ビューアを開く手間を一手減らす。**
   変更箇所は `contracts/cli.md` の `oboete view` の行です。一行の変更は、ブラウザも同時に開く `--open` を用意し、`setup` と `doctor` の出力に起動用の一行を必ず表示することです。

### M2 以降に送るもの（理由付き）

- **意味による検索（ベクトル検索）**。これが claude-mem との最大の差ですが、埋め込みの仕組みは「一つのファイル、常駐しない」という M1 の土台そのものを揺らします。計画は将来の合流点だけを予約済みで、後から作り直さずに追加できる形になっていますので、無理に前倒しせず M2 で入れるのが正しい判断です。
- **リポジトリをまたいだ検索**。持ち主自身が「M1 では同一リポジトリのみ」と既に決めた事項であり、記憶側には識別子が残るため後から広げられます。
- **常駐する画面表示と常時更新**。常駐しないという原則そのものに触れるため、原則を見直す段階まで送ります。
- **機器間の同期**。M2 の項目として既に明示されています。無料版の claude-mem も同期は持たないため、この時点では引き分けです（https://cmem.ai/pricing ）。
- **ブラウザ内での設定変更、フォルダごとの説明ファイルへの書き出し、記憶をまとめて質問できる上位機能**。いずれも claude-mem にはありますが（https://docs.claude-mem.ai/usage/folder-context ／ https://docs.claude-mem.ai/usage/knowledge-agents ）、自分ひとりで使う最初の版には不要で、M1 の作業量を押し上げるだけです。存在しない機能として計画の前提条件節に一行だけ明記してください。
- **記憶の年表を特定の一件から前後にたどる機能と、読み込み費用の目安表示**。claude-mem の方が細かいのですが、記憶が数千件程度の M1 では現在の機能で足ります。

---

## 実装前に持ち主がやること

以下は、いずれも持ち主本人でなければできない作業です。

1. **この報告の第三節（A1 から A16 の決定）を、そのまま承認するか修正して、書面として計画書の作業ゼロに貼り付けてください。** 実装はこの承認が記録されるまで始まりません。とくに条件付き承認の五件（A2、A7、A13、A14、A15、A16）については、この報告に書いた既定値を採用するかどうかを一言で示してください。

2. **検証専用の別ユーザーを一つ作り、そこで四つのエージェントすべてにログインしてください。** 手順は、まず作業機に新しい利用者を追加し（`sudo adduser oboete-e2e` のような形になります）、その利用者に切り替えたうえで Claude Code、Codex CLI、Grok Build、Pi の四つを導入し、それぞれのログイン画面で認証を済ませます。ログインは代理では行えません。この隔離環境は「持ち主本人の普段の作業環境を壊さない」ために必須と決められています。

3. **既定の要約先を使う場合、Cloudflare の無料アカウントを作り、二つの値を控えてください。** 具体的には、Cloudflare の管理画面で無料登録を行い、Workers AI を使える権限付き符号（API トークン）を一つ発行し、あわせてアカウント識別子を控えます。その二つを、先ほどの検証用ユーザーの環境変数（`OBOETE_CF_API_TOKEN` と `OBOETE_CF_ACCOUNT_ID`）に設定します。なお第四節の修正六を採用する場合、この作業は必須ではなくなり、「既にログイン済みのエージェントに要約させる」選択でも開始できます。

4. **有料の要約先（NIM、OpenRouter、Gemini、Anthropic）を M1 の検証対象に含めるかどうかを決めてください。** 含める場合はそれぞれの利用鍵を用意する必要があります。含めない場合は「M1 ではこの四つを検証しない」という例外を一行で書面にしてください。計画上、この四つは検証が通らない限り M1 の完了自体が止まる決まりになっているため、持ち主の明示的な判断が要ります。

5. **端末内のモデル（Ollama）を選択肢として試すかどうかを決め、試す場合は作業機に導入してください。** 導入しない場合、その項目は検証から外れます。

6. **外部への送信について同意の判断をしてください。** oboete は初回の設定時に「どこへ、どの資格情報で、どの費用区分で、どの機密度の内容が送られるか」を画面に表示し、持ち主の確認を求めます。これは自動では通せない設計になっています。あわせて、第四節の修正六を採用する場合は「要約のために自分の Claude や Codex の契約枠を消費してよいか」も決めてください。

7. **七日間続けて検証を回す期間を確保してください。** M1 を完了と宣言する条件に「四エージェントすべてで七日連続して問題が出ないこと」が含まれています。実装が終わってからさらに一週間かかる、という見込みを日程に入れておいてください。

なお、この判定はまだ実行されていない検証項目（計画書の R13 に並ぶ約二十件の実測）の結果に依存する部分があります。とくに Grok Build の並列呼び出しの挙動、Pi の再開時の継続性、Codex の圧縮時の挙動、各モデルの文脈上限の四項目は、結果しだいで対応範囲が狭まります。そのため、これらの実測が終わった時点で、この比較をもう一度短く見直すことをお勧めします。
