// Work-view spike: opens the spike viewer's /work-view/ pages in headless Chromium and records what
// they did. The viewer's token is read here, from the isolated user's viewer output, and never
// printed: every line this prints has it replaced by <t>, and no page URL is printed at all.
// Usage: node drive.mjs <stage> <outDir> <viewerOutFile|-> <route>... (a route is a query, e.g. "?view=rules").
// PLAYWRIGHT_CORE: playwright-core's index.mjs; CHROME: a Chromium; VIEWER_USER: the isolated user (oboete-dogfood).
const { chromium } = await import(process.env.PLAYWRIGHT_CORE || 'playwright-core')
import { execFileSync } from 'node:child_process'
import { mkdirSync, writeFileSync } from 'node:fs'

const [stage, outDir, viewerOut, ...routes] = process.argv.slice(2)
const BASE = 'http://localhost:17381/work-view/'
const WAIT_MS = Number(process.env.WAIT_MS || 6000)

let token = ''
if (viewerOut !== '-') {
  const text = execFileSync('sudo', ['-n', '-u', process.env.VIEWER_USER || 'oboete-dogfood', '-H', 'cat', viewerOut], { encoding: 'utf8' })
  token = (text.match(/#t=([A-Za-z0-9_-]+)/) || [])[1] || ''
  if (!token) throw new Error('no token in the viewer output')
}
const scrub = (s) => (token ? String(s).split(token).join('<t>') : String(s))
const origin = new URL(BASE).origin

mkdirSync(outDir, { recursive: true })
const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true })
const context = await browser.newContext({ viewport: { width: 1280, height: 900 }, colorScheme: 'light' })
await context.addInitScript(() => {
  window.__csp = []
  document.addEventListener('securitypolicyviolation', (e) => {
    window.__csp.push(`${e.effectiveDirective} ${e.blockedURI || '(inline)'} ${e.sourceFile ? e.sourceFile.replace(/^.*\//, '') : ''}:${e.lineNumber}`)
  })
})
const page = await context.newPage()
const log = { console: [], errors: [], requests: {}, other_origins: [], failed: [] }
page.on('console', (m) => { if (['error', 'warning'].includes(m.type())) log.console.push(`${m.type()}: ${scrub(m.text()).slice(0, 300)}`) })
page.on('pageerror', (e) => log.errors.push(scrub(e.message).slice(0, 300)))
page.on('requestfailed', (r) => log.failed.push(`${r.method()} ${scrub(new URL(r.url()).pathname)} ${r.failure()?.errorText || ''}`))
page.on('response', (r) => {
  const u = new URL(r.url())
  if (u.origin !== origin) { log.other_origins.push(`${r.request().method()} ${u.origin}${u.pathname} ${r.status()}`); return }
  const key = `${r.request().method()} ${scrub(u.pathname)} ${r.status()}`
  log.requests[key] = (log.requests[key] || 0) + 1
})

const shown = []
for (const [i, route] of routes.entries()) {
  const t0 = Date.now()
  await page.goto(`${BASE}${route}${token ? `#t=${token}` : ''}`, { waitUntil: 'load' })
  await page.waitForTimeout(WAIT_MS)
  const name = `${stage}-${i}-${route.replace(/[^a-z0-9]+/gi, '_').replace(/^_|_$/g, '') || 'work'}`
  await page.screenshot({ path: `${outDir}/${name}.png`, fullPage: false })
  const text = scrub(await page.evaluate(() => document.body.innerText))
  const csp = await page.evaluate(() => window.__csp.splice(0))
  shown.push({
    route, ms: Date.now() - t0, shot: `${name}.png`,
    live: /\bLive\b/.test(text), stale: /stale/i.test(text), errorCard: /couldn.t read|read error|could not read/i.test(text),
    csp: [...new Set(csp)].slice(0, 12),
    head: text.replace(/\s+/g, ' ').slice(0, 400),
  })
}
await browser.close()
const report = { stage, shown, ...log, console: [...new Set(log.console)].slice(0, 40), errors: [...new Set(log.errors)].slice(0, 20) }
writeFileSync(`${outDir}/${stage}.json`, JSON.stringify(report, null, 1))
console.log(scrub(JSON.stringify(report, null, 1)))
