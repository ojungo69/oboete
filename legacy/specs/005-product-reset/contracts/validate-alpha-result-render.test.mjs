import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { validateRenderEvidence } from "./alpha-result-render.mjs";

const contractDir = dirname(fileURLToPath(import.meta.url));
const fixtureRoot = join(contractDir, "../fixtures");
const fixture = JSON.parse(readFileSync(join(fixtureRoot,
  "slice1-bidirectional-en-v1.json"), "utf8"));
const result = JSON.parse(readFileSync(join(fixtureRoot,
  "alpha-result-v1.example.json"), "utf8"));
const scenario = fixture.scenarios.find((item) => item.scenarioId === result.scenarioId);
const duplicate = structuredClone(result.injectedItems[0]);
delete duplicate.selectionReason;
duplicate.reason = "duplicate_revision";
duplicate.fact = "Contradictory content under one stable revision.";
result.omittedItems.push(duplicate);

assert.throws(() => validateRenderEvidence(result, scenario, fixture, true),
  /duplicate revision omission has no retained active item/);

console.log("Alpha result render regression checks passed.");
