-- memories, memory_sources, memories_fts, memories_fts_cjk, destination_rules

CREATE TABLE memories (
  rid INTEGER PRIMARY KEY,
  id TEXT NOT NULL UNIQUE,
  repo_id TEXT NOT NULL REFERENCES repos(id),
  type TEXT NOT NULL CHECK (type IN (
    'bugfix', 'feature', 'refactor', 'change', 'discovery', 'decision',
    'security_alert', 'security_note', 'session_summary'
  )),
  title TEXT,
  body TEXT,
  concepts TEXT,
  cjk_bigrams TEXT,
  material_hash TEXT,
  content_hash TEXT NOT NULL UNIQUE,
  sensitivity TEXT NOT NULL CHECK (sensitivity IN (
    'local_only', 'eligible', 'secret', 'private'
  )),
  review_state TEXT NOT NULL DEFAULT 'unreviewed' CHECK (review_state IN (
    'unreviewed', 'reviewed', 'imported'
  )),
  degraded_reason TEXT CHECK (degraded_reason IN (
    'no_provider', 'unreachable', 'unusable_output', 'language_mismatch',
    'daily_cap', 'provider_exhausted', 'provider_paid', 'auth_failed',
    'consent_changed', 'model_alias', 'timeout', 'rule_based'
  )),
  source_session_id TEXT,
  source_batch_id TEXT,
  valid_from INTEGER,
  valid_to INTEGER,
  superseded_by TEXT,
  pinned_at INTEGER,
  pin_order INTEGER,
  last_injected_at INTEGER,
  citations_head TEXT,
  citations_ok INTEGER,
  deleted_at INTEGER,
  created_at INTEGER
) STRICT;

CREATE INDEX memories_repo_id_deleted_at_pinned_at ON memories (repo_id, deleted_at, pinned_at);
CREATE INDEX memories_repo_id_valid_to ON memories (repo_id, valid_to);
CREATE INDEX memories_repo_id_review_state ON memories (repo_id, review_state);

CREATE TABLE memory_sources (
  id INTEGER PRIMARY KEY,
  memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
  raw_event_id TEXT,
  citation_kind TEXT CHECK (citation_kind IN ('file_read', 'file_modified', 'commit')),
  citation_value TEXT,
  source_agent TEXT
) STRICT;

CREATE INDEX memory_sources_memory_id ON memory_sources (memory_id);

CREATE VIRTUAL TABLE memories_fts USING fts5(
  title,
  body,
  content='memories',
  content_rowid='rowid',
  tokenize='trigram'
);

CREATE VIRTUAL TABLE memories_fts_cjk USING fts5(
  cjk_bigrams,
  content='memories',
  content_rowid='rowid',
  tokenize='unicode61'
);

CREATE TRIGGER memories_fts_ai AFTER INSERT ON memories BEGIN
  INSERT INTO memories_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
END;

CREATE TRIGGER memories_fts_ad AFTER DELETE ON memories BEGIN
  INSERT INTO memories_fts(memories_fts, rowid, title, body)
    VALUES ('delete', old.rowid, old.title, old.body);
END;

-- Only the indexed columns re-tokenize: markInjected and pin run on the 300 ms hook path.
CREATE TRIGGER memories_fts_au AFTER UPDATE OF title, body ON memories BEGIN
  INSERT INTO memories_fts(memories_fts, rowid, title, body)
    VALUES ('delete', old.rowid, old.title, old.body);
  INSERT INTO memories_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
END;

CREATE TRIGGER memories_fts_cjk_ai AFTER INSERT ON memories BEGIN
  INSERT INTO memories_fts_cjk(rowid, cjk_bigrams) VALUES (new.rowid, new.cjk_bigrams);
END;

CREATE TRIGGER memories_fts_cjk_ad AFTER DELETE ON memories BEGIN
  INSERT INTO memories_fts_cjk(memories_fts_cjk, rowid, cjk_bigrams)
    VALUES ('delete', old.rowid, old.cjk_bigrams);
END;

CREATE TRIGGER memories_fts_cjk_au AFTER UPDATE OF cjk_bigrams ON memories BEGIN
  INSERT INTO memories_fts_cjk(memories_fts_cjk, rowid, cjk_bigrams)
    VALUES ('delete', old.rowid, old.cjk_bigrams);
  INSERT INTO memories_fts_cjk(rowid, cjk_bigrams) VALUES (new.rowid, new.cjk_bigrams);
END;

CREATE TABLE destination_rules (
  destination TEXT NOT NULL CHECK (destination IN (
    'remote_observer', 'local_observer', 'injection', 'sync'
  )),
  sensitivity TEXT NOT NULL CHECK (sensitivity IN (
    'local_only', 'eligible', 'secret', 'private'
  )),
  allowed INTEGER NOT NULL,
  same_repo_required INTEGER NOT NULL,
  PRIMARY KEY (destination, sensitivity)
) STRICT;

INSERT INTO destination_rules (destination, sensitivity, allowed, same_repo_required) VALUES
  ('remote_observer', 'eligible', 1, 0),
  ('remote_observer', 'local_only', 0, 0),
  ('remote_observer', 'private', 0, 0),
  ('remote_observer', 'secret', 0, 0),
  ('local_observer', 'eligible', 1, 1),
  ('local_observer', 'local_only', 1, 1),
  ('local_observer', 'private', 1, 1),
  ('local_observer', 'secret', 0, 0),
  ('injection', 'eligible', 1, 1),
  ('injection', 'local_only', 1, 1),
  ('injection', 'private', 1, 1),
  ('injection', 'secret', 0, 0),
  ('sync', 'eligible', 1, 0),
  ('sync', 'local_only', 1, 0),
  ('sync', 'private', 1, 0);
