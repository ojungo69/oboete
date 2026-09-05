import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  binVersion,
  childEnv,
  copyMode,
  gitInit,
  parseJsonl,
  parseObservationsJson,
  piContentText,
  runTimed,
  seedGrokHome,
} from "../probe-lib/agents.mjs";

const ROW_CLI =
  "`agent-cli` preset: headless JSON output of `claude -p`, `codex exec`, `grok -p` for a summarization prompt";
const ROW_TRANSPORT = "NIM / OpenRouter / Gemini transport, auth header, model id";

const SUMMARIZE =
  "Summarize the following session log into JSON with keys observations (array of {type, title, body}) and summary (string); respond with JSON only, no prose. LOG: user asked to add a retry to fetchUser; assistant edited src/api.ts adding exponential backoff with 3 attempts; tests passed.";

const SCHEMA = {
  type: "object",
  properties: { ok: { type: "boolean" } },
  required: ["ok"],
  additionalProperties: false,
};

const PRESETS = {
  nim: {
    url: "https://integrate.api.nvidia.com/v1/chat/completions",
    model: "meta/llama-3.2-11b-vision-instruct",
    cred: ["OBOETE_NIM_API_KEY"],
    headers: (c) => ({ Authorization: "Bearer " + c.OBOETE_NIM_API_KEY }),
    authName: "Authorization",
    openai: true,
  },
  openrouter: {
    url: "https://openrouter.ai/api/v1/chat/completions",
    model: "openai/gpt-4o-mini",
    cred: ["OBOETE_OPENROUTER_API_KEY"],
    headers: (c) => ({ Authorization: "Bearer " + c.OBOETE_OPENROUTER_API_KEY }),
    authName: "Authorization",
    openai: true,
  },
  gemini: {
    url: "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
    model: "gemini-2.5-flash",
    cred: ["OBOETE_GEMINI_API_KEY"],
    headers: (c) => ({ Authorization: "Bearer " + c.OBOETE_GEMINI_API_KEY }),
    authName: "Authorization",
    openai: true,
  },
  "workers-ai": {
    url: (c) =>
      `https://api.cloudflare.com/client/v4/accounts/${c.OBOETE_CF_ACCOUNT_ID}/ai/run/@cf/zai-org/glm-4.7-flash`,
    model: "@cf/zai-org/glm-4.7-flash",
    cred: ["OBOETE_CF_API_TOKEN", "OBOETE_CF_ACCOUNT_ID"],
    headers: (c) => ({ Authorization: "Bearer " + c.OBOETE_CF_API_TOKEN }),
    authName: "Authorization",
    openai: false,
  },
};

function loadCredentials() {
  const file = path.join(os.homedir(), ".oboete-credentials");
  const out = {};
  if (!fs.existsSync(file)) return out;
  for (const line of fs.readFileSync(file, "utf8").split("\n")) {
    const m = line.match(/^\s*(?:export\s+)?(OBOETE_[A-Z0-9_]+)=(.*)$/);
    if (!m) continue;
    let v = m[2].trim();
    if ((v.startsWith('"') && v.endsWith('"')) || (v.startsWith("'") && v.endsWith("'"))) v = v.slice(1, -1);
    if (v) out[m[1]] = v;
  }
  return out;
}

function pickClaudeModel(env) {
  const mu = env?.modelUsage || {};
  let best = null;
  let n = -1;
  for (const [id, u] of Object.entries(mu)) {
    const v = (u && (u.output_tokens ?? u.outputTokens)) || 0;
    if (v > n) {
      n = v;
      best = id;
    }
  }
  return best;
}

async function dummyKeySelfCheck() {
  const start = Date.now();
  try {
    const res = await fetch("https://integrate.api.nvidia.com/v1/chat/completions", {
      method: "POST",
      headers: { Authorization: "Bearer nvapi-oboete-dummy", "content-type": "application/json" },
      body: JSON.stringify({
        model: "meta/llama-3.2-11b-vision-instruct",
        messages: [{ role: "user", content: "reply with {\"ok\":true}" }],
        max_tokens: 8,
      }),
      signal: AbortSignal.timeout(20_000),
    });
    return { status: res.status, elapsed_ms: Date.now() - start };
  } catch (e) {
    return { status: "error", error: String(e && e.message ? e.message : e), elapsed_ms: Date.now() - start };
  }
}

function schemaBody(model, openai) {
  const prompt = 'Return a JSON object with a boolean field "ok" set to true.';
  const messages = [{ role: "user", content: prompt }];
  if (openai) {
    return {
      model,
      messages,
      response_format: { type: "json_schema", json_schema: { name: "tiny_ok", strict: true, schema: SCHEMA } },
      max_tokens: 64,
    };
  }
  return { messages, response_format: { type: "json_schema", json_schema: SCHEMA } };
}

function textBody(model, openai) {
  const prompt = 'Reply with exactly one JSON object {"ok": true} and nothing else.';
  const messages = [{ role: "user", content: prompt }];
  if (openai) return { model, messages, max_tokens: 64 };
  return { messages };
}

