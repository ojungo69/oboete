import fs from "node:fs";
import path from "node:path";
import { redactValue } from "./agents.mjs";

function sessionJsonlPaths(tree) {
  const dir = path.join(tree, "sessions");
  if (!fs.existsSync(dir)) return [];
  return fs.readdirSync(dir).filter((f) => f.endsWith(".jsonl")).map((f) => path.join(dir, f));
}

function entryPath(root, e) {
  return path.join(e.parentPath ?? e.path ?? root, e.name);
}

export function hasNodeModules(p) {
  return String(p).split(path.sep).includes("node_modules");
}

export function searchLines(text, re) {
  return (text || "")
    .split("\n")
    .filter((l) => re.test(l))
    .slice(0, 20);
}

export function stdoutErrorTypes(stdout) {
  const stdoutTypes = [];
  for (const line of (stdout || "").split("\n")) {
    try {
      const o = JSON.parse(line);
      if (o && /error/i.test(String(o.type || ""))) stdoutTypes.push(o.type);
    } catch {
      /* skip */
    }
  }
  return stdoutTypes;
}

export function sessionErrorRecords(tree, throwRe) {
  const sessionTypes = [];
  const durable = [];
  for (const f of sessionJsonlPaths(tree)) {
    const body = fs.readFileSync(f, "utf8");
    for (const line of body.split("\n").filter(Boolean)) {
      try {
        sessionTypes.push(JSON.parse(line).type);
      } catch {
        /* skip */
      }
    }
    const hits = searchLines(body, throwRe);
    if (hits.length) durable.push({ path: f, hits });
  }
  return { sessionTypes, durable };
}

export function findErrorLogs(root, skipLog, throwRe) {
  if (!fs.existsSync(root)) return [];
  let ents;
  try {
    ents = fs.readdirSync(root, { recursive: true, withFileTypes: true });
  } catch {
    return [];
  }
  const out = [];
  for (const e of ents) {
    if (!e.isFile()) continue;
    const fp = entryPath(root, e);
    if (hasNodeModules(fp)) continue;
    if (!/\.(log|txt|jsonl)$/i.test(e.name) || skipLog.has(e.name)) continue;
    const body = fs.readFileSync(fp, "utf8");
    if (throwRe.test(body)) out.push({ path: fp, hits: searchLines(body, throwRe) });
  }
  return out;
}

export function findRealErrorLogs(real, afterReal, throwRe) {
  const realLogs = [];
  for (const e of afterReal.entries || []) {
    if (e.dir || !/\.(log|txt)$/i.test(e.path)) continue;
    const p = path.join(real, e.path);
    try {
      if (fs.existsSync(p) && throwRe.test(fs.readFileSync(p, "utf8"))) realLogs.push(p);
    } catch {
      /* unreadable */
    }
  }
  return realLogs;
}

export function durableErrorPath(durable, tmpLogs, realLogs) {
  let durableNamed;
  if (durable[0]) {
    durableNamed = durable[0].path + " :: " + durable[0].hits[0];
  } else if (tmpLogs[0]) {
    durableNamed = tmpLogs[0].path + " :: " + tmpLogs[0].hits[0];
  } else {
    durableNamed = realLogs[0] || null;
  }
  return durableNamed;
}

export function piErrorEvidence(options) {
  const {
    r,
    continued,
    text,
    stderrHits,
    stdoutTypes,
    stdoutHits,
    sessionTypes,
    durable,
    tmpLogs,
    realDiff,
    realLogs,
    durableNamed,
    tmpDiff,
  } = options;
  return [
    `exit=${r.exitCode} elapsed_s=${(r.elapsedMs / 1000).toFixed(1)} continued=${continued} text=${JSON.stringify(text).slice(0, 200)}`,
    `stderr hits=${stderrHits.length ? stderrHits.slice(0, 5).join(" | ") : "none"}`,
    `stdout error types=${stdoutTypes.join(",") || "none"} stdout hits=${stdoutHits.length ? stdoutHits.slice(0, 3).join(" | ") : "none"}`,
    `session jsonl types=[${sessionTypes.join(",")}] throw records=${durable.length ? JSON.stringify(redactValue(durable, r.repo)).slice(0, 500) : "none"}`,
    `piagent logs with throw=${tmpLogs.length ? JSON.stringify(redactValue(tmpLogs, r.repo)).slice(0, 400) : "none"}`,
    `~/.pi/agent added=${realDiff.added.join(",") || "none"} changed=${realDiff.changed.join(",") || "none"} throw logs=${realLogs.join(",") || "none"}`,
    `durable=${durableNamed || "no durable record"} (stderr/in-memory only unless a path is named)`,
    `tmp tree files=${(tmpDiff.entries || []).map((e) => e.path).slice(0, 40).join(",")}`,
  ];
}
