// Work-view spike: the licence of each file in the bundle. JS: the packages its source map names,
// each with the licence its package.json declares. Other files: by where vite took them from.
import { readFileSync, readdirSync, existsSync } from 'node:fs'
import { join } from 'node:path'
const root = process.argv[2]
const dist = join(root, 'dist-map/assets')
const pkgs = new Map()
const app = new Set()
for (const f of readdirSync(dist).filter((f) => f.endsWith('.map'))) {
  const { sources } = JSON.parse(readFileSync(join(dist, f), 'utf8'))
  for (const s of sources) {
    const i = s.lastIndexOf('node_modules/')
    if (i < 0) { app.add(s.replace(/^(\.\.\/)+/, '').split('/').slice(0, 3).join('/')); continue }
    const rest = s.slice(i + 13).split('/')
    const name = rest[0].startsWith('@') ? `${rest[0]}/${rest[1]}` : rest[0]
    const dir = s.slice(0, i + 13) + name
    if (!pkgs.has(name)) pkgs.set(name, { dir, files: new Set() })
    pkgs.get(name).files.add(f.replace(/-[A-Za-z0-9_-]{8}\.js\.map$/, ''))
  }
}
const rows = []
for (const [name, { dir, files }] of pkgs) {
  const p = join(dist, dir, 'package.json')
  const j = existsSync(p) ? JSON.parse(readFileSync(p, 'utf8')) : {}
  const lic = typeof j.license === 'string' ? j.license : j.license?.type ?? (j.licenses ? JSON.stringify(j.licenses) : 'NONE')
  const hasFile = existsSync(join(dist, dir)) && readdirSync(join(dist, dir)).some((x) => /^(licen[cs]e|copying|notice)/i.test(x))
  rows.push([name, j.version ?? '?', lic, hasFile ? 'file' : 'no-file', [...files].join(',')])
}
rows.sort((a, b) => a[2].localeCompare(b[2]) || a[0].localeCompare(b[0]))
const byLic = {}
for (const r of rows) byLic[r[2]] = (byLic[r[2]] || 0) + 1
console.log(JSON.stringify({ packages: rows.length, byLicence: byLic, appSources: [...app].sort(), rows }))
