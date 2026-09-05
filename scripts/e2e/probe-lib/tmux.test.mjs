import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { PreconditionError } from "./agents.mjs";
import { TMUX_SOCKET, readyTui, settleTui, tmux, tmuxSession, tuiQuit, tuiSubmit } from "./tmux.mjs";

const hasTmux = spawnSync("tmux", ["-V"], { encoding: "utf8" }).status === 0;

test("pane settling restarts on changes and respects the lifecycle timeout", async () => {
  let clock = 0;
  const dependencies = { now: () => clock, sleep: async (ms) => { clock += ms; } };
  await settleTui({ capture: () => `${clock < 600 ? "finishing hook" : "done"}\n› Ask Codex` }, {
    timeoutMs: 2_000,
  }, dependencies);
  assert.equal(clock, 1_600);

  clock = 0;
  await assert.rejects(settleTui({ capture: () => `${clock}\n› Ask Codex` }, {
    timeoutMs: 1_500,
  }, dependencies), (error) => error instanceof PreconditionError && /did not settle within 1.5s/.test(error.message));
  assert.equal(clock, 1_500);
});

test("Claude's persistent onboarding pane respects the readiness deadline", async () => {
  let clock = 0;
  await assert.rejects(readyTui("claude", { capture: () => "Press enter to continue", send() {} }, {
    timeoutMs: 2_000,
  }, { now: () => clock, sleep: async (ms) => { clock += ms; } }), /composer unavailable/);
  assert.equal(clock, 2_000);
});

test("Claude's other onboarding panes still accept Enter; Codex hook review is a failure", async () => {
  let pane = "Choose the text style";
  let clock = 0;
  const dependencies = { now: () => clock, sleep: async (ms) => { clock += ms; } };
  await readyTui("claude", {
    capture: () => pane,
    send: (key) => { assert.equal(key, ""); pane = "Ask Claude"; },
  }, { timeoutMs: 2_000 }, dependencies);
  for (const text of ["hook needs review", "review required", "Trust to trust", "New hook", "untrusted hook"]) {
    await assert.rejects(
      readyTui("codex", { capture: () => `${text}\n› Ask Codex` }, { timeoutMs: 2_000 }, dependencies),
      (error) => error.name === "Error" && error.message.includes(text) && error.message.includes("FR-031"),
    );
  }
});

for (const prompt of ["Recall the repository's build token from oboete memory. ".repeat(5), "/compact", "/new"]) {
  test(`TUI submission waits for MCP startup to settle and retries dropped Enter: ${prompt.slice(0, 40)}`, async () => {
    let clock = 0;
    let typedAt;
    let enters = 0;
    let snapshots = 0;
    let snapshotAt;
    let pane;
    const tui = { capture: () => pane ?? `${clock < 600 ? "MCP starting" : "MCP startup incomplete (failed: oboete)"}\n› Ask Codex` };
    const result = await tuiSubmit("test", tui, prompt, {
      timeoutMs: 2_000,
      beforeSubmit: () => { snapshots += 1; snapshotAt = clock; },
    }, {
      now: () => clock,
      sleep: async (ms) => { clock += ms; },
      tmux: (argv) => {
        if (argv.includes("-l")) {
          typedAt = clock;
          assert.equal(argv.at(-1), prompt);
          // tmux's 200-column pane wraps long prompts; the whole string is absent.
          pane = `› ${prompt.slice(0, 198)}\n  ${prompt.slice(198)}`;
          if (prompt.length > 198) assert.equal(pane.includes(prompt), false);
        }
        if (argv.at(-1) === "C-m") {
          assert.equal(snapshots, 1, "capture the boundary once, even when Enter is retried");
          if (++enters === 1) assert.equal(snapshotAt, clock, "snapshot immediately before the first Enter");
          if (enters === 2) pane += "\n• DONE";
        }
        return { status: 0 };
      },
    });
    assert.ok(typedAt >= 1_600, `typed before startup was quiet for 1 s: ${typedAt}`);
    assert.equal(enters, 2);
    assert.match(result, /• DONE/);
  });
}

