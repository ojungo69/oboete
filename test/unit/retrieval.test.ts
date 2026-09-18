import assert from 'node:assert/strict';
import { mkdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import { grantVisibility } from '../../src/db/queries.js';
import type { Line } from '../../src/fixture/replay.js';
import { runGet, searchMemories } from '../../src/memories-cli.js';
import { oboetePaths } from '../../src/paths.js';
import { resolveRepoIdentity } from '../../src/repo-identity.js';
import { buildMatch, cjkBigrams, isCjk, segmentQuery } from '../../src/retrieval/fts.js';
import { searchCandidates } from '../../src/retrieval/query.js';
import type { RankRow } from '../../src/retrieval/rank.js';
import {
  applyThreshold,
  charTrigramCosine,
  cutToBudget,
  mmrSelect,
  normalizeBm25,
  rankCandidates,
  rrfFuse,
} from '../../src/retrieval/rank.js';
import {
  buildFactSeedingPrompt,
  factSet,
  factStem,
  recallPrompt,
} from '../../scripts/e2e/probe-lib/isolated-agent.mjs';
import { repositoryRoot } from '../helpers/compile-cache.js';
import { withTempHome } from '../helpers/home.js';

const SCOPE_A = { where: 'm.repo_id = ? AND m.deleted_at IS NULL', params: ['repo_a'] };

function row(partial: Partial<RankRow> & Pick<RankRow, 'id'>): RankRow {
  return {
    title: partial.title ?? 't',
    body: partial.body ?? 'b',
    scoreTrigram: partial.scoreTrigram ?? null,
    scoreCjk: partial.scoreCjk ?? null,
    viaLike: partial.viaLike ?? false,
    pinned_at: partial.pinned_at ?? null,
    created_at: partial.created_at ?? 1,
    ...partial,
  };
}

function insertRepo(db: DatabaseSync, id: string, identity: string): void {
  db.prepare(
    `INSERT INTO repos (id, identity_kind, normalized_identity, created_at, last_seen_at)
     VALUES (?, 'common_dir', ?, 1, 1)`,
  ).run(id, identity);
}

function insertMemory(
  db: DatabaseSync,
  memory: { id: string; repoId: string; title: string; body: string; createdAt?: number; type?: string },
): void {
  const cjk = cjkBigrams(`${memory.title} ${memory.body}`);
  db.prepare(
    `INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, content_hash, sensitivity, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, 'local_only', ?)`,
  ).run(
    memory.id,
    memory.repoId,
    memory.type ?? 'discovery',
    memory.title,
    memory.body,
    cjk,
    `hash_${memory.id}`,
    memory.createdAt ?? 1,
  );
}

function insertSearchable(
  db: DatabaseSync,
  memory: {
    id: string;
    repoId: string;
    title: string;
    body: string;
    createdAt?: number;
    type?: string;
    validTo?: number | null;
    supersededBy?: string | null;
  },
): void {
  insertMemory(db, memory);
  grantVisibility(db, memory.id, { audience: 'project', repoId: memory.repoId }, 'migration', memory.createdAt ?? 1);
  db.prepare('UPDATE memories SET valid_to = ?, superseded_by = ? WHERE id = ?').run(
    memory.validTo ?? null,
    memory.supersededBy ?? null,
    memory.id,
  );
}

type FactTag = NonNullable<NonNullable<Line['tags']>['fact']>;

// Plain strings only. 15 fixture lines carry a tool result as `output: [byte, ...]` and two of them
// are fact-tagged, but each of those also carries the same sentence as a string, so decoding the
// bytes changes no fact's sentence. The assertion below names this helper if that ever stops being
// true, rather than sending the reader to the fixture.
function payloadStrings(payload: unknown): string[] {
  if (typeof payload === 'string') return [payload];
  if (payload === null || typeof payload !== 'object') return [];
  return Object.values(payload).flatMap(payloadStrings);
}

function fixtureFacts(): Array<FactTag & { sentence: string }> {
  const facts: Array<FactTag & { sentence: string }> = [];
  for (const raw of readFileSync(join(repositoryRoot(), 'test/fixtures/events-1000.jsonl'), 'utf8')
    .trim()
    .split('\n')) {
    const line = JSON.parse(raw) as Line;
    const fact = line.tags?.fact;
    if (fact === undefined) continue;
    const sentence = payloadStrings(line.payload)
      .flatMap((text) => text.split('\n'))
      .find((entry) => entry.includes(fact.expect));
    assert.ok(sentence !== undefined,
      `fact ${fact.id} has no payload line containing ${fact.expect}: either that line no longer carries the sentence, or it now carries it only as a tool output byte array, which payloadStrings does not decode`);
    facts.push({ ...fact, sentence });
  }
  return facts;
}

function seedSearchDb(db: DatabaseSync): void {
  insertRepo(db, 'repo_a', '/tmp/oboete-a');
  insertRepo(db, 'repo_b', '/tmp/oboete-b');
  insertMemory(db, {
    id: 'm_en_1',
    repoId: 'repo_a',
    title: 'Busy timeout',
    body: 'the busy timeout is 150 ms',
    createdAt: 10,
  });
  insertMemory(db, {
    id: 'm_en_2',
    repoId: 'repo_a',
    title: 'SQLite',
    body: 'SQLite stores application data.',
    createdAt: 11,
  });
  insertMemory(db, {
    id: 'm_en_3',
    repoId: 'repo_a',
    title: 'Open WAL journal',
    body: 'Write-ahead logging is ok for readers.',
    createdAt: 12,
  });
  insertMemory(db, {
    id: 'm_ja_1',
    repoId: 'repo_a',
    title: 'データベース接続',
    body: '接続プールを実装する。',
    createdAt: 20,
  });
  insertMemory(db, {
    id: 'm_ja_2',
    repoId: 'repo_a',
    title: '文字コード',
    body: 'UTF-8 で保存する実装。',
    createdAt: 21,
  });
  insertMemory(db, {
    id: 'm_ja_3',
    repoId: 'repo_a',
    title: 'トリガー',
    body: '挿入時に索引を更新する実装。',
    createdAt: 22,
  });
  insertMemory(db, {
    id: 'm_ja_4',
    repoId: 'repo_a',
    title: '接続文字列',
    body: '接続文字列は設定ファイルにある。',
    createdAt: 23,
  });
  insertMemory(db, {
    id: 'm_ja_noise',
    repoId: 'repo_a',
    title: '確定かどうかの判断',
    body: '条件を確認して決める。',
    createdAt: 24,
  });
  insertMemory(db, {
    id: 'm_b_1',
    repoId: 'repo_b',
    title: 'SQLite busy timeout',
    body: 'Should not appear in repo A search.',
    createdAt: 30,
  });
}

test('cjkBigrams indexes CJK runs and ignores latin words', () => {
  const terms = cjkBigrams('SQLite の busy timeout');
  assert.equal(terms.includes('busy'), false);
  assert.equal(terms, 'の');
});

test('cjkBigrams emits overlapping bigrams for a Japanese compound', () => {
  assert.equal(cjkBigrams('データベース接続'), 'デー ータ タベ ベー ース ス接 接続');
});

test('cjkBigrams treats prolonged sound, iteration and middle-dot as CJK', () => {
  assert.equal(isCjk('ー'), true);
  assert.equal(isCjk('々'), true);
  assert.equal(isCjk('・'), true);
  assert.equal(cjkBigrams('ー々・'), 'ー々 々・');
});

test('segmentQuery sends latin words of three or more characters to trigram', () => {
  assert.deepEqual(segmentQuery('sqlite busy timeout'), {
    trigram: ['sqlite', 'busy', 'timeout'],
    cjk: [],
    like: [],
  });
});

test('segmentQuery drops natural-language stop words', () => {
  assert.deepEqual(segmentQuery('What is the SQLite busy timeout?'), {
    trigram: ['sqlite', 'busy', 'timeout'],
    cjk: [],
    like: [],
  });
});

test('segmentQuery keeps sqlite as trigram, bigrams the remaining Japanese, and drops の', () => {
  assert.deepEqual(segmentQuery('SQLiteのビジータイムアウト'), {
    trigram: ['sqlite'],
    cjk: ['ビジ', 'ジー', 'ータ', 'タイ', 'イム', 'ムア', 'アウ', 'ウト'],
    like: [],
  });
});

test('segmentQuery drops single-character CJK runs', () => {
  assert.deepEqual(segmentQuery('接続文字列はどこ？'), {
    trigram: [],
    cjk: ['接続', '続文', '文字', '字列', 'どこ'],
    like: [],
  });
});

test('segmentQuery sends a two-letter word to LIKE only', () => {
  assert.deepEqual(segmentQuery('ok'), { trigram: [], cjk: [], like: ['ok'] });
});

test('segmentQuery uses only the longest short term as its LIKE fallback', () => {
  assert.deepEqual(segmentQuery('x db'), { trigram: [], cjk: [], like: ['db'] });
});

test('segmentQuery counts non-CJK length in code points', () => {
  assert.deepEqual(segmentQuery('𐍈𐍈'), { trigram: [], cjk: [], like: ['𐍈𐍈'] });
});

test('segmentQuery drops particles and leaves every list empty', () => {
  assert.deepEqual(segmentQuery('は が を に で と の も へ や か ね よ な'), {
    trigram: [],
    cjk: [],
    like: [],
  });
});

test('segmentQuery caps indexed terms at 128 and keeps the longest terms when capped', () => {
  const query = [
    ...Array.from({ length: 129 }, (_, index) => `term${index}`),
    'exceptionallylongdiagnosticterm',
  ].join(' ');
  const terms = segmentQuery(query);
  assert.equal(terms.trigram.length + terms.cjk.length + terms.like.length, 128);
  assert.ok(terms.trigram.includes('exceptionallylongdiagnosticterm'));
});

test('buildMatch OR-joins quoted terms and returns null without terms', () => {
  assert.equal(buildMatch(['safe', 'NOT', 'say"hi', 'prefix*']), '"safe" OR "NOT" OR "say""hi" OR "prefix*"');
  assert.equal(buildMatch([]), null);
});

test('searchCandidates finds English rows of repo A only', async () => {
  await withTempHome((home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      seedSearchDb(opened.db);
      const result = searchCandidates(opened.db, {
        text: 'sqlite busy timeout',
        scope: SCOPE_A,
      });
      const ids = result.rows.map((item) => item.id).sort();
      assert.equal(result.usedLike, false);
      assert.deepEqual(result.terms.trigram, ['sqlite', 'busy', 'timeout']);
      assert.ok(ids.includes('m_en_1'));
      assert.ok(ids.includes('m_en_2'));
      assert.equal(ids.includes('m_en_3'), false);
      assert.equal(ids.includes('m_b_1'), false);
      assert.equal(ids.some((id) => id.startsWith('m_ja_')), false);
    } finally {
      opened.db.close();
    }
  });
});

