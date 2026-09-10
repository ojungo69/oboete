-- Accepted source processing is independent of temporary batch output and secret classification.
ALTER TABLE raw_events ADD COLUMN processing_state TEXT NOT NULL DEFAULT 'pending'
  CHECK (processing_state IN ('pending', 'waiting', 'processed', 'excluded', 'legacy_unknown'));
ALTER TABLE raw_events ADD COLUMN processed_at INTEGER;
ALTER TABLE raw_events ADD COLUMN retry_after INTEGER;
ALTER TABLE raw_events ADD COLUMN processing_attempts INTEGER NOT NULL DEFAULT 0
  CHECK (processing_attempts >= 0);
ALTER TABLE raw_events ADD COLUMN processing_offset INTEGER NOT NULL DEFAULT 0
  CHECK (processing_offset >= 0);
ALTER TABLE raw_events ADD COLUMN processing_hash TEXT;
ALTER TABLE sessions ADD COLUMN summary_updated_at INTEGER;
ALTER TABLE sessions ADD COLUMN summary_degraded_reason TEXT;
UPDATE sessions SET summary_degraded_reason = (
  SELECT degraded_reason FROM memories WHERE id = sessions.latest_summary_memory_id
);

CREATE INDEX raw_events_processing_due ON raw_events (processing_state, retry_after)
  WHERE batch_id IS NULL;
CREATE INDEX raw_events_processing_order ON raw_events (session_id, COALESCE(captured_at, 0), id)
  WHERE batch_id IS NULL;

CREATE TABLE observation_batch_sources (
  batch_id TEXT NOT NULL REFERENCES observation_batches(id) ON DELETE CASCADE,
  raw_event_id TEXT NOT NULL,
  turn_id TEXT,
  outcome TEXT NOT NULL CHECK (outcome IN (
    'assigned', 'processed', 'deferred', 'uncovered', 'rejected', 'legacy_unknown'
  )),
  reason TEXT,
  recorded_at INTEGER NOT NULL,
  portion_start INTEGER,
  portion_end INTEGER,
  source_total INTEGER,
  source_hash TEXT,
  historical_actions_json TEXT,
  PRIMARY KEY (batch_id, raw_event_id)
) STRICT;

CREATE INDEX observation_batch_sources_raw_event ON observation_batch_sources (raw_event_id);

ALTER TABLE memory_sources ADD COLUMN portion_start INTEGER;
ALTER TABLE memory_sources ADD COLUMN portion_end INTEGER;
ALTER TABLE memory_sources ADD COLUMN source_total INTEGER;
ALTER TABLE memory_sources ADD COLUMN source_hash TEXT;
ALTER TABLE memory_sources ADD COLUMN evidence TEXT;
ALTER TABLE memory_sources ADD COLUMN captured_at INTEGER;
ALTER TABLE memory_sources ADD COLUMN source_processed_at INTEGER;
ALTER TABLE memory_sources ADD COLUMN capture_root TEXT;
ALTER TABLE memory_sources ADD COLUMN source_paths_json TEXT;
CREATE UNIQUE INDEX memory_sources_portion ON memory_sources
  (memory_id, raw_event_id, source_hash, portion_start, portion_end) WHERE source_hash IS NOT NULL;
CREATE INDEX memory_sources_raw_event ON memory_sources (raw_event_id) WHERE raw_event_id IS NOT NULL;
CREATE TRIGGER memories_clear_source_evidence AFTER UPDATE OF deleted_at ON memories
  WHEN NEW.deleted_at IS NOT NULL BEGIN
  UPDATE memory_sources SET evidence = NULL, capture_root = NULL, source_paths_json = NULL, citation_value = NULL WHERE memory_id = NEW.id;
END;
ALTER TABLE memories ADD COLUMN source_captured_at INTEGER;
UPDATE memories SET source_captured_at = (
  SELECT MAX(r.captured_at) FROM memory_sources ms JOIN raw_events r ON r.id = ms.raw_event_id
  WHERE ms.memory_id = memories.id
);

-- Old terminal batches did not prove source coverage. Preserve their surviving raw material
-- without starting an unexpected historical provider backlog during an upgrade.
UPDATE raw_events SET processing_state = 'legacy_unknown'
  WHERE batch_id IN (SELECT id FROM observation_batches WHERE state IN ('applied', 'fallback'));

INSERT INTO observation_batch_sources (batch_id, raw_event_id, turn_id, outcome, reason, recorded_at)
  SELECT r.batch_id, r.id, r.turn_id,
    CASE WHEN r.processing_state = 'legacy_unknown' THEN 'legacy_unknown' ELSE 'assigned' END,
    CASE WHEN r.processing_state = 'legacy_unknown' THEN 'legacy_processing_unknown' ELSE NULL END,
    COALESCE(b.completed_at, b.claimed_at, r.captured_at, 0)
  FROM raw_events r JOIN observation_batches b ON b.id = r.batch_id;
