# oboete 検証用の隔離ユーザー `oboete-dogfood` を作る手順

対象: この WSL 上の Ubuntu 24.04(`jura` は passwordless sudo あり)。所要時間はログイン込みで
30 分程度。すべてターミナルにコピーして実行する。`#` で始まる行は説明なので入力不要。

目的: oboete の自動検証(4 つのエージェントで本物のフックを動かす)を、あなたの普段の環境を
一切触らずに行うため。憲法(FR-041)で「本人の環境には M1 の間インストールしない」と決めている。

---

## 1. ユーザーを作る(`jura` のまま実行)

```bash
sudo adduser --disabled-password --gecos "oboete dogfood" oboete-dogfood
sudo usermod -aG ollama oboete-dogfood      # 端末内モデル(Ollama)を使えるようにする。任意
```

確認: `getent passwd oboete-dogfood` で 1 行出れば成功。

## 2. Node.js をシステムに入れる(`jura` のまま実行、1 回だけ)

`jura` の Node は nvm 管理で他ユーザーから読めないため、隔離ユーザー用にシステム版を入れる
(Pi が Node 22.19 以上を要求するので 24 系)。`jura` 側は nvm の Node が優先されるので影響なし。

```bash
curl -fsSL https://deb.nodesource.com/setup_24.x | sudo -E bash -
sudo apt-get install -y nodejs
sudo -u oboete-dogfood -H /usr/bin/node -v     # v24.x と出れば成功
```

## 3. 隔離ユーザーに切り替える

```bash
sudo -iu oboete-dogfood
whoami            # oboete-dogfood と出ること
echo $HOME        # /home/oboete-dogfood と出ること
```

以降の 4〜8 はこのシェル(プロンプトが `oboete-dogfood@...`)で実行する。途中で閉じたら
`sudo -iu oboete-dogfood` で戻れる。

## 4. PATH と npm の置き場を用意する

```bash
mkdir -p ~/.local/bin ~/.npm-global
npm config set prefix ~/.npm-global
printf '\nexport PATH="$HOME/.local/bin:$HOME/.npm-global/bin:$PATH"\n' >> ~/.bashrc
source ~/.bashrc
```

## 5. 4 つのエージェントを入れてログインする

ブラウザが必要な手順では、表示された URL を Windows 側のブラウザに貼り付けて開く。
WSL2 では `localhost` への戻り(コールバック)がそのまま届く。うまくいかない場合は
「コードを貼り付ける方式」(Claude Code、Codex の device auth)を使う。

### 5-1. Claude Code

```bash
curl -fsSL https://claude.ai/install.sh | bash
source ~/.bashrc
claude --version                 # 2.1.258 前後
claude auth login                # URL が出る → ブラウザで承認 → 表示されたコードを貼る
claude auth status               # ログイン済みと出れば成功
```

### 5-2. Codex CLI

```bash
npm install -g @openai/codex
codex --version                  # codex-cli 0.152 前後
codex login --device-auth        # URL とコードが出る → ブラウザで入力
codex login status               # Logged in と出れば成功
```

### 5-3. Grok Build

```bash
curl -fsSL https://x.ai/cli/install.sh | bash
source ~/.bashrc
grok update --alpha --version 1.0.17   # 契約調査は 1.0.17 で検証済み。同じ版に揃える(install.sh は stable 1.0.13 を入れる)
grok --version
grok login                       # ブラウザで X(xAI)アカウントにログイン
grok -p "reply with the single word ok"   # ok と返れば成功
```

### 5-4. Pi

```bash
npm install -g @earendil-works/pi-coding-agent
pi --version                     # 0.84.4 前後
pi                               # 対話画面が開く
```

対話画面で次を順に打つ:

1. `/login` → 使うプロバイダを選ぶ(Anthropic の Claude Pro/Max 契約、または OpenAI の
   ChatGPT 契約(Codex)のどちらか。持っている方)。ブラウザで承認する。
2. `/model` → 既定にしたいモデルを選び、**Ctrl+S** で起動時の既定として保存する。
3. Ctrl+C を 2 回で終了。

確認:

```bash
pi -p "reply with the single word ok"     # ok と返れば成功
```

## 6. リポジトリを隔離ユーザーにも用意する

検証スクリプトは `jura` のホーム(他ユーザーから読めない)ではなく、隔離ユーザー自身の複製から動かす。

```bash
git clone https://github.com/ojungo69/oboete.git ~/oboete
```

## 7. 資格情報の置き場(あとで渡すもの。今は空で良い)

Cloudflare の token などは、次のファイルに 1 行ずつ書いておくと実装セッションが読みに行く。
ファイルは本人しか読めない権限にする。

```bash
touch ~/.oboete-credentials && chmod 600 ~/.oboete-credentials
```

書式(値が手に入ったら追記):

```bash
export OBOETE_CF_API_TOKEN=...        # Cloudflare Workers AI 用 API token(任意。既定の要約先を使う場合)
export OBOETE_CF_ACCOUNT_ID=...       # Cloudflare Account ID(同上)
export OBOETE_NIM_API_KEY=...         # 検証に含めるプロバイダの分だけ。名前は OBOETE_<プロバイダ名>_API_KEY
export OBOETE_OPENROUTER_API_KEY=...   #   (NIM / OPENROUTER / GEMINI)。無い行の検証は飛ばされる
```

Cloudflare の値の取り方: https://dash.cloudflare.com で無料登録 → 右上のプロフィール →
「API Tokens」→「Create Token」→ テンプレート「Workers AI」→ 発行された token を控える。
AI Gateway を使う場合も token には「Workers AI - Read」権限が必要（AI Gateway 権限だけの token は
401 になる）。gateway 経由は同じ token で `cf-aig-gateway-id: <gateway 名>` ヘッダを付けるだけ。
Account ID はダッシュボードの「Workers & Pages」画面の右側に表示される。
ログイン済みのエージェントに要約させる方式(`agent-cli`)を使うなら、この 3 つはどれも不要。

## 8. 端末内モデル(任意)

Ollama はこの機体で既に動いている。隔離ユーザーからも使える:

```bash
ollama list          # 一覧が出れば接続できている(グループ設定が効くのは再ログイン後)
```

小さなモデルの取得は実装セッションが必要なときに指示する。

## 9. できたら次のセッションに伝えること

「隔離ユーザー `oboete-dogfood` を作成し、Claude Code / Codex / Grok Build / Pi のログインが
済んだ。手順書は `docs/research/isolated-user-setup.md`」と伝えれば、実装(`speckit-implement`、
tasks.md の T002 完了扱い → T003 以降)から再開できる。ログインできなかったエージェントが
あればその名前も伝える(そのエージェントの検証だけ後回しにする)。
