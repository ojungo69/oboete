// build.sh and fetch.sh, copied into small checkouts made here (#1092): where they stop, and what
// they leave. node --test 'ui/work-view/*.test.mjs'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))

// A checkout with ui/work-view/ (the two scripts and `files`, each a path under it and its text)
// and an empty assets/ beside it.
function checkout(files = {}) {
  const root = mkdtempSync(join(tmpdir(), 'work-view-'))
  const view = join(root, 'ui/work-view')
  mkdirSync(join(root, 'assets'), { recursive: true })
  mkdirSync(view, { recursive: true })
  for (const script of ['build.sh', 'fetch.sh']) copyFileSync(join(here, script), join(view, script))
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(dirname(join(view, path)), { recursive: true })
    writeFileSync(join(view, path), text)
  }
  return { root, view }
}

// A script's run: its status and what it wrote to stderr.
function run(view, script, args = []) {
  const r = spawnSync('bash', [join(view, script), ...args], { encoding: 'utf8', env: { ...process.env, LC_ALL: 'C' } })
  return { status: r.status, stderr: r.stderr }
}

// A claude-mem checkout at tag v1 with each path fetch.sh takes, the folders among them with a file.
function claudeMem(root) {
  const src = join(root, 'claude-mem')
  const fetch = readFileSync(join(here, 'fetch.sh'), 'utf8')
  const list = /^paths=\(\n([\s\S]*?)\n\)$/m.exec(fetch)[1].split(/\s+/).filter(Boolean)
  for (const path of list) {
    const file = /\.\w+$/.test(path) || ['LICENSE', 'NOTICE'].includes(path) ? path : join(path, 'index.ts')
    mkdirSync(dirname(join(src, file)), { recursive: true })
    writeFileSync(join(src, file), `${path}\n`)
  }
  const git = (...args) => assert.equal(spawnSync('git', args, { cwd: src, encoding: 'utf8' }).status, 0, args.join(' '))
  git('init', '-q')
  git('add', '.')
  git('-c', 'user.name=t', '-c', 'user.email=t@example.invalid', 'commit', '-q', '-m', 'v1')
  git('tag', 'v1')
  return src
}

test('a fetch that did not finish stops the next fetch and the next build, which name it and leave no lock', () => {
  const { root, view } = checkout({ '.upstream.AbCdEf/UPSTREAM': 'claude-mem v1, commit 0\n' })
  try {
    const src = claudeMem(root)
    for (const [script, args] of [['fetch.sh', [src, 'v1']], ['build.sh', []]]) {
      const { status, stderr } = run(view, script, args)
      assert.equal(status, 1, `${script}: ${stderr}`)
      assert.match(stderr, /\.upstream\.AbCdEf is a fetch that did not finish; see ui\/work-view\/README\.md/)
      assert.equal(existsSync(join(view, '.lock')), false, script)
      assert.equal(existsSync(join(view, 'upstream')), false, script)
    }
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('a reserved folder with no UPSTREAM, left when its removal failed, stops nothing', () => {
  const { root, view } = checkout({ '.upstream.AbCdEf/new/left.txt': 'left\n' })
  try {
    const src = claudeMem(root)
    const { status, stderr } = run(view, 'fetch.sh', [src, 'v1'])
    assert.equal(status, 0, stderr)
    assert.match(readFileSync(join(view, 'UPSTREAM'), 'utf8'), /^claude-mem v1, commit [0-9a-f]{40}\n$/)
    assert.equal(readFileSync(join(view, 'upstream/NOTICE'), 'utf8'), 'NOTICE\n')
    assert.equal(existsSync(join(view, '.lock')), false)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('a patch that adds a line opening with "Skipped patch", trailing whitespace and all, applies', () => {
  const patch = '--- a/f.txt\n+++ b/f.txt\n@@ -1 +1,2 @@\n a\n+Skipped patch f.txt   \n'
  const { root, view } = checkout({ 'upstream/f.txt': 'a\n', 'patches/0001-a.patch': patch })
  try {
    // The tree has no packages: the build stops past the patches, where it installs them.
    const { status, stderr } = run(view, 'build.sh')
    assert.notEqual(status, 0)
    assert.doesNotMatch(stderr, /skipped a file/, stderr)
    assert.match(stderr, /src\/ui\/my-work/, stderr)
    assert.equal(existsSync(join(view, '.lock')), false)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
