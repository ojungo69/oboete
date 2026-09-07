// The quickstart tells a reader to pull the same credential the CLI pulls, with a shell one-liner.
// Two rules for one file drift apart silently, and the drift is only visible when a service refuses
// a token nobody can see. So the helper is read out of the document itself and compared against the
// CLI over every layout the reviews raised, including the ones only a hex editor would notice.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import { apiStub, evidence, fixture, readCalls, run, writeJson } from './quality-debt-record.test-support.mjs';

const quickstart = fileURLToPath(new URL('../specs/008-quality-debt-zero/quickstart.md', import.meta.url));

/** The helper exactly as the document publishes it: a reader copying the line gets what runs here. */
function documentedHelper() {
  const line = readFileSync(quickstart, 'utf8').split('\n').find((l) => l.trimStart().startsWith('tokenof()'));
  assert.ok(line, 'quickstart.md no longer defines tokenof()');
  return line.trim().replace(/^`|`$/g, '');
}

const TOKEN = 'fixture-token';
const NOTE = '# Note';
const layouts = [
  ['the token alone, no newline', TOKEN],
  ['the token alone', `${TOKEN}\n`],
  ['a note above the token', `${NOTE}\n${TOKEN}\n`],
  ['a note above and text below', `${NOTE}\n${TOKEN}\nignored\n`],
  ['several trailing newlines', `${NOTE}\n${TOKEN}\n\n\n`],
  ['a blank first line', `\n${TOKEN}\n`],
  ['a lone word', 'Credentials\n'],
  ['CRLF, token alone', `${TOKEN}\r\n`],
  ['CRLF, note above', `${NOTE}\r\n${TOKEN}\r\n`],
  ['CRLF, note above and text below', `${NOTE}\r\n${TOKEN}\r\nignored\r\n`],
  ['an empty file', ''],
  ['blank lines only', '\n\n'],
  ['a blank line under the token', `${TOKEN}\n\n`],
  ['a blank line under a lone word', 'Credentials\n\n'],
  ['a blank line where the token belongs', `${NOTE}\n\nexample.invalid/account\n`],
  ['blank lines around the token', `\n\n${TOKEN}\n\n`],
  ['a note, a blank line, the token', `Codacy account token\n\n${TOKEN}\n`],
  ['a blank line before a malformed token', '\nfixture token\nignored\n'],
  ['one line that is a sentence', 'Codacy account token for ojungo69\n'],
  ['a trailing space in the token', `${NOTE}\n${TOKEN} \n`],
  ['a bare carriage return in the token', `${NOTE}\nfixture\rtoken\n`],
  ['a NUL in the token', `${NOTE}\nfixture\0-token\n`],
];

/** What the CLI does with this file: whether it sent anything, and whether it sent the fixture token. */
function cliOutcome(t, contents) {
  const { cwd, ledger } = fixture(t);
  Object.assign(ledger[4], { state: 'resolved', reason: 'AcceptedUse' });
  delete ledger[4].confirmed;
  writeJson(cwd, 'ledger.json', ledger);
  writeFileSync(join(cwd, 'CODACY_TOKEN.md'), contents, 'latin1');
  const before = readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8');
  run(cwd, ['--apply-codacy'], apiStub([{ status: 200 }, { status: 200 }]));
  const calls = readCalls(cwd).filter((call) => call.url);
  return {
    sent: calls.length > 0,
    tokenMatched: calls.length > 0 && calls.every((call) => call.authMatches),
    ledgerUnchanged: readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8') === before,
  };
}

/** What a reader following the quickstart gets: the helper's exit status and whether it printed the token. */
function helperOutcome(t, contents, helper) {
  const dir = mkdtempSync(join(tmpdir(), 'token-parity-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const path = join(dir, 'TOKEN.md');
  writeFileSync(path, contents, 'latin1');
  try {
    const printed = execFileSync('bash', ['-c', `${helper}\ntokenof "$1"`, 'bash', path], { encoding: 'latin1' });
    return { sent: true, tokenMatched: printed.replace(/\n$/, '') === TOKEN };
  } catch {
    return { sent: false, tokenMatched: false };
  }
}

for (const [name, contents] of layouts) {
  test(`quickstart and CLI agree on a credentials file with ${name}`, (t) => {
    const cli = cliOutcome(t, contents);
    const helper = helperOutcome(t, contents, documentedHelper());
    assert.equal(helper.sent, cli.sent, `helper ${helper.sent ? 'accepted' : 'refused'}, CLI ${cli.sent ? 'sent' : 'sent nothing'}`);
    assert.equal(helper.tokenMatched, cli.tokenMatched, 'one of them used a value the other did not');
    if (!cli.sent) assert.equal(cli.ledgerUnchanged, true);
  });
}
