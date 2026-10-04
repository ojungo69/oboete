// Loaded as a module (strict, deferred, top-level await). Store text is rendered as text nodes.

const token = new URLSearchParams(location.hash.slice(1)).get('t') || '';
const $ = (id) => document.getElementById(id);

// append and replaceChildren would print a null as "null": a view leaves out a part with null.
const present = (nodes) => nodes.filter((n) => n !== null && n !== undefined);

function el(tag, cls, ...children) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  e.append(...present(children));
  return e;
}

// `textLang` marks the language of a translated status line (#274).
function setStatus(text, isError = false, textLang = null) {
  $('status').textContent = text;
  $('status').classList.toggle('error', isError);
  if (textLang) $('status').lang = textLang;
  else $('status').removeAttribute('lang');
}

function showError(error, retry) {
  const feedError = view === 'timeline';
  const reason = failureMessage(error);
  setStatus(feedError ? t('feed_error', { reason }) : reason, true, lang);
  const notice = $('status').firstChild;
  if (error.status !== 503 && !feedError) return notice;
  const button = word('button', 'quiet small', 'retry');
  button.type = 'button';
  button.addEventListener('click', async () => {
    button.disabled = true;
    try { await retry(); } finally { button.disabled = false; }
  });
  $('status').append(' ', button);
  return notice;
}

function failureMessage(error) {
  if (!error.status) return t(error.key || 'network_failed');
  const byStatus = { 400: 'bad_request', 401: 'unauthorized', 403: 'forbidden',
    404: 'not_found', 413: 'too_large', 503: 'memory_busy' };
  const key = error.status === 404 && ['doc', 'claim'].includes(error.resource)
    ? `${error.resource}_missing` : byStatus[error.status];
  return key ? t('request_error', { status: error.status, reason: t(key) })
    : t('load_failed', { status: error.status });
}

async function api(name, params = {}) {
  const res = await fetch(`/api/${name}?${new URLSearchParams(params)}`, {
    headers: { 'X-Oboete-Token': token },
  });
  if (!res.ok) {
    const error = new Error();
    error.status = res.status;
    error.resource = name;
    throw error;
  }
  return res.json();
}

// localStorage is a convenience: a private window or blocked storage just forgets.
function remember(key, value) {
  try { localStorage.setItem(key, value); } catch { /* ignore */ }
}
function recall(key, fallback) {
  try { return localStorage.getItem(key) ?? fallback; } catch { return fallback; }
}

// --- Theme: follow the system, or force one -------------------------------------------------

const THEMES = ['auto', 'light', 'dark'];
let theme = THEMES.includes(recall('oboete-theme', 'auto')) ? recall('oboete-theme', 'auto') : 'auto';

function applyTheme() {
  if (theme === 'auto') delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
  $('theme').textContent = t('theme', { theme: t(`theme_${theme}`) });
  $('theme').title = t('theme_help');
}

// --- Entries and their details --------------------------------------------------------------

function metadata(value) {
  const key = `meta_${value}`;
  return Object.hasOwn(TEXT, key) ? t(key) : value;
}

function badge(kind, translated = false) {
  const key = `meta_${kind}`;
  if (translated) {
    if (Object.hasOwn(TEXT, key)) return word('span', `badge ${kind}`, key);
    // Records joins a claim's kind and status; kinds such as "open item" contain a space.
    const at = kind.lastIndexOf(' ');
    const keys = [`meta_${kind.slice(0, at)}`, `meta_${kind.slice(at + 1)}`];
    if (at > 0 && keys.every((k) => Object.hasOwn(TEXT, k))) {
      return el('span', `badge ${kind}`, word('span', null, keys[0]), ' ', word('span', null, keys[1]));
    }
  }
  return el('span', `badge ${kind}`, kind);
}

function localTime(ms, language = 'en-US') {
  const date = new Date(ms);
  const time = el('time', null, date.toLocaleString(language));
  if (!Number.isNaN(date.getTime())) time.dateTime = date.toISOString();
  return time;
}

// A redraw reopens details the owner left open.
const opened = new Set();

function expander(label, key, load, restore = true) {
  const button = el('button', null, label);
  button.type = 'button';
  button.setAttribute('aria-expanded', 'false');
  let panel = null;
  let failureNotice = null;
  const toggle = async () => {
    if (button.disabled) return;
    const open = button.getAttribute('aria-expanded') === 'true';
    if (panel) {
      panel.hidden = open;
      button.setAttribute('aria-expanded', String(!open));
      if (restore) {
        if (open) opened.delete(key); else opened.add(key);
      }
      return;
    }
    button.disabled = true;
    try {
      const loaded = await load();
      if (!button.isConnected) return;
      panel = loaded;
      const row = button.parentElement;
      (row?.classList.contains('actions') ? row : button).after(panel);
      button.setAttribute('aria-expanded', 'true');
      if (restore) opened.add(key);
      if (failureNotice?.isConnected) setStatus('');
    } catch (e) {
      if (button.isConnected) failureNotice = showError(e, toggle);
    } finally {
      button.disabled = false;
    }
  };
  button.addEventListener('click', toggle);
  if (restore && opened.has(key)) void toggle();
  return button;
}

function fullText(key, label = null) {
  const button = expander(label ?? word('span', null, 'full_text'), `doc:${key}`, async () => {
    const doc = await api('doc', { id: key });
    return el('section', 'detail', el('h4', null, doc.id), el('pre', 'document-text', doc.text));
  });
  button.setAttribute('aria-label', t('full_text_of', { key }));
  button.dataset.i18nAria = 'full_text_of';
  button.dataset.i18nVars = JSON.stringify({ key });
  return button;
}

function claimLink(uid, label = uid, restore = false) {
  // Reciprocal claim links open only on a click; restoring them would follow a cycle forever.
  const button = expander(label, `claim:${uid}`, () => claimPanel(uid), restore);
  button.setAttribute('aria-label', t('view_claim_id', { uid }));
  button.dataset.i18nAria = 'view_claim_id';
  button.dataset.i18nVars = JSON.stringify({ uid });
  return button;
}

function claimLinks(heading, uids) {
  return el('section', null, word('h4', null, heading),
    uids.length ? el('ul', 'claim-links', ...uids.map((uid) => el('li', null, claimLink(uid)))) : word('p', 'text pending', 'none'));
}

async function claimPanel(uid) {
  const c = await api('claim', { id: uid });
  const quotes = c.quotes.map((q) => el('li', null,
    el('p', 'text', q.text), fullText(q.key, q.key)));
  const history = c.history.map((change) => el('li', null,
    el('div', 'meta', localTime(change.ts),
      change.tier === null && change.recipe === null ? word('span', null, 'owner_correction') : null,
      change.tier === null ? null : word('span', null, 'tier', { tier: change.tier }),
      change.recipe === null ? null : word('span', null, 'recipe', { recipe: change.recipe }),
      change.status === null ? word('span', null, 'status_unchanged')
        : el('span', null, t('claim_status', { status: metadata(change.status) }))),
    change.body === null ? word('p', 'text pending', 'text_unchanged') : el('p', 'text', change.body)));
  return el('section', 'detail claim-view',
    word('h3', null, 'claim_title', { uid: c.uid }),
    el('div', 'meta', badge(c.kind, true), badge(c.status, true), badge(c.label, true), localTime(c.when)),
    el('dl', 'claim-meta', ...row('delivered', t(c.delivered ? 'yes' : 'no')),
      ...row('speaker', metadata(c.speaker)), ...row('scope', metadata(c.scope)), ...row('repository', c.repo ?? '–')),
    el('p', 'text', c.text),
    c.later ? el('p', 'relation', word('span', null, 'later_claim'), claimLink(c.later)) : null,
    el('section', null, word('h4', null, 'evidence_quotes'),
      quotes.length ? el('ul', 'quotes', ...quotes) : word('p', 'text pending', 'no_evidence')),
    claimLinks('supersedes', c.supersedes), claimLinks('ended_by', c.ended_by),
    el('section', null, word('h4', null, 'claim_history'),
      history.length ? el('ol', 'claim-history', ...history) : word('p', 'text pending', 'no_changes')));
}

function entryMeta(d, all) {
  return el('div', 'meta', badge(d.class, true), d.label ? badge(d.label, true) : null,
    badge(d.kind, true), d.status ? badge(d.status, true) : null, localTime(d.when),
    all ? el('span', null, d.repo ?? '–') : null, el('span', null, d.key));
}

function entryActions(key, isClaim) {
  return el('div', 'actions', fullText(key), isClaim ? claimLink(key, word('span', null, 'view_claim'), true) : null);
}

// Built through el(), which leaves out the nulls: Element.append would print them as "null".
function hitEntry(h, all) {
  return el('li', h.class === 'delivered' ? 'entry delivered' : 'entry', entryMeta(h, all),
    h.class === 'delivered' ? el('p', 'relation', word('span', null, 'earlier_decision'), claimLink(h.later)) : null,
    h.class === 'superseded' && h.by ? el('p', 'relation', word('span', null, 'superseded_by'), claimLink(h.by)) : null,
    h.title ? el('p', 'title', h.title) : null, el('p', 'text', h.snippet),
    entryActions(h.key, ['current', 'delivered', 'superseded'].includes(h.class)));
}

function timelineEntry(item, all) {
  return el('li', 'entry', entryMeta(item, all), el('p', 'text', item.text),
    entryActions(item.key, item.class === 'claim'));
}

// --- Views ----------------------------------------------------------------------------------

const LIMIT = 100;
const VIEWS = ['timeline', 'records', 'context', 'stats', 'settings'];
let view = VIEWS.includes(recall('oboete-view', 'timeline')) ? recall('oboete-view', 'timeline') : 'timeline';
let currentRepo = '';
let reposLoaded = false;

function setView(name) {
  view = name;
  remember('oboete-view', name);
  document.querySelector('main').classList.toggle('timeline-view', name === 'timeline');
  $('controls').hidden = name === 'settings';
  for (const control of document.querySelectorAll('.record-controls')) control.hidden = name === 'timeline';
  for (const b of document.querySelectorAll('#tabs .tab')) {
    b.classList.toggle('active', b.dataset.view === name);
    b.setAttribute('aria-current', b.dataset.view === name ? 'page' : 'false');
  }
}

function draw(heading, list, panel) {
  $('heading').removeAttribute('lang');
  $('heading').replaceChildren(...(Array.isArray(heading) ? heading : [heading]));
  $('list').replaceChildren(...present(list));
  $('list').classList.toggle('feed', view === 'timeline');
  $('panel').replaceChildren(...present(panel));
  $('vector').textContent = '';
  $('vector').hidden = true;
}

function scope(repo) {
  return repo ? { repo } : { all: '1' };
}

async function showRecords(repo) {
  const params = { ...scope(repo), limit: LIMIT };
  const page = await api('timeline', params);
  return () => {
    const mine = generation;
    let next = page.next;
    const more = word('button', 'quiet more', 'more');
    more.type = 'button';
    const loadMore = async () => {
      if (more.disabled) return;
      more.disabled = true;
      try {
        const older = await api('timeline', { ...params, before: next });
        if (mine !== generation || !more.isConnected) return;
        $('list').append(...older.items.map((item) => timelineEntry(item, !repo)));
        next = older.next;
        if (!next) more.remove();
        setStatus(t('entries_loaded', { n: $('list').childElementCount }));
      } catch (e) {
        if (mine === generation) showError(e, loadMore);
      } finally {
        more.disabled = false;
      }
    };
    more.addEventListener('click', loadMore);
    draw(t('records'), page.items.map((item) => timelineEntry(item, !repo)), next ? [more] : []);
    setStatus(page.items.length ? t('entries_loaded', { n: page.items.length }) : t('no_entries'));
  };
}

