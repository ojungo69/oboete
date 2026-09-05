/**
 * Memory routes — observations, summaries, sessions, projects, pack, artifacts.
 */

import { inArray } from "drizzle-orm";
import { drizzle } from "drizzle-orm/better-sqlite3";
import { Hono } from "hono";
import { fromJson } from "../db.js";
import {
	CAPTURE_ONLY_DESTINATION_FINGERPRINT,
	compileUntrustedDestinationBoundary,
	type DestinationBoundaryV1,
	memoryDestinationBoundarySql,
} from "../destination-boundary.js";
import { buildFilterClausesWithContext } from "../filters.js";
import { parseStrictInteger } from "../integers.js";
import { schema } from "../schema.js";
import type { MemoryStore } from "../store.js";
import {
	canonicalMemoryKind,
	isSummaryLikeMemory as isCoreSummaryLikeMemory,
} from "../summary-memory.js";
import type { MemoryFilters } from "../types.js";

function queryInt(value: string | undefined, fallback: number): number {
	return parseStrictInteger(value) ?? fallback;
}

type StoreFactory = () => MemoryStore;

type OwnershipPredicate = (item: Record<string, unknown>) => boolean;

function serializeMemoryRow(
	ownedBySelf: OwnershipPredicate,
	row: Record<string, unknown>,
): Record<string, unknown> {
	const metadata = fromJson((row.metadata_json as string) ?? null);
	// Evaluate ownership against the raw row (top-level columns + metadata_json
	// fallback) before we overwrite metadata_json with the parsed object.
	const owned_by_self = ownedBySelf(row);
	return {
		...row,
		kind: canonicalMemoryKind((row.kind as string | null | undefined) ?? null, row.metadata_json),
		metadata_json: metadata,
		owned_by_self,
	};
}

/** Attach the safe display project to memory items. */
function attachSessionFields(store: MemoryStore, items: Record<string, unknown>[]): void {
	const sessionIds: number[] = [];
	const seen = new Set<number>();
	for (const item of items) {
		const value = item.session_id;
		if (value == null) continue;
		const sid = Number(value);
		if (Number.isNaN(sid) || seen.has(sid)) continue;
		seen.add(sid);
		sessionIds.push(sid);
	}
	if (sessionIds.length === 0) return;

	const d = drizzle(store.db, { schema });
	const rows = d
		.select({
			id: schema.sessions.id,
			project: schema.sessions.project,
		})
		.from(schema.sessions)
		.where(inArray(schema.sessions.id, sessionIds))
		.all();

	const bySession = new Map<number, string>();
	for (const row of rows) {
		const projectRaw = String(row.project ?? "").trim();
		const project = projectRaw ? projectBasename(projectRaw) : "";
		bySession.set(row.id, project);
	}

	for (const item of items) {
		const sid = Number(item.session_id);
		if (Number.isNaN(sid)) continue;
		const project = bySession.get(sid);
		if (project === undefined) continue;
		item.project ??= project;
	}
}

/**
 * Extract the basename of a project path.
 * Strips "fatal:" prefixed values.
 */
function projectBasename(raw: string): string {
	if (raw.toLowerCase().startsWith("fatal:")) return "";
	const parts = raw.replaceAll("\\", "/").split("/");
	return parts.at(-1) ?? raw;
}

function normalizeScope(raw: string | undefined): "mine" | "theirs" | undefined {
	const value = String(raw ?? "")
		.trim()
		.toLowerCase();
	if (value === "mine" || value === "theirs") return value;
	return undefined;
}

function buildViewerMemoryFilters(
	store: MemoryStore,
	destinationBoundary: DestinationBoundaryV1,
	filters?: MemoryFilters | null,
) {
	return buildFilterClausesWithContext(filters, store.ownershipFilterContext(destinationBoundary));
}

function countVisibleMemoryRows(
	store: MemoryStore,
	destinationBoundary: DestinationBoundaryV1,
	filters?: MemoryFilters | null,
): number {
	const filterResult = buildViewerMemoryFilters(store, destinationBoundary, filters);
	const clauses = ["memory_items.active = 1", ...filterResult.clauses];
	const from = filterResult.joinSessions
		? "memory_items JOIN sessions ON sessions.id = memory_items.session_id"
		: "memory_items";
	const row = store.db
		.prepare(`SELECT COUNT(*) AS total FROM ${from} WHERE ${clauses.join(" AND ")}`)
		.get(...filterResult.params) as Record<string, unknown> | undefined;
	return Number(row?.total ?? 0);
}

