import { h, render } from "preact";
import { act } from "preact/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HealthCard } from "./components";

vi.mock("../../components/primitives/tooltip", () => ({
	Tooltip: ({ children }: { children?: unknown }) => children,
	TooltipProvider: ({ children }: { children?: unknown }) => children,
}));

let mount: HTMLDivElement;

beforeEach(() => {
	mount = document.createElement("div");
	document.body.appendChild(mount);
});

afterEach(() => {
	act(() => {
		render(null, mount);
	});
	mount.remove();
});

describe("HealthCard", () => {
	it("renders the bare stat card when no extra class is given", () => {
		act(() => {
			render(h(HealthCard, { label: "Daemon", value: "running" }), mount);
		});

		const card = mount.querySelector("div.stat");
		expect(card?.getAttribute("class")).toBe("stat");
		expect(card?.getAttribute("style")).toBeNull();
		expect(mount.querySelector(".value")?.textContent).toBe("running");
		expect(mount.querySelector(".label")?.textContent).toBe("Daemon");
		expect(mount.querySelector(".small")).toBeNull();
		expect(mount.querySelector(".stat-icon")).toBeNull();
	});

	it("appends the extra class and marks the card as hoverable when a title is given", () => {
		act(() => {
			render(
				h(HealthCard, {
					label: "Pack age",
					value: "2m",
					detail: "last pack",
					icon: "clock-3",
					className: "warn",
					title: "Recency of last memory pack activity",
				}),
				mount,
			);
		});

		const card = mount.querySelector("div.stat");
		expect(card?.getAttribute("class")).toBe("stat warn");
		expect(card?.getAttribute("style")).toBe("cursor: help;");
		expect(mount.querySelector(".small")?.textContent).toBe("last pack");
		expect(mount.querySelector(".stat-icon")?.getAttribute("data-lucide")).toBe("clock-3");
	});
});
