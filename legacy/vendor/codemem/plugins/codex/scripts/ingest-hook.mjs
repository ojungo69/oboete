import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import process from "node:process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const bundledRuntime = join(scriptDir, "hook-runtime.mjs");
if (existsSync(bundledRuntime)) {
  spawnSync(process.execPath, [bundledRuntime, "codex-hook-ingest"], {
    stdio: "inherit",
    timeout: 4500
  });
  process.exit(0);
}
process.exit(0);
