// Allocate frozen quality findings and generate, validate, and apply their disposition record.
import { readFileSync, writeFileSync } from 'node:fs';
import { parseArgs } from 'node:util';

import {
  dispositionState, evidence, identity, indexLedger, isConfirmed, readJson, resolvedWithoutReason, securityPopulation,
} from './quality-debt-ledger.mjs';
import { applyCodacy, applySonar, confirmLedger, openCodacyIssues, openSonarIssues } from './quality-debt-services.mjs';

const batches = ['A', 'E', 'B1', 'B2', 'B3', 'C1', 'C2', 'C3', 'C4', 'D'];
const usage = 'Usage: quality-debt-record.mjs [--allocate | --check [--planned] | --check-live | --apply-sonar [--dry-run]'
  + ' | --apply-codacy [--dry-run] | --confirm --sonar-analysis <analysisKey> --codacy-commit <sha>]'
  + '\n--check-live searches both public main issue sets and fails if either has an id outside the frozen inventory.'
  + '\n--apply-codacy checks the current Codacy issue set before PATCHing; ids already absent are confirmed locally.'
  + '\n--confirm records the analysis key and commit SHA as labels; the caller must verify beforehand'
  + ' that both services finished analysing that revision'
  + ' (specs/008-quality-debt-zero/quickstart.md, "Final analysis confirmation").'
  + '\n--confirm refuses to run unless <analysisKey> is the latest Sonar analysis of main, made of commit <sha>, and <sha> is'
  + " Codacy's last analysed commit with a finished analysis (the issue search has no commit selector).";


/** The file's text, or `fallback` when it does not exist yet (read once; no check-then-read). */
function readOrDefault(path, fallback) {
  try {
    return readFileSync(path, 'utf8');
  } catch (error) {
    if (error.code === 'ENOENT') return fallback;
    throw error;
  }
}


function findings() {
  return [
    ...readJson('sonar-main-issues.json').map((row) => ({ ...row, service: 'sonar' })),
    ...readJson('codacy-main-issues.json').map((row) => ({ ...row, service: 'codacy', rule: row.pattern })),
  ];
}





// data-model.md: a verdict is required for a security-flavoured rule on a `src/` file, for the two
// harness `sudo` lines, and for the `ci.yml` pattern hit; a test or harness file excluded by file
// class carries no verdict, the exclusion row names the class instead.



function sessionOwned(file) {
  return ['src/injection/', 'src/mcp.ts', 'src/viewer/', 'src/transfer.ts',
    'src/db/queries.ts', 'src/privacy/', 'src/fixture/replay.ts']
    .some((prefix) => file.startsWith(prefix));
}

function structuralBatch(file) {
  if (file.startsWith('scripts/')) return 'C4';
  if (sessionOwned(file)) return 'C3';
  if (file.startsWith('src/worker/') || file.startsWith('src/observer/')) return 'C1';
  return 'C2';
}

// The Sonar rules the mechanical batches (B1-B3) are allowed to absorb, from the frozen inventory;
// a rule outside this list stays unallocated so it is looked at rather than rewritten by pattern.
const mechanicalRules = new Set(['S1066', 'S1854', 'S1874', 'S1940', 'S1994', 'S2310', 'S3358', 'S3516', 'S3735',
  'S3863', 'S4043', 'S4165', 'S4323', 'S4624', 'S5869', 'S6353', 'S6397', 'S6479', 'S6551', 'S6582', 'S6594',
  'S6653', 'S6772', 'S7688', 'S7726', 'S7737', 'S7741', 'S7744', 'S7750', 'S7755', 'S7758', 'S7765', 'S7776',
  'S7778', 'S7780', 'S7781', 'S7784', 'S7785', 'S7786']);

function sonarBatch({ rule, file }) {
  if (rule.startsWith('plsql:')) return 'A';
  const number = rule.split(':').at(-1);
  if (number === 'S8786') return 'E';
  if (number === 'S3776' || number === 'S107') return structuralBatch(file);
  if (!mechanicalRules.has(number)) return undefined;
  if (file.startsWith('scripts/')) return 'B1';
  return sessionOwned(file) ? 'B3' : 'B2';
}

function codacyExcluded(rule, file) {
  if (file === 'package-lock.json' || ['legacy/', 'build/', 'dist/', 'coverage/']
    .some((prefix) => file.startsWith(prefix))) return true;
  if ((rule.startsWith('TSQLLint_') || rule.startsWith('SQLint_')) && file.startsWith('src/db/migrations/')) return true;
  if (rule.startsWith('Lizard_') && file.startsWith('test/')) return true;
  if (rule.startsWith('Semgrep_') && (file.startsWith('test/') || file.startsWith('scripts/e2e/'))) return true;
  if (rule === 'markdownlint_MD024' && file.startsWith('docs/')) return true;
  return rule === 'Stylelint_scss_function-disallowed-list';
}