// Artifacts have no scope of their own: a session's artifacts are reachable
// only when the session's memories are all visible (scope authorization and
// destination boundary both hold for every active memory in the session).
function sessionAllowsArtifactAccess(
	store: MemoryStore,
	destinationBoundary: DestinationBoundaryV1,
	sessionId: number,
): boolean {
	const visibleCount = countVisibleMemoryRows(store, destinationBoundary, {
		session_id: sessionId,
	});
	if (visibleCount === 0) return false;
	const row = store.db
		.prepare(
			`SELECT COUNT(*) AS total FROM memory_items
			 WHERE session_id = ? AND active = 1`,
		)
		.get(sessionId) as Record<string, unknown> | undefined;
	return visibleCount === Number(row?.total ?? 0);
}

function summaryLikeSqlPredicate(): string {
	return `(
		LOWER(TRIM(COALESCE(memory_items.kind, ''))) = 'session_summary'
		OR (
			json_valid(COALESCE(memory_items.metadata_json, ''))
			AND (
				COALESCE(json_extract(memory_items.metadata_json, '$.is_summary') = 1, 0)
				OR LOWER(TRIM(COALESCE(json_extract(memory_items.metadata_json, '$.source'), ''))) = 'observer_summary'
			)
		)
	)`;
}

function countVisibleObservationRows(
	store: MemoryStore,
	destinationBoundary: DestinationBoundaryV1,
	filters?: MemoryFilters | null,
): number {
	const filterResult = buildViewerMemoryFilters(store, destinationBoundary, filters);
	const clauses = [
		"memory_items.active = 1",
		`NOT ${summaryLikeSqlPredicate()}`,
		...filterResult.clauses,
	];
	const from = filterResult.joinSessions
		? "memory_items JOIN sessions ON sessions.id = memory_items.session_id"
		: "memory_items";
	const row = store.db
		.prepare(`SELECT COUNT(*) AS total FROM ${from} WHERE ${clauses.join(" AND ")}`)
		.get(...filterResult.params) as Record<string, unknown> | undefined;
	return Number(row?.total ?? 0);
}

function countVisiblePromptRows(
	store: MemoryStore,
	destinationBoundary: DestinationBoundaryV1,
	project?: string | null,
): number {
	const destination = memoryDestinationBoundarySql(destinationBoundary, "user_prompts");
	// Ownership/scope authorization AND the sensitivity boundary both gate the
	// count: the EXISTS keeps prompts of sessions outside the viewer's visible
	// scopes out, the destination clause keeps restricted rows out.
	const filterResult = buildViewerMemoryFilters(store, destinationBoundary);
	const clauses = [
		"user_prompts.session_id IS NOT NULL",
		destination.clause,
		`EXISTS (
			SELECT 1 FROM memory_items
			WHERE memory_items.session_id = user_prompts.session_id
			  AND memory_items.active = 1
			  AND ${filterResult.clauses.join(" AND ")}
		)`,
	];
	const params: unknown[] = [...destination.params, ...filterResult.params];
	if (project) {
		clauses.unshift("user_prompts.project = ?");
		params.unshift(project);
	}
	const row = store.db
		.prepare(`SELECT COUNT(*) AS total FROM user_prompts WHERE ${clauses.join(" AND ")}`)
		.get(...params) as Record<string, unknown> | undefined;
	return Number(row?.total ?? 0);
}

function countVisibleArtifactRows(
	store: MemoryStore,
	destinationBoundary: DestinationBoundaryV1,
	project?: string | null,
): number {
	const destination = memoryDestinationBoundarySql(destinationBoundary, "artifacts");
	const filterResult = buildViewerMemoryFilters(store, destinationBoundary);
	// Mirror sessionAllowsArtifactAccess: count an artifact only when its
	// session has at least one visible active memory AND no restricted active
	// memory — otherwise counts reveal artifacts the /api/artifacts route 404s.
	const clauses = [
		destination.clause,
		`EXISTS (
			SELECT 1 FROM memory_items
			WHERE memory_items.session_id = artifacts.session_id
			  AND memory_items.active = 1
			  AND ${filterResult.clauses.join(" AND ")}
		)`,
		`NOT EXISTS (
			SELECT 1 FROM memory_items
			WHERE memory_items.session_id = artifacts.session_id
			  AND memory_items.active = 1
			  AND NOT COALESCE((${filterResult.clauses.join(" AND ")}), 0)
		)`,
	];
	const params: unknown[] = [...destination.params, ...filterResult.params, ...filterResult.params];
	const from = project
		? "artifacts JOIN sessions ON sessions.id = artifacts.session_id"
		: "artifacts";
	if (project) {
		clauses.unshift("sessions.project = ?");
		params.unshift(project);
	}
	const row = store.db
		.prepare(`SELECT COUNT(*) AS total FROM ${from} WHERE ${clauses.join(" AND ")}`)
		.get(...params) as Record<string, unknown> | undefined;
	return Number(row?.total ?? 0);
}