// --- The mixed feed: its cursor belongs to fetched pages, never to a live reread ------------

const FEED_LIMIT = 50;
let feed = null;
let feedDetailId = 0;

function word(tag, cls, key, vars = {}) {
  const node = el(tag, cls, t(key, vars));
  node.dataset.i18n = key;
  if (Object.keys(vars).length) node.dataset.i18nVars = JSON.stringify(vars);
  return node;
}

function shortenFile(path) {
  // Either separator, so a path recorded on Windows is shortened too (Codex on #373).
  const parts = path.split(/[\\/]/);
  // docs/page.md P6 defines priority, rather than whichever marker comes first in the path.
  for (const marker of ['src', 'docs', 'plugin', 'Scripts']) {
    const at = parts.indexOf(marker);
    if (at >= 0) return parts.slice(at).join('/');
  }
  return parts.slice(-3).join('/');
}

function feedHeader(item) {
  const kind = item.kind === 'card'
    ? (item.type ? badge(item.type) : word('span', 'badge', 'feed_card'))
    : word('span', `badge ${item.kind}`, `feed_${item.kind}`);
  return el('header', 'feed-card-header', kind,
    item.agent ? el('span', 'badge feed-agent', item.agent) : null,
    el('span', 'feed-repo', item.repo_name));
}

function feedFooter(item) {
  const time = localTime(item.ts, lang);
  time.dataset.feedTimestamp = String(item.ts);
  return el('footer', 'feed-card-footer', el('code', null, item.id), time);
}

function feedCard(item) {
  const title = item.title ? el('h3', 'feed-title', item.title) : word('h3', 'feed-title', 'untitled');
  const subtitle = item.subtitle ? el('p', 'text feed-subtitle', item.subtitle) : null;
  const hasFacts = item.facts.length || item.concepts.length || item.files_read.length || item.files_modified.length;
  const facts = hasFacts ? el('section', 'feed-facts',
    item.facts.length ? el('ul', 'facts-list', ...item.facts.map((fact) => el('li', 'text', fact))) : null,
    item.concepts.length ? el('div', 'feed-concepts', ...item.concepts.map((concept) => el('span', 'badge', concept))) : null,
    item.files_read.length ? el('p', 'feed-files', word('span', null, 'files_read'), ' ', item.files_read.map(shortenFile).join(', ')) : null,
    item.files_modified.length ? el('p', 'feed-files', word('span', null, 'files_modified'), ' ', item.files_modified.map(shortenFile).join(', ')) : null) : null;
  const narrative = item.narrative ? el('section', 'text feed-narrative', item.narrative) : null;
  if (narrative) narrative.tabIndex = 0;
  let mode = null;
  const toggles = [];
  const views = [['facts', facts], ['narrative', narrative]].filter(([, panel]) => panel);
  for (const [name, panel] of views) {
    panel.hidden = true;
    panel.id = `feed-detail-${++feedDetailId}`;
    const button = word('button', 'quiet small feed-toggle', name);
    button.type = 'button';
    button.setAttribute('aria-controls', panel.id);
    button.setAttribute('aria-expanded', 'false');
    button.setAttribute('aria-pressed', 'false');
    button.addEventListener('click', () => {
      mode = mode === name ? null : name;
      if (subtitle) subtitle.hidden = mode !== null;
      views.forEach(([key, content], index) => {
        content.hidden = mode !== key;
        toggles[index].classList.toggle('active', mode === key);
        toggles[index].setAttribute('aria-expanded', String(mode === key));
        toggles[index].setAttribute('aria-pressed', String(mode === key));
      });
    });
    toggles.push(button);
  }
  return el('article', 'feed-card observation-card', feedHeader(item), title,
    subtitle, toggles.length ? el('div', 'feed-toggles', ...toggles) : null,
    facts, narrative, feedFooter(item));
}

function feedEntry(item) {
  let card;
  if (item.kind === 'card') card = feedCard(item);
  else if (item.kind === 'summary') {
    const sections = ['investigated', 'learned', 'completed', 'next_steps'];
    card = el('article', 'feed-card summary-card', feedHeader(item),
      item.fields.request ? el('h3', 'feed-title', item.fields.request) : null,
      ...sections.filter((field) => item.fields[field]).map((field) => el('section', 'summary-section',
        word('h4', 'summary-label', field), el('p', 'text', item.fields[field]))),
      feedFooter(item));
  } else card = el('article', 'feed-card prompt-card', feedHeader(item),
    el('p', 'text prompt-text', item.text), feedFooter(item));
  return el('li', 'feed-entry', card);
}

function activeFeed(state) {
  return Boolean(state) && feed === state && state.generation === generation && view === 'timeline';
}

function updateFeedState(state) {
  if (!activeFeed(state)) return;
  const message = state.loading ? t('feed_loading')
    : state.error ? t('feed_error', { reason: failureMessage(state.error) })
      : !state.keys.size ? t('feed_empty') : state.next === null ? t('feed_exhausted') : '';
  state.notice.textContent = message;
  state.notice.classList.toggle('error', Boolean(state.error) && !state.loading);
  state.more.hidden = state.next === null;
  state.more.disabled = state.loading;
  state.retry.hidden = !state.error || state.loading;
  state.sentinel.hidden = !state.keys.size || state.next === null || state.loading || Boolean(state.error);
}

async function readFeed(state, mode) {
  if (!activeFeed(state)) return false;
  if (state.loading) {
    if (mode !== 'live') return false;
    // A live read waits for More, so it cannot replace or race the older-page cursor.
    await state.request;
    if (!activeFeed(state)) return false;
    return readFeed(state, mode);
  }
  // An empty feed has no older pages to retain: the new first page supplies its first cursor.
  if (mode === 'live' && (!state.loaded || !state.keys.size)) mode = 'initial';
  if (mode === 'more' && state.next === null) return false;
  state.loading = true;
  state.error = null;
  state.retryMode = mode;
  updateFeedState(state);
  state.request = (async () => {
    try {
      const params = { ...scope(state.repo), limit: FEED_LIMIT };
      if (mode === 'more') params.before = state.next;
      const page = await api('feed', params);
      if (!activeFeed(state)) return false;
      const entries = [];
      const unseen = new Set();
      let joined = false;
      for (const item of page.items) {
        const key = `${item.kind}:${item.id}`;
        if (state.keys.has(key)) {
          // Live, only what is newer than the newest item shown goes on top: an unseen item
          // below a shown one is older than it (a card made late for an early window), and a
          // page with none shown left a gap; Refresh shows those in their place (P5; Codex on
          // #373).
          joined = true;
          if (mode === 'live') break;
          continue;
        }
        if (unseen.has(key)) continue;
        entries.push(feedEntry(item));
        unseen.add(key);
      }
      if (mode === 'live' && !joined) {
        entries.length = 0;
        unseen.clear();
      }
      if (mode === 'live') $('list').prepend(...entries);
      else {
        $('list').append(...entries);
        state.next = page.next;
        state.loaded = true;
      }
      for (const key of unseen) state.keys.add(key);
      setStatus('');
      return true;
    } catch (error) {
      if (activeFeed(state)) state.error = error;
      return false;
    } finally {
      state.loading = false;
      updateFeedState(state);
    }
  })();
  return state.request;
}

async function showFeed(repo, mine) {
  const notice = el('p', 'feed-state');
  notice.setAttribute('role', 'status');
  notice.setAttribute('aria-live', 'polite');
  const more = word('button', 'quiet more', 'more');
  more.type = 'button';
  const retry = word('button', 'quiet', 'retry');
  retry.type = 'button';
  const sentinel = el('div', 'feed-sentinel');
  sentinel.setAttribute('aria-hidden', 'true');
  const state = { generation: mine, repo, keys: new Set(), next: null, loaded: false, loading: false,
    error: null, request: null, retryMode: 'initial', notice, more, retry, sentinel, observer: null };
  feed = state;
  more.addEventListener('click', () => void readFeed(state, 'more'));
  retry.addEventListener('click', () => void readFeed(state, state.retryMode));
  draw(t('timeline'), [], [el('div', 'feed-pagination', notice, more, retry, sentinel)]);
  setStatus('');
  if (typeof IntersectionObserver !== 'undefined') {
    state.observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting) && !state.loading && !state.error) void readFeed(state, 'more');
    }, { threshold: 0.1 });
    state.observer.observe(sentinel);
  }
  return readFeed(state, 'initial');
}

async function showSearch(repo, q) {
  const answer = await api('search', { q, ...scope(repo), limit: LIMIT,
    since: $('since').value, until: $('until').value,
    history: $('history').checked ? '1' : '0', raw: $('raw').value });
  return () => {
    // The server puts each delivered decision immediately after the claim that ended it.
    draw([word('span', null, 'search_heading'), el('span', 'query', q)], answer.hits.map((h) => hitEntry(h, !repo)), []);
    if (answer.vector !== 'used') {
      const known = Object.hasOwn(TEXT, `vector_${answer.vector}`);
      const state = t(known ? `vector_${answer.vector}` : 'vector_unavailable');
      const reason = t(known ? `vector_why_${answer.vector}` : 'vector_why_unavailable');
      $('vector').replaceChildren(word('span', null, answer.why ? 'vector_notice_reason' : 'vector_notice', { state, reason }));
      $('vector').hidden = false;
    }
    if (answer.hits.length === LIMIT) setStatus(t('best_matches', { n: LIMIT }));
    else if (answer.hits.length) setStatus(t('found', { n: answer.hits.length }));
    else setStatus(t('nothing_found'));
  };
}

async function showContext(repo) {
  // Omitting the viewer's own label also preserves its actual checkout branch.
  const c = await api('context', repo && repo !== currentRepo ? { repo } : {});
  return () => {
    // The resident viewer has no checkout of its own (docs/resident.md R8): it asks for one.
    if (c.choose) {
      draw(word('span', null, 'context_heading'), [], [
        word('p', 'lead', 'context_choose'),
      ]);
      setStatus('');
      return;
    }
    draw(word('span', null, 'context_heading'), [], [
      !repo ? word('p', 'lead', 'context_checkout') : null,
      el('dl', 'claim-meta', ...row('repository', c.repo), ...row('branch', c.branch || '–'),
        ...row('session_start', t(c.on ? 'context_on' : 'context_off')), ...row('size', t('characters', { n: c.chars }))),
      c.text ? el('pre', 'context', c.text) : word('p', 'text pending', 'context_empty'),
    ]);
    setStatus('');
  };
}

function row(term, value) {
  return [word('dt', null, term), el('dd', null, String(value))];
}

function statsTable(headers, rows, empty) {
  if (!rows.length) return word('p', 'text pending', empty);
  return el('div', 'table-scroll', el('table', 'stats-table',
    el('thead', null, el('tr', null, ...headers.map((h) => {
      const th = word('th', null, h);
      th.scope = 'col';
      return th;
    }))),
    el('tbody', null, ...rows.map((cells) => el('tr', null,
      ...cells.map((cell) => el('td', null, String(cell ?? '–'))))))));
}

