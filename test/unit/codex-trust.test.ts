// Contract test: src/setup/codex-trust.ts against the rule recorded during the R13 probe
// (scripts/e2e/probe-lib/trusthash.mjs, the implementation that made 12 of 12 Codex hooks fire
// without `--dangerously-bypass-hook-trust`; docs/research/oboete-contracts-probes.md). The
// installer writes those rows and src/setup/detect.ts reads them back, so both ends are checked
// against the recording rather than against each other.
//
// Domain of the recording: `trusthash.mjs` deletes a configured `timeout` from the file before it
// hashes (and rewrites the file that way), and it does not model `additionalContextLimit`. The
// fixture therefore goes through the same rewrite, and the two fields stay outside what this test
// can pin.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { parse as parseToml } from 'smol-toml';

import { trustedHash, trustKey, type CodexHandler } from '../../src/setup/codex-trust.js';
import { detectAgents, type VersionSpawn } from '../../src/setup/detect.js';
import { withTempHome } from '../helpers/home.js';

const root = fileURLToPath(new URL('../../..', import.meta.url));
const TRUSTHASH = join(root, 'scripts/e2e/probe-lib/trusthash.mjs');

type FixtureGroup = { matcher?: string; hooks: CodexHandler[]; oboete: true };

function handler(event: string, suffix = ''): CodexHandler {
  return {
    type: 'command',
    command: `'/usr/bin/node' '/opt/oboete/dist/oboete.mjs' hook --agent codex --event ${event}${suffix}`,
    // The timeout the installer states per hook (FR-031); `trusthash.mjs` strips it before hashing.
    timeout: 12,
  };
}

/** A matcher group, a group holding two handlers, and a late event (`session_end` normalization). */
const FIXTURE: { hooks: Record<string, FixtureGroup[]> } = {
  hooks: {
    SessionStart: [
      { matcher: 'startup|clear|compact', hooks: [handler('SessionStart')], oboete: true },
    ],
    PreToolUse: [
      { hooks: [handler('PreToolUse'), handler('PreToolUse', ' --second')], oboete: true },
    ],
    SessionEnd: [{ hooks: [handler('SessionEnd')], oboete: true }],
  },
};

/** `{ "<trust key>": "<trusted hash>" }` out of the TOML `trusthash.mjs` prints. */
function recordedRows(toml: string): Record<string, string> {
  const parsed = parseToml(toml) as { hooks?: { state?: Record<string, { trusted_hash?: string }> } };
  return Object.fromEntries(
    Object.entries(parsed.hooks?.state ?? {}).map(([key, row]) => [key, row.trusted_hash ?? '']),
  );
}

/** The same map, built by the installer's rule over the file `trusthash.mjs` left behind. */
function installerRows(hooksPath: string): Record<string, string> {
  const file = JSON.parse(readFileSync(hooksPath, 'utf8')) as typeof FIXTURE;
  const rows: Record<string, string> = {};
  for (const [event, groups] of Object.entries(file.hooks)) {
    groups.forEach((group, groupIndex) => {
      group.hooks.forEach((entry, handlerIndex) => {
        rows[trustKey(hooksPath, event, groupIndex, handlerIndex)] = trustedHash(
          event,
          group.matcher,
          entry,
        );
      });
    });
  }
  return rows;
}

function writeFixture(codexHome: string): { hooksPath: string; toml: string } {
  mkdirSync(codexHome, { recursive: true });
  const hooksPath = join(codexHome, 'hooks.json');
  writeFileSync(hooksPath, JSON.stringify(FIXTURE, null, 2));
  const result = spawnSync(process.execPath, [TRUSTHASH, hooksPath], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  return { hooksPath, toml: result.stdout };
}

const noVersion: VersionSpawn = () => {
  throw new Error('detection must not spawn a CLI in this test');
};

test('the installer trust rule produces the rows the recorded Codex rule produces', async () => {
  await withTempHome(async (home) => {
    const { hooksPath, toml } = writeFixture(join(home, 'user', '.codex'));
    const recorded = recordedRows(toml);

    assert.equal(Object.keys(recorded).length, 4, 'one row per handler in the fixture');
    assert.deepEqual(installerRows(hooksPath), recorded);
  });
});

test('detectAgents reports Codex as trusted from the recorded rows', async () => {
  await withTempHome(async (home) => {
    const userHome = join(home, 'user');
    const codexHome = join(userHome, '.codex');
    const { toml } = writeFixture(codexHome);
    writeFileSync(join(codexHome, 'config.toml'), toml);

    const env = { HOME: userHome, PATH: join(home, 'empty-bin') };
    assert.equal(detectAgents(env, noVersion)[1]?.trust, 'trusted');

    // One row away from the recording is not a trusted installation. The shortened file still
    // parses, so what makes it untrusted is the missing row and not an unreadable config.
    const shortened = toml.split('\n').slice(0, -3).join('\n');
    assert.equal(Object.keys(recordedRows(shortened)).length, 3);
    writeFileSync(join(codexHome, 'config.toml'), shortened);
    assert.equal(detectAgents(env, noVersion)[1]?.trust, 'untrusted');
  });
});

test("a group the developer wrote next to oboete's needs no trust row of its own", async () => {
  await withTempHome(async (home) => {
    const userHome = join(home, 'user');
    const codexHome = join(userHome, '.codex');
    const { hooksPath, toml } = writeFixture(codexHome);
    writeFileSync(join(codexHome, 'config.toml'), toml);

    // A handler the developer added themselves, under an event oboete does not wire so the trust
    // keys of the oboete groups keep their indices. It carries no `oboete` marker, so detection
    // must not demand a row for it: this file is a correctly wired installation.
    const file = JSON.parse(readFileSync(hooksPath, 'utf8')) as { hooks: Record<string, unknown[]> };
    file.hooks.Notification = [{ hooks: [{ type: 'command', command: 'notify-send oboete' }] }];
    writeFileSync(hooksPath, JSON.stringify(file, null, 2));

    const env = { HOME: userHome, PATH: join(home, 'empty-bin') };
    assert.equal(detectAgents(env, noVersion)[1]?.trust, 'trusted');
  });
});
