import assert from 'node:assert/strict';
import { readFileSync, symlinkSync } from 'node:fs';
import { Readable } from 'node:stream';
import test from 'node:test';

import { checkpointHash, contentHash, materialHash } from '../../src/db/identity.js';
import { sha256Hex, sha256Json } from '../../src/hash.js';
import { runExport } from '../../src/transfer.js';
import {
  NATIVE_FORMAT,
  NATIVE_REVISION,
  type NativeMemory,
  type NativeRecord,
} from '../../src/transfer-format.js';
import { readTransferPlan, TransferInputError } from '../../src/transfer-plan.js';
import { withFixture } from '../helpers/inject-fixture.js';

const ORIGIN = 'a'.repeat(32);
const REPO = 'source-repo';
const CONTEXT = 'source-context';
const WORK = 'source-work';

function output() {
  const text = { out: '', error: '' };
  return {
    text,
    io: {
      writeOut: (value: string) => { text.out += value; },
      writeError: (value: string) => { text.error += value; },
    },
  };
}

function header() {
  return { format: NATIVE_FORMAT, revision: NATIVE_REVISION, exported_at: 1, origin_id: ORIGIN };
}

function repo(): NativeRecord {
  return { kind: 'repo', id: REPO, identity_kind: 'remote', normalized_identity: 'github.com/example/source' };
}

function context(): NativeRecord {
  return {
    kind: 'context', id: CONTEXT, repo_id: REPO, local_key: 'source-local-key', root: '/source/repo',
    repo_secret_paths_json: '[]', created_at: 1, last_seen_at: 1, redacted: false,
  };
}

function work(): NativeRecord {
  return {
    kind: 'work', id: WORK, repo_id: REPO, origin_context_id: CONTEXT, purpose: 'Imported work',
    purpose_source_event_id: null, purpose_sensitivity: 'local_only', state: 'active',
    created_at: 1, updated_at: 1, completed_at: null, current_checkpoint_memory_id: null,
    redacted: false,
  };
}

function memory(input: {
  id: string;
  title?: string | null;
  body?: string | null;
  workId?: string | null;
  parentId?: string | null;
  personal?: boolean;
  sensitivity?: 'eligible' | 'local_only' | 'private' | 'secret';
  material?: string;
}): NativeMemory {
  const title = input.title === undefined ? input.id : input.title;
  const body = input.body === undefined ? `${input.id} body` : input.body;
  const workId = input.workId ?? null;
  const parentId = input.parentId ?? null;
  const material = input.material ?? materialHash(title ?? '', body ?? '');
  const content = input.personal
    ? sha256Json(['personal-projection-v1', title ?? '', body ?? ''])
    : workId === null ? contentHash(REPO, material) : checkpointHash(REPO, workId, parentId, material);
  return {
    kind: 'memory', id: input.id, repo_id: REPO, type: 'discovery', title, body, concepts: '[]',
    material_hash: material, content_hash: content, sensitivity: input.sensitivity ?? 'local_only',
    review_state: 'reviewed', degraded_reason: null, source_session_id: null, source_batch_id: null,
    source_agent: null, valid_from: null, valid_to: null, superseded_by: null, pinned_at: null,
    pin_order: null, deleted_at: null, created_at: 1,
    identity_domain: input.personal ? 'personal_projection' : 'ordinary', work_id: workId,
    checkpoint_parent_id: parentId, provenance_complete: 1, source_captured_at: 1,
  };
}

async function expectNativeReject(records: NativeRecord[], reason: string): Promise<void> {
  const text = [header(), ...records].map((row) => JSON.stringify(row)).join('\n') + '\n';
  await assert.rejects(async () => {
    const plan = await readTransferPlan(Readable.from([Buffer.from(text)]));
    plan.close();
  }, (error: unknown) => error instanceof TransferInputError && error.reason === reason);
}

test('native export refuses an alias of the live database reached through a symlinked parent', async () => {
  await withFixture(async (fixture) => {
    const alias = `${fixture.paths.home}/home-alias`;
    symlinkSync(fixture.paths.home, alias, 'dir');
    const target = `${alias}/memory.db`;
    const { text, io } = output();

    assert.equal(await runExport([target], io), 2);
    assert.equal(text.out, '');
    assert.match(text.error, /export_target_is_database/);
    assert.equal(readFileSync(fixture.paths.db).subarray(0, 16).toString(), 'SQLite format 3\u0000');
  });
});

for (const suffix of ['-wal', '-shm'] as const) {
  test(`native export refuses the live SQLite ${suffix.slice(1)} sidecar as its target`, async () => {
    await withFixture(async (fixture) => {
      const { text, io } = output();
      assert.equal(await runExport([`${fixture.paths.db}${suffix}`], io), 2);
      assert.equal(text.out, '');
      assert.match(text.error, /export_target_is_database/);
    });
  });
}

