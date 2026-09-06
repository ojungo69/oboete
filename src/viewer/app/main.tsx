// The memory viewer (spec FR-037): sessions grouped by turn on the left, memory cards on the
// right, search, pin, review, delete, the injection ledger of a session, and live updates.
import { render } from 'preact';
import { useEffect, useMemo, useState } from 'preact/hooks';

import { api, type Injection, type Memory, type SearchHit, type Session } from './api.js';
import './app.css';

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

function Provenance({ memory }: { memory: Memory }) {
  if (memory.sources.length === 0) return <p class="muted">Provenance: not recorded.</p>;
  return (
    <ul class="sources">
      {memory.sources.map((source, index) => (
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
  const act = async (work: () => Promise<unknown>): Promise<void> => {
    try {
      await work();
      onChanged();
    } catch (error) {
      onError(error instanceof Error ? error.message : String(error));
    }
  };
  const pinned = memory.pinned_at !== null;
  return (
    <article class={`card sensitivity-${memory.sensitivity}`}>
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
      {memory.degraded_reason === null ? null : (
        <p class="degraded">{DEGRADED_LABEL[memory.degraded_reason] ?? memory.degraded_reason}</p>
      )}
      <Provenance memory={memory} />
      <footer>
        <span class="muted">Created {when(memory.created_at)}</span>
        <div class="actions">
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
        </div>
      </footer>
    </article>
  );
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

function App() {
  const [repository, setRepository] = useState('');
  const [memories, setMemories] = useState<Memory[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
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
      .catch((cause: unknown) => setError(cause instanceof Error ? cause.message : String(cause)));
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
      {error === null ? null : <p class="error">{error}</p>}
      <div class="columns">
        <nav class="sessions">
          <h2>Sessions</h2>
          <button type="button" class={selected === null ? 'current' : ''} onClick={() => setSelected(null)}>
            All memories
          </button>
          {sessions.length === 0 ? <p class="muted">No session has been recorded yet.</p> : null}
          {sessions.map((entry) => (
            <div key={entry.id} class={`session ${selected === entry.id ? 'current' : ''}`}>
              <button type="button" onClick={() => setSelected(entry.id)}>
                <strong>{entry.agent}</strong> {entry.status}, {entry.turn_count} {entry.turn_count === 1 ? 'turn' : 'turns'}
                <br />
                <span class="muted">{when(entry.started_at)}</span>
                {entry.summary_state === 'pending' ? <span class="badge review"> summary pending</span> : null}
              </button>
              {selected === entry.id ? (
                <ol class="turns">
                  {entry.turns.map((turn) => (
                    <li key={turn.id}>
                      Turn {turn.ordinal}: {turn.memory_ids.length === 0 ? 'no memories' : `${turn.memory_ids.length} ${turn.memory_ids.length === 1 ? 'memory' : 'memories'}`}
                    </li>
                  ))}
                </ol>
              ) : null}
            </div>
          ))}
        </nav>
        <main class="memories">
          <label class="search">
            Search memories
            <input
              type="search"
              value={query}
              placeholder="Words to look for (search is lexical in this milestone)"
              onInput={(event) => setQuery((event.currentTarget as HTMLInputElement).value)}
            />
          </label>
          {hits === null ? null : <SearchResults hits={hits.hits} note={hits.note} />}
          {ledger === null ? null : (
            <section>
              <h2>Why this session received memories</h2>
              <Ledger injections={ledger} />
            </section>
          )}
          <h2>{session === null ? 'All memories' : `Memories of session ${session.id}`}</h2>
          {shown.length === 0 ? <p class="muted">There is nothing recorded here yet.</p> : null}
          {shown.map((memory) => (
            <MemoryCard key={memory.id} memory={memory} onChanged={refresh} onError={setError} />
          ))}
        </main>
      </div>
    </>
  );
}

render(<App />, document.getElementById('app') as HTMLElement);
