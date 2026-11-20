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
// The Build story the adapter writes for each case of STORIES in src/work_view/tests.rs.
const stories = JSON.parse(readFileSync(new URL('./stories.json', import.meta.url), 'utf8'))
const model = toWorkModel(snapshot, Number(snapshot.revision))
const key = (agent, id) => JSON.stringify(['conversation', agent, id])
const byId = (id) => model.requests.find((r) => r.id === id)
const byKey = (a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)

test('every request is read, in the state its records give it', () => {
  assert.deepEqual(model.requestReadErrors, [])
  // Pairs, not a map: a request read twice, or one more than the home made, fails too.
  const states = model.requests.map((r) => [r.id, r.state]).sort(byKey)
  assert.deepEqual(states, [
    // Its work state shipped the request.
    [key('claude', 'c:claude:s2'), 'shipped'],
    // Its work waits for the owner (Needs_Input).
    [key('claude', 'c:claude:s1'), 'your-move'],
    // Its work state said done: it waits for the owner's check.
    [key('claude', 'c:claude:s3'), 'needs-check'],
    // Two reviews found it rising.
    [key('claude', 'c:claude:s4'), 'working'],
    // A status shipped it after a review at 30%.
    [key('claude', 'c:claude:s5'), 'shipped'],
    // Its review shipped it, on a day of its own.
    [key('claude', 'c:claude:s6'), 'shipped'],
    // A request of its own name, worked on in two repositories.
    ['r:rq1', 'working'],
    // Codex only asked, minutes ago: a request with no state of its own reads as being worked on
    // for 6 hours (status-normalization.ts:10, :106).
    [key('codex', 'c:codex:c1'), 'working'],
  ].sort(byKey))
})

test('each request has its agent, its conversation at the time of its ask, and its work list', () => {
  const tools = Object.fromEntries(model.agents.map((a) => [a.id, a.tool]))
  assert.deepEqual(tools, {
    'claude:c:claude:s3': 'claude',
    'claude:c:claude:s4': 'claude',
    'claude:c:claude:s5': 'claude',
    'claude:c:claude:s6': 'claude',
    'claude:c:claude:s2': 'claude',
    'claude:c:claude:s1': 'claude',
    'codex:c:codex:c1': 'codex',
    // The mapper's own, for the request no conversation recorded.
    'agent-not-recorded': 'other',
  })
  // Each request's own agent, from its conversation; the request no conversation recorded has
  // the mapper's own.
  const agentOf = model.requests.map((r) => [r.id, r.agentId]).sort(byKey)
  const expected = [
    ...['s1', 's2', 's3', 's4', 's5', 's6'].map((s) => [key('claude', `c:claude:${s}`), `claude:c:claude:${s}`]),
    [key('codex', 'c:codex:c1'), 'codex:c:codex:c1'],
    ['r:rq1', 'agent-not-recorded'],
  ].sort(byKey)
  assert.deepEqual(agentOf, expected)
  // A conversation's own request gives it the time of its ask, which is also when the request
  // started: two fields of the answer, one time.
  for (const request of model.requests.filter((r) => r.id !== 'r:rq1')) {
    assert.equal(request.conversations.length, 1, request.id)
    assert.equal(request.conversations[0].at, request.startedAt, request.id)
  }
  assert.deepEqual(byId('r:rq1').conversations, [])
  assert.deepEqual(byId(key('claude', 'c:claude:s1')).workstreams, [
    { title: 'parser', tasks: [{ name: 'Report the line', done: false }] },
  ])
  // A list of one name in two repositories is two workstreams.
  const release = byId('r:rq1').workstreams.map((w) => w.tasks.map((t) => t.name)).sort()
  assert.deepEqual(release, [['Bump the version'], ['Tag the release']])
})