function queryMemoryPage(
	store: MemoryStore,
	destinationBoundary: DestinationBoundaryV1,
	options: {
		limit: number;
		offset: number;
		project?: string;
		scope?: "mine" | "theirs";
	},
): Record<string, unknown>[] {
	const filters: MemoryFilters = {};
	if (options.project) filters.project = options.project;
	if (options.scope) filters.ownership_scope = options.scope;

	const filterResult = buildViewerMemoryFilters(store, destinationBoundary, filters);
	const clauses = ["memory_items.active = 1", ...filterResult.clauses];
	const where = clauses.join(" AND ");
	const from = filterResult.joinSessions
		? "memory_items JOIN sessions ON sessions.id = memory_items.session_id"
		: "memory_items";

	const rows = store.db
		.prepare(
			`SELECT memory_items.* FROM ${from}
			 WHERE ${where}
			 ORDER BY memory_items.created_at DESC
			 LIMIT ? OFFSET ?`,
		)
		.all(...filterResult.params, options.limit + 1, options.offset) as Record<string, unknown>[];

	const ownedBySelf = store.buildOwnershipPredicate();
	return rows.map((row) => serializeMemoryRow(ownedBySelf, row));
}

function isSummaryLikeMemory(item: Record<string, unknown>): boolean {
	return isCoreSummaryLikeMemory({
		kind: item.kind as string | null | undefined,
		metadata: item.metadata_json,
	});
}

function selectMemoryPage(
	store: MemoryStore,
	destinationBoundary: DestinationBoundaryV1,
	options: {
		limit: number;
		offset: number;
		project?: string;
		scope?: "mine" | "theirs";
		matcher: (item: Record<string, unknown>) => boolean;
	},
): Record<string, unknown>[] {
	const pageSize = Math.max(options.limit + options.offset + 10, 50);
	let rawOffset = 0;
	const matched: Record<string, unknown>[] = [];

	while (matched.length < options.offset + options.limit + 1) {
		const page = queryMemoryPage(store, destinationBoundary, {
			limit: pageSize,
			offset: rawOffset,
			project: options.project,
			scope: options.scope,
		});
		if (page.length === 0) break;
		matched.push(...page.filter(options.matcher));
		if (page.length < pageSize) break;
		rawOffset += page.length;
	}

	return matched.slice(options.offset, options.offset + options.limit + 1);
}

