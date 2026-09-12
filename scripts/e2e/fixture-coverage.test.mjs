import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { assertCoverage } from '../fixtures/fixture-coverage.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

test('the committed fixture satisfies the generator coverage contract', () => {
  const body = fs.readFileSync(path.join(ROOT, 'test/fixtures/events-1000.jsonl'), 'utf8');
  const events = body.trimEnd().split('\n').map((line) => JSON.parse(line));
  const readJsonl = (name) => fs.readFileSync(path.join(ROOT, 'test/corpus', name), 'utf8')
    .trimEnd().split('\n').filter(Boolean).map((line) => JSON.parse(line));
  const report = assertCoverage(events, readJsonl('secrets.jsonl'), readJsonl('directives.jsonl'), body);
  assert.equal(report.total, 1051);
  assert.deepEqual(report.byAgent, { claude: 255, codex: 271, grok: 263, pi: 262 });
});
