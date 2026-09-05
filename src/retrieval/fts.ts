// Lexical query routing for FTS5 trigram + CJK bigram (research.md R5, FR-025).

const CJK_CHAR =
  /^(?:\p{Script=Han}|\p{Script=Hiragana}|\p{Script=Katakana}|\p{Script=Hangul}|[ー々・\uFF70\uFF65])$/u;

const STOP_WORDS = new Set([
  'the',
  'is',
  'what',
  'how',
  'of',
  'to',
  'in',
  'a',
  'an',
  'and',
  'or',
  'は',
  'が',
  'を',
  'に',
  'で',
  'と',
  'の',
  'も',
  'へ',
  'や',
  'か',
  'ね',
  'よ',
  'な',
]);
const MAX_QUERY_TERMS = 128;

const wordSegmenter = new Intl.Segmenter(undefined, { granularity: 'word' });

export function isCjk(char: string): boolean {
  return CJK_CHAR.test(char);
}

export function cjkBigrams(text: string): string {
  const terms: string[] = [];
  let run = '';
  const flush = (): void => {
    const chars = [...run];
    if (chars.length === 1) {
      terms.push(run);
    } else if (chars.length >= 2) {
      for (let i = 0; i + 1 < chars.length; i++) {
        terms.push(chars[i] + chars[i + 1]);
      }
    }
    run = '';
  };
  for (const char of text) {
    if (isCjk(char)) run += char;
    else flush();
  }
  flush();
  return terms.join(' ');
}

export type QueryTerms = {
  trigram: string[];
  cjk: string[];
  like: string[];
};

function codePointLength(text: string): number {
  return [...text].length;
}

function isAllCjk(text: string): boolean {
  if (text.length === 0) return false;
  for (const char of text) {
    if (!isCjk(char)) return false;
  }
  return true;
}

function splitByScript(text: string): string[] {
  const parts: string[] = [];
  let buf = '';
  let bufCjk: boolean | undefined;
  for (const char of text) {
    const cjk = isCjk(char);
    if (buf.length === 0 || cjk === bufCjk) {
      buf += char;
      bufCjk = cjk;
    } else {
      parts.push(buf);
      buf = char;
      bufCjk = cjk;
    }
  }
  if (buf.length > 0) parts.push(buf);
  return parts;
}

export function segmentQuery(text: string): QueryTerms {
  const trigram: string[] = [];
  const cjk: string[] = [];
  const like: string[] = [];
  let likeFallback = '';

  const pushIndexed = (terms: string[], term: string): void => {
    if (terms.includes(term)) return;
    terms.push(term);
  };
  const pushCjkRun = (run: string): void => {
    if (codePointLength(run) < 2) return;
    for (const term of cjkBigrams(run).split(' ')) {
      if (term.length > 0) pushIndexed(cjk, term);
    }
  };

  for (const run of splitByScript(text)) {
    if (isAllCjk(run)) {
      let searchable = '';
      for (const { segment, isWordLike } of wordSegmenter.segment(run)) {
        if (!isWordLike || STOP_WORDS.has(segment)) {
          pushCjkRun(searchable);
          searchable = '';
        } else {
          searchable += segment;
        }
      }
      pushCjkRun(searchable);
      continue;
    }
    for (const { segment, isWordLike } of wordSegmenter.segment(run)) {
      if (!isWordLike) continue;
      const lower = segment.toLowerCase();
      if (STOP_WORDS.has(lower)) continue;
      const length = codePointLength(lower);
      if (length >= 3) pushIndexed(trigram, lower);
      else if (length > codePointLength(likeFallback)) likeFallback = lower;
    }
  }

  if (trigram.length === 0 && cjk.length === 0 && likeFallback.length > 0) {
    like.push(likeFallback);
  }

  const indexed = [...trigram, ...cjk];
  const kept =
    indexed.length > MAX_QUERY_TERMS
      ? new Set(
          indexed
            .sort((left, right) => codePointLength(right) - codePointLength(left))
            .slice(0, MAX_QUERY_TERMS),
        )
      : null;

  return {
    trigram: kept === null ? trigram : trigram.filter((term) => kept.has(term)),
    cjk: kept === null ? cjk : cjk.filter((term) => kept.has(term)),
    like,
  };
}

export function buildMatch(terms: string[]): string | null {
  const quoted = terms
    .filter((term) => term.length > 0)
    .map((term) => `"${term.replaceAll('"', '""')}"`);
  return quoted.length > 0 ? quoted.join(' OR ') : null;
}
