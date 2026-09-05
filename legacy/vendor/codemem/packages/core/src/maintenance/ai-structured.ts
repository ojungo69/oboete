/* AI-powered structured-content backfill for older memories.
 */

import type { ProviderChoiceV1, ProviderTransportProfileV1 } from "../capability-manifest.js";
import type { Database } from "../db.js";
import {
	type DestinationBoundaryV1,
	memoryDestinationBoundarySql,
} from "../destination-boundary.js";
import {
	completeMaintenanceJob,
	failMaintenanceJob,
	startMaintenanceJob,
	updateMaintenanceJob,
} from "../maintenance-jobs.js";
import { ObserverAuthError, ObserverClient } from "../observer-client.js";
import { SecretScanner } from "../secret-scanner.js";
import { isSummaryLikeMemory } from "../summary-memory.js";
import { isOneOf, isWhitespace, trimEndWhere } from "../text-trim.js";

const CLOSE_BRACKET = isOneOf("]");

/** `/\s*```$/` の置き換え。末尾のフェンスと、その直前の空白だけを落とす。 */
export function stripTrailingFence(value: string): string {
	return value.endsWith("```") ? trimEndWhere(value.slice(0, -3), isWhitespace) : value;
}

const AI_BACKFILL_KINDS = [
	"change",
	"discovery",
	"bugfix",
	"feature",
	"decision",
	"exploration",
	"refactor",
] as const;

const AI_BACKFILL_CONCEPTS = [
	"how-it-works",
	"why-it-exists",
	"what-changed",
	"problem-solution",
	"gotcha",
	"pattern",
	"trade-off",
] as const;
const AI_BACKFILL_CONCEPT_SET = new Set<string>(AI_BACKFILL_CONCEPTS);

const AI_BACKFILL_JOB_KIND = "ai_structured_backfill";
const AI_BACKFILL_SCHEMA_NAME = "codemem_structured_memory_backfill";
const AI_BACKFILL_SCHEMA: Record<string, unknown> = {
	type: "object",
	additionalProperties: false,
	properties: {
		narrative: { type: ["string", "null"] },
		facts: { type: "array", items: { type: "string" } },
		concepts: { type: "array", items: { type: "string", enum: [...AI_BACKFILL_CONCEPTS] } },
	},
	required: ["narrative", "facts", "concepts"],
};

type StructuredBackfillObserver = Pick<
	ObserverClient,
	"observe" | "observeStructuredJson" | "getStatus"
>;

export interface AIBackfillStructuredContentResult {
	checked: number;
	updated: number;
	skipped: number;
	failed: number;
	samples?: Array<{
		id: number;
		kind: string;
		title: string;
		narrative: string | null;
		facts: string[];
		concepts: string[];
	}>;
}

export interface AIBackfillStructuredContentOptions {
	limit?: number | null;
	kinds?: string[] | null;
	dryRun?: boolean;
	overwrite?: boolean;
	observer?: StructuredBackfillObserver;
	summaryProvider?: ProviderChoiceV1;
	resourceProfile?: ProviderTransportProfileV1;
	destinationBoundary?: DestinationBoundaryV1;
	runtimeReason?:
		| "ready"
		| "pending_privacy_boundary"
		| "provider_unavailable"
		| "provider_tls_rejected";
	/**
	 * Secret scanner used to redact AI-generated narrative/facts/concepts
	 * before they are written back to memory_items. The summarizer can launder
	 * a secret past pattern-based detection on the source or invent new
	 * secret-shaped strings, so output must be re-scanned independent of input.
	 */
	scanner?: SecretScanner;
}

interface ParsedStructuredBackfill {
	narrative: string | null;
	facts: string[];
	concepts: string[];
}

type StructuredBackfillRow = {
	id: number;
	kind: string;
	title: string;
	body_text: string;
	metadata_json: string | null;
	narrative: string | null;
	facts: string | null;
	concepts: string | null;
};

function createStructuredBackfillObserver(
	provider: ProviderChoiceV1 | undefined,
	profile: ProviderTransportProfileV1 | undefined,
	destinationBoundary: DestinationBoundaryV1,
): StructuredBackfillObserver {
	if (!provider || !profile) throw new Error("Structured maintenance requires a frozen provider.");
	return new ObserverClient(provider, profile, { destinationBoundary });
}

