// Checks that every oboete entry `oboete setup` owns names the candidate bundle.
//
// The point is to look at registrations, not at text. Counting occurrences of the bundle path lets a
// deleted entry hide behind the same path in a comment or an unused property, which is how a missing
// MCP registration went unnoticed for four dogfood runs. So: the hook files are read as JSON and the
// commands are taken from the groups oboete marks as its own, the MCP server is read from its own
// entry, the two config.toml lines must be assignments rather than comments, and the Pi loader must
// carry both of the paths it needs -- the extension it imports and the engine it passes on. Usage:
//
//   node scripts/e2e/candidate-wiring.mjs <bundle> <claude-home> <codex-home> <grok-home> <pi-dir>
import { readFileSync } from "node:fs";
import path from "node:path";

const CLAUDE_EVENTS = ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "PostToolUseFailure", "Stop", "PostCompact", "SessionEnd"];
const CODEX_EVENTS = ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop", "PostCompact", "SessionEnd"];
const GROK_EVENTS = [...CLAUDE_EVENTS.slice(0, 5), "PermissionDenied", ...CLAUDE_EVENTS.slice(5)];

const [, , want, claudeHome, codexHome, grokHome, piDir] = process.argv;
if (!want || !piDir) throw new Error("usage: candidate-wiring.mjs <bundle> <claude> <codex> <grok> <pi>");
const extension = path.join(path.dirname(want), "pi-extension.mjs");
const bad = [];

// The file names below are literals, so only the directory arguments can carry a traversal; each
// read is resolved and required to stay under the directory it was asked for.
function readUnder(directory, ...names) {
  const root = path.resolve(directory);
  const file = path.resolve(root, ...names);
  if (file !== root && !file.startsWith(root + path.sep)) throw new Error(`path escapes ${root}: ${file}`);
  return readFileSync(file, "utf8");
}

// Splitting on the separators a shell command can use keeps this linear, and comparing whole words
// rejects a neighbouring path such as oboete.mjs.backup that a suffix match would accept.
const namesBundle = (value) => value.split(/[\s'"]+/).some((word) => word === want);

function checkHooks(label, directory, names, events) {
  let hooks;
  try {
    ({ hooks } = JSON.parse(readUnder(directory, ...names)));
  } catch (error) {
    bad.push(`${label}: ${error.message}`);
    return;
  }
  for (const event of events) {
    const commands = (hooks?.[event] ?? [])
      .filter((group) => group?.oboete === true)
      .flatMap((group) => group.hooks ?? [])
      .filter((entry) => entry?.type === "command")
      .map((entry) => entry.command);
    if (commands.length !== 1) bad.push(`${label}: ${commands.length} oboete commands for ${event}, expected 1`);
    else if (!namesBundle(commands[0])) bad.push(`${label}: ${event} does not run the candidate: ${commands[0]}`);
  }
}

function checkMcp(label, directory, name) {
  let server;
  try {
    server = JSON.parse(readUnder(directory, name))?.mcpServers?.oboete;
  } catch (error) {
    bad.push(`${label}: ${error.message}`);
    return;
  }
  if (!server) bad.push(`${label}: no mcpServers.oboete registration`);
  else if (!(server.args ?? []).includes(want)) bad.push(`${label}: mcpServers.oboete does not run the candidate`);
}

// An assignment, so that moving the path into a comment on the same line fails the check.
function checkAssignment(label, directory, name) {
  const live = readUnder(directory, name)
    .split("\n")
    .filter((line) => /^\s*[\w.-]+\s*=/.test(line) && line.split("#")[0].includes("oboete/dist/oboete.mjs"));
  if (live.length !== 1) bad.push(`${label}: ${live.length} assignments name the bundle, expected 1`);
  else if (!namesBundle(live[0].split("#")[0])) bad.push(`${label}: ${live[0].trim()}`);
}

function checkPiLoader(label, directory, ...names) {
  const source = readUnder(directory, ...names);
  const strings = source.match(/"[^"\n]*"/g) ?? [];
  const values = strings.map((literal) => literal.slice(1, -1));
  if (!values.some((value) => value === `file://${extension}`)) bad.push(`${label}: does not import ${extension}`);
  if (!values.includes(want)) bad.push(`${label}: does not pass the candidate engine bundle`);
  if (!/export default\s*\(/.test(source)) bad.push(`${label}: no default export, so Pi loads nothing`);
}

checkHooks("claude/settings.json", claudeHome, ["settings.json"], CLAUDE_EVENTS);
checkMcp("claude/.claude.json", claudeHome, ".claude.json");
checkHooks("codex/hooks.json", codexHome, ["hooks.json"], CODEX_EVENTS);
checkHooks("grok/hooks/oboete.json", grokHome, ["hooks", "oboete.json"], GROK_EVENTS);
checkAssignment("codex/config.toml", codexHome, "config.toml");
checkAssignment("grok/config.toml", grokHome, "config.toml");
checkPiLoader("pi/extensions/oboete.js", piDir, "extensions", "oboete.js");

if (bad.length > 0) {
  console.error(bad.join("\n"));
  process.exit(1);
}
console.log(`every owned entry names ${want}`);
