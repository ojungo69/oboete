import assert from 'node:assert/strict';
import { readFileSync, statSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import {
  ConfigError,
  EGRESS_CLASSES,
  PRESET_CATALOG,
  RepoConfigError,
  configSchema,
  consentHash,
  consentMatches,
  consentTuple,
  isPaused,
  loadConfig,
  loadRepoRules,
  readCredentials,
  MAX_REPO_SECRET_PATHS,
  MAX_REPO_SECRET_PATH_LENGTH,
} from '../../src/config.js';
import { appendLog, scrubCredentials } from '../../src/log.js';
import { ensureDirectories, oboetePaths, resolveHome } from '../../src/paths.js';
import { withTempHome } from '../helpers/home.js';

// Computed independently of the implementation with node:crypto:
// sha256(JSON.stringify([preset, host, credentialSource, costClass, egressClasses])).
const WORKERS_AI_CONSENT = '1fed0e50f04d41bd99be4d2c145d162ed3ca5079d0b305f60c745688327b5474';
const AGENT_CLI_CLAUDE_CONSENT = 'a03b982a20d085555fdaca58941ac61daae7ccb31a432f4c2ac5e0fec88a0c70';

function caught(fn: () => unknown): unknown {
  try {
    fn();
  } catch (error) {
    return error;
  }
  return assert.fail('the call was expected to throw');
}

const workersAiTuple = {
  preset: 'workers-ai',
  host: 'api.cloudflare.com',
  credentialSource: 'env:OBOETE_CF_API_TOKEN+OBOETE_CF_ACCOUNT_ID',
  costClass: 'free-tier',
  egressClasses: ['eligible'],
};

test('resolveHome honours OBOETE_HOME and otherwise falls back to ~/.oboete', async () => {
  await withTempHome((home) => {
    assert.equal(resolveHome(), home);
    assert.equal(resolveHome({ OBOETE_HOME: home }), home);
  });
  assert.equal(resolveHome({}), join(homedir(), '.oboete'));
  assert.equal(resolveHome({ OBOETE_HOME: '   ' }), join(homedir(), '.oboete'));
});

// Every process resolves the data directory for itself: the hook in the agent's working directory,
// the worker in its own. A relative override anchored to the current directory would give them
// different directories, and FR-039 has exactly one.
test('a relative OBOETE_HOME is the same directory whatever the working directory is', () => {
  const fromHere = resolveHome({ OBOETE_HOME: 'memories' });
  assert.equal(fromHere, join(homedir(), 'memories'));
  const previous = process.cwd();
  process.chdir(tmpdir());
  try {
    assert.equal(resolveHome({ OBOETE_HOME: 'memories' }), fromHere);
  } finally {
    process.chdir(previous);
  }
});

test('ensureDirectories creates the data directories and keeps the home directory private', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(join(home, 'data'));
    ensureDirectories(paths);
    for (const directory of [paths.home, paths.spool, paths.piAck, paths.logs]) {
      assert.equal(statSync(directory).isDirectory(), true, directory);
    }
    assert.equal(statSync(paths.home).mode & 0o777, 0o700);
    assert.equal(paths.config, join(paths.home, 'config.toml'));
    assert.equal(paths.db, join(paths.home, 'memory.db'));
    assert.equal(paths.hookLog, join(paths.logs, 'hook.log'));
    assert.equal(paths.observeLog, join(paths.logs, 'observe.log'));
    assert.equal(paths.paused, join(paths.home, 'paused'));
  });
});

test('loadConfig returns the defaults when there is no config file', async () => {
  await withTempHome((home) => {
    assert.deepEqual(loadConfig(oboetePaths(home)), {
      observer: { preset: 'workers-ai', agent_cli: 'claude' },
      injection: { context_fraction: 0.05, threshold: 0.3 },
      privacy: { secret_paths: [] },
      consent: {},
    });
  });
});

