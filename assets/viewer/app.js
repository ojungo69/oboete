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

// The settings view's text is in the language chosen there, the rest of the page in English:
// `textLang` marks which one the status line reads (#274).
function setStatus(text, isError = false, textLang = null) {
  $('status').textContent = text;
  $('status').classList.toggle('error', isError);
  if (textLang) $('status').lang = textLang;
  else $('status').removeAttribute('lang');
}

function showError(error, retry) {
  setStatus(error.message, true);
  const notice = $('status').firstChild;
  if (error.status !== 503) return notice;
  const button = el('button', 'quiet small', 'Retry');
  button.type = 'button';
  button.addEventListener('click', async () => {
    button.disabled = true;
    try { await retry(); } finally { button.disabled = false; }
  });
  $('status').append(' ', button);
  return notice;
}

async function api(name, params = {}) {
  const res = await fetch(`/api/${name}?${new URLSearchParams(params)}`, {
    headers: { 'X-Oboete-Token': token },
  });
  if (!res.ok) {
    let message = `${res.status}: ${await res.text()}`;
    if (res.status === 401) {
      message += '. This page needs the full address printed by `oboete view` (it carries the access key after #).';
    }
    const error = new Error(message);
    error.status = res.status;
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
  $('theme').textContent = `Theme: ${theme}`;
}

// --- Entries and their details --------------------------------------------------------------

function badge(kind) {
  return el('span', `badge ${kind}`, kind);
}

function localTime(ms) {
  const date = new Date(ms);
  const time = el('time', null, date.toLocaleString('en-US'));
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

function fullText(key, label = 'Full text') {
  const button = expander(label, `doc:${key}`, async () => {
    const doc = await api('doc', { id: key });
    return el('section', 'detail', el('h4', null, doc.id), el('pre', 'document-text', doc.text));
  });
  button.setAttribute('aria-label', `Full text of ${key}`);
  return button;
}

function claimLink(uid, label = uid, restore = false) {
  // Reciprocal claim links open only on a click; restoring them would follow a cycle forever.
  const button = expander(label, `claim:${uid}`, () => claimPanel(uid), restore);
  button.setAttribute('aria-label', `View claim ${uid}`);
  return button;
}

function claimLinks(heading, uids) {
  return el('section', null, el('h4', null, heading),
    uids.length ? el('ul', 'claim-links', ...uids.map((uid) => el('li', null, claimLink(uid)))) : el('p', 'text pending', 'None.'));
}

async function claimPanel(uid) {
  const c = await api('claim', { id: uid });
  const quotes = c.quotes.map((q) => el('li', null,
    el('p', 'text', q.text), fullText(q.key, q.key)));
  const history = c.history.map((change) => el('li', null,
    el('div', 'meta', localTime(change.ts),
      change.tier === null && change.recipe === null ? el('span', null, "Owner's correction") : null,
      change.tier === null ? null : el('span', null, `Tier: ${change.tier}`),
      change.recipe === null ? null : el('span', null, `Recipe: ${change.recipe}`),
      el('span', null, change.status === null ? 'Status unchanged' : `Status: ${change.status}`)),
    el('p', change.body === null ? 'text pending' : 'text', change.body ?? 'Text unchanged')));
  return el('section', 'detail claim-view',
    el('h3', null, `Claim ${c.uid}`),
    el('div', 'meta', badge(c.kind), badge(c.status), badge(c.label), localTime(c.when)),
    el('dl', 'claim-meta', ...row('Delivered', c.delivered ? 'Yes' : 'No'),
      ...row('Speaker', c.speaker), ...row('Scope', c.scope), ...row('Repository', c.repo ?? '–')),
    el('p', 'text', c.text),
    c.later ? el('p', 'relation', 'Later claim: ', claimLink(c.later)) : null,
    el('section', null, el('h4', null, 'Evidence quotes'),
      quotes.length ? el('ul', 'quotes', ...quotes) : el('p', 'text pending', 'No evidence quotes.')),
    claimLinks('Supersedes', c.supersedes), claimLinks('Ended by', c.ended_by),
    el('section', null, el('h4', null, 'History (oldest first)'),
      history.length ? el('ol', 'claim-history', ...history) : el('p', 'text pending', 'No changes.')));
}

function entryMeta(d, all) {
  return el('div', 'meta', badge(d.class), d.label ? badge(d.label) : null,
    badge(d.kind), d.status ? badge(d.status) : null, localTime(d.when),
    all ? el('span', null, d.repo ?? '–') : null, el('span', null, d.key));
}

function entryActions(key, isClaim) {
  return el('div', 'actions', fullText(key), isClaim ? claimLink(key, 'View claim', true) : null);
}

// Built through el(), which leaves out the nulls: Element.append would print them as "null".
function hitEntry(h, all) {
  return el('li', h.class === 'delivered' ? 'entry delivered' : 'entry', entryMeta(h, all),
    h.class === 'delivered' ? el('p', 'relation', 'Earlier decision, paired with later claim: ', claimLink(h.later)) : null,
    h.class === 'superseded' && h.by ? el('p', 'relation', 'Superseded by ', claimLink(h.by)) : null,
    h.title ? el('p', 'title', h.title) : null, el('p', 'text', h.snippet),
    entryActions(h.key, ['current', 'delivered', 'superseded'].includes(h.class)));
}

function timelineEntry(item, all) {
  return el('li', 'entry', entryMeta(item, all), el('p', 'text', item.text),
    entryActions(item.key, item.class === 'claim'));
}

// --- Views ----------------------------------------------------------------------------------

const LIMIT = 100;
const VIEWS = ['timeline', 'context', 'stats', 'settings'];
let view = VIEWS.includes(recall('oboete-view', 'timeline')) ? recall('oboete-view', 'timeline') : 'timeline';
let currentRepo = '';
let reposLoaded = false;

function setView(name) {
  view = name;
  remember('oboete-view', name);
  $('controls').hidden = name === 'settings';
  for (const b of document.querySelectorAll('#tabs .tab')) {
    b.classList.toggle('active', b.dataset.view === name);
    b.setAttribute('aria-current', b.dataset.view === name ? 'page' : 'false');
  }
}

function draw(heading, list, panel) {
  $('heading').removeAttribute('lang');
  $('heading').replaceChildren(...(Array.isArray(heading) ? heading : [heading]));
  $('list').replaceChildren(...present(list));
  $('panel').replaceChildren(...present(panel));
  $('vector').textContent = '';
  $('vector').hidden = true;
}

function scope(repo) {
  return repo ? { repo } : { all: '1' };
}

async function showTimeline(repo) {
  const params = { ...scope(repo), limit: LIMIT };
  const page = await api('timeline', params);
  return () => {
    const mine = generation;
    let next = page.next;
    const more = el('button', 'quiet more', 'More');
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
        setStatus(`${$('list').childElementCount} entries loaded.`);
      } catch (e) {
        if (mine === generation) showError(e, loadMore);
      } finally {
        more.disabled = false;
      }
    };
    more.addEventListener('click', loadMore);
    draw('Timeline', page.items.map((item) => timelineEntry(item, !repo)), next ? [more] : []);
    setStatus(page.items.length ? `${page.items.length} entries loaded.` : 'No entries recorded yet.');
  };
}

