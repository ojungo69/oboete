import type {
  ContinuityDiagnosticV1,
  IdempotencyLedger,
  LedgerEntryV1,
  WorkStateRevisionEntryV1,
} from "./reference-model.ts";

/**
 * 旧形入力（新しい任意欄を 1 つも持たない状態と event）に対する**振る舞い**の比較面。
 *
 * SC-003 は「旧形の入力に対する結果は #35 / #39 / #43 / #44 の是正以外変わらない」と言うが、
 * 「変わらない」を行数や issue 番号で語ると検証できない主張になる（測ってから書く）。ここでは
 * 比較面を構造から導く: 還元結果の**全体**をそのまま通し、比較のために形を変えるのは
 * 台帳（`Map`）を鍵で整列した配列に直す 1 か所だけにする。
 *
 * **欄を選び直さない**のが要点。`diagnostics` を `code` だけに、`history` と台帳を件数だけに
 * 縮約すると、`detail` の作り替えや台帳の中身の入れ替えが差分として見えなくなる（実際に
 * `terminal_order_unverifiable` の `detail` を壊しても全 test が緑のままだった）。ここは
 * 素通しにしておき、何が変わってよいかは呼び出し側の許可表だけで決める。
 *
 * hash も比較面に**残す**: 還元結果そのものが返す `contentHash`、状態の `stateRevision`、
 * `history` 各行の `contentHash` の 3 つ。状態が動いた step では当然食い違うので許可表に
 * 実測値で載る。逆に**状態が動かない step では一致しなければならず**、そこが「旧形入力では
 * 何も変わっていない」の一番強い証拠になる。返り値の `contentHash` を落としていた間は、
 * 隔離が返す `contentHash` を壊しても全 test が緑のままだった。
 */
export interface OldShapeLedgerRowV1 extends LedgerEntryV1 {
  /** 台帳の鍵（idempotency key）。`Map` の並びは挿入順なので、比較のために鍵で整列する */
  key: string;
}

export interface OldShapeOutcomeV1 {
  outcome: string;
  /** 還元結果が返す hash。`finalizeAbandonedState` は返さないので任意 */
  contentHash?: string;
  diagnostics: readonly ContinuityDiagnosticV1[];
  /** 状態そのもの。`stateRevision` も新しい任意欄も差分として見えるよう残す */
  state: Record<string, unknown>;
  history: readonly WorkStateRevisionEntryV1[];
  ledger: readonly OldShapeLedgerRowV1[];
}

export interface OldShapeStepInput {
  outcome: string;
  contentHash?: string;
  diagnostics: readonly ContinuityDiagnosticV1[];
  state: Record<string, unknown>;
  history: readonly WorkStateRevisionEntryV1[];
  ledger: IdempotencyLedger;
}

export function projectOldShape(step: OldShapeStepInput): OldShapeOutcomeV1 {
  return {
    outcome: step.outcome,
    contentHash: step.contentHash,
    diagnostics: step.diagnostics,
    state: step.state,
    history: step.history,
    ledger: [...step.ledger]
      .map(([key, entry]) => ({ key, ...entry }))
      .sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0)),
  };
}

/**
 * 2 つの projection が食い違う JSON path を列挙する。allowlist は issue 番号ではなく
 * **この path** で書く（「#43 の分は許す」では、#43 が新しく作る経路を数え落としても気づけない）。
 */
export function diffPaths(before: unknown, after: unknown, path = ""): string[] {
  if (Object.is(before, after)) return [];
  const bothArrays = Array.isArray(before) && Array.isArray(after);
  const bothObjects = isPlainObject(before) && isPlainObject(after);
  if (!bothArrays && !bothObjects) return [path === "" ? "." : path];
  if (bothArrays) {
    const paths: string[] = [];
    for (let i = 0; i < Math.max(before.length, after.length); i += 1) {
      paths.push(...diffPaths(before[i], after[i], `${path}[${i}]`));
    }
    return paths;
  }
  const b = before as Record<string, unknown>;
  const a = after as Record<string, unknown>;
  const paths: string[] = [];
  for (const key of [...new Set([...Object.keys(b), ...Object.keys(a)])].sort()) {
    paths.push(...diffPaths(b[key], a[key], path === "" ? key : `${path}.${key}`));
  }
  return paths;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** `diffPaths` が返す形の path を辿る。allowlist が「差分の中身」まで固定するために使う */
export function valueAtPath(root: unknown, path: string): unknown {
  let current: unknown = root;
  for (const token of path.split(".")) {
    const [key, ...indices] = token.split("[");
    if (key !== "") {
      if (!isPlainObject(current)) return undefined;
      current = current[key];
    }
    for (const index of indices) {
      if (!Array.isArray(current)) return undefined;
      current = current[Number.parseInt(index, 10)];
    }
  }
  return current;
}
