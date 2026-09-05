# T012 — Unsafe Path Action Plan（実施は Phase 1）

入力: T010 `codemem/write-handle-classification.md`（F1–F8 + 補遺）。
方針: v6.1 §29 Phase 1「undocumented/private provider/auth loader の物理削除・非到達化」。削除はコメントアウトや flag ではなく**コードとテストの物理削除 + 参照の除去**で行い、Phase 1 Exit の static scan で再確認する。セキュリティ関連のため全て Claude Code が自ら実施（委譲しない）。

## 削除ステップ（Phase 1 冒頭に一括実施）

| # | 対象 | 操作 | 検証 |
|---|---|---|---|
| A1 | F1 Anthropic OAuth consumer | `observer-client.ts` の `_callAnthropicConsumer`（2031-2073）と dispatch 分岐（1904 / 1911）を削除。`observer-auth.ts` の OAuth cascade 段も同時除去（A3 と共通） | grep で `anthropic` × `oauth` の到達経路ゼロ / tsc green |
| A2 | F2 Codex consumer + F4 OpenCode OAuth cache 読出し | `_callCodexConsumer`（1992-2030、dispatch 1899 / 1910）/ `buildCodexHeaders` / `observer-auth.ts:49-183`（cache path resolver・extractors・Codex header 構築）を削除 | 第三者 credential ファイルパス（OpenCode auth cache）への参照ゼロ |
| A3 | F8 credential command loader + cascade 縮小 | `observer-auth.ts:185-206` 削除。cascade を `explicit -> env -> file` に縮小し、`oauth`/`command` 段の型・設定キーも除去 | 設定 schema に残キーなし / 単体テスト更新 |
| A4 | F3 Claude sidecar（plain -p） | `_buildSidecarCommand`(2074-2089) / `_invokeSidecar`(2090-2206) / `_callSidecar`(2207-2255) の 3 関数（連続 2074-2255）と dispatch 分岐（1889）を削除。v6.1 §13.6 準拠実装（`--bare` + `ANTHROPIC_API_KEY` 必須 + certification manifest）は Phase 6 の別 optional PR まで**不在のまま**とする | `bypassPermissions` 文字列の残存ゼロ / tsc green（呼出し元残存なし） |
| A5 | F5/F6/F5'/F6' hook 直接書込み（4 経路） | `claude-hook-ingest.ts:128-199` の `directEnqueue` / `codex-hook-ingest.ts:128-197` の `directEnqueueCodexHook`（export 名はソース照合済み: 同 :121）、`claude-hook-inject.ts` の `buildLocalPack`（:128、呼出し 236-249 — **既定の第一経路**であり fallback ではない。HTTP 側 250-255 が fallback）、`claude-hook-file-context.ts:263-370` の store open を削除。置換: ingest は atomic spool（§8）、inject/file-context は daemon RPC read + event 投入 | Phase 1 Exit の runtime DB-open trace で hook プロセスから write-capable open ゼロ |
| A6 | F7 bootstrap template | `scripts/templates/workspace-codemem-bootstrap.sh` を削除（Core 1.0 に workspace peer 概念なし） | テンプレート参照ゼロ |
| A7 | carve-out 無効化（fatal ではないが同時実施が安全） | sync/coordinator/sharing/recipient-policy/MCP HTTP OAuth の各モジュール・routes・CLI サブコマンド登録を削除（T010 carve-out 節の一覧）。対応するテストファイルは retire リストに記録して削除 | CLI help に sync/coordinator 系が出ない / viewer から sync タブ関連 API 404 |

## 順序と依存

1. A7（大きい面を先に落とす — A1–A5 の対象の一部は carve-out 配下で共倒れするため、先にやると差分が小さくなる）
2. A1–A4（auth 系。observer-client/observer-auth に局在）
3. A5（hook 系。spool/RPC の置換実装を伴う）
4. A6
5. static scan + runtime DB-open trace（Phase 1 Exit gate、Hard Invariant 4 の blocking 検証）

## リスクノート

- A5 は置換実装（spool/RPC）を伴う唯一のステップで、削除だけでは hook が機能喪失する。Phase 1 の本体実装（daemon RPC/spool）と同一ブランチで行う。
- A7 のテスト retire は「壊す test 数」として delta-comparison の想定内。retire リスト（ファイル名列挙）を Phase 1 の PR 説明に含め、speckit-verify-tasks で照合する。
- 削除後も `direct Anthropic/OpenAI HTTP（API key/BYOK）` 経路は独立して残る（dispatch 構造上の確認済み）。Phase 6 まで生成は無効（daemon 側で generation 呼び出しを行わない）でよく、経路の存在自体は §13 の generic adapter 設計の土台になる。
