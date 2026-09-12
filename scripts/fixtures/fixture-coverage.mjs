const ROOT_PH = '__OBOETE_REPLAY_ROOT__';
const AT_BOUND = 1_048_576;
const ABOVE_ONE = 1_048_577;
const ABOVE_TWO = 2_097_152;
const FILL_ALPHABET = 'The quick brown fox jumps over the lazy dog. ';
const AGENTS = ['claude', 'codex', 'grok', 'pi'];

const PI_INPUT_SOURCES = ['interactive', 'rpc', 'extension'];

const FACTS = [
  { id: 'f-ja-01', lang: 'ja', expect: '3847', statement: 'ローカルの oboete はポート 3847 で listen する。設定ファイルは config/oboete.local.toml。', query: 'oboete のローカル listen ポートは何番？' },
  { id: 'f-ja-02', lang: 'ja', expect: 'memories_v3', statement: '要約の保存先テーブルは memories_v3 に切り替えた。旧 memories は読み取り専用。', query: '要約は今どのテーブルに書いてる？' },
  { id: 'f-ja-03', lang: 'ja', expect: 'ENABLE_CJK_BIGRAM', statement: '日本語検索は機能フラグ ENABLE_CJK_BIGRAM が立っているときだけ bigram を足す。', query: '日本語検索の bigram を制御するフラグ名は？' },
  { id: 'f-ja-04', lang: 'ja', expect: 'yamada-keisuke', statement: 'dogfood アカウントの担当は yamada-keisuke。鍵のローテは彼が持つ。', query: 'dogfood アカウントの担当者は誰？' },
  { id: 'f-ja-05', lang: 'ja', expect: 'E-OBOETE-4419', statement: '検出器が切れたときのログコードは E-OBOETE-4419。doctor も同じコードを出す。', query: '検出器タイムアウトのログコードは？' },
  { id: 'f-ja-06', lang: 'ja', expect: '0007_fts_ja', statement: '次のマイグレーションファイル名は 0007_fts_ja.sql。FTS の日本語トークナイザを足す。', query: '次に入れるマイグレーションのファイル名は？' },
  { id: 'f-ja-07', lang: 'ja', expect: 'memory-box.internal.example', statement: '社内の memory ホストは memory-box.internal.example。VPN 内からのみ解決する。', query: '社内 memory ホストの名前は？' },
  { id: 'f-ja-08', lang: 'ja', expect: '--ink-sumi-900', statement: 'ビューアの本文色トークンは --ink-sumi-900。ライトテーマでもこの名前のまま。', query: 'ビューア本文の色トークン名は？' },
  { id: 'f-ja-09', lang: 'ja', expect: 'dogfood-jp-07', statement: '隔離ユーザのログイン名は dogfood-jp-07。ホームは /home/dogfood-jp-07。', query: '日本語 dogfood のログイン名は？' },
  { id: 'f-ja-10', lang: 'ja', expect: 'src/privacy/cjk-normalize.ts', statement: '全角英数の正規化は src/privacy/cjk-normalize.ts に閉じ込めた。他から呼ばない。', query: '全角英数の正規化はどのファイル？' },
  { id: 'f-ja-11', lang: 'ja', expect: '2026-11-18', statement: '無料枠のリセット日は 2026-11-18。それまでは allowance を使い切らない。', query: '無料枠のリセット日はいつ？' },
  { id: 'f-ja-12', lang: 'ja', expect: 'oboete-ja-packs', statement: '日本語パックの R2 バケット名は oboete-ja-packs。本番だけ。', query: '日本語パックのバケット名は？' },
  { id: 'f-ja-13', lang: 'ja', expect: '7f3e91c', statement: '直近で入れた正規化の修正コミットは 7f3e91c。revert するならこれ。', query: '正規化修正のコミットはどれ？' },
  { id: 'f-ja-14', lang: 'ja', expect: 'OBOETE_JA_HINT', statement: '日本語ヒントを出すときは環境変数 OBOETE_JA_HINT を 1 にする。', query: '日本語ヒントの環境変数名は？' },
  { id: 'f-ja-15', lang: 'ja', expect: 'compact-q-ja', statement: '日本語セッションの compact キュー名は compact-q-ja。英語とは分ける。', query: '日本語 compact のキュー名は？' },
  { id: 'f-ja-16', lang: 'ja', expect: 'kuromoji-lite-0.4.2', statement: '形態素のフォールバックは kuromoji-lite-0.4.2。本番の tokenizer ではない。', query: '形態素フォールバックのライブラリと版は？' },
  { id: 'f-ja-17', lang: 'ja', expect: '937', statement: 'セッション要約の最短間隔は 937 秒。それより短い再実行は捨てる。', query: 'セッション要約の最短間隔は何秒？' },
  { id: 'f-ja-18', lang: 'ja', expect: 'm1/p5-t067-replay', statement: 'replay 作業ブランチは m1/p5-t067-replay。main には直接載せない。', query: 'replay 作業ブランチ名は？' },
  { id: 'f-ja-19', lang: 'ja', expect: '第3水曜日', statement: '鍵のローテーションは毎月 第3水曜日。前倒ししない。', query: '鍵ローテは月のいつ？' },
  { id: 'f-ja-20', lang: 'ja', expect: 'turn-budget-ja-48', statement: '日本語ターンの文字予算キーは turn-budget-ja-48。英語キーと混ぜない。', query: '日本語ターン予算のキー名は？' },
  { id: 'f-en-01', lang: 'en', expect: '9124', statement: 'The sidecar listens on port 9124. Health checks must use that port, not 8080.', query: 'Which port does the sidecar listen on?' },
  { id: 'f-en-02', lang: 'en', expect: 'raw_events_epoch', statement: 'Epoch annotations live in the raw_events_epoch table, not on sessions.', query: 'Which table stores epoch annotations?' },
  { id: 'f-en-03', lang: 'en', expect: 'ENABLE_PACK_TRIM', statement: 'Pack trimming is gated by ENABLE_PACK_TRIM. Leave it off in dogfood.', query: 'What flag gates pack trimming?' },
  { id: 'f-en-04', lang: 'en', expect: 'Priya-Nair', statement: 'The isolated-user probe owner is Priya-Nair. Ping her before killing tmux.', query: 'Who owns the isolated-user probe?' },
  { id: 'f-en-05', lang: 'en', expect: 'src/injection/epoch-key.ts', statement: 'Compaction epoch keys are computed in src/injection/epoch-key.ts only.', query: 'Where is the compaction epoch key computed?' },
  { id: 'f-en-06', lang: 'en', expect: 'E-OBOETE-8801', statement: 'A stalled Pi child is reported as E-OBOETE-8801 in doctor and the hook log.', query: 'What error code is a stalled Pi child?' },
  { id: 'f-en-07', lang: 'en', expect: 'feat/replay-harness', statement: 'Land replay harness work on feat/replay-harness, never on m1/p5-t067 directly.', query: 'Which branch should the replay harness land on?' },
  { id: 'f-en-08', lang: 'en', expect: '271', statement: 'The spool reclaim interval is 271 seconds so it never aligns with the 300 ms hook.', query: 'How many seconds is the spool reclaim interval?' },
  { id: 'f-en-09', lang: 'en', expect: 'pack-cache.internal', statement: 'The pack cache host is pack-cache.internal. It is not reachable off VPN.', query: 'What is the pack cache hostname?' },
  { id: 'f-en-10', lang: 'en', expect: '--accent-ember-600', statement: 'The viewer accent token is --accent-ember-600. Do not invent a second accent.', query: 'What is the viewer accent token name?' },
  { id: 'f-en-11', lang: 'en', expect: 'dogfood-en-03', statement: 'The English dogfood login is dogfood-en-03. Home is /home/dogfood-en-03.', query: 'What is the English dogfood login name?' },
  { id: 'f-en-12', lang: 'en', expect: '2026-12-02', statement: 'The next catalog freeze is 2026-12-02. Do not bump models after that date.', query: 'When is the next catalog freeze?' },
  { id: 'f-en-13', lang: 'en', expect: 'oboete-en-packs', statement: 'English packs go to the oboete-en-packs bucket. Japanese has its own.', query: 'Which bucket holds English packs?' },
  { id: 'f-en-14', lang: 'en', expect: '4c2a18d', statement: 'The injection ledger fix is commit 4c2a18d. Revert that if duplicates return.', query: 'Which commit is the injection ledger fix?' },
  { id: 'f-en-15', lang: 'en', expect: 'OBOETE_EN_HINT', statement: 'Set OBOETE_EN_HINT=1 to print English retrieval traces on stderr.', query: 'Which env var enables English retrieval traces?' },
  { id: 'f-en-16', lang: 'en', expect: 'observe-q-en', statement: 'The English observer queue name is observe-q-en. Do not share it with Japanese.', query: 'What is the English observer queue name?' },
  { id: 'f-en-17', lang: 'en', expect: 'murmur-pack-2.1.0', statement: 'Fallback packing uses murmur-pack-2.1.0. Do not upgrade it in M1.', query: 'Which library version does fallback packing use?' },
  { id: 'f-en-18', lang: 'en', expect: '0008_epoch_index', statement: 'The next English-side migration is 0008_epoch_index.sql.', query: 'What is the next English-side migration file?' },
  { id: 'f-en-19', lang: 'en', expect: 'turn-budget-64', statement: 'The English turn character budget key is turn-budget-64.', query: 'What is the English turn budget key?' },
  { id: 'f-en-20', lang: 'en', expect: 'grok-fixture-luna', statement: 'Grok fixture sessions pin model alias grok-fixture-luna so windows stay stable.', query: 'Which model alias do Grok fixture sessions pin?' },
];

