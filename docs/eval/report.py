"""PR-B2: metrics from the runs and the judge's grades (proposal §3.2), with ranx.

    uv run --with ranx python report.py <split> [judge] [--m4]

Compares the runs in ~/.oboete/eval/runs; to compare another method, put its run there and judge
the new pooled pairs first.

Relevant = grade 2 or 3 (UMBRELA "has an answer"); nDCG uses the grades. A question counts only
if the judge found at least one relevant document for it (the rest have no answer in the pool and
are reported as a count, not scored). Unjudged documents count as not relevant, so a run is only
comparable to the runs that fed the pool. Documents of the question's own session are left out
of every run first (the same rule as `oboete eval`).
"""
import functools, json, math, os, sqlite3, statistics, sys, time

from ranx import Qrels, Run, compare, evaluate
from scipy.stats import nct, t, ttest_rel

import judge as J
import m4
from common import JA

E = J.E
DAY = 86_400_000
FLOOR = 0.02  # The owner's item 9 may set this to 0.03 (D10); until then it stays 0.02.
# `-l2`: only grade 2 and 3 count as relevant; nDCG uses the grades themselves.
metrics = ['ndcg@10', 'mrr@10-l2', 'recall@10-l2', 'hit_rate@10-l2']
LINE_SLICES = ('all', 'question in Japanese', 'question in English', 'developer prompts',
               'prompts typed within 90 days', 'prompts typed 90 days ago or earlier')
TS = {
    'o': 'SELECT ts FROM observations WHERE id=?',
    's': 'SELECT ts FROM summaries WHERE id=?',
    'p': 'SELECT ts FROM prompts WHERE id=?',
}


def near_copy(question, text):
    """The document holds 80% of the question's trigrams: it quotes the question."""
    grams = m4.trigrams(question)
    return len(grams & m4.trigrams(text)) >= 0.8 * max(1, len(grams))


def table_data(runs, judged, queries, keep=lambda q: True, drop=lambda qid, doc: False,
               own=(), raw=False, depth=J.POOL_DEPTH):
    """An in-memory view, removing documents from both ranks and qrels before answerability."""
    def removed(qid, doc):
        return doc in own or (not raw and doc.startswith('r:')) or drop(qid, doc)

    qrels = {}
    for qid, q in queries.items():
        if not keep(q):
            continue
        grades = {d: g for d, g in judged.get(qid, {}).items() if g > 0 and not removed(qid, d)}
        if max(grades.values(), default=0) >= 2:
            qrels[qid] = grades
    # Filter within the judged depth: lifting a hit from below it would lift an unjudged hit.
    filtered = {name: {q: [d for d in per.get(q, [])[:depth] if not removed(q, d)] for q in qrels}
                for name, per in runs.items()}
    return qrels, filtered


def slices(asked, now, shown, timestamps, queries, m4_mode=False):
    out = [
        ('all', lambda q: True, lambda qid, doc: False),
        ('question in Japanese', lambda q: q['lang'] == 'ja', lambda qid, doc: False),
        ('question in English', lambda q: q['lang'] == 'en', lambda qid, doc: False),
        ('developer prompts', lambda q: q['set'] == 'prompt', lambda qid, doc: False),
        ('prompts typed within 90 days', lambda q: q['qid'] in asked and now - asked[q['qid']] < 90 * DAY,
         lambda qid, doc: False),
        ('prompts typed 90 days ago or earlier', lambda q: q['qid'] in asked and now - asked[q['qid']] >= 90 * DAY,
         lambda qid, doc: False),
        ('prompts, documents written before them only', lambda q: q['qid'] in asked,
         lambda qid, doc: timestamps.get(doc, 0) >= asked[qid]),
        ('documents quoting the question removed', lambda q: True,
         lambda qid, doc: near_copy(queries[qid]['text'], shown.get(doc, ''))),
        ('observations only', lambda q: True, lambda qid, doc: doc[0] != 'o'),
    ]
    if m4_mode:
        out += [
            ('Japanese questions, documents without Japanese characters', lambda q: q['lang'] == 'ja',
             lambda qid, doc: doc not in shown or bool(JA.search(shown[doc]))),
            ('English questions, documents with Japanese characters', lambda q: q['lang'] == 'en',
             lambda qid, doc: doc not in shown or not JA.search(shown[doc])),
        ]
    else:
        out.insert(4, ('agent searches', lambda q: q['set'] == 'agent', lambda qid, doc: False))
    return out


