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
mod failure;
mod gates;
mod hook;
mod hookstate;
mod import;
mod inject;
mod isolation;
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
    /// or a forget removed), the windows every provider skipped, or a span you name. It lists
    /// the windows and an estimate; nothing is sent without --yes
    Recurate {
        /// The windows every provider skipped
        #[arg(long, conflicts_with = "span")]
        skipped: bool,
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
    /// Search observations, summaries and prompts (this repository unless --all): by words, and
    /// by meaning too when `[embedding] provider = "workers-ai"`
    Search {
        /// Words or a sentence. Ranked by the 3-character pieces they share; a query too short
        /// for that matches its terms as literal substrings, all required. Put `--` before a
        /// term that starts with `-`
        query: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Print one document in full by its id from `search` (o12 = observation, s5 = summary,
    /// p7 = prompt)
    Get { id: String },
    /// Sessions newest first with their summaries (this repository unless --all)
    Timeline {
        #[arg(long)]
        all: bool,
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
    /// Browse the memory in a browser: a read-only page on 127.0.0.1 (prints its URL)
    View {
        /// Port to listen on (0 = any free port)
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Also open the page in the default browser
        #[arg(long)]
        open: bool,
    },
    /// Semantic search: embed every document that has no vector yet (without the daily cap)
    /// and rebuild the vector index. Needs `[embedding] provider = "workers-ai"`
    Reindex,
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
    /// Evaluation: run `{"qid","text"}` JSONL queries through search, print a TREC run
    #[command(hide = true)]
    Eval {
        queries: PathBuf,
        #[arg(long, default_value_t = 50)]
        depth: usize,
        /// fts (full-text) or hybrid (full-text and vectors, needs [embedding])
        #[arg(long, default_value = "fts")]
        method: String,
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
/// A raw hit's id as `oboete search` prints it, `<device>:<seq>` (milestone 2 Task 6): the event
/// with its time, kind and repo. `None` for any other id, or one raw does not hold, which then
/// goes to v1's store (whose synced uids also hold a colon).
fn raw_get(home: &std::path::Path, id: &str) -> Result<Option<String>> {
    let Some((device, seq)) = id.split_once(':') else {
        return Ok(None);
    };
    let Ok(seq) = seq.parse::<i64>() else {
        return Ok(None);
    };
    if seq < 1 || !raw::exists(home) {
        return Ok(None);
    }
    let raw = raw::open(home)?;
    let Some(r) = raw
        .after(device, seq - 1, 1)?
        .pop()
        .filter(|r| r.seq == seq)
    else {
        return Ok(None);
    };
    let raw::Item::Event(e) = r.item else {
        return Ok(None);
    };
    let when: String = rusqlite::Connection::open_in_memory()?.query_row(
        "SELECT strftime('%Y-%m-%d %H:%M', ?1 / 1000, 'unixepoch', 'localtime')",
        [e.ts],
        |r| r.get(0),
    )?;
    // Gated field by field as well as whole (`emit`): a rule may be anchored to a field's end.
    let repo = redact::outbound(e.repo.as_deref().unwrap_or(""));
    Ok(Some(format!(
        "{id} {when} {} {repo}\n\n{}\n",
        e.kind,
        redact::outbound_fields(&e.body)
    )))
}

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
        Cmd::Reindex => {
            let stats = embed::reindex(&home)?;
            println!("{}", serde_json::to_string(&stats)?);
            Ok(())
        }
        Cmd::Inject { session } => {
            let cwd = std::env::current_dir()?;
            print!("{}", hook::inject_text(&home, &cwd, session.as_deref()));
            Ok(())
        }
        Cmd::Mcp => mcp::run(&home),
        Cmd::Search { query, all, limit } => {
            let query = query.join(" ");
            let scope = repo_filter(all)?;
            let mut out = String::new();
            let mut left = limit;
            // Design B's none tier (milestone 2 Task 6): the raw index, by (device, seq). Until
            // every agent is ported (Task 2b) a home can hold both stores, and v1 commands such
            // as `timeline` create an empty oboete.db, so each store is searched when it exists.
            if raw::exists(&home) {
                let hits = search::raw(&home, &query, scope.as_deref(), limit)?;
                left -= hits.len().min(left);
                for h in hits {
                    // Each stored field through the gate on its own, before the lines are joined.
                    let repo = match (all, &h.repo) {
                        (true, Some(r)) => {
                            let r = redact::outbound(r);
                            format!("[{}] ", r.rsplit('/').next().unwrap_or(&r))
                        }
                        _ => String::new(),
                    };
                    let device: String = h.device.chars().take(8).collect();
                    out.push_str(&format!(
                        "{device}:{:<5} {}  {:<10} {repo}{}\n",
                        h.seq, h.when, h.kind, h.snippet
                    ));
                }
            }
            if left == 0 || !home.join("oboete.db").exists() {
                return emit(&out);
            }
            let conn = db::open(&home)?;
            let terms = search::terms(&query);
            let embedding = config::search_embedding(&home);
            for h in search::find(&conn, &embedding, &query, scope.as_deref(), left)? {
                let text = search::snippet(&redact::outbound(&h.body), &terms, 110);
                let repo = if all {
                    let name = std::path::Path::new(&h.repo)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| h.repo.clone());
                    format!("[{}] ", redact::outbound(&name))
                } else {
                    String::new()
                };
                out.push_str(&if h.title.is_empty() {
                    format!("{:<5} {}  {:<10} {repo}{text}\n", h.doc, h.when, h.kind)
                } else {
                    format!(
                        "{:<5} {}  {:<10} {repo}{}\n      {text}\n",
                        h.doc,
                        h.when,
                        h.kind,
                        redact::outbound(&h.title)
                    )
                });
            }
            emit(&out)
        }
        Cmd::Get { id } => {
            if let Some(text) = raw_get(&home, &id)? {
                return emit(&text);
            }
            let missing = || anyhow::anyhow!("no document {id} (ids come from `oboete search`)");
            if !home.join("oboete.db").exists() {
                return Err(missing());
            }
            let conn = db::open(&home)?;
            let h = search::get(&conn, &id)?.ok_or_else(missing)?;
            // Each stored field through the gate on its own, then the whole (`emit`).
            let title = if h.title.is_empty() {
                String::new()
            } else {
                format!("{}\n", redact::outbound(&h.title))
            };
            emit(&format!(
                "{} {} {} {}\n{title}\n{}\n",
                h.doc,
                h.when,
                h.kind,
                redact::outbound(&h.repo),
                redact::outbound(&h.body)
            ))
        }
        Cmd::Timeline { all, limit } => {
            let conn = db::open(&home)?;
            let mut out = String::new();
            for r in search::timeline(&conn, repo_filter(all)?.as_deref(), limit)? {
                // The tail of the id: UUIDv7 heads (Codex, Grok) are timestamps and collide.
                // Gated before it is shortened, as each field is.
                let id: String = redact::outbound(&r.id)
                    .chars()
                    .rev()
                    .take(8)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                // Gated before it is flattened and clipped, and the label on its own.
                let summary: String = redact::outbound(&r.summary)
                    .replace('\n', " ")
                    .chars()
                    .take(120)
                    .collect();
                out.push_str(&format!(
                    "{}  {:<6} {id}  {}  {summary}\n",
                    r.when,
                    r.agent,
                    redact::outbound(&r.repo)
                ));
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
            let mut conn = db::open(&home)?;
            let stats = import::claude_mem(&mut conn, &db)?;
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
            method,
        } => {
            let embedding = match method.as_str() {
                "fts" => None,
                "hybrid" => Some(config::load(&home)?.embedding),
                other => anyhow::bail!("--method {other}: use fts or hybrid"),
            };
            let conn = db::open(&home)?;
            emit(&search::trec_run(
                &conn,
                &std::fs::read_to_string(queries)?,
                depth,
                embedding.as_ref(),
            )?)
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
            for c in claims::current(&k, &repo)? {
                let body = redact::outbound(&c.body).replace('\n', " ");
                out.push_str(&format!("{}  {} {}  {body}\n", c.uid, c.kind, c.status));
            }
            emit(&out)
        }
        Cmd::Recurate { skipped, span, yes } => {
            let again = match span {
                Some(span) => {
                    let parsed = span.split_once(':').and_then(|(device, range)| {
                        let (from, to) = range.split_once('-')?;
                        let span = curate::Span::records(from.parse().ok()?, to.parse().ok()?);
                        Some(curate::Again::Span(device.to_owned(), span))
                    });
                    parsed.ok_or_else(|| {
                        anyhow::anyhow!("a span is <device>:<from>-<to>, such as 1a2b3c4d:120-180")
                    })?
                }
                None if skipped => curate::Again::Skipped,
                None => curate::Again::Queued,
            };
            print!("{}", curate::recurate(&home, again, yes)?);
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
        } => replay::run(&home, &fixture, repo_root, spawn_sample, &sizes, &agent),
    }
}
