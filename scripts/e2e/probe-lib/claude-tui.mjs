import fs from "node:fs";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import {
  writeClaudeSettings,
} from "./agents.mjs";
import {
  waitUntil,
} from "./process.mjs";
import {
  named,
  parseEvents,
} from "./agent-events.mjs";
import { readyTui, tmuxSession, tuiQuit, tuiSubmit } from "./tmux.mjs";

async function waitPostCompact(eventsPath, n, ms) {
  return (
    (await waitUntil(() => {
      const ev = parseEvents(eventsPath);
      return named(ev, "PostCompact").length >= n ? ev : null;
    }, ms, 400)) || parseEvents(eventsPath)
  );
}

function writeClaudeTuiLauncher(dir, settingsPath) {
  const launch = path.join(dir, "tui.sh");
  fs.writeFileSync(
    launch,
    `#!/bin/bash\nexport PATH="$HOME/.local/bin:$HOME/.npm-global/bin:$PATH"\nexec claude --settings ${JSON.stringify(settingsPath)} --dangerously-skip-permissions\n`,
    { mode: 0o755 },
  );
  return launch;
}

function appendClaudePaneDump(paneLog, tmux, label) {
  try {
    fs.appendFileSync(paneLog, `\n----- ${label} -----\n` + tmux.capture() + "\n");
  } catch {
    /* ignore */
  }
}

function claudeTuiFailure(tmux, paneLog, error, eventsPath) {
  let pane = "";
  try {
    pane = tmux.capture();
  } catch {
    pane = "";
  }
  appendClaudePaneDump(paneLog, tmux, "error");
  fs.appendFileSync(paneLog, "\n" + String(error?.message ? error.message : error) + "\n");
  return {
    events: parseEvents(eventsPath),
    pane,
    error: String(error?.message ? error.message : error),
    eventsPath,
  };
}

export async function tuiTwoCompacts(dir, repo) {
  const { settingsPath, eventsPath } = writeClaudeSettings(dir);
  const launch = writeClaudeTuiLauncher(dir, settingsPath);
  const name = `pbc${process.pid}${Date.now().toString(36)}`.replace(/[^a-zA-Z0-9]/g, "").slice(0, 24);
  // tmuxSession turns each variable into a `-e NAME=VALUE` argument of the tmux client, and a
  // command line is world-readable in /proc, so a pane is handed the variables it needs rather
  // than a copy of the developer's environment. The pane inherits the rest from the tmux server,
  // which probe-lib/tmux.mjs already strips of credentials.
  const tmux = tmuxSession({
    name,
    command: launch,
    cwd: repo,
    // The launch script above exports the PATH the agent needs.
    env: { TERM: "xterm-256color" },
  });
  const paneLog = path.join(dir, "tmux-pane.txt");
  try {
    await sleep(2000);
    await readyTui("claude", tmux);
    appendClaudePaneDump(paneLog, tmux, "after-onboard");
    await tuiSubmit(name, tmux, "Reply with exactly the word DONE. Do not use tools.", { timeoutMs: 120_000 });
    await tmux.waitFor(/\bDONE\b/, 120_000);
    appendClaudePaneDump(paneLog, tmux, "after-done");
    await sleep(2000);
    await tuiSubmit(name, tmux, "/compact", { timeoutMs: 120_000 });
    await sleep(1500);
    if (/compact this conversation|Are you sure|Yes/i.test(tmux.capture())) tmux.send("");
    let events = await waitPostCompact(eventsPath, 1, 120_000);
    appendClaudePaneDump(paneLog, tmux, "after-compact-1");
    if (named(events, "PostCompact").length < 1) {
      tmux.send("");
      await waitPostCompact(eventsPath, 1, 60_000);
    }
    await tuiSubmit(name, tmux, "/compact", { timeoutMs: 120_000 });
    await sleep(1500);
    if (/compact this conversation|Are you sure|Yes/i.test(tmux.capture())) tmux.send("");
    await waitPostCompact(eventsPath, 2, 120_000);
    appendClaudePaneDump(paneLog, tmux, "after-compact-2");
    const pane = tmux.capture();
    await tuiQuit(tmux, name, { timeoutMs: 120_000 });
    return { events: parseEvents(eventsPath), pane, eventsPath };
  } catch (e) {
    return claudeTuiFailure(tmux, paneLog, e, eventsPath);
  } finally {
    try {
      tmux.kill();
    } catch {
      /* ignore */
    }
  }
}
