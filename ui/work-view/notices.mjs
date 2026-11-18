// THIRD-PARTY-NOTICES.md for the work-view bundle (build.sh): every package the bundle holds, with
// its version, declared licence and licence text, and the other works it ships. The JS's packages
// come from the build's hidden source maps; the CSS's from its @import lines. A package with no
// licence text of its own takes it from licences/<name>.txt, and the build stops when neither exists.
// Usage: node notices.mjs <the built work tree> <licences folder>
import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

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
const section = (title, text) => out.push(`## ${title}`, '', '```text', text.trimEnd(), '```', '')
section('claude-mem (Apache-2.0): its NOTICE', readFileSync(join(work, 'NOTICE'), 'utf8'))
const cmp = (a, b) => (a < b ? -1 : a > b ? 1 : 0) // code-point order: the same whatever the locale
const copies = [...packages].map((rel) => {
  const dir = join(app, rel)
  return { dir, pkg: JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8')) }
}).sort((a, b) => cmp(a.pkg.name, b.pkg.name) || cmp(a.pkg.version, b.pkg.version) || cmp(a.dir, b.dir))
const written = new Set()
for (const { dir, pkg } of copies) {
  const name = pkg.name
  const declared = typeof pkg.license === 'string' ? pkg.license : pkg.license?.type ?? 'not declared'
  // Sorted: a folder's listing order differs between file systems, and the notices may not.
  const files = readdirSync(dir).sort()
  const own = files.find((f) => LICENCE_FILE.test(f))
  const given = join(overrides, `${name.replace('/', '__')}.txt`)
  let text = own ? readFileSync(join(dir, own), 'utf8') : existsSync(given) ? readFileSync(given, 'utf8') : null
  if (text === null) throw new Error(`${name} ${pkg.version}: no licence text, in the package or in ${given}`)
  // A tool's licence file also carries the licences of what it bundles for itself, none of which
  // reaches this bundle: its own licence is the part before that list.
  const bundled = text.search(/^#+ *Licenses? of bundled dependencies/im)
  if (bundled > 0) text = text.slice(0, bundled)
  for (const extra of files.filter((f) => WITH_IT.test(f))) text += `\n\n--- ${extra} ---\n\n${readFileSync(join(dir, extra), 'utf8')}`
  // Two folders holding the same version of one package are one entry.
  const entry = `${name} ${pkg.version} (${declared})`
  if (written.has(`${entry}\n${text}`)) continue
  written.add(`${entry}\n${text}`)
  section(entry, text)
}
const hanken = readdirSync(assets).some((f) => f.startsWith('HankenGrotesk'))
if (hanken) section('Hanken Grotesk (OFL-1.1), from claude-mem\'s src/setup/assets', readFileSync(join(app, 'src/setup/assets/HankenGrotesk-OFL.txt'), 'utf8'))
process.stdout.write(out.join('\n'))