test('searchCandidates ranks a natural-language busy-timeout match above SQLite alone', async () => {
  await withTempHome((home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      seedSearchDb(opened.db);
      const result = searchCandidates(opened.db, {
        text: 'What is the SQLite busy timeout?',
        scope: SCOPE_A,
      });
      const busyIndex = result.rows.findIndex((item) => item.id === 'm_en_1');
      const sqliteIndex = result.rows.findIndex((item) => item.id === 'm_en_2');
      assert.ok(busyIndex >= 0);
      assert.ok(sqliteIndex >= 0);
      assert.ok(busyIndex < sqliteIndex);
      assert.equal(result.rows[busyIndex]?.body, 'the busy timeout is 150 ms');
    } finally {
      opened.db.close();
    }
  });
});

test('searchCandidates keeps relevant terms after a long pasted prefix', async () => {
  await withTempHome((home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      seedSearchDb(opened.db);
      const filler = Array.from({ length: 30 }, (_, index) => `filler${index}`).join(' ');
      const result = searchCandidates(opened.db, {
        text: `${filler} why does the hook hit the busy timeout?`,
        scope: SCOPE_A,
      });
      assert.ok(result.terms.trigram.includes('busy'));
      assert.ok(result.terms.trigram.includes('timeout'));
      assert.ok(result.rows.some((item) => item.id === 'm_en_1'));
    } finally {
      opened.db.close();
    }
  });
});

