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

async function api(name, params = {}) {
  const res = await fetch(`/api/${name}?${new URLSearchParams(params)}`, {
    headers: { 'X-Oboete-Token': token },
  });
  if (res.status === 401) {
    throw new Error('This page needs the full address printed by `oboete view` (it carries the access key after #).');
  }
  if (!res.ok) throw new Error(`${res.status}: ${await res.text()}`);
  return res.json();
}

const base = (path) => path.split('/').filter(Boolean).pop() || path;

function badge(kind) {
  return el('span', `badge ${kind}`, kind);
}

// A button that loads a panel below it once, then shows and hides it.
function expander(label, load) {
  const button = el('button', null, label);
  button.type = 'button';
  button.setAttribute('aria-expanded', 'false');
  let panel = null;
  button.addEventListener('click', async () => {
    const open = button.getAttribute('aria-expanded') === 'true';
    if (panel) {
      panel.hidden = open;
      button.setAttribute('aria-expanded', String(!open));
      return;
    }
    button.disabled = true;
    try {
      panel = await load();
      button.after(panel);
      button.setAttribute('aria-expanded', 'true');
    } catch (e) {
      setStatus(e.message, true);
    } finally {
      button.disabled = false;
    }
  });
  return button;
}

function docItem(d) {
  return el('li', null,
    el('div', 'meta', badge(d.kind), el('span', null, d.doc), el('span', null, d.when)),
    d.title ? el('p', 'title', d.title) : null,
    el('p', 'text', d.text));
}

function sessionEntry(s) {
  const summary = s.summary
    ? el('p', 'text', s.summary)
    : el('p', 'text pending', 'Not summarized yet.');
  const more = expander('Observations', async () => {
    const docs = await api('session', { id: s.id });
    const rows = docs.filter((d) => d.kind !== 'summary');
    return rows.length
      ? el('ul', 'docs', ...rows.map(docItem))
      : el('p', 'text pending', 'No observations for this session.');
  });
  return el('li', 'entry',
    el('div', 'meta',
      el('span', null, s.when),
      el('span', null, s.agent),
      el('span', null, base(s.repo)),
      el('span', null, s.id.slice(-8))),
    summary,
    more);
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
  return el('li', 'entry',
    el('div', 'meta', badge(h.kind), el('span', null, h.doc), el('span', null, h.when), el('span', null, base(h.repo))),
    h.title ? el('p', 'title', h.title) : null,
    text,
    full);
}

async function show() {
  const repo = $('repo').value;
  const q = $('q').value.trim();
  setStatus('Loading…');
  try {
    if (q) {
      const hits = await api('search', { q, repo, limit: 100 });
      $('heading').replaceChildren('Search: ', el('span', 'query', q));
      $('list').replaceChildren(...hits.map(hitEntry));
      setStatus(hits.length ? `${hits.length} found` : 'Nothing found.');
    } else {
      const sessions = await api('timeline', { repo, limit: 100 });
      $('heading').textContent = 'Sessions';
      $('list').replaceChildren(...sessions.map(sessionEntry));
      setStatus(sessions.length ? '' : 'No sessions recorded yet.');
    }
  } catch (e) {
    setStatus(e.message, true);
  }
}

async function start() {
  try {
    const { current, repos } = await api('repos');
    const all = el('option', null, 'All repositories');
    all.value = '';
    const options = repos.map((r) => {
      const o = el('option', null, `${base(r.repo)} (${r.sessions})`);
      o.value = r.repo;
      o.title = r.repo;
      return o;
    });
    $('repo').replaceChildren(all, ...options);
    $('repo').value = repos.some((r) => r.repo === current) ? current : '';
  } catch (e) {
    setStatus(e.message, true);
    return;
  }
  $('controls').addEventListener('submit', (e) => {
    e.preventDefault();
    show();
  });
  $('repo').addEventListener('change', show);
  $('refresh').addEventListener('click', show);
  // Clearing the search box (its x button included) goes back to the timeline.
  $('q').addEventListener('search', () => {
    if (!$('q').value) show();
  });
  show();
}

// A restarted viewer prints a new key; pasting its address into this tab only changes the part
// after #, which does not reload the page by itself.
window.addEventListener('hashchange', () => location.reload());
start();
