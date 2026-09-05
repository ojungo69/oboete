-- injections, injection_items, provider_usage, sync_conflicts

CREATE TABLE injections (
  id TEXT PRIMARY KEY,
  repo_id TEXT,
  session_id TEXT,
  conversation_id TEXT,
  turn_id TEXT,
  kind TEXT NOT NULL CHECK (kind IN ('session_start', 'prompt', 'grok_deferred')),
  channel TEXT,
  state TEXT CHECK (state IN ('built', 'emitted', 'omitted', 'pending', 'attempted')),
  context_epoch INTEGER,
  attempts_json TEXT,
  delivery_count INTEGER,
  pack_hash TEXT,
  char_budget INTEGER,
  chars_used INTEGER,
  degraded_reason TEXT CHECK (degraded_reason IN (
    'summary_pending', 'index_unavailable', 'empty', 'window_unknown',
    'no_tool_call', 'not_delivered',
    'no_provider', 'unreachable', 'unusable_output', 'language_mismatch',
    'daily_cap', 'provider_exhausted', 'provider_paid', 'auth_failed',
    'consent_changed', 'model_alias', 'timeout', 'rule_based'
  )),
  created_at INTEGER,
  attempted_at INTEGER,
  emitted_at INTEGER
) STRICT;

CREATE TABLE injection_items (
  id INTEGER PRIMARY KEY,
  injection_id TEXT NOT NULL REFERENCES injections(id) ON DELETE CASCADE,
  conversation_id TEXT,
  context_epoch INTEGER,
  source_kind TEXT CHECK (source_kind IN ('memory', 'raw_activity', 'session_summary')),
  memory_id TEXT,
  raw_event_id TEXT,
  decision TEXT CHECK (decision IN ('planned', 'included', 'omitted')),
  reason TEXT CHECK (reason IN (
    'below_threshold', 'budget', 'duplicate_in_conversation', 'stale_path',
    'stale_commit', 'retired', 'mmr_redundant', 'pinned', 'summary', 'not_delivered',
    'secret_detected', 'directive'
  )),
  rank INTEGER,
  score_bm25 REAL,
  score_rrf REAL,
  score_mmr REAL,
  stale INTEGER
) STRICT;

CREATE UNIQUE INDEX injection_items_included_memory
  ON injection_items (conversation_id, context_epoch, memory_id)
  WHERE decision = 'included' AND memory_id IS NOT NULL;

CREATE TABLE provider_usage (
  utc_day TEXT NOT NULL,
  preset TEXT NOT NULL,
  calls INTEGER,
  neurons_estimate REAL,
  reset_at INTEGER,
  exhausted_at INTEGER,
  exhausted_reservation_id TEXT,
  resolved_model TEXT,
  PRIMARY KEY (utc_day, preset)
) STRICT;

CREATE TABLE sync_conflicts (
  id TEXT PRIMARY KEY,
  repo_id TEXT,
  content_hash TEXT,
  local_state_json TEXT,
  remote_state_json TEXT,
  status TEXT,
  created_at INTEGER
) STRICT;
