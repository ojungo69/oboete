// notices.mjs over small built trees made here (#1086): what it writes, and where it stops the
// build. node --test 'ui/work-view/*.test.mjs'
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

const notices = fileURLToPath(new URL('./notices.mjs', import.meta.url))

// A built tree: claude-mem's NOTICE, the app's packages (a folder each, with its package.json, its
// licence when it has one, and other files), one source map naming them, and the fonts the build
// emitted, each a path under the app and its bytes. The build tools are always among the packages,
// as notices.mjs adds them.
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
  for (const [path, bytes] of fonts) put(join(app, path), bytes)
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

test('a blank licence text stops the build, as does one that is blank once a tool\'s bundled licences are cut', () => {
  const sources = ['../../node_modules/x/i.js']
  const { error } = notice(tree({ packages: [['node_modules/x', x('1.0.0'), '  \n']], sources }))
  assert.match(error, /x 1\.0\.0: its licence text is blank/)
  const cut = notice(tree({ packages: [['node_modules/x', x('1.0.0'), '# Licenses of bundled dependencies\nMIT y\n']], sources }))
  assert.match(cut.error, /x 1\.0\.0: its licence text is blank/)
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
  // A checkout that gives the file CRLF line ends gives the same notices.
  const lf = notice(tree({ packages, sources, given: { 'x.txt': 'versions: 1.0.0\n\nMIT x\nline two\n' } }))
  const crlf = notice(tree({ packages, sources, given: { 'x.txt': 'versions: 1.0.0\r\n\r\nMIT x\r\nline two\r\n' } }))
  assert.equal(crlf.error, undefined)
  assert.equal(crlf.out, lf.out)
})

test('a font the bundle emits, in any folder, is a listed package\'s file byte for byte, or it stops the build', () => {
  const packages = [['node_modules/x', x('1.0.0'), 'MIT x', ['files/x-latin-wght-normal.woff2']]]
  const sources = ['../../node_modules/x/i.js']
  // The emitted name is the file's with `-` and an 8-character hash, which may hold a `-`.
  const { error } = notice(tree({ packages, sources, fonts: [['dist/assets/x-latin-wght-normal-Ab-d1234.woff2', 'font']] }))
  assert.equal(error, undefined)
  const stray = notice(tree({ packages, sources, fonts: [['dist/assets/mystery-AbCd1234.woff2', 'font']] }))
  assert.match(stray.error, /mystery-AbCd1234\.woff2: a font no package listed here holds/)
  const nested = notice(tree({ packages, sources, fonts: [['dist/fonts/other-AbCd1234.woff2', 'font']] }))
  assert.match(nested.error, /dist\/fonts\/other-AbCd1234\.woff2: a font no package listed here holds/)
  // A file of the same name with other bytes is not the package's.
  const unlike = notice(tree({ packages, sources, fonts: [['dist/assets/x-latin-wght-normal-AbCd1234.woff2', 'other bytes']] }))
  assert.match(unlike.error, /x-latin-wght-normal-AbCd1234\.woff2: a font no package listed here holds/)
  // The bundle's own folders are all read, one named node_modules among them.
  const inModules = notice(tree({ packages, sources, fonts: [['dist/node_modules/y-AbCd1234.woff2', 'font']] }))
  assert.match(inModules.error, /dist\/node_modules\/y-AbCd1234\.woff2: a font no package listed here holds/)
})