export function memoryRoutes(
	getStore: StoreFactory,
	destinationBoundary: DestinationBoundaryV1 = compileUntrustedDestinationBoundary({
		consumer: "viewer",
		configurationFingerprint: CAPTURE_ONLY_DESTINATION_FINGERPRINT,
	}),
) {
	const app = new Hono();

	// GET /api/sessions
	app.get("/api/sessions", (c) => {
		const store = getStore();
		{
			const limit = queryInt(c.req.query("limit"), 20);
			const filterResult = buildViewerMemoryFilters(store, destinationBoundary, null);
			const clauses = [
				"memory_items.session_id = sessions.id",
				"memory_items.active = 1",
				...filterResult.clauses,
			];
			const rows = store.db
				.prepare(
					`SELECT sessions.id, sessions.started_at, sessions.ended_at,
					        sessions.project, sessions.tool_version, sessions.import_key,
					        sessions.repository_identity
					 FROM sessions
					 WHERE EXISTS (SELECT 1 FROM memory_items WHERE ${clauses.join(" AND ")})
					 ORDER BY sessions.started_at DESC
					 LIMIT ?`,
				)
				.all(...filterResult.params, limit) as Record<string, unknown>[];
			return c.json({ items: rows });
		}
	});

	// GET /api/projects
	app.get("/api/projects", (c) => {
		const store = getStore();
		{
			const filterResult = buildViewerMemoryFilters(store, destinationBoundary, null);
			const clauses = [
				"memory_items.session_id = sessions.id",
				"memory_items.active = 1",
				"sessions.project IS NOT NULL",
				...filterResult.clauses,
			];
			const rows = store.db
				.prepare(
					`SELECT DISTINCT sessions.project AS project FROM sessions
					 JOIN memory_items ON memory_items.session_id = sessions.id
					 WHERE ${clauses.join(" AND ")}`,
				)
				.all(...filterResult.params) as Record<string, unknown>[];
			const projects = [
				...new Set(
					rows
						.map((r) => String(r.project ?? "").trim())
						.filter((p) => p && !p.toLowerCase().startsWith("fatal:"))
						.map((p) => projectBasename(p))
						.filter(Boolean),
				),
			].sort();
			return c.json({ projects });
		}
	});

	// GET /api/observations (aliased from /api/memories)
	app.get("/api/memories", (c) => {
		const search = new URL(c.req.url).search;
		return c.redirect(`/api/observations${search}`, 301);
	});

	app.get("/api/observations", (c) => {
		const store = getStore();
		{
			const limit = Math.max(1, queryInt(c.req.query("limit"), 20));
			const offset = Math.max(0, queryInt(c.req.query("offset"), 0));
			const project = c.req.query("project") || undefined;
			const scope = normalizeScope(c.req.query("scope"));
			const items = selectMemoryPage(store, destinationBoundary, {
				limit,
				offset,
				project,
				scope,
				matcher: (item) => !isSummaryLikeMemory(item),
			});
			const hasMore = items.length > limit;
			const result = hasMore ? items.slice(0, limit) : items;
			const asRecords = result as unknown as Record<string, unknown>[];
			attachSessionFields(store, asRecords);
			return c.json({
				items: asRecords,
				pagination: {
					limit,
					offset,
					next_offset: hasMore ? offset + result.length : null,
					has_more: hasMore,
				},
			});
		}
	});

	// GET /api/summaries
	app.get("/api/summaries", (c) => {
		const store = getStore();
		{
			const limit = Math.max(1, queryInt(c.req.query("limit"), 50));
			const offset = Math.max(0, queryInt(c.req.query("offset"), 0));
			const project = c.req.query("project") || undefined;
			const scope = normalizeScope(c.req.query("scope"));
			const items = selectMemoryPage(store, destinationBoundary, {
				limit,
				offset,
				project,
				scope,
				matcher: (item) => isSummaryLikeMemory(item),
			});
			const hasMore = items.length > limit;
			const result = hasMore ? items.slice(0, limit) : items;
			const asRecords = result as unknown as Record<string, unknown>[];
			attachSessionFields(store, asRecords);
			return c.json({
				items: asRecords,
				pagination: {
					limit,
					offset,
					next_offset: hasMore ? offset + result.length : null,
					has_more: hasMore,
				},
			});
		}
	});

	// GET /api/session (aggregate counts)
	app.get("/api/session", (c) => {
		const store = getStore();
		{
			const project = c.req.query("project") || null;
			let prompts: number;
			let artifacts: number;
			let memories: number;
			let observations: number;
			if (project) {
				prompts = countVisiblePromptRows(store, destinationBoundary, project);
				artifacts = countVisibleArtifactRows(store, destinationBoundary, project);
				memories = countVisibleMemoryRows(store, destinationBoundary, { project });
				observations = countVisibleObservationRows(store, destinationBoundary, { project });
			} else {
				prompts = countVisiblePromptRows(store, destinationBoundary);
				artifacts = countVisibleArtifactRows(store, destinationBoundary);
				memories = countVisibleMemoryRows(store, destinationBoundary);
				observations = countVisibleObservationRows(store, destinationBoundary);
			}
			const total = prompts + artifacts + memories;
			return c.json({ total, memories, artifacts, prompts, observations });
		}
	});

	// GET /api/memory
	app.get("/api/memory", (c) => {
		const store = getStore();
		{
			const limit = queryInt(c.req.query("limit"), 20);
			const kind = c.req.query("kind") || undefined;
			const project = c.req.query("project") || undefined;
			const filters: MemoryFilters = {};
			if (kind) filters.kind = kind;
			if (project) filters.project = project;
			const items = store.recent(limit, filters, 0, destinationBoundary);
			const asRecords = items as unknown as Record<string, unknown>[];
			attachSessionFields(store, asRecords);
			return c.json({ items: asRecords });
		}
	});

	// GET /api/artifacts
	app.get("/api/artifacts", (c) => {
		const store = getStore();
		{
			const sessionIdStr = c.req.query("session_id");
			if (!sessionIdStr) {
				return c.json({ error: "session_id required" }, 400);
			}
			const sessionId = parseStrictInteger(sessionIdStr);
			if (sessionId == null) {
				return c.json({ error: "session_id must be int" }, 400);
			}
			if (!sessionAllowsArtifactAccess(store, destinationBoundary, sessionId)) {
				return c.json({ error: "session not found" }, 404);
			}
			const destination = memoryDestinationBoundarySql(destinationBoundary, "artifacts");
			const rows = store.db
				.prepare(
					`SELECT artifacts.* FROM artifacts
					 WHERE artifacts.session_id = ? AND ${destination.clause}`,
				)
				.all(sessionId, ...destination.params) as Record<string, unknown>[];
			return c.json({ items: rows });
		}
	});

	return app;
}
