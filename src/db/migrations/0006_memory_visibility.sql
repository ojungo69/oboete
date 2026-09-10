CREATE TABLE sharing_proposals (
  id TEXT PRIMARY KEY,
  origin_memory_id TEXT NOT NULL REFERENCES memories(id),
  origin_repo_id TEXT NOT NULL REFERENCES repos(id),
  origin_work_id TEXT NOT NULL REFERENCES work_items(id),
  candidate_title TEXT NOT NULL,
  candidate_body TEXT NOT NULL,
  candidate_material_hash TEXT NOT NULL,
  candidate_sensitivity TEXT NOT NULL CHECK (candidate_sensitivity IN ('eligible', 'local_only', 'private', 'secret')),
  source_event_ids_json TEXT NOT NULL CHECK (json_valid(source_event_ids_json)),
  basis TEXT NOT NULL CHECK (basis IN ('direct_declaration', 'inferred')),
  state TEXT NOT NULL CHECK (state IN ('pending', 'approved', 'rejected')),
  decision_channel TEXT CHECK (decision_channel IN ('automatic_direct', 'cli', 'viewer')),
  projected_memory_id TEXT REFERENCES memories(id),
  created_at INTEGER NOT NULL,
  decided_at INTEGER,
  CHECK ((state = 'pending' AND decision_channel IS NULL AND projected_memory_id IS NULL AND decided_at IS NULL)
    OR (state = 'rejected' AND decision_channel IS NOT NULL AND projected_memory_id IS NULL AND decided_at IS NOT NULL)
    OR (state = 'approved' AND decision_channel IS NOT NULL AND projected_memory_id IS NOT NULL AND decided_at IS NOT NULL))
) STRICT;

CREATE TABLE memory_visibility (
  id TEXT PRIMARY KEY,
  memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
  audience TEXT NOT NULL CHECK (audience IN ('work', 'project', 'personal')),
  repo_id TEXT REFERENCES repos(id),
  work_id TEXT REFERENCES work_items(id),
  proposal_id TEXT REFERENCES sharing_proposals(id),
  grant_kind TEXT NOT NULL CHECK (grant_kind IN ('migration', 'observer', 'explicit_adoption', 'proposal_approval')),
  created_at INTEGER NOT NULL,
  CHECK ((audience = 'work' AND repo_id IS NOT NULL AND work_id IS NOT NULL AND proposal_id IS NULL)
    OR (audience = 'project' AND repo_id IS NOT NULL AND work_id IS NULL AND proposal_id IS NULL)
    OR (audience = 'personal' AND repo_id IS NULL AND work_id IS NULL AND proposal_id IS NOT NULL))
) STRICT;
CREATE UNIQUE INDEX memory_visibility_work ON memory_visibility(memory_id, repo_id, work_id) WHERE audience = 'work';
CREATE UNIQUE INDEX memory_visibility_project ON memory_visibility(memory_id, repo_id) WHERE audience = 'project';
CREATE UNIQUE INDEX memory_visibility_personal ON memory_visibility(memory_id) WHERE audience = 'personal';
CREATE INDEX memory_visibility_scope ON memory_visibility(audience, repo_id, work_id, memory_id);
CREATE INDEX sharing_proposals_origin ON sharing_proposals(origin_repo_id, state, created_at, id);

INSERT INTO memory_visibility (id, memory_id, audience, repo_id, work_id, grant_kind, created_at)
  SELECT 'v_migration:' || id, id, CASE WHEN work_id IS NULL THEN 'project' ELSE 'work' END,
    repo_id, work_id, 'migration', COALESCE(created_at, 0) FROM memories
    WHERE degraded_reason IS NULL OR work_id IS NOT NULL OR review_state = 'imported';

-- Temporary fallback guidance never widens to other work during upgrade. Unbound legacy
-- fallback rows remain stored without a grant until their origin can be established.
INSERT INTO memory_visibility (id, memory_id, audience, repo_id, work_id, grant_kind, created_at)
  SELECT 'v_migration:' || m.id, m.id, 'work', m.repo_id, w.id, 'migration', COALESCE(m.created_at, 0)
  FROM memories m JOIN observation_batches batch ON batch.id = m.source_batch_id
  JOIN work_bindings b ON b.id = batch.work_binding_id JOIN work_items w ON w.id = b.work_id
  WHERE m.degraded_reason IS NOT NULL AND m.work_id IS NULL AND m.review_state <> 'imported'
    AND m.repo_id = batch.repo_id AND w.repo_id = m.repo_id AND b.session_id = batch.session_id;