test('a complete config file round-trips through loadConfig', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    writeFileSync(
      paths.config,
      [
        '[observer]',
        'preset = "nim"',
        'model = "meta/llama-3.2-11b-vision-instruct"',
        'agent_cli = "codex"',
        '',
        '[injection]',
        'context_fraction = 0.1',
        'threshold = 0.5',
        '',
        '[privacy]',
        'secret_paths = ["deploy/*.pem", "**/.env"]',
        '',
        '[consent]',
        `hash = "${WORKERS_AI_CONSENT}"`,
        'accepted_at = 1756900000000',
        '',
      ].join('\n'),
    );
    assert.deepEqual(loadConfig(paths), {
      observer: { preset: 'nim', model: 'meta/llama-3.2-11b-vision-instruct', agent_cli: 'codex' },
      injection: { context_fraction: 0.1, threshold: 0.5 },
      privacy: { secret_paths: ['deploy/*.pem', '**/.env'] },
      consent: { hash: WORKERS_AI_CONSENT, accepted_at: 1756900000000 },
    });
  });
});

// A rule that cannot be compiled reaches the detector otherwise, where the blanket catch answers
// `detector_error` for every event that carries a path: one bad rule would blank every capture.
test('a path rule that cannot be compiled is refused where it is read', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    writeFileSync(paths.config, '[privacy]\nsecret_paths = ["deploy/[z-a].pem"]\n');
    assert.throws(
      () => loadConfig(paths),
      (error: unknown) => error instanceof ConfigError && error.code === 'config_malformed',
    );

    writeFileSync(join(home, '.oboete.toml'), '[privacy]\nsecret_paths = ["[z-a]"]\n');
    assert.throws(
      () => loadRepoRules(home),
      (error: unknown) => error instanceof RepoConfigError && error.code === 'repo_config_malformed',
    );

    // The rule the broken one was trying to be still loads.
    writeFileSync(join(home, '.oboete.toml'), '[privacy]\nsecret_paths = ["[a-z].pem"]\n');
    assert.deepEqual(loadRepoRules(home), { secretPaths: ['[a-z].pem'] });
  });
});

test('a malformed config file throws ConfigError instead of applying part of it', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    writeFileSync(paths.config, '[observer]\npreset = "nim"\n[injection\n');
    assert.throws(
      () => loadConfig(paths),
      (error: unknown) => error instanceof ConfigError && error.code === 'config_malformed',
    );
  });
});

test('an unknown config key is rejected', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    writeFileSync(paths.config, '[observer]\nprovider = "nim"\n');
    assert.throws(
      () => loadConfig(paths),
      (error: unknown) => error instanceof ConfigError && error.code === 'config_malformed',
    );
  });
});

test('a value out of range is rejected', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    writeFileSync(paths.config, '[injection]\ncontext_fraction = 0.9\n');
    assert.throws(() => loadConfig(paths), ConfigError);
    writeFileSync(paths.config, '[injection]\ncontext_fraction = 0.0\n');
    assert.throws(() => loadConfig(paths), ConfigError);
    writeFileSync(paths.config, '[injection]\nthreshold = 1.5\n');
    assert.throws(() => loadConfig(paths), ConfigError);
  });
});

test('a credential in the config file is refused and the message names the environment variables', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    for (const text of [
      '[credentials]\ncf_api_token = "token-value"\n',
      '[observer]\napi_key = "token-value"\n',
      '[observer]\nsecret = "token-value"\n',
    ]) {
      writeFileSync(paths.config, text);
      const error = caught(() => loadConfig(paths));
      assert.ok(error instanceof ConfigError, text);
      assert.equal(error.code, 'config_credentials');
      assert.match(error.message, /environment variable/);
      assert.match(error.message, /OBOETE_/);
      assert.equal(error.message.includes('token-value'), false);
    }
  });
});

