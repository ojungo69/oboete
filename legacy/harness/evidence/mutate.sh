#!/usr/bin/env bash
# 変異テスト: 証拠 digest まわりの各ゲートをわざと壊し、対応する test が落ちることを確かめる。
#
# 使い方: bash harness/evidence/mutate.sh
# 出力の各行は「M 番号 + ラベル」と、その変異を入れたときの fail 件数。
# **fail 0 の行は生存した変異**（そのゲートを壊しても test が落ちない = 検証が効いていない）。
#
# 実行件数も必ず突き合わせる。アンカーが実装の変更で外れると `assert count == 1` が落ちて
# `&&` が短絡し、その変異は出力に何も出ないまま黙って飛ばされる。
#
# 中断すると変異が残る。その場合は `git checkout harness/ .github/workflows/ci.yml`。
# **`harness/` だけでは足りない**: MUTABLE には .github/workflows/ci.yml も入っていて、
# CI の段を落とす変異を受ける。逆に specs/003-…/tasks.md は読むだけなので戻す必要はない
set -u
cd "$(dirname "$0")/../.."

ASSEMBLE=harness/assemble.ts
VERIFY=harness/evidence/verify.ts
NORMALIZE=harness/evidence/normalize.ts
SCHEMA=harness/schema/capability.schema.json
MSCHEMA=harness/schema/evidence-manifest.schema.json
IMPORT=harness/rig/import-evidence.mjs
RIG=harness/rig/rig.sh
SCHEMAV=harness/schema/validate.ts
CAP=harness/schema/capability.ts
# 出荷データ側。kill switch (#90) は実装ではなく commit 済みの成果物を見るので、
# 実装の変異では触れない。fixture 1 件を変異対象に入れて歯止めが本当に鳴るかを見る
FIXTURE=harness/fixtures/claude/lifecycle-basic.json
HASHES=harness/contract-hashes.json
TASKS=specs/003-evidence-hash-normalization/tasks.md
# 出荷 matrix。kill switch と drift 検査が「復号した値」で見ているかを確かめる
MATRIX=harness/matrix/claude.json
CI=.github/workflows/ci.yml

MUTABLE=("$ASSEMBLE" "$VERIFY" "$NORMALIZE" "$SCHEMA" "$MSCHEMA" "$SCHEMAV" "$CAP" "$IMPORT" "$RIG" "$HASHES" "$FIXTURE" "$MATRIX" "$CI")
TESTS=(
  harness/evidence/hash-inputs.test.ts
  harness/evidence/killswitch.test.ts
  harness/evidence/manifest.test.ts
  harness/evidence/matrix-drift.test.ts
  harness/evidence/normalize.test.ts
  harness/evidence/promotion.test.ts
  harness/evidence/schema.test.ts
  harness/evidence/secrets.test.ts
  harness/evidence/rig-manifest.test.mjs
)
BAKDIR=$(mktemp -d) || { echo "変異テスト失敗: 退避用の一時 directory を作れない" >&2; exit 1; }
restore_all() {
  local f rc=0
  for f in "${MUTABLE[@]}"; do cp "$BAKDIR/$f" "$f" || rc=1; done
  [ "$rc" -eq 0 ] || echo "変異テスト失敗: 変異を戻せない。退避 $BAKDIR は消さずに残す" >&2
  return "$rc"
}
# 復元に失敗した経路で退避まで消すと、変異が乗った source を戻す手立てが `git checkout` しか
# 無くなり、同じ作業ツリーに同居している未 commit の変更まで巻き添えで消える
trap 'restore_all && rm -rf "$BAKDIR"' EXIT
# 退避は path ごと持つ。basename で平潰しにすると、別 directory の同名 file を MUTABLE に
# 足した日に退避が上書きされ、復元が**別の file の中身**を書き戻す（ゲートの中で黙って repo を
# 壊す形になる）。退避が 1 つでも作れなかったら、変異を当てる前に降りる。復元できない状態で
# 144 件の変異を実 source に積み上げると、出口が `git checkout` しかなくなる
for f in "${MUTABLE[@]}"; do
  cp --parents "$f" "$BAKDIR" || { echo "変異テスト失敗: $f を退避できない（cp --parents は GNU coreutils 拡張）" >&2; exit 1; }
done

# --- 変異表との突き合わせ（T042）。長い実行に入る前に済ませる ---
# 表の M 番号がこのスクリプトに実在するか、表が挙げた test 名が本当に存在するかの両方を見る。
# 名前だけ書いて test を書いていない行は、これでしか塞げない
KILLERS="$BAKDIR/killers.tsv"
python3 - "$0" "$TASKS" "$KILLERS" <<'COVERAGE' || exit 1
import pathlib, re, sys
script, tasks = pathlib.Path(sys.argv[1]).read_text(), pathlib.Path(sys.argv[2]).read_text()
rows = re.findall(r"^\| (M\d+b?) \| [^|]+ \| ([^|]+) \|", tasks, re.M)
table = {mid for mid, _ in rows}
in_script = set(re.findall(r"&& run '(M\d+b?):", script)) | set(re.findall(r"&& run_custom '(M\d+b?):", script))
bad = []
# 件数だけは直書きにする。下の 2 つ（表→script・script→表）は片側の消し忘れしか捕まえず、
# **表の行と実変異を同時に消した**変異表の縮小を通してしまう。数え上げにすると、
# 減った件数がそのまま新しい正解になる
if len(table) != 144:
    bad.append(f"変異表の行が {len(table)} 件（144 件でない）")
for missing in sorted(table - in_script):
    bad.append(f"{missing}: 表にあるが mutate.sh に実変異が無い")
for extra in sorted(in_script - table):
    bad.append(f"{extra}: mutate.sh にあるが変異表に行が無い")
for mid, cell in rows:
    if cell.strip().startswith("custom:"):
        continue
    names = re.findall(r"`([\w.-]+\.test\.(?:ts|mjs))::([^`]+)`", cell)
    if not names:
        bad.append(f"{mid}: 殺す test を `file::name` の形で書いていない")
    for f, name in names:
        path = pathlib.Path("harness/evidence") / f
        if not path.exists():
            bad.append(f"{mid}: {f} が無い")
        elif f'"{name}"' not in path.read_text():
            bad.append(f"{mid}: {f} に test \"{name}\" が無い")
if bad:
    print("変異表の突き合わせ失敗:", file=sys.stderr)
    for b in bad:
        print(f"  - {b}", file=sys.stderr)
    raise SystemExit(1)
# 各変異が「表で名指しした test」を落としたかを run() が照合できるように書き出す。
# suite 全体で何かが落ちたことしか見ないと、別の test が落ちただけの変異が kill に計上される
with open(sys.argv[3], "w") as fh:
    for mid, cell in rows:
        for _f, name in re.findall(r"`([\w.-]+\.test\.(?:ts|mjs))::([^`]+)`", cell):
            fh.write(f"{mid}\t{name}\n")
print(f"変異表 {len(table)} 件と mutate.sh の実変異が一致し、挙げた test 名もすべて実在する")
COVERAGE

EXECUTED=0
SURVIVED=0
BASELINE_OUT=$(node --experimental-strip-types --test "${TESTS[@]}" 2>&1)
BASELINE_TESTS=$(printf '%s\n' "$BASELINE_OUT" | grep -E '^# tests |^ℹ tests ' | tail -1 | grep -oE '[0-9]+$')
if [ -z "${BASELINE_TESTS:-}" ]; then
  echo "変異テスト失敗: baseline の test 件数を取得できない" >&2
  exit 1
