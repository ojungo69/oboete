-- schema_migrations, repos, sessions, turns, raw_events, observation_batches, worker_lease, runtime_state, diagnostics

CREATE TABLE schema_migrations (
  version INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  applied_at INTEGER NOT NULL
) STRICT;

CREATE TABLE repos (
  id TEXT PRIMARY KEY,
  identity_kind TEXT NOT NULL CHECK (identity_kind IN ('remote', 'common_dir')),
  normalized_identity TEXT NOT NULL UNIQUE,
  display_root TEXT,
  created_at INTEGER,
  last_seen_at INTEGER
) STRICT;

CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  repo_id TEXT NOT NULL REFERENCES repos(id),
  agent TEXT NOT NULL CHECK (agent IN ('claude', 'codex', 'grok', 'pi', 'unknown')),
  native_session_id TEXT NOT NULL,
  conversation_id TEXT NOT NULL,
  model TEXT,
  started_at INTEGER,
  ended_at INTEGER,
  status TEXT NOT NULL CHECK (status IN ('active', 'ended')),
  turn_count INTEGER NOT NULL DEFAULT 0,
  latest_summary_memory_id TEXT,
  context_epoch INTEGER NOT NULL DEFAULT 0,
  last_compaction_key TEXT,
  summary_state TEXT CHECK (summary_state IN ('pending', 'done', 'no_content')),
  UNIQUE (agent, native_session_id)
) STRICT;

CREATE TABLE turns (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  ordinal INTEGER NOT NULL,
  started_at INTEGER,
  ended_at INTEGER,
  UNIQUE (session_id, ordinal)
) STRICT;

CREATE TABLE raw_events (
  id TEXT PRIMARY KEY,
  repo_id TEXT NOT NULL REFERENCES repos(id),
  session_id TEXT NOT NULL REFERENCES sessions(id),
  turn_id TEXT,
  agent TEXT CHECK (agent IN ('claude', 'codex', 'grok', 'pi', 'unknown')),
  kind TEXT NOT NULL CHECK (kind IN (
    'session_start', 'prompt', 'tool_call', 'tool_result', 'tool_failure',
    'turn_end', 'session_end', 'compaction_summary', 'last_assistant_message', 'probe'
  )),
  content TEXT,
  truncated INTEGER,
  payload_json TEXT,
  content_hash TEXT,
  sensitivity TEXT NOT NULL DEFAULT 'local_only' CHECK (sensitivity IN (
    'local_only', 'eligible', 'secret', 'private'
  )),
  classification_state TEXT CHECK (classification_state IN (
    'pending', 'done', 'partial', 'failed'
  )),
  captured_at INTEGER,
  expires_at INTEGER,
  batch_id TEXT,
  via_spool INTEGER
) STRICT;

CREATE INDEX raw_events_session_id_captured_at ON raw_events (session_id, captured_at);
CREATE INDEX raw_events_expires_at ON raw_events (expires_at);
CREATE INDEX raw_events_batch_id ON raw_events (batch_id);

CREATE TABLE observation_batches (
  id TEXT PRIMARY KEY,
  repo_id TEXT,
  session_id TEXT NOT NULL,
  through_event_id TEXT NOT NULL,
  destination TEXT NOT NULL CHECK (destination IN (
    'remote_observer', 'local_observer', 'fallback'
  )),
  trigger TEXT CHECK (trigger IN ('ten_turns', 'session_end', 'retention')),
  state TEXT CHECK (state IN ('pending', 'running', 'applied', 'fallback')),
  owner_token TEXT,
  provider_attempts INTEGER,
  last_reservation_id TEXT,
  degraded_reason TEXT CHECK (degraded_reason IN (
    'no_provider', 'unreachable', 'unusable_output', 'language_mismatch',
    'daily_cap', 'provider_exhausted', 'provider_paid', 'auth_failed',
    'consent_changed', 'model_alias', 'timeout', 'rule_based'
  )),
  excerpted INTEGER,
  claimed_at INTEGER,
  completed_at INTEGER,
  UNIQUE (session_id, through_event_id, destination)
) STRICT;

CREATE TABLE worker_lease (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  owner_token TEXT,
  pid INTEGER,
  started_at INTEGER,
  heartbeat_at INTEGER
) STRICT;

INSERT INTO worker_lease (id, owner_token, pid, started_at, heartbeat_at)
VALUES (1, NULL, NULL, NULL, NULL);

CREATE TABLE runtime_state (
  key TEXT PRIMARY KEY,
  value_json TEXT,
  updated_at INTEGER
) STRICT;

CREATE TABLE diagnostics (
  id TEXT PRIMARY KEY,
  kind TEXT,
  severity TEXT,
  agent TEXT,
  message_code TEXT,
  details_json TEXT,
  count INTEGER,
  first_seen_at INTEGER,
  last_seen_at INTEGER,
  cleared_at INTEGER
) STRICT;
