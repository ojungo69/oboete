import type { AgentRow, SetupDeps } from './setup.js';

export const VIEW_LINE = 'Open the memory viewer with `oboete view --open`.';

export function report(deps: SetupDeps, json: boolean, rows: readonly AgentRow[], notes: readonly string[]): void {
  if (json) {
    deps.write(`${JSON.stringify({ agents: rows, notes, view: VIEW_LINE }, null, 2)}\n`);
    return;
  }
  for (const line of notes) deps.write(`${line}\n`);
  if (rows.length > 0) deps.write(table(rows));
  deps.write(`${VIEW_LINE}\n`);
}

function table(rows: readonly AgentRow[]): string {
  return renderTable(
    ['agent', 'wired', 'probe', 'trust', 'native memory'],
    rows.map((row) => [row.agent, row.wired, row.probe, row.trust, row.native_memory ?? 'none']),
  );
}

export function renderTable(
  header: readonly string[],
  rows: readonly (readonly string[])[],
  extras: readonly (string | undefined)[] = [],
): string {
  const widths = header.map((title, column) =>
    Math.max(title.length, ...rows.map((row) => row[column]?.length ?? 0)),
  );
  const line = (cell: readonly string[]): string =>
    `${cell.map((value, column) => value.padEnd(widths[column] ?? 0)).join('  ').trimEnd()}\n`;
  let out = `\n${line(header)}`;
  for (const [index, row] of rows.entries()) {
    out += line(row);
    const extra = extras[index];
    if (extra !== undefined && extra !== '') out += extra;
  }
  return `${out}\n`;
}

export function describe(error: unknown): string {
  return error instanceof Error ? error.message.split('\n')[0] ?? error.name : String(error);
}
