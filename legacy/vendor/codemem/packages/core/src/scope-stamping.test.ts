import Database from "better-sqlite3";
import { describe, expect, it } from "vitest";
import { ensureMemoryScopeId } from "./scope-stamping.js";
import { initTestSchema } from "./test-utils.js";

describe("scope stamping", () => {
	it("trims an existing scope and stamps a whitespace-only scope", () => {
		const db = new Database(":memory:");
		try {
			initTestSchema(db);
			db.exec(`
				INSERT INTO sessions(id, started_at, cwd, project)
				VALUES (1, '2026-08-23T00:00:00Z', '/tmp/codemem', 'codemem');
				INSERT INTO memory_items(
					id, session_id, kind, title, body_text, active, created_at, updated_at, scope_id
				) VALUES
					(1, 1, 'discovery', 'Existing scope', 'body', 1, '2026-08-23T00:00:00Z', '2026-08-23T00:00:00Z', '  team-a  '),
					(2, 1, 'discovery', 'Missing scope', 'body', 1, '2026-08-23T00:00:00Z', '2026-08-23T00:00:00Z', '   ');
			`);

			expect(ensureMemoryScopeId(db, 1)).toBe("team-a");
			expect(ensureMemoryScopeId(db, 2)).toBe("local-default");
			expect(db.prepare("SELECT scope_id FROM memory_items WHERE id = 2").pluck().get()).toBe(
				"local-default",
			);
		} finally {
			db.close();
		}
	});
});