test('native export validates its staged cross-record graph before publishing stdout', async () => {
  await withFixture(async (fixture) => {
    const title = 'Dangling export';
    const body = 'The source graph is invalid.';
    const material = materialHash(title, body);
    fixture.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, concepts, material_hash,
      content_hash, sensitivity, review_state, superseded_by, created_at)
      VALUES ('dangling-export', ?, 'discovery', ?, ?, '[]', ?, ?, 'local_only', 'reviewed',
      'missing-memory', 1)`).run(fixture.identity.id, title, body, material,
        contentHash(fixture.identity.id, material));

    const { text, io } = output();
    assert.equal(await runExport(['-'], io), 2);
    assert.equal(text.out, '', 'no bytes publish before whole-file validation succeeds');
    assert.match(text.error, /invalid_export|unknown_supersession/);
  });
});

test('source-memory and checkpoint-parent edges share one dependency cycle check', async () => {
  const a = memory({ id: 'memory-a', workId: WORK, parentId: 'memory-b' });
  const b = memory({ id: 'memory-b', workId: WORK });
  const dependency: NativeRecord = {
    kind: 'source', id: 'dependency-b-a', memory_id: b.id, raw_event_id: null,
    source_memory_id: a.id, source_context_id: CONTEXT, citation_kind: null,
    citation_value: null, source_agent: null, portion_start: null, portion_end: null,
    source_total: null, source_hash: null, evidence: null, captured_at: 1,
    source_processed_at: 1, capture_root: null, source_paths_json: null, context_only: 1,
  };
  await expectNativeReject([repo(), context(), work(), a, b, dependency], 'source_lineage_cycle');
});

test('a repository-only migration origin must reference a repository in the source file', async () => {
  const payloadHash = sha256Hex('support-only');
  const origin = ['migration-origin-v1', NATIVE_FORMAT, NATIVE_REVISION, ORIGIN,
    'session', sha256Hex('source-session-id'), payloadHash];
  const record: NativeRecord = {
    kind: 'migration_origin', id: sha256Json(origin), repo_id: 'missing-repo', memory_id: null,
    origin_json: JSON.stringify(origin), payload_hash: payloadHash, stored_payload_hash: null,
    record_kind: 'session', payload: null, classification_state: 'not_applicable',
  };
  await expectNativeReject([record], 'migration_origin_repo_mismatch');
});

test('approved proposal comparison treats null and empty projection text as different', async () => {
  const origin = memory({ id: 'origin-memory', workId: WORK });
  const projection = memory({ id: 'personal-projection', title: null, body: 'Same body', personal: true });
  const candidateMaterial = materialHash('', 'Same body');
  const proposal: NativeRecord = {
    kind: 'sharing_proposal', id: 'proposal-null-text', origin_memory_id: origin.id,
    origin_repo_id: REPO, origin_work_id: WORK, candidate_title: '', candidate_body: 'Same body',
    candidate_material_hash: candidateMaterial, candidate_sensitivity: 'local_only',
    source_event_ids_json: '[]', basis: 'inferred', state: 'approved', decision_channel: 'cli',
    projected_memory_id: projection.id, created_at: 1, decided_at: 2, redacted: false,
  };
  await expectNativeReject([repo(), context(), work(), origin, projection, proposal], 'proposal_projection_mismatch');
});

test('a proposal must be redacted when its source origin memory is secret', async () => {
  const sourceMaterial = materialHash('Secret origin', 'Secret body');
  const origin = memory({ id: 'secret-origin', title: '', body: '', workId: WORK,
    sensitivity: 'secret', material: sourceMaterial });
  const proposal: NativeRecord = {
    kind: 'sharing_proposal', id: 'proposal-from-secret', origin_memory_id: origin.id,
    origin_repo_id: REPO, origin_work_id: WORK, candidate_title: 'Visible', candidate_body: 'Must reject',
    candidate_material_hash: materialHash('Visible', 'Must reject'), candidate_sensitivity: 'local_only',
    source_event_ids_json: '[]', basis: 'inferred', state: 'pending', decision_channel: null,
    projected_memory_id: null, created_at: 1, decided_at: null, redacted: false,
  };
  await expectNativeReject([repo(), context(), work(), origin, proposal], 'proposal_parent_redaction');
});

test('a migration origin cannot retain payload when its referenced memory is secret', async () => {
  const sourceMaterial = materialHash('Secret origin', 'Secret body');
  const parent = memory({ id: 'secret-parent', title: '', body: '', sensitivity: 'secret', material: sourceMaterial });
  const payload = { kind: 'session', id: 'b'.repeat(64),
    external_payload: { note: 'must not survive a terminal parent' } };
  const payloadHash = sha256Hex(JSON.stringify(payload));
  const origin = ['migration-origin-v1', NATIVE_FORMAT, NATIVE_REVISION, ORIGIN,
    'session', sha256Hex('source-session-id'), payloadHash];
  const record: NativeRecord = {
    kind: 'migration_origin', id: sha256Json(origin), repo_id: REPO, memory_id: parent.id,
    origin_json: JSON.stringify(origin), payload_hash: payloadHash,
    stored_payload_hash: sha256Hex(JSON.stringify(payload)), record_kind: 'session', payload,
    classification_state: 'pending',
  };
  await expectNativeReject([repo(), parent, record], 'migration_origin_parent_redaction');
});
