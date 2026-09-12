// US6 device sync, adversarial hardening (contracts/sync.md "Verification"): a bundle from an
// in-space peer is untrusted input, so the stage and apply passes reject an origin whose natural
// key changed, never auto-map a machine-local repository a peer names with this replica's prefix,
// and `resolve --keep` refuses a head whose content the publisher withheld.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { test } from 'node:test';

import { ResolveError, resolveRow } from '../../src/sync/apply.js';
import { canonicalJson, payloadHash, revisionId, snapshotId } from '../../src/sync/identity.js';
import { BundleRejected } from '../../src/sync/stage.js';
import { headsOf, readOrigin, readRevision } from '../../src/sync/store.js';
import { insertMemory, publish, pull, withReplicas, type Replica } from '../helpers/sync.js';

type Line = Record<string, unknown>;

/** Emit a plaintext bundle from a header and body lines, recomputing the digest the header forces. */
function writeBundle(path: string, header: Line, lines: Line[]): string {
  const digest = createHash('sha256');
  const body = lines.map((line) => `${canonicalJson(line)}\n`);
  for (const text of body) digest.update(Buffer.from(text, 'utf8'));
  const full: Line = { ...header, revision_lines: lines.filter((line) => line.kind !== 'repo').length,
    heads: lines.filter((line) => line.head === true).length, revisions_sha256: digest.digest('hex') };
  full.snapshot_id = snapshotId(String(full.space_id), String(full.replica_origin_id), String(full.revisions_sha256));
  writeFileSync(path, `${canonicalJson(full)}\n${body.join('')}`);
  return path;
}

function bundleLines(path: string): { header: Line; lines: Line[] } {
  const [header, ...rest] = readFileSync(path, 'utf8').split('\n').filter((line) => line !== '').map((line) => JSON.parse(line) as Line);
  return { header: header!, lines: rest };
}

/** Recompute the payload hash and revision id a revision line's edited body forces. */
function reseal(line: Line): Line {
  line.payload_hash = payloadHash(line.payload);
  line.revision_id = revisionId(line as never);
  return line;
}

test('a bundle that reuses a stored origin id with a different natural key is rejected', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'First', 'First body');
    insertMemory(a.db, 'm_two', 'Second', 'Second body');
    const path = publish(a, dir);
    pull(b, a, path); // B now stores the origin `${a.id}:m_one` with m_one's natural.
    // Forge a bundle whose only line relabels m_two's payload and natural onto m_one's origin id.
    const { header, lines } = bundleLines(path);
    const two = lines.find((line) => line.origin_id === `${a.id}:m_two`)!;
    two.origin_id = `${a.id}:m_one`;
    (two.payload as Line).id = `${a.id}:m_one`;
    const forged = writeBundle(`${path}.natural`, header, [reseal(two)]);
    assert.throws(() => pull(b, a, forged), (error: unknown) => error instanceof BundleRejected && error.code === 'origin_natural_conflict');
  });
});