fi
# **当てる前に** green を要求する。赤い状態から始めると、どの変異でも fail > 0 になって
# 全件が「殺した」に化ける——1 つも検査していないのにゲートだけが緑になる。
# CI は step の順序で守られているが、この script は単体でも回す（先頭の使い方参照）
BASELINE_FAIL_BEFORE=$(printf '%s\n' "$BASELINE_OUT" | grep -E '^# fail |^ℹ fail ' | tail -1 | grep -oE '[0-9]+$')
if [ -z "${BASELINE_FAIL_BEFORE:-}" ] || [ "$BASELINE_FAIL_BEFORE" -ne 0 ]; then
  echo "変異テスト失敗: 変異を当てる前の baseline が green でない（この状態では全変異が kill に化ける）" >&2
  exit 1
fi

run() {
  local label="$1" out failed n ran mid killed=1
  out=$(node --experimental-strip-types --test "${TESTS[@]}" 2>&1)
  failed=$(printf '%s' "$out" | grep -E '^# fail |^ℹ fail ' | tail -1)
  n=$(printf '%s' "$failed" | grep -oE '[0-9]+$')
  # 走った件数も見る。変異でソースが parse できないと node:test は「読み込みに失敗した 1 件」を
  # fail として数えるので、fail 件数だけを見るとゲートを一度も壊していない変異が kill として計上される
  ran=$(printf '%s' "$out" | grep -E '^# tests |^ℹ tests ' | tail -1 | grep -oE '[0-9]+$')
  printf '%-52s %s\n' "$label" "${failed:-<test が走らなかった>}"
  EXECUTED=$((EXECUTED + 1))
  if [ -z "$n" ] || [ "$n" -eq 0 ]; then
    killed=0
  elif [ -z "$ran" ] || [ "$ran" -ne "$BASELINE_TESTS" ]; then
    printf '  ^ 変異が test を走らせていない（tests %s / baseline %s）。ゲート未検証\n' "${ran:-?}" "$BASELINE_TESTS"
    killed=0
  else
    # 「suite の何かが落ちた」では足りない。表が名指しした test 自身が落ちたことを見る。
    # そうしないと、別の test が落ちただけの変異が kill として計上され、表の割当が嘘になる
    mid=${label%%:*}
    while IFS=$'\t' read -r row_mid name; do
      [ "$row_mid" = "$mid" ] || continue
      if ! printf '%s' "$out" | grep -Fq "✖ $name ("; then
        printf '  ^ 表が名指しした test が落ちていない: %s\n' "$name"
        killed=0
      fi
    done < "$KILLERS"
  fi
  [ "$killed" -eq 1 ] || SURVIVED=$((SURVIVED + 1))
  restore_all || exit 1
}

# node:test では殺せない変異のための口。殺すのは別のコマンドの終了状態
run_custom() {
  local label="$1"; shift
  EXECUTED=$((EXECUTED + 1))
  if "$@" >/dev/null 2>&1; then
    printf '%-52s ℹ fail 0\n' "$label"
    SURVIVED=$((SURVIVED + 1))
  else
    printf '%-52s ℹ fail 1 (custom)\n' "$label"
  fi
  restore_all || exit 1
}

mutate() { # file old new
  python3 - "$1" "$2" "$3" <<'PY'
import sys, pathlib
target, old, new = sys.argv[1], sys.argv[2], sys.argv[3]
# `\n` の変換はしない。この表には TS ソース中のリテラル `\n`（文字列内の改行エスケープ）を
# 含むアンカーがあり、変換すると本物の改行になって module が壊れる。改行はそのまま書く
p = pathlib.Path(target)
s = p.read_text()
# アンカーはソース中で一意でなければならない。2 箇所に出ると replace(old, new, 1) は必ず
# 先頭を書き換えるので、2 つ目を狙ったラベルが 1 つ目を壊すだけになる
count = s.count(old)
assert count == 1, f"anchor must be unique in {target} (found {count}): {old[:70]}"
p.write_text(s.replace(old, new, 1))
PY
  local py_rc=$?
  [ "$py_rc" -eq 0 ] || return "$py_rc"
  # 当てた後の file が言語として読めることを確かめる。構文を壊す変異は「保護を外した」ではなく
  # 「file を壊した」で、落ちるのは保護を迂回できたからではない。test が子 process として起動する
  # 対象（.mjs / .sh）では node:test の件数照合が働かないので、ここで見るしかない
  case "$1" in
    *.mjs|*.js) node --check "$1" ;;
    *.sh) bash -n "$1" ;;
    *.json) python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$1" ;;
    *) return 0 ;;
  esac || { echo "変異テスト失敗: 変異が $1 の構文を壊した（この変異は保護を検査していない）" >&2; exit 1; }
}

# contract-hashes の変異は node:test では殺せない。再生成との diff で殺す
# 生成が落ちた場合を「差分あり」と同じ非ゼロで返さない。run_custom は非ゼロを kill と数えるので、
# node の異常終了がそのまま「変異を殺した」になり、検査していないのに緑になる
check_hashes() {
  local out
  out=$(node harness/contract-hashes.mjs) || return 0
  # `$( )` は末尾改行を落とすので書き戻す（落としたままだと常に「差分あり」になる）
  printf '%s\n' "$out" | diff - "$HASHES"
}

mutate $ASSEMBLE 'promotion.evidenceKind === "real-cli-e2e" && prev.evidenceKind !== "real-cli-e2e";' 'false;' && run 'M0: 証跡の優劣を無視する'
mutate $ASSEMBLE 'for (const f of fixtures) verifiedByFixture.set(f.fixtureId, verifyEvidence(f, ctx));' 'for (const f of fixtures) verifiedByFixture.set(f.fixtureId, []);' && run 'M1: 証拠の検証を丸ごと飛ばす'
mutate $VERIFY 'if (evidenceHash !== ref.evidenceHash) {' 'if (false) {' && run 'M2: evidenceHash の不一致を通す'
mutate $VERIFY 'if (ref.normalizationVersion !== NORMALIZATION_VERSION) {' 'if (false) {' && run 'M3: 未知の正規化版を通す'
mutate $VERIFY '  "subagentCapture",' '  "subagentCapture",
  "promptAwareInjection",' && run 'M4: 導けない高位主張を導けることにする'
mutate $NORMALIZE 'if (relPath.split(/[\\/]/).includes("..")) fail(' 'if (false) fail(' && run 'M5: 相対 path の .. を通す'
mutate $NORMALIZE 'candidate = realpathSync(join(realRoot, relPath));' 'candidate = join(realRoot, relPath);' && run 'M6: 候補 path を realpath せず symlink を追わない'
mutate $NORMALIZE '  "prompt",
]);' ']);' && run 'M7: prompt を verbatim 集合から外す'
mutate $NORMALIZE 'if (key === "at") continue; // 時刻は取得のたびに変わる' 'if (false) continue; // 時刻は取得のたびに変わる' && run 'M8: at を落とさない'
mutate $NORMALIZE 'if (typeof value === "string") return value === "" ? "<string:empty>" : "<string>";' 'if (typeof value === "string") return value === "" ? "<string:empty>" : value;' && run 'M8b: 深い階層の文字列まで verbatim にする'
mutate $NORMALIZE '    out[key] = redact(value);
  }
  return out;
}' '    if (VERBATIM_PAYLOAD_KEYS.has(key) || ID_KEYS.has(key) || PATH_KEYS.has(key)) out[key] = redact(value);
  }
  return out;
}' && run 'M9: 未知の欄を落とす'
mutate $NORMALIZE 'return `${lines.join("\n")}\n`;' 'return lines.join("\n");' && run 'M10: 最終行の後の LF を落とす'
mutate $NORMALIZE '    for (const item of value) out.push(redact(item));
    return out;' '    for (const item of value) out.push(redact(item));
    return out.slice(0, 1);' && run 'M11: 配列の長さを保たない'
