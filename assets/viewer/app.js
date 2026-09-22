'use strict';
// Everything from the store is rendered with text nodes only: bodies are model-written text
// that may contain HTML.

const token = new URLSearchParams(location.hash.slice(1)).get('t') || '';
const $ = (id) => document.getElementById(id);

function el(tag, cls, ...children) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  e.append(...children.filter((c) => c !== null && c !== undefined));
  return e;
}

function setStatus(text, isError = false) {
  $('status').textContent = text;
  $('status').classList.toggle('error', isError);
}

async function api(name, params = {}, method = 'GET') {
  const res = await fetch(`/api/${name}?${new URLSearchParams(params)}`, {
    method,
    headers: { 'X-Oboete-Token': token },
  });
  if (res.status === 401) {
    throw new Error('This page needs the full address printed by `oboete view` (it carries the access key after #).');
  }
  if (!res.ok) throw new Error(`${res.status}: ${await res.text()}`);
  return res.status === 204 ? null : res.json();
}

const base = (path) => path.split('/').filter(Boolean).pop() || path;

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
  $('theme').textContent = `Theme: ${theme}`;
}

// --- Cards ----------------------------------------------------------------------------------

const GLYPH = new Map([
  ['summary', '🎯'], ['decision', '⚖'], ['bugfix', '●'], ['feature', '◆'],
  ['discovery', '○'], ['change', '✓'], ['preference', '★'],
]);

function badge(kind) {
  return el('span', `badge ${kind}`, kind);
}

// A button that loads a panel below it once, then shows and hides it. `key` names the panel so a
// redraw can reopen what was open.
const opened = new Set();

function expander(label, key, load) {
  const button = el('button', null, label);
  button.type = 'button';
  button.setAttribute('aria-expanded', 'false');
  let panel = null;
  const toggle = async () => {
    const open = button.getAttribute('aria-expanded') === 'true';
    if (panel) {
      panel.hidden = open;
      button.setAttribute('aria-expanded', String(!open));
      if (open) opened.delete(key); else opened.add(key);
      return;
    }
    button.disabled = true;
    try {
      panel = await load();
      // Below the whole button row when the button sits in one.
      const row = button.parentElement;
      (row && row.classList.contains('actions') ? row : button).after(panel);
      button.setAttribute('aria-expanded', 'true');
      opened.add(key);
    } catch (e) {
      setStatus(e.message, true);
    } finally {
      button.disabled = false;
    }
  };
  button.addEventListener('click', toggle);
  if (opened.has(key)) toggle();
  return button;
}

// Delete in two clicks, no dialog: the first arms the button for five seconds.
function deleter(label, run) {
  const button = el('button', 'danger', label);
  button.type = 'button';
  let armed = false;
  let timer = null;
  const disarm = () => {
    armed = false;
    button.textContent = label;
    button.classList.remove('armed');
    clearTimeout(timer);
  };
  button.addEventListener('click', async () => {
    if (!armed) {
      armed = true;
      button.textContent = `Confirm ${label.toLowerCase()}`;
      button.classList.add('armed');
      timer = setTimeout(disarm, 5000);
      return;
    }
    clearTimeout(timer);
    button.disabled = true;
    try {
      await run();
    } catch (e) {
      setStatus(e.message, true);
      button.disabled = false;
      disarm();
    }
  });
  return button;
}

function deleteDoc(d, card) {
  return deleter('Delete', async () => {
    await api('doc', { id: d.doc }, 'DELETE');
    card.remove();
    setStatus(`Deleted ${d.doc}.`);
  });
}

// One observation or summary. `extra` are meta cells shown before the id (agent, repository).
function card(d, extra = []) {
  const kind = d.kind || 'summary';
  const li = el('li', `card ${kind}`);
  const head = el('div', 'head',
    el('span', 'glyph', GLYPH.get(kind) || '·'),
    badge(kind),
    el('span', 'title', kind === 'summary' ? 'Session summary' : d.title));
  const meta = el('div', 'meta', el('span', null, d.when), ...extra, el('span', null, d.doc));
  li.append(head, el('p', 'text', d.text), meta, deleteDoc(d, li));
  return li;
}

function sessionEntry(s) {
  const summary = s.summary
    ? el('p', 'text', s.summary)
    : el('p', 'text pending', 'Not summarized yet.');
  const li = el('li', 'entry');
  const more = expander('Observations', s.id, async () => {
    const docs = await api('session', { id: s.id });
    const rows = docs.filter((d) => d.kind !== 'summary');
    return rows.length
      ? el('ul', 'cards', ...rows.map((d) => card(d)))
      : el('p', 'text pending', 'No observations for this session.');
  });
  const del = deleter('Delete session', async () => {
    await api('session', { id: s.id }, 'DELETE');
    li.remove();
    setStatus(`Deleted session ${s.id.slice(-8)} with everything it left.`);
  });
  li.append(
    el('div', 'meta',
      el('span', null, s.when),
      el('span', null, s.agent),
      el('span', null, base(s.repo)),
      el('span', null, s.id.slice(-8))),
    summary,
    el('div', 'actions', more, del));
  return li;
}

