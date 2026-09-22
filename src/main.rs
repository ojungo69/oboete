//! oboete — lightweight, single-binary memory for coding agents.
//!
//! M0 spike: Claude Code hook capture → SQLite → summarizer chain with
//! fallback → SessionStart injection. See docs/plan.md.

mod config;
mod db;
mod hook;
mod inject;
mod observe;
mod provider;
mod redact;
mod replay;
mod repo;

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
    /// Replay a JSONL fixture through the hook path and measure
    Replay {
        fixture: PathBuf,
        /// Directory that stands in for the fixture's repository root
        #[arg(long)]
        repo_root: Option<PathBuf>,
        /// Also time N real `oboete hook` process spawns (startup + insert)
        #[arg(long, default_value_t = 30)]
        spawn_sample: usize,
        /// Only replay events of this agent (default: claude)
        #[arg(long, default_value = "claude")]
        agent: String,
    },
}

fn main() {
    let cli = Cli::parse();
    let home = cli.home.unwrap_or_else(|| dirs_home().join(".oboete"));
    let code = match run(cli.cmd, home) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("oboete: {e:#}");
            1
        }
    };
    std::process::exit(code);
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
        Cmd::Replay {
            fixture,
            repo_root,
            spawn_sample,
            agent,
        } => replay::run(&home, &fixture, repo_root, spawn_sample, &agent),
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
