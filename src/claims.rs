//! Claims (spec 3.2, milestone 3 Task 6): what curation keeps from a window, one claim op each,
//! and the current claims derived from those ops in knowledge.db.

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// Where a claim's quote is: its event (`device`, `seq`), its byte range in that event's long
/// text (`curate::long_text`), the start of the sentence it starts in, and the quote itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub device: String,
    pub seq: i64,
    pub offset: i64,
    pub length: i64,
    pub sentence: i64,
    pub quote: String,
    /// On a claim's first quote, when its sentence holds another claim of its kind: where the
    /// claim starts in the event, which its uid adds (#125, `curate::keyed`). Left out otherwise,
    /// so a claim alone in its sentence keeps the uid it always had.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_at: Option<i64>,
}

/// The body of a claim op, which the curation phase writes (Task 7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimOp {
    /// Local to its window: a sibling's `supersedes` names it (MUST-M2).
    pub id: String,
    pub kind: String,
    pub status: String,
    pub speaker: String,
    pub scope: String,
    pub body: String,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    /// Sibling ids, or the uids of candidates it was shown.
    #[serde(default)]
    pub supersedes: Vec<String>,
    /// How it was derived (the provider and model), kept with it so a rebuild picks the same
    /// derivation (MUST-M18).
    pub recipe: String,
    /// 0 code only, 1 free or local, 2 subscription, 3 paid (spec 1.4): the highest is active.
    pub tier: i64,
    /// A change's reason as the record gives it, or `unknown` (spec 3.3): the gates write it
    /// (Task 8). Empty for other kinds, and in an op written before the gates, which has none.
    #[serde(default)]
    pub why: String,
    /// A proposal whose words came from tool content (MUST-M4): an acceptance in the session's
    /// next window does not promote it (#144).
    #[serde(default)]
    pub tainted: bool,
}

/// Spec 3.2's kinds.
pub const KINDS: [&str; 7] = [
    "decision",
    "preference",
    "lesson",
    "fix",
    "open item",
    "repo fact",
    "change",
];

/// Spec 3.2's statuses, and `unverified` for a kind or status it does not know (D13).
const STATUSES: [&str; 4] = ["decided", "proposed", "retracted", "done"];

/// `(kind, status)` as stored: the old kinds mapped (feature to change, discovery to repo fact)
/// and anything else a repo fact with status unverified (spec 3.2, D13).
pub fn normalize(kind: &str, status: &str) -> (&'static str, &'static str) {
    let kind = match kind {
        "feature" => Some("change"),
        "discovery" => Some("repo fact"),
        k => KINDS.iter().copied().find(|&known| known == k),
    };
    let status = STATUSES.iter().copied().find(|&s| s == status);
    match (kind, status) {
        (Some(k), Some(s)) => (k, s),
        (Some(k), None) => (k, "unverified"),
        (None, _) => ("repo fact", "unverified"),
    }
}

/// MUST-M18: a claim's uid is its kind and the sentence its first quote starts in, never a name
/// the model gives, so a recuration that rewords it derives the same uid; and where it starts
/// when its sentence holds another claim of its kind, so two of them keep two uids (#125).
pub fn uid(kind: &str, first: &Evidence) -> String {
    let key = format!(
        "{kind}\0{}\0{}\0{}",
        first.device, first.seq, first.sentence
    );
    crate::curate::sha256_hex(&match first.claim_at {
        None => key,
        Some(at) => format!("{key}\0@{at}"),
    })
}

/// Windows whose claims lost a quote to a mask or a removal since: Task 11 sends them again.
/// With the claim op that lost it, so a rewind that loses the op takes it back, and its first
/// record: a recuration of a span's middle leaves the parts on both sides (#192).
const RECURATE: &str = "CREATE TABLE IF NOT EXISTS recurate(
  device TEXT NOT NULL, from_seq INTEGER NOT NULL, to_seq INTEGER NOT NULL,
  op_device TEXT NOT NULL, op_seq INTEGER NOT NULL,
  PRIMARY KEY (op_device, op_seq, from_seq)
)";

/// Runs `change` when `needed` says so, asked again under the write lock: a worker and a command
/// can open the same older knowledge.db at once, and the second to change it would fail or
/// change it twice (#213). The read first keeps a current file free of the lock; a step's
/// transaction holds it already (it is immediate), and anywhere else one is taken here.
fn migrate(k: &Connection, needed: &str, change: &str) -> Result<()> {
    let is = |c: &Connection| c.query_row(needed, [], |r| r.get::<_, bool>(0));
    if !is(k)? {
        return Ok(());
    }
    if !k.is_autocommit() {
        return Ok(k.execute_batch(change)?);
    }
    let tx = rusqlite::Transaction::new_unchecked(k, rusqlite::TransactionBehavior::Immediate)?;
    if is(&tx)? {
        tx.execute_batch(change)?;
    }
    Ok(tx.commit()?)
}