test('a request done or shipped is complete: its clock stops, and a shipment by status has no reading', () => {
  const done = byId(key('claude', 'c:claude:s3'))
  assert.ok(!done.runningLive, 'a done request does not count on')
  const shipped = byId(key('claude', 'c:claude:s2'))
  assert.ok(shipped.progress.completedAt, 'a shipped request has its completion')
  assert.ok(!shipped.runningLive, 'a shipped request does not count on')
  assert.equal(shipped.progress.readings, 0)
  assert.ok(byId(key('claude', 'c:claude:s1')).runningLive, 'an open request counts on')
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
  // The 31 days that end on the day of the read, as the goals page read them.
  assert.match(model.currentPeriod.label, /^[A-Z][a-z]{2} \d{1,2}(, \d{4})? – [A-Z][a-z]{2} \d{1,2}(, \d{4})?$/)
  assert.equal(model.pastPeriods.length, 1)
  const [month] = model.pastPeriods
  // A whole month, from its first day.
  // The month two months back is in the year before in January and February: its label names the year.
  assert.match(month.label, /^[A-Z][a-z]{2} 1 – [A-Z][a-z]{2} \d{2}(, \d{4})?$/)
  assert.equal(month.headline, 'Worked on the lexer.')
  assert.deepEqual(month.areas.map((a) => [a.title, a.timesAsked, a.quote?.quote]), [
    ['Lexer', 1, "the lexer's tokens"],
  ])
})

test('a request two reviews found rising has its history and the estimate its pace gives', () => {
  const rising = byId(key('claude', 'c:claude:s4'))
  assert.deepEqual(rising.progress.history, [20, 50])
  assert.equal(rising.progress.readings, 2)
  // 30 points in the 4 minutes between the reviews leave 50 for less than 7 more: the day of the
  // newer review, or the next one when it fell just before midnight.
  const reviewed = snapshot.humanRequests.find((r) => r.id === key('claude', 'c:claude:s4')).progress.readings
  const newer = new Date(Date.parse(reviewed.at(-1).at))
  // The next calendar day here, not 24 hours on: a change of summer time makes a day of 23 or 25.
  const next = new Date(newer.getFullYear(), newer.getMonth(), newer.getDate() + 1)
  const days = [newer, next].map((d) => d.toLocaleDateString('en-US', { month: 'short', day: 'numeric' }))
  assert.ok(days.map((d) => `at this pace, about ${d}`).includes(rising.estimate), rising.estimate)
})

test('a request a status shipped after a review at 30% reads as shipped at 100%, its review its own', () => {
  const shipped = byId(key('claude', 'c:claude:s5'))
  assert.equal(shipped.state, 'shipped')
  assert.equal(shipped.progress.percent, 100)
  assert.equal(shipped.progress.history, undefined)
  assert.ok(shipped.progress.completedAt, 'it has its completion')
})

test('the Build story reads as written: paragraphs that open with an issue number, bold or as code, are prose, and a heading stays a heading', () => {
  const story = model.buildStory
  assert.ok(story, 'the story shows')
  // A word joiner opens such a paragraph, so that the pages read it as prose; it shows as nothing.
  const shown = (text) => text.replace(/\u2060/g, '')
  assert.equal(shown(story.recap), '#1087 shipped the read adapter.')
  assert.deepEqual(story.paragraphs.map(shown), ['#1087 shipped the read adapter.', '#1088 fixed the gate.', 'Next', 'Serve the pages.'])
})

test('a request its review shipped on a day of its own completes on that day, in the zone of the browser', () => {
  const shipped = byId(key('claude', 'c:claude:s6'))
  assert.equal(shipped.state, 'shipped')
  assert.equal(shipped.progress.completedAt, 'Nov 17, 12:00 pm')
})