test('searchCandidates finds Japanese rows for a Japanese query', async () => {
  await withTempHome((home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      seedSearchDb(opened.db);
      const result = searchCandidates(opened.db, { text: '実装', scope: SCOPE_A });
      const ids = result.rows.map((item) => item.id).sort();
      assert.equal(result.usedLike, false);
      assert.deepEqual(ids, ['m_ja_1', 'm_ja_2', 'm_ja_3']);
      assert.ok(result.rows.every((item) => item.scoreCjk !== null && item.viaLike === false));
    } finally {
      opened.db.close();
    }
  });
});

test('searchCandidates finds a connection string from a natural Japanese prompt', async () => {
  await withTempHome((home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      seedSearchDb(opened.db);
      const result = searchCandidates(opened.db, { text: '接続文字列はどこ？', scope: SCOPE_A });
      assert.ok(result.rows.some((item) => item.id === 'm_ja_4'));
      assert.equal(result.terms.cjk.includes('列'), false);
    } finally {
      opened.db.close();
    }
  });
});

test('searchCandidates treats a trailing Japanese particle as a term boundary', async () => {
  await withTempHome((home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      seedSearchDb(opened.db);
      const result = searchCandidates(opened.db, { text: '設定か', scope: SCOPE_A });
      const ids = result.rows.map((item) => item.id);
      assert.deepEqual(result.terms.cjk, ['設定']);
      assert.ok(ids.includes('m_ja_4'));
      assert.equal(ids.includes('m_ja_noise'), false);
    } finally {
      opened.db.close();
    }
  });
});

test('searchCandidates finds English and Japanese rows from a mixed prompt', async () => {
  await withTempHome((home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      seedSearchDb(opened.db);
      const result = searchCandidates(opened.db, { text: 'busy timeout 接続', scope: SCOPE_A });
      assert.ok(result.rows.some((item) => item.id === 'm_en_1'));
      assert.ok(result.rows.some((item) => item.id === 'm_ja_4'));
    } finally {
      opened.db.close();
    }
  });
});