async function showStats() {
  const s = await api('stats');
  return () => {
    draw(word('span', null, 'stats_heading'), [], [
      el('section', 'stat', word('h3', null, 'records_per_device'),
        statsTable(['device', 'records'], s.records.map((r) => [r.device, r.records]), 'no_records')),
      el('section', 'stat', word('h3', null, 'claims_by_kind'),
        statsTable(['kind', 'status', 'count'], s.claims.map((c) => [metadata(c.kind), metadata(c.status), c.count]), 'no_claims')),
      el('section', 'stat', word('h3', null, 'skipped_claims'),
        statsTable(['reason', 'count'], s.claim_skips.map((c) => [metadata(c.reason), c.count]), 'no_skipped_claims')),
      el('section', 'stat', word('h3', null, 'store'), el('dl', null,
        ...row('size', t('bytes', { bytes: s.bytes.toLocaleString('en-US'), mb: (s.bytes / 1048576).toFixed(1) })),
        ...row('rebuilding', t(s.rebuilding ? 'yes' : 'no')))),
      el('section', 'stat', word('h3', null, 'providers_recent'),
        statsTable(['provider', 'role', 'ok', 'calls_failed', 'waited', 'avg_ms'],
          s.providers.map((p) => [p.provider, metadata(p.role), p.ok, p.failed, p.waited, p.avg_ms]),
          'no_provider_calls')),
    ]);
    setStatus('');
  };
}

// --- Settings (#94): injection, capture and the curator chain, in config.toml ---------------
// The settings language also selects the feed and header's words. The server sends codes,
// and the page puts them in words.

