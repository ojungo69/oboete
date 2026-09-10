CREATE TABLE replica_identity (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  origin_id TEXT NOT NULL UNIQUE
) STRICT;
-- Public, random 128-bit namespace; this is an identifier, never an authentication secret.
INSERT INTO replica_identity (id, origin_id) VALUES (1, lower(hex(randomblob(16))));

CREATE TABLE migration_imports (
  id TEXT PRIMARY KEY,
  source_format TEXT NOT NULL,
  source_revision TEXT NOT NULL,
  source_sha256 TEXT NOT NULL,
  exported_at INTEGER,
  mapping_json TEXT NOT NULL CHECK (json_valid(mapping_json)),
  mapping_hash TEXT NOT NULL,
  counts_json TEXT NOT NULL CHECK (json_valid(counts_json)),
  imported_at INTEGER NOT NULL,
  UNIQUE (source_format, source_revision, source_sha256)
) STRICT;

-- Imported payload is private quarantine provenance. It never participates in FTS or ordinary
-- readers; an existing live memory receives only this separate receipt, never unclassified sources.
CREATE TABLE migration_records (
  id TEXT PRIMARY KEY,
  origin_key TEXT NOT NULL UNIQUE,
  origin_json TEXT NOT NULL CHECK (json_valid(origin_json)),
  target_key TEXT NOT NULL,
  first_import_id TEXT NOT NULL REFERENCES migration_imports(id),
  record_kind TEXT NOT NULL CHECK (record_kind IN (
    'memory', 'source', 'visibility', 'sharing_proposal', 'work', 'context', 'session', 'prompt', 'excluded'
  )),
  payload_hash TEXT NOT NULL,
  payload_json TEXT CHECK (payload_json IS NULL OR json_valid(payload_json)),
  destination_repo_id TEXT REFERENCES repos(id),
  destination_memory_id TEXT REFERENCES memories(id),
  identity_domain TEXT CHECK (identity_domain IN ('ordinary', 'personal_projection')),
  destination_context_id TEXT REFERENCES work_contexts(id),
  destination_context_key TEXT,
  effect TEXT NOT NULL CHECK (effect IN (
    'inserted', 'matched_existing', 'held_by_tombstone', 'historical_held', 'support_only', 'excluded'
  )),
  classification_state TEXT NOT NULL CHECK (classification_state IN ('pending', 'clean', 'secret', 'not_applicable')),
  detail_code TEXT NOT NULL,
  promoted_proposal_id TEXT REFERENCES sharing_proposals(id)
) STRICT;
CREATE INDEX migration_records_memory ON migration_records(destination_memory_id);
CREATE INDEX migration_records_classification ON migration_records(classification_state, id);
CREATE INDEX migration_records_import ON migration_records(first_import_id, record_kind);

CREATE TRIGGER migration_clear_deleted_payload AFTER UPDATE OF deleted_at, sensitivity ON memories
  WHEN NEW.deleted_at IS NOT NULL OR NEW.sensitivity = 'secret' BEGIN
  UPDATE migration_records SET payload_json = NULL,
    classification_state = CASE WHEN NEW.sensitivity = 'secret' THEN 'secret' ELSE 'not_applicable' END,
    detail_code = CASE WHEN NEW.sensitivity = 'secret' THEN 'source_secret' ELSE 'source_deleted' END
    WHERE destination_memory_id = NEW.id;
END;