test('searchCandidates uses LIKE for a two-letter query', async () => {
  await withTempHome((home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      seedSearchDb(opened.db);
      const result = searchCandidates(opened.db, { text: 'ok', scope: SCOPE_A });
      assert.equal(result.usedLike, true);
      assert.deepEqual(result.rows.map((item) => item.id), ['m_en_3']);
      assert.equal(result.rows[0]?.viaLike, true);
      assert.equal(result.rows[0]?.scoreTrigram, null);
      assert.equal(result.rows[0]?.scoreCjk, null);
    } finally {
      opened.db.close();
    }
  });
});

test('searchCandidates does not prepare SQL for a stop-word-and-punctuation-only query', async () => {
  await withTempHome((home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      seedSearchDb(opened.db);
      let prepareCalls = 0;
      const wrapped = new Proxy(opened.db, {
        get(target, property) {
          if (property === 'prepare') {
            return (...args: Parameters<DatabaseSync['prepare']>) => {
              prepareCalls += 1;
              return target.prepare(...args);
            };
          }
          const value = Reflect.get(target, property, target);
          return typeof value === 'function' ? value.bind(target) : value;
        },
      });
      const result = searchCandidates(wrapped, {
        text: 'the is what how of to in a an and or は が を に の で と も へ！？',
        scope: SCOPE_A,
      });
      assert.equal(result.usedLike, false);
      assert.deepEqual(result.rows, []);
      assert.deepEqual(result.terms, { trigram: [], cjk: [], like: [] });
      assert.equal(prepareCalls, 0);
    } finally {
      opened.db.close();
    }
  });
});

test('searchCandidates does not throw on FTS5 syntax', async () => {
  await withTempHome((home) => {
    const opened = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1000 });
    try {
      seedSearchDb(opened.db);
      const match = buildMatch(['NOT', '"', '*']);
      assert.ok(match !== null);
      assert.doesNotThrow(() =>
        opened.db.prepare('SELECT rowid FROM memories_fts WHERE memories_fts MATCH ?').all(match),
      );
      assert.doesNotThrow(() =>
        searchCandidates(opened.db, { text: 'NOT " *', scope: SCOPE_A }),
      );
    } finally {
      opened.db.close();
    }
  });
});

test('normalizeBm25 gives 1.0 to the best score', () => {
  const out = normalizeBm25(
    [
      row({ id: 'best', scoreTrigram: -10 }),
      row({ id: 'mid', scoreTrigram: -5 }),
      row({ id: 'weak', scoreTrigram: -2 }),
    ],
    'scoreTrigram',
  );
  assert.equal(out.find((item) => item.id === 'best')?.normTrigram, 1);
  assert.equal(out.find((item) => item.id === 'mid')?.normTrigram, 0.5);
  assert.equal(out.find((item) => item.id === 'weak')?.normTrigram, 0.2);
});

test('applyThreshold drops a row below 0.3 and keeps LIKE-only last', () => {
  const normalized = normalizeBm25(
    [
      row({ id: 'best', scoreTrigram: -10 }),
      row({ id: 'weak', scoreTrigram: -2 }),
      row({ id: 'like', viaLike: true, title: 'ok', body: 'ok' }),
    ],
    'scoreTrigram',
  );
  const { kept, dropped } = applyThreshold(normalized, 0.3);
  assert.deepEqual(
    dropped.map((item) => item.id),
    ['weak'],
  );
  assert.deepEqual(
    kept.map((item) => item.id),
    ['best', 'like'],
  );
});

test('rrfFuse ranks a row present in both tables above a row in one', () => {
  const fused = rrfFuse([
    row({
      id: 'both',
      scoreTrigram: -10,
      scoreCjk: -8,
      normTrigram: 1,
      normCjk: 1,
    }),
    row({
      id: 'one',
      scoreTrigram: -9,
      scoreCjk: null,
      normTrigram: 0.9,
      normCjk: 0,
    }),
    row({ id: 'like', viaLike: true, title: 'ok', body: 'ok' }),
  ]);
  const both = fused.find((item) => item.id === 'both')?.score_rrf ?? 0;
  const one = fused.find((item) => item.id === 'one')?.score_rrf ?? 0;
  assert.ok(both > one);
  assert.equal(both, 1 / 61 + 1 / 61);
  assert.equal(one, 1 / 62);
  assert.equal(fused.find((item) => item.id === 'like')?.score_rrf, 0);
});

