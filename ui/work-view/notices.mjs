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
const packages = new Map()
const add = (name) => { if (!packages.has(name)) packages.set(name, join(app, 'node_modules', name)) }
const nameOf = (spec) => (spec.startsWith('@') ? spec.split('/').slice(0, 2) : spec.split('/').slice(0, 1)).join('/')

for (const file of readdirSync(assets).filter((f) => f.endsWith('.map')).sort()) {
  for (const source of JSON.parse(readFileSync(join(assets, file), 'utf8')).sources) {
    const at = source.lastIndexOf('node_modules/')
    if (at >= 0) add(nameOf(source.slice(at + 'node_modules/'.length)))
  }
}
const cssFiles = (dir) => readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
  e.isDirectory() ? cssFiles(join(dir, e.name)) : e.name.endsWith('.css') ? [join(dir, e.name)] : [])
for (const file of cssFiles(join(app, 'src'))) {
  for (const [, spec] of readFileSync(file, 'utf8').matchAll(/@import\s+(?:url\()?["']([^"'.][^"']*)["']/g)) add(nameOf(spec))
}

const LICENCE_FILE = /^(licen[cs]e|copying)(\.(md|txt|markdown))?$/i
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
for (const [name, dir] of [...packages].sort(([a], [b]) => a.localeCompare(b))) {
  const pkg = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'))
  const declared = typeof pkg.license === 'string' ? pkg.license : pkg.license?.type ?? 'not declared'
  const own = readdirSync(dir).find((f) => LICENCE_FILE.test(f))
  const given = join(overrides, `${name.replace('/', '__')}.txt`)
  const text = own ? readFileSync(join(dir, own), 'utf8') : existsSync(given) ? readFileSync(given, 'utf8') : null
  if (text === null) throw new Error(`${name} ${pkg.version}: no licence text, in the package or in ${given}`)
  section(`${name} ${pkg.version} (${declared})`, text)
}
const hanken = readdirSync(assets).some((f) => f.startsWith('HankenGrotesk'))
if (hanken) section('Hanken Grotesk (OFL-1.1), from claude-mem\'s src/setup/assets', readFileSync(join(app, 'src/setup/assets/HankenGrotesk-OFL.txt'), 'utf8'))
process.stdout.write(out.join('\n'))