def tables(runs, judged, queries, asked, now, shown, timestamps, own=(), m4_mode=False):
    """All table views, using only supplied ranks, grades, question rows and shown text."""
    return {label: table_data(runs, judged, queries, keep, drop, own, raw=not m4_mode,
                              depth=m4.DEPTH if m4_mode else J.POOL_DEPTH)
            for label, keep, drop in slices(asked, now, shown, timestamps, queries, m4_mode)}


def run_objects(qrels, runs):
    out = {}
    for name, per in runs.items():
        ranked = {q: {d: 1000 - i for i, d in enumerate(per.get(q, []))} for q in qrels}
        # Run({q: {}}) cannot infer a document string width; an empty Run can be made comparable.
        out[name] = Run(ranked, name=name) if any(ranked.values()) else Run(name=name)
    return out


def score(qrels, runs):
    rs = run_objects(qrels, runs)
    if qrels:
        qr = Qrels(qrels)
        for run in rs.values():
            evaluate(qr, run, metrics=metrics, make_comparable=True, threads=1)
    return rs


def mean(rs, name, metric, qids=None):
    per = rs[name].scores[metric]
    values = list(per.values()) if qids is None else [per[q] for q in qids]
    return statistics.fmean(values) if values else math.nan


def table(label, qrels, runs, m4_mode=False):
    count = f'{len(qrels)} questions' + (' with an answer' if m4_mode else '')
    print(f'\n## {label}: {count}' + (' (too few to score; holds no line)' if len(qrels) < 5 and m4_mode
                                     else ' (too few to score)' if len(qrels) < 5 else ''))
    if not m4_mode:
        if len(qrels) >= 5:
            print(compare(Qrels(qrels), list(run_objects(qrels, runs).values()), metrics=metrics,
                          max_p=0.05, make_comparable=True))
        return
    rs = score(qrels, runs)
    if len(qrels) < 5:
        return rs
    print('system ' + ' '.join(metrics))
    for name in rs:
        print(name + ' ' + ' '.join(f'{mean(rs, name, metric):.6f}' for metric in metrics))
    return rs


def lines(label, rs, name):
    """The recall and claude-mem floors; a small slice holds no line."""
    if len(rs[name].scores['ndcg@10']) < 5:
        return []
    failures = []
    for metric, baseline, floor in [('recall@10-l2', 'hybrid-d2', 0.02),
                                    ('ndcg@10', 'claude-mem', 0.0),
                                    ('ndcg@10', 'claude-mem-nowindow', 0.0)]:
        a, b = mean(rs, name, metric), mean(rs, baseline, metric)
        passed = a >= b - floor - 1e-12
        label_metric = metric.split('-l')[0]
        print(f'{"PASS" if passed else "FAIL"} {name} [{label}] {label_metric}={a:.6f} '
              f'{baseline}={b:.6f} diff={a - b:+.6f} limit={-floor:+.6f}')
        if not passed:
            failures.append(f'{label}: {label_metric} against {baseline}')
    return failures


def paired(a, b):
    """Paired two-sided Student t, sample SD and a paired 95% interval."""
    differences = [x - y for x, y in zip(a, b, strict=True)]
    n = len(differences)
    diff = statistics.fmean(differences) if n else math.nan
    if n < 2:
        return dict(n=n, diff=diff, sd=math.nan, low=math.nan, high=math.nan, p=1.0)
    sd = statistics.stdev(differences)
    half = float(t.ppf(0.975, n - 1)) * sd / math.sqrt(n)
    p = float(ttest_rel(a, b, alternative='two-sided').pvalue) if sd else float(diff == 0)
    return dict(n=n, diff=diff, sd=sd, low=diff - half, high=diff + half, p=p)


def contrast(rs, name, baseline):
    a, b = rs[name].scores['ndcg@10'], rs[baseline].scores['ndcg@10']
    qids = sorted(a)
    return paired([a[q] for q in qids], [b[q] for q in qids])