function parseJsonArrayOfStrings(value: string | null): string[] {
	if (!value) return [];
	try {
		const parsed = JSON.parse(value) as unknown;
		if (Array.isArray(parsed)) {
			return parsed.filter((item): item is string => typeof item === "string");
		}
	} catch {
		return [];
	}
	return [];
}

function hasCompleteStructuredContent(row: {
	narrative: string | null;
	facts: string | null;
	concepts: string | null;
}): boolean {
	return (
		!!row.narrative?.trim() &&
		parseJsonArrayOfStrings(row.facts).length > 0 &&
		parseJsonArrayOfStrings(row.concepts).length > 0
	);
}

function buildStructuredBackfillPrompt(row: {
	id: number;
	kind: string;
	title: string;
	body_text: string;
}): { system: string; user: string } {
	const system = `You are converting older codemem memories into structured fields.

<output_contract>
- Output only valid JSON with exactly this shape:
  {"narrative": string|null, "facts": string[], "concepts": string[]}
- Do not add markdown fences or prose.
- Use null / [] when evidence is missing.
</output_contract>

<field_rules>
- narrative: 2-6 complete sentences, or 1-2 short paragraphs made of complete sentences.
- narrative must end cleanly on a full sentence. Do not output a truncated clause.
- facts: 2-8 source-grounded, self-contained statements. Prefer concrete details over generic purpose statements.
- concepts: 2-5 values from this exact list only:
  ["how-it-works", "why-it-exists", "what-changed", "problem-solution", "gotcha", "pattern", "trade-off"]
</field_rules>

<grounding_rules>
- Use ONLY the evidence in the provided title, kind, and body_text.
- Do not invent files, APIs, behavior, users, dates, or outcomes.
- If the source is vague, be specific only where the text is specific.
- If evidence is insufficient for a field, return null or [].
</grounding_rules>

<concept_rules>
- Use "gotcha" only when the source clearly describes a pitfall, surprise, failure mode, or caveat.
- Use "trade-off" only when the source clearly describes a comparison, compromise, or explicit design tension.
- Prefer fewer concepts over weak concepts.
</concept_rules>

<verbosity_controls>
- Keep the narrative concise and information-dense.
- Avoid repetition between narrative and facts.
</verbosity_controls>

<verification_loop>
- Before finalizing, verify: valid JSON, complete sentences in narrative, concepts only from the allowed list, and every claim grounded in the source.
</verification_loop>`;

	const user = `Memory ID: ${row.id}
Kind: ${row.kind}
Title: ${row.title}

Body text:
${row.body_text}`;

	return { system, user };
}