test('readCredentials reads only the OBOETE_ variables named for the preset', () => {
  assert.deepEqual(readCredentials('workers-ai', { OBOETE_CF_API_TOKEN: ' token ', OBOETE_CF_ACCOUNT_ID: 'account' }), {
    kind: 'cloudflare',
    present: true,
    source: 'env:OBOETE_CF_API_TOKEN+OBOETE_CF_ACCOUNT_ID',
    values: { token: 'token', accountId: 'account' },
  });
  // Workers AI needs both variables.
  assert.equal(readCredentials('workers-ai', { OBOETE_CF_API_TOKEN: 'token' }).present, false);
  assert.equal(readCredentials('workers-ai', { OBOETE_CF_ACCOUNT_ID: 'account' }).present, false);
  // The empty string counts as absent.
  assert.equal(readCredentials('nim', { OBOETE_NIM_API_KEY: '   ' }).present, false);
  assert.deepEqual(readCredentials('nim', { OBOETE_NIM_API_KEY: 'nvapi-1' }), {
    kind: 'api-key',
    present: true,
    source: 'env:OBOETE_NIM_API_KEY',
    values: { apiKey: 'nvapi-1' },
  });
  assert.equal(readCredentials('openrouter', { OBOETE_OPENROUTER_API_KEY: 'k' }).source, 'env:OBOETE_OPENROUTER_API_KEY');
  assert.equal(readCredentials('gemini', {}).source, 'env:OBOETE_GEMINI_API_KEY');
  assert.equal(readCredentials('gemini', {}).present, false);
  assert.equal(readCredentials('ollama', {}).source, 'none');
  assert.equal(readCredentials('agent-cli', {}, 'grok').source, 'agent login (grok)');
  // Another preset's variable is never borrowed (FR-016).
  assert.deepEqual(readCredentials('nim', { OBOETE_OPENROUTER_API_KEY: 'k' }).values, {});
});

test('a credential written into the config file is never used as a credential', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    writeFileSync(paths.config, '[credentials]\nnim_api_key = "nvapi-from-file"\n');
    assert.throws(() => loadConfig(paths), ConfigError);
    assert.equal(readCredentials('nim', {}).present, false);
  });
});

test('consentHash is stable for one tuple and changes with every field of it', () => {
  assert.equal(consentHash(workersAiTuple), WORKERS_AI_CONSENT);
  assert.equal(consentHash({ ...workersAiTuple }), WORKERS_AI_CONSENT);
  for (const changed of [
    { ...workersAiTuple, preset: 'nim' },
    { ...workersAiTuple, host: 'integrate.api.nvidia.com' },
    { ...workersAiTuple, credentialSource: 'env:OBOETE_NIM_API_KEY' },
    { ...workersAiTuple, costClass: 'remote' },
    { ...workersAiTuple, egressClasses: ['eligible', 'local_only', 'private'] },
  ]) {
    assert.notEqual(consentHash(changed), WORKERS_AI_CONSENT);
  }
});

test('consentTuple describes the live preset and credential source', () => {
  const config = configSchema.parse({ observer: { preset: 'workers-ai' } });
  assert.deepEqual(consentTuple(config, { OBOETE_CF_API_TOKEN: 't', OBOETE_CF_ACCOUNT_ID: 'a' }), workersAiTuple);
  assert.deepEqual(consentTuple(configSchema.parse({ observer: { preset: 'none' } }), {}), {
    preset: 'none',
    host: '',
    credentialSource: 'none',
    costClass: 'none',
    egressClasses: [],
  });
});

test('a remote preset is used only while the stored consent matches the live tuple', () => {
  const env = { OBOETE_CF_API_TOKEN: 't', OBOETE_CF_ACCOUNT_ID: 'a' };
  const noConsent = configSchema.parse({ observer: { preset: 'workers-ai' } });
  assert.equal(consentMatches(noConsent, env), false);

  const consented = configSchema.parse({
    observer: { preset: 'workers-ai' },
    consent: { hash: WORKERS_AI_CONSENT, accepted_at: 1756900000000 },
  });
  assert.equal(consentMatches(consented, env), true);

  const staleConsent = configSchema.parse({
    observer: { preset: 'nim' },
    consent: { hash: WORKERS_AI_CONSENT, accepted_at: 1756900000000 },
  });
  assert.equal(consentMatches(staleConsent, { OBOETE_NIM_API_KEY: 'k' }), false);

  // The credential source alone changes: the same host and cost class, a different agent login.
  const cliConsent = configSchema.parse({
    observer: { preset: 'agent-cli', agent_cli: 'claude' },
    consent: { hash: AGENT_CLI_CLAUDE_CONSENT, accepted_at: 1756900000000 },
  });
  assert.equal(consentMatches(cliConsent, {}), true);
  const switchedCli = configSchema.parse({
    observer: { preset: 'agent-cli', agent_cli: 'codex' },
    consent: { hash: AGENT_CLI_CLAUDE_CONSENT, accepted_at: 1756900000000 },
  });
  assert.equal(consentMatches(switchedCli, {}), false);
});

