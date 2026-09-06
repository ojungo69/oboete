// Ranking over lexical candidates (research.md R5): normalized BM25, RRF, MMR, budget cut.

export type RankRow = {
  id: string;
  title: string;
  body: string;
  pinned_at?: number | null;
  created_at?: number | null;
  scoreTrigram: number | null;
  scoreCjk: number | null;
  viaLike: boolean;
  normTrigram?: number;
  normCjk?: number;
  score_bm25?: number;
  score_rrf?: number;
  score_mmr?: number;
};

export type OmitReason = 'below_threshold' | 'mmr_redundant' | 'budget';

export type OmittedItem = { id: string; reason: OmitReason };

const DEFAULT_THRESHOLD = 0.3;
const DEFAULT_LAMBDA = 0.5;
const DEFAULT_RRF_K = 60;

function isLikeOnly(row: RankRow): boolean {
  return row.viaLike && row.scoreTrigram === null && row.scoreCjk === null;
}

function ratio(score: number | null, best: number): number {
  if (score === null || best === 0) return 0;
  return score / best;
}

export function normalizeBm25<K extends 'scoreTrigram' | 'scoreCjk'>(
  rows: readonly RankRow[],
  key: K,
): RankRow[] {
  let best = 0;
  let seen = false;
  for (const row of rows) {
    const score = row[key];
    if (score === null) continue;
    if (!seen || score < best) {
      best = score;
      seen = true;
    }
  }
  return rows.map((row) =>
    key === 'scoreTrigram'
      ? { ...row, normTrigram: seen ? ratio(row.scoreTrigram, best) : 0 }
      : { ...row, normCjk: seen ? ratio(row.scoreCjk, best) : 0 },
  );
}

function bestNorm(row: RankRow): number {
  return Math.max(row.normTrigram ?? 0, row.normCjk ?? 0);
}

export function applyThreshold(
  rows: readonly RankRow[],
  threshold = DEFAULT_THRESHOLD,
): { kept: RankRow[]; dropped: RankRow[] } {
  const scored: RankRow[] = [];
  const like: RankRow[] = [];
  const dropped: RankRow[] = [];
  for (const row of rows) {
    if (isLikeOnly(row)) {
      like.push({ ...row, score_bm25: 0 });
      continue;
    }
    const score = bestNorm(row);
    if (score >= threshold) scored.push({ ...row, score_bm25: score });
    else dropped.push(row);
  }
  return { kept: [...scored, ...like], dropped };
}

function compareId(a: string, b: string): number {
  if (a < b) return -1;
  if (a > b) return 1;
  return 0;
}

function ranksFor(
  rows: readonly RankRow[],
  normKey: 'normTrigram' | 'normCjk',
  rawKey: 'scoreTrigram' | 'scoreCjk',
): Map<string, number> {
  const list = rows
    .filter((row) => !isLikeOnly(row) && row[rawKey] !== null)
    .sort((a, b) => {
      const delta = (b[normKey] ?? 0) - (a[normKey] ?? 0);
      return delta !== 0 ? delta : compareId(a.id, b.id);
    });
  const ranks = new Map<string, number>();
  for (const [index, row] of list.entries()) ranks.set(row.id, index + 1);
  return ranks;
}

export function rrfFuse(rows: readonly RankRow[], k = DEFAULT_RRF_K): RankRow[] {
  const trigramRanks = ranksFor(rows, 'normTrigram', 'scoreTrigram');
  const cjkRanks = ranksFor(rows, 'normCjk', 'scoreCjk');
  return rows.map((row) => {
    if (isLikeOnly(row)) return { ...row, score_rrf: 0 };
    let score = 0;
    const trigramRank = trigramRanks.get(row.id);
    const cjkRank = cjkRanks.get(row.id);
    if (trigramRank !== undefined) score += 1 / (k + trigramRank);
    if (cjkRank !== undefined) score += 1 / (k + cjkRank);
    return { ...row, score_rrf: score };
  });
}

function trigramCounts(text: string): Map<string, number> {
  const chars = [...text.toLowerCase()];
  const counts = new Map<string, number>();
  for (let i = 0; i + 2 < chars.length; i++) {
    const gram = chars[i] + chars[i + 1] + chars[i + 2];
    counts.set(gram, (counts.get(gram) ?? 0) + 1);
  }
  return counts;
}

function cosineCounts(
  left: Map<string, number>,
  right: Map<string, number>,
  a: string,
  b: string,
): number {
  if (left.size === 0 && right.size === 0) {
    return a.toLowerCase() === b.toLowerCase() ? 1 : 0;
  }
  if (left.size === 0 || right.size === 0) return 0;
  let dot = 0;
  let normLeft = 0;
  let normRight = 0;
  for (const [gram, weight] of left) {
    normLeft += weight * weight;
    const other = right.get(gram);
    if (other !== undefined) dot += weight * other;
  }
  for (const weight of right.values()) normRight += weight * weight;
  const denom = Math.sqrt(normLeft) * Math.sqrt(normRight);
  return denom === 0 ? 0 : dot / denom;
}

