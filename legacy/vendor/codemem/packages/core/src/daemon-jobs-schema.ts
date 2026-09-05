export const DAEMON_JOBS_DDL = `
CREATE TABLE IF NOT EXISTS daemon_jobs (
	job_id TEXT PRIMARY KEY NOT NULL,
	kind TEXT NOT NULL,
	args_json TEXT NOT NULL,
	dry_run INTEGER NOT NULL DEFAULT 0,
	state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'completed', 'failed')),
	attempts INTEGER NOT NULL DEFAULT 0,
	max_attempts INTEGER NOT NULL DEFAULT 1 CHECK (max_attempts = 1),
	result_json TEXT,
	error_code TEXT,
	submitted_at TEXT NOT NULL,
	started_at TEXT,
	finished_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_daemon_jobs_kind_state_submitted
	ON daemon_jobs(kind, state, submitted_at DESC);
`;
