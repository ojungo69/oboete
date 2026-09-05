import Database from "better-sqlite3";
import { describe, expect, it } from "vitest";
import { compileDefaultCapabilityManifest } from "../capability-manifest.js";
import { compileProviderDestinationBoundary } from "../destination-boundary.js";
import { initTestSchema } from "../test-utils.js";
import { aiBackfillStructuredContent } from "./ai-structured.js";

const summaryProvider = {
	version: 1,
	role: "summary",
	state: "enabled",
	wireProtocol: "openai_chat_completions_v1",
	modelId: "deterministic-summary-model-v1",
	modelRevision: "1",
	endpointUrl: "https://summary.stub.invalid/v1/chat/completions",
	credentialRef: { kind: "environment", name: "FREE_MEM_SUMMARY_API_KEY" },
	providerFingerprint: "sha256:d184deae938722877e017d85ab382a4f72c287857bf0f346f483263680635ede",
	executionLocation: "remote",
	egressPolicy: "explicit_remote",
	costClass: "external_metered",
	tlsPolicy: "system",
	redirectPolicy: "reject",
} as const;

const observerProfile = {
	observerRequestTimeoutMs: 60_000,
	observerMaxInputChars: 12_000,
	observerMaxOutputTokens: 4_000,
	observerMaxResponseBytes: 1_048_576,
	observerTemperature: 0.2,
} as const;

type FrozenBackfillOptions = NonNullable<Parameters<typeof aiBackfillStructuredContent>[1]> & {
	summaryProvider: typeof summaryProvider;
	resourceProfile: typeof observerProfile;
	runtimeReason: "pending_privacy_boundary";
};

const REPOSITORY_A = `repo-v1:sha256:${"a".repeat(64)}`;
const REPOSITORY_B = `repo-v1:sha256:${"b".repeat(64)}`;

function manifest(endpointUrl: string) {
	return compileDefaultCapabilityManifest({
		version: 1,
		role: "summary",
		state: "enabled",
		wireProtocol: "openai_chat_completions_v1",
		modelId: "structured-maintenance-test",
		modelRevision: "1",
		endpointUrl,
		credentialRef: { kind: "none" },
	});
}

function addMemory(
	db: Database.Database,
	sessionId: number,
	title: string,
	sensitivity: "eligible" | "local_only" | "private" | "secret",
	repositoryIdentity: string | null,
): void {
	db.prepare(
		`INSERT INTO memory_items(
			session_id, kind, title, body_text, confidence, tags_text, active,
			created_at, updated_at, metadata_json, rev, visibility, sensitivity,
			repository_identity
		 ) VALUES (?, 'change', ?, ?, 0.8, '', 1, ?, ?, '{}', 1, 'shared', ?, ?)`,
	).run(
		sessionId,
		title,
		`${title} body`,
		"2026-08-31T00:00:00Z",
		"2026-08-31T00:00:00Z",
		sensitivity,
		repositoryIdentity,
	);
}

