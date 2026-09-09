import type { AgentName, NormalizedEvent } from './events.js';

/** The key that records "this epoch was opened by Claude Code's SessionStart(compact)" (A16). */
const CLAUDE_COMPACT_START_KEY = 'session_start:compact';
/**
 * The same record for the other order: Claude Code's two compaction hooks fire about 24 ms apart
 * and are serialized only by `BEGIN IMMEDIATE`, so `PostCompact` can commit first. The prefix marks
 * an epoch its `PostCompact` opened, so the companion `SessionStart(compact)` confirms it instead of
 * advancing a second time (A16).
 */
const CLAUDE_COMPACT_POST_PREFIX = 'post_compact:';

export type CompactionState = { contextEpoch: number; lastCompactionKey: string | null };

/**
 * The context epoch of a conversation (A12): 0 at the root, +1 per compaction. The authoritative
 * event is one per agent, and the key that distinguishes two compactions is what the R13 probe
 * found ("Compaction identity and order", 2026-09-03): Grok Build's `PostCompact.timestamp`, Pi's
 * `compactionEntry.id`, and on Claude Code and Codex the compaction event's own id, which the
 * caller passes as `eventIdentity` because it is the stored `raw_events.id` of that very row (A16,
 * which collapses two byte-identical compactions of one turn). Claude Code is the one agent whose
 * `SessionStart source = compact` runs about 24 ms *before* `PostCompact`, so there that hook opens
 * the epoch and `PostCompact` only confirms it. Returns the new state, or null when the event
 * leaves the epoch untouched.
 */
export function applyCompaction(
  agent: AgentName,
  event: NormalizedEvent,
  state: CompactionState,
  eventIdentity: string,
): CompactionState | null {
  const stored = state.lastCompactionKey;
  // The compaction the current epoch belongs to, whichever of Claude Code's two hooks opened it.
  const openedKey =
    stored?.startsWith(CLAUDE_COMPACT_POST_PREFIX)
      ? stored.slice(CLAUDE_COMPACT_POST_PREFIX.length)
      : stored;

  if (agent === 'claude' && event.kind === 'session_start' && event.source === 'compact') {
    if (stored === CLAUDE_COMPACT_START_KEY) return null;
    // The `PostCompact` of this same compaction committed first and already opened the epoch, so
    // this hook only consumes the marker; the next `PostCompact` opens the next epoch (A16).
    if (openedKey !== stored) return { contextEpoch: state.contextEpoch, lastCompactionKey: openedKey };
    return { contextEpoch: state.contextEpoch + 1, lastCompactionKey: CLAUDE_COMPACT_START_KEY };
  }
  if (event.kind !== 'compaction_summary') return null;

  const key = event.compaction_key !== '' ? event.compaction_key : eventIdentity;
  if (openedKey === key) return null;
  // The companion hook already opened this epoch, so PostCompact only records its own key, which
  // is what lets the next SessionStart(compact) open the next epoch.
  if (agent === 'claude' && stored === CLAUDE_COMPACT_START_KEY) {
    return { contextEpoch: state.contextEpoch, lastCompactionKey: key };
  }
  return {
    contextEpoch: state.contextEpoch + 1,
    // ponytail: the marker is the record that this epoch is still waiting for its companion hook;
    // a Claude session whose SessionStart(compact) never arrives keeps it until the next compaction.
    lastCompactionKey: agent === 'claude' ? `${CLAUDE_COMPACT_POST_PREFIX}${key}` : key,
  };
}
