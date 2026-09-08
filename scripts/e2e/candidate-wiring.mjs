// Checks that every oboete entry `oboete setup` owns names the candidate bundle.
//
// Counting text matches is not enough: an entry can be deleted and its path left behind in a
// comment, and the total is unchanged. The hook and MCP entries live in JSON, so they are read as
// JSON and counted per file; the two config.toml lines and the Pi loader are read as text with
// commented-out lines excluded. Usage:
//
//   node scripts/e2e/candidate-wiring.mjs <bundle> <claude-home> <codex-home> <grok-home> <pi-dir>
import { readFileSync } from "node:fs";
import path from "node:path";

const SUFFIX = "oboete/dist/oboete.mjs";
const [, , want, claudeHome, codexHome, grokHome, piDir] = process.argv;
if (!want || !piDir) throw new Error("usage: candidate-wiring.mjs <bundle> <claude> <codex> <grok> <pi>");

// The file names below are literals, so only the directory arguments can carry a traversal; each
// read is resolved and required to stay under the directory it was asked for.
function readUnder(directory, ...names) {
  const root = path.resolve(directory);
  const file = path.resolve(root, ...names);
  if (file !== root && !file.startsWith(root + path.sep)) throw new Error(`path escapes ${root}: ${file}`);
  return readFileSync(file, "utf8");
}

// Splitting on the separators a shell command or a JSON string can use keeps this linear; a regex
// that matches the path and everything before it backtracks on long lines.
const bundlePaths = (value) => value.split(/[\s'"]+/).filter((word) => word.endsWith(SUFFIX));

const bad = [];
const stringsIn = (value, out = []) => {
  if (typeof value === "string") out.push(...bundlePaths(value));
  else if (value && typeof value === "object") for (const nested of Object.values(value)) stringsIn(nested, out);
  return out;
};

function checkJson(label, expected, directory, ...names) {
  let found;
  try {
    found = stringsIn(JSON.parse(readUnder(directory, ...names)));
  } catch (error) {
    bad.push(`${label}: ${error.message}`);
    return;
  }
  if (found.length !== expected) bad.push(`${label}: ${found.length} entries name the bundle, expected ${expected}`);
  for (const value of found) if (value !== want) bad.push(`${label}: ${value}`);
}

function checkLine(label, directory, ...names) {
  const live = readUnder(directory, ...names)
    .split("\n")
    .filter((line) => line.includes(SUFFIX) && !/^\s*(#|\/\/)/.test(line));
  if (live.length !== 1) bad.push(`${label}: ${live.length} live lines name the bundle, expected 1`);
  else for (const value of bundlePaths(live[0])) if (value !== want) bad.push(`${label}: ${value}`);
}

checkJson("claude/settings.json", 8, claudeHome, "settings.json");
checkJson("claude/.claude.json", 1, claudeHome, ".claude.json");
checkJson("codex/hooks.json", 7, codexHome, "hooks.json");
checkJson("grok/hooks/oboete.json", 9, grokHome, "hooks", "oboete.json");
checkLine("codex/config.toml", codexHome, "config.toml");
checkLine("grok/config.toml", grokHome, "config.toml");
checkLine("pi/extensions/oboete.js", piDir, "extensions", "oboete.js");

if (bad.length > 0) {
  console.error(bad.join("\n"));
  process.exit(1);
}
console.log(`every owned entry names ${want}`);
