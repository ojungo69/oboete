import { describe, expect, it } from "vitest";
import { collectSettingsPayload } from "./collect-payload";
import { EMPTY_FORM_STATE } from "./constants";
import { formStateFromPayload } from "./form-state";
import { mergeOverrideBaseline } from "./value-helpers";

describe("Observer settings helpers", () => {
	it("P1-T031-02-settings-api-only loads and saves API-only observer settings", () => {
		const values = formStateFromPayload({
			config: {
				observer_runtime: "codex_sidecar",
				codex_command: ["codex"],
				observer_reasoning_effort: "low",
				observer_reasoning_summary: "concise",
			},
			effective: {
				observer_reasoning_effort: "medium",
				observer_reasoning_summary: "auto",
			},
		});

		expect(values.observerReasoningEffort).toBe("medium");
		expect(values.observerReasoningSummary).toBe("auto");

		const payload = collectSettingsPayload({
			values: {
				...EMPTY_FORM_STATE,
				observerReasoningEffort: values.observerReasoningEffort,
				observerReasoningSummary: values.observerReasoningSummary,
			},
			touchedKeys: new Set(["observer_reasoning_effort", "observer_reasoning_summary"]),
			baseline: {},
		});

		expect(payload.observer_reasoning_effort).toBe("medium");
		expect(payload.observer_reasoning_summary).toBe("auto");
		expect(payload).not.toHaveProperty("codex_command");
		expect(payload).not.toHaveProperty("observer_runtime");

		expect(
			mergeOverrideBaseline(
				{
					observer_reasoning_effort: "medium",
					observer_reasoning_summary: "auto",
				},
				{
					observer_reasoning_effort: " medium ",
					observer_reasoning_summary: " auto ",
				},
				{
					observer_reasoning_effort: "CODEMEM_OBSERVER_REASONING_EFFORT",
					observer_reasoning_summary: "CODEMEM_OBSERVER_REASONING_SUMMARY",
				},
			),
		).toEqual({
			observer_reasoning_effort: "medium",
			observer_reasoning_summary: "auto",
		});
	});
});
