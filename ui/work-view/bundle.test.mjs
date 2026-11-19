// The committed bundle (assets/work-view/), as build.sh made it from the pinned tree and the patches:
// patch 0003 takes claude-mem's installer out of the app, its route, its import and its handoff, so
// no part of it is built (#1085).
import assert from 'node:assert/strict'
import { readdirSync } from 'node:fs'
import { join } from 'node:path'
import { test } from 'node:test'

const assets = join(import.meta.dirname, '../../assets/work-view/assets')

test('the bundle carries no part of the installer', () => {
  const files = readdirSync(assets)
  assert.ok(files.some((file) => /^index-.+\.js$/.test(file)), 'the bundle is there')
  assert.deepEqual(files.filter((file) => file.startsWith('setup-')), [])
})