function codacyBatch({ rule, file }) {
  if (codacyExcluded(rule, file)) return 'A';
  if (rule.startsWith('Semgrep_') || rule === 'shellcheck_SC2024') return 'E';
  if (rule === 'Lizard_nloc-medium' || rule === 'Lizard_parameter-count-medium') return structuralBatch(file);
  if (rule === 'Lizard_file-nloc-medium') return 'D';
  return undefined;
}

function allocate(rows) {
  const allocation = Object.fromEntries(batches.map((batch) => [batch, []]));
  for (const row of rows) {
    const batch = row.service === 'sonar' ? sonarBatch(row) : codacyBatch(row);
    if (batch) allocation[batch].push(row.id);
  }
  allocation.counts = Object.fromEntries(batches.map((batch) => [batch, allocation[batch].length]));
  writeFileSync(`${evidence}/allocation.json`, `${JSON.stringify(allocation, null, 2)}\n`);
  console.log(JSON.stringify(allocation.counts, null, 2));
}

function checkAllocation(rows) {
  const text = readOrDefault(`${evidence}/allocation.json`, null);
  if (text === null) return ['allocation.json missing'];
  const allocation = JSON.parse(text);
  const assignments = new Map();
  for (const batch of batches) {
    for (const id of allocation[batch] ?? []) {
      if (!assignments.has(id)) assignments.set(id, []);
      assignments.get(id).push(batch);
    }
  }
  const problems = [];
  for (const row of rows) {
    const assigned = assignments.get(row.id) ?? [];
    if (assigned.length === 0) problems.push(`allocation missing: ${identity(row)}`);
    if (assigned.length > 1) problems.push(`allocation duplicate: ${identity(row)} (${assigned.join(', ')})`);
  }
  const known = new Set(rows.map((row) => row.id));
  for (const id of assignments.keys()) if (!known.has(id)) problems.push(`allocation unknown: ${id}`);
  return problems;
}

const blank = (value) => typeof value !== 'string' || !value.trim();

/** The problem classes one inventory row falls into, given its ledger entries. */
function rowProblems(row, entries, planned) {
  const closed = entries.filter((entry) => dispositionState(entry) !== 'open');
  const unconfirmed = closed.some((entry) => !isConfirmed(entry));
  const found = [];
  if (entries.length === 0) found.push('missing');
  if (entries.length > 1) found.push('duplicate');
  if (unconfirmed) found.push('unconfirmed');
  if (closed.length < entries.length || closed.length === 0 || (!planned && unconfirmed)) found.push('open');
  if (entries.some(resolvedWithoutReason)) found.push('resolved-without-reason');
  if (closed.some((entry) => blank(entry.where))) found.push('without-where');
  if (securityPopulation(row) && closed.some((entry) => entry.state !== 'excluded' && blank(entry.verdict))) {
    found.push('without-verdict');
  }
  return found;
}

function check(rows, ledger, planned) {
  const index = indexLedger(ledger);
  const ids = new Set(rows.map(identity));
  const problems = { missing: [], duplicate: [], open: [], unconfirmed: [], 'resolved-without-reason': [],
    unknown: [...index.keys()].filter((key) => !ids.has(key)), 'without-where': [], 'without-verdict': [] };
  for (const row of rows) {
    const key = identity(row);
    for (const problem of rowProblems(row, index.get(key) ?? [], planned)) problems[problem].push(key);
  }
  for (const [problem, ids] of Object.entries(problems)) {
    if (ids.length) console.error(`${problem} ${ids.length}: ${ids.join(', ')}`);
  }
  const allocationProblems = checkAllocation(rows);
  for (const problem of allocationProblems) console.error(problem);
  const summary = Object.entries(problems).map(([name, ids]) => `${ids.length} ${name}`).join(', ');
  console.log(`${rows.length} ids: ${summary}`);
  process.exitCode = allocationProblems.length > 0
    || Object.entries(problems).some(([name, ids]) => ids.length > 0 && !(planned && name === 'unconfirmed')) ? 1 : 0;
}

async function checkLive(rows) {
  const known = new Set(rows.map(identity));
  const open = { sonar: await openSonarIssues(), codacy: await openCodacyIssues() };
  const uncovered = Object.fromEntries(Object.entries(open).map(([service, ids]) => [service,
    [...ids].filter((id) => !known.has(`${service}:${id}`))]));
  for (const [service, ids] of Object.entries(uncovered)) {
    if (ids.length) console.error(`${service} uncovered ${ids.length}: ${ids.join(', ')}`);
  }
  if (uncovered.sonar.length || uncovered.codacy.length) process.exitCode = 1;
}

