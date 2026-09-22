//! oboete — lightweight, single-binary memory for coding agents.
//!
//! M0 spike: Claude Code hook capture → SQLite → summarizer chain with
//! fallback → SessionStart injection. See docs/plan.md.

mod config;
mod db;
mod hook;
mod inject;
mod mcp;
mod observe;
mod provider;
mod redact;
mod replay;
mod repo;
mod search;
mod setup;
mod view;

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
enum Cmd {
    /// Receive one agent hook event on stdin and store it (fail-open, always exit 0)
    Hook {
        /// Agent name: claude | codex | grok
        agent: String,
        /// Hook event name (e.g. SessionStart, UserPromptSubmit, PostToolUse, Stop, SessionEnd)
        event: String,
    },
    /// Summarize pending sessions through the provider chain
    Observe {
        /// Only process sessions idle for at least this long
        #[arg(long, default_value_t = 60_000)]
        settle_ms: u64,
    },
    /// Print the context that would be injected for the current directory
    Inject,
    /// Serve the memory as an MCP server on stdin/stdout (search / get / timeline tools)
    Mcp,
    /// Full-text search over observations and summaries (this repository unless --all)
    Search {
        /// Terms, all required. One under 3 characters matches as a literal substring
        /// (ASCII case folding only). Put `--` before a term that starts with `-`
        query: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Print one document in full by its id from `search` (o12 = observation, s5 = summary)
    Get { id: String },
    /// Sessions newest first with their summaries (this repository unless --all)
    Timeline {
        #[arg(long)]
        all: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Wire this binary into an agent's hooks (claude | codex | grok | all)
    Setup {
        agent: String,
        /// Take oboete's hook entries out again
        #[arg(long)]
        remove: bool,
    },
    /// Report hook wiring, stored data and provider readiness
    Doctor,
    /// Browse the memory in a browser: a read-only page on 127.0.0.1 (prints its URL)
    View {
        /// Port to listen on (0 = any free port)
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Also open the page in the default browser
        #[arg(long)]
        open: bool,
    },
    /// Replay a JSONL fixture through the hook path and measure
    Replay {
        fixture: PathBuf,
        /// Directory that stands in for the fixture's repository root
        #[arg(long)]
        repo_root: Option<PathBuf>,
        /// Also time N real `oboete hook` process spawns (startup + insert)
        #[arg(long, default_value_t = 30)]
        spawn_sample: usize,
        /// Only replay events of this agent: claude | codex | grok | all
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
fn emit(text: &str) -> Result<()> {
    use std::io::Write;
    match std::io::stdout().lock().write_all(text.as_bytes()) {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.into()),
        _ => Ok(()),
    }
}

fn run(cmd: Cmd, home: PathBuf) -> Result<()> {
    std::fs::create_dir_all(&home)?;
    match cmd {
        Cmd::Hook { agent, event } => {
            // Fail-open: a hook must never break the agent.
            if let Err(e) = hook::run_stdin(&home, &agent, &event) {
                eprintln!("oboete hook: {e:#}");
            }
            Ok(())
        }
        Cmd::Observe { settle_ms } => {
            let stats = observe::run(&home, settle_ms)?;
            println!("{}", serde_json::to_string(&stats)?);
            Ok(())
        }
        Cmd::Inject => {
            let cwd = std::env::current_dir()?;
            let conn = db::open(&home)?;
            let repo = repo::key(&cwd);
            print!("{}", inject::context(&conn, &repo)?);
            Ok(())
        }
        Cmd::Mcp => mcp::run(&home),
        Cmd::Search { query, all, limit } => {
            let conn = db::open(&home)?;
            let query = query.join(" ");
            let terms: Vec<&str> = query.split_whitespace().collect();
            let mut out = String::new();
            for h in search::search(&conn, &query, repo_filter(all)?.as_deref(), limit)? {
                let text = search::snippet(&h.body, &terms, 110);
                let repo = if all {
                    let name = std::path::Path::new(&h.repo)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| h.repo.clone());
                    format!("[{name}] ")
                } else {
                    String::new()
                };
                out.push_str(&if h.title.is_empty() {
                    format!("{:<5} {}  {:<10} {repo}{text}\n", h.doc, h.when, h.kind)
                } else {
                    format!(
                        "{:<5} {}  {:<10} {repo}{}\n      {text}\n",
                        h.doc, h.when, h.kind, h.title
                    )
                });
            }
            emit(&out)
        }
        Cmd::Get { id } => {
            let conn = db::open(&home)?;
            let h = search::get(&conn, &id)?.ok_or_else(|| {
                anyhow::anyhow!("no document {id} (ids come from `oboete search`)")
            })?;
            let title = if h.title.is_empty() {
                String::new()
            } else {
                format!("{}\n", h.title)
            };
            emit(&format!(
                "{} {} {} {}\n{title}\n{}\n",
                h.doc, h.when, h.kind, h.repo, h.body
            ))
        }
        Cmd::Timeline { all, limit } => {
            let conn = db::open(&home)?;
            let mut out = String::new();
            for r in search::timeline(&conn, repo_filter(all)?.as_deref(), limit)? {
                // The tail of the id: UUIDv7 heads (Codex, Grok) are timestamps and collide.
                let id: String =
                    r.id.chars()
                        .rev()
                        .take(8)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                let summary: String = r.summary.replace('\n', " ").chars().take(120).collect();
                out.push_str(&format!(
                    "{}  {:<6} {id}  {}  {summary}\n",
                    r.when, r.agent, r.repo
                ));
            }
            emit(&out)
        }
        Cmd::Setup { agent, remove } => setup::run(&home, &agent, remove),
        Cmd::Doctor => setup::doctor(&home),
        Cmd::View { port, open } => view::run(&home, port, open),
        Cmd::Replay {
            fixture,
            repo_root,
            spawn_sample,
            agent,
        } => replay::run(&home, &fixture, repo_root, spawn_sample, &agent),
    }
}