test('mmrSelect rejects a near-duplicate with reason mmr_redundant', () => {
  const original = row({
    id: 'orig',
    title: 'sqlite busy timeout',
    body: 'the busy timeout is five seconds',
    score_rrf: 0.03,
    scoreTrigram: -10,
  });
  const duplicate = row({
    id: 'dup',
    title: 'sqlite busy timeout',
    body: 'the busy timeout is five seconds',
    score_rrf: 0.029,
    scoreTrigram: -9,
  });
  const other = row({
    id: 'other',
    title: 'wal mode readers',
    body: 'writers append to the log',
    score_rrf: 0.02,
    scoreTrigram: -4,
  });
  assert.ok(charTrigramCosine(`${original.title} ${original.body}`, `${duplicate.title} ${duplicate.body}`) > 0.99);
  const { selected, rejected } = mmrSelect([original, duplicate, other], { lambda: 0.5, limit: 3 });
  assert.deepEqual(
    selected.map((item) => item.id),
    ['orig', 'other'],
  );
  assert.deepEqual(
    rejected.map((item) => ({ id: item.row.id, reason: item.reason })),
    [{ id: 'dup', reason: 'mmr_redundant' }],
  );
});

test('mmrSelect keeps multiple LIKE-only rows up to the limit', () => {
  const first = row({
    id: 'like_a',
    title: 'ok one',
    body: 'alpha',
    viaLike: true,
    score_rrf: 0,
  });
  const second = row({
    id: 'like_b',
    title: 'ok two',
    body: 'omega',
    viaLike: true,
    score_rrf: 0,
  });
  const { selected, rejected } = mmrSelect([first, second], { lambda: 0.5, limit: 2 });
  assert.deepEqual(
    selected.map((item) => item.id).sort(),
    ['like_a', 'like_b'],
  );
  assert.deepEqual(rejected, []);
});

test('cutToBudget omits overflow rows with reason budget', () => {
  const first = row({ id: 'first', title: 'aa', body: 'bb' });
  const second = row({ id: 'second', title: 'cccc', body: 'dddd' });
  const { included, omitted } = cutToBudget([first, second], 8);
  assert.deepEqual(
    included.map((item) => item.id),
    ['first'],
  );
  assert.deepEqual(
    omitted.map((item) => ({ id: item.row.id, reason: item.reason })),
    [{ id: 'second', reason: 'budget' }],
  );
});

test('rankCandidates returns bm25, rrf and mmr scores on included rows', () => {
  const result = rankCandidates(
    [
      row({ id: 'both', title: 'alpha one', body: 'unique alpha body', scoreTrigram: -10, scoreCjk: -8 }),
      row({ id: 'weak', title: 'zzzz', body: 'no overlap here', scoreTrigram: -1, scoreCjk: null }),
      row({
        id: 'dup',
        title: 'alpha one',
        body: 'unique alpha body',
        scoreTrigram: -9,
        scoreCjk: -7,
      }),
    ],
    { threshold: 0.3, lambda: 0.5, budgetChars: 10_000, limit: 10 },
  );
  assert.ok(result.included.length >= 1);
  for (const item of result.included) {
    assert.equal(typeof item.score_bm25, 'number');
    assert.equal(typeof item.score_rrf, 'number');
    assert.equal(typeof item.score_mmr, 'number');
  }
  assert.ok(result.included.every((item) => item.id !== 'weak'));
  assert.ok(result.omitted.some((item) => item.id === 'weak' && item.reason === 'below_threshold'));
  assert.ok(result.omitted.some((item) => item.id === 'dup' && item.reason === 'mmr_redundant'));
});

// Pin: measured on current product code (title = fact id, body = sentence) through searchMemories.
const MEASURED_FIRST_RANK_COUNT = 39;

test('searchMemories returns each events-1000 fact among the first five through the search surface', async () => {
  const facts = fixtureFacts();
  assert.equal(facts.length, 40);
  assert.equal(facts.filter((fact) => fact.lang === 'ja').length, 20);
  assert.equal(facts.filter((fact) => fact.lang === 'en').length, 20);

  await withTempHome((home) => {
    const paths = oboetePaths(home);
    const opened = openDatabase({ path: paths.db, timeoutMs: 1000 });
    try {
      insertRepo(opened.db, 'repo_a', '/tmp/oboete-a');
      for (const fact of facts) {
        insertSearchable(opened.db, {
          id: fact.id,
          repoId: 'repo_a',
          title: fact.id,
          body: fact.sentence,
        });
      }
      const placements = facts.map((fact) => {
        const ranked = searchMemories(opened.db, {
          repoId: 'repo_a',
          paths,
          query: fact.query,
          limit: 50,
        });
        const position = ranked.findIndex((row) => row.id === fact.id);
        const above = position > 0 ? ranked.slice(0, position).map((row) => row.id) : [];
        return { id: fact.id, query: fact.query, position, above };
      });
      for (const fact of placements) {
        assert.ok(
          fact.position >= 0 && fact.position < 5,
          `fact ${fact.id} query ${fact.query} position ${fact.position < 0 ? 'absent' : String(fact.position)} above [${fact.above.join(', ')}]`,
        );
      }
      const notFirst = placements.filter((fact) => fact.position !== 0);
      const firstCount = facts.length - notFirst.length;
      const notFirstText =
        notFirst.length === 0
          ? '(none)'
          : notFirst
              .map((fact) => `${fact.id} query ${fact.query} position ${fact.position} above [${fact.above.join(', ')}]`)
              .join('; ');
      assert.ok(
        firstCount >= MEASURED_FIRST_RANK_COUNT,
        `first-rank count ${firstCount} (measured ${MEASURED_FIRST_RANK_COUNT}); not first: ${notFirstText}`,
      );
    } finally {
      opened.db.close();
    }
  });
});

