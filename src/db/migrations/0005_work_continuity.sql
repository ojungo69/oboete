-- Keep the old global UNIQUE and its foreign-key graph. Only cross-repository native-ID
-- collisions need an internal storage ID; readers use the original agent-provided value.
ALTER TABLE sessions ADD COLUMN original_native_session_id TEXT;
CREATE UNIQUE INDEX sessions_repo_native ON sessions
  (repo_id, agent, COALESCE(original_native_session_id, native_session_id));
ALTER TABLE sessions ADD COLUMN last_captured_at INTEGER;
UPDATE sessions SET last_captured_at = MAX(COALESCE(started_at, 0), COALESCE(ended_at, 0),
  COALESCE((SELECT MAX(r.captured_at) FROM raw_events r WHERE r.session_id = sessions.id), 0));

CREATE TABLE work_contexts (
  id TEXT PRIMARY KEY,
  repo_id TEXT NOT NULL REFERENCES repos(id),
  local_key TEXT NOT NULL,
  root TEXT NOT NULL,
  repo_secret_paths_json TEXT CHECK (repo_secret_paths_json IS NULL OR json_valid(repo_secret_paths_json)),
  created_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  UNIQUE (repo_id, local_key)
) STRICT;

CREATE TABLE work_items (
  id TEXT PRIMARY KEY,
  repo_id TEXT NOT NULL REFERENCES repos(id),
  origin_context_id TEXT NOT NULL REFERENCES work_contexts(id),
  purpose TEXT,
  purpose_source_event_id TEXT,
  purpose_sensitivity TEXT NOT NULL DEFAULT 'local_only'
    CHECK (purpose_sensitivity IN ('eligible', 'local_only', 'private', 'secret')),
  state TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'dormant', 'completed')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  completed_at INTEGER,
  current_checkpoint_memory_id TEXT REFERENCES memories(id)
) STRICT;
CREATE INDEX work_items_context_state ON work_items (origin_context_id, state, created_at);

CREATE TABLE work_bindings (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  context_id TEXT NOT NULL REFERENCES work_contexts(id),
  work_id TEXT REFERENCES work_items(id),
  candidates_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(candidates_json)),
  created_at INTEGER NOT NULL,
  closed_at INTEGER,
  reason TEXT NOT NULL CHECK (reason IN ('new_context', 'only_active', 'new_purpose', 'ambiguous', 'explicit', 'late_source'))
) STRICT;
CREATE UNIQUE INDEX work_bindings_current ON work_bindings (session_id) WHERE closed_at IS NULL;
CREATE INDEX work_bindings_context ON work_bindings (context_id, closed_at);
CREATE INDEX work_bindings_work ON work_bindings (work_id, created_at);

-- Older deferred packs had no work binding. Their text cannot become current progress on upgrade.
UPDATE injection_items SET decision = 'omitted', reason = 'not_delivered'
  WHERE injection_id IN (SELECT id FROM injections WHERE state IN ('pending', 'attempted')) AND decision = 'planned';
UPDATE injections SET state = 'omitted', degraded_reason = 'not_delivered' WHERE state IN ('pending', 'attempted');
DELETE FROM runtime_state WHERE key LIKE 'injection_pending:%';

ALTER TABLE raw_events ADD COLUMN work_binding_id TEXT REFERENCES work_bindings(id);
ALTER TABLE memory_sources ADD COLUMN source_context_id TEXT REFERENCES work_contexts(id);
ALTER TABLE memory_sources ADD COLUMN context_only INTEGER NOT NULL DEFAULT 0 CHECK (context_only IN (0, 1));
ALTER TABLE memory_sources ADD COLUMN source_memory_id TEXT REFERENCES memories(id)
  CHECK (source_memory_id IS NULL OR (context_only = 1 AND source_memory_id <> memory_id));
CREATE INDEX memory_sources_source_memory ON memory_sources (source_memory_id) WHERE source_memory_id IS NOT NULL;
CREATE UNIQUE INDEX memory_sources_context_memory ON memory_sources (memory_id, source_memory_id)
  WHERE context_only = 1 AND source_memory_id IS NOT NULL;
CREATE UNIQUE INDEX memory_sources_context_raw ON memory_sources (memory_id, raw_event_id)
  WHERE context_only = 1 AND raw_event_id IS NOT NULL;
