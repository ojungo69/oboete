# T015 — Clean Install 検証

日付: 2026-08-12 / 結果: **PASS**

## 手順（vendor snapshot からの素の install）

1. `vendor/codemem/` の内容を独立ディレクトリへコピー（`.git` / node_modules / 生成物なしの tracked tree）
2. `corepack pnpm install --frozen-lockfile` — 成功（lockfile 改変なし）
3. `corepack pnpm run build` — 成功（viewer static 含む全 workspace build）
4. `node packages/cli/dist/index.js --version` → `0.40.2`

## 環境

- node v24.16.0（engines `>=24` を満たす）/ pnpm 11.8.0（corepack、packageManager pin どおり）
- Linux WSL2（6.18.33.2-microsoft-standard-WSL2）
- native addon: better-sqlite3 12.8.0 のビルド/取得成功（install ログ参照: scratchpad で実施、要点のみ本書に記録）

## 備考

- 上流テストの実行結果は `codemem/upstream-test.log`（HOST RETRY2 = 4028/4037 pass）が正。本書は「素の環境で install → build → CLI 起動が通る」ことのみを検証する（SC-0A の clean install 項）。