test("TUI stops retrying when the composer clears, even with an identical prompt in history", async () => {
  let clock = 0;
  let pane = "› Ask Codex";
  let enters = 0;
  await tuiSubmit("test", { capture: () => pane }, "Reply DONE", {}, {
    now: () => clock,
    sleep: async (ms) => { clock += ms; },
    tmux: (argv) => {
      if (argv.includes("-l")) pane = "› Reply DONE";
      if (argv.at(-1) === "C-m") {
        enters += 1;
        pane = "› Reply DONE\n• DONE\n› Ask Codex";
      }
      return { status: 0 };
    },
  });
  assert.equal(enters, 1);
});

test("TUI refuses to silently leave a prompt in the composer after three Enter attempts", async () => {
  let clock = 0;
  let enters = 0;
  let pane = "› Ask Codex";
  await assert.rejects(tuiSubmit("test", { capture: () => pane }, "stuck prompt", {}, {
    now: () => clock,
    sleep: async (ms) => { clock += ms; },
    tmux: (argv) => {
      if (argv.includes("-l")) pane = "› stuck prompt\nMCP still starting";
      if (argv.at(-1) === "C-m") enters += 1;
      return { status: 0 };
    },
  }), /after 3 Enter attempts; pane=› stuck prompt\nMCP still starting/);
  assert.equal(enters, 3);
  assert.ok(clock >= 16_000);
});

test("submission and quit pass the caller's deadline to settling and block a changing pane", async () => {
  let clock = 0;
  const keys = [];
  let killed = false;
  const tui = { capture: () => `starting ${clock}`, kill: () => { killed = true; } };
  const dependencies = {
    now: () => clock,
    sleep: async (ms) => { clock += ms; },
    tmux: (argv) => { keys.push(argv); return { status: 0 }; },
  };
  await assert.rejects(tuiSubmit("test", tui, "Reply DONE", { timeoutMs: 1_500 }, dependencies),
    (error) => error instanceof PreconditionError && /did not settle within 1.5s/.test(error.message));
  assert.equal(clock, 1_500);
  assert.deepEqual(keys, []);
  await tuiQuit(tui, "test", { timeoutMs: 1_500 }, dependencies);
  assert.equal(clock, 3_000);
  assert.equal(killed, true);
  assert.deepEqual(keys, []);
});

for (const composer of ["> ", "│ > "]) {
  test(`Claude retries an unsubmitted prompt in its ${JSON.stringify(composer)} composer`, async () => {
    let clock = 0;
    let pane = `${composer}\nshift+tab`;
    let enters = 0;
    const prompt = "Recall the repository's build token only from oboete memory.";
    await tuiSubmit("test", { capture: () => pane }, prompt, { timeoutMs: 2_000 }, {
      now: () => clock,
      sleep: async (ms) => { clock += ms; },
      tmux: (argv) => {
        if (argv.includes("-l")) pane = `╭─────────────────╮\n${composer}${prompt}\n╰─────────────────╯`;
        if (argv.at(-1) === "C-m" && ++enters === 2) pane += "\n● DONE";
        return { status: 0 };
      },
    });
    assert.equal(enters, 2);
  });
}

for (const [agent, dialog, key] of [
  ["claude", "WARNING: Claude Code running in Bypass Permissions mode\n❯ No, exit\n  Yes, I accept", "Down"],
  ["claude", "Do you trust this folder?\n❯ No, exit\n  Yes, I trust this folder", "Down"],
  ["codex", "Do you trust the contents of this directory?\n› 1. Yes, continue\n  2. No, quit", ""],
]) {
  test(`${agent} accepts ${dialog.split("\n")[0]} with the correct keys`, async () => {
    const calls = [];
    let pane = dialog;
    let clock = 0;
    const tui = tmuxSession({ name: "dialog" }, { tmux: (argv) => {
      calls.push(argv);
      if (argv[0] === "send-keys") pane = agent === "codex" ? "› Ask Codex" : "Ask Claude";
      return { status: 0, stdout: argv[0] === "capture-pane" ? pane : "0" };
    } });
    await readyTui(agent, tui, { timeoutMs: 2_000 }, {
      now: () => clock, sleep: async (ms) => { clock += ms; },
    });
    assert.deepEqual(calls.filter((argv) => argv[0] === "send-keys"), [
      ["send-keys", "-t", "dialog", key, "Enter"],
    ]);
  });
}

