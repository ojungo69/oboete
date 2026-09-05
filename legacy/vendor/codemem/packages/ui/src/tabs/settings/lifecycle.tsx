/* Settings modal lifecycle — mounts the Preact dialog shell, wires
 * the public open/close/save/init API, and binds the SettingsDialogShell
 * to SettingsDialogContent + the settingsState-driven handlers. */

import { render } from "preact";
import { $, $button } from "../../lib/dom";
import { state } from "../../lib/state";
import type { ObserverStatusShape } from "./components/ObserverStatusBanner";
import { ObserverStatusBanner as ObserverStatusBannerComponent } from "./components/ObserverStatusBanner";
import { SettingsDialogShell } from "./components/SettingsDialogShell";
import { SettingsModalContent } from "./components/SettingsModalContent";
import { createSettingsEventHandlers } from "./data/event-handlers";
import {
	getObserverModelDescription as getObserverModelDescriptionRaw,
	getObserverModelHint as getObserverModelHintRaw,
	getObserverModelLabel as getObserverModelLabelRaw,
	getObserverModelTooltip as getObserverModelTooltipRaw,
	getTieredRoutingHelperText as getTieredRoutingHelperTextRaw,
	hiddenUnlessAdvanced as hiddenUnlessAdvancedRaw,
	protectedConfigHelp,
} from "./data/model-accessors";
import { settingsState } from "./data/state";
import {
	hideHelpTooltip,
	onAdvancedToggle,
	setDirty,
	setSettingsTab,
	updateFormState,
} from "./data/state-ops";
import type { SettingsPanelProps } from "./data/types";

const getObserverModelHint = (): string =>
	getObserverModelHintRaw(settingsState.renderState.values, settingsState.envOverrides);
const getTieredRoutingHelperText = (): string =>
	getTieredRoutingHelperTextRaw(settingsState.renderState.values);
const getObserverModelLabel = (): string =>
	getObserverModelLabelRaw(settingsState.renderState.values);
const getObserverModelTooltip = (): string =>
	getObserverModelTooltipRaw(settingsState.renderState.values);
const getObserverModelDescription = (): string =>
	getObserverModelDescriptionRaw(settingsState.renderState.values);
const hiddenUnlessAdvanced = (): boolean => hiddenUnlessAdvancedRaw(settingsState.showAdvanced);

const { onTextInput, onSelectValueChange, onSwitchInput } = createSettingsEventHandlers({
	getTouchedKeys: () => settingsState.touchedKeys,
	getValues: () => settingsState.renderState.values,
	updateFormState,
	setDirty: (dirty) => setDirty(dirty),
});

function ObserverStatusBanner() {
	const status = settingsState.renderState.observerStatus as ObserverStatusShape | null;
	return <ObserverStatusBannerComponent status={status} />;
}

function SettingsDialogContent() {
	const values = settingsState.renderState.values;
	const observerMaxCharsDefault = String(state.configDefaults?.observer_max_chars || "");
	const showAuthFile = values.observerAuthSource === "file";
	const showTieredRouting = values.observerTierRoutingEnabled;
	const providerOptions = Array.from(
		new Set(
			settingsState.renderState.providers.concat(
				values.observerProvider ? [values.observerProvider] : [],
			),
		),
	)
		.sort((left, right) => left.localeCompare(right))
		.map((provider) => ({ label: provider, value: provider }));

	const panelProps: SettingsPanelProps = {
		values,
		observerMaxCharsDefault,
		providerOptions,
		showAuthFile,
		showTieredRouting,
		hiddenUnlessAdvanced,
		onTextInput,
		onSelectValueChange,
		onSwitchInput,
		getObserverModelLabel,
		getObserverModelTooltip,
		getObserverModelDescription,
		getObserverModelHint,
		getTieredRoutingHelperText,
		protectedConfigHelp,
	};

	return (
		<SettingsModalContent
			panelProps={panelProps}
			activeTab={settingsState.activeTab}
			showAdvanced={settingsState.showAdvanced}
			renderState={settingsState.renderState}
			onClose={() => {
				if (settingsState.startPolling && settingsState.refresh) {
					closeSettings(settingsState.startPolling, settingsState.refresh);
				}
			}}
			onActiveTabChange={setSettingsTab}
			onAdvancedToggle={onAdvancedToggle}
			observerStatusBannerSlot={<ObserverStatusBanner />}
		/>
	);
}

function SettingsDialogShellBound() {
	return <SettingsDialogShell DialogContent={SettingsDialogContent} onClose={closeSettings} />;
}

function renderSettingsShell() {
	const mount = $("settingsDialogMount");
	if (!mount) return;
	render(<SettingsDialogShellBound />, mount);
}

function ensureSettingsShell() {
	const mount = $("settingsDialogMount");
	if (!mount) return;
	if (settingsState.shellMounted) return;
	renderSettingsShell();
	settingsState.shellMounted = true;
}

export function openSettings(stopPolling: () => void) {
	if (!settingsState.shellMounted) {
		ensureSettingsShell();
	}
	settingsState.open = true;
	settingsState.previouslyFocused = document.activeElement as HTMLElement | null;
	stopPolling();
	settingsState.controller?.setOpen(true);
}

export function closeSettings(startPolling: () => void, refreshCallback: () => void) {
	if (state.settingsDirty) {
		if (!globalThis.confirm("Discard unsaved changes?")) {
			settingsState.controller?.setOpen(true);
			return;
		}
	}
	settingsState.open = false;
	settingsState.controller?.setOpen(false);
	hideHelpTooltip();
	const restoreTarget =
		settingsState.previouslyFocused && typeof settingsState.previouslyFocused.focus === "function"
			? settingsState.previouslyFocused
			: $button("settingsButton");
	restoreTarget?.focus();
	settingsState.previouslyFocused = null;
	settingsState.touchedKeys = new Set<string>();
	startPolling();
	refreshCallback();
}

export function initSettings(
	stopPolling: () => void,
	startPolling: () => void,
	refreshCallback: () => void,
) {
	settingsState.startPolling = startPolling;
	settingsState.refresh = refreshCallback;
	ensureSettingsShell();

	const settingsButton = $button("settingsButton");
	settingsButton?.addEventListener("click", () => openSettings(stopPolling));
}