function extractModelAndText(body) {
  const d = body?.result && typeof body.result === "object" ? { ...body, ...body.result } : body;
  const model = d?.model || d?.result?.model || null;
  const choice = d?.choices?.[0]?.message;
  let text = choice?.content ?? d?.response ?? d?.result?.response ?? null;
  if (Array.isArray(text)) {
    text = text.map((p) => (typeof p === "string" ? p : p?.text || "")).join("");
  }
  return { model, text: typeof text === "string" ? text : text == null ? null : JSON.stringify(text) };
}

function schemaHonoured(text) {
  if (!text) return false;
  try {
    const obj = JSON.parse(String(text).trim().replace(/^```(?:json)?\s*|\s*```$/g, ""));
    return obj && typeof obj === "object" && typeof obj.ok === "boolean" && Object.keys(obj).length === 1;
  } catch {
    return false;
  }
}

async function callPreset(preset, creds, withSchema) {
  const url = typeof preset.url === "function" ? preset.url(creds) : preset.url;
  const headers = { "content-type": "application/json", ...preset.headers(creds) };
  const body = withSchema ? schemaBody(preset.model, preset.openai) : textBody(preset.model, preset.openai);
  const start = Date.now();
  const res = await fetch(url, {
    method: "POST",
    headers,
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(60_000),
  });
  const raw = await res.text();
  let parsed = null;
  try {
    parsed = JSON.parse(raw);
  } catch {
    parsed = null;
  }
  const { model, text } = extractModelAndText(parsed || {});
  return {
    http: res.status,
    elapsed_ms: Date.now() - start,
    model_reported: model || null,
    text,
    schema_rejected: withSchema && (res.status === 400 || res.status === 422),
  };
}

async function providerProbe(name, ctx) {
  const preset = PRESETS[name];
  const creds = loadCredentials();
  const evidence = [];
  if (name === "nim") {
    const dummy = await dummyKeySelfCheck();
    evidence.push(
      dummy.status === "error"
        ? `dummy-key self-check: error ${dummy.error}`
        : `dummy-key self-check: HTTP ${dummy.status} ${dummy.elapsed_ms}ms`,
    );
  }
  const missing = preset.cred.filter((k) => !creds[k]);
  if (missing.length) {
    evidence.push("credential absent");
    return { status: "skipped", evidence };
  }
  try {
    let first = await callPreset(preset, creds, true);
    let structured = "rejected";
    if (first.http >= 200 && first.http < 300 && schemaHonoured(first.text)) structured = "honoured";
    else if (first.schema_rejected || (first.http >= 200 && first.http < 300 && !schemaHonoured(first.text))) {
      const second = await callPreset(preset, creds, false);
      first = {
        ...second,
        schema_first_http: first.http,
      };
      structured = schemaHonoured(second.text) ? "text-JSON" : "unusable";
    }
    evidence.push(`HTTP ${first.http}`);
    evidence.push(`auth=${preset.authName}`);
    evidence.push(`model=${first.model_reported || "not reported"}`);
    evidence.push(`response_format=${structured}`);
    evidence.push(`elapsed_ms=${first.elapsed_ms}`);
    const pass = first.http >= 200 && first.http < 300;
    return {
      status: pass ? "pass" : "fail",
      evidence,
      data: { structured, http: first.http, model_reported: first.model_reported },
    };
  } catch (e) {
    evidence.push(String(e && e.message ? e.message : e));
    return { status: "fail", evidence };
  }
}

