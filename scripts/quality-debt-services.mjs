// Service side of the quality-debt record: SonarCloud transitions and comments, Codacy ignores, and
// the confirmation of planned rows against each service's open set. Called by quality-debt-record.mjs.
import { readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as pause } from 'node:timers/promises';

import {
  identity, isConfirmed, resolvedWithoutReason, securityPopulation, validateInventory, writeLedger,
} from './quality-debt-ledger.mjs';

const CODACY = 'https://app.codacy.com/api/v3/analysis/organizations/gh/ojungo69/repositories/oboete';
const codacyIssues = `${CODACY}/issues`;

/** The resolved rows of one service that are still to be sent, validated; confirmed rows are `--check`'s business. */
function pendingResolved(service, ledger, inventory) {
  const rows = ledger.filter((row) => row.service === service && row.state === 'resolved' && !isConfirmed(row));
  validateInventory(rows, ledger, inventory);
  const findings = new Map(inventory.map((finding) => [identity(finding), finding]));
  for (const row of rows) {
    if (resolvedWithoutReason(row)) throw new Error(`${row.service} ${row.id}: resolved without a valid reason`);
    if (row.service === 'sonar' && !['wontfix', 'falsepositive'].includes(row.transition ?? 'wontfix')) {
      throw new Error(`Sonar ${row.id}: invalid transition`);
    }
    if (securityPopulation(findings.get(identity(row))) && (typeof row.verdict !== 'string' || !row.verdict.trim())) {
      throw new Error(`${row.service} ${row.id}: resolved without a verdict`);
    }
  }
  return rows;
}

const ORIGINS = new Set(['https://sonarcloud.io', 'https://app.codacy.com']);

/**
 * One fetch, only to the two services (the allowlist is also what Codacy's SSRF rule accepts as a
 * sanitiser); a request-layer failure (whose message may quote a header value) is reported by its
 * name only.
 */
async function request(service, url, init, failure) {
  if (ORIGINS.has(new URL(url).origin)) {
    let response;
    try {
      response = await fetch(url, { ...init, redirect: 'manual' });
    } catch (error) {
      throw new Error(`${service} request failed: ${error.name}`, { cause: error });
    }
    if (!response.ok) throw new Error(failure(response.status));
    return response;
  }
  throw new Error(`${service} request refused: ${new URL(url).origin} is not the service`);
}

/** The parsed body of a response; a body that is not JSON is reported without quoting it. */
async function readBody(service, response) {
  try {
    return await response.json();
  } catch (error) {
    throw new Error(`${service} returned a body that is not JSON`, { cause: error });
  }
}

/** After a success status the call has been applied; a failure to discard the body is not a failure of the call. */
async function discardBody(response) {
  try {
    await response.body?.cancel();
  } catch {
    // The stream is already unusable; nothing to release.
  }
}

function readToken(service) {
  const file = `${service}_TOKEN.md`;
  // The token is on line 2, under the note that says what it is — unless the file is nothing but
  // the token, which is how CODACY_TOKEN.md is written. Deciding on the number of lines rather than
  // on what a line looks like leaves the character check below as the only thing that can reject a
  // token, so a stray space in one still reports itself instead of being read as prose.
  const lines = readFileSync(join(homedir(), file), 'utf8').split(/\r?\n/).filter((line) => line !== '');
  const token = lines.length === 1 ? lines[0] : lines[1];
  if (!token) throw new Error(`${file} must contain the token, alone or on the line under a note`);
  if (/[^A-Za-z0-9_.~+/=-]/.test(token)) throw new Error(`${file} contains an unexpected character`);
  return token;
}

function sonarAuthorization() {
  const credentials = Buffer.from(`${readToken('SONAR')}:`).toString('base64');
  return `Basic ${credentials}`;
}

async function postSonar(call, body, authorization) {
  const response = await request('Sonar', `https://sonarcloud.io/api/issues/${call.action}`, {
    method: 'POST', headers: { Authorization: authorization, 'Content-Type': 'application/x-www-form-urlencoded' }, body,
  }, (status) => `Sonar ${call.fields.issue}: ${call.action} returned HTTP ${status}`);
  await discardBody(response);
  return response.status;
}

/** The two Sonar calls of one resolved row; the transition is skipped once the ledger says it was made. */
function sonarCalls(row) {
  return [
    { action: 'do_transition', fields: { issue: row.id, transition: row.transition ?? 'wontfix' } },
    { action: 'add_comment', fields: { issue: row.id, text: row.where } },
  ].filter((call) => !(call.action === 'do_transition' && row.transitioned));
}

