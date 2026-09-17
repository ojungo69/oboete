#!/usr/bin/env node
// `trustedHashToml` as a command: rewrite the hooks file in place and print its `[hooks.state]` rows.
// It lives apart from `trusthash.mjs` so that library can be imported from a bundled test without a
// main-module guard firing on the bundle's own path.
import fs from "node:fs";

import { trustedHashToml } from "./trusthash.mjs";

const p = process.argv[2];
const file = JSON.parse(fs.readFileSync(p, "utf8"));
const toml = trustedHashToml(p, file);
fs.writeFileSync(p, JSON.stringify(file, null, 2));
process.stdout.write(toml);