test('claude-mem\'s NOTICE gives the same notices with CRLF line ends, and a tree with no setup assets and no Hanken font builds', () => {
  const packages = [['node_modules/x', x('1.0.0'), 'MIT x']]
  const sources = ['../../node_modules/x/i.js']
  const lf = tree({ packages, sources })
  writeFileSync(join(lf.work, 'NOTICE'), 'claude-mem\nline two\n')
  const crlf = tree({ packages, sources })
  writeFileSync(join(crlf.work, 'NOTICE'), 'claude-mem\r\nline two\r\n')
  const bare = tree({ packages, sources })
  rmSync(join(bare.work, 'src/ui/my-work/src/setup/assets'), { recursive: true })
  const [a, b, c] = [notice(lf), notice(crlf), notice(bare)]
  assert.equal(b.error, undefined)
  assert.equal(b.out, a.out)
  // The NOTICE whole, its second line too, whichever line ends it has.
  assert.match(b.out, /^claude-mem\nline two\n```$/m)
  assert.equal(c.error, undefined)
})

test('a font collection the bundle emits is a listed package\'s file too, or it stops the build', () => {
  const packages = [['node_modules/x', x('1.0.0'), 'MIT x', ['files/x.ttc']]]
  const sources = ['../../node_modules/x/i.js']
  const listed = notice(tree({ packages, sources, fonts: [['dist/assets/x-AbCd1234.ttc', 'font']] }))
  assert.equal(listed.error, undefined)
  const stray = notice(tree({ packages, sources, fonts: [['dist/assets/y-AbCd1234.otc', 'font']] }))
  assert.match(stray.error, /y-AbCd1234\.otc: a font no package listed here holds/)
})

test('a font copied under its own name, with no hash, is a listed package\'s file by that name', () => {
  // `-variable` looks like a hash of 8 characters: the name with it taken off is not the font's.
  const packages = [['node_modules/x', x('1.0.0'), 'MIT x', ['files/inter-variable.woff2']]]
  const sources = ['../../node_modules/x/i.js']
  const { error } = notice(tree({ packages, sources, fonts: [['dist/assets/inter-variable.woff2', 'font']] }))
  assert.equal(error, undefined)
  const hashed = notice(tree({ packages, sources, fonts: [['dist/assets/inter-variable-AbCd1234.woff2', 'font']] }))
  assert.equal(hashed.error, undefined)
})

test('a font the build inlines as a data: URL stops the build, listed or not; other data: URLs do not', () => {
  const packages = [['node_modules/x', x('1.0.0'), 'MIT x', ['files/x-latin-wght-normal.woff2']]]
  const sources = ['../../node_modules/x/i.js']
  const b64 = (bytes) => Buffer.from(bytes).toString('base64')
  const file = (path, text) => notice(tree({ packages, sources, fonts: [[path, text]] }))
  for (const [path, text] of [
    ['dist/assets/index-AbCd1234.css', `@font-face{src:url(data:font/woff2;base64,${b64('font')})}`],
    ['dist/assets/index-AbCd1234.css', `@font-face{src:url( "data: font/woff2 ; base64,${b64('font')}" )}`],
    ['dist/assets/index-AbCd1234.js', `const f = \`DATA:Font/WOFF;base64,${b64('other bytes')}\``],
    ['dist/index.html', `<link rel=preload href=data:application/x-font-ttf,%00%01%00%00>`],
    ['dist/assets/index-AbCd1234.css', `src:url(data:application/vnd.ms-fontobject;base64,${b64('eot')})`],
  ]) {
    const { error } = file(path, text)
    assert.match(error, /a font inlined as a data: URL .*set Vite's assetsInlineLimit so that fonts go as files/, text)
  }
  for (const text of [`url(data:image/png;base64,${b64('png')})`, `"data:application/json;base64,${b64('true')}"`, 'see data: below']) {
    assert.equal(file('dist/assets/index-AbCd1234.css', text).error, undefined, text)
  }
})

// Vite inlines an asset under 4 KiB as a data: URL typed by its extension, a .otc as
// application/octet-stream; patch 0005 sends a file of each extension notices.mjs reads as a font
// as a file instead, so that it is read against the package it comes from. Both name the same.
test('every extension notices.mjs reads as a font is one the build sends as a file', () => {
  const font = /^const FONT = \/(.+)\/i$/m.exec(readFileSync(notices, 'utf8'))?.[1]
  const patch = readFileSync(fileURLToPath(new URL('./patches/0005-fonts-as-files.patch', import.meta.url)), 'utf8')
  const inline = /^\+ +assetsInlineLimit: \(file: string\) => \(\/(.+)\/i\.test\(file\) \? false : undefined\),$/m.exec(patch)?.[1]
  assert.ok(font, 'notices.mjs names its font extensions')
  assert.equal(inline, font)
})