/** Records one successful Sonar call on its row: progress after the transition, completion after the comment. */
function recordSonarCall(row, call, status) {
  if (call.action === 'do_transition') {
    row.transitioned = new Date().toISOString();
  } else {
    // The row is complete; a later reopen only has to clear `confirmed` to run both calls again.
    delete row.transitioned;
    row.confirmed = `HTTP ${status} ${new Date().toISOString()}`;
  }
}

export async function applySonar(ledger, dryRun, inventory) {
  const rows = pendingResolved('sonar', ledger, inventory);
  if (rows.length === 0) return;
  const authorization = dryRun ? undefined : sonarAuthorization();
  let firstCall = true;
  for (const row of rows) {
    for (const call of sonarCalls(row)) {
      const body = new URLSearchParams(call.fields);
      if (dryRun) {
        console.log(`POST https://sonarcloud.io/api/issues/${call.action} ${body}`);
        continue;
      }
      if (!firstCall) await pause(200);
      firstCall = false;
      recordSonarCall(row, call, await postSonar(call, body, authorization));
      writeLedger(ledger);
    }
  }
}

async function codacyHeaders() {
  const headers = { 'api-token': readToken('CODACY'), 'content-type': 'application/json' };
  const response = await request('Codacy', 'https://app.codacy.com/api/v3/user', { method: 'GET', headers }, () => 'Codacy token rejected');
  await discardBody(response);
  return headers;
}

export async function applyCodacy(ledger, dryRun, inventory) {
  const rows = pendingResolved('codacy', ledger, inventory);
  if (rows.length === 0) return;
  const headers = dryRun ? undefined : await codacyHeaders();
  for (const row of rows) {
    const body = JSON.stringify({ ignored: true, reason: row.reason, comment: row.where });
    if (dryRun) {
      console.log(`PATCH ${codacyIssues}/${encodeURIComponent(row.id)} ${body}`);
      continue;
    }
    await pause(200);
    const response = await request('Codacy', `${codacyIssues}/${encodeURIComponent(row.id)}`, { method: 'PATCH', headers, body },
      (status) => `Codacy ${row.id}: PATCH returned HTTP ${status}`);
    await discardBody(response);
    row.confirmed = `HTTP ${response.status} ${new Date().toISOString()}`;
    writeLedger(ledger);
  }
}

// Both labels must describe the same repository state, so the Sonar analysis must be of the Codacy commit.
async function verifySonarAnalysis(analysis, sha, authorization) {
  const response = await request('Sonar', 'https://sonarcloud.io/api/project_analyses/search?project=ojungo69_free-mem&branch=main&ps=1',
    { method: 'GET', headers: { Authorization: authorization } }, (status) => `Sonar analyses search returned HTTP ${status}`);
  const latest = (await readBody('Sonar', response)).analyses?.[0];
  // Messages never carry a value taken from a response body; the quickstart shows how to look it up.
  if (latest?.key !== analysis) throw new Error(`Sonar: the latest analysis is not ${analysis}`);
  if (latest.revision !== sha) throw new Error(`Sonar: analysis ${analysis} is not of commit ${sha}`);
}

// The issue search has no commit selector: it answers for the repository's last analysed commit,
// so that commit must be the one being confirmed, and its analysis must have ended.
async function verifyCodacyCommit(sha) {
  const response = await request('Codacy', CODACY, { method: 'GET' }, (status) => `Codacy repository returned HTTP ${status}`);
  const last = (await readBody('Codacy', response)).data?.lastAnalysedCommit;
  if (last?.sha !== sha) throw new Error(`Codacy: the last analysed commit is not ${sha}`);
  if (typeof last.endedAnalysis !== 'string' || !last.endedAnalysis.trim()) {
    throw new Error(`Codacy: analysis of ${sha} has not finished`);
  }
}

/** The total a Sonar page reports, checked against itself and against the earlier pages. */
function sonarTotal(data, page, previous) {
  if (data.paging?.total !== undefined && data.total !== undefined && data.paging.total !== data.total) {
    throw new Error('Sonar issues search returned inconsistent totals');
  }
  const total = data.paging?.total ?? data.total;
  if (previous !== undefined && total !== previous) throw new Error('Sonar issues search total changed between pages');
  if (!Array.isArray(data.issues) || !Number.isSafeInteger(total) || total < 0
    || (data.issues.length === 0 && total > (page - 1) * 500)) {
    throw new Error('Sonar issues search returned an invalid page');
  }
  return total;
}

