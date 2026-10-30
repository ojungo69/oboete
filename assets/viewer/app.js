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

// `textLang` marks the language of a translated status line (#274). `hold` keeps the line, for
// that many ms, through the redraws that clear it: a forget's result outlives the live view's next
// redraw, which the forget itself brings (parity row 3).
let statusHeld = null;
// `action`, a button after the text (a removal's Undo), is held with it.
function setStatus(text, isError = false, textLang = null, hold = 0, action = null) {
  if (!text && statusHeld && Date.now() < statusHeld.until) ({ text, isError, textLang, action } = statusHeld);
  else statusHeld = text && hold ? { text, isError, textLang, action, until: Date.now() + hold } : null;
  $('status').textContent = text;
  if (text && action) $('status').append(' ', action);
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
  // docs/retention.md: a quote whose record's text expired is not shown, nor its record offered.
  const quotes = c.quotes.map((q) => (q.expired
    ? el('li', null, word('p', 'text pending', 'quote_expired'), el('span', 'meta', q.key))
    : el('li', null, el('p', 'text', q.text), fullText(q.key, q.key))));
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
  const fields = el('div', null, correction, el('div', 'actions', mute, forgetControl('claim', c.uid)));
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
const VIEWS = ['my_work', 'my_rules', 'timeline', 'sessions', 'records', 'context', 'stats', 'settings'];
// My Work opens first once (decision 55), whatever view was stored before it existed; from then
// on the page opens on the view last used.
const firstMyWork = recall('oboete-tab', '') !== 'my_work';
if (firstMyWork) remember('oboete-tab', 'my_work');
const storedView = recall('oboete-view', 'my_work');
let view = !firstMyWork && VIEWS.includes(storedView) ? storedView : 'my_work';
let currentRepo = '';
let reposLoaded = false;

function setView(name) {
  view = name;
  remember('oboete-view', name);
  document.querySelector('main').classList.toggle('timeline-view', name === 'timeline');
  document.querySelector('main').classList.toggle('my-work-view', ['my_work', 'my_rules'].includes(name));
  // My Work has its own search and filters, and its tab opens on the requests, not on one; My
  // Goals reads no repository.
  $('controls').hidden = ['settings', 'my_work', 'my_rules'].includes(name);
  if (name === 'my_work') Object.assign(mw, { request: null, work: null, back: null });
  // The tab opens on the list, not on a session (docs/page.md S1).
  if (name === 'sessions') Object.assign(ses, { open: null, back: null });
  for (const control of document.querySelectorAll('.record-controls')) control.hidden = name === 'timeline' || name === 'sessions';
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

// What an entry's footer offers: forget for a prompt or a note, Remove for a card or a summary.
function feedControl(item) {
  if (['prompt', 'note'].includes(item.kind)) return forgetControl(item.kind, item.id);
  if (['card', 'summary'].includes(item.kind)) return hideControl(item.kind, item.id);
  return null;
}

function feedFooter(item) {
  const time = localTime(item.ts, lang);
  time.dataset.feedTimestamp = String(item.ts);
  return el('footer', 'feed-card-footer', el('code', null, item.id), time, feedControl(item));
}

// A note an agent saved has no subtitle of its own: it says whose it is, as claude-mem's manual
// memory says "Manual memory" (row N3).
function feedSubtitle(item) {
  if (item.subtitle) return el('p', 'text feed-subtitle', item.subtitle);
  return item.kind === 'note' ? word('p', 'text feed-subtitle', 'feed_note_subtitle') : null;
}

function feedCard(item) {
  const title = item.title ? el('h3', 'feed-title', item.title) : word('h3', 'feed-title', 'untitled');
  const subtitle = feedSubtitle(item);
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

// claude-mem's PromptCard (#4757): a prompt past 8 lines or 600 characters is clamped to 8 lines
// behind Show more, so one long prompt does not push the timeline off the screen (parity row X9).
const CLAMP_LINES = 8;
const CLAMP_MIN_CHARS = 600;

function promptText(text) {
  const body = el('p', 'text prompt-text', text);
  if (text.split('\n').length <= CLAMP_LINES && text.length <= CLAMP_MIN_CHARS) return [body];
  body.classList.add('clamped');
  const more = word('button', 'show-more', 'show_more');
  more.type = 'button';
  more.setAttribute('aria-expanded', 'false');
  let open = false;
  more.addEventListener('click', () => {
    open = !open;
    body.classList.toggle('clamped', !open);
    more.dataset.i18n = open ? 'show_less' : 'show_more';
    more.textContent = t(more.dataset.i18n);
    more.setAttribute('aria-expanded', String(open));
  });
  return [body, more];
}

function feedEntry(item) {
  let card;
  // A note an agent saved is shown as claude-mem shows a manual memory: an observation (row N3).
  if (item.kind === 'card' || item.kind === 'note') card = feedCard(item);
  else if (item.kind === 'summary') {
    const sections = ['investigated', 'learned', 'completed', 'next_steps', 'notes'];
    card = el('article', 'feed-card summary-card', feedHeader(item),
      item.fields.request ? el('h3', 'feed-title', item.fields.request) : null,
      ...sections.filter((field) => item.fields[field]).map((field) => el('section', 'summary-section',
        word('h4', 'summary-label', field), el('p', 'text', item.fields[field]))),
      feedFooter(item));
  } else card = el('article', 'feed-card prompt-card', feedHeader(item),
    ...promptText(item.text), feedFooter(item));
  return el('li', 'feed-entry', card);
}

function activeFeed(state) {
  return Boolean(state) && feed === state && state.generation === generation
    && view === (state.session ? 'sessions' : 'timeline');
}

function updateFeedState(state) {
  if (!activeFeed(state)) return;
  const message = state.loading ? t('feed_loading')
    : state.error ? t('feed_error', { reason: failureMessage(state.error) })
      : !state.keys.size ? t('feed_empty') : state.next === null ? t('feed_exhausted') : '';
  state.notice.textContent = message;
  state.notice.classList.toggle('error', Boolean(state.error) && !state.loading);
  state.notice.classList.toggle('loading', state.loading);
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
      // A session's page reads that session's items alone (docs/page.md S4).
      const params = state.session ? { session: state.session, limit: FEED_LIMIT } : { ...scope(state.repo), limit: FEED_LIMIT };
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
        const entry = feedEntry(item);
        entry.dataset.category = feedCategory(item);
        entries.push(entry);
        unseen.add(key);
      }
      if (mode === 'live' && !joined) {
        entries.length = 0;
        unseen.clear();
      }
      if (mode === 'live') state.list.prepend(...entries);
      else {
        state.list.append(...entries);
        state.next = page.next;
        state.loaded = true;
      }
      for (const key of unseen) state.keys.add(key);
      state.drawn?.();
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
  const state = { generation: mine, repo, list: $('list'), keys: new Set(), next: null, loaded: false, loading: false,
    error: null, request: null, retryMode: 'initial', notice, more, retry, sentinel, observer: null };
  feed = state;
  more.addEventListener('click', () => void readFeed(state, 'more'));
  retry.addEventListener('click', () => void readFeed(state, state.retryMode));
  draw(t('timeline'), [], [el('div', 'feed-pagination', notice, more, retry, sentinel)]);
  setStatus('');
  observeEnd(state, () => void readFeed(state, 'more'));
  return readFeed(state, 'initial');
}

// New pages load when the end of a list comes into view, and by its More button (P9).
function observeEnd(state, more) {
  if (typeof IntersectionObserver === 'undefined') return;
  state.observer = new IntersectionObserver((entries) => {
    if (entries.some((entry) => entry.isIntersecting) && !state.loading && !state.error) more();
  }, { threshold: 0.1 });
  state.observer.observe(state.sentinel);
}

// --- Sessions: claude-mem's Sessions tab (docs/page.md slice 2) ----------------------------

// claude-mem's `CATEGORY_ORDER`: the kinds a session's page filters by.
const CATEGORIES = ['prompt', 'summary', 'discovery', 'decision', 'bugfix', 'feature', 'refactor', 'change',
  'security_alert', 'security_note', 'other'];
const CARD_TYPES = CATEGORIES.slice(2, -1);
// The list as read (`items`, `next`, of `repo`), the session open, the card it was opened from
// and the kinds its page shows.
const ses = { items: [], next: null, repo: null, open: null, back: null, kinds: new Set(), stale: false };
let sessionsList = null;

function feedCategory(item) {
  if (item.kind !== 'card') return item.kind;
  return CARD_TYPES.includes(item.type) ? item.type : 'other';
}

function sessionCard(s) {
  const id = el('code', 'session-id', s.session.slice(0, 8));
  id.title = s.session;
  const button = el('button', 'session-card',
    el('span', 'session-title', s.title || s.repo_name || s.repo || ''),
    el('span', 'session-meta', id,
      word('span', 'session-count', s.items === 1 ? 'session_item' : 'session_items', { n: s.items }),
      s.repo_name ? el('span', 'feed-repo', s.repo_name) : null,
      el('span', 'badge feed-agent', s.agent), localTime(s.started, lang)));
  button.type = 'button';
  button.dataset.session = s.key;
  button.addEventListener('click', () => sessionOpen(s));
  return el('li', 'session-entry', button);
}

function activeSessions(state) {
  return Boolean(state) && sessionsList === state && state.generation === generation && view === 'sessions' && !ses.open;
}

function updateSessionsState(state) {
  if (!activeSessions(state)) return;
  state.notice.textContent = state.loading ? t('feed_loading')
    : state.error ? t('feed_error', { reason: failureMessage(state.error) })
      : !ses.items.length ? t('sessions_empty') : '';
  state.notice.classList.toggle('error', Boolean(state.error) && !state.loading);
  state.more.hidden = ses.next === null;
  state.more.disabled = state.loading;
  state.retry.hidden = !state.error || state.loading;
  state.sentinel.hidden = !ses.items.length || ses.next === null || state.loading || Boolean(state.error);
}

// The cards again, the one in focus keeping it.
function drawSessions(state) {
  const focused = document.activeElement?.dataset?.session;
  state.list.replaceChildren(...ses.items.map(sessionCard));
  const again = focused && [...state.list.querySelectorAll('button')].find((b) => b.dataset.session === focused);
  if (again) again.focus({ preventScroll: true });
}

async function readSessions(state, mode) {
  if (!activeSessions(state) || state.loading) return false;
  if (mode === 'live' && !ses.items.length) mode = 'initial';
  if (mode === 'more' && ses.next === null) return false;
  state.loading = true;
  state.error = null;
  state.retryMode = mode;
  updateSessionsState(state);
  try {
    const params = { ...scope(state.repo), limit: FEED_LIMIT };
    if (mode === 'more') params.before = ses.next;
    const page = await api('sessions', params);
    if (!activeSessions(state)) return false;
    if (mode === 'initial') {
      ses.items = page.sessions;
      ses.next = page.next;
    } else if (mode === 'more') {
      const known = new Set(ses.items.map((s) => s.key));
      ses.items.push(...page.sessions.filter((s) => !known.has(s.key)));
      ses.next = page.next;
    } else {
      // Live (S6): the first page as it is now takes the place of the sessions it covers: those
      // begun since on top, a changed one in its new place, one no longer listed gone. The pages
      // read after it stay; a first page that does not reach the list shown starts it again.
      const live = page.sessions;
      const keys = new Set(live.map((s) => s.key));
      const last = live[live.length - 1];
      if (page.next === null) {
        ses.items = live;
        ses.next = null;
      } else if (ses.items.some((s) => keys.has(s.key))) {
        ses.items = [...live, ...ses.items.filter((s) => !keys.has(s.key) && s.started <= last.started)];
      } else {
        ses.items = live;
        ses.next = page.next;
      }
    }
    drawSessions(state);
    setStatus('');
    return true;
  } catch (error) {
    if (activeSessions(state)) state.error = error;
    return false;
  } finally {
    state.loading = false;
    updateSessionsState(state);
  }
}

async function showSessions(repo, mine) {
  if (ses.open) return showSessionPage(mine);
  const notice = el('p', 'feed-state');
  notice.setAttribute('role', 'status');
  notice.setAttribute('aria-live', 'polite');
  const more = word('button', 'quiet more', 'more');
  more.type = 'button';
  const retry = word('button', 'quiet', 'retry');
  retry.type = 'button';
  const sentinel = el('div', 'feed-sentinel');
  sentinel.setAttribute('aria-hidden', 'true');
  const state = { generation: mine, repo, list: $('list'), loading: false, error: null, retryMode: 'initial',
    notice, more, retry, sentinel, observer: null };
  sessionsList = state;
  more.addEventListener('click', () => void readSessions(state, 'more'));
  retry.addEventListener('click', () => void readSessions(state, state.retryMode));
  draw(t('sessions'), [], [el('div', 'feed-pagination', notice, more, retry, sentinel)]);
  setStatus('');
  observeEnd(state, () => void readSessions(state, 'more'));
  // Back from a session: the list as it was read, not read again (S4), unless the store changed
  // while the session's page was open (S6).
  if (ses.back && ses.repo === repo && ses.items.length) {
    drawSessions(state);
    updateSessionsState(state);
    if (ses.stale) {
      ses.stale = false;
      void readSessions(state, 'live');
    }
    return true;
  }
  Object.assign(ses, { repo, items: [], next: null, stale: false });
  return readSessions(state, 'initial');
}

function sessionOpen(s) {
  ses.back = { key: s.key, scroll: window.scrollY || 0 };
  ses.open = s;
  ses.kinds = new Set();
  void show();
  window.scrollTo?.(0, 0);
}

async function sessionClose() {
  const back = ses.back;
  ses.open = null;
  await show();
  ses.back = null;
  const opener = back && [...$('list').querySelectorAll('button')].find((b) => b.dataset.session === back.key);
  if (opener) opener.focus({ preventScroll: true });
  window.scrollTo?.(0, back?.scroll || 0);
}

// The kinds among the items read, each a chip with its count, and how many are shown (S5).
function drawKinds(state, chips, count) {
  const focused = document.activeElement?.dataset?.kind;
  const entries = [...state.list.children];
  const counts = new Map();
  for (const entry of entries) counts.set(entry.dataset.category, (counts.get(entry.dataset.category) || 0) + 1);
  chips.replaceChildren(...CATEGORIES.filter((kind) => counts.has(kind)).map((kind) => {
    const chip = el('button', 'quiet small session-chip', word('span', null, `category_${kind}`), ` (${counts.get(kind)})`);
    chip.type = 'button';
    chip.dataset.kind = kind;
    chip.setAttribute('aria-pressed', String(ses.kinds.has(kind)));
    chip.addEventListener('click', () => {
      if (ses.kinds.has(kind)) ses.kinds.delete(kind);
      else ses.kinds.add(kind);
      drawKinds(state, chips, count);
    });
    return chip;
  }));
  // The chip in focus keeps it, as a session's card does (`drawSessions`).
  const again = focused && [...chips.querySelectorAll('button')].find((b) => b.dataset.kind === focused);
  if (again) again.focus({ preventScroll: true });
  let shown = 0;
  for (const entry of entries) {
    entry.hidden = ses.kinds.size > 0 && !ses.kinds.has(entry.dataset.category);
    if (!entry.hidden) shown += 1;
  }
  count.replaceChildren(word('span', null, state.next === null ? 'session_shown' : 'session_shown_more',
    { n: shown, m: entries.length }));
}

async function showSessionPage(mine) {
  const s = ses.open;
  const back = word('button', 'quiet small session-back', 'back_to_sessions');
  back.type = 'button';
  back.addEventListener('click', () => void sessionClose());
  const id = el('p', 'session-detail-id', el('span', 'badge feed-agent', s.agent), el('code', null, s.session));
  const chips = el('div', 'session-chips');
  chips.setAttribute('role', 'group');
  chips.setAttribute('aria-label', t('session_filter'));
  const count = el('span', 'session-chips-count');
  const list = el('ol', 'entries feed');
  const notice = el('p', 'feed-state');
  notice.setAttribute('role', 'status');
  notice.setAttribute('aria-live', 'polite');
  const more = word('button', 'quiet more', 'more');
  more.type = 'button';
  const retry = word('button', 'quiet', 'retry');
  retry.type = 'button';
  const sentinel = el('div', 'feed-sentinel');
  sentinel.setAttribute('aria-hidden', 'true');
  const state = { generation: mine, repo: $('repo').value, session: s.key, list, keys: new Set(), next: null,
    loaded: false, loading: false, error: null, request: null, retryMode: 'initial', notice, more, retry, sentinel,
    observer: null };
  state.drawn = () => drawKinds(state, chips, count);
  feed = state;
  more.addEventListener('click', () => void readFeed(state, 'more'));
  retry.addEventListener('click', () => void readFeed(state, state.retryMode));
  draw(s.title || s.repo_name || s.session, [], [el('div', 'actions', back, forgetControl('session', s.key)), id,
    el('div', 'session-filter', chips, count), list,
    el('div', 'feed-pagination', notice, more, retry, sentinel)]);
  setStatus('');
  observeEnd(state, () => void readFeed(state, 'more'));
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
  simple_view: ['Simple settings', 'かんたん設定'],
  simple_detailed: ['Detailed settings', '詳しい設定'],
  simple_mode_label: ['Settings view', '設定の表示方法'],
  simple_intro: ['Changes save automatically, and actions that send data or remove it ask you to confirm first.', '変更は自動で保存され、データの送信や削除を伴う操作では先に確認します。'],
  simple_saved: ['Saved', '保存しました'],
  simple_use: ['Use', '使う'],
  simple_model_short: ['Model', 'モデル'],
  simple_key_shared: ['Uses the same key as {name}; add it there.', '{name} と同じキーを使います。キーはそちらで登録してください。'],
  simple_ai_h: ['AI for summaries', '要約に使う AI'],
  simple_ai_desc: ['The enabled AI services are tried in this order and receive the records needed to write a summary.', 'オンにした AI がこの順番で使われ、要約に必要な記録が送られます。'],
  simple_no_ai: ['No AI for summaries can be used yet; add a key below or add an AI.', '要約に使える AI がまだないため、下の一覧でキーを登録するか、AI を追加してください。'],
  simple_curate: ['Create summaries', '記録を要約する'],
  simple_status_missing: ['Needs an API key', 'API キーの登録が必要です'],
  simple_status_not_installed: ['Not installed', 'インストールされていません'],
  simple_status_stopped: ['Stopped', '停止しています'],
  simple_status_off: ['Off', 'オフです'],
  simple_status_ready: ['Ready to use', '利用できます'],
  simple_status_unknown: ['Could not check availability', '利用できるか確認できません'],
  simple_ai_on: ['Use {name}', '{name} を使う'],
  simple_model: ['Model for {name}', '{name} のモデル'],
  simple_model_default: ['The app’s default model', 'アプリの標準モデル'],
  simple_other_model: ['Another model…', 'ほかのモデルを選ぶ…'],
  simple_entry_detail: ['Details for {name}', '{name} の詳しい設定'],
  simple_differs: ['These entries have different settings; check their details.', '同じ名前の接続先で設定が異なるため、詳しい設定をご確認ください。'],
  simple_add_ai: ['Add AI', 'AI を追加する'],
  simple_catalog_choose: ['Choose {name}', '{name} を選ぶ'],
  simple_catalog_listed: ['Already in the list', '一覧に追加されています'],
  simple_catalog_open: ['Find it in the list', '一覧で確認する'],
  simple_add_save: ['Add this AI', 'この AI を追加する'],
  simple_add_key: ['After adding an API service, register its key in the list.', 'API を使う接続先を追加したら、一覧でキーを登録してください。'],
  simple_catalog_groq: ['Groq · GPT OSS 120B', 'Groq・GPT OSS 120B'],
  simple_catalog_groq_20b: ['Groq · GPT OSS 20B', 'Groq・GPT OSS 20B'],
  simple_catalog_groq_qwen: ['Groq · Qwen', 'Groq・Qwen'],
  simple_catalog_openrouter: ['OpenRouter', 'OpenRouter'],
  simple_catalog_nim: ['NVIDIA NIM', 'NVIDIA NIM'],
  simple_catalog_opencode_go: ['OpenCode Go', 'OpenCode Go'],
  simple_catalog_anthropic: ['Anthropic API', 'Anthropic API'],
  simple_catalog_codex: ['Codex subscription', 'Codex のサブスクリプション'],
  simple_catalog_claude: ['Claude subscription', 'Claude のサブスクリプション'],
  simple_catalog_gemini: ['Gemini API', 'Gemini API'],
  simple_catalog_custom: ['Another OpenAI-compatible server', 'ほかの OpenAI 互換サーバー'],
  simple_gemini_warning: ['Gemini’s free service may use your input for training and human review.', 'Gemini の無料サービスでは、送った内容が学習や人による確認に使われることがあります。'],
  simple_spending_h: ['Monthly limit on paid calls', '有料の AI に使う月額上限'],
  simple_spending_desc: ['Paid summary calls stop when they reach this limit in US dollars.', '有料の要約に使う金額がこの米ドルの上限に達すると、有料の呼び出しを止めます。'],
  simple_limit: ['Monthly limit (USD)', '月額上限（米ドル）'],
  simple_spent: ['Used this month: {usd}', '今月の利用額：{usd}'],
  simple_limit_invalid: ['Enter a valid amount of zero or more.', '0 以上の有効な金額を入力してください。'],
  simple_search_h: ['Search by meaning', '意味で記憶を探す'],
  simple_search_desc: ['Find related memories even when the words differ, with a review before changing how it works.', '言葉が違っても関連する記憶を探せるようにし、方法を変える前に内容を確認できます。'],
  simple_search_none: ['Words only', '言葉が一致する記憶だけを探す'],
  simple_search_none_desc: ['Searches the recorded words without AI.', 'AI を使わず、記録された言葉で探します。'],
  simple_search_local: ['This computer', 'このパソコンで探す'],
  simple_search_local_desc: ['Downloads a model to find related memories on this computer.', 'モデルをダウンロードし、このパソコンの中で関連する記憶を探します。'],
  simple_search_workers: ['Workers AI', 'Workers AI で探す'],
  simple_search_workers_desc: ['Sends search text and records to Cloudflare and may cost money.', '検索文や記録を Cloudflare に送り、費用がかかることがあります。'],
  simple_search_review: ['Review this choice', 'この方法を確認する'],
  simple_cancel: ['Cancel', 'キャンセル'],
  simple_inject_h: ['What agents are given', 'エージェントに渡す記憶'],
  simple_inject_desc: ['Share previous memories with your agent so you can continue without explaining them again.', '以前の記憶をエージェントに渡し、説明し直さずに続きを進められるようにします。'],
  simple_session: ['Memories at the start of a session', 'セッションの開始時に記憶を渡す'],
  simple_prompt: ['Related decisions, cards, summaries, notes and imported history with each prompt', '入力のたびに関連する決定・カード・要約・メモ・取り込み履歴を渡す'],
  simple_never_h: ['Places never recorded', '記録しない場所'],
  simple_never_desc: ['New activity in these repositories and folders is not recorded, while existing memories remain.', '指定したリポジトリやフォルダーでの新しい作業を記録せず、以前の記憶はそのまま残します。'],
  simple_never_empty: ['No places are excluded from recording.', '記録しない場所は指定されていません。'],
  simple_never_more: ['Choose a repository in detailed settings', '詳しい設定でリポジトリを選ぶ'],
  simple_mask_h: ['Masking of secrets', '機密情報のマスキング'],
  simple_mask_desc: ['Hide API keys and passwords in records, outgoing text and this page.', '記録や送信する内容、この画面に含まれる API キーやパスワードを隠します。'],
  simple_mask_on: ['Mask secrets', '機密情報を隠す'],
  simple_mask_confirm: ['Turn masking off', '確認してマスキングをオフにする'],
  simple_agents_h: ['Agents', '接続するエージェント'],
  simple_agents_desc: ['Check whether each agent is connected to record activity and receive memories.', '作業の記録と記憶の受け渡しのために、各エージェントが接続されているか確認できます。'],
  simple_agent_connected: ['Connected', '接続されています'],
  simple_agent_disconnected: ['Not connected', '接続されていません'],
  simple_agent_partial: ['Check the connection', '接続の確認が必要です'],
  simple_agent_connect: ['Connection settings for {name}', '{name} の接続設定'],
  timeline: ['Timeline', 'タイムライン'],
  sessions: ['Sessions', '会話'],
  back_to_sessions: ['← Back to sessions', '← 会話の一覧へ'],
  session_item: ['1 item', '1 件'],
  session_items: ['{n} items', '{n} 件'],
  sessions_empty: ['No sessions to display', '表示する会話はありません'],
  session_filter: ['Filter by kind', '種類で絞り込む'],
  session_shown: ['{n} of {m}', '{m} 件中 {n} 件'],
  session_shown_more: ['{n} of {m}+', '{m}+ 件中 {n} 件'],
  category_prompt: ['Prompt', 'プロンプト'],
  category_summary: ['Summary', '要約'],
  category_discovery: ['Discovery', '発見'],
  category_decision: ['Decision', '決定'],
  category_bugfix: ['Bugfix', '不具合の修正'],
  category_feature: ['Feature', '機能'],
  category_refactor: ['Refactor', '構造の整理'],
  category_change: ['Change', '変更'],
  category_security_alert: ['Security Alert', '安全性についての警告'],
  category_security_note: ['Security Note', '安全性についての補足'],
  category_other: ['Other', 'その他'],
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
  quote_expired: ['Raw expired: the record\'s text was removed after its retention period, so the quote is not shown.', '元の記録は保存期間を過ぎて本文が削除されたため、引用は表示できません。'],
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
  vector_loading: ['loading', '読み込み中'],
  'vector_no-model': ['no-model', 'モデル未準備'],
  vector_why_off: ['embedding is off', '意味に基づく検索は無効になっています。'],
  vector_why_excluded: ['this repository or the one searched is excluded, so the query is not sent out', 'このリポジトリまたは検索対象が除外されているため、検索語句を外部に送りません。'],
  'vector_why_no-vectors': ['no document has a vector yet', '意味に基づく検索に必要な記録の準備がまだできていません。'],
  vector_why_building: ["the new embedder's vectors are still being made", '新しい設定で意味に基づく検索を利用できるよう、記録を準備しています。'],
  vector_why_waiting: ['the embedder is resting, or its cap is spent', '意味に基づく検索は一時停止中、または利用上限に達しています。'],
  vector_why_timeout: ["the query's embedding took too long", '意味に基づく検索に必要な語句の処理が、制限時間内に終わりませんでした。'],
  vector_why_error: ['the query could not be embedded', '意味に基づく検索に必要な語句の処理に失敗しました。'],
  vector_why_unavailable: ['Vector search is unavailable.', '意味に基づく検索を利用できません。'],
  vector_why_loading: ['the local model is loading, so this search is full text only', '意味に基づく検索に使うモデルをこの端末で読み込んでいます。この検索は全文検索のみで行いました。'],
  'vector_why_no-model': ['the local model is not ready; oboete doctor shows why', '意味に基づく検索に使うモデルがこの端末でまだ準備できていません。理由は oboete doctor で確認できます。'],
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
  meta_note: ['note', 'エージェントのメモ'],
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
  show_more: ['Show more', 'もっと見る'],
  show_less: ['Show less', '折りたたむ'],
  feed_note: ['Note', 'メモ'],
  feed_note_subtitle: ['Saved by an agent', 'エージェントが保存したメモ'],
  untitled: ['Untitled', 'タイトルなし'],
  facts: ['facts', '事実'],
  narrative: ['narrative', '説明'],
  files_read: ['read:', '参照:'],
  files_modified: ['modified:', '変更:'],
  investigated: ['Investigated', '調査したこと'],
  learned: ['Learned', '学んだこと'],
  completed: ['Completed', '完了したこと'],
  next_steps: ['Next steps', '次の作業'],
  notes: ['Notes', 'メモ'],
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
  onboarding_first_h: ['Choose a starting AI tier', '最初のAI利用段階を選ぶ'],
  onboarding_again_h: ['Adjust AI tier', 'AI利用段階を変更'],
  onboarding_desc: ['Copy a tier into the unsaved settings below, review the named providers, then Save settings. Choosing a tier sends no request.', '利用段階を下の未保存の設定に反映し、対象の要約役を確認してから「設定を保存」を押します。段階の選択だけでは通信しません。'],
  onboarding_tier: ['AI tier', 'AI利用段階'],
  onboarding_choose: ['Choose a tier', '利用段階を選択'],
  onboarding_none: ['None: record and search, no new summaries', 'なし：記録と検索のみ、新しい要約はしない'],
  onboarding_free: ['Configured free-plan and local candidates', '無料枠候補とローカル設定'],
  onboarding_subscription: ['Plus my subscription entries', 'サブスクリプションも使う'],
  onboarding_paid: ['Plus priced APIs with a monthly cap', '月額上限内で有料APIも使う'],
  onboarding_copy: ['Use this tier in draft', 'この段階を編集中の設定へ反映'],
  onboarding_effect: ['New curation through: {names}. Monthly paid cap: USD {cap}.', '新しい要約に使う要約役：{names}。有料利用の月額上限：USD {cap}。'],
  onboarding_effect_no_cap: ['New curation through: {names}. Monthly paid cap: none.', '新しい要約に使う要約役：{names}。有料利用の月額上限：なし。'],
  onboarding_no_names: ['none', 'なし'],
  onboarding_unavailable: ['No eligible provider is configured for this tier. Applying it would leave summarizing off; inspect or add a provider below.', 'この段階で利用できる要約役は設定されていません。反映すると要約はオフになります。下で要約役を確認・追加してください。'],
  onboarding_draft_ready: ['Draft updated. Review the controls below and Save settings explicitly.', '編集中の設定に反映しました。下の項目を確認し、明示的に「設定を保存」を押してください。'],
  onboarding_no_assurance: ['A key file or CLI on PATH does not prove login, a free allowance or a working connection. Test a supported provider explicitly if needed; that test may send a request and incur a charge.', '鍵ファイルやCLIが見つかっても、ログイン・無料枠・接続成功は確認できません。必要なら対応する要約役を明示的に接続確認してください。その確認では通信や料金が発生する場合があります。'],
  onboarding_providers: ['Provider entries', '要約役の設定'],
  onboarding_save: ['Save settings', '設定を保存'],
  onboarding_agents: ['Agent connections', 'エージェントの接続'],
  onboarding_doctor: ['Run diagnostics', '診断を実行'],
  onboarding_never: ['If the work in some repositories or folders must never be stored, add them under “{section}” before their next session.', '記録したくないリポジトリやフォルダーがあれば、次のセッションの前に「{section}」で指定してください。'],
  onboarding_search_step: ['After saving, confirm an agent connection separately. In its next session, type a harmless unique phrase; then open Records and search for it. This checks only that session’s recording and search.', '保存後、エージェントの接続を別に確認してください。その次のセッションで害のない固有の語を入力し、「記録」で検索します。確認できるのは、そのセッションの記録と検索だけです。'],
  onboarding_scope: ['These choices control AI summaries. Embeddings and other AI roles have separate settings. Saving starts no connection test, model download or agent session.', 'ここで選ぶのは要約用の AI です。埋め込みや他の AI の役割は個別に設定します。保存だけで接続確認・モデルのダウンロード・エージェントのセッションを開始しません。'],
  onboarding_policy_h: ['Subscription rules and sign-in', 'サブスクリプションの規約とログイン'],
  onboarding_policy: ['Subscription use shares your coding quota. Check your account’s limits and paid-credit settings; you are responsible for following each provider’s terms. Choose None or turn off its chain entry to stop new summaries through it. Complete login in the provider’s own flow, then return here and refresh the agent inventory or run diagnostics; registration alone is not a login check.', 'サブスクリプションは開発時と同じ利用枠を使います。アカウントの上限・有料クレジット設定を確認し、各社の規約に従って利用してください。「なし」を選ぶか、要約役をオフにするとその要約役での新しい要約を停止できます。ログインは提供元の画面で完了し、ここへ戻って登録状況を更新するか診断を実行してください。登録状況だけではログインを確認できません。'],
  onboarding_claude_policy: ['Anthropic restricts third-party routing of users’ subscription credentials and collecting session tokens. Its policy also says:', 'Anthropic は第三者による利用者のサブスク認証の仲介やセッショントークンの収集を制限しています。同じポリシーには次の記載があります：'],
  onboarding_go_policy: ['Go requires each user’s own key and internal use; prompts pass to upstream model providers. Its policy says:', 'Go は利用者本人のキーと内部利用を求め、入力を上流のモデル提供元へ渡します。利用ポリシーには次の記載があります：'],
  onboarding_search_open: ['Find my test phrase', '確認用の語を検索'],
  onboarding_search_phrase: ['The harmless phrase recorded in your agent', 'エージェントで記録した確認用の語'],
  onboarding_prompt_off: ['Prompt text storage is off. A new typed phrase will not be searchable. Keep this privacy choice and search an existing record, or explicitly turn on “{setting}” under {section} and save before trying a new harmless phrase.', '入力文を保存しない設定です。新しく入力した語は検索できません。この選択を維持して既存の記録を検索するか、新しい確認用の語を試す前に「{section}」で「{setting}」を明示的にオンにして保存してください。'],
  resident_timing: ['The next agent hook or running oboete view starts the resident processes. Turning this off applies when the worker is idle; the resident page stops once it has no requests.', '次のエージェントのフックか oboete view の実行で常駐プロセスが起動します。オフはワーカーの待機時に反映され、常駐の画面はアクセスがなくなった後に終了します。'],
  resident_unsupported: ['Resident mode is currently available on Linux and WSL. This system keeps the worker that exits when idle.', '常駐は現在 Linux と WSL に対応しています。この環境のワーカーは、処理がなくなった時に終了します。'],
  view_h: ['Page address', '画面のアドレス'],
  view_port: ['Port of the resident page', '常駐の画面のポート'],
  view_port_resident: ['Saving another port moves this page there at once with a new token, so the old bookmark stops working. Bookmark the new address afterwards and delete the old bookmark.', '別のポートを保存すると、この画面は新しいトークンとともにすぐにそのポートへ移り、古いブックマークは使えなくなります。移った後は新しいアドレスをブックマークし、古いブックマークを削除してください。'],
  view_port_foreground: ['The resident page uses this port when “Keep oboete running between sessions” is on. Saving another port also gives the resident page a new token, so its old bookmark stops working. This page keeps its own address until it closes.', '「セッション間も oboete を起動したままにする」がオンのとき、常駐の画面がこのポートを使います。別のポートを保存すると常駐の画面のトークンも新しくなり、古いブックマークは使えなくなります。この画面は閉じるまで今のアドレスのままです。'],
  view_moved: ['This is the new address of oboete’s page. Bookmark it and delete the old bookmark.', 'oboete の画面の新しいアドレスです。ブックマークし直し、古いブックマークは削除してください。'],
  view_token_h: ['New page token', '画面のトークンを新しくする'],
  view_token_desc: ['Makes a new token and moves this page to another port. The old bookmark, and any page still open at the old address, stop working. Use it when someone else may have seen the address.', '新しいトークンを作り、この画面を別のポートへ移します。古いブックマークと、古いアドレスで開いたままの画面は使えなくなります。アドレスを他人に見られた可能性があるときに使います。'],
  view_token_confirm: ['I understand that the old bookmark stops working.', '古いブックマークが使えなくなることを理解しました。'],
  view_token_go: ['Make a new token', '新しいトークンを作る'],
  view_token_unchanged: ['The token was not replaced, and the address is unchanged.', 'トークンは変わっていません。アドレスもそのままです。'],
  view_token_unsure: ['The page moved, but replacing the token did not finish. Open the new address below, then make a new token again.', '画面は移りましたが、トークンの置き換えが完了しませんでした。下の新しいアドレスを開き、もう一度新しいトークンを作ってください。'],
  view_open_new: ['Open the new address', '新しいアドレスを開く'],
  view_lost: ['The answer did not arrive. If this page stops working, run oboete view in a terminal to see the current address.', '応答が届きませんでした。この画面が使えなくなった場合は、ターミナルで oboete view を実行すると現在のアドレスが表示されます。'],
  view_resident_h: ['Run in the background', 'バックグラウンドで動かす'],
  view_resident_desc: ['Starts oboete in the background: the worker, and its page at a fixed address (port {port}). Both keep running after this window closes, and the worker records and summarizes with the saved settings. This page then moves to that address, and this run ends.', 'oboete をバックグラウンドで起動します。ワーカーと、固定アドレス(ポート {port})の画面です。どちらもこのウィンドウを閉じても動き続け、ワーカーは保存済みの設定で記録と要約を行います。その後この画面はそのアドレスへ移り、今の起動は終了します。'],
  view_resident_go: ['Start in the background now', '今すぐバックグラウンドで起動'],
  view_port_in_use: ['Another program uses port {port}. Choose another port above, save, and try again. This page keeps working.', 'ポート {port} は別のプログラムが使っています。上で別のポートを選んで保存し、もう一度試してください。この画面はそのまま使えます。'],
  view_not_private: ['This folder cannot keep the page’s token readable by you alone, so the resident page cannot run here. This page keeps working.', 'このフォルダではトークンを本人だけが読める形で保存できないため、常駐の画面は使えません。この画面はそのまま使えます。'],
  view_not_started: ['The resident page did not start. “Run diagnostics now” below shows why. This page keeps working.', '常駐の画面が起動しませんでした。理由は下の「今すぐ診断」で確認できます。この画面はそのまま使えます。'],
  view_not_resident: ['Save “Keep oboete running between sessions” first.', '先に「セッション間も oboete を起動したままにする」を保存してください。'],
  port_unavailable: ['This port cannot be used: another program may hold it. Choose another.', 'このポートは使えません。別のプログラムが使っている可能性があります。別のポートを選んでください。'],
  token_replaced: ['The settings were not saved, but the resident page’s token was already replaced. Run oboete view in a terminal to see its address.', '設定は保存されませんでしたが、常駐の画面のトークンはすでに新しくなっています。ターミナルで oboete view を実行すると、その画面のアドレスが表示されます。'],
  language: ['Language', '言語'],
  lead: [
    'Choose the settings below. Each save applies its own section; individual provider checkboxes and arrows save immediately. Recording and memory delivery read them at their next use; the background summarizer reads them before its next window, even while it stays running. This page cannot tell which values a running request has loaded. Opening this page or saving sends nothing to a provider.',
    'ここで設定を選べます。それぞれの保存ボタンで対象の設定を反映します。要約役の個別のチェックと矢印はすぐに保存します。記録・記憶の受け渡しには次の利用時から反映されます。要約の設定は、次のまとまりを処理する前に読み直すので、処理が動き続けていても反映されます。現在実行中の呼び出しが読み込んでいる値は、この画面では確認できません。画面を開いたり保存したりしても、要約役への送信は始まりません。',
  ],
  group_start_h: ['Getting started', 'はじめに'],
  group_start_desc: ['Choose how much AI to use, then check that an agent records and finds a test phrase.', 'AI をどこまで使うかを選び、エージェントで記録と検索ができることを確かめます。'],
  group_everyday_h: ['Everyday settings', 'よく使う設定'],
  group_everyday_desc: ['Summaries, the limit for paid calls, what agents are given and what is recorded.', '要約、有料の呼び出しの上限、エージェントに渡す記憶、記録する内容です。'],
  group_providers_h: ['AI providers', '要約役'],
  group_providers_desc: ['The AI services that write summaries: their order, keys and connection tests.', '要約を書く AI の接続先と、その順番、キー、接続テストです。'],
  group_privacy_h: ['Privacy', 'プライバシー'],
  group_privacy_desc: ['Masking sensitive values, repositories whose records are not sent out, and repositories and folders never recorded.', '機密値のマスキング、記録を外部に送らないリポジトリ、記録しないリポジトリとフォルダーの指定です。'],
  group_computer_h: ['This computer', 'このパソコン'],
  group_computer_desc: ['Running between sessions, the page’s address and where backups go.', 'セッション間の常駐、画面のアドレス、バックアップの保存先です。'],
  group_agents_h: ['Agents', 'エージェント'],
  group_agents_desc: ['A preference for every agent, which agents are registered, and connecting agents.', 'すべてのエージェントに伝える好み、登録状況の確認、エージェントの接続です。'],
  group_maintenance_h: ['Diagnostics and maintenance', '診断と保守'],
  group_maintenance_desc: ['Diagnostics, importing and rebuilding history, and other occasional tasks.', '診断、履歴の取り込みや再構築など、ときどき使う操作です。'],
  summary_h: ['Summarizing recorded activity', '記録の要約'],
  summary_desc: [
    'When enabled, the background summarizer may send recorded text to the providers below. Paid calls can cost money. Turning it off keeps existing memories and stops new summaries at the next run.',
    '有効にすると、バックグラウンドの要約処理が記録した本文を下の要約役へ送ることがあります。有料の呼び出しには費用がかかります。無効にしても既存の記憶は残り、次の処理で設定を読み直すと、新しい要約を止めます。',
  ],
  curate_on: ['Summarize recorded activity (default: off)', '記録を要約する(既定: 切)'],
  my_work_h: ['My Work, written by AI', 'AI が書く My Work'],
  my_work_desc: ['When on, the AI for summaries also groups your conversations into requests and writes their progress, your goals and suggested rules for the My Work page. It runs only while summaries are on.', 'オンにすると、要約に使う AI が会話を依頼ごとにまとめ、進み具合・目標・ルールの提案を書いて My Work の画面に出します。要約がオンのときだけ動きます。'],
  my_work_on: ['Write My Work with AI (default: off)', 'My Work を AI で書く(既定: 切)'],
  my_work_calls: ['Calls a day, at most ({min} to {max})', '1 日の呼び出しの上限({min}〜{max} 回)'],
  my_work_calls_desc: ['At the limit, the work waits for the next day. The monthly limit on paid calls applies too.', '上限に達すると翌日まで待ちます。有料の AI の月額上限もそのまま効きます。'],
  my_work_estimate: ['Calls a day with it on: about {calls}, at most {most}. At {name} ({model}) prices: about {usd} a day, at most {most_usd}.', 'オンのときの 1 日の呼び出し: およそ {calls} 回、多くても {most} 回。{name}({model})の料金で 1 日およそ {usd}、多くても {most_usd} です。'],
  my_work_estimate_free: ['Calls a day with it on: about {calls}, at most {most}. No paid AI is in the list, so nothing is charged; a subscription\'s allowance may be used.', 'オンのときの 1 日の呼び出し: およそ {calls} 回、多くても {most} 回。有料の AI が一覧にないため料金はかかりません(サブスクリプションの利用枠は使われることがあります)。'],
  my_work_estimate_none: ['An estimate appears here once something is recorded.', '記録がたまると、ここに見積もりが出ます。'],
  my_work_estimate_no_cap: ['Calls a day with it on: about {calls}, with no cap. At {name} ({model}) prices: about {usd} a day; a busy day costs more.', 'オンのときの 1 日の呼び出し: およそ {calls} 回(上限なし)。{name}({model})の料金で 1 日およそ {usd} です。作業の多い日はこれより多くかかります。'],
  my_work_estimate_free_no_cap: ['Calls a day with it on: about {calls}, with no cap. No paid AI is in the list, so nothing is charged; a subscription\'s allowance may be used.', 'オンのときの 1 日の呼び出し: およそ {calls} 回(上限なし)。有料の AI が一覧にないため料金はかかりません(サブスクリプションの利用枠は使われることがあります)。'],
  judge_h: ['Judge (Jev)', '判定役(Jev)'],
  judge_desc: ['Jev, a model by Typesafe, answers questions with probabilities instead of text. My Work asks it whether each new conversation continues an open request, and puts the conversation into that request when Jev is sure. Next, My Work will also ask it which focus area a request belongs to. What Jev cannot decide stays with the AI for summaries, as before.', 'Jev(Typesafe 社のモデル)は、文章ではなく確率で質問に答えます。My Work は新しい会話ごとに、開いている依頼の続きかどうかを Jev に尋ね、Jev が確かだと答えたときはその依頼にまとめます。次の更新から、依頼がどの分野に入るかも Jev に尋ねます。Jev が決められないものは、これまでどおり要約に使う AI が決めます。'],
  judge_sends: ['What it is sent: for each new conversation, its repository, when it started, its first 3 asks and its first 8 card titles, and the same of up to 19 open requests of that repository, with their titles; each masked as all text sent out is. Nothing from a repository you excluded is sent. Price: USD 0.042 for a million input tokens, counted in the monthly limit on paid calls. One judged conversation is about 10,000 to 16,000 tokens, under USD 0.001.', '送るもの: 新しい会話ごとに、そのリポジトリ、始まった時刻、最初の 3 つの依頼、最初の 8 枚のカードの題と、同じリポジトリの開いている依頼 (最大 19 件) の同じ内容と題名です。どれも外へ送る文と同じく伏せ字を通します。除外したリポジトリのものは送りません。料金: 入力 100 万トークンで 0.042 ドルで、有料の呼び出しの月額上限に含まれます。1 つの会話の判定はおよそ 1 万〜1.6 万トークンで、0.001 ドル未満です。'],
  judge_on: ['Use the judge', '判定役を使う'],
  judge_no_key: ['Off: no key yet. Paste the key from Typesafe below to turn it on.', 'オフ: まだキーがありません。下に Typesafe のキーを貼り付けるとオンになります。'],
  judge_key_file: ['Key: {state} ({file})', 'キー: {state}（{file}）'],
  judge_key_label: ['New Typesafe key', 'Typesafe の新しいキー'],
  judge_usage: ['Last 24 hours: {calls} of {most} calls. This month: {usd}.', '直近 24 時間: {most} 回のうち {calls} 回。今月の費用: {usd}。'],
  judge_usage_unknown: ['Its calls and spend cannot be read now.', '呼び出し回数と費用をいま読めません。'],
  judge_usage_no_cap: ['Last 24 hours: {calls} calls, with no cap. This month: {usd}.', '直近 24 時間: {calls} 回(上限なし)。今月の費用: {usd}。'],
  judge_calls: ['Calls a day, at most ({min} to {max})', '1 日の呼び出しの上限({min}〜{max} 回)'],
  judge_calls_desc: ['At the limit, the judge is not asked until the next day.', '上限に達すると、翌日まで判定役に尋ねません。'],
  judge_test: ['Test the connection', '接続を試す'],
  judge_test_desc: ['Sends one made-up question, with nothing from your records. It counts as one call.', '作った質問を 1 つだけ送ります。記録の中身は送りません。呼び出し 1 回に数えます。'],
  judge_test_ok: ['Jev answered: the key works.', 'Jev が答えました。キーは使えます。'],
  judge_test_error: ['Jev did not answer. Check the key, then try again.', 'Jev が答えませんでした。キーを確かめて、もう一度試してください。'],
  judge_test_shape: ['Jev answered in an unexpected form. Try again later.', 'Jev の答えが想定と違う形でした。しばらくしてから試してください。'],
  judge_test_budget: ['Today\'s calls or this month\'s limit on paid calls are used up.', '今日の呼び出し回数か、今月の有料の呼び出しの上限に達しています。'],
  judge_test_gate: ['The judge is paused for now. Try again later.', '判定役はいま休止中です。しばらくしてから試してください。'],
  judge_test_off: ['The judge is off. Turn it on and save, then try again.', '判定役がオフです。オンにして保存してから試してください。'],
  judge_test_key: ['No key was found in its file. Paste the key again.', 'キーのファイルにキーがありません。もう一度貼り付けてください。'],
  judge_test_missing_key: ['Paste the key first.', '先にキーを貼り付けてください。'],
  judge_test_failed: ['The test could not run. Try again.', 'テストを実行できませんでした。もう一度試してください。'],
  simple_my_work_h: ['My Work, written by AI', 'AI が書く My Work'],
  simple_my_work_desc: ['The AI for summaries groups conversations into requests and writes their progress and your goals. It runs only while summaries are created.', '要約に使う AI が会話を依頼ごとにまとめ、進み具合と目標を書きます。「記録を要約する」がオンのときだけ動きます。'],
  simple_my_work: ['Write My Work with AI', 'My Work を AI で書く'],
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
    'Default: 10 minutes. While you work, each step that finishes is sent at once. A new request whose work has not started yet waits for it, up to this time. Zero skips this wait. The actual wait is at most 30 minutes, even when a larger value is saved. Changing it does not start a paid call.',
    '既定は 10 分です。作業中は、終わった手順をすぐに送ります。まだ作業が始まっていない新しい依頼は、この時間まで作業の始まりを待ちます。0 ではこの待ち時間をなくします。大きい値を保存しても実際に待つのは最大 30 分です。変更しても有料の呼び出しは始まりません。',
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
  session_note_desc: ['A short status line. This changes at the next session start.', '短い状態の 1 行です。次のセッション開始から反映されます。'],
  session_list_on: ['Also show the list of memories handed over, as claude-mem does', '渡した記憶の一覧も表示する (claude-mem と同じ表示)'],
  session_list_desc: ['The list shows the titles and summaries from your memory that the agent gets. Off, only the status line is shown.', '一覧には、AI に渡したのと同じ記憶の題名や要約が表示されます。オフにすると、状態の 1 行だけを表示します。'],
  inject_chars: ['Size in characters ({min} to {max})', '大きさ(文字数、{min}〜{max})'],
  per_prompt_on: [
    'Also give the decisions, cards and session summaries that match each prompt in words or meaning, and the imported history and notes that match it in meaning (off until it has been measured)',
    'プロンプトごとに、言葉や意味が近い決定・カード・セッションの要約と、意味が近い取り込み履歴・メモも渡す(測定が済むまでは切)',
  ],
  per_prompt_chars: ['Their size in characters ({min} to {max})', 'その大きさ(文字数、{min}〜{max})'],
  per_prompt_wait_ms: ['How long each prompt waits for search by meaning, in milliseconds ({min} to {max}); past it, what its words match is used', '各プロンプトで意味の検索を待つ上限(ミリ秒、{min}〜{max})。過ぎたら言葉の一致だけを使います'],
  prompt_side_meaning: ['The last prompt ({time}) also took search by meaning.', '直近のプロンプト({time})は意味の検索も使いました。'],
  prompt_side_no_viewer: ['The last prompt ({time}) took what its words match alone: no resident viewer was answering.', '直近のプロンプト({time})は言葉の一致だけでした。常駐の画面が応答していません。'],
  prompt_side_late: ['The last prompt ({time}) took what its words match alone: search by meaning answered after the wait.', '直近のプロンプト({time})は言葉の一致だけでした。意味の検索が待ち時間に間に合いませんでした。'],
  prompt_side_failed: ['The last prompt ({time}) took what its words match alone: the resident viewer did not answer.', '直近のプロンプト({time})は言葉の一致だけでした。常駐の画面が応答しませんでした。'],
  prompt_side_loading: ['The last prompt ({time}) took what its words match alone while the local model was loading; the next ones use it.', '直近のプロンプト({time})はモデルの読み込み中だったため言葉の一致だけでした。次からは使います。'],
  prompt_side_other: ['The last prompt ({time}) took what its words match alone ({why}).', '直近のプロンプト({time})は言葉の一致だけでした({why})。'],
  memory_lookup_on: [
    'When an agent looks through its own memory notes, also give what oboete remembers about it',
    'エージェントが自分のメモを調べるときに、oboete の記憶からも探して渡す',
  ],
  memory_instructions_on: [
    'At session start, tell the agent how to search and save memory',
    'セッション開始時に、記憶の検索と保存の使い方をエージェントに伝える',
  ],
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
  retention_h: ['How long raw records are kept', '元の記録を残す期間'],
  retention_desc: ['The text of a raw record (your prompts, the replies and tool output) is removed once it is older than this many days, counted from the record\'s own time (for imported history, the original time), and curation and the session summaries are done with it. Records that are already older are removed at the next check. Claims, cards and session summaries stay, and a record not curated yet is kept until it is.', '元の記録 (入力したプロンプト、応答、ツールの出力) の本文は、記録の日時 (取り込んだ履歴は元の日時) からこの日数を過ぎ、記録の要約とセッションの要約が済んだものから削除されます。すでにこの日数を過ぎている記録は、次の確認で削除されます。主張・カード・セッションの要約は残り、まだ要約していない記録は要約が済むまで残します。'],
  retention_forever: ['Keep forever (the default)', 'いつまでも残す (既定)'],
  retention_days: ['Days to keep ({min} to {max})', '残す日数 ({min}〜{max})'],
  retention_forever_value: ['forever', 'いつまでも'],
  retention_days_value: ['{days} days', '{days} 日'],
  retention_day_value: ['{days} day', '{days} 日'],
  retention_note: ['The worker checks once a week. Each time it removes text, it builds the search index again, which takes a few minutes; searching the memory may be unavailable meanwhile. Removed text cannot be brought back.', 'ワーカーは週に 1 回確認します。本文を消すたびに検索の索引を作り直すため数分かかり、その間は記憶の検索が使えないことがあります。消した本文は元に戻せません。'],
  backup_h: ['Backup location', 'バックアップの保存先'],
  backup_dir: ['Directory', 'ディレクトリ'],
  backup_default: ['Default: backups inside the memory home', '既定: 記憶の保存先にある backups'],
  backup_home: ['Memory home (an empty saved path)', '記憶の保存先(保存済みパスが空文字)'],
  backup_reset: ['Use the default backup location', '既定のバックアップ先を使う'],
  backup_desc: ['An empty field uses the default. Relative paths start at the memory home; absolute paths stay absolute. The next backup uses this location. Saving creates no directory and moves no existing backups or forget logs.', '空欄では既定の場所を使います。相対パスは記憶の保存先を基準とし、絶対パスはそのまま使います。次のバックアップから反映されます。保存してもディレクトリは作らず、既存のバックアップや忘却のログは移動しません。'],
  redaction_h: ['Masking sensitive values', '機密値のマスキング'],
  redaction_desc: ['Masking replaces secrets such as API keys with [REDACTED] before text is stored, sent or shown, using the built-in rules and the rules you add. Saved changes apply at the next recording, send or display. The worker rescans this device’s stored records at its next run; a saved change does not start the worker. This does not erase backups or cover other devices.', 'マスキングは、本文を保存・送信・表示する前に、組み込みのルールと追加したルールで API キーなどの機密値を [REDACTED] に置き換えます。保存した変更は、次の記録・送信・表示から使われます。ワーカーは次回の処理で、この端末の保存済み記録を再検査します。保存だけではワーカーは起動しません。バックアップの削除や他の端末の検査は含みません。'],
  redaction_enabled: ['Mask secrets such as API keys and passwords', 'API キーやパスワードなどの機密値をマスクする'],
  redaction_off_warning: ['With masking off, secrets are stored as typed on this device and in its backups, sent to the AI services set up here for summaries and search, and shown to your agents. What has been sent cannot be called back. Turning masking on again masks what was stored meanwhile from the worker’s next run on, but not what was sent.', 'マスキングがオフの間は、機密値が入力どおりにこの端末とバックアップに保存され、要約や検索のために設定した AI サービスへ送られ、エージェントにもそのまま渡ります。送信したものは取り消せません。マスキングをオンに戻すと、それまでに保存された分はワーカーの次回の処理からマスクされますが、送信したものは戻りません。'],
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
  privacy_never_recorded: ['Never recorded, so never sent', '記録しないため送信もしません'],
  never_h: ['Never record', '記録しない'],
  never_desc: ['Nothing that happens in these repositories and folders is written from now on: no prompt, tool call or reply. A repository that is never recorded is never sent out either. What was already recorded stays; for a repository, `oboete forget --repo <a folder in it>` removes it. Recording again starts from then on, and nothing from while it was excluded comes back. A repository can also stop its own recording with `capture = false` in its `.oboete.toml`.', 'ここに挙げたリポジトリとフォルダーで起きたことを、今後は書き込みません。プロンプト・ツールの実行・返答のどれも記録しません。記録しないリポジトリは外部にも送りません。すでに記録した分は残ります。リポジトリの分は `oboete forget --repo <そのリポジトリ内のフォルダー>` で消せます。記録を再開すると、その時点から記録します。除外していた間の記録は戻りません。リポジトリ側でも `.oboete.toml` に `capture = false` と書くと記録を止められます。'],
  never_repos_h: ['Repositories', 'リポジトリ'],
  never_folders_h: ['Folders on this device', 'この端末のフォルダー'],
  never_folders_none: ['No folder is excluded.', '除外しているフォルダーはありません。'],
  never_on: ['Never recorded', '記録しない'],
  never_off: ['Not excluded', '除外していません'],
  never_exclude: ['Never record from now on', '今後は記録しない'],
  never_undo: ['Record again from now on', '今後は記録を再開'],
  never_folder_input: ['Folder to never record (full path)', '記録しないフォルダー (フルパス)'],
  never_folder_add: ['Never record this folder', 'このフォルダーを記録しない'],
  never_repo_stopped: ['Recording of {label} stopped. What was recorded before stays; `oboete forget --repo <a folder in it>` removes it.', '{label} の記録を止めました。これまでの記録は残ります。`oboete forget --repo <そのリポジトリ内のフォルダー>` で消せます。'],
  never_folder_stopped: ['Recording in this folder stopped. What was recorded before stays.', 'このフォルダーの記録を止めました。これまでの記録は残ります。'],
  never_undone: ['The exclusion is removed. From now on this is recorded, unless another exclusion or a checkout’s .oboete.toml still covers it. Nothing from while it was excluded comes back.', '除外を外しました。別の除外やチェックアウトの .oboete.toml が当てはまらない限り、今後は記録します。除外していた間の記録は戻りません。'],
  never_undone_repo_covered: ['This exclusion is removed, but the repository is not recorded yet: another exclusion reads as it under the rules now. Record that one again from its own row.', 'この除外は外しましたが、今の規則で同じに読める別の除外があるため、このリポジトリはまだ記録しません。その除外の行から記録を再開してください。'],
  never_undone_covered: ['This folder is no longer excluded, but nothing there is recorded yet: another exclusion, or its checkout’s .oboete.toml, still covers it.', 'このフォルダーの除外は外しましたが、別の除外かチェックアウトの .oboete.toml がまだ当てはまるため、ここはまだ記録しません。'],
  capture_folder: ["Enter the folder's full path: from / (or a drive letter on Windows).", 'フォルダーのフルパスを入力してください (/ から、Windows ではドライブ名から)。'],
  folder_not_found: ['This folder is no longer in the list. Reload its state.', 'このフォルダーは一覧にありません。状態を再読み込みしてください。'],
  capture_pattern: ['Enter a folder name or a pattern of at most 256 bytes.', 'フォルダー名かパターンを入力してください (256 バイトまで)。'],
  pattern_not_found: ['This name or pattern is no longer in the list. Reload its state.', 'この名前やパターンは一覧にありません。状態を再読み込みしてください。'],
  never_patterns_h: ['Folder names and patterns', 'フォルダー名とパターン'],
  never_patterns_desc: ['A name such as my-secret-project stops recording in every folder of that name and inside one. A pattern works as in claude-mem: * stands for any characters within one folder name, ** for any folders, ? for one character, and a leading ~ for your home folder.', 'my-secret-project のような名前を入れると、その名前のフォルダーとその中では記録しません。パターンは claude-mem と同じ書き方です。* はフォルダー名の中の任意の文字、** は任意の階層、? は 1 文字、先頭の ~ はホームフォルダーを表します。'],
  never_patterns_none: ['No folder name or pattern is excluded.', '除外しているフォルダー名やパターンはありません。'],
  never_pattern_input: ['Folder name or pattern to never record', '記録しないフォルダー名またはパターン'],
  never_pattern_add: ['Never record the folders it names', '当てはまるフォルダーを記録しない'],
  never_pattern_stopped: ['Recording in the folders it names stopped. What was recorded before stays.', '当てはまるフォルダーの記録を止めました。これまでの記録は残ります。'],
  never_other: ['Another repository never recorded reads as this one under the rules now, so this one is not recorded either. Recording it again here would record that repository too: record it again from its own row, or change the rule that makes them read alike.', '記録しない別のリポジトリが、今の規則ではこのリポジトリと同じに読めるため、こちらも記録されていません。ここで記録を再開すると、そのリポジトリも記録されます。そのリポジトリの行から再開するか、同じに読ませている規則を変えてください。'],
  never_ambiguous: ['Several repositories never recorded read as this one now: record each again from its own row, or with `oboete capture exclude --undo` and the name it was excluded under.', '記録しない複数のリポジトリが、今の規則ではこのリポジトリと同じに読めます。それぞれの行から、または `oboete capture exclude --undo` に除外したときの名前を付けて、記録を再開してください。'],
  rescan_h: ['Rescan on this device', 'この端末の再検査'],
  rescan_empty: ['No raw records to scan.', '検査する元の記録はありません。'],
  rescan_pending: ['Waiting for the worker or still scanning.', 'ワーカーの処理待ち、または検査中です。'],
  rescan_complete: ['The current rules were checked through this device’s current raw sequence.', '現在のルールで、この端末の元の記録の末尾まで検査済みです。'],
  rescan_unavailable: ['Rescan state could not be read.', '再検査の状態を読み込めませんでした。'],
  rescan_progress: ['Checkpoint: {processed}; current raw sequence: {total}', '検査位置: {processed}、元の記録の末尾: {total}'],
  memory_action_failed: ['The operation was refused ({status}).', '操作を受け付けられませんでした({status})。'],
  memory_result_unknown: ['The connection ended before the result was confirmed. The operation may already be recorded. Check the stored state before repeating it.', '結果を確認する前に接続が終了しました。操作がすでに記録されている可能性があります。繰り返す前に保存済みの状態を確認してください。'],
  forget_button: ['Forget', '忘れる'],
  forget_session_button: ['Forget this session', 'この会話を忘れる'],
  forget_preview_session: ['This forgets the session: {records} records, and the {claims} memories, {cards} cards and {summaries} summaries made from them.', 'この会話の記録 {records} 件と、そこから作った記憶 {claims} 件・カード {cards} 件・要約 {summaries} 件を忘れます。'],
  forget_preview_prompt: ['This forgets the prompt, and the {claims} memories, {cards} cards and {summaries} summaries made from it.', 'このプロンプトと、そこから作った記憶 {claims} 件・カード {cards} 件・要約 {summaries} 件を忘れます。'],
  forget_preview_claim: ['This forgets the memory.', 'この記憶を忘れます。'],
  forget_preview_note: ['This forgets the note.', 'このメモを忘れます。'],
  forget_preview_copies: ['It also forgets {copies} records of the same conversation from another source, which may hold a copy of it.', '同じ会話を別の記録元から取り込んだ記録 {copies} 件も、写しを含むことがあるため一緒に忘れます。'],
  forget_preview_written: ['The {written} memories written with those cards lose their text until they are curated again.', 'それらのカードと一緒に作られた記憶 {written} 件は、作り直されるまで文章が消えます。'],
  forget_preview_again: ['It replaced {again} older memories; they become current again unless another memory replaces them.', 'この記憶が置き換えていた古い記憶 {again} 件は、ほかに置き換えるものがなければ、再び有効になります。'],
  forget_claim_records: ['The records it was made from stay, and search still finds them: forget the prompt or the conversation when the text itself must go.', 'もとになった記録は残り、検索でも見つかります。文章そのものを消すには、そのプロンプトか会話を忘れてください。'],
  forget_note_outside: ["The agent's own transcript of the conversation that saved it keeps a copy.", 'これを保存した会話の、エージェント自身の会話ファイルには写しが残ります。'],
  forget_outside: ["The agent's own transcript files keep their copy: delete it there too if it must go.", 'エージェント自身の会話ファイルには写しが残ります。必要ならそちらも消してください。'],
  forget_limits: ['What was already given to an agent or sent to a curator is not taken back, and a conversation that starts during the cleaning may get only part of its memory.', 'すでにエージェントに渡した内容や要約役に送った内容は取り消せません。掃除の間に始まった会話には、記憶の一部しか渡らないことがあります。'],
  forget_irreversible: ['This cannot be undone.', 'この操作は取り消せません。'],
  forget_confirm: ['Forget', '忘れる'],
  forget_cancel: ['Cancel', 'やめる'],
  forget_done: ['Forgotten. The stores are cleaned in the background.', '忘れました。保存先の掃除は裏で続きます。'],
  forget_done_waiting: ['Forgotten. Its cleaning starts after the one already running.', '忘れました。保存先の掃除は、いま動いている掃除が終わってから始まります。'],
  forget_done_later: ['Forgotten. Its cleaning could not be started now: the worker starts it at its next look, or run oboete forget --continue.', '忘れました。保存先の掃除は今は始められませんでした。ワーカーが次に見回ったときに始めます(すぐ始めるには oboete forget --continue を実行してください)。'],
  forget_not_logged: ['The request could not be written to every log: raw.db holds it, but a restore from the backups alone would bring the text back.', '忘れる依頼をすべての控えには書けませんでした。raw.db には残っていますが、バックアップだけから戻すと、忘れた内容が戻ります。'],
  forget_stale: ['It changed since the preview. Press Forget again to see it.', '確認のあとで内容が変わりました。もう一度「忘れる」を押して確かめてください。'],
  forget_item: ['This cannot be forgotten from here.', 'これはここからは忘れられません。'],
  forget_too_large: ['This takes too many records to forget from here: forget its whole conversation instead.', '一度に忘れる記録が多すぎます。会話ごと忘れてください。'],
  forget_other_device: ['This prompt was recorded by another device: forget its whole conversation instead.', 'このプロンプトは別の端末で記録されたものです。会話ごと忘れてください。'],
  forget_preview_failed: ['What it would forget could not be read.', '忘れる内容を読めませんでした。'],
  forget_failed: ['It was not forgotten.', '忘れられませんでした。'],
  forget_confirmation: ['It was not forgotten.', '忘れられませんでした。'],
  hide_button: ['Remove from the page', 'ページから外す'],
  hide_done: ['Removed from the page. Undo it here, or later under Settings, Removed from the page.', 'ページから外しました。ここで元に戻すか、あとで設定の「ページから外したもの」から戻せます。'],
  hide_undo: ['Undo', '元に戻す'],
  hide_restored: ['Brought back to the page.', 'ページに戻しました。'],
  hide_pending: ['Recorded. The page shows it once the memory has caught up.', '受け付けました。記憶の更新が追いつくと、ページに反映されます。'],
  hide_item: ['This cannot be removed from here.', 'これはここからは外せません。'],
  hide_not_found: ['It is no longer there. Reload the page.', 'すでにありません。ページを読み込み直してください。'],
  hide_unavailable: ['The memory is busy. Try again in a moment.', '記憶が使用中です。少し待ってからもう一度お試しください。'],
  hidden_h: ['Removed from the page', 'ページから外したもの'],
  simple_hidden_h: ['Removed from the page', 'ページから外したもの'],
  simple_hidden_desc: ['Cards and summaries removed from the page, left out of session start, search and what agents are given until brought back.', 'ページから外したカードと要約です。戻すまで、会話の開始時・検索・エージェントに渡す内容にも出ません。'],
  hidden_desc: ['Cards and summaries removed from the page. Until brought back, they are left out of the page, session start, search and what agents are given. To erase one, forget its conversation or the prompt it came from.', 'ページから外したカードと要約です。戻すまで、ページ・会話の開始時・検索・エージェントに渡す内容に出ません。完全に消すには、その会話か、もとになったプロンプトを忘れてください。'],
  hidden_none: ['Nothing is removed.', '外したものはありません。'],
  hidden_restore: ['Bring back', '戻す'],
  hidden_unavailable: ['What was removed could not be read.', '外したものを読み込めませんでした。'],
  forget_key: ['It was not forgotten.', '忘れられませんでした。'],
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
  // Decision 64: what still stops the calls of a cap turned off.
  no_cap_paid_warning: ['With no cap, oboete does not stop paid calls by what they cost: only the limits you set with each service do. The caps of calls a day still apply.', '上限なしにすると、oboete は有料の呼び出しを金額では止めません。金額で止めるのは、各サービス側で決めた上限だけです。1 日の回数の上限は、そのまま効きます。'],
  no_cap_calls_warning: ['With no cap, oboete does not stop these calls by their number a day. The monthly limit on paid calls and each service\'s own limits still apply.', '上限なしにすると、oboete はこの呼び出しを 1 日の回数では止めません。有料の呼び出しの月額上限と、各サービス側の上限は、そのまま効きます。'],
  no_cap_row: ['Not stopped by its calls a day', '1 日の回数では止めません'],
  simple_no_cap_confirm: ['Remove the monthly limit', '確認して月額上限をなくす'],
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
  provider_endpoint_desc: ['Use HTTPS, or HTTP with a numeric loopback address such as 127.0.0.1. The server must support the API chosen below. Redirects are not followed by tests.', 'HTTPS、または 127.0.0.1 など数値のループバックアドレスの HTTP を指定します。サーバーは下で選ぶ API に対応している必要があります。接続テストではリダイレクトを追跡しません。'],
  provider_api: ['API', 'API の種類'],
  provider_api_openai: ['OpenAI-compatible', 'OpenAI 互換'],
  provider_api_anthropic: ['Anthropic Messages', 'Anthropic Messages'],
  provider_api_desc: ['Anthropic’s own Messages API returns JSON in the requested format; its OpenAI-compatible endpoint ignores that request. For Anthropic, choose Anthropic Messages and enter https://api.anthropic.com/v1 as the API base URL.', 'Anthropic 独自の Messages API は、指定した形式どおりの JSON を返します。Anthropic の OpenAI 互換の接続先では、この指定が無視されます。Anthropic を使う場合は「Anthropic Messages」を選び、API の接続先 URL に https://api.anthropic.com/v1 を入力してください。'],
  provider_enabled: ['Enable this entry', 'この要約役を有効にする'],
  provider_subscription: ['Covered by a subscription', 'サブスクリプション内の利用'],
  provider_credits: ['Past the plan’s usage limits, use the account’s credits', 'プランの利用上限を超えたら、アカウントのクレジットを使う'],
  provider_credits_desc: ['Off by default. When it is on, curation goes on past the plan’s usage limits while the account has credits, as Codex counts them, and stops when they run out. With automatic reload on, OpenAI buys credits with the card on file, so check automatic reload in ChatGPT (Settings > Usage) first.', '既定ではオフです。オンにすると、プランの利用上限を超えても、Codex が数えるクレジットがアカウントにある間は要約を続け、なくなったら止めます。自動チャージ（automatic reload）がオンの場合は登録済みのカードでクレジットが購入されるため、先に ChatGPT の「設定 > 使用量」で自動チャージを確認してください。'],
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
  agent_inventory_desc: ['This checks saved files only. A launch file does not prove login, executable permissions or live use. Use Run diagnostics below for the current read-only checks. Review connection changes in Connect or disconnect agents.', 'ここでは保存されたファイルだけを確認します。起動用ファイルが見つかっても、ログイン、実行権限、実際の利用は確認できません。読み取り専用の確認は下の「診断を実行」で行えます。接続の変更は「エージェントの接続と解除」で確認してください。'],
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
  agent_setup_h: ['Connect or disconnect agents', 'エージェントの接続と解除'],
  agent_setup_desc: ['Select agents, review the exact file and integration effects, then confirm. This does not test login or live use.', '対象を選び、ファイルと連携への影響を確認してから承認します。ログインや実際の利用は検証しません。'],
  agent_setup_action: ['Action', '操作'],
  agent_setup_wire: ['Connect', '接続'],
  agent_setup_unwire: ['Disconnect', '解除'],
  agent_setup_select: ['Agents', 'エージェント'],
  agent_setup_preview: ['Preview selected changes', '選択した変更を確認'],
  agent_setup_previewing: ['Preparing preview…', '変更内容を確認中…'],
  agent_setup_preview_h: ['Changes to approve', '承認する変更'],
  agent_setup_no_steps: ['No completed step has been reported for this agent', 'このエージェントで確定した処理はまだありません'],
  agent_setup_step: ['{component}: {effect}; backup {backup}.', '{component}：{effect}。バックアップ：{backup}。'],
  agent_setup_step_result: ['{kind}: {outcome}.', '{kind}：{outcome}。'],
  agent_setup_activation: ['The agent reads these changes when it starts again. Disabled integrations stay disabled and required approvals remain. Live use remains unverified.', 'エージェントの次回起動時に設定が読み込まれます。無効になっている連携と承認待ちはそのままです。実際の利用は未確認です。'],
  agent_setup_plugin_note: ['A same-name plugin replacement or manual step is shown per component; inspect it before applying.', '同名プラグインの置き換えや手動作業は項目ごとに表示します。適用前に確認してください。'],
  agent_setup_consent: ['I approve the changes listed above.', '上に表示された変更を承認します。'],
  agent_setup_consent_native: ['I approve the listed changes, including the local Claude Code MCP command where shown. Claude Code may run configured startup hooks, which may have their own effects or model calls. oboete does not request a model probe.', '表示されたClaude Code MCPのローカルコマンドを含め、上の変更を承認します。Claude Codeの起動時フックが実行され、独自の変更やモデル呼び出しが起こる場合があります。oboeteはモデルの動作確認を要求しません。'],
  agent_setup_apply: ['Apply confirmed changes', '承認した変更を適用'],
  agent_setup_sending: ['Applying selected agent changes…', '選択したエージェントの連携変更を適用中…'],
  agent_setup_receipt_current_h: ['Current operation', '現在の操作'],
  agent_setup_receipt_previous_h: ['Previous operation', '前回の操作'],
  agent_setup_status: ['Inspect operation status', '操作状況を確認'],
  agent_setup_prepare: ['Prepare another operation', '別の操作を準備'],
  agent_setup_prepare_note: ['The previous result stays unknown. New changes need a fresh preview and approval; local commands may run again.', '前の操作は結果不明のまま残ります。新しい変更には再確認と承認が必要です。ローカルコマンドが再び実行される可能性があります。'],
  agent_setup_status_loading: ['Checking operation status…', '操作状況を確認中…'],
  agent_setup_status_none: ['No operation receipt is available.', '操作の結果はまだありません。'],
  agent_setup_receipt_h: ['Operation receipt', '操作結果'],
  agent_setup_receipt: ['{action}: {phase}.', '{action}：{phase}。'],
  agent_setup_no_selection: ['Select at least one agent.', 'エージェントを1つ以上選んでください。'],
  agent_setup_preview_failed: ['A safe preview was not available. Nothing was applied.', '安全な確認結果を得られませんでした。変更は適用していません。'],
  agent_setup_start_failed: ['The operation did not complete. Inspect its status before trying another.', '操作を完了できませんでした。次の操作の前に状況を確認してください。'],
  agent_setup_status_unavailable: ['Operation status could not be read.', '操作状況を読み取れませんでした。'],
  agent_setup_stale: ['The preview is stale. Prepare a new preview.', '確認した変更内容は古くなりました。もう一度確認してください。'],
  agent_setup_busy: ['Another operation is active. Inspect its status.', '別の操作が進行中です。状況を確認してください。'],
  agent_setup_unknown: ['The result may have been committed. Inspect operation status; it will not retry automatically.', '変更が確定している可能性があります。操作状況を確認してください。自動では再実行しません。'],
  agent_setup_unknown_value: ['unknown', '不明'],
  agent_setup_component_hooks: ['Hooks', 'フック'],
  agent_setup_component_mcp: ['MCP', 'MCP'],
  agent_setup_component_trust: ['Hook trust', 'フックの信頼設定'],
  agent_setup_component_plugin: ['Plugin', 'プラグイン'],
  agent_setup_component_extension: ['Extension', '拡張機能'],
  agent_setup_effect_create: ['create', '作成'],
  agent_setup_effect_replace: ['replace', '置き換え'],
  agent_setup_effect_delete: ['delete', '削除'],
  agent_setup_effect_unchanged: ['unchanged', '変更なし'],
  agent_setup_effect_manual: ['manual step', '手動作業'],
  agent_setup_effect_skipped: ['skipped', '見送り'],
  agent_setup_effect_unavailable: ['unavailable', '利用不可'],
  agent_setup_effect_native_command: ['local command', 'ローカルコマンド'],
  agent_setup_backup_create: ['create', '作成'],
  agent_setup_backup_kept: ['kept', '保持'],
  agent_setup_backup_none: ['none', 'なし'],
  agent_setup_phase_running: ['running', '実行中'],
  agent_setup_phase_complete: ['complete', '完了'],
  agent_setup_phase_partial: ['partly complete', '一部完了'],
  agent_setup_phase_failed: ['failed', '失敗'],
  agent_setup_phase_stale: ['stale', '古い確認結果'],
  agent_setup_phase_unknown: ['unknown', '結果不明'],
  agent_setup_outcome_committed: ['committed', '確定'],
  agent_setup_outcome_noop: ['no change', '変更なし'],
  agent_setup_outcome_manual: ['manual step needed', '手動作業が必要'],
  agent_setup_outcome_skipped: ['skipped', '見送り'],
  agent_setup_outcome_failed: ['failed', '失敗'],
  agent_setup_outcome_stale: ['stale', '古い状態'],
  agent_setup_outcome_unknown: ['unknown', '結果不明'],
  agent_setup_kind_backup: ['backup', 'バックアップ'],
  agent_setup_kind_write: ['write', '書き込み'],
  agent_setup_kind_delete: ['delete', '削除'],
  agent_setup_kind_native_remove: ['local removal', 'ローカルの解除'],
  agent_setup_kind_native_add: ['local addition', 'ローカルの追加'],
  agent_setup_kind_readback: ['readback', '再読取'],
  agent_setup_kind_stage: ['prepare change', '変更の準備'],
  agent_setup_outcome_prepared: ['prepared', '準備済み'],
  agent_setup_code_manual_jsonc: ['Commented configuration is preserved. This component needs the agent’s own setup.', 'コメント付きの設定は保持します。この項目はエージェント側の設定が必要です。'],
  agent_setup_code_manual_json: ['The existing file could not be safely updated and is preserved.', '既存ファイルを安全に更新できないため、そのまま保持します。'],
  agent_setup_code_foreign_extension_kept: ['The existing unmarked extension is preserved.', 'oboeteの印がない既存の拡張機能は保持します。'],
  agent_setup_code_agent_missing: ['The agent was not found; this integration is skipped.', 'エージェントが見つからないため、この連携は見送ります。'],
  agent_setup_code_launcher_missing: ['The Claude MCP launcher could not be verified. Hook changes are shown separately.', 'Claude MCPの起動用ファイルを確認できません。フックの変更は別に表示します。'],
  agent_setup_code_unavailable_file: ['This file could not be assessed and is preserved.', 'このファイルは状態を確認できないため保持します。'],
  agent_setup_code_unsupported_command: ['Antigravity and Cursor on Windows need installation and memory folder paths without spaces or shell metacharacters.', 'WindowsのAntigravityとCursorでは、インストール先と記憶フォルダーのパスに空白やシェルの特殊文字を使えません。'],
  agent_setup_code_prior_unknown: ['An earlier local command has an unknown result. Later steps were left untouched.', '先のローカルコマンドの結果が不明なため、後続の処理は行っていません。'],
  agent_setup_code_mcp_arguments_too_large: ['The local MCP registration command exceeds its safe size budget. Existing settings are preserved.', 'ローカルMCP登録コマンドの安全なサイズ上限を超えるため、既存設定は保持します。'],
  agent_setup_code_manual_mcp: ['This existing Claude MCP registration is preserved. Change it in Claude’s own setup so saved credentials never enter process arguments.', '既存のClaude MCP登録は保持します。保存済みの秘密値をプロセス引数へ渡さないため、変更はClaude側の設定で行ってください。'],
  doctor_h: ['Run diagnostics', '診断を実行'],
  doctor_desc: ['Run these read-only checks only when you choose. They do not repair files, contact agents or call a model.', '選んだときだけ読み取り専用の診断を実行します。ファイルの修復、エージェントへの接続、モデルの呼び出しは行いません。'],
  doctor_run: ['Run diagnostics now', '今すぐ診断'],
  doctor_running: ['Checking saved data…', '保存済みデータを確認中…'],
  doctor_not_run: ['No diagnostics have run in this viewer session.', 'このビューアのセッションでは、まだ診断を実行していません。'],
  doctor_busy: ['The viewer is busy. No diagnostic result was returned. Try again after the current operation finishes.', 'ビューアは別の処理中です。診断結果は返っていません。現在の処理が終わってから再実行してください。'],
  doctor_failed: ['Diagnostics could not be completed. No result is available.', '診断を完了できませんでした。結果はありません。'],
  doctor_unknown: ['The response was lost. No result is available; the check will not restart automatically.', '応答が失われたため結果は不明です。診断は自動では再実行されません。'],
  doctor_incomplete: ['These diagnostics are incomplete. They do not say the system is healthy.', 'この診断には未確認の項目があります。システム全体が正常という意味ではありません。'],
  doctor_remaining: ['This version does not implement every diagnostic check.', 'この版では、まだ実装されていない診断項目があります。'],
  doctor_unhealthy_h: ['Confirmed issues', '確認された問題'],
  doctor_unhealthy_recording_failed: ['Recording failed', '記録に失敗しています'],
  doctor_unhealthy_worker_failed: ['The resident worker last failed', '常駐ワーカーの直近の処理が失敗しました'],
  doctor_unhealthy_low_free_space: ['Available storage is low', '空き容量が少なくなっています'],
  doctor_unhealthy_raw_damaged: ['Saved records are damaged', '保存された記録が破損しています'],
  doctor_unhealthy_knowledge_damaged: ['Search information is damaged', '検索用情報が破損しています'],
  doctor_unhealthy_legacy_damaged: ['Older history is damaged', '旧版の履歴が破損しています'],
  doctor_unhealthy_providers_damaged: ['Provider history is damaged', '要約役の利用履歴が破損しています'],
  doctor_unhealthy_backup_checksum_missing: ['A backup checksum is missing', 'バックアップの照合情報が欠けています'],
  doctor_unhealthy_backup_checksum_mismatch: ['A backup checksum does not match', 'バックアップの照合情報が一致しません'],
  doctor_unhealthy_config_invalid: ['Saved configuration is invalid', '保存済み設定が不正です'],
  doctor_unhealthy_other: ['Another confirmed issue was reported', 'その他の確認済みの問題があります'],
  doctor_source_changed: ['Saved sources changed during this check. The numbers may describe different moments. Run diagnostics again explicitly after activity settles.', '診断中に保存データが変わりました。件数が異なる時点のものかもしれません。処理が落ち着いてから明示的に再実行してください。'],
  doctor_source: ['Sources during this check: {state}', '診断中の保存データ：{state}'],
  doctor_agents_h: ['Agent files in this diagnostic check', '今回の診断で確認したエージェント設定'],
  doctor_files_h: ['Saved files', '保存ファイル'],
  doctor_file: ['{name}: main file {file}; pending changes file {wal}.', '{name}：本体{file}、未統合の変更ファイル{wal}。'],
  doctor_file_raw: ['Current records', '現在の記録'],
  doctor_file_raw_restored: ['Retained record recovery copy', '保持された記録の復旧用コピー'],
  doctor_file_knowledge: ['Search information', '検索用情報'],
  doctor_file_legacy: ['Older history', '旧版の履歴'],
  doctor_file_providers: ['Provider history', '要約役の利用履歴'],
  doctor_raw_h: ['Recorded activity', '記録された活動'],
  doctor_raw: ['Record checks: {state}. Highest record position {records}; highest change position {ops}; curated through {curated}.', '記録の確認：{state}。レコードの最終位置{records}、変更履歴の最終位置{ops}、要約処理済み位置{curated}。'],
  doctor_parking: ['{source}: kept aside {parked}; waiting {waiting}.', '{source}：送信せず保持{parked}件、待機中{waiting}件。'],
  doctor_parking_unknown: ['Import waiting state: {state}', '取り込み履歴の待機状態：{state}'],
  doctor_source_v1: ['Older oboete history', '旧 oboete の履歴'],
  doctor_source_transcript: ['Agent transcripts', 'エージェントの会話履歴'],
  doctor_source_other: ['Other sources', 'その他の元データ'],
  doctor_knowledge_h: ['Search information', '検索用情報'],
  doctor_knowledge: ['Search checks: {state}. Prior rewinds {rewinds}; missing session coverage {missing}; shortened coverage {short}.', '検索用情報の確認：{state}。以前の巻き戻し{rewinds}回、欠けたセッション範囲{missing}件、短くなった範囲{short}件。'],
  doctor_rewind_time: ['Most recent rewind: {time}', '最後の巻き戻し：{time}'],
  doctor_time_none: ['none recorded', '記録なし'],
  doctor_gap_row: ['{agent}: ended {ended}, checked {checked}, shortened {short}, missing {missing}.', '{agent}：終了{ended}件、確認済み{checked}件、短縮{short}件、欠落{missing}件。'],
  doctor_gap_other: ['Other agents', 'その他のエージェント'],
  doctor_legacy_h: ['Older history', '旧版の履歴'],
  doctor_legacy: ['Older history checks: {state}. Sessions {sessions}; events {events}; observations {observations}; summaries {summaries}; events still to import {remaining}.', '旧版履歴の確認：{state}。セッション{sessions}件、イベント{events}件、観測{observations}件、要約{summaries}件、未取り込みイベント{remaining}件。'],
  doctor_legacy_recent: ['Recent older call records: {count}', '旧版の最近の呼び出し記録：{count}件'],
  doctor_recent_row: ['Result {outcome}; duration {ms} ms.', '結果{outcome}、所要時間{ms}ミリ秒。'],
  doctor_outcome_ok: ['completed', '完了'],
  doctor_outcome_error: ['error', 'エラー'],
  doctor_outcome_invalid: ['invalid answer', '不正な応答'],
  doctor_outcome_wait: ['waited', '待機'],
  doctor_outcome_empty: ['empty answer', '空の応答'],
  doctor_outcome_timeout: ['timed out', '時間切れ'],
  doctor_outcome_sent: ['sent; settlement unknown', '送信済み・精算不明'],
  doctor_outcome_reserved: ['reserved', '予約済み'],
  doctor_outcome_prose: ['unusable prose', '利用できない文章'],
  doctor_outcome_shape: ['invalid format', '形式が不正'],
  doctor_outcome_over_cap: ['output limit exceeded', '出力上限超過'],
  doctor_outcome_unanchored: ['evidence missing', '根拠が不足'],
  doctor_outcome_budget: ['budget refused', '予算により拒否'],
  doctor_outcome_gate: ['privacy gate refused', 'プライバシー確認で拒否'],
  doctor_outcome_too_big: ['request too large', '要求が大きすぎます'],
  doctor_outcome_other: ['other result', 'その他の結果'],
  doctor_embedding_h: ['Embedding work', '埋め込み処理'],
  doctor_embedding: ['Embedding checks: {state}. Generation {generation}; waiting claims {claims}, cards {cards}, session summaries {summaries}, imported items {imports}, records {records}; skipped items {skipped}.', '埋め込み処理の確認：{state}。世代{generation}、待機中の記憶{claims}件・カード{cards}件・セッションの要約{summaries}件・取り込み項目{imports}件・記録{records}件、処理を見送った項目{skipped}件。'],
  doctor_skipped_row: ['{reason}: {count} skipped.', '{reason}：見送り{count}件。'],
  doctor_skip_excluded: ['excluded', '除外'],
  doctor_skip_held: ['held back', '保留'],
  doctor_skip_refused: ['refused', '拒否'],
  doctor_skip_empty: ['empty', '空'],
  doctor_skip_other: ['other reason', 'その他の理由'],
  doctor_generation_active: ['active', '使用中'],
  doctor_generation_other: ['another state', '別の状態'],
  doctor_generation_none: ['none recorded', '記録なし'],
  doctor_providers_h: ['Provider history', '要約役の利用履歴'],
  doctor_providers: ['Provider history checks: {state}. Recorded curation amount {curation}, including the pending reservation bound {reserved}; embedding/search amount {embedding}; waiting requests {pending}. Do not add the reservation bound again.', '要約役の利用履歴の確認：{state}。記録された要約処理の金額{curation}には、保留中の予約上限額{reserved}が含まれます。埋め込み・検索の金額{embedding}、待機中の要求{pending}件。予約額をもう一度加算しないでください。'],
  doctor_provider_usage: ['In the last 24 hours: recorded calls {calls}, recorded tokens {tokens}, reserved calls {reserved}, reserved token bounds {reserved_tokens}. Cached key-limit entries: {key_cache}.', '過去24時間：記録された呼び出し{calls}回、記録されたトークン{tokens}、予約中の呼び出し{reserved}回、予約中トークンの上限{reserved_tokens}。保存済みキー上限の履歴：{key_cache}件。'],
  doctor_key_unchecked: ['The cached key limits are historical. Association with current keys was not checked.', '保存済みのキー上限は過去の情報です。現在のキーとの対応は確認していません。'],
  doctor_key_unknown: ['Current key association could not be confirmed.', '現在のキーとの対応は確認できません。'],
  doctor_provider_holds: ['Provider holds: owner {owner}; temporary {resting}. Recent call records {recent}.', '要約役の停止：利用者による停止{owner}件、一時停止{resting}件。最近の呼び出し記録{recent}件。'],
  doctor_isolation: ['Past isolated-run records: passed {passed}; failed {failed}. These are history, not a current isolation test.', '過去の分離実行記録：成功{passed}件、失敗{failed}件。履歴であり、現在の分離状態のテストではありません。'],
  doctor_isolation_unknown: ['Current isolation was not checked.', '現在の分離状態は確認していません。'],
  doctor_backup_h: ['Backups', 'バックアップ'],
  doctor_backup: ['Backup checks: {state}. Record segments {records}; change-history segments {ops}; covered through record {through}; change position {through_ops}.', 'バックアップの確認：{state}。記録区間{records}件、変更履歴区間{ops}件、対象レコードの最終位置{through}、変更履歴の最終位置{through_ops}。'],
  doctor_backup_checksums: ['Missing checksums {missing}; mismatches {mismatch}. Known counts can remain visible even when another backup check fails.', '照合情報の欠落{missing}件、不一致{mismatch}件。別のバックアップ確認に失敗しても、確認済みの件数は表示します。'],
  doctor_runtime_h: ['Recording and resident processes', '記録と常駐処理'],
  doctor_runtime_state: ['Runtime-file checks: {state}', '実行状態ファイルの確認：{state}'],
  doctor_recording_clear: ['No current recording failure marker was found.', '現在の記録失敗マーカーは見つかりません。'],
  doctor_recording_failed: ['A recording failure was recorded: {cause}. First seen around {since}.', '記録失敗が報告されています：{cause}。最初の記録はおよそ{since}。'],
  doctor_recording_unknown: ['Recording failure state: {state}', '記録失敗の状態：{state}'],
  doctor_failure_disk_full: ['storage full', '空き容量不足'],
  doctor_failure_io: ['read/write error', '読み書きの失敗'],
  doctor_failure_busy: ['store busy', '保存先が使用中'],
  doctor_failure_other: ['other failure', 'その他の失敗'],
  doctor_worker: ['Resident worker: saved choice {configured}; supported {supported}; running {running}; last report {last}.', '常駐ワーカー：保存済みの選択{configured}、対応{supported}、稼働中{running}、直近の結果{last}。'],
  doctor_viewer: ['Resident viewer: saved port {configured}; running {running}; observed port {actual}; last report {last}.', '常駐ビューア：保存済みポート{configured}、稼働中{running}、確認できたポート{actual}、直近の結果{last}。'],
  doctor_restore_note: ['Restore note present: {value}', '復元の記録：{value}'],
  doctor_yes: ['yes', 'はい'],
  doctor_no: ['no', 'いいえ'],
  doctor_last_clean: ['completed cleanly', '正常に終了'],
  doctor_last_running: ['in progress', '処理中'],
  doctor_last_interrupted: ['interrupted', '中断'],
  doctor_last_failed: ['failed', '失敗'],
  doctor_last_up: ['reported listening', '待ち受けを報告'],
  doctor_last_other_port: ['another port', '別のポート'],
  doctor_last_stopped: ['stopped', '停止'],
  doctor_last_starting: ['starting', '起動中'],
  doctor_last_port_in_use: ['port in use', 'ポートが使用中'],
  doctor_last_not_private: ['not private', '非公開設定ではありません'],
  doctor_disk_h: ['Free space', '空き容量'],
  doctor_disk: ['Free-space check: {state}. Available bytes {bytes}; low-space warning {low}.', '空き容量の確認：{state}。利用可能な容量{bytes}バイト、空き容量不足の注意{low}。'],
  doctor_quota_h: ['Embedding request limits', '埋め込み要求の上限'],
  doctor_quota: ['Saved/local quota facts: {state}. Document requests per day {documents}; query requests per day {queries}; monthly spending limit {cap}; recorded calls in the last 24 hours {calls}. No live provider quota was queried.', '保存済み・ローカルの上限情報：{state}。文書の1日あたりの要求上限{documents}件、検索の要求上限{queries}件、月間金額上限{cap}、過去24時間の記録済み呼び出し{calls}件。要約役の実際の残量は照会していません。'],
  doctor_quota_history: ['Cached pause: {rest}; last recorded embedding error: {error}.', '保存済みの一時停止状態：{rest}、最後に記録された埋め込みエラー：{error}。'],
  doctor_rest_ready: ['no pause recorded', '停止記録なし'],
  doctor_rest_resting: ['temporarily paused', '一時停止中'],
  doctor_rest_owner_hold: ['held by the owner', '利用者が停止中'],
  doctor_error_none: ['none recorded', '記録なし'],
  doctor_error_embed: ['document embedding', '文書の埋め込み'],
  doctor_error_query: ['search query', '検索要求'],
  doctor_error_other: ['other request', 'その他の要求'],
  doctor_error_at: ['{role} around {time}', '{time}頃の{role}'],
  doctor_retained_h: ['Retained older files and evaluation copies', '保持された旧ファイルと評価用コピー'],
  doctor_retained: ['Retained-file metadata: {state}. File contents were not read for this summary.', '保持ファイルの情報：{state}。この概要ではファイル内容を読み取っていません。'],
  doctor_retained_row: ['{name}: present {present}; top-level targets {targets}; logical bytes {bytes}.', '{name}：存在{present}、最上位の対象{targets}件、論理サイズ{bytes}バイト。'],
  doctor_retained_eval: ['Evaluation copies: present {present}; logical bytes {bytes}.', '評価用コピー：存在{present}、論理サイズ{bytes}バイト。'],
  doctor_retained_v1_database: ['Older oboete database', '旧 oboete データベース'],
  doctor_retained_memory_database: ['Older memory database', '旧版の記憶データベース'],
  doctor_retained_snapshot: ['Older snapshots', '旧版のスナップショット'],
  doctor_retained_rollout_directory: ['Older rollout folders', '旧版の処理フォルダー'],
  doctor_retained_spool: ['Older spool', '旧版の一時保存'],
  doctor_retained_cache: ['Older cache', '旧版のキャッシュ'],
  doctor_retained_logs: ['Older logs', '旧版のログ'],
  doctor_state_known: ['measured', '確認済み'],
  doctor_state_off: ['turned off', '無効'],
  doctor_state_invalid: ['invalid', '不正'],
  doctor_state_absent: ['absent', 'なし'],
  doctor_state_schema_missing: ['data format unavailable', '必要なデータ形式がありません'],
  doctor_state_damaged: ['damaged', '破損'],
  doctor_state_unreadable: ['unreadable', '読み取り不可'],
  doctor_state_busy: ['busy', '使用中で確認不可'],
  doctor_state_changed: ['changed during check', '確認中に変更'],
  doctor_state_unavailable: ['unavailable', '確認不可'],
  doctor_state_present: ['present', 'あり'],
  doctor_state_unknown: ['unknown', '不明'],
  file_error: [
    'Saved configuration has a mistake. Preview safe recovery below, or use Run diagnostics for read-only facts.',
    '保存済み設定に誤りがあります。下で安全な復旧内容を確認するか、「診断を実行」で読み取り専用の項目を確認してください。',
  ],
  recovery_h: ['Recover invalid settings', '壊れた設定を復旧する'],
  recovery_desc: ['Keep a private copy of the current file, then replace all settings. Saved AI use, paid usage, resident mode, prompt text storage and memory delivery will be off. Already admitted work may finish. Other events still use built-in redaction; custom rules and other settings return to defaults. Records, agent files and saved keys are kept.', '現在のファイルを非公開のコピーに保存してから、設定全体を置き換えます。AI・有料利用・常駐・入力文の保存・記憶の配信をオフに設定します。すでに開始済みの処理は完了する場合があります。その他の記録は組み込みの秘匿ルールを使い、独自ルールや設定は初期値に戻ります。記録済みデータ・エージェントの設定・保存済みキーは保持します。'],
  recovery_preview: ['Preview recovery', '復旧内容を確認'],
  recovery_confirm: ['I agree to keep the current file and replace all settings as described.', '現在のファイルを保存し、説明された内容で設定全体を置き換えることに同意します。'],
  recovery_start: ['Save a copy and recover', 'コピーを保存して復旧'],
  recovery_copy: ['Original-copy filename (saved before replacement): {name}', '書き換え前に保存した原本コピーのファイル名：{name}'],
  recovery_unknown: ['The result is unknown. Check the saved settings before preparing another recovery; do not resend automatically.', '結果を確認できません。自動で再送せず、保存済み設定を確認してから改めて復旧内容を確認してください。'],
  recovery_inspect: ['Check saved settings', '保存済み設定を確認'],
  recovery_unavailable: ['Recovery needs a bounded regular writable file and verified owner-only Linux storage. It is unavailable for this file or platform; nothing was reset.', '復旧には上限内の通常ファイルと書き込み権限、Linux 上で確認できる本人専用の保存先が必要です。このファイルまたは環境では実行できず、初期化していません。'],
  recovery_failed: ['Recovery stopped. The current settings or their private recoverable copy are retained. Check saved settings before trying a fresh preview.', '復旧を停止しました。現在の設定、または復元可能な非公開のコピーを保持しています。保存済み設定を確認してから、改めて復旧内容を確認してください。'],
  recovery_confirmation: ['Preview recovery and agree to its effects first.', '復旧内容を確認し、その影響に同意してください。'],
  recovery_stale: ['Recovery preview is no longer current. Check the saved settings and prepare a fresh preview.', '復旧の確認内容が最新ではありません。保存済み設定を確認し、改めて復旧内容を確認してください。'],
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
  dispatch_busy: ['A call to an AI service is still being sent, so the masking settings were not saved. Save again in a moment.', 'AI サービスへの送信が続いているため、マスキングの設定を保存できませんでした。少し待ってからもう一度保存してください。'],
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
  group_search_h: ['Search by meaning', '意味での検索'],
  group_search_desc: ['How memories are also found by what they mean, and where that work runs.', '記憶を意味でも探すための処理と、その処理を行う場所です。'],
  embedding_h: ['Search by meaning (embeddings)', '意味での検索（埋め込み）'],
  embedding_desc: ['oboete turns each memory into numbers (a vector), so a search also finds memories close in meaning. Full-text search works with every method.', 'oboete は各記憶を数値（ベクトル）に変換し、意味の近い記憶も検索で見つかるようにします。どの方法を選んでも全文検索は使えます。'],
  embedding_choice: ['Method', '方法'],
  embedding_none: ['Off (full-text search only)', '使わない（全文検索のみ）'],
  embedding_local: ['On this computer (bge-m3)', 'このパソコンで処理（bge-m3）'],
  embedding_workers_ai: ['Cloudflare Workers AI (bge-m3)', 'Cloudflare Workers AI（bge-m3）'],
  embedding_none_desc: ['Nothing is sent anywhere to be turned into vectors. The vectors made so far stay stored.', 'ベクトルへの変換のための送信は行いません。これまでに作ったベクトルは保存したままにします。'],
  embedding_local_desc: ['bge-m3 runs on this computer. It needs a one-time download of about 2.3 GB; after that, no text or search leaves this computer to be turned into vectors.', 'bge-m3 をこのパソコンで動かします。最初に一度だけ約 2.3 GB のダウンロードが必要です。その後は、ベクトルへの変換のために文章や検索語が外部へ送られることはありません。'],
  embedding_workers_ai_desc: ['Each memory’s text (after masking) and each search query are sent to Cloudflare Workers AI under your account, within the limits set below.', '各記憶の文章（マスキング後）と検索語を、あなたのアカウントの Cloudflare Workers AI に送ります。送信は下で設定する上限の範囲内で行います。'],
  embedding_files_ready: ['The model’s files are ready in {dir} ({size}).', 'モデルのファイルは {dir} に準備できています（{size}）。'],
  embedding_files_not_downloaded: ['The model has not been downloaded yet.', 'モデルはまだダウンロードしていません。'],
  embedding_files_incomplete: ['The model’s download is incomplete ({size} so far, in {dir}). Choosing this method again resumes it.', 'モデルのダウンロードが途中です（{dir} に {size}）。もう一度この方法を選ぶと続きから再開します。'],
  embedding_files_changed: ['The model’s files in {dir} changed after they were checked. Choosing this method again checks them.', '{dir} にあるモデルのファイルが、検証後に変更されています。もう一度この方法を選ぶと検証し直します。'],
  embedding_unavailable_no_feature: ['This oboete was built without on-computer embeddings, so this method cannot be used.', 'この oboete はパソコン上での処理を含めずにビルドされているため、この方法は使えません。'],
  embedding_unavailable_no_runtime: ['ONNX Runtime is not provided for this computer, so this method cannot be used here.', 'このパソコン向けの ONNX Runtime が提供されていないため、ここではこの方法を使えません。'],
  embedding_preview: ['Review this method', '内容を確認'],
  embedding_confirm: ['I agree to the above.', '上記の内容に同意します。'],
  embedding_start: ['Use this method', 'この方法にする'],
  embedding_consent_none_files: ['The model’s files stay in {dir} ({size}); oboete does not delete them.', 'モデルのファイルは {dir} に残します（{size}）。oboete が削除することはありません。'],
  embedding_consent_workers_ai: ['Each memory’s text (after masking) and each search query will be sent to Cloudflare Workers AI (bge-m3) under account {account}, using the token in {file}: at most {requests} requests a day, and at most {usd} a month beyond the free allowance.', '各記憶の文章（マスキング後）と検索語を、アカウント {account} の Cloudflare Workers AI（bge-m3）に、{file} のトークンを使って送ります。上限は 1 日 {requests} 回、無料枠を超える費用は月 {usd} までです。'],
  embedding_consent_get: ['{size} will be downloaded from {host} ({source}).', '{host} から {size} をダウンロードします（{source}）。'],
  embedding_consent_into: ['The files go into {dir}, and each one is checked against the SHA-256 value built into oboete.', 'ファイルは {dir} に保存し、それぞれを oboete に組み込まれた SHA-256 値で検証します。'],
  embedding_consent_check: ['The files already in {dir} will be checked against the SHA-256 values built into oboete; a file that does not match is downloaded again.', '{dir} にあるファイルを、oboete に組み込まれた SHA-256 値で検証します。一致しないファイルはダウンロードし直します。'],
  embedding_consent_local: ['After that, no text or search leaves this computer to be turned into vectors.', 'その後は、ベクトルへの変換のために文章や検索語が外部へ送られることはありません。'],
  embedding_consent_ready: ['bge-m3 runs from the files in {dir}, which were checked when they were downloaded. No text or search leaves this computer to be turned into vectors.', '{dir} のファイル（ダウンロード時に検証済み）で bge-m3 を動かします。ベクトルへの変換のために文章や検索語が外部へ送られることはありません。'],
  embedding_source_model: ['BAAI’s bge-m3, MIT License', 'BAAI の bge-m3、MIT ライセンス'],
  embedding_source_runtime: ['Microsoft’s ONNX Runtime 1.28.0, MIT License', 'Microsoft の ONNX Runtime 1.28.0、MIT ライセンス'],
  embedding_running: ['Downloading the model: {done} of {total}. Other settings can be saved meanwhile.', 'モデルをダウンロードしています（{done} / {total}）。その間もほかの設定は保存できます。'],
  embedding_checking: ['Checking the model’s files. Other settings can be saved meanwhile.', 'モデルのファイルを検証しています。その間もほかの設定は保存できます。'],
  embedding_done: ['The method was changed. Background processing uses it from its next run.', '方法を切り替えました。次回のバックグラウンド処理から使われます。'],
  embedding_failed: ['The download stopped, and the method was not changed. The files downloaded so far are kept: choose the method again to resume. Running `oboete setup --embeddings local` in a terminal shows the details.', 'ダウンロードが止まったため、方法は変更していません。ここまでのファイルは保持しているので、もう一度選ぶと続きから再開します。ターミナルで `oboete setup --embeddings local` を実行すると詳細を確認できます。'],
  embedding_busy: ['Another oboete is downloading or checking the model, or another change of method is running. Try again when it ends.', '別の oboete がモデルのダウンロードや検証を行っているか、別の切り替えを実行中です。終わってからもう一度お試しください。'],
  embedding_no_space: ['There is not enough free disk space for the model (about 2.3 GB). Free some space, then choose the method again.', 'モデルを保存する空き容量（約 2.3 GB）が足りません。空きを作ってから、もう一度選んでください。'],
  embedding_token: ['The page’s token changed during the download, so the method was not changed. The files are kept: choose the method again.', 'ダウンロード中に画面のトークンが変わったため、方法は変更していません。ファイルは保持しているので、もう一度選んでください。'],
  embedding_write_failed: ['The files are ready, but the method could not be saved. Check config.toml, then try again.', 'ファイルは準備できましたが、方法を保存できませんでした。config.toml を確認してから、もう一度お試しください。'],
  embedding_unknown: ['The result could not be confirmed. Check the saved method before choosing again.', '結果を確認できませんでした。保存済みの方法を確認してから、もう一度選んでください。'],
  embedding_stale: ['The settings or the model’s files changed after the review. Review the method again.', '確認したあとで設定またはモデルのファイルが変わりました。もう一度内容を確認してください。'],
  embedding_no_account: ['Enter and save your Cloudflare account ID first.', '先に Cloudflare のアカウント ID を入力して保存してください。'],
  embedding_no_key: ['The Workers AI token file could not be read. Save a token first.', 'Workers AI のトークンのファイルを読み込めませんでした。先にトークンを保存してください。'],
  embedding_local_unavailable: ['On-computer embeddings cannot be used with this build or on this computer.', 'この構成またはこのパソコンでは、パソコン上での処理を使えません。'],
  embedding_confirmation: ['Review the method and agree to it first.', '先に内容を確認し、同意してください。'],
  embedding_request_failed: ['The request could not be completed. Please try again.', '処理を完了できませんでした。もう一度お試しください。'],
  embedding_index: ['This computer and Workers AI make the same vectors (bge-m3), so switching between them keeps every vector. Turning it off keeps them too.', 'このパソコンと Workers AI は同じベクトル（bge-m3）を作るため、切り替えてもベクトルはそのまま使えます。「使わない」にしてもベクトルは残ります。'],
  embedding_workers_ai_h: ['Cloudflare Workers AI settings', 'Cloudflare Workers AI の設定'],
  embedding_account: ['Account ID (32 characters)', 'アカウント ID（32 文字）'],
  embedding_account_destination: ['While Workers AI is the method, memories and searches are sent to the account saved here.', 'Workers AI を使っている間は、ここに保存したアカウントへ記憶と検索語を送ります。'],
  embedding_daily_requests: ['Requests a day, at most ({min}–{max})', '1 日のリクエスト数の上限（{min}〜{max}）'],
  embedding_monthly_usd: ['USD a month beyond the free allowance, at most (more than 0)', '無料枠を超える費用の月上限（USD、0 より大きい値）'],
  embedding_usage: ['This month: {usd}. Last 24 hours: {requests} requests.', '今月の費用: {usd}。直近 24 時間: {requests} 回。'],
  embedding_usage_unknown: ['Usage could not be read.', '利用状況を読み込めませんでした。'],
  embedding_key_file: ['Token: {state} ({file})', 'トークン: {state}（{file}）'],
  embedding_key_desc: ['Paste a Cloudflare API token limited to Workers AI. oboete chooses private storage, and the saved value is never shown. Saving sends nothing.', 'Workers AI 用に権限を絞った Cloudflare の API トークンを貼り付けます。専用の保存先は oboete が選び、保存した値は表示しません。保存時に送信は行いません。'],
  embedding_key_label: ['New Workers AI token', 'Workers AI の新しいトークン'],

  my_work: ['My Work', 'マイワーク'],
  my_work_eyebrow: ['Your workspace, at a glance', '作業の状況をひと目で'],
  my_work_subtitle: ['Your requests. What landed. What needs you.', 'あなたの依頼と、終わったこと、あなたの対応が必要なこと。'],
  my_work_view_group: ['Work view', '表示の切り替え'],
  my_work_cards: ['Cards', 'カード'],
  my_work_list: ['List', '一覧'],
  my_work_agents: ['Agents', 'エージェント'],
  my_work_search_label: ['Search requests and agents', '依頼とエージェントを検索'],
  my_work_search_placeholder: ['Search requests or agents', '依頼やエージェントを検索'],
  my_work_project_label: ['Filter by project', 'プロジェクトで絞り込む'],
  my_work_all_projects: ['All projects', 'すべてのプロジェクト'],
  my_work_range_label: ['Time range', '期間'],
  my_work_range_day: ['Past 24 hours', '過去 24 時間'],
  my_work_range_week: ['Past 7 days', '過去 7 日間'],
  my_work_range_all: ['All recorded work', '記録されたすべての作業'],
  my_work_sort_label: ['Sort requests', '依頼の並び順'],
  my_work_sort_attention: ['Attention first', '対応が必要な順'],
  my_work_sort_latest: ['Latest update', '更新が新しい順'],
  my_work_sort_oldest: ['Oldest update', '更新が古い順'],
  my_work_sort_title: ['Request A–Z', '依頼の名前順'],
  my_work_filter_label: ['Filter work', '状態で絞り込む'],
  my_work_status_all: ['All', 'すべて'],
  my_work_status_needs: ['Needs you', '対応が必要'],
  my_work_status_working: ['In progress', '進行中'],
  my_work_status_quiet: ['No recent update', '最近の更新なし'],
  my_work_status_ready: ['Ready to check', '確認待ち'],
  my_work_status_checked: ['Checked', '確認済み'],
  my_work_status_blocked: ['Blocked', '止まっています'],
  my_work_status_todo: ['To do', '未着手'],
  my_work_status_unknown: ['Update recorded', '更新を記録'],
  my_work_status_closed: ['Closed', '終了'],
  my_work_status_shipped: ['Shipped', '完了'],
  my_work_progress: ['{p}% · reviewed {ago}', '{p}% ・{ago}に確認'],
  my_work_completed: ['Completed {date}', '{date} に完了'],
  my_work_pace: ['At this pace, about {date}', 'このペースなら {date} ごろ'],
  my_work_progress_label: ['Progress', '進み具合'],
  my_work_story: ['Build story', '作業の経緯'],
  my_work_status_other: ['Other states', 'その他の状態'],
  my_work_all_work: ['All work', 'すべての作業'],
  my_work_count_one: ['{n} request', '{n} 件の依頼'],
  my_work_count: ['{n} requests', '{n} 件の依頼'],
  my_work_no_match: ['No matching work', '一致する作業はありません'],
  my_work_no_match_body: ['Try another request, project or contributor.', '別の依頼名、プロジェクト、作業者で試してください。'],
  my_work_empty: ['No requests here yet', 'まだ依頼はありません'],
  my_work_empty_body: ['Recorded human requests appear here as your tools save work. You can also explore your existing memory in the Timeline.', 'ツールが作業を保存すると、記録された依頼がここに表示されます。これまでの記憶はタイムラインでも見られます。'],
  my_work_unlinked: ['{n} work records need a linked human request', '依頼と結びついていない作業記録が {n} 件あります'],
  my_work_unlinked_body: ['These records stay separate because their original human request was not captured or linked. Explore their source in the Timeline.', '元の依頼が記録されていないか、結びついていないため、分けて表示しています。元の記録はタイムラインで見られます。'],
  my_work_unlinked_link: ['Explore recorded memory →', '記録された記憶を見る →'],
  my_work_states_note: ['Working, reported complete, and human checked are separate states.', '作業中、完了の報告、人による確認は、それぞれ別の状態です。'],
  my_work_copy_updating: ['Summary updating', '要約を更新しています'],
  my_work_contributors: ['Contributors', '作業者'],
  my_work_contributors_more: [' · {n} contributors', ' · 作業者 {n} 人'],
  my_work_no_contributor: ['No contributor recorded', '作業者の記録はありません'],
  my_work_contributor: ['Contributor', '作業者'],
  my_work_next_step: ['Next step', '次の一手'],
  my_work_open_review: ['Review result', '結果を確認'],
  my_work_open_handoff: ['Read handoff', '引き継ぎを読む'],
  my_work_open_request: ['Open request', '依頼を開く'],
  my_work_open_label: ['Open request: {title}', '依頼を開く: {title}'],
  my_work_next_ready: ['Read the reported result and recorded evidence.', '報告された結果と、記録された根拠を読んでください。'],
  my_work_next_needs: ['Read the handoff, then continue in the original work conversation.', '引き継ぎを読んでから、元の作業の会話で続けてください。'],
  my_work_next_other: ['Follow the latest recorded activity.', '最新の記録された作業を確認してください。'],
  my_work_no_progress: ['No progress update has been recorded.', '進み具合の記録はまだありません。'],
  my_work_now: ['Just now', 'たった今'],
  my_work_minutes: ['{n}m ago', '{n} 分前'],
  my_work_hours: ['{n}h ago', '{n} 時間前'],
  my_work_yesterday: ['Yesterday', '昨日'],
  my_work_days: ['{n}d ago', '{n} 日前'],
  my_work_no_time: ['Time not recorded', '時刻の記録はありません'],
  my_work_col_status: ['Status', '状態'],
  my_work_col_request: ['Request', '依頼'],
  my_work_col_next: ['Next step / outcome', '次の一手・結果'],
  my_work_col_updated: ['Last update', '最終更新'],
  my_work_table: ['Requests', '依頼の一覧'],
  my_work_agent_role: ['Contributing to your requests', 'あなたの依頼に参加しています'],
  my_work_agent_count_one: ['{n} human request', '依頼 {n} 件'],
  my_work_agent_count: ['{n} human requests', '依頼 {n} 件'],
  my_work_agent_view: ['View requests', '依頼を見る'],
  my_work_no_agents: ['No contributors in this view', 'この表示に作業者はいません'],
  my_work_no_agents_body: ['Requests without recorded contributor identity remain in Cards and List.', '作業者が記録されていない依頼は、カードと一覧に表示されます。'],
  my_work_all_agents: ['← All agents', '← すべてのエージェント'],
  my_work_back: ['← Back to My Work', '← マイワークに戻る'],
  my_work_back_request: ['← Back to request', '← 依頼に戻る'],
  my_work_missing: ['This request is outside the loaded history', 'この依頼は、読み込んだ記録の範囲にありません'],
  my_work_missing_body: ['Return to My Work to see the current requests.', 'マイワークに戻ると、いまの依頼を見られます。'],
  my_work_line_off: ['Off: My Work is not written with AI.', 'オフ: My Work は AI で書いていません。'],
  my_work_line_summaries_off: ['On, but waiting: summaries are off.', 'オンですが、要約がオフのため待っています。'],
  my_work_line_settings: ['Open Settings', '設定を開く'],
  my_work_line_waiting_bound: ['Waiting: the daily limit of {n} calls is reached.', '待機中: 1 日の上限の {n} 回に達しました。'],
  my_work_line_waiting_budget: ['Waiting: an AI provider’s spending or call limit is reached.', '待機中: AI の利用上限に達しています。'],
  my_work_line_waiting_resting: ['Waiting: every AI provider is resting.', '待機中: どの AI も休んでいます。'],
  my_work_line_waiting_owner: ['Waiting: no AI provider can take it. Check the AI providers in Settings.', '待機中: 使える AI がありません。設定の AI を確かめてください。'],
  my_work_line_until: ['Next try:', '次に試すのは'],
  my_work_line_written: ['Written', '書いたのは'],
  my_work_line_none: ['Nothing written yet.', 'まだ何も書いていません。'],
  my_goals_tab_months: ['Month by month', '月ごと'],
  my_goals_asked_once: ['asked once', '1 回頼みました'],
  my_goals_asked_n: ['asked {n}×', '{n} 回頼みました'],
  my_goals_last: ['last {day}', '最後は {day}'],
  my_goals_why_one: ['Why it’s here · 1 quote', 'ここにある理由 · 引用 1 件'],
  my_goals_why_n: ['Why it’s here · {n} quotes', 'ここにある理由 · 引用 {n} 件'],
  my_goals_ask_meta: ['Asked {n}×, last {day}', '{n} 回、最後は {day}'],
  my_goals_history_note: ['What you asked for most, month by month, as the last update of each month kept it. A month closing archives its asks; it doesn’t mean they were done.', '月ごとに、いちばん多く頼んだことを、その月の最後の更新のとおりに残しています。月が終わると頼みごとは記録に移りますが、済んだという意味ではありません。'],
  my_goals_current: ['Current', '今月'],
  my_goals_archived: ['Archived', '記録'],
  my_goals_prompts: ['{n} prompts', 'プロンプト {n} 件'],
  my_goals_status_nothing: ['Nothing on it', '手つかず'],
  my_goals_status_your_move: ['Waiting on you', 'あなたの番'],
  my_goals_status_moving: ['Moving', '進んでいる'],
  my_goals_status_asked_again: ['Asked again after ship', '完了のあとにまた頼んだ'],
  my_goals_status_shipped: ['Shipped', '完了'],
  my_goals_status_stalled: ['Stalled', '止まっている'],
  my_goals_rollup: ['{p}% · {k} of {n} requests reviewed', '{p}% · 依頼 {n} 件のうち {k} 件を確認'],
  my_goals_rollup_one: ['{p}% · {k} of 1 request reviewed', '{p}% · 依頼 1 件のうち {k} 件を確認'],
  my_goals_rollup_unreviewed: ['Not reviewed yet · 0 of {n} requests', 'まだ確認していません · 依頼 {n} 件のうち 0 件'],
  my_goals_rollup_unreviewed_one: ['Not reviewed yet · 0 of 1 request', 'まだ確認していません · 依頼 1 件のうち 0 件'],
  my_goals_rollup_label: ['{p}% from {k} of {n} requests reviewed', '確認した依頼 {n} 件のうち {k} 件から {p}%'],
  my_goals_rollup_none: ['No requests, so no progress to measure', '依頼がないため、進み具合はありません'],
  my_goals_not_counted: ['Not counted: {titles}', '数えていない依頼: {titles}'],
  my_goals_again: ['You asked again on {asked}, after the last ship on {shipped}. What shipped doesn’t cover it yet.', '{shipped} の完了のあと、{asked} にまた頼んでいます。完了したものでは、まだ足りていません。'],
  my_work_build_story: ['Build story', '作業の経緯'],
  my_work_story_chapters: ['Week by week · {n}', '週ごと · {n}'],
  my_work_story_week: ['{start} to {end}', '{start}〜{end}'],
  my_focus_label: ['My focus', 'マイフォーカス'],
  my_focus_other: ['Other', 'その他'],
  my_focus_sorting: ['Sorting…', '振り分け中…'],
  my_focus_need_you: ['{n} need you', '{n} 件があなたの対応待ち'],
  my_focus_area: ['Focus area', '分野'],
  my_focus_area_empty: ['No requests in this area yet', 'この分野にはまだ依頼がありません'],
  my_focus_area_empty_body: ['New requests come here as My Work sorts them, or you can move one here from its page.', 'My Work が依頼を振り分けると、ここに入ります。依頼のページから、ここへ移すこともできます。'],
  my_focus_move: ['Move to…', '移す先…'],
  my_focus_moving: ['Moving…', '移しています…'],
  my_focus_move_label: ['Move to another focus area', '別の分野へ移す'],
  my_focus_suggested_suffix: [' · suggested', '（提案）'],
  my_focus_moved: ['Moved “{title}” to {area}.', '「{title}」を「{area}」へ移しました。'],
  my_focus_saved: ['Saved on this computer. The next round of sorting never overrides it.', 'このコンピューターに保存しました。この後の自動の振り分けで変わることはありません。'],
  my_focus_pending: ['Saved. It shows here once My Work has read it.', '保存しました。My Work が読み込むと、ここに反映されます。'],
  // J4: the judge's suggestion on a request, and the owner's two answers.
  my_work_suggestion: ['Looks like part of “{title}”.', '「{title}」の一部のようです。'],
  my_work_suggestion_join: ['Join', 'まとめる'],
  my_work_suggestion_not_this: ['Not this', '違う'],
  my_work_suggestion_joined: ['Joined to “{title}”.', '「{title}」にまとめました。'],
  my_work_suggestion_declined: ['Kept as a request of its own.', '別の依頼のままにしました。'],
  my_focus_undo: ['Undo', '元に戻す'],
  my_focus_no_undo: ['This one can’t be undone from here.', 'この変更は、ここからは元に戻せません。'],
  my_focus_suggested: ['Suggested', '提案'],
  my_focus_done: ['Done', '完了'],
  my_focus_page_word: ['“{name}” reuses the page name “{word}”. Rename it so each word means one thing.', '「{name}」はページの名前「{word}」と同じ言葉を使っています。言葉ごとに意味が 1 つになるよう、名前を変えてください。'],
  my_focus_shipped: ['{n} shipped', '完了 {n} 件'],
  my_focus_asked: ['{asked}, {last}', '{asked}、{last}'],
  my_focus_not_started: ['Not started · {n}', 'まだ始まっていない · {n}'],
  my_focus_not_started_badge: ['Not started', '未着手'],
  my_focus_evidence: ['Why it’s here: {evidence}', 'ここにある理由: {evidence}'],
  my_focus_change: ['Change {name}', '「{name}」を変更'],
  my_focus_rename: ['Rename', '名前を変更'],
  my_focus_edit_goal: ['Edit goal', '目的を編集'],
  my_focus_merge: ['Merge into…', '別の分野に合併…'],
  my_focus_up: ['Move up', '上へ'],
  my_focus_down: ['Move down', '下へ'],
  my_focus_mark_done: ['Done', '完了にする'],
  my_focus_back_in_focus: ['Back in focus', 'フォーカスに戻す'],
  my_focus_keep: ['Keep', '残す'],
  my_focus_dismiss: ['Dismiss', '見送る'],
  my_focus_goal_prompt: ['Write one sentence saying what this area is for (Edit goal).', 'この分野が何のためのものか、1 文で書いてください (目的を編集)。'],
  my_focus_rename_title: ['Rename “{name}”', '「{name}」の名前を変更'],
  my_focus_goal_title: ['Goal for “{name}”', '「{name}」の目的'],
  my_focus_merge_title: ['Merge “{name}” into…', '「{name}」を合併する先'],
  my_focus_rename_about: ['A short name you would ask for things inside of. It can’t reuse a page name.', 'この中の作業を頼むときに使う、短い名前です。ページの名前は使えません。'],
  my_focus_goal_about: ['One sentence saying what this area is for.', 'この分野が何のためのものかを 1 文で。'],
  my_focus_merge_about: ['Its requests and asks show under the area you choose. Undo right after brings “{name}” back as it was.', 'この分野の依頼と頼みごとは、選んだ分野の下に表示されます。直後に「元に戻す」を押すと、「{name}」は元のとおりに戻ります。'],
  my_focus_name: ['Name', '名前'],
  my_focus_goal: ['Goal', '目的'],
  my_focus_merge_into: ['Merge into', '合併する先'],
  my_focus_choose_area: ['Choose an area', '分野を選ぶ'],
  my_focus_page_word_typed: ['“{name}” reuses the page name “{word}”. Pick a name that means only this area.', '「{name}」はページの名前「{word}」と同じ言葉を使っています。この分野だけを指す名前にしてください。'],
  my_focus_save: ['Save', '保存'],
  my_focus_saving: ['Saving…', '保存しています…'],
  my_focus_cancel: ['Cancel', 'キャンセル'],
  my_focus_kept: ['Kept “{name}”.', '「{name}」を残しました。'],
  my_focus_dismissed: ['Dismissed “{name}”.', '「{name}」を見送りました。'],
  my_focus_marked_done: ['Marked “{name}” done.', '「{name}」を完了にしました。'],
  my_focus_renamed: ['Renamed “{name}” to “{to}”.', '「{name}」の名前を「{to}」に変えました。'],
  my_focus_goal_changed: ['Changed the goal of “{name}”.', '「{name}」の目的を変えました。'],
  my_focus_merged: ['Merged “{name}” into “{into}”.', '「{name}」を「{into}」に合併しました。'],
  my_focus_reordered: ['Changed the order of your focus areas.', '分野の並び順を変えました。'],
  my_focus_undone: ['Undone: {text}', '元に戻しました: {text}'],
  my_focus_n_suggested: ['{n} suggested', '提案 {n} 件'],
  my_focus_restored: ['Put “{name}” back in Suggested.', '「{name}」を提案に戻しました。'],
  my_focus_past: ['Past focus', '過去のフォーカス'],
  my_focus_past_about: ['Areas you finished, and what you asked for in earlier months.', '完了した分野と、これまでの月に頼んだことです。'],
  my_focus_done_areas: ['Done areas', '完了した分野'],
  my_focus_first_today: ['{name} · first in your focus today', '{name} · 今いちばん上の分野'],
  my_focus_none_kept: ['No area kept yet', 'まだ残した分野はありません'],
  my_focus_no_areas_saved: ['No areas saved', '分野の記録はありません'],
  my_focus_past_empty: ['Nothing here yet', 'まだ何もありません'],
  my_focus_past_empty_body: ['oboete reads your recent prompts and fills this in while My Work is written with AI.', 'My Work を AI で書く設定のあいだ、oboete が最近のプロンプトを読んで、ここを埋めます。'],
  my_focus_suggested_title: ['Suggested focus areas', '提案された分野'],
  my_focus_suggested_about: ['oboete suggests areas from your prompts and your requests. Keep the ones you’d ask for things inside of. A refresh never changes an area you decided.', 'oboete があなたのプロンプトと依頼から分野を提案します。その中で作業を頼みたいものを残してください。あなたが決めた分野は、更新で変わることはありません。'],
  my_focus_decided_count: ['{k} of {n} decided', '{n} 件のうち {k} 件を決定'],
  my_focus_nothing_waiting: ['Nothing waiting. New suggestions show up here after the next refresh.', '待っている提案はありません。新しい提案は、次の更新のあとにここに出ます。'],
  my_focus_placed_none: ['No requests placed in it yet', 'まだ依頼は入っていません'],
  my_focus_placed_one: ['1 request placed in it', '依頼 1 件が入っています'],
  my_focus_placed_n: ['{n} requests placed in it', '依頼 {n} 件が入っています'],
  my_focus_asks_one: [' · 1 ask not started', ' · まだ始まっていない頼みごと 1 件'],
  my_focus_asks_n: [' · {n} asks not started', ' · まだ始まっていない頼みごと {n} 件'],
  my_focus_merge_short: ['Merge', '合併'],
  my_focus_show_it: ['Show it', '見る'],
  my_focus_decided_kept: ['Kept. It’s in My focus now.', '残しました。マイフォーカスに入っています。'],
  my_focus_decided_merged: ['Merged into {into}. Its requests show there now.', '「{into}」に合併しました。依頼はそちらに表示されます。'],
  my_focus_decided_dismissed: ['Dismissed. It won’t be suggested again, and its requests are sorted again.', '見送りました。もう提案されず、依頼は振り分け直されます。'],
  my_focus_why_suggested: ['Why it’s suggested: {why}', '提案の理由: {why}'],
  my_focus_source_owner_goals: ['your goals found it in your prompts.', 'あなたのプロンプトから目標として見つかりました。'],
  my_focus_source_clustering: ['several Other requests read as steps of it.', '「その他」のいくつかの依頼が、この一部に見えます。'],
  my_focus_source_promotion: ['one request held several separate asks.', '1 つの依頼に、別々の頼みごとがいくつも入っていました。'],
  my_focus_source_owner: ['you wrote it.', 'あなたが作りました。'],
  my_focus_more_one: ['1 more quote', '引用をもう 1 件'],
  my_focus_more_n: ['{n} more quotes', '引用をあと {n} 件'],
  my_focus_renamed_pending: ['Renamed “{name}”.', '「{name}」の名前を変えました。'],
  my_focus_too_long: ['At most {n} characters.', '{n} 文字までです。'],
  my_focus_held_steps: ['Saved the first step. Press Undo again once My Work has read it.', '最初の手順を保存しました。My Work が読み込んだら、もう一度「元に戻す」を押してください。'],
  my_focus_other_about: ['Requests that fit none of your focus areas. A refresh suggests a new area when several of these turn out to be steps of something larger.', 'どの分野にも当てはまらない依頼です。そのいくつかが大きな作業の一部だとわかると、更新のときに新しい分野が提案されます。'],
  focus_move_target: ['That area takes no requests now.', 'その分野は、今は依頼を受け付けていません。'],
  focus_not_found: ['That area or request is no longer there. The page has been read again.', 'その分野か依頼は、もうありません。ページを読み直しました。'],
  focus_order: ['That order no longer matches your focus areas. The page has been read again.', 'その並び順は、今の分野と合いません。ページを読み直しました。'],
  focus_merged: ['That area was merged into another. The page has been read again.', 'その分野は別の分野に合併されています。ページを読み直しました。'],
  focus_page_word: ['That name reuses a page name. Pick a name that means only this area.', 'その名前はページの名前と同じ言葉を使っています。この分野だけを指す名前にしてください。'],
  focus_merge_self: ['An area can’t be merged into itself.', '分野を、その分野自身に合併することはできません。'],
  focus_merge_target: ['That area can’t take a merge now.', 'その分野には、今は合併できません。'],
  focus_restore: ['An area you wrote can’t be put back in Suggested.', 'あなたが作った分野は、提案に戻せません。'],
  focus_unavailable: ['My Focus can’t be saved right now. Try again in a moment.', '今は保存できません。少し待ってから、もう一度お試しください。'],
  suggestion_not_found: ['That suggestion is no longer waiting for an answer. The page has been read again.', 'その提案は、もう答えを待っていません。ページを読み直しました。'],
  suggestion_target_gone: ['The request it suggested is no longer there. The page has been read again.', '提案された依頼は、もうありません。ページを読み直しました。'],
  my_work_pending: ['The last answer is still being saved. Try again in a moment.', '前の答えをまだ保存しています。少し待ってから、もう一度お試しください。'],
  my_work_unavailable: ['My Work can’t be saved right now. Try again in a moment.', '今は保存できません。少し待ってから、もう一度お試しください。'],
  my_rules: ['My Rules', '規則'],
  my_rules_eyebrow: ['How you like to work, as rules your agents are given', 'エージェントに渡す、あなたの仕事の進め方の規則'],
  my_rules_where: ['Rules you approve or add reach Claude Code, Codex and Grok at the start of every session, as your preferences. A rule of one repository reaches only the sessions in it.', '承認した規則と追加した規則は、あなたの好みとして、Claude Code・Codex・Grok の毎回のセッションの始めに渡ります。1 つのリポジトリの規則は、そのリポジトリのセッションにだけ渡ります。'],
  my_rules_applies: ['Applies', '適用する範囲'],
  my_rules_everywhere: ['Every repository', 'すべてのリポジトリ'],
  my_rules_in_repo: ['In {name}', '{name} の中だけ'],
  my_rules_chosen_repo: ['In the repository you chose', '選んだリポジトリの中だけ'],
  my_rules_removed: ['Rule removed', '外れた規則'],
  my_rules_removed_note: ['You approved these, but the rule was retracted or forgotten since. Undo makes the suggestion wait for your decision again.', '承認したあとで、規則が取り消されたか忘れられたものです。元に戻すと、提案はまたあなたの判断を待ちます。'],
  my_rules_decide: ['Needs your decision', 'あなたの判断を待っています'],
  my_rules_decide_note: ['Suggested · learned from your requests, not yours until you approve', '提案 · 依頼から学んだもので、承認するまではあなたの規則ではありません'],
  my_rules_nothing_waiting: ['Nothing waiting. New suggestions show up here after the next learning run.', '判断を待つものはありません。次に学ぶと、新しい提案がここに出ます。'],
  my_rules_yours: ['Your rules', 'あなたの規則'],
  my_rules_none: ['No rules yet', 'まだ規則はありません'],
  my_rules_rejected: ['Rejected', '却下したもの'],
  my_rules_rejected_note: ['A rejected suggestion stays rejected, even when your requests say it again.', '却下した提案は、依頼でまた同じことを言っても却下のままです。'],
  my_rules_seen_one: ['Seen in 1 request', '依頼 1 件で見ました'],
  my_rules_seen_n: ['Seen in {n} requests', '依頼 {n} 件で見ました'],
  my_rules_strength_1: ['Light', '弱い'],
  my_rules_strength_2: ['Steady', '普通'],
  my_rules_strength_3: ['Strong', '強い'],
  my_rules_why_one: ['Why · 1 quote', '理由 · 引用 1 件'],
  my_rules_why_n: ['Why · {n} quotes', '理由 · 引用 {n} 件'],
  my_rules_approve: ['Approve', '承認'],
  my_rules_edit_approve: ['Edit, then approve', '直してから承認'],
  my_rules_save_approve: ['Save and approve', '保存して承認'],
  my_rules_reject: ['Reject', '却下'],
  my_rules_cancel: ['Cancel', 'やめる'],
  my_rules_undo: ['Undo', '元に戻す'],
  my_rules_undo_approve: ['Undo approval', '承認を取り消す'],
  my_rules_edit: ['Edit rule', '規則を直す'],
  my_rules_save: ['Save rule', '規則を保存'],
  my_rules_off: ['Turn rule off', '規則をオフにする'],
  my_rules_on: ['Turn rule on', '規則をオンにする'],
  my_rules_is_off: ['Off', 'オフ'],
  my_rules_set_by_you: ['Set by you', 'あなたが決めた規則'],
  my_rules_approved_by_you: ['Learned, approved by you', '学んだもので、あなたが承認'],
  my_rules_no_quotes: ['You wrote this rule, so there are no quotes behind it.', 'あなたが書いた規則なので、元になった引用はありません。'],
  my_rules_add: ['Add rule', '規則を追加'],
  my_rules_add_h: ['Add a rule', '規則を追加する'],
  my_rules_text: ['Rule', '規則'],
  my_rules_approved: ['Approved. It is now one of your rules.', '承認しました。あなたの規則になりました。'],
  my_rules_rejected_now: ['Rejected. It stays rejected, even when it comes up again.', '却下しました。また出てきても却下のままです。'],
  my_rules_undone: ['Undone. The suggestion waits for your decision again.', '元に戻しました。提案はまたあなたの判断を待っています。'],
  my_rules_saved: ['Saved.', '保存しました。'],
  my_rules_pending: ['Recorded. The worker applies it shortly.', '記録しました。まもなく反映されます。'],
  rule_not_found: ['That suggestion is no longer there. The page reads again.', 'その提案はもうありません。ページを読み直します。'],
  rule_decided: ['That suggestion is already decided. The page reads again.', 'その提案はもう判断済みです。ページを読み直します。'],
  rule_not_decided: ['That suggestion has no decision to undo. The page reads again.', 'その提案には元に戻す判断がありません。ページを読み直します。'],
  rule_scope: ['Choose where the rule applies: every repository, or one.', '規則を適用する範囲 (すべてのリポジトリか、1 つのリポジトリ) を選んでください。'],
  preference_repo_unknown: ['No record names that repository, so no rule is kept for it.', 'そのリポジトリの記録が無いので、規則は保存しません。'],
  preference_repo_excluded: ['That repository is not recorded, so no rule is kept for it.', 'そのリポジトリは記録しない設定なので、規則は保存しません。'],
  preference_repo_masked: ['That repository’s name holds a value the privacy rules hide, so no rule is kept for it.', 'そのリポジトリ名には伏せ字の対象が含まれるので、規則は保存しません。'],
  rule_partly_recorded: ['The rule is saved, but the suggestion still shows as waiting. Reject it, or leave it: approving it again adds the rule twice.', '規則は保存しましたが、提案は判断待ちのまま表示されます。却下するか、そのままにしてください。もう一度承認すると規則が 2 つになります。'],
  rule_pending: ['That decision is still being applied. The page reads again shortly.', 'その判断はまだ反映中です。少ししてからページを読み直します。'],
  rule_unavailable: ['My Rules could not save now. Try again shortly.', 'いまは保存できませんでした。少ししてからもう一度お試しください。'],
  my_goals_no_reviews: ['No request reviews cover this month, so it shows what you asked for, not what moved.', 'この月の依頼の見直しはまだないため、進んだことではなく、頼んだことを表示しています。'],
  my_work_stands: ['Where it stands', '現在の状況'],
  my_work_original: ['Original request', '元の依頼'],
  my_work_read_original: ['Read full original', '元の依頼の全文を読む'],
  my_work_streams: ['Work behind this request', 'この依頼の作業'],
  my_work_products: ['Work product', '成果物'],
  my_work_kind_commit: ['Commit', 'コミット'],
  my_work_kind_branch: ['Branch', 'ブランチ'],
  my_work_kind_pull_request: ['Pull request', 'プルリクエスト'],
  my_work_kind_artifact: ['Artifact', 'アーティファクト'],
  my_work_kind_page: ['Page', 'ページ'],
  my_work_kind_document: ['Document', '文書'],
  my_work_kind_deployment: ['Deployment', '公開先'],
  my_work_kind_deck: ['Slides', 'スライド'],
  my_work_kind_image: ['Image', '画像'],
  my_work_kind_video: ['Video', '動画'],
  my_work_kind_spreadsheet: ['Spreadsheet', '表'],
  my_work_kind_link: ['Link', 'リンク'],
  my_work_no_streams: ['No linked workstreams have been recorded yet.', '結びついた作業はまだ記録されていません。'],
  my_work_supporting: ['Supporting work', '補助の作業'],
  my_work_no_detail: ['No detailed update recorded.', '詳しい更新は記録されていません。'],
  my_work_follow_ups: ['Follow-ups to this request', 'この依頼への追加の指示'],
  my_work_result: ['Recorded result', '記録された結果'],
  my_work_cards_heading: ['Recorded cards', '記録されたカード'],
  my_work_changes: ['Recorded changes', '記録された変更'],
  my_work_work_update: ['Work update', '作業の更新'],
  my_work_no_changes: ['No detailed updates have been recorded yet.', '詳しい更新はまだ記録されていません。'],
  my_work_started: ['Started', '開始'],
  my_work_elapsed: ['Elapsed', '経過'],
  my_work_elapsed_title: ['Time since the human request, not active agent runtime', '依頼からの時間です。エージェントが動いていた時間ではありません'],
  my_work_just_started: ['Just started', '始まったばかり'],
  my_work_elapsed_m: ['{m}m', '{m} 分'],
  my_work_elapsed_hm: ['{h}h {m}m', '{h} 時間 {m} 分'],
  my_work_elapsed_dh: ['{d}d {h}h', '{d} 日 {h} 時間'],
  my_work_not_recorded: ['Not recorded', '記録なし'],
  my_work_to_timeline: ['Timeline →', 'タイムライン →'],
  my_work_updates_ready: ['New work updates are ready.', '新しい作業の更新があります。'],
  my_work_place_saved: ['Your place is saved.', '表示位置はそのままです。'],
  my_work_show_updates: ['Show updates', '更新を表示'],
  my_work_applied: ['New work updates applied.', '新しい作業の更新を表示しました。'],
  my_work_new: ['New update', '新しい更新'],
  my_work_copy_handoff: ['Copy handoff', '引き継ぎをコピー'],
  my_work_handoff_label: ['Handoff text to copy', 'コピーする引き継ぎの文'],
  my_work_copied: ['Handoff copied. Paste it into the agent or conversation you choose.', '引き継ぎをコピーしました。渡したいエージェントや会話に貼り付けてください。'],
  my_work_copied_announce: ['Handoff copied. Nothing was sent.', '引き継ぎをコピーしました。何も送信していません。'],
  my_work_clipboard: ['Clipboard unavailable. Copy the selected handoff text below.', 'クリップボードを使えません。下で選択した引き継ぎの文をコピーしてください。'],
  my_work_handoff_request: ['Request: {text}', '依頼: {text}'],
  my_work_handoff_project: ['Project: {text}', 'プロジェクト: {text}'],
  my_work_handoff_state: ['Recorded state: {text}', '記録された状態: {text}'],
  my_work_handoff_next: ['Next step: {text}', '次の一手: {text}'],
  my_work_handoff_updates: ['Latest recorded updates:', '最新の記録された更新:'],
  my_work_handoff_no_updates: ['No detailed updates recorded.', '詳しい更新の記録はありません。'],
  my_work_handoff_end: ['Continue in the original work conversation. This handoff does not assign or send work.', '元の作業の会話で続けてください。この引き継ぎは、作業を割り当てたり送ったりしません。'],
  my_work_handoff_ready: ['Reported complete', '完了の報告あり'],
  my_work_handoff_checked: ['Checked locally', 'この PC で確認済み'],
  my_work_handoff_recorded: ['Recorded update', '記録された更新'],
  my_work_review_eyebrow: ['Recorded result · your review', '記録された結果・あなたの確認'],
  my_work_evidence: ['Recorded evidence', '記録された根拠'],
  my_work_no_evidence: ['No separate test evidence was recorded. Inspect the source before checking this result.', 'テストの根拠は別に記録されていません。この結果を確認済みにする前に、元の記録を見てください。'],
  my_work_review_note: ['Checking saves a local note that you looked at this version. It does not certify correctness or change the agent’s work. New substantive updates need another check.', '確認済みにすると、この版を見たというメモがこの PC に保存されます。正しさを保証するものではなく、エージェントの作業も変えません。内容のある新しい更新があれば、もう一度確認が必要です。'],
  my_work_mark_checked: ['Mark checked', '確認済みにする'],
  my_work_undo_check: ['Undo check', '確認を取り消す'],
  my_work_checked_at: ['Checked locally {time}', '{time} にこの PC で確認済み'],
  my_work_checked_saved: ['Checked locally. Your review is saved.', 'この PC で確認済みにしました。確認は保存されています。'],
  my_work_check_undone: ['Check undone. The result is ready to review.', '確認を取り消しました。結果をもう一度確認できます。'],
  my_work_review_stale: ['This work changed while you were reading. Review the latest version before checking.', '読んでいる間に作業が変わりました。最新の版を見てから確認してください。'],
  my_work_review_failed: ['The local review could not be saved. Try again.', '確認を保存できませんでした。もう一度試してください。'],
  my_work_saved_unread: ['Saved. The page shows it at its next read.', '保存しました。次に読み直したときに画面に出ます。'],
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
  // My Work's heading before its first read, which the page opens on.
  if (view === 'my_work' && mw.rows === null) $('heading').textContent = t('my_work');
  if (view === 'my_rules' && mr.answer) myRulesDraw();
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
const SIZES = ['session_start_chars', 'per_prompt_chars', 'per_prompt_wait_ms', 'correction_chars'];

/** G23: how the last prompt that could ask search by meaning took its block, or nothing. */
function promptSide(side) {
  if (!side) return null;
  const time = new Date(side.at).toLocaleString(lang);
  const key = { meaning: 'prompt_side_meaning', 'no-viewer': 'prompt_side_no_viewer', late: 'prompt_side_late',
    failed: 'prompt_side_failed', loading: 'prompt_side_loading' }[side.side];
  return note(key ? t(key, { time }) : t('prompt_side_other', { time, why: side.side }));
}

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
  // A saved config answers a preview's key with `stale`: the review is made again.
  embeddingDraft.preview = null;
  embeddingDraft.confirmed = false;
  const workers = s.embedding?.workers_ai;
  return {
    version: s.version,
    savedSettings: s,
    saved: { summary: { ...s.summary }, capture: { ...s.capture }, paid_usd_per_month: s.paid_usd_per_month, gemini: s.gemini,
      worker: { resident: s.worker?.resident ?? false }, view: { port: s.view?.port }, my_work: { ...s.my_work } },
    firstRun: s.first_run === true,
    residentSupported: s.resident_supported === true,
    worker: { resident: s.first_run && s.resident_supported ? true : s.worker?.resident ?? false },
    view: { port: String(s.view?.port ?? '') },
    // A save's answer says where this page serves; other operations' answers keep what was known.
    viewRuntime: s.view_runtime ?? form?.viewRuntime ?? null,
    summary: { ...s.summary, window_tokens: String(s.summary.window_tokens), idle_minutes: String(s.summary.idle_minutes) },
    // Decision 64: no cap is null here and 4294967295 (`ranges.no_cap`) in a calls a day; turned
    // off, a cap is typed again.
    paid_usd_per_month: text(s.paid_usd_per_month),
    paidNoCap: s.paid_usd_per_month === null,
    // docs/my-work-model.md A6: the switch and the daily bound, with the estimate shown first.
    my_work: { curate: s.my_work.curate, calls_a_day: noCapText(s.my_work.calls_a_day, s.ranges),
      noCap: s.my_work.calls_a_day === s.ranges.no_cap, estimate: s.my_work.estimate },
    // docs/my-work-model.md J1: the judge's state; its switch only once a key is set.
    judge: s.judge ? { ...s.judge, budget: noCapText(s.judge.daily_budget, s.ranges), noCap: s.judge.daily_budget === s.ranges.no_cap } : null,
    gemini: s.gemini ?? 'none',
    usd_this_month: s.usd_this_month,
    prompt_side: s.prompt_side ?? null,
    stopped: s.stopped,
    inject: { ...s.inject, ...Object.fromEntries(SIZES.map((k) => [k, String(s.inject[k])])) },
    capture: { ...s.capture },
    backup: { dir: s.backup?.dir ?? null, edit: text(s.backup?.dir), reset: false },
    retention: { days: s.retention?.days ?? null, edit: text(s.retention?.days), forever: (s.retention?.days ?? null) === null },
    redaction: { enabled: s.redaction?.enabled !== false, rules: (s.redaction?.extra_rules || []).map(redactionEdit), hashes: (s.redaction?.allowlist || []).join('\n') },
    privacy: s.privacy || privacyUnavailable(),
    // Opening Settings reads the list; a save's or another operation's answer keeps what was read.
    hidden: 'hidden' in s ? s.hidden : form?.hidden ?? null,
    preference: preferenceDraft,
    maintenance: maintenanceDraft,
    chain: s.chain.map((e) => {
      const loaded = chainDailyAsLoaded(e, s.ranges);
      return { ...e, edit: { on: e.on, daily_budget: loaded.daily_budget, timeout_s: text(e.timeout_s), model: text(e.model),
        noCap: loaded.noCap, loaded } };
    }),
    providers: (s.providers || []).map((p) => ({ ...p, edit: providerEdit(p.name, p.saved), editing: false })),
    newProvider: null,
    warnings: s.warnings,
    ranges: s.ranges,
    keyInput: s.key_input,
    embedding: s.embedding ? {
      provider: s.embedding.provider, workers, local: s.embedding.local,
      edit: { account_id: text(workers.account_id), daily_requests: String(workers.daily_requests),
        monthly_usd: String(workers.monthly_usd) },
    } : null,
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
  return { value: { enabled: form.redaction.enabled, extra_rules, allowlist } };
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
  // Owner decision 43: off stops every rule; the warning shows before the save, without a redraw.
  const warning = el('p', 'desc error', t('redaction_off_warning'));
  warning.hidden = f.redaction.enabled;
  const enabled = el('label', 'check', checkbox(f.redaction.enabled, (v) => { f.redaction.enabled = v; warning.hidden = v; }),
    t('redaction_enabled'));
  return el('section', null, el('h3', null, t('redaction_h')), el('p', 'desc', t('redaction_desc')), enabled, warning,
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
  const simple = $('panel').querySelector('.simple-agent-inventory');
  if (simple) { simple.replaceChildren(simpleAgents()); return; }
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

const AGENT_SETUP_ACTIONS = ['wire','unwire'];
const AGENT_SETUP_COMPONENTS = ['hooks','mcp','trust','plugin','extension'];
const AGENT_SETUP_EFFECTS = ['create','replace','delete','unchanged','manual','skipped','unavailable','native_command'];
const AGENT_SETUP_BACKUPS = ['create','kept','none'];
const AGENT_SETUP_PHASES = ['running','complete','partial','failed','stale','unknown'];
const AGENT_SETUP_OUTCOMES = ['prepared','committed','noop','manual','skipped','failed','stale','unknown'];
const AGENT_SETUP_KINDS = ['stage','backup','write','delete','native_remove','native_add','readback'];
let agentSetupDraft = {action:'wire',selected:new Set(),preview:null,confirmed:false,
  previewing:false,sending:false,reading:false,unknown:false,unknownInspected:false,error:null,
  operationId:null,pendingAction:null,pendingAgents:null,receipt:null,status:null,previewRead:0,statusRead:0};

function agentSetupSelected(d) {
  return AGENT_INVENTORY_IDS.filter(id=>d.selected.has(id));
}
function agentSetupLabel(group,value) {
  const key=`agent_setup_${group}_${value}`;
  return typeof value==='string'&&Object.hasOwn(TEXT,key) ? t(key) : t('agent_setup_unknown_value');
}
function agentSetupStepKnown(step) {
  return step&&typeof step==='object'
    &&AGENT_SETUP_COMPONENTS.includes(step.component)
    &&AGENT_SETUP_EFFECTS.includes(step.effect)
    &&AGENT_SETUP_BACKUPS.includes(step.backup)
    &&(step.code===undefined || step.code===null || typeof step.code==='string'
      &&Object.hasOwn(TEXT,`agent_setup_code_${step.code}`));
}
function agentSetupPreviewMatches(preview,d,selected) {
  if(!preview||typeof preview!=='object'||!/^[0-9a-f]{64}$/.test(preview.preview_key||'')
    ||preview.action!==d.action||preview.live_verified!==false
    ||preview.activation!=='next_session'||!Array.isArray(preview.agents)
    ||preview.agents.length!==selected.length)return false;
  const names=preview.agents.map(row=>row?.agent);
  return new Set(names).size===selected.length&&selected.every(id=>names.includes(id))
    &&preview.agents.every(row=>Array.isArray(row.steps)&&row.steps.every(agentSetupStepKnown));
}
function agentSetupReceiptKnown(receipt) {
  if(!receipt||typeof receipt!=='object'||!Array.isArray(receipt.agents))return false;
  const names=receipt.agents.map(row=>row?.agent);
  return names.length>=1&&names.length<=AGENT_INVENTORY_IDS.length
    &&new Set(names).size===names.length
    &&/^[0-9a-f]{64}$/.test(receipt.operation_id||'')
    &&AGENT_SETUP_ACTIONS.includes(receipt.action)
    &&AGENT_SETUP_PHASES.includes(receipt.phase)
    &&receipt.live_verified===false&&receipt.activation==='next_session'
    &&receipt.agents.every(row=>AGENT_INVENTORY_IDS.includes(row?.agent)
      &&Array.isArray(row.steps)&&row.steps.every(step=>agentSetupStepKnown(step)
        &&AGENT_SETUP_OUTCOMES.includes(step.outcome)&&AGENT_SETUP_KINDS.includes(step.kind)));
}
function agentSetupReceiptMatches(receipt,d) {
  if(!agentSetupReceiptKnown(receipt)||!Array.isArray(d.pendingAgents))return false;
  const names=receipt.agents.map(row=>row.agent);
  return receipt.operation_id===d.operationId&&receipt.action===d.pendingAction
    &&names.length===d.pendingAgents.length&&d.pendingAgents.every(id=>names.includes(id));
}
function agentSetupBlocked(d) {
  return d.previewing||d.sending||d.unknown||Boolean(d.status?.active);
}
function agentSetupCanPrepare(d) {
  return d.unknown&&d.unknownInspected&&!d.sending&&!d.reading
    &&d.status?.active===null&&agentSetupReceiptMatches(d.status.last,d)
    &&d.status.last.phase==='unknown';
}
function invalidateAgentSetup(d) {
  d.previewRead++;
  d.preview=null;d.confirmed=false;d.error=null;
}
function renderAgentSetup() {
  if(view!=='settings')return;
  const section=$('panel').querySelector('.agent-setup');
  if(!section)return;
  const focused=section.contains(document.activeElement) ? document.activeElement : null;
  const field=focused?.dataset.field,action=focused?.dataset.action;
  section.replaceChildren(...agentSetupSection().childNodes);
  const next=field ? [...section.querySelectorAll('[data-field]')].find(node=>node.dataset.field===field)
    : action ? [...section.querySelectorAll('[data-action]')].find(node=>node.dataset.action===action) : null;
  if(next&&!next.disabled)next.focus({preventScroll:true});
  else if(action==='agent_setup.prepare')section.querySelector('[data-action="agent_setup.preview"]')?.focus({preventScroll:true});
}
async function previewAgentSetup(d=agentSetupDraft) {
  if(agentSetupBlocked(d)||!AGENT_SETUP_ACTIONS.includes(d.action))return;
  const agents=agentSetupSelected(d);
  if(!agents.length){d.error='agent_setup_no_selection';renderAgentSetup();return;}
  invalidateAgentSetup(d);
  const read=d.previewRead;
  const action=d.action;
  d.previewing=true;renderAgentSetup();
  try {
    const {res,answer}=await memoryWrite('setup/preview',{action,agents});
    if(read!==d.previewRead)return;
    if(!res.ok){d.error=res.status===503?'agent_setup_busy':'agent_setup_preview_failed';return;}
    if(!agentSetupPreviewMatches(answer,d,agents)){d.error='agent_setup_preview_failed';return;}
    d.preview=answer;
  } catch {if(read===d.previewRead)d.error='agent_setup_preview_failed';}
  finally {if(read===d.previewRead){d.previewing=false;renderAgentSetup();}}
}
async function startAgentSetup(d=agentSetupDraft) {
  const agents=agentSetupSelected(d);
  if(agentSetupBlocked(d)||!AGENT_SETUP_ACTIONS.includes(d.action)
    ||!d.confirmed||!agentSetupPreviewMatches(d.preview,d,agents))return;
  let bytes;
  try {bytes=crypto.getRandomValues(new Uint8Array(32));}
  catch {d.error='agent_setup_start_failed';renderAgentSetup();return;}
  const operationId=[...bytes].map(byte=>byte.toString(16).padStart(2,'0')).join('');
  const posted={action:d.action,agents,preview_key:d.preview.preview_key,
    operation_id:operationId,confirmed:true};
  d.operationId=operationId;d.pendingAction=posted.action;d.pendingAgents=agents;
  d.unknownInspected=false;
  d.statusRead++;d.reading=false;
  d.preview=null;d.confirmed=false;d.error=null;d.status=null;d.receipt=null;
  d.sending=true;renderAgentSetup();
  let refused=false;
  try {
    const {res,answer}=await memoryWrite('setup/start',posted);
    if(d.operationId!==operationId)return;
    if(!res.ok) {
      d.error=res.status===409||res.status===412?'agent_setup_stale'
        :res.status===503?'agent_setup_busy':'agent_setup_start_failed';
      refused=res.status<500||res.status===503;
      if(!refused)d.unknown=true;
      return;
    }
    if(!agentSetupReceiptMatches(answer,d)) {
      d.unknown=true;d.error='agent_setup_unknown';return;
    }
    d.statusRead++;d.reading=false;
    d.receipt=answer;
    d.status=answer.phase==='running'?{active:answer,last:null}:{active:null,last:answer};
    d.unknown=answer.phase==='unknown';
  } catch {if(d.operationId===operationId){d.unknown=true;d.error='agent_setup_unknown';}}
  finally {if(d.operationId===operationId){
    d.sending=false;
    if(refused){d.operationId=null;d.pendingAction=null;d.pendingAgents=null;}
    renderAgentSetup();
  }}
}
async function inspectAgentSetup(d=agentSetupDraft) {
  if(d.reading)return;
  d.unknownInspected=false;
  const read=d.statusRead= d.statusRead+1;
  d.reading=true;renderAgentSetup();
  try {
    const status=await api('setup/operation');
    if(read!==d.statusRead)return;
    if(!status||typeof status!=='object'||!Object.hasOwn(status,'active')
      ||!Object.hasOwn(status,'last')
      ||![status.active,status.last].every(row=>row===null||agentSetupReceiptKnown(row))
      ||[status.active,status.last].some(row=>row?.operation_id===d.operationId
        &&!agentSetupReceiptMatches(row,d)))throw new Error('invalid_status');
    d.status=status;
    d.error=null;
    const match=[status.active,status.last].find(row=>agentSetupReceiptMatches(row,d));
    if(d.unknown&&match){
      d.receipt=match;
      if(match.phase!=='unknown')d.unknown=false;
      else d.unknownInspected=status.active===null&&status.last===match;
    }
  } catch {if(read===d.statusRead)d.error='agent_setup_status_unavailable';}
  finally {if(read===d.statusRead){d.reading=false;renderAgentSetup();}}
}
function agentSetupRows(agents,receipt) {
  const rows=Array.isArray(agents)?agents:[];
  return AGENT_INVENTORY_IDS.filter(id=>rows.some(row=>row?.agent===id)).map(id=>{
    const row=rows.find(item=>item?.agent===id);
    const steps=row.steps.map(step=>{
      const summary=t('agent_setup_step',{
        component:agentSetupLabel('component',step.component),
        effect:agentSetupLabel('effect',step.effect),backup:agentSetupLabel('backup',step.backup)});
      const detail=receipt?t('agent_setup_step_result',{
        kind:agentSetupLabel('kind',step.kind),outcome:agentSetupLabel('outcome',step.outcome)}):null;
      const code=step.code ? agentSetupLabel('code',step.code) : null;
      return el('li',null,summary,detail ? ` ${detail}` : null,code ? ` ${code}` : null);
    });
    return el('li',null,el('h4',null,t('agent_inventory_agent_'+id)),
      steps.length?el('ul',null,...steps):el('p','desc',t('agent_setup_no_steps')));
  });
}
function agentSetupReceipt(receipt,heading) {
  if(!agentSetupReceiptKnown(receipt))return null;
  return el('div','agent-setup-receipt',el('h4',null,t(heading)),
    el('p','desc',t('agent_setup_receipt',{
      action:t('agent_setup_'+receipt.action),phase:agentSetupLabel('phase',receipt.phase)})),
    el('p','desc',t('agent_setup_activation')),
    el('ul',null,...agentSetupRows(receipt.agents,true)));
}
function agentSetupSection() {
  const d=agentSetupDraft,blocked=agentSetupBlocked(d),selected=agentSetupSelected(d);
  const action=el('select',null,...AGENT_SETUP_ACTIONS.map(value=>{
    const option=el('option',null,t('agent_setup_'+value));option.value=value;return option;
  }));
  action.value=d.action;action.dataset.field='agent_setup.action';action.disabled=blocked;
  action.addEventListener('change',()=>{d.action=action.value;invalidateAgentSetup(d);renderAgentSetup();});
  const choices=AGENT_INVENTORY_IDS.map(id=>{
    const box=checkbox(d.selected.has(id),checked=>{
      if(checked)d.selected.add(id);else d.selected.delete(id);
      invalidateAgentSetup(d);renderAgentSetup();
    });
    box.dataset.field=`agent_setup.agent.${id}`;box.disabled=blocked;
    return el('label','check',box,t('agent_inventory_agent_'+id));
  });
  const preview=el('button','quiet small',t('agent_setup_preview'));
  preview.type='button';preview.dataset.action='agent_setup.preview';
  preview.disabled=blocked||!selected.length;
  preview.addEventListener('click',()=>void previewAgentSetup(d));
  const native=d.preview?.agents?.some(row=>row.steps.some(step=>step.effect==='native_command'));
  const plugin=d.preview?.agents?.some(row=>row.steps.some(step=>step.component==='plugin'
    &&['replace','manual'].includes(step.effect)));
  const consent=checkbox(d.confirmed,checked=>{d.confirmed=checked;renderAgentSetup();});
  consent.dataset.field='agent_setup.confirmed';consent.disabled=blocked||!d.preview;
  const apply=el('button','quiet small',t('agent_setup_apply'));
  apply.type='button';apply.dataset.action='agent_setup.apply';
  apply.disabled=blocked||!d.confirmed||!agentSetupPreviewMatches(d.preview,d,selected);
  apply.addEventListener('click',()=>void startAgentSetup(d));
  const inspect=el('button','quiet small',t('agent_setup_status'));
  inspect.type='button';inspect.dataset.action='agent_setup.status';inspect.disabled=d.reading;
  inspect.addEventListener('click',()=>void inspectAgentSetup(d));
  const prepare=el('button','quiet small',t('agent_setup_prepare'));
  prepare.type='button';prepare.dataset.action='agent_setup.prepare';
  prepare.addEventListener('click',()=>{
    if(!agentSetupCanPrepare(d))return;
    d.unknown=false;d.unknownInspected=false;
    d.operationId=null;d.pendingAction=null;d.pendingAgents=null;
    d.statusRead++;d.reading=false;invalidateAgentSetup(d);renderAgentSetup();
  });
  const previewResult=d.preview ? el('div','agent-setup-preview',el('h4',null,t('agent_setup_preview_h')),
    el('p','desc',t('agent_setup_activation')),
    plugin?el('p','desc',t('agent_setup_plugin_note')):null,
    el('ul',null,...agentSetupRows(d.preview.agents,false)),
    el('label','check',consent,t(native?'agent_setup_consent_native':'agent_setup_consent'))) : null;
  const receipts=d.status ? [
    [d.status.active,'agent_setup_receipt_current_h'],
    [d.status.last,agentSetupReceiptMatches(d.status.last,d)
      ? 'agent_setup_receipt_current_h':'agent_setup_receipt_previous_h']
  ] : [[d.receipt,'agent_setup_receipt_current_h']];
  return el('section','agent-setup',el('h3',null,t('agent_setup_h')),
    el('p','desc',t('agent_setup_desc')),
    el('label','field',el('span',null,t('agent_setup_action')),action),
    el('fieldset',null,el('legend',null,t('agent_setup_select')),...choices),
    preview,d.previewing?el('p','desc',t('agent_setup_previewing')):null,
    previewResult,apply,d.sending?el('p','desc',t('agent_setup_sending')):null,
    d.error?el('p','desc text error',t(d.error)):null,
    d.unknown?el('p','desc',t('agent_setup_unknown')):null,
    ...receipts.map(([receipt,heading])=>agentSetupReceipt(receipt,heading)),inspect,
    agentSetupCanPrepare(d)?el('div',null,el('p','desc',t('agent_setup_prepare_note')),prepare):null,
    d.reading?el('p','desc',t('agent_setup_status_loading')):null,
    !receipts.some(([receipt])=>agentSetupReceiptKnown(receipt))&&!d.reading&&!d.sending
      ?el('p','desc',t('agent_setup_status_none')):null);
}

let doctorDraft={sending:false,report:null,error:null,read:0};
function doctorCode(group,value) {
  const key=`doctor_${group}_${value}`;
  return Object.hasOwn(TEXT,key) ? t(key) : t('doctor_state_unknown');
}
function doctorState(state) {
  return doctorCode('state',state);
}
function doctorNumber(value) {
  return Number.isSafeInteger(value) && value>=0 ? value.toLocaleString(lang) : t('doctor_state_unknown');
}
function doctorMoney(value) {
  return typeof value==='number' && Number.isFinite(value) && value>=0
    ? usdValue(value) : t('doctor_state_unknown');
}
function doctorFlag(fact) {
  if(fact?.state==='known')return typeof fact.value==='boolean'
    ? t(fact.value?'doctor_yes':'doctor_no') : t('doctor_state_unknown');
  return doctorState(fact?.state);
}
function doctorDate(milliseconds) {
  if(typeof milliseconds!=='number' || !Number.isFinite(milliseconds) || milliseconds<0)
    return t('doctor_state_unknown');
  const date=new Date(milliseconds);
  return Number.isNaN(date.getTime()) ? t('doctor_state_unknown') : date.toLocaleString(lang);
}
function doctorSince(sinceNs) {
  // Recording markers use Unix nanoseconds; Date expects milliseconds.
  return doctorDate(typeof sinceNs==='number' ? Math.floor(sinceNs/1_000_000) : null);
}
function doctorLast(last) {
  return last?.state==='known' ? doctorCode('last',last.kind) : doctorState(last?.state);
}
function doctorRecording(recording) {
  if(recording?.state!=='known')return t('doctor_recording_unknown',{state:doctorState(recording?.state)});
  if(recording.failed===false)return t('doctor_recording_clear');
  if(recording.failed===true)return t('doctor_recording_failed',{
    cause:doctorCode('failure',recording.class),since:doctorSince(recording.since_ns)});
  return t('doctor_recording_unknown',{state:t('doctor_state_unknown')});
}
function doctorMeasure(fact,money=false) {
  if(fact?.state!=='known')return doctorState(fact?.state);
  return money ? doctorMoney(fact.value) : doctorNumber(fact.value);
}
function doctorDecimal(fact) {
  if(fact?.state==='known' && typeof fact.value==='number' && Number.isFinite(fact.value)
    && fact.value>=0)return fact.value.toLocaleString(lang,{maximumSignificantDigits:15});
  return fact?.state==='known' ? t('doctor_state_unknown') : doctorState(fact?.state);
}
function doctorRowsTotal(fact,key) {
  const rows=fact?.rows;
  if(fact?.state!=='known')return doctorState(fact?.state);
  if(!Array.isArray(rows))return t('doctor_state_unknown');
  if(!rows.every(row=>Number.isSafeInteger(row?.[key]) && row[key]>=0))return t('doctor_state_unknown');
  const total=rows.reduce((sum,row)=>sum+row[key],0);
  return Number.isSafeInteger(total) ? total.toLocaleString(lang) : t('doctor_state_unknown');
}
function doctorRecent(recent) {
  if(recent?.state!=='known')return doctorState(recent?.state);
  return Array.isArray(recent.calls) ? recent.calls.length.toLocaleString(lang) : t('doctor_state_unknown');
}
function doctorRecentRows(recent) {
  if(recent?.state!=='known' || !Array.isArray(recent.calls))return [];
  return recent.calls.slice(0,5).map(call=>{
    const key=`doctor_outcome_${call?.outcome}`;
    return el('p','desc',t('doctor_recent_row',{
      outcome:t(Object.hasOwn(TEXT,key)?key:'doctor_outcome_other'),ms:doctorNumber(call?.ms)}));
  });
}
function doctorRewindTime(rewinds) {
  if(rewinds?.count?.state!=='known')return doctorState(rewinds?.count?.state);
  if(rewinds.last_at_ms===null)return rewinds.count.value===0
    ? t('doctor_time_none') : t('doctor_state_unknown');
  return doctorDate(rewinds.last_at_ms);
}
function doctorGapRows(gaps) {
  if(gaps?.state!=='known' || !Array.isArray(gaps.rows))return [];
  return [...AGENT_INVENTORY_IDS,'other'].map(agent=>{
    const row=gaps.rows.find(item=>item?.agent===agent);
    return el('p','desc',t('doctor_gap_row',{
      agent:agent==='other' ? t('doctor_gap_other') : t('agent_inventory_agent_'+agent),
      ended:doctorNumber(row?.ended),checked:doctorNumber(row?.checked),
      short:doctorNumber(row?.short),missing:doctorNumber(row?.missing)}));
  });
}
function doctorSkippedRows(skipped) {
  if(skipped?.state!=='known' || !Array.isArray(skipped.rows))return [];
  return ['excluded','held','refused','empty','other'].map(reason=>{
    const row=skipped.rows.find(item=>item?.reason===reason);
    return el('p','desc',t('doctor_skipped_row',{
      reason:t('doctor_skip_'+reason),count:doctorNumber(row?.count)}));
  });
}
function doctorGeneration(generation) {
  if(generation?.state!=='known')return doctorState(generation?.state);
  const labels={active:'doctor_generation_active',other:'doctor_generation_other'};
  const key=Object.hasOwn(labels,generation.value) ? labels[generation.value] : null;
  return generation.value===null ? t('doctor_generation_none')
    : key ? t(key) : t('doctor_state_unknown');
}
function doctorGroup(heading,...rows) {
  return el('section',null,el('h4',null,t(heading)),...rows);
}
function doctorParking(parking) {
  if(parking?.state!=='known')
    return [el('p','desc',t('doctor_parking_unknown',{state:doctorState(parking?.state)}))];
  if(!Array.isArray(parking.rows) || !parking.rows.every(row=>row && typeof row==='object'))
    return [el('p','desc',t('doctor_parking_unknown',{state:t('doctor_state_unknown')}))];
  const source={oboete_v1:'doctor_source_v1',transcript:'doctor_source_transcript',other:'doctor_source_other'};
  return parking.rows.map(row=>el('p','desc',t('doctor_parking',{
    source:t(source[row.source] || 'doctor_source_other'),
    parked:doctorNumber(row.parked),
    waiting:doctorNumber(row.waiting)})));
}
function doctorStores(stores) {
  return doctorGroup('doctor_files_h',...['raw','raw_restored','knowledge','legacy','providers'].map(name=>
    el('p','desc',t('doctor_file',{name:t('doctor_file_'+name),
      file:doctorState(stores?.[name]?.file),wal:doctorState(stores?.[name]?.wal)}))));
}
function doctorBackups(backups) {
  return doctorGroup('doctor_backup_h',
    el('p','desc',t('doctor_backup',{state:doctorState(backups.state),
      records:doctorMeasure(backups.record_segments),ops:doctorMeasure(backups.op_segments),
      through:doctorMeasure(backups.through_seq),through_ops:doctorMeasure(backups.through_op_seq)})),
    el('p','desc',t('doctor_backup_checksums',{
      missing:doctorMeasure(backups.verification?.missing_checksum),
      mismatch:doctorMeasure(backups.verification?.checksum_mismatch)})));
}
function doctorRuntime(runtime) {
  const worker=runtime.worker || {},viewer=runtime.resident_viewer || {};
  return doctorGroup('doctor_runtime_h',
    el('p','desc',t('doctor_runtime_state',{state:doctorState(runtime.state)})),
    el('p','desc',doctorRecording(runtime.recording)),
    el('p','desc',t('doctor_worker',{configured:doctorFlag(worker.configured_resident),
      supported:typeof worker.resident_supported==='boolean'
        ? t(worker.resident_supported?'doctor_yes':'doctor_no') : t('doctor_state_unknown'),
      running:doctorFlag(worker.running),last:doctorLast(worker.last)})),
    el('p','desc',t('doctor_viewer',{configured:doctorMeasure(viewer.configured_port),
      running:doctorFlag(viewer.running),actual:doctorNumber(viewer.actual_port),
      last:doctorLast(viewer.last)})),
    el('p','desc',t('doctor_restore_note',{value:doctorFlag(runtime.restore_note)})));
}
function doctorDisk(disk) {
  const low=disk.state==='known' && typeof disk.low_space==='boolean'
    ? t(disk.low_space?'doctor_yes':'doctor_no')
    : disk.state==='known' ? t('doctor_state_unknown') : doctorState(disk.state);
  return doctorGroup('doctor_disk_h',el('p','desc',t('doctor_disk',{
    state:doctorState(disk.state),bytes:disk.state==='known'
      ? doctorNumber(disk.free_bytes) : doctorState(disk.state),low})));
}
function doctorQuotaRest(rest) {
  return rest?.state==='known' ? doctorCode('rest',rest.kind) : doctorState(rest?.state);
}
function doctorQuotaError(last) {
  if(last?.state!=='known')return doctorState(last?.state);
  if(last.at_ms===null)return t('doctor_error_none');
  return t('doctor_error_at',{role:doctorCode('error',last.role),time:doctorDate(last.at_ms)});
}
function doctorQuota(quota) {
  return doctorGroup('doctor_quota_h',
    el('p','desc',t('doctor_quota',{state:doctorState(quota.state),
      documents:doctorNumber(quota.document_daily_requests),
      queries:doctorNumber(quota.query_daily_requests),cap:doctorMoney(quota.monthly_usd_cap),
      calls:doctorMeasure(quota.calls_last_day)})),
    el('p','desc',t('doctor_quota_history',{
      rest:doctorQuotaRest(quota.rest),error:doctorQuotaError(quota.last_error)})));
}
function doctorRetained(retained) {
  const categories=['v1_database','memory_database','snapshot','rollout_directory','spool','cache','logs'];
  const rows=Array.isArray(retained.categories) ? retained.categories : [];
  const listed=categories.map(category=>{
    const row=rows.find(item=>item?.category===category);
    return el('p','desc',t('doctor_retained_row',{
      name:t('doctor_retained_'+category),present:doctorFlag(row?.present),
      targets:doctorMeasure(row?.targets),bytes:doctorMeasure(row?.bytes)}));
  });
  return doctorGroup('doctor_retained_h',
    el('p','desc',t('doctor_retained',{state:doctorState(retained.state)})),...listed,
    el('p','desc',t('doctor_retained_eval',{
      present:doctorFlag(retained.evaluation_copies?.present),
      bytes:doctorMeasure(retained.evaluation_copies?.bytes)})));
}
function doctorUnhealthy(codes) {
  if(!Array.isArray(codes) || !codes.length)return null;
  return doctorGroup('doctor_unhealthy_h',el('ul',null,...codes.map(code=>{
    const key=`doctor_unhealthy_${code}`;
    return el('li',null,t(Object.hasOwn(TEXT,key)?key:'doctor_unhealthy_other'));
  })));
}
function doctorInventory(inventory) {
  const agents=Array.isArray(inventory?.agents) ? inventory.agents : [];
  return doctorGroup('doctor_agents_h',
    el('p','desc',t('agent_inventory_home',{state:agentInventoryCode('state',inventory?.home)})),
    el('p','desc',t('agent_inventory_config',{state:agentInventoryCode('state',inventory?.config)})),
    el('ul',null,...AGENT_INVENTORY_IDS.map(id=>agentInventoryRow(id,agents.find(row=>row?.agent===id)))));
}
function doctorReport(report) {
  const c=report.checks;
  const raw=c.raw,knowledge=c.knowledge,legacy=c.legacy,embeddings=c.embeddings,providers=c.providers;
  const usage=providers.usage || {};
  const rows=[];
  if(report.complete!==true)rows.push(el('p','desc',t('doctor_incomplete')));
  const unhealthy=doctorUnhealthy(report.unhealthy);
  if(unhealthy)rows.push(unhealthy);
  rows.push(el('p','desc',t('doctor_source',{state:doctorState(c.source_stability)})));
  if(c.source_stability==='changed')rows.push(el('p','desc',t('doctor_source_changed')));
  if(c.remaining==='not_checked')rows.push(el('p','desc',t('doctor_remaining')));
  rows.push(doctorInventory(report.inventory),doctorStores(report.stores),
    doctorGroup('doctor_raw_h',
      el('p','desc',t('doctor_raw',{state:doctorState(raw.integrity),
        records:doctorMeasure(raw.max_seq),ops:doctorMeasure(raw.max_op_seq),
        curated:doctorMeasure(raw.curated_through)})),...doctorParking(raw.parking)),
    doctorGroup('doctor_knowledge_h',
      el('p','desc',t('doctor_knowledge',{state:doctorState(knowledge.integrity),
        rewinds:doctorMeasure(knowledge.rewinds?.count),
        missing:doctorRowsTotal(knowledge.gaps,'missing'),short:doctorRowsTotal(knowledge.gaps,'short')})),
      el('p','desc',t('doctor_rewind_time',{time:doctorRewindTime(knowledge.rewinds)})),
      ...doctorGapRows(knowledge.gaps)),
    doctorGroup('doctor_legacy_h',
      el('p','desc',t('doctor_legacy',{state:doctorState(legacy.integrity),
        sessions:doctorMeasure(legacy.sessions),events:doctorMeasure(legacy.events),
        observations:doctorMeasure(legacy.observations),summaries:doctorMeasure(legacy.summaries),
        remaining:doctorMeasure(legacy.remaining_events)})),
      el('p','desc',t('doctor_legacy_recent',{count:doctorRecent(legacy.recent)})),
      ...doctorRecentRows(legacy.recent)),
    doctorGroup('doctor_embedding_h',
      el('p','desc',t('doctor_embedding',{state:doctorState(embeddings.state),
        generation:doctorGeneration(embeddings.generation),
        claims:doctorMeasure(embeddings.waiting?.claims),cards:doctorMeasure(embeddings.waiting?.cards),
        summaries:doctorMeasure(embeddings.waiting?.summaries),imports:doctorMeasure(embeddings.waiting?.imports),
        records:doctorMeasure(embeddings.waiting?.records),skipped:doctorRowsTotal(embeddings.skipped,'count')})),
      ...doctorSkippedRows(embeddings.skipped)),
    doctorGroup('doctor_providers_h',
      el('p','desc',t('doctor_providers',{state:doctorState(providers.integrity),
        curation:doctorMeasure(providers.spend?.curation_usd,true),
        embedding:doctorMeasure(providers.spend?.embed_query_usd,true),
        reserved:doctorMeasure(providers.spend?.reserved_usd,true),pending:doctorMeasure(providers.pending)})),
      el('p','desc',t('doctor_provider_usage',{calls:doctorMeasure(usage.daily_calls),
        tokens:doctorMeasure(usage.tokens),reserved:doctorMeasure(usage.reserved_calls),
        reserved_tokens:doctorDecimal(usage.reserved_tokens),key_cache:doctorMeasure(usage.key_cache_rows)})),
      el('p','desc',t(usage.key_binding==='unchecked'?'doctor_key_unchecked':'doctor_key_unknown')),
      el('p','desc',t('doctor_provider_holds',{owner:doctorMeasure(providers.stopped?.owner),
        resting:doctorMeasure(providers.stopped?.resting),recent:doctorRecent(providers.recent)})),
      ...doctorRecentRows(providers.recent),
      el('p','desc',providers.isolation?.historical===true
        ? t('doctor_isolation',{passed:doctorMeasure(providers.isolation?.passed),
          failed:doctorMeasure(providers.isolation?.failed)}) : t('doctor_isolation_unknown'))));
  if(providers.embedding_quota)rows.push(doctorQuota(providers.embedding_quota));
  if(c.backups)rows.push(doctorBackups(c.backups));
  if(c.runtime)rows.push(doctorRuntime(c.runtime));
  if(c.disk)rows.push(doctorDisk(c.disk));
  if(c.retained)rows.push(doctorRetained(c.retained));
  const result=el('div','doctor-result',...rows);
  result.setAttribute('role','status');
  return result;
}
function doctorSection() {
  const button=el('button','quiet small',t('doctor_run'));
  button.type='button';button.dataset.action='doctor.run';button.disabled=doctorDraft.sending;
  button.addEventListener('click',()=>void runDoctor());
  const status=doctorDraft.sending ? el('p','desc',t('doctor_running'))
    : doctorDraft.error ? el('p','desc text error',t(doctorDraft.error))
      : doctorDraft.report ? doctorReport(doctorDraft.report)
        : el('p','desc',t('doctor_not_run'));
  status.setAttribute('role','status');
  return el('section','doctor-diagnostics',el('h3',null,t('doctor_h')),
    el('p','desc',t('doctor_desc')),button,status);
}
function renderDoctor() {
  if(view!=='settings')return;
  const section=$('panel').querySelector('.doctor-diagnostics');
  if(!section)return;
  const focused=section.contains(document.activeElement) && document.activeElement?.dataset.action==='doctor.run';
  section.replaceChildren(...doctorSection().childNodes);
  if(focused)section.querySelector('[data-action="doctor.run"]')?.focus({preventScroll:true});
}
async function runDoctor() {
  if(doctorDraft.sending)return;
  const read=doctorDraft.read=doctorDraft.read+1;
  doctorDraft.sending=true;doctorDraft.report=null;doctorDraft.error=null;
  renderDoctor();
  try {
    const {res,answer}=await memoryWrite('doctor',{});
    if(read!==doctorDraft.read)return;
    if(!res.ok)doctorDraft.error=res.status===503?'doctor_busy':'doctor_failed';
    else if(answer && typeof answer==='object' && answer.stores && answer.checks
      && answer.inventory && Array.isArray(answer.inventory.agents)
      && ['raw','knowledge','legacy','embeddings','providers'].every(key=>answer.checks[key]))
      doctorDraft.report=answer;
    else doctorDraft.error='doctor_failed';
  } catch {
    if(read===doctorDraft.read)doctorDraft.error='doctor_unknown';
  } finally {
    if(read===doctorDraft.read){doctorDraft.sending=false;renderDoctor();}
  }
}

async function showSettings() {
  void refreshAgentInventory();
  const [s, privacy, hidden] = await Promise.all([api('settings'), api('privacy').catch(privacyUnavailable),
    api('hidden').catch(() => null)]);
  s.privacy = privacy;
  s.hidden = hidden;
  return () => {
    recoveryDraft.preview=null;
    recoveryDraft.confirmed=false;
    form = formOf(s);
    drawSettings();
    setStatus('');
    if (form?.embedding) void refreshEmbedding();
  };
}

let recoveryDraft = {preview:null,confirmed:false,busy:false,unknown:false,error:null,receipt:null};

// The page goes on only at an address the viewer gave: 127.0.0.1, a port and a token.
const PAGE_URL = /^http:\/\/127\.0\.0\.1:([1-9]\d{0,4})\/#t=[0-9a-f]{32}$/;
function movePage(url) {
  const port = Number(PAGE_URL.exec(url)?.[1]);
  if (Number.isNaN(port) || port > 65535) return false;
  location.replace(`${url}&moved=1`);
  return true;
}

let viewDraft = {confirmed:false,busy:false,error:null,status:null,url:null};

// The resident page's new token, or a foreground run's way to the resident page.
function viewSection(f) {
  const d=viewDraft, mode=f.viewRuntime?.mode;
  const resident=mode==='resident';
  if(!resident && !(mode==='foreground' && f.residentSupported && f.saved.worker.resident))return null;
  const go=el('button','quiet',t(resident?'view_token_go':'view_resident_go'));
  go.type='button';go.dataset.action=resident?'view.token':'view.resident';
  go.disabled=d.busy||resident&&!d.confirmed;
  go.addEventListener('click',()=>void viewAction(resident?'token':'resident'));
  let consent=null;
  if(resident) {
    consent=checkbox(d.confirmed,value=>{d.confirmed=value;go.disabled=!value||d.busy;});
    consent.dataset.field='view.confirmed';consent.disabled=d.busy;
  }
  let open=null;
  if(d.url){open=el('a',null,t('view_open_new'));open.href=d.url;}
  return el('section','view-address',el('h3',null,t(resident?'view_token_h':'view_resident_h')),
    el('p','desc',t(resident?'view_token_desc':'view_resident_desc',{port:f.saved.view.port})),
    consent?el('label','check',consent,t('view_token_confirm')):null,go,
    d.error?el('p','error',t(d.error,{port:f.saved.view.port,status:d.status})):null,open);
}

const VIEW_CODES={unchanged:'view_token_unchanged',token_unsure:'view_token_unsure',port_in_use:'view_port_in_use',
  not_private:'view_not_private',not_started:'view_not_started',not_resident:'view_not_resident'};

async function viewAction(kind) {
  const d=viewDraft;
  if(d.busy || kind==='token' && !d.confirmed)return;
  d.busy=true;d.error=null;d.url=null;
  if(view==='settings')drawSettings();
  try {
    const {res,answer}=await memoryWrite(`view/${kind}`,{});
    if(res.ok && movePage(answer.url))return;
    d.error=VIEW_CODES[answer.code]||'memory_action_failed';d.status=res.status;
    if(answer.code==='token_unsure' && PAGE_URL.test(answer.url||''))d.url=answer.url;
  } catch {
    d.error='view_lost';
  } finally {
    d.busy=false;d.confirmed=false;
    if(view==='settings')drawSettings();
  }
}

function recoverySection() {
  const d=recoveryDraft;
  if(form && !d.receipt && !d.unknown)return null;
  const preview=el('button','quiet',t('recovery_preview'));
  preview.type='button';preview.dataset.action='recovery.preview';
  preview.disabled=d.busy || d.unknown || Boolean(form);
  preview.addEventListener('click',()=>void recoveryAction('preview'));
  const consent=checkbox(d.confirmed,value=>{d.confirmed=value;start.disabled=!value||d.busy;});
  consent.dataset.field='recovery.confirmed';consent.disabled=!d.preview||d.busy;
  const start=el('button','quiet',t('recovery_start'));
  start.type='button';start.dataset.action='recovery.start';
  start.disabled=!d.preview||!d.confirmed||d.busy;
  start.addEventListener('click',()=>void recoveryAction('start'));
  const inspect=el('button','quiet',t('recovery_inspect'));
  inspect.type='button';inspect.disabled=d.busy;
  inspect.addEventListener('click',()=>void recoveryAction('inspect'));
  return el('section','settings-recovery',el('h3',null,t('recovery_h')),
    !form?el('p','desc',t('recovery_desc')):null,
    d.receipt?el('p','desc',t('recovery_copy',{name:d.receipt})):null,
    d.unknown?el('p','pending',t('recovery_unknown')):null,
    d.error?el('p','error',t(d.error)):null,
    !form?preview:null,
    !form && d.preview?el('label','check',consent,t('recovery_confirm')):null,
    !form && d.preview?start:null,
    d.unknown||d.error?inspect:null);
}

function receiveRecoveryStart(d,answer,mine,mineGeneration) {
  d.preview=null;d.confirmed=false;
  const copyKnown=['complete','unknown'].includes(answer.phase)
    && typeof answer.backup==='string' && /^config\.toml\.recovery-[0-9a-f]{32}\.bak$/.test(answer.backup);
  if(copyKnown)d.receipt=answer.backup;
  if(answer.phase==='complete' && copyKnown
    && answer.settings && !answer.settings.error) {
    d.receipt=answer.backup;d.unknown=false;
    if(view==='settings' && form===mine && generation===mineGeneration)form=formOf(answer.settings);
  } else d.unknown=true;
}

async function requestRecovery(action,d,key,mine,mineGeneration) {
  const {res,answer}=await memoryWrite(`settings/recovery/${action}`,
    action==='preview'?{}:{preview_key:key,confirmed:true});
  if(!res.ok) {
    d.preview=null;d.confirmed=false;
    d.error=['recovery_unavailable','recovery_failed','recovery_confirmation','stale'].includes(answer.code)
      ?answer.code:'recovery_failed';
    if(d.error==='stale')d.error='recovery_stale';
    return;
  }
  if(action==='preview') {
    if(view==='settings' && form===mine && generation===mineGeneration
      && typeof answer.preview_key==='string' && /^[0-9a-f]{64}$/.test(answer.preview_key)
      && answer.replaces==='all_settings' && answer.copy==='current_bytes' && answer.ai==='off'
      && answer.prompt_text==='off' && answer.injection==='off' && answer.other_capture==='builtin_redaction') {
      d.preview={preview_key:answer.preview_key};d.unknown=false;
    } else d.error='recovery_failed';
  } else receiveRecoveryStart(d,answer,mine,mineGeneration);
}

async function recoveryAction(action) {
  const d=recoveryDraft;
  if(d.busy || action==='preview' && d.unknown || action==='start' && (!d.preview || !d.confirmed))return;
  const mine=form, mineGeneration=generation;
  const key=d.preview?.preview_key;
  d.busy=true;d.error=null;
  if(action!=='start'){d.preview=null;d.confirmed=false;}
  if(view==='settings')drawSettings();
  try {
    if(action==='inspect') {
      const settings=await api('settings');
      if(view==='settings' && form===mine && generation===mineGeneration)form=formOf(settings);
      // This reads current configuration, without claiming the previous request succeeded.
      d.unknown=false;
      return;
    }
    await requestRecovery(action,d,key,mine,mineGeneration);
  } catch {
    d.preview=null;d.confirmed=false;
    if(action==='start')d.unknown=true;
    else d.error='recovery_failed';
  } finally {
    d.busy=false;
    if(view==='settings')drawSettings();
  }
}

// --- Search by meaning (W4): the embedder's choice is reviewed, then taken once agreed --------

const EMBEDDING_CHOICES = { none: 'embedding_none', local: 'embedding_local', 'workers-ai': 'embedding_workers_ai' };
const EMBEDDING_REFUSALS = { no_account: 'embedding_no_account', no_key: 'embedding_no_key',
  local_unavailable: 'embedding_local_unavailable', stale: 'embedding_stale', file_invalid: 'file_error' };
// `status` is the viewer's GET /api/embedding; `watch` keeps reading it after a start whose answer
// did not come back (a long download can outlast the browser's wait, while the viewer goes on).
let embeddingDraft = { choice: null, preview: null, confirmed: false, busy: false, error: null,
  status: null, watch: false, started: false };

// A size in MB, or in GB from 1 GB up, in units of 1,000 as the "about 2.3 GB" the page says.
function bytesText(n) {
  const gb = n >= 1e9;
  return new Intl.NumberFormat(lang, { style: 'unit', unit: gb ? 'gigabyte' : 'megabyte',
    maximumFractionDigits: gb ? 1 : 0 }).format(n / (gb ? 1e9 : 1e6));
}

function embeddingMessage(code) {
  const key = EMBEDDING_REFUSALS[code] || code;
  return t(Object.hasOwn(TEXT, key) ? key : 'embedding_request_failed');
}

// What the viewer said the choice does, in the page's words.
function embeddingConsent(consent) {
  const lines = [];
  if (consent.choice === 'none') {
    lines.push(t('embedding_none_desc'));
    if (consent.held > 0) lines.push(t('embedding_consent_none_files', { dir: consent.dir, size: bytesText(consent.held) }));
  } else if (consent.choice === 'workers-ai') {
    lines.push(t('embedding_consent_workers_ai', { account: consent.account, file: consent.key_file,
      requests: consent.daily_requests.toLocaleString(lang), usd: usdValue(consent.monthly_usd) }));
  } else if (consent.get.length) {
    for (const d of consent.get) {
      lines.push(t('embedding_consent_get', { size: bytesText(d.bytes), host: d.host, source: t(`embedding_source_${d.source}`) }));
    }
    lines.push(t('embedding_consent_into', { dir: consent.dir }), t('embedding_consent_local'));
  } else if (consent.check) {
    lines.push(t('embedding_consent_check', { dir: consent.dir }), t('embedding_consent_local'));
  } else lines.push(t('embedding_consent_ready', { dir: consent.dir }));
  return el('div', 'embedding-consent', ...lines.map((line) => el('p', 'desc', line)));
}

function embeddingRun(d) {
  const active = d.status?.active;
  if (active) {
    if (!active.get) return el('p', 'pending', t('embedding_checking'));
    const done = Math.min(Math.max(0, (d.status.held ?? 0) - active.held), active.get);
    return el('p', 'pending', t('embedding_running', { done: bytesText(done), total: bytesText(active.get) }));
  }
  const last = d.status?.last;
  if (!last || (last.phase === 'done' && !d.started)) return null;
  if (last.phase === 'done') return el('p', 'desc', t('embedding_done'));
  return el('p', 'error', embeddingMessage(last.code || 'embedding_unknown'));
}

// The part the poll redraws: the choice, its review and the run. The Workers AI fields stay, so a
// token being typed is not cleared.
function embeddingChoice(f, simple = false) {
  const d = embeddingDraft, e = f.embedding;
  const locked = d.busy || Boolean(d.status?.active);
  const choice = d.choice ?? e.provider;
  const pick = el('select', null, ...Object.entries(EMBEDDING_CHOICES).map(([value, key]) => {
    const option = el('option', null, t(key));
    option.value = value;
    return option;
  }));
  pick.value = choice;
  pick.dataset.field = 'embedding.choice';
  pick.disabled = locked;
  pick.addEventListener('change', () => {
    d.choice = pick.value;
    d.preview = null;
    d.confirmed = false;
    d.error = null;
    renderEmbedding();
  });
  const reviewed = d.preview?.choice === choice;
  const preview = el('button', 'quiet', t('embedding_preview'));
  preview.type = 'button';
  preview.dataset.action = 'embedding.preview';
  preview.disabled = locked;
  preview.addEventListener('click', () => void embeddingAction('preview'));
  const start = el('button', 'quiet', t('embedding_start'));
  start.type = 'button';
  start.dataset.action = 'embedding.start';
  start.disabled = !d.confirmed || locked;
  start.addEventListener('click', () => void embeddingAction('start'));
  const consent = checkbox(d.confirmed, (value) => { d.confirmed = value; start.disabled = !value || locked; });
  consent.dataset.field = 'embedding.confirmed';
  consent.disabled = locked;
  const local = e.local;
  const files = local.unavailable ? t(`embedding_unavailable_${local.unavailable}`)
    : t(`embedding_files_${local.files}`, { dir: local.dir, size: bytesText(local.held) });
  if (simple) return simpleEmbeddingChoice(f, { locked, choice, reviewed, preview, start, consent });
  return el('div', 'embedding-choice',
    el('label', 'field', el('span', null, t('embedding_choice')), pick,
      note(t('saved_value', { value: t(EMBEDDING_CHOICES[e.provider] || 'embedding_none') }))),
    el('p', 'desc', t(`${EMBEDDING_CHOICES[choice]}_desc`)),
    choice === 'local' ? el('p', 'desc', files) : null,
    reviewed ? embeddingConsent(d.preview.consent) : null,
    reviewed ? el('label', 'check', consent, t('embedding_confirm')) : null,
    reviewed ? start : preview,
    embeddingRun(d),
    d.error ? el('p', 'error', embeddingMessage(d.error)) : null);
}

function renderEmbedding() {
  if (!form?.embedding || view !== 'settings') return;
  const block = $('panel').querySelector('.embedding-choice');
  if (!block) return;
  const focused = block.contains(document.activeElement) ? document.activeElement : null;
  const simple = settingsMode === 'simple';
  const unclaimed = !document.activeElement || document.activeElement === document.body;
  const restore = simple ? focused?.dataset.simpleFocus || (unclaimed ? embeddingDraft.focus : null) : null;
  // Keep focus through a disabled redraw only while this request owns it. A later poll or a
  // user working in another block must not pull the keyboard back to the search choices.
  if (simple) embeddingDraft.focus = embeddingDraft.busy || embeddingDraft.status?.active ? restore : null;
  const key = focused?.dataset.action ? ['action', focused.dataset.action]
    : focused?.dataset.field ? ['field', focused.dataset.field] : null;
  block.replaceChildren(...embeddingChoice(form, simple).childNodes);
  if (simple) { simpleFocus(restore); return; }
  const next = key && block.querySelector(`[data-${key[0]}="${key[1]}"]`);
  if (next && !next.disabled) next.focus({ preventScroll: true });
}

function workersAiSettings(f) {
  const w = f.embedding.workers, edit = f.embedding.edit;
  const saved = (value) => note(t('saved_value', { value }));
  const account = input('text', edit.account_id, '', 'embedding.account_id', (v) => { edit.account_id = v; });
  const daily = input('number', edit.daily_requests, '', 'embedding.daily_requests', (v) => { edit.daily_requests = v; });
  const [least, most] = f.ranges.daily_requests;
  [daily.min, daily.max] = [least, most];
  const monthly = input('number', edit.monthly_usd, '', 'embedding.monthly_usd', (v) => { edit.monthly_usd = v; });
  monthly.step = 'any';
  monthly.inputMode = 'decimal';
  const usage = w.usd_this_month === null || w.requests_today === null ? t('embedding_usage_unknown')
    : t('embedding_usage', { usd: usdValue(w.usd_this_month), requests: w.requests_today.toLocaleString(lang) });
  const details = el('details', 'workers-ai', el('summary', null, t('embedding_workers_ai_h')),
    el('label', 'field', el('span', null, t('embedding_account')), account, saved(w.account_id ?? '—')),
    f.embedding.provider === 'workers-ai' ? note(t('embedding_account_destination')) : null,
    el('div', 'grid',
      el('label', 'field', el('span', null, t('embedding_daily_requests', { min: least.toLocaleString(lang), max: most.toLocaleString(lang) })),
        daily, saved(w.daily_requests.toLocaleString(lang))),
      el('label', 'field', el('span', null, t('embedding_monthly_usd')), monthly, saved(usdValue(w.monthly_usd)))),
    el('p', 'desc', usage),
    el('p', 'desc', t('embedding_key_file', { state: t(w.key === 'ok' ? 'key_ok' : 'key_missing'), file: w.key_file })),
    note(t('embedding_key_desc')),
    f.keyInput ? keyField('embedding/key', 'embedding.key', t('embedding_key_label')) : note(t('provider_key_unsupported')));
  details.open = (embeddingDraft.choice ?? f.embedding.provider) === 'workers-ai';
  return details;
}

// docs/my-work-model.md J1: Jev, Typesafe's judge. Off until its key is set here; then its switch,
// its key file's state, its calls and spend, and one test of a made-up question.
function judgeSection(f) {
  const j = f.judge;
  if (!j) return null;
  const parts = [el('h3', null, t('judge_h')), el('p', 'desc', t('judge_desc')), el('p', 'desc', t('judge_sends'))];
  if (j.set) {
    parts.push(el('label', 'check', checkbox(j.enabled, (v) => { j.enabled = v; }), t('judge_on')),
      note(t('saved_value', { value: t(f.savedSettings.judge?.enabled ? 'value_on' : 'value_off') })),
      el('p', 'desc', t('judge_key_file', { state: t(j.key === 'ok' ? 'key_ok' : 'key_missing'), file: j.key_file })));
  } else {
    parts.push(el('p', 'desc', t('judge_no_key')));
  }
  parts.push(f.keyInput ? keyField('judge/key', 'judge.key', t('judge_key_label')) : note(t('provider_key_unsupported')));
  if (j.set) parts.push(...judgeCalls(f, j), judgeTest());
  return el('section', 'judge-settings', ...parts);
}

// The judge's calls a day, with its No cap switch (decision 64) and its usage beside it.
function judgeCalls(f, j) {
  const open = j.daily_budget === f.ranges.no_cap;
  let usage = t('judge_usage_unknown');
  if (j.calls_today !== null && j.usd_this_month !== null) {
    usage = t(open ? 'judge_usage_no_cap' : 'judge_usage', { calls: j.calls_today.toLocaleString(lang),
      most: j.daily_budget.toLocaleString(lang), usd: usdValue(j.usd_this_month) });
  }
  const [least, most] = f.ranges.daily_budget;
  const calls = input('number', j.budget, '', 'judge.daily_budget', (v) => { j.budget = v; });
  [calls.min, calls.max] = [least, most];
  return [el('label', 'field', el('span', null, t('judge_calls', { min: least.toLocaleString(lang), max: most.toLocaleString(lang) })),
    calls, note(t('saved_value', { value: open ? t('no_cap') : j.daily_budget })), note(t('judge_calls_desc'))),
  ...noCapSwitch(j.noCap, 'judge', t('judge_h'), calls, 'no_cap_calls_warning', (v) => { j.noCap = v; }),
  el('p', 'desc', usage)];
}

// One made-up question sent on the owner's click, its outcome said in words.
function judgeTest() {
  const button = el('button', 'small', t('judge_test'));
  button.type = 'button';
  button.dataset.field = 'judge.test';
  button.addEventListener('click', async () => {
    // The visit too (#766): settings left and opened again keep the old form until the new one
    // is read, and a test of the old visit says and merges nothing into the new.
    const mine = form, mineGeneration = generation;
    const here = () => currentSettings(mine) && generation === mineGeneration;
    button.disabled = true;
    try {
      const { res, answer } = await memoryWrite('judge/test', { confirmed: true });
      if (!here()) return;
      const outcome = res.ok ? answer.outcome : answer.code || PROVIDER_HTTP_ERRORS[res.status];
      const key = Object.hasOwn(TEXT, `judge_test_${outcome}`) ? `judge_test_${outcome}` : 'judge_test_failed';
      // The call is counted: the judge's numbers read again, with what is being edited kept. A
      // save or a key saved meanwhile has the page now, its own answer included: this test says
      // nothing more, and a save still running keeps its answer's place.
      const settings = res.ok ? await api('settings').catch(() => null) : null;
      if (!here()) return;
      if (settings && !settings.error && !button.closest('.settings')?.inert) {
        form = mergeProviderSettings(settings, mine, { op: 'key' });
        drawSettings();
      }
      setStatus(t(key), outcome !== 'ok', lang);
    } catch {
      if (here()) setStatus(t('network_failed'), true, lang);
    } finally {
      button.disabled = false;
    }
  });
  return el('div', 'judge-test', button, note(t('judge_test_desc')));
}

function embeddingSection(f) {
  if (!f.embedding) return null;
  return el('section', 'embedding-settings',
    el('h3', null, t('embedding_h')),
    el('p', 'desc', t('embedding_desc')),
    embeddingChoice(f),
    el('p', 'desc', t('embedding_index')),
    workersAiSettings(f));
}

// As a provider's key field: the token is only in the field and the one request that carries it.
function keyField(route, field, label) {
  const i = el('input');
  i.type = 'text';
  i.autocomplete = 'off';
  i.spellcheck = false;
  i.autocapitalize = 'off';
  i.placeholder = t('key_placeholder');
  i.dataset.field = field;
  i.setAttribute('aria-label', label);
  const save = el('button', 'small', t('key_save'));
  save.type = 'button';
  save.disabled = true;
  i.addEventListener('input', () => { save.disabled = !i.value; });
  i.addEventListener('keydown', (event) => {
    if (event.key === 'Enter') { event.preventDefault(); if (!save.disabled) save.click(); }
  });
  save.addEventListener('click', () => {
    const body = { key: i.value, version: form.version };
    i.value = '';
    void keySave(route, save, body);
  });
  return el('div', 'key-input', i, save);
}

async function keySave(route, button, body) {
  const mine = form;
  const fields = button.closest('.settings');
  button.disabled = true;
  fields.inert = true;
  try {
    const { res, answer } = await memoryWrite(route, body);
    if (!currentSettings(mine)) return;
    if (res.status === 409) return reloadProviderSettings(await api('settings'));
    if (!res.ok) {
      fields.inert = false;
      if (answer.field) markInvalid(answer.field, fields);
      setStatus(providerReason(answer.code || PROVIDER_HTTP_ERRORS[res.status], 'key'), true, lang);
      return;
    }
    form = mergeProviderSettings(answer, mine, { op: 'key' });
    drawSettings();
    setStatus(t(answer.key_saved?.durable ? 'key_saved' : 'key_not_durable'), !answer.key_saved?.durable, lang);
  } catch {
    if (currentSettings(mine)) setStatus(t('network_failed'), true, lang);
  } finally {
    button.disabled = false;
    fields.inert = false;
  }
}

// After a choice: the saved method and the files' state as they are now; typed values stay.
// Another save's answer can replace the form while this reads, and be older or newer than this
// read (a save made before a download wrote the method), so this reads again for the form then
// shown until a read comes back with the form as it was. It answers whether it read them: a run
// whose read failed stays watched, so the next poll reads them again.
async function reloadEmbedding() {
  let mine, settings;
  do {
    mine = form;
    settings = await api('settings').catch(() => null);
    if (!settings || settings.error || view !== 'settings') return false;
  } while (form !== mine);
  form = mergeProviderSettings(settings, mine, { op: 'embedding' });
  if (embeddingDraft.status?.last?.phase === 'done') simpleReceipt = 'search';
  drawSettings();
  return true;
}

async function refreshEmbedding() {
  const d = embeddingDraft;
  try {
    const status = await api('embedding');
    // A run seen running, or a start whose answer did not come back (`none` and `workers-ai`
    // are never seen running), has ended: the saved method is read again.
    const ended = (Boolean(d.status?.active) || d.watch) && !status.active && !d.busy;
    d.status = status;
    if (ended) {
      // Cleared before the read, so a choice started meanwhile keeps its own watch.
      d.watch = false;
      if (!(await reloadEmbedding())) d.watch = true;
    } else renderEmbedding();
  } catch {
    // The next poll reads it again.
  }
}

function embeddingRefused(d, res, answer) {
  d.preview = null;
  d.confirmed = false;
  d.error = answer.code || PROVIDER_HTTP_ERRORS[res.status] || 'embedding_request_failed';
  if (answer.code === 'no_account') markInvalid('embedding.account_id');
}

async function embeddingAction(action) {
  const d = embeddingDraft, mine = form;
  if (d.busy || d.status?.active || !mine?.embedding) return;
  const choice = d.choice ?? mine.embedding.provider;
  const reviewed = d.preview?.choice === choice ? d.preview : null;
  if (action === 'start' && !(reviewed && d.confirmed)) return;
  d.busy = true;
  d.error = null;
  d.preview = null;
  d.confirmed = false;
  if (action === 'start') d.started = true;
  if (action === 'start' && choice === 'local') {
    // Until the first poll: what the review said is to download.
    const get = reviewed.consent.get.reduce((sum, x) => sum + x.bytes, 0);
    d.status = { ...d.status, active: { choice, phase: 'running', held: mine.embedding.local.held, get },
      held: mine.embedding.local.held };
  }
  renderEmbedding();
  try {
    if (action === 'preview') {
      const { res, answer } = await memoryWrite('embedding/preview', { choice });
      if (!currentSettings(mine)) return;
      if (res.ok && /^[0-9a-f]{64}$/.test(answer.preview_key ?? '') && answer.consent?.choice === choice) {
        d.preview = { choice, key: answer.preview_key, consent: answer.consent };
      } else embeddingRefused(d, res, answer);
      return;
    }
    const { res, answer } = await memoryWrite('embedding', { choice, preview_key: reviewed.key, confirmed: true });
    if (res.ok) { d.status = answer; if (!answer.active) simpleReceipt = 'search'; }
    else {
      embeddingRefused(d, res, answer);
      d.status = await api('embedding').catch(() => null);
    }
    if (res.ok && !(await reloadEmbedding())) d.watch = true;
  } catch {
    if (action === 'start') d.watch = true;
    else d.error = 'embedding_request_failed';
  } finally {
    d.busy = false;
    renderEmbedding();
  }
}

function privacyUnavailable() {
  return { available: false, repositories: [], folders: [], rescan: { state: 'unavailable', processed: null, total: null } };
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

let forgetAction = null;
let forgetPreviewId = 0;
// How long a forget's result stays through the redraws the store's change brings.
const FORGET_HOLD = 15000;

function forgetControl(kind, id) {
  const button = word('button', 'quiet small', kind === 'session' ? 'forget_session_button' : 'forget_button');
  button.type = 'button';
  button.setAttribute('aria-expanded', 'false');
  const control = el('div', 'forget-control', button);
  button.addEventListener('click', async () => {
    if (button.disabled || forgetAction?.busy) return;
    forgetAction?.close(false);
    const mine = generation;
    const from = view;
    const current = () => mine === generation && view === from && control.isConnected;
    let preview = null;
    const action = { busy: true, close: (focus = true) => {
      preview?.remove();
      button.disabled = false;
      button.setAttribute('aria-expanded', 'false');
      button.removeAttribute('aria-controls');
      if (forgetAction === action) forgetAction = null;
      if (focus && button.isConnected) button.focus();
    } };
    forgetAction = action;
    button.disabled = true;
    try {
      const { res, answer } = await memoryWrite('forget/preview', { item: { kind, id } });
      if (!current()) return;
      if (!res.ok) { setStatus(memoryFailure(res, answer), true, lang); return; }
      const confirm = word('button', 'quiet small danger', 'forget_confirm');
      const cancel = word('button', 'quiet small', 'forget_cancel');
      confirm.type = cancel.type = 'button';
      const description = el('div', null, word('p', null, `forget_preview_${kind}`, answer));
      if (kind === 'prompt' && answer.copies > 0) description.append(word('p', null, 'forget_preview_copies', answer));
      if (['prompt', 'session'].includes(kind) && answer.written > 0) description.append(word('p', null, 'forget_preview_written', answer));
      if (kind === 'claim' && answer.again > 0) description.append(word('p', null, 'forget_preview_again', answer));
      if (kind === 'claim') description.append(word('p', null, 'forget_claim_records'));
      if (kind === 'note') description.append(word('p', null, 'forget_note_outside'));
      if (['prompt', 'session'].includes(kind)) description.append(word('p', null, 'forget_outside'));
      description.append(word('p', null, 'forget_limits'));
      const warning = word('p', null, 'forget_irreversible');
      description.id = `forget-description-${++forgetPreviewId}`;
      warning.id = `${description.id}-warning`;
      for (const choice of [confirm, cancel]) choice.setAttribute('aria-describedby', `${description.id} ${warning.id}`);
      preview = el('section', 'forget-preview', description, warning, el('div', 'actions', confirm, cancel));
      preview.id = `${description.id}-block`;
      confirm.addEventListener('click', async () => {
        if (action.busy || forgetAction !== action) return;
        action.busy = true;
        confirm.disabled = cancel.disabled = true;
        let receipt = null;
        try {
          const { res, answer: result } = await memoryWrite('forget/start',
            { item: { kind, id }, preview_key: answer.preview_key, confirmed: true });
          if (!res.ok) { if (current()) setStatus(memoryFailure(res, result), true, lang); return; }
          receipt = result;
        } catch {
          if (current()) setStatus(t('memory_result_unknown'), true, lang);
        } finally { action.close(!receipt && current()); }
        if (receipt) {
          // The cleaning started now, after one that runs, or at the worker's next look.
          const done = t(!receipt.cleaning ? 'forget_done_later' : receipt.after ? 'forget_done_waiting' : 'forget_done')
            + (receipt.logged ? '' : ` ${t('forget_not_logged')}`);
          // Back normally reuses its list; forgotten records need a fresh read.
          ses.items = [];
          setStatus(done, false, lang, FORGET_HOLD);
          let reread;
          if (kind === 'session' && view === 'sessions' && ses.open?.key === id) {
            reread = sessionClose();
          } else if (kind === 'prompt' && view === 'sessions' && ses.open) {
            const key = ses.open.key;
            const back = ses.back;
            ses.open = null;
            if ((await refresh()) === false || ses.back !== back || !activeSessions(sessionsList)) return;
            const state = sessionsList;
            let entry = ses.items.find((s) => s.key === key);
            while (!entry && ses.next !== null) {
              if (!(await readSessions(state, 'more')) || !activeSessions(state) || ses.back !== back) return;
              entry = ses.items.find((s) => s.key === key);
            }
            if (entry) {
              ses.open = entry;
              reread = show();
            } else {
              ses.back = null;
              reread = true;
            }
          } else reread = refresh();
          if ((await reread) !== false) setStatus(done, false, lang, FORGET_HOLD);
        }
      });
      cancel.addEventListener('click', () => { if (!action.busy) action.close(); });
      preview.addEventListener('keydown', (event) => {
        if (event.key === 'Escape' && !action.busy) { event.preventDefault(); action.close(); }
      });
      control.append(preview);
      button.setAttribute('aria-expanded', 'true');
      button.setAttribute('aria-controls', preview.id);
      action.busy = false;
      cancel.focus();
    } catch {
      if (current()) setStatus(t('network_failed'), true, lang);
    } finally { if (!preview) action.close(current()); }
  });
  return control;
}

// Parity row 3 (docs/card-hide.md): a card or a summary removed from the page alone, and brought
// back. How long its result, with Undo, stays through the redraws the store's change brings.
const HIDE_HOLD = 15000;

async function hideWrite(kind, id, hidden) {
  try {
    const { res, answer } = await memoryWrite('hide', { item: { kind, id }, hidden });
    if (res.ok) return answer;
    // Held as a result is, so the reread a removal starts does not clear why its Undo failed.
    setStatus(memoryFailure(res, answer), true, lang, HIDE_HOLD);
  } catch {
    setStatus(t('memory_result_unknown'), true, lang, HIDE_HOLD);
  }
  return null;
}

function hideControl(kind, id) {
  const button = word('button', 'quiet small', 'hide_button');
  button.type = 'button';
  button.addEventListener('click', async () => {
    if (button.disabled) return;
    button.disabled = true;
    const answer = await hideWrite(kind, id, true);
    button.disabled = false;
    if (!answer) return;
    const undo = word('button', 'quiet small', 'hide_undo');
    undo.type = 'button';
    undo.addEventListener('click', async () => {
      if (undo.disabled) return;
      undo.disabled = true;
      const back = await hideWrite(kind, id, false);
      if (!back) return;
      const restored = t(back.state === 'pending' ? 'hide_pending' : 'hide_restored');
      setStatus(restored, false, lang, HIDE_HOLD);
      // A reread sets the status line itself.
      if ((await refresh()) !== false) setStatus(restored, false, lang, HIDE_HOLD);
    });
    const done = t(answer.state === 'pending' ? 'hide_pending' : 'hide_done');
    setStatus(done, false, lang, HIDE_HOLD, undo);
    // Unless Undo was pressed meanwhile, which says its own result.
    if ((await refresh()) !== false && !undo.disabled) setStatus(done, false, lang, HIDE_HOLD, undo);
  });
  return button;
}

async function refreshHidden(f) {
  const listed = await api('hidden').catch(() => null);
  if (!currentSettings(f)) return false;
  f.hidden = listed;
  drawSettings();
  return listed !== null;
}

// #661: the next page of what was removed, after the last of each kind shown, added below them.
async function moreHidden(f, button) {
  const list = f.hidden;
  const before = list?.next;
  if (!before || button.disabled) return;
  button.disabled = true;
  const page = await api('hidden', { before }).catch(() => null);
  // A reread meanwhile (a Bring back) replaced the list this page would follow, though its first
  // page and next position stayed as they were (CodeRabbit on #672, #679).
  if (!currentSettings(f) || f.hidden !== list) return;
  if (!page) {
    button.disabled = false;
    setStatus(t('hidden_unavailable'), true, lang);
    return;
  }
  f.hidden = { items: [...f.hidden.items, ...page.items], next: page.next };
  drawSettings();
}

function hiddenSection(f, simple = false) {
  const listed = f.hidden;
  const rows = (listed?.items || []).map((item) => {
    const back = word('button', 'quiet small', 'hidden_restore');
    back.type = 'button';
    back.addEventListener('click', async () => {
      if (back.disabled) return;
      back.disabled = true;
      const answer = await hideWrite(item.kind, item.id, false);
      if (!currentSettings(f)) return;
      if (!answer) { back.disabled = false; return; }
      const receipt = t(answer.state === 'pending' ? 'hide_pending' : 'hide_restored');
      await refreshHidden(f);
      if (currentSettings(f)) setStatus(receipt, false, lang);
    });
    const time = localTime(item.ts, lang);
    return el('li', null, el('p', 'text', item.title || item.id),
      el('p', 'desc', word('span', null, `feed_${item.kind}`), item.repo_name ? ` · ${item.repo_name} · ` : ' · ', time), back);
  });
  let listing = word('p', 'desc', 'hidden_unavailable');
  if (listed) listing = rows.length ? el('ul', null, ...rows) : word('p', 'desc', 'hidden_none');
  if (listed?.next) {
    const more = word('button', 'quiet more', 'more');
    more.type = 'button';
    more.dataset.simpleFocus = 'hidden.more';
    more.addEventListener('click', () => void moreHidden(f, more));
    listing = el('div', null, listing, more);
  }
  if (simple) return simpleBlock('hidden', listing);
  return el('section', 'hidden-settings', word('h3', null, 'hidden_h'), word('p', 'desc', 'hidden_desc'), listing);
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
    // A repository never recorded is never sent: its own row takes that back first.
    if (repo.never_recorded) return el('li', null, el('p', 'text', repo.label), el('p', 'desc', t('privacy_never_recorded')));
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

// Slice 4b (spec 6.1): never record a repository or a folder of this device, or record it again,
// through `oboete capture exclude`'s backend.
function captureSection(f, simple = false) {
  const state = f.privacy;
  if (!state.available) {
    const section = simple ? simpleBlock('never', el('p', 'desc', t('privacy_unavailable')))
      : el('section', null, el('h3', null, t('never_h')), el('p', 'desc', t('privacy_unavailable')));
    section.className += ' never-record';
    return section;
  }
  const write = async (button, body, receipt) => {
    const fields = button.closest('.settings');
    fields.inert = true;
    button.disabled = true;
    try {
      const { res, answer } = await memoryWrite('privacy/capture', body);
      if (!currentSettings(f)) return;
      if (!res.ok) { setStatus(memoryFailure(res, answer), true, lang); return; }
      if (body.folder) f.neverFolder = '';
      if (body.pattern) f.neverPattern = '';
      // A folder or a repository another entry still covers is not recorded again (Codex and
      // CodeRabbit on #491).
      if (body.folder_selector && answer.never_recorded === true) receipt = t('never_undone_covered');
      if (body.selector && body.undo && answer.never_recorded === true) receipt = t('never_undone_repo_covered');
      if (simple) simpleReceipt = 'never';
      const refreshed = await refreshPrivacy(f);
      if (currentSettings(f)) setStatus(refreshed ? receipt : t('privacy_readback_failed', { receipt }), !refreshed, lang);
    } catch {
      if (currentSettings(f)) setStatus(t('memory_result_unknown'), true, lang);
    } finally { fields.inert = false; button.disabled = false; }
  };
  const row = (label, never, onClick) => {
    const action = el('button', 'quiet small', t(never ? 'never_undo' : 'never_exclude'));
    action.type = 'button';
    action.addEventListener('click', () => onClick(action));
    return el('li', null, el('p', 'text', label), el('p', 'desc', t(never ? 'never_on' : 'never_off')), action);
  };
  const repos = (state.repositories || []).filter(repo => !simple || repo.never_recorded).map((repo) => row(repo.label, repo.never_recorded, (action) =>
    write(action, { selector: repo.selector, undo: !!repo.never_recorded },
      t(repo.never_recorded ? 'never_undone' : 'never_repo_stopped', { label: repo.label }))));
  const folders = (state.folders || []).map((folder) => row(folder.label, true, (action) =>
    write(action, { folder_selector: folder.selector, undo: true }, t('never_undone'))));
  // Parity row 14: folder names and claude-mem's patterns.
  const patterns = (state.patterns || []).map((pattern) => row(pattern.label, true, (action) =>
    write(action, { pattern_selector: pattern.selector, undo: true }, t('never_undone'))));
  const folder = input('text', f.neverFolder || '', '', 'never.folder', (v) => { f.neverFolder = v; });
  const add = el('button', 'quiet small', t('never_folder_add'));
  add.type = 'button';
  add.addEventListener('click', () => {
    if (add.disabled) return;
    // Sent as typed, as the CLI takes it: a folder's name may end in a space (Codex on #491).
    if (!folder.value.trim()) { setStatus(t('capture_folder'), true, lang); return; }
    void write(add, { folder: folder.value, undo: false }, t('never_folder_stopped'));
  });
  if (simple) {
    folder.dataset.simpleFocus = 'never.folder';
    folder.addEventListener('blur', () => { if (folder.value.trim() && !add.disabled) add.click(); });
    folder.addEventListener('keydown', event => {
      if (event.key === 'Enter') { event.preventDefault(); if (!add.disabled) add.click(); }
    });
    const section = simpleBlock('never',
      repos.length || folders.length || patterns.length ? el('ul', 'simple-places', ...repos, ...folders, ...patterns) : el('p', 'desc', t('simple_never_empty')),
      el('label', 'field', el('span', null, t('never_folder_input')), folder), add,
      detailedSettingsLink('simple_never_more', '.never-record'));
    section.className += ' never-record';
    return section;
  }
  const pattern = input('text', f.neverPattern || '', '', 'never.pattern', (v) => { f.neverPattern = v; });
  const addPattern = el('button', 'quiet small', t('never_pattern_add'));
  addPattern.type = 'button';
  addPattern.addEventListener('click', () => {
    if (addPattern.disabled) return;
    if (!pattern.value.trim()) { setStatus(t('capture_pattern'), true, lang); return; }
    void write(addPattern, { pattern: pattern.value, undo: false }, t('never_pattern_stopped'));
  });
  return el('section', 'never-record', el('h3', null, t('never_h')), el('p', 'desc', t('never_desc')),
    el('h4', null, t('never_repos_h')), repos.length ? el('ul', null, ...repos) : el('p', 'desc', t('privacy_none')),
    el('h4', null, t('never_folders_h')), folders.length ? el('ul', null, ...folders) : el('p', 'desc', t('never_folders_none')),
    el('label', 'field', el('span', null, t('never_folder_input')), folder), add,
    el('h4', null, t('never_patterns_h')), el('p', 'desc', t('never_patterns_desc')),
    patterns.length ? el('ul', null, ...patterns) : el('p', 'desc', t('never_patterns_none')),
    el('label', 'field', el('span', null, t('never_pattern_input')), pattern), addPattern);
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

// docs/retention.md: a raw record's text goes once it is older than the period, by its own time, and
// curation and the session summaries are done with it; forever unless a day count is set.
function retentionSection(f) {
  const r = f.retention;
  const [least, most] = f.ranges.retention_days;
  const days = input('number', r.edit, '', 'retention.days', (v) => { r.edit = v; });
  [days.min, days.max] = [least, most];
  days.disabled = r.forever;
  const forever = checkbox(r.forever, (v) => { r.forever = v; days.disabled = v; });
  forever.dataset.field = 'retention.forever';
  let value = t('retention_forever_value');
  if (r.days !== null) value = t(r.days === 1 ? 'retention_day_value' : 'retention_days_value', { days: r.days.toLocaleString('en-US') });
  return el('section', 'retention',
    el('h3', null, t('retention_h')),
    el('p', 'desc', t('retention_desc')),
    el('label', 'check', forever, t('retention_forever')),
    el('label', 'field', el('span', null, t('retention_days', { min: least.toLocaleString('en-US'), max: most.toLocaleString('en-US') })), days,
      note(t('saved_value', { value }))),
    el('p', 'desc', t('retention_note')));
}

function checkbox(checked, onChange) {
  const c = el('input');
  c.type = 'checkbox';
  c.checked = checked;
  c.addEventListener('change', () => onChange(c.checked));
  return c;
}

// Decision 64: a calls-a-day or USD cap as its field shows it: empty when there is none.
function noCapText(value, ranges) {
  return value === null || value === undefined || value === ranges?.no_cap ? '' : String(value);
}

// Decision 64: a chain row's calls a day as its field and No cap switch show config.toml's. A
// subscription has no calls a day to set; any other entry with none shows it as off.
function chainDailyAsLoaded(r, ranges) {
  return { daily_budget: noCapText(r.daily_budget, ranges), noCap: !r.subscription && r.effective_daily_budget === null };
}

// Whether a chain row's calls a day is still as loaded. Its No cap follows the entry's own value
// too, which a save must not turn into a `[chain]` value unless the row was changed (Codex on #772).
// The edit keeps what it was loaded as through the merges of later answers, so a change made
// against one answer still counts after the next (#773).
function chainDailyUntouched(r) {
  return r.edit.daily_budget === r.edit.loaded.daily_budget && r.edit.noCap === r.edit.loaded.noCap;
}

// Decision 64: a cap's No cap switch beside its number. On, the number is set aside (kept, to turn
// the cap on again) and `warning` says what still stops those calls.
function noCapSwitch(on, field, subject, number, warning, set) {
  const said = el('span', 'note no-cap-warning', t(warning));
  const show = (v) => { number.disabled = v; said.hidden = !v; };
  const box = checkbox(on, (v) => { set(v); show(v); });
  box.dataset.field = `${field}.no_cap`;
  box.setAttribute('aria-label', `${t('no_cap')}: ${subject}`);
  show(on);
  return [el('label', 'check', box, t('no_cap')), said];
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
      if (settingsMode === 'simple') { simpleReceipt = 'ai'; drawSettings(); }
      else state.replaceWith(stoppedState());
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
    cli: saved.cli || 'claude', base_url: saved.base_url || '', api: saved.api || 'openai', model: saved.model || '',
    timeout_s: providerText(saved.timeout_s ?? 60), subscription: saved.subscription ?? false,
    credits: saved.credits ?? false,
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
  if (draft.kind === 'cli') {
    entry.cli = draft.cli;
    if (draft.cli === 'codex') entry.credits = draft.credits;
  }
  else Object.assign(entry, { base_url: draft.base_url.trim(), api: draft.api, subscription: draft.subscription,
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
  for (const key of ['worker', 'view', 'summary', 'paid_usd_per_month', 'paidNoCap', 'inject', 'capture', 'backup', 'redaction', 'retention']) next[key] = mine[key];
  if (next.embedding && mine.embedding) next.embedding.edit = mine.embedding.edit;
  next.neverFolder = mine.neverFolder;
  next.neverPattern = mine.neverPattern;
  // My Work's switch and bound as edited, with the estimate the answer brings (Codex on #671).
  if (next.my_work && mine.my_work) next.my_work = { ...next.my_work, curate: mine.my_work.curate, calls_a_day: mine.my_work.calls_a_day, noCap: mine.my_work.noCap };
  // The judge's switch as edited; one left as it was follows the answer (a key saved turns it on).
  if (next.judge && mine.judge && mine.judge.enabled !== mine.savedSettings.judge?.enabled) next.judge.enabled = mine.judge.enabled;
  // Its calls a day as edited too; left as they were, they follow the answer.
  const daily = mine.savedSettings.judge?.daily_budget;
  if (next.judge && mine.judge && (mine.judge.budget !== noCapText(daily, mine.ranges) || mine.judge.noCap !== (daily === mine.ranges.no_cap))) {
    Object.assign(next.judge, { budget: mine.judge.budget, noCap: mine.judge.noCap });
  }
  next.gemini = mine.gemini === (mine.saved.gemini ?? 'none') ? next.gemini : mine.gemini;
  const groups = new Map(next.chain.map((r) => [r.name, r]));
  next.chain = [...mine.chain.filter((r) => groups.has(r.name)).map((r) => {
    const row = groups.get(r.name);
    // A calls a day left as it was follows the answer, whose entry may have changed.
    const fresh = row.edit;
    row.edit = { ...r.edit };
    if (chainDailyUntouched(r)) Object.assign(row.edit, { daily_budget: fresh.daily_budget, noCap: fresh.noCap, loaded: fresh.loaded });
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
  next.simpleCatalog = action.op === 'create' ? null : mine.simpleCatalog;
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
  if (settingsMode === 'simple') clearSimpleReceipt();
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
    if (settingsMode === 'simple') {
      simpleReceipt = path === '/key' && !answer.key_saved?.durable ? null : 'ai';
      if (path === '/key') simpleKeyName = null;
      if (action.op === 'create' && action.entry.kind === 'openai') simpleKeyName = action.entry.name;
    }
    drawSettings();
    setStatus(t(providerSuccessKey(action, path, answer)), path === '/key' && !answer.key_saved?.durable, lang);
    return true;
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
  i.dataset.simpleFocus = `key:${provider.name}`;
  i.setAttribute('aria-label', t('key_label', { name: provider.name }));
  const save = el('button', 'small', t('key_save'));
  save.type = 'button';
  save.dataset.simpleFocus = `key-save:${provider.name}`;
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

function providerTestPreview(provider, preview, simple = false) {
  const run = el('button', 'small', t('provider_test_run'));
  run.type = 'button';
  if (simple) run.dataset.simpleFocus = `test-run:${provider.name}`;
  run.disabled = !preview.ready;
  run.addEventListener('click', () => void previewProvider(provider, run, true));
  return el('div', 'test-preview',
    el('p', null, t('provider_test_destination', { destination: preview.destination })),
    el('p', null, t('provider_test_model', { model: preview.model ?? t('provider_default_model') })),
    el('h4', null, t('provider_test_fixture')), el('pre', 'document-text', preview.fixture),
    note(t('provider_test_tokens', { tokens: preview.estimated_input_tokens, output: preview.max_output_tokens ?? t('provider_test_unknown_cost') })),
    el('p', null, t(preview.possible_charge ? 'provider_test_charge' : 'provider_test_no_charge',
      { usd: usdValue(preview.estimated_usd), cap: preview.monthly_cap_usd === null ? t('no_cap') : usdValue(preview.monthly_cap_usd) })),
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

function providerTest(provider, simple = false) {
  const button = el('button', 'quiet small', t('provider_preview'));
  button.type = 'button';
  if (simple) button.dataset.simpleFocus = `test:${provider.name}`;
  button.addEventListener('click', () => void previewProvider(provider, button));
  const nodes = [simple ? null : note(t('provider_preview_desc')), button];
  if (provider.preview) nodes.push(providerTestPreview(provider, provider.preview, simple));
  if (provider.result) nodes.push(providerTestResult(provider.result));
  return el('div', 'provider-test', ...nodes);
}

function providerLimitGrid(draft, unsupported) {
  const grid = el('div', 'grid');
  const outputField = el('label', 'field');
  const syncOutput = () => {
    // Normal dispatch enforces this maximum for priced HTTP and for every Anthropic Messages entry,
    // which always sends it. Keep other entries' saved estimates in the typed draft without
    // offering an ineffective generation-limit control.
    const capped = draft.kind === 'openai' && (draft.api === 'anthropic'
      || Number(draft.limits.usd_per_mtok_in) > 0 || Number(draft.limits.usd_per_mtok_out) > 0);
    if (capped) {
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
  grid.syncOutput = syncOutput;
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
  const http = draft.kind === 'openai';
  const endpoint = http ? field('base_url', 'provider_endpoint', 'text', 'provider_endpoint_desc') : null;
  const api = el('select', null, ...['openai', 'anthropic'].map((value) => {
    const option = el('option', null, t(`provider_api_${value}`));
    option.value = value;
    return option;
  }));
  api.value = draft.api;
  api.dataset.field = 'providers.api';
  api.disabled = Boolean(unsupported);
  api.addEventListener('change', () => { draft.api = api.value; grid.syncOutput(); });
  // An endpoint that comes onto Anthropic's own host preselects its API; editing it there keeps
  // the user's choice (CodeRabbit on #409).
  const onAnthropic = () => /^https:\/\/api\.anthropic\.com(\/|$)/i.test(draft.base_url.trim());
  let wasOnAnthropic = onAnthropic();
  endpoint?.querySelector('input').addEventListener('input', () => {
    const now = onAnthropic();
    if (now && !wasOnAnthropic) {
      api.value = draft.api = 'anthropic';
      grid.syncOutput();
    }
    wasOnAnthropic = now;
  });
  const main = el('div', 'grid', field('name', 'provider_name'), endpoint,
    http ? el('label', 'field', el('span', null, t('provider_api')), api, note(t('provider_api_desc'))) : null,
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
  // Owner decision 41 (#164): codex alone can draw on the account's credits.
  let credits = null;
  if (draft.kind === 'cli' && draft.cli === 'codex') {
    const box = checkbox(draft.credits, (value) => { draft.credits = value; });
    box.dataset.field = 'providers.credits';
    box.disabled = Boolean(unsupported);
    credits = el('div', null, el('label', 'check', box, t('provider_credits')), note(t('provider_credits_desc')));
  }
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
    credits, limits, el('div', 'actions', save, cancel));
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
  on.dataset.field=`chain.${r.name}.on`;
  const model = input('text', r.edit.model, r.effective_model ?? '', `chain.${r.name}.model`, (v) => { r.edit.model = v; });
  model.setAttribute('aria-label', `${t('col_model')}: ${r.name}`);
  // A curator with prices keeps its model; a model config.toml sets already can still be emptied.
  model.disabled = r.model_rule === 'fixed' && !r.model;
  const budget = input('number', r.edit.daily_budget, r.effective_daily_budget === null ? t('no_cap') : String(r.effective_daily_budget), `chain.${r.name}.daily_budget`, (v) => { r.edit.daily_budget = v; });
  [budget.min, budget.max] = form.ranges.daily_budget;
  budget.setAttribute('aria-label', `${t('col_budget')}: ${r.name}`);
  const fromKey = !r.subscription && r.budget_from_key && !r.edit.daily_budget ? note(t('from_key', { n: r.effective_daily_budget })) : null;
  const budgetOff = r.subscription ? [] : noCapSwitch(r.edit.noCap, `chain.${r.name}`, `${t('col_budget')}: ${r.name}`, budget, 'no_cap_row',
    (v) => { r.edit.noCap = v; if (fromKey) fromKey.hidden = v; });
  if (fromKey) fromKey.hidden = r.edit.noCap;
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
    el('td', null, r.subscription ? note(t('provider_subscription')) : budget, ...budgetOff, fromKey),
    el('td', null, timeout));
  return tr;
}

// The save's body, or the field a value is wrong in.
function saveBody() {
  const redaction = redactionBody();
  if (!redaction.value) return { field: redaction.field };
  const whole = (v, min, max) => (/^\d+$/.test(v.trim()) && Number(v) >= min && Number(v) <= max ? Number(v) : Number.NaN);
  const port = whole(form.view.port, ...form.ranges.view_port);
  if (Number.isNaN(port)) return { field: 'view.port' };
  // docs/retention.md: a day count in range, or null for forever; sent only when it changed.
  const period = form.retention;
  const days = period.forever ? null : whole(period.edit, ...form.ranges.retention_days);
  if (Number.isNaN(days)) return { field: 'retention.days' };
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
  const cap = form.paidNoCap ? null : Number(paid);
  const [least, most] = form.ranges.paid_usd_per_month;
  if (!form.paidNoCap && (paid === '' || !Number.isFinite(cap) || cap < least || (most !== null && cap > most))) return { field: 'paid_usd_per_month' };
  if (!['none', 'before-subscriptions', 'after-subscriptions'].includes(form.gemini)) return { field: 'gemini' };
  // A value the file holds is sent as it is when it is not changed, in range or not, as a
  // provider's daily budget is (#680).
  const calls = form.my_work.calls_a_day;
  const unchangedCalls = /^\d+$/.test(String(calls)) && Number(calls) === form.savedSettings.my_work.calls_a_day;
  let bound = unchangedCalls ? Number(calls) : whole(String(calls), ...form.ranges.my_work_calls);
  if (form.my_work.noCap) bound = form.ranges.no_cap;
  const myWork = { curate: form.my_work.curate, calls_a_day: bound };
  if (Number.isNaN(myWork.calls_a_day)) return { field: 'my_work.calls_a_day' };
  let judge = null;
  if (form.judge?.set) {
    const j = form.judge, had = form.savedSettings.judge?.daily_budget;
    let daily = j.budget.trim() === String(had) ? had : whole(j.budget, ...form.ranges.daily_budget);
    if (j.noCap) daily = form.ranges.no_cap;
    if (Number.isNaN(daily)) return { field: 'judge.daily_budget' };
    // Sent only when it changed, as the retention period is.
    judge = { enabled: j.enabled, ...(daily === had ? {} : { daily_budget: daily }) };
  }
  let embedding = null;
  if (form.embedding) {
    const e = form.embedding.edit, w = form.embedding.workers;
    // A value config.toml has already, left as it is, is sent as it is, in range or not; an
    // account is sent only when typed (empty keeps the saved one: the page cannot remove it).
    const kept = (typed, had) => typed.trim() === String(had);
    const account = e.account_id.trim().toLowerCase();
    const typed = account !== '' && !kept(e.account_id, w.account_id ?? '');
    if (typed && !/^[0-9a-f]{32}$/.test(account)) return { field: 'embedding.account_id' };
    const daily = kept(e.daily_requests, w.daily_requests) ? w.daily_requests : whole(e.daily_requests, ...form.ranges.daily_requests);
    if (Number.isNaN(daily)) return { field: 'embedding.daily_requests' };
    const monthly = kept(e.monthly_usd, w.monthly_usd) ? w.monthly_usd : Number(e.monthly_usd.trim());
    if (monthly !== w.monthly_usd && (e.monthly_usd.trim() === '' || !Number.isFinite(monthly) || monthly <= 0)) return { field: 'embedding.monthly_usd' };
    embedding = { ...(typed ? { account_id: account } : {}), daily_requests: daily, monthly_usd: monthly };
  }
  const chain = [];
  for (const r of form.chain) {
    // Empty follows the curator's own value; a value config.toml has already stays as it is.
    const optional = (v, had, min, max) => {
      if (v.trim() === '') return null;
      return v.trim() === String(had) ? had : whole(v, min, max);
    };
    let daily = r.edit.noCap ? form.ranges.no_cap : optional(r.edit.daily_budget, r.daily_budget, ...form.ranges.daily_budget);
    if (chainDailyUntouched(r)) daily = r.daily_budget ?? null;
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
      ...(port === form.saved.view.port ? {} : { view: { port } }),
      summary,
      my_work: myWork,
      ...(judge ? { judge } : {}),
      paid_usd_per_month: cap,
      gemini: form.gemini === 'none' ? null : form.gemini,
      inject: {
        session_start: form.inject.session_start,
        session_start_note: form.inject.session_start_note,
        session_start_list: form.inject.session_start_list,
        per_prompt: form.inject.per_prompt,
        memory_lookup: form.inject.memory_lookup,
        memory_instructions: form.inject.memory_instructions,
        correction: form.inject.correction,
        ...sizes,
      },
      capture: { store_prompts: form.capture.store_prompts, tool_output: form.capture.tool_output },
      ...(!form.backup.reset && form.backup.edit === (form.backup.dir ?? '') ? {} : { backup: { dir: form.backup.reset ? null : form.backup.edit || null } }),
      ...(days === period.days ? {} : { retention: { days } }),
      redaction: redaction.value,
      ...(embedding ? { embedding } : {}),
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

function applySavedSettings(answer, mine, current, simple = null) {
  // The resident viewer moved onto the saved port with a new token: the page goes on to the
  // address in the answer and reads nothing more from the old one, which no longer answers.
  const runtime = current ? null : answer.view_runtime;
  // An address on port 80 shows no port.
  const moved = runtime?.mode === 'resident' && runtime.port !== (Number(location.port) || 80);
  if (moved && movePage(runtime.url || '')) return;
  form = current ? formOf(current) : mergeProviderSettings(answer, mine, { op: simple ? 'simple' : 'settings' });
  if (simple && !current && form) {
    simple.adopt(form, answer);
    simpleReceipt = simple.block;
  }
  if (form) form.privacy = privacyUnavailable();
  drawSettings();
  let status = current ? 'stale' : simple ? 'simple_saved' : 'saved';
  if (moved) status = 'view_lost';
  setStatus(t(status), Boolean(current) || moved, lang);
  if (form) void refreshPrivacy(form);
}

async function saveSettings(button, simple = null) {
  const { body, field } = simple || saveBody();
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
    // Only a stale version reloads; another refusal (a port in use) keeps what was typed.
    const current = res.status === 409 && answer.code === 'stale' ? await api('settings') : null;
    // Moved to another tab, or the values were loaded again, while saving: what is shown stays.
    if (view !== 'settings' || form !== mine) return;
    if (res.ok || current) {
      applySavedSettings(answer, mine, current, simple);
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
    if (simple && currentSettings(mine)) {
      clearSimpleReceipt();
      // Keep a refused amount available to correct; switches, models and arrows show the last
      // saved state again, especially when masking could not be enabled.
      if (button.type !== 'number') { mine.simpleMaskingOff = false; mine.simpleCapOff = false; drawSettings(); }
    }
  }
}

const ONBOARDING_TIERS = ['none','free','subscription','paid'];
let onboardingForm = null, onboardingTier = '', onboardingApplied = false;

function onboardingLocal(row) {
  if(row.saved?.kind!=='openai'||typeof row.saved.base_url!=='string')return false;
  try {
    const url=new URL(row.saved.base_url);
    const parts=url.hostname.split('.');
    return ['http:','https:'].includes(url.protocol)&&(url.hostname==='[::1]'
      ||parts.length===4&&parts[0]==='127'
        &&parts.every(part=>/^\d+$/.test(part)&&Number(part)<=255));
  } catch {return false;}
}
function onboardingClass(row) {
  const saved=row?.saved,limits=saved?.limits;
  if(saved?.enabled!==true)return 'disabled';
  if(saved.kind==='openai' && (saved.endpoint_supported!==true || typeof saved.base_url!=='string'))return 'unknown';
  if(saved.kind==='cli' && !['claude','codex'].includes(saved.cli))return 'unknown';
  if(!['openai','cli'].includes(saved.kind))return 'unknown';
  if(!limits||!['usd_per_mtok_in','usd_per_mtok_out'].every(key=>
    typeof limits[key]==='number'&&Number.isFinite(limits[key])&&limits[key]>=0))return 'unknown';
  if(limits.usd_per_mtok_in>0||limits.usd_per_mtok_out>0)return 'paid';
  if(saved.kind==='cli')return ['claude','codex'].includes(saved.cli)?'subscription':'unknown';
  if(saved.subscription===true)return 'subscription';
  if(saved.kind==='openai'&&(row.selector?.source==='builtin'||onboardingLocal(row)))return 'free';
  return 'unknown';
}
function onboardingGroups(f) {
  return new Map(f.chain.map(entry=>{
    const rows=f.providers.filter(row=>row.name===entry.name&&row.saved?.enabled===true);
    const classes=rows.map(onboardingClass);
    const kind=classes.length
      ? ['unknown','paid','subscription'].find(value=>classes.includes(value))||'free'
      : 'disabled';
    return [entry.name,kind];
  }));
}
function onboardingPlan(f,tier) {
  if(!ONBOARDING_TIERS.includes(tier))return null;
  if(tier==='none')return {available:true,curate:false,cap:'0',names:null};
  const rank={free:1,subscription:2,paid:3};
  const groups=onboardingGroups(f);
  const names=f.chain.filter(entry=>rank[groups.get(entry.name)]
    &&rank[groups.get(entry.name)]<=rank[tier]).map(entry=>entry.name);
  const hasLevel=[...groups.values()].includes(tier);
  if(!hasLevel||!names.length)return {available:false,curate:false,cap:f.paid_usd_per_month,names:null};
  const current=Number(f.paid_usd_per_month);
  let cap=tier==='paid'?'5':'0';
  if(tier==='paid'&&Number.isFinite(current)&&current>0)cap=f.paid_usd_per_month;
  // Decision 64: a cap the owner turned off stays off for paid calls; null is no cap.
  if(tier==='paid'&&f.paidNoCap)cap=null;
  return {available:true,curate:true,names,
    cap};
}
function applyOnboardingValues(f,plan) {
  if(!plan)return;
  f.summary.curate=plan.curate;
  if(plan.available){f.paidNoCap=plan.cap===null;if(plan.cap!==null)f.paid_usd_per_month=plan.cap;}
  if(plan.names){const enabled=new Set(plan.names);for(const row of f.chain)row.edit.on=enabled.has(row.name);}
}
function syncOnboardingControls(f) {
  const section=$('panel').querySelector('.settings');
  if(!section)return;
  const fields=[...section.querySelectorAll('[data-field]')];
  const field=name=>fields.find(node=>node.dataset.field===name);
  const curate=field('summary.curate');if(curate)curate.checked=f.summary.curate;
  const cap=field('paid_usd_per_month');if(cap)cap.value=f.paid_usd_per_month;
  const off=field('paid_usd_per_month.no_cap');
  if(off&&off.checked!==f.paidNoCap){off.checked=f.paidNoCap;off.dispatchEvent(new Event('change'));}
  for(const row of f.chain){
    const control=field(`chain.${row.name}.on`);
    if(control){control.checked=row.edit.on;control.closest('tr')?.classList.toggle('off',!row.edit.on);}
  }
}
function renderOnboarding(f) {
  if(view!=='settings'||form!==f)return;
  const section=$('panel').querySelector('.onboarding');
  if(!section)return;
  const focused=section.contains(document.activeElement)?document.activeElement?.dataset:null;
  section.replaceChildren(...onboardingSection(f).childNodes);
  if(focused?.field==='onboarding.tier')section.querySelector('[data-field="onboarding.tier"]')?.focus({preventScroll:true});
  else if(focused?.action==='onboarding.copy')section.querySelector('[data-action="onboarding.copy"]')?.focus({preventScroll:true});
}
function onboardingSection(f) {
  if(onboardingForm!==f){onboardingForm=f;onboardingTier='';onboardingApplied=false;}
  const select=el('select',null,el('option',null,t('onboarding_choose')),
    ...ONBOARDING_TIERS.map(value=>{const option=el('option',null,t(`onboarding_${value}`));option.value=value;return option;}));
  select.options[0].value='';select.value=onboardingTier;select.dataset.field='onboarding.tier';
  select.addEventListener('change',()=>{
    onboardingTier=select.value;onboardingApplied=false;renderOnboarding(f);
  });
  const plan=onboardingPlan(f,onboardingTier);
  const effect=plan?.cap===null?'onboarding_effect_no_cap':'onboarding_effect';
  const apply=el('button','quiet small',t('onboarding_copy'));
  apply.type='button';apply.dataset.action='onboarding.copy';apply.disabled=!plan;
  apply.addEventListener('click',()=>{
    if(form!==f)return;
    applyOnboardingValues(f,onboardingPlan(f,onboardingTier));
    syncOnboardingControls(f);onboardingApplied=true;renderOnboarding(f);
    // The chain's on/off it changed is to be reviewed before saving.
    const providers=$('panel').querySelector('.group-providers');if(providers)providers.open=true;
  });
  // A target in a folded settings group is opened before it is scrolled to.
  const jump=(selector,key)=>{
    const button=el('button','quiet small',t(key));button.type='button';
    button.addEventListener('click',()=>{
      const target=$('panel').querySelector(selector);
      for(let d=target?.closest('details');d;d=d.parentElement?.closest('details'))d.open=true;
      target?.scrollIntoView?.({block:'start'});
    });
    return button;
  };
  const link=(label,href)=>{const a=el('a',null,label);a.href=href;a.target='_blank';a.rel='noopener noreferrer';return a;};
  const phrase=input('text',f.testPhrase||'','', 'onboarding.phrase',value=>{f.testPhrase=value;search.disabled=!value.trim();});
  const search=el('button','quiet small',t('onboarding_search_open'));
  search.type='button';search.dataset.action='onboarding.search';search.disabled=!phrase.value.trim();
  search.addEventListener('click',async()=>{
    $('q').value=phrase.value.trim();$('raw').value='only';
    $('since').value='';$('until').value='';$('history').checked=false;
    setView('records');await refresh(false,'');
  });
  const destinations=plan?.names?.flatMap(name=>f.providers.filter(row=>row.name===name && row.saved?.enabled)
    .map(row=>el('li',null,`${name}: ${row.saved.kind==='cli'?row.saved.cli:row.saved.base_url||t('agent_inventory_unknown')}`)));
  return el('section','onboarding',
    el('h3',null,t(f.firstRun?'onboarding_first_h':'onboarding_again_h')),
    el('p','desc',t('onboarding_desc')),
    el('p','desc',t('onboarding_scope')),
    el('label','field',el('span',null,t('onboarding_tier')),select),
    plan && el('p','desc',plan.available
      ? t(effect,{names:plan.names?.join(', ')||t('onboarding_no_names'),cap:plan.cap})
      : t('onboarding_unavailable')),
    destinations?.length?el('ul',null,...destinations):null,
    apply,onboardingApplied?el('p','desc',t('onboarding_draft_ready')):null,
    el('p','desc',t('onboarding_no_assurance')),
    el('details',null,el('summary',null,t('onboarding_policy_h')),
      el('p','desc',t('onboarding_policy')),
      el('p','desc',t('onboarding_claude_policy')),
      el('blockquote',null,'Nor does it prevent an end user from signing in to the unmodified Claude Code binary with their own Claude subscription'),
      link('Anthropic policy','https://code.claude.com/docs/en/legal-and-compliance'), ' · ',
      link('Claude login','https://support.claude.com/en/articles/13189465-log-in-to-your-claude-account'), ' · ',
      link('Codex login','https://learn.chatgpt.com/docs/auth'),
      el('p','desc',t('onboarding_go_policy')),
      el('blockquote',null,'OpenCode Go is designed for OpenCode and other coding agents that produce similar types of requests. Send typical coding agent traffic.'),
      link('OpenCode Go','https://opencode.ai/docs/go/'), ' · ',
      link('Terms','https://opencode.ai/legal/terms-of-service'), ' · ',
      link('Privacy','https://opencode.ai/legal/privacy-policy')),
    el('div','actions',jump('.provider-entries','onboarding_providers'),
      jump('.settings form button.save','onboarding_save'),
      jump('.agent-setup','onboarding_agents'),
      jump('.doctor-diagnostics','onboarding_doctor')),
    el('p','desc',t('onboarding_never',{section:t('never_h')})),jump('.never-record','never_h'),
    el('p','desc',t(f.saved?.capture?.store_prompts===false?'onboarding_prompt_off':'onboarding_search_step',
      {section:t('capture_h'),setting:t('store_prompts')})),
    f.saved?.capture?.store_prompts===false?jump('.capture-settings','capture_h'):null,
    el('label','field',el('span',null,t('onboarding_search_phrase')),phrase),search);
}

// Catalog help, not validation: other models remain available in the detailed editor.
// Sources: config.rs's built-ins and docs/research/{curator-providers-*,anthropic-messages-*}.
// Key filenames describe the preset; registration still uses the existing managed key writer.
const AI_CATALOG = [
  { name: 'groq', label: 'simple_catalog_groq', kind: 'openai', base_url: 'https://api.groq.com/openai/v1', api: 'openai',
    key_file: 'GROQ_API_KEY.md', subscription: false, timeout_s: 90, daily_budget: 200, models: ['openai/gpt-oss-120b', 'openai/gpt-oss-20b'],
    limits: { max_request_tokens: 8000, daily_tokens: 40000 } },
  { name: 'groq-20b', label: 'simple_catalog_groq_20b', kind: 'openai', base_url: 'https://api.groq.com/openai/v1', api: 'openai',
    key_file: 'GROQ_API_KEY.md', subscription: false, timeout_s: 90, daily_budget: 200, models: ['openai/gpt-oss-20b', 'openai/gpt-oss-120b'],
    limits: { max_request_tokens: 8000, daily_tokens: 40000 } },
  { name: 'groq-qwen', label: 'simple_catalog_groq_qwen', kind: 'openai', base_url: 'https://api.groq.com/openai/v1', api: 'openai',
    key_file: 'GROQ_API_KEY.md', subscription: false, timeout_s: 90, daily_budget: 200, models: ['qwen/qwen3.8-27b'],
    limits: { max_request_tokens: 8000, daily_tokens: 40000 } },
  { name: 'openrouter', label: 'simple_catalog_openrouter', kind: 'openai', base_url: 'https://openrouter.ai/api/v1', api: 'openai',
    key_file: 'OPENROUTER_API_KEY.md', subscription: false, timeout_s: 90,
    models: ['nvidia/nemotron-3-super-120b-a12b:free', 'qwen/qwen3.8-27b:free'] },
  { name: 'nim', label: 'simple_catalog_nim', kind: 'openai', base_url: 'https://integrate.api.nvidia.com/v1', api: 'openai',
    key_file: 'NVIDIA_NIM_KEY.md', subscription: false, timeout_s: 160, daily_budget: 500, models: ['nvidia/nemotron-3-ultra-550b-a55b'] },
  { name: 'opencode-go', label: 'simple_catalog_opencode_go', kind: 'openai', base_url: 'https://opencode.ai/zen/go/v1', api: 'openai',
    key_file: 'OPENCODE_API_KEY.md', subscription: true, timeout_s: 150, models: ['glm-5.3-flash'] },
  { name: 'anthropic', label: 'simple_catalog_anthropic', kind: 'openai', base_url: 'https://api.anthropic.com/v1', api: 'anthropic',
    key_file: 'ANTHROPIC_API_KEY.md', subscription: false, timeout_s: 90, models: ['claude-haiku-5-5', 'claude-haiku-4-5-20251001'],
    limits: { max_request_tokens: 100000, usd_per_mtok_in: 0.10, usd_per_mtok_out: 0.50 } },
  { name: 'codex', label: 'simple_catalog_codex', kind: 'cli', cli: 'codex', base_url: '', api: null,
    key_file: null, subscription: true, timeout_s: 300, models: ['gpt-6-luna'] },
  { name: 'claude', label: 'simple_catalog_claude', kind: 'cli', cli: 'claude', base_url: '', api: null,
    key_file: null, subscription: true, timeout_s: 300, models: ['haiku', 'sonnet'] },
  { name: 'gemini', label: 'simple_catalog_gemini', kind: 'openai', base_url: 'https://generativelanguage.googleapis.com/v1beta/openai', api: 'openai',
    key_file: 'GEMINI_API_KEY.md', subscription: false, timeout_s: 90, daily_budget: 30, models: ['gemini-3.1-flash-lite'],
    limits: { usd_per_mtok_in: 0.25, usd_per_mtok_out: 1.50 } },
  { name: '', label: 'simple_catalog_custom', kind: 'openai', base_url: '', api: 'openai',
    key_file: null, subscription: false, models: [] },
];

function simpleProviderStatus(row, stopped) {
  if (row.key === 'missing') return 'missing';
  if (row.key === 'not-on-path') return 'not-installed';
  if (Array.isArray(stopped) && stopped.includes(row.name)) return 'stopped';
  if (row.on === false) return 'off';
  if (row.on !== true || !Array.isArray(stopped) || !['ok', 'none', 'on-path'].includes(row.key)) return 'unknown';
  return 'ready';
}

function simpleProviderModels(row, models) {
  if (!['any', 'free'].includes(row.model_rule)) return [];
  return [...new Set([row.effective_model, ...models].filter(model => typeof model === 'string' && model
    && (row.model_rule !== 'free' || model.endsWith(':free'))))];
}

function catalogHasProvider(entry, chain) {
  return Boolean(entry.name) && chain.some(row => row.name === entry.name);
}

function simpleSavedChain(f) {
  return f.savedSettings.chain.map(row => {
    const entry = AI_CATALOG.find(entry => entry.name === row.name);
    const providers = f.providers.filter(provider => provider.name === row.name);
    // A newly added HTTP entry has no key path until managed registration. For a keyed preset
    // that still needs a key, including after reloading; a custom keyless server stays keyless.
    const key = row.key === 'none' && entry?.key_file && providers[0]?.saved.base_url === entry.base_url ? 'missing' : row.key;
    return { ...row, key, on: row.on && providers.some(provider => provider.saved.enabled) };
  });
}

let settingsMode = recall('oboete-settings-view', 'simple') === 'detailed' ? 'detailed' : 'simple';
// The monthly limit as typed, kept across redraws until it is saved (Codex on #512).
let simpleReceipt = null, simpleKeyName = null, simpleCapDraft = null;

function clearSimpleReceipt() {
  simpleReceipt = null;
  for (const receipt of $('panel').querySelectorAll('.simple-saved')) receipt.replaceChildren();
}

function simpleFocus(key) {
  if (typeof key !== 'string' || !key) return;
  const controls = $('panel').querySelectorAll('button, input, select');
  const keys = [key];
  if (key?.startsWith('move:')) keys.push(key.endsWith(':-1') ? key.replace(/:-1$/, ':1') : key.replace(/:1$/, ':-1'));
  if (/^(resume|key|key-save):/.test(key || '')) keys.push(key.replace(/^[^:]+:/, 'on:'));
  if (key?.startsWith('test-run:')) keys.push(key.replace('test-run:', 'test:'));
  if (key === 'mask.confirm') keys.push('redaction.enabled');
  if (key === 'cap.confirm') keys.push('paid_usd_per_month.no_cap');
  if (key === 'embedding.start' || key === 'embedding.cancel') keys.push(`embedding:${form?.embedding?.provider}`);
  if (key === 'catalog.save' && simpleKeyName) keys.push(`key:${simpleKeyName}`);
  const next = keys.map(key => [...controls].find(control => control.dataset.simpleFocus === key && !control.disabled)).find(Boolean);
  next?.focus({ preventScroll: true });
}

function settingsViewSwitch() {
  const buttons = ['simple', 'detailed'].map(mode => {
    const button = el('button', 'quiet', t(mode === 'simple' ? 'simple_view' : 'simple_detailed'));
    button.type = 'button';
    button.setAttribute('aria-pressed', String(settingsMode === mode));
    button.dataset.simpleFocus = `mode:${mode}`;
    button.addEventListener('click', () => {
      settingsMode = mode;
      remember('oboete-settings-view', mode);
      drawSettings();
      simpleFocus(`mode:${mode}`);
    });
    return button;
  });
  const group = el('div', 'settings-view-switch', ...buttons);
  group.setAttribute('role', 'group');
  group.setAttribute('aria-label', t('simple_mode_label'));
  return group;
}

function detailedSettingsLink(key, selector, vars = {}, provider = null) {
  const button = el('button', 'quiet small simple-link', t(key, vars));
  button.type = 'button';
  button.addEventListener('click', () => {
    if (provider) { provider.editing = true; groupOpen.set('providers', true); }
    settingsMode = 'detailed';
    remember('oboete-settings-view', settingsMode);
    drawSettings();
    const target = provider ? [...$('panel').querySelectorAll('.provider-entry')]
      .find(node => node.dataset.selector === selectorKey(provider.selector)) : $('panel').querySelector(selector);
    for (let details = target?.closest('details'); details; details = details.parentElement?.closest('details')) details.open = true;
    if (target) { target.tabIndex = -1; target.focus({ preventScroll: true }); target.scrollIntoView?.({ block: 'start' }); }
  });
  return button;
}

function simpleBlock(name, ...children) {
  const receipt = el('span', 'simple-saved', simpleReceipt === name ? t('simple_saved') : '');
  receipt.setAttribute('role', 'status');
  const section = el('section', 'simple-block', el('div', 'simple-block-heading',
    el('h3', null, t(`simple_${name}_h`)), receipt), el('p', 'desc', t(`simple_${name}_desc`)), ...children);
  section.dataset.simpleBlock = name;
  return section;
}

// Required whole-settings fields come from the saved response. A simple change must not also
// commit a detailed draft, or a fresh home's suggested resident choice.
function simpleSettingsSave(button, change, adopt) {
  clearSimpleReceipt();
  const s = form.savedSettings;
  const body = JSON.parse(JSON.stringify({ version: form.version, summary: s.summary,
    paid_usd_per_month: s.paid_usd_per_month, gemini: s.gemini, inject: s.inject, capture: s.capture,
    chain: s.chain.map(row => ({ name: row.name, on: row.on, daily_budget: row.daily_budget,
      timeout_s: row.timeout_s, model: row.model })) }));
  change(body);
  return saveSettings(button, { body, adopt, block: button.closest('.simple-block').dataset.simpleBlock });
}

function simpleToggle(value, key, label, save, visible = label) {
  const control = checkbox(value, next => save(control, next));
  control.dataset.simpleFocus = key;
  control.setAttribute('role', 'switch');
  control.setAttribute('aria-label', label);
  return el('label', 'check simple-toggle', control, visible);
}

function simpleChainSave(button, name, key, value) {
  return simpleSettingsSave(button, body => { body.chain.find(row => row.name === name)[key] = value; },
    (next, answer) => { next.chain.find(row => row.name === name).edit[key] = answer.chain.find(row => row.name === name)[key] ?? ''; });
}

async function simpleProviderToggle(name, on, button) {
  // A native entry can be disabled independently of its name overlay. Reuse its own on/off
  // operation before lifting the name's off switch, so a built-in such as Anthropic can be used.
  if (on && !form.providers.some(provider => provider.name === name && provider.saved.enabled)) {
    const provider = form.providers.find(provider => provider.name === name);
    if (!provider || !await providerOperation({ op: 'enabled', selector: provider.selector, enabled: true }, button)) return;
    button = [...$('panel').querySelectorAll('input')].find(node => node.dataset.simpleFocus === `on:${name}`);
  }
  if (button && form.chain.find(row => row.name === name)?.on !== on) await simpleChainSave(button, name, 'on', on);
}

function simpleLabel(name) {
  const catalog = AI_CATALOG.find(entry => entry.name === name);
  return catalog ? t(catalog.label) : name;
}

function simpleProviderRow(f, row, index) {
  const label = simpleLabel(row.name);
  const catalog = AI_CATALOG.find(entry => entry.name === row.name);
  // Entries that read one key file (Groq's three) take the key on the first of them.
  // From the saved chain the rows are drawn in, not a detailed order still unsaved (Codex on #512).
  const keyOwner = row.key_file ? f.savedSettings.chain.find(other => other.key_file === row.key_file) : null;
  const providers = f.providers.filter(provider => provider.name === row.name);
  const provider = providers[0];
  const on = row.on;
  const status = simpleProviderStatus({ ...row, on }, f.stopped);
  const arrows = [-1, 1].map(direction => {
    const button = el('button', 'quiet small', direction < 0 ? '↑' : '↓');
    button.type = 'button';
    button.setAttribute('aria-label', `${t(direction < 0 ? 'up' : 'down')}: ${label}`);
    button.dataset.simpleFocus = `move:${row.name}:${direction}`;
    const to = index + direction;
    button.disabled = to < 0 || to >= f.chain.length;
    button.addEventListener('click', () => void simpleSettingsSave(button, body => {
      [body.chain[index], body.chain[to]] = [body.chain[to], body.chain[index]];
    }, (next, answer) => {
      const rows = new Map(next.chain.map(entry => [entry.name, entry]));
      next.chain = answer.chain.map(entry => rows.get(entry.name));
    }));
    return button;
  });
  const detail = () => detailedSettingsLink('simple_entry_detail', '.provider-entries', { name: label }, provider);
  const models = simpleProviderModels(row, catalog?.models || []);
  let model = el('p', 'simple-model', row.effective_model ?? t('simple_model_default'));
  if (row.model_rule !== 'fixed') {
    const select = el('select', null, ...models.map(model => {
      const option = el('option', null, model); option.value = model; return option;
    }));
    if (!models.includes(row.effective_model)) {
      const current = el('option', null, row.effective_model ?? t('simple_model_default'));
      current.value = ''; current.disabled = true; select.append(current);
    }
    const other = el('option', null, t('simple_other_model')); other.value = '__other'; select.append(other);
    select.value = models.includes(row.effective_model) ? row.effective_model : '';
    select.dataset.field = `chain.${row.name}.model`;
    select.dataset.simpleFocus = `model:${row.name}`;
    select.addEventListener('change', () => {
      if (select.value === '__other') detail().click();
      else void simpleChainSave(select, row.name, 'model', select.value);
    });
    select.setAttribute('aria-label', t('simple_model', { name: label }));
    model = el('label', 'field simple-model', el('span', null, t('simple_model_short')), select);
  }
  let resume = null;
  if (f.stopped?.includes(row.name)) {
    resume = el('button', 'quiet small', t('resume'));
    resume.type = 'button'; resume.setAttribute('aria-label', t('resume_label', { name: label }));
    resume.dataset.simpleFocus = `resume:${row.name}`;
    resume.addEventListener('click', () => void resumeProvider(row.name, resume));
  }
  const item = el('li', 'simple-provider',
    el('div', 'simple-provider-heading', el('h4', null, label),
      el('span', `simple-state ${status}`, t(`simple_status_${status.replaceAll('-', '_')}`))),
    el('div', 'simple-provider-controls',
      simpleToggle(on, `on:${row.name}`, t('simple_ai_on', { name: label }),
        (button, value) => void simpleProviderToggle(row.name, value, button), t('simple_use')),
      el('div', 'move', ...arrows)),
    model, row.differs?.length ? el('p', 'desc', t('simple_differs')) : null,
    keyOwner && keyOwner.name !== row.name && row.key === 'missing'
      ? el('p', 'desc', t('simple_key_shared', { name: simpleLabel(keyOwner.name) }))
      : provider && (row.key === 'missing' || simpleKeyName === row.name)
        ? el('div', 'provider-key', f.keyInput ? providerKeyField(provider) : note(t('provider_key_unsupported'))) : null,
    resume, provider ? providerTest(provider, true) : null, detail());
  item.dataset.providerName = row.name;
  return item;
}

function simpleCatalog(f) {
  const rows = AI_CATALOG.map(entry => {
    const listed = catalogHasProvider(entry, f.chain);
    const choose = el('button', 'quiet small', t(listed ? 'simple_catalog_open' : 'simple_catalog_choose', { name: t(entry.label) }));
    choose.type = 'button';
    choose.addEventListener('click', () => {
      if (listed) {
        const row = [...$('panel').querySelectorAll('.simple-provider')].find(row => row.dataset.providerName === entry.name);
        row?.scrollIntoView?.({ block: 'center' }); simpleFocus(`on:${entry.name}`); return;
      }
      f.newProvider = providerEdit(entry.name, { ...entry, model: entry.models[0] });
      f.simpleCatalog = entry;
      if (!entry.name) { detailedSettingsLink('simple_detailed', '.new-provider').click(); return; }
      drawSettings(); simpleFocus('catalog.save');
    });
    return el('li', null, el('strong', null, t(entry.label)), listed ? note(t('simple_catalog_listed')) : null, choose);
  });
  const catalog = el('details', 'simple-catalog', el('summary', null, t('simple_add_ai')), el('ul', null, ...rows));
  if (!f.newProvider || !f.simpleCatalog?.name) return catalog;
  const draft = f.newProvider, entry = f.simpleCatalog;
  const save = el('button', 'small', t('simple_add_save'));
  save.type = 'button'; save.dataset.simpleFocus = 'catalog.save';
  save.addEventListener('click', () => {
    if (providerValidation(draft)) { detailedSettingsLink('simple_detailed', '.new-provider').click(); return; }
    void providerOperation({ op: 'create', entry: providerBody(draft) }, save);
  });
  const cancel = el('button', 'quiet small', t('simple_cancel'));
  cancel.type = 'button'; cancel.addEventListener('click', () => { f.newProvider = null; f.simpleCatalog = null; drawSettings(); });
  return el('div', null, catalog, el('div', 'simple-add', el('h4', null, t(entry.label)),
    el('p', 'desc', draft.base_url || t('simple_model_default')), el('p', null, draft.model),
    entry.name === 'gemini' ? el('p', 'desc', t('simple_gemini_warning')) : null,
    draft.kind === 'openai' ? el('p', 'desc', t('simple_add_key')) : null, el('div', 'actions', save, cancel)));
}

function simpleEmbeddingChoice(f, { locked, choice, reviewed, preview, start, consent }) {
  preview.dataset.simpleFocus = 'embedding.preview';
  start.dataset.simpleFocus = 'embedding.start';
  consent.dataset.simpleFocus = 'embedding.confirmed';
  const choices = [['none', 'none'], ['local', 'local'], ['workers-ai', 'workers']].map(([value, key]) => {
    const radio = el('input'); radio.type = 'radio'; radio.name = 'simple-embedding'; radio.value = value;
    radio.checked = choice === value; radio.disabled = locked; radio.dataset.field = 'embedding.choice';
    radio.dataset.simpleFocus = `embedding:${value}`;
    radio.addEventListener('change', () => {
      embeddingDraft.choice = value; embeddingDraft.preview = null; embeddingDraft.confirmed = false;
      void embeddingAction('preview');
    });
    return el('label', 'simple-search-option', radio,
      el('span', null, el('strong', null, t(`simple_search_${key}`)), el('span', 'note', t(`simple_search_${key}_desc`))));
  });
  const cancel = el('button', 'quiet', t('simple_cancel'));
  cancel.type = 'button'; cancel.disabled = locked;
  cancel.dataset.simpleFocus = 'embedding.cancel';
  cancel.addEventListener('click', () => {
    embeddingDraft.choice = null; embeddingDraft.preview = null; embeddingDraft.confirmed = false;
    embeddingDraft.error = null; renderEmbedding(); simpleFocus(`embedding:${f.embedding.provider}`);
  });
  preview.textContent = t('simple_search_review');
  const saved = { none: 'none', local: 'local', 'workers-ai': 'workers' }[f.embedding.provider];
  return el('div', 'embedding-choice', ...choices,
    choice !== f.embedding.provider ? note(t('saved_value', { value: t(`simple_search_${saved}`) })) : null,
    reviewed ? embeddingConsent(embeddingDraft.preview.consent) : null,
    reviewed ? el('label', 'check', consent, t('embedding_confirm')) : null,
    reviewed ? el('div', 'actions', start, cancel) : null,
    !reviewed && choice !== f.embedding.provider ? el('div', 'actions', preview, cancel) : null,
    choice === 'workers-ai' ? detailedSettingsLink('simple_detailed', '.workers-ai') : null,
    embeddingRun(embeddingDraft), embeddingDraft.error ? el('p', 'error', embeddingMessage(embeddingDraft.error)) : null);
}

function simpleAgents() {
  const rows = ['claude', 'codex', 'grok'].map(id => {
    const row = agentInventory.report?.agents?.find(row => row.agent === id);
    const components = [row?.capture, row?.mcp];
    const connected = components.every(part => part?.state === 'registered' && part.matches_current === true)
      && (id !== 'codex' || ['matching', 'not_applicable'].includes(row?.trust));
    const disconnected = components.every(part => ['missing', 'disabled'].includes(part?.state));
    const key = agentInventory.loading ? 'agent_inventory_loading' : !row || agentInventory.error ? 'simple_status_unknown'
      : connected ? 'simple_agent_connected' : disconnected ? 'simple_agent_disconnected' : 'simple_agent_partial';
    const name = t(`agent_inventory_agent_${id}`);
    return el('li', null, el('strong', null, name), el('span', 'desc', t(key)),
      detailedSettingsLink('simple_agent_connect', '.agent-setup', { name }));
  });
  return el('ul', 'simple-agents', ...rows);
}

function drawSimpleSettings(panel, f) {
  const chain = simpleSavedChain(f);
  const curate = simpleToggle(f.savedSettings.summary.curate, 'summary.curate', t('simple_curate'), (button, value) =>
    void simpleSettingsSave(button, body => { body.summary.curate = value; },
      (next, answer) => { next.summary = { ...next.summary, curate: answer.summary.curate }; }));
  const myWork = simpleToggle(f.savedSettings.my_work.curate, 'my_work.curate', t('simple_my_work'), (button, value) =>
    void simpleSettingsSave(button, body => { body.my_work = { curate: value, calls_a_day: f.savedSettings.my_work.calls_a_day }; },
      (next, answer) => { next.my_work = { ...next.my_work, curate: answer.my_work.curate }; }));
  const ready = chain.some(row => simpleProviderStatus(row, f.stopped) === 'ready');
  // Decision 64: with no cap saved the limit is set aside until No cap is turned off, and turning
  // it on waits for its confirmation, as turning masking off does.
  const capOff = f.savedSettings.paid_usd_per_month === null;
  const cap = input('number', simpleCapDraft ?? noCapText(f.savedSettings.paid_usd_per_month), '', 'paid_usd_per_month',
    (v) => { simpleCapDraft = v; });
  cap.min = 0; cap.step = 'any'; cap.inputMode = 'decimal'; cap.dataset.simpleFocus = 'paid_usd_per_month';
  cap.disabled = (capOff && !f.simpleCapOn) || Boolean(f.simpleCapOff);
  const noCap = simpleToggle((capOff && !f.simpleCapOn) || Boolean(f.simpleCapOff), 'paid_usd_per_month.no_cap',
    `${t('no_cap')}: ${t('simple_limit')}`, (button, value) => {
      // Back to a no cap already saved, nothing is to confirm or save.
      if (value && capOff) { f.simpleCapOn = false; drawSettings(); simpleFocus('paid_usd_per_month.no_cap'); }
      else if (value) { f.simpleCapOff = true; drawSettings(); simpleFocus('cap.confirm'); }
      else { f.simpleCapOff = false; f.simpleCapOn = capOff; drawSettings(); simpleFocus(capOff ? 'paid_usd_per_month' : 'paid_usd_per_month.no_cap'); }
    }, t('no_cap'));
  const capConfirm = el('button', 'quiet', t('simple_no_cap_confirm'));
  capConfirm.type = 'button'; capConfirm.dataset.simpleFocus = 'cap.confirm';
  capConfirm.addEventListener('click', () => void simpleSettingsSave(capConfirm, body => { body.paid_usd_per_month = null; }, (next, answer) => {
    next.paid_usd_per_month = noCapText(answer.paid_usd_per_month);
    next.paidNoCap = answer.paid_usd_per_month === null;
    simpleCapDraft = null;
  }));
  const capCancel = el('button', 'quiet', t('simple_cancel'));
  capCancel.type = 'button'; capCancel.dataset.simpleFocus = 'cap.cancel';
  capCancel.addEventListener('click', () => { f.simpleCapOff = false; drawSettings(); simpleFocus('paid_usd_per_month.no_cap'); });
  // The warning: with its confirmation while the cap is being turned off, alone once it is off.
  let capWarning = capOff ? note(t('no_cap_paid_warning')) : null;
  if (f.simpleCapOff) capWarning = el('div', 'simple-warning', el('p', 'desc error', t('no_cap_paid_warning')), el('div', 'actions', capConfirm, capCancel));
  const saveCap = () => {
    const value = Number(cap.value);
    if (!cap.value.trim() || !Number.isFinite(value) || value < 0) {
      markInvalid('paid_usd_per_month'); setStatus(t('simple_limit_invalid'), true, lang); return;
    }
    if (cap.disabled) return;
    if (value === f.savedSettings.paid_usd_per_month) { simpleCapDraft = null; return; }
    void simpleSettingsSave(cap, body => { body.paid_usd_per_month = value; }, (next, answer) => {
      next.paid_usd_per_month = noCapText(answer.paid_usd_per_month);
      next.paidNoCap = answer.paid_usd_per_month === null;
      if (Number(simpleCapDraft) === answer.paid_usd_per_month) simpleCapDraft = null;
    });
  };
  cap.addEventListener('blur', saveCap);
  cap.addEventListener('keydown', event => { if (event.key === 'Enter') { event.preventDefault(); saveCap(); } });
  const inject = ['session_start', 'per_prompt'].map(key => simpleToggle(f.savedSettings.inject[key], `inject.${key}`,
    t(key === 'session_start' ? 'simple_session' : 'simple_prompt'), (button, value) =>
      void simpleSettingsSave(button, body => { body.inject[key] = value; },
        (next, answer) => { next.inject = { ...next.inject, [key]: answer.inject[key] }; })));
  const masking = simpleToggle(f.savedSettings.redaction?.enabled !== false && !f.simpleMaskingOff,
    'redaction.enabled', t('simple_mask_on'), (button, value) => {
      if (!value) { f.simpleMaskingOff = true; drawSettings(); simpleFocus('mask.confirm'); }
      else { f.simpleMaskingOff = false; saveMask(button, true); }
    });
  const saveMask = (button, enabled) => void simpleSettingsSave(button, body => {
    body.redaction = { ...f.savedSettings.redaction, enabled };
  }, (next, answer) => { next.redaction = { ...next.redaction, enabled: answer.redaction.enabled }; });
  const confirm = el('button', 'quiet', t('simple_mask_confirm'));
  confirm.type = 'button'; confirm.dataset.simpleFocus = 'mask.confirm';
  confirm.addEventListener('click', () => saveMask(confirm, false));
  const cancel = el('button', 'quiet', t('simple_cancel'));
  cancel.type = 'button'; cancel.addEventListener('click', () => { f.simpleMaskingOff = false; drawSettings(); simpleFocus('redaction.enabled'); });
  // `append` writes an absent part as the text "null": leave it out, as the detailed page does.
  panel.append(...present([el('p', 'lead', t('simple_intro')),
    !ready ? el('p', 'simple-empty', t('simple_no_ai')) : null,
    simpleBlock('ai', curate, el('ol', 'simple-providers', ...chain.map((row, i) => simpleProviderRow(f, row, i))), simpleCatalog(f)),
    simpleBlock('my_work', myWorkEstimate(f.my_work.estimate), myWork),
    simpleBlock('spending', el('div', 'spending-cap', el('label', 'field', el('span', null, t('simple_limit')), cap),
      el('p', 'spend', f.usd_this_month === null ? t('spend_unavailable') : t('simple_spent', { usd: usdValue(f.usd_this_month) }))),
    noCap, capWarning),
    f.embedding ? simpleBlock('search', embeddingChoice(f, true)) : null,
    simpleBlock('inject', ...inject), captureSection(f, true),
    simpleBlock('mask', masking, f.simpleMaskingOff ? el('div', 'simple-warning',
      el('p', 'desc error', t('redaction_off_warning')), el('div', 'actions', confirm, cancel)) : null),
    simpleBlock('agents', el('div', 'simple-agent-inventory', simpleAgents())),
    // Only while something is removed, to bring it back from here too.
    f.hidden?.items?.length ? hiddenSection(f, true) : null]));
}

// docs/my-work-model.md A6: what My Work's jobs would cost, before the switch is turned on.
function myWorkEstimate(estimate) {
  if (!estimate) return el('p', 'desc', t('my_work_estimate_none'));
  const money = (v) => new Intl.NumberFormat(lang, { style: 'currency', currency: 'USD', maximumSignificantDigits: 2 }).format(v);
  const [calls, most] = estimate.calls;
  // Decision 64: with no cap of calls a day there is no most.
  const open = most === null ? '_no_cap' : '';
  if (!estimate.usd) return el('p', 'desc', t(`my_work_estimate_free${open}`, { calls, most }));
  return el('p', 'desc', t(`my_work_estimate${open}`, { calls, most, usd: money(estimate.usd[0]),
    most_usd: estimate.usd[1] === null ? '' : money(estimate.usd[1]),
    name: estimate.priced?.name ?? '', model: estimate.priced?.model ?? '' }));
}

function drawSettings() {
  const focused = document.activeElement?.dataset.simpleFocus;
  if (document.activeElement && document.activeElement !== document.body
    && !document.activeElement.closest('.embedding-choice')) embeddingDraft.focus = null;
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
    panel.append(el('p', 'text pending', t('file_error')), recoverySection(), agentInventorySection(), agentSetupSection(), doctorSection());
    drawIn(panel);
    return;
  }
  const f = form;
  panel.append(settingsViewSwitch());
  if (settingsMode === 'simple') {
    drawSimpleSettings(panel, f);
    drawIn(panel);
    if (focused) simpleFocus(focused);
    return;
  }
  const saved = (value) => note(t('saved_value', { value }));
  const resident = checkbox(f.worker.resident, (v) => { f.worker.resident = v; });
  resident.dataset.field = 'worker.resident';
  resident.disabled = !f.residentSupported;
  const pagePort = input('number', f.view.port, '', 'view.port', (v) => { f.view.port = v; });
  [pagePort.min, pagePort.max] = f.ranges.view_port;
  const curate=checkbox(f.summary.curate,(v)=>{f.summary.curate=v;});
  curate.dataset.field='summary.curate';
  const myWorkCurate = checkbox(f.my_work.curate, (v) => { f.my_work.curate = v; });
  myWorkCurate.dataset.field = 'my_work.curate';
  const myWorkCalls = input('number', f.my_work.calls_a_day, '', 'my_work.calls_a_day', (v) => { f.my_work.calls_a_day = v; });
  [myWorkCalls.min, myWorkCalls.max] = f.ranges.my_work_calls;
  const myWorkOff = noCapSwitch(f.my_work.noCap, 'my_work', t('my_work_h'), myWorkCalls, 'no_cap_calls_warning', (v) => { f.my_work.noCap = v; });
  const savedCalls = f.saved.my_work.calls_a_day === f.ranges.no_cap ? t('no_cap') : f.saved.my_work.calls_a_day;
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
  const capOff = noCapSwitch(f.paidNoCap, 'paid_usd_per_month', t('paid_cap'), cap, 'no_cap_paid_warning', (v) => { f.paidNoCap = v; });
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
  // Decision 51: the memory list above the line, meaningful only while the line is shown.
  const terminalList = checkbox(f.inject.session_start_list, (v) => { f.inject.session_start_list = v; });
  terminalList.dataset.field = 'inject.session_start_list';
  terminalList.disabled = !f.inject.session_start_note;
  const terminalNote = checkbox(f.inject.session_start_note, (v) => { f.inject.session_start_note = v; terminalList.disabled = !v; });
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
  // The common settings first and open; the rest folded (docs/viewer-design.md). A new home opens
  // its guide and the resident choice it is to save, and a waiting provider its group.
  const formEl = el('form', null,
    settingsGroup('start', f.firstRun, onboardingSection(f)),
    settingsGroup('everyday', true,
      el('section', null,
        el('h3', null, t('summary_h')), el('p', 'desc', t('summary_desc')),
        el('label', 'check', curate, t('curate_on')),
        saved(t(f.saved.summary.curate ? 'value_on' : 'value_off')),
        el('label', 'field', el('span', null, t('summary_language')), summaryLanguage,
          saved(f.saved.summary.language), note(t('summary_language_desc'))),
        el('details', null, el('summary', null, t('summary_advanced')),
          el('div', 'grid', summarySize('window_tokens'), summarySize('idle_minutes')))),
      el('section', null,
        el('h3', null, t('my_work_h')), el('p', 'desc', t('my_work_desc')),
        myWorkEstimate(f.my_work.estimate),
        el('label', 'check', myWorkCurate, t('my_work_on')),
        saved(t(f.saved.my_work.curate ? 'value_on' : 'value_off')),
        el('label', 'field', el('span', null, t('my_work_calls', { min: f.ranges.my_work_calls[0].toLocaleString(lang), max: f.ranges.my_work_calls[1].toLocaleString(lang) })),
          myWorkCalls, saved(savedCalls), note(t('my_work_calls_desc'))),
        ...myWorkOff),
      judgeSection(f),
      el('section', null,
        el('h3', null, t('spending_h')),
        el('div', 'spending-cap',
          el('label', 'field', el('span', null, t('paid_cap')), cap,
            saved(f.saved.paid_usd_per_month === null ? t('no_cap') : usd(f.saved.paid_usd_per_month))),
          el('p', 'spend', spending)),
        ...capOff,
        el('p', 'desc', t('paid_cap_desc')),
        el('label', 'field', el('span', null, t('gemini_label')), gemini,
          saved(t(geminiOptions[f.saved.gemini ?? 'none'])), note(t('gemini_desc')))),
      el('section', null,
        el('h3', null, t('inject_h')),
        el('p', 'desc', t('inject_desc')),
        flag('session_start', 'inject_on'), size('session_start_chars', 'inject_chars'),
        el('label', 'check', terminalNote, t('session_note_on')), note(t('session_note_desc')),
        el('label', 'check', terminalList, t('session_list_on')), note(t('session_list_desc')),
        flag('per_prompt', 'per_prompt_on'), size('per_prompt_chars', 'per_prompt_chars'),
        size('per_prompt_wait_ms', 'per_prompt_wait_ms'), promptSide(f.prompt_side),
        flag('memory_lookup', 'memory_lookup_on'),
        flag('memory_instructions', 'memory_instructions_on'),
        flag('correction', 'correction_on'), size('correction_chars', 'correction_chars')),
      el('section', 'capture-settings',
        el('h3', null, t('capture_h')),
        el('p', 'desc', t('capture_desc')),
        el('label', 'check', checkbox(f.capture.store_prompts, (v) => { f.capture.store_prompts = v; }), t('store_prompts')),
        el('label', 'field', el('span', null, t('tool_output')), tool))),
    settingsGroup('providers', Boolean(f.stopped?.length),
      providersSection(),
      el('section', null,
        el('h3', null, t('chain_h')),
        el('p', 'desc', t('chain_desc')),
        el('div', 'scroll', el('table', 'chain', el('thead', null, head), rows))),
      el('section', null,
        el('h3', null, t('stopped_h')), el('p', 'desc', t('stopped_desc')), stoppedState())),
    settingsGroup('search', Boolean(embeddingDraft.status?.active), embeddingSection(f)),
    settingsGroup('privacy', false, redactionSection(f), privacySection(f), captureSection(f), retentionSection(f)),
    settingsGroup('computer', f.firstRun,
      el('section', null,
        el('h3', null, t('resident_h')), el('p', 'desc', t('resident_desc')),
        el('label', 'check', resident, t('resident_on')),
        saved(t(f.saved.worker.resident ? 'value_on' : 'value_off')),
        f.firstRun && f.residentSupported ? el('p', 'desc', t('resident_first')) : null,
        el('p', 'desc', t(f.residentSupported ? 'resident_timing' : 'resident_unsupported'))),
      el('section', null,
        el('h3', null, t('view_h')),
        el('label', 'field', el('span', null, t('view_port')), pagePort, saved(f.saved.view.port)),
        el('p', 'desc', t(f.viewRuntime?.mode === 'resident' ? 'view_port_resident' : 'view_port_foreground'))),
      el('section', null,
        el('h3', null, t('backup_h')),
        el('label', 'field', el('span', null, t('backup_dir')),
          input('text', f.backup.edit, t('backup_default'), 'backup.dir', (v) => { f.backup.edit = v; f.backup.reset = false; }),
          saved(f.backup.dir === '' ? t('backup_home') : f.backup.dir ?? t('backup_default'))),
        backupReset,
        el('p', 'desc', t('backup_desc')))),
    f.warnings.length
      ? el('section', 'warnings', el('h3', null, t('warnings_h')), el('ul', null, ...f.warnings.map((w) => el('li', null, w))))
      : null,
    el('div', 'save-bar', save));
  formEl.noValidate = true;
  formEl.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && event.target.closest('.provider-entry') && event.target.tagName !== 'BUTTON' && event.target.tagName !== 'SUMMARY') event.preventDefault();
  });
  formEl.addEventListener('submit', (e) => {
    e.preventDefault();
    void saveSettings(save);
  });
  const recovered=recoverySection();
  if(recovered)panel.append(recovered);
  panel.append(...present([el('p', 'lead', t('lead')), formEl,
    settingsGroup('agents', false, preferenceSection(f), agentInventorySection(), agentSetupSection()),
    settingsGroup('maintenance', false, viewSection(f), hiddenSection(f), doctorSection(), maintenanceSection(f))]));
  drawIn(panel);
}

// Whether each settings group is open, kept across redraws as a provider's editor keeps its own.
const groupOpen = new Map();

function settingsGroup(key, open, ...sections) {
  const group = el('details', `settings-group group-${key}`,
    el('summary', null, el('span', 'group-title', t(`group_${key}_h`)), el('span', 'group-desc', t(`group_${key}_desc`))),
    ...sections);
  group.open = groupOpen.get(key) ?? open;
  group.addEventListener('toggle', () => { if (group.isConnected) groupOpen.set(key, group.open); });
  return group;
}

// The settings view in its own language, its heading included (#274).
function drawIn(panel) {
  draw(t('heading'), [], [panel]);
  $('heading').lang = lang;
}

// --- My Work (parity row X10, docs/my-work.md): the requests and where they stand ----------
// claude-mem 15.0.1's work view (`plugin/ui/work-view`) on the page: the API gives the requests
// of all recorded work (`GET /api/my-work?range=all`), and the page scopes, sorts and draws them
// as claude-mem's page draws its snapshot. A request opens within the page, and its back button
// returns the focus to what opened it: the address holds the token (decision 55).

// claude-mem's status tabs (`app.js` `labels`): the first six always, the rest with a count.
const MY_WORK_TABS = ['all', 'needs', 'working', 'quiet', 'ready', 'checked', 'shipped', 'blocked', 'todo', 'unknown', 'closed', 'other'];
const MY_WORK_ALWAYS = ['all', 'needs', 'working', 'quiet', 'ready', 'checked'];
// `presentation.js` `attention`; a status it does not name ranks with `unknown`.
const MY_WORK_ATTENTION = { needs: 0, blocked: 1, ready: 2, working: 3, quiet: 4, todo: 5, unknown: 6, closed: 7, checked: 8, shipped: 9 };
const MY_WORK_LAYOUTS = ['cards', 'list', 'agents'];
const MY_WORK_NEEDS = ['needs-you', 'needs-input', 'waiting-on-user', 'awaiting-approval', 'needs-approval'];
const MY_WORK_REVIEW = new Set(['needs-review', 'review', 'in-review', 'awaiting-review', 'ready-for-review']);
const myWorkTitles = new Intl.Collator('en', { sensitivity: 'base', numeric: true });
// The answer and the page's place in it. The filters live in the page, as claude-mem's live in
// its address; the layout is remembered.
const mw = {
  rows: null, unlinked: 0, goals: 0, story: null, storyOpen: false, signature: '', fetched: 0, pending: null, slot: null, results: null,
  layout: MY_WORK_LAYOUTS.includes(recall('oboete-my-work-layout', 'cards')) ? recall('oboete-my-work-layout', 'cards') : 'cards',
  filter: 'all', query: '', project: '', range: 'day', sort: 'attention',
  agent: null, request: null, work: null, back: null,
  // F7: the focus area chosen (an id or `other`), for this visit only, as claude-mem's; the areas;
  // a save in progress, and the last change with its Undo; the asks under no area, and the
  // header's quotes opened.
  area: null, areas: [], moving: false, saved: null, otherAsks: [], why: new Set(),
  // J4: an answer to a judge's suggestion on its way.
  answering: false,
  // F7: the words an area's name may not reuse; the area being edited, how, and the draft.
  pageWords: [], editing: null, draft: '',
  // F7: the Suggested page's decisions on this visit (claude-mem keeps them until the page is
  // left), each with the area as it was; Past focus's months, once read.
  decided: new Map(), past: null,
  // The reads in the order they began, and the latest whose answer the page took.
  reads: 0, taken: 0,
  // The status line's last answer, its place, and how many reads of it began (V1).
  status: null, line: null, lineReads: 0,
  // claude-mem's `changed` (a flash, once) and `unread` (the "New update" mark until opened).
  changed: new Set(), unread: new Set(),
  // A check or its Undo being saved (`saveReview`'s `busy`).
  saving: false,
  // The open request's work product, by its id and version, once read (slice 4), and the number
  // of its latest read: an earlier read's late answer is not kept (CodeRabbit on #650).
  products: null, productsAsk: 0,
};

const myWorkCategory = (status) => (['working', 'needs', 'ready', 'checked'].includes(status) ? status : 'other');
const myWorkFlat = (text) => String(text ?? '').replace(/\s+/g, ' ').trim();

// claude-mem's `cleanText` cut: at most `max` characters, at a space past two thirds of them.
function myWorkClip(text, max) {
  const flat = myWorkFlat(text);
  if (flat.length <= max) return flat;
  const cut = flat.slice(0, max - 1);
  const space = cut.lastIndexOf(' ');
  return `${cut.slice(0, space > max * 0.65 ? space : max - 1).replace(/[,:;\-\s]+$/, '')}…`;
}

// An agent as the page names it elsewhere (Claude Code, Codex, Grok), else as recorded.
function myWorkAgent(name) {
  const key = `agent_inventory_agent_${name}`;
  return Object.hasOwn(TEXT, key) ? t(key) : String(name ?? '');
}

// `presentation.js`: the summary, else the newest recorded update, else the ask, else a line.
function myWorkActivity(row) {
  return row.summary || row.updates?.[0]?.text || myWorkClip(row.ask?.text, 300) || t('my_work_no_progress');
}

// `presentation.js`: the recorded next step, else a line for the status.
function myWorkNext(row) {
  if (row.next) return row.next;
  return t(row.status === 'ready' ? 'my_work_next_ready' : row.status === 'needs' ? 'my_work_next_needs' : 'my_work_next_other');
}

// `request-views.js` `nextCopy`: the next step where it says more than the summary.
function myWorkNextLine(row) {
  const next = myWorkFlat(myWorkNext(row));
  if (!next || next === myWorkFlat(myWorkActivity(row))) return null;
  const label = row.status === 'needs' ? 'my_work_status_needs' : row.status === 'ready' ? 'my_work_status_ready' : 'my_work_next_step';
  return { label, text: next };
}

const myWorkOpenLabel = (status) => (status === 'ready' ? 'my_work_open_review' : status === 'needs' ? 'my_work_open_handoff' : 'my_work_open_request');
const myWorkProjects = (row) => [...new Set((row.projects || []).map((p) => String(p).split('/')[0]).filter(Boolean))].join(' · ');

// `view-model.js` `relativeTime`.
function myWorkAgo(ms, now = Date.now()) {
  if (!Number.isFinite(ms) || ms <= 0) return t('my_work_no_time');
  const age = Math.max(0, now - ms);
  if (age < 60000) return t('my_work_now');
  if (age < 3600000) return t('my_work_minutes', { n: Math.floor(age / 60000) });
  if (age < 86400000) return t('my_work_hours', { n: Math.floor(age / 3600000) });
  if (age < 172800000) return t('my_work_yesterday');
  return t('my_work_days', { n: Math.floor(age / 86400000) });
}

function myWorkTime(ms) {
  const node = el('time', 'my-work-time', myWorkAgo(ms));
  if (Number.isFinite(ms) && ms > 0) {
    node.dateTime = new Date(ms).toISOString();
    node.title = new Date(ms).toLocaleString(lang);
    node.dataset.time = String(ms);
  }
  return node;
}

// claude-mem rewrites its times each second (`app.js` `timer`); here each poll's tick does.
function myWorkTimes(now = Date.now()) {
  for (const node of $('panel').querySelectorAll('.my-work-time')) {
    if (node.dataset.time) node.replaceChildren(myWorkAgo(Number(node.dataset.time), now));
  }
  for (const node of $('panel').querySelectorAll('.my-work-elapsed')) {
    node.replaceChildren(myWorkElapsed({ started: Number(node.dataset.started) }, now));
  }
}

// `view-model.js` `requestElapsed`, from the original ask.
function myWorkElapsed(row, now = Date.now()) {
  if (!Number.isFinite(row.started) || row.started <= 0) return t('my_work_not_recorded');
  const minutes = Math.max(0, Math.floor((now - row.started) / 60000));
  if (minutes < 1) return t('my_work_just_started');
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor(minutes / 60) % 24;
  if (days) return t('my_work_elapsed_dh', { d: days, h: hours });
  return hours ? t('my_work_elapsed_hm', { h: hours, m: minutes % 60 }) : t('my_work_elapsed_m', { m: minutes });
}

// `request-views.js` `badge`: `!` for needs, `✓` for ready.
function myWorkBadge(status) {
  const key = Object.hasOwn(TEXT, `my_work_status_${status}`) ? `my_work_status_${status}` : 'my_work_status_unknown';
  const glyph = status === 'needs' ? '!' : ['ready', 'checked', 'shipped'].includes(status) ? '✓' : '';
  const icon = glyph ? el('span', 'my-work-glyph', glyph) : null;
  icon?.setAttribute('aria-hidden', 'true');
  return el('span', `badge my-work-badge my-work-${status}`, icon, word('span', null, key));
}

// `request-views.js` `contributors`: one name an agent, with how many of its conversations.
function myWorkContributors(row) {
  const counts = new Map();
  for (const s of row.sessions || []) counts.set(s.agent, (counts.get(s.agent) || 0) + 1);
  if (!counts.size) return word('span', 'my-work-contributor muted', 'my_work_no_contributor');
  return el('span', 'my-work-contributors', ...[...counts].map(([agent, n]) =>
    el('span', 'my-work-contributor', myWorkAgent(agent), n > 1 ? t('my_work_contributors_more', { n }) : null)));
}

// A request's open button, found again after a redraw by its id.
function myWorkOpener(cls, row, label) {
  const button = el('button', cls, label);
  button.type = 'button';
  button.dataset.id = row.id;
  button.addEventListener('click', () => myWorkOpen(row.id));
  return button;
}

// A recorded day (YYYY-MM-DD) or a time, as claude-mem's work view shows a milestone's: "Oct 25".
function myWorkDay(value) {
  const at = typeof value === 'number' ? new Date(value) : new Date(`${value}T00:00:00`);
  return Number.isNaN(at.getTime()) ? String(value) : at.toLocaleDateString(lang, { month: 'short', day: 'numeric' });
}

// docs/my-work-model.md P5, as claude-mem 15.1's card shows a request's progress: the percent of its
// review with when it was reviewed, or the day it was completed, and the pace's estimate. Nothing
// before a review, as no empty field is shown.
function myWorkProgress(row) {
  const p = row.progress;
  if (p?.percent == null) return null;
  const meter = el('meter', 'my-work-meter');
  Object.assign(meter, { min: 0, max: 100, value: p.percent });
  meter.setAttribute('aria-label', t('my_work_progress_label'));
  // The review's age is a time each poll's tick rewrites, as the card's own (Codex on #694).
  const [before, after = ''] = t('my_work_progress', { p: p.percent, ago: '\u0000' }).split('\u0000');
  const text = p.completed ? t('my_work_completed', { date: myWorkDay(p.completed) })
    : p.reviewed ? el('span', null, before, myWorkTime(p.reviewed), after) : `${p.percent}%`;
  return el('div', 'my-work-progress', meter, el('span', 'my-work-progress-text', text),
    p.estimate ? el('span', 'my-work-estimate', t('my_work_pace', { date: myWorkDay(p.estimate) })) : null);
}

// P5: the build story on the request's page, the review's narrative and its milestones, the
// oldest first; none without them.
function myWorkStory(row) {
  const s = row.story;
  if (!s) return null;
  return el('section', 'my-work-story', word('h3', null, 'my_work_story'),
    s.narrative ? el('p', 'text', s.narrative) : null,
    s.milestones?.length ? el('ol', 'my-work-milestones', ...s.milestones.map((m) =>
      el('li', null, el('span', 'my-work-milestone-date', myWorkDay(m.date)), ' ', m.text))) : null);
}

// F7 (claude-mem's `needsYou`): a request waiting on the owner, to act on it or to check it.
const myFocusNeedsYou = (row) => row.status === 'needs' || row.status === 'ready';

// F7: whether My Focus is in use: an area listed, or a request placed. Until then neither the
// chips nor a request's area is drawn, as no card would say more than "Sorting…".
const myFocusInUse = () => mw.areas.length > 0 || (mw.rows || []).some((row) => row.focus);

// F7: the area the choice stands for: Other, or a listed area that is not dismissed; none, all of
// My Work. A merged area is not listed, so a choice of one falls back to all.
// shortcut: a merged area's choice shows all, where claude-mem shows its target; 10c-2's merge sets
// the choice to the target as it saves.
function myFocusSelected() {
  if (mw.area === 'other') return 'other';
  const area = mw.areas.find((a) => a.id === mw.area);
  return area && area.status !== 'dismissed' ? area.id : null;
}

const myFocusName = (id) => (id === 'other' ? t('my_focus_other') : mw.areas.find((a) => a.id === id)?.name ?? id);

// F7: the page of My Focus chosen instead of an area: Suggested, or Past focus.
const myFocusPage = () => (mw.area === 'suggested' || mw.area === 'past-focus' ? mw.area : null);
const myFocusSuggestions = () => mw.areas.filter((a) => a.status === 'suggested');

// F7: open My Work on `id` (an area or `other`), or on all of it for none; `focus` takes the focus
// after the redraw.
function myFocusChoose(id, focus = 'heading') {
  // Leaving Suggested ends its visit (claude-mem's decided cards).
  if (mw.area === 'suggested' && id !== 'suggested') mw.decided = new Map();
  mw.area = id;
  mw.editing = null;
  if (id === 'past-focus') void myFocusReadPast();
  mw.request = null;
  mw.work = null;
  myWorkDraw(focus);
}

// F7: the request's area, a button that opens it, or "Sorting…" while it has none.
function myFocusChip(row) {
  if (!myFocusInUse()) return null;
  if (!row.focus) return word('span', 'my-focus-chip sorting', 'my_focus_sorting');
  const button = el('button', 'my-focus-chip', myFocusName(row.focus));
  button.type = 'button';
  // On a card nothing else says what the name is.
  button.setAttribute('aria-label', `${t('my_focus_area')}: ${myFocusName(row.focus)}`);
  button.addEventListener('click', (event) => {
    event?.stopPropagation?.();
    myFocusChoose(row.focus);
  });
  return button;
}

// F7, claude-mem's chip row: the kept areas in order, then Other, each with how many of its
// requests need the owner; the chosen one pressed, and pressed again it shows all of My Work.
function myFocusChips() {
  if (!myFocusInUse()) return null;
  const area = myFocusSelected();
  const chosen = area ?? myFocusPage();
  const suggested = myFocusSuggestions().length;
  // claude-mem's chip row: then "N suggested" while any wait (and while its page is open), and Past
  // focus.
  const pages = [...(suggested || mw.area === 'suggested' ? ['suggested'] : []), 'past-focus'];
  const entries = [...mw.areas.filter((a) => a.status === 'kept').map((a) => a.id), 'other'];
  // A suggested or done area a card opened stands with them while chosen, pressed, to be cleared;
  // cleared, it is gone, and the focus goes to the requests' heading (CodeRabbit on #755).
  const temporary = area && !entries.includes(area) ? area : null;
  if (temporary) entries.splice(-1, 0, temporary);
  const label = (id) => (id === 'suggested' ? t('my_focus_n_suggested', { n: suggested })
    : id === 'past-focus' ? t('my_focus_past') : myFocusName(id));
  const nav = el('nav', 'my-focus-chips', ...[...entries, ...pages].map((id) => {
    const need = pages.includes(id) ? 0 : (mw.rows || []).filter((row) => row.focus === id && myFocusNeedsYou(row)).length;
    // The count is seen; what it counts is read (claude-mem's " need you").
    const count = need ? el('span', 'my-work-count', String(need)) : null;
    count?.setAttribute('aria-hidden', 'true');
    const said = need ? el('span', 'sr-only', t('my_focus_need_you', { n: need })) : null;
    const button = el('button', pages.includes(id) ? 'quiet my-focus-entry my-focus-muted' : 'quiet my-focus-entry', el('span', null, label(id)), count, said);
    button.type = 'button';
    button.setAttribute('aria-pressed', String(chosen === id));
    button.dataset.myWork = `chip:${id}`;
    button.addEventListener('click', () => myFocusChoose(chosen === id ? null : id, id === temporary ? 'current' : `chip:${id}`));
    return button;
  }));
  nav.setAttribute('aria-label', t('my_focus_label'));
  return nav;
}

// F6, F7: one change on My Focus at a time, its steps in order (claude-mem's Undo of a merge takes
// two), then My Work read again (My Rules' write): the change with its Undo when every step was
// applied, its words read after the reread when `text` is a function, so that they name what the
// gate kept (Codex on #757); a refusal's words otherwise, the steps after it not sent. A step saved
// but not read yet holds back the ones after it, which act on what it changed (Codex on #757):
// they are returned, for the same Undo to send.
async function myFocusWrite(bodies, text, undo, record = true) {
  if (mw.moving) return false;
  mw.moving = true;
  myWorkDraw();
  let message = null;
  let failed = false;
  let saved = null;
  try {
    let applied = true;
    for (const [i, body] of bodies.entries()) {
      const { res, answer } = await memoryWrite('my-focus', body);
      if (!res.ok) {
        failed = true;
        message = memoryFailure(res, answer);
        return false;
      }
      if (answer.state === 'applied') continue;
      applied = false;
      if (i < bodies.length - 1) {
        message = t('my_focus_held_steps');
        return bodies.slice(i + 1);
      }
    }
    saved = { applied };
    return applied || 'pending';
  } catch {
    failed = true;
    message = t('memory_result_unknown');
    return false;
  } finally {
    mw.moving = false;
    mw.signature = '';
    // Read again and drawn, whichever page its caller draws on after it, so that what was drawn
    // while saving is usable again (Codex on #776); a save the server took stays said as taken,
    // with its Undo, when the read fails (#602, #775).
    const read = view === 'my_work' ? await myWorkReread().then(() => true, () => false) : true;
    // Applied, and not read: a request moved is where the store has it now, so that a later
    // move's Undo takes it back there (#777).
    if (saved?.applied && !read) {
      for (const body of bodies.filter((b) => b.action === 'move-request')) {
        const row = (mw.rows || []).find((r) => r.id === body.request);
        if (row) row.focus = body.area;
      }
    }
    if (view === 'my_work') myWorkDraw();
    if (saved && !read) saved.unread = true;
    if (saved) {
      const said = typeof text === 'function' ? text() : text;
      // A Suggested card's Keep, Merge and Dismiss: the card shows it with its own Undo, once it
      // was read (Codex on #758).
      if (record) {
        mw.saved = { text: said, applied: saved.applied,
          undo: saved.applied && undo ? { bodies: undo.bodies, text: undo.text ?? t('my_focus_undone', { text: said }) } : null };
      }
      message = record || saved.applied ? said : t('my_focus_pending');
      if (saved.unread) message = t('my_work_saved_unread');
    }
    // The page's live region says it: the change bar is drawn anew with each redraw, too late to
    // be read out. Held, it outlasts the redraw that clears the status.
    if (message && view === 'my_work') setStatus(message, failed, lang, 8000);
  }
}

// Whether the chosen area has a header with a menu: not Other's.
const myFocusHasMenu = () => Boolean(myFocusSelected()) && myFocusSelected() !== 'other';

// F7: the last change, what saving it means, and its Undo (claude-mem's FocusChangeBar).
function myFocusChangeBar() {
  if (!mw.saved) return null;
  const { text, undo, applied } = mw.saved;
  let action = word('span', 'muted', 'my_focus_no_undo');
  if (undo) {
    action = word('button', 'quiet small', 'my_focus_undo');
    action.type = 'button';
    action.dataset.myWork = 'focus:undo';
    action.disabled = mw.moving;
    action.addEventListener('click', () => {
      void myFocusWrite(undo.bodies, undo.text, null).then((done) => {
        if (done === true || done === 'pending') mw.saved = null;
        else if (Array.isArray(done)) mw.saved.undo.bodies = done;
        if (view === 'my_work') myWorkDraw(mw.request !== null ? 'focus:move' : myFocusHasMenu() ? 'focus:menu' : 'heading');
      });
    });
  }
  const bar = el('div', 'my-focus-change', el('p', null, el('strong', null, text), ' ',
    word('span', null, applied ? 'my_focus_saved' : 'my_focus_pending')), applied ? action : null);
  return bar;
}

// F6: an area a request can be moved into (claude-mem's `isPlaceableFocusArea`).
const myFocusPlaceable = (a) => a.status === 'kept' || a.status === 'suggested';

// F6, F7: Move to… on a request's page: the areas that take requests but its own, then Other
// unless it is there; none when there is nowhere to move it. Undo moves it back, none from
// "Sorting…" or from an area that takes no requests (Codex on #755). A disclosure of buttons, as claude-mem's popover: a closed select would move the
// request as a person browses it with the arrow keys.
function myFocusMove(row) {
  const targets = mw.areas.filter((a) => myFocusPlaceable(a) && a.id !== row.focus)
    .map((a) => [a.id, a.status === 'suggested' ? `${a.name}${t('my_focus_suggested_suffix')}` : a.name]);
  if (row.focus !== 'other') targets.push(['other', t('my_focus_other')]);
  if (!targets.length) return null;
  const summary = word('summary', null, mw.moving ? 'my_focus_moving' : 'my_focus_move');
  summary.setAttribute('aria-label', t('my_focus_move_label'));
  summary.dataset.myWork = 'focus:move';
  return el('details', 'my-focus-move', summary, el('div', null, ...targets.map(([to, name]) => {
    const button = el('button', 'quiet small', name);
    button.type = 'button';
    button.disabled = mw.moving;
    button.dataset.myWork = `move:${to}`;
    button.addEventListener('click', () => {
      const from = row.focus;
      const text = t('my_focus_moved', { title: row.title, area: myFocusName(to) });
      const back = from === 'other' || mw.areas.some((a) => a.id === from && myFocusPlaceable(a));
      const undo = back ? { bodies: [{ action: 'move-request', request: row.id, area: from }],
        text: t('my_focus_moved', { title: row.title, area: myFocusName(from) }) } : null;
      // Not over another page opened while it saved (Codex on #755).
      void myFocusWrite([{ action: 'move-request', request: row.id, area: to }], text, undo)
        .then(() => view === 'my_work' && myWorkDraw('focus:move'));
    });
    return button;
  })));
}

// F7: an item's quotes behind one disclosure (claude-mem's WhyItsHere), kept open through redraws.
function myFocusWhy(key, quotes) {
  const n = quotes.length;
  const summary = el('summary', null, t(n === 1 ? 'my_goals_why_one' : 'my_goals_why_n', { n }));
  summary.dataset.myWork = `why:${key}`;
  const why = el('details', 'my-goals-why', summary,
    el('ul', 'my-goals-quotes', ...quotes.map((q) => el('li', null, el('q', null, q.text), ' ', myGoalsAt(q.ts)))));
  why.open = mw.why.has(key);
  why.addEventListener('toggle', () => (why.open ? mw.why.add(key) : mw.why.delete(key)));
  return why;
}

// F7 (claude-mem's NotStartedAsks): the open asks the goals found, none linked to a request.
function myFocusAsks(asks) {
  if (!asks.length) return null;
  return el('section', 'my-focus-asks', word('h4', null, 'my_focus_not_started', { n: asks.length }),
    el('ul', 'my-goals-list', ...asks.map((ask) => el('li', 'my-goals-item',
      el('div', 'my-goals-row', word('span', 'my-focus-badge', 'my_focus_not_started_badge'), el('span', 'my-goals-title', ask.title)),
      el('p', 'my-goals-meta', t('my_goals_ask_meta', { n: ask.asked, day: myGoalsDay(ask.latest) })),
      ask.quotes.length ? myFocusWhy(`ask:${ask.id}`, ask.quotes) : null))));
}

// claude-mem's `backTo`: the change that gives an area back the status it had; none for a
// suggestion the owner wrote, which nothing puts back in Suggested.
function myFocusBack(area) {
  if (area.status === 'suggested') return area.source === 'owner' ? null : [{ action: 'restore-suggestion', area: area.id }];
  return [{ action: { kept: 'keep', done: 'done', dismissed: 'dismiss' }[area.status], area: area.id }];
}

// claude-mem's `describeFocusChange`: what a change on `area` says, and its Undo, read from the
// area as it was before it: none for a goal written where there was none, nor for a merge of an
// area the owner wrote, whose Undo would have to put it back in Suggested.
function myFocusDescribe(body, area) {
  const name = area.name;
  const said = (key, vars, bodies) => ({ text: t(key, { name, ...vars }), undo: bodies ? { bodies } : null });
  switch (body.action) {
    case 'keep': return said('my_focus_kept', {}, myFocusBack(area));
    case 'dismiss': return said('my_focus_dismissed', {}, myFocusBack(area));
    case 'done': return said('my_focus_marked_done', {}, myFocusBack(area));
    // The new name as the gate kept it, read again, not as it was typed; none while it waits to
    // be read. No Undo back to a name that reuses a page word, which the server refuses (Codex and
    // CodeRabbit on #757).
    case 'rename': return { undo: area.page_word ? null : { bodies: [{ action: 'rename', area: area.id, name }] }, text: () => {
      const to = myFocusName(area.id);
      return to === name ? t('my_focus_renamed_pending', { name }) : t('my_focus_renamed', { name, to });
    } };
    case 'edit-goal': return said('my_focus_goal_changed', {}, area.goal ? [{ action: 'edit-goal', area: area.id, goal: area.goal }] : null);
    case 'merge': return said('my_focus_merged', { into: myFocusName(body.into) }, area.source === 'owner' ? null
      : [{ action: 'restore-suggestion', area: area.id }, ...(area.status === 'suggested' ? [] : myFocusBack(area))]);
    default: return said('my_focus_reordered', {}, [{ action: 'reorder', order: mw.areas.filter((a) => a.status === 'kept').map((a) => a.id) }]);
  }
}

// One change on `area` from its header, its menu or its editor: the editor closed once saved, a
// merge's choice moved to the area merged into; the focus back on the menu, or on the heading
// when the area is gone from the choice.
function myFocusChange(body, area) {
  const { text, undo } = myFocusDescribe(body, area);
  const from = mw.area;
  void myFocusWrite([body], text, undo).then((ok) => {
    if (ok === true || ok === 'pending') {
      mw.editing = null;
      // Not over a choice made while it saved (Codex on #757).
      if (body.action === 'merge' && mw.area === from) mw.area = body.into;
    }
    // Refused, back on the editor kept open (CodeRabbit on #757).
    if (view === 'my_work') myWorkDraw(ok !== true && mw.editing ? `edit:${mw.editing.kind}` : myFocusHasMenu() ? 'focus:menu' : 'current');
  });
}

// `my_work_focus::page_word`: the page word a name reuses as whole words of its ASCII letters and
// digits, or the page's Japanese name it is.
function myFocusPageWord(name) {
  const words = ` ${String(name).toLowerCase().split(/[^a-z0-9]+/).filter(Boolean).join(' ')} `;
  return mw.pageWords.find((w) => (/^[a-z0-9 ]+$/.test(w) ? words.includes(` ${w} `) : String(name).trim() === w)) ?? null;
}

function myFocusButton(key, focus, onClick, cls = 'quiet small') {
  const button = word('button', cls, key);
  button.type = 'button';
  button.dataset.myWork = focus;
  button.disabled = mw.moving;
  button.addEventListener('click', onClick);
  return button;
}

// F7 (claude-mem's AreaMenu): Rename, Edit goal, Merge into…; Move up and Move down in the owner's
// order of the kept areas, the whole order sent; then Done, or Back in focus. A disclosure of
// buttons, as Move to….
function myFocusMenu(area) {
  const order = mw.areas.filter((a) => a.status === 'kept').map((a) => a.id);
  const at = order.indexOf(area.id);
  const swap = (other) => {
    const next = [...order];
    [next[at], next[other]] = [next[other], next[at]];
    myFocusChange({ action: 'reorder', order: next }, area);
  };
  const edit = (kind) => () => {
    mw.editing = { kind, area: area.id };
    mw.draft = kind === 'rename' ? area.name : kind === 'edit-goal' ? area.goal ?? '' : '';
    myWorkDraw(`edit:${kind}`);
  };
  const moves = at < 0 ? [] : [
    myFocusButton('my_focus_up', 'menu:up', () => swap(at - 1)),
    myFocusButton('my_focus_down', 'menu:down', () => swap(at + 1)),
  ];
  if (moves.length) {
    moves[0].disabled ||= at === 0;
    moves[1].disabled ||= at === order.length - 1;
  }
  const summary = el('summary', null, '…');
  summary.setAttribute('aria-label', t('my_focus_change', { name: area.name }));
  summary.dataset.myWork = 'focus:menu';
  const menu = el('details', 'my-focus-menu', summary, el('div', null,
    myFocusButton('my_focus_rename', 'menu:rename', edit('rename')),
    myFocusButton('my_focus_edit_goal', 'menu:edit-goal', edit('edit-goal')),
    myFocusButton('my_focus_merge', 'menu:merge', edit('merge')), ...moves,
    area.status === 'done' ? myFocusButton('my_focus_back_in_focus', 'menu:keep', () => myFocusChange({ action: 'keep', area: area.id }, area))
      : myFocusButton('my_focus_mark_done', 'menu:done', () => myFocusChange({ action: 'done', area: area.id }, area))));
  // It opens over the page, so Escape closes it, the focus back on it.
  menu.addEventListener('keydown', (event) => {
    if (event.key !== 'Escape' || !menu.open) return;
    menu.open = false;
    summary.focus();
  });
  return menu;
}

// F7 (claude-mem's AreaEditDialog), inline as My Rules edits: a name, a goal or the area to merge
// into; Save held while the field is empty, the name reuses a page word or no area is chosen. The
// server refuses what it cannot save.
function myFocusEditor(area, change = myFocusChange, back = 'focus:menu') {
  const { kind } = mw.editing;
  const save = myFocusButton(mw.moving ? 'my_focus_saving' : 'my_focus_save', 'edit:save', () => change(
    kind === 'merge' ? { action: 'merge', area: area.id, into: mw.draft }
      : kind === 'rename' ? { action: 'rename', area: area.id, name: mw.draft } : { action: 'edit-goal', area: area.id, goal: mw.draft }, area), 'small');
  const cancel = myFocusButton('my_focus_cancel', 'edit:cancel', () => {
    mw.editing = null;
    myWorkDraw(back);
  });
  const warning = el('p', 'my-focus-warning');
  warning.setAttribute('aria-live', 'polite');
  // The server's bounds, in characters as it counts them, not the field's UTF-16 units (CodeRabbit
  // on #757).
  const most = kind === 'rename' ? 80 : 300;
  const check = () => {
    const reused = kind === 'rename' ? myFocusPageWord(mw.draft) : null;
    const long = kind !== 'merge' && [...mw.draft.trim()].length > most;
    warning.replaceChildren(...(reused ? [t('my_focus_page_word_typed', { name: mw.draft.trim(), word: reused })]
      : long ? [t('my_focus_too_long', { n: most })] : []));
    save.disabled = mw.moving || !mw.draft.trim() || Boolean(reused) || long;
  };
  let field;
  if (kind === 'merge') {
    const targets = mw.areas.filter((a) => myFocusPlaceable(a) && a.id !== area.id);
    field = el('select', null, el('option', null, t('my_focus_choose_area')), ...targets.map((a) => {
      const option = el('option', null, a.status === 'suggested' ? `${a.name}${t('my_focus_suggested_suffix')}` : a.name);
      option.value = a.id;
      return option;
    }));
    field.options[0].value = '';
    field.value = mw.draft;
    field.addEventListener('change', () => { mw.draft = field.value; check(); });
  } else {
    field = input('text', mw.draft, '', `focus-${kind}`, (v) => { mw.draft = v; check(); });
  }
  field.dataset.myWork = `edit:${kind}`;
  field.disabled = mw.moving;
  check();
  const label = { rename: 'my_focus_name', 'edit-goal': 'my_focus_goal', merge: 'my_focus_merge_into' }[kind];
  const title = { rename: 'my_focus_rename_title', 'edit-goal': 'my_focus_goal_title', merge: 'my_focus_merge_title' }[kind];
  const about = { rename: 'my_focus_rename_about', 'edit-goal': 'my_focus_goal_about', merge: 'my_focus_merge_about' }[kind];
  return el('section', 'my-focus-editor', word('h4', null, title, { name: area.name }), word('p', 'my-goals-note', about, { name: area.name }),
    el('label', 'field', el('span', null, t(label)), field), warning, el('div', 'my-rules-actions', save, cancel));
}

// claude-mem's SOURCE_TEXT: why the system suggested an area that has no evidence of its own.
const MY_FOCUS_SOURCES = ['owner-goals', 'clustering', 'promotion', 'owner'];

// A Suggested card's Keep, Dismiss or Merge (claude-mem's `decide` and its merge, sent with
// `record: false`): the card says it with its own Undo, `restore-suggestion`, until the page is left.
function myFocusDecide(area, body, decision) {
  const into = body.into ? myFocusName(body.into) : null;
  const text = t(`my_focus_decided_${decision}`, { into });
  // The visit it was made in: one left while it saved records nothing in the next (#759).
  const visit = mw.decided;
  void myFocusWrite([body], text, null, false).then((ok) => {
    // Decided once applied, as a read then shows it; one held back stays as it was (Codex on #758).
    // Applied, it is decided though the read after it failed: the store holds it, its Undo acts on
    // the store, and the next poll reads it (Codex on #776).
    if (ok === true) visit.set(area.id, { decision, area, into: body.into ?? null });
    if (ok === true || ok === 'pending') mw.editing = null;
    if (view !== 'my_work' || mw.area !== 'suggested') return;
    // The focus on what the card has now (CodeRabbit on #758): its Undo, or Show it for an area the
    // owner wrote, which has none; refused, the editor still open or the action again.
    const verb = { kept: 'keep', dismissed: 'dismiss', merged: 'merge' }[decision];
    myWorkDraw(ok === true ? (area.source !== 'owner' ? `card:undo:${area.id}` : decision === 'dismissed' ? 'heading' : `card:show:${area.id}`)
      : mw.editing?.area === area.id ? `edit:${mw.editing.kind}` : `card:${verb}:${area.id}`);
  });
}

function myFocusUndecide(area) {
  const visit = mw.decided;
  void myFocusWrite([{ action: 'restore-suggestion', area: area.id }], t('my_focus_restored', { name: area.name }), null, false).then((ok) => {
    if (ok === true) visit.delete(area.id);
    if (view === 'my_work' && mw.area === 'suggested') myWorkDraw(ok === true ? `card:keep:${area.id}` : `card:undo:${area.id}`);
  });
}

// F7, claude-mem's SuggestionCard: a suggested area, its name and goal, how many requests it holds
// and asks not started; Keep, Rename, Merge and Dismiss, then why it is suggested; decided, what
// was decided, Show it and Undo.
function myFocusCard(area) {
  const decided = mw.decided.get(area.id);
  const placed = (mw.rows || []).filter((row) => row.focus === area.id).length;
  const asks = area.asks?.length ?? 0;
  const facts = t(placed === 1 ? 'my_focus_placed_one' : placed ? 'my_focus_placed_n' : 'my_focus_placed_none', { n: placed })
    + (asks ? t(asks === 1 ? 'my_focus_asks_one' : 'my_focus_asks_n', { n: asks }) : '');
  const head = [el('p', 'my-focus-card-name', area.name), area.goal ? el('p', 'my-focus-goal', area.goal) : null, el('p', 'my-goals-meta', facts)];
  if (decided) {
    const show = decided.decision === 'dismissed' ? null
      : myFocusButton('my_focus_show_it', `card:show:${area.id}`, () => myFocusChoose(decided.into ?? area.id));
    return el('li', `my-goals-item my-focus-card my-focus-card-${decided.decision}`, ...head,
      el('div', 'my-rules-actions', el('p', 'my-focus-decided', t(`my_focus_decided_${decided.decision}`, { into: myFocusName(decided.into) })),
        // No Undo for an area the owner wrote, which the server never puts back in Suggested (Codex
        // and CodeRabbit on #758).
        show, area.source === 'owner' ? null : myFocusButton('my_focus_undo', `card:undo:${area.id}`, () => myFocusUndecide(area))));
  }
  const merge = (body, a) => (body.action === 'merge' ? myFocusDecide(a, body, 'merged') : myFocusChange(body, a));
  const editing = mw.editing?.area === area.id ? myFocusEditor(area, merge, `card:${mw.editing.kind}:${area.id}`) : null;
  const edit = (kind) => () => {
    mw.editing = { kind, area: area.id };
    mw.draft = kind === 'rename' ? area.name : '';
    myWorkDraw(`edit:${kind}`);
  };
  const [first, ...more] = area.quotes ?? [];
  const why = area.evidence && !first ? area.evidence
    : t(MY_FOCUS_SOURCES.includes(area.source) ? `my_focus_source_${area.source.replace('-', '_')}` : 'my_focus_source_owner_goals');
  return el('li', 'my-goals-item my-focus-card', ...head,
    el('div', 'my-rules-actions',
      myFocusButton('my_focus_keep', `card:keep:${area.id}`, () => myFocusDecide(area, { action: 'keep', area: area.id }, 'kept'), 'small'),
      myFocusButton('my_focus_rename', `card:rename:${area.id}`, edit('rename')),
      myFocusButton('my_focus_merge_short', `card:merge:${area.id}`, edit('merge')),
      myFocusButton('my_focus_dismiss', `card:dismiss:${area.id}`, () => myFocusDecide(area, { action: 'dismiss', area: area.id }, 'dismissed'))),
    editing,
    el('p', 'my-goals-note', t('my_focus_why_suggested', { why })),
    first ? el('p', 'my-goals-quote', el('q', null, first.text), ' ', myGoalsAt(first.ts)) : null,
    more.length ? myFocusMore(`card:${area.id}`, more) : null);
}

// The quotes after the first, behind one disclosure (claude-mem's "n more quotes").
function myFocusMore(key, quotes) {
  const summary = el('summary', null, t(quotes.length === 1 ? 'my_focus_more_one' : 'my_focus_more_n', { n: quotes.length }));
  summary.dataset.myWork = `why:${key}`;
  const more = el('details', 'my-goals-why', summary,
    el('ul', 'my-goals-quotes', ...quotes.map((q) => el('li', null, el('q', null, q.text), ' ', myGoalsAt(q.ts)))));
  more.open = mw.why.has(key);
  more.addEventListener('toggle', () => (more.open ? mw.why.add(key) : mw.why.delete(key)));
  return more;
}

// F7, claude-mem's SuggestedAreas: the suggestions waiting, then those decided on this visit, which
// stay until the page is left; how many are decided; the change bar for a rename.
function myFocusSuggested() {
  const decided = [...mw.decided.values()].map((d) => mw.areas.find((a) => a.id === d.area.id) ?? d.area);
  const cards = [...myFocusSuggestions().filter((a) => !mw.decided.has(a.id)), ...decided];
  const waiting = cards.length - decided.length;
  return el('section', 'my-focus-page',
    el('div', 'my-focus-header-name', word('h3', null, 'my_focus_suggested_title'),
      waiting ? word('span', 'my-goals-status my-goals-status-your_move', 'my_goals_status_your_move') : null,
      el('span', 'my-goals-meta', t('my_focus_decided_count', { k: decided.length, n: cards.length }))),
    word('p', 'my-goals-note', 'my_focus_suggested_about'), myFocusChangeBar(),
    cards.length ? el('ul', 'my-goals-list', ...cards.map(myFocusCard)) : word('p', 'my-goals-note', 'my_focus_nothing_waiting'));
}

// Past focus's months: My Goals' answer, read when the page opens.
async function myFocusReadPast() {
  try {
    const answer = await api('my-goals');
    mw.past = answer;
    if (view === 'my_work' && mw.area === 'past-focus') myWorkDraw(document.activeElement?.dataset?.myWork ?? null);
  } catch {
    // The months stay as they were read last; the done areas need none.
  }
}

// F7, claude-mem's PastFocus: the areas marked done, each opening on its requests; then month by
// month, this month's row naming the first kept area today, and each month kept before with its
// headline and its priorities.
function myFocusPast() {
  const done = mw.areas.filter((a) => a.status === 'done');
  const doneList = done.length ? el('div', 'my-focus-done', word('h4', null, 'my_focus_done_areas'), el('ul', 'my-goals-list', ...done.map((a) => {
    const open = el('button', 'quiet my-focus-done-open', el('span', 'my-goals-title', a.name), a.goal ? el('span', 'my-goals-meta', a.goal) : null);
    open.type = 'button';
    open.dataset.myWork = `done:${a.id}`;
    open.addEventListener('click', () => myFocusChoose(a.id));
    return el('li', null, open);
  }))) : null;
  const p = mw.past;
  let months = null;
  if (p?.current || p?.months?.length) {
    const top = mw.areas.find((a) => a.status === 'kept');
    const row = (month, badge, title, prompts) => [el('span', 'my-goals-month', myGoalsMonthName(month)),
      word('span', 'my-goals-badge', badge), el('span', 'my-goals-title', title), myGoalsMeta([t('my_goals_prompts', { n: prompts })])];
    const rows = [el('li', 'my-goals-month-row', el('div', 'my-goals-row', ...row(p.month?.month, 'my_goals_current',
      top ? t('my_focus_first_today', { name: top.name }) : t('my_focus_none_kept'), p.month?.prompts ?? 0)))];
    for (const m of p.months ?? []) {
      const summary = el('summary', 'my-goals-row', ...row(m.month, 'my_goals_archived', m.priorities[0]?.title ?? t('my_focus_no_areas_saved'), m.prompts));
      summary.dataset.myWork = `month:${m.month}`;
      const details = el('details', 'my-goals-month-detail', summary,
        m.summary ? el('p', null, m.summary) : null,
        el('ol', 'my-goals-list', ...m.priorities.map((pr) => el('li', 'my-goals-item',
          el('div', 'my-goals-row', el('span', 'my-goals-title', pr.title), myGoalsMeta([myGoalsAsked(pr.asked)], pr.product)),
          pr.quote ? el('p', 'my-goals-quote', el('q', null, pr.quote.text), ' ', myGoalsAt(pr.quote.ts)) : null))),
        word('p', 'my-goals-note', 'my_goals_no_reviews'));
      details.open = mw.why.has(`month:${m.month}`);
      details.addEventListener('toggle', () => (details.open ? mw.why.add(`month:${m.month}`) : mw.why.delete(`month:${m.month}`)));
      rows.push(el('li', 'my-goals-month-row', details));
    }
    months = el('div', 'my-focus-months', word('h4', null, 'my_goals_tab_months'), word('p', 'my-goals-note', 'my_goals_history_note'), el('ul', 'my-goals-months', ...rows));
  }
  return el('section', 'my-focus-page', word('h3', null, 'my_focus_past'), word('p', 'my-goals-note', 'my_focus_past_about'),
    doneList, months, !doneList && !months && mw.past ? myWorkEmpty('my_focus_past_empty', 'my_focus_past_empty_body') : null);
}

// F7, claude-mem's FocusHeader and OtherHeader, above the chosen area's requests: its name, a
// Suggested or Done badge, its goal, the page word its name reuses; the progress of all its
// requests, closed ones too, where it stands, how many shipped, how often it was asked and when
// last, an ask after the last ship; the asks not started, and why it is here; its menu, an
// editor open, and a suggested area's Keep and Dismiss.
function myFocusHeader(id) {
  const requests = (mw.rows || []).filter((row) => row.focus === id);
  const shipped = word('span', null, 'my_focus_shipped', { n: requests.filter((row) => row.status === 'shipped').length });
  if (id === 'other') {
    return el('section', 'my-focus-header', el('h3', null, t('my_focus_other')), word('p', 'my-goals-note', 'my_focus_other_about'),
      myGoalsRollup(requests), el('p', 'my-goals-meta', shipped), myFocusAsks(mw.otherAsks));
  }
  const area = mw.areas.find((a) => a.id === id);
  if (!area) return null;
  const goal = { last: area.latest };
  const status = myGoalsStatus(goal, requests);
  const again = area.latest ? myGoalsAskedAgain(goal, requests) : null;
  const facts = [myGoalsStatusWord(status), shipped];
  if (area.asked !== null && area.latest) facts.push(word('span', null, 'my_focus_asked', { asked: myGoalsAsked(area.asked), last: t('my_goals_last', { day: myGoalsDay(area.latest) }) }));
  const badge = area.status === 'suggested' || area.status === 'done' ? word('span', 'my-focus-badge', `my_focus_${area.status}`) : null;
  const editing = mw.editing?.area === area.id ? myFocusEditor(area) : null;
  return el('section', 'my-focus-header',
    el('div', 'my-focus-header-name', el('h3', null, area.name), badge, myFocusMenu(area)),
    area.goal ? el('p', 'my-focus-goal', area.goal) : word('p', 'my-focus-goal my-focus-prompt', 'my_focus_goal_prompt'), editing,
    area.page_word ? el('p', 'my-focus-warning', t('my_focus_page_word', { name: area.name, word: area.page_word })) : null,
    myGoalsRollup(requests), el('p', 'my-goals-meta my-focus-facts', ...facts),
    again ? el('p', 'my-goals-again', t('my_goals_again', { asked: myWorkDay(area.latest), shipped: myWorkDay(again) })) : null,
    myFocusAsks(area.asks || []),
    area.quotes?.length ? myFocusWhy(`area:${area.id}`, area.quotes)
      : area.evidence ? el('p', 'my-goals-note', t('my_focus_evidence', { evidence: area.evidence })) : null,
    area.status === 'suggested' ? el('div', 'my-rules-actions my-focus-decide',
      myFocusButton('my_focus_keep', 'decide:keep', () => myFocusChange({ action: 'keep', area: area.id }, area), 'small'),
      myFocusButton('my_focus_dismiss', 'decide:dismiss', () => myFocusChange({ action: 'dismiss', area: area.id }, area))) : null);
}

// P6, V4: the build story at My Work's top, as claude-mem's recap: the recap, then the chapters,
// the newest week first, behind one disclosure; nothing while none was written.
function myWorkBuildStory() {
  const s = mw.story;
  if (!s || (!s.recap && !s.chapters?.length)) return null;
  const end = (week) => {
    const d = new Date(`${week}T00:00:00`);
    d.setDate(d.getDate() + 6);
    return myGoalsIsoDay(d.getTime());
  };
  const chapters = (s.chapters || []).map((c) => el('section', 'my-work-chapter',
    el('h4', null, t('my_work_story_week', { start: myWorkDay(c.week), end: myWorkDay(end(c.week)) })),
    ...String(c.text ?? '').split(/\n\s*\n/).map((p) => el('p', 'text', p))));
  let details = null;
  if (chapters.length) {
    details = el('details', 'my-work-chapters', word('summary', null, 'my_work_story_chapters', { n: chapters.length }), ...chapters);
    details.open = mw.storyOpen;
    details.addEventListener('toggle', () => { mw.storyOpen = details.open; });
  }
  return el('section', 'my-work-build-story', word('h3', null, 'my_work_build_story'),
    s.recap ? el('p', 'my-work-recap', s.recap) : null, details);
}

// `request-views.js` `requestCards`: the title opens the request, and the whole card with it.
function myWorkCard(row) {
  const next = myWorkNextLine(row);
  const card = el('article', `my-work-card my-work-status-${row.status}${mw.changed.has(row.id) ? ' my-work-updated' : ''}`,
    el('div', 'my-work-card-meta', myWorkBadge(row.status), myWorkTime(row.updated),
      mw.unread.has(row.id) ? word('span', 'my-work-new', 'my_work_new') : null),
    el('h3', 'my-work-title', myWorkOpener('my-work-open', row, row.title)),
    el('p', 'my-work-summary', myWorkActivity(row)),
    row.updating ? word('small', 'my-work-copy-note', 'my_work_copy_updating') : null,
    myWorkSuggestion(row),
    myWorkProgress(row),
    myFocusChip(row),
    el('div', 'my-work-context', myWorkContributors(row), el('span', 'my-work-project', myWorkProjects(row))),
    el('div', next ? 'my-work-footer' : 'my-work-footer no-next',
      next ? el('div', 'my-work-next', word('span', 'my-work-next-label', next.label), el('p', null, next.text)) : null,
      word('span', 'my-work-open-label', myWorkOpenLabel(row.status))));
  card.dataset.id = row.id;
  return card;
}

function myWorkCell(label, ...children) {
  const cell = el('td', null, ...children);
  cell.dataset.label = t(label);
  return cell;
}

// J4: "Looks like part of <title>" with Join and Not this, on the request of a conversation the
// judge suggested; nothing when it suggested none.
function myWorkSuggestion(row) {
  const s = row.suggestion;
  if (!s) return null;
  const button = (action, key) => {
    const b = el('button', 'quiet small my-work-answer', t(key));
    b.type = 'button';
    b.dataset.action = `my-work.suggestion.${action}`;
    b.setAttribute('aria-label', `${t(key)}: ${s.title}`);
    b.disabled = mw.answering;
    b.addEventListener('click', () => void myWorkAnswer(row, action));
    return b;
  };
  return el('div', 'my-work-suggestion', el('p', null, t('my_work_suggestion', { title: s.title })),
    el('div', 'actions', button('join', 'my_work_suggestion_join'), button('not-this', 'my_work_suggestion_not_this')));
}

// J4: the owner's answer, then My Work read again and drawn once (#775). Only the answers' buttons
// wait, as a review's do, since drawing clears the status line; a save the server took stays said
// as taken when the read after it fails (#602). A Join names, and from the request's page shows,
// the request the conversation is in as the read after it has it.
async function myWorkAnswer(row, action) {
  if (mw.answering) return;
  mw.answering = true;
  for (const button of $('panel').querySelectorAll('.my-work-answer')) button.disabled = true;
  let message = t('memory_result_unknown');
  let failed = true;
  let applied = false;
  try {
    const { res, answer } = await memoryWrite('my-work/suggestion', { request: row.id, action });
    if (!res.ok) message = memoryFailure(res, answer);
    else {
      failed = false;
      applied = answer.state === 'applied';
      if (!applied) message = t('my_focus_pending');
    }
    await myWorkReread().catch(() => {
      applied = false;
      if (res.ok) message = t('my_work_saved_unread');
    });
  } catch {
    // Not saved, as far as the page knows; nothing changed.
  } finally {
    mw.answering = false;
  }
  if (view !== 'my_work') return;
  if (applied && action === 'not-this') message = t('my_work_suggestion_declined');
  if (applied && action === 'join') {
    // Where the conversation is now, the one the suggestion is about, not the request's member
    // active last (#777); in no request read (out of the range read), said as saved.
    const session = row.suggestion?.conversation?.id;
    const now = (mw.rows || []).find((r) => r.sessions?.some((s) => s.id === session));
    if (mw.request === row.id) mw.request = now?.id ?? null;
    message = now ? t('my_work_suggestion_joined', { title: now.title }) : t('my_work_saved_unread');
  }
  // The read after the answer recorded what it read, so the poll after it finds nothing new; a read
  // that failed left the one before, which the next answer is not (#777).
  myWorkDraw('current');
  setStatus(message, failed, lang, 8000);
}

// `request-views.js` `requestList`.
function myWorkTable(rows) {
  const heads = ['my_work_col_status', 'my_work_col_request', 'my_work_col_next', 'my_work_col_updated'].map((key) => word('th', null, key));
  heads.push(el('th', null, word('span', 'sr-only', 'my_work_open_request')));
  for (const th of heads) th.scope = 'col';
  const body = rows.map((row) => {
    const arrow = myWorkOpener('quiet small my-work-row-open', row, '→');
    arrow.setAttribute('aria-label', t('my_work_open_label', { title: row.title }));
    const tr = el('tr', 'my-work-row',
      myWorkCell('my_work_col_status', myWorkBadge(row.status)),
      myWorkCell('my_work_col_request', myWorkOpener('my-work-row-title', row, row.title), el('p', 'my-work-project', myWorkProjects(row)),
        myWorkSuggestion(row)),
      myWorkCell('my_work_col_next', el('p', 'my-work-next-copy', myWorkNextLine(row)?.text || myWorkActivity(row)),
        row.updating ? word('small', 'my-work-copy-note', 'my_work_copy_updating') : null),
      myWorkCell('my_work_col_updated', myWorkTime(row.updated)),
      el('td', 'my-work-row-arrow', arrow));
    tr.dataset.id = row.id;
    return tr;
  });
  const table = el('table', 'my-work-table', el('thead', null, el('tr', null, ...heads)), el('tbody', null, ...body));
  table.setAttribute('aria-label', t('my_work_table'));
  return el('div', 'table-scroll', table);
}

// `request-views.js` `agentCards`: one card a conversation (decision 55), in the order of the
// visible requests; choosing one shows its requests.
function myWorkAgents(rows) {
  const groups = new Map();
  for (const row of rows) {
    for (const session of row.sessions || []) {
      if (!groups.has(session.id)) groups.set(session.id, { session, requests: [] });
      groups.get(session.id).requests.push(row);
    }
  }
  if (!groups.size) return myWorkEmpty('my_work_no_agents', 'my_work_no_agents_body');
  return el('div', 'my-work-agents', ...[...groups.values()].map(({ session, requests }) => {
    const latest = [...requests].sort((a, b) => b.updated - a.updated)[0];
    const label = myWorkAgent(session.agent);
    const avatar = el('span', 'my-work-avatar', label.slice(0, 1).toUpperCase() || '?');
    avatar.setAttribute('aria-hidden', 'true');
    const open = el('button', 'my-work-open', label);
    open.type = 'button';
    open.dataset.session = session.id;
    open.addEventListener('click', () => {
      mw.agent = session.id;
      myWorkDraw('agents');
    });
    const count = requests.length === 1 ? 'my_work_agent_count_one' : 'my_work_agent_count';
    const card = el('article', 'my-work-card my-work-agent',
      el('div', 'my-work-agent-top', avatar,
        el('div', 'my-work-agent-identity', el('h3', 'my-work-title', open), word('p', 'my-work-agent-role', 'my_work_agent_role')),
        myWorkTime(latest.updated)),
      myWorkBadge(latest.status),
      el('p', 'my-work-agent-summary', latest.title),
      el('div', 'my-work-agent-requests', ...requests.slice(0, 2).map((row) => el('p', null, myWorkActivity(row)))),
      el('div', 'my-work-footer', word('span', null, count, { n: requests.length }), word('span', 'my-work-open-label', 'my_work_agent_view')));
    card.dataset.session = session.id;
    return card;
  }));
}

function myWorkEmpty(title, body, timeline = false) {
  return el('div', 'my-work-empty', word('h3', null, title), word('p', null, body), timeline ? myWorkTimeline() : null);
}

function myWorkTimeline(key = 'my_work_to_timeline') {
  const button = word('button', 'quiet small my-work-link', key);
  button.type = 'button';
  button.addEventListener('click', () => {
    setView('timeline');
    void show();
  });
  return button;
}

const myWorkSearchText = (row) => [row.title, myWorkActivity(row), ...(row.sessions || []).map((s) => myWorkAgent(s.agent)),
  ...(row.projects || [])].join(' ').toLowerCase();

// `presentation.js` `scopeWork`, then the conversation the agent view chose (`scopedRows`).
function myWorkScoped(now = Date.now()) {
  const since = mw.range === 'day' ? now - 86400000 : mw.range === 'week' ? now - 7 * 86400000 : 0;
  const query = mw.query.trim().toLowerCase();
  const area = myFocusSelected();
  return (mw.rows || []).filter((row) => row.updated >= since
    && (!area || row.focus === area)
    && (!mw.project || (row.projects || []).includes(mw.project))
    && (!query || myWorkSearchText(row).includes(query))
    && (!mw.agent || (row.sessions || []).some((s) => s.id === mw.agent)));
}

// `presentation.js` `sortWork`.
function myWorkSorted(rows) {
  return [...rows].sort((a, b) => {
    const title = () => myWorkTitles.compare(a.title, b.title) || String(a.id).localeCompare(String(b.id));
    if (mw.sort === 'title') return title();
    const rank = mw.sort === 'attention' ? (MY_WORK_ATTENTION[a.status] ?? 6) - (MY_WORK_ATTENTION[b.status] ?? 6) : 0;
    const at = Number.isFinite(a.updated) && a.updated > 0 ? a.updated : 0;
    const bt = Number.isFinite(b.updated) && b.updated > 0 ? b.updated : 0;
    const time = !at || !bt ? (at ? -1 : bt ? 1 : 0) : mw.sort === 'oldest' ? at - bt : bt - at;
    return rank || time || title();
  });
}

// `app.js` `visibleRows`.
const myWorkVisible = (scoped) => myWorkSorted(scoped.filter((row) => mw.filter === 'all'
  || (mw.filter === 'other' ? myWorkCategory(row.status) === 'other' : row.status === mw.filter)));

// `app.js` `render`'s tabs: the counts are of the scoped requests, before the tab's own filter.
function myWorkTabs(scoped) {
  const counts = Object.fromEntries(MY_WORK_TABS.map((key) => [key, key === 'all' ? scoped.length : 0]));
  for (const row of scoped) {
    counts[MY_WORK_TABS.includes(row.status) ? row.status : 'unknown']++;
    if (myWorkCategory(row.status) === 'other') counts.other++;
  }
  const shown = MY_WORK_TABS.filter((key) => MY_WORK_ALWAYS.includes(key) || key === mw.filter || (key !== 'other' && counts[key]));
  const nav = el('nav', 'my-work-tabs', ...shown.map((key) => {
    const button = el('button', 'tab my-work-tab', word('span', null, `my_work_status_${key}`), el('span', 'my-work-count', String(counts[key])));
    button.type = 'button';
    button.setAttribute('aria-pressed', String(mw.filter === key));
    button.dataset.myWork = `tab:${key}`;
    button.addEventListener('click', () => {
      mw.filter = key;
      myWorkDraw(`tab:${key}`);
    });
    return button;
  }));
  nav.setAttribute('aria-label', t('my_work_filter_label'));
  return nav;
}

function myWorkSelect(name, label, options, value, set) {
  const select = el('select', `my-work-${name}`, ...options.map(([v, text]) => {
    const option = el('option', null, text);
    option.value = v;
    option.selected = v === value;
    return option;
  }));
  select.value = value;
  select.setAttribute('aria-label', t(label));
  select.dataset.myWork = name;
  select.addEventListener('change', () => {
    set(select.value);
    myWorkDraw(name);
  });
  return select;
}

// The view switch and the toolbar (`index.html`'s `view-switch` and `personal-toolbar`).
function myWorkControls() {
  const switches = el('div', 'my-work-switch', ...MY_WORK_LAYOUTS.map((layout) => {
    const button = word('button', 'quiet', `my_work_${layout}`);
    button.type = 'button';
    button.setAttribute('aria-pressed', String(mw.layout === layout));
    button.dataset.myWork = `view:${layout}`;
    button.addEventListener('click', () => {
      mw.layout = layout;
      mw.agent = null;
      remember('oboete-my-work-layout', layout);
      myWorkDraw(`view:${layout}`);
    });
    return button;
  }));
  switches.setAttribute('role', 'group');
  switches.setAttribute('aria-label', t('my_work_view_group'));
  const search = el('input', 'my-work-search');
  search.type = 'search';
  search.autocomplete = 'off';
  search.value = mw.query;
  search.placeholder = t('my_work_search_placeholder');
  search.setAttribute('aria-label', t('my_work_search_label'));
  search.dataset.myWork = 'search';
  search.addEventListener('input', () => {
    mw.query = search.value;
    myWorkResults();
  });
  // Every project of every request, before any filter (`app.js` `render`).
  const projects = [...new Set((mw.rows || []).flatMap((row) => row.projects || []).filter(Boolean))].sort((a, b) => a.localeCompare(b));
  if (mw.project && !projects.includes(mw.project)) mw.project = '';
  const toolbar = el('div', 'my-work-toolbar', search,
    myWorkSelect('project', 'my_work_project_label', [['', t('my_work_all_projects')], ...projects.map((p) => [p, p])], mw.project, (v) => { mw.project = v; }),
    myWorkSelect('range', 'my_work_range_label', ['day', 'week', 'all'].map((r) => [r, t(`my_work_range_${r}`)]), mw.range, (v) => { mw.range = v; }),
    myWorkSelect('sort', 'my_work_sort_label', ['attention', 'latest', 'oldest', 'title'].map((s) => [s, t(`my_work_sort_${s}`)]), mw.sort, (v) => { mw.sort = v; }));
  toolbar.setAttribute('role', 'search');
  return [switches, toolbar];
}

// Where new data waits while a person reads (`app.js` `apply`): kept across redraws.
function myWorkSlot() {
  mw.slot = el('div', 'my-work-updates');
  // A polite live region, so that a screen reader says an update is held (CodeRabbit on #597).
  mw.slot.setAttribute('role', 'status');
  if (mw.pending) myWorkBanner();
  return mw.slot;
}

function myWorkBanner() {
  const apply = word('button', 'small', 'my_work_show_updates');
  apply.type = 'button';
  apply.addEventListener('click', () => {
    if (!mw.pending) return;
    myWorkAccept(mw.pending);
    myWorkDraw('current');
    setStatus(t('my_work_applied'), false, lang);
  });
  mw.slot.replaceChildren(el('div', 'my-work-banner',
    el('span', null, word('strong', null, 'my_work_updates_ready'), ' ', word('span', null, 'my_work_place_saved')), apply));
}

function myWorkDraw(focus = null) {
  const typing = myWorkTyping();
  if (mw.request !== null) {
    myWorkDrawRequest();
    mw.changed.clear();
    myWorkFocus(focus);
    return;
  }
  const [switches, toolbar] = myWorkControls();
  mw.results = el('div', 'my-work-results');
  myWorkResults();
  const page = myFocusPage();
  const intro = el('div', 'my-work-intro', word('p', 'my-work-eyebrow', 'my_work_eyebrow'), word('p', 'my-work-subtitle', 'my_work_subtitle'), myWorkLineSlot());
  // F7: Suggested and Past focus are pages of their own, in place of the requests (claude-mem's).
  // The held update's banner too, its Show updates the only way to take one in there (Codex and
  // CodeRabbit on #758).
  draw(word('span', null, 'my_work'), [], present(page ? [intro, myFocusChips(), myWorkSlot(), page === 'suggested' ? myFocusSuggested() : myFocusPast()] : [
    intro, myWorkBuildStory(), myFocusChips(), myFocusChangeBar(), myFocusSelected() ? myFocusHeader(myFocusSelected()) : null,
    switches, toolbar, mw.results, word('p', 'my-work-note', 'my_work_states_note'),
  ]));
  mw.changed.clear();
  setStatus('');
  // The search field the owner types in keeps the focus and its caret, whatever a redraw asked
  // for: a save that ends meanwhile draws the page again (#777).
  if (typing && myWorkFocus('search')) document.activeElement.setSelectionRange?.(...typing);
  else myWorkFocus(focus);
}

// The caret of the search field while the owner types in it, before a redraw replaces it.
function myWorkTyping() {
  const active = document.activeElement;
  if (active?.dataset?.myWork !== 'search' || !$('panel').contains(active)) return null;
  return [active.selectionStart ?? active.value.length, active.selectionEnd ?? active.value.length,
    active.selectionDirection ?? 'none'];
}

// The tabs, the requests and the notes below the toolbar, drawn again alone as a person types in
// the search field, which stays: an IME's text being composed in it is not cut off.
function myWorkResults() {
  const scoped = myWorkScoped();
  const rows = myWorkVisible(scoped);
  let trail = null;
  if (mw.agent) {
    const session = (mw.rows || []).flatMap((row) => row.sessions || []).find((s) => s.id === mw.agent);
    const back = word('button', 'quiet small my-work-back', 'my_work_all_agents');
    back.type = 'button';
    back.dataset.myWork = 'all-agents';
    back.addEventListener('click', () => {
      mw.agent = null;
      myWorkDraw('view:agents');
    });
    trail = el('div', 'my-work-trail', back, el('span', null, session ? myWorkAgent(session.agent) : t('my_work_contributor')));
  }
  const body = !rows.length
    ? (mw.query.trim() ? myWorkEmpty('my_work_no_match', 'my_work_no_match_body')
      // claude-mem's: only an area with no request at all; one the range hides is not empty.
      : myFocusSelected() && !(mw.rows || []).some((row) => row.focus === myFocusSelected()) ? myWorkEmpty('my_focus_area_empty', 'my_focus_area_empty_body')
        : myWorkEmpty('my_work_empty', 'my_work_empty_body', true))
    : mw.layout === 'list' ? myWorkTable(rows)
      : mw.layout === 'agents' && !mw.agent ? myWorkAgents(rows)
        : el('div', 'my-work-grid', ...rows.map(myWorkCard));
  const heading = mw.filter === 'all' && myFocusSelected() ? el('h3', 'my-work-heading', myFocusName(myFocusSelected()))
    : word('h3', 'my-work-heading', mw.filter === 'all' ? 'my_work_all_work' : `my_work_status_${mw.filter}`);
  heading.tabIndex = -1;
  const unlinked = mw.unlinked ? el('details', 'my-work-unlinked', word('summary', null, 'my_work_unlinked', { n: mw.unlinked }),
    el('div', null, word('p', null, 'my_work_unlinked_body'), myWorkTimeline('my_work_unlinked_link'))) : null;
  mw.results.replaceChildren(...present([myWorkTabs(scoped), myWorkSlot(), trail,
    el('section', 'my-work-section',
      el('div', 'sr-only', heading, word('span', null, rows.length === 1 ? 'my_work_count_one' : 'my_work_count', { n: rows.length })),
      body, unlinked)]));
}

// A redraw replaces the controls: the one in use takes the focus back (`app.js`'s handlers).
// Whether it found what to focus.
function myWorkFocus(focus) {
  if (!focus) return false;
  const panel = $('panel');
  if (focus === 'heading' || (focus === 'current' && mw.request !== null)) {
    $('heading').tabIndex = -1;
    $('heading').focus({ preventScroll: true });
    return true;
  }
  const nodes = [...panel.querySelectorAll('input, select, button, summary, h3')];
  const found = focus === 'current' || focus === 'agents' ? nodes.find((node) => node.className === 'my-work-heading')
    : nodes.find((node) => node.dataset.myWork === focus);
  if (!found) return false;
  found.focus({ preventScroll: true });
  return true;
}

function myWorkOpen(id) {
  // Its work product is read again: new records, expired ones and new rules change it without
  // changing the request's version (Codex on #650).
  mw.products = null;
  mw.unread.delete(id);
  mw.back = { id, scroll: window.scrollY || 0 };
  mw.request = id;
  mw.work = null;
  myWorkDraw('heading');
  window.scrollTo?.(0, 0);
}

// The button that opened what the page closes, by its `data-` key and value.
function myWorkOpenerOf(key, value) {
  return [...$('panel').querySelectorAll('button')].find((node) => node.dataset[key] === value);
}

function myWorkClose() {
  const back = mw.back;
  mw.request = null;
  mw.work = null;
  mw.back = null;
  myWorkDraw();
  const opener = back && myWorkOpenerOf('id', back.id);
  if (opener) opener.focus({ preventScroll: true });
  else myWorkFocus('current');
  window.scrollTo?.(0, back?.scroll || 0);
}

function myWorkFact(term, ...value) {
  return el('div', null, word('dt', null, term), el('dd', null, ...value));
}

// `statusFor` over a work stream's items, as the page names its statuses: needs review comes
// before blocked and `open` is in progress since claude-mem 15.1.
function myWorkStreamStatus(items, now = Date.now()) {
  const states = items.map((w) => {
    const s = String(w.status || '').trim().toLowerCase().replace(/[_ ]/g, '-');
    return MY_WORK_NEEDS.includes(s) ? 'needs-you' : s;
  });
  if (states.includes('needs-you')) return 'needs';
  if (states.some((s) => MY_WORK_REVIEW.has(s))) return 'ready';
  if (states.some((s) => ['blocked', 'failed'].includes(s))) return 'blocked';
  if (items.length && states.every((s) => ['done', 'completed'].includes(s))) return 'ready';
  if (items.length && states.every((s) => s === 'dropped')) return 'closed';
  if (now - Math.max(0, ...items.map((w) => w.updated || 0)) > 6 * 3600000) return 'quiet';
  if (states.some((s) => ['doing', 'in-progress', 'active', 'running', 'open'].includes(s))) return 'working';
  if (states.some((s) => ['todo', 'planned', 'queued'].includes(s))) return 'todo';
  return 'unknown';
}

// The request's work by repository and list, claude-mem's work streams (`listId`, a project's
// list): its newest change is its summary.
function myWorkStreams(row) {
  const lists = new Map();
  for (const item of row.work || []) {
    const id = item.stream ?? JSON.stringify([item.repo ?? null, item.list]);
    if (!lists.has(id)) lists.set(id, []);
    lists.get(id).push(item);
  }
  return [...lists].map(([id, items]) => {
    const newest = [...items].sort((a, b) => b.updated - a.updated);
    const latest = newest[0];
    return { id, list: latest.list, repo: latest.repo, items: newest, status: myWorkStreamStatus(items),
      summary: latest.detail || latest.task || latest.list || null };
  });
}

// `request-views.js` `requestPage` and `workstreamPage`, without the rows oboete does not
// record (decision 55).
function myWorkDrawRequest() {
  const row = (mw.rows || []).find((r) => r.id === mw.request);
  const all = row ? myWorkStreams(row) : [];
  const stream = mw.work !== null ? all.find((s) => s.id === mw.work) : null;
  // A list names its repository, as `projects` does, where the request's work is in two
  // (Greptile on #597).
  const several = new Set(all.map((s) => s.repo)).size > 1;
  const project = (s) => (several ? el('span', 'my-work-project', row.projects?.[row.repos?.indexOf(s.repo)] ?? s.repo ?? '') : null);
  // The list's button holds it too, so that a screen reader tells two of one name apart (#600).
  const back = word('button', 'quiet small my-work-back', stream ? 'my_work_back_request' : 'my_work_back');
  back.type = 'button';
  back.addEventListener('click', () => {
    if (!stream) return myWorkClose();
    mw.work = null;
    myWorkDraw();
    const opener = myWorkOpenerOf('stream', stream.id);
    if (opener) opener.focus({ preventScroll: true });
    else myWorkFocus('heading');
  });
  if (!row) {
    draw(word('span', null, 'my_work'), [], [back, myWorkSlot(), myWorkEmpty('my_work_missing', 'my_work_missing_body')]);
    return setStatus('');
  }
  if (stream) {
    const changes = stream.items.map((item) => {
      const title = item.task || item.list;
      return el('li', null, el('div', 'meta', localTime(item.updated, lang), word('span', null, 'my_work_work_update')),
        el('p', null, title), item.detail && item.detail !== title ? el('p', 'my-work-detail', item.detail) : null);
    });
    draw(stream.list || t('my_work_supporting'), [], [back, myWorkSlot(),
      el('div', 'my-work-request-meta', myWorkBadge(stream.status), project(stream)),
      el('p', 'my-work-intro-text', stream.summary || t('my_work_no_detail')),
      el('section', 'my-work-changes', word('h3', null, 'my_work_changes'),
        changes.length ? el('ul', 'my-work-change-list', ...changes) : word('p', 'text pending', 'my_work_no_changes')),
    ]);
    return setStatus('');
  }
  const next = myWorkNextLine(row);
  const elapsed = el('span', 'my-work-elapsed', myWorkElapsed(row));
  elapsed.title = t('my_work_elapsed_title');
  elapsed.dataset.started = String(row.started);
  const streams = all.map((s) => {
    const name = project(s);
    const open = el('button', 'my-work-open', s.list || t('my_work_supporting'), name ? ' ' : null, name);
    open.type = 'button';
    open.dataset.stream = s.id;
    open.addEventListener('click', () => {
      mw.work = s.id;
      myWorkDraw('heading');
    });
    return el('article', 'my-work-stream', el('div', 'my-work-stream-text', el('h4', 'my-work-title', open),
      el('p', null, s.summary || t('my_work_no_detail'))), myWorkBadge(s.status));
  });
  // The IDs `get` takes, folded with their count: each opens its full text.
  const ids = (key, list) => (list?.length ? el('details', 'my-work-recorded', el('summary', null, `${t(key)} (${list.length})`),
    el('ul', 'my-work-ids', ...list.map((id) => el('li', null, fullText(id, id))))) : null);
  draw(row.title, [], [back, myWorkSlot(),
    el('div', 'my-work-request-meta', myWorkBadge(row.status), el('span', 'my-work-project', myWorkProjects(row)), myWorkTime(row.updated)),
    myWorkSuggestion(row),
    myWorkProgress(row),
    el('dl', 'my-work-facts', myWorkFact('my_work_contributors', myWorkContributors(row)),
      myWorkFact('my_work_started', row.started > 0 ? localTime(row.started, lang) : t('my_work_not_recorded')),
      myWorkFact('my_work_elapsed', elapsed),
      myFocusInUse() ? myWorkFact('my_focus_area', el('span', 'my-focus-fact', myFocusChip(row), myFocusMove(row))) : null),
    myFocusChangeBar(),
    el('section', 'my-work-stands', word('h3', null, 'my_work_stands'), el('p', null, myWorkActivity(row)),
      row.updating ? word('small', 'my-work-copy-note', 'my_work_copy_updating') : null,
      el('p', 'my-work-request-next', el('strong', null, `${t(next?.label || 'my_work_next_step')}:`), ' ', next?.text || myWorkNext(row)),
      el('details', 'my-work-original', word('summary', null, 'my_work_original'),
        el('div', null, el('p', 'text', row.ask?.text ?? ''), row.ask?.id ? fullText(row.ask.id, word('span', null, 'my_work_read_original')) : null))),
    myWorkStory(row),
    el('section', 'my-work-streams', word('h3', null, 'my_work_streams'),
      streams.length ? el('div', 'my-work-stream-list', ...streams) : word('p', 'text pending', 'my_work_no_streams')),
    myWorkProducts(row),
    row.follow_ups?.length ? el('details', 'my-work-original', word('summary', null, 'my_work_follow_ups'),
      el('div', null, ...row.follow_ups.flatMap((f) => [el('p', 'text', f.text ?? ''), fullText(f.id)]))) : null,
    ids('my_work_result', row.summaries), ids('my_work_cards_heading', row.cards),
    row.status === 'ready' ? myWorkReviewSection(row) : null,
    ...myWorkHandoff(row),
  ]);
  setStatus('');
}

// Slice 4 (docs/my-work.md): what the request produced, as claude-mem 15.1's drawer shows it,
// read once its page opens and again when the request changes. Nothing shows while it reads, nor
// when there is nothing, as no empty field is shown.
function myWorkProducts(row) {
  const box = el('section', 'my-work-products');
  box.hidden = true;
  const fill = (list) => {
    if (!list.length) return;
    box.replaceChildren(word('h3', null, 'my_work_products'),
      el('ul', 'my-work-product-list', ...list.map(myWorkProduct)));
    box.hidden = false;
  };
  const key = `${row.id}\n${row.version}`;
  if (mw.products?.key === key) {
    fill(mw.products.list);
  } else {
    const ask = ++mw.productsAsk;
    api('my-work/product', { request: row.id }).then((answer) => {
      if (ask !== mw.productsAsk) return;
      mw.products = { key, list: answer.products || [] };
      if (mw.request === row.id) fill(mw.products.list);
    }).catch(() => {});
  }
  return box;
}

function myWorkProduct(p) {
  const label = `my_work_kind_${String(p.kind).replace(/-/g, '_')}`;
  const kind = Object.hasOwn(TEXT, label) ? word('span', 'my-work-product-kind', label) : el('span', 'my-work-product-kind', p.kind);
  let title = el('span', null, p.title);
  // The answer holds only http and https links; the page checks again before it makes one.
  if (/^https?:\/\//i.test(p.url || '')) {
    title = el('a', null, p.title);
    title.href = p.url;
    title.target = '_blank';
    title.rel = 'noopener noreferrer';
  }
  return el('li', null, kind, ' ', title, p.path ? el('code', 'my-work-product-path', p.path) : null,
    p.at ? el('span', 'meta', localTime(p.at, lang)) : null);
}

// claude-mem's review dialog (`openReview`), in the page: this viewer opens a request within the
// page, and has no dialog (decision 55). Its evidence is the recorded updates, each once, at most
// 8, as `presentation.js` takes the work's outcomes and updates.
function myWorkReviewSection(row) {
  const evidence = [...new Set((row.updates || []).map((u) => u.text))].slice(0, 8);
  return el('section', 'my-work-review',
    word('p', 'my-work-eyebrow', 'my_work_review_eyebrow'),
    el('p', null, myWorkActivity(row)),
    word('h3', null, 'my_work_evidence'),
    el('ul', null, ...(evidence.length ? evidence.map((text) => el('li', null, text)) : [word('li', null, 'my_work_no_evidence')])),
    word('p', 'my-work-review-note', 'my_work_review_note'),
    myWorkReviewButton(row, true));
}

function myWorkReviewButton(row, checked) {
  const button = word('button', checked ? 'my-work-check' : 'small my-work-undo', checked ? 'my_work_mark_checked' : 'my_work_undo_check');
  button.type = 'button';
  button.disabled = mw.saving;
  button.addEventListener('click', () => myWorkReview(row, checked));
  return button;
}

// `app.js` `saveReview`: the check of the version the page read, or its Undo; then My Work read
// again, in the reads' order. A version that changed meanwhile is refused, and read again.
async function myWorkReview(row, checked) {
  if (mw.saving) return;
  mw.saving = true;
  for (const button of $('panel').querySelectorAll('.my-work-check, .my-work-undo')) button.disabled = true;
  let said = 'my_work_review_failed';
  try {
    const { res } = await memoryWrite('my-work/review', { request: row.id, version: row.version, checked });
    if (res.status === 409) said = 'my_work_review_stale';
    else if (res.ok) said = checked ? 'my_work_checked_saved' : 'my_work_check_undone';
    // A save the server took stays said as taken when the read after it fails (#602).
    if (res.ok || res.status === 409) {
      await myWorkReread().catch(() => {
        if (res.ok) said = 'my_work_saved_unread';
      });
    }
  } catch {
    // The page says it was not saved; nothing changed.
  } finally {
    mw.saving = false;
  }
  if (view !== 'my_work') return;
  // Drawn first, as drawing clears the status line.
  myWorkDraw('current');
  setStatus(t(said), said === 'my_work_review_failed', lang);
}

// A read the page asked for itself, applied though a person reads, unless a newer one was taken.
async function myWorkReread() {
  const mine = generation;
  const read = ++mw.reads;
  const answer = await api('my-work', { range: 'all' });
  if (mine !== generation || view !== 'my_work' || read < mw.taken) return;
  mw.taken = read;
  myWorkAccept(answer);
}

// `presentation.js` `handoffText`, in the page's language (decision 55); a state the page names
// is named here too, where claude-mem says "Recorded update" for all but four.
function myWorkHandoffText(row) {
  const named = row.status !== 'unknown' && Object.hasOwn(TEXT, `my_work_status_${row.status}`);
  const state = { ready: 'my_work_handoff_ready', checked: 'my_work_handoff_checked' }[row.status]
    || (named ? `my_work_status_${row.status}` : 'my_work_handoff_recorded');
  const updates = (row.updates || []).slice(0, 4).map((u) => `- ${u.text}`);
  return [
    t('my_work_handoff_request', { text: row.ask?.text || row.title }),
    t('my_work_handoff_project', { text: myWorkProjects(row) }),
    t('my_work_handoff_state', { text: t(state) }),
    t('my_work_handoff_next', { text: myWorkNext(row) }),
    '', t('my_work_handoff_updates'),
    ...(updates.length ? updates : [`- ${t('my_work_handoff_no_updates')}`]),
    '', t('my_work_handoff_end'),
  ].join('\n');
}

// "Copy handoff" (`app.js`): to the clipboard, else selected in a field to copy. Nothing is sent.
function myWorkHandoff(row) {
  const copy = word('button', 'small my-work-copy', 'my_work_copy_handoff');
  copy.type = 'button';
  const note = el('p', 'my-work-handoff-note');
  note.setAttribute('role', 'status');
  const area = el('textarea', 'my-work-handoff-text');
  area.readOnly = true;
  area.rows = 9;
  area.hidden = true;
  area.setAttribute('aria-label', t('my_work_handoff_label'));
  copy.addEventListener('click', async () => {
    const text = myWorkHandoffText(row);
    try {
      if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable');
      await navigator.clipboard.writeText(text);
      note.replaceChildren(t('my_work_copied'));
      setStatus(t('my_work_copied_announce'), false, lang);
    } catch {
      area.hidden = false;
      area.value = text;
      area.focus();
      area.select();
      note.replaceChildren(t('my_work_clipboard'));
    }
  });
  const checked = row.status === 'checked'
    ? [myWorkReviewButton(row, false),
      // Text, which the label's variables keep through a switch of language (#602).
      word('span', 'my-work-checked-at', 'my_work_checked_at', { time: new Date(row.checked).toLocaleString(lang) })]
    : [];
  return [el('div', 'my-work-actions', ...checked, copy, myWorkTimeline()), note, area];
}

// claude-mem's `contentSignature` and `contentFingerprint`: what a card shows, and what was
// recorded, without the summary a model writes.
const myWorkContent = (row) => JSON.stringify([row.ask, row.follow_ups, row.work, row.updates, row.cards]);
const myWorkSignature = (row) => JSON.stringify([myWorkContent(row), row.title, row.status, myWorkActivity(row), row.next]);

// What tells two answers of My Work apart: a change of any is drawn, or held while a person reads.
const myWorkAnswerSignature = (answer) => JSON.stringify([answer.requests, answer.unlinked, answer.story, answer.areas, answer.other_asks]);

function myWorkAccept(answer) {
  mw.signature = myWorkAnswerSignature(answer);
  // claude-mem 15's `normalizeRequestStatus`: a checked result ready to check is Checked on the
  // page; a shipped one stays Shipped.
  for (const row of answer.requests) if (row.checked && row.status === 'ready') row.status = 'checked';
  // `app.js` `apply`: a request new or changed since the last answer flashes, and one whose record
  // changed is marked new until it is opened; the first answer marks none, an empty one is a
  // first answer too (#600).
  if (mw.rows) {
    const shown = new Map(mw.rows.map((row) => [row.id, myWorkSignature(row)]));
    const content = new Map(mw.rows.map((row) => [row.id, myWorkContent(row)]));
    for (const row of answer.requests) {
      if (shown.get(row.id) === myWorkSignature(row)) continue;
      mw.changed.add(row.id);
      if (content.get(row.id) !== myWorkContent(row)) mw.unread.add(row.id);
    }
  }
  mw.rows = answer.requests;
  mw.unlinked = answer.unlinked;
  mw.story = answer.story ?? null;
  mw.areas = answer.areas ?? [];
  mw.otherAsks = answer.other_asks ?? [];
  mw.pageWords = answer.page_words ?? [];
  mw.fetched = Date.now();
  mw.pending = null;
}

// `app.js` `isReading`: a request open, the page scrolled, or the focus on My Work's controls.
function myWorkReading() {
  if (mw.request !== null || (window.scrollY || 0) > 220) return true;
  const active = document.activeElement;
  // The build story's disclosure too, at the page's top (#733).
  return ['.my-work-toolbar', '.my-work-tabs', '.my-work-switch', '.my-work-grid', '.my-work-table', '.my-work-agents', '.my-work-build-story',
    '.my-focus-chips', '.my-focus-change', '.my-focus-header', '.my-focus-page']
    .some((selector) => active?.closest?.(selector));
}

// A poll's read (`refresh`): unchanged, nothing; changed while a person reads, held behind
// "Show updates", which never applies itself when the reading ends (`app.js` `apply`). A read a
// newer drawing overtook (`show`) draws nothing.
async function myWorkLive() {
  const mine = generation;
  const read = ++mw.reads;
  const answer = await api('my-work', { range: 'all' });
  // Nor one older than a read the page took: the read after a check begins while a poll's may be
  // out (`myWorkReread`).
  if (mine !== generation || view !== 'my_work' || read < mw.taken) return true;
  void myWorkLineRead();
  // Taken, so that the tab's older read lands on nothing (`showMyWork`; Codex and Greptile on #597).
  mw.taken = read;
  mw.fetched = Date.now();
  const signature = myWorkAnswerSignature(answer);
  if (signature === mw.signature) return true;
  if (myWorkReading()) {
    // The last answer received, as claude-mem's `accept` keeps it: the same answer again stays
    // held, and one like the shown one replaces it.
    mw.signature = signature;
    mw.pending = answer;
    if (mw.slot) myWorkBanner();
    return true;
  }
  myWorkAccept(answer);
  myWorkDraw();
  return true;
}

async function showMyWork() {
  const read = ++mw.reads;
  const answer = await api('my-work', { range: 'all' });
  return () => {
    void myWorkLineRead();
    // A poll's read that began after this one and came first keeps its answer; the page is drawn.
    if (read > mw.taken) {
      mw.taken = read;
      myWorkAccept(answer);
    }
    myWorkDraw();
  };
}

// V1 (docs/my-work-model.md): My Work's status line has a place of its own, filled by a read of
// its own after the requests': it is no part of the held updates, and a line that does not read
// leaves the requests as they are.
function myWorkLineSlot() {
  mw.line = el('div', 'my-work-line', myWorkStatusLine(mw.status));
  return mw.line;
}

async function myWorkLineRead() {
  // Only the latest read draws: an older one that lands later is dropped (#722).
  const read = ++mw.lineReads;
  let status = null;
  try {
    status = await api('my-work/status');
  } catch {
    return;
  }
  if (read !== mw.lineReads || view !== 'my_work' || JSON.stringify(status) === JSON.stringify(mw.status)) return;
  mw.status = status;
  mw.line?.replaceChildren(...present([myWorkStatusLine(status)]));
}

// V1: the status line of My Work and My Goals, as the reads say it, with no call: off or waiting
// for summaries with a way to Settings, waiting and why, or when they last wrote.
function myWorkStatusLine(status) {
  if (!status?.state) return null;
  let parts;
  if (status.state === 'off' || status.state === 'summaries_off') {
    const open = word('button', 'small', 'my_work_line_settings');
    open.type = 'button';
    open.dataset.focus = 'settings';
    open.addEventListener('click', () => {
      setView('settings');
      void show();
    });
    parts = [word('span', null, `my_work_line_${status.state}`), ' ', open];
  } else if (status.state === 'waiting') {
    const why = ['bound', 'budget', 'resting', 'owner'].includes(status.why) ? status.why : 'resting';
    parts = [word('span', null, `my_work_line_waiting_${why}`, { n: status.calls_a_day ?? '' })];
    if (why !== 'owner' && Number.isFinite(status.until)) parts.push(' ', word('span', null, 'my_work_line_until'), ' ', myGoalsAt(status.until));
  } else if (status.state === 'written') {
    parts = [word('span', null, 'my_work_line_written'), ' ', myWorkTime(status.at)];
  } else {
    parts = [word('span', null, 'my_work_line_none')];
  }
  const node = el('p', 'my-work-status-line', ...parts);
  node.setAttribute('aria-live', 'polite');
  return node;
}

// --- The goals' parts My Focus shows (docs/my-work-model.md V2, F7), from My Goals' page, which
// claude-mem 15.4 and slice 10c-3b retired ---

// claude-mem's `presentStatus` (status-normalization.ts): the state a request's status is on its
// area's header, from which the area's status word is read.
function myGoalsState(row, now = Date.now()) {
  switch (row.status) {
    case 'needs': case 'blocked': case 'checked': return 'your-move';
    case 'ready': return 'needs-check';
    case 'shipped': return 'shipped';
    case 'closed': case 'quiet': return 'idle';
    case 'working': case 'todo': return 'working';
    default: return now - (row.updated || 0) <= 6 * 3600000 ? 'working' : 'idle';
  }
}

function myGoalsIsoDay(ms) {
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

// The last ship among `requests`, its recorded day, when the goal was asked again on a later
// day, by full dates (V2: claude-mem compares the month and day alone), both in UTC, as the ship
// day is recorded (#731).
function myGoalsAskedAgain(goal, requests) {
  const shipped = requests.map((row) => row.progress?.completed).filter(Boolean).sort().at(-1);
  return shipped && new Date(goal.last).toISOString().slice(0, 10) > shipped ? shipped : null;
}

// claude-mem's `statusOf`: where a goal stands, from the requests serving it.
function myGoalsStatus(goal, requests) {
  if (!requests.length) return 'nothing';
  const states = requests.map((row) => myGoalsState(row));
  if (states.includes('your-move')) return 'your_move';
  if (states.includes('working') || states.includes('needs-check')) return 'moving';
  if (states.every((s) => s === 'shipped')) return myGoalsAskedAgain(goal, requests) ? 'asked_again' : 'shipped';
  return 'stalled';
}

function myGoalsStatusWord(status) {
  return word('span', `my-goals-status my-goals-status-${status}`, `my_goals_status_${status}`);
}

// claude-mem's `Rollup`: the average percent of the reviewed requests serving a goal, and what it
// leaves out.
function myGoalsRollup(requests) {
  if (!requests.length) return word('p', 'my-goals-rollup my-goals-note', 'my_goals_rollup_none');
  const reviewed = requests.filter((row) => row.progress?.reviewed != null);
  const unreviewed = requests.filter((row) => row.progress?.reviewed == null);
  const p = reviewed.length ? Math.round(reviewed.reduce((sum, row) => sum + (row.progress.percent ?? 0), 0) / reviewed.length) : 0;
  const one = requests.length === 1 ? '_one' : '';
  const vars = { p, k: reviewed.length, n: requests.length };
  const meter = el('meter', 'my-work-meter');
  Object.assign(meter, { min: 0, max: 100, value: p });
  meter.setAttribute('aria-label', t('my_goals_rollup_label', vars));
  return el('div', 'my-goals-rollup', meter,
    el('p', 'my-goals-note', t(reviewed.length ? `my_goals_rollup${one}` : `my_goals_rollup_unreviewed${one}`, vars)),
    unreviewed.length && reviewed.length ? el('p', 'my-goals-note', t('my_goals_not_counted', { titles: unreviewed.map((row) => row.title).join(', ') })) : null);
}

// A quote's time, and when a wait ends: to the minute, in the page's language.
function myGoalsAt(ms) {
  const at = new Date(ms);
  const node = el('time', 'my-goals-at', at.toLocaleString(lang, { year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }));
  node.dateTime = at.toISOString();
  return node;
}

function myGoalsDay(ms) {
  return new Date(ms).toLocaleDateString(lang, { month: 'short', day: 'numeric' });
}

// "2026-10", as the page names a month.
function myGoalsMonthName(month) {
  const [y, m] = String(month ?? '').split('-').map(Number);
  return y && m ? new Date(y, m - 1, 1).toLocaleDateString(lang, { year: 'numeric', month: 'long' }) : String(month ?? '');
}

function myGoalsAsked(n) {
  return n === 1 ? t('my_goals_asked_once') : t('my_goals_asked_n', { n });
}

// A goal's or a month's line of facts, its product last when it has one.
function myGoalsMeta(facts, product) {
  return el('span', 'my-goals-meta', [...facts, ...(product ? [product] : [])].join(' · '));
}

// --- My Rules (docs/my-work-model.md V3): the rules learned from your requests, and yours ---

// What a redraw keeps: the answer drawn, the suggestion or rule being edited and its draft, the
// rule being added, and the last approve or reject, which "Undo" takes back.
// `applies` is where each form's rule applies, the edited approve's (`edit`) and the added rule's
// (`add`) apart (#743): '' for every repository, else a repository's key.
const mr = { answer: null, signature: null, editing: null, draft: '', applies: { edit: '', add: '' }, adding: null, last: null, busy: false, open: new Set() };

async function showMyRules() {
  const answer = await api('my-rules');
  return () => {
    const signature = JSON.stringify(answer);
    if (signature === mr.signature && $('panel').querySelector('.my-rules')) return setStatus('');
    mr.signature = signature;
    mr.answer = answer;
    myRulesDraw();
  };
}

// claude-mem's strength bands: Strong at 4 and over, Steady at 1.5, else Light.
const myRulesBand = (strength) => (strength >= 4 ? 3 : strength >= 1.5 ? 2 : 1);

function myRulesButton(key, focus, onClick, cls = 'quiet small') {
  const button = word('button', cls, key);
  button.type = 'button';
  button.dataset.focus = focus;
  button.disabled = mr.busy;
  button.addEventListener('click', onClick);
  return button;
}

// One write of the page, then a read again: a refusal's words, else the receipt's.
async function myRulesWrite(path, body, done) {
  if (mr.busy) return;
  mr.busy = true;
  myRulesDraw();
  let message = null;
  let failed = false;
  try {
    const { res, answer } = await memoryWrite(path, body);
    if (!res.ok) {
      failed = true;
      message = memoryFailure(res, answer);
      return false;
    }
    // Only what was applied is done (#739): a partial record says what it kept, and closes nothing.
    const applied = answer.state === 'applied';
    message = applied ? t(done) : t(Object.hasOwn(TEXT, answer.code ?? '') ? answer.code : 'my_rules_pending');
    failed = !applied;
    return applied;
  } catch {
    failed = true;
    message = t('memory_result_unknown');
    return false;
  } finally {
    mr.busy = false;
    mr.signature = null;
    // Another tab opened meanwhile is not read again, nor given this page's message (#741): on
    // Settings that would replace a form not saved yet.
    if (view === 'my_rules') await show();
    // Held through the redraw that follows a done action (Undo's line, a closed editor).
    if (message && view === 'my_rules') setStatus(message, failed, lang, 8000);
  }
}

function myRulesWhy(item) {
  const n = item.quotes?.length ?? 0;
  if (!n) return null;
  const summary = el('summary', null, t(n === 1 ? 'my_rules_why_one' : 'my_rules_why_n', { n }));
  const key = `why:${item.id ?? item.uid}`;
  summary.dataset.focus = key;
  const why = el('details', 'my-goals-why', summary,
    el('ul', 'my-goals-quotes', ...item.quotes.map((q) => el('li', null, el('q', null, q.text), ' ', q.at ? myGoalsAt(q.at) : null))));
  // Kept open through the redraws an action brings, as My Goals keeps its own.
  why.open = mr.open.has(key);
  why.addEventListener('toggle', () => (why.open ? mr.open.add(key) : mr.open.delete(key)));
  return why;
}

function myRulesEditor(focus, label) {
  const field = input('textarea', mr.draft, '', focus, (v) => { mr.draft = v; });
  field.dataset.focus = focus;
  field.disabled = mr.busy;
  return el('label', 'field my-rules-editor', el('span', null, t(label)), field);
}

// Slice 8b: where `form`'s rule applies, every repository or one the sessions started in (as
// claude-mem's "Applies"). A repository chosen stays offered when a reread leaves it out (#743).
function myRulesApplies(form, focus) {
  const offered = mr.answer?.repos ?? [];
  const chosen = mr.applies[form];
  // Kept by its key, shown by no text of it: a rule added since may mask the name (#745).
  const repos = chosen && !offered.some((r) => r.repo === chosen) ? [...offered, { repo: chosen, name: null }] : offered;
  const choice = el('select', null, el('option', null, t('my_rules_everywhere')),
    ...repos.map((r) => {
      const option = el('option', null, r.name === null ? t('my_rules_chosen_repo') : t('my_rules_in_repo', { name: r.name }));
      option.value = r.repo;
      if (r.name !== null) option.title = r.repo;
      return option;
    }));
  choice.options[0].value = '';
  choice.value = chosen;
  choice.dataset.focus = focus;
  choice.disabled = mr.busy;
  choice.addEventListener('change', () => { mr.applies[form] = choice.value; });
  return el('label', 'field my-rules-editor', el('span', null, t('my_rules_applies')), choice);
}

// The scope fields of a post: `/api/my-rules`' approve and `/api/preferences`' add.
const myRulesScope = () => (mr.applies.edit ? { scope: 'repo', repo: mr.applies.edit } : { scope: 'global' });
const myRulesWhere = () => (mr.applies.add ? { apply_to_all_repos: false, repo: mr.applies.add } : { apply_to_all_repos: true });

// A suggestion waiting: approve it as it is, edit then approve it, or reject it (R3).
function myRulesSuggestion(sg) {
  const meta = [t(sg.seen === 1 ? 'my_rules_seen_one' : 'my_rules_seen_n', { n: sg.seen }), t(`my_rules_strength_${myRulesBand(sg.strength)}`)];
  // As it is, every repository's; edited, where the editor says.
  const approve = (statement) => myRulesWrite('my-rules', { suggestion: sg.id, action: 'approve', ...(statement === undefined ? { scope: 'global' } : { statement, ...myRulesScope() }) }, 'my_rules_approved')
    .then((ok) => { if (ok) { mr.editing = null; mr.last = { id: sg.id, action: 'approve' }; myRulesDraw(); } });
  let actions;
  if (mr.editing === `suggestion:${sg.id}`) {
    actions = [myRulesEditor(`draft:${sg.id}`, 'my_rules_text'), myRulesApplies('edit', `applies:${sg.id}`), el('div', 'my-rules-actions',
      myRulesButton('my_rules_save_approve', `save:${sg.id}`, () => void approve(mr.draft), 'small'),
      myRulesButton('my_rules_cancel', `cancel:${sg.id}`, () => { mr.editing = null; myRulesDraw(); }))];
  } else {
    actions = [el('div', 'my-rules-actions',
      myRulesButton('my_rules_approve', `approve:${sg.id}`, () => void approve(undefined), 'small'),
      myRulesButton('my_rules_edit_approve', `edit:${sg.id}`, () => { mr.editing = `suggestion:${sg.id}`; mr.draft = sg.statement; mr.applies.edit = ''; myRulesDraw(); }),
      myRulesButton('my_rules_reject', `reject:${sg.id}`, () => void myRulesWrite('my-rules', { suggestion: sg.id, action: 'reject' }, 'my_rules_rejected_now')
        .then((ok) => { if (ok) { mr.last = { id: sg.id, action: 'reject' }; myRulesDraw(); } })))];
  }
  return el('li', 'my-goals-item my-rules-item', el('p', 'my-rules-statement', sg.statement),
    el('p', 'my-goals-meta', meta.join(' · ')), myRulesWhy(sg), ...actions);
}

// One of your rules: edit it, turn it off or on, and for one approved from a suggestion, undo the
// approval (R3).
function myRulesRule(rule) {
  const key = `rule:${rule.uid}`;
  const meta = [t(rule.suggestion ? 'my_rules_approved_by_you' : 'my_rules_set_by_you')];
  if (!rule.on) meta.push(t('my_rules_is_off'));
  if (rule.seen) meta.push(t(rule.seen === 1 ? 'my_rules_seen_one' : 'my_rules_seen_n', { n: rule.seen }));
  let body;
  if (mr.editing === key) {
    body = [myRulesEditor(`draft:${rule.uid}`, 'my_rules_text'), el('div', 'my-rules-actions',
      myRulesButton('my_rules_save', `save:${rule.uid}`, () => void myRulesWrite('claims/correct', { uid: rule.uid, body: mr.draft }, 'my_rules_saved')
        .then((ok) => { if (ok) { mr.editing = null; myRulesDraw(); } }), 'small'),
      myRulesButton('my_rules_cancel', `cancel:${rule.uid}`, () => { mr.editing = null; myRulesDraw(); }))];
  } else {
    const toggle = myRulesButton(rule.on ? 'my_rules_off' : 'my_rules_on', `toggle:${rule.uid}`,
      () => void myRulesWrite('claims/mute', { uid: rule.uid, muted: rule.on }, 'my_rules_saved'));
    toggle.setAttribute('aria-pressed', String(rule.on));
    body = [el('div', 'my-rules-actions', toggle,
      myRulesButton('my_rules_edit', `edit:${rule.uid}`, () => { mr.editing = key; mr.draft = rule.text; myRulesDraw(); }),
      rule.suggestion ? myRulesButton('my_rules_undo_approve', `undo:${rule.suggestion}`,
        () => void myRulesWrite('my-rules', { suggestion: rule.suggestion, action: 'undo' }, 'my_rules_undone')) : null)];
  }
  const why = rule.quotes?.length ? myRulesWhy({ ...rule, id: rule.uid }) : el('p', 'my-goals-note', t('my_rules_no_quotes'));
  return el('li', rule.on ? 'my-goals-item my-rules-item' : 'my-goals-item my-rules-item my-rules-off',
    el('p', 'my-rules-statement', rule.text), el('p', 'my-goals-meta', meta.join(' · ')), why, ...body);
}

// Add rule: a preference of every repository, as the settings page's preference adds one, or of
// one repository.
function myRulesAdd() {
  if (mr.adding === null) {
    return el('div', 'my-rules-actions', myRulesButton('my_rules_add', 'add', () => { mr.adding = ''; mr.applies.add = ''; myRulesDraw(); }));
  }
  const field = input('textarea', mr.adding, '', 'add:text', (v) => { mr.adding = v; });
  field.dataset.focus = 'add:text';
  field.disabled = mr.busy;
  return el('div', 'my-rules-add', word('h4', null, 'my_rules_add_h'),
    el('label', 'field my-rules-editor', el('span', null, t('my_rules_text')), field), myRulesApplies('add', 'add:applies'),
    el('div', 'my-rules-actions',
      myRulesButton('my_rules_save', 'add:save', () => void myRulesWrite('preferences', { text: mr.adding, ...myRulesWhere() }, 'my_rules_saved')
        .then((ok) => { if (ok) { mr.adding = null; myRulesDraw(); } }), 'small'),
      myRulesButton('my_rules_cancel', 'add:cancel', () => { mr.adding = null; myRulesDraw(); })));
}

// After an approve or a reject on this page: what it did, with Undo (an op, so the rule's own
// Undo and the Rejected list undo it after a reload too).
function myRulesLast() {
  if (!mr.last) return null;
  const undo = myRulesButton('my_rules_undo', 'last:undo', () => {
    const { id } = mr.last;
    void myRulesWrite('my-rules', { suggestion: id, action: 'undo' }, 'my_rules_undone').then((ok) => { if (ok) { mr.last = null; myRulesDraw(); } });
  });
  const node = el('p', 'my-rules-last', word('span', null, mr.last.action === 'approve' ? 'my_rules_approved' : 'my_rules_rejected_now'), ' ', undo);
  node.setAttribute('aria-live', 'polite');
  return node;
}

function myRulesDraw() {
  // A write that ends after the owner opened another tab draws nothing over it (#739).
  if (view !== 'my_rules') return;
  const focus = $('panel').contains(document.activeElement) ? document.activeElement.dataset?.focus : undefined;
  const a = mr.answer ?? {};
  const waiting = a.waiting ?? [];
  const rejected = a.rejected ?? [];
  const removed = a.removed ?? [];
  const scopes = a.yours ?? [];
  const rules = scopes.flatMap((scope) => scope.rules ?? []);
  // A suggestion decided since, or a rule gone, is no longer edited.
  if (mr.editing && ![...waiting.map((sg) => `suggestion:${sg.id}`), ...rules.map((r) => `rule:${r.uid}`)].includes(mr.editing)) mr.editing = null;
  const decide = [word('h3', null, 'my_rules_decide'), word('p', 'my-goals-note', 'my_rules_decide_note'),
    waiting.length ? el('ul', 'my-goals-list', ...waiting.map(myRulesSuggestion)) : word('p', 'my-goals-note', 'my_rules_nothing_waiting')];
  // Each scope with rules under its own heading, every repository's first (slice 8b).
  const byScope = scopes.filter((scope) => scope.rules?.length).flatMap((scope) => [
    el('h4', 'my-rules-scope', scope.scope === 'repo' ? t('my_rules_in_repo', { name: scope.name }) : t('my_rules_everywhere')),
    el('ul', 'my-goals-list', ...scope.rules.map(myRulesRule))]);
  const yours = [word('h3', null, 'my_rules_yours'),
    ...(byScope.length ? byScope : [word('p', 'my-goals-note', 'my_rules_none')]), myRulesAdd()];
  const sections = [el('section', 'my-rules-decide', ...decide), el('section', 'my-rules-yours', ...yours)];
  const undoable = (sg) => el('li', 'my-goals-item my-rules-item', el('p', 'my-rules-statement', sg.statement),
    el('div', 'my-rules-actions', myRulesButton('my_rules_undo', `undo:${sg.id}`,
      () => void myRulesWrite('my-rules', { suggestion: sg.id, action: 'undo' }, 'my_rules_undone'))));
  if (removed.length) {
    sections.push(el('section', 'my-rules-removed', word('h3', null, 'my_rules_removed'), word('p', 'my-goals-note', 'my_rules_removed_note'),
      el('ul', 'my-goals-list', ...removed.map(undoable))));
  }
  if (rejected.length) {
    sections.push(el('section', 'my-rules-rejected', word('h3', null, 'my_rules_rejected'), word('p', 'my-goals-note', 'my_rules_rejected_note'),
      el('ul', 'my-goals-list', ...rejected.map(undoable))));
  }
  draw(word('span', null, 'my_rules'), [], [
    el('div', 'my-work-intro', word('p', 'my-work-eyebrow', 'my_rules_eyebrow'), myWorkStatusLine(a.status)),
    word('p', 'my-goals-note', 'my_rules_where'), myRulesLast(),
    el('div', 'my-goals my-rules', ...sections),
  ]);
  if (focus) [...$('panel').querySelectorAll('button, summary, textarea, select')].find((node) => node.dataset.focus === focus)?.focus({ preventScroll: true });
  setStatus('');
}

const LOADERS = new Map([
  ['my_work', showMyWork], ['my_rules', showMyRules], ['records', showRecords], ['context', showContext], ['stats', showStats], ['settings', showSettings],
]);

// Only the latest request may draw: an earlier, slower one must not overwrite it.
let generation = 0;
let drawnWithoutBaseline = false;

async function show() {
  // First, so a settings render still out cannot draw over the tab opened after it.
  const mine = ++generation;
  feed?.observer?.disconnect();
  feed = null;
  sessionsList?.observer?.disconnect();
  sessionsList = null;
  // The settings tab needs no repository: it names the config.toml mistake that fails their list (#325).
  if (!reposLoaded && view !== 'settings') return refresh();
  const repo = $('repo').value;
  const q = $('q').value.trim();
  if (view === 'timeline' || view === 'sessions') {
    const shown = await (view === 'timeline' ? showFeed(repo, mine) : showSessions(repo, mine));
    if (shown && version === null) drawnWithoutBaseline = true;
    return shown;
  }
  setStatus(t('loading'));
  try {
    const search = q && !['settings', 'my_work', 'my_rules'].includes(view);
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

async function refresh(live, selectedRepo) {
  let listed = true;
  try {
    await loadRepos();
    // An explicit scope is applied after the initial list chooses its default checkout.
    if(selectedRepo!==undefined)$('repo').value=selectedRepo;
  } catch (e) {
    if (view !== 'settings') {
      showError(e, selectedRepo===undefined?refresh:()=>refresh(live,selectedRepo));
      return false;
    }
    listed = false;
  }
  // Preserve unsaved settings, including when a poll began before the Settings tab opened. The
  // note of a config.toml mistake stays while the list still fails, and goes once it loads.
  if (view === 'settings' && $('panel').querySelector('.settings') && (form || !listed)) return true;
  if (live && activeFeed(feed) && feed.session) ses.stale = true;
  if (live && activeFeed(feed) && feed.repo === $('repo').value) return readFeed(feed, 'live');
  if (live && activeSessions(sessionsList) && sessionsList.repo === $('repo').value) return readSessions(sessionsList, 'live');
  // A poll never redraws My Work under a reader (`myWorkLive`).
  if (live && view === 'my_work' && mw.rows) return myWorkLive();
  return show();
}

// --- Live: one request at a time, including visibility changes -------------------------------

let version = null;
let polling = false;
let pollFailureNotice = null;

async function pollMaintenance() {
  if(view==='settings' && form?.maintenance)await refreshMaintenance(form.maintenance);
  const d = embeddingDraft;
  if (view === 'settings' && form?.embedding && (d.busy || d.watch || d.status?.active)) await refreshEmbedding();
}
async function poll() {
  if (polling || document.visibilityState !== 'visible') return;
  polling = true;
  try {
    if ((view === 'my_work' && mw.rows) || view === 'my_rules') myWorkTimes();
    await pollMaintenance();
    const { v } = await api('version');
    $('live').classList.remove('off');
    $('live').textContent = t('live');
    // Either language's, as the page may have switched since (Codex on #373).
    if (pollFailureNotice?.isConnected || TEXT.unreachable.includes($('status').textContent)) setStatus('');
    const changed = version === null ? drawnWithoutBaseline : v !== version;
    // Provider calls and tool records do not move v, so Stats also follows each poll, and My Work
    // reads again each minute: its statuses age, and a write's link to its session comes later
    // (claude-mem ages its requests each minute).
    const wanted = !reposLoaded || (view !== 'settings' && (changed || (view === 'stats' && !$('q').value.trim())
      || (view === 'my_work' && Date.now() - mw.fetched >= 60000)));
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
  // Arrived from the page's own move (`movePage`): the bookmark is to change. The mark leaves the
  // address, so a bookmark made now holds the token alone; the welcome was seen at the old one.
  const arrived = new URLSearchParams(location.hash.slice(1));
  const moved = arrived.get('moved') === '1';
  if (moved) {
    arrived.delete('moved');
    history.replaceState(null, '', `#${arrived}`);
  }
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
  if (!moved && recall('oboete-welcome-dismissed', '') !== 'true') openWelcome();
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
  if (moved) setStatus(t('view_moved'), false, lang);
}

// Pasting a restarted viewer's address changes its fragment without reloading the page.
window.addEventListener('hashchange', () => location.reload());
await start();