def detectable(n, sd):
    """Smallest paired mean difference at two-sided alpha 0.05 and power 0.8."""
    if n < 2 or not math.isfinite(sd):
        return math.nan
    if sd == 0:
        return 0.0
    critical = float(t.ppf(0.975, n - 1))

    def power(noncentrality):
        return nct.sf(critical, n - 1, noncentrality) + nct.cdf(-critical, n - 1, noncentrality)

    low, high = 0.0, 1.0
    while power(high) < 0.8:
        high *= 2
    for _ in range(60):
        middle = (low + high) / 2
        if power(middle) < 0.8:
            low = middle
        else:
            high = middle
    return high * sd / math.sqrt(n)


def raw_table(label, data, queries, selectors, adjusted, eligible, reported=False):
    """Raw's slice checks stay inside its eligible population and retain record qrels. Raw N is
    D10's: the eligible questions, then how many of them have an answer."""
    qrels, runs = data
    print(f'\n## {label}: Raw N = {eligible} eligible questions, {len(qrels)} with an answer, '
          f'{eligible - len(qrels)} without' + (' (reported only)' if reported else ''))
    rs = score(qrels, runs)
    failures = {name: [] for name in m4.RAW_RUNS}
    for slice_label, keep, _ in selectors:
        if slice_label not in LINE_SLICES:
            continue
        qids = [qid for qid in qrels if keep(queries[qid])]
        print(f'Raw [{slice_label}]: {len(qids)} questions with an answer'
              + (' (holds no line)' if len(qids) < 5 else ''))
        if len(qids) < 5:
            continue
        for name in m4.RAW_RUNS:
            a, b = mean(rs, name, 'ndcg@10', qids), mean(rs, 'b-off', 'ndcg@10', qids)
            passed = a >= b - 0.02 - 1e-12
            print(f'{"PASS" if passed else "FAIL"} {name} [Raw {slice_label}] ndcg@10={a:.6f} '
                  f'b-off={b:.6f} diff={a - b:+.6f} limit=-0.020000')
            if not passed:
                failures[name].append(f'{slice_label}: nDCG@10 drops more than 0.02')
    for name in m4.RAW_RUNS:
        result = contrast(rs, name, 'b-off')
        reasons = failures[name]
        if not result['diff'] >= 0.02 - 1e-12:
            reasons.append('difference below +0.02')
        if adjusted[name] >= 0.05:
            reasons.append('Holm p >= 0.05')
        print(f'{name}: ndcg@10={mean(rs, name, "ndcg@10"):.6f} '
              f'b-off={mean(rs, "b-off", "ndcg@10"):.6f} diff={result["diff"]:+.6f} '
              f'paired SD={result["sd"]:.6f} detectable difference={detectable(result["n"], result["sd"]):.6f} '
              f'(two-sided 0.05, power 0.8) p={result["p"]:.6g} Holm p={adjusted[name]:.6g}')
        print(f'{"NO-GO" if reasons else "GO"} {name}: ' + ('; '.join(reasons) or 'all Raw lines held')
              + (' (reported only)' if reported else ''))


