// Work-view spike, pass item 3: the badge reaches "Live updates", goes stale while the viewer is
// stopped for 40 s (SIGSTOP), and comes back after SIGCONT. The token is read here and never printed.
// Usage: node live.mjs <viewerOutFile> <home name, e.g. home-s>. NOSTOP=1 only watches; WATCH_S sets how long.
// PLAYWRIGHT_CORE, CHROME and VIEWER_USER as in drive.mjs.
const { chromium } = await import(process.env.PLAYWRIGHT_CORE || 'playwright-core')
import { execFileSync } from 'node:child_process'

const [viewerOut, home] = process.argv.slice(2)
const as = (...args) => execFileSync('sudo', ['-n', '-u', process.env.VIEWER_USER || 'oboete-dogfood', '-H', ...args], { encoding: 'utf8' })
const token = (as('cat', viewerOut).match(/#t=([A-Za-z0-9_-]+)/) || [])[1]
if (!token) throw new Error('no token in the viewer output')
const pid = as('pgrep', '-f', `^\\./bin/oboete --home \\./${home} view`).trim().split('\n')[0]
if (!/^\d+$/.test(pid)) throw new Error('no viewer pid')

const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true })
const page = await (await browser.newContext({ viewport: { width: 1280, height: 900 } })).newPage()
const t0 = Date.now()
const reads = []
let withToken = 0
page.on('request', (r) => {
  const u = new URL(r.url())
  if (r.url().includes(token)) withToken++
  if (u.pathname === '/api/work-view') reads.push(Math.round((Date.now() - t0) / 1000))
})
await page.goto(`http://localhost:17381/work-view/#t=${token}`, { waitUntil: 'load' })
const badge = async () => (await page.evaluate(() => document.body.innerText)).match(/Live updates|[A-Z][a-z]+(?: [a-z]+)? stale|Saved activity stale|Reconnecting[^\n]*|Connecting[^\n]*|[A-Z][a-z]+ source unavailable|Activity freshness unknown/)?.[0] ?? '(none)'
const seen = []
const at = () => Math.round((Date.now() - t0) / 1000)
let stopped = false
for (let s = 0; s <= Number(process.env.WATCH_S || 110); s += 5) {
  if (s === 20 && !process.env.NOSTOP) { as('kill', '-STOP', pid); stopped = true; seen.push(`${at()}s STOP`) }
  if (s === 60 && !process.env.NOSTOP) { as('kill', '-CONT', pid); stopped = false; seen.push(`${at()}s CONT`) }
  seen.push(`${at()}s ${await badge()}`)
  await page.waitForTimeout(5000)
}
if (stopped) as('kill', '-CONT', pid)
await page.goto('about:blank')
await browser.close()
console.log(seen.join('\n'))
console.log(`work-view reads at s: ${reads.join(' ')} | request URLs holding the token: ${withToken}`)
