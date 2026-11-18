// THIRD-PARTY-NOTICES.md for the work-view bundle (build.sh): every package the bundle holds, with
// its version, declared licence and licence text, and the other works it ships. The JS's packages
// come from the build's hidden source maps; the CSS's from its @import lines. A package with no
// licence text of its own takes it from licences/<name>.txt, which names the versions it was checked
// for; the build stops when neither exists, when a text is blank, at a version not checked, and at a
// font no notice covers.
// Usage: node notices.mjs <the built work tree> <licences folder>
import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { basename, join, relative, sep } from 'node:path'

const [work, overrides] = process.argv.slice(2)
const app = join(work, 'src/ui/my-work')
const assets = join(app, 'dist/assets')
// Each copy of a package is counted by its folder: two versions of one name (a nested node_modules)
// are two copies, each with its own licence.
const packages = new Set()
const nameOf = (spec) => (spec.startsWith('@') ? spec.split('/').slice(0, 2) : spec.split('/').slice(0, 1)).join('/')
// node_modules/a/node_modules/@s/b/dist/x.js gives node_modules/a/node_modules/@s/b.
function packageDir(path) {
  const parts = path.split('/')
  let at = parts.indexOf('node_modules')
  const dir = []
  while (at >= 0 && parts[at] === 'node_modules' && at + 1 < parts.length) {
    const name = nameOf(parts.slice(at + 1).join('/')).split('/')
    dir.push('node_modules', ...name)
    at += 1 + name.length
  }
  return dir.join('/')
}
const add = (dir) => packages.add(dir)

for (const file of readdirSync(assets).filter((f) => f.endsWith('.map')).sort()) {
  for (const source of JSON.parse(readFileSync(join(assets, file), 'utf8')).sources) {
    if (source.includes('node_modules/')) add(packageDir(source))
  }
}
const cssFiles = (dir) => readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
  e.isDirectory() ? cssFiles(join(dir, e.name)) : e.name.endsWith('.css') ? [join(dir, e.name)] : [])
