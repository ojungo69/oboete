# PR-C — id と repo キー (2026-09-24 着手)

仕様の正本は提案 §4.2 (先に直すもの)、§7 の PR-C 行、決定 7 と 11、issue #30 の PR-C の行。ここは実装で決めたことだけを置く。

## 分け方

提案の PR-C は、同期の前に直す 5 つを 1 つにまとめていた。レビューしやすい大きさにするため分ける。

- **C1 = repo キーを取得元 URL に** (#43): §4.2 の 1。
- **C2 = 端末と session** (この PR): 端末 id と、DB を別の機械へ写したときの作り直し (§4.2 の 3 の前半)。`"unknown"` の session id を端末ごとに (§4.2 の 2)。session が触れた repo の集合 (決定 11、#30 の「入れ子の repo に移ったイベント」)。
- **C3 = 文書の `uid`** (§4.2 の 3 の後半)。

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

## C2 で決めたこと

1. **端末 id** は store を作ったときに乱数で決める 8 桁の 16 進 (`meta` 表の `device_id`)。
   - 提案は「ホスト名と OS のマシン ID が変わったら作り直す」だったが、**store のファイルそのものの同一性** (unix は device と inode、Windows は作成時刻) で見分ける。ホスト名を std だけで取れない OS (macOS) があり、そのために依存や unsafe を足さないため。別の機械へ写した `~/.oboete` は別のファイルなので新しい id になる。同じ機械でバックアップから戻したときも新しい id になるが、それで困ることは無い (前の行の id はそのまま)。
   - 2 つの hook が同時に最初に開いても、store の同一性を書き換えられた方だけが id を決める。
2. **session id の無いイベント**は `unknown-<端末 id>` の session に入れる。今までの `unknown` は全端末で同じ session になり、同期で衝突する。
3. **session が触れた repo** は `session_repos` 表に、イベントごとの作業ディレクトリの repo を足していく (同じ組は 1 回)。session を消すとこの行も消える。
   - この表ができる前に記録した session には行が無い。同期 (PR-H) は、行の無い session を「触れた repo が分からない」扱いにする (決定 11)。
   - 道具の作業ディレクトリやファイルのパスから足すもの (#30) は、同期で除外を判定する PR-H で作る。今はイベントの作業ディレクトリだけ。

## C2 の回帰テスト

- 端末 id は 8 桁の 16 進で、同じファイルを開き直しても変わらず、別の場所へ写したファイルでは別の id になる (写した先でも以後は変わらない)。
- 同じ session のイベントが外側の repo と入れ子の repo から来ると、両方が記録される。session id の無いイベントは `unknown-<端末 id>` に入る。

## C3 で決めたこと

1. **`uid` は `<端末 id>:<文書の id>`** (例 `7f3a9c21:o123`)。観測・要約・prompt の表に `uid` 列と一意の索引を足し、挿入のたびに DB のトリガーが埋める (hook・observe・viewer の削除以外のどの経路で入れても付く)。文書の id は AUTOINCREMENT なので使い回されず (m1.md 決定 14)、端末 id が端末ごとに違うので、端末をまたいでも重ならない。
2. **取り込んだ文書の `uid` は `<取り込み元>:<元の行>`** (例 `claude-mem:<DB の id>:o5`)。同じ claude-mem の DB を 2 台で取り込んでも、同じ記憶は同じ `uid` になり、同期で二重にならない。
3. **前からある行は `db::open` で 1 回だけ埋める** (`user_version` 2)。`imports` にある行は取り込み元から、残りはこの端末の id から。評価用の store (約 18 万件) で 2.1 秒、ピークの RAM 29 MB。
4. 画面と agent に見せる id は今までどおり `o123`。`get` (CLI・MCP) は `uid` も受け付ける。リモート MCP が `uid` を返すのは、それを作る PR で。

## C3 の回帰テスト

- 新しい prompt の `uid` は `<端末 id>:p<id>`、取り込んだ観測は `<取り込み元>:<元の行>`。`uid` を消して `user_version` を 1 に戻すと、開き直したときに同じ規則で埋まる。
- `get` は `uid` でも同じ文書を返し、知らない `uid` は無し。
