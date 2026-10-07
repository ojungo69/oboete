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

async function claimPanel(uid, existing = null) {
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
  const loaded = el('section', 'detail claim-view',
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
  const panel = existing || loaded;
  if (existing) panel.replaceChildren(...loaded.childNodes);
  panel.append(claimOwnerControls(c, panel));
  return panel;
}

function claimOwnerControls(c, panel) {
  const body = input('textarea', '', c.text, 'claim.body', () => {});
  const status = el('select', null, el('option', null, t('status_unchanged')));
  status.options[0].value = '';
  for (const value of ['decided', 'proposed', 'retracted', 'done']) {
    const option = el('option', null, metadata(value));
    option.value = value;
    status.append(option);
  }
  status.dataset.field = 'claim.status';
  const save = el('button', 'quiet small', t('claim_correct_save'));
  save.type = 'submit';
  const correction = el('form', null,
    el('label', 'field', el('span', null, t('claim_correct_text')), body),
    el('label', 'field', el('span', null, t('claim_correct_status')), status), save);
  correction.noValidate = true;
  const mute = el('button', 'quiet small', t(c.muted ? 'claim_unmute' : 'claim_mute'));
  mute.type = 'button';
  const result = el('p', 'desc');
  const fields = el('div', null, correction, mute);
  const reload = el('button', 'quiet small', t('claim_refresh'));
  reload.type = 'button';
  reload.addEventListener('click', async () => {
    reload.disabled = true;
    try {
      await claimPanel(c.uid, panel);
    } catch (e) { if (panel.isConnected) showError(e, () => reload.click()); }
    finally { reload.disabled = false; }
  });
  const submit = async (path, posted) => {
    fields.inert = true;
    let hold = false;
    try {
      const { res, answer } = await memoryWrite(path, posted);
      if (!panel.isConnected) return;
      if (!res.ok) { result.textContent = memoryFailure(res, answer); return; }
      hold = answer.state !== 'pending' || !['claim_pending', 'claim_not_applied'].includes(answer.code);
      result.textContent = claimReceipt(answer);
      if (answer.state === 'applied') {
        try {
          await claimPanel(c.uid, panel);
          if (panel.isConnected) {
            setStatus(t('claim_applied'), false, lang);
          }
        } catch { result.textContent += ` ${t('claim_refresh_hint')}`; }
      }
    } catch {
      hold = true; // A dropped answer cannot prove that append did not happen.
      if (panel.isConnected) result.textContent = t('memory_result_unknown');
    } finally { if (!hold) fields.inert = false; }
  };
  correction.addEventListener('submit', (event) => {
    event.preventDefault();
    if (fields.inert) return;
    const posted = { uid: c.uid };
    if (status.value) posted.status = status.value;
    if (body.value) posted.body = body.value;
    if (!posted.status && !posted.body) { result.textContent = t('claim_invalid'); return; }
    void submit('claims/correct', posted);
  });
  mute.addEventListener('click', () => { if (!fields.inert) void submit('claims/mute', { uid: c.uid, muted: !c.muted }); });
  return el('section', 'claim-owner', el('h4', null, t('claim_owner_h')), el('p', 'desc', t('claim_owner_desc')),
    el('p', 'desc', t(c.muted ? 'claim_muted_state' : 'claim_unmuted_state')), fields, result, reload);
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
  resident_timing: ['The next agent hook or running oboete view starts the resident processes. Turning this off applies when the worker is idle; the resident page stops once it has no requests.', '次のエージェントのフックか oboete view の実行で常駐プロセスが起動します。オフはワーカーの待機時に反映され、常駐の画面はアクセスがなくなった後に終了します。'],
  resident_unsupported: ['Resident mode is currently available on Linux and WSL. This system keeps the worker that exits when idle.', '常駐は現在 Linux と WSL に対応しています。この環境のワーカーは、処理がなくなった時に終了します。'],
  language: ['Language', '言語'],
  lead: [
    'Choose the settings below. Each save applies its own section; individual provider checkboxes and arrows save immediately. Recording and memory delivery read them at their next use; the background summarizer reads them before its next window, even while it stays running. This page cannot tell which values a running request has loaded. Opening this page or saving sends nothing to a provider.',
    'ここで設定を選べます。それぞれの保存ボタンで対象の設定を反映します。要約役の個別のチェックと矢印はすぐに保存します。記録・記憶の受け渡しには次の利用時から反映されます。要約の設定は、次のまとまりを処理する前に読み直すので、処理が動き続けていても反映されます。現在実行中の呼び出しが読み込んでいる値は、この画面では確認できません。画面を開いたり保存したりしても、要約役への送信は始まりません。',
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
  session_note_on: ['Show a status line in the terminal when a session starts', 'セッション開始時に端末へ状態を表示する'],
  session_note_desc: ['A short notice with no stored text. This changes at the next session start.', '保存した本文を含まない短い案内です。次のセッション開始から反映されます。'],
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
  backup_h: ['Backup location', 'バックアップの保存先'],
  backup_dir: ['Directory', 'ディレクトリ'],
  backup_default: ['Default: backups inside the memory home', '既定: 記憶の保存先にある backups'],
  backup_home: ['Memory home (an empty saved path)', '記憶の保存先(保存済みパスが空文字)'],
  backup_reset: ['Use the default backup location', '既定のバックアップ先を使う'],
  backup_desc: ['An empty field uses the default. Relative paths start at the memory home; absolute paths stay absolute. The next backup uses this location. Saving creates no directory and moves no existing backups or forget logs.', '空欄では既定の場所を使います。相対パスは記憶の保存先を基準とし、絶対パスはそのまま使います。次のバックアップから反映されます。保存してもディレクトリは作らず、既存のバックアップや忘却のログは移動しません。'],
  redaction_h: ['Masking sensitive values', '機密値のマスキング'],
  redaction_desc: ['Built-in rules always stay on. Saved changes apply at the next recording, send or display. The worker rescans this device’s stored records at its next run; a saved change does not start the worker. This does not erase backups or cover other devices.', '組み込みのルールは常に有効です。保存した変更は、次の記録・送信・表示から使われます。ワーカーは次回の処理で、この端末の保存済み記録を再検査します。保存だけではワーカーは起動しません。バックアップの削除や他の端末の検査は含みません。'],
  redaction_rule: ['Additional rule {number}', '追加ルール {number}'],
  redaction_add: ['Add a masking rule', 'マスキングのルールを追加'],
  redaction_remove: ['Remove this rule', 'このルールを削除'],
  redaction_id: ['Rule name', 'ルール名'],
  redaction_regex: ['Pattern (regular expression)', 'パターン(正規表現)'],
  redaction_advanced: ['Advanced rule fields', 'ルールの詳細項目'],
  redaction_keywords: ['Keywords (JSON array of strings)', 'キーワード(文字列の JSON 配列)'],
  redaction_entropy: ['Minimum entropy (optional)', '最小エントロピー(任意)'],
  redaction_group: ['Secret capture group (optional)', '機密値を取り出すグループ番号(任意)'],
  redaction_allow_h: ['Keep an exact false-positive value', '誤検知した値をそのまま残す'],
  redaction_allow_desc: ['Paste one exact value and add it. The browser keeps only its SHA-256 hash; the value is cleared and is never sent. Exceptions affect future masking. They do not restore text already masked or forgotten.', '値を正確に貼り付けて追加します。ブラウザーには SHA-256 のハッシュだけを残し、値は消去して送信しません。例外は今後のマスキングに使われます。すでに伏字にした本文や忘却した本文は復元しません。'],
  redaction_value: ['Exact value', '正確な値'],
  redaction_hash_add: ['Add its hash', 'ハッシュを追加'],
  redaction_hashes: ['Allowed SHA-256 hashes (one per line)', '例外の SHA-256 ハッシュ(1行に1つ)'],
  redaction_invalid: ['Check the additional rules and SHA-256 hashes. Nothing was saved.', '追加ルールと SHA-256 ハッシュを確認してください。保存はしていません。'],
  redaction_hash_failed: ['The browser could not hash this value. It was cleared and nothing was saved.', 'ブラウザーで値をハッシュ化できませんでした。値は消去し、保存はしていません。'],
  privacy_h: ['Repository sends', 'リポジトリの送信'],
  privacy_desc: ['Excluding a repository stops future curation and embedding sends for sessions that touch it. Recording and search continue. Existing memories and backups remain. Undo allows future sends to resume; these buttons send nothing to a provider.', 'リポジトリを除外すると、それに関わるセッションの今後の要約・埋め込みへの送信を止めます。記録と検索は続き、既存の記憶とバックアップは残ります。解除すると今後の送信を再開できる状態になります。このボタンでは要約役へ送信しません。'],
  privacy_none: ['No recorded repositories or send exclusions.', '記録済みのリポジトリや送信除外はありません。'],
  privacy_excluded: ['Send exclusion on', '送信除外中'],
  privacy_allowed: ['Eligible for future sends', '今後の送信対象'],
  privacy_exclude: ['Exclude future sends', '今後の送信から除外'],
  privacy_undo: ['Undo exclusion; allow future sends', '除外を解除し、今後の送信を許可'],
  privacy_recorded: ['The send exclusion was recorded.', '送信除外を記録しました。'],
  privacy_undone: ['The exclusion was undone. Future sends may resume.', '除外を解除しました。今後の送信を再開できる状態です。'],
  privacy_readback_failed: ['{receipt} The updated privacy state could not be read. Refresh privacy state to confirm it; do not repeat the write.', '{receipt} 更新後のプライバシー状態を読み込めませんでした。書き込みは繰り返さず、「プライバシーの状態を更新」で確認してください。'],
  privacy_unavailable: ['Privacy state could not be read or changed. Nothing is reported as complete.', 'プライバシーの状態を読み込み・変更できませんでした。完了したとは判定していません。'],
  privacy_selector: ['This repository selection is invalid. Reload its state.', 'リポジトリの選択が無効です。状態を再読み込みしてください。'],
  repo_not_found: ['This repository is no longer in the stored list. Reload its state.', 'このリポジトリは保存済み一覧にありません。状態を再読み込みしてください。'],
  repo_changed: ['The repository selection changed. Reload its state.', 'リポジトリの選択対象が変わりました。状態を再読み込みしてください。'],
  privacy_refresh: ['Refresh privacy state', 'プライバシーの状態を更新'],
  rescan_h: ['Rescan on this device', 'この端末の再検査'],
  rescan_empty: ['No raw records to scan.', '検査する元の記録はありません。'],
  rescan_pending: ['Waiting for the worker or still scanning.', 'ワーカーの処理待ち、または検査中です。'],
  rescan_complete: ['The current rules were checked through this device’s current raw sequence.', '現在のルールで、この端末の元の記録の末尾まで検査済みです。'],
  rescan_unavailable: ['Rescan state could not be read.', '再検査の状態を読み込めませんでした。'],
  rescan_progress: ['Checkpoint: {processed}; current raw sequence: {total}', '検査位置: {processed}、元の記録の末尾: {total}'],
  memory_action_failed: ['The operation was refused ({status}).', '操作を受け付けられませんでした({status})。'],
  memory_result_unknown: ['The connection ended before the result was confirmed. The operation may already be recorded. Check the stored state before repeating it.', '結果を確認する前に接続が終了しました。操作がすでに記録されている可能性があります。繰り返す前に保存済みの状態を確認してください。'],
  preference_h: ['A preference for every repository', 'すべてのリポジトリに共通する希望'],
  preference_desc: ['Save an owner preference for agents in every repository. It creates a new global claim; it does not rewrite existing claims. This operation makes no model request.', 'すべてのリポジトリでエージェントに伝える本人の希望を保存します。新しい共通の記憶を作り、既存の記憶は書き換えません。この操作ではモデルを呼び出しません。'],
  preference_text: ['Preference (up to 1,000 characters)', '希望すること(1,000文字まで)'],
  preference_confirm: ['Apply this preference to all repositories', 'この希望をすべてのリポジトリに適用する'],
  preference_save: ['Add a global preference', '共通の希望を追加'],
  preference_new: ['Write another preference', '別の希望を入力'],
  preference_confirmation: ['Confirm that this preference applies to all repositories.', 'すべてのリポジトリに適用することを確認してください。'],
  preference_empty: ['Write the preference first.', '希望することを入力してください。'],
  preference_too_long: ['Use 1,000 characters or fewer.', '1,000文字以内で入力してください。'],
  preference_partly_recorded: ['The owner instruction was recorded, but its global claim was not created. Do not repeat this action automatically. Check the stored memories before adding it again.', '本人の指示は記録されましたが、共通の記憶は作成されませんでした。操作を自動で繰り返していません。再度追加する前に保存済みの記憶を確認してください。'],
  claim_applied: ['Recorded and applied.', '記録し、適用しました。'],
  claim_pending: ['Recorded; application is still pending. Check this claim after the worker runs. Do not repeat the operation.', '記録済みで、適用を待っています。ワーカーの処理後にこの記憶を確認してください。操作を繰り返す必要はありません。'],
  claim_not_applied: ['Recorded, but the derived claim did not confirm application. Check this claim before making another change.', '操作は記録済みですが、派生した記憶への適用は確認できませんでした。次の変更をする前にこの記憶を確認してください。'],
  claim_uid: ['Select a complete claim identifier.', '記憶の完全な識別子を選んでください。'],
  claim_not_found: ['This claim is no longer available. Reload its details.', 'この記憶を利用できません。詳細を再読み込みしてください。'],
  claim_invalid: ['Check the correction and its status. Nothing was recorded.', '修正内容と状態を確認してください。操作は記録していません。'],
  claim_unavailable: ['The claim could not be changed. Check its stored state before trying again.', '記憶を変更できませんでした。再試行する前に保存済みの状態を確認してください。'],
  claim_owner_h: ['Owner changes', '本人による変更'],
  claim_owner_desc: ['Correct this claim, or mute it from memory handed to agents. Muted claims remain searchable. Unmute restores the existing eligibility rules. These actions make no model request.', 'この記憶を修正したり、エージェントに渡す記憶からミュートしたりできます。ミュート後も検索でき、解除すると既存の条件に従って再び渡せる状態になります。この操作ではモデルを呼び出しません。'],
  claim_correct_text: ['Replacement text (leave empty to keep it)', '修正後の本文(空欄では変更しない)'],
  claim_correct_status: ['Status', '状態'],
  claim_correct_save: ['Record this correction', 'この修正を記録'],
  claim_mute: ['Mute from agent memory', 'エージェントに渡す記憶からミュート'],
  claim_unmute: ['Unmute', 'ミュートを解除'],
  claim_muted_state: ['Muted from agent memory; still searchable.', 'エージェントに渡す記憶からミュート中です。検索はできます。'],
  claim_unmuted_state: ['Not muted. Normal memory eligibility applies.', 'ミュートしていません。通常の条件に従って記憶を渡します。'],
  claim_refresh: ['Reload this claim', 'この記憶を再読み込み'],
  claim_refresh_hint: ['Reload this claim to see its stored state.', '保存済みの状態は、この記憶の再読み込みで確認してください。'],
  chain_h: ['Controls for every entry with the same name', '同じ名前のすべての要約役への設定'],
  chain_desc: [
    'These controls apply to every entry with each name, including duplicates. This group order takes priority over the individual order above. An empty field follows the entry’s saved value, shown in grey.',
    'ここでの設定は、同じ名前のすべての要約役に反映されます。この名前ごとの順番が、上の個別の順番より優先されます。空欄は、各要約役の保存済みの値(灰色で表示)に従います。',
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
  key_unknown: ['Installation could not be checked', 'インストール状況は確認できません'],
  key_file: ['Key file: {path}', 'キーのファイル: {path}'],
  key_label: ['New key for {name}', '{name} の新しいキー'],
  key_placeholder: ['Paste a new key', '新しいキーを貼り付け'],
  key_save: ['Save key', 'キーを保存'],
  key_saved: ['The key was registered in private storage. It has not been tested yet.', 'キーを専用の保存先に登録しました。接続はまだ確認していません。'],
  key_not_durable: [
    'The key was registered, but the disk did not confirm the write. After an unexpected shutdown, register it again if it is missing.',
    'キーを登録しましたが、ディスクへの書き込みを確認できませんでした。予期しない終了の後、登録が失われていた場合は再登録してください。',
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
    'This entry has model prices. Edit its model and prices in the individual provider above.',
    'この要約役にはモデルの料金があります。上の個別設定で、モデルと料金を一緒に変更してください。',
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
  provider_entries_h: ['Individual providers', '要約役の個別設定'],
  provider_entries_desc: ['Add an API or a local OpenAI-compatible server, or use an installed Claude/Codex CLI. Each Save provider changes only that entry. Other settings use Save settings below. Checkboxes and arrows save immediately. A save sends no test request.', 'API・ローカルの OpenAI 互換サーバー、またはインストール済みの Claude/Codex CLI を登録できます。「要約役を保存」はその項目だけを変更します。ほかの設定は下の「設定を保存」で反映します。個別のチェックと矢印はすぐに保存します。保存では接続テストを行いません。'],
  provider_add: ['Add provider', '要約役を追加'],
  provider_create: ['Create provider', '要約役を登録'],
  provider_save: ['Save provider', '要約役を保存'],
  provider_cancel: ['Cancel', 'キャンセル'],
  provider_edit: ['Edit provider', '要約役を編集'],
  provider_remove: ['Remove provider', '要約役を削除'],
  provider_remove_confirm: ['Remove only “{name}” ({source} #{index})? Other entries with this name keep their settings. Removing the final Gemini entry also stops its automatic placement.', '「{name}」({source} #{index}) だけを削除しますか？ 同名のほかの項目の設定は維持されます。最後の Gemini を削除すると、自動追加も解除します。'],
  provider_name: ['Name', '名前'],
  provider_type: ['Connection type', '接続方法'],
  provider_http: ['API / local OpenAI-compatible server', 'API / ローカルの OpenAI 互換サーバー'],
  provider_cli_claude: ['Claude CLI (subscription)', 'Claude CLI (サブスクリプション)'],
  provider_cli_codex: ['Codex CLI (subscription)', 'Codex CLI (サブスクリプション)'],
  provider_endpoint: ['API base URL', 'API の接続先 URL'],
  provider_endpoint_desc: ['Use HTTPS, or HTTP with a numeric loopback address such as 127.0.0.1. The server must support the OpenAI-compatible chat API. Redirects are not followed by tests.', 'HTTPS、または 127.0.0.1 など数値のループバックアドレスの HTTP を指定します。サーバーは OpenAI 互換のチャット API に対応している必要があります。接続テストではリダイレクトを追跡しません。'],
  provider_enabled: ['Enable this entry', 'この要約役を有効にする'],
  provider_subscription: ['Covered by a subscription', 'サブスクリプション内の利用'],
  provider_subscription_desc: ['Subscription entries have no daily-call cap control. Existing legacy caps are preserved. Use API prices below for calls billed per token.', 'サブスクリプションには 1 日の回数を設定しません。既存の回数設定は維持します。トークン数で課金される API は下の料金を設定してください。'],
  provider_cli_desc: ['Uses the fixed installed adapter. Complete the provider’s own login first. oboete does not collect subscription login credentials. Installation and a successful connection test are separate states.', '対応するインストール済みアダプターを使います。先にサービス側のログインを完了してください。oboete はサブスクリプションのログイン情報を収集しません。インストール状態と接続確認は別々に表示します。'],
  provider_saved: ['Provider saved. It applies at the next eligible call; it has not been tested.', '要約役を保存しました。次の実行条件が整った呼び出しから反映されます。接続はまだ確認していません。'],
  provider_removed: ['The selected provider was removed.', '選んだ要約役を削除しました。'],
  provider_moved: ['Individual order saved. Name-group order may take priority.', '個別の順番を保存しました。名前ごとの順番が優先される場合があります。'],
  provider_source_file: ['Saved entry', '保存した項目'],
  provider_source_builtin: ['Built-in entry', '既定の項目'],
  provider_source_gemini: ['Automatic Gemini entry', '自動追加の Gemini'],
  provider_position: ['{source} #{index}', '{source} #{index}'],
  provider_order_desc: ['Arrows save the original individual order, including disabled entries. The effective order below includes name-group overrides and automatic Gemini placement. Editing a built-in entry saves the native list.', '矢印は、無効な項目も含めた個別の元の順番を保存します。実際の順番には、名前ごとの設定と Gemini の自動追加が反映されます。既定の項目を編集すると、個別の一覧が保存されます。'],
  provider_effective: ['Effective: {on}; order {order}; model {model}; timeout {timeout} s', '実際の設定: {on}、順番 {order}、モデル {model}、待ち時間 {timeout} 秒'],
  provider_saved_native: ['Saved: {on}; model {model}; timeout {timeout} s', '個別の保存値: {on}、モデル {model}、待ち時間 {timeout} 秒'],
  provider_default_model: ['Adapter default', 'アダプターの既定'],
  provider_advanced: ['Limits and API prices', '使用量の上限と API の料金'],
  provider_limits_desc: ['Blank token limits mean no native cap. API prices are USD per million tokens; enter your model’s prices for paid-call accounting. Zero prices count as free. The output cap is available only for API entries with a nonzero token price (new-entry default: 4,000). Normal free-API and CLI output bounds cannot be controlled here; their saved estimates are preserved.', '空欄のトークン数には個別の上限を設けません。API の料金は 100 万トークンあたりの米ドルです。有料利用の集計にはモデルの料金を入力してください。料金が 0 なら無料として数えます。出力の上限は、トークンの料金が 0 より大きい API でのみ設定できます(新しい項目の既定: 4,000)。無料 API・CLI の通常の出力上限はここでは制御できず、保存済みの見積もり値を維持します。'],
  provider_max_request_tokens: ['Input tokens per request (optional)', '1 回に送るトークン数の上限(任意)'],
  provider_daily_tokens: ['Total tokens a day (optional)', '1 日の合計トークン数の上限(任意)'],
  provider_usd_per_mtok_in: ['Input price (USD / million tokens)', '入力料金(米ドル / 100 万トークン)'],
  provider_usd_per_mtok_out: ['Output price (USD / million tokens)', '出力料金(米ドル / 100 万トークン)'],
  provider_max_output_tokens: ['Maximum output tokens', '出力トークン数の上限'],
  provider_key_manage: ['Register a key on the selected individual provider above.', 'キーは上の個別の要約役で登録してください。'],
  provider_key_none: ['No key registered; a local server may not require one.', 'キーは未登録です。ローカルサーバーでは不要な場合があります。'],
  provider_key_desc: ['Paste a provider-issued API key. oboete chooses private storage; the saved value is never shown. Registration sends no inference request.', 'サービスで発行した API キーを貼り付けます。専用の保存先は oboete が選び、保存した値は表示しません。登録では AI への送信を行いません。'],
  provider_key_unsupported: ['Key registration is currently available on Linux and WSL. This platform’s managed storage is not available yet.', 'キーの登録は現在 Linux と WSL に対応しています。この環境の専用の保存先はまだ利用できません。'],
  provider_storage_failed: ['A safe private key location could not be used. The previous key and configuration were preserved.', '安全な専用の保存先を利用できませんでした。以前のキーと設定は維持されています。'],
  provider_unsupported_cli: ['This legacy CLI adapter is not supported here. You can disable or remove it; add Claude or Codex to use a supported adapter.', 'この既存の CLI アダプターには対応していません。無効化・削除できます。対応するアダプターを使うには Claude または Codex を追加してください。'],
  provider_unsupported_endpoint: ['This saved destination is not supported by this UI and is not displayed. Set a supported API URL to repair the entry; tests remain unavailable until then.', '保存された接続先はこの画面の対応範囲外のため表示しません。対応する API の URL を設定して修正してください。それまでは接続テストを利用できません。'],
  provider_preview: ['Preview connection test', '接続テストの内容を確認'],
  provider_preview_desc: ['The test uses the saved entry, not pending edits. Preview sends nothing. Review the destination, fixed synthetic data and possible charge, then choose Run test separately.', '保存済みの要約役をテストします。編集中の値は使いません。内容の確認だけでは送信しません。接続先・固定のテスト用データ・費用の可能性を確認し、別途「テストを実行」を押してください。'],
  provider_test_destination: ['Destination: {destination}', '接続先: {destination}'],
  provider_test_model: ['Model: {model}', 'モデル: {model}'],
  provider_test_fixture: ['Fixed test data', '固定のテスト用データ'],
  provider_test_tokens: ['Estimated input: {tokens} tokens; output limit: {output}', '推定入力: {tokens} トークン、出力上限: {output}'],
  provider_test_charge: ['This test may incur a charge. Estimated cost: {usd}; saved monthly cap: {cap}.', 'このテストには費用がかかる可能性があります。推定費用: {usd}、保存済みの月額上限: {cap}。'],
  provider_test_no_charge: ['No per-token charge is expected. Subscription or local-server limits still apply. Saved monthly cap: {cap}.', 'トークンごとの料金は想定されていません。サブスクリプションやローカルサーバーの上限は適用されます。保存済みの月額上限: {cap}。'],
  provider_test_unknown_cost: ['Not available', '算出できません'],
  provider_test_run: ['Run test', 'テストを実行'],
  provider_test_ok: ['Connection test succeeded ({ms} ms).', '接続テストに成功しました({ms} ミリ秒)。'],
  provider_test_failed: ['Connection test failed ({ms} ms): {reason}', '接続テストに失敗しました({ms} ミリ秒): {reason}'],
  provider_test_blocked: ['Connection test is blocked: {reason}', '接続テストを実行できません: {reason}'],
  provider_test_billed: ['Recorded test cost: {usd}', '記録されたテストの費用: {usd}'],
  provider_test_retry: ['Retry time: {time}. Preview again when ready; resuming a stopped provider remains a separate action.', '再試行可能な時刻: {time}。準備が整ったら内容を再確認してください。停止した要約役の再開は別の操作です。'],
  provider_test_reason: ['The provider could not complete this test.', 'この要約役ではテストを完了できませんでした。'],
  provider_test_budget: ['The saved spending or usage limit does not allow this test.', '保存済みの費用・使用量の上限により、このテストを実行できません。'],
  provider_test_cooldown: ['This provider is waiting after a limit or failure.', '上限や失敗により、この要約役は待機しています。'],
  provider_test_hold: ['This provider is stopped. Use again is a separate action below.', 'この要約役は停止しています。下の「再び使う」で別途再開してください。'],
  provider_test_off: ['This entry or its name group is off.', 'この要約役、または同じ名前の設定が無効です。'],
  provider_test_missing_key: ['Register an API key for this entry first.', '先にこの要約役の API キーを登録してください。'],
  provider_test_unavailable: ['Connection testing is not available for this entry.', 'この要約役では接続テストを利用できません。'],
  provider_test_cli_unbounded: ['Connection testing is unavailable for CLI providers because an output limit cannot be guaranteed. Normal curation still uses your saved settings.', 'CLI の接続テストは、出力の上限を保証できないため利用できません。保存済みの設定は、通常の要約に引き続き適用されます。'],
  provider_test_busy: ['Another operation is using the spending history or sending recorded data. Wait for it to finish, then try again.', '別の処理が使用履歴を更新中、または記録データを送信中です。完了を待ってから再試行してください。'],
  provider_test_ledger_invalid: ['The spending history could not be read. Run oboete doctor to check it before retrying.', '使用履歴を読み取れません。再試行する前に oboete doctor で状態を確認してください。'],
  provider_test_gate: ['The data-sending safety gate refused this test.', 'データ送信の安全性の確認により、テストを拒否しました。'],
  provider_test_auth: ['The provider refused authentication. Check the registered key or the provider’s own login.', '接続先が認証を拒否しました。登録したキー、またはサービス側のログインを確認してください。'],
  provider_test_http: ['The provider returned an HTTP error.', '接続先が HTTP のエラーを返しました。'],
  provider_test_http_status: ['HTTP status: {status}', 'HTTP ステータス: {status}'],
  provider_test_size: ['The fixed test data exceeds this entry’s saved request limit.', '固定のテスト用データが、この要約役の保存済みの入力上限を超えています。'],
  provider_test_invalid: ['The provider’s answer did not match the expected test response.', '接続先の応答が、テストで必要な形式と一致しませんでした。'],
  provider_test_isolation: ['The required isolated test environment could not be prepared.', 'テストに必要な分離された実行環境を準備できませんでした。'],
  provider_test_allowance: ['The available allowance for this entry could not be confirmed.', 'この要約役の利用可能な残量を確認できませんでした。'],
  provider_test_confirmation: ['Review a ready preview and explicitly choose Run test.', '実行できるテスト内容を確認してから、明示的に「テストを実行」を選んでください。'],
  provider_bad_model: ['Enter a model name of at most 200 characters without control characters. A CLI model cannot start with a dash.', '制御文字を含まない 200 文字以内のモデル名を入力してください。CLI のモデル名はハイフンで始めることはできません。'],
  provider_key_not_applicable: ['API-key registration is for API entries. Complete a subscription provider’s own login instead.', 'API キーは API の要約役に登録します。サブスクリプションでは、サービス側のログインを完了してください。'],
  provider_test_timeout: ['The provider did not answer within the saved timeout.', '保存済みの待ち時間内に応答がありませんでした。'],
  provider_bad_name: ['Enter a name of 1 to 64 characters without control characters.', '制御文字を含まない 1〜64 文字の名前を入力してください。'],
  provider_bad_endpoint: ['Enter an HTTPS API base URL, or numeric loopback HTTP URL, without credentials or a fragment.', '認証情報や # 以降の部分を含まない HTTPS の API URL、または数値のループバックアドレスの HTTP URL を入力してください。'],
  provider_kind_error: ['An existing entry’s connection type cannot be changed. Add a new entry instead.', '登録済みの接続方法は変更できません。新しい要約役を追加してください。'],
  provider_operation_refused: ['The provider operation was refused. The previous configuration was preserved.', '要約役の操作を拒否しました。以前の設定は維持されています。'],
  provider_operation_failed: ['The provider operation could not be completed ({status}).', '要約役の操作を完了できませんでした({status})。'],
  settings_save: ['Save settings', '設定を保存'],
  save: ['Save', '保存'],
  saved: ['Saved. Each setting takes effect at the time described beside it.', '保存しました。反映されるタイミングは各設定の説明をご確認ください。'],
  warnings_h: ['Notes on config.toml', 'config.toml についての注意'],
  agent_inventory_h: ['Agent registrations', 'エージェントの登録状況'],
  agent_inventory_desc: ['This checks saved files only. A launch file does not prove login, executable permissions or live use. Full doctor checks and connecting agents are not available on this page yet.', '保存されたファイルだけを確認します。起動用ファイルが見つかっても、ログイン、実行権限、実際の利用は確認できません。詳しい診断とエージェントの接続操作は、この画面ではまだ使えません。'],
  agent_inventory_refresh: ['Refresh file inventory', '設定ファイルを再確認'],
  agent_inventory_loading: ['Checking agent files…', 'エージェントのファイルを確認中…'],
  agent_inventory_not_checked: ['Agent files have not been checked yet. Press Refresh.', 'エージェントのファイルはまだ確認していません。「再確認」を押してください。'],
  agent_inventory_unavailable: ['Agent files could not be checked. Try Refresh.', 'エージェントのファイルを確認できませんでした。「再確認」をお試しください。'],
  agent_inventory_unknown: ['Unknown', '不明'],
  agent_inventory_home: ['Memory folder: {state}', '記憶の保存先：{state}'],
  agent_inventory_config: ['Saved configuration: {state}', '保存済み設定：{state}'],
  agent_inventory_agent_claude: ['Claude Code', 'Claude Code'],
  agent_inventory_agent_codex: ['Codex', 'Codex'],
  agent_inventory_agent_grok: ['Grok', 'Grok'],
  agent_inventory_agent_agy: ['Antigravity', 'Antigravity'],
  agent_inventory_agent_opencode: ['OpenCode', 'OpenCode'],
  agent_inventory_agent_pi: ['Pi', 'Pi'],
  agent_inventory_agent_cursor: ['Cursor', 'Cursor'],
  agent_inventory_launch: ['Launch file: {state}', '起動用ファイル：{state}'],
  agent_inventory_launch_found: ['found; login and live use untested', '見つかりました。ログインと実際の利用は未確認です'],
  agent_inventory_launch_missing: ['not found', '見つかりません'],
  agent_inventory_directory: ['Settings folder: {state}', '設定フォルダー：{state}'],
  agent_inventory_directory_found: ['found', '見つかりました'],
  agent_inventory_directory_missing: ['not found', '見つかりません'],
  agent_inventory_capture: ['{kind}: {state}; {match}', '{kind}：{state}。{match}'],
  agent_inventory_mcp: ['MCP: {state}; {match}', 'MCP：{state}。{match}'],
  agent_inventory_trust: ['Hook trust: {state}', 'フックの信頼状態：{state}'],
  agent_inventory_live: ['Live use: {state}', '実際の利用：{state}'],
  agent_inventory_live_unverified: ['not checked', '未確認'],
  agent_inventory_kind_hooks: ['Hooks', 'フック'],
  agent_inventory_kind_plugin: ['Plugin', 'プラグイン'],
  agent_inventory_kind_extension: ['Extension', '拡張機能'],
  agent_inventory_state_missing: ['missing', '見つかりません'],
  agent_inventory_state_present: ['folder present', 'フォルダーあり'],
  agent_inventory_state_valid: ['file parses', 'ファイルの形式は有効'],
  agent_inventory_state_registered: ['registered in file', 'ファイルに登録済み'],
  agent_inventory_state_partial: ['partly registered', '一部のみ登録'],
  agent_inventory_state_stale: ['out of date', '現在の内容と不一致'],
  agent_inventory_state_disabled: ['disabled', '無効'],
  agent_inventory_state_invalid: ['invalid file', 'ファイルが不正'],
  agent_inventory_state_unreadable: ['unreadable', '読み取り不可'],
  agent_inventory_state_unavailable: ['cannot assess', '確認不可'],
  agent_inventory_state_not_applicable: ['not applicable', '対象外'],
  agent_inventory_state_matching: ['matching', '一致'],
  agent_inventory_match_true: ['matches this installation', '現在の導入内容と一致'],
  agent_inventory_match_false: ['does not match this installation', '現在の導入内容と不一致'],
  agent_inventory_match_unknown: ['current-installation comparison unavailable', '現在の導入内容との比較はできません'],
  agent_inventory_match_not_applicable: ['no comparison applies', '比較の対象外'],
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
  maintenance_h: ["History and recovery", "履歴の取り込みと復元"],
  maintenance_desc: ["Preview the saved data and effects, then confirm this operation. Imports, rebuild, restore and v1 finalization make no model request. Recuration preparation makes no model request; its confirmed run may send history to your saved providers. Later background processing follows saved settings.", "保存された記録と変更内容を確認してから実行します。取り込み・再構築・復元・旧版の移行完了はモデルを呼びません。要約のやり直しの準備もモデルを呼びませんが、確認後の実行では保存済みの要約役へ履歴を送る場合があります。その後のバックグラウンド処理は保存済み設定に従います。"],
  maintenance_finish: ["Finish v1 migration and remove old files", "旧版の移行を完了して旧ファイルを削除"],
  maintenance_finish_hint: ["One final import runs before deleting only the listed v1 files and folders. Preview reads the current v1 source and selected files without changing them. This action makes no model request.", "最後の取り込みを1回行ってから、一覧の旧版ファイルとフォルダーだけを削除します。プレビューは現在の旧版データと対象ファイルを読み取るだけです。この操作ではモデルを呼びません。"],
  maintenance_finish_preview: ["Preview final import and exact deletion scope", "最後の取り込みと正確な削除対象を確認"],
  maintenance_finish_preview_h: ["Final import and selected v1 files", "最後の取り込みと旧版の削除対象"],
  maintenance_finish_candidates: ["Final import candidates: {events} events, {records} records, {repositories} repositories, {documents} documents, {bytes} source bytes.", "最後の取り込み候補：イベント{events}件、レコード{records}件、リポジトリ{repositories}件、文書{documents}件、元データ{bytes}バイト。"],
  maintenance_finish_order: ["The final import runs before deletion. No model request is made by this operation.", "最後の取り込みは削除より前に実行します。この操作ではモデルへの要求を行いません。"],
  maintenance_finish_deletion: ["Exact deletion scope: {targets} targets, {nodes} nodes, {bytes} logical bytes. Removed bytes do not guarantee freed disk space.", "正確な削除範囲：対象{targets}件、内部を含む項目{nodes}個、論理サイズ{bytes}バイト。削除バイト数は空き容量の増加を保証しません。"],
  maintenance_finish_target_hidden: ["Name hidden by display rules", "表示規則により名前を非表示"],
  maintenance_finish_consent: ["Run the final import, then permanently remove exactly the listed v1 targets. I understand partial import or deletion may remain after a failure and there is no automatic rollback.", "最後の取り込みを実行し、一覧の旧版対象だけを完全に削除することに同意します。失敗時には取り込みや削除が一部残り、自動で元に戻らないことを確認しました。"],
  maintenance_finish_start: ["Finish migration and remove listed files", "移行を完了して一覧のファイルを削除"],
  maintenance_finish_refresh: ["Inspect finalization status", "移行完了の状況を確認"],
  maintenance_finish_another: ["Inspect remaining files before a new preview", "残るファイルを確認してから再プレビュー"],
  maintenance_finish_unknown_hint: ["The final import or deletion may already have happened. Inspect the receipt, current memory, backups and remaining v1 files. Do not assume the source still exists. Nothing is retried automatically.", "最後の取り込みや削除が既に行われた可能性があります。結果、現在の記憶、バックアップ、残る旧版ファイルを確認してください。元データが残っているとは限りません。自動では再試行しません。"],
  maintenance_finish_settings_hint: ["This operation uses saved settings and makes no model request. Later background processing follows saved settings; pending edits on this page are not submitted.", "この操作は保存済み設定を使い、モデルを呼びません。その後のバックグラウンド処理は保存済み設定に従います。この画面の編集中の設定は送信しません。"],
  maintenance_finish_phase_running: ["Final import and deletion in progress", "最後の取り込みと削除を処理中"],
  maintenance_finish_phase_complete: ["v1 finalization completed", "旧版の移行完了処理が完了"],
  maintenance_finish_phase_partial: ["v1 finalization stopped; inspect completed and possible changes", "旧版の移行完了処理が停止：変更内容を確認"],
  maintenance_finish_phase_failed: ["v1 finalization did not complete", "旧版の移行完了処理は完了せず"],
  maintenance_finish_phase_unknown: ["v1 finalization needs inspection", "旧版の移行完了結果を確認してください"],
  maintenance_finish_progress: ["Final import so far: {records} records, {repositories} repository touches, {documents} documents committed.", "最後の取り込みの途中経過：レコード{records}件、リポジトリ履歴{repositories}件、文書{documents}件が確定。"],
  maintenance_finish_import: ["Final import: {events} events read; {records} records, {repositories} repository touches, {documents} documents committed; {seen} already seen. Missing from older source {deleted_sessions}; identifiers requiring inspection {uncertain_identifiers}.", "最後の取り込み：イベント{events}件を読み取り、レコード{records}件・リポジトリ履歴{repositories}件・文書{documents}件が確定。既存{seen}件、旧データから消えたセッション{deleted_sessions}件、確認が必要な識別情報{uncertain_identifiers}件。"],
  maintenance_finish_missing_sessions: ["The older source no longer contains {total} imported sessions. Showing {shown} names; review their retained memory and use Forget separately if needed.", "旧データから消えた取り込み済みセッションは{total}件です。{shown}件の名前を表示します。保持された記憶を確認し、必要なら別途忘却してください。"],
  maintenance_finish_uncertain_sessions: ["Cannot reliably match {total} older session identities. Showing {shown} names; inspect their retained memory before forgetting.", "旧セッションの識別情報を確実に対応づけられない対象は{total}件です。{shown}件の名前を表示します。忘却する前に保持された記憶を確認してください。"],
  maintenance_finish_effects: ["Local stores changed: {changed}; deletion requests applied: {applied}; deletion-history warnings: {warnings}.", "ローカルの保存情報の変更：{changed}。反映した削除指定：{applied}件、削除履歴の注意：{warnings}件。"],
  maintenance_finish_changed_yes: ["yes", "あり"],
  maintenance_finish_changed_no: ["no", "なし"],
  maintenance_finish_deletion_progress: ["Deletion progress: {attempted} attempted, {removed} removed, {failed} failed, {uncertain} uncertain.", "削除の途中経過：試行{attempted}件、削除{removed}件、失敗{failed}件、結果が不確実{uncertain}件。"],
  maintenance_finish_deletion_result: ["Deletion: {selected} selected, {attempted} attempted, {removed} removed, {failed} failed, {uncertain} uncertain; {authorized_bytes} logical bytes authorized, {removed_bytes} logical bytes reported removed.", "削除：対象{selected}件、試行{attempted}件、削除{removed}件、失敗{failed}件、結果が不確実{uncertain}件。許可した論理サイズ{authorized_bytes}バイト、削除を確認した論理サイズ{removed_bytes}バイト。"],
  maintenance_finish_target_result: ["{label}: {state} ({bytes} logical bytes)", "{label}：{state}（論理サイズ{bytes}バイト）"],
  maintenance_finish_target_removed: ["removed", "削除済み"],
  maintenance_finish_target_failed: ["failed", "失敗"],
  maintenance_finish_target_changed: ["changed before deletion", "削除前に変更"],
  maintenance_finish_target_not_attempted: ["not attempted", "未試行"],
  maintenance_finish_target_unknown_extent: ["subtree effect uncertain", "内部の削除範囲が不確実"],
  maintenance_finish_committed_boundary: ["Some local changes completed. Inspect the receipt and remaining files before continuing.", "一部のローカル変更は完了しています。続行する前に結果と残るファイルを確認してください。"],
  maintenance_finish_inspect: ["Earlier imports or removals are not rolled back. Some targets may remain or a subtree may have changed. The v1 source may already be gone; inspect current memory, backups and remaining files before considering any new operation.", "先に行われた取り込みや削除は自動で元に戻りません。対象が残る場合や、フォルダー内の変更範囲が不確実な場合があります。旧版の元データが既にない可能性もあるため、新たな操作を検討する前に現在の記憶・バックアップ・残るファイルを確認してください。"],
  maintenance_finish_kept: ["Current memory and configured backups are kept. New imports follow your saved background processing settings.", "現在の記憶と設定済みバックアップは保持します。新しい取り込みのバックグラウンド処理は保存済み設定に従います。"],
  maintenance_recurate: ["Recurate selected history", "対象の履歴を再整理"],
  maintenance_recurate_hint: ["Preparation can update local indexing, recover or rescan stored data. It makes no model request. A confirmed run may send selected text to saved providers and incur charges.", "準備ではローカルの索引更新、保存データの復旧・再走査が起こり得ます。準備ではモデルを呼びません。確認後の実行では選んだ本文を保存済みの要約役へ送り、料金が発生する場合があります。"],
  maintenance_recurate_scope: ["History to recurate", "再整理する履歴"],
  maintenance_recurate_queued: ["Queued history on this device", "この端末の待機中の履歴"],
  maintenance_recurate_skipped: ["Previously skipped history on this device", "この端末で以前に読み飛ばした履歴"],
  maintenance_recurate_imported_v1: ["Imported oboete v1 history", "取り込んだ旧 oboete の履歴"],
  maintenance_recurate_imported_transcripts: ["Imported agent transcripts", "取り込んだエージェントの会話履歴"],
  maintenance_recurate_records: ["This device's record range", "この端末のレコード範囲"],
  maintenance_recurate_from: ["First record number", "最初のレコード番号"],
  maintenance_recurate_to: ["Last record number", "最後のレコード番号"],
  maintenance_recurate_range_hint: ["Enter two whole record numbers; the last must be at least the first.", "整数のレコード番号を二つ入力してください。最後は最初以上にします。"],
  maintenance_recurate_preview: ["Prepare selected history without a model request", "モデルを呼ばずに対象履歴を準備"],
  maintenance_recurate_preview_h: ["Prepared scope and paid-chain estimate", "準備した範囲と有料経路の見積もり"],
  maintenance_recurate_selected_scope: ["Prepared scope: {scope}.", "準備した対象：{scope}。"],
  maintenance_recurate_range: ["this device's records {from}–{to}", "この端末のレコード{from}～{to}"],
  maintenance_recurate_plan: ["{spans} spans; {windows} candidate windows; {tokens} estimated input tokens in total; {kept} kept back; {unparked} newly imported records still awaiting the curation checkpoint, outside this run.", "対象{spans}区間、候補{windows}窓、推定入力トークン合計{tokens}、送信対象から保持{kept}窓、今回の対象外で整理処理の読み取り位置より先にある取り込み記録{unparked}件。"],
  maintenance_recurate_cost: ["Worst paid-chain estimate: USD {usd} if every paid entry bills every selected window. This is an estimate, not a spending cap.", "有料経路の最大見積もり：全ての有料要約役が各対象窓を請求した場合 USD {usd}。支出上限ではなく見積もりです。"],
  maintenance_recurate_no_paid: ["No paid entry is in the saved provider chain. A confirmed run may still send to free or subscription providers.", "保存済みの要約役の経路に有料の項目はありません。確認後の実行では無料またはサブスクリプションの要約役へ送る場合があります。"],
  maintenance_recurate_long: ["Long imported v1 sessions, longest first ({total} total)", "長い旧版取り込みセッション、長い順（計{total}件）"],
  maintenance_recurate_long_item: ["{label}: {characters} characters", "{label}：{characters}文字"],
  maintenance_recurate_consent: ["Send this freshly prepared scope through my saved provider chain. I understand the shown estimate and that charges may occur.", "今準備した対象を保存済みの要約役の経路へ送ることに同意します。表示された見積もりと料金が発生する可能性を確認しました。"],
  maintenance_recurate_start: ["Start confirmed recuration", "確認した履歴の再整理を開始"],
  maintenance_recurate_refresh: ["Inspect recuration status", "再整理の状況を確認"],
  maintenance_recurate_another: ["Prepare another scope after inspection", "確認後に別の対象を準備"],
  maintenance_recurate_unknown_hint: ["The request or local preparation may already have changed saved data or reached a provider. Inspect the receipt and current data. Nothing is retried automatically.", "要求またはローカルの準備で保存データが変わり、要約役へ送信済みの可能性もあります。結果と現在の記録を確認してください。自動では再試行しません。"],
  maintenance_recurate_settings_hint: ["Uses saved provider, privacy and spending settings; pending edits on this page are not sent.", "保存済みの要約役・プライバシー・支出設定を使います。この画面の編集中の設定は送信しません。"],
  maintenance_recurate_phase_prepared: ["Local preparation completed", "ローカルの準備が完了"],
  maintenance_recurate_phase_running: ["Recuration is running", "履歴を再整理中"],
  maintenance_recurate_phase_complete: ["Recuration completed", "履歴の再整理が完了"],
  maintenance_recurate_phase_partial: ["Recuration stopped with retained effects", "変更内容を保持したまま再整理が停止"],
  maintenance_recurate_phase_failed: ["Recuration did not complete", "履歴の再整理は完了せず"],
  maintenance_recurate_phase_unknown: ["Recuration result needs inspection", "履歴の再整理結果を確認してください"],
  maintenance_recurate_stage: ["Stage: {stage}.", "段階：{stage}。"],
  maintenance_stage_preparing: ["Preparing local data", "ローカルのデータを準備"],
  maintenance_stage_prepared: ["Ready for explicit consent", "明示的な同意を待機"],
  maintenance_stage_curating: ["Processing selected windows", "選んだ窓を処理"],
  maintenance_recurate_providers: ["Provider attempts: reserved {reserved}, possibly sent {sent}, settled {settled}, cancelled {cancelled}, pending {pending}, uncertain {uncertain}; usage unknown {usage_unknown}.", "要約役への試行：予約{reserved}回、送信済みの可能性{sent}回、精算{settled}回、取消{cancelled}回、保留{pending}回、不確実{uncertain}回。使用量不明{usage_unknown}回。"],
  maintenance_recurate_accounted: ["Accounted at saved prices: {usd}, including pending bounds. This is not a provider invoice.", "保存済み単価による計上額：{usd}（保留中の上限額を含む）。要約役からの請求額ではありません。"],
  maintenance_recurate_windows: ["Windows: committed {committed}, failed {failed}, stopped {stopped}, kept back {kept_back}; claims {claims}, retractions {retracted}.", "窓の処理：確定{committed}、失敗{failed}、停止{stopped}、送信対象から保持{kept_back}。記憶{claims}、撤回{retracted}。"],
  maintenance_recurate_local_receipt: ["Local preparation can leave saved changes even without a consent key. Inspect this receipt before preparing again.", "同意用のキーがなくてもローカルの準備で保存内容が変わる場合があります。再準備する前にこの結果を確認してください。"],
  maintenance_recurate_preview_failed: ["Preparation returned no usable consent key. Inspect the local receipt before trying again; no model request was started by this preview.", "準備から有効な同意用キーが返りませんでした。再試行する前にローカルの結果を確認してください。この準備ではモデルへの要求を開始していません。"],
  maintenance_recurate_prepare_unknown: ["Preparation response was lost. Local work may have completed. Inspect status and saved data before preparing again; no automatic retry occurs.", "準備の応答が失われました。ローカルの処理が完了済みの可能性があります。再準備する前に状況と保存データを確認してください。自動では再試行しません。"],
  maintenance_recurate_committed_boundary: ["Earlier local or provider effects are retained. Inspect pending and uncertain attempts before another operation.", "先に確定したローカル処理や要約役への試行は保持されています。次の処理の前に保留・不確実な試行を確認してください。"],
  maintenance_scope: ["The selected record range is invalid. Enter two valid record numbers.", "指定したレコード範囲が不正です。有効な番号を二つ入力してください。"],
  maintenance_incomplete: ["Some selected windows remain. Inspect the receipt before another operation.", "選んだ窓の一部が未完了です。次の処理の前に結果を確認してください。"],
  maintenance_rebuild: ["Rebuild search and derived information", "検索用の情報を作り直す"],
  maintenance_restore: ["Restore from the configured backup", "バックアップから復元する"],
  maintenance_rebuild_hint: ["Use the saved records and change history to rebuild search, cards and summaries. Preview the current data and recovery needs first.", "保存した記録と変更履歴から検索・カード・要約を作り直します。現在の記録と復元の必要性を先に確認します。"],
  maintenance_restore_hint: ["Restore the configured backup into this memory home. This can replace current records; preview the selected backup and preserved data first.", "設定済みのバックアップをこの保存先に復元します。現在の記録が置き換わるため、選択されたバックアップと残す記録を先に確認します。"],
  maintenance_native_preview: ["Preview data and effects", "変更する記録と影響を確認"],
  maintenance_native_preview_h: ["Current data and recovery effects", "現在の記録と変更内容"],
  maintenance_rebuild_consent: ["Rebuild search from the saved records and apply the previewed changes.", "保存した記録から検索用の情報を再構築し、確認した変更を適用する。"],
  maintenance_restore_consent: ["Replace current records with the previewed backup and rebuild search information.", "確認したバックアップで現在の記録を置き換え、検索用の情報を再構築する。"],
  maintenance_native_start: ["Apply the previewed changes", "確認した内容で実行"],
  maintenance_native_refresh: ["Inspect current status", "現在の状況を確認"],
  maintenance_native_another: ["Prepare another operation after inspection", "確認後に別の処理を準備"],
  maintenance_native_unknown_hint: ["This operation may already have completed. Inspect the receipt and current data before another operation. It is not sent again automatically.", "処理が完了済みの可能性があります。結果と現在の記録を確認してから、別の処理を準備してください。自動では再送しません。"],
  maintenance_native_settings_hint: ["This operation uses saved settings and does not submit other pending edits.", "保存済みの設定を使い、ほかの編集中の内容は送信しません。"],
  maintenance_native_committed_boundary: ["Some local stages completed, even when their record count is zero. Inspect the shown results and preserved data before continuing.", "件数がゼロでも、一部のローカル処理は完了しています。表示された結果と保持データを確認してから続けてください。"],
  maintenance_native_phase_running: ["Operation is running", "処理中"],
  maintenance_native_phase_complete: ["Operation completed", "処理完了"],
  maintenance_native_phase_partial: ["Operation stopped after local progress", "保存済みの進捗を残して停止"],
  maintenance_native_phase_failed: ["Operation did not complete", "処理は完了しませんでした"],
  maintenance_native_phase_unknown: ["Operation result needs inspection", "処理結果の確認が必要です"],
  maintenance_current_records: ["Current saved records: {records}; change history: {ops}.", "現在の保存記録：{records}件、変更履歴：{ops}件。"],
  maintenance_current_unknown: ["Current record counts are unavailable.", "現在の記録数は取得できません。"],
  maintenance_cached: ["Cached search information: {count}; previously preserved files: {files}.", "保存済みの検索用情報：{count}件、以前から保持されている保存ファイル：{files}個。"],
  maintenance_unknown_count: ["unknown", "不明"],
  maintenance_forget_requests: ["Deletion requests to preserve: {count}.", "復元でも守る削除指定：{count}件。"],
  maintenance_backup_candidates: ["Backup: {bytes} compressed bytes; {records} record segments and {ops} change-history segments. Skipped or invalid segments: {skipped}.", "バックアップ：圧縮後{bytes}バイト、記録{records}区間、変更履歴{ops}区間。無効・読み飛ばし区間：{skipped}。"],
  maintenance_backup_work: ["Candidate record rows: {records}; change-history rows: {ops}. These are candidates; actual restored counts are known after validation.", "候補の記録：{records}行、変更履歴：{ops}行。これは候補です。実際に復元した件数は検証後に分かります。"],
  maintenance_cache_limit: ["Cached search data does not mean semantic search is ready.", "保存済みの検索用情報があっても、意味検索の準備完了とは限りません。"],
  maintenance_restored_actual: ["Restored records: {records}; replayed change history: {ops}; dropped changes: {dropped}; skipped segments: {skipped}.", "復元した記録：{records}件、再適用した変更履歴：{ops}件、除いた変更：{dropped}件、読み飛ばした区間：{skipped}。"],
  maintenance_restored_prepared: ["Prepared records: {records}; change history: {ops}. The prepared data has not replaced the current records.", "準備した記録：{records}件、変更履歴：{ops}件。この準備データは現在の記録に置き換わっていません。"],
  maintenance_kept_files: ["Preserved previous files: records {raw}, search information {knowledge}, backups {backups}.", "以前の保存ファイルを保持：記録{raw}個、検索用{knowledge}個、バックアップ{backups}個。"],
  maintenance_index_complete: ["Search and derived information are ready.", "検索と表示用の情報を作り直しました。"],
  maintenance_index_failed: ["Search rebuilding failed. Inspect this receipt before trying another operation.", "検索用の情報の再構築に失敗しました。次の操作を始める前に、この結果を確認してください。"],
  maintenance_index_not_started: ["Search rebuilding has not started.", "検索用の情報の再構築は始まっていません。"],
  maintenance_carried_cache: ["Cached search information carried: {count}.", "引き継いだ保存済みの検索用情報：{count}件。"],
  maintenance_cleanup_warnings: ["Previous-file cleanup warnings: {count}.", "以前のファイルの整理に関する注意：{count}件。"],
  maintenance_backup_warnings: ["Backup warnings: {count}.", "バックアップに関する注意：{count}件。"],
  maintenance_forget_warnings: ["Deletion-history warnings: {count}.", "削除指定の履歴に関する注意：{count}件。"],
  maintenance_put_back: ["Previous search information was put back.", "以前の検索用情報を戻しました。"],
  maintenance_raw_put_back: ["Previous record files were put back.", "以前の記録ファイルを戻しました。"],
  maintenance_old_knowledge_kept: ["Previous search files are preserved for recovery.", "復旧確認に使えるよう、以前の検索用保存ファイルを保持しています。"],
  maintenance_stopped_restore_finished: ["A previously interrupted restore was completed before continuing.", "続行する前に、以前中断した復元を完了しました。"],
  maintenance_quarantined_records: ["Previous record files preserved: {count}.", "保持した以前の記録ファイル：{count}個。"],
  maintenance_quarantined_segments: ["Backup segments preserved: {count}.", "保持したバックアップ区間：{count}。"],
  maintenance_raw_recovery: ["Saved records were also recovered from backup.", "保存記録のバックアップからの復元も行いました。"],
  maintenance_native_progress: ["Stage: {stage}.", "進行段階：{stage}。"],
  maintenance_index_progress: ["Saved update: {consumer}; processed position {checkpoint} ({unit}). This is a checkpoint, not a count of new records.", "保存した更新：{consumer}、処理済み位置{checkpoint}（{unit}）。新しく作った記録の件数ではありません。"],
  maintenance_unit_records: ["records", "記録"],
  maintenance_unit_ops: ["change history", "変更履歴"],
  maintenance_stage_knowledge_set_aside: ["Preserving previous search information", "以前の検索用情報を保持"],
  maintenance_stage_knowledge_put_back: ["Putting previous search information back", "以前の検索用情報を戻す"],
  maintenance_stage_vectors_carried: ["Carrying cached search information", "保存済みの検索用情報を引き継ぐ"],
  maintenance_stage_raw_quarantined: ["Preserving previous records", "以前の記録を保持"],
  maintenance_stage_raw_swapped: ["Saving restored records", "復元した記録を保存"],
  maintenance_stage_raw_put_back: ["Putting previous record files back", "以前の記録ファイルを戻す"],
  maintenance_stage_stopped_restore_finished: ["Completing an interrupted restore", "中断した復元を完了"],
  maintenance_stage_segments_quarantined: ["Preserving damaged backup files", "壊れたバックアップを保持"],
  maintenance_stage_stores_changed: ["Saving updated local information", "更新した保存情報を保存"],
  maintenance_stage_forget_reconciled: ["Applying deletion requests", "削除指定を反映"],
  maintenance_stage_backup_written: ["Saving backup files", "バックアップを保存"],
  maintenance_stage_backup_quarantined: ["Preserving invalid backup files", "無効なバックアップを保持"],
  maintenance_stage_indexing: ["Rebuilding search and display information", "検索と表示用の情報を再構築"],
  maintenance_staged_partial: ["An interrupted restore left prepared data. Inspect the outcome of this operation before continuing.", "中断した復元の準備データが残っています。続行する前に今回の実行結果を確認してください。"],
  maintenance_stage_directory_synced: ["Saving file changes", "ファイルの変更を保存"],
  maintenance_consumer_rescan: ["Redaction and deletion checks", "伏せ字と削除の確認"],
  maintenance_consumer_fts: ["Full-text search", "全文検索"],
  maintenance_consumer_claims: ["Memories", "記憶"],
  maintenance_consumer_anchors: ["Evidence", "根拠"],
  maintenance_consumer_cards: ["Cards", "カード"],
  maintenance_consumer_turns: ["Session summaries", "セッションの要約"],
  maintenance_consumer_imported: ["Imported history", "取り込み履歴"],
  maintenance_consumer_manifest: ["Work locations", "作業場所"],
  maintenance_consumer_gaps: ["Uncurated records", "未整理の記録"],
  maintenance_consumer_compress: ["Storage compression", "保存の圧縮"],
  maintenance_recovery_required: ["An interrupted restore or damaged store needs recovery before this operation. Inspect the preserved data first.", "中断した復元、または壊れた保存記録の復旧が必要です。先に保持されている記録を確認してください。"],
  maintenance_kind: ["Operation", "操作"],
  maintenance_transcripts: ["Agent transcripts", "エージェントの会話履歴"],
  maintenance_v1: ["Older oboete store", "旧oboeteのストア"],
  maintenance_agent: ["Agent", "エージェント"],
  maintenance_all_agents: ["Claude Code and Codex", "Claude CodeとCodex"],
  maintenance_claude: ["Claude Code", "Claude Code"],
  maintenance_codex: ["Codex", "Codex"],
  maintenance_source_path: ["Source database path (optional)", "元データベースのパス（任意）"],
  maintenance_source_default: ["Leave the path empty to use the older oboete.db in this memory folder. Other paths follow the native local-file operation.", "空欄ではこの記憶フォルダー内の旧oboete.dbを使います。別のパスは既存のローカルファイル操作に従います。"],
  maintenance_native_roots: ["Reads the agent’s saved transcript folders. The preview shows candidates, before duplicate and forgotten-record checks.", "エージェントが保存した会話フォルダーを読みます。プレビューは重複や忘却済みデータの確認前の候補数です。"],
  maintenance_preview: ["Preview local import", "取り込み内容を確認"],
  maintenance_preview_h: ["Candidate history and settings", "候補の履歴と設定"],
  maintenance_consent: ["Import this previewed source and apply the listed settings effects.", "確認した履歴を取り込み、表示された設定への影響を適用する。"],
  maintenance_start: ["Import confirmed history", "確認した履歴を取り込む"],
  maintenance_refresh: ["Inspect current import status", "取り込み状況を確認"],
  maintenance_another: ["Prepare another import after inspection", "確認後に別の取り込みを準備"],
  maintenance_unknown_hint: ["The request may already be recorded. Inspect its status and stored history before preparing another import. It is not sent again automatically.", "処理が記録済みの可能性があります。状況と保存された履歴を確認してから、別の取り込みを準備してください。自動では再送しません。"],
  maintenance_no_receipt: ["No receipt is available in this viewer session. After a restart, inspect stored history before another preview.", "このビューアのセッションには結果がありません。再起動後は保存された履歴を確認してから再びプレビューしてください。"],
  maintenance_settings_hint: ["Import actions use saved settings and do not submit unrelated drafts. If older settings were copied, reload settings explicitly to inspect them.", "取り込みは保存済み設定を使い、ほかの編集中の内容は送信しません。旧設定がコピーされた場合は、設定を明示的に再読み込みして確認してください。"],
  maintenance_other_unavailable: ["Claude-mem import waits for repository mapping; updating is not built yet.", "claude-memの取り込みはリポジトリ対応づけを待っています。更新機能はまだ未実装です。"],
  maintenance_transcript_candidates: ["{agent}: {files} files, {sessions} sessions, {records} candidate records, {bytes} selected bytes; {waiting} waiting, {refused} refused.", "{agent}: {files}ファイル、{sessions}セッション、候補{records}レコード、選択{bytes}バイト。待機{waiting}、拒否{refused}。"],
  maintenance_transcript_actual: ["{agent}: {records} committed records; {seen} previously seen, {waiting} waiting, {refused} refused.", "{agent}: 確定{records}レコード。既存{seen}、待機{waiting}、拒否{refused}。"],
  maintenance_conditional_v1: ["This transcript import also includes the older store in this memory folder. Confirmation covers both imports and the following settings effect.", "この会話履歴の取り込みには、この記憶フォルダー内の旧ストアも含まれます。確認は両方の取り込みと次の設定への影響を対象にします。"],
  maintenance_v1_candidates: ["Older store candidates: {events} events, {records} records, {repos} repository touches, {documents} documents, {bytes} bytes.", "旧ストアの候補: {events}イベント、{records}レコード、{repos}リポジトリ履歴、{documents}文書、{bytes}バイト。"],
  maintenance_v1_actual: ["Older store: {events} events read; {records} records, {repos} repository touches and {documents} documents committed; {seen} previously imported.", "旧ストア: {events}イベントを読み取り、{records}レコード・{repos}リポジトリ履歴・{documents}文書が確定しました。既存{seen}。"],
  maintenance_settings_effect: ["Settings: {effect}. {missing} groups are not answered by the older file.", "設定: {effect}。旧ファイルには{missing}種類の設定がありません。"],
  maintenance_settings_preserve: ["keep the current file", "現在のファイルを保持"],
  maintenance_settings_copy: ["copy the older file unchanged into a folder without settings", "設定がないフォルダーへ旧ファイルをそのままコピー"],
  maintenance_settings_defaults: ["use the current defaults", "現在の既定値を使用"],
  maintenance_progress: ["{stage}: committed {records} older records, {repos} repository touches, {documents} documents; Claude Code {claude} and Codex {codex} transcript records.", "{stage}: 旧レコード{records}、リポジトリ履歴{repos}、文書{documents}が確定。会話履歴はClaude Code {claude}、Codex {codex}レコード。"],
  maintenance_committed_boundary: ["A native checkpoint or settings step has committed, even when its payload count is zero. Earlier progress is kept; inspect it before a new preview.", "保存件数がゼロでも、読み取り位置または設定の確定済み処理があります。それまでの進捗は残ります。新しいプレビュー前に確認してください。"],
  maintenance_phase_running: ["Import is running", "取り込み中"],
  maintenance_phase_complete: ["Import completed", "取り込み完了"],
  maintenance_phase_partial: ["Import stopped with committed progress", "確定した進捗を残して停止"],
  maintenance_phase_failed: ["Import did not complete", "取り込みは完了しませんでした"],
  maintenance_phase_unknown: ["Import result needs inspection", "取り込み結果の確認が必要です"],
  maintenance_stage_checking: ["Checking confirmed scope", "確認した範囲を検証"],
  maintenance_stage_settings: ["Older settings", "旧設定"],
  maintenance_stage_v1_events: ["Older events", "旧イベント"],
  maintenance_stage_v1_repositories: ["Older repository history", "旧リポジトリ履歴"],
  maintenance_stage_v1_documents: ["Older documents", "旧文書"],
  maintenance_stage_transcripts: ["Agent transcripts", "会話履歴"],
  maintenance_stage_complete: ["Completed", "完了"],
  maintenance_stage_partial: ["Stopped after committed progress", "確定済み進捗を残して停止"],
  maintenance_stage_failed: ["Stopped", "停止"],
  maintenance_stage_unknown: ["Completion needs inspection", "完了状態の確認が必要"],
  maintenance_preview_failed: ["A safe preview could not be prepared. The operation was not started.", "安全なプレビューを作成できませんでした。処理は開始していません。"],
  maintenance_status_unavailable: ["Operation status could not be read. The running operation is not cancelled.", "状況を読み取れませんでした。進行中の処理は中止していません。"],
  maintenance_unknown: ["The result is unknown. Inspect before another operation.", "処理結果が不明です。別の処理の前に確認してください。"],
  maintenance_stale: ["Source, settings or effects changed. Prepare a fresh preview.", "元データ・設定・影響が変わりました。新しいプレビューを作成してください。"],
  maintenance_source: ["The local source could not be safely used. Check its path and data.", "元データを安全に使えませんでした。パスとデータを確認してください。"],
  maintenance_source_missing: ["There is no older oboete.db in this memory folder. Choose an existing source file to preview.", "この記憶フォルダーには旧oboete.dbがありません。確認する元ファイルを指定してください。"],
  maintenance_config: ["Saved or older settings are invalid. Inspect them before continuing.", "保存済み設定または旧設定が不正です。続行する前に確認してください。"],
  maintenance_busy: ["Another operation is active. Inspect status and wait.", "別の処理が進行中です。状況を確認して待ってください。"],
  maintenance_refused: ["Some files were refused. Earlier committed progress is kept. Inspect it before another preview.", "取り込めないファイルがありました。確定済みの進捗は残ります。別のプレビュー前に確認してください。"],
  maintenance_failed: ["The operation stopped. Inspect the shown progress and current data before proceeding.", "処理が停止しました。表示された進捗と保存済みの記録を確認してから進めてください。"],
  maintenance_confirmation: ["Confirm the previewed scope and effects first.", "先にプレビューの対象と影響を確認してください。"],
  maintenance_key: ["The preview is invalid. Prepare it again.", "プレビューが不正です。作成し直してください。"],
  maintenance_id: ["The operation identifier is invalid. Prepare a fresh operation.", "処理の識別情報が不正です。新しい処理を準備してください。"],
  maintenance_id_changed: ["That operation identifier belongs to another confirmed request. Inspect status.", "その識別情報は別の確認済み処理のものです。状況を確認してください。"],

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
// Preferences are separate append operations: retain their draft/receipt for this page only.
// Wire refreshMaintenance before database reads in the existing poll(), while settings is shown
// or an operation/unknown receipt needs inspection. This replaces a separate polling timer.
let maintenanceDraft = { kind:'transcripts', agent:'all', from:'', recurateScope:'queued', recurateFrom:'', recurateTo:'', preview:null, confirmed:false,
  operationId:null, previewing:false, sending:false, unknown:false, status:null, error:null };
const MAINTENANCE_PREFIX = {rebuild:'maintenance_native_',restore:'maintenance_native_',
  recurate:'maintenance_recurate_',finish:'maintenance_finish_'};
function maintenancePrefix(kind) {
  return Object.hasOwn(MAINTENANCE_PREFIX,kind) ? MAINTENANCE_PREFIX[kind] : 'maintenance_';
}
function maintenanceOperation(d) {
  if(d.kind==='rebuild'||d.kind==='restore'||d.kind==='finish')return {kind:d.kind};
  if(d.kind==='recurate') {
    let scope={kind:d.recurateScope};
    if(d.recurateScope==='records')scope={kind:'records',from:Number(d.recurateFrom),to:Number(d.recurateTo)};
    else if(d.recurateScope==='imported_v1')scope={kind:'imported',source:'v1'};
    else if(d.recurateScope==='imported_transcripts')scope={kind:'imported',source:'transcripts'};
    return {kind:'recurate',scope};
  }
  if(d.kind==='v1')return {kind:'v1',from:d.from || null};
  return {kind:'transcripts',agent:d.agent === 'all' ? null : d.agent};
}
function maintenanceRangeValid(d) {
  if(d.kind!=='recurate'||d.recurateScope!=='records')return true;
  const from=Number(d.recurateFrom),to=Number(d.recurateTo);
  return /^\d+$/.test(d.recurateFrom) && /^\d+$/.test(d.recurateTo)
    && Number.isSafeInteger(from) && Number.isSafeInteger(to) && from>0 && to>=from;
}
function maintenanceRun(d) {
  return [d.status?.active,d.status?.last].find(run=>run?.operation_id===d.operationId) || null;
}
function maintenanceBlocked(d) {
  return d.previewing || d.sending || d.unknown || Boolean(d.status?.active);
}
function renderMaintenance(d) {
  if(!form || form.maintenance!==d || view!=='settings')return;
  const section=$('panel').querySelector('.maintenance');
  if(!section)return;
  const focused=section.contains(document.activeElement) ? document.activeElement : null;
  let key=null;
  if(focused?.dataset.field)key=['field',focused.dataset.field];
  else if(focused?.dataset.action)key=['action',focused.dataset.action];
  const selection=typeof focused?.selectionStart==='number'
    ? [focused.selectionStart,focused.selectionEnd,focused.selectionDirection] : null;
  section.replaceChildren(...maintenanceSection(form).childNodes);
  const next=key && section.querySelector(`[data-${key[0]}="${key[1]}"]`);
  if(next && !next.disabled){next.focus({preventScroll:true});if(selection)next.setSelectionRange(...selection);}
}
async function refreshMaintenance(d = maintenanceDraft) {
  const read = d.statusRead = (d.statusRead || 0) + 1;
  try {
    const status = await api('maintenance');
    if (d.statusRead !== read) return;
    const changed=JSON.stringify(d.status)!==JSON.stringify(status) || d.error!==null;
    const wasUnknown=d.unknown;
    d.status=status;
    const prepared=status.last?.result?.outcome?.preview;
    let previewChanged=false;
    if(d.preview?.kind==='recurate' && prepared?.preview_key===d.preview.preview_key) {
      previewChanged=JSON.stringify(d.preview.plan)!==JSON.stringify(prepared.plan);
      if(previewChanged)d.confirmed=false;
      d.preview={...prepared,preparation:status};
    }
    d.error=null;
    if(maintenanceRun(d))d.unknown=false;
    if(changed || previewChanged || wasUnknown!==d.unknown)renderMaintenance(d);
  } catch {
    if (d.statusRead !== read) return;
    const changed=d.error!=='maintenance_status_unavailable';
    d.error='maintenance_status_unavailable';
    if(changed)renderMaintenance(d);
  }
}
function applyMaintenancePreview(d,recurate,res,answer) {
  if(!res.ok) {
    d.error=answer.code || 'maintenance_preview_failed';
    if(recurate && res.status>=500){d.unknown=true;}
    return;
  }
  if(recurate && answer.preparation) {
    d.statusRead=(d.statusRead || 0)+1;
    d.status=answer.preparation;
  }
  if(recurate && (!/^[0-9a-f]{64}$/.test(answer.preview_key || '')
    || answer.preparation && answer.preparation.last?.phase!=='prepared')) {
    d.error='maintenance_recurate_preview_failed';
    return;
  }
  d.preview=answer;
}
async function prepareMaintenance(d,f,invalidate,recurate) {
  if(maintenanceBlocked(d)||!maintenanceRangeValid(d))return;
  invalidate();d.previewing=true;renderMaintenance(d);
  try {
    const {res,answer}=await memoryWrite('maintenance/preview',{operation:maintenanceOperation(d)});
    if(form!==f)return;
    applyMaintenancePreview(d,recurate,res,answer);
  } catch {
    if(recurate){d.unknown=true;d.error='maintenance_recurate_prepare_unknown';await refreshMaintenance(d);}
    else d.error='maintenance_preview_failed';
  } finally {
    d.previewing=false;
    if(form!==f && recurate){await refreshMaintenance(d);}
    renderMaintenance(d);
  }
}
async function startMaintenance(d,f,recurate,finish) {
  if(maintenanceBlocked(d)||!d.preview?.preview_key||!d.confirmed)return;
  const bytes=crypto.getRandomValues(new Uint8Array(32));
  d.operationId=[...bytes].map(b=>b.toString(16).padStart(2,'0')).join('');
  d.sending=true;d.unknown=false;d.error=null;
  const id=d.operationId;
  const posted={operation:maintenanceOperation(d),preview_key:d.preview.preview_key,
    operation_id:id,confirmed:true};
  d.preview=null;d.confirmed=false;
  renderMaintenance(d);
  try {
    const {res,answer}=await memoryWrite('maintenance/start',posted);
    if(d.operationId!==id || form!==f)return;
    if(!res.ok) {
      d.error=answer.code || 'maintenance_failed';
      if((recurate||finish) && res.status>=500){d.unknown=true;}
      return;
    }
    d.statusRead = (d.statusRead || 0) + 1;
    d.status=answer;
  } catch {
    if(d.operationId===id){d.unknown=true;d.error='maintenance_unknown';}
  } finally {
    d.sending=false;
    await refreshMaintenance(d);
    renderMaintenance(d);
  }
}
function maintenanceStatusSection(d,prefix) {
  const active=d.status?.active,last=d.status?.last;
  const unknownHint=d.kind==='recurate'&&!d.operationId
    ? 'maintenance_recurate_prepare_unknown' : prefix+'unknown_hint';
  return [d.previewing ? el('p','desc',t('loading')) : null,
    d.error ? el('p','desc text maintenance-error',t(d.error)) : null,
    d.unknown ? el('p','desc',t(unknownHint)) : null,
    active ? maintenanceStatus(active) : null,last ? maintenanceStatus(last) : null,
    d.status && !d.status.available ? el('p','desc',t('maintenance_no_receipt')) : null];
}
function invalidateMaintenance(d) {
  d.preview=null;d.confirmed=false;d.operationId=null;d.error=null;
  const section=$('panel').querySelector('.maintenance');
  section?.querySelector('.maintenance-preview')?.remove();
  section?.querySelector('.maintenance-error')?.remove();
  const consent=section && [...section.querySelectorAll('input')].find(input=>input.dataset.field==='maintenance.confirmed');
  if(consent){consent.checked=false;consent.disabled=true;}
  const start=section && [...section.querySelectorAll('button')].find(button=>button.dataset.action==='maintenance.start');
  if(start)start.disabled=true;
}
function maintenanceSection(f) {
  const d=f.maintenance;
  const native=d.kind==='rebuild'||d.kind==='restore';
  const recurate=d.kind==='recurate';
  const finish=d.kind==='finish';
  const prefix=maintenancePrefix(d.kind);
  const busy=maintenanceBlocked(d);
  const invalidate=()=>invalidateMaintenance(d);
  const kind=el('select',null);
  for(const [value,label] of [['transcripts','maintenance_transcripts'],['v1','maintenance_v1'],['rebuild','maintenance_rebuild'],['restore','maintenance_restore'],['recurate','maintenance_recurate'],['finish','maintenance_finish']]) {
    const option=el('option',null,t(label));option.value=value;kind.append(option);
  }
  kind.value=d.kind;kind.dataset.field='maintenance.kind';kind.disabled=busy;
  kind.addEventListener('change',()=>{d.kind=kind.value;invalidate();renderMaintenance(d);});
  const agent=el('select',null);
  for(const [value,label] of [['all','maintenance_all_agents'],['claude','maintenance_claude'],['codex','maintenance_codex']]) {
    const option=el('option',null,t(label));option.value=value;agent.append(option);
  }
  agent.value=d.agent;agent.dataset.field='maintenance.agent';agent.disabled=busy;
  agent.addEventListener('change',()=>{d.agent=agent.value;invalidate();});
  const from=input('text',d.from,'','maintenance.from',value=>{d.from=value;invalidate();});
  from.disabled=busy;
  const scope=el('select',null);
  for(const [value,label] of [['queued','maintenance_recurate_queued'],['skipped','maintenance_recurate_skipped'],
    ['imported_v1','maintenance_recurate_imported_v1'],['imported_transcripts','maintenance_recurate_imported_transcripts'],
    ['records','maintenance_recurate_records']]) {
    const option=el('option',null,t(label));option.value=value;scope.append(option);
  }
  scope.value=d.recurateScope;scope.dataset.field='maintenance.scope';scope.disabled=busy;
  scope.addEventListener('change',()=>{d.recurateScope=scope.value;invalidate();renderMaintenance(d);});
  const recordField=(name)=>{
    const control=input('number',d[name],'',`maintenance.${name}`,value=>{d[name]=value;invalidate();renderMaintenance(d);});
    control.min='1';control.disabled=busy;
    return el('label','field',el('span',null,t(name==='recurateFrom'?'maintenance_recurate_from':'maintenance_recurate_to')),control);
  };
  const preview=el('button','quiet small',t(prefix+'preview'));
  preview.dataset.action='maintenance.preview';
  preview.type='button';preview.disabled=busy || !maintenanceRangeValid(d);
  preview.addEventListener('click',()=>void prepareMaintenance(d,f,invalidate,recurate));
  const confirm=checkbox(d.confirmed,value=>{d.confirmed=value;renderMaintenance(d);});
  confirm.dataset.field='maintenance.confirmed';confirm.disabled=busy || !d.preview;
  const start=el('button','quiet small',t(prefix+'start'));
  start.dataset.action='maintenance.start';
  start.type='button';start.disabled=busy || !d.preview?.preview_key || !d.confirmed;
  start.addEventListener('click',()=>void startMaintenance(d,f,recurate,finish));
  const refresh=el('button','quiet small',t(prefix+'refresh'));
  refresh.dataset.action='maintenance.refresh';
  refresh.type='button';refresh.addEventListener('click',()=>{void refreshMaintenance(d);});
  const another=el('button','quiet small',t(prefix+'another'));
  another.dataset.action='maintenance.another';
  another.type='button';another.disabled=d.previewing || d.sending || Boolean(d.status?.active);
  another.addEventListener('click',()=>{
    if(d.sending||d.status?.active)return;
    d.unknown=false;invalidate();renderMaintenance(d);
  });
  const selection={transcripts:['maintenance_agent',agent],v1:['maintenance_source_path',from],recurate:['maintenance_recurate_scope',scope]}[d.kind];
  const scopeControl=selection ? el('label','field',el('span',null,t(selection[0])),selection[1]) : null;
  const hintKey={v1:'maintenance_source_default',transcripts:'maintenance_native_roots'}[d.kind] ?? `maintenance_${d.kind}_hint`;
  const consentKey={rebuild:'maintenance_rebuild_consent',restore:'maintenance_restore_consent',recurate:'maintenance_recurate_consent',finish:'maintenance_finish_consent'}[d.kind] ?? 'maintenance_consent';
  const previewDetails=[];
  if(d.preview)previewDetails.push(maintenancePreview(d.preview),el('label','check',confirm,t(consentKey)));
  return el('section','maintenance',el('h3',null,t('maintenance_h')),el('p','desc',t('maintenance_desc')),
    el('label','field',el('span',null,t('maintenance_kind')),kind),
    scopeControl,recurate&&d.recurateScope==='records' ? el('div','grid',recordField('recurateFrom'),recordField('recurateTo')) : null,
    recurate&&d.recurateScope==='records' ? el('p','desc',t('maintenance_recurate_range_hint')) : null,
    el('p','desc',t(hintKey)),preview,...previewDetails,
    start,...maintenanceStatusSection(d,prefix),
    refresh,(d.operationId || d.unknown) && !d.status?.active ? another : null,
    el('p','desc',t(prefix+'settings_hint')),
    native || recurate || finish ? null : el('p','desc',t('maintenance_other_unavailable')));
}
function maintenanceNativePreview(p) {
  const rows=[];
  rows.push(el('p','desc',p.raw ? t('maintenance_current_records',p.raw) : t('maintenance_current_unknown')),
    el('p','desc',t('maintenance_cached',{count:p.cached_vectors ?? t('maintenance_unknown_count'),files:p.kept_files})),
    el('p','desc',t('maintenance_forget_requests',{count:p.forget_requests})),
    el('p','desc',t('maintenance_cache_limit')));
  if(p.staged_partial)rows.push(el('p','desc',t('maintenance_staged_partial')));
  if(p.backup)rows.push(el('p','text',p.backup_label),
    el('p','desc',t('maintenance_backup_candidates',{bytes:p.backup.compressed_bytes,
      records:p.backup.record_segments,ops:p.backup.op_segments,
      skipped:p.backup.invalid_record_segments+p.backup.skipped_op_segments})),
    el('p','desc',t('maintenance_backup_work',{records:p.backup.record_lines ?? t('maintenance_unknown_count'),
      ops:p.backup.op_lines ?? t('maintenance_unknown_count')})));
  if(p.forget_log_warnings)rows.push(el('p','desc',t('maintenance_forget_warnings',{count:p.forget_log_warnings})));
  return rows;
}
function maintenanceRecuratePreview(p) {
  const s=p.plan;
  let scope;
  if(p.scope.kind==='records')scope=t('maintenance_recurate_range',p.scope);
  else {
    const kind=p.scope.kind==='imported' ? `imported_${p.scope.source}` : p.scope.kind;
    scope=t('maintenance_recurate_'+kind);
  }
  const rows=[el('p','desc',t('maintenance_recurate_selected_scope',{scope})),
    el('p','desc',t('maintenance_recurate_plan',{
    spans:s.spans,windows:s.windows,tokens:s.tokens,kept:s.kept_back,unparked:s.unparked_records}))];
  rows.push(el('p','desc',s.worst_paid_usd===null
    ? t('maintenance_recurate_no_paid')
    : t('maintenance_recurate_cost',{usd:String(s.worst_paid_usd)})));
  if(s.long_sessions_total)rows.push(el('p','desc',t('maintenance_recurate_long',{total:s.long_sessions_total})),
    el('ol',null,...s.long_sessions.map(item=>el('li',null,t('maintenance_recurate_long_item',item)))));
  return rows;
}
function maintenanceFinishPreview(p) {
  return [el('p','desc',t('maintenance_finish_candidates',p.candidates)),
    el('p','desc',t('maintenance_finish_order')),
    el('p','desc',t('maintenance_finish_deletion',{
      targets:p.deletion.targets.length,nodes:p.deletion.nodes,bytes:p.deletion.bytes})),
    el('ul',null,...p.deletion.targets.map(label=>el('li','text',label || t('maintenance_finish_target_hidden'))))];
}
function maintenanceTranscriptPreview(p) {
  const rows=[];
  for(const [agent,s] of Object.entries(p.candidates)) {
    if(s)rows.push(el('p','desc',t('maintenance_transcript_candidates',{
      agent:t(agent==='claude'?'maintenance_claude':'maintenance_codex'),files:s.files,
      sessions:s.sessions,records:s.events,bytes:s.bytes,waiting:s.waiting,refused:s.refused})));
  }
  if(p.v1)rows.push(el('p','desc',t('maintenance_conditional_v1')),
    maintenanceV1(p.v1.candidates,p.v1.settings,true));
  return rows;
}
function maintenancePreview(p) {
  const rows=[];
  const native=p.kind==='rebuild'||p.kind==='restore';
  if(p.kind==='finish') {
    rows.push(...maintenanceFinishPreview(p));
  } else if(p.kind==='recurate') {
    rows.push(...maintenanceRecuratePreview(p));
  } else if(native) {
    rows.push(...maintenanceNativePreview(p));
  } else if(p.kind==='v1') {
    rows.push(el('p','text',p.source),maintenanceV1(p.candidates,p.settings,true));
  } else {
    rows.push(...maintenanceTranscriptPreview(p));
  }
  return el('div','maintenance-preview',el('h4',null,t(maintenancePrefix(p.kind)+'preview_h')),...rows);
}
function maintenanceV1(s,settings,candidate=false) {
  return el('div',null,el('p','desc',t(candidate?'maintenance_v1_candidates':'maintenance_v1_actual',{
    events:s.events,records:s.records,repos:s.repos ?? s.repositories,documents:s.documents,
    bytes:s.bytes ?? 0,seen:s.seen ?? 0})),settings ? el('p','desc',t('maintenance_settings_effect',{
      effect:t('maintenance_settings_'+settings.effect),missing:settings.missing.length})) : null);
}
function maintenanceRestoreReceipt(receipt,recovery=false) {
  const effects=receipt.effects;
  const rows=[];
  if(!(recovery&&effects.stopped_restore_finished&&receipt.replayed_records===0&&receipt.replayed_ops===0)) {
    rows.push(el('p','desc',t(effects.raw_swapped?'maintenance_restored_actual':'maintenance_restored_prepared',{
      records:receipt.replayed_records,ops:receipt.replayed_ops,dropped:receipt.dropped_ops,skipped:receipt.skipped_segments})));
  }
  rows.push(el('p','desc',t('maintenance_kept_files',{raw:receipt.old_raw_kept_files,
    knowledge:receipt.old_knowledge_kept_files,backups:receipt.backup_files_kept})));
  if(effects.raw_put_back)rows.push(el('p','desc',t('maintenance_raw_put_back')));
  if(effects.knowledge_put_back)rows.push(el('p','desc',t('maintenance_put_back')));
  if(effects.stopped_restore_finished)rows.push(el('p','desc',t('maintenance_stopped_restore_finished')));
  if(effects.raw_files_quarantined)rows.push(el('p','desc',t('maintenance_quarantined_records',{count:effects.raw_files_quarantined})));
  if(effects.segments_quarantined)rows.push(el('p','desc',t('maintenance_quarantined_segments',{count:effects.segments_quarantined})));
  if(receipt.forget_log_warnings)rows.push(el('p','desc',t('maintenance_forget_warnings',{count:receipt.forget_log_warnings})));
  return rows;
}
function maintenanceNativeOutcome(out) {
  const effects=out.effects;
  const rows=[el('p','desc',t('maintenance_index_'+out.index.state)),
    el('p','desc',t('maintenance_carried_cache',{count:out.cached_vectors_carried})),
    el('p','desc',t('maintenance_cache_limit'))];
  if(out.restore)rows.push(...maintenanceRestoreReceipt(out.restore));
  if(effects.knowledge_put_back)rows.push(el('p','desc',t('maintenance_put_back')));
  if(effects.raw_put_back)rows.push(el('p','desc',t('maintenance_raw_put_back')));
  if(out.old_knowledge_kept)rows.push(el('p','desc',t('maintenance_old_knowledge_kept')));
  if(effects.stopped_restore_finished)rows.push(el('p','desc',t('maintenance_stopped_restore_finished')));
  if(effects.raw_files_quarantined)rows.push(el('p','desc',t('maintenance_quarantined_records',{count:effects.raw_files_quarantined})));
  if(effects.segments_quarantined)rows.push(el('p','desc',t('maintenance_quarantined_segments',{count:effects.segments_quarantined})));
  if(out.index.raw_recovery) {
    const recovery=out.index.raw_recovery;
    if(recovery.effects.raw_swapped&&!recovery.effects.stopped_restore_finished)rows.push(el('p','desc',t('maintenance_raw_recovery')));
    rows.push(...maintenanceRestoreReceipt(recovery,true));
  }
  for(const [key,count] of [['cleanup',out.cleanup_warnings],['backup',out.index.backup_warnings],
    ['forget',out.index.forget_log_warnings]]) {
    if(count)rows.push(el('p','desc',t(`maintenance_${key}_warnings`,{count})));
  }
  return rows;
}
function maintenanceRecurateTotals(providers,windows) {
  const rows=[];
  if(providers)rows.push(el('p','desc',t('maintenance_recurate_providers',providers)),
    el('p','desc',t('maintenance_recurate_accounted',{usd:usdValue(providers.accounted_usd)})));
  if(windows)rows.push(el('p','desc',t('maintenance_recurate_windows',windows)));
  return rows;
}
function maintenanceFinishEffects(effects) {
  if(!effects)return [];
  return [el('p','desc',t('maintenance_finish_effects',{
    changed:t(effects.stores_changed?'maintenance_finish_changed_yes':'maintenance_finish_changed_no'),
    applied:effects.forget_requests_applied,warnings:effects.forget_log_warnings}))];
}
function maintenanceFinishDeletion(deletion) {
  if(!deletion)return [];
  return [el('p','desc',t('maintenance_finish_deletion_result',deletion)),
    el('ul',null,...(deletion.targets || []).map(target=>{
      const key='maintenance_finish_target_'+target.state;
      return el('li','text',t('maintenance_finish_target_result',{
        label:target.label || t('maintenance_finish_target_hidden'),
        state:t(Object.hasOwn(TEXT,key)?key:'maintenance_finish_target_unknown_extent'),bytes:target.bytes}));
    }))];
}
function maintenanceFinishOutcome(out) {
  const rows=[];
  if(out.import)rows.push(el('p','desc',t('maintenance_finish_import',out.import)));
  if(out.import)for(const [count,labels,key] of [
    [out.import.deleted_sessions,out.import.deleted_session_labels,'maintenance_finish_missing_sessions'],
    [out.import.uncertain_identifiers,out.import.uncertain_identifier_labels,'maintenance_finish_uncertain_sessions']]) {
    if(count)rows.push(el('p','desc',t(key,{total:count,shown:labels?.length || 0})),
      el('ul',null,...(labels || []).map(label=>el('li','text',label || t('maintenance_finish_target_hidden')))));
  }
  rows.push(...maintenanceFinishEffects(out.effects),...maintenanceFinishDeletion(out.deletion),
    el('p','desc',t('maintenance_finish_kept')));
  return rows;
}
function maintenanceImportOutcome(out) {
  const rows=[];
  if(out.transcripts)for(const [agent,s] of Object.entries(out.transcripts)) {
    if(s)rows.push(el('p','desc',t('maintenance_transcript_actual',{
      agent:t(agent==='claude'?'maintenance_claude':'maintenance_codex'),records:s.events,
      seen:s.seen,waiting:s.waiting,refused:s.refused})));
  }
  const v1=out.v1 || (!out.transcripts ? out : null);
  if(v1)rows.push(maintenanceV1(v1,v1.settings));
  return rows;
}
function maintenanceOutcome(out,phase) {
  if(!out)return [];
  if(out.operation==='rebuild'||out.operation==='restore')return maintenanceNativeOutcome(out);
  if(out.operation==='finish')return maintenanceFinishOutcome(out);
  if(out.operation==='recurate')return [
    ...maintenanceRecurateTotals(out.providers,out.windows),
    out.index?.state ? el('p','desc',t('maintenance_index_'+out.index.state)) : null,
  ].filter(Boolean);
  if(out.preview?.kind==='recurate')return [
    phase==='prepared' ? null : el('p','desc',t('maintenance_recurate_local_receipt')),
    out.index?.state ? el('p','desc',t('maintenance_index_'+out.index.state)) : null,
  ].filter(Boolean);
  return maintenanceImportOutcome(out);
}
function maintenanceIndexProgress(native) {
  return t('maintenance_index_progress',{consumer:t('maintenance_consumer_'+native.consumer),
    checkpoint:native.checkpoint,unit:t('maintenance_unit_'+native.unit)});
}
function maintenanceProgressText(run) {
  const p=run.progress;
  if(run.kind==='finish')return t('maintenance_finish_progress',{
    records:p.v1_records,repositories:p.v1_repositories,documents:p.v1_documents});
  if(run.kind==='recurate')return p.native?.kind==='index'
    ? maintenanceIndexProgress(p.native)
    : t('maintenance_recurate_stage',{stage:t('maintenance_stage_'+run.stage)});
  if(run.kind==='rebuild'||run.kind==='restore')return p.native?.kind==='index'
    ? maintenanceIndexProgress(p.native)
    : t('maintenance_native_progress',{stage:t('maintenance_stage_'+run.stage)});
  return t('maintenance_progress',{
    stage:t('maintenance_stage_'+run.stage),records:p.v1_records,repos:p.v1_repositories,
    documents:p.v1_documents,claude:p.claude.events,codex:p.codex.events});
}
function maintenanceStatus(run) {
  const p=run.progress;
  const prefix=maintenancePrefix(run.kind);
  const rows=[el('p','desc',maintenanceProgressText(run))];
  if(run.result?.code)rows.push(el('p','desc text',t(run.result.code)));
  if(run.phase==='partial' && run.committed)rows.push(el('p','desc',t(prefix+'committed_boundary')));
  if(run.kind==='finish' && run.phase==='running')rows.push(...maintenanceFinishEffects(p.finish_effects));
  if(run.kind==='finish' && run.phase==='running' && p.native?.kind==='deletion')rows.push(el('p','desc',t('maintenance_finish_deletion_progress',p.native.receipt)));
  if(run.kind==='recurate' && run.phase==='running')rows.push(...maintenanceRecurateTotals(p.providers,p.windows));
  rows.push(...maintenanceOutcome(run.result?.outcome,run.phase));
  if(run.kind==='finish' && ['partial','failed','unknown'].includes(run.phase))rows.push(el('p','desc',t('maintenance_finish_inspect')));
  const result=el('div','maintenance-result',el('h4',null,t(prefix+'phase_'+run.phase)),...rows);
  result.setAttribute('role','status');
  return result;
}

let preferenceDraft = { text: '', confirmed: false, result: null };
// `[inject]`'s sizes, each checked against the range the server states for it.
const SIZES = ['session_start_chars', 'per_prompt_chars', 'correction_chars'];

function formOf(s) {
  if (s.error) return null;
  // Saved settings invalidate consent and cached display text. Keep known effects and unknown
  // operation IDs; the regular status GET reloads labels through the current server gate.
  maintenanceDraft.preview=null;
  maintenanceDraft.confirmed=false;
  maintenanceDraft.statusRead=(maintenanceDraft.statusRead || 0)+1;
  if(maintenanceDraft.status)maintenanceDraft.status=JSON.parse(JSON.stringify(maintenanceDraft.status,
    (key,value)=>{
      if(key==='label')return '';
      if(['deleted_session_labels','uncertain_identifier_labels'].includes(key))return [];
      return value;
    }));
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
    backup: { dir: s.backup?.dir ?? null, edit: text(s.backup?.dir), reset: false },
    redaction: { rules: (s.redaction?.extra_rules || []).map(redactionEdit), hashes: (s.redaction?.allowlist || []).join('\n') },
    privacy: s.privacy || privacyUnavailable(),
    preference: preferenceDraft,
    maintenance: maintenanceDraft,
    chain: s.chain.map((e) => ({
      ...e,
      edit: { on: e.on, daily_budget: text(e.daily_budget), timeout_s: text(e.timeout_s), model: text(e.model) },
    })),
    providers: (s.providers || []).map((p) => ({ ...p, edit: providerEdit(p.name, p.saved), editing: false })),
    newProvider: null,
    warnings: s.warnings,
    ranges: s.ranges,
    keyInput: s.key_input,
  };
}

function redactionEdit(rule = {}) {
  return { id: rule.id ?? '', regex: rule.regex ?? '', keywords: JSON.stringify(rule.keywords || []),
    entropy: rule.entropy === null || rule.entropy === undefined ? '' : String(rule.entropy),
    secret_group: rule.secret_group === null || rule.secret_group === undefined ? '' : String(rule.secret_group) };
}

function redactionRuleBody(edit, field) {
  let keywords;
  try { keywords = JSON.parse(edit.keywords); } catch { return { field: `${field}.keywords` }; }
  if (!Array.isArray(keywords) || keywords.some((v) => typeof v !== 'string')) return { field: `${field}.keywords` };
  const entropy = edit.entropy.trim() === '' ? null : Number(edit.entropy);
  if (entropy !== null && !Number.isFinite(entropy)) return { field: `${field}.entropy` };
  const group = edit.secret_group.trim();
  let secret_group = null;
  if (group !== '') secret_group = /^\d+$/.test(group) && Number.isSafeInteger(Number(group)) ? Number(group) : Number.NaN;
  if (Number.isNaN(secret_group)) return { field: `${field}.secret_group` };
  return { value: { id: edit.id, regex: edit.regex, keywords, entropy, secret_group } };
}

function redactionBody() {
  const extra_rules = [];
  for (const [index, edit] of form.redaction.rules.entries()) {
    const { value, field } = redactionRuleBody(edit, `redaction.extra_rules.${index}`);
    if (field) return { field };
    extra_rules.push(value);
  }
  const allowlist = form.redaction.hashes.split(/\r?\n/).map((v) => v.trim()).filter(Boolean);
  if (allowlist.some((v) => !/^[0-9a-fA-F]{64}$/.test(v))) return { field: 'redaction.allowlist' };
  return { value: { extra_rules, allowlist } };
}

function redactionSection(f) {
  const rows = f.redaction.rules.map((rule, index) => {
    const field = `redaction.extra_rules.${index}`;
    const item = (key, label) => el('label', 'field', el('span', null, t(label)),
      input(key === 'regex' || key === 'keywords' ? 'textarea' : 'text', rule[key], '', `${field}.${key}`, (v) => { rule[key] = v; }));
    const remove = el('button', 'quiet small', t('redaction_remove'));
    remove.type = 'button';
    remove.addEventListener('click', () => { f.redaction.rules.splice(index, 1); drawSettings(); });
    return el('section', 'redaction-rule', el('h4', null, t('redaction_rule', { number: index + 1 })),
      item('id', 'redaction_id'), item('regex', 'redaction_regex'),
      el('details', null, el('summary', null, t('redaction_advanced')),
        item('keywords', 'redaction_keywords'), item('entropy', 'redaction_entropy'), item('secret_group', 'redaction_group')), remove);
  });
  const add = el('button', 'quiet small', t('redaction_add'));
  add.type = 'button';
  add.addEventListener('click', () => { f.redaction.rules.push(redactionEdit()); drawSettings(); });
  // Plaintext exists only in this live field and the digest call, never in form/localStorage.
  const exact = el('input');
  exact.type = 'password';
  exact.autocomplete = 'off';
  exact.spellcheck = false;
  exact.autocapitalize = 'off';
  const hash = el('button', 'quiet small', t('redaction_hash_add'));
  hash.type = 'button';
  hash.disabled = true;
  exact.addEventListener('input', () => { hash.disabled = !exact.value; });
  exact.addEventListener('keydown', (event) => {
    if (event.key === 'Enter') { event.preventDefault(); if (!hash.disabled) hash.click(); }
  });
  hash.addEventListener('click', async () => {
    const bytes = new TextEncoder().encode(exact.value);
    exact.value = '';
    hash.disabled = true;
    const fields = hash.closest('.settings');
    f.hashing = true;
    if (fields) fields.inert = true;
    try {
      const digest = await crypto.subtle.digest('SHA-256', bytes);
      if (!currentSettings(f)) return;
      const hex = [...new Uint8Array(digest)].map((v) => v.toString(16).padStart(2, '0')).join('');
      const values = f.redaction.hashes.split(/\r?\n/).map((v) => v.trim()).filter(Boolean);
      if (!values.some((v) => v.toLowerCase() === hex)) f.redaction.hashes += `${f.redaction.hashes ? '\n' : ''}${hex}`;
      f.hashing = false;
      drawSettings();
    } catch {
      if (currentSettings(f)) setStatus(t('redaction_hash_failed'), true, lang);
    } finally {
      bytes.fill(0);
      f.hashing = false;
      if (fields) fields.inert = false;
    }
  });
  return el('section', null, el('h3', null, t('redaction_h')), el('p', 'desc', t('redaction_desc')),
    ...rows, add, el('h4', null, t('redaction_allow_h')), el('p', 'desc', t('redaction_allow_desc')),
    el('label', 'field', el('span', null, t('redaction_value')), exact), hash,
    el('details', null, el('summary', null, t('redaction_hashes')),
      input('textarea', f.redaction.hashes, '', 'redaction.allowlist', (v) => { f.redaction.hashes = v; })));
}

const AGENT_INVENTORY_IDS = ['claude','codex','grok','agy','opencode','pi','cursor'];
let agentInventory = {report:null,loading:false,error:false,read:0,expanded:null};

function agentInventoryCode(group,value) {
  if(typeof value!=='string')return t('agent_inventory_unknown');
  const key=`agent_inventory_${group}_${value}`;
  return Object.hasOwn(TEXT,key) ? t(key) : t('agent_inventory_unknown');
}

function agentInventoryFlag(group,value) {
  if(typeof value!=='boolean')return t('agent_inventory_unknown');
  return agentInventoryCode(group,value?'found':'missing');
}

function agentInventoryMatch(component) {
  if(component?.state==='not_applicable')return t('agent_inventory_match_not_applicable');
  if(component?.matches_current===true)return t('agent_inventory_match_true');
  if(component?.matches_current===false)return t('agent_inventory_match_false');
  return t('agent_inventory_match_unknown');
}

function agentInventoryRow(id,row) {
  if(!row)return el('li',null,el('h4',null,t('agent_inventory_agent_'+id)),
    el('p','desc',t('agent_inventory_unknown')));
  const capture=row?.capture;
  const mcp=row?.mcp;
  return el('li',null,el('h4',null,t('agent_inventory_agent_'+id)),
    el('p','desc',t('agent_inventory_launch',{
      state:agentInventoryFlag('launch',row.launch_file_found)})),
    el('p','desc',t('agent_inventory_directory',{
      state:agentInventoryFlag('directory',row.directory_found)})),
    el('p','desc',t('agent_inventory_capture',{
      kind:agentInventoryCode('kind',capture?.kind),state:agentInventoryCode('state',capture?.state),
      match:agentInventoryMatch(capture)})),
    el('p','desc',t('agent_inventory_mcp',{
      state:agentInventoryCode('state',mcp?.state),match:agentInventoryMatch(mcp)})),
    el('p','desc',t('agent_inventory_trust',{state:agentInventoryCode('state',row?.trust)})),
    el('p','desc',t('agent_inventory_live',{
      state:t(row.live_verified===false?'agent_inventory_live_unverified':'agent_inventory_unknown')})));
}

function agentInventorySection() {
  const refresh=el('button','quiet small',t('agent_inventory_refresh'));
  refresh.type='button';refresh.dataset.action='agent_inventory.refresh';
  refresh.addEventListener('click',()=>void refreshAgentInventory());
  const rows=[el('summary',null,t('agent_inventory_h')),
    el('p','desc',t('agent_inventory_desc')),refresh];
  const report=agentInventory.report;
  if(agentInventory.loading)rows.push(el('p','desc',t('agent_inventory_loading')));
  else if(agentInventory.error || (report && !Array.isArray(report.agents)))
    rows.push(el('p','desc',t('agent_inventory_unavailable')));
  else if(!report)rows.push(el('p','desc',t('agent_inventory_not_checked')));
  else rows.push(
    el('p','desc',t('agent_inventory_home',{state:agentInventoryCode('state',report.home)})),
    el('p','desc',t('agent_inventory_config',{state:agentInventoryCode('state',report.config)})),
    el('ul',null,...AGENT_INVENTORY_IDS.map(id=>agentInventoryRow(id,report.agents.find(row=>row?.agent===id)))));
  const section=el('details','agent-readiness',...rows);
  section.open=agentInventory.expanded ?? !form;
  section.addEventListener('toggle',()=>{if(section.isConnected)agentInventory.expanded=section.open;});
  return section;
}

function renderAgentInventory() {
  if(view!=='settings')return;
  const section=$('panel').querySelector('.agent-readiness');
  if(!section)return;
  const focused=section.contains(document.activeElement) && document.activeElement?.dataset.action==='agent_inventory.refresh';
  section.replaceChildren(...agentInventorySection().childNodes);
  if(focused)section.querySelector('[data-action="agent_inventory.refresh"]')?.focus({preventScroll:true});
}

async function refreshAgentInventory() {
  const read=agentInventory.read=agentInventory.read+1;
  agentInventory.loading=true;agentInventory.error=false;agentInventory.report=null;
  renderAgentInventory();
  try {
    const report=await api('setup');
    if(read!==agentInventory.read || view!=='settings')return;
    agentInventory.report=report;
  } catch {
    if(read!==agentInventory.read || view!=='settings')return;
    agentInventory.error=true;
  } finally {
    if(read===agentInventory.read && view==='settings') {
      agentInventory.loading=false;
      renderAgentInventory();
    }
  }
}

async function showSettings() {
  void refreshAgentInventory();
  const [s, privacy] = await Promise.all([api('settings'), api('privacy').catch(privacyUnavailable)]);
  s.privacy = privacy;
  return () => {
    form = formOf(s);
    drawSettings();
    setStatus('');
  };
}

function privacyUnavailable() {
  return { available: false, repositories: [], rescan: { state: 'unavailable', processed: null, total: null } };
}

async function memoryWrite(path, body) {
  const res = await fetch(`/api/${path}`, {
    method: 'POST', headers: { 'X-Oboete-Token': token, 'Content-Type': 'application/json' },
    body: JSON.stringify(body), referrerPolicy: 'same-origin', credentials: 'omit',
  });
  const answer = (res.headers.get('content-type') || '').startsWith('application/json') ? await res.json() : {};
  return { res, answer };
}

function memoryFailure(res, answer) {
  const code = answer.code || PROVIDER_HTTP_ERRORS[res.status];
  return code && Object.hasOwn(TEXT, code) ? t(code) : t('memory_action_failed', { status: res.status });
}

async function refreshPrivacy(f) {
  if (!f) return false;
  const mine = f.privacyRead = (f.privacyRead || 0) + 1;
  const state = await api('privacy').catch(privacyUnavailable);
  if (!currentSettings(f) || f.privacyRead !== mine) return false;
  f.privacy = state;
  if (!f.hashing) drawSettings();
  return state.available === true;
}

function privacySection(f) {
  const state = f.privacy;
  const refresh = el('button', 'quiet small', t('privacy_refresh'));
  refresh.type = 'button';
  refresh.addEventListener('click', async () => {
    refresh.disabled = true;
    try { if (await refreshPrivacy(f)) setStatus(''); } finally { refresh.disabled = false; }
  });
  const rows = (state.repositories || []).map((repo) => {
    const action = el('button', 'quiet small', t(repo.excluded ? 'privacy_undo' : 'privacy_exclude'));
    action.type = 'button';
    action.dataset.selector = repo.selector;
    action.addEventListener('click', async () => {
      const fields = action.closest('.settings');
      fields.inert = true;
      action.disabled = true;
      try {
        const { res, answer } = await memoryWrite('privacy/exclude', { selector: repo.selector, undo: repo.excluded });
        if (!currentSettings(f)) return;
        if (!res.ok) { setStatus(memoryFailure(res, answer), true, lang); return; }
        const refreshed = await refreshPrivacy(f);
        if (currentSettings(f)) {
          const receipt = t(repo.excluded ? 'privacy_undone' : 'privacy_recorded');
          setStatus(refreshed ? receipt : t('privacy_readback_failed', { receipt }), !refreshed, lang);
        }
      } catch {
        if (currentSettings(f)) setStatus(t('memory_result_unknown'), true, lang);
      } finally { fields.inert = false; action.disabled = false; }
    });
    return el('li', null, el('p', 'text', repo.label), el('p', 'desc', t(repo.excluded ? 'privacy_excluded' : 'privacy_allowed')), action);
  });
  const scan = state.rescan;
  const scanKey = `rescan_${scan.state}`;
  let listing = el('p', 'desc', t('privacy_unavailable'));
  if (state.available) listing = rows.length ? el('ul', null, ...rows) : el('p', 'desc', t('privacy_none'));
  return el('section', null, el('h3', null, t('privacy_h')), el('p', 'desc', t('privacy_desc')),
    listing,
    el('h4', null, t('rescan_h')), el('p', 'desc', t(Object.hasOwn(TEXT, scanKey) ? scanKey : 'rescan_unavailable')),
    scan.processed !== null && scan.total !== null ? el('p', 'desc', t('rescan_progress', scan)) : null, refresh);
}

function claimReceipt(answer) {
  if (answer.state === 'applied') return t('claim_applied');
  const code = answer.code;
  return code && ['claim_pending', 'claim_not_applied', 'preference_partly_recorded'].includes(code)
    ? t(code) : t('memory_result_unknown');
}

function preferenceValidation(draft) {
  if (!draft.text.trim()) return 'preference_empty';
  if ([...draft.text.trim()].length > 1000) return 'preference_too_long';
  if (!draft.confirmed) return 'preference_confirmation';
  return null;
}

function preferenceBlocked(draft) {
  return Boolean(draft.sending || (draft.result && draft.result.state !== 'applied'));
}

function preferenceRefused(res, answer) {
  if (res.ok || answer.state) return false;
  const code = answer.code || PROVIDER_HTTP_ERRORS[res.status];
  return ['bad_request', 'unauthorized', 'forbidden', 'too_large', 'preference_confirmation',
    'preference_empty', 'preference_too_long', 'claim_unavailable'].includes(code);
}

function renderPreference(draft) {
  if (view !== 'settings' || form?.preference !== draft) return false;
  // Update only this independent action, preserving another settings write or exact-value digest.
  const section = $('panel').querySelector('.preference');
  if (section) section.replaceChildren(...preferenceSection(form).childNodes);
  return true;
}

function preferenceSection(f) {
  const draft = f.preference;
  const field = input('textarea', draft.text, '', 'preference.text', (v) => { draft.text = v; });
  const confirm = checkbox(draft.confirmed, (v) => { draft.confirmed = v; });
  confirm.dataset.field = 'preference.apply_to_all_repos';
  const save = el('button', 'quiet small', t('preference_save'));
  save.type = 'submit';
  save.disabled = preferenceBlocked(draft);
  const action = el('form', null,
    el('label', 'field', el('span', null, t('preference_text')), field),
    el('label', 'check', confirm, t('preference_confirm')), save);
  action.noValidate = true;
  action.inert = Boolean(draft.sending);
  action.addEventListener('submit', async (event) => {
    event.preventDefault();
    if (save.disabled || preferenceBlocked(draft)) return;
    const code = preferenceValidation(draft);
    if (code) { setStatus(t(code), true, lang); return; }
    const fields = action.closest('.settings');
    draft.sending = true;
    draft.result = null;
    fields.inert = true;
    save.disabled = true;
    try {
      const { res, answer } = await memoryWrite('preferences', { text: draft.text, apply_to_all_repos: true });
      draft.sending = false;
      if (preferenceRefused(res, answer)) {
        if (renderPreference(draft)) setStatus(memoryFailure(res, answer), true, lang);
        return;
      }
      draft.result = res.ok || ['pending', 'directive_only'].includes(answer.state) ? answer : { state: 'unknown' };
      draft.confirmed = false;
      if (draft.result.state === 'applied') draft.text = '';
      if (renderPreference(draft)) setStatus(claimReceipt(draft.result), draft.result.state !== 'applied', lang);
    } catch {
      draft.sending = false;
      draft.result = { state: 'unknown' };
      draft.confirmed = false;
      if (renderPreference(draft)) setStatus(t('memory_result_unknown'), true, lang);
    } finally { draft.sending = false; fields.inert = false; save.disabled = preferenceBlocked(draft); }
  });
  const next = el('button', 'quiet small', t('preference_new'));
  next.type = 'button';
  next.addEventListener('click', () => {
    preferenceDraft = { text: '', confirmed: false, result: null };
    f.preference = preferenceDraft;
    renderPreference(preferenceDraft);
    document.querySelector('[data-field="preference.text"]')?.focus();
  });
  return el('section', 'preference', el('h3', null, t('preference_h')), el('p', 'desc', t('preference_desc')),
    action, draft.sending ? el('p', 'desc', t('loading')) : null, draft.result ? el('p', 'desc', claimReceipt(draft.result)) : null,
    draft.result?.uid ? claimLink(draft.result.uid) : null, draft.result && draft.result.state !== 'applied' ? next : null);
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
  const state = note(t(`key_${r.key.replaceAll('-', '_')}`));
  return [state, r.key_file ? note(t('provider_key_manage')) : null];
}

const LIMIT_FIELDS = ['max_request_tokens', 'daily_tokens', 'usd_per_mtok_in', 'usd_per_mtok_out', 'max_output_tokens'];

function providerText(value) {
  return value === null || value === undefined ? '' : String(value);
}

function providerEdit(name = '', saved = {}) {
  const defaults = { max_request_tokens: null, daily_tokens: null, usd_per_mtok_in: 0,
    usd_per_mtok_out: 0, max_output_tokens: 4000 };
  return { kind: saved.kind || 'openai', name, enabled: saved.enabled ?? true,
    cli: saved.cli || 'claude', base_url: saved.base_url || '', model: saved.model || '',
    timeout_s: providerText(saved.timeout_s ?? 60), subscription: saved.subscription ?? false,
    daily_budget: providerText(saved.daily_budget), savedDailyBudget: saved.daily_budget ?? null,
    limits: Object.fromEntries(LIMIT_FIELDS.map((key) => [key, providerText(saved.limits?.[key] ?? defaults[key])])),
  };
}

function selectorKey(selector) {
  return `${selector.source}:${selector.index}`;
}

// The native editor never serializes key paths, extras, headers, retry options or effective values.
function providerBody(draft) {
  const limits = Object.fromEntries(LIMIT_FIELDS.map((key) => {
    const v = draft.limits[key].trim();
    return [key, v === '' && ['max_request_tokens', 'daily_tokens'].includes(key) ? null : Number(v)];
  }));
  const entry = { kind: draft.kind, name: draft.name, enabled: draft.enabled,
    model: draft.kind === 'cli' ? draft.model.trim() || null : draft.model.trim(),
    timeout_s: Number(draft.timeout_s), limits };
  if (draft.kind === 'cli') entry.cli = draft.cli;
  else Object.assign(entry, { base_url: draft.base_url.trim(), subscription: draft.subscription,
    daily_budget: providerDailyBudget(draft) });
  return entry;
}

// A subscription save preserves its legacy cap; the editable value stays pending while hidden.
function providerDailyBudget(draft) {
  if (draft.subscription) return draft.savedDailyBudget;
  return draft.daily_budget.trim() === '' ? null : Number(draft.daily_budget);
}

function providerWholeInRange(value, [min, max]) {
  return /^\d+$/.test(value) && !(Number(value) < min || Number(value) > max);
}

function providerConnectionField(draft) {
  if (!draft.name.trim() || [...draft.name].length > 64 || /[\x00-\x1f\x7f]/.test(draft.name)) return 'providers.name';
  if (draft.kind === 'openai' && !draft.base_url.trim()) return 'providers.base_url';
  if ((draft.kind === 'openai' && !draft.model.trim()) || [...draft.model].length > 200) return 'providers.model';
  return null;
}

function providerBudgetField(draft) {
  if (!providerWholeInRange(draft.timeout_s, form.ranges.timeout_s)) return 'providers.timeout_s';
  const unchangedDaily = /^\d+$/.test(draft.daily_budget) && Number(draft.daily_budget) === draft.savedDailyBudget;
  if (draft.kind === 'openai' && !draft.subscription && draft.daily_budget.trim()
      && !unchangedDaily
      && !providerWholeInRange(draft.daily_budget, form.ranges.daily_budget)) return 'providers.daily_budget';
  return null;
}

function providerLimitValid(key, value) {
  const optional = ['max_request_tokens', 'daily_tokens'].includes(key);
  if (optional && value === '') return true;
  const number = Number(value);
  const price = key.startsWith('usd_');
  if (value === '' || !Number.isFinite(number) || number < (price ? 0 : 1)) return false;
  return price || (/^\d+$/.test(value) && Number.isSafeInteger(number)
    && (key === 'daily_tokens' || number <= 4294967295));
}

function providerValidation(draft) {
  const field = providerConnectionField(draft) || providerBudgetField(draft);
  if (field) return field;
  for (const key of LIMIT_FIELDS) {
    if (!providerLimitValid(key, draft.limits[key].trim())) return `providers.limits.${key}`;
  }
  return null;
}

// Config-changing entry/key writes replace the server baseline, while unrelated pending global
// and name-group edits stay local. Physical selectors are remapped only by the operation's raw
// index changes, never by the first entry with a matching name.
function preserveSettingsDrafts(next, mine) {
  for (const key of ['worker', 'summary', 'paid_usd_per_month', 'inject', 'capture', 'backup', 'redaction']) next[key] = mine[key];
  next.gemini = mine.gemini === (mine.saved.gemini ?? 'none') ? next.gemini : mine.gemini;
  const groups = new Map(next.chain.map((r) => [r.name, r]));
  next.chain = [...mine.chain.filter((r) => groups.has(r.name)).map((r) => {
    const row = groups.get(r.name);
    row.edit = { ...r.edit };
    groups.delete(r.name);
    return row;
  }), ...groups.values()];
}

function geminiInsertion(mine, action) {
  const materializes = (action.selector?.source === 'gemini' && !['remove', 'move'].includes(action.op))
    || (action.op === 'create' && action.entry.name === 'gemini' && mine.providers.some((p) => p.selector.source === 'gemini'));
  if (!materializes) return null;
  const native = mine.providers.filter((p) => p.selector.source !== 'gemini');
  const firstCli = native.find((p) => p.saved.kind === 'cli');
  return mine.saved.gemini === 'before-subscriptions' && firstCli ? firstCli.selector.index : native.length;
}

function movedProviderIndex(index, from, to) {
  if (index === from) return to;
  if (from < to && index > from && index <= to) return index - 1;
  if (from > to && index >= to && index < from) return index + 1;
  return index;
}

function mappedProviderSelector(selector, hasFiles, action, inserted) {
  const mapped = { ...selector };
  if (mapped.source === 'gemini') {
    if (inserted !== null) Object.assign(mapped, { source: 'file', index: inserted });
    return mapped;
  }
  if (hasFiles) mapped.source = 'file';
  if (inserted !== null) {
    if (mapped.index >= inserted) mapped.index += 1;
    return mapped;
  }
  if (!action.selector || action.selector.source === 'gemini') return mapped;
  const at = action.selector.index;
  if (action.op === 'remove' && mapped.index > at) mapped.index -= 1;
  if (action.op === 'move') mapped.index = movedProviderIndex(mapped.index, at, action.to);
  return mapped;
}

function preserveProviderDrafts(next, mine, action) {
  const inserted = geminiInsertion(mine, action);
  const hasFiles = next.providers.some((p) => p.selector.source === 'file');
  for (const old of mine.providers) {
    const selected = action.selector && selectorKey(old.selector) === selectorKey(action.selector);
    if (selected && action.op === 'remove') continue;
    const mapped = mappedProviderSelector(old.selector, hasFiles, action, inserted);
    const row = next.providers.find((p) => selectorKey(p.selector) === selectorKey(mapped));
    if (selected && action.op === 'edit') {
      // The hidden cap was not sent. Keep that pending edit while the response supplies the
      // current saved baseline and every other saved field, including a rename.
      if (row && action.entry.kind === 'openai' && action.entry.subscription) row.edit.daily_budget = old.edit.daily_budget;
      continue;
    }
    if (!row || row.name !== old.name) continue;
    row.edit = old.edit;
    row.edit.enabled = row.saved.enabled;
    row.editing = old.editing;
    row.advanced = old.advanced;
  }
}

function preserveCreatedSubscriptionDraft(next, mine, action) {
  if (action.op !== 'create' || action.entry.kind !== 'openai' || !action.entry.subscription || !mine.newProvider) return;
  // Creation appends one native entry, even if it also materializes automatic Gemini.
  const at = next.providers.filter((p) => p.selector.source !== 'gemini').length - 1;
  const created = next.providers.find((p) => p.selector.source === 'file' && p.selector.index === at);
  if (created?.name === action.entry.name) created.edit.daily_budget = mine.newProvider.daily_budget;
}

function mergeProviderSettings(answer, mine, action) {
  const next = formOf(answer);
  if (!next) return next;
  if (action.op !== 'settings') preserveSettingsDrafts(next, mine);
  next.privacy = mine.privacy;
  next.preference = mine.preference;
  next.newProvider = action.op === 'create' ? null : mine.newProvider;
  preserveProviderDrafts(next, mine, action);
  preserveCreatedSubscriptionDraft(next, mine, action);
  return next;
}

function providerReason(code, context = 'test') {
  const codes = {
    bad_endpoint: 'provider_bad_endpoint', provider_kind: 'provider_kind_error', bad_model: 'provider_bad_model',
    unsupported_provider: 'provider_test_unavailable', unsupported_cli: 'provider_test_unavailable', unavailable: 'provider_test_unavailable',
    cli_probe_unbounded: 'provider_test_cli_unbounded',
    provider_busy: 'provider_test_busy', ledger_invalid: 'provider_test_ledger_invalid',
    budget: 'provider_test_budget', budget_exceeded: 'provider_test_budget', monthly_cap: 'provider_test_budget',
    daily_cap: 'provider_test_budget', daily_budget: 'provider_test_budget', token_cap: 'provider_test_budget',
    cooldown: 'provider_test_cooldown', rate_limit: 'provider_test_cooldown', owner_hold: 'provider_test_hold',
    off: 'provider_test_off', disabled: 'provider_test_off', missing_key: 'provider_test_missing_key',
    gate: 'provider_test_gate', egress: 'provider_test_gate', unsafe_headers: 'provider_test_gate', auth: 'provider_test_auth',
    unauthorized_provider: 'provider_test_auth', timeout: 'provider_test_timeout',
    unsupported: 'provider_key_unsupported', managed_storage: 'provider_storage_failed',
    no_safe_storage: 'provider_storage_failed', protected: 'provider_storage_failed',
    no_dir: 'provider_storage_failed', shared_folder: 'provider_storage_failed', not_private: 'provider_storage_failed',
    not_absolute: 'provider_storage_failed', not_a_key_file: 'provider_storage_failed',
    not_a_file: 'provider_storage_failed', not_utf8: 'provider_storage_failed', ambiguous: 'provider_storage_failed',
    no_key_file: 'provider_key_not_applicable',
    http: 'provider_test_http', invalid: 'provider_test_invalid', isolation: 'provider_test_isolation',
    allowance: 'provider_test_allowance', confirmation: 'provider_test_confirmation',
    too_big: context === 'key' ? 'provider_storage_failed' : 'provider_test_size',
  };
  let fallback = context === 'test' ? 'provider_test_reason' : 'provider_operation_refused';
  if (Object.hasOwn(TEXT, code)) fallback = code;
  return t(codes[code] || fallback);
}

const PROVIDER_HTTP_ERRORS = { 400: 'bad_request', 401: 'unauthorized', 403: 'forbidden', 413: 'too_large' };

async function providerRequest(path, body) {
  const res = await fetch(`/api/providers${path}`, {
    method: 'POST', headers: { 'X-Oboete-Token': token, 'Content-Type': 'application/json' },
    body, referrerPolicy: 'same-origin', credentials: 'omit',
  });
  const answer = (res.headers.get('content-type') || '').startsWith('application/json') ? await res.json() : {};
  return { res, answer, current: res.status === 409 ? await api('settings') : null };
}

function currentSettings(mine) {
  return view === 'settings' && form === mine;
}

function reloadProviderSettings(current) {
  form = formOf(current);
  drawSettings();
  setStatus(t('stale'), true, lang);
}

function providerSuccessKey(action, path, answer) {
  if (path === '/key') return answer.key_saved?.durable ? 'key_saved' : 'key_not_durable';
  if (action.op === 'remove') return 'provider_removed';
  if (action.op === 'move') return 'provider_moved';
  return 'provider_saved';
}

function providerOperationFailure(action, path, res, answer, fields, scope) {
  fields.inert = false;
  if (answer.field) markInvalid(answer.field, scope || fields);
  const known = answer.code || PROVIDER_HTTP_ERRORS[res.status];
  let message = t('provider_operation_failed', { status: res.status });
  if (answer.field === 'providers.name') message = t('provider_bad_name');
  else if (known) message = providerReason(known, path === '/key' ? 'key' : 'operation');
  setStatus(message, true, lang);
  if (action.op === 'enabled') drawSettings();
}

async function providerOperation(action, button, body = null, path = '') {
  const mine = form;
  const fields = button.closest('.settings');
  const scope = button.closest('.provider-entry');
  button.disabled = true;
  fields.inert = true;
  try {
    const { res, answer, current } = await providerRequest(path, body || JSON.stringify({ version: mine.version, action }));
    if (!currentSettings(mine)) return;
    if (current) return reloadProviderSettings(current);
    if (!res.ok) return providerOperationFailure(action, path, res, answer, fields, scope);
    form = mergeProviderSettings(answer, mine, action);
    drawSettings();
    setStatus(t(providerSuccessKey(action, path, answer)), path === '/key' && !answer.key_saved?.durable, lang);
  } catch {
    if (currentSettings(mine)) {
      if (action.op === 'enabled') drawSettings();
      setStatus(t('network_failed'), true, lang);
    }
  } finally {
    button.disabled = false;
    fields.inert = false;
  }
}

// Secrets belong only to the live field and the single outgoing body. They are not copied into
// form state; redraws clear them and the selected field is emptied before fetch is called.
function providerKeyField(provider) {
  const i = el('input');
  i.type = 'text';
  i.autocomplete = 'off';
  i.spellcheck = false;
  i.autocapitalize = 'off';
  i.placeholder = t('key_placeholder');
  i.dataset.field = 'providers.key';
  i.setAttribute('aria-label', t('key_label', { name: provider.name }));
  const save = el('button', 'small', t('key_save'));
  save.type = 'button';
  save.disabled = true;
  i.addEventListener('input', () => { save.disabled = !i.value; });
  i.addEventListener('keydown', (event) => {
    if (event.key === 'Enter') { event.preventDefault(); if (!save.disabled) save.click(); }
  });
  save.addEventListener('click', () => {
    const body = JSON.stringify({ version: form.version, selector: provider.selector, key: i.value });
    i.value = '';
    void providerOperation({ op: 'key', selector: provider.selector }, save, body, '/key');
  });
  return el('div', 'key-input', i, save);
}

function usdValue(value) {
  if (value === null || value === undefined) return t('provider_test_unknown_cost');
  const precision = Math.abs(value) < 1
    ? { maximumSignificantDigits: 6 } : { maximumFractionDigits: 6 };
  return new Intl.NumberFormat(lang, { style: 'currency', currency: 'USD', ...precision }).format(value);
}

function providerPreviewMatches(preview, provider, version) {
  return preview.version === version && selectorKey(preview.selector) === selectorKey(provider.selector);
}

async function previewProvider(provider, button, run = false) {
  const mine = form;
  const fields = button.closest('.settings');
  const preview = provider.preview;
  if (run && (!preview?.ready || !providerPreviewMatches(preview, provider, mine.version))) return;
  const body = JSON.stringify({ version: mine.version, selector: provider.selector, ...(run ? { confirmed: true } : {}) });
  fields.inert = true;
  button.disabled = true;
  try {
    const { res, answer, current } = await providerRequest(run ? '/test' : '/test/preview', body);
    if (!currentSettings(mine)) return;
    if (current) return reloadProviderSettings(current);
    if (!res.ok) {
      const known = answer.code || PROVIDER_HTTP_ERRORS[res.status] || 'unavailable';
      setStatus(providerReason(known), true, lang);
      return;
    }
    if (run) { provider.result = answer; provider.preview = null; }
    else {
      if (!answer.selector || !providerPreviewMatches(answer, provider, mine.version)) {
        setStatus(t('stale'), true, lang);
        return;
      }
      provider.preview = answer;
      provider.result = null;
    }
    drawSettings();
    setStatus('');
  } catch {
    if (currentSettings(mine)) setStatus(t('network_failed'), true, lang);
  } finally {
    fields.inert = false;
    button.disabled = false;
  }
}

function providerTestPreview(provider, preview) {
  const run = el('button', 'small', t('provider_test_run'));
  run.type = 'button';
  run.disabled = !preview.ready;
  run.addEventListener('click', () => void previewProvider(provider, run, true));
  return el('div', 'test-preview',
    el('p', null, t('provider_test_destination', { destination: preview.destination })),
    el('p', null, t('provider_test_model', { model: preview.model ?? t('provider_default_model') })),
    el('h4', null, t('provider_test_fixture')), el('pre', 'document-text', preview.fixture),
    note(t('provider_test_tokens', { tokens: preview.estimated_input_tokens, output: preview.max_output_tokens ?? t('provider_test_unknown_cost') })),
    el('p', null, t(preview.possible_charge ? 'provider_test_charge' : 'provider_test_no_charge',
      { usd: usdValue(preview.estimated_usd), cap: usdValue(preview.monthly_cap_usd) })),
    !preview.ready ? el('p', 'test-result error', t('provider_test_blocked', { reason: providerReason(preview.code) })) : null,
    run);
}

function providerTestResult(result) {
  let status = 'failed';
  if (result.status === 'ok') status = 'ok';
  else if (result.status === 'blocked') status = 'blocked';
  return el('div', 'test-result',
    el('p', result.status === 'ok' ? null : 'error', t(`provider_test_${status}`,
      { ms: result.latency_ms ?? 0, reason: providerReason(result.code) })),
    result.http_status === null || result.http_status === undefined ? null : note(t('provider_test_http_status', { status: result.http_status })),
    result.usd === null || result.usd === undefined ? null : note(t('provider_test_billed', { usd: usdValue(result.usd) })),
    result.retry_at ? note(t('provider_test_retry', { time: new Date(result.retry_at).toLocaleString(lang) })) : null);
}

function providerTest(provider) {
  const button = el('button', 'quiet small', t('provider_preview'));
  button.type = 'button';
  button.addEventListener('click', () => void previewProvider(provider, button));
  const nodes = [note(t('provider_preview_desc')), button];
  if (provider.preview) nodes.push(providerTestPreview(provider, provider.preview));
  if (provider.result) nodes.push(providerTestResult(provider.result));
  return el('div', 'provider-test', ...nodes);
}

function providerLimitGrid(draft, unsupported) {
  const grid = el('div', 'grid');
  const outputField = el('label', 'field');
  const syncOutput = () => {
    // Normal dispatch enforces this maximum only for priced HTTP. Keep other entries' saved
    // estimates in the typed draft without offering an ineffective generation-limit control.
    const paidHttp = draft.kind === 'openai'
      && (Number(draft.limits.usd_per_mtok_in) > 0 || Number(draft.limits.usd_per_mtok_out) > 0);
    if (paidHttp) {
      if (outputField.parentElement !== grid) grid.append(outputField);
    } else outputField.remove();
  };
  for (const key of LIMIT_FIELDS) {
    const price = key.startsWith('usd_');
    const control = input('number', draft.limits[key], '', `providers.limits.${key}`, (value) => {
      draft.limits[key] = value;
      if (price) syncOutput();
    });
    control.min = price ? 0 : 1;
    const max = key === 'daily_tokens' ? Number.MAX_SAFE_INTEGER : 4294967295;
    if (!price) control.max = max;
    if (price) { control.step = 'any'; control.inputMode = 'decimal'; }
    control.disabled = Boolean(unsupported) || (draft.kind === 'cli' && price);
    const label = key === 'max_output_tokens' ? outputField : el('label', 'field');
    label.append(el('span', null, t(`provider_${key}`)), control);
    if (key !== 'max_output_tokens') grid.append(label);
  }
  syncOutput();
  return grid;
}

function providerEditor(draft, provider, card) {
  const unsupported = provider?.saved.kind === 'cli' && !['claude', 'codex'].includes(provider.saved.cli);
  const field = (key, label, type = 'text', description = null) => {
    const control = input(type, draft[key], '', `providers.${key}`, (value) => { draft[key] = value; });
    control.disabled = Boolean(unsupported);
    if (key === 'timeout_s') [control.min, control.max] = form.ranges.timeout_s;
    if (key === 'name') control.maxLength = 64;
    if (key === 'model') control.maxLength = 200;
    return el('label', 'field', el('span', null, t(label)), control, description ? note(t(description)) : null);
  };
  const main = el('div', 'grid', field('name', 'provider_name'),
    draft.kind === 'openai' ? field('base_url', 'provider_endpoint', 'text', 'provider_endpoint_desc') : null,
    field('model', 'col_model'), field('timeout_s', 'col_timeout', 'number'));
  const limits = el('details', 'provider-limits', el('summary', null, t('provider_advanced')));
  limits.open = provider?.advanced ?? false;
  if (provider) limits.addEventListener('toggle', () => { provider.advanced = limits.open; });
  const grid = providerLimitGrid(draft, unsupported);
  const daily = field('daily_budget', 'col_budget', 'number');
  [daily.querySelector('input').min, daily.querySelector('input').max] = form.ranges.daily_budget;
  daily.hidden = draft.kind === 'cli' || draft.subscription;
  if (draft.kind === 'openai') grid.append(daily);
  limits.append(note(t('provider_limits_desc')), grid);
  const subscribe = checkbox(draft.subscription, (value) => {
    draft.subscription = value;
    daily.hidden = value;
  });
  const save = el('button', 'small', t(provider ? 'provider_save' : 'provider_create'));
  save.type = 'button';
  save.disabled = Boolean(unsupported);
  save.addEventListener('click', () => {
    const field = providerValidation(draft);
    if (field) {
      markInvalid(field, card);
      const message = { 'providers.base_url': 'provider_bad_endpoint', 'providers.name': 'provider_bad_name',
        'providers.model': 'provider_bad_model' }[field] || 'range';
      setStatus(t(message), true, lang);
      return;
    }
    void providerOperation({ op: provider ? 'edit' : 'create', ...(provider ? { selector: provider.selector } : {}), entry: providerBody(draft) }, save);
  });
  const cancel = el('button', 'quiet small', t('provider_cancel'));
  cancel.type = 'button';
  cancel.addEventListener('click', () => {
    if (provider) { provider.edit = providerEdit(provider.name, provider.saved); provider.editing = false; }
    else form.newProvider = null;
    drawSettings();
  });
  return el('div', 'provider-editor', main,
    !provider ? el('label', 'check', checkbox(draft.enabled, (value) => { draft.enabled = value; }), t('provider_enabled')) : null,
    draft.kind === 'openai' ? el('label', 'check', subscribe, t('provider_subscription')) : note(t('provider_cli_desc')),
    draft.kind === 'openai' ? note(t('provider_subscription_desc')) : null,
    limits, el('div', 'actions', save, cancel));
}

function providerKeyPanel(provider) {
  if (provider.saved.kind !== 'openai') return null;
  return el('div', 'provider-key', note(t('provider_key_desc')),
    form.keyInput ? providerKeyField(provider) : note(t('provider_key_unsupported')));
}

function providerEntry(provider, nativeCount) {
  const saved = provider.saved;
  const unsupported = saved.kind === 'cli' && !['claude', 'codex'].includes(saved.cli);
  const source = t(`provider_source_${provider.selector.source}`);
  const position = t('provider_position', { source, index: provider.selector.index + 1 });
  const card = el('article', 'provider-entry');
  card.dataset.selector = selectorKey(provider.selector);
  const enabled = checkbox(saved.enabled, (value) => void providerOperation({ op: 'enabled', selector: provider.selector, enabled: value }, enabled));
  enabled.disabled = !saved.enabled && (unsupported || saved.endpoint_supported === false);
  enabled.setAttribute('aria-label', `${t('provider_enabled')}: ${provider.name} (${position})`);
  const arrows = [-1, 1].map((direction) => {
    const button = el('button', 'quiet small', direction < 0 ? '↑' : '↓');
    button.type = 'button';
    button.title = t(direction < 0 ? 'up' : 'down');
    button.setAttribute('aria-label', `${button.title}: ${provider.name} (${position})`);
    const to = provider.selector.index + direction;
    button.disabled = provider.selector.source === 'gemini' || to < 0 || to >= nativeCount;
    button.addEventListener('click', () => void providerOperation({ op: 'move', selector: provider.selector, to }, button));
    return button;
  });
  const remove = el('button', 'quiet small', t('provider_remove'));
  remove.type = 'button';
  remove.addEventListener('click', () => {
    if (window.confirm(t('provider_remove_confirm', { name: provider.name, source, index: provider.selector.index + 1 }))) {
      void providerOperation({ op: 'remove', selector: provider.selector }, remove);
    }
  });
  const edit = el('details', 'provider-edit', el('summary', null, t('provider_edit')));
  edit.open = provider.editing;
  edit.addEventListener('toggle', () => { provider.editing = edit.open; });
  edit.append(providerEditor(provider.edit, provider, card));
  const model = (value) => value ?? t('provider_default_model');
  const key = saved.key === 'none' && saved.kind === 'openai' ? 'provider_key_none' : `key_${saved.key.replaceAll('-', '_')}`;
  card.append(...present([el('div', 'provider-header', el('h4', null, provider.name), note(position),
      el('label', 'check', enabled, t('provider_enabled')), el('div', 'move', ...arrows), remove),
    note(t('provider_saved_native', { on: t(saved.enabled ? 'value_on' : 'value_off'), model: model(saved.model), timeout: saved.timeout_s })),
    note(t('provider_effective', { on: t(provider.effective.on ? 'value_on' : 'value_off'), order: provider.effective.order + 1,
      model: model(provider.effective.model), timeout: provider.effective.timeout_s })),
    saved.kind === 'openai' ? note(saved.base_url || t('provider_unsupported_endpoint')) : note(saved.cli),
    unsupported ? note(t('provider_unsupported_cli')) : null,
    note(t(key)),
    providerKeyPanel(provider),
    unsupported ? null : edit,
    providerTest(provider)]));
  return card;
}

function providersSection() {
  const section = el('section', 'provider-entries', el('h3', null, t('provider_entries_h')),
    el('p', 'desc', t('provider_entries_desc')), note(t('provider_order_desc')));
  const nativeCount = form.providers.filter((p) => p.selector.source !== 'gemini').length;
  section.append(...form.providers.map((provider) => providerEntry(provider, nativeCount)));
  if (form.newProvider) {
    const draft = form.newProvider;
    const card = el('article', 'provider-entry new-provider', el('h4', null, t('provider_add')));
    const options = { openai: 'provider_http', claude: 'provider_cli_claude', codex: 'provider_cli_codex' };
    const type = el('select', null, ...Object.entries(options).map(([value, key]) => {
      const option = el('option', null, t(key)); option.value = value; return option;
    }));
    type.value = draft.kind === 'openai' ? 'openai' : draft.cli;
    type.addEventListener('change', () => {
      draft.kind = type.value === 'openai' ? 'openai' : 'cli';
      if (draft.kind === 'cli') {
        draft.cli = type.value;
        draft.limits.usd_per_mtok_in = '0';
        draft.limits.usd_per_mtok_out = '0';
      }
      drawSettings();
    });
    card.append(el('label', 'field', el('span', null, t('provider_type')), type), providerEditor(draft, null, card));
    section.append(card);
  } else {
    const add = el('button', 'quiet small', t('provider_add'));
    add.type = 'button';
    add.addEventListener('click', () => { form.newProvider = providerEdit(); drawSettings(); });
    section.append(add);
  }
  return section;
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
    el('td', null, r.subscription ? note(t('provider_subscription')) : budget, !r.subscription && r.budget_from_key && !r.edit.daily_budget ? note(t('from_key', { n: r.effective_daily_budget })) : null),
    el('td', null, timeout));
  return tr;
}

// The save's body, or the field a value is wrong in.
function saveBody() {
  const redaction = redactionBody();
  if (!redaction.value) return { field: redaction.field };
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
        session_start_note: form.inject.session_start_note,
        per_prompt: form.inject.per_prompt,
        correction: form.inject.correction,
        ...sizes,
      },
      capture: { store_prompts: form.capture.store_prompts, tool_output: form.capture.tool_output },
      ...(!form.backup.reset && form.backup.edit === (form.backup.dir ?? '') ? {} : { backup: { dir: form.backup.reset ? null : form.backup.edit || null } }),
      redaction: redaction.value,
      chain,
    },
  };
}

