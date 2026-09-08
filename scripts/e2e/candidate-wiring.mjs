// Checks that every oboete entry `oboete setup` owns names the candidate bundle.
//
// Counting text matches is not enough: an entry can be deleted and its path left behind in a
// comment, and the total is unchanged. The hook and MCP entries live in JSON, so they are read as
// JSON and counted per file; the two config.toml lines and the Pi loader are read as text with
// commented-out lines excluded. Usage:
//
//   node scripts/e2e/candidate-wiring.mjs <bundle> <claude-home> <codex-home> <grok-home> <pi-dir>
import { readFileSync } from "node:fs";
const want = process.argv[2];
const homes = { claude: process.argv[3], codex: process.argv[4], grok: process.argv[5], pi: process.argv[6] };
const bad = [];
const paths = (value, out = []) => {
  if (typeof value === "string") { if (value.includes("oboete/dist/oboete.mjs")) out.push(value); }
  else if (value && typeof value === "object") for (const v of Object.values(value)) paths(v, out);
  return out;
};
const json = (file, expect) => {
  let found;
  try { found = paths(JSON.parse(readFileSync(file, "utf8"))); }
  catch (error) { bad.push(`${file}: ${error.message}`); return; }
  if (found.length !== expect) bad.push(`${file}: ${found.length} bundle references, expected ${expect}`);
  for (const value of found) for (const hit of value.match(/[^\s'"]*oboete\/dist\/oboete\.mjs/g) ?? []) if (hit !== want) bad.push(`${file}: ${hit}`);
};
const line = (file, needle) => {
  const hit = readFileSync(file, "utf8").split("\n").filter((l) => l.includes(needle) && !/^\s*(#|\/\/)/.test(l));
  if (hit.length !== 1) bad.push(`${file}: ${hit.length} live lines naming the bundle, expected 1`);
  else for (const found of hit[0].match(/[^\s'"]*oboete\/dist\/oboete\.mjs/g) ?? []) if (found !== want) bad.push(`${file}: ${found}`);
};
json(`${homes.claude}/settings.json`, 8);
json(`${homes.claude}/.claude.json`, 1);
json(`${homes.codex}/hooks.json`, 7);
json(`${homes.grok}/hooks/oboete.json`, 9);
line(`${homes.codex}/config.toml`, "oboete/dist/oboete.mjs");
line(`${homes.grok}/config.toml`, "oboete/dist/oboete.mjs");
line(`${homes.pi}/extensions/oboete.js`, "oboete/dist/oboete.mjs");
if (bad.length) { console.error(bad.join("\n")); process.exit(1); }
console.log(`every owned entry names ${want}`);
