// oboete spike: replaces claude-mem 15.19.0 src/ui/local-api-token-client.ts.
// The token comes from #t= (oboete's app.js:7) or the one this browser kept, and goes in the
// X-Oboete-Token header. oboete's viewer holds no event streams (one thread per connection), so
// window.EventSource for /api/work-view/events becomes a poll that hands the page the same events.

export const LOCAL_API_TOKEN_HEADER = 'X-Oboete-Token';
const STORAGE_KEY = 'oboete-token';
const EVENTS_PATH = '/api/work-view/events';
const VERSION_PATH = '/api/work-view/version';
const WORK_VIEW_PATH = '/api/work-view';
const POLL_MS = 3000; // app.js:11119
const REREAD_MS = 60_000; // app.js:11026-11027: `v` ignores tool calls and replies
// oboete answers an unknown POST 405 (view.rs:1642, 2474-2475).
const NOT_SENT = new Set(['/api/work-view/usage', '/api/viewer-usage', '/api/work-view/activity']);
const REFUSED = 'This page needs the full address printed by `oboete view` (with the key after #).';

let refusedOnce = false;

export function installLocalApiToken(
  win: Window & typeof globalThis = globalThis as unknown as Window & typeof globalThis,
): string {
  const recall = (): string => {
    try { return win.localStorage.getItem(STORAGE_KEY) ?? ''; } catch { return ''; }
  };
  const fromAddress = new URLSearchParams(win.location.hash.slice(1)).get('t') || '';
  const token = fromAddress || recall();
  let kept = fromAddress === '';

  const refused = (): void => {
    if (refusedOnce) return;
    refusedOnce = true;
    try { if (fromAddress === '' && recall() === token) win.localStorage.removeItem(STORAGE_KEY); } catch { /* blocked */ }
    const bar = win.document.createElement('div');
    bar.setAttribute('role', 'alert');
    bar.textContent = REFUSED;
    Object.assign(bar.style, { position: 'fixed', inset: '0 0 auto 0', zIndex: '2147483647', padding: '12px 16px', background: '#7f1d1d', color: '#fff' });
    win.document.body.prepend(bar);
  };

  const nativeFetch = win.fetch.bind(win);
  win.fetch = (async (input: Parameters<typeof fetch>[0], init?: Parameters<typeof fetch>[1]): Promise<Response> => {
    const given = input instanceof URL ? input.href : input;
    const url = new URL(typeof given === 'string' ? given : given.url, win.location.href);
    if (url.origin !== win.location.origin) throw new TypeError(`oboete: no address but its own (${url.origin})`);
    if (NOT_SENT.has(url.pathname)) return new Response(null, { status: 204 });
    const withPolicy = {
      ...init,
      referrerPolicy: (init?.referrerPolicy || (typeof given === 'string' ? '' : given.referrerPolicy) || 'same-origin') as ReferrerPolicy,
    };
    const request = typeof given === 'string' ? new Request(given, withPolicy) : new Request(given, withPolicy);
    if (token !== '' && !request.headers.has(LOCAL_API_TOKEN_HEADER)) request.headers.set(LOCAL_API_TOKEN_HEADER, token);
    const answer = await nativeFetch(request);
    if (url.pathname.startsWith('/api/')) {
      if (answer.status === 401) refused();
      else if (answer.ok && !kept) {
        kept = true;
        try { win.localStorage.setItem(STORAGE_KEY, token); } catch { /* blocked */ }
      }
    }
    return answer;
  }) as typeof fetch;

  class PollingEventSource extends EventTarget {
    readyState = 0;
    readonly url: string;
    readonly withCredentials = false;
    onopen: ((event: Event) => void) | null = null;
    onerror: ((event: Event) => void) | null = null;
    onmessage: ((event: MessageEvent) => void) | null = null;
    private timer: ReturnType<typeof setTimeout> | undefined;
    private abort = new AbortController();
    private busy = false;
    private seen: string | undefined;
    private readAt = 0;
    private source = { kind: 'oboete', connected: true, error: null as string | null };

    constructor(url: URL) {
      super();
      this.url = url.href;
      win.document.addEventListener('visibilitychange', this.wake);
      this.timer = setTimeout(() => void this.tick(), 0); // after the page sets its listeners
    }

    dispatchEvent(event: Event): boolean {
      const handler = (this as unknown as Record<string, unknown>)[`on${event.type}`];
      if (typeof handler === 'function') { try { handler.call(this, event); } catch (error) { console.error(error); } }
      return super.dispatchEvent(event);
    }

    close(): void {
      this.readyState = 2;
      clearTimeout(this.timer);
      this.abort.abort();
      win.document.removeEventListener('visibilitychange', this.wake);
    }

    private wake = (): void => {
      if (win.document.visibilityState !== 'visible' || this.readyState === 2) return;
      clearTimeout(this.timer);
      void this.tick();
    };

    private next(): void {
      if (this.readyState !== 2) this.timer = setTimeout(() => void this.tick(), POLL_MS);
    }

    // Error while readyState is not 2, or local-source.ts:602-604 reopens every second.
    private refuse(): void {
      this.readyState = 0;
      this.dispatchEvent(new Event('error'));
      this.close();
    }

    private async get(path: string): Promise<Response | null> {
      const answer = await win.fetch(path, { cache: 'no-store', signal: this.abort.signal });
      if (answer.status !== 401) return answer;
      this.refuse();
      return null;
    }

    private async tick(): Promise<void> {
      if (this.busy || this.readyState === 2) return;
      if (refusedOnce) { this.refuse(); return; }
      if (this.readyState === 1 && win.document.visibilityState === 'hidden') { this.next(); return; }
      this.busy = true;
      try {
        const head = await this.get(VERSION_PATH);
        if (head === null) return;
        if (!head.ok) throw new Error(`HTTP ${head.status}`);
        const { v } = (await head.json()) as { v?: unknown };
        if (this.readyState !== 1) { this.readyState = 1; this.dispatchEvent(new Event('open')); }
        const now = Date.now();
        if (String(v) !== this.seen || now - this.readAt >= REREAD_MS) {
          const page = await this.get(WORK_VIEW_PATH);
          if (page === null) return;
          if (page.ok) {
            const data = await page.text();
            this.seen = String(v);
            this.readAt = now;
            try {
              const { source } = JSON.parse(data) as { source?: { connected?: unknown; error?: unknown } };
              this.source = { kind: 'oboete', connected: source?.connected !== false, error: typeof source?.error === 'string' ? source.error : null };
            } catch { /* the page drops an unreadable frame too */ }
            this.dispatchEvent(new MessageEvent('work-view', { data }));
          } else {
            this.source = { kind: 'oboete', connected: false, error: `HTTP ${page.status}` };
          }
        }
        const at = new Date().toISOString(); // 3 fraction digits, this browser's clock
        this.dispatchEvent(new MessageEvent('heartbeat', { data: JSON.stringify({ at, source: { ...this.source, lastCheckedAt: at } }) }));
      } catch {
        if (this.readyState !== 2) { this.readyState = 0; this.seen = undefined; this.dispatchEvent(new Event('error')); }
      } finally {
        this.busy = false;
        this.next();
      }
    }
  }

  const NativeEventSource = win.EventSource;
  win.EventSource = function (target: string | URL, init?: EventSourceInit) {
    const url = new URL(String(target), win.location.href);
    return url.origin === win.location.origin && url.pathname === EVENTS_PATH
      ? new PollingEventSource(url)
      : new NativeEventSource(target, init);
  } as unknown as typeof EventSource;

  win.addEventListener('hashchange', () => win.location.reload());
  return token;
}

/** oboete takes no token in a URL; an <img> cannot send the header, so the snapshot sends no media. */
export function withLocalApiTokenQuery(url: string): string {
  return url;
}