const LANGS = ['en', 'ja'];
// Each string in the languages of LANGS, in that order.
const TEXT = {
  timeline: ['Timeline', 'タイムライン'],
  records: ['Records', '記録'],
  context: ['Context', 'コンテキスト'],
  stats: ['Stats', '統計'],
  settings: ['Settings', '設定'],
  views: ['Views', '表示'],
  repository: ['Repository', 'リポジトリ'],
  all_repositories: ['All repositories', 'すべてのリポジトリ'],
  repo_counts: ['{repo} ({claims} claims, {imported} imported, {records} records)', '{repo} (主張 {claims} 件、インポート {imported} 件、記録 {records} 件)'],
  current_checkout: ['Current checkout', '現在のチェックアウト'],
  last_activity: ['Last activity: {time}', '最終更新: {time}'],
  refresh: ['Refresh', '更新'],
  search: ['Search', '検索'],
  search_placeholder: ['Words from claims, imported history or records', '主張、インポートした履歴、記録に含まれる語句'],
  since: ['Since', '開始日'],
  until: ['Until (whole day included)', '終了日 (当日を含む)'],
  raw_records: ['Raw records', '元の記録'],
  raw_below: ['Below claims and imported history', '主張とインポートした履歴の下に表示'],
  raw_off: ['Off', '表示しない'],
  raw_only: ['Only raw records', '元の記録のみ'],
  history: ['Include ended claims in their place', '終了した主張も元の位置に表示'],
  theme: ['Theme: {theme}', 'テーマ: {theme}'],
  theme_auto: ['auto', '自動'],
  theme_light: ['light', 'ライト'],
  theme_dark: ['dark', 'ダーク'],
  theme_help: ['Follow the system, or force light or dark', 'システムに合わせるか、ライト・ダークを選択'],
  live: ['Live', '更新中'],
  live_off: ['Viewer unreachable', 'ビューアーに接続できません'],
  live_hint: ['Checks for new memory every few seconds', '数秒ごとに新しい記憶を確認します'],
  unreachable: ['The viewer is not answering. Start `oboete view` again and open the address it prints.', 'ビューアーから応答がありません。`oboete view` を起動し直し、表示されたアドレスを開いてください。'],
  more: ['More', 'もっと見る'],
  entries_loaded: ['{n} entries loaded.', '{n} 件を読み込みました。'],
  no_entries: ['No entries recorded yet.', 'まだ記録はありません。'],
  best_matches: ['The {n} best matches. Add words to narrow the search.', '一致度の高い {n} 件です。言葉を足すと絞り込めます。'],
  found: ['{n} found.', '{n} 件見つかりました。'],
  nothing_found: ['Nothing found.', '見つかりませんでした。'],
  loading: ['Loading…', '読み込み中…'],
  retry: ['Retry', '再試行'],
  full_text: ['Full text', '全文を表示'],
  full_text_of: ['Full text of {key}', '{key} の全文を表示'],
  view_claim: ['View claim', '主張を表示'],
  view_claim_id: ['View claim {uid}', '主張 {uid} を表示'],
  claim_title: ['Claim {uid}', '主張 {uid}'],
  owner_correction: ["Owner's correction", 'あなたによる訂正'],
  tier: ['Tier: {tier}', '生成段階: {tier}'],
  recipe: ['Recipe: {recipe}', '生成方法: {recipe}'],
  claim_status: ['Status: {status}', '状態: {status}'],
  status_unchanged: ['Status unchanged', '状態に変更はありません'],
  text_unchanged: ['Text unchanged', '本文に変更はありません'],
  delivered: ['Delivered', '記憶として渡す対象'],
  yes: ['Yes', 'はい'],
  no: ['No', 'いいえ'],
  speaker: ['Speaker', '発言者'],
  scope: ['Scope', '適用範囲'],
  later_claim: ['Later claim: ', '後の主張: '],
  evidence_quotes: ['Evidence quotes', '根拠となる引用'],
  no_evidence: ['No evidence quotes.', '根拠となる引用はありません。'],
  supersedes: ['Supersedes', '置き換えた主張'],
  ended_by: ['Ended by', 'この主張を終了させた主張'],
  none: ['None.', 'ありません。'],
  claim_history: ['History (oldest first)', '変更履歴(古い順)'],
  no_changes: ['No changes.', '変更はありません。'],
  earlier_decision: ['Earlier decision, paired with later claim: ', '後の主張と組で渡す以前の決定: '],
  superseded_by: ['Superseded by ', 'この主張を置き換えた主張: '],
  search_heading: ['Search: ', '検索: '],
  vector_notice: ['Results are full text only ({state}).', '全文検索のみの結果です({state})。'],
  vector_notice_reason: ['Results are full text only ({state}): {reason}', '全文検索のみの結果です({state}): {reason}'],
  vector_off: ['off', '無効'],
  vector_excluded: ['excluded', '除外対象'],
  'vector_no-vectors': ['no-vectors', '準備前'],
  vector_building: ['building', '準備中'],
  vector_waiting: ['waiting', '待機中'],
  vector_timeout: ['timeout', '時間切れ'],
  vector_error: ['error', '失敗'],
  vector_unavailable: ['unavailable', '利用できません'],
  vector_why_off: ['embedding is off', '意味に基づく検索は無効になっています。'],
  vector_why_excluded: ['this repository or the one searched is excluded, so the query is not sent out', 'このリポジトリまたは検索対象が除外されているため、検索語句を外部に送りません。'],
  'vector_why_no-vectors': ['no document has a vector yet', '意味に基づく検索に必要な記録の準備がまだできていません。'],
  vector_why_building: ["the new embedder's vectors are still being made", '新しい設定で意味に基づく検索を利用できるよう、記録を準備しています。'],
  vector_why_waiting: ['the embedder is resting, or its cap is spent', '意味に基づく検索は一時停止中、または利用上限に達しています。'],
  vector_why_timeout: ["the query's embedding took too long", '意味に基づく検索に必要な語句の処理が、制限時間内に終わりませんでした。'],
  vector_why_error: ['the query could not be embedded', '意味に基づく検索に必要な語句の処理に失敗しました。'],
  vector_why_unavailable: ['Vector search is unavailable.', '意味に基づく検索を利用できません。'],
  context_heading: ['Context handed to a new session', '新しいセッションに渡す記憶'],
  context_choose: ['Choose a repository above: Context shows what a new session there is handed.', '上でリポジトリを選んでください。そのリポジトリの新しいセッションに渡す記憶を表示します。'],
  context_checkout: ["Context shows one checkout; All repositories uses the viewer's checkout.", 'この画面には、1 つの作業場所で渡す記憶を表示します。「すべてのリポジトリ」を選ぶと、ビューアーを起動した作業場所の記憶を表示します。'],
  branch: ['Branch', 'ブランチ'],
  session_start: ['SessionStart', 'セッション開始時の受け渡し'],
  context_on: ['On', '有効'],
  context_off: ['Off', '無効'],
  size: ['Size', '大きさ'],
  characters: ['{n} characters', '{n} 文字'],
  context_empty: ['Nothing is handed over for this checkout yet.', 'この作業場所で渡す記憶はまだありません。'],
  stats_heading: ['Stats (all repositories)', '統計(すべてのリポジトリ)'],
  records_per_device: ['Records per device', '端末ごとの記録数'],
  device: ['Device', '端末'],
  no_records: ['No records.', '記録はありません。'],
  claims_by_kind: ['Claims by kind and status', '種類・状態ごとの主張数'],
  kind: ['Kind', '種類'],
  status: ['Status', '状態'],
  count: ['Count', '件数'],
  no_claims: ['No claims.', '主張はありません。'],
  skipped_claims: ['Skipped claims', '採用されなかった主張'],
  reason: ['Reason', '理由'],
  no_skipped_claims: ['No skipped claims.', '採用されなかった主張はありません。'],
  store: ['Store', '保存領域'],
  bytes: ['{bytes} bytes ({mb} MB)', '{bytes} バイト({mb} メガバイト)'],
  rebuilding: ['Rebuilding', '記憶を再構築中'],
  providers_recent: ['Providers, last 7 days', '過去 7 日間の AI の呼び出し'],
  provider: ['Provider', '接続先'],
  role: ['Role', '役割'],
  ok: ['OK', '成功'],
  calls_failed: ['Failed', '失敗'],
  waited: ['Waited', '待機'],
  avg_ms: ['Avg ms', '平均時間(ミリ秒)'],
  no_provider_calls: ['No provider calls in the last seven days.', '過去 7 日間に AI の呼び出しはありません。'],
  meta_current: ['current', '現在の主張'],
  meta_delivered: ['delivered', '記憶として渡す対象'],
  meta_superseded: ['superseded', '置き換え済み'],
  meta_imported: ['imported', 'インポート済み'],
  meta_card: ['card', 'カード'],
  meta_summary: ['summary', '要約'],
  meta_raw: ['raw', '元の記録'],
  meta_claim: ['claim', '主張'],
  meta_start: ['start', '開始'],
  'meta_session start': ['session start', 'セッション開始'],
  meta_prompt: ['prompt', 'プロンプト'],
  meta_reply: ['reply', 'エージェントの応答'],
  meta_tool: ['tool', 'ツール'],
  meta_envelope: ['envelope', '実行環境からの情報'],
  meta_compaction: ['compaction', '会話の圧縮'],
  meta_citable: ['citable', '元の記録を参照できます'],
  'meta_quote-only': ['quote-only', '引用のみ'],
  meta_decision: ['decision', '決定'],
  meta_preference: ['preference', '好み・方針'],
  meta_lesson: ['lesson', '教訓'],
  meta_fix: ['fix', '修正'],
  meta_bugfix: ['bugfix', '不具合の修正'],
  meta_feature: ['feature', '機能'],
  meta_discovery: ['discovery', '発見'],
  meta_refactor: ['refactor', '構造の整理'],
  meta_security_alert: ['security_alert', '安全性についての警告'],
  meta_security_note: ['security_note', '安全性についての補足'],
  meta_sensitive: ['sensitive', '慎重に扱う情報'],
  'meta_open item': ['open item', '未完了の項目'],
  'meta_repo fact': ['repo fact', 'リポジトリについての事実'],
  meta_change: ['change', '変更'],
  meta_decided: ['decided', '決定済み'],
  meta_proposed: ['proposed', '提案中'],
  meta_retracted: ['retracted', '撤回済み'],
  meta_done: ['done', '完了'],
  meta_unverified: ['unverified', '未確認'],
  meta_user: ['user', 'ユーザー'],
  'meta_assistant proposal': ['assistant proposal', 'エージェントの提案'],
  'meta_assistant inferred': ['assistant inferred', 'エージェントの推測'],
  'meta_tool result': ['tool result', 'ツールの結果'],
  meta_repo: ['repo', 'このリポジトリ'],
  meta_global: ['global', 'すべてのリポジトリ'],
  meta_curator: ['curator', '要約'],
  meta_embed: ['embed', '検索用の処理(記録)'],
  meta_query: ['query', '検索用の処理(検索語句)'],
  'meta_not a claim': ['not a claim', '主張の形式ではありません'],
  'meta_no evidence': ['no evidence', '根拠がありません'],
  'meta_a quote no longer reads in raw': ['a quote no longer reads in raw', '引用した元の記録を参照できなくなりました'],
  'meta_not a correction': ['not a correction', '訂正の形式ではありません'],
  'meta_not a claim uid': ['not a claim uid', '主張の識別子が正しくありません'],
  'meta_corrects nothing': ['corrects nothing', '訂正する内容がありません'],
  'meta_an unknown status': ['an unknown status', '対応していない状態です'],
  'meta_an empty body': ['an empty body', '本文がありません'],
  'meta_over the 1,000-character cap': ['over the 1,000-character cap', '本文が 1,000 文字の上限を超えています'],
  request_error: ['{status}: {reason}', '{status}: {reason}'],
  not_found: ['not found', '見つかりませんでした。'],
  doc_missing: ['no such document', 'この記録は見つかりませんでした。'],
  claim_missing: ['no such claim', 'この主張は見つかりませんでした。'],
  memory_busy: ['the memory is being restored or rebuilt: try again in a moment', '記憶を復元または再構築しています。少し待ってから、もう一度お試しください。'],
  load_failed: ['Could not load data ({status}).', '読み込めませんでした({status})。もう一度お試しください。'],
  feed_loading: ['Loading…', '読み込み中…'],
  feed_empty: ['No items to display', '表示する項目はありません'],
  feed_exhausted: ['No more items', 'すべての項目を表示しました'],
  feed_error: ['Could not load items: {reason}', '項目を読み込めませんでした: {reason}'],
  feed_card: ['card', 'カード'],
  feed_summary: ['Session summary', 'セッションの要約'],
  feed_prompt: ['Prompt', 'プロンプト'],
  untitled: ['Untitled', 'タイトルなし'],
  facts: ['facts', '事実'],
  narrative: ['narrative', '説明'],
  files_read: ['read:', '参照:'],
  files_modified: ['modified:', '変更:'],
  investigated: ['Investigated', '調査したこと'],
  learned: ['Learned', '学んだこと'],
  completed: ['Completed', '完了したこと'],
  next_steps: ['Next steps', '次の作業'],
  welcome_help: ['Show welcome', '使い方を表示'],
  welcome_title: ['Welcome to oboete', 'oboete へようこそ'],
  welcome_close: ['Close welcome', '使い方を閉じる'],
  welcome_close_hint: ['Close (Esc)', '閉じる (Esc)'],
  welcome_feed_h: ['The feed', 'フィード'],
  welcome_feed: ['Cards, session summaries and prompts appear here as the worker curates your work, a few minutes after it happens.', '作業の数分後、ワーカーがまとめたカード、セッションの要約、プロンプトがここに表示されます。'],
  welcome_settings_h: ['Settings', '設定'],
  welcome_settings: ['The Settings tab controls what a new session receives and who curates the work.', '設定タブで、新しいセッションに渡す記憶と、作業をまとめる要約役を選べます。'],
  welcome_recall_h: ['Recall', '思い出す'],
  welcome_recall: ['Your agent asks oboete’s search, timeline and get to recall past work. The Records tab searches it here.', 'エージェントは oboete の search、timeline、get で過去の作業を探します。この画面では記録タブから検索できます。'],
  heading: ['Settings', '設定'],
  resident_h: ['Between sessions', 'セッション間の常駐'],
  resident_on: ['Keep oboete running between sessions', 'セッション間も oboete を起動したままにする'],
  resident_desc: ['The worker stays ready and keeps the memory page reachable. Turn this off to let the worker exit when idle.', 'ワーカーを待機させ、記憶の画面をいつでも開けるようにします。オフにすると、ワーカーは処理がなくなった時に終了します。'],
  resident_first: ['Recommended for a new home. Save applies the choice shown here.', '新しい保存先ではオンを推奨します。保存すると、ここで選んだ設定が反映されます。'],
  resident_timing: ['The next agent hook or opening the page starts the resident processes. Turning this off applies when the worker is idle; the resident page stops once it has no requests.', '常駐を開始するのは次のエージェントのフックか、次に画面を開く操作です。オフはワーカーの待機時に反映され、常駐の画面はアクセスがなくなった後に終了します。'],
  resident_unsupported: ['Resident mode is currently available on Linux and WSL. This system keeps the worker that exits when idle.', '常駐は現在 Linux と WSL に対応しています。この環境のワーカーは、処理がなくなった時に終了します。'],
  language: ['Language', '言語'],
  lead: [
    'Choose the settings below. Edits stay on this page until you save them in config.toml. Recording and memory delivery read them at their next use; the background summarizer reads them before its next window, even while it stays running. This page cannot tell which values a running request has loaded. Opening this page or saving sends nothing to a provider.',
    'ここで設定を選べます。変更は「保存」を押すまでこの画面だけに残ります。記録・記憶の受け渡しには次の利用時から反映されます。要約の設定は、次のまとまりを処理する前に読み直すので、処理が動き続けていても反映されます。現在実行中の呼び出しが読み込んでいる値は、この画面では確認できません。画面を開いたり保存したりしても、要約役への送信は始まりません。',
  ],
  summary_h: ['Summarizing recorded activity', '記録の要約'],
  summary_desc: [
    'When enabled, the background summarizer may send recorded text to the providers below. Paid calls can cost money. Turning it off keeps existing memories and stops new summaries at the next run.',
    '有効にすると、バックグラウンドの要約処理が記録した本文を下の要約役へ送ることがあります。有料の呼び出しには費用がかかります。無効にしても既存の記憶は残り、次の処理で設定を読み直すと、新しい要約を止めます。',
  ],
  curate_on: ['Summarize recorded activity (default: off)', '記録を要約する(既定: 切)'],
  summary_language: ['Language of summaries', '要約を書く言語'],
  summary_language_desc: [
    'Write the language you want, such as English. Default: Japanese. Changing the language does not start a call.',
    '要約を書いてほしい言語を自由に入力します。例: English。既定は Japanese(日本語)です。言語を変えても呼び出しは始まりません。',
  ],
  summary_advanced: ['Text size and waiting time', '一度に送る大きさと待ち時間'],
  window_tokens: ['Text per request ({min} to {max} estimated tokens)', '一度に送る大きさ(推定トークン数、{min}〜{max})'],
  window_tokens_desc: [
    'Default: 5,000. Tokens are small pieces of text counted by a model. Larger requests can send more text and cost more. Saving does not send a request.',
    '既定は 5,000 です。トークンはモデルが数える小さな本文の単位です。大きくすると一度に送る本文が増え、費用が増えることがあります。保存しても送信は始まりません。',
  ],
  idle_minutes: ['Wait after the last activity (minutes, {min} to {max})', '最後の記録から待つ時間(分、{min}〜{max})'],
  idle_minutes_desc: [
    'Default: 10 minutes. Waits for more activity before sending a short request. Zero skips this wait. The actual wait is at most 30 minutes, even when a larger value is saved. Changing it does not start a paid call.',
    '既定は 10 分です。短い記録を送る前に、続きの記録を待ちます。0 ではこの待ち時間をなくします。大きい値を保存しても実際に待つのは最大 30 分です。変更しても有料の呼び出しは始まりません。',
  ],
  saved_value: ['Saved: {value}', '保存済み: {value}'],
  value_on: ['On', '入'],
  value_off: ['Off', '切'],
  spending_h: ['Paid calls and spending', '有料の呼び出しと利用額'],
  paid_cap: ['Monthly limit for paid calls (USD)', '有料の呼び出しの月額上限(米ドル)'],
  paid_cap_desc: [
    'Default: USD 5 per calendar month (UTC). Zero stops paid calls; free and subscription providers can still be used. This limit covers the paid calls recorded by oboete. Search embeddings have their own limit. Saving the limit costs nothing.',
    '既定は暦月ごとに 5 米ドル(UTC)です。0 では有料の呼び出しを止め、無料・サブスクリプションの要約役は引き続き使えます。oboete が記録する有料の呼び出しが対象で、検索用の処理には別の上限があります。上限を保存するだけでは費用はかかりません。',
  ],
  month_spend: ['Recorded spending this month (UTC): {usd}', '今月の記録された利用額(UTC): {usd}'],
  no_spend: ['Nothing has been spent this month (UTC).', '今月(UTC)の利用額はありません。'],
  spend_unavailable: ['This month\'s spending could not be read.', '今月の利用額を読み込めませんでした。'],
  gemini_label: ['Where to add Gemini', 'Gemini を加える場所'],
  gemini_none: ['Do not add automatically (default)', '自動では加えない(既定)'],
  gemini_before: ['Before subscription providers', 'サブスクリプションの要約役の前'],
  gemini_after: ['After subscription providers', 'サブスクリプションの要約役の後'],
  gemini_desc: [
    'Gemini sends text to Google and can cost money within the paid limit. An existing Gemini entry keeps its place, and the order below takes priority. To stop using that entry, turn off its Use checkbox. Saving this choice sends nothing.',
    'Gemini は本文を Google に送り、有料の上限内で費用がかかることがあります。すでに登録した Gemini は元の場所に残り、下で指定した順番が優先されます。その要約役を使わない場合は「使う」のチェックを外します。この選択を保存しても送信は始まりません。',
  ],
  stopped_h: ['Providers waiting for you', '再開の操作を待っている要約役'],
  stopped_desc: [
    'Use again clears the stop and makes waiting work eligible at the next background run. It keeps the Use checkbox and spending limits. Later calls may cost money. This button sends nothing and does not start the summarizer.',
    '「再び使う」で停止を解除し、待っている処理を次のバックグラウンド処理で再試行できる状態に戻します。「使う」のチェックと利用額の上限は維持します。後の呼び出しには費用がかかることがあります。このボタンでは送信や要約処理の起動はしません。',
  ],
  stopped_none: ['No providers are waiting for your action.', '再開の操作を待っている要約役はありません。'],
  resume: ['Use again', '再び使う'],
  resume_label: ['Use {name} again', '{name} を再び使う'],
  resumed: ['The stop was cleared. It can be used at the next eligible background run.', '停止を解除しました。条件が整えば次のバックグラウンド処理で使われます。'],
  not_stopped: ['This provider was already available.', 'この要約役の停止はすでに解除されていました。'],
  providers_unavailable: ['Provider stops could not be read or changed. Please try again.', '要約役の停止状態を読み込み・変更できませんでした。もう一度お試しください。'],
  resume_failed: ['The provider could not be resumed ({status}).', '要約役を再開できませんでした({status})。'],
  network_failed: ['The viewer could not be reached. Please try again.', 'ビューアーに接続できませんでした。もう一度お試しください。'],
  inject_h: ['Handing memory to agents', '記憶の受け渡し'],
  inject_desc: [
    'The summary of your memory that an agent is given when a session starts.',
    'セッションの開始時に、エージェントへ渡す記憶のまとめです。',
  ],
  inject_on: ['Give agents the summary of your memory', 'エージェントに記憶のまとめを渡す'],
  inject_chars: ['Size in characters ({min} to {max})', '大きさ(文字数、{min}〜{max})'],
  per_prompt_on: [
    'Also give the decisions that match each prompt (off until it has been measured)',
    'プロンプトごとに、関係する決定も渡す(測定が済むまでは切)',
  ],
  per_prompt_chars: ['Their size in characters ({min} to {max})', 'その大きさ(文字数、{min}〜{max})'],
  correction_on: [
    'Tell the agent at the next prompt when a decision it was given has changed',
    '渡した決定が変わったときは、次のプロンプトでエージェントに伝える',
  ],
  correction_chars: ['Its size in characters ({min} to {max})', 'その大きさ(文字数、{min}〜{max})'],
  capture_h: ['Recording', '記録'],
  capture_desc: ['What is recorded from each session.', '各セッションから記録する内容です。'],
  store_prompts: ['Keep the text of your prompts', 'プロンプトの本文を保存する'],
  tool_output: ['Output of tools', 'ツールの出力'],
  tool_full: ['Keep it whole (the start and end when very long)', 'すべて残す(非常に長いときは先頭と末尾)'],
  tool_head_tail: ['Keep only the start and end', '先頭と末尾だけ残す'],
  chain_h: ['Curators', '要約役'],
  chain_desc: [
    'The curators are asked in this order. An empty field follows the curator\'s own value, shown in grey.',
    '要約役は上から順に使われます。空欄の項目は、その要約役の既定の値(灰色で表示)に従います。',
  ],
  col_order: ['Order', '順番'],
  col_on: ['Use', '使う'],
  col_name: ['Curator', '要約役'],
  col_model: ['Model', 'モデル'],
  col_budget: ['Calls a day', '1 日の回数'],
  col_timeout: ['Timeout (s)', '待ち時間(秒)'],
  up: ['Move up', '上へ移動'],
  down: ['Move down', '下へ移動'],
  key_ok: ['Key found', 'キーがあります'],
  key_missing: ['Key file not found', 'キーのファイルが見つかりません'],
  key_none: ['No key needed', 'キーは不要です'],
  key_on_path: ['Installed', 'インストール済み'],
  key_not_on_path: ['Not installed', 'インストールされていません'],
  key_file: ['Key file: {path}', 'キーのファイル: {path}'],
  key_label: ['New key for {name}', '{name} の新しいキー'],
  key_placeholder: ['Paste a new key', '新しいキーを貼り付け'],
  key_save: ['Save key', 'キーを保存'],
  key_saved: ['The key was saved in its file.', 'キーをファイルに保存しました。'],
  key_not_durable: [
    'The key was saved in its file, but the disk did not confirm the write. If the computer loses power soon, check the key file.',
    'キーをファイルに保存しましたが、ディスクへの書き込みを確認できませんでした。この後すぐに電源が切れた場合は、キーのファイルを確認してください。',
  ],
  key_by_hand: [
    'On this computer, set the key by editing its file.',
    'このコンピューターでは、キーはファイルを直接編集して設定してください。',
  ],
  bad_key: [
    'A key is 8 to 512 characters: letters, digits and . _ ~ + / = : - (no spaces).',
    'キーは 8〜512 文字で、使えるのは英数字と . _ ~ + / = : - だけです(空白は使えません)。',
  ],
  bad_entry: ['This curator could not be found. Please reload the page.', 'この要約役が見つかりません。ページを再読み込みしてください。'],
  no_entry: ['This curator could not be found. Please reload the page.', 'この要約役が見つかりません。ページを再読み込みしてください。'],
  no_key_file: ['This curator has no key file in config.toml.', 'この要約役には config.toml でキーのファイルが設定されていません。'],
  ambiguous: [
    'Curators of this name use different key files, so please set each key by editing its file.',
    'この名前の要約役がそれぞれ別のキーのファイルを使っているため、キーはファイルを直接編集して設定してください。',
  ],
  not_absolute: [
    'To save a key here, write the key file in config.toml as a full path (starting with /).',
    'この画面からキーを保存するには、config.toml のキーのファイルを / から始まる完全なパスで書いてください。',
  ],
  not_a_key_file: [
    'This page writes only a key file whose name ends in _KEY.md.',
    'この画面から書き込めるのは、名前が _KEY.md で終わるキーのファイルだけです。',
  ],
  protected: [
    'This page does not write a key file inside the oboete folder.',
    'oboete のフォルダの中にあるキーのファイルには、この画面から書き込みません。',
  ],
  no_dir: [
    'The folder for the key file does not exist. Please create it first.',
    'キーのファイルを置くフォルダがありません。先にフォルダを作成してください。',
  ],
  not_a_file: [
    'The key file\'s path is not a plain file (it is a link or a folder, for example), so nothing was changed.',
    'キーのファイルのパスが通常のファイルではない(リンクやフォルダなど)ため、何も変更しませんでした。',
  ],
  too_big: ['The key file is larger than 64 KiB, so nothing was changed.', 'キーのファイルが 64 KiB を超えているため、何も変更しませんでした。'],
  not_utf8: ['The key file is not UTF-8 text, so nothing was changed.', 'キーのファイルが UTF-8 のテキストではないため、何も変更しませんでした。'],
  not_private: [
    'The key file is on a drive where this page cannot keep it private (a Windows drive seen from WSL, a network share, or a FUSE mount such as sshfs), so it was not written.',
    'キーのファイルが、ほかのユーザーから読めないことをこの画面では保証できないドライブ(WSL から見た Windows のドライブ、ネットワーク共有、sshfs などの FUSE)にあるため、書き込みませんでした。',
  ],
  shared_folder: [
    'The folder for the key file, or a folder above it, can be changed by another user of this computer (the group or everyone may write to it, or its owner is neither you nor root), so the key was not written. Please remove that write permission from the folder (chmod go-w), or use a folder only you can write to.',
    'キーのファイルを置くフォルダ、またはその上のフォルダを、このコンピューターのほかのユーザーが変更できる状態(グループまたは全員が書き込める、あるいは所有者があなたでも root でもない)のため、書き込みませんでした。そのフォルダから書き込み権限を外す(chmod go-w)か、あなただけが書き込めるフォルダをお使いください。',
  ],
  changed: [
    'The key file changed while the key was being saved, so it was not overwritten. Please try again.',
    'キーの保存中にキーのファイルが変更されたため、上書きしませんでした。もう一度お試しください。',
  ],
  unsupported: [
    'On this computer, set the key by editing its file.',
    'このコンピューターでは、キーはファイルを直接編集して設定してください。',
  ],
  failed: ['The key file could not be written. It is as it was.', 'キーのファイルに書き込めませんでした。ファイルは元のままです。'],
  entries: [
    '{n} curators share this name in config.toml, and these settings change all of them.',
    'config.toml でこの名前の要約役が {n} つあり、ここでの設定はすべてに反映されます。',
  ],
  from_key: [
    'From its key: a fifth of the key\'s daily limit ({n} now)',
    'キーから決まります: キーの 1 日の上限の 5 分の 1(現在 {n} 回)',
  ],
  no_cap: ['No cap', '上限なし'],
  model_fixed: [
    'This curator has prices for its model, so its model is changed in [[providers]] together with its prices.',
    'この要約役にはモデルの料金が設定されているため、モデルは [[providers]] で料金と一緒に変更してください。',
  ],
  model_free: ['Only models whose names end in :free.', '名前が :free で終わるモデルだけ設定できます。'],
  model_unapplied: [
    'config.toml sets this model, but this curator cannot use it, so it is not applied.',
    'config.toml でこのモデルが設定されていますが、この要約役では使えないため、適用されていません。',
  ],
  differs: [
    'The curators of this name use different values for: {what}. This row shows the first one\'s.',
    'この名前の要約役は、使う値が次の点で異なります: {what}。この行には最初の要約役の値を表示しています。',
  ],
  differs_key_file: ['key file', 'キーのファイル'],
  differs_model: ['model', 'モデル'],
  differs_daily_budget: ['calls a day', '1 日の回数'],
  differs_timeout_s: ['timeout', '待ち時間'],
  save: ['Save', '保存'],
  saved: ['Saved. Each setting takes effect at the time described beside it.', '保存しました。反映されるタイミングは各設定の説明をご確認ください。'],
  warnings_h: ['Notes on config.toml', 'config.toml についての注意'],
  file_error: [
    'config.toml has a mistake, so this page shows no settings. Run `oboete doctor` to see the line.',
    'config.toml に誤りがあるため、設定を表示できません。`oboete doctor` で該当する行を確認してください。',
  ],
  stale: [
    'config.toml was changed elsewhere. The page now shows its current values; please make your change again.',
    'config.toml がほかの場所で変更されました。現在の値を表示し直しましたので、もう一度変更してください。',
  ],
  range: ['This value is out of range.', 'この値は範囲外です。'],
  names: ['The list of curators has changed. Please reload the page.', '要約役の一覧が変わりました。ページを再読み込みしてください。'],
  model: [
    'A model name uses letters, digits and . _ : / @ + - (200 characters at most).',
    'モデル名に使えるのは英数字と . _ : / @ + - だけです(200 文字まで)。',
  ],
  paid_model: [
    'This model could be billed outside the monthly limit, so it cannot be set here.',
    'このモデルは月の上限の外で課金されるおそれがあるため、ここでは設定できません。',
  ],
  chain_empty: [
    'Please keep at least one provider in use. To stop summaries, turn off Summarize recorded activity above.',
    '要約役を少なくとも 1 つは使う設定にしてください。要約を止めるには、上の「記録を要約する」を切にします。',
  ],
  file_invalid: [
    'config.toml has a mistake, so this page cannot change it. Run `oboete doctor` to see the line.',
    'config.toml に誤りがあるため、この画面からは変更できません。`oboete doctor` で該当する行を確認してください。',
  ],
  type: [
    'The page sent a value of the wrong kind. Please reload the page.',
    '画面から誤った種類の値が送られました。ページを再読み込みしてください。',
  ],
  bad_request: ['The request was not understood. Please reload the page.', '要求を処理できませんでした。ページを再読み込みしてください。'],
  too_large: ['The request is too large.', '要求が大きすぎます。'],
  write_failed: ['config.toml could not be written.', 'config.toml に書き込めませんでした。'],
  unauthorized: [
    'This page needs the full address printed by `oboete view` (it carries the access key after #).',
    'このページは `oboete view` が表示するアドレス全体(# の後ろのアクセスキーを含む)で開いてください。',
  ],
  forbidden: [
    'Please open the viewer through the address `oboete view` prints.',
    '`oboete view` が表示するアドレスから開いてください。',
  ],
  other: ['Saving failed ({status}).', '保存できませんでした({status})。'],
};