describe("AI structured maintenance frozen provider", () => {
	it.each([
		{
			name: "remote projects eligible rows only",
			manifest: manifest("https://summary.stub.invalid/v1/chat/completions"),
			repositoryIdentity: REPOSITORY_A,
			rows: [
				["REMOTE_ELIGIBLE", "eligible", REPOSITORY_A],
				["FORBIDDEN_REMOTE_PRIVATE", "private", REPOSITORY_A],
			],
			expectedTitles: ["REMOTE_ELIGIBLE"],
		},
		{
			name: "verified local HTTPS projects restricted rows only from the exact repository",
			manifest: manifest("https://127.0.0.1:1234/v1/chat/completions"),
			repositoryIdentity: REPOSITORY_A,
			rows: [
				["LOCAL_ELIGIBLE", "eligible", REPOSITORY_A],
				["LOCAL_ONLY", "local_only", REPOSITORY_A],
				["LOCAL_PRIVATE", "private", REPOSITORY_A],
				["FORBIDDEN_LOCAL_SECRET", "secret", REPOSITORY_A],
				["FORBIDDEN_CROSS_REPOSITORY", "private", REPOSITORY_B],
				["FORBIDDEN_UNKNOWN_REPOSITORY", "private", null],
			],
			expectedTitles: ["LOCAL_ELIGIBLE", "LOCAL_ONLY", "LOCAL_PRIVATE"],
		},
		{
			name: "rejects mixed restricted rows when the local repository is unknown",
			manifest: manifest("https://127.0.0.1:1234/v1/chat/completions"),
			repositoryIdentity: null,
			rows: [
				// The eligible row proves the boundary excludes only the restricted
				// rows here, not everything, when the repository is unknown.
				["ELIGIBLE_UNKNOWN_REPOSITORY", "eligible", null],
				["FORBIDDEN_MIXED_A", "private", REPOSITORY_A],
				["FORBIDDEN_MIXED_B", "local_only", REPOSITORY_B],
				["FORBIDDEN_MIXED_UNKNOWN", "private", null],
			],
			expectedTitles: ["ELIGIBLE_UNKNOWN_REPOSITORY"],
		},
	] as const)("T032 $name before prompt construction", async (fixture) => {
		const db = new Database(":memory:");
		try {
			initTestSchema(db);
			const sessionId = Number(
				db
					.prepare("INSERT INTO sessions(started_at, project) VALUES (?, ?)")
					.run("2026-08-31T00:00:00Z", "forged-project-label").lastInsertRowid,
			);
			for (const [title, sensitivity, repositoryIdentity] of fixture.rows) {
				addMemory(db, sessionId, title, sensitivity, repositoryIdentity);
			}
			const prompts: string[] = [];
			const response = {
				raw: JSON.stringify({
					narrative: "The projected fixture changed safely.",
					facts: ["The projected fixture changed."],
					concepts: ["what-changed"],
				}),
				parsed: null,
				provider: "openai",
				model: "structured-maintenance-test",
				usage: null,
				usedStructuredOutputs: false,
			};
			const observer = {
				getStatus: () => ({ provider: "openai", model: "structured-maintenance-test" }),
				observe: async (_system: string, user: string) => {
					prompts.push(user);
					return response;
				},
				observeStructuredJson: async (_system: string, user: string) => {
					prompts.push(user);
					return response;
				},
			};
			const boundary = compileProviderDestinationBoundary(fixture.manifest, {
				repositoryIdentity: fixture.repositoryIdentity,
				tlsPeerVerified: true,
			});

			await aiBackfillStructuredContent(db, {
				dryRun: true,
				runtimeReason: "ready",
				summaryProvider: fixture.manifest.summaryProvider,
				resourceProfile: fixture.manifest.resourceProfile,
				observer,
				destinationBoundary: boundary,
			});

			expect(
				prompts.map((prompt) => fixture.rows.find(([title]) => prompt.includes(title))?.[0]),
			).toEqual(fixture.expectedTitles);
			const serialized = JSON.stringify(prompts);
			for (const [title] of fixture.rows) {
				if (!fixture.expectedTitles.includes(title)) expect(serialized).not.toContain(title);
			}
		} finally {
			db.close();
		}
	});

	it("keeps frozen maintenance pending without reading legacy provider environment", async () => {
		const db = new Database(":memory:");
		const originalFetch = globalThis.fetch;
		const environment = {
			FREE_MEM_SUMMARY_API_KEY: process.env.FREE_MEM_SUMMARY_API_KEY,
			CODEMEM_OBSERVER_API_KEY: process.env.CODEMEM_OBSERVER_API_KEY,
			CODEMEM_OBSERVER_BASE_URL: process.env.CODEMEM_OBSERVER_BASE_URL,
			CODEMEM_OBSERVER_HEADERS: process.env.CODEMEM_OBSERVER_HEADERS,
			OPENAI_API_KEY: process.env.OPENAI_API_KEY,
		};
		const namedToken = "fixture-maintenance-named-token";
		let request: { url: string; headers: Record<string, string> } | undefined;
		let fetchCalls = 0;

		try {
			initTestSchema(db);
			const sessionId = Number(
				db
					.prepare("INSERT INTO sessions(started_at, project) VALUES (?, ?)")
					.run("2026-08-31T00:00:00Z", "fixture-project").lastInsertRowid,
			);
			db.prepare(
				`INSERT INTO memory_items(
					session_id, kind, title, body_text, confidence, tags_text, active,
					created_at, updated_at, metadata_json, rev, visibility
				 ) VALUES (?, 'change', 'Fixture change', 'The fixture changed safely.', 0.8, '', 1,
					?, ?, '{}', 1, 'shared')`,
			).run(sessionId, "2026-08-31T00:00:00Z", "2026-08-31T00:00:00Z");

			process.env.FREE_MEM_SUMMARY_API_KEY = namedToken;
			process.env.CODEMEM_OBSERVER_API_KEY = "fixture-legacy-codemem-token";
			process.env.CODEMEM_OBSERVER_BASE_URL = "https://legacy.invalid/v1";
			process.env.CODEMEM_OBSERVER_HEADERS = JSON.stringify({ "x-legacy": "must-not-send" });
			process.env.OPENAI_API_KEY = "fixture-legacy-openai-token";
			globalThis.fetch = (async (input: string | URL | Request, init: RequestInit = {}) => {
				fetchCalls += 1;
				request = {
					url: String(input),
					headers: Object.fromEntries(
						Object.entries((init.headers as Record<string, string>) ?? {}),
					),
				};
				const structured = JSON.stringify({
					narrative: "The fixture changed safely.",
					facts: ["The fixture changed"],
					concepts: ["what-changed"],
				});
				return new Response(
					JSON.stringify({
						output_text: structured,
						output: [{ type: "message", content: [{ type: "output_text", text: structured }] }],
						choices: [{ message: { content: structured } }],
					}),
					{ status: 200, headers: { "content-type": "application/json" } },
				);
			}) as typeof globalThis.fetch;

			const result = await aiBackfillStructuredContent(db, {
				dryRun: true,
				summaryProvider,
				resourceProfile: observerProfile,
				runtimeReason: "pending_privacy_boundary",
			} as FrozenBackfillOptions);

			expect(result).toMatchObject({ checked: 0, updated: 0, skipped: 1, failed: 0 });
			expect(fetchCalls).toBe(0);
			expect(request).toBeUndefined();
		} finally {
			globalThis.fetch = originalFetch;
			for (const [key, value] of Object.entries(environment)) {
				if (value === undefined) delete process.env[key];
				else process.env[key] = value;
			}
			db.close();
		}
	});
});