pub(crate) fn schema(k: &Connection) -> Result<()> {
    // Before #192 a queued window was keyed by its claim op alone, so it could not be split: the
    // table is made again with its rows, in one savepoint (a step's transaction may hold it).
    migrate(
        k,
        "SELECT count(*) = 2 FROM pragma_table_info('recurate') WHERE pk > 0",
        &format!(
            "SAVEPOINT recurate_key;
             ALTER TABLE recurate RENAME TO recurate_old;
             {RECURATE};
             INSERT INTO recurate SELECT device, from_seq, to_seq, op_device, op_seq
               FROM recurate_old;
             DROP TABLE recurate_old;
             RELEASE recurate_key;"
        ),
    )?;
    k.execute_batch(RECURATE)?;
    k.execute_batch(
        "-- Every derivation of a claim: one claim op each (op_device, op_seq).
         CREATE TABLE IF NOT EXISTS derivations(
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, uid TEXT NOT NULL,
           ts INTEGER NOT NULL, tier INTEGER NOT NULL, recipe TEXT NOT NULL,
           kind TEXT NOT NULL, status TEXT NOT NULL, speaker TEXT NOT NULL, scope TEXT NOT NULL,
           repo TEXT, body TEXT NOT NULL, valid_from INTEGER NOT NULL,
           anchor_device TEXT NOT NULL, anchor_seq INTEGER NOT NULL,
           PRIMARY KEY (op_device, op_seq)
         );
         CREATE INDEX IF NOT EXISTS derivations_uid ON derivations(uid);
         -- `decisions` walks a repository's newest first and stops at its limit.
         CREATE INDEX IF NOT EXISTS derivations_repo
           ON derivations(repo, valid_from, anchor_device, anchor_seq);
         -- Each derivation's quotes, where they are in raw.
         CREATE TABLE IF NOT EXISTS evidence(
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, idx INTEGER NOT NULL,
           device TEXT NOT NULL, seq INTEGER NOT NULL, offset INTEGER NOT NULL,
           length INTEGER NOT NULL, sentence INTEGER NOT NULL, quote TEXT NOT NULL,
           claim_at INTEGER,
           PRIMARY KEY (op_device, op_seq, idx)
         );
         CREATE INDEX IF NOT EXISTS evidence_anchor ON evidence(device, seq);
         -- What a derivation supersedes: it counts while that derivation is active.
         CREATE TABLE IF NOT EXISTS edges(
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, to_uid TEXT NOT NULL,
           type TEXT NOT NULL,
           PRIMARY KEY (op_device, op_seq, to_uid)
         );
         CREATE INDEX IF NOT EXISTS edges_to ON edges(to_uid);
         -- Each uid's active derivation: the highest tier, then the newest (MUST-M18).
         CREATE TABLE IF NOT EXISTS claims(
           rowid INTEGER PRIMARY KEY, uid TEXT NOT NULL UNIQUE,
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS claims_op ON claims(op_device, op_seq);
         -- The active derivations' bodies and quotes, for Task 7's candidates.
         CREATE VIRTUAL TABLE IF NOT EXISTS claims_fts USING fts5(text, tokenize='trigram');
         -- The owner's corrections (spec 3.4, MUST-M21): each field the newest one gives
         -- applies over whatever derivation of the uid is active, so it survives recuration,
         -- re-derivation and rebuild. One for a uid with no claim yet waits for it.
         CREATE TABLE IF NOT EXISTS corrections(
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, ts INTEGER NOT NULL,
           uid TEXT NOT NULL, status TEXT, body TEXT,
           PRIMARY KEY (op_device, op_seq)
         );
         CREATE INDEX IF NOT EXISTS corrections_uid ON corrections(uid, ts);
         -- Each claim as it is now: its active derivation with the owner's corrections over it.
         CREATE VIEW IF NOT EXISTS active AS
           SELECT c.uid, d.kind, d.speaker, d.scope, d.repo, d.valid_from, d.anchor_device,
             d.anchor_seq,
             COALESCE((SELECT x.status FROM corrections x WHERE x.uid = c.uid
                       AND x.status IS NOT NULL
                       ORDER BY x.ts DESC, x.op_device DESC, x.op_seq DESC LIMIT 1),
                      d.status) AS status,
             COALESCE((SELECT x.body FROM corrections x WHERE x.uid = c.uid
                       AND x.body IS NOT NULL
                       ORDER BY x.ts DESC, x.op_device DESC, x.op_seq DESC LIMIT 1),
                      d.body) AS body
           FROM claims c JOIN derivations d ON d.op_device = c.op_device AND d.op_seq = c.op_seq;
         -- Claim ops that gave no claim, and why: doctor counts them.
         CREATE TABLE IF NOT EXISTS claim_skips(
           op_device TEXT NOT NULL, op_seq INTEGER NOT NULL, reason TEXT NOT NULL,
           PRIMARY KEY (op_device, op_seq)
         );",
    )?;
    // Before #125 a quote kept no `claim_at`: the column is added, empty, as those ops had none.
    migrate(
        k,
        "SELECT count(*) = 0 FROM pragma_table_info('evidence') WHERE name = 'claim_at'",
        "ALTER TABLE evidence ADD COLUMN claim_at INTEGER",
    )
}

/// The body of a correction op, which `oboete correct` writes (spec 3.4, MUST-M21): the owner's
/// status or body for a claim, by uid and by the raw record the claim anchors on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorrectionOp {
    pub uid: String,
    pub anchor: Anchor,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
}