export function charTrigramCosine(a: string, b: string): number {
  return cosineCounts(trigramCounts(a), trigramCounts(b), a, b);
}

function packedText(row: RankRow): string {
  return `${row.title} ${row.body}`;
}

export function mmrSelect(
  rows: readonly RankRow[],
  options: { lambda?: number; limit?: number },
): { selected: RankRow[]; rejected: { row: RankRow; reason: 'mmr_redundant' }[] } {
  const lambda = options.lambda ?? DEFAULT_LAMBDA;
  const limit = options.limit ?? rows.length;
  const maxRrf = rows.reduce((best, row) => Math.max(best, row.score_rrf ?? 0), 0);
  const remaining = rows.map((row) => {
    const text = packedText(row);
    return { row, text, counts: trigramCounts(text), maxSim: 0 };
  });
  const selected: RankRow[] = [];
  const rejected: { row: RankRow; reason: 'mmr_redundant' }[] = [];

  while (remaining.length > 0) {
    let bestIndex = 0;
    let bestMmr = Number.NEGATIVE_INFINITY;
    for (let i = 0; i < remaining.length; i++) {
      const item = remaining[i];
      const relevance = maxRrf > 0 ? (item.row.score_rrf ?? 0) / maxRrf : 0;
      const sim = selected.length > 0 ? item.maxSim : 0;
      const mmr = lambda * relevance - (1 - lambda) * sim;
      if (
        mmr > bestMmr ||
        (mmr === bestMmr && compareId(item.row.id, remaining[bestIndex].row.id) < 0)
      ) {
        bestMmr = mmr;
        bestIndex = i;
      }
    }

    const [picked] = remaining.splice(bestIndex, 1);
    const relevance = maxRrf > 0 ? (picked.row.score_rrf ?? 0) / maxRrf : 0;
    const bestSim = selected.length > 0 ? picked.maxSim : 0;
    // R5: LIKE never votes, so rrf is 0; do not treat any cosine > 0 as redundancy.
    const redundant =
      maxRrf > 0 &&
      selected.length > 0 &&
      bestSim > 0 &&
      (1 - lambda) * bestSim >= lambda * relevance;
    if (redundant || selected.length >= limit) {
      rejected.push({ row: picked.row, reason: 'mmr_redundant' });
      continue;
    }
    selected.push({ ...picked.row, score_mmr: bestMmr });
    for (const item of remaining) {
      item.maxSim = Math.max(
        item.maxSim,
        cosineCounts(item.counts, picked.counts, item.text, picked.text),
      );
    }
  }

  return { selected, rejected };
}

function rowCost(row: RankRow): number {
  // contracts/agents.md pack lines: two '> ' prefixes (2 + 2).
  return row.title.length + row.body.length + 4;
}

export function cutToBudget(
  rows: readonly RankRow[],
  budgetChars: number,
): { included: RankRow[]; omitted: { row: RankRow; reason: 'budget' }[] } {
  const included: RankRow[] = [];
  const omitted: { row: RankRow; reason: 'budget' }[] = [];
  let used = 0;
  for (const row of rows) {
    const cost = rowCost(row);
    if (used + cost <= budgetChars) {
      included.push(row);
      used += cost;
    } else {
      omitted.push({ row, reason: 'budget' });
    }
  }
  return { included, omitted };
}

export type RankOptions = {
  threshold?: number;
  lambda?: number;
  budgetChars?: number;
  limit?: number;
};

export type RankedCandidate = RankRow & {
  score_bm25: number;
  score_rrf: number;
  score_mmr: number;
};

export function rankCandidates(
  candidates: readonly RankRow[],
  options: RankOptions = {},
): { included: RankedCandidate[]; omitted: OmittedItem[] } {
  const withTrigram = normalizeBm25(candidates, 'scoreTrigram');
  const withBoth = normalizeBm25(withTrigram, 'scoreCjk');
  const { kept, dropped } = applyThreshold(withBoth, options.threshold);
  const omitted: OmittedItem[] = dropped.map((row) => ({ id: row.id, reason: 'below_threshold' }));
  const fused = rrfFuse(kept);
  const { selected, rejected } = mmrSelect(fused, {
    lambda: options.lambda,
    limit: options.limit,
  });
  for (const item of rejected) omitted.push({ id: item.row.id, reason: 'mmr_redundant' });
  const { included, omitted: budgetOmitted } = cutToBudget(
    selected,
    options.budgetChars ?? Number.POSITIVE_INFINITY,
  );
  for (const item of budgetOmitted) omitted.push({ id: item.row.id, reason: 'budget' });
  return {
    included: included.map((row) => ({
      ...row,
      score_bm25: row.score_bm25 ?? bestNorm(row),
      score_rrf: row.score_rrf ?? 0,
      score_mmr: row.score_mmr ?? 0,
    })),
    omitted,
  };
}
