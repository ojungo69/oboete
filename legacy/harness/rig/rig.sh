#!/usr/bin/env bash
# Phase 0B capture rig — 実 CLI (claude / codex) をユーザー実環境から隔離して起動し、
# hook lifecycle を JSONL で捕捉する。v6.1 §29 Phase 0B / §13.6 (internal run marker)。
#
# 隔離の内容:
#   - scratch HOME / CLAUDE_CONFIG_DIR / CODEX_HOME (実環境の plugin/hook/設定を継承しない)
#   - capture 専用 hook のみを配線
#   - 使い捨て git workspace (実 repo に触れない)
#   - AGENT_MEMORY_INTERNAL_RUN=1 (§13.6 marker; 将来の adapter は capture 対象外にする)
#   - 資格情報ファイルのみ scratch へコピー (子 CLI の認証に必要)。コピーは 1 回の実行中だけ
#     置かれ、EXIT/INT/TERM の trap で必ず消える (teardown 待ちで /tmp に残さない)
#
# 使い方:
#   rig.sh setup                     # RIG_BASE を作る (env RIG_BASE で場所指定可)
#   rig.sh claude-run <label> <prompt> [claude 追加引数...]
#   rig.sh codex-run  <label> <prompt> [codex exec 追加引数...]
#   rig.sh import <cli> <label> <scenario-id>   # 証拠置き場へ持ち込み manifest を書く
#   rig.sh teardown                  # RIG_BASE を完全削除 (資格情報コピー含む)
set -euo pipefail
# 呼び出し元が export した shell function は環境から引き継がれ、**PATH より先に**選ばれる
# （`export -f git` を 1 つ置くだけで `command -v git` は path ではなく `git` を返す）。
# 隔離を作る側の command がそれだと、`env -i` も `git` も呼び出し元の code になり、それでも
# manifest は isolated: true を書く。自分の関数を定義する前に、継いだものを全部外す
while read -r _ _ fn; do unset -f "$fn"; done < <(declare -F)
# 隔離の境界で使う実行 file は、呼び出し元の PATH からも探さない
TRUSTED_PATH=/usr/local/bin:/usr/bin:/bin
ENV_BIN=$(PATH="$TRUSTED_PATH" type -P env) || { echo "env not found in the trusted path" >&2; exit 3; }
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RIG_BASE="${RIG_BASE:-/tmp/free-mem-rig-$USER}"
HOOK="$DIR/capture-hook.sh"
CLAUDE_BIN="${CLAUDE_BIN:-$(command -v claude || true)}"
CODEX_BIN="${CODEX_BIN:-$(command -v codex || true)}"
NODE_BIN="$(command -v node || true)"
[ -n "$NODE_BIN" ] || { echo "node not found on PATH — refusing to run (PATH に '.' が混ざる事故を防ぐ)" >&2; exit 3; }
NODE_DIR="$(dirname "$NODE_BIN")"

require_rig_base() { # 消す・権限を変える前に、そこが rig の置き場だと確かめる
  # RIG_BASE を決めるのは呼んだ側で、teardown は中身ごと消す。既にある directory をそのまま
  # 扱う形だと、打ち間違い 1 つで無関係な木を消し、権限まで変える。setup が置いた marker が
  # ある場所と、まだ何も無い場所だけを扱う
  [ ! -e "$RIG_BASE" ] || [ -f "$RIG_BASE/.rig-base" ] || {
    echo "the given base was not created by the rig — refusing to touch it" >&2; exit 5; }
}

# 資格情報コピーは必ず消す。teardown を呼び忘れても、異常終了しても残さない。
purge_credentials() {
  rm -f "$RIG_BASE/claude-config/.credentials.json" "$RIG_BASE/codex-home/auth.json" 2>/dev/null || true
}
# **置いた本人だけが消す**。lock を取れずに exit 4 で降りる process までここを通ると、
# 走っている run のために置いてある資格情報を横から消し、その run の認証を壊す
STAGED=0
purge_own_credentials() { [ "$STAGED" -eq 1 ] && purge_credentials; return 0; }
trap purge_own_credentials EXIT INT TERM

