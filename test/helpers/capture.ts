import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  captureEvent,
  type CaptureDeps,
  type CaptureOutcome,
} from '../../src/capture.js';
import { openDatabase } from '../../src/db/open.js';
import type { AgentName } from '../../src/events.js';
import { ensureDirectories, oboetePaths, type OboetePaths } from '../../src/paths.js';
import { detectSync } from '../../src/privacy/detect.js';
import { withTempHome } from './home.js';

export type Json = Record<string, unknown>;

export const NOW = 1_757_000_000_000;

function repositoryRoot(): string {
  let directory = fileURLToPath(new URL('.', import.meta.url));
  for (;;) {
    if (existsSync(join(directory, 'package.json'))) return directory;
    const parent = dirname(directory);
    assert.notEqual(parent, directory, 'the repository root must contain package.json');
    directory = parent;
  }
}

export const ROOT = repositoryRoot();

export function fixture(agent: string, name: string): Json {
  return JSON.parse(readFileSync(join(ROOT, 'test', 'contracts', agent, name), 'utf8')) as Json;
}

export type Context = {
  home: string;
  repo: string;
  paths: OboetePaths;
  deps: CaptureDeps;
  spawned: number;
  capture(
    agent: AgentName,
    eventName: string,
    payload: unknown,
    over?: {
      deps?: Partial<CaptureDeps>;
      text?: string;
      truncated?: boolean;
      priorFailures?: string[];
    },
  ): Promise<CaptureOutcome>;
  all(sql: string, ...params: (string | number)[]): Json[];
};

/**
 * A temporary data directory with a migrated database and a working directory that stands in for a
 * repository. The detector is the real one, called in this process: the worker is the hook's wall
 * time bound (contracts/agents.md SLAs), which the end-to-end test exercises through the bundle.
 */
export async function withCapture(
  fn: (context: Context) => Promise<void>,
  options: { database?: boolean } = {},
): Promise<void> {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    const repo = join(home, 'workspace');
    mkdirSync(repo, { recursive: true });
    if (options.database !== false) openDatabase({ path: paths.db, timeoutMs: 2_000 }).db.close();

    const context: Context = {
      home,
      repo,
      paths,
      spawned: 0,
      deps: {
        // The cutoff is the worker's business; in process the real detector simply runs.
        detect: (input) => detectSync(input),
        now: () => NOW,
        elapsedMs: () => 0,
        spawnWorker: () => {
          context.spawned += 1;
        },
      },
      capture: (agent, eventName, payload, over = {}) => {
        const text = over.text ?? JSON.stringify(payload);
        return captureEvent(
          { ...context.deps, ...over.deps },
          {
            agent,
            eventName,
            paths,
            readStdin: () => ({ text, truncated: over.truncated ?? false }),
            priorFailures: over.priorFailures,
          },
        );
      },
      all: (sql, ...params) => {
        const opened = openDatabase({ path: paths.db, timeoutMs: 2_000 });
        try {
          return opened.db.prepare(sql).all(...params) as Json[];
        } finally {
          opened.db.close();
        }
      },
    };
    await fn(context);
  });
}

export function claudePostToolUse(repo: string, content: string): Json {
  const payload = { ...((fixture('claude', 'read.json').events as Json).PostToolUse as Json) };
  payload.cwd = repo;
  payload.tool_response = {
    type: 'text',
    file: { filePath: `${repo}/README.md`, content, numLines: 1, startLine: 1, totalLines: 1 },
  };
  (payload.tool_input as Json).file_path = `${repo}/README.md`;
  return payload;
}
