// build.sh and fetch.sh, copied into small checkouts made here (#1092): where they stop, and what
// they leave. node --test 'ui/work-view/*.test.mjs'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
// Only what the runs need, so that no GIT_DIR, GIT_INDEX_FILE or GIT_WORK_TREE of the caller's (a
// Git hook's, say) sends a fixture's Git to another repository; and Git as it comes, whatever this
// machine's settings say (`apply.whitespace` among them).
const kept = Object.fromEntries(['PATH', 'HOME', 'TMPDIR'].filter((k) => process.env[k] !== undefined).map((k) => [k, process.env[k]]))
const env = { ...kept, LC_ALL: 'C', GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1' }

// A checkout with ui/work-view/ (the two scripts and `files`, each a path under it and its text)
// and an empty assets/ beside it; it is removed after `use`.
function checkout(files, use, prefix = 'work-view-') {
  // Absolute, even under a relative TMPDIR: the npm stub's PATH entry and Git's ceiling are read
  // from other folders.
  const root = realpathSync(mkdtempSync(join(tmpdir(), prefix)))
  const view = join(root, 'ui/work-view')
  mkdirSync(join(root, 'assets'), { recursive: true })
  mkdirSync(view, { recursive: true })
  for (const script of ['build.sh', 'fetch.sh']) copyFileSync(join(here, script), join(view, script))
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(dirname(join(view, path)), { recursive: true })
    writeFileSync(join(view, path), text)
  }
  try {
    use({ root, view })
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
}

// A script's run: its status and what it wrote to stderr.
function run(view, script, args = [], path = process.env.PATH, more = {}) {
  const r = spawnSync('bash', [join(view, script), ...args], { encoding: 'utf8', env: { ...env, PATH: path, ...more } })
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
  const git = (...args) => assert.equal(spawnSync('git', args, { cwd: src, encoding: 'utf8', env }).status, 0, args.join(' '))
  git('init', '-q')
  git('add', '.')
  git('-c', 'user.name=t', '-c', 'user.email=t@example.invalid', 'commit', '-q', '-m', 'v1')
  git('tag', 'v1')
  return src
}

const unfinished = { '.upstream.AbCdEf/UPSTREAM': 'claude-mem v1, commit 0\n' }
// A patch that adds a line opening with "Skipped patch", with trailing whitespace, to a tree that has
// the app's folder and no packages.
const patched = {
  'upstream/f.txt': 'a\n',
  'upstream/src/ui/my-work/.keep': '',
  'patches/0001-a.patch': '--- a/f.txt\n+++ b/f.txt\n@@ -1 +1,2 @@\n a\n+Skipped patch f.txt   \n',
}

// A PATH whose npm shows the patched file and stops, so that a build goes no further.
function npmStops(root) {
  const bin = join(root, 'bin')
  mkdirSync(bin)
  writeFileSync(join(bin, 'npm'), '#!/bin/sh\nprintf \'PATCHED<%s>\\n\' "$(cat ../../../f.txt)" >&2\nexit 3\n', { mode: 0o755 })
  return `${bin}:${process.env.PATH}`
}

test('a fetch that did not finish stops the next fetch, which names it and leaves no lock', () => {
  checkout(unfinished, ({ root, view }) => {
    const { status, stderr } = run(view, 'fetch.sh', [claudeMem(root), 'v1'])
    assert.equal(status, 1, stderr)
    assert.match(stderr, /\.upstream\.AbCdEf is a fetch that did not finish; see ui\/work-view\/README\.md/)
    assert.equal(existsSync(join(view, '.lock')), false)
    assert.equal(existsSync(join(view, 'upstream')), false)
  })
})

test('a fetch that did not finish stops the next build, which names it and leaves no lock', () => {
  checkout({ ...unfinished, 'upstream/f.txt': 'a\n' }, ({ view }) => {
    const { status, stderr } = run(view, 'build.sh')
    assert.equal(status, 1, stderr)
    assert.match(stderr, /\.upstream\.AbCdEf is a fetch that did not finish; see ui\/work-view\/README\.md/)
    assert.equal(existsSync(join(view, '.lock')), false)
  })
})

test('a caller\'s exported glob settings change neither guard', () => {
  checkout({ ...unfinished, 'upstream/f.txt': 'a\n' }, ({ root, view }) => {
    const src = claudeMem(root)
    for (const more of [{ SHELLOPTS: 'noglob' }, { GLOBIGNORE: '.upstream.*' }]) {
      for (const [script, args] of [['fetch.sh', [src, 'v1']], ['build.sh', []]]) {
        const { status, stderr } = run(view, script, args, process.env.PATH, more)
        assert.equal(status, 1, `${script} ${JSON.stringify(more)}: ${stderr}`)
        assert.match(stderr, /\.upstream\.AbCdEf is a fetch that did not finish/, `${script} ${JSON.stringify(more)}`)
      }
    }
  })
  // With failglob, a checkout with no reserved folder still fetches.
  checkout({}, ({ root, view }) => {
    const { status, stderr } = run(view, 'fetch.sh', [claudeMem(root), 'v1'], process.env.PATH, { BASHOPTS: 'failglob' })
    assert.equal(status, 0, stderr)
  })
})

test('a reserved folder this user cannot look in stops both scripts', { skip: process.getuid?.() === 0 && 'root looks in any folder' }, () => {
  checkout({ '.upstream.AbCdEf/new/left.txt': 'left\n', 'upstream/f.txt': 'a\n' }, ({ root, view }) => {
    const src = claudeMem(root)
    chmodSync(join(view, '.upstream.AbCdEf'), 0o000)
    try {
      for (const [script, args] of [['fetch.sh', [src, 'v1']], ['build.sh', []]]) {
        const { status, stderr } = run(view, script, args)
        assert.equal(status, 1, `${script}: ${stderr}`)
        assert.match(stderr, /cannot look in .*\.upstream\.AbCdEf, which may be a fetch that did not finish/, script)
        assert.equal(existsSync(join(view, '.lock')), false, script)
      }
    } finally {
      chmodSync(join(view, '.upstream.AbCdEf'), 0o700)
    }
  })
})

test('a work-view folder this user cannot list stops both scripts', { skip: process.getuid?.() === 0 && 'root lists any folder' }, () => {
  checkout({ 'upstream/f.txt': 'a\n' }, ({ root, view }) => {
    const src = claudeMem(root)
    chmodSync(view, 0o300)
    try {
      for (const [script, args] of [['fetch.sh', [src, 'v1']], ['build.sh', []]]) {
        const { status, stderr } = run(view, script, args)
        assert.equal(status, 1, `${script}: ${stderr}`)
        assert.match(stderr, /cannot list .*ui\/work-view, so a fetch that did not finish there would go unseen/, script)
      }
    } finally {
      chmodSync(view, 0o755)
    }
    assert.equal(existsSync(join(view, '.lock')), false)
    assert.equal(existsSync(join(view, 'UPSTREAM')), false)
  })
})

test('a build with no patch in patches/ stops, and names the folder', () => {
  checkout({ 'upstream/f.txt': 'a\n', 'patches/README': 'none\n' }, ({ view }) => {
    const { status, stderr } = run(view, 'build.sh')
    assert.equal(status, 1, stderr)
    assert.match(stderr, /patches holds no patch/)
    assert.equal(existsSync(join(view, '.lock')), false)
  })
})

test('a reserved folder with no UPSTREAM, left when its removal failed, stops neither script', () => {
  // A plain patch: this case is about the folder alone.
  const plain = { ...patched, 'patches/0001-a.patch': '--- a/f.txt\n+++ b/f.txt\n@@ -1 +1,2 @@\n a\n+b\n' }
  checkout({ '.upstream.AbCdEf/new/left.txt': 'left\n', ...plain }, ({ root, view }) => {
    // The build gets past the guard and the patches, to the install.
    const build = run(view, 'build.sh', [], npmStops(root))
    assert.equal(build.status, 3, build.stderr)
    assert.doesNotMatch(build.stderr, /did not finish|cannot look in/)
    const fetch = run(view, 'fetch.sh', [claudeMem(root), 'v1'])
    assert.equal(fetch.status, 0, fetch.stderr)
    assert.match(readFileSync(join(view, 'UPSTREAM'), 'utf8'), /^claude-mem v1, commit [0-9a-f]{40}\n$/)
    assert.equal(readFileSync(join(view, 'upstream/NOTICE'), 'utf8'), 'NOTICE\n')
    assert.equal(existsSync(join(view, '.lock')), false)
  })
})

test('a checkout whose path holds glob characters builds its patches as any other', () => {
  const plain = { ...patched, 'patches/0001-a.patch': '--- a/f.txt\n+++ b/f.txt\n@@ -1 +1,2 @@\n a\n+b\n' }
  checkout(plain, ({ root, view }) => {
    const { status, stderr } = run(view, 'build.sh', [], npmStops(root))
    assert.equal(status, 3, stderr)
    assert.ok(stderr.includes('PATCHED<a\nb>'), stderr)
  }, 'work-view-[2]*-')
})

test('a patch that adds a line opening with "Skipped patch", trailing whitespace and all, applies as it is', () => {
  checkout(patched, ({ root, view }) => {
    const { status, stderr } = run(view, 'build.sh', [], npmStops(root))
    assert.equal(status, 3, stderr)
    assert.doesNotMatch(stderr, /skipped a file/, stderr)
    assert.ok(stderr.includes('PATCHED<a\nSkipped patch f.txt   >'), stderr)
    assert.equal(existsSync(join(view, '.lock')), false)
  })
})

test('--whitespace=nowarn still lets git apply say it skipped a file', () => {
  checkout({ 'upstream/f.txt': 'a\n' }, ({ root, view }) => {
    const patch = join(root, 'p.patch')
    writeFileSync(patch, '--- a/f.txt\n+++ b/f.txt\n@@ -1 +1,2 @@\n a\n+b   \n')
    const r = spawnSync('git', ['apply', '--verbose', '--whitespace=nowarn', '--exclude=f.txt', patch],
      { cwd: join(view, 'upstream'), encoding: 'utf8', env: { ...env, GIT_CEILING_DIRECTORIES: root } })
    assert.equal(r.status, 0, r.stderr)
    assert.match(r.stderr, /^Skipped patch 'f\.txt'\.$/m)
  })
})

// A PATH whose npm installs nothing and succeeds, so that a build goes on to claude-mem's tests.
function npmPasses(root) {
  const bin = join(root, 'bin')
  mkdirSync(bin)
  writeFileSync(join(bin, 'npm'), '#!/bin/sh\nexit 0\n', { mode: 0o755 })
  return `${bin}:${process.env.PATH}`
}

const TESTS = 'node --test src/live/*.test.ts src/setup/*.test.ts src/store/*.test.ts'
// An app whose package.json names `command` as its test, with one test in each of `folders`, which
// fails in `failing`, and a plain patch.
function app(command, folders, failing = '') {
  const files = {
    'upstream/f.txt': 'a\n',
    'patches/0001-a.patch': '--- a/f.txt\n+++ b/f.txt\n@@ -1 +1,2 @@\n a\n+b\n',
    'upstream/src/ui/my-work/package.json': JSON.stringify({ scripts: { test: command } }),
  }
  for (const folder of folders) {
    files[`upstream/src/ui/my-work/src/${folder}/a.test.ts`] = `import { test } from 'node:test'\ntest('${folder}', () => { if (${folder === failing}) throw new Error('failed in ${folder}') })\n`
  }
  return files
}

test('a test folder with no test left stops the build before it type-checks', () => {
  checkout(app(TESTS, ['live', 'store']), ({ root, view }) => {
    const { status, stderr } = run(view, 'build.sh', [], npmPasses(root))
    assert.notEqual(status, 0, stderr)
    assert.match(stderr, /no match: src\/setup\/\*\.test\.ts/)
    assert.doesNotMatch(stderr, /node_modules\/\.bin\/tsc/)
    assert.equal(existsSync(join(view, '.lock')), false)
  })
})

test('a test command package.json no longer names stops the build before it type-checks', () => {
  checkout(app('node --test src/live/*.test.ts', ['live', 'setup', 'store']), ({ root, view }) => {
    const { status, stderr } = run(view, 'build.sh', [], npmPasses(root))
    assert.equal(status, 1, stderr)
    assert.match(stderr, /package\.json's test command is no longer node --test src\/live/)
    assert.doesNotMatch(stderr, /node_modules\/\.bin\/tsc/)
  })
})

test('a failing test stops the build before it type-checks, and passing ones let it go on', () => {
  checkout(app(TESTS, ['live', 'setup', 'store'], 'store'), ({ root, view }) => {
    const { status, stderr } = run(view, 'build.sh', [], npmPasses(root))
    assert.notEqual(status, 0, stderr)
    assert.doesNotMatch(stderr, /node_modules\/\.bin\/tsc/)
  })
  checkout(app(TESTS, ['live', 'setup', 'store']), ({ root, view }) => {
    // The tests pass and the build reaches the type check, which the stub installed nothing for.
    const { status, stderr } = run(view, 'build.sh', [], npmPasses(root))
    assert.notEqual(status, 0, stderr)
    assert.match(stderr, /node_modules\/\.bin\/tsc/)
  })
})

test('a build run from a Node test still runs its tests, which a failing one stops before the type check', () => {
  // A Node test's children inherit its NODE_TEST_CONTEXT, with which a node --test inside skips
  // every file and passes.
  checkout(app(TESTS, ['live', 'setup', 'store'], 'store'), ({ root, view }) => {
    const { status, stderr } = run(view, 'build.sh', [], npmPasses(root), { NODE_TEST_CONTEXT: 'child-v8' })
    assert.notEqual(status, 0, stderr)
    assert.doesNotMatch(stderr, /node_modules\/\.bin\/tsc/)
  })
})