def m4_report(runs, judged, queries, own, eligible, asked, now, shown, timestamps):
    # The Raw runs hold the eligible questions only: they are scored in the Raw table alone.
    views = tables({name: per for name, per in runs.items() if name not in m4.RAW_RUNS}, judged, queries,
                   asked, now, shown, timestamps, own, m4_mode=True)
    results, rerank_failures = {}, []
    for label, (qrels, per) in views.items():
        results[label] = table(label, qrels, per, m4_mode=True)
        if label in LINE_SLICES:
            lines(label, results[label], 'b-off')
            if 'b-rerank' in runs:
                rerank_failures += lines(label, results[label], 'b-rerank')
    all_rs = results['all']
    if len(views['all'][0]) >= 5:
        a, b = mean(all_rs, 'b-off', 'ndcg@10'), mean(all_rs, 'hybrid-d2', 'ndcg@10')
        print(f'{"PASS" if a >= b - FLOOR - 1e-12 else "FAIL"} b-off [all] ndcg@10={a:.6f} '
              f'hybrid-d2={b:.6f} diff={a - b:+.6f} FLOOR={FLOOR:.6f}')

    raw = table_data(runs, judged, queries, keep=lambda q: q['qid'] in eligible, own=own,
                     raw=True, depth=m4.DEPTH)
    raw_rs = score(*raw)
    family = [('b-off', 'hybrid-d2', contrast(all_rs, 'b-off', 'hybrid-d2'))]
    if 'b-rerank' in runs:
        family.append(('b-rerank', 'b-off', contrast(all_rs, 'b-rerank', 'b-off')))
    family += [(name, 'b-off', contrast(raw_rs, name, 'b-off')) for name in m4.RAW_RUNS]
    adjusted = m4.holm([result['p'] for _, _, result in family])
    print(f'\n## Holm family: m={len(family)} (paired two-sided Student t)')
    for (name, baseline, result), p in zip(family, adjusted):
        print(f'{name} against {baseline}: N={result["n"]} diff={result["diff"]:+.6f} '
              f'95% CI=[{result["low"]:+.6f}, {result["high"]:+.6f}] '
              f'p={result["p"]:.6g} Holm p={p:.6g}')
    corrected = {name: p for (name, _, _), p in zip(family, adjusted)}
    if 'b-rerank' not in runs:
        print('NO-GO b-rerank: absent (left the run at the dev resource check)')
    else:
        rerank = next(result for name, _, result in family if name == 'b-rerank')
        if not rerank['diff'] >= 0.03 - 1e-12:
            rerank_failures.append('difference below +0.03')
        if corrected['b-rerank'] >= 0.05:
            rerank_failures.append('Holm p >= 0.05')
        print(f'{"NO-GO" if rerank_failures else "GO"} b-rerank: '
              f'ndcg@10={mean(all_rs, "b-rerank", "ndcg@10"):.6f} '
              f'b-off={mean(all_rs, "b-off", "ndcg@10"):.6f} diff={rerank["diff"]:+.6f} '
              f'Holm p={corrected["b-rerank"]:.6g}; ' + ('; '.join(rerank_failures) or 'all lines held'))

    selectors = slices(asked, now, shown, timestamps, queries, m4_mode=True)
    raw_table('Raw', raw, queries, selectors, corrected, len(eligible))
    removed = {(qid, doc) for qid in eligible for doc in judged.get(qid, {})
               if doc not in own and doc.startswith('r:') and near_copy(queries[qid]['text'], shown[doc])}
    print(f'near-copy records in Raw pools: {len({doc for _, doc in removed})} unique, '
          f'{len(removed)} question-record pairs')
    clean = table_data(runs, judged, queries, keep=lambda q: q['qid'] in eligible,
                       drop=lambda qid, doc: (qid, doc) in removed, own=own, raw=True, depth=m4.DEPTH)
    clean_rs = score(*clean)
    # A sensitivity view of the same family, reported only; the primary decisions above stay put.
    clean_ps = [contrast(clean_rs, name, baseline)['p'] if name in m4.RAW_RUNS else result['p']
                for name, baseline, result in family]
    clean_adjusted = {name: p for (name, _, _), p in zip(family, m4.holm(clean_ps))}
    raw_table('Raw, near-copy records removed', clean, queries, selectors, clean_adjusted, len(eligible),
              reported=True)

    print('\n## English-minus-Japanese gap (M21)')
    # One-sided, as M21's error rates are (a system equal in both languages fails 4-6% of the
    # time at N = 60 against 103): English at most 0.05 below Japanese. hybrid-d2 is reported only.
    for name in ('b-off', 'b-rerank', 'hybrid-d2'):
        if name not in all_rs:
            continue
        per = all_rs[name].scores['ndcg@10']
        en = [value for qid, value in per.items() if queries[qid]['lang'] == 'en']
        ja = [value for qid, value in per.items() if queries[qid]['lang'] == 'ja']
        if min(len(en), len(ja)) < 2:
            print(f'{name}: English N={len(en)} Japanese N={len(ja)} '
                  '(too few for a Welch interval; holds no line)')
            continue
        diff, low, high = m4.gap(en, ja)
        holds = name != 'hybrid-d2' and min(len(en), len(ja)) >= 5
        status = ('PASS ' if diff >= -0.05 - 1e-12 else 'FAIL ') if holds else ''
        print(f'{status}{name}: '
              f'English N={len(en)} Japanese N={len(ja)} diff={diff:+.6f} '
              f'95% Welch CI=[{low:+.6f}, {high:+.6f}] limit=-0.050000'
              + (' (holds no line)' if not status else ''))