/** Adds the ids of one page to `open`, refusing an id that is not a non-empty string or was already listed. */
function collectIds(open, items, field, service) {
  for (const item of items) {
    if (typeof item[field] !== 'string' || !item[field]) throw new Error(`${service} issues search returned an invalid id`);
    if (open.has(item[field])) throw new Error(`${service} issues search returned a repeated id`);
    open.add(item[field]);
  }
}

async function openSonarIssues(authorization) {
  const open = new Set();
  const headers = { Authorization: authorization };
  let total;
  for (let page = 1; ; page++) {
    const response = await request('Sonar',
      `https://sonarcloud.io/api/issues/search?componentKeys=ojungo69_free-mem&branch=main&resolved=false&ps=500&p=${page}`,
      { method: 'GET', headers }, (status) => `Sonar issues search returned HTTP ${status}`);
    const data = await readBody('Sonar', response);
    total = sonarTotal(data, page, total);
    collectIds(open, data.issues, 'key', 'Sonar');
    if (page * 500 >= total) {
      if (open.size !== total) throw new Error('Sonar issues search returned an incomplete result');
      return open;
    }
  }
}

/** The total a Codacy page reports (it may omit it), checked against the earlier pages. */
function codacyTotal(data, previous) {
  if (!Array.isArray(data.data) || !data.pagination || typeof data.pagination !== 'object' || Array.isArray(data.pagination)) {
    throw new Error('Codacy issues search returned an invalid page');
  }
  const total = data.pagination.total === undefined ? previous : data.pagination.total;
  if (previous !== undefined && total !== previous) throw new Error('Codacy issues search total changed between pages');
  if (total !== undefined && (!Number.isSafeInteger(total) || total < 0)) {
    throw new Error('Codacy issues search returned an invalid total');
  }
  return total;
}

async function openCodacyIssues() {
  const open = new Set();
  const cursors = new Set();
  const params = new URLSearchParams({ limit: '100' });
  let total;
  for (;;) {
    const response = await request('Codacy', `${codacyIssues}/search?${params}`,
      { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' },
      (status) => `Codacy issues search returned HTTP ${status}`);
    const data = await readBody('Codacy', response);
    total = codacyTotal(data, total);
    collectIds(open, data.data, 'issueId', 'Codacy');
    const cursor = data.pagination.cursor;
    if (cursor === undefined || cursor === null || cursor === '') {
      if (total !== undefined && open.size !== total) throw new Error('Codacy issues search returned an incomplete result');
      return open;
    }
    if (typeof cursor !== 'string' || cursors.has(cursor)) throw new Error('Codacy issues search returned an invalid cursor');
    cursors.add(cursor);
    params.set('cursor', cursor);
  }
}

// Verify both pending services before querying issues or recording confirmation labels.
export async function confirmLedger(ledger, sonarAnalysis, codacyCommit, inventory) {
  const pending = ledger.filter((row) => ['fixed', 'excluded'].includes(row.state) && !isConfirmed(row));
  validateInventory(pending, ledger, inventory);
  const rowsOf = (service) => pending.filter((row) => row.service === service);
  const authorization = rowsOf('sonar').length ? sonarAuthorization() : undefined;
  if (rowsOf('sonar').length) await verifySonarAnalysis(sonarAnalysis, codacyCommit, authorization);
  if (rowsOf('codacy').length) await verifyCodacyCommit(codacyCommit);
  const open = {
    sonar: rowsOf('sonar').length ? await openSonarIssues(authorization) : null,
    codacy: rowsOf('codacy').length ? await openCodacyIssues() : null,
  };
  const labels = { sonar: sonarAnalysis, codacy: codacyCommit };
  for (const service of ['sonar', 'codacy']) {
    const rows = rowsOf(service);
    const stillOpen = rows.length ? labelConfirmed(rows, open[service], labels[service]) : 0;
    if (rows.length) writeLedger(ledger);
    console.log(`${service}: ${rows.length - stillOpen} confirmed, ${stillOpen} still open`);
  }
}

/** Stamps `label` on every row the service no longer reports; returns how many it still reports. */
function labelConfirmed(rows, open, label) {
  let stillOpen = 0;
  for (const row of rows) {
    if (open.has(row.id)) {
      console.log(`${row.service} ${row.id}: still open`);
      stillOpen++;
    } else {
      row.confirmed = label;
    }
  }
  return stillOpen;
}