# 資格情報は「その 1 回の子 CLI 実行の間だけ」置く（trap で必ず消える）。
stage_credentials() { # $1 = claude | codex
  # 置く前に必ず消す。SIGKILL で trap が走らなかった前回の残りと、もう一方の provider の
  # 分がここに残っていると、測定対象の tool から同じ UID で読める。
  # ここを通った時点で「置いた本人」になる（lock は既に握っている）
  STAGED=1
  purge_credentials
  # **実行する CLI の分だけ**置く。両方置くと、測定対象の CLI が動かす tool から
  # もう一方の資格情報を同じ UID で読める（read-only sandbox は読み取りを防がない）
  case "$1" in
    claude) [ -f "$HOME/.claude/.credentials.json" ] && install -m 600 "$HOME/.claude/.credentials.json" "$RIG_BASE/claude-config/.credentials.json" ;;
    codex)  [ -f "$HOME/.codex/auth.json" ] && install -m 600 "$HOME/.codex/auth.json" "$RIG_BASE/codex-home/auth.json" ;;
    *) echo "stage_credentials: unknown cli $1" >&2; return 1 ;;
  esac
  return 0
}

setup() {
  require_rig_base
  mkdir -p "$RIG_BASE"; chmod 700 "$RIG_BASE"
  : > "$RIG_BASE/.rig-base"
  mkdir -p "$RIG_BASE"/{home,claude-config,codex-home,workspace,capture}
  # 設定を書き換えるのは、走っている run の足元。取らずに書くと、測定対象が hook の無い
  # settings.json を読んだ記録が撮れてしまい、それでも digest は合うので証拠として通る。
  #
  # lock file を置くのもここ（`exec 9>` が無ければ作る）。teardown 側で「あれば取る」に
  # すると、無い瞬間を見た直後に run が作って握る隙間ができる（その run の下から base を
  # 消せてしまう）。既にあれば truncate されるだけで inode は変わらないので、握っている
  # 側の lock は落ちない
  with_lock
  # 握れた = 走っている run はいない。SIGKILL で trap が走らなかった前回の残りは誰のものでも
  # ないので、ここで消す（置いた本人以外が消してよいのは、lock を握れたこの瞬間だけ）
  purge_credentials
  sed "s|__HOOK__|$HOOK|g" "$DIR/claude-settings-template.json" > "$RIG_BASE/claude-config/settings.json"
  sed "s|__HOOK__|$HOOK|g" "$DIR/codex-config-template.toml" > "$RIG_BASE/codex-home/config.toml"
  if [ ! -d "$RIG_BASE/workspace/.git" ]; then
    # git の既定 template（`/usr/share/git-core/templates`）も複製させない。空を明示しないと
    # その hook が測定用 workspace の .git に並ぶ
    git_iso init -q --template=
    echo "rig workspace" > "$RIG_BASE/workspace/README.md"
    git_iso add -A
    git_iso -c user.email=rig@local -c user.name=rig commit -qm init
  fi
  # 置き場を報告に書かない。FR-015 は診断出力に実行環境の絶対 path を載せることを禁じており、
  # どこに作るかを決めたのは呼んだ側なので、書いても分かることは増えない
  echo "rig ready"
}

