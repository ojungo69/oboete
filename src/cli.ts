import { parseArgs } from 'node:util';
import { isMainThread, workerData } from 'node:worker_threads';

import { appendLogQuietly, errorCode } from './log.js';
import { oboetePaths, resolveHome } from './paths.js';

const knownCommands = [
  'setup',
  'doctor',
  'hook',
  'capture',
  'inject',
  'observe',
  'search',
  'timeline',
  'get',
  'why',
  'pin',
  'unpin',
  'delete',
  'pause',
  'resume',
  'export',
  'import',
  'mcp',
  'view',
  'fixture',
];

const commands: Record<string, () => Promise<(argv: string[]) => Promise<number>>> = {
  setup: () => import('./setup/setup.js').then((module) => module.runSetup),
  hook: () => import('./capture.js').then((module) => module.runHook),
  capture: () => import('./capture.js').then((module) => module.runCapture),
  observe: () => import('./worker/observe.js').then((module) => module.runObserve),
  inject: () => import('./injection/inject.js').then((module) => module.runInject),
  search: () => import('./memories-cli.js').then((module) => module.runSearch),
  timeline: () => import('./memories-cli.js').then((module) => module.runTimeline),
  get: () => import('./memories-cli.js').then((module) => module.runGet),
  pin: () => import('./memories-cli.js').then((module) => module.runPin),
  unpin: () => import('./memories-cli.js').then((module) => module.runUnpin),
  delete: () => import('./memories-cli.js').then((module) => module.runDelete),
};

/**
 * `node:sqlite` is bundled on the hook path, and on Node 22 loading it prints an experimental
 * warning that the developer cannot act on. Without this filter every command and every hook
 * invocation would carry those two lines, where research R6 reserves stderr for the count of
 * events that could not be stored. Only that one warning is dropped; the rest still print.
 */
function silenceSqliteExperimentalWarning(): void {
  const printers = process.listeners('warning');
  process.removeAllListeners('warning');
  process.on('warning', (warning) => {
    if (warning.name === 'ExperimentalWarning' && warning.message.includes('SQLite')) return;
    for (const printer of printers) printer(warning);
  });
}

function usage(): string {
  return `Usage: oboete <command> [options]\n\nCommands: ${knownCommands.join(', ')}.\n`;
}

async function main(argv: string[]): Promise<number> {
  const { values, positionals } = parseArgs({
    args: argv,
    allowPositionals: true,
    strict: false,
    options: {
      help: { type: 'boolean' },
      version: { type: 'boolean' },
    },
  });

  if (values.version) {
    process.stdout.write(`${OBOETE_VERSION}\n`);
    return 0;
  }

  const name = positionals[0];
  if (values.help || name === undefined) {
    process.stdout.write(usage());
    return 0;
  }

  const load = commands[name];
  if (load) {
    const run = await load();
    const from = argv.indexOf(name);
    return run(argv.slice(from === -1 ? 1 : from + 1));
  }

  if (knownCommands.includes(name)) {
    process.stderr.write(`oboete ${name} is not implemented yet\n`);
    return 2;
  }

  process.stderr.write(usage());
  return 2;
}

silenceSqliteExperimentalWarning();

// The detector Worker runs this same bundle (R4), so the CLI dispatch must not run inside it: any
// worker on this bundle stays silent, even one carrying a role this build does not know.
if (!isMainThread) {
  if (workerData?.role === 'oboete-detector') {
    await (await import('./privacy/detect.js')).detectorWorkerMain();
  }
} else {
  try {
    process.exitCode = await main(process.argv.slice(2));
  } catch (error) {
    const command = process.argv[2];
    if (command === 'hook' || command === 'capture' || command === 'inject') {
      // contracts/cli.md: the agent-invoked commands exit 0 and print nothing; the failure is one
      // hook-log line carrying the error's code, never its message (it can quote captured content).
      process.exitCode = 0;
      try {
        appendLogQuietly(oboetePaths(resolveHome()).hookLog, 'error', 'command failed', {
          command,
          reason: errorCode(error),
        });
      } catch {
        // Even the home directory being unresolvable must not change the exit code.
      }
    } else {
      const message = error instanceof Error ? error.message : String(error);
      process.stderr.write(`${message.split('\n')[0]}\n`);
      process.exitCode = 3;
    }
  }
}