const stored = recall('oboete-lang', '');
const browserLang = (navigator.language || '').toLowerCase().startsWith('ja') ? 'ja' : 'en';
let lang = LANGS.includes(stored) ? stored : browserLang;

// A string of the current language; `{name}` takes vars.name.
function t(key, vars = {}) {
  const text = Object.hasOwn(TEXT, key) ? TEXT[key][LANGS.indexOf(lang)] : key;
  return text.replace(/\{(\w+)\}/g, (_, k) => (Object.hasOwn(vars, k) ? String(vars[k]) : ''));
}

function applyLanguage() {
  document.documentElement.lang = lang;
  const labels = { 'repo-label': 'repository', refresh: 'refresh', 'search-label': 'search',
    'search-button': 'search', 'since-label': 'since', 'until-label': 'until',
    'raw-label': 'raw_records', 'history-label': 'history' };
  for (const [id, key] of Object.entries(labels)) $(id).textContent = t(key);
  $('q').placeholder = t('search_placeholder');
  for (const option of $('raw').options) option.textContent = t(`raw_${option.value}`);
  $('tabs').setAttribute('aria-label', t('views'));
  for (const button of document.querySelectorAll('#tabs .tab')) button.textContent = t(button.dataset.view);
  for (const node of document.querySelectorAll('[data-i18n]')) {
    node.textContent = t(node.dataset.i18n, JSON.parse(node.dataset.i18nVars || '{}'));
  }
  for (const node of document.querySelectorAll('[data-i18n-aria]')) {
    node.setAttribute('aria-label', t(node.dataset.i18nAria, JSON.parse(node.dataset.i18nVars || '{}')));
  }
  for (const node of document.querySelectorAll('[data-feed-timestamp]')) {
    node.textContent = new Date(Number(node.dataset.feedTimestamp)).toLocaleString(lang);
  }
  for (const option of $('repo').options) labelRepo(option);
  $('live').textContent = t($('live').classList.contains('off') ? 'live_off' : 'live');
  $('live').title = t('live_hint');
  $('help').title = t('welcome_help');
  $('help').setAttribute('aria-label', t('welcome_help'));
  $('welcome-title').textContent = t('welcome_title');
  $('welcome-close').title = t('welcome_close_hint');
  $('welcome-close').setAttribute('aria-label', t('welcome_close'));
  $('welcome-parts').replaceChildren(...['feed', 'settings', 'recall'].map((part) =>
    el('section', null, el('h3', null, t(`welcome_${part}_h`)), el('p', null, t(`welcome_${part}`)))));
  applyTheme();
  if (activeFeed(feed)) {
    $('heading').textContent = t('timeline');
    updateFeedState(feed);
  }
}

