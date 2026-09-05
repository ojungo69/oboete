# T022 — Capability Golden Matrix（version-pinned）

生成物: `claude.json`（fixtures 5 件）/ `codex.json`（fixtures 3 件）。いずれも `harness/assemble.ts` が
`harness/fixtures/<cli>/*.json` から組み立てたもので、fixture 間で `nativeVersion` が一致しない場合は
組立自体が失敗する（version-pin の強制）。

| pin | 値 |
|---|---|
| Claude Code | `2.1.228 (Claude Code)` |
| Codex | `codex-cli 0.147.0` |
| OS | Linux WSL2 6.18.33.2-microsoft-standard-WSL2 |
| 取得日 | 2026-08-12 |
| evidenceKind | 全 cell `source-test`（下記「証跡種別」参照） |

## 証跡種別と、digest が裏付ける範囲

各 cell の `evidenceKind` は、その値がどこまで機械で確かめられたかを表す。

- `real-cli-e2e` — fixture が名指しした観測記録を再計算して digest が一致し、その記録に rig が
  書いた run manifest（CLI 版・隔離・内部実行 marker・記録失敗数）が付いていて、さらに cell の
  値そのものを記録から導けた場合だけ付く。manifest の終了コードは schema が存在と 0–255 の範囲
  だけを見る（`evidence-manifest.schema.json`）。値そのものは照合表の対象外で、意図的な中断を
  記録した run も昇格しうる
- `source-test` — 上のどれかが欠けている場合。現在の全 cell がこれで、理由は 2 つある

1. **legacy 証拠**: `harness/fixtures/<cli>/raw/*.jsonl` の 16 件は manifest 制度より前に取った
   ものなので、記録は digest で結び付いているが run の素性を裏付ける manifest が無い。
   cell の limitations に `unverified: no manifest-backed evidence` が入る
2. **導けない主張**: hook の stdout が実際にモデルへ届いたか、といった主張は hook 記録の側に
   現れないため、記録から導けない。`sessionStartInjection` / `promptAwareInjection` /
   `promptDeliveryBeforeModel` がこれに当たる。したがって `resumeDeliveryStrategy` は、この
   証拠経路では `manual_only` から動かない

digest が裏付けるのは「その記録である」「事象の並びがそうである」「識別子の相関がそうである」
までで、モデルが書いた自由文の中身は正規化で伏せるため裏付けの対象にならない。

### 記録の取得側に残っている限界

`real-cli-e2e` が言えるのは「隔離 rig の下で run が起き、その記録が申告どおりである」ところまでで、
**「測定対象の CLI が記録を捏造していない」ことは言えない**。hook は測定対象 CLI の子として動き、
記録先の file に同じ UID で書けるので、CLI や CLI が動かした tool は hook 風の行を自分で追記できる。
`harness/evidence/rig-manifest.test.mjs` の stub CLI が実際にその方法で記録を作っている（test では
意図した使い方だが、同じことを本物の CLI もできる、という意味でもある）。

閉じるには記録を測定対象から書けない場所へ移す必要がある（別 UID の recorder・監督プロセス・
隔離した IPC）。設計変更なので別 issue に切り出した。**現時点でこの経路の証拠は 1 件も
committed されていない**（既存 16 件はすべて manifest 制度より前の legacy 証拠）ので、
出荷済みの matrix には影響しない。

同じ理由で、manifest の `isolated` と `internalRunMarker` は rig が「そう起動したつもり」を書いた
値であって、実効状態から導いた値ではない。

## capture cell 対照

| EventKind | Claude | Codex | 備考 |
|---|---|---|---|
| session_started | native | native | SessionStart |
| user_prompted | native | native | UserPromptSubmit |
| assistant_completed | synthesized | synthesized | 両者とも Stop.last_assistant_message から復元 |
| tool_started | native | native | PreToolUse |
| tool_completed | native | native | PostToolUse |
| tool_failed | synthesized | synthesized | **挙動が逆**: Claude は失敗時 PostToolUse を発火せず（2 回再現）、Codex は発火するが payload は `tool_response: ""` のみで成功と区別不能 |
| turn_completed | synthesized | **native** | Codex の Stop payload には `turn_id` がある。**Claude も turn 対応付け自体は可能**（UserPromptSubmit / Stop / SessionEnd が同一 `prompt_id` を共有。生 capture で確認）。差は id の名称と、Codex の SessionEnd には turn_id が無い点。[2026-08-12 訂正: 初版の「Claude には無い」は誤り] |
| pre_compact / post_compact | unknown | unknown | 本 Phase では未観測（compact を発火させる長時間セッションが必要） |
| session_idle | unknown | unknown | 未観測 |
| session_interrupted | synthesized | unknown | Claude は「Stop 無しで SessionEnd」パターンで合成可能（SIGINT 実測）。Codex は未観測 |
| session_ended | native | native | 両者とも `reason` は正常終了でも `other` で、終了理由の判別には使えない |

## Tier について

**Tier A は宣言しない**（v6.1 §29 Phase 0B の明示要件 + HI-23）。未観測 cell は `unknown` のままであり、
本 matrix は「観測できたものだけを証跡付きで確定した」ものである。

高位 cell は fixture が `highLevel` で観測結果を明記したものだけを反映する（推定はしない）。ただし
そのうち記録から導けるのは `subagentCapture` / `stableNativeSessionId` の 2 つで、注入系は上記のとおり
導けない。実測で
確定したのは Claude の `sessionStartInjection` / `subagentCapture` / `stableNativeSessionId`（= native）
と Codex の `sessionStartInjection`（= native）で、残りは `unknown` のまま。`compactionRecoveryStrategy`
は §7.2 の union に "unknown" が無いため **null**（未計測）とし、`unsupported` とは書かない — 未計測を
否定的事実として断定しないため。

未観測 cell は `evidenceKind: null` / `verifiedAt: null` を持つ。観測していない cell に証跡種別と
検証時刻を書くと provenance の捏造になるため、埋めない（初版は組立時刻を書いていた。2026-08-12 訂正）。

`toolFailurePhases` は観測できた phase、`toolFailurePhasesUntested` は試していない phase。前者だけを
見て「非対応」と読まないための対。fixture 単位の caveat は `fixtureLimitations` に全件保持する。

## 未観測を埋めるための追試（将来）

- pre_compact / post_compact: 長いセッションを実行して compact を誘発（子セッションの拡張思考は無効のまま）
- session_idle: idle 判定の発火条件を CLI 側ドキュメントで特定してから
- Codex の session_interrupted: SIGINT を送った際の hook 発火有無（Claude と同型の追試）
- tool failure phase の permission_denied / schema_invalid / unknown_tool: 権限拒否・不正 schema を意図的に起こす rig 拡張が必要