async function showSearch(repo, q) {
  const answer = await api('search', { q, ...scope(repo), limit: LIMIT,
    since: $('since').value, until: $('until').value,
    history: $('history').checked ? '1' : '0', raw: $('raw').value });
  return () => {
    // The server puts each delivered decision immediately after the claim that ended it.
    draw(['Search: ', el('span', 'query', q)], answer.hits.map((h) => hitEntry(h, !repo)), []);
    if (answer.vector !== 'used') {
      $('vector').textContent = `Results are full text only (${answer.vector})${answer.why ? `: ${answer.why}` : '.'}`;
      $('vector').hidden = false;
    }
    if (answer.hits.length === LIMIT) setStatus(`The ${LIMIT} best matches. Add words to narrow the search.`);
    else if (answer.hits.length) setStatus(`${answer.hits.length} found.`);
    else setStatus('Nothing found.');
  };
}

async function showContext(repo) {
  // Omitting the viewer's own label also preserves its actual checkout branch.
  const c = await api('context', repo && repo !== currentRepo ? { repo } : {});
  return () => {
    draw('Context handed to a new session', [], [
      !repo ? el('p', 'lead', "Context shows one checkout; All repositories uses the viewer's checkout.") : null,
      el('dl', 'claim-meta', ...row('Repository', c.repo), ...row('Branch', c.branch || '–'),
        ...row('SessionStart', c.on ? 'On' : 'Off'), ...row('Size', `${c.chars} characters`)),
      c.text ? el('pre', 'context', c.text) : el('p', 'text pending', 'Nothing is handed over for this checkout yet.'),
    ]);
    setStatus('');
  };
}

function row(term, value) {
  return [el('dt', null, term), el('dd', null, String(value))];
}

function statsTable(headers, rows, empty) {
  if (!rows.length) return el('p', 'text pending', empty);
  return el('div', 'table-scroll', el('table', 'stats-table',
    el('thead', null, el('tr', null, ...headers.map((h) => {
      const th = el('th', null, h);
      th.scope = 'col';
      return th;
    }))),
    el('tbody', null, ...rows.map((cells) => el('tr', null,
      ...cells.map((cell) => el('td', null, String(cell ?? '–'))))))));
}