function fillBytes(n) {
  if (n <= 0) return '';
  const unit = FILL_ALPHABET;
  const copies = Math.ceil(n / unit.length);
  return unit.repeat(copies).slice(0, n);
}

function walkExpand(value, expandString) {
  if (typeof value === 'string') return expandString(value);
  if (Array.isArray(value)) return value.map((item) => walkExpand(item, expandString));
  if (value !== null && typeof value === 'object') {
    const out = {};
    for (const [key, item] of Object.entries(value)) out[key] = walkExpand(item, expandString);
    return out;
  }
  return value;
}

function expandFillOnly(payload) {
  return walkExpand(payload, (text) =>
    text.replace(/__FILL:(\d+)__/g, (_, n) => fillBytes(Number(n))),
  );
}

function secretToken(id) {
  return `__SECRET:${id}__`;
}

function directiveToken(index) {
  return `__DIRECTIVE:${index}__`;
}

function countBy(events, keyFn) {
  const out = {};
  for (const event of events) {
    const key = keyFn(event);
    out[key] = (out[key] ?? 0) + 1;
  }
  return out;
}

function promptTextOf(event) {
  if (event.event === 'UserPromptSubmit') return event.payload.prompt ?? '';
  if (event.event === 'input') return event.payload.payload?.text ?? '';
  return '';
}

