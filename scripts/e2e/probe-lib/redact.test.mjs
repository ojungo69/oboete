import assert from "node:assert/strict";
import { redactValue } from "./agents.mjs";

const id = "2026-09-03T12-35-03-000Z";
const forms = [
  `~/.cache/oboete-probes/${id}`,
  `~%2F.cache%2Foboete-probes%2F${id}`,
  `~-cache-oboete-probes-${id}`,
];
for (const form of forms) {
  assert.equal(redactValue(form, null), "<run>", form);
  assert.equal(redactValue(`pre ${form}/repo post`, null), "pre <run>/repo post", form + " wrapped");
}
assert.equal(redactValue(`~/.cache/oboete-probes/${id} and ~-cache-oboete-probes-${id}`, null), "<run> and <run>");
console.log("ok");