CREATE INDEX raw_events_work_binding ON raw_events (work_binding_id, captured_at, id);
ALTER TABLE observation_batches ADD COLUMN work_binding_id TEXT REFERENCES work_bindings(id);
ALTER TABLE observation_batches ADD COLUMN checkpoint_decision TEXT;
ALTER TABLE observation_batches ADD COLUMN checkpoint_parent_id TEXT REFERENCES memories(id);
ALTER TABLE observation_batches ADD COLUMN checkpoint_memory_id TEXT REFERENCES memories(id);
ALTER TABLE observation_batches ADD COLUMN checkpoint_reason TEXT;
ALTER TABLE observation_batches ADD COLUMN checkpoint_source_ids_json TEXT;
ALTER TABLE memories ADD COLUMN work_id TEXT REFERENCES work_items(id);
ALTER TABLE memories ADD COLUMN checkpoint_parent_id TEXT REFERENCES memories(id);
ALTER TABLE memories ADD COLUMN provenance_complete INTEGER CHECK (provenance_complete IN (0, 1));
CREATE INDEX memories_work_material ON memories (work_id, material_hash);
CREATE INDEX memories_checkpoint_parent ON memories (checkpoint_parent_id) WHERE checkpoint_parent_id IS NOT NULL;
CREATE TRIGGER memories_checkpoint_parent_immutable BEFORE UPDATE OF work_id, checkpoint_parent_id ON memories
  WHEN OLD.work_id IS NOT NULL AND (OLD.work_id IS NOT NEW.work_id OR OLD.checkpoint_parent_id IS NOT NEW.checkpoint_parent_id) BEGIN
  SELECT RAISE(ABORT, 'checkpoint parent is immutable');
END;
-- One recursive statement covers the whole lineage even with recursive_triggers disabled.
CREATE TRIGGER memories_provenance_privacy AFTER UPDATE OF sensitivity ON memories
  WHEN OLD.sensitivity <> NEW.sensitivity BEGIN
  UPDATE memories SET sensitivity = NEW.sensitivity,
    review_state = CASE WHEN NEW.sensitivity = 'secret' THEN 'imported' ELSE review_state END,
    provenance_complete = CASE WHEN NEW.sensitivity = 'secret' THEN 0 ELSE provenance_complete END
    WHERE id IN (WITH RECURSIVE descendants(id) AS (
      SELECT NEW.id UNION SELECT m.id FROM memories m JOIN descendants d ON m.checkpoint_parent_id = d.id
      UNION SELECT ms.memory_id FROM memory_sources ms JOIN descendants d ON ms.source_memory_id = d.id
    ) SELECT id FROM descendants) AND (
      NEW.sensitivity = 'secret' OR
      (NEW.sensitivity = 'private' AND sensitivity IN ('eligible', 'local_only')) OR
      (NEW.sensitivity = 'local_only' AND sensitivity = 'eligible'));
  UPDATE memory_sources SET evidence = NULL, capture_root = NULL, source_paths_json = NULL, citation_value = NULL
    WHERE NEW.sensitivity = 'secret' AND memory_id IN (WITH RECURSIVE descendants(id) AS (
      SELECT NEW.id UNION SELECT m.id FROM memories m JOIN descendants d ON m.checkpoint_parent_id = d.id
      UNION SELECT ms.memory_id FROM memory_sources ms JOIN descendants d ON ms.source_memory_id = d.id
    ) SELECT id FROM descendants);
END;
CREATE TRIGGER memories_checkpoint_tombstone AFTER UPDATE OF deleted_at ON memories
  WHEN NEW.work_id IS NOT NULL AND NEW.deleted_at IS NOT NULL AND OLD.deleted_at IS NULL BEGIN
  UPDATE memories SET deleted_at = NEW.deleted_at WHERE work_id = NEW.work_id
    AND material_hash = NEW.material_hash AND deleted_at IS NULL;
END;

-- Old attempts have no proven purpose boundary. Keep their accepted sources for explicit choice.
UPDATE observation_batch_sources SET outcome = 'deferred', reason = 'work_selection_required'
  WHERE outcome = 'assigned' AND batch_id IN (SELECT id FROM observation_batches WHERE state IN ('pending', 'running'));
UPDATE raw_events SET batch_id = NULL WHERE batch_id IN
  (SELECT id FROM observation_batches WHERE state IN ('pending', 'running'));
UPDATE observation_batches SET state = 'fallback', degraded_reason = 'rule_based'
  WHERE state IN ('pending', 'running');
UPDATE raw_events SET processing_state = 'waiting', retry_after = NULL
  WHERE processing_state IN ('pending', 'waiting') AND kind IN
    ('prompt', 'tool_call', 'tool_result', 'tool_failure', 'last_assistant_message', 'compaction_summary');

CREATE TRIGGER raw_events_clear_work_purpose AFTER UPDATE OF sensitivity ON raw_events
  WHEN NEW.sensitivity = 'secret' BEGIN
  UPDATE work_items SET purpose = NULL, purpose_sensitivity = 'secret'
    WHERE purpose_source_event_id = NEW.id;
END;