function claudeAdapterOutput(payload) {
    const response = payload.tool_response;
    const tool = payload.tool_name;
    // Write/Edit adapter output is the path (`writtenPath`), not file content.
    if (tool === 'Write' || tool === 'Edit') {
      return typeof response?.filePath === 'string' ? response.filePath : '';
    }
    if (typeof response === 'string') return response;
    if (response === null || typeof response !== 'object') return '';
    if (typeof response.file?.content === 'string') return response.file.content;
    const stdout = typeof response.stdout === 'string' ? response.stdout : '';
    const stderr = typeof response.stderr === 'string' ? response.stderr : '';
    if ('stdout' in response || 'stderr' in response) return `${stdout}\n${stderr}`;
    return '';
}

function grokAdapterOutput(payload) {
  const result = payload.toolResult ?? {};
  if (typeof result.FileContent?.content === 'string') return result.FileContent.content;
  if (typeof result.EditsApplied?.tool_output_for_prompt === 'string') {
    return result.EditsApplied.tool_output_for_prompt;
  }
  if (typeof result.output_for_prompt === 'string') return result.output_for_prompt;
  return '';
}

function piAdapterOutput(payload) {
  const blocks = payload.payload?.content;
  if (!Array.isArray(blocks)) return '';
  return blocks.map((block) => (typeof block?.text === 'string' ? block.text : '')).join('\n');
}

