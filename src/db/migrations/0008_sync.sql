-- Device sync (contracts/sync.md "Schema"). Additive: no trigger and no column on the tracked
-- tables; sync_conflicts (0003) is reused as the conflict report. Heads are derived (a stored
-- revision with no stored child), never stored.

-- One row per configured space. The key itself lives only in $OBOETE_HOME/sync/<space_id>.key.
CREATE TABLE sync_spaces (
  space_id TEXT PRIMARY KEY,
  directory TEXT NOT NULL,
  directory_realpath TEXT NOT NULL,
  key_id TEXT NOT NULL,
  classes_json TEXT NOT NULL CHECK (json_valid(classes_json)),
  consent_hash TEXT NOT NULL,
  last_pushed_snapshot_id TEXT,
  published_sha256 TEXT,
  published_size INTEGER,
  created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE sync_cursors (
  space_id TEXT NOT NULL REFERENCES sync_spaces(space_id) ON DELETE CASCADE,
  replica_origin_id TEXT NOT NULL,
  snapshot_id TEXT NOT NULL,
  pulled_at INTEGER NOT NULL,
  PRIMARY KEY (space_id, replica_origin_id)
) STRICT;

-- origin_id <-> (kind, local id). Several origins may map to one local row (natural-key
-- aliasing); the row's selected head and materialized state live on its canonical origin, the
-- smallest origin_id mapped to it. local_id is NULL while the origin is withheld_on_apply.
CREATE TABLE sync_origins (
  origin_id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('memory', 'source', 'visibility', 'sharing_proposal', 'work', 'context')),
  local_id TEXT,
  natural_json TEXT NOT NULL CHECK (json_valid(natural_json)),
  canonical_origin_id TEXT NOT NULL,
  selected_head TEXT,
  materialized_revision TEXT,
  materialized_hash TEXT,
  withheld_reason TEXT
) STRICT;
CREATE INDEX sync_origins_local ON sync_origins(kind, local_id);
CREATE INDEX sync_origins_natural ON sync_origins(kind, natural_json);
CREATE INDEX sync_origins_canonical ON sync_origins(canonical_origin_id);

-- Identity fields verbatim as received or created; payload bytes only while no secret floor or
-- tombstone is stored for the origin.
CREATE TABLE sync_revisions (
  revision_id TEXT PRIMARY KEY,
  origin_id TEXT NOT NULL REFERENCES sync_origins(origin_id),
  kind TEXT NOT NULL,
  author TEXT NOT NULL,
  parents_json TEXT NOT NULL CHECK (json_valid(parents_json)),
  control_json TEXT NOT NULL CHECK (json_valid(control_json)),
  natural_json TEXT NOT NULL CHECK (json_valid(natural_json)),
  payload_hash TEXT,
  payload_json TEXT CHECK (payload_json IS NULL OR json_valid(payload_json)),
  -- A source revision's UNIQUE-tuple members (origin form): its payload's, or its parent's for a
  -- control revision; kept when the payload is erased, so a deletion still names its row.
  tuple_json TEXT CHECK (tuple_json IS NULL OR json_valid(tuple_json)),
  received_from TEXT,
  stored_at INTEGER NOT NULL
) STRICT;
CREATE INDEX sync_revisions_origin ON sync_revisions(origin_id);

-- A source row a pass set aside: its own head could not be applied yet and another head took its
-- UNIQUE tuple, so the row waits here as it was (the log never made the two the same) until a
-- pass can apply its head or put it back.
CREATE TABLE sync_parked (
  sync_key TEXT PRIMARY KEY,
  memory_id TEXT NOT NULL,
  row_json TEXT NOT NULL CHECK (json_valid(row_json))
) STRICT;

CREATE TABLE sync_revision_parents (
  child TEXT NOT NULL REFERENCES sync_revisions(revision_id),
  parent TEXT NOT NULL,
  PRIMARY KEY (child, parent)
) STRICT;
CREATE INDEX sync_revision_parents_parent ON sync_revision_parents(parent);

-- Repository keys are fixed-size (contracts/sync.md "Repository identity"); the full identity
-- travels on the repo line and is kept here. local_repo_id is NULL until a common_dir key of
-- another replica is mapped by `oboete sync map-repo`.
CREATE TABLE sync_repo_mappings (
  repo_key TEXT PRIMARY KEY,
  identity_kind TEXT NOT NULL CHECK (identity_kind IN ('remote', 'common_dir')),
  normalized_identity TEXT NOT NULL,
  local_repo_id TEXT REFERENCES repos(id)
) STRICT;

-- A source row's sync identity: assigned at its first capture (the hash of its fields and its
-- memory's material then, so an identical row, re-inserted here or captured independently on
-- another device, takes the same key), carried by the origin's natural key, never recomputed.
-- memory_sources.id is a reusable rowid and never crosses devices.
ALTER TABLE memory_sources ADD COLUMN sync_key TEXT;
CREATE UNIQUE INDEX memory_sources_sync_key ON memory_sources (sync_key) WHERE sync_key IS NOT NULL;

-- The local approval record: what this device's user approved, so a pulled approval only keeps
-- a projection when it approves exactly the same candidate, projection and scope.
CREATE TABLE sync_approvals (
  proposal_id TEXT PRIMARY KEY REFERENCES sharing_proposals(id),
  candidate_hash TEXT NOT NULL,
  projection_hash TEXT,
  scope_json TEXT NOT NULL CHECK (json_valid(scope_json)),
  approved_at INTEGER NOT NULL
) STRICT;