mutate $NORMALIZE 'if (lines.length === 0) fail("capture has 0 usable lines");' 'if (false) fail("capture has 0 usable lines");' && run 'M12: 空の観測記録を通す'
mutate $SCHEMA '    "evidence": {
      "$comment": "実 CLI 観測' '    "evidenceDISABLED": {
      "$comment": "実 CLI 観測' && run 'M13: schema から evidence の定義を落とす'
mutate $VERIFY 'reject(f.fixtureId, `captureRawHash mismatch for ${ref.path}`);' 'reject(f.fixtureId, `captureRawHash mismatch for ${capturePath}`);' && run 'M14: 失敗の説明に絶対 path を載せる'
mutate $SCHEMA '      "minItems": 1,
      "items": {
        "type": "object",
        "additionalProperties": false,' '      "minItems": 0,
      "items": {
        "type": "object",
        "additionalProperties": false,' && run 'M15: schema の minItems を外す'
mutate $NORMALIZE '      if (ID_KEYS.has(key)) {
        out[key] = tokens.id(value);' '      if (ID_KEYS.has(key)) {
        out[key] = "<id>";' && run 'M16: 識別子の等値関係を捨てる'
mutate $NORMALIZE 'const seen = { id: new Map<string, string>(), path: new Map<string, string>() };' 'const shared = new Map<string, string>();
  const seen = { id: shared, path: shared };' && run 'M17: id と path の番号空間を共有する'
mutate $ASSEMBLE '        if (!observed.has(name)) {' '        if (false) {' && run 'M18: 申告した hook の実在を確かめない'
mutate $ASSEMBLE 'export const shareRef = (a: number[], b: number[]): boolean => a.some((i) => b.includes(i));' 'export const shareRef = (a: number[], b: number[]): boolean => a.length > 0 || b.length > 0;' && run 'M19: 対の成立に共有記録を求めない'
mutate $VERIFY '["captureHash", manifest.captureHash === computed.evidenceHash],' '["captureHash", true],' && run 'M20: manifest の captureHash 照合を外す'
mutate $NORMALIZE '      parsed = parseIJson(rawLine);' '      parsed = JSON.parse(rawLine);' && run 'M21: 重複キーを持つ行を通す'
mutate $NORMALIZE 'if (Object.hasOwn(parsed.payload, "unparsed")) {' 'if (false) {' && run 'M22: payload.unparsed を通す'
mutate $NORMALIZE '  const out = Object.create(null) as { [k: string]: JsonValue };
  // 整列した後の順で走る' '  const out = {} as { [k: string]: JsonValue };
  // 整列した後の順で走る' && run 'M23: 中間 object を素の {} で作る'
mutate $VERIFY '  "cwd",
  "transcript_path",' '  "transcript_path",' && run 'M24: 警報の材料から cwd を外す'
mutate $ASSEMBLE 'const supporting = refs.filter((r) => derive(r).value === declared);' 'const supporting = refs.slice();' && run 'M26: 申告値と導出値の照合を外す'
mutate $ASSEMBLE '    if (!derivable || refs.length === 0) {' '    if (refs.length === 0) {' && run 'M27: 導けない主張にも導出を求める'
mutate $ASSEMBLE 'evidenceKind: backed.length > 0 ? "real-cli-e2e" : "source-test",' 'evidenceKind: "real-cli-e2e",' && run 'M28: manifest 無しでも real-cli-e2e にする'
mutate $VERIFY '    if (captureRawHash !== ref.captureRawHash) {' '    if (false) {' && run 'M29: captureRawHash の照合を外す'
mutate $NORMALIZE '  for (const key of Object.keys(payload).sort(byUtf16)) {' '  for (const key of Object.keys(payload)) {' && run 'M30: payload の走査を整列前の書き順にする'
mutate $NORMALIZE '  if (!ArrayBuffer.isView(bytes)) {
    fail("normalizeCapture takes raw bytes (Uint8Array), not a decoded string");
  }' '  if (false) {
    fail("normalizeCapture takes raw bytes (Uint8Array), not a decoded string");
  }' && run 'M31: 復号済み文字列も受け取る'
mutate $ASSEMBLE '  capabilities.capabilityHashInputs = [
    canonicalizeJson({ cli, nativeVersion }),' '  capabilities.capabilityHashInputs = [
    [cli, nativeVersion].join("@") +
    canonicalizeJson({ cli, nativeVersion }).slice(0, 0),' && run 'M32: hash 入力を 1 本の連結文字列に戻す'
mutate $ASSEMBLE 'for (const c of f.limitationCodes ?? []) fixtureLimitations.push(`[${f.fixtureId}] ${c}`);' 'for (const c of f.limitationCodes ?? []) fixtureLimitations.push(`[${f.scenario}] ${c}`);' && run 'M33: 自由文の scenario を成果物へ載せる'
mutate $VERIFY '    const captureRawHash = digestRaw(bytes);
    if (captureRawHash !== ref.captureRawHash) {' '    const captureRawHash = digestRaw(bytes);
    if (ref.manifest !== undefined && captureRawHash !== ref.captureRawHash) {' && run 'M34: legacy ref では生 byte を照合しない'
mutate $ASSEMBLE '    const backed = supporting.filter(' '    const backed = refs.filter(' && run 'M35: 支持しない記録の manifest でも昇格させる'
mutate $ASSEMBLE 'const supporting = refs.filter((r) => derive(r).value === declared);' 'const supporting = refs.every((r) => derive(r).value === declared) ? refs.slice() : [];' && run 'M36: 全 ref の一致を要求する'
mutate $VERIFY '  if (cli === "codex") {
    if (shares("turn_id")) derived("turn_completed", "native", ["UserPromptSubmit", "Stop"]);' '  if (false) {
    if (shares("turn_id")) derived("turn_completed", "native", ["UserPromptSubmit", "Stop"]);' && run 'M37: Codex の turn 規則を捨てる'
mutate $VERIFY '["internalRunMarker", manifest.internalRunMarker === true],' '["internalRunMarker", manifest.internalRunMarker === f.rig.internalRunMarker],' && run 'M38: internalRunMarker を fixture との一致で見る'
mutate $IMPORT 'die("the CLI printed more than one line for --version");' 'void 0;' && run 'M39: 複数行の CLI 版を黙って受け取る'
mutate $SCHEMA '            "then": {
              "required": [
                "manifestHash"
              ]
            }' '            "then": {
              "required": [
                "path"
              ]
            }' && run 'M40: manifest 対の要求を別の欄へ向ける'
mutate $ASSEMBLE 'const SECRET_WINDOW = 16;' 'const SECRET_WINDOW = 4096;' && run 'M41: 警報の窓を実質無効な幅へ広げる'
mutate $ASSEMBLE 'for (const c of f.limitationCodes ?? []) fixtureLimitations.push(`[${f.fixtureId}] ${c}`);' 'for (const c of f.limitations ?? []) fixtureLimitations.push(`[${f.fixtureId}] ${c}`);' && run 'M42: 散文の limitations を成果物へ転記する'
mutate $SCHEMA '          "assistant-completion-synthesized-from-stop",
          "codex-home-in-tmp-warns",' '          "assistant-completion-synthesized-from-stop",
          "nope",
          "codex-home-in-tmp-warns",' && run 'M43: 限界コードの enum を緩める'
mutate $SCHEMA '  "title": "CaptureFixture",' '  "title": "CaptureFixture",
  "maxProperties": 500,' && run 'M44: validate.ts が解釈しない keyword を足す'
mutate $VERIFY '    const captureRawHash = digestRaw(bytes);' '    const captureRawHash = ref.captureRawHash;' && run 'M45: 生 byte の digest を申告値で代用する'
mutate $VERIFY '  if (digestRaw(bytes) !== ref.manifestHash) {' '  if (false) {' && run 'M46: manifest を parse する前の照合を外す'
mutate $ASSEMBLE '    if (!derivable || refs.length === 0) {' '    if (!derivable) throw new Error("underivable claim");
    if (refs.length === 0) {' && run 'M47: 導けない主張で組み立てを落とす'
mutate $NORMALIZE 'realRoot = realpathSync(root ?? defaultEvidenceRoot(cli));' 'realRoot = realpathSync(defaultEvidenceRoot(cli));' && run 'M48: root の差し替え口を無視する'
mutate $ASSEMBLE '  const assembled = assembleFromFixtures(fixtures);' '  const assembled = assembleFromFixtures(fixtures, { evidenceRoot: process.env.EVIDENCE_ROOT });' && run 'M49: 組み立ての入口で root を環境変数から取る'
mutate $ASSEMBLE '  for (const issue of validateAgainstSchema(data, SCHEMA, SCHEMA)) {' '  for (const issue of validateAgainstSchema(data.observedEvents ?? [], SCHEMA.properties?.observedEvents, SCHEMA, "observedEvents")) {' && run 'M50: fixture 全体ではなく欄を選んで検査する'
mutate $NORMALIZE '    if (typeof value === "string" && value !== "") {' '    if (typeof value === "string") {' && run 'M60: 空文字にも相関 token を振る'
mutate $VERIFY '  } else if (shares("prompt_id") && !lines.some((l) => tokenOf(l, "turn_id") !== undefined)) {' '  } else if (shares("prompt_id") && !lines.some((l) => has(l, "turn_id"))) {' && run 'M59: turn_id の在不在を伏せ字の綴りで見る'
mutate $VERIFY 'const has = (line: NormalizedLine, key: string): boolean => line.payload[key] === "<string>";' 'const has = (line: NormalizedLine, key: string): boolean => Object.hasOwn(line.payload, key);' && run 'M56: 欄の有無を型を見ずに判定する'
mutate $ASSEMBLE '    canonicalizeJson(evidenceSources),' '    canonicalizeJson(evidenceSources.map((e) => [e.fixtureId, e.path, e.evidenceHash])),' && run 'M57: hash の入力で欄を数え上げる'
mutate $ASSEMBLE '    if (!data.fixtureId.startsWith(`${data.cli}/`)) errs.push("fixtureId must be prefixed with its own cli");' '    if (!data.fixtureId.startsWith(`${data.cli}/`)) errs.push(`fixtureId must be prefixed with ${data.fixtureId}`);' && run 'M58: 診断へ fixture の生値を混ぜる'
mutate $VERIFY '  return typeof v === "string" && CORRELATION_TOKEN.test(v) ? v : undefined;' '  return typeof v === "string" ? v : undefined;' && run 'M53: 伏せ字の綴りを相関 token として受け取る'
mutate $VERIFY '    if (seenPaths.has(ref.path)) reject(f.fixtureId, `evidence names ${ref.path} more than once`);' '    if (false) reject(f.fixtureId, `evidence names ${ref.path} more than once`);' && run 'M54: 同じ記録の重複を通す'
mutate $ASSEMBLE '    if (!data.fixtureId.startsWith(`${data.cli}/`)) errs.push("fixtureId must be prefixed with its own cli");' '    if (false) errs.push("fixtureId must be prefixed with its own cli");' && run 'M55: fixtureId の帰属を確かめない'
mutate $ASSEMBLE '    if (typeof value === "string" && !isRealInstant(value)) {' '    if (false) {' && run 'M52: 暦の検査を外す'
mutate $VERIFY '  if (sessionTokens.length >= 2 && new Set(sessionTokens).size === 1 && sessionTokens[0] !== undefined) {' '  if (new Set(sessionTokens.filter((t) => t !== undefined)).size === 1) {' && run 'M51: 欄の無い行を除いてから安定性を見る'
mutate $VERIFY '    ["capturedAt", manifest.capturedAt === computed.capturedAt],' '    ["capturedAt", manifest.capturedAt === f.capturedAt],' && run 'M61: 記録の時刻を fixture 単位で縛る'
mutate $VERIFY '    const capturedAt = captureCapturedAt(bytes);' '    const capturedAt = (ref as unknown as { capturedAt: string }).capturedAt ?? f.capturedAt;' && run 'M62: 時刻を記録から導かず申告から取る'
mutate $ASSEMBLE '      (r) => r.manifestBacked && claimedEvents.every((n) => derive(r).sources.includes(n)),' '      (r) => r.manifestBacked,' && run 'M63: 申告 hook 名を持たない記録で昇格させる'
mutate $ASSEMBLE '        verifiedAt: promotion.verifiedAt ?? f.capturedAt,' '        verifiedAt: f.capturedAt,' && run 'M64: 公開する時刻を fixture の申告から取る'
mutate $ASSEMBLE '    return x >= y ? a : b;' '    return a > b ? a : b;' && run 'M67: 遅いほうの判定を文字列比較へ戻す'
mutate $NORMALIZE '  if (!isRealInstant(parsed.at)) fail(' '  if (false && !isRealInstant(parsed.at)) fail(' && run 'M68: 暦に無い日付を記録の時刻として通す'
mutate $ASSEMBLE '        ...backedOnly(promotion.evidenceKind === "real-cli-e2e", ev.sourceEvents ?? []),' '        ...(ev.sourceEvents ?? []),' && run 'M69: 裏付けの無い fixture の hook 名を統合する'
mutate $IMPORT 'const sourceBytes = readFileSync(source);' 'mkdirSync(destDir, { recursive: true });
copyFileSync(source, dest);
const sourceBytes = readFileSync(source);' && run 'M70: 検証より先に保存済みの記録を置き換える'
mutate $RIG '  [ "$ver_rc" -eq 0 ] || { rm -f "$stem.version.new" "$stem.version.err.new"; echo "claude --version failed (exit=$ver_rc)" >&2; exit 1; }
  # ここから前の記録を置き換える。記録失敗の痕跡も run ごとに消す。残すと前回の失敗が今回の
  # manifest の recorderErrors に載り、正しい証拠が棄却される。終了コードも同じ理由で消す:
  # run が SIGKILL で落ちると前回の成功が残り、途中で切れた記録に exitStatus=0 が付く。
  # .errors だけは hook が $CAPTURE_FILE から作るので記録側の名前になる
  # 消すのが先、公開が後。逆にすると、その間に落ちた瞬間だけ「前の記録 + 新しい版」が揃い、
  # 取り込みが**前の run の記録を新しい版で測ったこと**にできる。この順なら、途中で落ちても
  # 記録が空で終了コードも無い＝取り込みが必ず断る
  : > "$capture"; rm -f "$capture.errors" "$stem.exit"' '  [ "$ver_rc" -eq 0 ] || { rm -f "$stem.version.new" "$stem.version.err.new"; echo "claude --version failed (exit=$ver_rc)" >&2; exit 1; }
  # ここから前の記録を置き換える。記録失敗の痕跡も run ごとに消す。残すと前回の失敗が今回の
  # manifest の recorderErrors に載り、正しい証拠が棄却される。終了コードも同じ理由で消す:
  # run が SIGKILL で落ちると前回の成功が残り、途中で切れた記録に exitStatus=0 が付く。
  # .errors だけは hook が $CAPTURE_FILE から作るので記録側の名前になる
  # 消すのが先、公開が後。逆にすると、その間に落ちた瞬間だけ「前の記録 + 新しい版」が揃い、
  # 取り込みが**前の run の記録を新しい版で測ったこと**にできる。この順なら、途中で落ちても
  # 記録が空で終了コードも無い＝取り込みが必ず断る
  : > "$capture"; rm -f "$capture.errors"' && run 'M71: claude の run で前回の終了コードを残す'
mutate $RIG '  [ "$ver_rc" -eq 0 ] || { rm -f "$stem.version.new" "$stem.version.err.new"; echo "codex --version failed (exit=$ver_rc)" >&2; exit 1; }
  # ここから前の記録を置き換える。記録失敗の痕跡も run ごとに消す。残すと前回の失敗が今回の
  # manifest の recorderErrors に載り、正しい証拠が棄却される。終了コードも同じ理由で消す:
  # run が SIGKILL で落ちると前回の成功が残り、途中で切れた記録に exitStatus=0 が付く。
  # .errors だけは hook が $CAPTURE_FILE から作るので記録側の名前になる
  # 消すのが先、公開が後。逆にすると、その間に落ちた瞬間だけ「前の記録 + 新しい版」が揃い、
  # 取り込みが**前の run の記録を新しい版で測ったこと**にできる。この順なら、途中で落ちても
  # 記録が空で終了コードも無い＝取り込みが必ず断る
  : > "$capture"; rm -f "$capture.errors" "$stem.exit"' '  [ "$ver_rc" -eq 0 ] || { rm -f "$stem.version.new" "$stem.version.err.new"; echo "codex --version failed (exit=$ver_rc)" >&2; exit 1; }
  # ここから前の記録を置き換える。記録失敗の痕跡も run ごとに消す。残すと前回の失敗が今回の
  # manifest の recorderErrors に載り、正しい証拠が棄却される。終了コードも同じ理由で消す:
  # run が SIGKILL で落ちると前回の成功が残り、途中で切れた記録に exitStatus=0 が付く。
  # .errors だけは hook が $CAPTURE_FILE から作るので記録側の名前になる
  # 消すのが先、公開が後。逆にすると、その間に落ちた瞬間だけ「前の記録 + 新しい版」が揃い、
  # 取り込みが**前の run の記録を新しい版で測ったこと**にできる。この順なら、途中で落ちても
  # 記録が空で終了コードも無い＝取り込みが必ず断る
  : > "$capture"; rm -f "$capture.errors"' && run 'M72: codex の run で前回の終了コードを残す'
mutate $ASSEMBLE '      (r) => r.manifestBacked && claimedEvents.every((n) => derive(r).sources.includes(n)),' '      (r) => r.manifestBacked && claimedEvents.every((n) => r.events.includes(n)),' && run 'M73: 出どころを「記録に在る」だけで認める'
mutate $ASSEMBLE '        errs.push(`observedEvents[${i}].kind is not one of the kinds capability.schema.json lists`);' '        errs.push(`observedEvents[${i}].kind invalid: ${ev.kind}`);' && run 'M74: 手書き検証が棄却した値を診断へ戻す'
mutate $SCHEMAV '        issues.push({ path, message: `unknown property #${ordinal}` });' '        issues.push({ path, message: `unknown property: ${k}` });' && run 'M75: schema 検証が未知 key 名を診断へ載せる'
mutate $MATRIX '  "generatedAt"' '  "smuggled": "real-cli-\u0065\u0032e",
  "generatedAt"' && run 'M76: 出荷 matrix の real-cli-e2e を escape で綴る'
mutate $MATRIX '  "fixtureCount"' '  "cli": "smuggled",
  "fixtureCount"' && run 'M77: 出荷 matrix へ重複キーを紛れ込ませる'
mutate $FIXTURE '      "normalizationVersion": 1' '      "normalizationVersion": 1,
      "manifest": "anything.json",
      "manifestHash": "0000000000000000000000000000000000000000000000000000000000000000"' && run 'M66: 出荷 fixture から manifest を名指しする'
mutate $SCHEMAV '    issues.push({ path, message: "value not in enum" });' '    issues.push({ path, message: `value not in enum: ${JSON.stringify(value)}` });' && run 'M65: 棄却した値を診断へ戻す'
mutate $ASSEMBLE '    if (!KNOWN_KEYS.has(k)) errs.push(`unknown top-level key #${n + 1} (capability.schema.json 未定義)`);' '    if (!KNOWN_KEYS.has(k)) errs.push(`unknown top-level key ${k} (capability.schema.json 未定義)`);' && run 'M78: 手書き検証が未知 key 名を診断へ載せる'
mutate $MATRIX '"generatedAt": "' '"generatedAt": "/home/private/CANARY", "notGeneratedAt": "' && run 'M79: 出荷 matrix の生成時刻を別の値に差し替える'
mutate $RIG '  wait "$run_pid" || rc=$?
  reap_group "$run_pid"
  # 終了コードは数値で別に残す。manifest の exitStatus はここから読む
  printf '"'"'%s\n'"'"' "$rc" > "$stem.exit"
  [ "$rc" -eq 0 ] || echo "exit=$rc (recorded)" >> "$stem.stderr"
  echo "captured: ${capture##*/} ($(wc -l < "$capture") events)"
}

codex_run' '  wait "$run_pid" || rc=$?
  # 終了コードは数値で別に残す。manifest の exitStatus はここから読む
  printf '"'"'%s\n'"'"' "$rc" > "$stem.exit"
  [ "$rc" -eq 0 ] || echo "exit=$rc (recorded)" >> "$stem.stderr"
  echo "captured: ${capture##*/} ($(wc -l < "$capture") events)"
}

codex_run' && run 'M80: claude の run が残した子を畳まない'
mutate $RIG '  wait "$run_pid" || rc=$?
  reap_group "$run_pid"
  # 終了コードは数値で別に残す。manifest の exitStatus はここから読む
  printf '"'"'%s\n'"'"' "$rc" > "$stem.exit"
  [ "$rc" -eq 0 ] || echo "exit=$rc (recorded)" >> "$stem.stderr"
  echo "captured: ${capture##*/} ($(wc -l < "$capture") events)"
}

# 証拠置き場へ' '  wait "$run_pid" || rc=$?
  # 終了コードは数値で別に残す。manifest の exitStatus はここから読む
  printf '"'"'%s\n'"'"' "$rc" > "$stem.exit"
  [ "$rc" -eq 0 ] || echo "exit=$rc (recorded)" >> "$stem.stderr"
  echo "captured: ${capture##*/} ($(wc -l < "$capture") events)"
}

# 証拠置き場へ' && run 'M81: codex の run が残した子を畳まない'
mutate $IMPORT 'if (issues.length > 0) die(' 'if (false && issues.length > 0) die(' && run 'M82: schema を満たさない manifest でも記録を置き換える'
mutate $IMPORT 'if (manifest.recorderErrors !== 0) die(' 'if (false && manifest.recorderErrors !== 0) die(' && run 'M83: 記録器のエラーが残ったまま持ち込む'
mutate $ASSEMBLE '        ...backedOnly(prev.evidenceKind === "real-cli-e2e", prev.value === "unknown" ? [] : prev.sourceEvents),' '        ...(prev.value === "unknown" ? [] : prev.sourceEvents),' && run 'M84: 先に見た側の裏付け無し hook 名を統合する'
mutate $ASSEMBLE '        derivable && o.value === "native",' '        derivable,' && run 'M85: 高位 cell の導出可否を key だけで決める'
mutate $MSCHEMA '(\\.\\d{1,3})?Z$' '(\\.\\d+)?Z$' && run 'M86: manifest の時刻に ms より細かい桁を許す'
mutate $RIG '  wait "$ver_pid" || ver_rc=$?
  reap_group "$ver_pid"
  # 版として読むのは stdout だけ。混ぜると、stdout に何も出さず stderr に 1 行だけ出して
  # 終了 0 で帰る CLI で、その診断文が cliVersion として証拠に載る
  # 問い合わせの記録も state も持ち込みの対象にしない。中身も残さない
  rm -rf "$ver_state"
  rm -f "$ver_capture" "$ver_capture.errors"
  # 落ちた問い合わせで前の run の記録を失わない。降りる前に消すのは今回書いたものだけで、
  # 取り込み前の記録・その終了コード・記録失敗の痕跡はそのまま残す（測定は始まってもいない）
  [ "$ver_rc" -eq 0 ] || { rm -f "$stem.version.new" "$stem.version.err.new"; echo "claude --version failed' '  wait "$ver_pid" || ver_rc=$?
  # 版として読むのは stdout だけ。混ぜると、stdout に何も出さず stderr に 1 行だけ出して
  # 終了 0 で帰る CLI で、その診断文が cliVersion として証拠に載る
  # 問い合わせの記録も state も持ち込みの対象にしない。中身も残さない
  rm -rf "$ver_state"
  rm -f "$ver_capture" "$ver_capture.errors"
  # 落ちた問い合わせで前の run の記録を失わない。降りる前に消すのは今回書いたものだけで、
  # 取り込み前の記録・その終了コード・記録失敗の痕跡はそのまま残す（測定は始まってもいない）
  [ "$ver_rc" -eq 0 ] || { rm -f "$stem.version.new" "$stem.version.err.new"; echo "claude --version failed' && run 'M88: 版の問い合わせが残した子を畳まない'
mutate $RIG '      > "$stem.stdout" 2> "$stem.stderr" ) & run_pid=$!
  set +m
  wait "$run_pid" || rc=$?
  reap_group "$run_pid"
  # 終了コードは数値で別に残す。manifest の exitStatus はここから読む
  printf '"'"'%s\n'"'"' "$rc" > "$stem.exit"
  [ "$rc" -eq 0 ] || echo "exit=$rc (recorded)" >> "$stem.stderr"
  echo "captured: ${capture##*/} ($(wc -l < "$capture") events)"
}

codex_run' '      > "$stem.stdout" 2> "$stem.stderr" ) 9>&- & run_pid=$!
  set +m
  wait "$run_pid" || rc=$?
  reap_group "$run_pid"
  # 終了コードは数値で別に残す。manifest の exitStatus はここから読む
  printf '"'"'%s\n'"'"' "$rc" > "$stem.exit"
  [ "$rc" -eq 0 ] || echo "exit=$rc (recorded)" >> "$stem.stderr"
  echo "captured: ${capture##*/} ($(wc -l < "$capture") events)"
}

codex_run' && run 'M89: lock の fd を測定対象へ渡さない'
mutate $RIG '  ( cd "$ver_state/workspace" && RIG_BASE="$ver_state" run_env claude "$ver_capture" \
      timeout --foreground --kill-after="${VERSION_KILL_AFTER:-5s}" "${VERSION_TIMEOUT:-60}" "$CLAUDE_BIN" --version )' '( { "$CLAUDE_BIN" --version; } )' && run 'M93: 版の問い合わせを隔離の外で行う'
mutate $RIG '[ "$ver_rc" -eq 0 ] || { rm -f "$stem.version.new" "$stem.version.err.new"; echo "claude --version failed (exit=$ver_rc)" >&2; exit 1; }' '[ "$ver_rc" -eq 0 ] || true' && run 'M94: 失敗した版の問い合わせでも測定を続ける'
mutate $CAP '    Array.isArray(cell.evidenceRefs) &&
    cell.evidenceRefs.length > 0' '    true' && run 'M95: 裏付けた記録が無くても証明済みとする'
mutate $IMPORT 'try {
  writeFileSync(stagedManifest, `${JSON.stringify(manifest, null, 2)}\n`);
  renameSync(stagedCapture, dest);
  renameSync(stagedManifest, manifestPath);
} catch (e) {
  if (previous) renameSync(previous, dest);
  // 失敗の説明に絶対 path を出さない。file system の error message は source と destination の
  // 絶対 path を含むので、そのまま出すと CI log へ実行環境の path が流れる
  dieStaged(`import failed while staging: ${e?.constructor?.name ?? "Error"}`);
}
' 'writeFileSync(stagedManifest, `${JSON.stringify(manifest, null, 2)}\n`);
renameSync(stagedCapture, dest);
renameSync(stagedManifest, manifestPath);
' && run 'M96: 置き換えで落ちても一時 file を片付けない'
mutate $RIG '( cd "$ver_state/workspace" && RIG_BASE="$ver_state" run_env claude' '( RIG_BASE="$ver_state" run_env claude' && run 'M97: 版の問い合わせを呼び出し元の作業場所で行う'
mutate $RIG 'run_env claude "$ver_capture" \
      timeout' 'run_env claude "$capture" \
      timeout' && run 'M98: 版の問い合わせと本実行で記録先を共有する'