/// A raw record: a claim's first quote's event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub device: String,
    pub seq: i64,
}

/// A claim body, at most (spec 6.5).
pub const MAX_BODY_CHARS: usize = 1_000;

impl CorrectionOp {
    /// Why it corrects nothing, if it does not: a synced op may come from any device.
    pub fn fault(&self) -> Option<&'static str> {
        let uid = self.uid.len() == 64 && self.uid.bytes().all(|b| b.is_ascii_hexdigit());
        if !uid {
            return Some("not a claim uid");
        }
        if self.status.is_none() && self.body.is_none() {
            return Some("corrects nothing");
        }
        if self
            .status
            .as_deref()
            .is_some_and(|s| !STATUSES.contains(&s))
        {
            return Some("an unknown status");
        }
        match self.body.as_deref() {
            Some(b) if b.trim().is_empty() => Some("an empty body"),
            Some(b) if b.chars().count() > MAX_BODY_CHARS => Some("over the 1,000-character cap"),
            _ => None,
        }
    }
}

/// `oboete correct`: the owner's correction of the claim `uid`, appended as a correction op. The
/// body goes through the same gate as every stored string; the claims consumer applies it, and
/// this returns once it has (at most 10 seconds, else an error that says it is recorded).
pub fn correct(
    home: &std::path::Path,
    uid: &str,
    status: Option<&str>,
    body: Option<&str>,
) -> Result<()> {
    use rusqlite::OptionalExtension;
    let rules = crate::capture::Settings::load(home)?.rules;
    // raw.db first, as every reader of knowledge.db holds it (a rebuild's swap waits for it).
    let mut raw = crate::raw::open(home)?;
    let k = crate::knowledge::open(home)?;
    schema(&k)?;
    let anchor = k
        .query_row(
            "SELECT anchor_device, anchor_seq FROM active WHERE uid = ?1",
            [uid],
            |r| {
                Ok(Anchor {
                    device: r.get(0)?,
                    seq: r.get(1)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("no claim has the uid {uid}"))?;
    let op = CorrectionOp {
        uid: uid.to_owned(),
        anchor,
        status: status.map(str::to_owned),
        // As a typed prompt is stored: private blocks out (an unclosed one hides the rest), then
        // the scanners.
        body: body.map(|b| crate::redact::scan(&crate::hook::strip_blocks(b, true), &rules).0),
    };
    if let Some(why) = op.fault() {
        anyhow::bail!("the correction is refused: {why}");
    }
    let seqs = raw.append_ops(&[(crate::raw::OpKind::Correction, serde_json::to_value(&op)?)])?;
    let device = raw.device().to_owned();
    drop(raw);
    // A search or a SessionStart right after never shows the old claim.
    applied(home, &k, &device, &seqs, "correction")
}

/// The owner's ops `seqs`, applied before the command returns: by this process or by the worker
/// already running (which wakes on the new op). At most 10 seconds, else an error that says the
/// `what` is recorded.
fn applied(
    home: &std::path::Path,
    k: &Connection,
    device: &str,
    seqs: &[i64],
    what: &str,
) -> Result<()> {
    crate::worker::run_once(home)?;
    let at = seqs.last().copied().unwrap_or(0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let done = || -> Result<bool> {
        let got = crate::knowledge::checkpoint::get_in(
            k,
            crate::knowledge::checkpoint::OPS,
            "claims",
            device,
        )?;
        Ok(got >= at)
    };
    while !done()? {
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("the {what} is recorded; the worker applies it when it next runs");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(())
}

/// `oboete pref add`: the owner's directive as an event, and a claim op quoting it whole, a
/// decided preference of global scope (spec 3.3: global scope only through this or the viewer).
/// Returns its uid, once the claims consumer has applied it: the next SessionStart shows it.
// ponytail: two appends; a crash between them leaves the directive event with no claim (run it
// again). One transaction when raw can append an event and ops together.
pub fn pref_add(home: &std::path::Path, text: &str) -> Result<String> {
    let settings = crate::capture::Settings::load(home)?;
    let c = crate::capture::directive(text, crate::db::now_ms(), &settings);
    // What the gate stores: the quote reads verbatim there. Checked before either append, so a
    // preference that cannot be a claim leaves no event behind.
    let quote = crate::curate::long_text(&c.event).unwrap_or_default();
    if quote.trim().is_empty() {
        anyhow::bail!("nothing is left to record once the <private> parts are removed");
    }
    if quote.chars().count() > MAX_BODY_CHARS {
        anyhow::bail!(
            "a preference can be at most {MAX_BODY_CHARS} characters; this one has {}",
            quote.chars().count()
        );
    }
    // raw.db first, as every reader of knowledge.db holds it (a rebuild's swap waits for it).
    let mut raw = crate::raw::open(home)?;
    let k = crate::knowledge::open(home)?;
    let seq = raw.append_with_ledger(&c.event, &c.ledger, settings.rules.version())?;
    let evidence = Evidence {
        device: raw.device().to_owned(),
        seq,
        offset: 0,
        length: i64::try_from(quote.len())?,
        sentence: 0,
        quote: quote.clone(),
        claim_at: None,
    };
    let uid = uid("preference", &evidence);
    let op = ClaimOp {
        id: "p1".into(),
        kind: "preference".into(),
        status: "decided".into(),
        speaker: "user".into(),
        scope: "global".into(),
        body: quote,
        evidence: vec![evidence],
        supersedes: Vec::new(),
        recipe: "oboete pref add".into(),
        tier: 0,
        why: String::new(),
        tainted: false,
    };
    let seqs = raw.append_ops(&[(crate::raw::OpKind::Claim, serde_json::to_value(op)?)])?;
    let device = raw.device().to_owned();
    drop(raw);
    applied(home, &k, &device, &seqs, "preference")?;
    Ok(uid)
}

/// A current claim.
#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    pub uid: String,
    pub kind: String,
    pub status: String,
    pub speaker: String,
    pub scope: String,
    pub body: String,
    pub valid_from: i64,
    /// Its first quote's event.
    pub device: String,
    pub seq: i64,
    /// For an earlier claim that is delivered (spec 3.4, `delivered`): the newest of the later
    /// claims whose curator links ended it. None for a current claim.
    pub later: Option<String>,
}

/// Whether `uid`'s first quote (its active derivation's) starts before `offset` in its record: in
/// the part of a split record that an earlier window read.
pub fn quoted_before(k: &Connection, uid: &str, offset: i64) -> Result<bool> {
    Ok(k.query_row(
        "SELECT e.offset < ?2 FROM claims c
         JOIN evidence e ON e.op_device = c.op_device AND e.op_seq = c.op_seq AND e.idx = 0
         WHERE c.uid = ?1",
        rusqlite::params![uid, offset],
        |r| r.get(0),
    )?)
}

/// Derivation `d`, with its first quote `q` (its `evidence` row of `idx = 0`), is anchored in a
/// window: on the window's `:device`, in its records `:from_seq` to `:to_seq`, and in a split
/// record in the window's part (`:from_offset`, `:to_offset`, NULL for a whole record). A
/// recuration of the window retracts what it leaves out of these (`curate::anchored_in`) and reads
/// the claims without them (`before_window`, #249), so both use this one span; `window_params`
/// binds it.
pub(crate) const IN_WINDOW: &str = "d.anchor_device = :device
        AND d.anchor_seq BETWEEN :from_seq AND :to_seq
        AND (q.seq <> :from_seq OR :from_offset IS NULL OR q.offset >= :from_offset)
        AND (q.seq <> :to_seq OR :to_offset IS NULL OR q.offset + q.length <= :to_offset)";

/// `IN_WINDOW`'s parameters for window `w`.
pub(crate) fn window_params(
    w: &crate::curate::Window,
) -> [(&'static str, &dyn rusqlite::ToSql); 5] {
    [
        (":device", &w.device),
        (":from_seq", &w.from_seq),
        (":to_seq", &w.to_seq),
        (":from_offset", &w.from_offset),
        (":to_offset", &w.to_offset),
    ]
}

/// Claim `a` (a row of the `active` view) as the window of `IN_WINDOW` found it before it was
/// curated (#249): not one of the window's own claims (its active derivation anchored in the
/// window), and no active derivation of another uid supersedes or retracts it but the window's
/// own, which a recuration replaces. A window curated for the first time has none.
fn before_window() -> String {
    // Claim `c`'s active derivation is anchored in the window.
    let own = format!(
        "EXISTS (SELECT 1 FROM derivations d
           JOIN evidence q ON q.op_device = d.op_device AND q.op_seq = d.op_seq AND q.idx = 0
           WHERE d.op_device = c.op_device AND d.op_seq = c.op_seq AND {IN_WINDOW})"
    );
    format!(
        "a.status <> 'retracted'
         AND NOT EXISTS (SELECT 1 FROM claims c WHERE c.uid = a.uid AND {own})
         AND NOT EXISTS (
           SELECT 1 FROM edges e
           JOIN claims c ON c.op_device = e.op_device AND c.op_seq = e.op_seq
           WHERE e.to_uid = a.uid AND c.uid <> a.uid AND NOT {own})"
    )
}

/// When `uid` is a current claim as window `w` found it (`before_window`), its repository (`None`
/// for one anchored outside any) and its active derivation.
pub fn tip(
    k: &Connection,
    uid: &str,
    w: &crate::curate::Window,
) -> Result<Option<(Option<String>, Claim)>> {
    use rusqlite::OptionalExtension;
    schema(k)?;
    Ok(k.query_row(
        &format!(
            "SELECT a.repo, a.uid, a.kind, a.status, a.speaker, a.scope, a.body, a.valid_from,
                    a.anchor_device, a.anchor_seq
             FROM active a WHERE a.uid = :uid AND {}",
            before_window()
        ),
        &[
            &window_params(w)[..],
            &[(":uid", &uid as &dyn rusqlite::ToSql)],
        ]
        .concat()[..],
        |r| {
            let claim = Claim {
                uid: r.get(1)?,
                kind: r.get(2)?,
                status: r.get(3)?,
                speaker: r.get(4)?,
                scope: r.get(5)?,
                body: r.get(6)?,
                valid_from: r.get(7)?,
                device: r.get(8)?,
                seq: r.get(9)?,
                later: None,
            };
            Ok((r.get(0)?, claim))
        },
    )
    .optional()?)
}

/// `repo`'s current claims: the chain tips (no active derivation supersedes or retracts them)
/// that are not retracted, in spec 3.4's order, (valid_from, device, seq), with the uid last so
/// two claims of one event keep one order on every device (MUST-M7).
pub fn current(k: &Connection, repo: &str) -> Result<Vec<Claim>> {
    tips(
        k,
        &format!("{TIPS} ORDER BY a.valid_from, a.anchor_device, a.anchor_seq, a.uid"),
        (repo,),
    )
}

/// `current`, as window `w` found it (`before_window`): to a recuration, what its earlier curation
/// superseded is current, and what it derived is not.
pub fn current_before(k: &Connection, repo: &str, w: &crate::curate::Window) -> Result<Vec<Claim>> {
    tips(
        k,
        &format!(
            "SELECT a.uid, a.kind, a.status, a.speaker, a.scope, a.body, a.valid_from,
                    a.anchor_device, a.anchor_seq, NULL
             FROM active a WHERE a.repo = :repo AND {}
             ORDER BY a.valid_from, a.anchor_device, a.anchor_seq, a.uid",
            before_window()
        ),
        &[
            &window_params(w)[..],
            &[(":repo", &repo as &dyn rusqlite::ToSql)],
        ]
        .concat()[..],
    )
}

/// The current claims of no repository, in `current`'s order: `oboete pref add`'s preferences,
/// whose event records no checkout.
pub fn global(k: &Connection) -> Result<Vec<Claim>> {
    let sql = TIPS.replacen("a.repo = ?1", "a.repo IS NULL", 1);
    tips(
        k,
        &format!("{sql} ORDER BY a.valid_from, a.anchor_device, a.anchor_seq, a.uid"),
        [],
    )
}

/// `repo`'s delivered claims (`delivered`) of spec 4.4's kinds, decided or open items not done: at
/// most `limit`, the newest first (`current`'s order reversed). SessionStart chooses among these
/// at every start, so the filter, the order and the limit are the query's.
pub fn decisions(k: &Connection, repo: &str, limit: usize) -> Result<Vec<Claim>> {
    tips(
        k,
        &format!(
            "{} {DECIDED}",
            delivered("a.repo = ?1", LINKS_END_DECISIONS)
        ),
        (repo, limit as i64),
    )
}

/// Spec 3.4's switch (D2 of milestone 4's plan): whether a curator link from a later claim the
/// owner backs ends an earlier decision, as every other end does. False until curator links pass
/// milestone 3's control line; the PR that records that result sets it. A constant, not a
/// setting: it follows a measurement, not a preference.
pub const LINKS_END_DECISIONS: bool = false;

/// Claim `t`, a row of the `active` view, is backed by the owner: the user's own words, a proposal
/// the user accepted, or a claim whose status the owner corrected to decided (the active status is
/// then the newest correction's).
fn owner_backed(t: &str) -> String {
    format!(
        "({t}.speaker = 'user'
          OR {t}.status = 'decided' AND ({t}.speaker = 'assistant proposal'
             OR EXISTS (SELECT 1 FROM corrections oc WHERE oc.uid = {t}.uid
                        AND oc.status IS NOT NULL)))"
    )
}

/// Spec 3.4's delivered claims among the rows of the `active` view `a` that `which` selects, with
/// `TIPS`' columns: the chain tips, and each decided decision or preference whose every end is a
/// curator link from a later claim the owner backs, decided or done (D1): the user's own words, a
/// proposal the user accepted, or a claim the owner corrected (`owner_backed`). Such a claim's
/// `later` is the newest of those claims. Spec 3.4's other ends stay ends: a retraction, a proposal
/// (the one an acceptance replaced among them) or an open item is not a decided decision or
/// preference; and a link from a retraction, from a claim of the same time (a restatement quotes
/// the same words, #261) or from a claim the owner does not back still ends a decision. With
/// `links_end`, every link ends it: the tips.
pub(crate) fn delivered(which: &str, links_end: bool) -> String {
    let linkers = LINKERS;
    let ends = ends(links_end);
    format!(
        "SELECT a.uid, a.kind, a.status, a.speaker, a.scope, a.body, a.valid_from,
                a.anchor_device, a.anchor_seq,
                (SELECT l.uid {linkers}
                 ORDER BY l.valid_from DESC, l.anchor_device DESC, l.anchor_seq DESC, l.uid DESC
                 LIMIT 1)
         FROM active a
         WHERE {which} AND a.status <> 'retracted'
           AND NOT EXISTS (SELECT 1 {linkers} AND {ends})"
    )
}

/// Whether the link from `l` (edge `e`) ends claim `a` (`delivered`): every link does with
/// `links_end`.
fn ends(links_end: bool) -> String {
    if links_end {
        return "1".to_owned();
    }
    format!(
        "NOT (a.kind IN ('decision', 'preference') AND a.status = 'decided'
              AND e.type = 'supersedes' AND l.status IN ('decided', 'done')
              AND {} AND l.valid_from > a.valid_from)",
        owner_backed("l")
    )
}

/// Claim `uid` if it is delivered (`delivered`): a tip, or an earlier decision only curator links
/// ended, with the newest of them as its `later`.
pub fn delivered_one(k: &Connection, uid: &str) -> Result<Option<Claim>> {
    Ok(tips(k, &delivered("a.uid = ?1", LINKS_END_DECISIONS), [uid])?.pop())
}

/// Claim `uid` as the `active` view holds it, delivered or not, with the newest claim whose link
/// ended it as its `later` (MUST-M11: search names it), or, when no link ended it, the newest
/// linker, as `delivered` names it (Codex on #306).
pub fn active_one(k: &Connection, uid: &str) -> Result<Option<Claim>> {
    let newest = "ORDER BY l.valid_from DESC, l.anchor_device DESC, l.anchor_seq DESC, l.uid DESC
                  LIMIT 1";
    let ends = ends(LINKS_END_DECISIONS);
    Ok(tips(
        k,
        &format!(
            "SELECT a.uid, a.kind, a.status, a.speaker, a.scope, a.body, a.valid_from,
                    a.anchor_device, a.anchor_seq,
                    COALESCE((SELECT l.uid {LINKERS} AND {ends} {newest}),
                             (SELECT l.uid {LINKERS} {newest}))
             FROM active a WHERE a.uid = ?1"
        ),
        [uid],
    )?
    .pop())
}

/// The active claims `l` whose derivation links claim `a` (a row of the `active` view).
pub(crate) const LINKERS: &str = "FROM edges e
           JOIN claims x ON x.op_device = e.op_device AND x.op_seq = e.op_seq
           JOIN active l ON l.uid = x.uid
           WHERE e.to_uid = a.uid AND x.uid <> a.uid";

/// Spec 3.4's pair rule (D2), which every surface applies to the delivered claims it ranked:
/// each claim of `ranked` (the most relevant first) with the later claims that ended it, one unit
/// per chain's newest claim, listed newest first inside it, so an earlier claim is read after the
/// claim that may overturn it. Units come in the order of their first claim in `ranked`. A unit
/// is left out whole when `hidden` says so of one of its claims, or when its chain meets a claim
/// that is not delivered itself: an earlier claim is never shown without the claim that ended it.
pub fn units(
    k: &Connection,
    ranked: &[Claim],
    hidden: impl Fn(&str) -> Result<bool>,
) -> Result<Vec<Vec<Claim>>> {
    let sql = delivered("a.uid = ?1", LINKS_END_DECISIONS);
    let mut known: std::collections::HashMap<String, Option<Claim>> =
        std::collections::HashMap::new();
    let mut units: Vec<(String, Vec<Claim>, bool)> = Vec::new();
    for c in ranked {
        // The claim, then each later claim that ended the one before, to the chain's newest.
        let mut chain = vec![c.clone()];
        let mut whole = true;
        while let Some(later) = chain.last().and_then(|c| c.later.clone()) {
            if !known.contains_key(&later) {
                let found = tips(k, &sql, [&later])?.pop();
                known.insert(later.clone(), found);
            }
            match &known[&later] {
                // Strictly later each time (`delivered`), so the walk ends; a loop is a broken chain.
                Some(l) if !chain.iter().any(|c| c.uid == l.uid) => chain.push(l.clone()),
                _ => {
                    whole = false;
                    break;
                }
            }
        }
        let mut shown = whole;
        for c in &chain {
            shown = shown && !hidden(&c.uid)?;
        }
        let root = chain.last().map(|c| c.uid.clone()).unwrap_or_default();
        let at = match units.iter().position(|(r, _, _)| *r == root) {
            Some(at) => at,
            None => {
                units.push((root, Vec::new(), true));
                units.len() - 1
            }
        };
        let unit = &mut units[at];
        unit.2 = unit.2 && shown;
        for c in chain {
            if !unit.1.iter().any(|u| u.uid == c.uid) {
                unit.1.push(c);
            }
        }
    }
    Ok(units
        .into_iter()
        .filter(|(_, _, shown)| *shown)
        .map(|(_, mut unit, _)| {
            unit.sort_by(|a, b| newest(b).cmp(&newest(a)));
            unit
        })
        .collect())
}

/// The key claims are ordered by (MUST-M7): (valid_from, device, seq), then the uid.
fn newest(c: &Claim) -> (i64, &str, i64, &str) {
    (c.valid_from, &c.device, c.seq, &c.uid)
}

/// The whole units of `units` that fit in `places` claims, in their order, and the rest (D2): a
/// pair takes two places, and one that does not fit goes to the rest whole.
pub fn place(units: Vec<Vec<Claim>>, places: usize) -> (Vec<Vec<Claim>>, Vec<Vec<Claim>>) {
    let mut left = places;
    let (mut fit, mut rest) = (Vec::new(), Vec::new());
    for unit in units {
        if unit.len() <= left {
            left -= unit.len();
            fit.push(unit);
        } else {
            rest.push(unit);
        }
    }
    (fit, rest)
}

/// What raw.db holds that the worker has not applied to the claims yet (spec 4.1, D3): the uids of
/// the owner's corrections past the claims consumer, and the records of tombstones past the
/// anchors consumer. A reader of knowledge.db leaves out a claim either touches until the worker
/// has applied it, as the manifest's read leaves out a text a tombstone may touch: a claim the
/// owner retracted, or one quoting what the owner removed, is never shown.
#[derive(Debug, Default)]
pub struct Pending {
    uids: std::collections::HashSet<String>,
    records: std::collections::HashSet<(String, i64)>,
}

impl Pending {
    /// Read-only, as a hook reads: `k` is a knowledge.db the worker has run on.
    /// Each consumer's own devices: a device it never steps has no checkpoint to be past.
    pub fn read(raw: &crate::raw::Raw, k: &Connection) -> Result<Self> {
        use crate::consumer::claims::{Anchors, Claims};
        use crate::knowledge::checkpoint;
        use crate::worker::Consumer;
        let mut p = Self::default();
        for device in Claims.devices(raw)? {
            let at = checkpoint::get_in(k, Claims.checkpoints(), Claims.name(), &device)?;
            for body in raw.ops_of(crate::raw::OpKind::Correction, &device, at)? {
                if let Some(uid) = body["uid"].as_str() {
                    p.uids.insert(uid.to_owned());
                }
            }
        }
        for device in Anchors.devices(raw)? {
            let at = checkpoint::get_in(k, Anchors.checkpoints(), Anchors.name(), &device)?;
            p.records.extend(raw.tombstones_after(&device, at)?);
        }
        Ok(p)
    }

    /// Whether they touch the claim `uid`: its uid, or a quote of its active derivation.
    pub fn touches(&self, k: &Connection, uid: &str) -> Result<bool> {
        if self.uids.contains(uid) {
            return Ok(true);
        }
        if self.records.is_empty() {
            return Ok(false);
        }
        let mut st = k.prepare_cached(
            "SELECT q.device, q.seq FROM claims c
             JOIN evidence q ON q.op_device = c.op_device AND q.op_seq = c.op_seq
             WHERE c.uid = ?1",
        )?;
        let quotes = st.query_map([uid], |r| Ok((r.get(0)?, r.get(1)?)))?;
        for q in quotes {
            if self.records.contains(&q?) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// `units` listed newest first, each by its newest claim (spec 4.4): a claim dated between the two
/// claims of a pair never separates them.
pub fn newest_first(units: &mut [Vec<Claim>]) {
    units.sort_by(|a, b| b.iter().map(newest).max().cmp(&a.iter().map(newest).max()));
}

/// `repo`'s current claims the owner backs, anchored on `device` at or before `seq`, the newest
/// first, at most `limit`: those a session's digest may cite (milestone 3 Task 9). SessionStart
/// injects the digest, so only the user's own settled words, proposals the user accepted, and
/// claims whose status the owner corrected to decided (the active status is then the newest
/// correction's) are shown: never a proposal, which may stand on tool content, nor a tool result
/// or the assistant's own completion, which a passing run settles.
pub fn anchored_through(
    k: &Connection,
    repo: &str,
    device: &str,
    seq: i64,
    limit: usize,
) -> Result<Vec<Claim>> {
    tips(
        k,
        &format!(
            "{TIPS} AND a.status NOT IN ('proposed', 'unverified') AND {}
             AND a.anchor_device = ?2 AND a.anchor_seq <= ?3
             ORDER BY a.valid_from DESC, a.anchor_device DESC, a.anchor_seq DESC, a.uid DESC
             LIMIT ?4",
            owner_backed("a")
        ),
        (repo, device, seq, limit as i64),
    )
}

/// `decisions`' filter, order and limit (`?2`), which `derivations_repo` serves in order: spec
/// 4.4's kinds (fixes, changes and repo facts are for search, D4).
pub(crate) const DECIDED: &str = "AND a.kind IN ('decision', 'preference', 'open item', 'lesson')
     AND (a.status = 'decided' OR (a.kind = 'open item' AND a.status <> 'done'))
     ORDER BY a.valid_from DESC, a.anchor_device DESC, a.anchor_seq DESC, a.uid DESC
     LIMIT ?2";

/// A repository's current claims, over the `active` view; `?1` is the repository. A claim `a` is
/// current as a chain tip (no active derivation of another uid supersedes or retracts it) that is
/// not retracted, by its derivation or the owner. A tip has no `later` claim.
pub(crate) const TIPS: &str =
    "SELECT a.uid, a.kind, a.status, a.speaker, a.scope, a.body, a.valid_from,
            a.anchor_device, a.anchor_seq, NULL
     FROM active a
     WHERE a.repo = ?1 AND a.status <> 'retracted'
       AND NOT EXISTS (
         SELECT 1 FROM edges e
         JOIN claims x ON x.op_device = e.op_device AND x.op_seq = e.op_seq
         WHERE e.to_uid = a.uid AND x.uid <> a.uid)";

fn tips(k: &Connection, sql: &str, params: impl rusqlite::Params) -> Result<Vec<Claim>> {
    // Cached: `units` runs one lookup per later claim, and compiling the query costs more than it.
    let mut st = k.prepare_cached(sql)?;
    let rows = st.query_map(params, |r| {
        Ok(Claim {
            uid: r.get(0)?,
            kind: r.get(1)?,
            status: r.get(2)?,
            speaker: r.get(3)?,
            scope: r.get(4)?,
            body: r.get(5)?,
            valid_from: r.get(6)?,
            device: r.get(7)?,
            seq: r.get(8)?,
            later: r.get(9)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
