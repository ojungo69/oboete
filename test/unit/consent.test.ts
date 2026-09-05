import assert from 'node:assert/strict';
import { chmodSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { test } from 'node:test';

import { configSchema, consentHash, consentTuple, loadConfig, type OboeteConfig } from '../../src/config.js';
import { ensureDirectories, oboetePaths } from '../../src/paths.js';
import {
  consentDisplay,
  credentialGuidance,
  decideConsent,
  saveConsent,
  saveProviderPreset,
} from '../../src/setup/consent.js';
import { withTempHome } from '../helpers/home.js';

const CLOUDFLARE = {
  OBOETE_CF_API_TOKEN: 'token-value',
  OBOETE_CF_ACCOUNT_ID: 'account-value',
};

function config(overrides: Record<string, unknown> = {}): OboeteConfig {
  return configSchema.parse({ observer: { preset: 'workers-ai' }, ...overrides });
}

test('the consent display states host, credential source, cost class and egress classes', () => {
  const lines = consentDisplay(config(), CLOUDFLARE).join('\n');
  assert.match(lines, /api\.cloudflare\.com/);
  assert.match(lines, /OBOETE_CF_API_TOKEN/);
  assert.match(lines, /free-tier/);
  assert.match(lines, /eligible/);
});

test('the agent-cli display names the subscription the preset consumes', () => {
  const lines = consentDisplay(
    configSchema.parse({ observer: { preset: 'agent-cli', agent_cli: 'grok' } }),
    {},
  ).join('\n');
  assert.match(lines, /subscription/);
  assert.match(lines, /grok/);
});

test('--yes is refused after the host, the credential source or an egress class changed', () => {
  const live = config();
  const tuple = consentTuple(live, CLOUDFLARE);
  const changes = [
    { host: 'ai.example.com' },
    { credentialSource: 'env:OBOETE_OPENROUTER_API_KEY' },
    { egressClasses: ['eligible', 'private'] },
  ];
  for (const change of changes) {
    const stored = config({ consent: { hash: consentHash({ ...tuple, ...change }), accepted_at: 1 } });
    assert.equal(
      decideConsent({ config: stored, env: CLOUDFLARE, yes: true }).state,
      'missing',
      `a stored record of ${JSON.stringify(change)} does not accept the tuple shown now`,
    );
  }

  const matching = config({ consent: { hash: consentHash(tuple), accepted_at: 1 } });
  assert.equal(decideConsent({ config: matching, env: CLOUDFLARE, yes: true }).state, 'accepted');
});

test('--yes without a stored record is refused and --accept-egress accepts the tuple shown', () => {
  const live = config();
  assert.equal(decideConsent({ config: live, env: CLOUDFLARE, yes: true }).state, 'missing');

  const decision = decideConsent({ config: live, env: CLOUDFLARE, acceptEgress: true });
  assert.equal(decision.state, 'accepted');
  assert.equal(decision.hash, consentHash(consentTuple(live, CLOUDFLARE)));
});

test('a preset that sends nothing off this machine needs no consent', () => {
  for (const preset of ['ollama', 'none']) {
    const local = configSchema.parse({ observer: { preset } });
    assert.equal(decideConsent({ config: local, env: {} }).state, 'not_required');
  }
});

test('the consent record and the preset are stored without disturbing other settings', async () => {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    writeFileSync(
      paths.config,
      ['[injection]', 'threshold = 0.4', '', '[privacy]', 'secret_paths = ["deploy/**"]', ''].join('\n'),
    );

    saveProviderPreset(paths, 'workers-ai');
    const decision = decideConsent({ config: loadConfig(paths), env: CLOUDFLARE, acceptEgress: true });
    saveConsent(paths, decision.hash, 1_700_000_000_000);

    const stored = loadConfig(paths);
    assert.equal(stored.observer.preset, 'workers-ai');
    assert.equal(stored.consent.hash, decision.hash);
    assert.equal(stored.consent.accepted_at, 1_700_000_000_000);
    assert.equal(stored.injection.threshold, 0.4);
    assert.deepEqual(stored.privacy.secret_paths, ['deploy/**']);
    assert.equal(decideConsent({ config: stored, env: CLOUDFLARE, yes: true }).state, 'accepted');
    assert.match(readFileSync(paths.config, 'utf8'), /threshold/);
  });
});

test('missing Workers AI credentials produce the free-account steps and the three alternatives', () => {
  const guidance = credentialGuidance(config(), {}).join('\n');
  assert.match(guidance, /https:\/\/dash\.cloudflare\.com\/sign-up/);
  assert.match(guidance, /https:\/\/dash\.cloudflare\.com\/profile\/api-tokens/);
  assert.match(guidance, /OBOETE_CF_API_TOKEN/);
  assert.match(guidance, /--provider ollama/);
  assert.match(guidance, /--provider agent-cli/);
  assert.match(guidance, /without a provider/);

  assert.deepEqual(credentialGuidance(config(), CLOUDFLARE), [], 'credentials present: nothing to say');
});

test('a preset with an API key names its own variable', () => {
  const guidance = credentialGuidance(configSchema.parse({ observer: { preset: 'gemini' } }), {}).join('\n');
  assert.match(guidance, /OBOETE_GEMINI_API_KEY/);
  assert.doesNotMatch(guidance, /dash\.cloudflare\.com/);
});

test('the configuration keeps its mode when the consent record is written under a tight umask', () => {
  return withTempHome(async (home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    writeFileSync(paths.config, '[observer]\npreset = "ollama"\n');
    chmodSync(paths.config, 0o644);

    const previous = process.umask(0o077);
    try {
      saveConsent(paths, 'stored-hash', 1_700_000_000_000);
    } finally {
      process.umask(previous);
    }

    assert.equal(statSync(paths.config).mode & 0o777, 0o644, 'the mode is the file\u2019s, not the umask\u2019s');
    assert.equal(loadConfig(paths).consent.hash, 'stored-hash');
  });
});
