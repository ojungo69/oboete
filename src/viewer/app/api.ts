// The viewer's client: every call carries the per-launch token from the page URL (FR-038). The
// token never goes into a cookie or into local storage, so it lives exactly as long as the tab.

export type Source = {
  raw_event_id: string | null;
  citation_kind: string | null;
  citation_value: string | null;
  source_agent: string | null;
};

export type Memory = {
  id: string;
  type: string;
  title: string | null;
  body: string | null;
  sensitivity: string;
  review_state: string;
  degraded_reason: string | null;
  source_session_id: string | null;
  pinned_at: number | null;
  created_at: number | null;
  citations_ok: number | null;
  sources: Source[];
};

export type SearchHit = {
  id: string;
  type: string;
  title: string;
  body: string;
  sensitivity: string;
  created_at: number | null;
  score: number;
  reasons: string[];
};

export type Turn = { id: string; ordinal: number; started_at: number | null; memory_ids: string[] };

export type Session = {
  id: string;
  agent: string;
  started_at: number | null;
  ended_at: number | null;
  status: string;
  turn_count: number;
  summary_state: string | null;
  turns: Turn[];
  memory_ids: string[];
};

export type WhyItem = {
  title: string | null;
  sourceKind: string | null;
  decision: string | null;
  reason: string | null;
  rank: number | null;
  stale: boolean;
};

export type Injection = {
  id: string;
  kind: string;
  channel: string | null;
  state: string;
  contextEpoch: number;
  degradedReason: string | null;
  charBudget: number | null;
  charsUsed: number | null;
  deliveryCount: number;
  deferred: boolean;
  items: WhyItem[];
  createdAt: number | null;
};

const token = new URLSearchParams(location.search).get('token') ?? '';

async function call<T>(path: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(path, {
    ...init,
    headers: { ...(init.headers ?? {}), authorization: `Bearer ${token}` },
  });
  if (!response.ok) {
    throw new Error(
      response.status === 401
        ? 'This page needs the address printed by `oboete view`, including its token.'
        : `The viewer could not complete the request (${response.status}).`,
    );
  }
  return (await response.json()) as T;
}

export const api = {
  memories: () => call<{ repository: string; memories: Memory[] }>('/api/memories'),
  sessions: () => call<{ sessions: Session[] }>('/api/sessions'),
  search: (query: string) =>
    call<{ memories: SearchHit[]; note: string }>(`/api/search?q=${encodeURIComponent(query)}`),
  why: (sessionId: string) =>
    call<{ injections: Injection[] }>(`/api/sessions/${encodeURIComponent(sessionId)}/why`),
  review: (id: string) => call(`/api/memories/${encodeURIComponent(id)}/review`, { method: 'POST' }),
  pin: (id: string) => call(`/api/memories/${encodeURIComponent(id)}/pin`, { method: 'POST' }),
  unpin: (id: string) => call(`/api/memories/${encodeURIComponent(id)}/unpin`, { method: 'POST' }),
  remove: (id: string) => call(`/api/memories/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  /** Fires on every commit seen by the server (research R9: `PRAGMA data_version` every 500 ms). */
  events: (onChange: () => void): (() => void) => {
    const stream = new EventSource(`/api/events?token=${encodeURIComponent(token)}`);
    stream.addEventListener('change', onChange);
    return () => stream.close();
  },
};
