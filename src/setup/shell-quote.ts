// Claude Code, Codex and Grok Build all run a `command` hook handler through a shell, so the one
// command line oboete writes into the developer's configuration has to survive that shell exactly
// as it was written (contracts/agents.md "Capture and injection per agent"). One rule for the three
// writers, so a path with a space -- or with a quote in it -- cannot be handled three ways.
/** A single-quoted POSIX shell word; a single quote inside one is closed, escaped and reopened. */
export function shellQuote(value: string): string {
  return `'${value.replaceAll("'", `'\\''`)}'`;
}
