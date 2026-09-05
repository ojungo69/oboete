import { generateSQLiteDrizzleJson, generateSQLiteMigration } from "drizzle-kit/api";
import { describe, expect, it } from "vitest";
import { schema as drizzleSchema } from "./schema.js";
import { TEST_SCHEMA_BASE_DDL } from "./test-schema.generated.js";

function makeIdempotentStatements(statements: string[]): string[] {
	return statements.map((statement) =>
		statement
			.replace(/^CREATE TABLE /, "CREATE TABLE IF NOT EXISTS ")
			.replace(/^CREATE UNIQUE INDEX /, "CREATE UNIQUE INDEX IF NOT EXISTS ")
			.replace(/^CREATE INDEX /, "CREATE INDEX IF NOT EXISTS "),
	);
}

describe("test schema generation", () => {
	it("matches the current Drizzle schema snapshot", async () => {
		const prev = await generateSQLiteDrizzleJson({});
		const cur = await generateSQLiteDrizzleJson(drizzleSchema);
		const statements = await generateSQLiteMigration(prev, cur);

		expect(TEST_SCHEMA_BASE_DDL).toBe(makeIdempotentStatements(statements).join("\n"));
	});

	it("includes the Slice 1 v21 persistence declarations", () => {
		for (const fragment of [
			"`sensitivity`",
			"`repository_identity`",
			"`payload_digest_version`",
			"`payload_digest`",
			"`admission_manifest_fingerprint`",
			"`frontier_already_advanced`",
			"`raw_event_identity_conflicts`",
			"`raw_event_quarantine`",
			"`processing_resume_producer_receipts`",
			"`processing_resume_signals`",
			"`provider_health_states`",
		]) {
			expect(TEST_SCHEMA_BASE_DDL).toContain(fragment);
		}
	});
});
