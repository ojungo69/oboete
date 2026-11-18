// Work-view spike, step 9: upstream's toWorkModel, unchanged, over the snapshots oboete's adapter made.
import { readFileSync } from 'node:fs'
import { toWorkModel } from './build/src/ui/my-work/src/live/to-work-model.ts'
for (const file of process.argv.slice(2)) {
  const m = toWorkModel(JSON.parse(readFileSync(file, 'utf8')), Date.now())
  console.log(JSON.stringify({
    file: file.replace(/^.*\//, ''),
    requests: m.requests.map((r) => [r.id.slice(0, 24), r.state, r.stage, (r.title || '').slice(0, 28), r.agentId ? 'agent' : 'no-agent', r.conversations.length, r.conversations.filter((c) => !c.title.startsWith('Conversation ')).length]),
    readErrors: m.requestReadErrors.map((e) => [e.requestId.slice(0, 24), e.reason.slice(0, 120)]),
    goals: m.goalsReadError, rules: m.rulesReadError,
    areas: m.focusAreas.map((a) => [a.id, a.status, a.timesAsked, a.latestAt, a.quotes.length, a.asks.length]),
    otherAsks: m.otherAsks.map((a) => [a.id, a.timesAsked, a.latestAt, a.quotes.length]),
    current: m.currentPeriod ? 'present' : null, story: m.buildStory ? 'present' : null, unlinked: m.unlinkedWorkCount,
  }))
}
