// The memory viewer (spec FR-037): sessions grouped by turn on the left, memory cards on the
// right, search, pin, review, delete, the injection ledger of a session, and live updates.
import { render } from 'preact';
import { useEffect, useMemo, useState } from 'preact/hooks';

import { api, type Injection, type Memory, type SearchHit, type Session, type SharingProposal } from './api.js';
import './app.css';

type SharingPage = Awaited<ReturnType<typeof api.sharing>> & { error?: string };

const SENSITIVITY_LABEL: Record<string, string> = {
  eligible: 'May be sent to the summarizer',
  local_only: 'Stays on this machine',
  private: 'Private to this machine',
  secret: 'Secret',
};

const DEGRADED_LABEL: Record<string, string> = {
  no_provider: 'Written by the built-in rules: no summarizer is configured.',
  unreachable: 'Written by the built-in rules: the summarizer could not be reached.',
  unusable_output: 'Written by the built-in rules: the summarizer returned an unusable answer.',
  language_mismatch: 'Written by the built-in rules: the summarizer answered in another language.',
  daily_cap: "Written by the built-in rules: today's free summary quota was used up.",
  provider_exhausted: "Written by the built-in rules: the summarizer's free allowance was used up.",
  provider_paid: 'Written by the built-in rules: the configured model is not on the free plan.',
  auth_failed: 'Written by the built-in rules: the summarizer rejected the credentials.',
  consent_changed: 'Written by the built-in rules: the summarizer settings changed after consent.',
  model_alias: 'Written by the built-in rules: the configured model resolved to a different one.',
  timeout: 'Written by the built-in rules: the summarizer did not answer in time.',
  rule_based: 'Written by the built-in rules rather than by a summarizer.',
};

function when(at: number | null): string {
  return at === null ? 'unknown time' : new Date(at).toLocaleString();
}

function memoryCount(count: number): string {
  if (count === 0) return 'no memories';
  return count === 1 ? '1 memory' : `${count} memories`;
}

function Provenance({ memory }: { memory: Memory }) {
  if (memory.sources === undefined) return <p class="muted">Approved personal preference. Available across your projects.</p>;
  if (memory.sources.length === 0) return <p class="muted">Provenance: not recorded.</p>;
  return (
    <ul class="sources">
      {memory.sources.map((source, index) => (
        // The rows reach the page without their id (the same rows are the CLI output), and the list
        // is shown in table order and never reordered, so the position is its identity.
        <li key={index}>
          {source.citation_kind ?? 'source'}: <code>{source.citation_value ?? source.raw_event_id ?? 'unspecified'}</code>
          {source.source_agent === null ? '' : ` (from ${source.source_agent})`}
        </li>
      ))}
    </ul>
  );
}

function MemoryCard({
  memory,
  onChanged,
  onError,
}: {
  memory: Memory;
  onChanged: () => void;
  onError: (message: string) => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const act = async (work: () => Promise<unknown>): Promise<void> => {
    if (busy) return;
    setBusy(true);
    setActionError(null);
    try {
      await work();
      onChanged();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setActionError(message);
      onError(message);
    } finally { setBusy(false); }
  };
  const pinned = memory.pinned_at !== null;
  return (
    <article id={`memory-${memory.id}`} class={`card sensitivity-${memory.sensitivity}`} aria-busy={busy}>
      <header>
        <h3>{memory.title || '(untitled)'}</h3>
        <div class="badges">
          <span class="badge type">{memory.type}</span>
          <span class="badge sensitivity" title={SENSITIVITY_LABEL[memory.sensitivity] ?? memory.sensitivity}>
            {memory.sensitivity}
          </span>
          <span class="badge review">{memory.review_state}</span>
          {pinned ? <span class="badge pinned">pinned</span> : null}
          {memory.citations_ok === 0 ? <span class="badge stale">citations stale</span> : null}
        </div>
      </header>
      <p class="body">{memory.body}</p>
      {memory.degraded_reason == null ? null : (
        <p class="degraded">{DEGRADED_LABEL[memory.degraded_reason] ?? memory.degraded_reason}</p>
      )}
      <Provenance memory={memory} />
      {actionError === null ? null : <p class="action-error" role="alert">{actionError}</p>}
      <footer>
        <span class="muted">Created {when(memory.created_at)}</span>
        <fieldset class="actions" disabled={busy} aria-label="Memory actions">
          {memory.can_adopt ? <button type="button" onClick={() => act(() => api.adopt(memory.id))}>Share with project</button> : null}
          {memory.review_state === 'unreviewed' ? (
            <button type="button" onClick={() => act(() => api.review(memory.id))}>
              Mark as reviewed
            </button>
          ) : null}
          <button type="button" onClick={() => act(() => (pinned ? api.unpin(memory.id) : api.pin(memory.id)))}>
            {pinned ? 'Unpin' : 'Pin'}
          </button>
          {confirming ? (
            <>
              <button type="button" class="danger" onClick={() => act(() => api.remove(memory.id))}>
                Confirm deletion
              </button>
              <button type="button" onClick={() => setConfirming(false)}>
                Keep it
              </button>
            </>
          ) : (
            <button type="button" onClick={() => setConfirming(true)}>
              Delete
            </button>
          )}
        </fieldset>
      </footer>
    </article>
  );
}

function SharingCard({ proposal, onChanged }: { proposal: SharingProposal; onChanged: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const decide = async (decision: 'approve' | 'reject') => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await api.decideSharing(proposal.id, decision);
      onChanged();
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusy(false); }
  };
  return <article class="card sharing-card" aria-busy={busy}>
    <h3>{proposal.candidate_title}</h3>
    <p class="body">{proposal.candidate_body}</p>
    <details class="muted"><summary>Origin of this suggestion</summary>
      <p>Memory <code>{proposal.origin_memory_id}</code><br />Work <code>{proposal.origin_work_id}</code></p>
    </details>
    <footer>
      <span role="status">{busy ? 'Saving your decision…' : proposal.state === 'approved'
        ? 'Shared across your projects.' : proposal.state === 'rejected' ? 'Sharing declined.' : 'Awaiting your approval.'}</span>
      <fieldset class="actions" disabled={busy || proposal.state !== 'pending'} aria-label="Sharing decision">
        <button type="button" onClick={() => decide('approve')}>Share across projects</button>
        <button type="button" onClick={() => decide('reject')}>Decline</button>
      </fieldset>
    </footer>
    {error === null ? null : <p class="action-error" role="alert">{error}</p>}
  </article>;
}

