#!/usr/bin/env node
// T067 smoke: first session of each agent through the real hook/capture bundle.
// Expands placeholders, runs inside a temporary git repository, checks raw_events
// with node:sqlite. Does not use the sqlite3 CLI.
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { fileURLToPath } from 'node:url';

import { ROOT_PH, fillBytes } from './generate-1000-events.mjs';

const HERE = fileURLToPath(new URL('.', import.meta.url));
const REPO = join(HERE, '../..');
const BUNDLE = join(REPO, 'dist', 'oboete.mjs');
const FIXTURE = join(REPO, 'test/fixtures/events-1000.jsonl');
const AGENTS = ['claude', 'codex', 'grok', 'pi'];

function loadJsonl(path) {
  return readFileSync(path, 'utf8')
    .trimEnd()
    .split('\n')
    .filter((line) => line !== '')
    .map((line) => JSON.parse(line));
}

function corpusMaps() {
  const secrets = new Map();
  for (const row of loadJsonl(join(REPO, 'test/corpus/secrets.jsonl'))) {
    secrets.set(row.id, row.text);
  }
  const directives = loadJsonl(join(REPO, 'test/corpus/directives.jsonl')).map((row) => row.phrase);
  return { secrets, directives };
}

function expandString(text, root, secrets, directives) {
  let out = text.split(ROOT_PH).join(root);
  out = out.replace(/__SECRET:([a-z0-9-]+)__/g, (_, id) => {
    const value = secrets.get(id);
    if (value === undefined) throw new Error(`unknown secret id ${id}`);
    return value;
  });
  out = out.replace(/__DIRECTIVE:(\d+)__/g, (_, index) => {
    const phrase = directives[Number(index)];
    if (phrase === undefined) throw new Error(`unknown directive ${index}`);
    return phrase;
  });
  out = out.replace(/__FILL:(\d+)__/g, (_, n) => fillBytes(Number(n)));
  return out;
}

function expandPayload(payload, root, secrets, directives) {
  const walk = (value) => {
    if (typeof value === 'string') return expandString(value, root, secrets, directives);
    if (Array.isArray(value)) return value.map(walk);
    if (value !== null && typeof value === 'object') {
      const out = {};
      for (const [key, item] of Object.entries(value)) out[key] = walk(item);
      return out;
    }
    return value;
  };
  return walk(payload);
}

function migrate(dbPath) {
  mkdirSync(dirname(dbPath), { recursive: true });
  const db = new DatabaseSync(dbPath);
  db.exec('PRAGMA journal_mode = WAL');
  db.exec('PRAGMA foreign_keys = ON');
  const files = [
    [1, '0001_core'],
    [2, '0002_memory_search'],
    [3, '0003_operations'],
  ];
  for (const [version, name] of files) {
    const sql = readFileSync(join(REPO, 'src/db/migrations', `${name}.sql`), 'utf8');
    const sha = createHash('sha256').update(sql, 'utf8').digest('hex');
    db.exec('BEGIN IMMEDIATE');
    db.exec(sql);
    db.prepare(
      'INSERT INTO schema_migrations(version, name, sha256, applied_at) VALUES (?, ?, ?, ?)',
    ).run(version, name, sha, Date.now());
    db.exec(`PRAGMA user_version = ${version}`);
    db.exec('COMMIT');
  }
  db.close();
}

function hookCommand(agent, event) {
  if (agent === 'pi') {
    return {
      args: ['capture', '--agent', 'pi', '--event', event, '--invocation', `smoke-${event}`],
      extra: {},
    };
  }
  if (agent === 'codex') {
    return { args: ['hook', '--agent', 'codex', '--event', event], extra: {} };
  }
  const extra =
    agent === 'grok' ? { GROK_HOOK_EVENT: event, GROK_SESSION_ID: 'smoke-grok' } : {};
  return { args: ['hook', '--agent', 'claude-or-grok', '--event', event], extra };
}

function firstSession(lines, agent) {
  const mine = lines.filter((row) => row.agent === agent);
  if (mine.length === 0) throw new Error(`no events for ${agent}`);
  const label = mine[0].session;
  return mine.filter((row) => row.session === label);
}

function run() {
  const lines = loadJsonl(FIXTURE);
  const { secrets, directives } = corpusMaps();
  const home = mkdtempSync(join(tmpdir(), 'oboete-t067-home-'));
  const repo = mkdtempSync(join(tmpdir(), 'oboete-t067-repo-'));
  try {
    spawnSync('git', ['-C', repo, 'init', '--quiet'], { encoding: 'utf8' });
    migrate(join(home, 'memory.db'));
    mkdirSync(join(home, 'spool', 'pi-ack'), { recursive: true });
    mkdirSync(join(home, 'logs'), { recursive: true });

    let piped = 0;
    for (const agent of AGENTS) {
      const session = firstSession(lines, agent);
      for (const row of session) {
        const { args, extra } = hookCommand(agent, row.event);
        const env = { ...process.env, OBOETE_HOME: home, NODE_ENV: 'test', ...extra };
        if (agent !== 'grok') {
          delete env.GROK_HOOK_EVENT;
          delete env.GROK_SESSION_ID;
        }
        const input = JSON.stringify(expandPayload(row.payload, repo, secrets, directives));
        const result = spawnSync(process.execPath, [BUNDLE, ...args], {
          input,
          cwd: repo,
          encoding: 'utf8',
          env,
          timeout: 10_000,
        });
        if (result.status !== 0) {
          throw new Error(
            `${agent} ${row.event} seq=${row.seq} exited ${result.status}: ${result.stderr}`,
          );
        }
        piped += 1;
      }
    }

    const db = new DatabaseSync(join(home, 'memory.db'), { timeout: 5_000 });
    const rows = db.prepare('SELECT agent, kind, payload_json FROM raw_events').all();
    db.close();
    if (rows.length === 0) throw new Error('raw_events is empty');
    const bad = [];
    for (const row of rows) {
      if (typeof row.payload_json !== 'string') continue;
      if (row.payload_json.includes('failure_reason')) {
        bad.push(`${row.agent} ${row.kind} ${row.payload_json}`);
      }
    }
    if (bad.length > 0) {
      throw new Error(`payload_json carries failure_reason:\n${bad.join('\n')}`);
    }
    process.stdout.write(
      `smoke ok: ${piped} hook calls, ${rows.length} raw_events, no failure_reason\n`,
    );
  } finally {
    rmSync(home, { recursive: true, force: true });
    rmSync(repo, { recursive: true, force: true });
  }
}

run();