test('a preset that sends nothing off the machine needs no stored consent', () => {
  assert.equal(consentMatches(configSchema.parse({ observer: { preset: 'none' } }), {}), true);
  assert.equal(consentMatches(configSchema.parse({ observer: { preset: 'ollama' } }), {}), true);
  const wrongHash = configSchema.parse({
    observer: { preset: 'ollama' },
    consent: { hash: WORKERS_AI_CONSENT, accepted_at: 1 },
  });
  assert.equal(consentMatches(wrongHash, {}), false);
});

test('isPaused reflects the paused marker file', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    assert.equal(isPaused(paths), false);
    writeFileSync(paths.paused, '');
    assert.equal(isPaused(paths), true);
  });
});

test('loadRepoRules accepts path rules only', async () => {
  await withTempHome((home) => {
    assert.deepEqual(loadRepoRules(home), { secretPaths: [] });

    writeFileSync(join(home, '.oboete.toml'), '[privacy]\nsecret_paths = ["deploy/*.pem", "**/.env"]\n');
    assert.deepEqual(loadRepoRules(home), { secretPaths: ['deploy/*.pem', '**/.env'] });

    writeFileSync(join(home, '.oboete.toml'), '[privacy]\nsecret_paths = []\n\n[observer]\npreset = "nim"\n');
    assert.throws(
      () => loadRepoRules(home),
      (error: unknown) => error instanceof RepoConfigError && error.code === 'repo_config_malformed',
    );

    writeFileSync(join(home, '.oboete.toml'), '[privacy\n');
    assert.throws(
      () => loadRepoRules(home),
      (error: unknown) => error instanceof RepoConfigError && error.code === 'repo_config_malformed',
    );

    // R4: the rules are compiled and matched on the hook path, so the repository cannot hand over
    // an unbounded list or an unbounded rule.
    const rule = 'a'.repeat(MAX_REPO_SECRET_PATH_LENGTH);
    const atTheBound = Array.from({ length: MAX_REPO_SECRET_PATHS }, () => rule);
    writeFileSync(
      join(home, '.oboete.toml'),
      `[privacy]\nsecret_paths = ${JSON.stringify(atTheBound)}\n`,
    );
    assert.equal(loadRepoRules(home).secretPaths.length, MAX_REPO_SECRET_PATHS);

    writeFileSync(
      join(home, '.oboete.toml'),
      `[privacy]\nsecret_paths = ${JSON.stringify([...atTheBound, rule])}\n`,
    );
    assert.throws(
      () => loadRepoRules(home),
      (error: unknown) => error instanceof RepoConfigError && error.code === 'repo_config_malformed',
    );

    writeFileSync(
      join(home, '.oboete.toml'),
      `[privacy]\nsecret_paths = ${JSON.stringify([`${rule}a`])}\n`,
    );
    assert.throws(
      () => loadRepoRules(home),
      (error: unknown) => error instanceof RepoConfigError && error.code === 'repo_config_malformed',
    );
  });
});

test('scrubCredentials replaces credential values wherever they appear', () => {
  const env = {
    OBOETE_NIM_API_KEY: 'nvapi-000111222333',
    OBOETE_CF_API_TOKEN: 'cf-token-value',
    OBOETE_CF_ACCOUNT_ID: 'account-9',
    OBOETE_HOME: '/home/example/.oboete',
    OBOETE_GEMINI_API_KEY: '',
  };
  assert.equal(
    scrubCredentials('key=nvapi-000111222333 token=cf-token-value account=account-9 home=/home/example/.oboete', env),
    'key=[credential] token=[credential] account=[credential] home=/home/example/.oboete',
  );
  assert.equal(scrubCredentials('nvapi-000111222333/nvapi-000111222333', env), '[credential]/[credential]');
  assert.equal(scrubCredentials('nothing to hide', env), 'nothing to hide');
});