mutate $RIG 'reap_group() {
  kill -- "-$1" 2>/dev/null || true
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    kill -0 -- "-$1" 2>/dev/null || return 0
    sleep 0.2
  done
  kill -KILL -- "-$1" 2>/dev/null || true
}
' 'reap_group() { kill -- "-$1" 2>/dev/null || true; }
' && run 'M99: SIGTERM を無視する残骸を畳み切らない'
mutate $RIG '  teardown) require_rig_base; mkdir -p "$RIG_BASE"; chmod 700 "$RIG_BASE"; with_lock; rm -f' '  teardown) require_rig_base; rm -f' && run 'M100: teardown が lock を取らない'
mutate $RIG 'timeout --foreground --kill-after="${VERSION_KILL_AFTER:-5s}" "${VERSION_TIMEOUT:-60}" "$CLAUDE_BIN" --version' '"$CLAUDE_BIN" --version' && run 'M101: 版の問い合わせに時間制限を掛けない'
mutate $RIG 'RIG_BASE="$ver_state" run_env claude' 'run_env claude' && run 'M102: 版の問い合わせを本実行と同じ state で行う'
mutate $RIG 'purge_own_credentials() { [ "$STAGED" -eq 1 ] && purge_credentials; return 0; }' 'purge_own_credentials() { purge_credentials; return 0; }' && run 'M103: lock を取れなかった process も資格情報を消す'
mutate $RIG '"$CLAUDE_BIN" --version ) > "$stem.version.new" 2> "$stem.version.err.new"' '"$CLAUDE_BIN" --version 2>&1 ) > "$stem.version.new" 2> "$stem.version.err.new"' && run 'M105: 版の問い合わせの stderr を版として記録する'
mutate $IMPORT '  if (previous) renameSync(previous, dest);' '  if (previous) rmSync(previous, { force: true });' && run 'M106: 置き換えに失敗しても古い記録を戻さない'
mutate $IMPORT 'dieStaged(`import failed while staging: ${e?.constructor?.name ?? "Error"}`);' 'dieStaged(`import failed while staging: ${e instanceof Error ? e.message : String(e)}`);' && run 'M107: 持ち込みの失敗に file system の説明をそのまま出す'
mutate $RIG 'timeout --foreground --kill-after="${VERSION_KILL_AFTER:-5s}" "${VERSION_TIMEOUT:-60}" "$CLAUDE_BIN"' 'timeout --foreground "${VERSION_TIMEOUT:-60}" "$CLAUDE_BIN"' && run 'M108: 時間切れの問い合わせに止めの signal を送らない'
mutate $RIG 'run_env claude "$capture" timeout --foreground --kill-after="${RUN_KILL_AFTER:-5s}"' 'run_env claude "$capture" timeout --foreground' && run 'M110: 測定の時間切れに止めの signal を送らない'
mutate $RIG 'echo "another rig run holds the lock" >&2' 'echo "another rig run holds $RIG_BASE" >&2' && run 'M111: lock 競合の説明に実行環境の絶対 path を出す'
mutate $RIG '  with_lock
  # 握れた = 走っている run はいない' '  # 握れた = 走っている run はいない' && run 'M112: setup が lock を取らずに state を書き換える'