test('rankCandidates ignores created_at when trigram and cjk scores are equal', () => {
  const older = 1;
  const newer = Date.now();
  const alpha = row({
    id: 'a_old',
    title: 'Hydrazine tank',
    body: 'The hydrazine tank uses a burst disk.',
    scoreTrigram: -2,
    scoreCjk: -2,
    created_at: older,
  });
  const omega = row({
    id: 'z_new',
    title: 'Kerosene pump',
    body: 'The kerosene pump runs at two thousand RPM.',
    scoreTrigram: -2,
    scoreCjk: -2,
    created_at: newer,
  });
  const options = { threshold: 0.3, lambda: 0.5, budgetChars: 10_000, limit: 10 };
  const first = rankCandidates([alpha, omega], options);
  const swapped = rankCandidates(
    [
      { ...alpha, created_at: newer },
      { ...omega, created_at: older },
    ],
    options,
  );
  const expected = [alpha.id, omega.id].sort();
  assert.deepEqual(first.included.map((item) => item.id), expected);
  assert.deepEqual(swapped.included.map((item) => item.id), expected);
});

test('searchMemories returns a relevant older fact among newer unrelated memories', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    const opened = openDatabase({ path: paths.db, timeoutMs: 1000 });
    try {
      insertRepo(opened.db, 'repo_a', '/tmp/oboete-a');
      insertSearchable(opened.db, {
        id: 'm_old',
        repoId: 'repo_a',
        title: 'Busy timeout',
        body: 'The busy timeout for hooks is 150 ms.',
        createdAt: 1,
      });
      insertSearchable(opened.db, {
        id: 'm_new',
        repoId: 'repo_a',
        title: 'SQLite',
        body: 'SQLite stores application data.',
        createdAt: Date.now(),
      });
      insertSearchable(opened.db, {
        id: 'm_newer',
        repoId: 'repo_a',
        title: 'WAL journal',
        body: 'Write-ahead logging is ok for readers.',
        createdAt: Date.now(),
      });
      const found = searchMemories(opened.db, {
        repoId: 'repo_a',
        paths,
        query: 'What is the SQLite busy timeout?',
        limit: 10,
      });
      assert.equal(
        found[0]?.id,
        'm_old',
        `older fact not first; returned ${found.map((row) => row.id).join(', ') || '(none)'}`,
      );
    } finally {
      opened.db.close();
    }
  });
});

test('searchMemories hides a superseded fact unless history is requested', async () => {
  await withTempHome(async (home) => {
    const repo = join(home, 'repos', 'current');
    mkdirSync(repo, { recursive: true });
    const identity = resolveRepoIdentity(repo);
    const paths = oboetePaths(home);
    const opened = openDatabase({ path: paths.db, timeoutMs: 1000 });
    try {
      insertRepo(opened.db, identity.id, identity.normalizedIdentity);
      insertSearchable(opened.db, {
        id: 'm_current',
        repoId: identity.id,
        title: 'Busy timeout',
        body: 'The busy timeout is 2000 ms.',
        createdAt: 20,
      });
      insertSearchable(opened.db, {
        id: 'm_old',
        repoId: identity.id,
        title: 'Busy timeout',
        body: 'The busy timeout is 150 ms.',
        createdAt: 10,
        validTo: 15,
        supersededBy: 'm_current',
      });
      const query = 'busy timeout';
      const current = searchMemories(opened.db, { repoId: identity.id, paths, query, limit: 10 });
      const currentIds = current.map((row) => row.id).join(', ') || '(none)';
      assert.ok(current.some((row) => row.id === 'm_current'), `current fact absent: ${currentIds}`);
      assert.equal(current.some((row) => row.id === 'm_old'), false, `superseded fact returned: ${currentIds}`);
      const withHistory = searchMemories(opened.db, {
        repoId: identity.id,
        paths,
        query,
        limit: 10,
        history: true,
      });
      const withHistoryIds = withHistory.map((row) => row.id).join(', ') || '(none)';
      assert.ok(withHistory.some((row) => row.id === 'm_current'), `current fact absent with history: ${withHistoryIds}`);
      assert.ok(withHistory.some((row) => row.id === 'm_old'), `superseded fact absent with history: ${withHistoryIds}`);
    } finally {
      opened.db.close();
    }
    let stdout = '';
    let stderr = '';
    const status = await runGet(['m_old', '--history', '--json'], {
      cwd: repo,
      writeOut: (text) => {
        stdout += text;
      },
      writeError: (text) => {
        stderr += text;
      },
    });
    assert.equal(status, 0, stderr || stdout);
    const historical = JSON.parse(stdout) as { valid_to?: number | null; superseded_by?: string | null };
    assert.ok(historical.valid_to != null, `valid_to missing or null in get --history --json: ${stdout}`);
    assert.equal(historical.superseded_by, 'm_current', `superseded_by in get --history --json: ${stdout}`);
  });
});

