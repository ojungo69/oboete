import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { devNull } from 'node:os';

export function git(cwd: string, ...args: string[]): string {
  const result = spawnSync('git', ['-C', cwd, ...args], { encoding: 'utf8', env: {
    ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('GIT_'))),
    GIT_CONFIG_GLOBAL: devNull, GIT_CONFIG_SYSTEM: devNull, GIT_TERMINAL_PROMPT: '0',
  } });
  assert.equal(result.status, 0, `git ${args.join(' ')}: ${result.stderr}`);
  return result.stdout;
}