mutate $RIG 'run_env claude "$capture" timeout --foreground' 'run_env claude "$capture" timeout' && run 'M90: timeout に別の process group を作らせる'
mutate $IMPORT 'copyFileSync(source, stagedCapture);' 'copyFileSync(source, dest);
copyFileSync(source, stagedCapture);' && run 'M91: 一時 file を経ずに置き場を直接触る'
mutate $CI '            --source . --config .gitleaks.toml' '            --source . --config .gitleaks.toml --log-opts=HEAD~1..HEAD' && run 'M92: 秘密走査に範囲を持ち込む'
mutate "$HASHES" '"schema/capability.schema.json"' '"schema/capability.schema.json.moved"' \
  && run_custom 'M25: 契約 hash の入力名を書き換える' check_hashes

mutate $RIG '  teardown) require_rig_base; mkdir -p "$RIG_BASE"; chmod 700 "$RIG_BASE"; with_lock; rm -f' '  teardown) require_rig_base; [ -d "$RIG_BASE" ] && with_lock; rm -f' && run 'M113: teardown が「あれば取る」で lock を飛ばす'
mutate $RIG '  echo "rig ready"' '  echo "rig ready: $RIG_BASE"' && run 'M114: setup の報告に実行環境の絶対 path を出す'
mutate $RIG '  echo "captured: ${capture##*/} ($(wc -l < "$capture") events)"
}