function SearchResults({ hits, note }: { hits: SearchHit[]; note: string }) {
  if (hits.length === 0) return <p class="muted">No memories matched this search. {note}</p>;
  return (
    <ol class="hits">
      {hits.map((hit) => (
        <li key={hit.id}>
          <strong>{hit.title || '(untitled)'}</strong> <span class="badge type">{hit.type}</span>{' '}
          <span class="muted">score {hit.score.toFixed(3)}</span>
          <p class="body">{hit.body}</p>
        </li>
      ))}
    </ol>
  );
}

function Ledger({ injections }: { injections: Injection[] }) {
  if (injections.length === 0) return <p class="muted">No injection was built for this session.</p>;
  return (
    <div class="ledger">
      {injections.map((injection) => (
        <section key={injection.id}>
          <h4>
            {injection.kind} pack via {injection.channel ?? 'unknown channel'}: {injection.state}, epoch {injection.contextEpoch}
            {injection.deferred ? `, delivered with tool calls (${injection.deliveryCount})` : ''}
          </h4>
          <p class="muted">
            {injection.charsUsed ?? 0} of {injection.charBudget ?? 0} characters, built {when(injection.createdAt)}
            {injection.degradedReason === null ? '' : `; degraded: ${injection.degradedReason}`}
          </p>
          <ul>
            {injection.items.map((item, index) => (
              // Same as the provenance list: a merged pack can carry one memory twice (planned, then
              // omitted), so no field of the row is unique; the rows arrive in ledger order.
              <li key={index}>
                {item.decision}: {item.title ?? item.sourceKind ?? 'item'}
                {item.reason === null ? '' : ` (${item.reason.replaceAll('_', ' ')})`}
                {item.stale ? ' (stale)' : ''}
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

/** The left column: every recorded session, with the selected one's turns unfolded. */
function SessionList(props: {
  sessions: Session[];
  selected: string | null;
  onSelect: (id: string | null) => void;
}) {
  const { sessions, selected, onSelect } = props;
  return (
    <nav class="sessions">
      <h2>Sessions</h2>
      <button type="button" class={selected === null ? 'current' : ''} onClick={() => onSelect(null)}>
        All memories
      </button>
      {sessions.length === 0 ? <p class="muted">No session has been recorded yet.</p> : null}
      {sessions.map((entry) => (
        <div key={entry.id} class={`session ${selected === entry.id ? 'current' : ''}`}>
          <button type="button" onClick={() => onSelect(entry.id)}>
            <strong>{entry.agent}</strong> {entry.status}, {entry.turn_count} {entry.turn_count === 1 ? 'turn' : 'turns'}
            <br />
            <span class="muted">{when(entry.started_at)}</span>
            {entry.summary_state === 'pending' ? <span class="badge review"> summary pending</span> : null}
          </button>
          {selected === entry.id ? (
            <ol class="turns">
              {entry.turns.map((turn) => (
                <li key={turn.id}>
                  Turn {turn.ordinal}: {memoryCount(turn.memory_ids.length)}
                </li>
              ))}
            </ol>
          ) : null}
        </div>
      ))}
    </nav>
  );
}

/** The right column: the search box, the ledger of the selected session, and the memories shown. */
function MemoryPane(props: {
  query: string;
  onQuery: (value: string) => void;
  hits: { hits: SearchHit[]; note: string } | null;
  ledger: Injection[] | null;
  session: Session | null;
  shown: Memory[];
  sharing: SharingPage;
  onChanged: () => void;
  onError: (message: string) => void;
}) {
  const { query, hits, ledger, session, shown } = props;
  return (
    <main class="memories">
      <label class="search">
        <span>Search memories</span>
        <input
          type="search"
          value={query}
          placeholder="Words to look for (search is lexical in this milestone)"
          onInput={(event) => props.onQuery((event.currentTarget as HTMLInputElement).value)}
        />
      </label>
      {hits === null ? null : <SearchResults hits={hits.hits} note={hits.note} />}
      {ledger === null ? null : (
        <section>
          <h2>Why this session received memories</h2>
          <Ledger injections={ledger} />
        </section>
      )}
      <section aria-labelledby="sharing-title">
        <h2 id="sharing-title">Personal preferences to share</h2>
        <p class="muted">Approve only the statement shown here to make it available across your projects.</p>
        {props.sharing.error !== undefined ? <p class="error" role="alert">Could not load sharing proposals. {props.sharing.error}</p>
          : props.sharing.proposals.length === 0 ? <p class="muted">No sharing proposals need your review.</p>
            : props.sharing.proposals.map((proposal) => <SharingCard key={proposal.id} proposal={proposal} onChanged={props.onChanged} />)}
        {props.sharing.hasMore ? <p class="muted">More proposals are available. Review these to see the next ones.</p> : null}
      </section>
      <h2>{session === null ? 'All memories' : `Memories of session ${session.id}`}</h2>
      {shown.length === 0 ? <p class="muted">There is nothing recorded here yet.</p> : null}
      {shown.map((memory) => (
        <MemoryCard key={memory.id} memory={memory} onChanged={props.onChanged} onError={props.onError} />
      ))}
    </main>
  );
}

function App() {
  const [repository, setRepository] = useState('');
  const [memories, setMemories] = useState<Memory[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [sharing, setSharing] = useState<SharingPage>({ proposals: [], hasMore: false });
  const [selected, setSelected] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [hits, setHits] = useState<{ hits: SearchHit[]; note: string } | null>(null);
  const [ledger, setLedger] = useState<Injection[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [version, setVersion] = useState(0);

  const refresh = () => setVersion((value) => value + 1);

  useEffect(() => {
    let cancelled = false;
    Promise.all([api.memories(), api.sessions()])
      .then(([memoryPage, sessionPage]) => {
        if (cancelled) return;
        setRepository(memoryPage.repository);
        setMemories(memoryPage.memories);
        setSessions(sessionPage.sessions);
        setError(null);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(cause instanceof Error ? cause.message : String(cause));
      });
    api.sharing().then((page) => { if (!cancelled) setSharing(page); })
      .catch((cause: unknown) => {
        if (!cancelled) setSharing({ proposals: [], hasMore: false,
          error: cause instanceof Error ? cause.message : String(cause) });
      });
    return () => {
      cancelled = true;
    };
  }, [version]);

  useEffect(() => api.events(refresh), []);

  useEffect(() => {
    if (query.trim() === '') {
      setHits(null);
      return;
    }
    let cancelled = false;
    api
      .search(query)
      .then((page) => {
        if (!cancelled) setHits({ hits: page.memories, note: page.note });
      })
      .catch((cause: unknown) => setError(cause instanceof Error ? cause.message : String(cause)));
    return () => {
      cancelled = true;
    };
  }, [query, version]);

  useEffect(() => {
    if (selected === null) {
      setLedger(null);
      return;
    }
    let cancelled = false;
    api
      .why(selected)
      .then((page) => {
        if (!cancelled) setLedger(page.injections);
      })
      .catch((cause: unknown) => setError(cause instanceof Error ? cause.message : String(cause)));
    return () => {
      cancelled = true;
    };
  }, [selected, version]);

  const session = useMemo(() => sessions.find((entry) => entry.id === selected) ?? null, [sessions, selected]);
  const shown = useMemo(() => {
    if (session === null) return memories;
    const ids = new Set(session.memory_ids);
    return memories.filter((memory) => ids.has(memory.id) || memory.source_session_id === session.id);
  }, [memories, session]);

  return (
    <>
      <header class="top">
        <h1>oboete memory viewer</h1>
        <p class="muted">Repository {repository || 'unknown'}. This page is reachable only from this machine.</p>
      </header>
      {error === null ? null : <p class="error" role="alert">{error}</p>}
      <div class="columns">
        <SessionList sessions={sessions} selected={selected} onSelect={setSelected} />
        <MemoryPane
          query={query}
          onQuery={setQuery}
          hits={hits}
          ledger={ledger}
          session={session}
          shown={shown}
          sharing={sharing}
          onChanged={refresh}
          onError={setError}
        />
      </div>
    </>
  );
}

render(<App />, document.getElementById('app') as HTMLElement);