export const probes = [
  {
    id: "agent-cli-json",
    agent: "providers",
    row: ROW_CLI,
    async run(ctx) {
      const repo = gitInit(path.join(ctx.dir, "repo"));
      const evidence = [];
      const per = {};
      const HOME = os.homedir();

      const claude = await runTimed(
        ["claude", "-p", SUMMARIZE, "--output-format", "json", "--dangerously-skip-permissions"],
        {
          cwd: repo,
          env: childEnv(),
          stdoutPath: path.join(ctx.dir, "claude_out.json"),
          stderrPath: path.join(ctx.dir, "claude_err.txt"),
        },
      );
      let claudeObj = null;
      try {
        claudeObj = JSON.parse(claude.stdout);
      } catch {
        claudeObj = null;
      }
      const claudeText = typeof claudeObj?.result === "string" ? claudeObj.result : "";
      const claudeOk = !!parseObservationsJson(claudeText);
      const claudeModel = pickClaudeModel(claudeObj);
      per.claude = { ok: claudeOk, elapsed_s: claude.elapsedMs / 1000, model: claudeModel, where: "result" };
      evidence.push(
        `claude ${claudeOk ? "pass" : "fail"} ${per.claude.elapsed_s.toFixed(2)}s text=result model=${claudeModel || "none"} ver=${binVersion("claude")}`,
      );

      const lastMsg = path.join(ctx.dir, "codex_lastmsg.txt");
      const cHome = path.join(ctx.dir, "codex-home");
      fs.mkdirSync(cHome, { recursive: true });
      copyMode(path.join(HOME, ".codex/auth.json"), path.join(cHome, "auth.json"));
      const codex = await runTimed(
        ["codex", "exec", "--json", "--skip-git-repo-check", "--output-last-message", lastMsg, SUMMARIZE],
        {
          cwd: repo,
          env: childEnv({ CODEX_HOME: cHome }),
          stdoutPath: path.join(ctx.dir, "codex_out.jsonl"),
          stderrPath: path.join(ctx.dir, "codex_err.txt"),
        },
      );
      const last = fs.existsSync(lastMsg) ? fs.readFileSync(lastMsg, "utf8") : "";
      let itemText = "";
      for (const line of parseJsonl(codex.stdout)) {
        if (line.type === "item.completed" && line.item?.text) itemText = line.item.text;
      }
      const codexText = last.trim() || itemText;
      const codexOk = !!parseObservationsJson(codexText);
      per.codex = { ok: codexOk, elapsed_s: codex.elapsedMs / 1000, model: "none", where: "output-last-message" };
      evidence.push(
        `codex ${codexOk ? "pass" : "fail"} ${per.codex.elapsed_s.toFixed(2)}s text=output-last-message model=none ver=${binVersion("codex")}`,
      );

      const grokSeed = ctx.grokSeed || (await seedGrokHome(path.dirname(ctx.dir)));
      const gHome = path.join(ctx.dir, "grok-home");
      fs.cpSync(grokSeed, gHome, { recursive: true });
      const grok = await runTimed(["grok", "-p", SUMMARIZE, "--output-format", "json", "--always-approve"], {
        cwd: repo,
        env: childEnv({ GROK_HOME: gHome, GROK_CLAUDE_HOOKS_ENABLED: "0" }),
        stdoutPath: path.join(ctx.dir, "grok_out.json"),
        stderrPath: path.join(ctx.dir, "grok_err.txt"),
      });
      let grokObj = null;
      try {
        grokObj = JSON.parse(grok.stdout);
      } catch {
        grokObj = null;
      }
      const grokText = typeof grokObj?.text === "string" ? grokObj.text : "";
      const grokOk = !!parseObservationsJson(grokText);
      const grokModel = grokObj?.modelUsage ? Object.keys(grokObj.modelUsage)[0] : null;
      per.grok = { ok: grokOk, elapsed_s: grok.elapsedMs / 1000, model: grokModel, where: "text" };
      evidence.push(
        `grok ${grokOk ? "pass" : "fail"} ${per.grok.elapsed_s.toFixed(2)}s text=text model=${grokModel || "none"} ver=${binVersion("grok")}`,
      );

      const pHome = path.join(ctx.dir, "piagent");
      const sessions = path.join(pHome, "sessions");
      fs.mkdirSync(sessions, { recursive: true });
      copyMode(path.join(HOME, ".pi/agent/auth.json"), path.join(pHome, "auth.json"));
      for (const name of ["settings.json", "models-store.json"]) {
        const src = path.join(HOME, ".pi/agent", name);
        if (fs.existsSync(src)) fs.copyFileSync(src, path.join(pHome, name));
      }
      const pi = await runTimed(["pi", "-p", SUMMARIZE, "--mode", "json", "--session-dir", sessions], {
        cwd: repo,
        env: childEnv({ PI_CODING_AGENT_DIR: pHome }),
        stdoutPath: path.join(ctx.dir, "pi_out.jsonl"),
        stderrPath: path.join(ctx.dir, "pi_err.txt"),
      });
      let piText = "";
      let piModel = null;
      for (const line of parseJsonl(pi.stdout)) {
        if (line.type === "message_end" || line.type === "turn_end") {
          const t = piContentText(line.message?.content);
          if (t) piText = t;
          if (line.message?.model) piModel = line.message.model;
        }
      }
      const piOk = !!parseObservationsJson(piText);
      per.pi = { ok: piOk, elapsed_s: pi.elapsedMs / 1000, model: piModel, where: "turn_end text blocks" };
      evidence.push(
        `pi ${piOk ? "pass" : "fail"} ${per.pi.elapsed_s.toFixed(2)}s text=turn_end text blocks model=${piModel || "none"} ver=${binVersion("pi")}`,
      );

      const all = claudeOk && codexOk && grokOk && piOk;
      return { status: all ? "pass" : "fail", evidence, data: per };
    },
  },
  {
    id: "provider-nim",
    agent: "providers",
    row: ROW_TRANSPORT,
    run: (ctx) => providerProbe("nim", ctx),
  },
  {
    id: "provider-openrouter",
    agent: "providers",
    row: ROW_TRANSPORT,
    run: (ctx) => providerProbe("openrouter", ctx),
  },
  {
    id: "provider-gemini",
    agent: "providers",
    row: ROW_TRANSPORT,
    run: (ctx) => providerProbe("gemini", ctx),
  },
  {
    id: "provider-workers-ai",
    agent: "providers",
    row: ROW_TRANSPORT,
    run: (ctx) => providerProbe("workers-ai", ctx),
  },
];