test('each Build story the adapter writes reads, through the pinned mapper, as its stored text means', () => {
  // What the pages show for each case, word joiners aside: the recap, then the paragraphs, a
  // heading as its text.
  const want = {
    'a recap of a heading alone': ['Shipped the adapter', ['Shipped the adapter']],
    'a recap of a heading alone, and a chapter': ['Shipped the adapter', ['Shipped the adapter', '2026-11-16', 'Built the parser.']],
    'a recap that opens on a heading': ['Ship the adapter.', ['Plan', 'Ship the adapter.']],
    'a recap that opens with an issue number, and a chapter whose heading sits above its body': ['#1087 shipped the adapter.', ['#1087 shipped the adapter.', '2026-11-16', 'Parser', 'The parser reads lines.']],
    'a recap that opens with an issue number in bold, alone': ['#1087 shipped the adapter.', ['#1087 shipped the adapter.']],
    'a recap that opens with an issue number as code, alone': ['#1088 fixed the gate.', ['#1088 fixed the gate.']],
    'a recap after a byte-order mark, alone': ['#7 follows a byte-order mark.', ['#7 follows a byte-order mark.']],
    // A story of chapters alone opens on its newest chapter.
    'chapters alone': ['Built the parser.', ['2026-11-16', 'Built the parser.']],
    // A story of headings alone opens on its first heading of its own.
    'chapters of headings alone': ['Parser', ['2026-11-16', 'Parser']],
    // Nothing of its own to show: no story.
    'chapters of no text': null,
  }
  assert.deepEqual(stories.map((s) => s.name).sort(), Object.keys(want).sort())
  const shown = (text) => text.replace(/\u2060/g, '')
  for (const { name, story } of stories) {
    const read = toWorkModel({ ...snapshot, story }, Number(snapshot.revision)).buildStory
    assert.deepEqual(read && [shown(read.recap), read.paragraphs.map(shown)], want[name], name)
  }
})

// The gate home's answer (#1089): `home()` read under rules on the keys the snapshot reads its
// counts, days and times by, on a status as the pages would read it, and on an Antigravity
// conversation's id beside `gemini` (`work_view::tests::the_contract_gated_snapshot_is_a_gate_homes_answer`).
const gated = JSON.parse(readFileSync(new URL('./gated.json', import.meta.url), 'utf8'))
const gatedModel = toWorkModel(gated, Number(gated.revision))
const gatedById = (id) => gatedModel.requests.find((r) => r.id === id)

test('under rules on the keys it reads, the snapshot keeps the goals, their asks, Past focus and each completion', () => {
  assert.equal(gatedModel.goalsReadError, null)
  assert.deepEqual(gatedModel.focusAreas.map((a) => [a.name, a.asks.map((k) => k.title)]), [
    ['Parser', ['A header test']],
    ['Docs', ['Write docs']],
  ])
  assert.deepEqual(gatedModel.otherAsks.map((k) => k.title), ['Fail fast'])
  assert.equal(gatedModel.currentPeriod.windowDays, 31)
  assert.equal(gatedModel.pastPeriods.length, 1)
  assert.ok(!gatedById(key('claude', 'c:claude:s3')).runningLive, 'a done request does not count on')
  const shipped = gatedById(key('claude', 'c:claude:s6'))
  assert.equal(shipped.state, 'shipped')
  assert.equal(shipped.progress.completedAt, 'Nov 17, 12:00 pm')
})

test('a conversation the gate would mask in its key goes by one reference, with its agent and its time', () => {
  const masked = gatedModel.requests.filter((r) => r.id.startsWith('["conversation","gemini",'))
  assert.equal(masked.length, 1)
  const [request] = masked
  assert.ok(request.id.includes('"m:'), request.id)
  assert.notEqual(request.agentId, 'agent-not-recorded')
  const [conversation] = request.conversations
  assert.equal(conversation.platform, 'gemini')
  assert.notEqual(conversation.at, 'Time not recorded')
  assert.ok(!conversation.title.startsWith('Conversation '), conversation.title)
  const text = JSON.stringify(gated)
  assert.ok(!text.includes('6f1c2a3b-4d5e'), 'the id goes nowhere')
  assert.ok(!text.includes('release-4827'), 'nor the status as the pages would read it')
})