async function showStats() {
  const s = await api('stats');
  return () => {
    draw('Stats (all repositories)', [], [
      el('section', 'stat', el('h3', null, 'Records per device'),
        statsTable(['Device', 'Records'], s.records.map((r) => [r.device, r.records]), 'No records.')),
      el('section', 'stat', el('h3', null, 'Claims by kind and status'),
        statsTable(['Kind', 'Status', 'Count'], s.claims.map((c) => [c.kind, c.status, c.count]), 'No claims.')),
      el('section', 'stat', el('h3', null, 'Skipped claims'),
        statsTable(['Reason', 'Count'], s.claim_skips.map((c) => [c.reason, c.count]), 'No skipped claims.')),
      el('section', 'stat', el('h3', null, 'Store'), el('dl', null,
        ...row('Size', `${s.bytes.toLocaleString('en-US')} bytes (${(s.bytes / 1048576).toFixed(1)} MB)`),
        ...row('Rebuilding', s.rebuilding ? 'Yes' : 'No'))),
      el('section', 'stat', el('h3', null, 'Providers, last 7 days'),
        statsTable(['Provider', 'Role', 'OK', 'Failed', 'Waited', 'Avg ms'],
          s.providers.map((p) => [p.provider, p.role, p.ok, p.failed, p.waited, p.avg_ms]),
          'No provider calls in the last seven days.')),
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
// `[inject]`'s sizes, each checked against the range the server states for it.
const SIZES = ['session_start_chars', 'per_prompt_chars', 'correction_chars'];

function formOf(s) {
  if (s.error) return null;
  const text = (v) => (v === null || v === undefined ? '' : String(v));
  return {
    version: s.version,
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
    setStatus(e.message, true);
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
    drawIn(panel);
    return;
  }
  const f = form;
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
  ['timeline', showTimeline], ['context', showContext], ['stats', showStats], ['settings', showSettings],
]);

// Only the latest request may draw: an earlier, slower one must not overwrite it.
let generation = 0;
let drawnWithoutBaseline = false;

async function show() {
  // The settings tab needs no repository: it names the config.toml mistake that fails their list (#325).
  if (!reposLoaded && view !== 'settings') return refresh();
  const mine = ++generation;
  const repo = $('repo').value;
  const q = $('q').value.trim();
  setStatus('Loading…');
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

// The current checkout always has the first option, including before its first stored record.
async function loadRepos() {
  const { current, repos } = await api('repos');
  const keep = reposLoaded ? $('repo').value : current;
  currentRepo = current;
  const own = repos.find((r) => r.repo === current) ?? { repo: current, claims: 0, imported: 0, records: 0 };
  const options = [own, ...repos.filter((r) => r.repo !== current)].map((r) => {
    const o = new Option(`${r.repo} (${r.claims} claims, ${r.imported} imported, ${r.records} records)`, r.repo);
    o.title = `${r.repo}${r.repo === current ? ' (current checkout)' : ''}${r.last === undefined ? '' : `; last activity: ${new Date(r.last).toLocaleString('en-US')}`}`;
    return o;
  });
  $('repo').replaceChildren(...options, new Option('All repositories', ''));
  $('repo').value = keep === '' || options.some((o) => o.value === keep) ? keep : current;
  reposLoaded = true;
}

async function refresh() {
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
  return show();
}

// --- Live: one request at a time, including visibility changes -------------------------------

let version = null;
let polling = false;
let pollFailureNotice = null;
const UNREACHABLE = 'The viewer is not answering. Start `oboete view` again and open the address it prints.';

async function poll() {
  if (polling || document.visibilityState !== 'visible') return;
  polling = true;
  try {
    const { v } = await api('version');
    $('live').classList.remove('off');
    if (pollFailureNotice?.isConnected || $('status').textContent === UNREACHABLE) setStatus('');
    const changed = version === null ? drawnWithoutBaseline : v !== version;
    // Provider calls and tool records do not move v, so Stats also follows each poll.
    const wanted = !reposLoaded || (view !== 'settings' && (changed || (view === 'stats' && !$('q').value.trim())));
    if (wanted && !(await refresh())) return;
    // A failed redraw leaves the marker unchanged, so the next poll retries it.
    version = v;
    drawnWithoutBaseline = false;
  } catch (e) {
    $('live').classList.add('off');
    if (e.status) pollFailureNotice = showError(e, poll);
    else setStatus(UNREACHABLE, true);
  } finally {
    polling = false;
  }
}

async function start() {
  applyTheme();
  setView(view);
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
