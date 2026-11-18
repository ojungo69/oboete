// The contract between oboete's work-view answer (src/work_view.rs) and claude-mem's mapper, run here
// unchanged from ui/work-view/upstream/. snapshot.json is the answer for the synthetic home of
// work_view::tests::the_contract_snapshot_is_this_homes_answer, which keeps it current. What each
// test expects follows from how that home is made, not from this answer. Node strips the mapper's
// types, as claude-mem runs its own tests: node --test 'ui/work-view/contract/*.test.mjs'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { toWorkModel } from '../upstream/src/ui/my-work/src/live/to-work-model.ts'

const snapshot = JSON.parse(readFileSync(new URL('./snapshot.json', import.meta.url), 'utf8'))
const model = toWorkModel(snapshot, Number(snapshot.revision))
const key = (agent, id) => JSON.stringify(['conversation', agent, id])
const byId = (id) => model.requests.find((r) => r.id === id)

test('every request is read, in the state its records give it', () => {
  assert.deepEqual(model.requestReadErrors, [])
  const states = Object.fromEntries(model.requests.map((r) => [r.id, r.state]))
  assert.deepEqual(states, {
    // Its work state shipped the request.
    [key('claude', 'c:claude:s2')]: 'shipped',
    // Its work waits for the owner (Needs_Input).
    [key('claude', 'c:claude:s1')]: 'your-move',
    // Codex only asked, minutes ago: a request with no state of its own reads as being worked on
    // for 6 hours (status-normalization.ts:10, :106).
    [key('codex', 'c:codex:c1')]: 'working',
  })
})

test('each request has its agent, its conversation at the time of its ask, and its work list', () => {
  const tools = Object.fromEntries(model.agents.map((a) => [a.id, a.tool]))
  assert.deepEqual(tools, {
    'claude:c:claude:s2': 'claude',
    'claude:c:claude:s1': 'claude',
    'codex:c:codex:c1': 'codex',
  })
  for (const request of model.requests) {
    assert.equal(request.conversations.length, 1, request.id)
    assert.notEqual(request.conversations[0].at, 'Time not recorded', request.id)
  }
  assert.deepEqual(byId(key('claude', 'c:claude:s1')).workstreams, [
    { title: 'parser', tasks: [{ name: 'Report the line', done: false }] },
  ])
})

test('the focus screens read the areas, their open asks and Past focus', () => {
  assert.equal(model.goalsReadError, null)
  const areas = model.focusAreas.map((a) => [a.name, a.timesAsked, a.asks.map((k) => k.title)])
  assert.deepEqual(areas, [
    ['Parser', 1, ['A header test']],
    // An area nothing was asked for in keeps its open ask.
    ['Docs', null, ['Write docs']],
  ])
  assert.deepEqual(model.otherAsks.map((k) => k.title), ['Fail fast'])
  assert.equal(model.currentPeriod.windowDays, 31)
  assert.equal(model.pastPeriods.length, 1)
  const [month] = model.pastPeriods
  // A whole month, from its first day.
  assert.match(month.label, /^[A-Z][a-z]{2} 1 – [A-Z][a-z]{2} \d{2}$/)
  assert.equal(month.headline, 'Worked on the lexer.')
  assert.deepEqual(month.areas.map((a) => [a.title, a.timesAsked, a.quote?.quote]), [
    ['Lexer', 1, "the lexer's tokens"],
  ])
})