function labelRepo(option) {
  if (!option.value) option.textContent = t('all_repositories');
  else {
    option.textContent = t('repo_counts', option.dataset);
    option.title = `${option.value}${option.value === currentRepo ? ` (${t('current_checkout')})` : ''}${option.dataset.last === undefined ? '' : `; ${t('last_activity', { time: new Date(Number(option.dataset.last)).toLocaleString(lang) })}`}`;
  }
}

// Welcome behaves as a modal for both mouse and keyboard, and returns focus to its opener.
let welcomeFocus = null;
let welcomeInert = [];
let welcomeOverflow = '';

function openWelcome() {
  if (!$('welcome').hidden) return;
  welcomeFocus = document.activeElement;
  welcomeInert = [document.querySelector('.bar'), document.querySelector('main')].map((node) => [node, node.inert]);
  for (const [node] of welcomeInert) node.inert = true;
  welcomeOverflow = document.body.style.overflow;
  document.body.style.overflow = 'hidden';
  $('welcome').hidden = false;
  $('welcome-close').focus();
}

function closeWelcome() {
  if ($('welcome').hidden) return;
  $('welcome').hidden = true;
  remember('oboete-welcome-dismissed', 'true');
  for (const [node, wasInert] of welcomeInert) node.inert = wasInert;
  document.body.style.overflow = welcomeOverflow;
  if (welcomeFocus?.isConnected && welcomeFocus !== document.body) welcomeFocus.focus();
  else $('help').focus();
}

function welcomeKey(event) {
  if ($('welcome').hidden) return;
  if (event.key === 'Escape') {
    event.preventDefault();
    closeWelcome();
  } else if (event.key === 'Tab') {
    const dialog = $('welcome-dialog');
    const focusable = [...dialog.querySelectorAll('button, a[href], input, select, textarea, [tabindex]:not([tabindex="-1"])')]
      .filter((node) => !node.disabled && !node.hidden);
    const first = focusable[0] || dialog;
    const last = focusable.at(-1) || dialog;
    if (!dialog.contains(document.activeElement) || (event.shiftKey ? document.activeElement === first : document.activeElement === last)) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    }
  }
}

// The form's values between redraws: a language switch or a move keeps what is not saved yet.
let form = null;
// `[inject]`'s sizes, each checked against the range the server states for it.
const SIZES = ['session_start_chars', 'per_prompt_chars', 'correction_chars'];

function formOf(s) {
  if (s.error) return null;
  const text = (v) => (v === null || v === undefined ? '' : String(v));
  return {
    version: s.version,
    saved: { summary: { ...s.summary }, paid_usd_per_month: s.paid_usd_per_month, gemini: s.gemini,
      worker: { resident: s.worker?.resident ?? false } },
    firstRun: s.first_run === true,
    residentSupported: s.resident_supported === true,
    worker: { resident: s.first_run && s.resident_supported ? true : s.worker?.resident ?? false },
    summary: { ...s.summary, window_tokens: String(s.summary.window_tokens), idle_minutes: String(s.summary.idle_minutes) },
    paid_usd_per_month: String(s.paid_usd_per_month),
    gemini: s.gemini ?? 'none',
    usd_this_month: s.usd_this_month,
    stopped: s.stopped,
    inject: { ...s.inject, ...Object.fromEntries(SIZES.map((k) => [k, String(s.inject[k])])) },
    capture: { ...s.capture },
    chain: s.chain.map((e) => ({
      ...e,
      edit: { on: e.on, daily_budget: text(e.daily_budget), timeout_s: text(e.timeout_s), model: text(e.model) },
    })),
    warnings: s.warnings,
    ranges: s.ranges,
    keyInput: s.key_input,
  };
}

async function showSettings() {
  const s = await api('settings');
  return () => {
    form = formOf(s);
    drawSettings();
    setStatus('');
  };
}

function checkbox(checked, onChange) {
  const c = el('input');
  c.type = 'checkbox';
  c.checked = checked;
  c.addEventListener('change', () => onChange(c.checked));
  return c;
}

// A text or number field that keeps its value in the form as it is typed; `field` is the name
// the server uses when it refuses the value.
function input(type, value, placeholder, field, onInput) {
  const i = el(type === 'textarea' ? 'textarea' : 'input');
  if (type === 'textarea') i.rows = 2;
  else i.type = type;
  if (type === 'number') {
    i.inputMode = 'numeric';
    i.step = '1';
  } else {
    i.autocomplete = 'off';
    i.spellcheck = false;
  }
  i.value = value;
  i.placeholder = placeholder;
  i.dataset.field = field;
  i.addEventListener('input', () => {
    i.classList.remove('invalid');
    i.removeAttribute('aria-invalid');
    onInput(i.value);
  });
  return i;
}

function note(text) {
  return el('span', 'note', text);
}

function stoppedState() {
  const state = el('div', 'stopped-state');
  if (form.stopped === null) state.append(note(t('providers_unavailable')));
  else if (!form.stopped.length) state.append(note(t('stopped_none')));
  else state.append(el('ul', 'stopped', ...form.stopped.map((name) => {
    const button = el('button', 'quiet small', t('resume'));
    button.type = 'button';
    button.setAttribute('aria-label', t('resume_label', { name }));
    button.addEventListener('click', () => void resumeProvider(name, button));
    return el('li', null, el('span', 'entry-name', name), button);
  })));
  return state;
}

async function resumeProvider(name, button) {
  const mine = form;
  const fields = button.closest('.settings');
  const state = button.closest('.stopped-state');
  button.disabled = true;
  fields.inert = true;
  try {
    const res = await fetch('/api/resume', {
      method: 'POST',
      headers: { 'X-Oboete-Token': token, 'Content-Type': 'application/json' },
      body: JSON.stringify({ provider: name }),
      referrerPolicy: 'same-origin',
      credentials: 'omit',
    });
    const answer = (res.headers.get('content-type') || '').startsWith('application/json') ? await res.json() : {};
    if (view !== 'settings' || form !== mine) return;
    if (res.ok) {
      mine.stopped = mine.stopped.filter((provider) => provider !== answer.provider);
      state.replaceWith(stoppedState());
      setStatus(t(answer.resumed ? 'resumed' : 'not_stopped'), false, lang);
      return;
    }
    const byStatus = { 400: 'bad_request', 401: 'unauthorized', 403: 'forbidden', 413: 'too_large' };
    setStatus(t(answer.code || byStatus[res.status] || 'resume_failed', { status: res.status }), true, lang);
  } catch {
    if (view === 'settings' && form === mine) setStatus(t('network_failed'), true, lang);
  } finally {
    button.disabled = false;
    fields.inert = false;
  }
}

function keyState(r) {
  const state = el('span', 'note', t(`key_${r.key.replaceAll('-', '_')}`));
  if (!r.key_file) return [state];
  return [state, note(t('key_file', { path: r.key_file })), form.keyInput ? keyField(r, state) : note(t('key_by_hand'))];
}

// A new key for the row's key file (#94 part 3). What is typed leaves the page only in the
// request's body, and the field is emptied before the request goes. A text field masked by CSS,
// not a password field: a browser offers to save a password field's typed value once the field
// leaves the page after a request, and this page redraws. `autocomplete` off keeps the value out
// of form history and saved page state.
function keyField(r, state) {
  const i = el('input');
  i.type = 'text';
  i.autocomplete = 'off';
  i.spellcheck = false;
  i.autocapitalize = 'off';
  i.placeholder = t('key_placeholder');
  i.dataset.field = `chain.${r.name}.key`;
  i.setAttribute('aria-label', t('key_label', { name: r.name }));
  const save = el('button', 'small', t('key_save'));
  save.type = 'button';
  save.disabled = true;
  i.addEventListener('input', () => {
    i.classList.remove('invalid');
    i.removeAttribute('aria-invalid');
    save.disabled = i.value === '';
  });
  // Enter saves the key, not the settings form around it.
  i.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter') return;
    e.preventDefault();
    if (!save.disabled) save.click();
  });
  save.addEventListener('click', () => void saveKey(r.name, i, save, state));
  return el('span', 'key-input', i, save);
}

