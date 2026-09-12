import fs from "node:fs";
import path from "node:path";

import { redactValue } from "./agent-events.mjs";

export function buildReportRow({
  agent,
  status,
  protocolVersion = null,
  toolName = "unknown",
  frames = 0,
  reason = "",
}) {
  if (!["pass", "fail", "blocked"].includes(status)) throw new Error(`invalid status '${status}'`);
  return { agent, status, protocolVersion, toolName, frames, reason };
}

export function exitCodeFor(rows, stdioStatus = "pass") {
  if (stdioStatus === "fail") return 1;
  if ((rows || []).some((row) => row.status === "fail")) return 1;
  return 0;
}

function markdownCell(value) {
  return String(value ?? "").replaceAll(/[\\|]/g, (c) => `\\${c}`).replace(/\r?\n/g, " ");
}

export function markdownSection(report) {
  const passed = report.agents.filter((row) => row.status === "pass").length;
  const blocked = report.agents.filter((row) => row.status === "blocked").map((row) => row.agent);
  let markdown = `## ${report.started_at.slice(0, 10)} MCP clients run ${report.runId}\n\n`;
  markdown += `- ${passed} of ${report.agents.length} agents pass`;
  if (blocked.length) markdown += ` (blocked: ${blocked.join(", ")})`;
  markdown += `\n- Report: ${report.runDir}/report.json\n\n`;
  markdown += "| agent | status | protocolVersion | toolName | frames | reason |\n|---|---|---|---|---:|---|\n";
  for (const row of report.agents) {
    markdown += `| ${row.agent} | ${row.status} | ${markdownCell(row.protocolVersion ?? "n/a")} | ${markdownCell(row.toolName)} | ${row.frames} | ${markdownCell(row.reason || "none")} |\n`;
  }
  markdown += "\n";
  for (const line of report.assertions || []) markdown += `- ${line}\n`;
  markdown += "\n";
  return markdown;
}

export function writeDaily(report, cwd) {
  const destination = path.join(cwd, "docs", "evidence", "m1-dogfood.md");
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  const handle = fs.openSync(destination, "a+");
  try {
    const heading =
      fs.fstatSync(handle).size > 0
        ? ""
        : "# oboete M1 dogfood evidence\n\nIsolated-user cross-agent runs for SC-001, SC-004, and SC-007.\n\n";
    fs.writeFileSync(handle, heading + markdownSection(report));
  } finally {
    fs.closeSync(handle);
  }
}

function frameFiles(runDir, agents) {
  const files = [`${runDir}/stdio/frames.jsonl`];
  for (const agent of agents) {
    if (agent === "pi") files.push(`${runDir}/pi/stdout.txt`);
    else files.push(`${runDir}/${agent}/frames.jsonl`);
  }
  return files;
}

export function reportMarkdown(report) {
  let markdown = markdownSection(report);
  markdown += "Frame files:\n";
  for (const file of report.frame_files || []) markdown += `- ${file}\n`;
  markdown += "\n";
  return markdown;
}

export function finishHarnessReport(options, dependencies, stdio, rows, runId, runDir, started) {
  const assertions = [...stdio.assertions];
  for (const row of rows) {
    assertions.push(`${row.agent}: ${row.status} (${row.reason})`);
  }
  const finished = new Date(dependencies.now());
  const report = redactValue(
    {
      runId,
      runDir,
      started_at: started.toISOString(),
      finished_at: finished.toISOString(),
      agents: rows,
      stdio: { status: stdio.status, frames: stdio.frames, reason: stdio.reason },
      assertions,
      frame_files: frameFiles(runDir, options.agents),
      summary: `${rows.filter((row) => row.status === "pass").length} of ${rows.length} agents pass`,
      exit_code: exitCodeFor(rows, stdio.status),
    },
    runDir,
    "<run>",
  );
  fs.writeFileSync(path.join(runDir, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
  fs.writeFileSync(path.join(runDir, "report.md"), reportMarkdown(report));
  if (options.daily) writeDaily(report, dependencies.cwd);
  return report;
}
