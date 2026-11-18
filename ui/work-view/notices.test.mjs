// notices.mjs over small built trees made here (#1086): what it writes, and where it stops the
// build. node --test 'ui/work-view/*.test.mjs'
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

const notices = fileURLToPath(new URL('./notices.mjs', import.meta.url))

// A built tree: claude-mem's NOTICE, the app's packages (a folder each, with its package.json, its
// licence when it has one, and other files), one source map naming them, and the fonts the build
// emitted. The build tools are always among the packages, as notices.mjs adds them.
function tree({ packages, sources, fonts = [], given = {} }) {
  const work = mkdtempSync(join(tmpdir(), 'notices-'))
  const app = join(work, 'src/ui/my-work')
  const put = (path, text) => {
    mkdirSync(dirname(path), { recursive: true })
    writeFileSync(path, text)
  }
  put(join(work, 'NOTICE'), 'claude-mem\n')
  mkdirSync(join(app, 'src/setup/assets'), { recursive: true })
  const tools = [
    ['node_modules/vite', { name: 'vite', version: '7.0.0', license: 'MIT' }, 'MIT vite'],
    ['node_modules/rolldown', { name: 'rolldown', version: '1.0.0', license: 'MIT' }, 'MIT rolldown'],
  ]
  for (const [dir, pkg, licence, files = []] of [...tools, ...packages]) {
    put(join(app, dir, 'package.json'), JSON.stringify(pkg))
    if (licence !== null) put(join(app, dir, 'LICENSE'), licence)
    for (const file of files) put(join(app, dir, file), 'font')
  }
  put(join(app, 'dist/assets/index.js.map'), JSON.stringify({ sources }))
  for (const font of fonts) put(join(app, 'dist/assets', font), 'font')
  const licences = join(work, 'licences')
  mkdirSync(licences)
  for (const [name, text] of Object.entries(given)) writeFileSync(join(licences, name), text)
  return { work, licences }
}

// The notices a tree gives, or the error that stopped it.
function notice(t) {
  try {
    return { out: execFileSync(process.execPath, [notices, t.work, t.licences], { encoding: 'utf8', stdio: 'pipe' }) }
  } catch (e) {
    return { error: String(e.stderr) }
  } finally {
    rmSync(t.work, { recursive: true, force: true })
  }
}

const x = (version) => ({ name: 'x', version, license: 'MIT' })

test('two copies of one version with one text are one entry, which names both folders', () => {
  const { out, error } = notice(tree({
    packages: [
      ['node_modules/x', x('1.0.0'), 'MIT x'],
      ['node_modules/y', { name: 'y', version: '2.0.0', license: 'MIT' }, 'MIT y'],
      ['node_modules/y/node_modules/x', x('1.0.0'), 'MIT x'],
    ],
    sources: ['../../node_modules/x/i.js', '../../node_modules/y/i.js', '../../node_modules/y/node_modules/x/i.js'],
  }))
  assert.equal(error, undefined)
  assert.equal(out.split('\n## x 1.0.0 (MIT)\n').length, 2, out)
  assert.match(out, /^Folders: `node_modules\/x`, `node_modules\/y\/node_modules\/x`$/m)
})

test('a blank licence text stops the build', () => {
  const { error } = notice(tree({ packages: [['node_modules/x', x('1.0.0'), '  \n']], sources: ['../../node_modules/x/i.js'] }))
  assert.match(error, /x 1\.0\.0: its licence text is blank/)
})

test('a text in licences/ goes for the versions it names, without its versions line, and stops the build at another', () => {
  const packages = [['node_modules/x', x('1.0.0'), null]]
  const sources = ['../../node_modules/x/i.js']
  const { out } = notice(tree({ packages, sources, given: { 'x.txt': 'versions: 0.9.0, 1.0.0\n\nMIT x, from its repository\n' } }))
  assert.match(out, /MIT x, from its repository/)
  assert.doesNotMatch(out, /versions:/)
  const other = notice(tree({ packages, sources, given: { 'x.txt': 'versions: 0.9.0\n\nMIT x\n' } }))
  assert.match(other.error, /was checked for 0\.9\.0, not x 1\.0\.0/)
  const none = notice(tree({ packages, sources, given: { 'x.txt': 'MIT x\n' } }))
  assert.match(none.error, /no "versions:" line/)
})

test('a font the bundle emits is a file of a listed package, or it stops the build', () => {
  const packages = [['node_modules/x', x('1.0.0'), 'MIT x', ['files/x-latin-wght-normal.woff2']]]
  const sources = ['../../node_modules/x/i.js']
  // The emitted name is the file's with `-` and an 8-character hash, which may hold a `-`.
  const { error } = notice(tree({ packages, sources, fonts: ['x-latin-wght-normal-Ab-d1234.woff2'] }))
  assert.equal(error, undefined)
  const stray = notice(tree({ packages, sources, fonts: ['mystery-AbCd1234.woff2'] }))
  assert.match(stray.error, /mystery-AbCd1234\.woff2: a font no package listed here holds/)
})