async function saveKey(name, field, button, state) {
  const body = JSON.stringify({ entry: name, key: field.value, version: form.version });
  field.value = '';
  button.disabled = true;
  const mine = form;
  const fields = button.closest('.settings');
  fields.inert = true;
  try {
    const res = await fetch('/api/key', {
      method: 'POST',
      headers: { 'X-Oboete-Token': token, 'Content-Type': 'application/json' },
      body,
      referrerPolicy: 'same-origin',
      credentials: 'omit',
    });
    const answer = (res.headers.get('content-type') || '').startsWith('application/json') ? await res.json() : {};
    const current = answer.code === 'stale' ? await api('settings') : null;
    if (view !== 'settings' || form !== mine) return;
    if (current) {
      form = formOf(current);
      drawSettings();
      setStatus(t('stale'), true, lang);
      return;
    }
    if (res.ok) {
      // Only the row's key state changes, in place: what is typed elsewhere and not saved yet
      // stays, a key in another row included.
      for (const r of form.chain) if (r.name === answer.entry) r.key = answer.key;
      state.textContent = t(`key_${answer.key}`);
      setStatus(t(answer.durable ? 'key_saved' : 'key_not_durable'), !answer.durable, lang);
      return;
    }
    const byStatus = { 400: 'bad_request', 401: 'unauthorized', 403: 'forbidden', 413: 'too_large' };
    fields.inert = false;
    if (answer.field) markInvalid(answer.field);
    setStatus(t(answer.code || byStatus[res.status] || 'other', { status: res.status }), true, lang);
  } catch (e) {
    if (view === 'settings' && form === mine) setStatus(failureMessage(e), true, lang);
  } finally {
    fields.inert = false;
  }
}

function chainRow(r, i, redraw) {
  const tr = el('tr', r.edit.on ? null : 'off');
  const move = (d) => {
    const to = i + d;
    [form.chain[i], form.chain[to]] = [form.chain[to], form.chain[i]];
    redraw();
    // Keep the keyboard on the row that moved. At the top or the bottom its arrow this way is
    // disabled and takes no focus, so the other one does.
    const same = document.querySelector(`[data-move="${to}:${d}"]`);
    (same && !same.disabled ? same : document.querySelector(`[data-move="${to}:${-d}"]`))?.focus();
  };
  const arrow = (d, glyph, label) => {
    const b = el('button', 'quiet small', glyph);
    b.type = 'button';
    b.title = label;
    b.setAttribute('aria-label', `${label}: ${r.name}`);
    b.dataset.move = `${i}:${d}`;
    b.disabled = i + d < 0 || i + d >= form.chain.length;
    b.addEventListener('click', () => move(d));
    return b;
  };
  const on = checkbox(r.edit.on, (v) => {
    r.edit.on = v;
    tr.classList.toggle('off', !v);
  });
  on.setAttribute('aria-label', `${t('col_on')}: ${r.name}`);
  const model = input('text', r.edit.model, r.effective_model ?? '', `chain.${r.name}.model`, (v) => { r.edit.model = v; });
  model.setAttribute('aria-label', `${t('col_model')}: ${r.name}`);
  // A curator with prices keeps its model; a model config.toml sets already can still be emptied.
  model.disabled = r.model_rule === 'fixed' && !r.model;
  const budget = input('number', r.edit.daily_budget, r.effective_daily_budget === null ? t('no_cap') : String(r.effective_daily_budget), `chain.${r.name}.daily_budget`, (v) => { r.edit.daily_budget = v; });
  [budget.min, budget.max] = form.ranges.daily_budget;
  budget.setAttribute('aria-label', `${t('col_budget')}: ${r.name}`);
  const timeout = input('number', r.edit.timeout_s, String(r.effective_timeout_s), `chain.${r.name}.timeout_s`, (v) => { r.edit.timeout_s = v; });
  [timeout.min, timeout.max] = form.ranges.timeout_s;
  timeout.setAttribute('aria-label', `${t('col_timeout')}: ${r.name}`);
  const modelNote = { fixed: t('model_fixed'), free: t('model_free') }[r.model_rule];
  tr.append(
    el('td', null, el('span', 'move', arrow(-1, '↑', t('up')), arrow(1, '↓', t('down')))),
    el('td', null, on),
    el('td', null, el('span', 'entry-name', r.name), ...keyState(r), r.entries > 1 ? note(t('entries', { n: r.entries })) : null,
      r.differs.length ? note(t('differs', { what: r.differs.map((d) => t(`differs_${d}`)).join(lang === 'ja' ? '、' : ', ') })) : null),
    el('td', null, model, modelNote ? note(modelNote) : null, r.model !== null && !r.model_applied ? note(t('model_unapplied')) : null),
    el('td', null, budget, r.budget_from_key && !r.edit.daily_budget ? note(t('from_key', { n: r.effective_daily_budget })) : null),
    el('td', null, timeout));
  return tr;
}

// The save's body, or the field a value is wrong in.
function saveBody() {
  const whole = (v, min, max) => (/^\d+$/.test(v.trim()) && Number(v) >= min && Number(v) <= max ? Number(v) : Number.NaN);
  const sizes = {};
  for (const key of SIZES) {
    sizes[key] = whole(form.inject[key], ...form.ranges[key]);
    if (Number.isNaN(sizes[key])) return { field: `inject.${key}` };
  }
  const summary = { curate: form.summary.curate, language: form.summary.language };
  for (const key of ['window_tokens', 'idle_minutes']) {
    summary[key] = whole(form.summary[key], ...form.ranges[key]);
    if (Number.isNaN(summary[key])) return { field: `summary.${key}` };
  }
  const paid = form.paid_usd_per_month.trim();
  const cap = Number(paid);
  const [least, most] = form.ranges.paid_usd_per_month;
  if (paid === '' || !Number.isFinite(cap) || cap < least || (most !== null && cap > most)) return { field: 'paid_usd_per_month' };
  if (!['none', 'before-subscriptions', 'after-subscriptions'].includes(form.gemini)) return { field: 'gemini' };
  const chain = [];
  for (const r of form.chain) {
    // Empty follows the curator's own value; a value config.toml has already stays as it is.
    const optional = (v, had, min, max) => {
      if (v.trim() === '') return null;
      return v.trim() === String(had) ? had : whole(v, min, max);
    };
    const daily = optional(r.edit.daily_budget, r.daily_budget, ...form.ranges.daily_budget);
    if (Number.isNaN(daily)) return { field: `chain.${r.name}.daily_budget` };
    const timeout = optional(r.edit.timeout_s, r.timeout_s, ...form.ranges.timeout_s);
    if (Number.isNaN(timeout)) return { field: `chain.${r.name}.timeout_s` };
    // A model config.toml has, left as it is, is sent as it is.
    const model = r.edit.model === (r.model ?? '') ? r.model : r.edit.model.trim() || null;
    chain.push({ name: r.name, on: r.edit.on, daily_budget: daily, timeout_s: timeout, model });
  }
  return {
    body: {
      version: form.version,
      worker: { resident: form.worker.resident },
      summary,
      paid_usd_per_month: cap,
      gemini: form.gemini === 'none' ? null : form.gemini,
      inject: {
        session_start: form.inject.session_start,
        per_prompt: form.inject.per_prompt,
        correction: form.inject.correction,
        ...sizes,
      },
      capture: { store_prompts: form.capture.store_prompts, tool_output: form.capture.tool_output },
      chain,
    },
  };
}

function markInvalid(field) {
  const i = [...document.querySelectorAll('#panel input, #panel select, #panel textarea')].find((x) => x.dataset.field === field);
  if (!i) return;
  i.classList.add('invalid');
  i.setAttribute('aria-invalid', 'true');
  i.focus();
}