function adapterOutputText(event) {
  const payload = event.payload;
  if (event.agent === 'claude' && event.event === 'PostToolUse') {
    return claudeAdapterOutput(payload);
  }
  if (event.agent === 'codex' && event.event === 'PostToolUse') {
    return typeof payload.tool_response === 'string' ? payload.tool_response : '';
  }
  if (event.agent === 'grok' && event.event === 'PostToolUse') {
    return grokAdapterOutput(payload);
  }
  if (event.agent === 'pi' && event.event === 'tool_result') {
    return piAdapterOutput(payload);
  }
  return '';
}

function newCoverageStats(secrets) {
  const secretIds = new Set(
    secrets.filter((row) => row.secret !== null).map((row) => row.id),
  );
  const seenSecrets = new Set();
  const dirPrompt = new Set();
  const dirOutput = new Set();
  const factsPlanted = new Set();
  const factsRecalled = new Set();
  const plantSession = new Map();
  const recallSession = new Map();
  const lifecycle = { resume: new Set(), compact: new Set(), fork: new Set(), clear: new Set() };
  const sizes = [];
  const langs = { ja: 0, en: 0 };
  return {
    secretIds, seenSecrets, dirPrompt, dirOutput, factsPlanted, factsRecalled,
    plantSession, recallSession, lifecycle, sizes, langs,
  };
}

function collectDirectiveCoverage(event, tags, stats) {
    const { dirPrompt, dirOutput } = stats;
    if (tags.directive !== undefined) {
      const promptEvent =
        event.event === 'UserPromptSubmit' || event.event === 'input';
      if (promptEvent) dirPrompt.add(tags.directive);
      else dirOutput.add(tags.directive);
    }
}

function collectFactCoverage(event, tags, stats) {
    const { factsPlanted, plantSession, langs } = stats;
    if (tags.fact !== undefined) {
      factsPlanted.add(tags.fact.id);
      plantSession.set(tags.fact.id, `${event.agent}:${event.session}`);
      if (tags.fact.lang === 'ja') langs.ja += 1;
      else langs.en += 1;
    }
}

function collectCoverageEvent(event, stats) {
    const {
      seenSecrets, factsRecalled, recallSession, lifecycle, sizes,
    } = stats;
    const tags = event.tags ?? {};
    if (tags.secret !== undefined) seenSecrets.add(tags.secret);
    collectDirectiveCoverage(event, tags, stats);
    collectFactCoverage(event, tags, stats);
    if (tags.recall !== undefined) {
      factsRecalled.add(tags.recall);
      recallSession.set(tags.recall, `${event.agent}:${event.session}`);
    }
    if (tags.lifecycle !== undefined) lifecycle[tags.lifecycle]?.add(event.agent);
    if (tags.size !== undefined) {
      const bytes = Buffer.byteLength(JSON.stringify(expandFillOnly(event.payload)));
      sizes.push({ agent: event.agent, seq: event.seq, tag: tags.size, bytes });
    }
}

function collectCoverageEvents(events, stats) {
  for (const event of events) {
    collectCoverageEvent(event, stats);
  }
}

function missingDirectives(directives, dirPrompt, dirOutput) {
  const missingDir = [];
  for (let i = 0; i < directives.length; i += 1) {
    if (!dirPrompt.has(i) || !dirOutput.has(i)) missingDir.push(i);
  }
  return missingDir;
}