// The Build stories `work_view::tests::a_story_is_read_whole_before_any_mask` writes under its
// rules (#1089): each with the value a rule finds in the story as the pages read it.
const gatedStories = JSON.parse(readFileSync(new URL('./gated-stories.json', import.meta.url), 'utf8'))

test('each Build story a rule finds a value in reads, through the pinned mapper, with its blocks and without the value', () => {
  // The recap, then the paragraphs, a heading as its text, word joiners aside.
  const want = {
    "a heading masked whole, which another rule needs in the story": ['[REDACTED]', ['[REDACTED]', '[REDACTED]', '[REDACTED] is in the drawer.']],
    "an issue's number after a recap that ends on a rule's keyword": ['Ask for the [REDACTED]', ['Ask for the [REDACTED]', '[REDACTED]', '[REDACTED] went out.']],
    'a heading followed by prose, as the pages show it': ['The rest is prose.', ['[REDACTED]', 'The rest is prose.', '2026-11-16', 'Nothing here.']],
    'a value a rule finds beside what a mask left': ['[REDACTED]', ['[REDACTED]', '[REDACTED] is the code.', '2026-11-16', 'Nothing here.']],
    'a heading and a line of prose under it, as they were kept': ['[REDACTED]', ['[REDACTED]', '[REDACTED]', '2026-11-16', 'Nothing here.']],
    'a recap of a heading alone, as it was kept': ['[REDACTED]', ['[REDACTED]', '2026-11-16', 'Nothing here.']],
    'a heading masked in part, read again as the pages show it': ['[REDACTED] [REDACTED]', ['[REDACTED] [REDACTED]', '2026-11-16', 'Nothing here.']],
    'a paragraph masked in part by a rule that needs the next, read again alone': ['[REDACTED] [REDACTED] Beta', ['[REDACTED] [REDACTED] Beta', 'Gamma', '2026-11-16', 'Nothing here.']],
    "a paragraph that opens with an issue's number, read without its word joiner": ['#1087 [REDACTED]', ['#1087 [REDACTED]', 'Other.', '2026-11-16', 'Nothing here.']],
    "a block that would open with a character the gate's reading drops": ['Orchid', ['Orchid', '2026-11-16', 'Nothing here.']],
    // The heading's text as the pages show it: its marks off, the U+0085 after them kept.
    "a heading over prose whose text opens with a character the gate's reading drops": ['Ordinary prose.', ['\u0085[REDACTED]', 'Ordinary prose.', '2026-11-16', 'Nothing here.']],
    "a heading over prose whose text a rule finds once the gate's reading trims it": ['Ordinary prose.', ['\u0085[REDACTED]', 'Ordinary prose.', '2026-11-16', 'Nothing here.']],
  }
  assert.deepEqual(gatedStories.map((s) => s.name).sort(), Object.keys(want).sort())
  const shown = (text) => text.replace(/⁠/g, '')
  for (const { name, hidden, story } of gatedStories) {
    const read = toWorkModel({ ...snapshot, story }, Number(snapshot.revision)).buildStory
    const got = [shown(read.recap), read.paragraphs.map(shown)]
    assert.ok(!JSON.stringify(got).includes(hidden), name)
    assert.deepEqual(got, want[name], name)
  }
})

test('a request a rule masks a part of the id of, and one whose own id is what the first one\'s was, are each their own', () => {
  const [first, second] = ['Plan the first one', 'Plan the second one'].map((title) => {
    const found = gatedModel.requests.filter((r) => r.title === title)
    assert.equal(found.length, 1, title)
    return found[0]
  })
  assert.notEqual(first.id, second.id)
  assert.ok(!JSON.stringify(gated).includes('Orchid'), 'no part of either id goes')
  assert.equal(gatedById(first.id), first)
  assert.equal(gatedById(second.id), second)
})

