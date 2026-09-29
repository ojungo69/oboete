// Loaded as a module (strict, deferred, top-level await). Everything from the store is rendered
// with text nodes only: bodies are model-written text that may contain HTML.

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

const base = (path) => path.split('/').findLast(Boolean) || path;

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
  ['summary', '🎯'], ['prompt', '💬'], ['decision', '⚖'], ['bugfix', '●'], ['feature', '◆'],
  ['discovery', '○'], ['change', '✓'], ['preference', '★'],
]);
const HEADINGS = new Map([['summary', 'Session summary'], ['prompt', 'User prompt']]);

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
      (row?.classList.contains('actions') ? row : button).after(panel);
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

// Long text (a pasted log, a task written for another agent) starts folded to its head: the
// first lines, at most so many characters. `unfolded` holds the doc ids opened in full (here or
// through a search hit's Full text), so a redraw keeps them open.
const FOLD_LINES = 8;
const FOLD_CHARS = 600;
const unfolded = new Set();

function folded(text, key) {
  const head = Array.from(text.split('\n').slice(0, FOLD_LINES).join('\n')).slice(0, FOLD_CHARS).join('');
  if (head === text || unfolded.has(key)) return [el('p', 'text', text)];
  const p = el('p', 'text', `${head}…`);
  const all = el('button', 'quiet small', 'Show all');
  all.type = 'button';
  all.addEventListener('click', () => {
    p.textContent = text;
    all.remove();
    unfolded.add(key);
  });
  return [p, all];
}

// One summary, prompt or observation. `extra` are meta cells shown before the id (agent,
// repository).
function card(d, extra = []) {
  const kind = d.kind || 'summary';
  const li = el('li', `card ${kind}`);
  const head = el('div', 'head',
    el('span', 'glyph', GLYPH.get(kind) || '·'),
    badge(kind),
    el('span', 'title', HEADINGS.get(kind) || d.title));
  const meta = el('div', 'meta', el('span', null, d.when), ...extra, el('span', null, d.doc));
  li.append(head, ...folded(d.text, d.doc), meta, deleteDoc(d, li));
  return li;
}

function sessionEntry(s) {
  const summary = s.summary
    ? el('p', 'text', s.summary)
    : el('p', 'text pending', 'Not summarized yet.');
  const li = el('li', 'entry');
  const more = expander('Prompts and observations', s.id, async () => {
    const docs = await api('session', { id: s.id });
    const rows = docs.filter((d) => d.kind !== 'summary');
    return rows.length
      ? el('ul', 'cards', ...rows.map((d) => card(d)))
      : el('p', 'text pending', 'No prompts or observations for this session.');
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
  // The snippet becomes the full text in place, and stays so when the page redraws.
  const full = el('button', null, 'Full text');
  full.type = 'button';
  const expand = async () => {
    full.disabled = true;
    // Before the await: a redraw landing while the text loads must expand this hit too.
    unfolded.add(h.doc);
    try {
      text.textContent = (await api('doc', { id: h.doc })).text;
      full.remove();
    } catch (e) {
      unfolded.delete(h.doc);
      setStatus(e.message, true);
      full.disabled = false;
    }
  };
  full.addEventListener('click', expand);
  if (unfolded.has(h.doc)) expand();
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

const VIEWS = ['feed', 'sessions', 'context', 'stats', 'settings'];
let view = VIEWS.includes(recall('oboete-view', 'feed')) ? recall('oboete-view', 'feed') : 'feed';

function setView(name) {
  view = name;
  remember('oboete-view', name);
  // The repository picker, search and Refresh redraw the view, which would drop unsaved settings.
  $('controls').hidden = name === 'settings';
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
    if (hits.length === LIMIT) setStatus(`The ${LIMIT} best matches. Add words to narrow the search.`);
    else if (hits.length) setStatus(`${hits.length} found`);
    else setStatus('Nothing found.');
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
        ...row('Summaries', s.summaries),
        ...row('Prompts', s.prompts)),
        kinds.length ? el('ul', 'kinds', ...kinds) : null),
      el('section', 'stat', el('h3', null, 'Store'), el('dl', null,
        ...row('Database', `${(s.db_bytes / 1048576).toFixed(1)} MB (all repositories)`))),
      el('section', 'stat', el('h3', null, 'Providers, last 7 days (all repositories)'), providers),
    ]);
    setStatus('');
  };
}

// --- Settings (#94): injection, capture and the curator chain, in config.toml ---------------
// The settings panel's words switch between English and Japanese; the rest of the viewer stays
// English. The server sends codes, and the page puts them in words.

