#!/usr/bin/env node
// Codex [hooks.state] trusted_hash rows. Corrected rule (recon 2026-09-03):
//   async:false is part of the handler object (not omitted as None)
//   the group's matcher is part of the preimage when present
//   timeout is the normalized default (600, or 1 for session_end/interrupt)
import fs from "node:fs";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";

const snake = (e) => e.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();
const canon = (v) =>
  Array.isArray(v)
    ? "[" + v.map(canon).join(",") + "]"
    : v && typeof v === "object"
      ? "{" + Object.keys(v).sort().map((k) => JSON.stringify(k) + ":" + canon(v[k])).join(",") + "}"
      : JSON.stringify(v);

export function trustedHashToml(hooksPath, file) {
  const rows = [];
  for (const [event, groups] of Object.entries(file.hooks || {})) {
    const ev = snake(event);
    const defTimeout = ev === "session_end" || ev === "interrupt" ? 1 : 600;
    groups.forEach((g, gi) => {
      g.hooks.forEach((h, hi) => {
        delete h.timeout;
        const norm = { async: false, command: h.command, timeout: defTimeout, type: h.type };
        const group = { event_name: ev, hooks: [norm] };
        if (g.matcher !== undefined && g.matcher !== null) group.matcher = g.matcher;
        const hash = crypto.createHash("sha256").update(canon(group)).digest("hex");
        rows.push(`[hooks.state."${hooksPath}:${ev}:${gi}:${hi}"]\ntrusted_hash = "sha256:${hash}"`);
      });
    });
  }
  return rows.join("\n\n") + (rows.length ? "\n" : "");
}

const self = fileURLToPath(import.meta.url);
if (process.argv[1] && fs.existsSync(process.argv[1]) && fs.realpathSync(process.argv[1]) === self) {
  const p = process.argv[2];
  const file = JSON.parse(fs.readFileSync(p, "utf8"));
  const toml = trustedHashToml(p, file);
  fs.writeFileSync(p, JSON.stringify(file, null, 2));
  process.stdout.write(toml);
}