// The build tools add code of their own that no source map lists: Vite's module preload helper and
// Rolldown's runtime helpers.
add('node_modules/vite')
add('node_modules/rolldown')
for (const file of cssFiles(join(app, 'src'))) {
  for (const [, spec] of readFileSync(file, 'utf8').matchAll(/@import\s+(?:url\()?["']([^"'.][^"']*)["']/g)) add(`node_modules/${nameOf(spec)}`)
}

const LICENCE_FILE = /^(licen[cs]e|copying)(\.(md|txt|markdown))?$/i
// What a licence asks to go with it: a package's NOTICE (Apache-2.0, section 4) and the third-party
// licences of the code it carries, as Rolldown's runtime helpers have.
const WITH_IT = /^(notice|third[-_ ]?party[-_ ]?(licen[cs]es?|notices?))(\.(md|txt|markdown))?$/i
const out = [
  '# Third-party notices: the work-view bundle',
  '',
  'assets/work-view/ is claude-mem\'s work-view (ui/work-view/UPSTREAM) with oboete\'s patches, built by',
  'ui/work-view/build.sh. claude-mem\'s own code, its focus artwork among it, is under the Apache License,',
  'Version 2.0, with the NOTICE below. Each package and font it holds follows, with its licence.',
  '',
]
const section = (title, text, folders = []) => out.push(`## ${title}`, '',
  ...(folders.length ? [`Folders: ${folders.map((f) => `\`${f}\``).join(', ')}`, ''] : []),
  '```text', text.trimEnd(), '```', '')
// The checked-in texts are read with LF, whatever line ends a checkout gave them.
const lf = (path) => readFileSync(path, 'utf8').replace(/\r\n/g, '\n')
section('claude-mem (Apache-2.0): its NOTICE', lf(join(work, 'NOTICE')))
const cmp = (a, b) => (a < b ? -1 : a > b ? 1 : 0) // code-point order: the same whatever the locale
const copies = [...packages].map((rel) => {
  const dir = join(app, rel)
  return { rel, dir, pkg: JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8')) }
// The folder as the notices write it, with `/`, breaks a tie: a native path sorts otherwise on Windows.
}).sort((a, b) => cmp(a.pkg.name, b.pkg.name) || cmp(a.pkg.version, b.pkg.version) || cmp(a.rel, b.rel))
// A text given in licences/ opens with the versions it was checked for, `versions: 6.2.6` and a
// blank line: a bump to another version stops the build until the licence is checked again.
function given(path, pkg) {
  // A checkout may give the file CRLF line ends: the text is the same.
  const text = lf(path)
  const head = /^versions: ([^\n]+)\n\n/.exec(text)
  if (!head) throw new Error(`${path}: no "versions:" line naming the versions it was checked for`)
  if (!head[1].split(/,\s*/).includes(pkg.version)) {
    throw new Error(`${path} was checked for ${head[1]}, not ${pkg.name} ${pkg.version}: check its licence and add the version`)
  }
  return text.slice(head[0].length)
}
// One entry per name, version and text, with every folder that holds it: a nested copy a bump adds
// shows in the notices' diff.
const entries = new Map()
for (const { rel, dir, pkg } of copies) {
  const name = pkg.name
  const declared = typeof pkg.license === 'string' ? pkg.license : pkg.license?.type ?? 'not declared'
  // Sorted: a folder's listing order differs between file systems, and the notices may not.
  const files = readdirSync(dir).sort()
  const own = files.find((f) => LICENCE_FILE.test(f))
  const path = join(overrides, `${name.replace('/', '__')}.txt`)
  let text = own ? readFileSync(join(dir, own), 'utf8') : existsSync(path) ? given(path, pkg) : null
  if (text === null) throw new Error(`${name} ${pkg.version}: no licence text, in the package or in ${path}`)
  // A tool's licence file also carries the licences of what it bundles for itself, none of which
  // reaches this bundle: its own licence is the part before that list.
  const bundled = text.search(/^#+ *Licenses? of bundled dependencies/im)
  if (bundled >= 0) text = text.slice(0, bundled)
  if (!text.trim()) throw new Error(`${name} ${pkg.version}: its licence text is blank`)
  for (const extra of files.filter((f) => WITH_IT.test(f))) text += `\n\n--- ${extra} ---\n\n${readFileSync(join(dir, extra), 'utf8')}`
  const entry = `${name} ${pkg.version} (${declared})`
  const key = `${entry}\n${text}`
  if (!entries.has(key)) entries.set(key, { entry, text, folders: [] })
  entries.get(key).folders.push(rel)
}
for (const { entry, text, folders } of entries.values()) section(entry, text, folders)
// Every font the bundle emits, in any of its folders, is a file of a package listed above or
// claude-mem's own Hanken Grotesk, with its licence below, byte for byte: Vite copies fonts as they
// are. The emitted name is the file's with `-` and an 8-character hash before the extension, or the
// file's own when it is copied as it is; a font under Vite's `assetsInlineLimit` (4 KiB) is inlined
// in a script or a style as a `data:` URL instead, and is read by its bytes alone.
const FONT = /\.(woff2?|ttf|otf|eot)$/i
// A package's own files, not its nested node_modules (packages of their own); every file of a folder.
const walk = (dir, nested = true) => readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
  e.name === 'node_modules' && !nested ? [] : e.isDirectory() ? walk(join(dir, e.name), nested) : [join(dir, e.name)])
const own = join(app, 'src/setup/assets')
const hankenFiles = (existsSync(own) ? walk(own) : []).filter((f) => basename(f).startsWith('HankenGrotesk') && FONT.test(f))
const sources = new Map()
for (const file of [...copies.flatMap(({ dir }) => walk(dir, false)), ...hankenFiles].filter((f) => FONT.test(f))) {
  sources.set(basename(file), [...(sources.get(basename(file)) ?? []), file])
}
let hanken = false
const shown = (path) => relative(app, path).split(sep).join('/')
const dist = walk(join(app, 'dist')).sort()
for (const font of dist.filter((f) => FONT.test(f))) {
  const bytes = readFileSync(font)
  const names = new Set([basename(font), basename(font).replace(/-[\w-]{8}(\.[^.]+)$/, '$1')])
  const source = [...names].flatMap((name) => sources.get(name) ?? []).find((f) => readFileSync(f).equals(bytes))
  if (!source) throw new Error(`${shown(font)}: a font no package listed here holds, and no notice covers`)
  if (source.startsWith(own + sep)) hanken = true
}
// Each data: URL of a font type in the bundle's scripts, styles and pages: its bytes are a listed
// package's font, or Hanken Grotesk's. Vite writes a font's bytes in base64; one it cannot read so
// stops the build too.
const DATA_FONT = /data:(font\/[\w.+-]+|application\/(?:x-)?font-[\w.+-]+|application\/vnd\.ms-fontobject)([^,'")\s]*),([A-Za-z0-9+/=]*)/gi
const known = [...sources.values()].flat()
for (const file of dist.filter((f) => /\.(m?js|css|html)$/i.test(f))) {
  for (const [, type, params, data] of readFileSync(file, 'utf8').matchAll(DATA_FONT)) {
    if (!/;base64$/i.test(params)) throw new Error(`${shown(file)}: a ${type} data: URL not in base64, which this check cannot read`)
    const bytes = Buffer.from(data, 'base64')
    const source = known.find((f) => readFileSync(f).equals(bytes))
    if (!source) throw new Error(`${shown(file)}: a ${type} font inlined as a data: URL that no package listed here holds, and no notice covers`)
    if (source.startsWith(own + sep)) hanken = true
  }
}
if (hanken) {
  const ofl = lf(join(own, 'HankenGrotesk-OFL.txt'))
  if (!ofl.trim()) throw new Error('Hanken Grotesk: its licence text is blank')
  section('Hanken Grotesk (OFL-1.1), from claude-mem\'s src/setup/assets', ofl)
}
process.stdout.write(out.join('\n'))