test('searchMemories returns two distinct facts that share a title when they are the only candidates', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    const opened = openDatabase({ path: paths.db, timeoutMs: 1000 });
    try {
      insertRepo(opened.db, 'repo_a', '/tmp/oboete-a');
      insertSearchable(opened.db, {
        id: 'm_hooks',
        repoId: 'repo_a',
        title: 'Busy timeout',
        body: 'The busy timeout for hooks is 150 ms.',
      });
      insertSearchable(opened.db, {
        id: 'm_cli',
        repoId: 'repo_a',
        title: 'Busy timeout',
        body: 'The busy timeout for the CLI is 2000 ms.',
      });
      const found = searchMemories(opened.db, { repoId: 'repo_a', paths, query: 'busy timeout', limit: 10 });
      assert.deepEqual(found.map((row) => row.id).sort(), ['m_cli', 'm_hooks']);
    } finally {
      opened.db.close();
    }
  });
});

/**
 * The pair the artifact below reproduces: the `claude-to-codex` leg of that run, named the way
 * `scripts/e2e/isolated-user.mjs` names it. This does not migrate if `factStem` changes — the
 * seeding prompt below spells the stem out, so a change there fails this file instead. That is the
 * behaviour a frozen run record wants: the artifact must keep saying what the run said.
 */
const PAIR_STEM = factStem('2026-09-17T15-05-08-894Z', 'claude', 'codex');

const PAIR_FACTS: [string, string, string] = [
  `${PAIR_STEM}-1: the build token is cedar.`,
  `${PAIR_STEM}-2: the release bird is heron.`,
  `${PAIR_STEM}-3: 配布色は琥珀。`,
];

/** `recallPrompt('codex', false)`: what the receiving agent was asked. */
const PAIR_RECALL_PROMPT = [
  'Before the tool call, remember the fact lines already present inside the oboete memory context markers.',
  "Use the shell tool exactly once to run: sed -n '1,20p' NOTES.md",
  'Make no other tool call.',
  'After the result, reply with every remembered fact line verbatim, joined by |. Do not derive the answer from NOTES.md.',
].join('\n');

/** `buildFactSeedingPrompt(PAIR_FACTS)`: what the sending agent was asked, and what its free summary quotes. */
const PAIR_SEEDING_PROMPT = [
  'These three exact strings are durable facts about this repository. Preserve them verbatim:',
  ...PAIR_FACTS,
  'Use exactly one tool call and no other tools. In that one call, use the shell tool to run:',
  "printf '%s\\n' 'fact-2026-09-17T15-05-08-894Z-claude-to-codex-1: the build token is cedar.'"
    + " 'fact-2026-09-17T15-05-08-894Z-claude-to-codex-2: the release bird is heron.'"
    + " 'fact-2026-09-17T15-05-08-894Z-claude-to-codex-3: 配布色は琥珀。' >> NOTES.md",
  'After the tool result, reply on one line with the same three exact strings joined by |.',
].join('\n');

// Runs whether or not the artifact below is skipped: a reword in the probe library must not leave
// that corpus reproducing a prompt no agent sends. The comparison is exact, against what the library
// actually returns, so a change to the prompt text fails it — but only once the change is built.
// The import is static and esbuild inlines it, so `npm test` sees an edit to the source and a bare
// `node --test build/...` does not.
test('the pinned pair prompts are still the ones the probe library sends', () => {
  assert.deepEqual(factSet(PAIR_STEM), PAIR_FACTS);
  assert.equal(recallPrompt('codex', false), PAIR_RECALL_PROMPT);
  assert.equal(buildFactSeedingPrompt(PAIR_FACTS), PAIR_SEEDING_PROMPT);
  // The pair's facts hold no apostrophe, so they cannot show whether the command is shell-quoted.
  // Find the line rather than index it: a line added above the command would otherwise fail this
  // with a diff between two unrelated prompt lines instead of naming the quoting rule.
  assert.equal(
    buildFactSeedingPrompt(["it's a", 'b', 'c'] as const).split('\n').find((line) => line.startsWith('printf ')),
    String.raw`printf '%s\n' 'it'\''s a' 'b' 'c' >> NOTES.md`,
  );
});