function tableRow(cells) {
  const escaped = cells.map((cell) => String(cell ?? '')
    .replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;')
    .replaceAll('\\', String.raw`\\`).replaceAll('|', String.raw`\|`).replace(/\r\n|\r|\n/g, '<br>')).join(' | ');
  return `| ${escaped} |`;
}

function serviceSection(service, name, rows, index) {
  const counts = { open: 0, fixed: 0, resolved: 0, excluded: 0 };
  const lines = rows.filter((row) => row.service === service)
    .sort((a, b) => a.file.localeCompare(b.file) || (a.line ?? 0) - (b.line ?? 0))
    .map((row) => {
      const disposition = index.get(identity(row))?.[0];
      const state = dispositionState(disposition);
      counts[state]++;
      let where = disposition?.where ?? '';
      if (disposition?.verdict !== undefined) where += ` — ${disposition.verdict}`;
      let label = state;
      if (state !== 'open') label += isConfirmed(disposition) ? ' ✓' : ' (planned)';
      return tableRow([row.id, row.rule, `${row.file}:${row.line ?? ''}`, label, where]);
    });
  return [
    `## ${name}`, '', Object.entries(counts).map(([state, count]) => `${state} ${count}`).join(' / '), '',
    '| id | rule | file:line | state | where / reason |', '| --- | --- | --- | --- | --- |', ...lines, '',
  ];
}

function generate(rows, ledger) {
  const path = `${evidence}.md`;
  const previous = readOrDefault(path, '# Quality debt — September 2026\n');
  const marker = previous.match(/^<!-- generated below -->(?:\r?\n|$)/m);
  let prefix = marker ? previous.slice(0, marker.index + marker[0].length) : previous;
  if (!prefix.endsWith('\n')) prefix += '\n';
  if (!marker) prefix += '\n<!-- generated below -->\n';
  const index = indexLedger(ledger);
  const lines = [
    '', ...serviceSection('sonar', 'SonarCloud', rows, index),
    ...serviceSection('codacy', 'Codacy', rows, index),
    '## History', '', '| service | id | from | to | when | why |', '| --- | --- | --- | --- | --- | --- |',
    ...ledger.flatMap((row) => (row.history ?? []).map((entry) =>
      tableRow([row.service, row.id, entry.from, entry.to, entry.when, entry.why]))), '',
  ];
  writeFileSync(path, prefix + lines.join('\n'));
}

async function main() {
  const { values } = parseArgs({ options: {
    allocate: { type: 'boolean' }, check: { type: 'boolean' }, 'check-live': { type: 'boolean' }, planned: { type: 'boolean' },
    'apply-sonar': { type: 'boolean' }, 'apply-codacy': { type: 'boolean' }, 'dry-run': { type: 'boolean' },
    confirm: { type: 'boolean' }, 'sonar-analysis': { type: 'string' }, 'codacy-commit': { type: 'string' },
  } });
  const modes = ['allocate', 'check', 'check-live', 'apply-sonar', 'apply-codacy', 'confirm'].filter((mode) => values[mode]);
  const [mode] = modes;
  const dryRun = values['dry-run'];
  const sonarAnalysis = values['sonar-analysis'];
  const codacyCommit = values['codacy-commit'];
  if (modes.length > 1 || (values.planned && mode !== 'check')
    || (dryRun && !['apply-sonar', 'apply-codacy'].includes(mode))
    || (mode === 'confirm' ? !sonarAnalysis?.trim() || !codacyCommit?.trim()
      : sonarAnalysis !== undefined || codacyCommit !== undefined)) {
    throw new Error(usage);
  }
  const rows = findings();
  if (mode === 'apply-sonar' || mode === 'apply-codacy') {
    const ledger = readJson('ledger.json');
    if (mode === 'apply-sonar') await applySonar(ledger, dryRun, rows);
    else await applyCodacy(ledger, dryRun, rows);
    return;
  }
  if (mode === 'confirm') {
    await confirmLedger(readJson('ledger.json'), sonarAnalysis, codacyCommit, rows);
    return;
  }
  if (mode === 'check-live') {
    await checkLive(rows);
    return;
  }
  if (mode === 'allocate') allocate(rows);
  else if (mode === 'check') check(rows, readJson('ledger.json'), values.planned);
  else generate(rows, readJson('ledger.json'));
}

await main().catch((error) => {
  console.error(error.code?.startsWith('ERR_PARSE_ARGS') ? usage : error.message);
  process.exitCode = 1;
});