for (const [label, wait] of [
  ["ready", (tui, dependencies) => readyTui("codex", tui, { timeoutMs: 2_000 }, dependencies)],
  ["settle", (tui, dependencies) => settleTui(tui, { timeoutMs: 2_000 }, dependencies)],
  ["submit", (tui, dependencies) => tuiSubmit("dead", tui, "Reply DONE", { timeoutMs: 2_000 }, dependencies)],
]) {
  test(`${label} stops on a dead pane with its last output, and cleanup kills the session`, async () => {
    let dead = false;
    let clock = 0;
    const calls = [];
    const tui = tmuxSession({ name: "dead", command: "exit 7" }, { tmux: (argv) => {
      calls.push(argv);
      const output = ["startup failed", "last diagnostic"];
      if (dead) {
        if (!argv.includes("-S")) output.shift(); // The dead-pane banner scrolls the first line out of view.
        output.push("Pane is dead (status 7)");
      }
      return {
        status: 0,
        stdout: argv[0] === "capture-pane" ? output.join("\n") : dead ? "1\n" : "0\n",
      };
    } });
    const launch = calls.find((argv) => argv[0] === "new-session");
    assert.deepEqual(launch.slice(-7), ["exit 7", ";", "set-option", "-t", "dead", "remain-on-exit", "on"]);
    await assert.rejects(wait(tui, {
      now: () => clock,
      sleep: async (ms) => { clock += ms; dead = true; },
    }), (error) => error instanceof PreconditionError &&
      error.message === "Tmux pane dead exited; pane=startup failed\nlast diagnostic\nPane is dead (status 7)");
    assert.equal(clock, 200, "stop at the first capture after exit");
    assert.ok(calls.some((argv) => JSON.stringify(argv) === JSON.stringify(["list-panes", "-t", "dead", "-F", "#{pane_dead}"])));
    assert.ok(calls.some((argv) => JSON.stringify(argv) === JSON.stringify(["capture-pane", "-p", "-S", "-200", "-t", "dead"])));
    tui.kill();
    assert.deepEqual(calls.at(-1), ["kill-session", "-t", "dead"]);
    assert.equal(calls.some((argv) => argv[0] === "send-keys"), false);
  });
}

// FR-016: the pane environment comes from the tmux server, and a server left running by an earlier
// probe was started before this rule existed, so a filtered launcher environment is not enough.
test("a pane sees no credential a running server carries", { skip: !hasTmux }, async () => {
  const name = "oboete-tmux-test-" + process.pid;
  const dir = mkdtempSync(join(tmpdir(), "oboete-tmux-test-"));
  const out = join(dir, "seen.txt");
  // A server has to be running for its environment to be the one a new pane reads, which is the
  // situation this covers: the server outlives the probe that started it.
  const holder = "oboete-tmux-test-holder-" + process.pid;
  tmux(["new-session", "-d", "-s", holder, "bash"]);
  // Put the credential where only the server can hand it on: its own global environment.
  tmux(["set-environment", "-g", "OBOETE_CF_API_TOKEN", "leak-marker"]);
  const session = tmuxSession({ name, command: "bash", cwd: dir });
  try {
    // The pane echoes the command as well as its output, so the marker carries the exit status:
    // `done-$?` is what was typed, `done-0` or `done-1` is what the shell answered.
    session.send(`printenv OBOETE_CF_API_TOKEN > ${out}; echo done-$?`);
    await session.waitFor(/done-[01]/);
    assert.equal(readFileSync(out, "utf8").includes("leak-marker"), false);
  } finally {
    session.kill();
    spawnSync("tmux", ["-L", TMUX_SOCKET, "kill-session", "-t", holder]);
    spawnSync("tmux", ["-L", TMUX_SOCKET, "set-environment", "-g", "-u", "OBOETE_CF_API_TOKEN"]);
    rmSync(dir, { recursive: true, force: true });
  }
});