function markInvalid(field, root = document) {
  const controls = [...root.querySelectorAll('input, select, textarea')];
  const i = controls.find((x) => x.dataset.field === field) || controls.find((x) => x.dataset.field?.startsWith(`${field}.`));
  if (!i) return;
  i.classList.add('invalid');
  i.setAttribute('aria-invalid', 'true');
  for (let details = i.closest('details'); details; details = details.parentElement?.closest('details')) details.open = true;
  i.focus();
}

function applySavedSettings(answer, mine, current) {
  form = current ? formOf(current) : mergeProviderSettings(answer, mine, { op: 'settings' });
  if (form) form.privacy = privacyUnavailable();
  drawSettings();
  setStatus(t(current ? 'stale' : 'saved'), Boolean(current), lang);
  if (form) void refreshPrivacy(form);
}

async function saveSettings(button) {
  const { body, field } = saveBody();
  if (!body) {
    markInvalid(field);
    setStatus(t(field.startsWith('redaction.') ? 'redaction_invalid' : 'range'), true, lang);
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
      applySavedSettings(answer, mine, current);
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
    panel.append(el('p', 'text pending', t('file_error')), agentInventorySection());
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
  let spending;
  if (f.usd_this_month === null) spending = t('spend_unavailable');
  else if (f.usd_this_month === 0) spending = t('no_spend');
  else spending = t('month_spend', { usd: usdValue(f.usd_this_month) });
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
  const terminalNote = checkbox(f.inject.session_start_note, (v) => { f.inject.session_start_note = v; });
  terminalNote.dataset.field = 'inject.session_start_note';
  const backupReset = el('button', 'quiet small', t('backup_reset'));
  backupReset.type = 'button';
  backupReset.addEventListener('click', () => { f.backup.edit = ''; f.backup.reset = true; drawSettings(); });
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
  const save = el('button', 'save', t('settings_save'));
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
      el('label', 'check', terminalNote, t('session_note_on')), note(t('session_note_desc')),
      flag('per_prompt', 'per_prompt_on'), size('per_prompt_chars', 'per_prompt_chars'),
      flag('correction', 'correction_on'), size('correction_chars', 'correction_chars')),
    el('section', null,
      el('h3', null, t('capture_h')),
      el('p', 'desc', t('capture_desc')),
      el('label', 'check', checkbox(f.capture.store_prompts, (v) => { f.capture.store_prompts = v; }), t('store_prompts')),
      el('label', 'field', el('span', null, t('tool_output')), tool)),
    el('section', null,
      el('h3', null, t('backup_h')),
      el('label', 'field', el('span', null, t('backup_dir')),
        input('text', f.backup.edit, t('backup_default'), 'backup.dir', (v) => { f.backup.edit = v; f.backup.reset = false; }),
        saved(f.backup.dir === '' ? t('backup_home') : f.backup.dir ?? t('backup_default'))),
      backupReset,
      el('p', 'desc', t('backup_desc'))),
    redactionSection(f),
    privacySection(f),
    providersSection(),
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
  formEl.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && event.target.closest('.provider-entry') && event.target.tagName !== 'BUTTON' && event.target.tagName !== 'SUMMARY') event.preventDefault();
  });
  formEl.addEventListener('submit', (e) => {
    e.preventDefault();
    void saveSettings(save);
  });
  panel.append(el('p', 'lead', t('lead')), formEl, preferenceSection(f), agentInventorySection(), maintenanceSection(f));
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

async function pollMaintenance() {
  if(view==='settings' && form?.maintenance)await refreshMaintenance(form.maintenance);
}
async function poll() {
  if (polling || document.visibilityState !== 'visible') return;
  polling = true;
  try {
    await pollMaintenance();
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
