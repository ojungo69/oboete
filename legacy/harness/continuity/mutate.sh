#!/usr/bin/env bash
# 変異テスト: 参照模型の各ゲートをわざと壊し、対応する test が落ちることを確かめる。
#
# 使い方: bash harness/continuity/mutate.sh
# 出力の各行は「変異のラベル」と「その変異を入れたときの fail 件数」。
# **fail 0 の行は生存した変異**（そのゲートを壊しても test が落ちない = 検証が効いていない）。
# 期待は全行が fail 1 件以上。evidence/phase3-reference-model.md §5 の表はこの出力から作る。
#
# 実行件数も必ず突き合わせる。アンカー文字列が実装の変更で外れると `assert old in s` が落ちて
# `&&` が短絡し、その変異は **出力に何も出ないまま黙って飛ばされる**:
#   grep -oP '&& run "\K[^"]+' harness/continuity/mutate.sh | grep -v '^\\K' | sort > /tmp/want.txt
#   bash harness/continuity/mutate.sh | grep -oP '^.*?(?=\s+ℹ fail )' | sed 's/ *$//' | sort > /tmp/got.txt
#   comm -23 /tmp/want.txt /tmp/got.txt   # 空でなければ黙って飛ばされている
# （このコメント行自体が 1 行目の grep に引っかかるので `grep -v '^\K'` で落とす）
#
# 中断すると変異が残るので、その場合は `git checkout harness/continuity/ harness/fixtures/continuity/`。
set -u
cd "$(dirname "$0")/../.."
SRC=harness/continuity/reference-model.ts
# 旧形 parity の門は、還元器ではなく**比較面と corpus**を壊して検証する（門の assert 自体を
# 壊す変異は、その門でしか検出できないので kill できない = 変異の対象にならない）
PROJECTION=harness/continuity/old-shape-projection.ts
CORPUS=harness/fixtures/continuity/old-shape-parity.json
MUTABLE=("$SRC" "$PROJECTION" "$CORPUS")
TESTS=(harness/continuity/reference-model.test.ts harness/continuity/old-shape-parity.test.ts)
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
# 足した日に退避が上書きされ、復元が**別の file の中身**を書き戻す。退避が 1 つでも作れなかったら
# 変異を当てる前に降りる（復元できないまま変異を積むと、出口が `git checkout` しかなくなる）
for f in "${MUTABLE[@]}"; do
  cp --parents "$f" "$BAKDIR" || { echo "変異テスト失敗: $f を退避できない（cp --parents は GNU coreutils 拡張）" >&2; exit 1; }
done

EXECUTED=0
SURVIVED=0
# 変異前の test 件数。run() がこれと突き合わせて「変異が test を走らせていない」を検出する
BASELINE_TESTS=$(node --experimental-strip-types --test "${TESTS[@]}" 2>&1 \
  | grep -E '^# tests |^ℹ tests ' | tail -1 | grep -oE '[0-9]+$')
if [ -z "${BASELINE_TESTS:-}" ]; then
  echo "変異テスト失敗: baseline の test 件数を取得できない" >&2
  exit 1
fi

run() {
  local label="$1"
  local out
  out=$(node --experimental-strip-types --test "${TESTS[@]}" 2>&1)
  local failed
  failed=$(printf '%s' "$out" | grep -E '^# fail |^ℹ fail ' | tail -1)
  local n
  n=$(printf '%s' "$failed" | grep -oE '[0-9]+$')
  # **走った件数も見る**。変異でソースが parse できないと node:test は「読み込みに失敗した 1 件」を
  # fail として数えるので、`fail 1` だけを見ていると**ゲートを一度も壊していない変異が kill として
  # 計上される**（実測: 壊れた module で tests 1 / pass 0 / fail 1）。baseline と件数が違えば、
  # その変異はゲートの検証になっていないので生存と同じ扱いにする
  local ran
  ran=$(printf '%s' "$out" | grep -E '^# tests |^ℹ tests ' | tail -1 | grep -oE '[0-9]+$')
  printf '%-46s %s\n' "$label" "${failed:-<test が走らなかった>}"
  EXECUTED=$((EXECUTED + 1))
  if [ -z "$n" ] || [ "$n" -eq 0 ]; then
    SURVIVED=$((SURVIVED + 1))
  elif [ -z "$ran" ] || [ "$ran" -ne "$BASELINE_TESTS" ]; then
    printf '  ^ 変異が test を走らせていない（tests %s / baseline %s）。ゲート未検証\n' \
      "${ran:-?}" "$BASELINE_TESTS"
    SURVIVED=$((SURVIVED + 1))
  fi
  restore_all || exit 1
}

# 既定の対象は還元器。第 3 引数で別ファイルを狙う
mutate() { # python replacement [file]
  python3 - "$1" "$2" "${3:-$SRC}" <<'PY'
import sys, pathlib
old, new, target = sys.argv[1], sys.argv[2], sys.argv[3]
# 置換後の文字列に書いた `\n` は改行として扱う。bash の二重引用符は `\n` を展開しないので、
# 素で渡すと**リテラルのバックスラッシュ n が TS に埋まって module が parse できなくなる**。
# その状態でも node:test は「読み込み失敗 1 件」を fail として数えるため、ゲートを一度も
# 壊していない変異が kill として計上されていた（実測で 5 件。run() 側の件数突き合わせと対で塞ぐ）
new = new.replace("\\n", "\n")
p = pathlib.Path(target)
s = p.read_text()
# アンカーは**ソース中で一意**でなければならない。2 箇所に出るアンカーだと
# `replace(old, new, 1)` は必ず先頭を書き換えるので、2 つ目の site を狙ったラベルが
# 1 つ目を二重に壊すだけになり、狙ったゲートは無検証のまま kill として計上される（実測）
count = s.count(old)
assert count == 1, f"anchor must be unique (found {count}): {old[:70]}"
p.write_text(s.replace(old, new, 1))
PY
}

