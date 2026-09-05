import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';

import {
  contentHash,
  materialHash,
  memoryIdFor,
  normalizeForIdentity,
} from '../../src/db/identity.js';

/**
 * The expected hashes are built here from literal normalized strings, so a test never asks the
 * module under test what its own answer should be (docs/dev/conventions.md "Tests").
 */
function sha256OfParts(parts: string[]): string {
  return createHash('sha256').update(JSON.stringify(parts), 'utf8').digest('hex');
}

test('normalizeForIdentity applies NFKC, trims, collapses whitespace, and lowercases', () => {
  assert.equal(normalizeForIdentity('  Ｆｉｘ\tthe\n\n SQLite  timeout '), 'fix the sqlite timeout');
  assert.equal(normalizeForIdentity('Cafe\u0301'), 'café');
  assert.equal(normalizeForIdentity(' ｶﾞ　ｸﾞ '), 'ガ グ');
  assert.equal(normalizeForIdentity(''), '');
});

test('the material hash ignores whitespace, casing, and Unicode form', () => {
  const expected = sha256OfParts(['fix the sqlite busy timeout', 'the hook retries once.']);

  assert.equal(materialHash('Fix the SQLite busy timeout', 'The hook retries once.'), expected);
  assert.equal(
    materialHash('  fix   the\tSQLITE  busy\ntimeout ', 'The  hook\nretries once. '),
    expected,
  );
  assert.equal(
    materialHash('Ｆｉｘ the ＳＱＬｉｔｅ busy timeout', 'The hook retries once.'),
    expected,
  );
});

test('the material hash equates composed and decomposed titles', () => {
  // 'Cafe\u0301' is the decomposed form of the composed 'Café' on the next line.
  const expected = sha256OfParts(['café note', 'body']);

  assert.equal(materialHash('Cafe\u0301 note', 'body'), expected);
  assert.equal(materialHash('Café note', 'body'), expected);
});

test('a different body gives a different material hash', () => {
  const one = materialHash('Same title', 'The hook retries once.');
  const other = materialHash('Same title', 'The hook retries twice.');

  assert.equal(one, sha256OfParts(['same title', 'the hook retries once.']));
  assert.notEqual(one, other);
});

test('the title and the body cannot bleed into each other', () => {
  // JSON.stringify([...parts]) keeps the parts separated (conventions "Identifiers, hashes, time").
  assert.notEqual(materialHash('ab', 'c'), materialHash('a', 'bc'));
});

test('the observation type is not an input to the material hash (A13)', () => {
  const title = 'Deleted decision';
  const body = 'This content must not resurrect under another type.';

  // The two rows differ only in observation type, which the signature cannot even accept.
  const asBugfix = materialHash(title, body);
  const asDecision = materialHash(title, body);

  assert.equal(asBugfix, asDecision);
  assert.equal(asBugfix, sha256OfParts(['deleted decision', 'this content must not resurrect under another type.']));
  assert.equal(materialHash.length, 2);
});

test('the content hash separates repositories while the material hash stays equal', () => {
  const material = materialHash('Shared title', 'Shared body');
  const inRepoA = contentHash('aaaaaaaaaaaaaaa1', material);
  const inRepoB = contentHash('bbbbbbbbbbbbbbb2', material);

  assert.equal(material, sha256OfParts(['shared title', 'shared body']));
  assert.equal(inRepoA, sha256OfParts(['aaaaaaaaaaaaaaa1', material]));
  assert.notEqual(inRepoA, inRepoB);
});

test('memoryIdFor takes the first 24 hex characters of the content hash', () => {
  const hash = sha256OfParts(['aaaaaaaaaaaaaaa1', sha256OfParts(['title', 'body'])]);
  const id = memoryIdFor(hash);

  assert.match(id, /^m_[0-9a-f]{24}$/);
  assert.equal(id, `m_${hash.slice(0, 24)}`);
});