test('appendLog writes one scrubbed line and creates the log directory', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    process.env.OBOETE_NIM_API_KEY = 'nvapi-000111222333';
    try {
      appendLog(paths.observeLog, 'warn', 'The provider request failed.', {
        preset: 'nim',
        key: 'nvapi-000111222333',
        note: 'two words',
        attempts: 2,
        retried: true,
      });
      appendLog(paths.observeLog, 'info', 'The batch was applied.');
      const text = readFileSync(paths.observeLog, 'utf8');
      const lines = text.split('\n').filter((line) => line !== '');
      assert.equal(lines.length, 2);
      assert.equal(text.includes('nvapi-000111222333'), false);
      assert.match(
        lines[0],
        /^\d{4}-\d{2}-\d{2}T[0-9:.]+Z warn The provider request failed\. preset=nim key=\[credential\] note="two words" attempts=2 retried=true$/,
      );
      assert.match(lines[1], / info The batch was applied\.$/);
    } finally {
      delete process.env.OBOETE_NIM_API_KEY;
    }
  });
});

test('a credential is refused wherever it sits in the config file', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    writeFileSync(paths.config, '[[credentials]]\nvalue = "token-value"\n');
    assert.ok(caught(() => loadConfig(paths)) instanceof ConfigError);
  });
  // Any OBOETE_*_API_KEY / _API_TOKEN name counts; a value shorter than a real credential does not.
  assert.equal(scrubCredentials('value-01', { OBOETE_API_KEY: 'value-01' }), '[credential]');
  assert.equal(scrubCredentials('value-01', { OBOETE_GROK_API_TOKEN: 'value-01' }), '[credential]');
  assert.equal(scrubCredentials('v', { OBOETE_GROK_API_TOKEN: 'v' }), 'v');
});

// The literals below are transcribed from contracts/observer.md "Provider presets", the R13
// evaluation of docs/research/oboete-contracts-probes.md and data-model.md destination_rules.
// They are the only guard on the catalog, so they must never be read back out of src/config.ts.
test('the preset catalog matches the contract and the R13 provider probe', () => {
  assert.deepEqual(EGRESS_CLASSES, {
    remote: ['eligible'],
    local: ['eligible', 'local_only', 'private'],
    none: [],
  });
  assert.deepEqual(PRESET_CATALOG, {
    'workers-ai': {
      host: 'api.cloudflare.com',
      baseUrl: 'https://api.cloudflare.com/client/v4/accounts/<account>/ai',
      credential: { kind: 'cloudflare' },
      costClass: 'free-tier',
      egress: 'remote',
      defaultModel: '@cf/zai-org/glm-4.7-flash',
      structuredOutput: 'json_schema',
      capped: true,
    },
    ollama: {
      host: '127.0.0.1:11434',
      baseUrl: 'http://127.0.0.1:11434/v1',
      credential: { kind: 'none' },
      costClass: 'local',
      egress: 'local',
      defaultModel: '',
      structuredOutput: 'response_format',
      capped: false,
    },
    nim: {
      host: 'integrate.api.nvidia.com',
      baseUrl: 'https://integrate.api.nvidia.com/v1',
      credential: { kind: 'api-key', envName: 'OBOETE_NIM_API_KEY' },
      costClass: 'remote',
      egress: 'remote',
      defaultModel: 'meta/llama-3.2-11b-vision-instruct',
      structuredOutput: 'text-json',
      capped: true,
    },
    openrouter: {
      host: 'openrouter.ai',
      baseUrl: 'https://openrouter.ai/api/v1',
      credential: { kind: 'api-key', envName: 'OBOETE_OPENROUTER_API_KEY' },
      costClass: 'remote',
      egress: 'remote',
      defaultModel: 'openai/gpt-4o-mini',
      structuredOutput: 'response_format',
      capped: true,
    },
    gemini: {
      host: 'generativelanguage.googleapis.com',
      baseUrl: 'https://generativelanguage.googleapis.com/v1beta/openai',
      credential: { kind: 'api-key', envName: 'OBOETE_GEMINI_API_KEY' },
      costClass: 'remote',
      egress: 'remote',
      defaultModel: 'gemini-2.5-flash',
      structuredOutput: 'text-json',
      capped: true,
    },
    'agent-cli': {
      host: 'agent-cli child process',
      baseUrl: '',
      credential: { kind: 'agent-login' },
      costClass: 'own-subscription',
      egress: 'remote',
      defaultModel: '',
      structuredOutput: 'text-json',
      capped: false,
    },
  });
});