mutate "  const applied = idempotencyLedger.get(key);
  if (applied !== undefined) {
    // §4.3「terminal は … payload/source hash が衝突しないこと」。同じ配送 ID で内容が違う" "  const applied = idempotencyLedger.get(key);
  if (false) {
    // §4.3「terminal は … payload/source hash が衝突しないこと」。同じ配送 ID で内容が違う" && run "dedupe 判定を外す"
mutate "  return compareIngestSeq(a, b) >= 0 ? a : b;" "  return b;" && run "lastIngestSeq の max を外す"
mutate "  if (a.length !== b.length) return a.length < b.length ? -1 : 1;" "  return Number(a) === Number(b) ? 0 : Number(a) < Number(b) ? -1 : 1;\n  // eslint-disable-next-line" && run "ingestSeq を数値比較にする"
mutate "  if (operation === undefined) {
    throw new Error(\`§3.1 違反: operation event \${event.kind} に operation envelope が無い\`);
  }" "  if (operation === undefined) {
    return;
  }" && run "envelope 必須を外す"
mutate "    attestation !== undefined &&" "    true &&" && run "intake の attestation 必須を外す"
mutate "  const { ingestAttestation: _claimed, ...provenance } = event.provenance;" "  const provenance = event.provenance;" && run "caller の attestation を信じる"
mutate "  if (!isBlank(context.expectedSourceAgent) && event.sourceAgent !== context.expectedSourceAgent) {" "  if (false) {" && run "sourceAgent の束縛を外す"
mutate "  if (!isBlank(context.expectedSourceAgent) && event.sourceAgent !== context.expectedSourceAgent) {" "  if (event.sourceAgent !== context.expectedSourceAgent) {" && run "認証できない経路でも Agent 名で落とす"
mutate "  if (!isBlank(context.expectedSessionId) && event.sessionId !== context.expectedSessionId) {" "  if (false) {" && run "session の束縛を外す"
mutate "  if (!isBlank(context.expectedSessionId) && event.sessionId !== context.expectedSessionId) {" "  if (context.expectedSessionId !== \"\" && event.sessionId !== context.expectedSessionId) {" && run "空白の session 束縛を実在する名前として扱う"
mutate "  if (!isBlank(context.expectedSessionId) && event.sessionId !== context.expectedSessionId) {" "  if (event.sessionId !== context.expectedSessionId) {" && run "session を名乗れない経路でも session 名で落とす"
mutate "  if (event.turnIdSource === \"synthesized_monotonic\" && !authenticatedPeer) {" "  if (false) {" && run "未認証の synthesized_monotonic を診断に出さない"
mutate "  if (event.turnIdSource === \"synthesized_monotonic\" && !authenticatedPeer) {" "  if (event.turnIdSource === \"synthesized_monotonic\" && !authenticatedVersion) {" && run "未認証の判定に version 一致まで求める"
mutate "  const authenticatedVersion =
    authenticatedPeer &&" "  const authenticatedVersion =
    true &&" && run "version authority が経路の認証を前提にしない"
mutate "    !isBlank(context.expectedSourceAgent);" "    true;" && run "空の Agent 名を素通しする"
mutate "    event.turnIdSource === \"native\" && !(authenticatedVersion && context.nativeTurnIdentityProven);" "    false;" && run "native turn の証明要求を外す"
mutate "!(authenticatedVersion && context.nativeTurnIdentityProven)" "!context.nativeTurnIdentityProven" && run "turn 証明の version 束縛を外す"
mutate "  if (turnDowngraded) {" "  if (false) {" && run "turn 降格を黙って行う"
mutate "export function assertTurnIdentity(event: NormalizedContinuityEvent): void {" "export function assertTurnIdentity(event: NormalizedContinuityEvent): void {\n  if (event) return;" && run "turn 同一性の不変条件を外す"
mutate "  if (event.sourceAgent !== state.sourceAgent) {" "  if (false) {" && run "state への Agent 束縛を外す"
mutate "  const delivery =
    event.adapterDeliveryId !== undefined && isBlank(event.adapterDeliveryId)
      ? undefined
      : event.adapterDeliveryId;" "  const delivery = event.adapterDeliveryId;" && run "空 adapterDeliveryId の fallback を外す"
mutate "    operation.nativeOperationId !== undefined
      ? byNativeId" "    byNativeId.length > 0
      ? byNativeId" && run "rule 1 の排他を外す"
mutate "            declared(pending.correlation.turnId) !== undefined &&" "            true &&" && run "rule 2 の turn 同一性要求を外す"
mutate "  if (open.length > 1) {" "  if (false) {" && run "候補が複数のときの拒否を外す"
mutate "  const identityConflicts = (pending: PendingOperation): boolean =>" "  const identityConflicts = (pending: PendingOperation): boolean =>
    pending.correlation.operationMatchKey !== operation.operationMatchKey ||" && run "terminal 側に matchKey 一致を要求し直す"
mutate "  const compatible = candidates.filter((pending) => !identityConflicts(pending));" "  const compatible = candidates.some(identityConflicts) ? [] : candidates;" && run "identity 衝突を候補 1 件で判定する"
mutate "      pending.correlation.canonicalInputHash !== operation.canonicalInputHash);" "      false);" && run "terminal の canonicalInputHash 衝突検査を外す"
mutate "  if (compatible.length === 0) {
    return {
      matched: null,
      diagnostic: \"terminal_conflict\"," "  if (false) {
    return {
      matched: null,
      diagnostic: \"terminal_conflict\"," && run "identity 衝突の隔離を外す"
mutate "  return event.kind === \"tool_failed\" && event.successful === true;" "  return false;" && run "kind と successful の矛盾を素通しする"
mutate "      ...contradictionDiagnostics,
    ];" "    ];" && run "矛盾診断を照合済み経路だけに戻す"
mutate "  if (terminalEvidenceContradicts(event)) return \"unknown\";" "  if (false) return \"unknown\";" && run "矛盾した terminal を succeeded にする"
mutate "  if (startIngestSeq === undefined) {" "  if (false) {" && run "start 不在の分岐を外す"
mutate "  if (compareIngestSeq(terminalEvent.ingestSeq, startIngestSeq) <= 0) {" "  if (false) {" && run "terminal の権威順序検査を外す"
mutate "  return seq !== undefined && INGEST_SEQ_PATTERN.test(seq) ? seq : undefined;" "  return seq;" && run "綴りの合わない順序材料を値として読む（空白・語彙外）"
mutate "function startTurnIdSourceOf(pending: PendingOperation): string | undefined {
  return declared(pending.startTurnIdSource);" "function startTurnIdSourceOf(pending: PendingOperation): string | undefined {
  return pending.startTurnIdSource;" && run "空白の turn 種別を値として読む"
mutate "      detail: \"terminal が start より後でない\",
      // 一致した 1 件だけが unknown。同じ matchKey の無関係な open を巻き込まない
      unresolved: [matched]," "      detail: \"terminal が start より後でない\",
      unresolved: compatible.filter(isOpen)," && run "順序違反で候補を巻き込む"
mutate "      correlation.unresolved.length === 0 &&" "      false &&" && run "候補ゼロの terminal を台帳に入れる"
mutate "      detail: \`operation \${matched.operationId} の start が状態に無く、権威順序を確認できない\`,
      unresolved: [matched]," "      detail: \`operation \${matched.operationId} の start が状態に無く、権威順序を確認できない\`,
      unresolved: []," && run "順序不明で候補を unknown にしない"
mutate "    startIngestSeq: event.ingestSeq,
    startTurnIdSource: event.turnIdSource," "    startTurnIdSource: event.turnIdSource," && run "start の取り込み連番を記録しない"
mutate "    startIngestSeq: event.ingestSeq,
    startTurnIdSource: event.turnIdSource," "    startIngestSeq: event.ingestSeq," && run "start の turn 種別を記録しない"
mutate "        pendingOperations: previous.state.pendingOperations.map((pending) =>
          pending === existing
            ? withSourceEvent({ ...pending, correlation: recovered }, event.eventId)
            : pending," "        pendingOperations: previous.state.pendingOperations.map((pending) =>
          pending === existing
            ? withSourceEvent({ ...pending, correlation: recovered, startIngestSeq: event.ingestSeq }, event.eventId)
            : pending," && run "再配送 start でも順序材料を書く"
mutate "      operation.nativeOperationId === undefined
        ? []
        : inLineage.filter(" "      true
        ? []
        : inLineage.filter(" && run "再配送 start を nativeOperationId で拾わない"
mutate "              pending.correlation.nativeOperationId === operation.nativeOperationId &&" "              pending.correlation.operationMatchKey === operation.operationMatchKey &&" && run "再配送の判定を matchKey にする"
mutate "    if (startConflict) {" "    if (false) {" && run "start の identity 衝突検査を外す"
mutate "        existing.correlation.operationMatchKey !== operation.operationMatchKey ||" "        false ||" && run "start の matchKey 衝突検査を外す"
mutate "            existing.correlation.canonicalInputHash !== operation.canonicalInputHash)" "            false)" && run "start の canonicalInputHash 衝突検査を外す"
mutate "      (pending) =>
        pending.status === \"started\" &&
        pending.correlation.sessionId === event.sessionId &&
        pending.correlation.taskLineageId === state.taskLineageId," "      (pending) =>\n        pending.status === \"started\" &&\n        pending.correlation.taskLineageId === state.taskLineageId," && run "放棄を session で絞らない"
mutate "        unresolved.has(pending)
          ? withSourceEvent(" "        false
          ? withSourceEvent(" && run "候補の unknown 化を外す"
mutate "          ? withSourceEvent(
              pending.status === \"started\" ? { ...pending, status: \"unknown\" as const } : pending,
              event.eventId,
            )" "          ? (pending.status === \"started\" ? { ...pending, status: \"unknown\" as const } : pending)" && run "unknown 化で証跡を残さない"
mutate "  if (sourceEventLost(pending, eventId)) return pending;" "  // eslint-disable-next-line" && run "sourceEventIds の上限を外す"
mutate "  if (pending.length < CONTINUITY_LIMITS.arrayItems) return pending;" "  return pending;\n  // eslint-disable-next-line" && run "pendingOperations の上限を外す"
mutate "const EVICTION_ORDER: readonly PendingOperation[\"status\"][] = [
  \"succeeded\",
  \"failed\",
  \"unknown\",
  \"started\",
];" "const EVICTION_ORDER: readonly PendingOperation[\"status\"][] = [\"succeeded\", \"failed\"];" && run "退避対象から open を外す（詰まる）"
mutate "      if (dropped.size === dropCount) break;" "      if (false) break;" && run "退避件数の上限を外す"
mutate "        ...(evicted.length === 0
          ? []" "        ...(true
          ? []" && run "退避を黙って行う"
mutate "    pendingOperations: [...pendingOperations]," "    pendingOperations," && run "revision ごとの配列分離を外す"
mutate "  const applied = idempotencyLedger.get(key);
  if (applied !== undefined) {
    // reducer 側と同じ判定にする。" "  const applied = idempotencyLedger.get(key);
  if (false) {
    // reducer 側と同じ判定にする。" && run "放棄経路の dedupe を外す"
mutate "  return event.adapterDeliveryId === undefined || isBlank(event.adapterDeliveryId)
    ? \`f:\${key}\`
    : \`d:\${key}\`;" "  return key;" && run "台帳の keyspace 分離を外す"
mutate "    !isBlank(context.activeCapabilityHash) &&
    provenance.capabilityHash === context.activeCapabilityHash &&" "    provenance.capabilityHash === context.activeCapabilityHash &&" && run "空の capabilityHash を素通しする"
mutate "  return known < 0 ? SENSITIVITIES.length - 1 : known;" "  return known;" && run "未知の sensitivity で fail open する"
mutate "  if (
    (operation.nativeOperationId !== undefined && isBlank(operation.nativeOperationId)) ||
    (operation.canonicalInputHash !== undefined && isBlank(operation.canonicalInputHash))
  ) {" "  if (false) {" && run "空文字の任意欄を素通しする"
mutate "          (declared(existing.correlation.toolName) !== undefined &&
            existing.correlation.toolName !== operation.operationKind) ||" "          false ||" && run "start の operationKind 比較を外す"
mutate "          (declared(existing.correlation.toolName) !== undefined &&
            existing.correlation.toolName !== operation.operationKind) ||" "          (existing.correlation.toolName !== operation.operationKind) ||" && run "start の toolName 存在ガードを外す"
mutate "  if (!ABANDONMENT_EVENT_KINDS.has(event.kind)) {" "  if (false) {" && run "放棄 kind の制限を外す"
mutate "    if (applied.sourceHash !== undefined && incoming !== undefined && applied.sourceHash !== incoming) {
      return quarantine(previous, idempotencyLedger, [" "    if (false) {
      return quarantine(previous, idempotencyLedger, [" && run "配送 ID 衝突の隔離を外す"
mutate "  visit(content);
  return SENSITIVITIES[rank] as Sensitivity;" "  visit(content);
  return \"normal\";" && run "sensitivity 集約を normal 固定にする"

mutate "    if (operation !== undefined) {
      assertOperationFields(operation);
    }
    return;" "    return;" && run "adapter 固有 kind の欄検査を外す"
mutate "          (declared(existing.correlation.nativeOperationId) !== undefined &&
            declared(operation.nativeOperationId) !== undefined &&
            existing.correlation.nativeOperationId !== operation.nativeOperationId) ||" "          false ||" && run "start の nativeOperationId 比較を外す"
mutate "    (declared(pending.correlation.toolName) !== undefined &&
      pending.correlation.toolName !== operation.operationKind) ||" "    false ||" && run "terminal の operationKind 比較を外す"
mutate "    (declared(pending.correlation.toolName) !== undefined &&
      pending.correlation.toolName !== operation.operationKind) ||" "    (pending.correlation.toolName !== operation.operationKind) ||" && run "terminal の toolName 存在ガードを外す"
mutate "    if (applied.sourceHash !== undefined && incoming !== undefined && applied.sourceHash !== incoming) {
      // 診断も還元器側と同じものを出す。" "    if (false) {
      // 診断も還元器側と同じものを出す。" && run "放棄経路の配送 ID 衝突検査を外す"

mutate "  if (isBlank(event.canonicalFingerprint)) {" "  if (false) {" && run "空 canonicalFingerprint を素通しする"
mutate "    if (contradicted !== undefined) {" "    if (false) {" && run "確定済み成否との矛盾検査を外す"
mutate "      incoming === \"unknown\" || plausible.some((pending) => pending.status === incoming)" "      false" && run "成否を主張しない terminal も矛盾扱いにする"
mutate "      incoming === \"unknown\" || plausible.some((pending) => pending.status === incoming)" "      incoming === \"unknown\"" && run "成否が一致する兄弟の検査を外す"

mutate "  if (ABANDONMENT_EVENT_KINDS.has(event.kind)) {" "  if (false) {" && run "放棄 kind を還元器に通す"
mutate "  if (event.turnId !== undefined && isBlank(event.turnId)) {" "  if (false) {" && run "空文字の turnId を素通しする"
mutate "  if (isBlank(event.eventId)) {" "  if (false) {" && run "空文字の eventId を素通しする"
mutate "  let rank = rankOfSensitivity(floor);" "  let rank = 0;" && run "sensitivity の下限に直前の集約値を使わない"
mutate "  if (isBlank(event.sessionId)) {" "  if (false) {" && run "空文字の sessionId を素通しする"
mutate "  return /^[\\s\\p{Cf}]*\$/u.test(value);" "  return value === \"\";" && run "空白文字を identity 材料として通す"
mutate "  return /^[\\s\\p{Cf}]*\$/u.test(value);" "  return /^\\s*\$/u.test(value);" && run "書式制御文字だけの identity 材料を通す"
mutate "  if (isBlank(operation.operationMatchKey) || isBlank(operation.operationKind)) {" "  if (false) {" && run "空の operationMatchKey / operationKind を素通しする"
mutate "  const sameTurn = sameTurnOf(compatible);" "  const sameTurn = sameTurnOf(candidates);" && run "open の選択を identity 互換に絞らない"
mutate "    declared(pending.correlation.canonicalInputHash) !== undefined &&
    declared(operation.canonicalInputHash) === undefined;" "    false;" && run "canonicalInputHash の省略を照合可能として扱う"
mutate "        existing.correlation.sessionId !== event.sessionId ||" "        false ||" && run "再配送 start の session 検査を外す"
mutate "    diagnostics: truncated.length === 0 ? [] : [truncationDiagnostic(event, truncated)]," "    diagnostics: []," && run "放棄で落とした証跡を報告しない"
mutate "  assertOperationEnvelope(terminalEvent);" "" && run "直接呼びの envelope 検査を外す"
mutate "          (declared(existing.correlation.turnId) !== undefined &&
            declared(event.turnId) !== undefined &&
            existing.correlation.turnId !== event.turnId) ||" "          false ||" && run "再配送 start の turn 検査を外す"
mutate "          (declared(existing.correlation.turnId) !== undefined &&
            declared(event.turnId) !== undefined &&
            existing.correlation.turnId !== event.turnId) ||" "          (existing.correlation.turnId !== event.turnId) ||" && run "再配送 start の turn 存在ガードを外す"
mutate "  const unverifiable = plausible.length > 1 ? plausible.find(identityUnverifiable) : undefined;" "  const unverifiable = plausible.find(identityUnverifiable);" && run "候補 1 件でも照合不能ゲートを発火させる"
mutate "  const unverifiable = plausible.length > 1 ? plausible.find(identityUnverifiable) : undefined;" "  const unverifiable = plausible.length > 2 ? plausible.find(identityUnverifiable) : undefined;" && run "照合不能ゲートの候補数を 1 件ずらす"
mutate "  if (open.length === 0) {" "  if (open.length === 0 && compatible.find(identityUnverifiable) === undefined) {" && run "照合不能を成否矛盾検査より先に判定する"

mutate "    !isBlank(context.activeCapabilityHash) &&" "    context.activeCapabilityHash !== \"\" &&" && run "空白だけの capability hash を authority にする"
mutate "    !isBlank(context.expectedSourceAgent);" "    context.expectedSourceAgent !== \"\";" && run "空白だけの Agent 名を authority にする"
mutate "    !isBlank(context.exactAgentVersion) &&" "    context.exactAgentVersion !== \"\" &&" && run "空白だけの exact version を authority にする"
mutate "  assertSameScope(previous.state, terminalEvent);" "" && run "直接呼びの Agent 検査を外す"
mutate "          const recorded = startTurnIdSourceOf(pending);
          return recorded === undefined || recorded === terminalEvent.turnIdSource;" "          return true;" && run "rule 2 の turn 種別の絞り込みを外す"
mutate "          return recorded === undefined || recorded === terminalEvent.turnIdSource;" "          return recorded === terminalEvent.turnIdSource;" && run "turn 種別の材料が無い候補も落とす"
mutate "        unresolved: sourceMismatch ? sameTurnOpen : compatibleOpen," "        unresolved: compatibleOpen," && run "種別違いの巻き込み範囲を広げる"
# 絞り込みを抜けた候補には「種別が一致した」と「確認できなかった」が混ざる。閉じる直前の門で
# 両方向を潰す: 外して合格させる / 締めすぎて健全な terminal を止める
mutate "  if (rule === \"match_key\" && startTurnIdSourceOf(matched) === undefined) {" "  if (false) {" && run "turn 種別が無いまま rule 2 を閉じさせる（FR-004）"
mutate "  if (rule === \"match_key\" && startTurnIdSourceOf(matched) === undefined) {" "  if (startTurnIdSourceOf(matched) === undefined) {" && run "turn を要求しない rule 1 まで種別で止める"
mutate "  if (rule === \"match_key\" && startTurnIdSourceOf(matched) === undefined) {" "  if (rule === \"match_key\" && matched.startTurnIdSource === undefined) {" && run "空白だけの turn 種別を材料として通す"
mutate '      detail: `operation ${matched.operationId} の start が turn 種別を持たず、rule 2 の turn 両立を確認できない`,
      unresolved: [matched],' '      detail: "unverifiable",
      unresolved: [],' && run "turn 種別が無い候補を unknown に倒さず据え置く"

mutate "    !isBlank(attestation.ingestReceiptId) &&" "    true &&" && run "受領証 ID が空でも認証済みとする"
mutate "    !isBlank(attestation.peerIdentityId) &&" "    true &&" && run "peer identity が空でも認証済みとする"
mutate "    !isBlank(attestation.ingestReceiptId) &&" "    attestation.ingestReceiptId !== \"\" &&" && run "空白だけの受領証 ID を authority にする"
mutate "    !isBlank(provenance.scenarioId) &&" "    true &&" && run "空白の scenarioId で proven を成立させる"
mutate "  assertIngestSeq(terminalEvent.ingestSeq);" "" && run "直接呼びの ingestSeq 検査を外す"
mutate "  assertIngestSeq(terminalEvent.ingestSeq);" "  assertSameScope(previous.state, terminalEvent);
  assertIngestSeq(terminalEvent.ingestSeq);" && run "直接呼びだけ scope を ingestSeq より先に見る"
mutate "  assertIdentityMaterial(terminalEvent);" "" && run "直接呼びの identity 材料検査を外す"
mutate "  if (isBlank(event.sourceAgent)) {" "  if (false) {" && run "空白の sourceAgent を素通しする"
mutate "    if (plausible.length === 0) {" "    if (false) {" && run "turn 両立ゼロの確定済みを適用済みにする"
mutate "        : compatible.filter((pending) => !isOpen(pending)).find((pending) => pending.status !== incoming);" "        : plausible.find((pending) => pending.status !== incoming);" && run "矛盾判定の母数まで turn で絞る"
mutate "        unresolved.has(pending)" "        [...unresolved].some((c) => c.operationId === pending.operationId)" && run "候補の unknown 化を operationId の等値で当てる"
mutate "    state.pendingOperations.filter(
      (pending) =>
        pending.status === \"started\" &&
        pending.correlation.sessionId === event.sessionId &&
        pending.correlation.taskLineageId === state.taskLineageId,
    ),
  );
  const pendingOperations = state.pendingOperations.map((pending) =>
    abandoned.has(pending)" "    state.pendingOperations.filter(
      (pending) =>
        pending.status === \"started\" &&
        pending.correlation.sessionId === event.sessionId &&
        pending.correlation.taskLineageId === state.taskLineageId,
    ).map((pending) => pending.operationId) as unknown as PendingOperation[],
  );
  const pendingOperations = state.pendingOperations.map((pending) =>
    abandoned.has(pending.operationId as unknown as PendingOperation)" && run "放棄の適用先を operationId の等値で当てる"
mutate "      incoming === \"unknown\" || plausible.some((pending) => pending.status === incoming)" "      incoming === \"unknown\" || compatible.some((pending) => pending.status === incoming)" && run "確定済みの説明に turn 両立を求めない"
mutate "      incoming === \"unknown\" || plausible.some((pending) => pending.status === incoming)" "      incoming === \"unknown\" || sameTurn.some((pending) => pending.status === incoming)" && run "確定済みの説明で turn 種別だけ見ない"
mutate "  const open = plausible.filter(isOpen);" "  const open = compatible.filter(isOpen);" && run "open の切り分けを turn 絞り込みより前にする"
mutate "        : compatible.filter((pending) => !isOpen(pending)).find((pending) => pending.status !== incoming);" "        : compatible.find((pending) => pending.status !== incoming);" && run "矛盾判定に open な候補も混ぜる"
mutate "  return pending.filter((candidate) => !dropped.has(candidate));" "  return pending.filter((candidate) => ![...dropped].some((d) => d.operationId === candidate.operationId));" && run "退避の保持判定を operationId の一致に戻す"
mutate "    const contradicted = recordable ? undefined : contradicting;" "    const contradicted = contradicting;" && run "記録できる候補が居ても隔離を優先する"
mutate "          (contradicting === undefined" "          (true" && run "抑止した矛盾を報告に残さない"
mutate "      detail: \`operation \${unverifiable.operationId} は canonicalInputHash を持つのに terminal が省いている\`,
      unresolved: open," "      detail: \`operation \${unverifiable.operationId} は canonicalInputHash を持つのに terminal が省いている\`,
      unresolved: compatible.filter(isOpen)," && run "照合不能で turn 非両立の候補も巻き込む"
mutate "  if (rule === \"native_operation_id\" && compatible.length > 1) {" "  if (false) {" && run "rule 1 の候補が複数でも 1 件選ぶ"
mutate "  if (rule === \"native_operation_id\" && compatible.length > 1) {" "  if (rule === \"native_operation_id\" && byNativeId.length > 1) {" && run "rule 1 の候補数を identity 絞り込み前で数える"
mutate "          (recordedSource !== undefined &&
            recordedSource !== \"unavailable\" &&
            event.turnIdSource !== \"unavailable\" &&
            recordedSource !== event.turnIdSource) ||" "          false ||" && run "再配送 start の turn 種別を見ない"
mutate "            event.turnIdSource !== \"unavailable\" &&" "            true &&" && run "降格した再配送 start も隔離する"
mutate "            recordedSource !== \"unavailable\" &&" "            true &&" && run "記録が降格されていても再配送を隔離する"
mutate "    const inLineage = previous.state.pendingOperations.filter(
      (pending) => pending.correlation.taskLineageId === previous.state.taskLineageId,
    );" "    const inLineage = previous.state.pendingOperations;" && run "別 lineage の pending も再配送の相手にする"
mutate "        pending.correlation.sessionId === event.sessionId &&
        pending.correlation.taskLineageId === state.taskLineageId," "        pending.correlation.sessionId === event.sessionId," && run "放棄が別 lineage の operation も倒す"
# 「derived id の兄弟は先頭 1 件で決める」は削除。候補選びを 1 箇所に統合して集合ごとに別の
# 選び方をする変異が書けなくなった。互換優先の軸は下の「互換な候補を選ばない」が両集合まとめて
# 覆い、集合をまたげるかの軸は「再配送の相手を集合ごとに選ぶ」が、それぞれ別に見る。
# 互換な兄弟が複数居るときの帰属は「native id が一致する兄弟へ帰属させない」が、埋め戻しが
# 2 件目を作らないかは「名乗っている兄弟が非互換でも空白へ埋める」が、それぞれ別に見る
# （前者を消して後者だけにしたら、native ID の位置しか見ていない test では変異が生存した。
#  帰属は `sourceEventIds` で観測できるので、検証不能ではなく test が狭かった）
mutate "    (candidate) => candidate.correlation.taskLineageId !== taskLineageId," "    () => false," && run "退避で lineage 外を優先しない"
mutate "    for (const candidate of pending) {" "    for (const candidate of [...pending].sort((a, b) => a.startedAt.localeCompare(b.startedAt))) {" && run "群の中を配列位置でなく startedAt で退避する"
mutate "        nativeOperationId: declared(existing.correlation.nativeOperationId) ?? fillableNativeId," "        nativeOperationId: existing.correlation.nativeOperationId," && run "再配送が持つ native id を記録に埋めない"
mutate "      const fillableNativeId = nativeIdTaken ? undefined : operation.nativeOperationId;" "      const fillableNativeId = operation.nativeOperationId;" && run "名乗っている兄弟が非互換でも空白へ埋める"
mutate "        incomingNativeId !== undefined && nativeMatches.some((pending) => pending !== existing);" "        incomingNativeId !== undefined && siblings.some((pending) => pending !== existing);" && run "抑止の走査集合を session で絞らない"
mutate "  const plausible = orderableOf(eligibleOf(sameTurn));" "  const plausible = eligibleOf(sameTurn);" && run "候補を start の順序で絞らない"
mutate "    return orderable.length === 0 ? list : orderable;" "    return orderable;" && run "全件順序不適合でも空に絞る"
mutate "  if (!(TURN_ID_SOURCES as readonly string[]).includes(event.turnIdSource)) {" "  if (false) {" && run "turnIdSource の語彙検査を外す"
mutate "            (declared(pending.correlation.toolName) === undefined ||
              pending.correlation.toolName === operation.operationKind)," "            pending.correlation.toolName === operation.operationKind," && run "候補の toolName を素で比べる"
mutate "      correlation.diagnostic !== \"terminal_already_applied\"" "      true" && run "適用済みの再配送も隔離する"
mutate "    !pending.sourceEventIds.includes(eventId)
  );" "    true
  );" && run "記録済みの event でも truncation を出す"
mutate "            ? withSourceEvent({ ...pending, correlation: recovered }, event.eventId)" "            ? { ...pending, correlation: recovered }" && run "再配送 start の原因 event を残さない"
mutate "  const unverifiable = plausible.length > 1 ? plausible.find(identityUnverifiable) : undefined;" "  const unverifiable = compatible.length > 1 ? compatible.find(identityUnverifiable) : undefined;" && run "照合不能ゲートの母数を compatible に戻す"
mutate "            code: \"delivery_conflict\",
            eventId: event.eventId,
            detail: \`event \${applied.eventId} と同じ配送 ID で source hash が違う\`,
          },
        ],
      };" "            code: \"delivery_conflict\",
            eventId: event.eventId,
            detail: \`event \${applied.eventId} と同じ配送 ID で source hash が違う\`,
          },
        ].slice(0, 0),
      };" && run "放棄の配送衝突を診断に出さない"
mutate "  if (terminalEvent.operation?.phase !== \"terminal\") {" "  if (false) {" && run "correlate の入口で terminal 相を要求しない"
mutate "    const compatible = siblings.filter((pending) => !startConflictsWith(pending));" "    const compatible: readonly PendingOperation[] = [];" && run "兄弟から互換な候補を選ばない（derived id / native id 両方）"
mutate "    const compatible = siblings.filter((pending) => !startConflictsWith(pending));" "    const compatible = idMatches.filter((pending) => !startConflictsWith(pending));" && run "再配送の相手を集合ごとに選ぶ"
mutate "      compatible.at(0) ??
      siblings.at(0);" "      compatible.at(0);" && run "全件衝突のとき衝突の証拠を持たない"
mutate "      (incomingNativeId === undefined
        ? undefined
        : compatible.find(
            (pending) => declared(pending.correlation.nativeOperationId) === incomingNativeId,
          )) ??" "" && run "native id が一致する兄弟へ帰属させない"
mutate "      (incomingNativeId === undefined
        ? undefined
        : compatible.find(" "      (false
        ? undefined
        : compatible.find(" && run "届いた start が native id を持たなくても帰属を動かす"
mutate "    const siblings = [...new Set([...idMatches, ...nativeMatches])];" "    const siblings = [...new Set([...nativeMatches, ...idMatches])];" && run "兄弟の連結順を入れ替える"
mutate "    new Set([correlation.matched])," "    new Set(previous.state.pendingOperations)," && run "truncation の対象を照合相手の外へ広げる"
mutate "      const truncated = sourceEventLost(existing, event.eventId) ? [existing.operationId] : [];" "      const truncated = previous.state.pendingOperations.map((p) => p.operationId);" && run "再配送 start の truncation 対象を全 pending にする"
mutate "  if (value !== undefined && !isRealInstant(value)) {" "  if (false) {" && run "IsoTimestamp の暦検査を外す"
mutate "  assertRealInstant(
    \"provenance.ingestAttestation.attestedAt\",
    declared(event.provenance.ingestAttestation?.attestedAt),
  );" "" && run "受領証の時刻を暦検査から外す"
mutate "  if (provenance === undefined || provenance === null) {" "  if (false) {" && run "provenance 不在を節で落とさない"
mutate "  if (declaredProvenance === undefined || declaredProvenance === null) {" "  if (false) {" && run "書く層で provenance 不在を落とさない"
mutate "  assertRealInstant(\"受領証の attestedAt\", declared(attestation?.attestedAt));" "" && run "書く層で受領証の時刻を検査しない"
mutate "  assertRealInstant(\"受領証の attestedAt\", declared(attestation?.attestedAt));" "  assertRealInstant(\"受領証の attestedAt\", attestation?.attestedAt);" && run "空白の受領証時刻を暦違反として落とす"
mutate "    !isBlank(attestation.attestedAt) &&" "    true &&" && run "時刻を名乗らない受領証を authority にする"
mutate "    declared(event.provenance.ingestAttestation?.attestedAt),
  );" "    event.provenance.ingestAttestation?.attestedAt,
  );" && run "読む層で空白の受領証時刻を暦違反にする"
mutate "  if (!isCanonicalTimestamp(value)) return false;" "" && run "暦検査の前に綴りを当てない"
mutate "  if (!value.endsWith(\"Z\")) return false;" "  if (false) return false;" && run "offset の Z 固定を外す"
mutate "    (fraction === \"\" || ISO_SECFRAC_PATTERN.test(fraction))" "    true" && run "小数部の綴りを見ない"
mutate "  return value === undefined || isBlank(value) ? undefined : value;" "  return value;" && run "任意欄の空白を present として読む"
mutate "  return seq !== undefined && INGEST_SEQ_PATTERN.test(seq) ? seq : undefined;" "  return seq !== undefined && !isBlank(seq) ? seq : undefined;" && run "空白だけ弾いて語彙外は比較へ渡す（#35 FR-004）"
mutate "  if (isBlank(state.taskLineageId)) {" "  if (false) {" && run "状態側の空白 lineage を通す"
mutate "  if (event.taskLineageId !== undefined && isBlank(event.taskLineageId)) {" "  if (false) {" && run "event 側の空白 lineage を通す"
mutate "          ...(truncated.length === 0 ? [] : [truncationDiagnostic(event, truncated)])," "" && run "再配送 start の truncation 診断を落とす"
mutate "        .filter((pending) => !compatibleSet.has(pending))" "        .filter(() => false)" && run "飛ばした衝突兄弟を報告しない"

# --- #43 / #39: 消えた証跡の記録 --------------------------------------------
mutate "  const overflowed = kept.splice(0, Math.max(0, kept.length - CONTINUITY_LIMITS.arrayItems));" "  const overflowed = kept.splice(Math.min(kept.length, CONTINUITY_LIMITS.arrayItems));" && run "記録を末尾から落とす（FR-008）"
mutate "  const overflowed = kept.splice(0, Math.max(0, kept.length - CONTINUITY_LIMITS.arrayItems));" "  const overflowed: DroppedEvidenceEntryV1[] = [];" && run "記録の上限検査を外す（FR-015）"
mutate "              code: \"dropped_evidence_recorded\" as const," "              code: \"pending_operations_evicted\" as const," && run "記録の追加を別の診断で報告する（FR-009）"
mutate "      ...(overflowed.length === 0
        ? []
        : [" "      ...(true
        ? []
        : [" && run "記録の脱落を診断に出さない（FR-009）"
mutate "        sensitivity: pending.sensitivity," "        sensitivity: \"normal\" as const," && run "退避の記録で機密度を引き継がない"
mutate "              sensitivity: \"private\"," "              sensitivity: \"normal\"," && run "孤児の記録を normal で残す"
mutate "    return key === undefined || !recordedOrphans.has(key);" "    return true;" && run "孤児の記録を再送のたびに足す"
mutate "  const delivery = declared(entry.adapterDeliveryId);" "  const delivery = entry.eventId;" && run "孤児の重複判定を eventId で行う（再送 DoS）"
mutate "  if (delivery !== undefined) return \`d:\${delivery}\`;" "" && run "重複判定で配送鍵を見ず指紋だけにする（§8.2 の順を崩す）"
mutate "  return fingerprint === undefined ? undefined : \`f:\${fingerprint}\`;" "  return undefined;" && run "配送鍵の無い記録を同一性なしにする"
mutate "                : { adapterDeliveryId: event.adapterDeliveryId })," "                : {})," && run "孤児の記録に配送鍵を残さない"
mutate "          : { eventId: pending.correlation.startEventId })," "          : {})," && run "退避の記録に兄弟を判別できる識別子を残さない"
mutate "          : { eventId: pending.correlation.startEventId })," "          : { eventId: pending.sourceEventIds[0] })," && run "start を provenance 配列の先頭から取る"
mutate "        if (recorded.diagnostics.length > 0) {" "        if (recorded.added.length > 0) {" && run "刈っただけの修復を捨てる（FR-015）"
mutate "        if (recorded.diagnostics.length > 0) {" "        if (true) {" && run "足せていなくても状態を進める"
mutate "  const recordedFingerprint = recordedOrphanFingerprints(previous.state.droppedEvidence).get(key);" "  const recordedFingerprint = undefined;" && run "同じ配送鍵の指紋食い違いを黙って重複にする"
mutate "    recordedFingerprint !== undefined &&" "    true &&" && run "材料が欠けていても指紋の食い違いにする"
mutate "              terminalFingerprint: event.canonicalFingerprint," "" && run "孤児の記録に同一性の鍵を残さない"
mutate "        correlation.diagnostic === \"terminal_orphaned\" ||" "        false ||" && run "候補ゼロの terminal を状態に記録しない"
mutate "        correlation.diagnostic === \"terminal_unmatched\"" "        false" && run "開いた候補ゼロの unmatched を状態に記録しない"
mutate "      ],
      droppedEvidence: recorded.droppedEvidence," "      ]," && run "退避を状態に記録しない"
mutate "        ...recorded.diagnostics,
      ]," "      ]," && run "上限超えの復元状態を刈った事実を黙る（FR-015）"
mutate "  const carried = droppedEvidence ?? previous.droppedEvidence;" "  const carried = droppedEvidence;" && run "記録に触らない経路で記録を落とす"
mutate "    ...(carried === undefined || carried.length === 0 ? {} : { droppedEvidence: [...carried] })," "    ...(carried === undefined ? {} : { droppedEvidence: [...carried] })," && run "復元状態の空配列をそのまま残す（FR-013）"
mutate "    ...(carried === undefined || carried.length === 0 ? {} : { droppedEvidence: [...carried] })," "    ...(carried === undefined || carried.length === 0 ? {} : { droppedEvidence: carried })," && run "記録の配列を revision 間で共有する（§4.2）"
mutate "      lastIngestSeq: previous.state.lastIngestSeq," "" && run "記録だけの隔離で watermark を進める（§4.1）"
mutate "    lastIngestSeq: lastIngestSeq ?? maxIngestSeq(previous.lastIngestSeq, event.ingestSeq)," "    lastIngestSeq: maxIngestSeq(previous.lastIngestSeq, event.ingestSeq)," && run "呼び出し側が渡した watermark を無視する"

# --- #44: 受理した terminal の指紋 ------------------------------------------
mutate "              ...(status === \"unknown\" ? {} : { terminalFingerprint: event.canonicalFingerprint })," "" && run "受理した terminal の指紋を残さない（FR-010）"
mutate "              ...(status === \"unknown\" ? {} : { terminalFingerprint: event.canonicalFingerprint })," "              terminalFingerprint: event.canonicalFingerprint," && run "unknown に倒した operation にも指紋を残す"
mutate "    if (fingerprintConflict !== undefined) {" "    if (false) {" && run "指紋の衝突検査を外す（FR-011）"
mutate "      return stored !== undefined && stored !== incomingFingerprint;" "      return stored !== undefined;" && run "指紋が一致しても再配送として説明しない"
mutate "      return stored !== undefined && stored !== incomingFingerprint;" "      return stored !== incomingFingerprint;" && run "指紋を持たない旧い状態も衝突にする（FR-012）"
mutate "    const fingerprintUnexplained = plausible.every((pending) => {" "    const fingerprintUnexplained = plausible.some((pending) => {" && run "兄弟の 1 件が名乗っていれば全員分の衝突にする（FR-012 混在）"
mutate "  const storedFingerprint = declared(matched.terminalFingerprint);" "  const storedFingerprint: string | undefined = undefined;" && run "open な候補の指紋の食い違いを見ない（FR-011）"
mutate "  const storedFingerprint = declared(matched.terminalFingerprint);" "  const storedFingerprint = matched.terminalFingerprint;" && run "open な候補の空白の指紋を「違う指紋」と読む（FR-012）"
mutate "    terminalStatusOf(terminalEvent) !== \"unknown\"" "    true" && run "unknown に倒れる terminal でも指紋の食い違いで隔離する"
mutate "    storedFingerprint !== undefined &&" "    startIngestSeqOf(matched) !== undefined &&\n    storedFingerprint !== undefined &&" && run "指紋の衝突判定を順序材料がある場合だけにする"
mutate "    storedFingerprint !== undefined &&" "    rule === \"native_operation_id\" &&\n    storedFingerprint !== undefined &&" && run "指紋の衝突判定を rule 1 の terminal だけにする"
restore_all || exit 1
# --- 旧形 parity の門（SC-003）--------------------------------------------
# 比較面を緩める / corpus を実際より広く見せる、の 2 方向を潰す
mutate "    contentHash: step.contentHash," "    contentHash: undefined," "$PROJECTION" && run "旧形 parity の比較面から還元結果の hash を落とす"
mutate "    diagnostics: step.diagnostics," "    diagnostics: []," "$PROJECTION" && run "旧形 parity の比較面から診断を落とす"
mutate "    diagnostics: step.diagnostics," "    diagnostics: step.diagnostics.map((d) => ({ code: d.code }))," "$PROJECTION" && run "旧形 parity の診断を code だけに縮める"
mutate "    state: step.state," "    state: {}," "$PROJECTION" && run "旧形 parity の比較面から状態を落とす"
mutate "    state: step.state," "    state: (({ stateRevision: _dropped, ...rest }) => rest)(step.state)," "$PROJECTION" && run "旧形 parity の比較面から stateRevision を外す"
mutate "    history: step.history," "    history: []," "$PROJECTION" && run "旧形 parity の比較面から履歴を落とす"
mutate "      .map(([key, entry]) => ({ key, ...entry }))" "      .map(([key]) => ({ key }))" "$PROJECTION" && run "旧形 parity の台帳を鍵だけに縮める"
mutate "      \"name\": \"restored-orphan-terminal-redelivered\"," "      \"name\": \"restored-orphan-terminal-renamed\"," "$CORPUS" && run "旧形 corpus の case 名を許可表から外す"
mutate "          \"eventId\": \"event-terminal-orphan-again\",
          \"adapterDeliveryId\": \"delivery-terminal-orphan\"," "          \"eventId\": \"event-terminal-orphan-again\",
          \"adapterDeliveryId\": \"delivery-terminal-orphan-2\"," "$CORPUS" && run "旧形 corpus の再送を別の配送にすり替える"
mutate "            \"operationId\": \"op-filled-0\",
            \"correlation\": {
              \"operationId\": \"op-filled-0\",
              \"startEventId\": \"event-filled-1\"," "            \"operationId\": \"op-filled-1\",
            \"correlation\": {
              \"operationId\": \"op-filled-1\",
              \"startEventId\": \"event-filled-1\"," "$CORPUS" && run "旧形 corpus の退避 case から同名の兄弟を消す"

echo "--- 復元後 ---"
# 出力を目視するだけにしない。`node ... | grep` は grep の終了状態を返すので、`set -u` しか
# 立てていないこのスクリプトでは復元後の baseline が赤でも exit 0 になり、「全変異が kill された」
# だけを見て緑に見えてしまう。件数を取り出して 0 でなければ落とす
BASELINE=$(node --experimental-strip-types --test "${TESTS[@]}" 2>&1)
printf '%s\n' "$BASELINE" | grep -E '^ℹ (pass|fail) '
BASELINE_FAIL=$(printf '%s' "$BASELINE" | grep -E '^ℹ fail ' | tail -1 | grep -oE '[0-9]+$')
if [ -z "$BASELINE_FAIL" ] || [ "$BASELINE_FAIL" -ne 0 ]; then
  echo "変異テスト失敗: 復元後の baseline が green でない（変異が残ったか test が壊れている）" >&2
  exit 1
fi

# 集計は自己申告にしない。生存（fail 0）と、アンカーが外れて `&&` が短絡し黙って飛ばされた変異の
# 両方を数え、どちらかがあれば非ゼロで終わる。期待件数はこのスクリプト自身の `run` ラベル数から
# 数える（末尾の grep -v は、この数え方を説明している冒頭のコメント行自身を除くため）。
echo "--- 集計 ---"
EXPECTED=$(grep -oP '&& run "\K[^"]+' "$0" | grep -v '^\\K' | wc -l)
printf '実行 %d / 期待 %d、生存 %d\n' "$EXECUTED" "$EXPECTED" "$SURVIVED"
if [ "$EXECUTED" -ne "$EXPECTED" ] || [ "$SURVIVED" -ne 0 ]; then
  echo "変異テスト失敗: 生存した変異か、黙って飛ばされた変異がある" >&2
  exit 1
fi
