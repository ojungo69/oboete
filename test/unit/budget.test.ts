import assert from 'node:assert/strict';
import { test } from 'node:test';

import windowsDoc from '../../docs/research/context-windows.md';
import {
  CHANNEL_CAPS,
  charBudget,
  documentedWindow,
  normalizeModelId,
} from '../../src/injection/budget.js';

/**
 * The document is the source of truth (R12 "Context window"), so the expectations are read from it
 * here with a regex of their own instead of from the module under test.
 */
function verifiedWindows(): Map<string, number> {
  const rows = new Map<string, number>();
  for (const match of windowsDoc.matchAll(/^\| `([^`]+)` \| ([\d,]+) \|/gm)) {
    rows.set(match[1], Number(match[2].replaceAll(',', '')));
  }
  return rows;
}

function smallestWindows(): Map<string, number> {
  const rows = new Map<string, number>();
  for (const match of windowsDoc.matchAll(
    /^\| (Claude Code|Codex CLI|Grok Build|Pi) \| ([\d,]+) \|/gm,
  )) {
    rows.set(match[1], Number(match[2].replaceAll(',', '')));
  }
  return rows;
}

const AGENT_OF_LABEL = {
  'Claude Code': 'claude',
  'Codex CLI': 'codex',
  'Grok Build': 'grok',
  Pi: 'pi',
} as const;

/** The agent that reports a model id, by the prefix the catalog uses. */
function agentOf(modelId: string): 'claude' | 'codex' | 'grok' | 'pi' {
  if (modelId.startsWith('claude-')) return 'claude';
  if (modelId.startsWith('grok-')) return 'grok';
  return 'codex';
}

test('every verified window in the document round-trips through documentedWindow', () => {
  const expected = verifiedWindows();
  assert.ok(expected.size >= 10, 'the document should list the verified models');
  for (const [modelId, tokens] of expected) {
    assert.deepEqual(
      documentedWindow(agentOf(modelId), modelId),
      { tokens, known: true },
      `window of ${modelId}`,
    );
  }
});

test('the smallest verified window per agent is the fallback for an unknown model', () => {
  const smallest = smallestWindows();
  assert.equal(smallest.size, 4);
  for (const [label, tokens] of smallest) {
    const agent = AGENT_OF_LABEL[label as keyof typeof AGENT_OF_LABEL];
    assert.deepEqual(documentedWindow(agent, 'model-that-is-not-in-the-table'), {
      tokens,
      known: false,
    });
    assert.deepEqual(documentedWindow(agent, undefined), { tokens, known: false });
  }
});

test('the runtime id alias rules of the document are applied before the lookup', () => {
  assert.ok(windowsDoc.includes('`claude-opus-5[1m]`'));
  assert.ok(windowsDoc.includes('`grok-4.6-build`'));

  assert.equal(normalizeModelId('claude', 'claude-opus-5[1m]'), 'claude-opus-5');
  assert.equal(normalizeModelId('grok', 'grok-4.6-build'), 'grok-4.6');
  assert.equal(normalizeModelId('codex', 'gpt-5.6-sol'), 'gpt-5.6-sol');
  assert.equal(normalizeModelId('pi', 'gpt-5.6-luna'), 'gpt-5.6-luna');
  // The suffix belongs to one agent only: Grok Build never strips Claude Code's alias.
  assert.equal(normalizeModelId('grok', 'claude-opus-5[1m]'), 'claude-opus-5[1m]');

  assert.deepEqual(documentedWindow('claude', 'claude-opus-5[1m]'), {
    tokens: 1_000_000,
    known: true,
  });
  assert.deepEqual(documentedWindow('grok', 'grok-4.6-build'), { tokens: 500_000, known: true });
});

test('an agent with no verified window at all blocks its injection lane', () => {
  assert.equal(documentedWindow('unknown', 'anything'), null);
  assert.deepEqual(
    charBudget({
      agent: 'unknown',
      model: 'anything',
      channelCap: 10_000,
      contextFraction: 0.05,
      script: 'en',
    }),
    { chars: 0, windowUnknown: false, blocked: true },
  );
});

test('the budget is the smaller of the channel cap and the share of the window', () => {
  // Claude Code, English: 0.05 x 1,000,000 x 4 = 200,000 characters, cut by the 10,000 cap.
  assert.deepEqual(
    charBudget({
      agent: 'claude',
      model: 'claude-opus-5[1m]',
      channelCap: CHANNEL_CAPS.claude,
      contextFraction: 0.05,
      script: 'en',
    }),
    { chars: 10_000, windowUnknown: false, blocked: false },
  );

  // Codex, Japanese: 0.05 x 272,000 x 1.5 = 20,400 characters and no channel cap.
  assert.deepEqual(
    charBudget({
      agent: 'codex',
      model: 'gpt-5.6-sol',
      channelCap: CHANNEL_CAPS.codex,
      contextFraction: 0.05,
      script: 'cjk',
    }),
    { chars: 20_400, windowUnknown: false, blocked: false },
  );
});

test('the channel caps are the ones the agent contract documents', () => {
  assert.deepEqual(CHANNEL_CAPS, {
    claude: 10_000,
    grok: 10_000,
    codex: null,
    pi: null,
    unknown: null,
  });
});

test('an unknown model keeps the lane open and reports window_unknown', () => {
  const budget = charBudget({
    agent: 'codex',
    model: 'gpt-9.9-unreleased',
    channelCap: null,
    contextFraction: 0.05,
    script: 'en',
  });
  // 0.05 x 272,000 (the smallest verified Codex window) x 4.
  assert.deepEqual(budget, { chars: 54_400, windowUnknown: true, blocked: false });
});
