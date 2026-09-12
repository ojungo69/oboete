import assert from 'node:assert/strict';
import { test } from 'node:test';

import { guardLeadingBrace } from '../../src/injection/pack-format.js';

test('the leading-brace guard holds even when a caller hands it JSON', () => {
  assert.equal(guardLeadingBrace('{"hookSpecificOutput": 1}'), ' {"hookSpecificOutput": 1}');
  assert.equal(guardLeadingBrace('oboete memory context'), 'oboete memory context');
});
