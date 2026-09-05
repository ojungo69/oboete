/**
 * Drizzle ORM schema for the codemem SQLite database.
 */

import { sql } from "drizzle-orm";
import {
	check,
	index,
	integer,
	primaryKey,
	real,
	sqliteTable,
	text,
	uniqueIndex,
} from "drizzle-orm/sqlite-core";
import { PROCESSING_JOB_RETRY_LIMIT } from "./capability-manifest.js";

export const sessions = sqliteTable("sessions", {
	id: integer("id").primaryKey(),
	started_at: text("started_at").notNull(),
	ended_at: text("ended_at"),
	cwd: text("cwd"),
	project: text("project"),
	git_remote: text("git_remote"),
	git_branch: text("git_branch"),
	user: text("user"),
	tool_version: text("tool_version"),
	metadata_json: text("metadata_json"),
	import_key: text("import_key"),
	repository_identity: text("repository_identity"),
});

export type Session = typeof sessions.$inferSelect;
export type NewSession = typeof sessions.$inferInsert;

export const replicationScopes = sqliteTable(
	"replication_scopes",
	{
		scope_id: text("scope_id").primaryKey(),
		label: text("label").notNull(),
		kind: text("kind").notNull().default("user"),
		authority_type: text("authority_type").notNull().default("local"),
		coordinator_id: text("coordinator_id"),
		group_id: text("group_id"),
		manifest_issuer_device_id: text("manifest_issuer_device_id"),
		membership_epoch: integer("membership_epoch").notNull().default(0),
		manifest_hash: text("manifest_hash"),
		status: text("status").notNull().default("active"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("idx_replication_scopes_status").on(table.status),
		index("idx_replication_scopes_authority_group").on(table.coordinator_id, table.group_id),
	],
);

export type ReplicationScope = typeof replicationScopes.$inferSelect;
export type NewReplicationScope = typeof replicationScopes.$inferInsert;

export const projectScopeMappings = sqliteTable(
	"project_scope_mappings",
	{
		id: integer("id").primaryKey(),
		workspace_identity: text("workspace_identity"),
		project_pattern: text("project_pattern").notNull(),
		scope_id: text("scope_id").notNull(),
		priority: integer("priority").notNull().default(0),
		source: text("source").notNull().default("user"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("idx_project_scope_mappings_workspace_priority").on(
			table.workspace_identity,
			table.priority,
		),
		index("idx_project_scope_mappings_pattern_priority").on(table.project_pattern, table.priority),
		index("idx_project_scope_mappings_scope").on(table.scope_id),
	],
);

export type ProjectScopeMapping = typeof projectScopeMappings.$inferSelect;
export type NewProjectScopeMapping = typeof projectScopeMappings.$inferInsert;

export const scopeMemberships = sqliteTable(
	"scope_memberships",
	{
		scope_id: text("scope_id").notNull(),
		device_id: text("device_id").notNull(),
		role: text("role").notNull().default("member"),
		status: text("status").notNull().default("active"),
		membership_epoch: integer("membership_epoch").notNull().default(0),
		coordinator_id: text("coordinator_id"),
		group_id: text("group_id"),
		manifest_issuer_device_id: text("manifest_issuer_device_id"),
		manifest_hash: text("manifest_hash"),
		signed_manifest_json: text("signed_manifest_json"),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		primaryKey({ columns: [table.scope_id, table.device_id] }),
		index("idx_scope_memberships_device_status").on(table.device_id, table.status),
		index("idx_scope_memberships_scope_status").on(table.scope_id, table.status),
		index("idx_scope_memberships_authority_group").on(table.coordinator_id, table.group_id),
	],
);

export type ScopeMembership = typeof scopeMemberships.$inferSelect;
export type NewScopeMembership = typeof scopeMemberships.$inferInsert;

export const artifacts = sqliteTable(
	"artifacts",
	{
		id: integer("id").primaryKey(),
		session_id: integer("session_id")
			.notNull()
			.references(() => sessions.id, { onDelete: "cascade" }),
		kind: text("kind").notNull(),
		path: text("path"),
		content_text: text("content_text"),
		content_hash: text("content_hash"),
		created_at: text("created_at").notNull(),
		metadata_json: text("metadata_json"),
		sensitivity: text("sensitivity").notNull().default("secret"),
		repository_identity: text("repository_identity"),
	},
	(table) => [
		index("idx_artifacts_session_kind").on(table.session_id, table.kind),
		check(
			"artifacts_sensitivity_check",
			sql`${table.sensitivity} IN ('eligible', 'local_only', 'private', 'secret')`,
		),
	],
);

export type Artifact = typeof artifacts.$inferSelect;
export type NewArtifact = typeof artifacts.$inferInsert;

export const memoryItems = sqliteTable(
	"memory_items",
	{
		id: integer("id").primaryKey(),
		session_id: integer("session_id")
			.notNull()
			.references(() => sessions.id, { onDelete: "cascade" }),
		kind: text("kind").notNull(),
		title: text("title").notNull(),
		subtitle: text("subtitle"),
		body_text: text("body_text").notNull(),
		confidence: real("confidence").default(0.5),
		tags_text: text("tags_text").default(""),
		active: integer("active").default(1),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
		metadata_json: text("metadata_json"),
		actor_id: text("actor_id"),
		actor_display_name: text("actor_display_name"),
		visibility: text("visibility"),
		workspace_id: text("workspace_id"),
		workspace_kind: text("workspace_kind"),
		origin_device_id: text("origin_device_id"),
		origin_source: text("origin_source"),
		trust_state: text("trust_state"),
		facts: text("facts"),
		narrative: text("narrative"),
		concepts: text("concepts"),
		files_read: text("files_read"),
		files_modified: text("files_modified"),
		user_prompt_id: integer("user_prompt_id"),
		prompt_number: integer("prompt_number"),
		deleted_at: text("deleted_at"),
		rev: integer("rev").default(0),
		dedup_key: text("dedup_key"),
		import_key: text("import_key"),
		scope_id: text("scope_id"),
		// Denormalized project name. Carries the originating session's project
		// across sync boundaries so cross-device memories surface under their
		// real project identity instead of an inferred placeholder. Backfilled
		// from sessions.project for legacy rows during migration. May be null
		// when the originating session had no project.
		project: text("project"),
		sensitivity: text("sensitivity").notNull().default("secret"),
		repository_identity: text("repository_identity"),
		lineage_id: text("lineage_id"),
		revision_id: text("revision_id"),
		revision_ordinal: integer("revision_ordinal"),
		supersedes_memory_id: integer("supersedes_memory_id"),
		derivation_key: text("derivation_key"),
		source_event_ids_json: text("source_event_ids_json"),
		source_spans_json: text("source_spans_json"),
		manifest_fingerprint: text("manifest_fingerprint"),
		provider_fingerprint: text("provider_fingerprint"),
		attempt_fingerprint: text("attempt_fingerprint"),
	},
	(table) => [
		index("idx_memory_items_active_created").on(table.active, table.created_at),
		index("idx_memory_items_origin_device_active").on(table.origin_device_id, table.active),
		index("idx_memory_items_session").on(table.session_id),
		index("idx_memory_items_project").on(table.project),
		index("idx_memory_items_scope_visibility_created").on(
			table.scope_id,
			table.visibility,
			table.created_at,
		),
		index("idx_memory_items_scope_backfill_pending")
			.on(table.id)
			.where(sql`scope_id IS NULL OR scope_id = ''`),
		index("idx_memory_items_dedup_key_active_created").on(
			table.dedup_key,
			table.active,
			table.created_at,
		),
		uniqueIndex("idx_memory_items_same_session_dedup_unique")
			.on(table.session_id, table.kind, table.visibility, table.workspace_id, table.dedup_key)
			.where(sql`active = 1 AND dedup_key IS NOT NULL`),
		check(
			"memory_items_sensitivity_check",
			sql`${table.sensitivity} IN ('eligible', 'local_only', 'private', 'secret')`,
		),
	],
);

export type MemoryItem = typeof memoryItems.$inferSelect;
export type NewMemoryItem = typeof memoryItems.$inferInsert;

export const memoryFileRefs = sqliteTable(
	"memory_file_refs",
	{
		memory_id: integer("memory_id")
			.notNull()
			.references(() => memoryItems.id, { onDelete: "cascade" }),
		file_path: text("file_path").notNull(),
		relation: text("relation").notNull(), // 'read' | 'modified'
	},
	(table) => [
		primaryKey({ columns: [table.memory_id, table.file_path, table.relation] }),
		index("idx_memory_file_refs_path").on(table.file_path),
	],
);

export type MemoryFileRef = typeof memoryFileRefs.$inferSelect;
export type NewMemoryFileRef = typeof memoryFileRefs.$inferInsert;

export const memoryConceptRefs = sqliteTable(
	"memory_concept_refs",
	{
		memory_id: integer("memory_id")
			.notNull()
			.references(() => memoryItems.id, { onDelete: "cascade" }),
		concept: text("concept").notNull(),
	},
	(table) => [
		primaryKey({ columns: [table.memory_id, table.concept] }),
		index("idx_memory_concept_refs_concept").on(table.concept),
	],
);

export type MemoryConceptRef = typeof memoryConceptRefs.$inferSelect;
export type NewMemoryConceptRef = typeof memoryConceptRefs.$inferInsert;

export const usageEvents = sqliteTable(
	"usage_events",
	{
		id: integer("id").primaryKey(),
		session_id: integer("session_id").references(() => sessions.id, {
			onDelete: "set null",
		}),
		event: text("event").notNull(),
		tokens_read: integer("tokens_read").default(0),
		tokens_written: integer("tokens_written").default(0),
		tokens_saved: integer("tokens_saved").default(0),
		created_at: text("created_at").notNull(),
		metadata_json: text("metadata_json"),
	},
	(table) => [
		index("idx_usage_events_event_created").on(table.event, table.created_at),
		index("idx_usage_events_session").on(table.session_id),
	],
);

export type UsageEvent = typeof usageEvents.$inferSelect;
export type NewUsageEvent = typeof usageEvents.$inferInsert;

export const retrievalAttempts = sqliteTable(
	"retrieval_attempts",
	{
		attempt_id: text("attempt_id").primaryKey(),
		contract_version: integer("contract_version").notNull(),
		surface: text("surface").notNull(),
		trigger: text("trigger").notNull(),
		started_at: text("started_at").notNull(),
		completed_at: text("completed_at"),
		retrieval_status: text("retrieval_status").notNull(),
		delivery_status: text("delivery_status").notNull(),
		candidate_count: integer("candidate_count").notNull(),
		selected_count: integer("selected_count").notNull(),
		persisted_candidate_count: integer("persisted_candidate_count").notNull(),
		recorder_version: text("recorder_version").notNull(),
		session_id: integer("session_id").references(() => sessions.id, { onDelete: "cascade" }),
		source: text("source"),
		stream_id: text("stream_id"),
		source_session_id: text("source_session_id"),
		prompt_number: integer("prompt_number"),
		request_id: text("request_id"),
		raw_event_start_seq: integer("raw_event_start_seq"),
		raw_event_end_seq: integer("raw_event_end_seq"),
		experiment_id: text("experiment_id"),
		experiment_cell_id: text("experiment_cell_id"),
		evaluation_checkout_id: text("evaluation_checkout_id"),
		evaluation_fixture_id: text("evaluation_fixture_id"),
		evaluation_seed: integer("evaluation_seed"),
		latency_ms: integer("latency_ms"),
		project: text("project"),
		scope_id: text("scope_id"),
		mode: text("mode"),
		limit_requested: integer("limit_requested"),
		token_budget: integer("token_budget"),
		output_tokens: integer("output_tokens"),
		working_set_file_count: integer("working_set_file_count"),
		working_set_files_json: text("working_set_files_json"),
		query_hash_sha256: text("query_hash_sha256"),
		query_char_count: integer("query_char_count"),
		query_token_estimate: integer("query_token_estimate"),
		filter_summary_json: text("filter_summary_json"),
		failure_code: text("failure_code"),
		failure_stage: text("failure_stage"),
		trace_version: integer("trace_version"),
		retention_until: text("retention_until"),
		retention_pinned: integer("retention_pinned").notNull().default(0),
		retention_finalized_at: text("retention_finalized_at"),
	},
	(table) => [
		index("idx_retrieval_attempts_session_started").on(table.session_id, table.started_at),
		index("idx_retrieval_attempts_source_stream_started").on(
			table.source,
			table.stream_id,
			table.started_at,
		),
		index("idx_retrieval_attempts_retention").on(table.retention_pinned, table.retention_until),
		index("idx_retrieval_attempts_started").on(table.started_at, table.attempt_id),
		index("idx_retrieval_attempts_surface_started").on(table.surface, table.started_at),
		uniqueIndex("idx_retrieval_attempts_request_identity")
			.on(table.source, table.surface, table.request_id)
			.where(sql`request_id IS NOT NULL`),
		index("idx_retrieval_attempts_experiment_cell").on(
			table.experiment_id,
			table.experiment_cell_id,
		),
	],
);

export type RetrievalAttempt = typeof retrievalAttempts.$inferSelect;
export type NewRetrievalAttempt = typeof retrievalAttempts.$inferInsert;

export const retrievalExposures = sqliteTable(
	"retrieval_exposures",
	{
		exposure_id: integer("exposure_id").primaryKey({ autoIncrement: true }),
		attempt_id: text("attempt_id")
			.notNull()
			.references(() => retrievalAttempts.attempt_id, { onDelete: "cascade" }),
		memory_id: integer("memory_id").references(() => memoryItems.id, { onDelete: "set null" }),
		memory_import_key: text("memory_import_key"),
		origin_device_id: text("origin_device_id"),
		rank: integer("rank").notNull(),
		disposition: text("disposition").notNull(),
		section: text("section"),
		handoff_status: text("handoff_status").notNull(),
		memory_rev: integer("memory_rev"),
		memory_updated_at: text("memory_updated_at"),
		memory_scope_id: text("memory_scope_id"),
		memory_kind: text("memory_kind"),
		memory_active: integer("memory_active"),
		memory_deleted_at: text("memory_deleted_at"),
		score_summary_json: text("score_summary_json"),
		reason_codes_json: text("reason_codes_json"),
	},
	(table) => [
		uniqueIndex("idx_retrieval_exposures_attempt_rank").on(table.attempt_id, table.rank),
		index("idx_retrieval_exposures_memory").on(table.memory_id),
	],
);

export type RetrievalExposure = typeof retrievalExposures.$inferSelect;
export type NewRetrievalExposure = typeof retrievalExposures.$inferInsert;

export const outcomeEvidence = sqliteTable(
	"outcome_evidence",
	{
		evidence_id: text("evidence_id").primaryKey(),
		contract_version: integer("contract_version").notNull(),
		dimension: text("dimension").notNull(),
		evidence_type: text("evidence_type").notNull(),
		source_class: text("source_class").notNull(),
		observed_at: text("observed_at").notNull(),
		producer: text("producer").notNull(),
		producer_version: text("producer_version").notNull(),
		status: text("status").notNull(),
		value_type: text("value_type"),
		value_integer: integer("value_integer"),
		value_real: real("value_real"),
		value_unit: text("value_unit"),
		session_id: integer("session_id").references(() => sessions.id, { onDelete: "cascade" }),
		source: text("source"),
		stream_id: text("stream_id"),
		source_session_id: text("source_session_id"),
		prompt_number: integer("prompt_number"),
		raw_event_start_seq: integer("raw_event_start_seq"),
		raw_event_end_seq: integer("raw_event_end_seq"),
		experiment_id: text("experiment_id"),
		experiment_cell_id: text("experiment_cell_id"),
		window_start_at: text("window_start_at"),
		window_end_at: text("window_end_at"),
		references_json: text("references_json"),
		retention_until: text("retention_until"),
		retention_pinned: integer("retention_pinned").notNull().default(0),
		retention_finalized_at: text("retention_finalized_at"),
	},
	(table) => [
		index("idx_outcome_evidence_observed_id").on(table.observed_at, table.evidence_id),
		index("idx_outcome_evidence_session_observed").on(table.session_id, table.observed_at),
		index("idx_outcome_evidence_source_stream_observed").on(
			table.source,
			table.stream_id,
			table.observed_at,
		),
		index("idx_outcome_evidence_type_observed").on(table.evidence_type, table.observed_at),
		index("idx_outcome_evidence_retention").on(table.retention_pinned, table.retention_until),
	],
);

export type OutcomeEvidence = typeof outcomeEvidence.$inferSelect;
export type NewOutcomeEvidence = typeof outcomeEvidence.$inferInsert;

export const attributionAssessments = sqliteTable(
	"attribution_assessments",
	{
		assessment_id: text("assessment_id").primaryKey(),
		contract_version: integer("contract_version").notNull(),
		subject_type: text("subject_type").notNull(),
		attempt_id: text("attempt_id")
			.notNull()
			.references(() => retrievalAttempts.attempt_id, { onDelete: "cascade" }),
		exposure_id: integer("exposure_id").references(() => retrievalExposures.exposure_id, {
			onDelete: "cascade",
		}),
		dimension: text("dimension").notNull(),
		impact_label: text("impact_label").notNull(),
		basis: text("basis").notNull(),
		confidence_level: text("confidence_level").notNull(),
		method: text("method").notNull(),
		method_version: text("method_version").notNull(),
		created_at: text("created_at").notNull(),
		claim_type: text("claim_type").notNull().default("observational"),
	},
	(table) => [
		index("idx_attribution_assessments_attempt_created").on(table.attempt_id, table.created_at),
		index("idx_attribution_assessments_label_created").on(table.impact_label, table.created_at),
		index("idx_attribution_assessments_exposure").on(table.exposure_id),
	],
);

export type AttributionAssessment = typeof attributionAssessments.$inferSelect;
export type NewAttributionAssessment = typeof attributionAssessments.$inferInsert;

export const attributionAssessmentEvidence = sqliteTable(
	"attribution_assessment_evidence",
	{
		assessment_id: text("assessment_id")
			.notNull()
			.references(() => attributionAssessments.assessment_id, { onDelete: "cascade" }),
		evidence_id: text("evidence_id")
			.notNull()
			.references(() => outcomeEvidence.evidence_id, { onDelete: "cascade" }),
	},
	(table) => [
		primaryKey({ columns: [table.assessment_id, table.evidence_id] }),
		index("idx_attribution_assessment_evidence_evidence").on(
			table.evidence_id,
			table.assessment_id,
		),
	],
);

export type AttributionAssessmentEvidence = typeof attributionAssessmentEvidence.$inferSelect;
export type NewAttributionAssessmentEvidence = typeof attributionAssessmentEvidence.$inferInsert;

export const maintenanceJobs = sqliteTable(
	"maintenance_jobs",
	{
		kind: text("kind").primaryKey(),
		title: text("title").notNull(),
		status: text("status").notNull(),
		message: text("message"),
		progress_current: integer("progress_current").notNull().default(0),
		progress_total: integer("progress_total"),
		progress_unit: text("progress_unit").notNull().default("items"),
		metadata_json: text("metadata_json"),
		started_at: text("started_at"),
		updated_at: text("updated_at").notNull(),
		finished_at: text("finished_at"),
		error: text("error"),
	},
	(table) => [index("idx_maintenance_jobs_status_updated").on(table.status, table.updated_at)],
);

export type MaintenanceJob = typeof maintenanceJobs.$inferSelect;
export type NewMaintenanceJob = typeof maintenanceJobs.$inferInsert;

export const rawEvents = sqliteTable(
	"raw_events",
	{
		id: integer("id").primaryKey(),
		source: text("source").notNull().default("opencode"),
		stream_id: text("stream_id").notNull().default(""),
		opencode_session_id: text("opencode_session_id").notNull(),
		event_id: text("event_id").notNull(),
		event_seq: integer("event_seq").notNull(),
		event_type: text("event_type").notNull(),
		ts_wall_ms: integer("ts_wall_ms"),
		ts_mono_ms: real("ts_mono_ms"),
		payload_json: text("payload_json").notNull(),
		created_at: text("created_at").notNull(),
		sensitivity: text("sensitivity").notNull().default("secret"),
		repository_identity: text("repository_identity"),
		capture_manifest_fingerprint: text("capture_manifest_fingerprint"),
		capture_state: text("capture_state").notNull().default("accepted"),
		safe_error_code: text("safe_error_code"),
		payload_digest_version: text("payload_digest_version")
			.notNull()
			.default("event-payload-digest-v1"),
		payload_digest: text("payload_digest").notNull(),
	},
	(table) => [
		uniqueIndex("idx_raw_events_source_stream_seq").on(
			table.source,
			table.stream_id,
			table.event_seq,
		),
		index("idx_raw_events_session_seq").on(table.opencode_session_id, table.event_seq),
		index("idx_raw_events_created").on(table.created_at),
		check(
			"raw_events_sensitivity_check",
			sql`${table.sensitivity} IN ('eligible', 'local_only', 'private', 'secret')`,
		),
		check(
			"raw_events_capture_state_check",
			sql`${table.capture_state} IN ('accepted', 'quarantined')`,
		),
		check(
			"raw_events_quarantine_error_check",
			sql`${table.capture_state} != 'quarantined' OR COALESCE(${table.safe_error_code}, '') = 'redaction_degraded'`,
		),
		check(
			"raw_events_repository_identity_check",
			sql`COALESCE(${table.repository_identity}, '') != 'repo-v1:unknown'`,
		),
		check(
			"raw_events_payload_digest_check",
			sql`${table.payload_digest_version} = 'event-payload-digest-v1' AND length(${table.payload_digest}) = 71 AND substr(${table.payload_digest}, 1, 7) = 'sha256:' AND substr(${table.payload_digest}, 8) NOT GLOB '*[^0-9a-f]*' AND ${table.payload_digest} != 'sha256:0000000000000000000000000000000000000000000000000000000000000000'`,
		),
	],
);

export type RawEvent = typeof rawEvents.$inferSelect;
export type NewRawEvent = typeof rawEvents.$inferInsert;

export const rawEventIdentityConflicts = sqliteTable(
	"raw_event_identity_conflicts",
	{
		receipt_id: text("receipt_id").primaryKey(),
		repository_identity: text("repository_identity"),
		source: text("source").notNull(),
		stream_id: text("stream_id").notNull(),
		event_id: text("event_id").notNull(),
		payload_digest_version: text("payload_digest_version").notNull(),
		canonical_payload_digest: text("canonical_payload_digest").notNull(),
		conflicting_payload_digest: text("conflicting_payload_digest").notNull(),
		reason: text("reason").notNull().default("event_identity_payload_conflict"),
		receipt_state: text("receipt_state").notNull().default("non_success"),
		canonical_unchanged: integer("canonical_unchanged").notNull().default(1),
		memory_delta: integer("memory_delta").notNull().default(0),
		capture_manifest_fingerprint: text("capture_manifest_fingerprint"),
		first_seen_at: text("first_seen_at").notNull(),
		last_seen_at: text("last_seen_at").notNull(),
		occurrence_count: integer("occurrence_count").notNull().default(1),
	},
	(table) => [
		check(
			"raw_event_identity_conflicts_receipt_state_check",
			sql`${table.receipt_state} = 'non_success'`,
		),
		check(
			"raw_event_identity_conflicts_canonical_unchanged_check",
			sql`${table.canonical_unchanged} = 1`,
		),
		check("raw_event_identity_conflicts_memory_delta_check", sql`${table.memory_delta} = 0`),
		check(
			"raw_event_identity_conflicts_repository_identity_check",
			sql`COALESCE(${table.repository_identity}, '') != 'repo-v1:unknown'`,
		),
		check(
			"raw_event_identity_conflicts_digest_check",
			sql`${table.payload_digest_version} = 'event-payload-digest-v1' AND length(${table.canonical_payload_digest}) = 71 AND substr(${table.canonical_payload_digest}, 1, 7) = 'sha256:' AND substr(${table.canonical_payload_digest}, 8) NOT GLOB '*[^0-9a-f]*' AND ${table.canonical_payload_digest} != 'sha256:0000000000000000000000000000000000000000000000000000000000000000' AND length(${table.conflicting_payload_digest}) = 71 AND substr(${table.conflicting_payload_digest}, 1, 7) = 'sha256:' AND substr(${table.conflicting_payload_digest}, 8) NOT GLOB '*[^0-9a-f]*' AND ${table.conflicting_payload_digest} != 'sha256:0000000000000000000000000000000000000000000000000000000000000000' AND ${table.canonical_payload_digest} != ${table.conflicting_payload_digest}`,
		),
	],
);

export type RawEventIdentityConflict = typeof rawEventIdentityConflicts.$inferSelect;
export type NewRawEventIdentityConflict = typeof rawEventIdentityConflicts.$inferInsert;

export const rawEventQuarantine = sqliteTable(
	"raw_event_quarantine",
	{
		receipt_id: text("receipt_id").primaryKey(),
		repository_identity: text("repository_identity"),
		source: text("source").notNull(),
		stream_id: text("stream_id").notNull(),
		event_id: text("event_id").notNull(),
		event_type: text("event_type").notNull(),
		ts_wall_ms: integer("ts_wall_ms"),
		ts_mono_ms: real("ts_mono_ms"),
		payload_json: text("payload_json").notNull().default("{}"),
		payload_digest_version: text("payload_digest_version").notNull(),
		payload_digest: text("payload_digest").notNull(),
		sensitivity: text("sensitivity").notNull().default("secret"),
		capture_state: text("capture_state").notNull().default("quarantined"),
		safe_error_code: text("safe_error_code").notNull(),
		capture_manifest_fingerprint: text("capture_manifest_fingerprint"),
		first_seen_at: text("first_seen_at").notNull(),
		last_seen_at: text("last_seen_at").notNull(),
		occurrence_count: integer("occurrence_count").notNull().default(1),
	},
	(table) => [
		check("raw_event_quarantine_sensitivity_check", sql`${table.sensitivity} = 'secret'`),
		check("raw_event_quarantine_capture_state_check", sql`${table.capture_state} = 'quarantined'`),
		check(
			"raw_event_quarantine_payload_check",
			sql`${table.safe_error_code} != 'redaction_degraded' OR ${table.payload_json} = '{}'`,
		),
		check(
			"raw_event_quarantine_error_check",
			sql`${table.safe_error_code} IN ('repository_identity_unknown_collision', 'redaction_degraded')`,
		),
		check(
			"raw_event_quarantine_repository_identity_check",
			sql`COALESCE(${table.repository_identity}, '') != 'repo-v1:unknown'`,
		),
		check(
			"raw_event_quarantine_digest_check",
			sql`${table.payload_digest_version} = 'event-payload-digest-v1' AND length(${table.payload_digest}) = 71 AND substr(${table.payload_digest}, 1, 7) = 'sha256:' AND substr(${table.payload_digest}, 8) NOT GLOB '*[^0-9a-f]*' AND ${table.payload_digest} != 'sha256:0000000000000000000000000000000000000000000000000000000000000000'`,
		),
	],
);

export type RawEventQuarantine = typeof rawEventQuarantine.$inferSelect;
export type NewRawEventQuarantine = typeof rawEventQuarantine.$inferInsert;

export const rawEventSessions = sqliteTable(
	"raw_event_sessions",
	{
		source: text("source").notNull().default("opencode"),
		stream_id: text("stream_id").notNull().default(""),
		opencode_session_id: text("opencode_session_id").notNull(),
		cwd: text("cwd"),
		project: text("project"),
		started_at: text("started_at"),
		last_seen_ts_wall_ms: integer("last_seen_ts_wall_ms"),
		last_received_event_seq: integer("last_received_event_seq").notNull().default(-1),
		last_flushed_event_seq: integer("last_flushed_event_seq").notNull().default(-1),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [primaryKey({ columns: [table.source, table.stream_id] })],
);

export type RawEventSession = typeof rawEventSessions.$inferSelect;
export type NewRawEventSession = typeof rawEventSessions.$inferInsert;

export const opencodeSessions = sqliteTable(
	"opencode_sessions",
	{
		source: text("source").notNull().default("opencode"),
		stream_id: text("stream_id").notNull().default(""),
		opencode_session_id: text("opencode_session_id").notNull(),
		session_id: integer("session_id").references(() => sessions.id, {
			onDelete: "cascade",
		}),
		created_at: text("created_at").notNull(),
	},
	(table) => [
		primaryKey({ columns: [table.source, table.stream_id] }),
		index("idx_opencode_sessions_session").on(table.session_id),
	],
);

export type OpencodeSession = typeof opencodeSessions.$inferSelect;
export type NewOpencodeSession = typeof opencodeSessions.$inferInsert;

export const rawEventFlushBatches = sqliteTable(
	"raw_event_flush_batches",
	{
		id: integer("id").primaryKey(),
		source: text("source").notNull().default("opencode"),
		stream_id: text("stream_id").notNull().default(""),
		opencode_session_id: text("opencode_session_id").notNull(),
		start_event_seq: integer("start_event_seq").notNull(),
		end_event_seq: integer("end_event_seq").notNull(),
		extractor_version: text("extractor_version").notNull(),
		status: text("status").notNull(),
		error_message: text("error_message"),
		error_type: text("error_type"),
		observer_provider: text("observer_provider"),
		observer_model: text("observer_model"),
		observer_runtime: text("observer_runtime"),
		observer_auth_source: text("observer_auth_source"),
		observer_auth_type: text("observer_auth_type"),
		observer_error_code: text("observer_error_code"),
		observer_error_message: text("observer_error_message"),
		attempt_count: integer("attempt_count").notNull().default(0),
		admission_manifest_fingerprint: text("admission_manifest_fingerprint"),
		admission_provider_fingerprint: text("admission_provider_fingerprint"),
		retry_limit: integer("retry_limit").notNull().default(PROCESSING_JOB_RETRY_LIMIT),
		claim_generation: integer("claim_generation").notNull().default(0),
		attempt_manifest_fingerprint: text("attempt_manifest_fingerprint"),
		attempt_provider_fingerprint: text("attempt_provider_fingerprint"),
		attempt_fingerprint: text("attempt_fingerprint"),
		attempt_max_memory_items: integer("attempt_max_memory_items"),
		resume_grant_id: text("resume_grant_id"),
		resume_grant_reason: text("resume_grant_reason"),
		resume_grant_state: text("resume_grant_state").notNull().default("none"),
		resume_grant_consumed_at: text("resume_grant_consumed_at"),
		last_resume_signal_id: text("last_resume_signal_id"),
		last_resume_sequence: integer("last_resume_sequence").notNull().default(0),
		last_resume_signal_disposition: text("last_resume_signal_disposition")
			.notNull()
			.default("none"),
		safe_error_code: text("safe_error_code"),
		egress_diagnostic_json: text("egress_diagnostic_json"),
		output_count: integer("output_count").notNull().default(0),
		observed_output_count: integer("observed_output_count").notNull().default(0),
		completion_disposition: text("completion_disposition").notNull().default("none"),
		legacy_recovery_state: text("legacy_recovery_state").notNull().default("not_legacy"),
		frontier_already_advanced: integer("frontier_already_advanced").notNull().default(0),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("idx_flush_batches_source_stream_seq_ver").on(
			table.source,
			table.stream_id,
			table.start_event_seq,
			table.end_event_seq,
			table.extractor_version,
		),
		index("idx_flush_batches_session_created").on(table.opencode_session_id, table.created_at),
		index("idx_flush_batches_status_updated").on(table.status, table.updated_at),
		check(
			"raw_event_flush_batches_status_check",
			sql`${table.status} IN ('queued', 'processing', 'failed', 'retry_exhausted', 'completed')`,
		),
		check(
			"raw_event_flush_batches_resume_grant_state_check",
			sql`${table.resume_grant_state} IN ('none', 'pending', 'consumed')`,
		),
		check(
			"raw_event_flush_batches_completion_disposition_check",
			sql`${table.completion_disposition} IN ('none', 'memory_committed', 'privacy_skip', 'legacy_unrecoverable')`,
		),
		check(
			"raw_event_flush_batches_legacy_recovery_state_check",
			sql`${table.legacy_recovery_state} IN ('not_legacy', 'complete_range', 'missing_or_ambiguous_range')`,
		),
		check(
			"raw_event_flush_batches_last_resume_disposition_check",
			sql`${table.last_resume_signal_disposition} IN ('none', 'accepted', 'duplicate', 'stale', 'grant_pending', 'wrong_job', 'wrong_role', 'wrong_provider', 'unchanged_configuration', 'unrelated_component')`,
		),
		check(
			"raw_event_flush_batches_attempt_max_memory_items_check",
			sql`${table.attempt_max_memory_items} IS NULL OR ${table.attempt_max_memory_items} IN (16, 17)`,
		),
		check(
			"raw_event_flush_batches_resume_grant_reason_check",
			sql`${table.resume_grant_reason} IS NULL OR ${table.resume_grant_reason} IN ('validated_configuration_activation', 'recorded_provider_healthy_transition', 'user_confirmed_doctor_retry')`,
		),
	],
);

export type RawEventFlushBatch = typeof rawEventFlushBatches.$inferSelect;
export type NewRawEventFlushBatch = typeof rawEventFlushBatches.$inferInsert;

export const processingResumeProducerReceipts = sqliteTable(
	"processing_resume_producer_receipts",
	{
		receipt_id: text("receipt_id").primaryKey(),
		producer_kind: text("producer_kind").notNull(),
		configuration_fingerprint: text("configuration_fingerprint").notNull(),
		provider_fingerprint: text("provider_fingerprint").notNull(),
		producer_sequence: integer("producer_sequence").notNull(),
		fanout_count: integer("fanout_count").notNull().default(0),
		target_job_ids_json: text("target_job_ids_json").notNull().default("[]"),
		safe_error_code: text("safe_error_code"),
		created_at: text("created_at").notNull(),
	},
	(table) => [
		check(
			"processing_resume_producer_receipts_kind_check",
			sql`${table.producer_kind} IN ('validated_configuration_activation', 'recorded_provider_healthy_transition', 'user_confirmed_doctor_retry')`,
		),
		check(
			"processing_resume_producer_receipts_fingerprint_check",
			sql`length(${table.configuration_fingerprint}) = 71 AND substr(${table.configuration_fingerprint}, 1, 7) = 'sha256:' AND substr(${table.configuration_fingerprint}, 8) NOT GLOB '*[^0-9a-f]*' AND length(${table.provider_fingerprint}) = 71 AND substr(${table.provider_fingerprint}, 1, 7) = 'sha256:' AND substr(${table.provider_fingerprint}, 8) NOT GLOB '*[^0-9a-f]*'`,
		),
	],
);

export type ProcessingResumeProducerReceipt = typeof processingResumeProducerReceipts.$inferSelect;
export type NewProcessingResumeProducerReceipt =
	typeof processingResumeProducerReceipts.$inferInsert;

export const processingResumeSignals = sqliteTable(
	"processing_resume_signals",
	{
		signal_id: text("signal_id").primaryKey(),
		job_id: integer("job_id").notNull(),
		producer_receipt_id: text("producer_receipt_id")
			.notNull()
			.references(() => processingResumeProducerReceipts.receipt_id),
		sequence: integer("sequence").notNull(),
		target_role: text("target_role").notNull().default("summary"),
		target_provider_fingerprint: text("target_provider_fingerprint").notNull(),
		target_manifest_fingerprint: text("target_manifest_fingerprint").notNull(),
		kind: text("kind").notNull(),
		disposition: text("disposition").notNull(),
		grant_id: text("grant_id"),
		created_at: text("created_at").notNull(),
	},
	(table) => [
		uniqueIndex("idx_processing_resume_signals_job_receipt").on(
			table.job_id,
			table.producer_receipt_id,
		),
		uniqueIndex("idx_processing_resume_signals_job_signal").on(table.job_id, table.signal_id),
		uniqueIndex("idx_processing_resume_signals_grant").on(table.grant_id),
		index("idx_processing_resume_signals_job_sequence").on(table.job_id, table.sequence),
		check("processing_resume_signals_role_check", sql`${table.target_role} = 'summary'`),
		check(
			"processing_resume_signals_kind_check",
			sql`${table.kind} IN ('validated_configuration_activation', 'recorded_provider_healthy_transition', 'user_confirmed_doctor_retry')`,
		),
		check(
			"processing_resume_signals_disposition_check",
			sql`${table.disposition} IN ('accepted', 'stale', 'wrong_job', 'wrong_role', 'wrong_provider', 'unchanged_configuration', 'unrelated_component')`,
		),
		check(
			"processing_resume_signals_fingerprint_check",
			sql`length(${table.target_provider_fingerprint}) = 71 AND substr(${table.target_provider_fingerprint}, 1, 7) = 'sha256:' AND substr(${table.target_provider_fingerprint}, 8) NOT GLOB '*[^0-9a-f]*' AND length(${table.target_manifest_fingerprint}) = 71 AND substr(${table.target_manifest_fingerprint}, 1, 7) = 'sha256:' AND substr(${table.target_manifest_fingerprint}, 8) NOT GLOB '*[^0-9a-f]*'`,
		),
	],
);

export type ProcessingResumeSignal = typeof processingResumeSignals.$inferSelect;
export type NewProcessingResumeSignal = typeof processingResumeSignals.$inferInsert;

export const providerHealthStates = sqliteTable(
	"provider_health_states",
	{
		configuration_fingerprint: text("configuration_fingerprint").notNull(),
		provider_fingerprint: text("provider_fingerprint").notNull(),
		health_state: text("health_state").notNull(),
		last_transition_sequence: integer("last_transition_sequence").notNull().default(0),
		last_transition_receipt_id: text("last_transition_receipt_id"),
		safe_error_code: text("safe_error_code"),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		primaryKey({ columns: [table.configuration_fingerprint, table.provider_fingerprint] }),
		check(
			"provider_health_states_health_state_check",
			sql`${table.health_state} IN ('healthy', 'unhealthy')`,
		),
		check(
			"provider_health_states_fingerprint_check",
			sql`length(${table.configuration_fingerprint}) = 71 AND substr(${table.configuration_fingerprint}, 1, 7) = 'sha256:' AND substr(${table.configuration_fingerprint}, 8) NOT GLOB '*[^0-9a-f]*' AND length(${table.provider_fingerprint}) = 71 AND substr(${table.provider_fingerprint}, 1, 7) = 'sha256:' AND substr(${table.provider_fingerprint}, 8) NOT GLOB '*[^0-9a-f]*'`,
		),
		check(
			"provider_health_states_error_code_check",
			sql`(${table.health_state} = 'healthy' AND ${table.safe_error_code} IS NULL) OR (${table.health_state} = 'unhealthy' AND ${table.safe_error_code} IS NOT NULL AND ${table.safe_error_code} IN ('provider_unavailable', 'provider_tls_rejected'))`,
		),
	],
);

export type ProviderHealthState = typeof providerHealthStates.$inferSelect;
export type NewProviderHealthState = typeof providerHealthStates.$inferInsert;

export const userPrompts = sqliteTable(
	"user_prompts",
	{
		id: integer("id").primaryKey(),
		session_id: integer("session_id").references(() => sessions.id, {
			onDelete: "cascade",
		}),
		project: text("project"),
		prompt_text: text("prompt_text").notNull(),
		prompt_number: integer("prompt_number"),
		created_at: text("created_at").notNull(),
		created_at_epoch: integer("created_at_epoch").notNull(),
		metadata_json: text("metadata_json"),
		import_key: text("import_key"),
		sensitivity: text("sensitivity").notNull().default("secret"),
		repository_identity: text("repository_identity"),
	},
	(table) => [
		index("idx_user_prompts_session").on(table.session_id),
		index("idx_user_prompts_project").on(table.project),
		index("idx_user_prompts_epoch").on(table.created_at_epoch),
		check(
			"user_prompts_sensitivity_check",
			sql`${table.sensitivity} IN ('eligible', 'local_only', 'private', 'secret')`,
		),
	],
);

export type UserPrompt = typeof userPrompts.$inferSelect;
export type NewUserPrompt = typeof userPrompts.$inferInsert;

export const sessionSummaries = sqliteTable(
	"session_summaries",
	{
		id: integer("id").primaryKey(),
		session_id: integer("session_id").references(() => sessions.id, {
			onDelete: "cascade",
		}),
		project: text("project"),
		request: text("request"),
		investigated: text("investigated"),
		learned: text("learned"),
		completed: text("completed"),
		next_steps: text("next_steps"),
		notes: text("notes"),
		files_read: text("files_read"),
		files_edited: text("files_edited"),
		prompt_number: integer("prompt_number"),
		created_at: text("created_at").notNull(),
		created_at_epoch: integer("created_at_epoch").notNull(),
		metadata_json: text("metadata_json"),
		import_key: text("import_key"),
		sensitivity: text("sensitivity").notNull().default("secret"),
		repository_identity: text("repository_identity"),
	},
	(table) => [
		index("idx_session_summaries_session").on(table.session_id),
		index("idx_session_summaries_project").on(table.project),
		index("idx_session_summaries_epoch").on(table.created_at_epoch),
		check(
			"session_summaries_sensitivity_check",
			sql`${table.sensitivity} IN ('eligible', 'local_only', 'private', 'secret')`,
		),
	],
);

export type SessionSummary = typeof sessionSummaries.$inferSelect;
export type NewSessionSummary = typeof sessionSummaries.$inferInsert;

export const replicationOps = sqliteTable(
	"replication_ops",
	{
		op_id: text("op_id").primaryKey(),
		entity_type: text("entity_type").notNull(),
		entity_id: text("entity_id").notNull(),
		op_type: text("op_type").notNull(),
		payload_json: text("payload_json"),
		clock_rev: integer("clock_rev").notNull(),
		clock_updated_at: text("clock_updated_at").notNull(),
		clock_device_id: text("clock_device_id").notNull(),
		device_id: text("device_id").notNull(),
		created_at: text("created_at").notNull(),
		scope_id: text("scope_id"),
	},
	(table) => [
		index("idx_replication_ops_created").on(table.created_at, table.op_id),
		index("idx_replication_ops_entity").on(table.entity_type, table.entity_id),
		index("idx_replication_ops_scope_created").on(table.scope_id, table.created_at, table.op_id),
	],
);

export type ReplicationOp = typeof replicationOps.$inferSelect;
export type NewReplicationOp = typeof replicationOps.$inferInsert;

export const replicationCursors = sqliteTable("replication_cursors", {
	peer_device_id: text("peer_device_id").primaryKey(),
	last_applied_cursor: text("last_applied_cursor"),
	last_acked_cursor: text("last_acked_cursor"),
	updated_at: text("updated_at").notNull(),
});

export type ReplicationCursor = typeof replicationCursors.$inferSelect;
export type NewReplicationCursor = typeof replicationCursors.$inferInsert;

export const replicationCursorsV2 = sqliteTable(
	"replication_cursors_v2",
	{
		peer_device_id: text("peer_device_id").notNull(),
		scope_id: text("scope_id").notNull(),
		last_applied_cursor: text("last_applied_cursor"),
		last_acked_cursor: text("last_acked_cursor"),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		primaryKey({ columns: [table.peer_device_id, table.scope_id] }),
		index("idx_replication_cursors_v2_scope").on(table.scope_id),
	],
);

export type ReplicationCursorV2 = typeof replicationCursorsV2.$inferSelect;
export type NewReplicationCursorV2 = typeof replicationCursorsV2.$inferInsert;

export const syncPeers = sqliteTable("sync_peers", {
	peer_device_id: text("peer_device_id").primaryKey(),
	name: text("name"),
	pinned_fingerprint: text("pinned_fingerprint"),
	public_key: text("public_key"),
	addresses_json: text("addresses_json"),
	claimed_local_actor: integer("claimed_local_actor").notNull().default(0),
	actor_id: text("actor_id"),
	projects_include_json: text("projects_include_json"),
	projects_exclude_json: text("projects_exclude_json"),
	created_at: text("created_at").notNull(),
	last_seen_at: text("last_seen_at"),
	last_sync_at: text("last_sync_at"),
	last_error: text("last_error"),
	discovered_via_coordinator_id: text("discovered_via_coordinator_id"),
	discovered_via_group_id: text("discovered_via_group_id"),
	trust_provenance: text("trust_provenance"),
	pending_bootstrap_grant_id: text("pending_bootstrap_grant_id"),
});

export type SyncPeer = typeof syncPeers.$inferSelect;
export type NewSyncPeer = typeof syncPeers.$inferInsert;

export const syncNonces = sqliteTable("sync_nonces", {
	nonce: text("nonce").primaryKey(),
	device_id: text("device_id").notNull(),
	created_at: text("created_at").notNull(),
});

export type SyncNonce = typeof syncNonces.$inferSelect;
export type NewSyncNonce = typeof syncNonces.$inferInsert;

export const syncDevice = sqliteTable("sync_device", {
	device_id: text("device_id").primaryKey(),
	public_key: text("public_key").notNull(),
	fingerprint: text("fingerprint").notNull(),
	created_at: text("created_at").notNull(),
});

export type SyncDevice = typeof syncDevice.$inferSelect;
export type NewSyncDevice = typeof syncDevice.$inferInsert;

export const syncAttempts = sqliteTable(
	"sync_attempts",
	{
		id: integer("id").primaryKey({ autoIncrement: true }),
		peer_device_id: text("peer_device_id").notNull(),
		started_at: text("started_at").notNull(),
		finished_at: text("finished_at"),
		ok: integer("ok").notNull(),
		ops_in: integer("ops_in").notNull(),
		ops_out: integer("ops_out").notNull(),
		error: text("error"),
		local_sync_capability: text("local_sync_capability"),
		peer_sync_capability: text("peer_sync_capability"),
		negotiated_sync_capability: text("negotiated_sync_capability"),
	},
	(table) => [index("idx_sync_attempts_peer_started").on(table.peer_device_id, table.started_at)],
);

export type SyncAttempt = typeof syncAttempts.$inferSelect;
export type NewSyncAttempt = typeof syncAttempts.$inferInsert;

export const syncScopeRejections = sqliteTable(
	"sync_scope_rejections",
	{
		id: integer("id").primaryKey({ autoIncrement: true }),
		peer_device_id: text("peer_device_id"),
		op_id: text("op_id").notNull(),
		entity_type: text("entity_type").notNull(),
		entity_id: text("entity_id").notNull(),
		scope_id: text("scope_id"),
		reason: text("reason").notNull(),
		created_at: text("created_at").notNull(),
	},
	(table) => [
		index("idx_sync_scope_rejections_peer_created").on(table.peer_device_id, table.created_at),
		index("idx_sync_scope_rejections_scope_created").on(table.scope_id, table.created_at),
	],
);

export type SyncScopeRejection = typeof syncScopeRejections.$inferSelect;
export type NewSyncScopeRejection = typeof syncScopeRejections.$inferInsert;

export const syncDaemonState = sqliteTable("sync_daemon_state", {
	id: integer("id").primaryKey(),
	last_error: text("last_error"),
	last_traceback: text("last_traceback"),
	last_error_at: text("last_error_at"),
	last_ok_at: text("last_ok_at"),
	phase: text("phase"),
});

export type SyncDaemonState = typeof syncDaemonState.$inferSelect;
export type NewSyncDaemonState = typeof syncDaemonState.$inferInsert;

export const syncResetState = sqliteTable("sync_reset_state", {
	id: integer("id").primaryKey(),
	generation: integer("generation").notNull(),
	snapshot_id: text("snapshot_id").notNull(),
	baseline_cursor: text("baseline_cursor"),
	retained_floor_cursor: text("retained_floor_cursor"),
	updated_at: text("updated_at").notNull(),
});

export type SyncResetState = typeof syncResetState.$inferSelect;
export type NewSyncResetState = typeof syncResetState.$inferInsert;

export const syncResetStateV2 = sqliteTable("sync_reset_state_v2", {
	scope_id: text("scope_id").primaryKey(),
	generation: integer("generation").notNull(),
	snapshot_id: text("snapshot_id").notNull(),
	baseline_cursor: text("baseline_cursor"),
	retained_floor_cursor: text("retained_floor_cursor"),
	updated_at: text("updated_at").notNull(),
});

export type SyncResetStateV2 = typeof syncResetStateV2.$inferSelect;
export type NewSyncResetStateV2 = typeof syncResetStateV2.$inferInsert;

export const syncRetentionState = sqliteTable("sync_retention_state", {
	id: integer("id").primaryKey(),
	last_run_at: text("last_run_at"),
	last_duration_ms: integer("last_duration_ms"),
	last_deleted_ops: integer("last_deleted_ops").notNull().default(0),
	last_estimated_bytes_before: integer("last_estimated_bytes_before"),
	last_estimated_bytes_after: integer("last_estimated_bytes_after"),
	retained_floor_cursor: text("retained_floor_cursor"),
	last_error: text("last_error"),
	last_error_at: text("last_error_at"),
});

export type SyncRetentionState = typeof syncRetentionState.$inferSelect;
export type NewSyncRetentionState = typeof syncRetentionState.$inferInsert;

export const syncRetentionStateV2 = sqliteTable("sync_retention_state_v2", {
	scope_id: text("scope_id").primaryKey(),
	last_run_at: text("last_run_at"),
	last_duration_ms: integer("last_duration_ms"),
	last_deleted_ops: integer("last_deleted_ops").notNull().default(0),
	last_estimated_bytes_before: integer("last_estimated_bytes_before"),
	last_estimated_bytes_after: integer("last_estimated_bytes_after"),
	retained_floor_cursor: text("retained_floor_cursor"),
	last_error: text("last_error"),
	last_error_at: text("last_error_at"),
});

export type SyncRetentionStateV2 = typeof syncRetentionStateV2.$inferSelect;
export type NewSyncRetentionStateV2 = typeof syncRetentionStateV2.$inferInsert;

export const rawEventIngestSamples = sqliteTable("raw_event_ingest_samples", {
	id: integer("id").primaryKey({ autoIncrement: true }),
	created_at: text("created_at").notNull(),
	inserted_events: integer("inserted_events").notNull().default(0),
	skipped_invalid: integer("skipped_invalid").notNull().default(0),
	skipped_duplicate: integer("skipped_duplicate").notNull().default(0),
	skipped_conflict: integer("skipped_conflict").notNull().default(0),
});

export type RawEventIngestSample = typeof rawEventIngestSamples.$inferSelect;
export type NewRawEventIngestSample = typeof rawEventIngestSamples.$inferInsert;

export const rawEventIngestStats = sqliteTable("raw_event_ingest_stats", {
	id: integer("id").primaryKey(),
	inserted_events: integer("inserted_events").notNull().default(0),
	skipped_events: integer("skipped_events").notNull().default(0),
	skipped_invalid: integer("skipped_invalid").notNull().default(0),
	skipped_duplicate: integer("skipped_duplicate").notNull().default(0),
	skipped_conflict: integer("skipped_conflict").notNull().default(0),
	updated_at: text("updated_at").notNull(),
});

export type RawEventIngestStat = typeof rawEventIngestStats.$inferSelect;
export type NewRawEventIngestStat = typeof rawEventIngestStats.$inferInsert;

export const coordinatorGroupPreferences = sqliteTable(
	"coordinator_group_preferences",
	{
		coordinator_id: text("coordinator_id").notNull(),
		group_id: text("group_id").notNull(),
		projects_include_json: text("projects_include_json"),
		projects_exclude_json: text("projects_exclude_json"),
		auto_seed_scope: integer("auto_seed_scope").notNull().default(1),
		default_space_scope_id: text("default_space_scope_id"),
		auto_grant_default_space_on_join: integer("auto_grant_default_space_on_join")
			.notNull()
			.default(0),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [primaryKey({ columns: [table.coordinator_id, table.group_id] })],
);

export type CoordinatorGroupPreferences = typeof coordinatorGroupPreferences.$inferSelect;
export type NewCoordinatorGroupPreferences = typeof coordinatorGroupPreferences.$inferInsert;

export const actors = sqliteTable(
	"actors",
	{
		actor_id: text("actor_id").primaryKey(),
		display_name: text("display_name").notNull(),
		is_local: integer("is_local").notNull().default(0),
		status: text("status").notNull().default("active"),
		merged_into_actor_id: text("merged_into_actor_id"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("idx_actors_is_local").on(table.is_local),
		index("idx_actors_status").on(table.status),
	],
);

export type Actor = typeof actors.$inferSelect;
export type NewActor = typeof actors.$inferInsert;

export const recipientPolicyReviewResolutions = sqliteTable(
	"recipient_policy_review_resolutions",
	{
		review_item_id: text("review_item_id").notNull(),
		source_fingerprint: text("source_fingerprint").notNull(),
		decision: text("decision").notNull(),
		decision_input_json: text("decision_input_json").notNull(),
		preview_json: text("preview_json").notNull(),
		decided_by_identity_id: text("decided_by_identity_id").notNull(),
		decided_by_device_id: text("decided_by_device_id").notNull(),
		resolved_at: text("resolved_at").notNull(),
	},
	(table) => [primaryKey({ columns: [table.review_item_id, table.source_fingerprint] })],
);

export type RecipientPolicyReviewResolution = typeof recipientPolicyReviewResolutions.$inferSelect;
export type NewRecipientPolicyReviewResolution =
	typeof recipientPolicyReviewResolutions.$inferInsert;

export const coordinatorEnrollmentReconciliationIssues = sqliteTable(
	"coordinator_enrollment_reconciliation_issues",
	{
		coordinator_id: text("coordinator_id").notNull(),
		group_id: text("group_id").notNull(),
		kind: text("kind").notNull(),
		reference_id: text("reference_id").notNull(),
		code: text("code").notNull(),
		status: text("status").notNull().default("open"),
		first_seen_at: text("first_seen_at").notNull(),
		last_seen_at: text("last_seen_at").notNull(),
		resolved_at: text("resolved_at"),
		occurrence_count: integer("occurrence_count").notNull().default(1),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		primaryKey({
			columns: [table.coordinator_id, table.group_id, table.kind, table.reference_id, table.code],
		}),
		index("idx_coordinator_enrollment_issues_boundary_status").on(
			table.coordinator_id,
			table.group_id,
			table.status,
		),
		index("idx_coordinator_enrollment_issues_status_recent").on(
			table.status,
			table.last_seen_at,
			table.resolved_at,
		),
	],
);

export type CoordinatorEnrollmentReconciliationIssue =
	typeof coordinatorEnrollmentReconciliationIssues.$inferSelect;
export type NewCoordinatorEnrollmentReconciliationIssue =
	typeof coordinatorEnrollmentReconciliationIssues.$inferInsert;

export const policyTeams = sqliteTable("policy_teams", {
	team_id: text("team_id").primaryKey(),
	display_name: text("display_name").notNull(),
	status: text("status").notNull(),
	provenance: text("provenance").notNull(),
	revision: text("revision").notNull(),
	migration_state: text("migration_state").notNull(),
	source_fingerprint: text("source_fingerprint"),
	idempotency_key: text("idempotency_key").notNull().unique(),
	created_at: text("created_at").notNull(),
	updated_at: text("updated_at").notNull(),
});

export type PolicyTeam = typeof policyTeams.$inferSelect;
export type NewPolicyTeam = typeof policyTeams.$inferInsert;

export const policyTeamMemberships = sqliteTable(
	"policy_team_memberships",
	{
		team_id: text("team_id").notNull(),
		identity_id: text("identity_id").notNull(),
		role: text("role").notNull(),
		status: text("status").notNull(),
		provenance: text("provenance").notNull(),
		revision: text("revision").notNull(),
		migration_state: text("migration_state").notNull(),
		source_fingerprint: text("source_fingerprint"),
		idempotency_key: text("idempotency_key").notNull().unique(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		primaryKey({ columns: [table.team_id, table.identity_id] }),
		index("idx_policy_team_memberships_identity_status").on(table.identity_id, table.status),
	],
);

export type PolicyTeamMembership = typeof policyTeamMemberships.$inferSelect;
export type NewPolicyTeamMembership = typeof policyTeamMemberships.$inferInsert;

export const identityDevices = sqliteTable(
	"identity_devices",
	{
		device_id: text("device_id").primaryKey(),
		identity_id: text("identity_id").notNull(),
		display_name: text("display_name").notNull(),
		status: text("status").notNull(),
		provenance: text("provenance").notNull(),
		revision: text("revision").notNull(),
		migration_state: text("migration_state").notNull(),
		source_fingerprint: text("source_fingerprint"),
		idempotency_key: text("idempotency_key").notNull().unique(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [index("idx_identity_devices_identity_status").on(table.identity_id, table.status)],
);

export type IdentityDevice = typeof identityDevices.$inferSelect;
export type NewIdentityDevice = typeof identityDevices.$inferInsert;

export const projectRecipients = sqliteTable(
	"project_recipients",
	{
		canonical_project_identity: text("canonical_project_identity").notNull(),
		recipient_kind: text("recipient_kind").notNull(),
		recipient_id: text("recipient_id").notNull(),
		status: text("status").notNull(),
		provenance: text("provenance").notNull(),
		policy_revision: text("policy_revision").notNull(),
		migration_state: text("migration_state").notNull(),
		source_fingerprint: text("source_fingerprint"),
		idempotency_key: text("idempotency_key").notNull().unique(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		primaryKey({
			columns: [table.canonical_project_identity, table.recipient_kind, table.recipient_id],
		}),
		index("idx_project_recipients_project_status").on(
			table.canonical_project_identity,
			table.status,
		),
	],
);

export type ProjectRecipient = typeof projectRecipients.$inferSelect;
export type NewProjectRecipient = typeof projectRecipients.$inferInsert;

export const recipientManagedProjectProjections = sqliteTable(
	"recipient_managed_project_projections",
	{
		canonical_project_identity: text("canonical_project_identity").notNull(),
		display_name: text("display_name").notNull(),
		managed_scope_id: text("managed_scope_id").notNull(),
		coordinator_id: text("coordinator_id").notNull(),
		group_id: text("group_id").notNull(),
		recipient_identity_id: text("recipient_identity_id").notNull(),
		accepting_device_id: text("accepting_device_id").notNull(),
		source_operation_id: text("source_operation_id").notNull(),
		reviewed_project_set_digest: text("reviewed_project_set_digest").notNull(),
		status: text("status").notNull().default("active"),
		accepted_at: text("accepted_at").notNull(),
		revoked_at: text("revoked_at"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		primaryKey({
			columns: [table.source_operation_id, table.canonical_project_identity],
		}),
		index("idx_recipient_managed_projects_identity_status").on(
			table.recipient_identity_id,
			table.status,
		),
		index("idx_recipient_managed_projects_scope_authority").on(
			table.managed_scope_id,
			table.coordinator_id,
			table.group_id,
			table.status,
		),
	],
);

export type RecipientManagedProjectProjection =
	typeof recipientManagedProjectProjections.$inferSelect;
export type NewRecipientManagedProjectProjection =
	typeof recipientManagedProjectProjections.$inferInsert;

export const recipientPolicyAuthorityStates = sqliteTable("recipient_policy_authority_states", {
	canonical_project_identity: text("canonical_project_identity").primaryKey(),
	authority_state: text("authority_state").notNull().default("legacy"),
	generation: integer("generation").notNull().default(0),
	desired_devices_digest: text("desired_devices_digest"),
	current_devices_digest: text("current_devices_digest"),
	stable_parity_evidence_digest: text("stable_parity_evidence_digest"),
	stable_parity_passed_at: text("stable_parity_passed_at"),
	fresh_snapshot_fingerprint: text("fresh_snapshot_fingerprint"),
	fresh_snapshot_observed_at: text("fresh_snapshot_observed_at"),
	safe_error_code: text("safe_error_code"),
	state_changed_at: text("state_changed_at").notNull(),
	last_error_at: text("last_error_at"),
	attempt_count: integer("attempt_count").notNull().default(0),
	last_attempt_at: text("last_attempt_at"),
	last_completed_at: text("last_completed_at"),
	lease_owner: text("lease_owner"),
	lease_acquired_at: text("lease_acquired_at"),
	lease_expires_at: text("lease_expires_at"),
	created_at: text("created_at").notNull(),
	updated_at: text("updated_at").notNull(),
});

export type RecipientPolicyAuthorityStateRow = typeof recipientPolicyAuthorityStates.$inferSelect;
export type NewRecipientPolicyAuthorityStateRow =
	typeof recipientPolicyAuthorityStates.$inferInsert;

export const recipientPolicyReconciliationSteps = sqliteTable(
	"recipient_policy_reconciliation_steps",
	{
		canonical_project_identity: text("canonical_project_identity").notNull(),
		generation: integer("generation").notNull(),
		step_key: text("step_key").notNull(),
		effect_id: text("effect_id").notNull(),
		payload_digest: text("payload_digest").notNull(),
		status: text("status").notNull().default("pending"),
		attempt_count: integer("attempt_count").notNull().default(0),
		started_at: text("started_at"),
		completed_at: text("completed_at"),
		last_attempt_at: text("last_attempt_at"),
		safe_error_code: text("safe_error_code"),
		error_at: text("error_at"),
		lease_owner: text("lease_owner"),
		lease_acquired_at: text("lease_acquired_at"),
		lease_expires_at: text("lease_expires_at"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		primaryKey({
			columns: [table.canonical_project_identity, table.generation, table.step_key],
		}),
		uniqueIndex("idx_recipient_policy_reconciliation_steps_effect").on(table.effect_id),
		index("idx_recipient_policy_reconciliation_steps_status").on(
			table.canonical_project_identity,
			table.status,
		),
		index("idx_recipient_policy_reconciliation_steps_pending_refresh")
			.on(table.canonical_project_identity, table.generation, table.step_key)
			.where(
				sql`${table.status} IN ('pending', 'running', 'failed') AND ${table.step_key} GLOB 'refresh-after-revocations-v2:*'`,
			),
	],
);

export type RecipientPolicyReconciliationStep =
	typeof recipientPolicyReconciliationSteps.$inferSelect;
export type NewRecipientPolicyReconciliationStep =
	typeof recipientPolicyReconciliationSteps.$inferInsert;

export const recipientPolicyDenyOverlays = sqliteTable(
	"recipient_policy_deny_overlays",
	{
		canonical_project_identity: text("canonical_project_identity").notNull(),
		scope_id: text("scope_id").notNull(),
		device_id: text("device_id").notNull(),
		generation: integer("generation").notNull(),
		reason_code: text("reason_code").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		primaryKey({
			columns: [table.canonical_project_identity, table.scope_id, table.device_id],
		}),
		index("idx_recipient_policy_deny_overlays_scope_device").on(table.scope_id, table.device_id),
	],
);

export type RecipientPolicyDenyOverlay = typeof recipientPolicyDenyOverlays.$inferSelect;
export type NewRecipientPolicyDenyOverlay = typeof recipientPolicyDenyOverlays.$inferInsert;

export const schema = {
	sessions,
	replicationScopes,
	projectScopeMappings,
	scopeMemberships,
	artifacts,
	memoryItems,
	memoryFileRefs,
	memoryConceptRefs,
	usageEvents,
	retrievalAttempts,
	retrievalExposures,
	outcomeEvidence,
	attributionAssessments,
	attributionAssessmentEvidence,
	rawEvents,
	rawEventIdentityConflicts,
	rawEventQuarantine,
	rawEventSessions,
	opencodeSessions,
	rawEventFlushBatches,
	processingResumeProducerReceipts,
	processingResumeSignals,
	providerHealthStates,
	userPrompts,
	sessionSummaries,
	replicationOps,
	replicationCursors,
	replicationCursorsV2,
	syncPeers,
	syncNonces,
	syncDevice,
	syncAttempts,
	syncScopeRejections,
	syncDaemonState,
	syncResetState,
	syncResetStateV2,
	syncRetentionStateV2,
	rawEventIngestSamples,
	rawEventIngestStats,
	coordinatorGroupPreferences,
	actors,
	recipientPolicyReviewResolutions,
	coordinatorEnrollmentReconciliationIssues,
	policyTeams,
	policyTeamMemberships,
	identityDevices,
	projectRecipients,
	recipientManagedProjectProjections,
	recipientPolicyAuthorityStates,
	recipientPolicyReconciliationSteps,
	recipientPolicyDenyOverlays,
};
