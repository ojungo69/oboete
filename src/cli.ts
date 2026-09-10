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
  'work',
  'share',
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
  doctor: () => import('./doctor.js').then((module) => module.runDoctor),
  hook: () => import('./capture-command.js').then((module) => module.runHook),
  capture: () => import('./capture-command.js').then((module) => module.runCapture),
  observe: () => import('./worker/observe.js').then((module) => module.runObserve),
  inject: () => import('./injection/pi.js').then((module) => module.runInject),
  search: () => import('./memories-cli.js').then((module) => module.runSearch),
  timeline: () => import('./memories-cli.js').then((module) => module.runTimeline),
  get: () => import('./memories-cli.js').then((module) => module.runGet),
  why: () => import('./why.js').then((module) => module.runWhy),
  work: () => import('./work-cli.js').then((module) => module.runWork),
  share: () => import('./memories-cli.js').then((module) => module.runShare),
  pin: () => import('./memories-cli.js').then((module) => module.runPin),
  unpin: () => import('./memories-cli.js').then((module) => module.runUnpin),
  delete: () => import('./memories-cli.js').then((module) => module.runDelete),
  pause: () => import('./pause.js').then((module) => module.runPause),
  resume: () => import('./pause.js').then((module) => module.runResume),
  export: () => import('./transfer.js').then((module) => module.runExport),
  import: () => import('./transfer.js').then((module) => module.runImport),
  mcp: () => import('./mcp.js').then((module) => module.runMcp),
  view: () => import('./viewer/server.js').then((module) => module.runView),
  fixture: () => import('./fixture/replay.js').then((module) => module.runFixture),
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
  return `Usage: oboete <command> [options]\n\nCommands: ${knownCommands.join(', ')}.\n\n` +
    'Memory reads: search, get, timeline accept --binding <binding-id> and --history.\n' +
    'Reprocess retained source: oboete observe --reprocess-source <source-id>\n' +
    'Use oboete why <session-id> to inspect source IDs and processing outcomes.\n' +
    'Work: oboete work status [--all] [--json]\n' +
    '      oboete work choose <binding-id> <work-id|new>\n' +
    '      oboete work choose-source <source-id> <work-id|new>\n' +
    '      oboete work complete <work-id>\n' +
    'Sharing: oboete share status | approve <proposal-id> | reject <proposal-id> | adopt <memory-id> [--binding <binding-id>] [--json]\n' +
    'Export: oboete export [file|-] [--format 1|2]\n' +
    'Import: oboete import [file|-] [--dry-run|--apply] [--json]\n' +
    '        oboete import promote <migration-record-id> --map-work <source-work>=<local-work> [--json]\n' +
    '        Native mappings: --map-repo <source>=<repo> [--map-work <source>=<work>]\n' +
    '        Claude-mem: --from claude-mem --map-project <exact-name>=<repo>\n' +
    '        Use --map-project-hash <sha256>=<repo> for a private project name.\n' +
    '        Optional context: --map-context <repo>=<context>. Native v2 and Claude-mem default to preview; v1 defaults to apply.\n';
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

  // `oboete constructor` would otherwise pick Object's constructor off the prototype and crash
  // with a TypeError instead of printing the usage.
  const load = Object.hasOwn(commands, name) ? commands[name] : undefined;
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
