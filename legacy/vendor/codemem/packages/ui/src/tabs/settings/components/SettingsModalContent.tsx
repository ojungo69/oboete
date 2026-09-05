import * as Dialog from "@radix-ui/react-dialog";
import type { ComponentChildren } from "preact";
import { DialogCloseButton } from "../../../components/primitives/dialog-close-button";
import { RadixTabs, RadixTabsContent } from "../../../components/primitives/radix-tabs";
import { SETTINGS_TABS } from "../data/constants";
import type { SettingsPanelProps, SettingsRenderState } from "../data/types";
import { ObserverPanel } from "./ObserverPanel";
import { ProcessingPanel } from "./ProcessingPanel";
import { SettingsHint } from "./SettingsHint";
import { SettingsSwitchRow } from "./SettingsSwitchRow";

export interface SettingsModalContentProps {
	panelProps: SettingsPanelProps;
	activeTab: string;
	showAdvanced: boolean;
	renderState: SettingsRenderState;
	onClose: () => void;
	onActiveTabChange: (tab: string) => void;
	onAdvancedToggle: (checked: boolean) => void;
	observerStatusBannerSlot: ComponentChildren;
}

export function SettingsModalContent({
	panelProps,
	activeTab,
	showAdvanced,
	renderState,
	onClose,
	onActiveTabChange,
	onAdvancedToggle,
	observerStatusBannerSlot,
}: SettingsModalContentProps) {
	return (
		<div className="modal-card">
			<div className="modal-header">
				<Dialog.Title asChild>
					<h2>Settings</h2>
				</Dialog.Title>
				<DialogCloseButton
					ariaLabel="Close settings"
					className="modal-close-button"
					onClick={onClose}
				/>
			</div>
			<div className="modal-body">
				<div className="small" id="settingsDescription">
					View the effective configuration. Edit the local config file to make changes.
				</div>
				<div className="settings-advanced-toolbar">
					<SettingsSwitchRow
						checked={showAdvanced}
						id="settingsAdvancedToggle"
						label="Show advanced controls"
						onCheckedChange={onAdvancedToggle}
					/>
					<button
						aria-label="About advanced controls"
						className="help-icon"
						data-tooltip="Advanced controls include JSON fields, tuning values, and network overrides."
						type="button"
					>
						<i aria-hidden="true" data-lucide="help-circle" />
					</button>
				</div>
				<SettingsHint hidden={!showAdvanced}>
					Advanced controls are visible. Leave JSON fields, tuning values, and network overrides
					alone unless you are debugging or matching a known deployment setup.
				</SettingsHint>

				<RadixTabs
					ariaLabel="Settings sections"
					listClassName="settings-tabs"
					onValueChange={onActiveTabChange}
					tabs={SETTINGS_TABS}
					triggerClassName="settings-tab"
					value={activeTab}
				>
					<RadixTabsContent className="settings-panel" forceMount value="observer">
						<fieldset className="settings-panel-readonly" disabled>
							<ObserverPanel {...panelProps} observerStatusBannerSlot={observerStatusBannerSlot} />
						</fieldset>
					</RadixTabsContent>

					<RadixTabsContent className="settings-panel" forceMount value="queue">
						<fieldset className="settings-panel-readonly" disabled>
							<ProcessingPanel {...panelProps} />
						</fieldset>
					</RadixTabsContent>
				</RadixTabs>

				<div className="small mono" id="settingsPath">
					{renderState.pathText}
				</div>
				<div className="small" id="settingsEffective">
					{renderState.effectiveText}
				</div>
				<div
					className="settings-note"
					hidden={!renderState.overridesVisible}
					id="settingsOverrides"
				>
					Some values are controlled outside this screen and take priority.
				</div>
				<div className="settings-note" hidden={showAdvanced}>
					Advanced controls are hidden right now to keep this screen focused on everyday settings.
				</div>
			</div>
			<div className="modal-footer">
				<div className="small" id="settingsStatus">
					{renderState.statusText}
				</div>
				<button className="settings-save" disabled id="settingsSave" type="button">
					Read-only
				</button>
			</div>
		</div>
	);
}