async function saveSettings(button) {
  const { body, field } = saveBody();
  if (!body) {
    markInvalid(field);
    setStatus(t('range'), true, lang);
    return;
  }
  const mine = form;
  // The whole panel, the language picker too: what is typed, or drawn again in another language,
  // while the save is on its way would not survive its answer.
  const fields = button.closest('.settings');
  button.disabled = true;
  fields.inert = true;
  try {
    // 'same-origin': under the document's no-referrer policy a same-origin POST would carry
    // `Origin: null`, which the viewer refuses.
    const res = await fetch('/api/settings', {
      method: 'POST',
      headers: { 'X-Oboete-Token': token, 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
      referrerPolicy: 'same-origin',
      credentials: 'omit',
    });
    const answer = (res.headers.get('content-type') || '').startsWith('application/json') ? await res.json() : {};
    const current = res.status === 409 ? await api('settings') : null;
    // Moved to another tab, or the values were loaded again, while saving: what is shown stays.
    if (view !== 'settings' || form !== mine) return;
    if (res.ok || current) {
      form = formOf(current || answer);
      drawSettings();
      setStatus(t(current ? 'stale' : 'saved'), Boolean(current), lang);
      return;
    }
    const byStatus = { 400: 'bad_request', 401: 'unauthorized', 403: 'forbidden', 413: 'too_large' };
    const code = answer.code || byStatus[res.status] || 'other';
    // An inert form takes no focus, and the refused field is to be reached.
    fields.inert = false;
    if (answer.field) markInvalid(answer.field);
    setStatus(t(code, { status: res.status }), true, lang);
  } catch {
    if (view === 'settings' && form === mine) setStatus(t('network_failed'), true, lang);
  } finally {
    button.disabled = false;
    fields.inert = false;
  }
}

function drawSettings() {
  const pick = el('select', null, ...LANGS.map((l) => {
    const o = el('option', null, l === 'ja' ? '日本語' : 'English');
    o.value = l;
    return o;
  }));
  pick.value = lang;
  pick.addEventListener('change', () => {
    lang = pick.value;
    remember('oboete-lang', lang);
    applyLanguage();
    drawSettings();
    setStatus('');
  });
  const panel = el('div', 'settings', el('label', 'field lang', el('span', null, t('language')), pick));
  panel.lang = lang;
  if (!form) {
    panel.append(el('p', 'text pending', t('file_error')));
    drawIn(panel);
    return;
  }
  const f = form;
  const saved = (value) => note(t('saved_value', { value }));
  const resident = checkbox(f.worker.resident, (v) => { f.worker.resident = v; });
  resident.dataset.field = 'worker.resident';
  resident.disabled = !f.residentSupported;
  const summaryLanguage = input('textarea', f.summary.language, 'Japanese', 'summary.language', (v) => { f.summary.language = v; });
  summaryLanguage.className = 'summary-language';
  const summarySize = (key) => {
    const i = input('number', f.summary[key], '', `summary.${key}`, (v) => { f.summary[key] = v; });
    const [min, max] = f.ranges[key];
    [i.min, i.max] = [min, max];
    return el('label', 'field', el('span', null, t(key, { min: min.toLocaleString(lang), max: max.toLocaleString(lang) })),
      i, saved(f.saved.summary[key]), note(t(`${key}_desc`)));
  };
  const cap = input('number', f.paid_usd_per_month, '', 'paid_usd_per_month', (v) => { f.paid_usd_per_month = v; });
  cap.min = f.ranges.paid_usd_per_month[0];
  if (f.ranges.paid_usd_per_month[1] !== null) cap.max = f.ranges.paid_usd_per_month[1];
  cap.step = 'any';
  cap.inputMode = 'decimal';
  const usd = (v) => new Intl.NumberFormat(lang, { style: 'currency', currency: 'USD', maximumFractionDigits: 6 }).format(v);
  const spending = f.usd_this_month === null ? t('spend_unavailable')
    : f.usd_this_month === 0 ? t('no_spend') : t('month_spend', { usd: usd(f.usd_this_month) });
  const geminiOptions = { none: 'gemini_none', 'before-subscriptions': 'gemini_before', 'after-subscriptions': 'gemini_after' };
  const gemini = el('select', null, ...Object.entries(geminiOptions).map(([value, key]) => {
    const option = el('option', null, t(key));
    option.value = value;
    return option;
  }));
  gemini.value = f.gemini;
  gemini.dataset.field = 'gemini';
  gemini.addEventListener('change', () => {
    f.gemini = gemini.value;
    gemini.classList.remove('invalid');
    gemini.removeAttribute('aria-invalid');
  });
  const size = (key, label) => {
    const i = input('number', f.inject[key], '', `inject.${key}`, (v) => { f.inject[key] = v; });
    const [least, most] = f.ranges[key];
    [i.min, i.max] = [least, most];
    return el('label', 'field', el('span', null, t(label, { min: least.toLocaleString('en-US'), max: most.toLocaleString('en-US') })), i);
  };
  const flag = (key, label) => el('label', 'check', checkbox(f.inject[key], (v) => { f.inject[key] = v; }), t(label));
  const tool = el('select', null, ...['full', 'head-tail'].map((v) => {
    const o = el('option', null, t(v === 'full' ? 'tool_full' : 'tool_head_tail'));
    o.value = v;
    return o;
  }));
  tool.value = f.capture.tool_output;
  tool.addEventListener('change', () => { f.capture.tool_output = tool.value; });
  const rows = el('tbody');
  const redraw = () => rows.replaceChildren(...f.chain.map((r, i) => chainRow(r, i, redraw)));
  redraw();
  const head = el('tr', null, ...['col_order', 'col_on', 'col_name', 'col_model', 'col_budget', 'col_timeout'].map((k) => {
    const th = el('th', null, t(k));
    th.scope = 'col';
    return th;
  }));
  const save = el('button', 'save', t('save'));
  save.type = 'submit';
  const formEl = el('form', null,
    el('section', null,
      el('h3', null, t('resident_h')), el('p', 'desc', t('resident_desc')),
      el('label', 'check', resident, t('resident_on')),
      saved(t(f.saved.worker.resident ? 'value_on' : 'value_off')),
      f.firstRun && f.residentSupported ? el('p', 'desc', t('resident_first')) : null,
      el('p', 'desc', t(f.residentSupported ? 'resident_timing' : 'resident_unsupported'))),
    el('section', null,
      el('h3', null, t('summary_h')), el('p', 'desc', t('summary_desc')),
      el('label', 'check', checkbox(f.summary.curate, (v) => { f.summary.curate = v; }), t('curate_on')),
      saved(t(f.saved.summary.curate ? 'value_on' : 'value_off')),
      el('label', 'field', el('span', null, t('summary_language')), summaryLanguage,
        saved(f.saved.summary.language), note(t('summary_language_desc'))),
      el('details', null, el('summary', null, t('summary_advanced')),
        el('div', 'grid', summarySize('window_tokens'), summarySize('idle_minutes')))),
    el('section', null,
      el('h3', null, t('spending_h')),
      el('div', 'spending-cap',
        el('label', 'field', el('span', null, t('paid_cap')), cap, saved(usd(f.saved.paid_usd_per_month))),
        el('p', 'spend', spending)),
      el('p', 'desc', t('paid_cap_desc')),
      el('label', 'field', el('span', null, t('gemini_label')), gemini,
        saved(t(geminiOptions[f.saved.gemini ?? 'none'])), note(t('gemini_desc')))),
    el('section', null,
      el('h3', null, t('inject_h')),
      el('p', 'desc', t('inject_desc')),
      flag('session_start', 'inject_on'), size('session_start_chars', 'inject_chars'),
      flag('per_prompt', 'per_prompt_on'), size('per_prompt_chars', 'per_prompt_chars'),
      flag('correction', 'correction_on'), size('correction_chars', 'correction_chars')),
    el('section', null,
      el('h3', null, t('capture_h')),
      el('p', 'desc', t('capture_desc')),
      el('label', 'check', checkbox(f.capture.store_prompts, (v) => { f.capture.store_prompts = v; }), t('store_prompts')),
      el('label', 'field', el('span', null, t('tool_output')), tool)),
    el('section', null,
      el('h3', null, t('chain_h')),
      el('p', 'desc', t('chain_desc')),
      el('div', 'scroll', el('table', 'chain', el('thead', null, head), rows))),
    el('section', null,
      el('h3', null, t('stopped_h')), el('p', 'desc', t('stopped_desc')), stoppedState()),
    f.warnings.length
      ? el('section', 'warnings', el('h3', null, t('warnings_h')), el('ul', null, ...f.warnings.map((w) => el('li', null, w))))
      : null,
    save);
  formEl.noValidate = true;
  formEl.addEventListener('submit', (e) => {
    e.preventDefault();
    void saveSettings(save);
  });
  panel.append(el('p', 'lead', t('lead')), formEl);
  drawIn(panel);
}

// The settings view in its own language, its heading included (#274).
function drawIn(panel) {
  draw(t('heading'), [], [panel]);
  $('heading').lang = lang;
}

const LOADERS = new Map([
  ['records', showRecords], ['context', showContext], ['stats', showStats], ['settings', showSettings],
]);

// Only the latest request may draw: an earlier, slower one must not overwrite it.
let generation = 0;
let drawnWithoutBaseline = false;

async function show() {
  // First, so a settings render still out cannot draw over the tab opened after it.
  const mine = ++generation;
  feed?.observer?.disconnect();
  feed = null;
  // The settings tab needs no repository: it names the config.toml mistake that fails their list (#325).
  if (!reposLoaded && view !== 'settings') return refresh();
  const repo = $('repo').value;
  const q = $('q').value.trim();
  if (view === 'timeline') {
    const shown = await showFeed(repo, mine);
    if (shown && version === null) drawnWithoutBaseline = true;
    return shown;
  }
  setStatus(t('loading'));
  try {
    const search = q && view !== 'settings';
    const render = search ? await showSearch(repo, q) : await LOADERS.get(view)(repo);
    if (mine !== generation) return false;
    render();
    if (version === null) drawnWithoutBaseline = true;
    return true;
  } catch (e) {
    if (mine === generation) showError(e, show);
    return false;
  }
}

// All repositories leads the list; the current checkout is listed before its first stored record
// too, and stays selectable in Records. The resident viewer has none (docs/resident.md R8).
async function loadRepos() {
  const { current, repos } = await api('repos');
  const keep = reposLoaded ? $('repo').value : view === 'timeline' ? '' : current;
  currentRepo = current;
  const listed = current === '' || repos.some((r) => r.repo === current) ? repos
    : [...repos, { repo: current, claims: 0, imported: 0, records: 0 }];
  const options = listed.map((r) => {
    const o = new Option('', r.repo);
    for (const key of ['repo', 'claims', 'imported', 'records', 'last']) {
      if (r[key] !== undefined) o.dataset[key] = String(r[key]);
    }
    labelRepo(o);
    return o;
  });
  $('repo').replaceChildren(new Option(t('all_repositories'), ''), ...options);
  $('repo').value = keep === '' || options.some((o) => o.value === keep) ? keep : '';
  reposLoaded = true;
}

async function refresh(live = false) {
  let listed = true;
  try {
    await loadRepos();
  } catch (e) {
    if (view !== 'settings') {
      showError(e, refresh);
      return false;
    }
    listed = false;
  }
  // Preserve unsaved settings, including when a poll began before the Settings tab opened. The
  // note of a config.toml mistake stays while the list still fails, and goes once it loads.
  if (view === 'settings' && $('panel').querySelector('.settings') && (form || !listed)) return true;
  if (live && activeFeed(feed) && feed.repo === $('repo').value) return readFeed(feed, 'live');
  return show();
}

// --- Live: one request at a time, including visibility changes -------------------------------

let version = null;
let polling = false;
let pollFailureNotice = null;

async function poll() {
  if (polling || document.visibilityState !== 'visible') return;
  polling = true;
  try {
    const { v } = await api('version');
    $('live').classList.remove('off');
    $('live').textContent = t('live');
    // Either language's, as the page may have switched since (Codex on #373).
    if (pollFailureNotice?.isConnected || TEXT.unreachable.includes($('status').textContent)) setStatus('');
    const changed = version === null ? drawnWithoutBaseline : v !== version;
    // Provider calls and tool records do not move v, so Stats also follows each poll.
    const wanted = !reposLoaded || (view !== 'settings' && (changed || (view === 'stats' && !$('q').value.trim())));
    if (wanted && !(await refresh(true))) return;
    // A failed redraw leaves the marker unchanged, so the next poll retries it.
    version = v;
    drawnWithoutBaseline = false;
  } catch (e) {
    $('live').classList.add('off');
    $('live').textContent = t('live_off');
    if (e.status || view === 'timeline') pollFailureNotice = showError(e.status ? e : { key: 'unreachable' }, poll);
    else {
      setStatus(t('unreachable'), true);
      pollFailureNotice = $('status').firstChild;
    }
  } finally {
    polling = false;
  }
}

async function start() {
  applyLanguage();
  setView(view);
  $('help').addEventListener('click', openWelcome);
  $('welcome-close').addEventListener('click', closeWelcome);
  $('welcome').addEventListener('click', (event) => {
    if (event.target === $('welcome')) closeWelcome();
  });
  document.addEventListener('keydown', welcomeKey);
  document.addEventListener('focusin', (event) => {
    if (!$('welcome').hidden && !$('welcome-dialog').contains(event.target)) $('welcome-close').focus();
  });
  if (recall('oboete-welcome-dismissed', '') !== 'true') openWelcome();
  $('controls').addEventListener('submit', (e) => {
    e.preventDefault();
    void show();
  });
  $('repo').addEventListener('change', () => void show());
  $('refresh').addEventListener('click', () => void refresh());
  for (const id of ['since', 'until', 'history', 'raw']) {
    $(id).addEventListener('change', () => {
      if ($('q').value.trim()) void show();
    });
  }
  $('theme').addEventListener('click', () => {
    theme = THEMES[(THEMES.indexOf(theme) + 1) % THEMES.length];
    remember('oboete-theme', theme);
    applyTheme();
  });
  for (const b of document.querySelectorAll('#tabs .tab')) {
    b.addEventListener('click', () => {
      setView(b.dataset.view);
      $('q').value = '';
      void show();
    });
  }
  $('q').addEventListener('search', () => {
    if (!$('q').value) void show();
  });
  document.addEventListener('visibilitychange', () => void poll());
  setInterval(poll, 3000);
  await poll();
}

// Pasting a restarted viewer's address changes its fragment without reloading the page.
window.addEventListener('hashchange', () => location.reload());
await start();