function missingRequiredKinds(byEvent) {
  const required = {
    claude: ['SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'PostToolUseFailure', 'Stop', 'PostCompact', 'SessionEnd'],
    codex: ['SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'PostToolUseFailure', 'Stop', 'PostCompact', 'SessionEnd'],
    grok: ['SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'PostToolUseFailure', 'PermissionDenied', 'Stop', 'PostCompact', 'SessionEnd'],
    pi: ['session_start', 'input', 'tool_result', 'agent_settled', 'session_shutdown', 'session_compact'],
  };
  const missingKinds = [];
  for (const agent of AGENTS) {
    for (const name of required[agent]) {
      if ((byEvent[`${agent}:${name}`] ?? 0) === 0) missingKinds.push(`${agent}:${name}`);
    }
  }
  return missingKinds;
}

function coverageGaps(stats, secrets, directives, byEvent) {
  const {
    secretIds, seenSecrets, dirPrompt, dirOutput, factsPlanted, factsRecalled,
    plantSession, recallSession,
  } = stats;
  const missingSecrets = [...secretIds].filter((id) => !seenSecrets.has(id));
  const negativeIds = secrets.filter((row) => row.secret === null).map((row) => row.id);
  const missingNegatives = negativeIds.filter((id) => !seenSecrets.has(id));
  const missingDir = missingDirectives(directives, dirPrompt, dirOutput);
  const missingPlant = FACTS.filter((fact) => !factsPlanted.has(fact.id)).map((fact) => fact.id);
  const missingRecall = FACTS.filter((fact) => !factsRecalled.has(fact.id)).map((fact) => fact.id);
  const sameSessionRecall = [];
  for (const id of factsRecalled) {
    if (plantSession.get(id) === recallSession.get(id)) sameSessionRecall.push(id);
  }

  const missingKinds = missingRequiredKinds(byEvent);
  return {
    missingSecrets, negativeIds, missingNegatives, missingDir,
    missingPlant, missingRecall, sameSessionRecall, missingKinds,
  };
}

function coverage(events, secrets, directives) {
  const byAgent = countBy(events, (event) => event.agent);
  const byEvent = countBy(events, (event) => `${event.agent}:${event.event}`);
  const stats = newCoverageStats(secrets);
  collectCoverageEvents(events, stats);
  const gaps = coverageGaps(stats, secrets, directives, byEvent);

  return {
    total: events.length,
    byAgent,
    byEvent,
    secrets: {
      required: stats.secretIds.size,
      seen: stats.seenSecrets.size,
      missing: gaps.missingSecrets,
      negatives: gaps.negativeIds.length,
      missingNegatives: gaps.missingNegatives,
    },
    directives: {
      total: directives.length,
      prompt: stats.dirPrompt.size,
      output: stats.dirOutput.size,
      missing: gaps.missingDir,
    },
    facts: {
      planted: stats.factsPlanted.size,
      recalled: stats.factsRecalled.size,
      ja: stats.langs.ja,
      en: stats.langs.en,
      missingPlant: gaps.missingPlant,
      missingRecall: gaps.missingRecall,
      sameSessionRecall: gaps.sameSessionRecall,
    },
    lifecycle: Object.fromEntries(
      Object.entries(stats.lifecycle).map(([name, set]) => [name, [...set]]),
    ),
    sizes: stats.sizes,
    missingKinds: gaps.missingKinds,
  };
}

function assertAgentCoverage(report, problems) {
  if (report.total < 1000) problems.push(`only ${report.total} events`);
  for (const agent of AGENTS) {
    const n = report.byAgent[agent] ?? 0;
    if (n < 240) problems.push(`${agent} has ${n} events`);
  }
}

function assertCorpusCoverage(report, problems) {
  if (report.secrets.missing.length > 0) {
    problems.push(`missing secrets ${report.secrets.missing.join(',')}`);
  }
  if (report.secrets.missingNegatives.length > 0) {
    problems.push(`missing negative secrets ${report.secrets.missingNegatives.join(',')}`);
  }
  if (report.directives.missing.length > 0) {
    problems.push(`directives not in both prompt and output: ${report.directives.missing.join(',')}`);
  }
  if (report.facts.ja < 20 || report.facts.en < 20) {
    problems.push(`facts ja=${report.facts.ja} en=${report.facts.en}`);
  }
  if (report.facts.missingPlant.length > 0) problems.push(`unplanted ${report.facts.missingPlant.join(',')}`);
  if (report.facts.missingRecall.length > 0) problems.push(`unrecalled ${report.facts.missingRecall.join(',')}`);
  if (report.facts.sameSessionRecall.length > 0) {
    problems.push(`recall in same session ${report.facts.sameSessionRecall.join(',')}`);
  }
  if (report.missingKinds.length > 0) problems.push(`missing kinds ${report.missingKinds.join(',')}`);
}

function assertLifecycleCoverage(report, problems) {
  for (const name of ['resume', 'compact', 'clear']) {
    if (report.lifecycle[name].length < 4) problems.push(`lifecycle ${name} agents=${report.lifecycle[name]}`);
  }
  if (!report.lifecycle.fork.includes('claude') || !report.lifecycle.fork.includes('grok') || !report.lifecycle.fork.includes('pi')) {
    problems.push(`lifecycle fork agents=${report.lifecycle.fork}`);
  }
}

function assertSizeCoverage(report, problems) {
  const at = report.sizes.filter((row) => row.tag === 'at_bound' && row.bytes === AT_BOUND);
  const above = report.sizes.filter((row) => row.tag === 'above_bound');
  if (at.length < 2) problems.push(`at_bound count ${at.length}`);
  if (above.length < 2) problems.push(`above_bound count ${above.length}`);
  if (!above.some((row) => row.bytes === ABOVE_ONE)) problems.push('missing 1048577');
  if (!above.some((row) => row.bytes === ABOVE_TWO)) problems.push('missing 2097152');
  for (const row of report.sizes) {
    if (row.tag === 'at_bound' && row.bytes !== AT_BOUND) {
      problems.push(`size seq ${row.seq} at_bound is ${row.bytes}`);
    }
    if (row.tag === 'above_bound' && row.bytes !== ABOVE_ONE && row.bytes !== ABOVE_TWO) {
      problems.push(`size seq ${row.seq} above_bound is ${row.bytes}`);
    }
  }
}

function assertNoSecretLeaks(secrets, body, problems) {
  for (const row of secrets) {
    if (row.secret !== null && body.includes(row.secret)) problems.push(`secret value leaked for ${row.id}`);
  }
}

function validateFactEvent(event, tags, blob, problems) {
  if (tags.fact !== undefined) {
    const expect = tags.fact.expect;
    if (typeof expect !== 'string' || expect === '' || !blob.includes(expect)) {
      problems.push(`fact ${tags.fact.id} expect missing from payload seq ${event.seq}`);
    }
    if (
      (event.event === 'PostToolUse' || event.event === 'tool_result') &&
      !adapterOutputText(event).includes(expect)
    ) {
      problems.push(`fact ${tags.fact.id} expect not in adapter output seq ${event.seq}`);
    }
  }
}

function validatePiSecretEvent(event, tags, token, problems) {
  if (event.event === 'tool_result' && !adapterOutputText(event).includes(token)) {
    const inputBlob = JSON.stringify(event.payload.payload?.input ?? {});
    const toolName = event.payload.payload?.toolName;
    if (!inputBlob.includes(token)) {
      problems.push(`secret ${tags.secret} not in pi input or content seq ${event.seq}`);
    } else if (toolName === 'write' || toolName === 'edit') {
      problems.push(`secret ${tags.secret} in pi ${toolName} input seq ${event.seq}`);
    }
  }
}

function validateSecretEvent(event, tags, blob, problems) {
  if (tags.secret !== undefined) {
    const token = secretToken(tags.secret);
    if (!blob.includes(token)) {
      problems.push(`secret token missing from payload seq ${event.seq} id=${tags.secret}`);
    }
    if (event.event === 'PostToolUse' && !adapterOutputText(event).includes(token)) {
      problems.push(`secret ${tags.secret} not in adapter output seq ${event.seq}`);
    }
    validatePiSecretEvent(event, tags, token, problems);
  }
}

function validatePiDirectiveEvent(event, tags, token, problems) {
  if (event.event === 'tool_result' && !adapterOutputText(event).includes(token)) {
    const toolName = event.payload.payload?.toolName;
    if (toolName === 'write' || toolName === 'edit') {
      problems.push(`directive ${tags.directive} in pi ${toolName} input seq ${event.seq}`);
    }
  }
}

function validateDirectiveEvent(event, tags, blob, problems) {
  if (tags.directive !== undefined) {
    const token = directiveToken(tags.directive);
    if (!blob.includes(token)) {
      problems.push(`directive token missing from payload seq ${event.seq} index=${tags.directive}`);
    }
    if (event.event === 'PostToolUse' && !adapterOutputText(event).includes(token)) {
      problems.push(`directive ${tags.directive} not in adapter output seq ${event.seq}`);
    }
    validatePiDirectiveEvent(event, tags, token, problems);
  }
}

function validateRecallEvent(event, tags, factsById, problems) {
  if (tags.recall !== undefined) {
    const fact = factsById.get(tags.recall);
    const prompt = promptTextOf(event);
    if (fact === undefined || !prompt.includes(fact.query)) {
      problems.push(`recall ${tags.recall} prompt missing query seq ${event.seq}`);
    }
  }
}

function validateGrokTimestamp(event, grokTimestamps, problems) {
  const ts = event.payload.timestamp;
  if (typeof ts !== 'string' || ts === '') {
    problems.push(`grok missing timestamp seq ${event.seq}`);
  } else {
    if (grokTimestamps.includes(ts)) problems.push(`duplicate grok timestamp ${ts} seq ${event.seq}`);
    if (grokTimestamps.length > 0 && ts <= grokTimestamps.at(-1)) {
      problems.push(`grok timestamp not increasing seq ${event.seq}`);
    }
    grokTimestamps.push(ts);
  }
}

function validateGrokFork(event, tags, problems) {
  if (tags.lifecycle === 'fork') {
    const expected = `${ROOT_PH}/.oboete-replay/grok/${event.session}.jsonl`;
    if (event.payload.transcriptPath !== expected) {
      problems.push(`grok fork reuses transcript seq ${event.seq}`);
    }
  }
}

function validateGrokEvent(event, tags, grokTimestamps, grokCompacts, problems) {
  if (event.agent === 'grok') {
    validateGrokTimestamp(event, grokTimestamps, problems);
    if (event.event === 'PostCompact') {
      grokCompacts[event.session] = (grokCompacts[event.session] ?? 0) + 1;
    }
    validateGrokFork(event, tags, problems);
  }
}

function validateCoverageEvents(events, problems) {
  const factsById = new Map(FACTS.map((fact) => [fact.id, fact]));
  const grokTimestamps = [];
  const grokCompacts = {};
  const piSources = new Set();
  let prev = 0;
  for (const event of events) {
    if (event.seq !== prev + 1) problems.push(`seq gap at ${event.seq}`);
    prev = event.seq;
    JSON.parse(JSON.stringify(event));
    const tags = event.tags ?? {};
    const blob = JSON.stringify(event.payload);
    if (event.agent === 'claude' && Object.hasOwn(event.payload, 'model')) {
      problems.push(`claude payload has model at seq ${event.seq}`);
    }
    validateFactEvent(event, tags, blob, problems);
    validateSecretEvent(event, tags, blob, problems);
    validateDirectiveEvent(event, tags, blob, problems);
    validateRecallEvent(event, tags, factsById, problems);
    validateGrokEvent(event, tags, grokTimestamps, grokCompacts, problems);
    if (event.agent === 'pi' && event.event === 'input') {
      piSources.add(event.payload.payload?.source);
    }
  }
  return { grokCompacts, piSources };
}

function assertAdapterCoverage(grokCompacts, piSources, problems) {
  if (!Object.values(grokCompacts).some((n) => n >= 2)) {
    problems.push('no grok session compact twice');
  }
  for (const source of PI_INPUT_SOURCES) {
    if (!piSources.has(source)) problems.push(`pi input source missing ${source}`);
  }
}

function coverageTurnsBySession(events, problems) {
  const turnsBySession = {};
  for (const event of events) {
    if (event.event === 'UserPromptSubmit' || event.event === 'input') {
      turnsBySession[event.session] = (turnsBySession[event.session] ?? 0) + 1;
    }
  }
  for (const [label, turns] of Object.entries(turnsBySession)) {
    if (turns < 4 || turns > 12) problems.push(`session ${label} has ${turns} turns`);
  }
  return turnsBySession;
}

function assertCoverage(events, secrets, directives, body) {
  const report = coverage(events, secrets, directives);
  const problems = [];
  assertAgentCoverage(report, problems);
  assertCorpusCoverage(report, problems);
  assertLifecycleCoverage(report, problems);
  assertSizeCoverage(report, problems);
  assertNoSecretLeaks(secrets, body, problems);
  const { grokCompacts, piSources } = validateCoverageEvents(events, problems);
  assertAdapterCoverage(grokCompacts, piSources, problems);
  const turnsBySession = coverageTurnsBySession(events, problems);
  if (problems.length > 0) throw new Error(`coverage failed:\n- ${problems.join('\n- ')}`);
  report.turnsBySession = turnsBySession;
  return report;
}

export {
  ABOVE_ONE, ABOVE_TWO, AGENTS, AT_BOUND, FACTS, FILL_ALPHABET, PI_INPUT_SOURCES, ROOT_PH,
  assertCoverage, countBy, directiveToken, expandFillOnly, fillBytes, secretToken,
};