function hitEntry(h) {
  const text = el('p', 'text', h.text);
  // The snippet becomes the full text in place.
  const full = el('button', null, 'Full text');
  full.type = 'button';
  full.addEventListener('click', async () => {
    full.disabled = true;
    try {
      text.textContent = (await api('doc', { id: h.doc })).text;
      full.remove();
    } catch (e) {
      setStatus(e.message, true);
      full.disabled = false;
    }
  });
  const li = el('li', 'entry');
  li.append(
    el('div', 'meta', badge(h.kind), el('span', null, h.doc), el('span', null, h.when), el('span', null, base(h.repo))),
    h.title ? el('p', 'title', h.title) : null,
    text,
    el('div', 'actions', full, deleteDoc(h, li)));
  return li;
}

// --- Views ----------------------------------------------------------------------------------

// The API returns at most this many rows; the page says so when a list is cut there.
const LIMIT = 100;

const VIEWS = ['feed', 'sessions', 'context', 'stats'];
let view = VIEWS.includes(recall('oboete-view', 'feed')) ? recall('oboete-view', 'feed') : 'feed';

function setView(name) {
  view = name;
  remember('oboete-view', name);
  for (const b of document.querySelectorAll('#tabs .tab')) {
    b.classList.toggle('active', b.dataset.view === name);
    b.setAttribute('aria-current', b.dataset.view === name ? 'page' : 'false');
  }
}

function draw(heading, list, panel) {
  $('heading').replaceChildren(...(Array.isArray(heading) ? heading : [heading]));
  $('list').replaceChildren(...list);
  $('panel').replaceChildren(...panel);
}

function cutNotice(n, what, more) {
  return n === LIMIT ? `The newest ${LIMIT} ${what}. ${more}` : '';
}

async function showFeed(repo) {
  const rows = await api('feed', { repo, limit: LIMIT });
  return () => {
    draw('Feed', rows.map((d) => card(d, [el('span', null, d.agent), el('span', null, base(d.repo)), el('span', null, d.session.slice(-8))])), []);
    setStatus(rows.length ? cutNotice(rows.length, 'entries', 'Search to find older ones.') : 'Nothing remembered yet.');
  };
}

async function showSessions(repo) {
  const sessions = await api('timeline', { repo, limit: LIMIT });
  return () => {
    draw('Sessions', sessions.map(sessionEntry), []);
    setStatus(sessions.length ? cutNotice(sessions.length, 'sessions', 'Search to find older ones.') : 'No sessions recorded yet.');
  };
}

async function showSearch(repo, q) {
  const hits = await api('search', { q, repo, limit: LIMIT });
  return () => {
    draw(['Search: ', el('span', 'query', q)], hits.map(hitEntry), []);
    setStatus(hits.length === LIMIT
      ? `The ${LIMIT} best matches. Add words to narrow the search.`
      : hits.length ? `${hits.length} found` : 'Nothing found.');
  };
}

async function showContext(repo) {
  const c = await api('context', { repo });
  return () => {
    const body = c.text
      ? el('pre', 'context', c.text)
      : el('p', 'text pending', 'Nothing to hand over yet: this repository has no summaries or observations.');
    draw('Context handed to a new session', [], [
      el('p', 'lead', `What an agent starting in ${base(c.repo)} reads first. ${c.chars} characters.`),
      body,
    ]);
    setStatus('');
  };
}

function row(term, value) {
  return [el('dt', null, term), el('dd', null, String(value))];
}

async function showStats(repo) {
  const s = await api('stats', { repo });
  return () => {
    const kinds = s.observations.kinds.map((k) => el('li', null, badge(k.kind), ` ${k.count}`));
    const providers = s.providers.length
      ? el('table', 'providers',
        el('thead', null, el('tr', null, ...['Provider', 'OK', 'Failed', 'Waited', 'Avg ms'].map((h) => {
          const th = el('th', null, h);
          th.scope = 'col';
          return th;
        }))),
        el('tbody', null, ...s.providers.map((p) => el('tr', null,
          el('td', null, p.provider), el('td', null, String(p.ok)), el('td', null, String(p.failed)),
          el('td', null, String(p.waited)), el('td', null, p.avg_ms === null ? '–' : String(p.avg_ms))))))
      : el('p', 'text pending', 'No provider calls in the last seven days.');
    draw('Stats', [], [
      el('section', 'stat', el('h3', null, 'Sessions'), el('dl', null,
        ...row('Total', s.sessions.total),
        ...row('Summarized', s.sessions.summarized),
        ...row('Awaiting summary', s.sessions.pending),
        ...row('Handed context', s.sessions.injected),
        ...row('First', s.sessions.first || '–'),
        ...row('Last activity', s.sessions.last || '–'))),
      el('section', 'stat', el('h3', null, 'Knowledge'), el('dl', null,
        ...row('Observations', s.observations.total),
        ...row('Summaries', s.summaries)),
        kinds.length ? el('ul', 'kinds', ...kinds) : null),
      el('section', 'stat', el('h3', null, 'Store'), el('dl', null,
        ...row('Database', `${(s.db_bytes / 1048576).toFixed(1)} MB (all repositories)`))),
      el('section', 'stat', el('h3', null, 'Providers, last 7 days (all repositories)'), providers),
    ]);
    setStatus('');
  };
}