codex_run' '  echo "captured: $capture ($(wc -l < "$capture") events)"
}

codex_run' && run 'M115: claude の run の報告に絶対 path を出す'
mutate $RIG '  echo "captured: ${capture##*/} ($(wc -l < "$capture") events)"
}

# 証拠置き場へ byte 同一で持ち込んでから' '  echo "captured: $capture ($(wc -l < "$capture") events)"
}

# 証拠置き場へ byte 同一で持ち込んでから' && run 'M116: codex の run の報告に絶対 path を出す'
mutate $MSCHEMA '      "minimum": 0,
      "maximum": 255' '      "minimum": 0' && run 'M117: manifest の終了コードに上限を求めない'
mutate $RIG '    *) echo "run_env: unknown cli" >&2; exit 2 ;;' '    *) : ;;' && run 'M118: 知らない cli を隔離設定なしで起動する'

mutate $VERIFY '      if (line.payload["hook_event_name"] !== line.event) {' '      if (false) {' && run 'M119: hook の名乗りを CLI の payload で裏取りしない'
mutate $RIG '    git_iso init -q --template=' '    git_iso init -q' && run 'M121: workspace の作成に git の既定 template を許す'
mutate $RIG '  [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || { echo "label must be a plain file-name token" >&2; exit 2; }' '  :' && run 'M122: label の綴りを見ない'
mutate $RIG '  require_label "$label"
  with_lock
  [ -n "$CODEX_BIN" ]' '  with_lock
  [ -n "$CODEX_BIN" ]' && run 'M123: codex の run だけ label を見ない'

