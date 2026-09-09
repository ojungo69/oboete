import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

export function lifecycleDatabase(t) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "oboete-lifecycle-db-"));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const oboeteHome = path.join(directory, ".oboete");
  fs.mkdirSync(oboeteHome);
  const file = path.join(oboeteHome, "memory.db");
  const db = new DatabaseSync(file);
  const migrations = new URL("../../src/db/migrations/", import.meta.url);
  for (const name of fs.readdirSync(migrations).filter((name) => name.endsWith(".sql")).sort()) {
    db.exec(fs.readFileSync(new URL(name, migrations), "utf8"));
  }
  db.exec(`
    PRAGMA journal_mode = WAL;
    PRAGMA foreign_keys = ON;
    INSERT INTO repos (id, identity_kind, normalized_identity) VALUES ('repo', 'remote', 'test');
    INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, context_epoch, status, started_at, summary_state)
      VALUES ('session', 'repo', 'codex', 'native', 'session', 1, 'active', 1, 'pending');
    INSERT INTO raw_events (id, repo_id, session_id, kind, payload_json, captured_at)
      VALUES ('event', 'repo', 'session', 'session_start', '{"source":"compact"}', 2);
    INSERT INTO injections (id, session_id, conversation_id, kind, channel, state, context_epoch, pack_hash, delivery_count, created_at)
      VALUES ('injection', 'session', 'session', 'session_start', 'codex:SessionStart', 'emitted', 1, 'hash', 1, 3);
    INSERT INTO injection_items (id, injection_id, conversation_id, context_epoch, memory_id, decision)
      VALUES (1, 'injection', 'session', 1, 'memory', 'included');
    INSERT INTO memories (id, repo_id, type, source_session_id, content_hash, sensitivity)
      VALUES ('memory', 'repo', 'session_summary', 'session', 'hash', 'local_only');
  `);
  t.after(() => db.close());
  return { directory, file, db };
}

export function lifecycleSnapshot(agent) {
  return {
    sessions: [
      {
        id: "parent",
        repoId: "repo",
        nativeSessionId: "native-parent",
        conversationId: "parent",
        contextEpoch: 0,
        lastCompactionKey: null,
        status: "ended",
        startedAt: 1,
      },
    ],
    events: [
      {
        id: "event-start",
        sessionId: "parent",
        nativeSessionId: "native-parent",
        kind: "session_start",
        payload: { source: "startup" },
        capturedAt: 1,
      },
      {
        id: "event-prompt",
        sessionId: "parent",
        nativeSessionId: "native-parent",
        kind: "prompt",
        payload: {},
        capturedAt: 2,
      },
    ],
    injections: [],
    items: [],
    memories: [{ id: "memory-parent", repoId: "repo", sourceSessionId: "parent", deletedAt: null }],
    agent,
  };
}

export function childSession() {
  return {
    id: "child",
    repoId: "repo",
    nativeSessionId: "native-child",
    conversationId: "child",
    contextEpoch: 0,
    lastCompactionKey: null,
    status: "active",
    startedAt: 2,
  };
}

export function addMemoryInjection(snapshot, agent, kind, channel, sessionId = "child", contextEpoch = 0) {
  const injectionId = `injection-${sessionId}-${contextEpoch}-${snapshot.injections.length}`;
  snapshot.injections.push({
    id: injectionId,
    sessionId,
    conversationId: sessionId,
    kind,
    channel,
    state: "emitted",
    contextEpoch,
    packHash: `pack-${sessionId}`,
    deliveryCount: 1,
  });
  snapshot.items.push({
    id: snapshot.items.length + 1,
    injectionId,
    conversationId: sessionId,
    contextEpoch,
    memoryId: "memory-parent",
    decision: "included",
    agent,
  });
}

export function isolatedAccount(t) {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "oboete-account-")));
  t.after(() => fs.rmSync(home, { recursive: true, force: true }));
  const hooksPath = path.join(home, ".codex", "hooks.json");
  fs.mkdirSync(path.dirname(hooksPath), { recursive: true });
  fs.writeFileSync(
    hooksPath,
    JSON.stringify({
      hooks: { SessionStart: [{ hooks: [{ type: "command", command: "oboete hook" }], oboete: true }] },
    }),
  );
  fs.writeFileSync(
    path.join(home, ".codex", "config.toml"),
    `[mcp_servers.oboete]\ncommand = "node"\n\n[hooks.state."${hooksPath}:session_start:0:0"]\ntrusted_hash = "sha256:aaa"\n`,
  );
  fs.mkdirSync(path.join(home, ".claude"), { recursive: true });
  fs.writeFileSync(path.join(home, ".claude", "settings.json"), "{}\n");
  fs.mkdirSync(path.join(home, ".oboete"), { recursive: true });
  fs.writeFileSync(path.join(home, ".oboete", "config.toml"), "[observer]\npreset = \"nim\"\n");
  return { home, hooksPath };
}

/** Every child the harness would spawn, recorded; the agents behave the way a passing pair does. */
export function recordingDependencies(home) {
  const calls = [];
  let facts = [];
  return {
    calls,
    observerLeaseIsFree: () => true,
    gitInit: (repo) => {
      fs.mkdirSync(repo, { recursive: true });
      return repo;
    },
    // The double follows the real childEnv (probe-lib/process.mjs): credentials only on request.
    childEnv: (extra = {}, { credentials = false } = {}) => ({
      PATH: "/usr/bin",
      HOME: home,
      ...(credentials
        ? {
            OBOETE_NIM_API_KEY: "nim-secret",
            OBOETE_CF_API_TOKEN: "cf-secret",
            OBOETE_CF_ACCOUNT_ID: "cf-account",
          }
        : {}),
      ...extra,
    }),
    runTimed: async (argv, options) => {
      calls.push({ argv, env: options.env, cwd: options.cwd, stdoutPath: options.stdoutPath });
      const prompt = argv.find((word) => word.includes("durable facts about this repository"));
      if (prompt) {
        facts = prompt.split("\n").slice(1, 4);
        fs.writeFileSync(path.join(options.cwd, "NOTES.md"), `${facts.join("\n")}\n`);
      }
      const stdout =
        argv[0] === "oboete" && argv[1] === "search"
          ? JSON.stringify({ memories: [{ text: facts.join(" ") }] })
          : JSON.stringify({ result: facts.join(" | ") });
      return { exitCode: 0, signal: null, elapsedMs: 1, stdout, stderr: "" };
    },
    sleep: async () => {},
    now: () => Date.parse("2026-09-05T09:00:00.000Z"),
    env: {},
    home,
    repoRoot: home,
    log: () => {},
  };
}