git_iso() { # 測定用 workspace の git を実環境から切り離して走らせる
  # 実環境の設定が入ると、隔離したはずの測定の中で operator の hook が走る（それでも manifest は
  # isolated: true を書く）。設定の入口は file だけではない: GIT_CONFIG_COUNT / GIT_CONFIG_PARAMETERS は
  # command-scope の設定を注入でき、GIT_DIR 以下は repository の位置そのもの、GIT_EXEC_PATH は git が
  # 呼ぶ実行 file の在り処を指す。外す変数を列挙する形は、次に増えた入口が黙って通る側へ回るので、
  # run_env と同じく渡すものだけを決める。`env -i` は /etc/gitconfig を止めないので明示する
  # 探す場所も呼び出し元から取らない。`command -v git` は**環境を畳む前**の PATH で解決するので、
  # 先頭に `.` や wrapper を置いた呼び出し元は、隔離したはずの workspace を自分の program に
  # 作らせられる（それでも manifest は isolated: true を書く）
  local git_bin; git_bin=$(PATH="$TRUSTED_PATH" type -P git) \
    || { echo "git not found in the trusted path" >&2; exit 1; }
  "$ENV_BIN" -i \
    PATH="${git_bin%/*}:/usr/bin:/bin" \
    HOME="$RIG_BASE/home" \
    GIT_CONFIG_NOSYSTEM=1 \
    "$git_bin" -C "$RIG_BASE/workspace" "$@" 9>&-
}

run_env() { # 最小環境で子 CLI を起動する共通部。$1 = claude | codex
  local cli="$1" capture="$2"; shift 2
  # 対象 provider の設定だけ渡す。両方渡すと、測定対象がもう一方の config directory を
  # 辿れる（資格情報を消していても、設定そのものが観測の対象外の情報になる）
  local cfg=()
  case "$cli" in
    claude) cfg=(CLAUDE_CONFIG_DIR="$RIG_BASE/claude-config") ;;
    codex)  cfg=(CODEX_HOME="$RIG_BASE/codex-home") ;;
    # 知らない名前で呼ばれたら降りる。設定 dir を渡さずに起動すると、測定対象は既定の場所
    # （= 実環境）を見に行く。隔離の外で測ったのに、記録も manifest も同じ形で残る
    *) echo "run_env: unknown cli" >&2; exit 2 ;;
  esac
  "$ENV_BIN" -i \
    PATH="$NODE_DIR:/usr/local/bin:/usr/bin:/bin" \
    HOME="$RIG_BASE/home" \
    TERM=dumb \
    "${cfg[@]}" \
    AGENT_MEMORY_INTERNAL_RUN=1 \
    GIT_CONFIG_NOSYSTEM=1 \
    CAPTURE_FILE="$capture" \
    ${INJECT_MARKER:+INJECT_MARKER="$INJECT_MARKER"} \
    ${HOOK_SLEEP:+HOOK_SLEEP="$HOOK_SLEEP"} \
    "$@"
}

with_lock() { # 並行 run を禁止する。同じ RIG_BASE を共有すると、片方の credential を
  # もう片方の測定対象が同じ UID で読める
  exec 9>"$RIG_BASE/.lock"
  # 失敗の説明に $RIG_BASE を出さない。FR-015 は診断出力に実行環境の絶対 path を載せることを
  # 禁じており、置き場を決めたのは呼んだ側なので、path を書いても分かることは増えない
  flock -n 9 || { echo "another rig run holds the lock" >&2; exit 4; }
}