test('a conversation of an agent the raw hook named goes under the platform other, and keeps its agent', () => {
  const found = gatedModel.requests.filter((r) => r.title === 'Plan the raw one.')
  assert.equal(found.length, 1)
  const [conversation] = found[0].conversations
  assert.equal(conversation.platform, 'other')
  assert.equal(conversation.title, 'Plan the raw one.')
  // The rule on its name, `^(tenant-orchid):`, finds no string of the snapshot.
  const strings = (v) => typeof v === 'string' ? [v] : Array.isArray(v) ? v.flatMap(strings) : v && typeof v === 'object' ? Object.values(v).flatMap(strings) : []
  assert.ok(!strings(gated).some((s) => s.startsWith('tenant-orchid:')), 'no string opens with its name')
})

// suggested.json is the answer of work_view::tests::the_contract_suggestion_is_a_suggestion_homes_answer:
// the judge's open suggestion that a conversation is part of the parser's request (J4, W3b-3).
const suggested = JSON.parse(readFileSync(new URL('./suggested.json', import.meta.url), 'utf8'))
const suggestedModel = toWorkModel(suggested, Number(suggested.revision))

test("a judge's open suggestion reads with its request and the request it names, each by an id the mapper gives a request", () => {
  assert.deepEqual(suggestedModel.requestReadErrors, [])
  const carriers = suggestedModel.requests.filter((r) => r.suggestion)
  assert.equal(carriers.length, 1)
  const [carrier] = carriers
  assert.equal(carrier.suggestion.conversationKey, carrier.id)
  const named = suggestedModel.requests.find((r) => r.id === carrier.suggestion.requestId)
  assert.ok(named, carrier.suggestion.requestId)
  assert.notEqual(named.id, carrier.id)
  assert.equal(carrier.suggestion.requestTitle, 'Parser')
  assert.equal(named.title, 'Parser')
  assert.equal(carrier.suggestion.evidence, '')
  assert.match(carrier.suggestion.expectedSuggestionHash, /^[0-9a-f]{64}$/)
})

const subagented = JSON.parse(readFileSync(new URL('./subagented.json', import.meta.url), 'utf8'))
const subagentedModel = toWorkModel(subagented, Number(subagented.revision))

// The home of work_view::tests::subagented: conversation s1, of this hour, with a working subagent,
// a quiet one, one finished 5 minutes back and one finished 40 minutes back; conversation s0, eight
// days back, with a quiet one (decision 118: the work-view lists a carried conversation's subagents
// not finished or finished within 15 minutes).
test("a carried conversation's subagents read as agents of their own, and leave its request as it is", () => {
  assert.deepEqual(subagentedModel.requestReadErrors, [])
  const parent = subagentedModel.agents.find((a) => a.id === 'claude:c:claude:s1')
  assert.ok(parent)
  const children = subagentedModel.agents.filter((a) => a.id !== parent.id)
  // The working one, the quiet one and the one finished 5 minutes back.
  assert.equal(children.length, 3)
  for (const child of children) {
    assert.match(child.id, /^s:[0-9a-f]{12}$/)
    assert.equal(child.tool, 'claude')
    assert.equal(child.role, 'Subagent · Fix the parser.')
  }
  assert.equal(new Set(children.map((a) => a.id)).size, 3)
  for (const status of ['working', 'quiet']) {
    const one = children.filter((a) => a.status === status)
    assert.equal(one.length, 1, status)
    assert.equal(one[0].canMarkFinished, true)
    assert.equal(one[0].finishedAt, undefined)
  }
  const finished = children.filter((a) => a.status === 'finished')
  assert.equal(finished.length, 1)
  assert.equal(finished[0].canMarkFinished, false)
  // Its stop's time, which the mapper shows only when it reads it as a time.
  assert.equal(typeof finished[0].finishedAt, 'string')
  const request = subagentedModel.requests.find((r) => r.id === key('claude', 'c:claude:s1'))
  assert.ok(request)
  assert.equal(request.agentId, parent.id)
  assert.deepEqual(request.conversations.map((c) => c.title), ['Fix the parser.'])
})
