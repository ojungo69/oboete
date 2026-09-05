import { render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, describe, expect, it } from "vitest";
import { ObserverStatusBanner } from "./ObserverStatusBanner";

let mount: HTMLDivElement | null = null;

afterEach(() => {
	if (mount) {
		act(() => render(null, mount as HTMLDivElement));
		mount.remove();
		mount = null;
	}
});

describe("ObserverStatusBanner", () => {
	it("shows pending privacy state without claiming an active observer or missing token", () => {
		mount = document.createElement("div");
		document.body.appendChild(mount);
		act(() => {
			render(
				<ObserverStatusBanner
					status={{
						active: null,
						capability: {
							configurationFingerprint: "sha256:configured-observer",
							providerEnabled: false,
							runtimeReason: "pending_privacy_boundary",
							providerHealth: "available",
						},
					}}
				/>,
				mount as HTMLDivElement,
			);
		});

		expect(mount.textContent).toContain("Configured — waiting for privacy safeguards");
		expect(mount.textContent).not.toContain("Active observer");
		expect(mount.querySelector('[aria-label="token missing"]')).toBeNull();
	});

	it.each([
		[
			"provider TLS rejection",
			"provider_tls_rejected",
			"Configured — provider trust check failed; privacy safeguards are still pending",
		],
		[
			"provider unavailability",
			"provider_unavailable",
			"Configured — provider unavailable; privacy safeguards are still pending",
		],
	])("shows pending privacy state for %s", (_name, providerHealth, expected) => {
		mount = document.createElement("div");
		document.body.appendChild(mount);
		act(() => {
			render(
				<ObserverStatusBanner
					status={{
						active: null,
						capability: {
							configurationFingerprint: "sha256:configured-observer",
							providerEnabled: false,
							providerHealth,
						},
					}}
				/>,
				mount as HTMLDivElement,
			);
		});

		expect(mount.textContent).toContain(expected);
	});

	it("does not report capture-only mode as configured while its provider is disabled", () => {
		mount = document.createElement("div");
		document.body.appendChild(mount);
		act(() => {
			render(
				<ObserverStatusBanner
					status={{
						active: null,
						capability: {
							configurationFingerprint: null,
							providerEnabled: false,
							runtimeReason: "manifest_absent",
							providerHealth: "not_configured",
						},
					}}
				/>,
				mount as HTMLDivElement,
			);
		});

		expect(mount.textContent).toContain("Not yet initialized (waiting for first session)");
		expect(mount.textContent).not.toContain("Configured — waiting for privacy safeguards");
	});
});