test('a common_dir repo key whose hash does not match its normalized identity is rejected', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    // B holds a machine-local repository at a path a peer can learn. The peer forges a key with
    // B's own replica prefix and that path, but an arbitrary hash, so the old normalized-identity
    // lookup would bind it (and steer revisions into B's repo) without an explicit map-repo.
    const path = '/work/victim/project';
    b.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
      VALUES ('cd_b', 'common_dir', ?, '/work/victim', 1, 1)`).run(path);
    insertMemory(a.db, 'm_one', 'First', 'First body');
    const published = publish(a, dir);
    const key = `${b.id}:common_dir:${'f'.repeat(64)}`; // B's prefix, path below, but a bogus hash.
    const { header, lines } = bundleLines(published);
    const memory = lines.find((line) => line.origin_id === `${a.id}:m_one`)!;
    (memory.payload as Line).repo_id = key;
    (memory.natural as Line).repo = key;
    const repoLine: Line = { kind: 'repo', origin_id: key, identity_kind: 'common_dir', normalized_identity: path };
    const forged = writeBundle(`${published}.commondir`, header, [repoLine, reseal(memory)]);
    assert.throws(() => pull(b, a, forged), (error: unknown) => error instanceof BundleRejected && error.code === 'repo_key_mismatch');
  });
});

test('a remote repo key over a local path this device already holds resolves to no repository', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    // The `common_dir` prefix check refuses a peer that names B's own replica id, so the same peer
    // sends B's path under a `remote:` key instead: the hash matches, and `normalized_identity` is
    // unique across both kinds, so B's insert is ignored against the row it already has. Binding
    // the forged key to that row would steer A's revisions into B's local repository unasked.
    const path = '/work/victim/project';
    b.db.prepare(`INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
      VALUES ('cd_b', 'common_dir', ?, '/work/victim', 1, 1)`).run(path);
    insertMemory(a.db, 'm_one', 'First', 'First body');
    const published = publish(a, dir);
    const key = `remote:${createHash('sha256').update(path).digest('hex')}`;
    const { header, lines } = bundleLines(published);
    const memory = lines.find((line) => line.origin_id === `${a.id}:m_one`)!;
    (memory.payload as Line).repo_id = key;
    (memory.natural as Line).repo = key;
    const repoLine: Line = { kind: 'repo', origin_id: key, identity_kind: 'remote', normalized_identity: path };
    pull(b, a, writeBundle(`${published}.forgedremote`, header, [repoLine, reseal(memory)]));
    const mapping = b.db.prepare('SELECT local_repo_id FROM sync_repo_mappings WHERE repo_key = ?').get(key);
    assert.equal(mapping?.local_repo_id ?? null, null, 'the forged key waits for an explicit map-repo');
    assert.equal(b.db.prepare('SELECT identity_kind FROM repos WHERE normalized_identity = ?').get(path)?.identity_kind, 'common_dir',
      'B\'s own repository was not relabelled');
  });
});

test('a repo line whose declared kind disagrees with its key prefix is rejected', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    // The branch below reads the key prefix while the mapping row records the declared kind, so a
    // line that disagrees would file a remote key as machine-local (and be read back as one).
    const path = 'https://example.invalid/one.git';
    const key = `remote:${createHash('sha256').update(path).digest('hex')}`;
    insertMemory(a.db, 'm_one', 'First', 'First body');
    const { header, lines } = bundleLines(publish(a, dir));
    const memory = lines.find((line) => line.origin_id === `${a.id}:m_one`)!;
    (memory.payload as Line).repo_id = key;
    (memory.natural as Line).repo = key;
    const repoLine: Line = { kind: 'repo', origin_id: key, identity_kind: 'common_dir', normalized_identity: path };
    const forged = writeBundle(`${dir}/kindmismatch.osb.json`, header, [repoLine, reseal(memory)]);
    assert.throws(() => pull(b, a, forged), (error: unknown) => error instanceof BundleRejected && error.code === 'repo_key_mismatch');
  });
});

test('resolve --keep refuses a head whose payload the publisher withheld', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    insertMemory(a.db, 'm_one', 'Title', 'Body text');
    insertMemory(b.db, 'm_one', 'Title', 'Body text'); // same content: the two origins alias onto one row.
    pull(b, a, publish(a, dir));
    // Divergent edits make two heads; A's is marked private and published without that class, so it
    // reaches B as identity-only (payload withheld, hash present).
    a.db.prepare("UPDATE memories SET sensitivity = 'private', pinned_at = 5, pin_order = 1 WHERE id = 'm_one'").run();
    b.db.prepare('UPDATE memories SET valid_to = 9 WHERE id = ?').run(readOrigin(b.db, `${b.id}:m_one`)!.local_id as string);
    pull(a, b, publish(b, dir));
    pull(b, a, publish(a, dir, ['eligible', 'local_only']));
    const origin = readOrigin(b.db, `${a.id}:m_one`)!.canonical_origin_id;
    const withheld = headsOf(b.db, origin).find((head) => {
      const revision = readRevision(b.db, head)!;
      return revision.author === a.id && revision.payload === null && revision.payload_hash !== null;
    })!;
    assert.ok(withheld, 'A\'s head reached B without its payload');
    b.db.exec('BEGIN IMMEDIATE');
    try {
      assert.throws(() => resolveRow(b.db, origin, withheld, 300), (error: unknown) => error instanceof ResolveError && error.code === 'kept_withheld');
    } finally { b.db.exec('ROLLBACK'); }
  });
});