const LANGS = ['en', 'ja'];
// Each string in the languages of LANGS, in that order.
const TEXT = {
  heading: ['Settings', '設定'],
  language: ['Language', '言語'],
  lead: [
    'These settings are kept in config.toml in the oboete home. They apply from the next session start or the next curation window; nothing needs a restart.',
    'ここでの設定は、oboete のホームにある config.toml に保存されます。次のセッションの開始時か、次の要約から反映されます。再起動は必要ありません。',
  ],
  inject_h: ['Handing memory to agents', '記憶の受け渡し'],
  inject_desc: [
    'The summary of your memory that an agent is given when a session starts.',
    'セッションの開始時に、エージェントへ渡す記憶のまとめです。',
  ],
  inject_on: ['Give agents the summary of your memory', 'エージェントに記憶のまとめを渡す'],
  inject_chars: ['Size in characters ({min} to {max})', '大きさ(文字数、{min}〜{max})'],
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
  save: ['Save', '保存'],
  saved: ['Saved.', '保存しました。'],
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
    'Please keep at least one curator in use. To stop curation, set [summary] curate = false.',
    '要約役を少なくとも 1 つは使う設定にしてください。要約を止めるには、[summary] curate = false を設定します。',
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

// The form's values between redraws: a language switch or a move keeps what is not saved yet.
let form = null;

function formOf(s) {
  if (s.error) return null;
  const text = (v) => (v === null || v === undefined ? '' : String(v));
  return {
    version: s.version,
    inject: { ...s.inject, session_start_chars: String(s.inject.session_start_chars) },
    capture: { ...s.capture },
    chain: s.chain.map((e) => ({
      ...e,
      edit: { on: e.on, daily_budget: text(e.daily_budget), timeout_s: text(e.timeout_s), model: text(e.model) },
    })),
    warnings: s.warnings,
    ranges: s.ranges,
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
  const i = el('input');
  i.type = type;
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

function keyState(r) {
  const state = el('span', 'note', t(`key_${r.key.replaceAll('-', '_')}`));
  return r.key_file ? [state, note(t('key_file', { path: r.key_file }))] : [state];
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
    el('td', null, el('span', 'entry-name', r.name), ...keyState(r), r.entries > 1 ? note(t('entries', { n: r.entries })) : null),
    el('td', null, model, modelNote ? note(modelNote) : null),
    el('td', null, budget, r.budget_from_key && !r.edit.daily_budget ? note(t('from_key', { n: r.effective_daily_budget })) : null),
    el('td', null, timeout));
  return tr;
}

// The save's body, or the field a value is wrong in.
function saveBody() {
  const whole = (v, min, max) => (/^\d+$/.test(v.trim()) && Number(v) >= min && Number(v) <= max ? Number(v) : Number.NaN);
  const chars = whole(form.inject.session_start_chars, ...form.ranges.session_start_chars);
  if (Number.isNaN(chars)) return { field: 'inject.session_start_chars' };
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
      inject: { session_start: form.inject.session_start, session_start_chars: chars },
      capture: { store_prompts: form.capture.store_prompts, tool_output: form.capture.tool_output },
      chain,
    },
  };
}

function markInvalid(field) {
  const i = [...document.querySelectorAll('#panel input')].find((x) => x.dataset.field === field);
  if (!i) return;
  i.classList.add('invalid');
  i.setAttribute('aria-invalid', 'true');
  i.focus();
}

async function saveSettings(button) {
  const { body, field } = saveBody();
  if (!body) {
    markInvalid(field);
    setStatus(t('range'), true);
    return;
  }
  const mine = form;
  const fields = button.form;
  button.disabled = true;
  // What is typed while the save is on its way would not survive its answer.
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
      setStatus(t(current ? 'stale' : 'saved'), Boolean(current));
      return;
    }
    const byStatus = { 400: 'bad_request', 401: 'unauthorized', 403: 'forbidden', 413: 'too_large' };
    const code = answer.code || byStatus[res.status] || 'other';
    // An inert form takes no focus, and the refused field is to be reached.
    fields.inert = false;
    if (answer.field) markInvalid(answer.field);
    setStatus(t(code, { status: res.status }), true);
  } catch (e) {
    setStatus(e.message, true);
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
    drawSettings();
    setStatus('');
  });
  const panel = el('div', 'settings', el('label', 'field lang', el('span', null, t('language')), pick));
  panel.lang = lang;
  if (!form) {
    panel.append(el('p', 'text pending', t('file_error')));
    draw(t('heading'), [], [panel]);
    return;
  }
  const f = form;
  const chars = input('number', f.inject.session_start_chars, '', 'inject.session_start_chars', (v) => { f.inject.session_start_chars = v; });
  const [least, most] = f.ranges.session_start_chars;
  [chars.min, chars.max] = [least, most];
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
      el('h3', null, t('inject_h')),
      el('p', 'desc', t('inject_desc')),
      el('label', 'check', checkbox(f.inject.session_start, (v) => { f.inject.session_start = v; }), t('inject_on')),
      el('label', 'field', el('span', null, t('inject_chars', { min: least.toLocaleString('en-US'), max: most.toLocaleString('en-US') })), chars)),
    el('section', null,
      el('h3', null, t('capture_h')),
      el('p', 'desc', t('capture_desc')),
      el('label', 'check', checkbox(f.capture.store_prompts, (v) => { f.capture.store_prompts = v; }), t('store_prompts')),
      el('label', 'field', el('span', null, t('tool_output')), tool)),
    el('section', null,
      el('h3', null, t('chain_h')),
      el('p', 'desc', t('chain_desc')),
      el('div', 'scroll', el('table', 'chain', el('thead', null, head), rows))),
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
  draw(t('heading'), [], [panel]);
}

const LOADERS = new Map([
  ['feed', showFeed], ['sessions', showSessions], ['context', showContext], ['stats', showStats],
  ['settings', showSettings],
]);

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
    // The settings view has no search: its controls are hidden, and a query left in them is not its.
    const search = q && view !== 'settings';
    const render = search ? await showSearch(repo, q) : await (LOADERS.get(view) || showFeed)(repo);
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
  // A poll that began on another view does not redraw the settings opened meanwhile: that would
  // drop what is typed and not saved.
  if (view === 'settings') return true;
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
    // Never the settings: a redraw would drop what is typed and not saved.
    const wanted = view !== 'settings' && (changed || (view === 'stats' && !$('q').value.trim()));
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
await start();
