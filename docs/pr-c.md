# PR-C — id と repo キー (2026-09-24 着手)

仕様の正本は提案 §4.2 (先に直すもの)、§7 の PR-C 行、決定 7 と 11、issue #30 の PR-C の行。ここは実装で決めたことだけを置く。

## 分け方

提案の PR-C は、同期の前に直す 5 つを 1 つにまとめていた。レビューしやすい大きさにするため分ける。

- **C1 = repo キーを取得元 URL に** (この PR): §4.2 の 1。
- **C2 = session**: `"unknown"` の session id を端末 id 付きの一意な値に (§4.2 の 2)。session が触れた repo の集合 (決定 11、#30 の「入れ子の repo に移ったイベント」と「道具のパスから」)。
- **C3 = 文書の `uid`**: 端末 id と、DB を別の機械へ写したときの作り直し (§4.2 の 3)。

提案で PR-C に入っていた残りは、使う PR へ移す。使わない表を先に作ると、形を決める材料が無いまま固まるため。

- `embedder_id` とベクトル表 (`producer` 列つき) → PR-D (意味検索の本体)。
- 各行の `synced_at` と変更用の outbox 表 → PR-H (端末側の同期)。
- plan.md・m1.md の直し (提案 §7 の「docs の直し」) → C3 と一緒に。

## C1 で決めたこと

1. **キーは `origin` の URL を `host[:port]/path` にしたもの** (`repo::key`)。git は起動せず、`.git/config` (linked worktree は元の repo の、submodule は自分の git ディレクトリの `config`) の `[remote "origin"]` の `url` を読む。`[include] path = ...` で読み込むファイルも git と同じ位置に差し込んで読む (相対パスは読み込む側のファイルの場所から、`~/` はホーム、10 段まで)。`includeIf` は評価しない (条件はほぼ全体設定のためのもので、repo の設定では使われない)。
   - 正規化: scheme、認証情報 (userinfo)、query、fragment、末尾の `.git` と `/` を外し、host を小文字にする。port は scheme の既定 (ssh 22、https 443、http 80、git 9418) だけを外し、ほかは `host:port` で残す (#30、§7.1)。scp 形式 (`git@host:owner/repo`) も ssh と同じ形になる。
   - 手元のパスや `file://` の remote は、ほかの端末から同じ場所を指さないので使わず、パスのキーのままにする。
   - scp 形式の `host:/srv/x` (絶対パス) と `host:srv/x` (ホームからの相対) は同じキーになる。GitHub などでは `host:owner/repo` と `ssh://host/owner/repo` (URL 形式では絶対パス) が同じ repo を指すため、先頭の `/` で分けると同じ repo が 2 つのキーに割れる。混ざるのは、素の ssh サーバーで `~/srv/x` と `/srv/x` を別の repo として両方使う場合だけで、これは受け入れる。
   - `origin` の無い repo と、repo の外のディレクトリは、今までどおりパス。
2. **既存の行は `db::open` で 1 回だけ移し替え** (`PRAGMA user_version` 1)、**その後も `observe` のたびに同じ移し替えを行う** (`db::rekey_paths`、記録 hook の外。`git init` で使い始めて後から `git remote add origin` した repo の古い行が、次の observe で新しいキーに移る。評価用の store 約 18 万件で表の走査は 49 ms)。`sessions`・`observations`・`summaries`・`prompts`・`fts` の repo がこの機械にまだある絶対パスなら、今のキーに書き換える。無くなったパスや、パスでないキー (`claude-mem:<project>`) はそのまま。この PC の普段の store では 12 session が `github.com/ojungo69/oboete` に移った。
3. **MCP の `repo` 引数は、パスに加えてキーも受け付ける** (store が知っているキーだけ。結果や `timeline` に出るのはキーなので、agent がそれを渡せるように)。知らない文字列は今までどおりエラー。
4. **`oboete repo alias` (取得元の無い repo に本人の設定で名前を付ける、決定 7) は C1 では作らない。** この PC の repo は取得元があり、使う場面がまだ無い。2 台で取得元の無い repo を使う必要が出たら作る。repo の中の `.oboete.toml` では名前を付けられない (#30) のは、そもそも読まないので満たしている。

## C1 の回帰テスト

- ssh / scp 形式 / https / 認証情報付き / 既定の port 付きの同じ repo が 1 つのキーになる。既定でない port は残る。手元のパス・Windows のパス・`file://`・path の無い URL はキーにしない。
- `.git/config` の `origin` だけを読む (`upstream` や別の節の `url` は読まない。値の引用符とコメントを外す)。
- 元の repo・その linked worktree・別の場所に別の名前で clone したものが同じキーになる。取得元の無い repo はパスのまま。
- 移し替え: 残っているパスは新しいキーへ、無くなったパスと `claude-mem:x` はそのまま、`fts` も同じ、`user_version` が 1 になる。後から取得元を足した repo は、次の `rekey_paths` で移る。
- `[include]` で読み込んだファイルの `origin` を読む。自分を読み込むファイルは 10 段で止まる。読み込みの後に続く親の行は `[include]` の中のまま。
- MCP: キーで検索でき、知らないキーはエラー。