function sanitizeNarrative(value: string | null): string | null {
	if (!value) return null;
	let text = value.trim();
	// 末尾側の `\]+$` だけが二次。先頭側の `^\[+` は anchor が効くので regex のまま
	text = trimEndWhere(text.replace(/^\[+/, ""), CLOSE_BRACKET).trim();
	if (!text) return null;

	// If the model trails off without sentence punctuation, trim to the last
	// complete sentence if possible. Otherwise reject it as likely truncated.
	if (!/[.!?]["')\]]?\s*$/.test(text)) {
		const lastSentenceEnd = Math.max(
			text.lastIndexOf("."),
			text.lastIndexOf("!"),
			text.lastIndexOf("?"),
		);
		if (lastSentenceEnd >= 20) {
			text = text.slice(0, lastSentenceEnd + 1).trim();
		} else {
			return null;
		}
	}

	return text.length > 0 ? text : null;
}

function parseStructuredBackfillResponse(raw: string | null): ParsedStructuredBackfill {
	if (!raw) throw new Error("observer returned empty response");
	const trimmed = raw.trim();
	const cleaned = trimmed.startsWith("```")
		? stripTrailingFence(trimmed.replace(/^```(?:json)?\s*/i, ""))
		: trimmed;
	const parsed = JSON.parse(cleaned) as Record<string, unknown>;
	if (
		!Object.hasOwn(parsed, "narrative") ||
		!Object.hasOwn(parsed, "facts") ||
		!Object.hasOwn(parsed, "concepts")
	) {
		throw new Error("observer returned schema-invalid object");
	}
	if (
		!(typeof parsed.narrative === "string" || parsed.narrative === null) ||
		!Array.isArray(parsed.facts) ||
		!Array.isArray(parsed.concepts)
	) {
		throw new Error("observer returned schema-invalid field types");
	}
	const narrative =
		typeof parsed.narrative === "string" && parsed.narrative.trim()
			? sanitizeNarrative(parsed.narrative)
			: null;
	const facts = Array.isArray(parsed.facts)
		? parsed.facts.filter(
				(item): item is string => typeof item === "string" && item.trim().length > 0,
			)
		: [];
	const concepts = Array.isArray(parsed.concepts)
		? parsed.concepts
				.filter(
					(item): item is string =>
						typeof item === "string" &&
						item.trim().length > 0 &&
						AI_BACKFILL_CONCEPT_SET.has(item.trim().toLowerCase()),
				)
				.map((item) => item.trim().toLowerCase())
		: [];
	return { narrative, facts, concepts };
}

/**
 * AI-powered backfill for older non-session-summary memories that still lack
 * structured content (`narrative`, `facts`, `concepts`) through the frozen
 * manifest-selected provider and existing ObserverClient integration.
 */
export async function aiBackfillStructuredContent(
	db: Database,
	opts: AIBackfillStructuredContentOptions = {},
): Promise<AIBackfillStructuredContentResult> {
	const kinds = opts.kinds?.length ? opts.kinds : [...AI_BACKFILL_KINDS];
	const placeholders = kinds.map(() => "?").join(",");
	const limitClause = opts.limit != null && opts.limit > 0 ? `LIMIT ${Number(opts.limit)}` : "";
	const structuredFilter = opts.overwrite
		? "1=1"
		: `(narrative IS NULL OR LENGTH(narrative) = 0 OR facts IS NULL OR LENGTH(facts) <= 2 OR concepts IS NULL OR LENGTH(concepts) <= 2)`;
	if (opts.runtimeReason !== "ready") {
		const pendingRows = db
			.prepare(
				`SELECT kind, metadata_json
				 FROM memory_items
				 WHERE active = 1
				   AND kind IN (${placeholders})
				   AND body_text IS NOT NULL
				   AND LENGTH(body_text) > 0
				   AND ${structuredFilter}
				 ORDER BY created_at ASC
				 ${limitClause}`,
			)
			.all(...kinds) as Array<{ kind: string; metadata_json: string | null }>;
		const skipped = pendingRows.filter(
			(row) => !isSummaryLikeMemory({ kind: row.kind, metadata: row.metadata_json }),
		).length;
		return { checked: 0, updated: 0, skipped, failed: 0 };
	}
	if (!opts.destinationBoundary) throw new Error("destination_unknown");
	const destinationPredicate = memoryDestinationBoundarySql(
		opts.destinationBoundary,
		"memory_items",
	);
	const rows = db
		.prepare(
			`SELECT memory_items.id, memory_items.kind, memory_items.title, memory_items.body_text,
			        memory_items.metadata_json, memory_items.narrative, memory_items.facts,
			        memory_items.concepts
			 FROM memory_items
			 WHERE active = 1
			   AND kind IN (${placeholders})
			   AND body_text IS NOT NULL
			   AND LENGTH(body_text) > 0
			   AND ${structuredFilter}
			   AND ${destinationPredicate.clause}
			 ORDER BY created_at ASC
			 ${limitClause}`,
		)
		.all(...kinds, ...destinationPredicate.params) as StructuredBackfillRow[];
	const eligibleRows = rows.filter(
		(row) => !isSummaryLikeMemory({ kind: row.kind, metadata: row.metadata_json }),
	);
	if (eligibleRows.length === 0) return { checked: 0, updated: 0, skipped: 0, failed: 0 };

	const observer =
		opts.observer ??
		createStructuredBackfillObserver(
			opts.summaryProvider,
			opts.resourceProfile,
			opts.destinationBoundary,
		);
	const scanner = opts.scanner ?? new SecretScanner();
	const total = eligibleRows.length;
	startMaintenanceJob(db, {
		kind: AI_BACKFILL_JOB_KIND,
		title: "Backfilling structured content",
		message: `Preparing structured-content extraction for ${total} memories`,
		progressTotal: total,
		metadata: {
			model: observer.getStatus().model,
			provider: observer.getStatus().provider,
			kinds,
			overwrite: opts.overwrite === true,
		},
	});

	let checked = 0;
	let updated = 0;
	let skipped = 0;
	let failed = 0;
	const samples: NonNullable<AIBackfillStructuredContentResult["samples"]> = [];
	const updateStmt = db.prepare(
		"UPDATE memory_items SET narrative = ?, facts = ?, concepts = ?, updated_at = ? WHERE id = ?",
	);

	try {
		for (const row of eligibleRows) {
			checked++;
			if (!opts.overwrite && hasCompleteStructuredContent(row)) {
				skipped++;
				updateMaintenanceJob(db, AI_BACKFILL_JOB_KIND, {
					message: `Skipped ${skipped} already-structured memories`,
					progressCurrent: checked,
					progressTotal: total,
				});
				continue;
			}

			try {
				const prompt = buildStructuredBackfillPrompt(row);
				const response = await observer.observeStructuredJson(
					prompt.system,
					prompt.user,
					AI_BACKFILL_SCHEMA_NAME,
					AI_BACKFILL_SCHEMA,
				);
				const parsed =
					response.usedStructuredOutputs && response.parsed
						? parseStructuredBackfillResponse(JSON.stringify(response.parsed))
						: parseStructuredBackfillResponse(response.raw);

				// Redact AI output before adoption. Source content was already
				// scanned at write time, but the model can paraphrase past pattern
				// detection or invent secret-shaped strings, so output is its own
				// scan surface.
				if (parsed.narrative != null) {
					parsed.narrative = scanner.scan(parsed.narrative).redacted;
				}
				parsed.facts = parsed.facts.map((f) => scanner.scan(f).redacted);
				parsed.concepts = parsed.concepts.map((c) => scanner.scan(c).redacted);

				const nextNarrative =
					row.narrative?.trim() && !opts.overwrite ? row.narrative : parsed.narrative;
				const existingFacts = parseJsonArrayOfStrings(row.facts);
				const nextFacts =
					existingFacts.length > 0 && !opts.overwrite ? existingFacts : parsed.facts;
				const existingConcepts = parseJsonArrayOfStrings(row.concepts);
				const nextConcepts =
					existingConcepts.length > 0 && !opts.overwrite ? existingConcepts : parsed.concepts;

				const changed =
					(nextNarrative ?? null) !== (row.narrative ?? null) ||
					JSON.stringify(nextFacts) !== JSON.stringify(existingFacts) ||
					JSON.stringify(nextConcepts) !== JSON.stringify(existingConcepts);

				if (!changed) {
					skipped++;
				} else {
					if (opts.dryRun && samples.length < 10) {
						samples.push({
							id: row.id,
							kind: row.kind,
							title: row.title,
							narrative: nextNarrative,
							facts: nextFacts,
							concepts: nextConcepts,
						});
					}
					if (!opts.dryRun) {
						updateStmt.run(
							nextNarrative,
							JSON.stringify(nextFacts),
							JSON.stringify(nextConcepts),
							new Date().toISOString(),
							row.id,
						);
					}
					updated++;
				}
			} catch {
				failed++;
			}

			updateMaintenanceJob(db, AI_BACKFILL_JOB_KIND, {
				message: `Processed ${checked} of ${total} memories`,
				progressCurrent: checked,
				progressTotal: total,
				metadata: {
					model: observer.getStatus().model,
					provider: observer.getStatus().provider,
					kinds,
					overwrite: opts.overwrite === true,
					updated,
					skipped,
					failed,
				},
			});
		}

		completeMaintenanceJob(db, AI_BACKFILL_JOB_KIND, {
			message: `Processed ${checked} memories: ${updated} updated, ${skipped} skipped, ${failed} failed`,
			progressCurrent: checked,
			progressTotal: total,
			metadata: {
				model: observer.getStatus().model,
				provider: observer.getStatus().provider,
				kinds,
				overwrite: opts.overwrite === true,
				updated,
				skipped,
				failed,
			},
		});
	} catch (exc) {
		// Classify into a closed set of content-free codes — the raw message is
		// inspected, never persisted, so job rows stay free of provider content.
		const code =
			exc instanceof ObserverAuthError
				? "auth_failed"
				: exc instanceof Error &&
						(exc.name === "TimeoutError" || exc.message.toLowerCase().includes("timeout"))
					? "provider_timeout"
					: "output_invalid";
		failMaintenanceJob(db, AI_BACKFILL_JOB_KIND, code, {
			message: code,
			progressCurrent: checked,
			progressTotal: total,
		});
		throw new Error(code);
	}

	return { checked, updated, skipped, failed, ...(opts.dryRun ? { samples } : {}) };
}
