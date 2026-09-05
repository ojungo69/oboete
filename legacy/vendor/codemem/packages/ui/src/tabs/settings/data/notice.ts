/* Build the user-facing notice (message + severity) from a settings-save
 * response payload. Interprets the `effects` block — hot-reloaded keys,
 * restart requirements, and warnings — into one joined status line. */

import { formatSettingsKey, joinPhrases } from "./format";

interface SettingsSaveEffects {
	hot_reloaded_keys?: unknown;
	restart_required_keys?: unknown;
	warnings?: unknown;
}

export function buildSettingsNotice(payload: unknown): {
	message: string;
	type: "success" | "warning";
} {
	const raw = (payload as { effects?: SettingsSaveEffects } | null | undefined)?.effects;
	const effects: SettingsSaveEffects =
		raw && typeof raw === "object" && !Array.isArray(raw) ? raw : {};
	const hotReloaded = Array.isArray(effects.hot_reloaded_keys)
		? effects.hot_reloaded_keys.map((key) => formatSettingsKey(String(key)))
		: [];
	const restartRequired = Array.isArray(effects.restart_required_keys)
		? effects.restart_required_keys.map((key) => formatSettingsKey(String(key)))
		: [];
	const warnings = Array.isArray(effects.warnings)
		? effects.warnings.filter(
				(value): value is string => typeof value === "string" && value.trim().length > 0,
			)
		: [];
	const lines: string[] = [];

	if (hotReloaded.length) {
		lines.push(`Applied now: ${joinPhrases(hotReloaded)}.`);
	}
	if (restartRequired.length) {
		lines.push(`Restart required for ${joinPhrases(restartRequired)}. Run: codemem serve restart`);
	}
	warnings.forEach((warning) => {
		lines.push(warning);
	});
	if (!lines.length) {
		lines.push("Saved.");
	}

	const hasWarning = restartRequired.length > 0 || warnings.length > 0;
	return { message: lines.join(" "), type: hasWarning ? "warning" : "success" };
}