// Artifact for issue #275: the five memories of the `claude-to-codex` pair of the
// 2026-09-17T15-05-08-894Z dogfood run (JST 2026-09-18) as they stood when the receiving prompt
// pack was built, copied verbatim from that pair's database (the run's copy of it is
// `/var/tmp/oboete-dogfood-upgrade/all0917/claude-to-codex/memory.db`, verified row for row on
// 2026-09-17; that copy is the dogfood account's and the cron keeps writing to it, so these rows are
// the frozen ones). `m_fact` carries the three facts the recall prompt asks for and the pack dropped
// it as `below_threshold`, keeping `m_confirm`, which carries none of them.
//
// Keep all five rows; quickstart E12 Limits says what trimming the two summaries would change.
//
// What it does not model: the pair's checkpoint is work-scoped in production, while these rows take
// the file's ordinary project grant and are excluded by `m.type <> 'session_summary'` alone; the
// three searchable rows were `feature`, `decision` and `feature` in the run and are `discovery`
// here, which nothing in retrieval reads; and all five rows share one `created_at`, so nothing here
// can show a recency- or retirement-driven drop.
//
// Skipped until #275 is fixed. Un-skip it with the fix. On its own this test cannot tell a real
// fix from one that lowers the threshold until everything is admitted, so #275 carries the
// counter-pin it has to land with: an unrelated memory in the same corpus must still be omitted.
// The counter-pin is not written here because it is the fix's evidence, not this artifact's.
test('searchMemories returns the fact-bearing memory of a five-row corpus', { skip: 'issue #275' }, async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    const opened = openDatabase({ path: paths.db, timeoutMs: 1000 });
    try {
      insertRepo(opened.db, 'repo_a', '/tmp/oboete-a');
      insertSearchable(opened.db, {
        id: 'm_checkpoint',
        repoId: 'repo_a',
        type: 'session_summary',
        title: 'Record three exact strings as durable facts in NOTES.md.',
        body:
          'Purpose\nRecord three exact strings as durable facts in NOTES.md.\n\nConstraints\n' +
          '- Preserve the three exact strings verbatim.\n- Use exactly one tool call.\n' +
          '- Append the strings to NOTES.md.\n\nDecisions\n' +
          '- The three exact strings will be appended to NOTES.md.\n\nOutstanding\n' +
          '- Verify the contents of NOTES.md to ensure the strings were written correctly.',
      });
      insertSearchable(opened.db, {
        id: 'm_fact',
        repoId: 'repo_a',
        title: 'Durable facts recorded to NOTES.md',
        body:
          'Three exact strings were written to NOTES.md to serve as durable facts about the repository. ' +
          `The strings are: '${PAIR_FACTS[0]}', '${PAIR_FACTS[1]}', and '${PAIR_FACTS[2]}'.`,
      });
      insertSearchable(opened.db, {
        id: 'm_decision',
        repoId: 'repo_a',
        title: 'Use NOTES.md for durable facts',
        body: 'A decision was made to append the three exact strings to NOTES.md to preserve them as durable facts about the repository.',
      });
      insertSearchable(opened.db, {
        id: 'm_confirm',
        repoId: 'repo_a',
        title: 'Assistant message confirms fact strings',
        body: "The assistant's final message contained the three exact strings joined by a pipe character (|), confirming the successful execution of the tool call.",
      });
      // The sending session's free summary: the seeding prompt the agent received, with the batch's
      // empty accounting tail, and the truncated title the summariser gave it.
      insertSearchable(opened.db, {
        id: 'm_request',
        repoId: 'repo_a',
        type: 'session_summary',
        title:
          'These three exact strings are durable facts about this repository. Preserve them verbatim:\n' +
          'fact-2026-09-17T15-05-08-894Z',
        body:
          `request: ${PAIR_SEEDING_PROMPT}\ninvestigated:\nlearned: Assistant message confirms fact strings, ` +
          'Durable facts recorded to NOTES.md, Use NOTES.md for durable facts\ncompleted:\nnext_steps:',
      });
      const found = searchMemories(opened.db, { repoId: 'repo_a', paths, query: PAIR_RECALL_PROMPT, limit: 10 });
      assert.ok(
        found.some((row) => row.id === 'm_fact'),
        `fact-bearing memory absent; returned ${found.map((row) => row.id).join(', ') || '(none)'}`,
      );
    } finally {
      opened.db.close();
    }
  });
});
