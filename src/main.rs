//! oboete — lightweight, single-binary memory for coding agents.
//!
//! M0 spike: Claude Code hook capture → SQLite → summarizer chain with
//! fallback → SessionStart injection. See docs/plan.md.

mod backup;
mod budget;
mod capture;
mod cards;
mod claims;
mod codex_probe;
mod config;
mod consumer;
#[cfg(test)]
mod crash;
mod curate;
mod db;
mod dispatch;
mod embed;
#[cfg(feature = "local-embed")]
mod embed_local;
mod embed_phase;
mod executable;
mod failure;
mod forget;
mod gates;
mod hook;
mod hookstate;
mod import;
mod isolation;
mod keyfile;
mod knowledge;
mod manifest;
mod mcp;
mod migrate;
mod model_fetch;
mod provider;
mod providers_db;
// Design B's store: the hook writes to it; its readers come with the worker (milestone 2 Task 5).
#[allow(dead_code)]
mod raw;
mod redact;
mod replay;
mod repo;
mod resident;
mod search;
mod settings;
mod setup;
mod shortlist;
mod transcript;
mod turns;
mod view;
mod work_state;
mod worker;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "oboete", version, about = "Memory for coding agents")]
struct Cli {
    /// Data directory (default: ~/.oboete)
    #[arg(long, global = true, env = "OBOETE_HOME")]
    home: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum PrefCmd {
    /// Keep a preference that applies in every repository
    Add { text: String },
}

#[derive(Subcommand)]
enum Cmd {
    /// Receive one agent hook event on stdin and store it (fail-open, always exit 0)
    Hook {
        /// Agent name: claude | codex | grok | agy | opencode | pi | cursor
        agent: String,
        /// Hook event name (e.g. SessionStart, PreInvocation, UserPromptSubmit, PostToolUse, Stop, PreCompact, SessionEnd)
        event: String,
    },
    /// Run Design B's consumers over raw.db (hooks start it; one per home). It exits when idle,
    /// or stays where config.toml says `[worker] resident = true`
    Worker {
        /// Exit after this long without a new record, whatever config.toml says
        #[arg(long)]
        idle_ms: Option<u64>,
    },
    /// Rebuild knowledge.db (claims, cards, summaries, indexes, manifests) from raw.db and its op log,
    /// with no AI call
    Rebuild,
    /// Register an irreversible forget of raw records with native import identities (physical
    /// purge remains unfinished; legacy/hook raw is not supported by this first slice)
    Forget {
        /// Raw id from search: <device>:<seq>
        #[arg(long, conflicts_with_all = ["span", "status"])]
        record: Option<String>,
        /// This device's raw span: <device>:<from>-<to> (at most 500 records)
        #[arg(long, conflicts_with_all = ["record", "status"])]
        span: Option<String>,
        /// A claim's uid, or an imported document's, from search or get
        #[arg(long, conflicts_with_all = ["record", "span", "status"])]
        uid: Option<String>,
        /// Print the preview and skip its confirmation
        #[arg(long, conflicts_with = "status")]
        yes: bool,
        /// The requests raw.db holds, after the request logs are reconciled with it
        #[arg(long)]
        status: bool,
    },
    /// Curate again what was curated before: the spans queued since (a quote a new rule masked
    /// or a forget removed), the windows every provider skipped, the imported records of a
    /// source, or a span you name. It lists the windows and an estimate; nothing is sent without
    /// --yes
    Recurate {
        /// The windows every provider skipped
        #[arg(long, conflicts_with = "span")]
        skipped: bool,
        /// The imported records of this source, which curation leaves aside: oboete-v1 or
        /// transcript
        #[arg(long, conflicts_with_all = ["skipped", "span"])]
        source: Option<String>,
        /// A span of this device's records, as <device>:<from>-<to>
        span: Option<String>,
        /// Send them
        #[arg(long)]
        yes: bool,
    },
    /// Correct a remembered claim by its uid: your status or text holds over whatever curation
    /// derives for it, now and after any recuration or rebuild
    Correct {
        /// The claim's uid (as `oboete claims` lists it)
        uid: String,
        /// decided, proposed, done or retracted
        #[arg(long, required_unless_present = "body")]
        status: Option<String>,
        /// The claim's text as it should read (at most 1,000 characters)
        #[arg(long)]
        body: Option<String>,
    },
    /// Keep a claim searchable without injecting it into an agent's context
    Mute { uid: String },
    /// Inject a muted claim again
    Unmute { uid: String },
    /// List the current claims of the repository in the current directory, each with the uid
    /// `oboete correct` takes
    Claims,
    /// Keep a repository's sessions from every curator and embedder: nothing of a session that
    /// touched it is sent out from now on (what was sent before stays sent). The repository in
    /// the current directory unless one is named
    Exclude {
        /// The repository as oboete labels it (github.com/<owner>/<name>, or its path)
        repo: Option<String>,
        /// Take it back out of the list: its new records can be sent again
        #[arg(long)]
        undo: bool,
    },
    /// Rebuild raw.db from the backup segments (MUST-M15); the current file is kept aside.
    /// The worker does this by itself when raw.db is damaged.
    Restore,
    /// Print the context a new session in the current directory gets (OpenCode's plugin reads it)
    Inject {
        /// The session it is for, so it is not listed among the other active sessions
        #[arg(long)]
        session: Option<String>,
        /// OpenCode's context packet, acknowledged only after SDK insertion
        #[arg(long, hide = true, requires = "session", conflicts_with = "prompt")]
        json: bool,
        /// Print instead the claims a prompt read from stdin would get in --repo, each with the
        /// share of the prompt's trigrams its body holds: from --session's shortlists, or from
        /// every delivered claim; ignores [inject] and keeps no hook state
        #[arg(long, requires = "repo")]
        prompt: bool,
        #[arg(long, requires = "prompt")]
        repo: Option<String>,
        /// The share a claim's body must hold (default 0.53)
        #[arg(long, requires = "prompt")]
        threshold: Option<f64>,
    },
    /// Serve the memory as an MCP server on stdin/stdout (search / get / timeline tools)
    Mcp,
    /// Search what is remembered (this repository unless --all or --repo): the decisions and
    /// other claims first, then cards, session summaries and imported history, then raw records and the
    /// claims later ones ended
    Search {
        /// Words or a sentence. Ranked by the 3-character pieces they share; a 2-character word
        /// beside longer ones (同期, M5; not one of ASCII letters or of hiragana only) puts the hits that hold it
        /// first. A query too short for pieces matches its terms as literal substrings, all
        /// required. Put `--` before a term that starts with `-`
        query: Vec<String>,
        #[arg(long)]
        all: bool,
        /// A repository's key instead of this one's
        #[arg(long)]
        repo: Option<String>,
        /// Only what is dated at or after this (2026-09-30, or 2026-09-30T14:00; UTC unless it
        /// says `Z` or an offset)
        #[arg(long)]
        since: Option<String>,
        /// Only what is dated before this; a date runs to the end of its day
        #[arg(long)]
        until: Option<String>,
        /// Rank the claims later ones ended where their words rank them
        #[arg(long)]
        history: bool,
        /// Comma-separated categories or kinds (observations, sessions, prompts, claims, bugfix, ...)
        #[arg(long = "type")]
        kind: Option<String>,
        /// relevance (default), date_desc or date_asc
        #[arg(long, default_value = "relevance")]
        order: String,
        /// The raw records: below the rest (below), not at all (off), or alone (only)
        #[arg(long, default_value = "below")]
        raw: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// With the local embedder: load the model (a few seconds) and search by meaning too;
        /// without it, this search is full text
        #[arg(long)]
        vectors: bool,
    },
    /// Print 1–20 chosen ids in full: claims, cards, summaries, imports or raw records
    Get {
        #[arg(required = true, num_args = 1..)]
        ids: Vec<String>,
    },
    /// Claims, imported history and session starts, newest first (this repository unless --all)
    Timeline {
        #[arg(long)]
        all: bool,
        /// An id `get` takes: what is around its time, instead of the newest
        #[arg(long)]
        anchor: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Wire this binary into an agent's hooks (claude | codex | grok | agy | opencode | pi | cursor | all),
    /// or choose the embedder with --embeddings
    Setup {
        #[arg(required_unless_present = "embeddings")]
        agent: Option<String>,
        /// Take oboete's hook entries out again
        #[arg(long, requires = "agent", conflicts_with = "embeddings")]
        remove: bool,
        /// none (full-text search), local (bge-m3 on this machine, downloaded once) or workers-ai
        /// (Cloudflare): says what leaves the machine before anything changes
        #[arg(long, conflicts_with = "agent", value_parser = ["none", "local", "workers-ai"])]
        embeddings: Option<String>,
        /// Take the choice without asking (for scripts)
        #[arg(long, requires = "embeddings")]
        yes: bool,
    },
    /// Report hook wiring, stored data and provider readiness
    Doctor,
    /// Use a provider again after it stopped for the owner (claude's credits) or cooled down
    Resume { provider: String },
    /// A preference for every repository, in your own words (the only way to one besides the
    /// viewer)
    Pref {
        #[command(subcommand)]
        action: PrefCmd,
    },
    /// Browse the memory and change the settings in a browser: a page on 127.0.0.1 (prints its URL)
    View {
        /// Port to listen on (0 = any free port); in a resident home, without it, the resident
        /// viewer's address
        #[arg(long)]
        port: Option<u16>,
        /// Also open the page in the default browser
        #[arg(long)]
        open: bool,
        /// The resident viewer: on `[view] port`, with the token of state/view-token
        /// (docs/resident.md R5); what a resident worker starts
        #[arg(long, hide = true, conflicts_with_all = ["port", "open"])]
        resident: bool,
        /// Replace the resident viewer's token, and move it to a new address: the old bookmark
        /// stops opening the page
        #[arg(long, conflicts_with_all = ["port", "open", "resident"])]
        new_token: bool,
    },
    /// Import claude-mem's SQLite database or Claude Code and Codex transcripts
    Import {
        /// Source: claude-mem or transcripts
        source: String,
        /// Its database file (read-only; e.g. ~/.claude-mem/claude-mem.db)
        #[arg(required_if_eq("source", "claude-mem"))]
        db: Option<PathBuf>,
        /// The --home store is for evaluation, not the one the hooks write (required until PR-H)
        #[arg(long)]
        eval_store: bool,
        /// Import only this agent's transcripts (default: both)
        #[arg(long, value_parser = ["claude", "codex"])]
        agent: Option<String>,
        /// Import transcripts; without this flag, print a preview and write nothing
        #[arg(long)]
        yes: bool,
    },
    /// Move v1's store (oboete.db) into Design B (spec 7.4): its events as records, its documents
    /// as imported documents, its settings into a home that has none. v1's store is never written;
    /// run it again to import what v1 wrote since
    Migrate {
        /// v1's store (default: <home>/oboete.db)
        #[arg(long, conflicts_with = "finish")]
        from: Option<PathBuf>,
        /// Import once more, then list v1's old files and delete them if you answer yes
        #[arg(long)]
        finish: bool,
    },
    /// Evaluation (milestone 4 D12): each claim's label and evidence rows, whether each still
    /// reads in its record, as JSON: what M6's harness checks a cited span against
    #[command(hide = true)]
    Cite { uids: Vec<String> },
    /// Evaluation: pass stdin through the outbound gate (what may leave the machine) to stdout
    #[command(hide = true)]
    Gate,
    /// Evaluation: print a Claude Code or Codex transcript as a replay fixture
    #[command(hide = true)]
    Transcript {
        path: PathBuf,
        /// claude or codex
        #[arg(long)]
        agent: String,
    },
    /// Evaluation (milestone 4 Task 6): run `{"qid","text","session"}` JSONL questions through
    /// Design B's search, one TREC run per arm and the sidecar of what they print
    #[command(hide = true)]
    Eval {
        queries: PathBuf,
        #[arg(long, default_value_t = 50)]
        depth: usize,
        /// Comma-separated: off, below, only, rrf:<n>
        #[arg(long, default_value = "off")]
        arms: String,
        /// Directory for b-<arm>.trec and b-docs.jsonl
        #[arg(long)]
        out: PathBuf,
    },
    /// Task 10's spike: embed `{"id","text"}` lines from stdin with the local bge-m3
    #[cfg(feature = "local-embed")]
    #[command(hide = true)]
    EmbedSpike {
        /// The directory of the model's pinned files
        #[arg(long)]
        model: PathBuf,
        /// ONNX Runtime's threads (all the machine's when absent)
        #[arg(long)]
        threads: Option<usize>,
        /// Check every file's size and SHA-256 first
        #[arg(long)]
        verify: bool,
    },
    /// Replay a JSONL fixture through the hook path and measure
    Replay {
        fixture: PathBuf,
        /// Directory that stands in for the fixture's repository root
        #[arg(long)]
        repo_root: Option<PathBuf>,
        /// Also time N real `oboete hook` process spawns (startup + insert) per output size
        #[arg(long, default_value_t = 30)]
        spawn_sample: usize,
        /// Tool-output sizes of the spawned hooks, in KB (M14: 1,64,256)
        #[arg(long, value_delimiter = ',', default_value = "1")]
        sizes: Vec<usize>,
        /// Only replay events of this agent: claude | codex | grok | agy | opencode | pi | cursor | all
        #[arg(long, default_value = "claude")]
        agent: String,
        /// Also time N SessionStart and prompt hooks and N in-process reads of what SessionStart
        /// shows, before and after the consumers are drained (milestone 4 Task 0)
        #[arg(long, default_value_t = 0)]
        read_sample: usize,
        /// Spawns of each read hook run and left out before the N timed ones (milestone 4 D15)
        #[arg(long, default_value_t = 0)]
        read_warmup: usize,
    },
}

fn main() {
    executable::init();
    let cli = Cli::parse();
    let home = cli
        .home
        .unwrap_or_else(|| config::home_dir().join(".oboete"));
    let code = match run(cli.cmd, home) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("oboete: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

/// The current directory's repository key, or every repository with `--all`.
fn repo_filter(all: bool) -> Result<Option<String>> {
    Ok(if all {
        None
    } else {
        Some(repo::key(&std::env::current_dir()?))
    })
}

/// Listing output. Piped into `head`, stdout closes early; that is not an error. Anything
/// else (a full disk behind a redirect) is.
/// Stored text leaves through the egress gate: the user's rules as they are now (spec 6.4), so a
/// rule added after capture hides its value, labels included, before the rescan (Task 7b) has
/// tombstoned it.
fn emit(text: &str) -> Result<()> {
    use std::io::Write;
    match std::io::stdout()
        .lock()
        .write_all(redact::outbound(text).as_bytes())
    {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.into()),
        _ => Ok(()),
    }
}

fn run(cmd: Cmd, home: PathBuf) -> Result<()> {
    // R9's new image must validate the previous resident's home before even creating it.
    executable::check_home(
        &home,
        match &cmd {
            Cmd::Worker { .. } => Some(executable::Role::Worker),
            Cmd::View { resident: true, .. } => Some(executable::Role::Viewer),
            _ => None,
        },
    )?;
    // Hooks create storage after the skip guards; transcript preview creates nothing.
    let preview = matches!(&cmd, Cmd::Import { source, yes: false, .. } if source == "transcripts");
    if !matches!(&cmd, Cmd::Hook { .. }) && !preview {
        std::fs::create_dir_all(&home)?;
    }
    // The egress gate (`redact::outbound`) applies the user's rules as they are now (spec 6.4).
    // Hooks load them per call inside their fail-open boundary; doctor and setup report a
    // broken `[redaction]` table instead of stopping on it, and inject still prints the
    // recording-failure line such a table causes (OpenCode reads its context there).
    if !matches!(
        &cmd,
        Cmd::Hook { .. } | Cmd::Doctor | Cmd::Setup { .. } | Cmd::Inject { .. }
    ) {
        redact::set_home(&home)?;
    }
    match cmd {
        Cmd::Hook { agent, event } => {
            // Fail-open: a hook must never break the agent.
            if let Err(e) = hook::run_stdin(&home, &agent, &event) {
                eprintln!("oboete hook: {e:#}");
            }
            Ok(())
        }
        Cmd::Worker { idle_ms: Some(ms) } => worker::run(&home, ms),
        Cmd::Worker { idle_ms: None } => worker::run_default(&home),
        Cmd::Inject {
            session,
            prompt: true,
            repo,
            threshold,
            ..
        } => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
            let repo = repo.unwrap_or_default();
            let threshold = threshold.unwrap_or(shortlist::THRESHOLD);
            let shown = shortlist::report(&home, &repo, session.as_deref(), &text, threshold)?;
            print!("{shown}");
            Ok(())
        }
        Cmd::Inject { session, json, .. } => {
            let cwd = std::env::current_dir()?;
            if json {
                print!("{}", hook::inject_json(&home, &cwd, session.as_deref()));
            } else {
                print!("{}", hook::inject_text(&home, &cwd, session.as_deref()));
            }
            Ok(())
        }
        Cmd::Mcp => mcp::run(&home),
        Cmd::Search {
            query,
            all,
            repo,
            since,
            until,
            history,
            kind,
            order,
            raw,
            limit,
            vectors,
        } => {
            let q = search::b::Query {
                text: query.join(" "),
                caller: repo_filter(false)?,
                repo,
                all,
                since: since.map(|s| search::b::time(&s, false)).transpose()?,
                until: until.map(|s| search::b::time(&s, true)).transpose()?,
                history,
                types: kind.as_deref().map(str::parse).transpose()?,
                order: order.parse()?,
                raw: match raw.as_str() {
                    "below" => search::b::RawArm::Below,
                    "off" => search::b::RawArm::Off,
                    "only" => search::b::RawArm::Only,
                    other => anyhow::bail!("--raw {other}: use below, off or only"),
                },
                limit: limit.min(100),
                skip_session: None,
            };
            let answer = search::b::query_cli(&home, &q, vectors)?;
            if let search::b::Vector::Skipped(why) = answer.vector
                && why != search::b::VectorSkip::Off
            {
                eprintln!("oboete: full text only: {}", why.why());
            }
            let shown = q.searched().is_none();
            let rules = redact::Rules::load(&home)?;
            let out = answer
                .hits
                .iter()
                .map(|h| search::b::line(h, shown, &rules))
                .collect::<String>();
            emit(if out.is_empty() { "no hits" } else { &out })
        }
        Cmd::Get { ids } => {
            if let [id] = ids.as_slice() {
                match search::b::get(&home, id)? {
                    Some(text) => emit(&text),
                    None => Err(anyhow::anyhow!("no document {id} (ids come from search)")),
                }
            } else {
                emit(&search::b::get_many(&home, &ids)?)
            }
        }
        Cmd::Timeline { all, anchor, limit } => {
            let mut out = String::new();
            let repo = repo_filter(all)?;
            for i in search::b::timeline(&home, repo.as_deref(), anchor.as_deref(), None, limit)? {
                out.push_str(&search::b::item_line(&i, all));
            }
            emit(&out)
        }
        Cmd::Setup {
            embeddings: Some(choice),
            yes,
            ..
        } => setup::embeddings(&home, &choice, yes),
        Cmd::Setup { agent, remove, .. } => {
            setup::run(&home, &agent.context("name an agent")?, remove)
        }
        Cmd::Import {
            source,
            db,
            eval_store,
            agent,
            yes,
        } => {
            if source == "transcripts" {
                anyhow::ensure!(db.is_none(), "import transcripts takes no database path");
                anyhow::ensure!(!eval_store, "--eval-store is only for import claude-mem");
                let claude = setup::claude_dir().join("projects");
                let codex = setup::codex_home().join("sessions");
                let roots: Vec<(&str, &std::path::Path)> =
                    [("claude", claude.as_path()), ("codex", codex.as_path())]
                        .into_iter()
                        .filter(|(name, _)| agent.as_deref().is_none_or(|a| a == *name))
                        .collect();
                transcript::import(&home, &roots, yes, &mut std::io::stdout().lock())?;
                return Ok(());
            }
            if source != "claude-mem" {
                anyhow::bail!("unknown source {source}: use claude-mem or transcripts");
            }
            let db = db.context("import claude-mem requires a database path")?;
            anyhow::ensure!(!yes, "--yes is only for import transcripts");
            anyhow::ensure!(agent.is_none(), "--agent is only for import transcripts");
            // Until repositories map onto claude-mem's project names (PR-H), the rows would
            // reach no repository's injection; keep them out of the store the hooks write.
            // The hooks may write a custom home (OBOETE_HOME), so the caller has to say the store
            // is for evaluation; the default home is refused even then. Resolved paths:
            // `~/.oboete/../.oboete` or a symlink is the same store.
            let resolved =
                |p: &std::path::Path| std::fs::canonicalize(p).or_else(|_| std::path::absolute(p));
            if !eval_store || resolved(&home)? == resolved(&config::home_dir().join(".oboete"))? {
                anyhow::bail!(
                    "importing into the everyday store waits for the repository mapping (PR-H); for an evaluation store pass --home <dir> --eval-store"
                );
            }
            let _lock = import::lock(&home)?;
            let mut raw = raw::open(&home)?;
            let stats = import::claude_mem(&mut raw, &db)?;
            println!("{}", serde_json::to_string(&stats)?);
            Ok(())
        }
        Cmd::Migrate { from, finish } => {
            let from = from.unwrap_or_else(|| home.join("oboete.db"));
            if finish {
                let mut out = std::io::stdout().lock();
                return migrate::finish(&home, std::io::stdin().lock(), &mut out);
            }
            let outcome = match migrate::run(&home, &from, None, &mut |_| {}) {
                Ok(outcome) => outcome,
                Err(failure) => {
                    if let Some(settings) = &failure.outcome.settings {
                        for line in migrate::settings_lines(&settings.missing) {
                            println!("{line}");
                        }
                    }
                    return Err(failure.cause);
                }
            };
            if let Some(settings) = &outcome.settings {
                for line in migrate::settings_lines(&settings.missing) {
                    println!("{line}");
                }
            }
            println!("{}", serde_json::to_string(&outcome.stats)?);
            Ok(())
        }
        Cmd::Cite { uids } => {
            println!(
                "{}",
                serde_json::to_string(&search::b::cite(&home, &uids)?)?
            );
            Ok(())
        }
        Cmd::Gate => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
            emit(&redact::outbound(&text))
        }
        Cmd::Transcript { path, agent } => {
            let stats = transcript::convert(&path, &agent, std::io::stdout().lock())?;
            let ignored: Vec<String> = stats
                .ignored
                .iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect();
            eprintln!(
                "oboete transcript: {} lines, {} skipped, {} events; not read: {}",
                stats.lines,
                stats.skipped,
                stats.events,
                ignored.join(", ")
            );
            Ok(())
        }
        Cmd::Eval {
            queries,
            depth,
            arms,
            out,
        } => {
            let arms = arms
                .split(',')
                .map(str::parse)
                .collect::<Result<Vec<search::b::RawArm>>>()?;
            search::b::trec_run(
                &home,
                &std::fs::read_to_string(queries)?,
                depth,
                &arms,
                &out,
            )
        }
        Cmd::Doctor => setup::doctor(&home),
        Cmd::Pref {
            action: PrefCmd::Add { text },
        } => {
            // Whole: `oboete correct` takes the full id.
            let id = claims::pref_add(&home, &text)?;
            println!("kept for every repository ({id})");
            Ok(())
        }
        Cmd::Resume { provider } => {
            let db = providers_db::open(&home)?;
            if providers_db::resume(&db, &provider)? {
                println!("{provider} will be used again");
            } else {
                println!("{provider} was not stopped");
            }
            Ok(())
        }
        Cmd::Rebuild => {
            worker::rebuild(&home)?;
            println!("knowledge.db rebuilt from raw.db, with no AI call");
            Ok(())
        }
        Cmd::Forget {
            record,
            span,
            uid,
            yes,
            status,
        } => forget::run(
            &home,
            record.as_deref(),
            span.as_deref(),
            uid.as_deref(),
            yes,
            status,
        ),
        Cmd::Claims => {
            let settings = capture::Settings::load(&home)?;
            let cwd = std::env::current_dir()?;
            // The repository label as capture stores it on the claims' records.
            // As a hook's JSON payload carries it: a path that is not UTF-8 never panics here.
            let cwd = cwd.to_string_lossy();
            let (_, repo, _) = capture::checkout(&serde_json::json!({ "cwd": cwd }), &settings);
            emit(&claims_listed(&home, &repo)?)
        }
        Cmd::Recurate {
            skipped,
            source,
            span,
            yes,
        } => {
            let again = match (span, source) {
                (_, Some(source)) => curate::Again::Source(source),
                (Some(span), None) => {
                    let parsed = span.split_once(':').and_then(|(device, range)| {
                        let (from, to) = range.split_once('-')?;
                        let span = curate::Span::records(from.parse().ok()?, to.parse().ok()?);
                        Some(curate::Again::Span(device.to_owned(), span))
                    });
                    parsed.ok_or_else(|| {
                        anyhow::anyhow!("a span is <device>:<from>-<to>, such as 1a2b3c4d:120-180")
                    })?
                }
                (None, None) if skipped => curate::Again::Skipped,
                (None, None) => curate::Again::Queued,
            };
            print!("{}", curate::recurate(&home, again, yes)?);
            Ok(())
        }
        Cmd::Exclude { repo, undo } => {
            let repo = match repo {
                Some(r) => r,
                None => {
                    // The label as capture stores it on the records, as `claims` finds it.
                    let settings = capture::Settings::load(&home)?;
                    let cwd = std::env::current_dir()?;
                    let cwd = cwd.to_string_lossy();
                    capture::checkout(&serde_json::json!({ "cwd": cwd }), &settings).1
                }
            };
            let mut raw = raw::open(&home)?;
            let was = raw.exclusions()?.contains(&repo);
            raw.exclude(&repo, undo)?;
            let list = raw.exclusions()?;
            if undo {
                if was {
                    println!("no longer excluded: {repo}");
                } else {
                    println!("{repo} was not excluded");
                }
            } else {
                println!("excluded: {repo}");
                // A label that matches no record yet may be a typo: say so.
                if raw.sessions_in(std::slice::from_ref(&repo))?.is_empty() {
                    println!("no session recorded so far touched {repo}");
                }
            }
            if list.is_empty() {
                println!("no repository is excluded");
            } else {
                println!("excluded repositories: {}", list.join(", "));
            }
            Ok(())
        }
        Cmd::Correct { uid, status, body } => {
            claims::correct(&home, &uid, status.as_deref(), body.as_deref())?;
            println!("corrected {uid}");
            Ok(())
        }
        Cmd::Mute { uid } => {
            claims::mute(&home, &uid, true)?;
            println!("muted {uid}");
            Ok(())
        }
        Cmd::Unmute { uid } => {
            claims::mute(&home, &uid, false)?;
            println!("unmuted {uid}");
            Ok(())
        }
        Cmd::Restore => {
            let said = worker::restore(&home)?;
            println!("{said}");
            Ok(())
        }
        Cmd::View { resident: true, .. } => view::resident(&home),
        Cmd::View {
            new_token: true, ..
        } => {
            let (moved, url) = view::new_token(&home)?;
            println!("{url}");
            if moved {
                println!(
                    "Delete the old bookmark and bookmark this address: the viewer moves here within two minutes, and the old address no longer opens the page."
                );
            } else {
                println!("(a new token; this home's viewer uses it once the home is resident)");
            }
            Ok(())
        }
        Cmd::View { port, open, .. } => view::run(&home, port, open),
        #[cfg(feature = "local-embed")]
        Cmd::EmbedSpike {
            model,
            threads,
            verify,
        } => embed_local::spike(&model, threads, verify),
        Cmd::Replay {
            fixture,
            repo_root,
            spawn_sample,
            sizes,
            agent,
            read_sample,
            read_warmup,
        } => {
            let report = replay::run(
                &home,
                &fixture,
                repo_root,
                spawn_sample,
                &sizes,
                &agent,
                read_sample,
                read_warmup,
            )?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
    }
}

/// `oboete claims`: `repo`'s current claims and the global preferences, one a line.
fn claims_listed(home: &std::path::Path, repo: &str) -> Result<String> {
    // raw.db first, as every reader of knowledge.db holds it (a rebuild's swap waits).
    let raw = raw::open(home)?;
    let k = knowledge::open(home)?;
    claims::schema(&k)?;
    let mut out = String::new();
    // The global preferences too: `oboete correct` needs their uids, and no repository
    // lists them. A forgotten claim is no claim (milestone 5 D5).
    for c in claims::current(&k, repo)?
        .into_iter()
        .chain(claims::global(&k)?)
    {
        if raw.forgotten(&c.uid)? {
            continue;
        }
        let body = redact::outbound(&c.body).replace('\n', " ");
        out.push_str(&format!("{}  {} {}  {body}\n", c.uid, c.kind, c.status));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Milestone 5 D5: `oboete claims` lists no forgotten claim, and an owner's mute of one is
    /// refused as of a claim that is not there.
    #[test]
    fn claims_list_no_forgotten_claim_and_refuse_its_mute() {
        let mut s = search::b::fixture::Store::new();
        let gone = s.decided("r", 5, "Ship on Fridays.", &[]);
        let kept = s.decided("r", 6, "Ship on Mondays.", &[]);
        s.run();
        let home = s.home.path();
        let target = forget::Target::parse_uid(&gone).unwrap();
        forget::start(home, &forget::preview(home, target).unwrap()).unwrap();
        let listed = claims_listed(home, "r").unwrap();
        assert!(
            listed.contains(&kept) && !listed.contains(&gone),
            "{listed}"
        );
        let refused = claims::mute(home, &gone, true).unwrap_err();
        assert!(
            refused.to_string().contains("no claim has the uid"),
            "{refused:#}"
        );
    }

    #[test]
    fn mute_and_unmute_commands_wait_for_the_claims_consumer() {
        let mut s = search::b::fixture::Store::new();
        let uid = s.decided("r", 5, "Ship on Fridays.", &[]);
        s.run();
        for (name, muted) in [("mute", true), ("unmute", false)] {
            let cli = Cli::try_parse_from(["oboete", name, &uid]).unwrap();
            run(cli.cmd, s.home.path().to_owned()).unwrap();
            let k = knowledge::open(s.home.path()).unwrap();
            assert_eq!(
                k.query_row("SELECT muted FROM active WHERE uid = ?1", [&uid], |r| r
                    .get::<_, bool>(0))
                    .unwrap(),
                muted
            );
        }
        let ops = s.raw.ops_after(s.raw.device(), 0, 10).unwrap();
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[1].kind, raw::OpKind::Correction);
        assert_eq!(ops[1].body["muted"], true);
        assert_eq!(ops[2].body["muted"], false);
        let k = knowledge::open(s.home.path()).unwrap();
        assert_eq!(
            knowledge::checkpoint::get_in(&k, knowledge::checkpoint::OPS, "claims", s.raw.device())
                .unwrap(),
            3
        );
    }

    /// A restore that fails still runs the consumers, as one that succeeds does: a hook that
    /// appended while it held the worker lock started no worker (Codex on #359).
    #[test]
    fn a_restore_that_fails_runs_the_consumers() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path();
        let seq = raw::open(p)
            .unwrap()
            .append(&raw::test_event("first"))
            .unwrap();
        let cli = Cli::try_parse_from(["oboete", "restore"]).unwrap();
        let why = run(cli.cmd, p.to_owned()).unwrap_err().to_string();
        assert!(why.contains("no usable backup segment"), "{why}");
        let k = knowledge::open(p).unwrap();
        let read: Option<i64> = k
            .query_row("SELECT MIN(seq) FROM checkpoints", [], |r| r.get(0))
            .unwrap();
        assert_eq!(read, Some(seq));
    }

    /// `setup` takes an agent or `--embeddings`, never both; `--yes` only with `--embeddings`,
    /// `--remove` only with an agent, and only the three embedders.
    #[test]
    fn setup_takes_an_agent_or_an_embedder() {
        let parses =
            |args: &[&str]| Cli::try_parse_from([&["oboete", "setup"], args].concat()).is_ok();
        assert!(parses(&["claude"]));
        assert!(parses(&["all", "--remove"]));
        assert!(parses(&["--embeddings", "local"]));
        assert!(parses(&["--embeddings", "workers-ai", "--yes"]));
        assert!(!parses(&[]));
        assert!(!parses(&["--yes"]));
        assert!(!parses(&["--remove"]));
        assert!(!parses(&["claude", "--embeddings", "none"]));
        assert!(!parses(&["--embeddings", "fastembed"]));
        assert!(!parses(&["--embeddings", "none", "--remove"]));
    }
}