mutate $RIG 'timeout --foreground --kill-after="${VERSION_KILL_AFTER:-5s}" "${VERSION_TIMEOUT:-60}" "$CLAUDE_BIN" --version' 'timeout --kill-after="${VERSION_KILL_AFTER:-5s}" "${VERSION_TIMEOUT:-60}" "$CLAUDE_BIN" --version' && run 'M124: 版の問い合わせの timeout に別の process group を作らせる'
mutate $IMPORT 'process.on("uncaughtException", (e) => {
  cleanupStaged();' 'process.on("uncaughtException", (e) => {' && run 'M125: 未捕捉例外の経路が一時 file を証拠置き場に残す'
mutate $RIG '    ${INJECT_MARKER:+INJECT_MARKER="$INJECT_MARKER"} \' '    ${INJECT_MARKER:+INJECT_MARKER=$INJECT_MARKER} \' && run 'M126: knob の値の引用を外す'

mutate $RIG '    AGENT_MEMORY_INTERNAL_RUN=1 \
    GIT_CONFIG_NOSYSTEM=1 \' '    AGENT_MEMORY_INTERNAL_RUN=1 \' && run 'M127: 測定対象の環境で system の git 設定を遮断しない'
mutate $RIG '  purge_credentials
  sed "s|__HOOK__|$HOOK|g" "$DIR/claude-settings-template.json"' '  sed "s|__HOOK__|$HOOK|g" "$DIR/claude-settings-template.json"' && run 'M128: 誰のものでもない資格情報を setup で消さない'

mutate $IMPORT '  for (const f of [stagedCapture, stagedManifest]) rmSync(f, { force: true, recursive: true });' '  for (const f of [stagedCapture, stagedManifest]) rmSync(f, { force: true, recursive: true });
  rmSync(`${dest}.prev`, { force: true, recursive: true });' && run 'M129: 復元に失敗した経路で退避まで消す'
mutate $SCHEMAV '    if (!SUPPORTED_KEYWORD_SET.has(key)) {' '    if (!SUPPORTED_KEYWORDS.includes(key)) {' && run 'M130: 検証が実行時に広げられる一覧を見る'

mutate $RIG '  local git_bin; git_bin=$(PATH="$TRUSTED_PATH" type -P git) \
    || { echo "git not found in the trusted path" >&2; exit 1; }
  "$ENV_BIN" -i \
    PATH="${git_bin%/*}:/usr/bin:/bin" \
    HOME="$RIG_BASE/home" \
    GIT_CONFIG_NOSYSTEM=1 \
    "$git_bin" -C "$RIG_BASE/workspace" "$@"' '  git -C "$RIG_BASE/workspace" "$@"' && run 'M131: 測定用 workspace の git を実環境の環境ごと走らせる'
mutate $RIG '${RUN_SIGNAL:+--signal="$RUN_SIGNAL"} "${RUN_TIMEOUT:-300}" "$CLAUDE_BIN"' '${RUN_SIGNAL:+--signal=$RUN_SIGNAL} "${RUN_TIMEOUT:-300}" "$CLAUDE_BIN"' && run 'M132: signal knob の引用を外す'
mutate $IMPORT 'if (!/^(?:0|[1-9]\d*)\n$/.test(text)) die("the recorded exit status is not a plausible exit code");' 'if (!/^\d{1,3}\n$/.test(text)) die("the recorded exit status is not a plausible exit code");' && run 'M133: 3 桁に収まる 0 詰めを通す'
mutate $IMPORT 'if (!/^(?:0|[1-9]\d*)\n$/.test(text)) die("the recorded exit status is not a plausible exit code");' 'if (!/^\s*(?:0|[1-9]\d{0,2})\s*$/.test(text)) die("the recorded exit status is not a plausible exit code");' && run 'M134: 綴りを畳んでから見る'

mutate $VERIFY '    const secret = inSubtree || SECRET_KEY_SET.has(key) || SECRET_SUBTREE_SET.has(key);' '    const secret = inSubtree || SECRET_KEYS.includes(key) || SECRET_SUBTREES.includes(key);' && run 'M135: 警報の対象に実行時に広げられる一覧を使う'
mutate $RIG '  teardown) require_rig_base; mkdir -p "$RIG_BASE"' '  teardown) mkdir -p "$RIG_BASE"' && run 'M136: teardown が置き場の出どころを確かめない'
mutate $RIG 'setup() {
  require_rig_base' 'setup() {' && run 'M137: setup が置き場の出どころを確かめない'
mutate $IMPORT 'if (existsSync(`${dest}.prev`)) {
  die("a previous record is still set aside for recovery; resolve it before importing again");
}' 'existsSync(`${dest}.prev`);' && run 'M138: 復旧待ちの退避があっても持ち込みを始める'

mutate $RIG 'PATH="$TRUSTED_PATH" type -P git' 'command -v git' && run 'M139: 隔離用の git を呼び出し元の PATH から選ぶ'
mutate $RIG '  [ "$ver_rc" -eq 0 ] || { rm -f "$stem.version.new" "$stem.version.err.new"; echo "claude --version failed (exit=$ver_rc)" >&2; exit 1; }' '  [ "$ver_rc" -eq 0 ] || { : > "$capture"; rm -f "$capture.errors" "$stem.exit"; echo "claude --version failed (exit=$ver_rc)" >&2; exit 1; }' && run 'M140: 落ちた版の問い合わせでも前の記録を消す'
mutate $RIG '  stage_credentials codex' '  : > "$capture"; rm -f "$capture.errors" "$stem.exit"
  stage_credentials codex' && run 'M141: codex の run が問い合わせより先に前の記録を消す'

mutate $RIG 'while read -r _ _ fn; do unset -f "$fn"; done < <(declare -F)' ':' && run 'M142: 呼び出し元が export した関数を外さない'
mutate $RIG '  local git_bin; git_bin=$(PATH="$TRUSTED_PATH" type -P git) \
    || { echo "git not found in the trusted path" >&2; exit 1; }
  "$ENV_BIN" -i \
' '  local git_bin; git_bin=$(PATH="$TRUSTED_PATH" type -P git) \
    || { echo "git not found in the trusted path" >&2; exit 1; }
  env -i \
' && run 'M143: workspace を作る env を呼び出し元の PATH から選ぶ'
mutate $RIG '  "$ENV_BIN" -i \
    PATH="$NODE_DIR:/usr/local/bin:/usr/bin:/bin" \
' '  env -i \
    PATH="$NODE_DIR:/usr/local/bin:/usr/bin:/bin" \
' && run 'M144: 測定対象を起動する env を呼び出し元の PATH から選ぶ'
mutate $RIG '  [ "$ver_rc" -eq 0 ] || { rm -f "$stem.version.new" "$stem.version.err.new"; echo "claude --version failed (exit=$ver_rc)" >&2; exit 1; }
  # ここから前の記録を置き換える。記録失敗の痕跡も run ごとに消す。残すと前回の失敗が今回の
  # manifest の recorderErrors に載り、正しい証拠が棄却される。終了コードも同じ理由で消す:
  # run が SIGKILL で落ちると前回の成功が残り、途中で切れた記録に exitStatus=0 が付く。
  # .errors だけは hook が $CAPTURE_FILE から作るので記録側の名前になる
  # 消すのが先、公開が後。逆にすると、その間に落ちた瞬間だけ「前の記録 + 新しい版」が揃い、
  # 取り込みが**前の run の記録を新しい版で測ったこと**にできる。この順なら、途中で落ちても
  # 記録が空で終了コードも無い＝取り込みが必ず断る
  : > "$capture"; rm -f "$capture.errors" "$stem.exit"
  mv "$stem.version.new" "$stem.version"; mv "$stem.version.err.new" "$stem.version.err"' '  [ "$ver_rc" -eq 0 ] || { rm -f "$stem.version.new" "$stem.version.err.new"; echo "claude --version failed (exit=$ver_rc)" >&2; exit 1; }
  # ここから前の記録を置き換える。記録失敗の痕跡も run ごとに消す。残すと前回の失敗が今回の
  # manifest の recorderErrors に載り、正しい証拠が棄却される。終了コードも同じ理由で消す:
  # run が SIGKILL で落ちると前回の成功が残り、途中で切れた記録に exitStatus=0 が付く。
  # .errors だけは hook が $CAPTURE_FILE から作るので記録側の名前になる
  # 消すのが先、公開が後。逆にすると、その間に落ちた瞬間だけ「前の記録 + 新しい版」が揃い、
  # 取り込みが**前の run の記録を新しい版で測ったこと**にできる。この順なら、途中で落ちても
  # 記録が空で終了コードも無い＝取り込みが必ず断る
  mv "$stem.version.new" "$stem.version"; mv "$stem.version.err.new" "$stem.version.err"
  : > "$capture"; rm -f "$capture.errors" "$stem.exit"' && run 'M145: 前の記録を無効にする前に新しい版を公開する'

mutate $RIG '"$git_bin" -C "$RIG_BASE/workspace" "$@" 9>&-' '"$git_bin" -C "$RIG_BASE/workspace" "$@"' && run 'M146: workspace を作る git に lock の fd を渡す'

echo "--- 復元後 ---"
# 目視で終わらせない。`node ... | grep` は grep の終了状態を返すので、件数を取り出して 0 でなければ落とす
BASELINE=$(node --experimental-strip-types --test "${TESTS[@]}" 2>&1)
printf '%s\n' "$BASELINE" | grep -E '^ℹ (pass|fail) '
BASELINE_FAIL=$(printf '%s' "$BASELINE" | grep -E '^# fail |^ℹ fail ' | tail -1 | grep -oE '[0-9]+$')
if [ -z "$BASELINE_FAIL" ] || [ "$BASELINE_FAIL" -ne 0 ]; then
  echo "変異テスト失敗: 復元後の baseline が green でない（変異が残ったか test が壊れている）" >&2
  exit 1
fi

echo "--- 集計 ---"
EXPECTED=$(grep -cE "&& run(_custom)? 'M[0-9]+b?:" "$0")
printf '実行 %d / 期待 %d、生存 %d\n' "$EXECUTED" "$EXPECTED" "$SURVIVED"
if [ "$EXECUTED" -ne "$EXPECTED" ] || [ "$SURVIVED" -ne 0 ]; then
  echo "変異テスト失敗: 生存した変異か、黙って飛ばされた変異がある" >&2
  exit 1
fi