def main():
    args = sys.argv[1:]
    m4_mode = '--m4' in args
    args = [arg for arg in args if arg != '--m4']
    if len(args) not in (1, 2):
        sys.exit('usage: report.py <split> [judge] [--m4]')
    if m4_mode and J.POOL_DEPTH != m4.DEPTH:
        sys.exit('--m4 requires OBOETE_EVAL_DEPTH=50')
    split, judge = args[0], args[1] if len(args) > 1 else J.JUDGE
    # The deciding test runs are read only under D10; milestone 4's dev runs (Task 6 Step 10) decide
    # nothing and keep the plain report.
    if not m4_mode and split == 'test' and os.path.exists(f'{J.RUNS}/b-off.trec'):
        sys.exit('these are milestone 4 runs: add --m4')
    if m4_mode and judge != J.JUDGE:
        sys.exit(f'--m4 reports the pinned judge only (D10): {J.JUDGE}')
    db = sqlite3.connect(f'file:{E}/home/oboete.db?mode=ro', uri=True)
    queries = {q['qid']: q for q in map(json.loads, open(J.QUESTIONS)) if q['split'] == split}
    if m4_mode and (len(queries) != m4.TEST_N or sum(q['lang'] == 'en' for q in queries.values()) != m4.ENGLISH_N):
        sys.exit(f'--m4 scores the {m4.TEST_N} test questions, {m4.ENGLISH_N} of them English: '
                 'set OBOETE_EVAL_QUESTIONS to questions-test-m4.jsonl')
    latest = J.latest(judge)
    own = m4.v1_own() if m4_mode else set()
    runs_now = m4.load(J.RUNS, m4.RUNS) if m4_mode else J.load_runs()
    if m4_mode:
        missing_runs = set(m4.RUNS) - {'b-rerank'} - runs_now.keys()
        if missing_runs:
            sys.exit('missing milestone 4 runs: ' + ', '.join(sorted(missing_runs)))
        with open(f'{m4.E}/corpus-m4.json') as f:
            window = json.load(f)['window']
        eligible = {qid for qid, q in queries.items() if m4.raw_eligible(q, window)}
        # The gate's own check: an empty or partial run would score its missing questions as zeros.
        gaps = m4.missing(runs_now, sorted(runs_now), set(queries), eligible)
        if gaps:
            sys.exit(f'{len(gaps)} run lines missing, as m4.py gate counts them; first: {gaps[0]}')
    pools = {qid: [(d, text, n) for d, text, n in J.pool(db, runs_now, q) if d not in own]
             for qid, q in queries.items()}
    # A partly judged pool would score whichever questions happened to be judged first.
    missing = sum(1 for qid, pool in pools.items() for doc, _, n in pool
                  if not ((qid, doc) in latest and J.covers(latest[(qid, doc)], n)))
    if missing:
        sys.exit(f'{missing} pooled pairs have no grade for the text the judge now sees; '
                 f'run judge.py {split} {len(queries)} first')
    # Only the current pool's grades define the qrels and recall denominators.
    judged = {qid: {doc: latest[(qid, doc)]['grade'] for doc, _, _ in pool} for qid, pool in pools.items()}

    @functools.cache
    def session_of(doc):
        return J.doc_text(db, doc)[1]

    runs = {name: {qid: [d for d in docs if session_of(d) != queries[qid]['session']]
                   for qid, docs in per.items() if qid in queries} for name, per in sorted(runs_now.items())}

    @functools.cache
    def ts(doc):
        if doc.startswith('r:'):
            return m4.record(J.side(), doc)['ts']
        row = db.execute(TS[doc[0]], (int(doc[1:]),)).fetchone()
        return row[0] if row else 0

    asked = {qid: ts(qid) for qid, q in queries.items() if q['set'] == 'prompt'}
    path = f'{J.RUNS}/claude-mem.trec'
    now = os.path.getmtime(path) * 1000 if m4_mode or os.path.exists(path) else time.time() * 1000
    shown = {doc: text for pool in pools.values() for doc, text, _ in pool}
    timestamps = {doc: ts(doc) for doc in shown}
    answerable = {qid for qid, grades in judged.items()
                  if any(g >= 2 and (not m4_mode or not d.startswith('r:')) for d, g in grades.items())}
    print(f'split={split} judge={judge} questions={len(queries)} judged={len(judged)} '
          f'with an answer={len(answerable)} without={len(judged) - len(answerable)}')
    if m4_mode:
        m4_report(runs, judged, queries, own, eligible, asked, now, shown, timestamps)
    else:
        for label, data in tables(runs, judged, queries, asked, now, shown, timestamps).items():
            table(label, *data)
    db.close()


if __name__ == '__main__':
    main()
