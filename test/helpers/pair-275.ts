import { factStem } from '../../scripts/e2e/probe-lib/isolated-agent.mjs';

/**
 * The `claude-to-codex` pair of the 2026-09-17T15-05-08-894Z dogfood run (JST 2026-09-18), as it
 * stood when the receiving prompt pack was built: five memories, of which the two session summaries
 * are out of the search scope but still in the FTS index. It is the reproduction of #275 — the pack
 * dropped `m_fact`, which carries the three facts the recall prompt asks for, as `below_threshold`
 * while keeping `m_confirm`, which carries none of them. The run's copy of that pair's database,
 * `/var/tmp/oboete-dogfood-upgrade/all0917/claude-to-codex/memory.db`, is the receipt for the rows.
 *
 * Keep all five rows. The miss still reproduces on the three searchable ones, but removing the
 * summaries takes the corpus from five documents to three, which lifts `m_decision` above the
 * threshold and changes what comes back — measuring a fix against a corpus the run never had.
 *
 * The prompts and facts below are copies of what `scripts/e2e/probe-lib/isolated-agent.mjs` builds.
 * They are not taken on trust: `the pinned pair prompts are still the ones the probe library sends`
 * compares all three against that library's own output.
 */

/**
 * Named the way `scripts/e2e/isolated-user.mjs` names it. This does not migrate if `factStem`
 * changes — the seeding prompt below spells the stem out, so a change there fails this file
 * instead. That is the behaviour a frozen run record wants: it must keep saying what the run said.
 */
export const PAIR_STEM = factStem('2026-09-17T15-05-08-894Z', 'claude', 'codex');

export const PAIR_FACTS: [string, string, string] = [
  `${PAIR_STEM}-1: the build token is cedar.`,
  `${PAIR_STEM}-2: the release bird is heron.`,
  `${PAIR_STEM}-3: 配布色は琥珀。`,
];

/** `recallPrompt('codex', false)`: what the receiving agent was asked. */
export const PAIR_RECALL_PROMPT = [
  'Before the tool call, remember the fact lines already present inside the oboete memory context markers.',
  "Use the shell tool exactly once to run: sed -n '1,20p' NOTES.md",
  'Make no other tool call.',
  'After the result, reply with every remembered fact line verbatim, joined by |. Do not derive the answer from NOTES.md.',
].join('\n');

/** `buildFactSeedingPrompt(PAIR_FACTS)`: what the sending agent was asked, and what its free summary quotes. */
export const PAIR_SEEDING_PROMPT = [
  'These three exact strings are durable facts about this repository. Preserve them verbatim:',
  ...PAIR_FACTS,
  'Use exactly one tool call and no other tools. In that one call, use the shell tool to run:',
  "printf '%s\\n' 'fact-2026-09-17T15-05-08-894Z-claude-to-codex-1: the build token is cedar.'"
    + " 'fact-2026-09-17T15-05-08-894Z-claude-to-codex-2: the release bird is heron.'"
    + " 'fact-2026-09-17T15-05-08-894Z-claude-to-codex-3: 配布色は琥珀。' >> NOTES.md",
  'After the tool result, reply on one line with the same three exact strings joined by |.',
].join('\n');

/** The pair's five rows, in the order the run created them. */
export const PAIR_ROWS: { id: string; type?: string; title: string; body: string }[] = [
  {
    id: 'm_checkpoint',
    type: 'session_summary',
    title: 'Record three exact strings as durable facts in NOTES.md.',
    body:
      'Purpose\nRecord three exact strings as durable facts in NOTES.md.\n\nConstraints\n'
      + '- Preserve the three exact strings verbatim.\n- Use exactly one tool call.\n'
      + '- Append the strings to NOTES.md.\n\nDecisions\n'
      + '- The three exact strings will be appended to NOTES.md.\n\nOutstanding\n'
      + '- Verify the contents of NOTES.md to ensure the strings were written correctly.',
  },
  {
    id: 'm_fact',
    title: 'Durable facts recorded to NOTES.md',
    body:
      'Three exact strings were written to NOTES.md to serve as durable facts about the repository. '
      + `The strings are: '${PAIR_FACTS[0]}', '${PAIR_FACTS[1]}', and '${PAIR_FACTS[2]}'.`,
  },
  {
    id: 'm_decision',
    title: 'Use NOTES.md for durable facts',
    body: 'A decision was made to append the three exact strings to NOTES.md to preserve them as durable facts about the repository.',
  },
  {
    id: 'm_confirm',
    title: 'Assistant message confirms fact strings',
    body: "The assistant's final message contained the three exact strings joined by a pipe character (|), confirming the successful execution of the tool call.",
  },
  {
    id: 'm_request',
    type: 'session_summary',
    title:
      'These three exact strings are durable facts about this repository. Preserve them verbatim:\n'
      + 'fact-2026-09-17T15-05-08-894Z',
    body:
      `request: ${PAIR_SEEDING_PROMPT}\ninvestigated:\nlearned: Assistant message confirms fact strings, `
      + 'Durable facts recorded to NOTES.md, Use NOTES.md for durable facts\ncompleted:\nnext_steps:',
  },
];
