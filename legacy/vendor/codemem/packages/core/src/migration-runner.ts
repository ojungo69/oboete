import {
	assertSchemaReady,
	ensureAdditiveSchemaCompatibility,
	getSchemaVersion,
	isSchemaCompatibilityCurrent,
	MIN_WRITABLE_SCHEMA,
	migrateV20ToV21,
	SCHEMA_VERSION,
	tableExists,
	V21_MIGRATION_SOURCE_SCHEMA,
} from "./db.js";
import { bootstrapSchema, canAutoBootstrapSchema } from "./schema-bootstrap.js";
import type { WriterActor } from "./writer-actor.js";

export interface MigrationBackupContext {
	db: WriterActor;
	dbPath: string;
	schemaVersion: number;
	kind: "bootstrap" | "upgrade";
}

export interface MigrationBackupVerification {
	verified: boolean;
	evidence: string;
}

export type MigrationBackupVerifier = (
	context: MigrationBackupContext,
) => MigrationBackupVerification;

export interface RunDatabaseMigrationsOptions {
	dbPath: string;
	backupAndVerify: MigrationBackupVerifier;
}

export function peekMigrationKind(db: WriterActor): "bootstrap" | "upgrade" | null {
	const version = getSchemaVersion(db);
	if (canAutoBootstrapSchema(db)) return "bootstrap";
	if (version === 0) {
		throw new Error("Refusing to migrate a non-empty database without a codemem schema.");
	}
	if (!tableExists(db, "memory_items") || !tableExists(db, "sessions")) {
		throw new Error("Refusing to migrate an unrecognized or partial codemem database.");
	}
	if (version > SCHEMA_VERSION) return null;
	if (version < MIN_WRITABLE_SCHEMA) {
		throw new Error(
			`Direct writable upgrade to schema ${SCHEMA_VERSION} requires schema ${MIN_WRITABLE_SCHEMA}.`,
		);
	}
	if (version === V21_MIGRATION_SOURCE_SCHEMA) return "upgrade";
	if (isSchemaCompatibilityCurrent(db)) return null;
	return "upgrade";
}

/** Run schema changes only after the supplied backup proof succeeds. */
export function runDatabaseMigrations(
	db: WriterActor,
	options: RunDatabaseMigrationsOptions,
): void {
	const kind = peekMigrationKind(db);
	if (!kind) return;

	const verification = options.backupAndVerify({
		db,
		dbPath: options.dbPath,
		schemaVersion: getSchemaVersion(db),
		kind,
	});
	if (!verification.verified || !verification.evidence.trim()) {
		throw new Error("Database migration requires a verified backup before schema changes begin.");
	}

	if (kind === "bootstrap") {
		bootstrapSchema(db);
		ensureAdditiveSchemaCompatibility(db);
	} else if (getSchemaVersion(db) === V21_MIGRATION_SOURCE_SCHEMA) {
		migrateV20ToV21(db);
	} else if (getSchemaVersion(db) === SCHEMA_VERSION) {
		ensureAdditiveSchemaCompatibility(db);
	} else {
		throw new Error("Unsupported database migration path.");
	}
	assertSchemaReady(db);
}

/** Fresh empty databases have no prior state to preserve. */
export const verifyFreshDatabase: MigrationBackupVerifier = ({ db }) => {
	const verified = canAutoBootstrapSchema(db);
	return { verified, evidence: verified ? "fresh-empty-database" : "" };
};
