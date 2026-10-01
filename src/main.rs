//! oboete — lightweight, single-binary memory for coding agents.
//!
//! M0 spike: Claude Code hook capture → SQLite → summarizer chain with
//! fallback → SessionStart injection. See docs/plan.md.

mod backup;
mod budget;
mod capture;
mod claims;
mod codex_probe;
mod config;
mod consumer;
#[cfg(test)]
mod crash;
mod curate;
mod db;
mod digest;
mod embed;
mod embed_phase;
mod failure;
mod gates;
mod hook;
mod hookstate;
mod import;
mod isolation;
mod keyfile;
mod knowledge;
mod manifest;
mod mcp;
mod provider;
mod providers_db;
// Design B's store: the hook writes to it; its readers come with the worker (milestone 2 Task 5).
#[allow(dead_code)]
mod raw;
mod redact;
mod replay;
mod repo;
mod search;
mod settings;
mod setup;
mod transcript;
mod view;
mod worker;

use std::path::PathBuf;

use anyhow::Result;
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
    /// Run Design B's consumers over raw.db until idle (hooks start it; one per home)
    Worker {
        /// Exit after this long without a new record
        #[arg(long, default_value_t = 60_000)]
        idle_ms: u64,
    },
    /// Rebuild knowledge.db (claims, digests, indexes, manifests) from raw.db and its op log,
    /// with no AI call
    Rebuild,
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
    },
    /// Serve the memory as an MCP server on stdin/stdout (search / get / timeline tools)
    Mcp,
    /// Search what is remembered (this repository unless --all or --repo): the decisions and
    /// other claims first, then claude-mem's imported history, then the raw records, then the
    /// claims later ones ended
    Search {
        /// Words or a sentence. Ranked by the 3-character pieces they share; a query too short
        /// for that matches its terms as literal substrings, all required. Put `--` before a
        /// term that starts with `-`
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
        /// The raw records: below the rest (below), not at all (off), or alone (only)
        #[arg(long, default_value = "below")]
        raw: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Print one in full by its id from `search`: a claim's uid (or its first characters), an
    /// imported document's uid, or a record's `device:seq`
    Get { id: String },
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
    /// Wire this binary into an agent's hooks (claude | codex | grok | agy | opencode | pi | cursor | all)
    Setup {
        agent: String,
        /// Take oboete's hook entries out again
        #[arg(long)]
        remove: bool,
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
        /// Port to listen on (0 = any free port)
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Also open the page in the default browser
        #[arg(long)]
        open: bool,
    },
    /// Copy another memory tool's store into this one (claude-mem's SQLite database)
    Import {
        /// Source tool: claude-mem
        source: String,
        /// Its database file (read-only; e.g. ~/.claude-mem/claude-mem.db)
        db: PathBuf,
        /// The --home store is for evaluation, not the one the hooks write (required until PR-H)
        #[arg(long)]
        eval_store: bool,
    },
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
    },
}

fn main() {
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
    // Hooks create storage after the skip guards, inside their fail-open boundary.
    if !matches!(&cmd, Cmd::Hook { .. }) {
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
        Cmd::Worker { idle_ms } => worker::run(&home, idle_ms),
        Cmd::Inject { session } => {
            let cwd = std::env::current_dir()?;
            print!("{}", hook::inject_text(&home, &cwd, session.as_deref()));
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
            raw,
            limit,
        } => {
            let q = search::b::Query {
                text: query.join(" "),
                caller: repo_filter(false)?,
                repo,
                all,
                since: since.map(|s| search::b::time(&s, false)).transpose()?,
                until: until.map(|s| search::b::time(&s, true)).transpose()?,
                history,
                raw: match raw.as_str() {
                    "below" => search::b::RawArm::Below,
                    "off" => search::b::RawArm::Off,
                    "only" => search::b::RawArm::Only,
                    other => anyhow::bail!("--raw {other}: use below, off or only"),
                },
                limit,
                skip_session: None,
            };
            let answer = search::b::query(&home, &q)?;
            if let search::b::Vector::Skipped(why) = answer.vector
                && why != search::b::VectorSkip::Off
            {
                eprintln!("oboete: full text only: {}", why.why());
            }
            let shown = q.searched().is_none();
            emit(
                &answer
                    .hits
                    .iter()
                    .map(|h| search::b::line(h, shown))
                    .collect::<String>(),
            )
        }
        Cmd::Get { id } => match search::b::get(&home, &id)? {
            Some(text) => emit(&text),
            None => Err(anyhow::anyhow!(
                "no document {id} (ids come from `oboete search`)"
            )),
        },
        Cmd::Timeline { all, anchor, limit } => {
            let mut out = String::new();
            let repo = repo_filter(all)?;
            for i in search::b::timeline(&home, repo.as_deref(), anchor.as_deref(), None, limit)? {
                out.push_str(&search::b::item_line(&i, all));
            }
            emit(&out)
        }
        Cmd::Setup { agent, remove } => setup::run(&home, &agent, remove),
        Cmd::Import {
            source,
            db,
            eval_store,
        } => {
            if source != "claude-mem" {
                anyhow::bail!("unknown source {source}: use claude-mem");
            }
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
        Cmd::Claims => {
            let settings = capture::Settings::load(&home)?;
            let cwd = std::env::current_dir()?;
            // The repository label as capture stores it on the claims' records.
            // As a hook's JSON payload carries it: a path that is not UTF-8 never panics here.
            let cwd = cwd.to_string_lossy();
            let (_, repo, _) = capture::checkout(&serde_json::json!({ "cwd": cwd }), &settings);
            // raw.db first, as every reader of knowledge.db holds it (a rebuild's swap waits).
            let _raw = raw::open(&home)?;
            let k = knowledge::open(&home)?;
            claims::schema(&k)?;
            let mut out = String::new();
            // The global preferences too: `oboete correct` needs their uids, and no repository
            // lists them.
            for c in claims::current(&k, &repo)?
                .into_iter()
                .chain(claims::global(&k)?)
            {
                let body = redact::outbound(&c.body).replace('\n', " ");
                out.push_str(&format!("{}  {} {}  {body}\n", c.uid, c.kind, c.status));
            }
            emit(&out)
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
        Cmd::Restore => {
            // The worker's lock, so no worker reads raw.db while it is replaced.
            let held = worker::lock(&home)?.ok_or_else(|| {
                anyhow::anyhow!("a worker is running; try again when it has exited")
            })?;
            let said = backup::restore(&home)?;
            // Derived data was moved aside: it is rebuilt before this returns, so a search right
            // after finds the restored records.
            drop(held);
            worker::run_once(&home)?;
            println!("{said}");
            Ok(())
        }
        Cmd::View { port, open } => view::run(&home, port, open),
        Cmd::Replay {
            fixture,
            repo_root,
            spawn_sample,
            sizes,
            agent,
            read_sample,
        } => replay::run(
            &home,
            &fixture,
            repo_root,
            spawn_sample,
            &sizes,
            &agent,
            read_sample,
        ),
    }
}