// Only the latest request may draw: an earlier, slower one must not overwrite it.
let generation = 0;
// A view drawn before the live baseline was taken may already be stale.
let drawnWithoutBaseline = false;

async function show() {
  const mine = ++generation;
  const repo = $('repo').value;
  const q = $('q').value.trim();
  setStatus('Loading…');
  try {
    const render = q ? await showSearch(repo, q)
      : view === 'sessions' ? await showSessions(repo)
        : view === 'context' ? await showContext(repo)
          : view === 'stats' ? await showStats(repo)
            : await showFeed(repo);
    if (mine !== generation) return false;
    render();
    if (version === null) drawnWithoutBaseline = true;
    return true;
  } catch (e) {
    if (mine === generation) setStatus(e.message, true);
    return false;
  }
}

// The picker: every repository with sessions. The first load selects the one the viewer was
// started in; later loads (Refresh) keep the current choice.
async function loadRepos(first) {
  const { current, repos } = await api('repos');
  const keep = first ? current : $('repo').value;
  const all = el('option', null, 'All repositories');
  all.value = '';
  const options = repos.map((r) => {
    const o = el('option', null, `${base(r.repo)} (${r.sessions})`);
    o.value = r.repo;
    o.title = r.repo;
    return o;
  });
  $('repo').replaceChildren(all, ...options);
  $('repo').value = repos.some((r) => r.repo === keep) ? keep : '';
}

// True when the page now shows the store as it is.
async function refresh() {
  try {
    await loadRepos(false);
  } catch (e) {
    setStatus(e.message, true);
    return false;
  }
  return show();
}

// --- Live: redraw when the store changes -----------------------------------------------------

let version = null;
const UNREACHABLE = 'The viewer is not answering. Start `oboete view` again and open the address it prints.';

async function poll() {
  if (document.visibilityState !== 'visible') return;
  try {
    const { v } = await api('version');
    if ($('live').classList.contains('off')) {
      $('live').classList.remove('off');
      if ($('status').textContent === UNREACHABLE) setStatus('');
    }
    const changed = version === null ? drawnWithoutBaseline : v !== version;
    // Stats also counts raw events, provider calls and handed-over context, which the marker
    // leaves out on purpose; that view is cheap, so it just follows every poll.
    const wanted = changed || (view === 'stats' && !$('q').value.trim());
    // The marker moves on only once the page shows that state; a failed redraw is retried by
    // the next poll.
    if (wanted && !(await refresh())) return;
    version = v;
    drawnWithoutBaseline = false;
  } catch {
    // The dot is for the eye; the status line is the page's live region.
    $('live').classList.add('off');
    setStatus(UNREACHABLE, true);
  }
}

async function start() {
  applyTheme();
  setView(view);
  try {
    await loadRepos(true);
  } catch (e) {
    setStatus(e.message, true);
    return;
  }
  $('controls').addEventListener('submit', (e) => {
    e.preventDefault();
    show();
  });
  $('repo').addEventListener('change', show);
  $('refresh').addEventListener('click', refresh);
  $('theme').addEventListener('click', () => {
    theme = THEMES[(THEMES.indexOf(theme) + 1) % THEMES.length];
    remember('oboete-theme', theme);
    applyTheme();
  });
  for (const b of document.querySelectorAll('#tabs .tab')) {
    b.addEventListener('click', () => {
      setView(b.dataset.view);
      $('q').value = '';
      show();
    });
  }
  // Clearing the search box (its x button included) goes back to the current view.
  $('q').addEventListener('search', () => {
    if (!$('q').value) show();
  });
  // Baseline first, then draw: a change between the two is caught by the next poll.
  await poll();
  show();
  setInterval(poll, 3000);
  document.addEventListener('visibilitychange', poll);
}

// A restarted viewer prints a new key; pasting its address into this tab only changes the part
// after #, which does not reload the page by itself.
window.addEventListener('hashchange', () => location.reload());
start();