# 測定対象を独自の process group で起動し、終わったら group ごと畳む。
#
# **これは測定対象の話**。rig 自身の道具（workspace を作る git）には逆で、`git_iso` は fd を
# 閉じて渡す。git は commit のあと維持作業を daemon として切り離すことがあり、その子が lock を
# 握ったまま残ると、次の run が「別の run が走っている」と言って止まる——測定対象の残骸と違って
# 止めても守るものが無く、operator の rig が理由も分からず使えなくなるだけ。
#
# lock の fd を子へ渡さない形（`9>&-`）は採らない。渡さないと、CLI が残した process が
# 生きているうちに次の run が始まり、その run が置いた**別 provider の資格情報**を、
# 残った process が同じ UID で読める。fd を渡しておけば残骸が lock を握り続け、次の run は
# 止まる。一方で握らせたままだと daemon 1 つで rig が使えなくなるので、run の終わりに
# group ごと畳んで普通の残骸は片付ける。
#
# ここで成り立つのは 2 つまで。事故で残った daemon は畳める。group を抜けたが fd を握った
# ままの process がいれば次の run は止まる（fail closed）。**抜けたうえで自分で fd 9 を
# 閉じた process には効かない** —— lock は協調的な仕組みなので、協調しない相手は止められない。
# 同一 UID で走らせる限りこれは閉じない（#95）。#90 と同じで、rig は敵対的な測定対象に
# 対する信頼境界ではない
# `timeout` は既定で対象を**自分の** process group へ移すので、そのままだと下の
# `kill -- -<pid>` が測定対象へ届かない。`--foreground` で group を作らせず、畳むのは
# こちらの group 1 つに統一する（timeout 自身の group kill を失うが、後段で group ごと
# 畳むので取りこぼしは増えない）
# SIGTERM だけでは足りない。無視する子が group に残ると lock を握ったままになり、以後の run が
# 全部止まる（畳めるはずの残骸で可用性を失う）。猶予のあとに生き残りを確かめて SIGKILL する。
# group を抜けた process には届かないままなので、逃げた側の fail closed は変わらない
# 取り込み側と同じ綴りを、測定を始める前に要求する。合わない label で撮った記録は import が
# 必ず落とすので、走らせた分の CLI 起動が丸ごと無駄になる。`/` を含む label なら capture/ の
# 外へ書ける
require_label() {
  [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || { echo "label must be a plain file-name token" >&2; exit 2; }
}

reap_group() {
  kill -- "-$1" 2>/dev/null || true
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    kill -0 -- "-$1" 2>/dev/null || return 0
    sleep 0.2
  done
  kill -KILL -- "-$1" 2>/dev/null || true
}

claude_run() {
  [ "$#" -ge 2 ] || { echo "usage: rig.sh claude-run <label> <prompt> [claude 追加引数...]" >&2; exit 2; }
  local label="$1" prompt="$2" rc=0; shift 2
  require_label "$label"
  with_lock
  [ -n "$CLAUDE_BIN" ] || { echo "claude not found" >&2; exit 1; }
  # 付属物は import-evidence.mjs と同じ stem で並べる。記録だけ拡張子付きの別名にすると、
  # 「消す名前」と「書く名前」がずれても誰も気づかない（.exit が実際にずれていた）
  local stem="$RIG_BASE/capture/claude-$label"
  local capture="$stem.jsonl"
  stage_credentials claude
  # --version も測定対象の起動なので、本実行と同じ扱いにする（ここだけ残骸が残ると
  # 次の run が lock で止まる。実際にこの経路だけ監督から漏れていた）
  local ver_pid=0 run_pid=0 ver_rc=0
  # 版の問い合わせも測定対象の起動なので、本実行と同じ隔離で行う。ここだけ素で起動していると、
  # その 1 回だけ実 HOME・実設定・実 plugin を見た状態で測定対象が動く。作業場所も分ける:
  # claude は cwd から上へ設定を探すので、呼び出し元の repository に居るだけで実設定に届く。
  # 記録先も分ける。問い合わせが hook を起こしたとき、その event が scenario の記録に
  # 混ざって同じ manifest と digest に入るのを防ぐ
  local ver_capture="$stem.version-probe.jsonl"
  # 問い合わせは**使い捨ての state** で行う。同じ scratch を使うと、初回起動で設定を書く claude が
  # 本実行を「2 回目の起動」にしてしまい、その差が scenario の記録として残る
  local ver_state="$RIG_BASE/version-state"
  rm -rf "$ver_state"; mkdir -p "$ver_state"/{home,claude-config,codex-home,workspace}
  set -m
  # 問い合わせにも時間制限を掛ける。掛けないと、更新待ちなどで固まった --version が lock と
  # 資格情報を握ったまま帰らず、以後の run・import・teardown が全部止まる。
  # `--kill-after` まで付ける: SIGTERM を捕まえる・無視する測定対象だと、timeout は最初の
  # signal のあと待ち続けるので、時間制限を付けただけでは同じところで固まる。
  # 締め切りを env で縮められるようにしてあるのは `VERSION_TIMEOUT` と同じ理由で、test から
  # 秒を待たずに「止めの signal が飛ぶ」ことだけを見るため
  ( cd "$ver_state/workspace" && RIG_BASE="$ver_state" run_env claude "$ver_capture" \
      timeout --foreground --kill-after="${VERSION_KILL_AFTER:-5s}" "${VERSION_TIMEOUT:-60}" "$CLAUDE_BIN" --version ) > "$stem.version.new" 2> "$stem.version.err.new" & ver_pid=$!
  set +m
  # 失敗した問い合わせの出力を版として扱わない。1 行だけ吐いて非ゼロで終える測定対象は、
  # そのエラー行がそのまま cliVersion として証拠に載り「この版で測った」と読めてしまう。
  # 畳むのが先。ここで抜けるときも残骸を置いていかない
  wait "$ver_pid" || ver_rc=$?
  reap_group "$ver_pid"
  # 版として読むのは stdout だけ。混ぜると、stdout に何も出さず stderr に 1 行だけ出して
  # 終了 0 で帰る CLI で、その診断文が cliVersion として証拠に載る
  # 問い合わせの記録も state も持ち込みの対象にしない。中身も残さない
  rm -rf "$ver_state"
  rm -f "$ver_capture" "$ver_capture.errors"
  # 落ちた問い合わせで前の run の記録を失わない。降りる前に消すのは今回書いたものだけで、
  # 取り込み前の記録・その終了コード・記録失敗の痕跡はそのまま残す（測定は始まってもいない）
  [ "$ver_rc" -eq 0 ] || { rm -f "$stem.version.new" "$stem.version.err.new"; echo "claude --version failed (exit=$ver_rc)" >&2; exit 1; }
  # ここから前の記録を置き換える。記録失敗の痕跡も run ごとに消す。残すと前回の失敗が今回の
  # manifest の recorderErrors に載り、正しい証拠が棄却される。終了コードも同じ理由で消す:
  # run が SIGKILL で落ちると前回の成功が残り、途中で切れた記録に exitStatus=0 が付く。
  # .errors だけは hook が $CAPTURE_FILE から作るので記録側の名前になる
  # 消すのが先、公開が後。逆にすると、その間に落ちた瞬間だけ「前の記録 + 新しい版」が揃い、
  # 取り込みが**前の run の記録を新しい版で測ったこと**にできる。この順なら、途中で落ちても
  # 記録が空で終了コードも無い＝取り込みが必ず断る
  : > "$capture"; rm -f "$capture.errors" "$stem.exit"
  mv "$stem.version.new" "$stem.version"; mv "$stem.version.err.new" "$stem.version.err"
  # 測定にも止めの signal の締め切りを付ける。SIGTERM を捕まえる・無視する測定対象だと
  # timeout は最初の signal のあと待ち続け、`wait` が帰らないので reap_group まで届かない
  # ——lock と staged な資格情報を握ったまま、rig が丸ごと止まる
  set -m
  ( cd "$RIG_BASE/workspace" && \
    run_env claude "$capture" timeout --foreground --kill-after="${RUN_KILL_AFTER:-5s}" ${RUN_SIGNAL:+--signal="$RUN_SIGNAL"} "${RUN_TIMEOUT:-300}" "$CLAUDE_BIN" -p "$prompt" \
      --model haiku --output-format json --max-turns 4 "$@" \
      > "$stem.stdout" 2> "$stem.stderr" ) & run_pid=$!
  set +m
  wait "$run_pid" || rc=$?
  reap_group "$run_pid"
  # 終了コードは数値で別に残す。manifest の exitStatus はここから読む
  printf '%s\n' "$rc" > "$stem.exit"
  [ "$rc" -eq 0 ] || echo "exit=$rc (recorded)" >> "$stem.stderr"
  echo "captured: ${capture##*/} ($(wc -l < "$capture") events)"
}

codex_run() {
  [ "$#" -ge 2 ] || { echo "usage: rig.sh codex-run <label> <prompt> [codex exec 追加引数...]" >&2; exit 2; }
  local label="$1" prompt="$2" rc=0; shift 2
  require_label "$label"
  with_lock
  [ -n "$CODEX_BIN" ] || { echo "codex not found" >&2; exit 1; }
  local stem="$RIG_BASE/capture/codex-$label"
  local capture="$stem.jsonl"
  stage_credentials codex
  local ver_pid=0 run_pid=0 ver_rc=0
  # 版の問い合わせも測定対象の起動なので、本実行と同じ隔離で行う。ここだけ素で起動していると、
  # その 1 回だけ実 HOME・実設定・実 plugin を見た状態で測定対象が動く。作業場所も分ける:
  # codex は cwd から上へ設定を探すので、呼び出し元の repository に居るだけで実設定に届く。
  # 記録先も分ける。問い合わせが hook を起こしたとき、その event が scenario の記録に
  # 混ざって同じ manifest と digest に入るのを防ぐ
  local ver_capture="$stem.version-probe.jsonl"
  # 問い合わせは**使い捨ての state** で行う。同じ scratch を使うと、初回起動で設定を書く codex が
  # 本実行を「2 回目の起動」にしてしまい、その差が scenario の記録として残る
  local ver_state="$RIG_BASE/version-state"
  rm -rf "$ver_state"; mkdir -p "$ver_state"/{home,claude-config,codex-home,workspace}
  set -m
  # 問い合わせにも時間制限を掛ける。掛けないと、更新待ちなどで固まった --version が lock と
  # 資格情報を握ったまま帰らず、以後の run・import・teardown が全部止まる。
  # `--kill-after` まで付ける: SIGTERM を捕まえる・無視する測定対象だと、timeout は最初の
  # signal のあと待ち続けるので、時間制限を付けただけでは同じところで固まる。
  # 締め切りを env で縮められるようにしてあるのは `VERSION_TIMEOUT` と同じ理由で、test から
  # 秒を待たずに「止めの signal が飛ぶ」ことだけを見るため
  ( cd "$ver_state/workspace" && RIG_BASE="$ver_state" run_env codex "$ver_capture" \
      timeout --foreground --kill-after="${VERSION_KILL_AFTER:-5s}" "${VERSION_TIMEOUT:-60}" "$CODEX_BIN" --version ) > "$stem.version.new" 2> "$stem.version.err.new" & ver_pid=$!
  set +m
  # 失敗した問い合わせの出力を版として扱わない。1 行だけ吐いて非ゼロで終える測定対象は、
  # そのエラー行がそのまま cliVersion として証拠に載り「この版で測った」と読めてしまう。
  # 畳むのが先。ここで抜けるときも残骸を置いていかない
  wait "$ver_pid" || ver_rc=$?
  reap_group "$ver_pid"
  # 版として読むのは stdout だけ。混ぜると、stdout に何も出さず stderr に 1 行だけ出して
  # 終了 0 で帰る CLI で、その診断文が cliVersion として証拠に載る
  # 問い合わせの記録も state も持ち込みの対象にしない。中身も残さない
  rm -rf "$ver_state"
  rm -f "$ver_capture" "$ver_capture.errors"
  # 落ちた問い合わせで前の run の記録を失わない。降りる前に消すのは今回書いたものだけで、
  # 取り込み前の記録・その終了コード・記録失敗の痕跡はそのまま残す（測定は始まってもいない）
  [ "$ver_rc" -eq 0 ] || { rm -f "$stem.version.new" "$stem.version.err.new"; echo "codex --version failed (exit=$ver_rc)" >&2; exit 1; }
  # ここから前の記録を置き換える。記録失敗の痕跡も run ごとに消す。残すと前回の失敗が今回の
  # manifest の recorderErrors に載り、正しい証拠が棄却される。終了コードも同じ理由で消す:
  # run が SIGKILL で落ちると前回の成功が残り、途中で切れた記録に exitStatus=0 が付く。
  # .errors だけは hook が $CAPTURE_FILE から作るので記録側の名前になる
  # 消すのが先、公開が後。逆にすると、その間に落ちた瞬間だけ「前の記録 + 新しい版」が揃い、
  # 取り込みが**前の run の記録を新しい版で測ったこと**にできる。この順なら、途中で落ちても
  # 記録が空で終了コードも無い＝取り込みが必ず断る
  : > "$capture"; rm -f "$capture.errors" "$stem.exit"
  mv "$stem.version.new" "$stem.version"; mv "$stem.version.err.new" "$stem.version.err"
  # 測定にも止めの signal の締め切りを付ける。SIGTERM を捕まえる・無視する測定対象だと
  # timeout は最初の signal のあと待ち続け、`wait` が帰らないので reap_group まで届かない
  # ——lock と staged な資格情報を握ったまま、rig が丸ごと止まる
  set -m
  ( cd "$RIG_BASE/workspace" && \
    run_env codex "$capture" timeout --foreground --kill-after="${RUN_KILL_AFTER:-5s}" ${RUN_SIGNAL:+--signal="$RUN_SIGNAL"} "${RUN_TIMEOUT:-300}" "$CODEX_BIN" exec --json --skip-git-repo-check \
      --dangerously-bypass-hook-trust "$@" "$prompt" \
      > "$stem.stdout" 2> "$stem.stderr" ) & run_pid=$!
  set +m
  wait "$run_pid" || rc=$?
  reap_group "$run_pid"
  # 終了コードは数値で別に残す。manifest の exitStatus はここから読む
  printf '%s\n' "$rc" > "$stem.exit"
  [ "$rc" -eq 0 ] || echo "exit=$rc (recorded)" >> "$stem.stderr"
  echo "captured: ${capture##*/} ($(wc -l < "$capture") events)"
}

# 証拠置き場へ byte 同一で持ち込んでから digest を取る。持ち込む前に取ると、
# 持ち込みで内容が変わっても気づけない
import_evidence() {
  [ "$#" -eq 3 ] || { echo "usage: rig.sh import <cli> <label> <scenario-id>" >&2; exit 2; }
  local cli="$1" label="$2" scenario="$3"
  # 記録中に読むと、途中までで一貫した prefix を掴んで正しく見える manifest を作る
  with_lock
  node --experimental-strip-types "$DIR/import-evidence.mjs" \
    --cli "$cli" --label "$label" --scenario-id "$scenario" --from "$RIG_BASE/capture"
}

case "${1:-}" in
  setup) setup ;;
  claude-run) shift; claude_run "$@" ;;
  codex-run) shift; codex_run "$@" ;;
  import) shift; import_evidence "$@" ;;
  # teardown も lock を取る。走っている run の下で消すと、setup が新しい .lock inode を作り、
  # 生きた測定対象の隣へ次の run が資格情報を置ける（直列化と資格情報の分離が両方外れる）。
  # 「あれば取る」にはしない。無い瞬間を見た直後に run が作った base を、lock を取らないまま
  # 消せてしまう。取るために作ってから取る（消すだけの回で作り直す形になるが、空の base を
  # 1 度作って消すだけで、握れなければそこで降りる）
  teardown) require_rig_base; mkdir -p "$RIG_BASE"; chmod 700 "$RIG_BASE"; with_lock; rm -f "$RIG_BASE/claude-config/.credentials.json" "$RIG_BASE/codex-home/auth.json"; rm -rf "$RIG_BASE"; echo "rig removed" ;;
  *) echo "usage: rig.sh setup|claude-run <label> <prompt>|codex-run <label> <prompt>|import <cli> <label> <scenario-id>|teardown" >&2; exit 2 ;;
esac
