//! `oboete setup <agent>` wires agent hooks to this binary and
//! `--remove` takes exactly those entries out again; `oboete doctor` reports the state.
//! Only oboete's own entries are ever touched. The first write to a file the developer owned
//! leaves a `.oboete.bak` copy next to it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{config, db};

pub const AGENTS: [&str; 4] = ["claude", "codex", "grok", "agy"];
const BACKUP_SUFFIX: &str = ".oboete.bak";

/// The name each agent knows our MCP server by (tools show up as `oboete__search` and so on).
const MCP_NAME: &str = "oboete";

/// (event, timeout seconds). Timeouts only bound a stalled hook; the hook itself takes ~10 ms.
const CLAUDE_EVENTS: &[(&str, u32)] = &[
    ("SessionStart", 10),
    ("UserPromptSubmit", 5),
    ("PostToolUse", 5),
    ("PostToolUseFailure", 5),
    ("Stop", 5),
    ("PostCompact", 5),
    ("SessionEnd", 3),
];
/// (event, matcher, timeout, injects). `resume` is left out: the transcript already carries it.
const CODEX_EVENTS: &[(&str, Option<&str>, u32, bool)] = &[
    ("SessionStart", Some("startup|clear|compact"), 10, true),
    ("UserPromptSubmit", None, 5, true),
    ("PostToolUse", None, 5, false),
    ("Stop", None, 5, false),
    ("PostCompact", None, 5, false),
    ("SessionEnd", None, 3, false),
];
/// Grok ignores SessionStart stdout, so PreToolUse is wired too: that is where context goes in.
const GROK_EVENTS: &[(&str, u32)] = &[
    ("SessionStart", 5),
    ("UserPromptSubmit", 5),
    ("PreToolUse", 5),
    ("PostToolUse", 5),
    ("PostToolUseFailure", 5),
    ("Stop", 5),
    ("PostCompact", 5),
    ("SessionEnd", 3),
];
const AGY_FLAT_EVENTS: [&str; 3] = ["SessionStart", "PreInvocation", "Stop"];

pub fn run(home: &Path, agent: &str, remove: bool) -> Result<()> {
    let agents: Vec<&str> = if agent == "all" {
        AGENTS.to_vec()
    } else if AGENTS.contains(&agent) {
        vec![agent]
    } else {
        return Err(anyhow!(
            "unknown agent {agent}: use claude | codex | grok | agy | all"
        ));
    };
    let cmd = HookCommand::current(home)?;
    for a in agents {
        let files = match a {
            "claude" => claude(&cmd, remove)?,
            "codex" => codex(&cmd, remove)?,
            "grok" => grok(&cmd, remove)?,
            "agy" if !agy_available(&agy_dir(), on_path("agy")) => {
                println!("agy: skipped (`~/.gemini` and `agy` on PATH are absent)");
                continue;
            }
            "agy" => agy_files(&agy_dir(), &cmd, remove, cfg!(windows))?,
            _ => unreachable!(),
        };
        let verb = if remove { "removed from" } else { "written to" };
        if files.is_empty() {
            println!("{a}: hooks: nothing to remove");
        } else {
            println!("{a}: hooks {verb} {}", files.join(", "));
        }
        let mcp = match a {
            "claude" => claude_mcp(&cmd, remove)?,
            "codex" => toml_mcp(&codex_home().join("config.toml"), &cmd, remove)?,
            "grok" => toml_mcp(&grok_config_file(), &cmd, remove)?,
            "agy" => (if remove {
                "removed with hooks above"
            } else {
                "written with hooks above"
            })
            .to_string(),
            _ => unreachable!(),
        };
        println!("{a}: mcp server {mcp}");
    }
    if !remove {
        println!("Hook files are read when an agent starts: restart running sessions.");
    }
    Ok(())
}

/// The command line every hook entry runs: this binary's absolute path plus `hook <agent> <event>`;
/// the MCP registration is the same binary with `mcp`.
struct HookCommand {
    exe: String,
    home: Option<String>,
}

impl HookCommand {
    fn current(home: &Path) -> Result<Self> {
        let exe = std::env::current_exe()?.canonicalize()?;
        // Hooks run from the agent's working directory: a custom home is stored absolute.
        let home = home
            .canonicalize()
            .with_context(|| format!("resolve home {}", home.display()))?;
        let default_home = config::home_dir().join(".oboete");
        let home = (Some(&home) != default_home.canonicalize().ok().as_ref())
            .then(|| home.to_string_lossy().into_owned());
        Ok(Self {
            exe: exe.to_string_lossy().into_owned(),
            home,
        })
    }
    /// `[--home <dir>] mcp`, the arguments after the binary in an MCP server entry.
    fn mcp_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(h) = &self.home {
            args.push("--home".to_string());
            args.push(h.clone());
        }
        args.push("mcp".to_string());
        args
    }
    fn line(&self, agent: &str, event: &str) -> String {
        let mut s = shell_quote(&self.exe);
        if let Some(h) = &self.home {
            s.push_str(" --home ");
            s.push_str(&shell_quote(h));
        }
        format!("{s} hook {agent} {event}")
    }

    fn agy_line(&self, event: &str, windows: bool) -> Result<String> {
        if !windows {
            return Ok(self.line("agy", event));
        }
        // agy on Windows splits the command on spaces without removing quotes.
        for path in std::iter::once(&self.exe).chain(self.home.iter()) {
            anyhow::ensure!(
                !path
                    .chars()
                    .any(|c| c.is_whitespace() || "&|<>()^%!\"'".contains(c)),
                "agy on Windows needs a space-free executable and --home path (got {path}); use a space-free path"
            );
        }
        let home = self
            .home
            .as_ref()
            .map(|h| format!(" --home {h}"))
            .unwrap_or_default();
        Ok(format!("{}{home} hook agy {event}", self.exe))
    }
}

fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+:@%".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// A handler is ours when its command has exactly the shape `HookCommand::line` writes:
/// `<path ending in oboete[.exe]> [--home <dir>] hook <agent> <Event>`. No marker key: an
/// agent that rejected unknown keys in hook groups would take the developer's other hooks
/// down with ours. A developer's script that merely mentions oboete and hook is not ours.
fn is_our_handler(h: &Value) -> bool {
    let Some(cmd) = h["command"].as_str() else {
        return false;
    };
    let words = shell_words(cmd);
    let Some((exe, rest)) = words.split_first() else {
        return false;
    };
    // The canonical path setup writes may be versioned or renamed (`oboete-0.1`, `oboete.exe`).
    let is_exe = Path::new(exe)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|f| f.starts_with("oboete"));
    let rest = match rest {
        [flag, _dir, tail @ ..] if flag == "--home" => tail,
        tail => tail,
    };
    is_exe
        && matches!(rest, [hook, agent, _event] if hook == "hook" && AGENTS.contains(&agent.as_str()))
}

/// Inverse of `shell_quote` for the lines we write: whitespace-separated words, single-quoted
/// segments (with `'\''` for a literal quote) kept whole.
fn shell_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '\\' if chars.peek() == Some(&'\'') => {
                chars.next();
                cur.push('\'');
                in_word = true;
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

pub(crate) fn has_ours(group: &Value) -> bool {
    group["hooks"]
        .as_array()
        .is_some_and(|hs| hs.iter().any(is_our_handler))
}

/// The group without our handlers; `None` when nothing else was in it.
fn without_ours(group: &Value) -> Option<Value> {
    let handlers = group["hooks"].as_array()?;
    let theirs: Vec<Value> = handlers
        .iter()
        .filter(|h| !is_our_handler(h))
        .cloned()
        .collect();
    if theirs.len() == handlers.len() {
        return Some(group.clone());
    }
    if theirs.is_empty() {
        return None;
    }
    let mut g = group.clone();
    g["hooks"] = Value::Array(theirs);
    Some(g)
}

fn backup_once(file: &Path) -> Result<()> {
    let bak = file.with_file_name(format!(
        "{}{BACKUP_SUFFIX}",
        file.file_name().unwrap_or_default().to_string_lossy()
    ));
    if file.exists() && !bak.exists() {
        std::fs::copy(file, &bak).with_context(|| format!("backup {}", file.display()))?;
    }
    Ok(())
}

fn read_json_object(file: &Path) -> Result<Value> {
    if !file.exists() {
        return Ok(json!({}));
    }
    let text = std::fs::read_to_string(file).with_context(|| format!("read {}", file.display()))?;
    let v: Value = if text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&text).with_context(|| format!("parse {}", file.display()))?
    };
    anyhow::ensure!(v.is_object(), "{} is not a JSON object", file.display());
    Ok(v)
}

fn write_json(file: &Path, v: &Value) -> Result<()> {
    write_atomic(file, &json_text(v)?)
}

fn json_text(v: &Value) -> Result<String> {
    Ok(format!("{}\n", serde_json::to_string_pretty(v)?))
}

/// A missing file reads as empty; any other read error stops setup before it writes anything
/// (an unreadable config must not be replaced by one holding only our entries).
fn read_text(file: &Path) -> Result<String> {
    match std::fs::read_to_string(file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        r => r.with_context(|| format!("read {}", file.display())),
    }
}

/// Agents read these files while they run, so a write never leaves one truncated: a temp file
/// next to the target, then a rename. A symlinked file (dotfile managers) is written through to
/// its target; the target's mode is kept and a new file gets 0600.
fn write_atomic(file: &Path, text: &str) -> Result<()> {
    stage(file, text)?.commit()
}

/// Where a write to `file` lands: through its symlinks, also to a target that does not exist yet.
fn resolve_links(file: &Path) -> PathBuf {
    if let Ok(t) = std::fs::canonicalize(file) {
        return t;
    }
    let mut p = file.to_path_buf();
    for _ in 0..40 {
        match std::fs::read_link(&p) {
            Ok(to) => p = p.parent().map(|d| d.join(&to)).unwrap_or(to),
            Err(_) => break,
        }
    }
    p
}

/// The new content written and synced next to its target, not yet renamed over it. Everything
/// that can fail (a read-only file or directory, a full disk) fails here, so several files can
/// be staged first and committed together. Dropped uncommitted, the temp file goes.
struct Staged {
    file: PathBuf,
    target: PathBuf,
    tmp: PathBuf,
    done: bool,
}

impl Staged {
    fn commit(mut self) -> Result<()> {
        std::fs::rename(&self.tmp, &self.target)
            .with_context(|| format!("write {}", self.file.display()))?;
        self.done = true;
        Ok(())
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.done {
            let _ = std::fs::remove_file(&self.tmp);
        }
    }
}

fn stage(file: &Path, text: &str) -> Result<Staged> {
    use std::io::Write;
    let target = resolve_links(file);
    refuse_read_only(&target)?;
    let dir = target
        .parent()
        .with_context(|| format!("no directory for {}", file.display()))?;
    std::fs::create_dir_all(dir)?;
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(".{name}.{}.oboete-tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let staged = Staged {
        file: file.to_path_buf(),
        target,
        tmp,
        done: false,
    };
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    (|| -> std::io::Result<()> {
        let mut f = opts.open(&staged.tmp)?;
        f.write_all(text.as_bytes())?;
        if let Ok(m) = std::fs::metadata(&staged.target) {
            f.set_permissions(m.permissions())?;
        }
        f.sync_all()
    })()
    .with_context(|| format!("write {}", file.display()))?;
    Ok(staged)
}

/// A rename would replace a file the developer locked with chmod 444; stop instead.
fn refuse_read_only(file: &Path) -> Result<()> {
    if std::fs::metadata(file).is_ok_and(|m| m.permissions().readonly()) {
        anyhow::bail!("{} is read-only", file.display());
    }
    Ok(())
}

/// Drop our handlers from every event (a group that held only ours goes; one shared with the
/// developer's handlers keeps theirs), then (unless removing) append the wanted groups.
fn merge_groups(root: &mut Value, wanted: Vec<(String, Value)>) {
    let hooks = root["hooks"]
        .as_object_mut()
        .map(std::mem::take)
        .unwrap_or_default();
    let mut hooks: serde_json::Map<String, Value> = hooks
        .into_iter()
        .map(|(event, groups)| {
            let kept: Vec<Value> = groups
                .as_array()
                .map(|g| g.iter().filter_map(without_ours).collect())
                .unwrap_or_default();
            (event, Value::Array(kept))
        })
        .collect();
    for (event, group) in wanted {
        hooks
            .entry(event)
            .or_insert_with(|| Value::Array(vec![]))
            .as_array_mut()
            .expect("array")
            .push(group);
    }
    hooks.retain(|_, v| !v.as_array().is_some_and(Vec::is_empty));
    root["hooks"] = Value::Object(hooks);
}

/// Claude Code's config directory: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
fn claude_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| config::home_dir().join(".claude"))
}

/// The file `claude mcp --scope user` edits, found the way Claude Code 2.1 finds it: a legacy
/// `<config dir>/.config.json` if one exists, else `.claude.json` in `$CLAUDE_CONFIG_DIR` or home.
fn claude_mcp_file() -> PathBuf {
    let legacy = claude_dir().join(".config.json");
    if legacy.exists() {
        return legacy;
    }
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(config::home_dir)
        .join(".claude.json")
}

/// That file is Claude Code's state file, rewritten all the time under its own lock; a write
/// from outside races it (a session that reads the file mid-write "repairs" it from its cache,
/// dropping our entry). So Claude Code's own CLI makes the change. Keys the developer added to
/// our entry (`env`, ...) are carried over.
fn claude_mcp(cmd: &HookCommand, remove: bool) -> Result<String> {
    if !on_path("claude") {
        return Ok("skipped: `claude` is not on PATH".into());
    }
    let file = claude_mcp_file();
    let entry_now = || -> Result<Option<Value>> {
        Ok(read_json_object(&file)?["mcpServers"]
            .get(MCP_NAME)
            .cloned())
    };
    let present = entry_now()?;
    if remove && present.is_none() {
        return Ok("nothing to remove".into());
    }
    backup_once(&file)?;
    // `add-json` refuses an existing name, so whatever sits under it goes first.
    if present.is_some() {
        claude_cli(&["mcp", "remove", "--scope", "user", MCP_NAME])?;
    }
    if !remove {
        let old = present.filter(Value::is_object);
        let mut entry = old.clone().unwrap_or_else(|| json!({"env": {}}));
        entry["type"] = json!("stdio");
        entry["command"] = json!(cmd.exe);
        entry["args"] = json!(cmd.mcp_args());
        let add = |e: &Value| {
            claude_cli(&[
                "mcp",
                "add-json",
                "--scope",
                "user",
                MCP_NAME,
                &e.to_string(),
            ])
        };
        if let Err(e) = add(&entry) {
            // Put the developer's entry back rather than leave nothing registered.
            if let Some(old) = &old
                && let Err(r) = add(old)
            {
                return Err(e.context(format!(
                    "the previous {MCP_NAME} entry is removed and could not be put back: {r:#}"
                )));
            }
            return Err(e);
        }
    }
    // `claude mcp` exits 0 also when it could not save (a read-only config): check the file.
    let landed = if remove {
        entry_now()?.is_none()
    } else {
        mcp_command(&file) == Some((cmd.exe.clone(), cmd.mcp_args()))
    };
    anyhow::ensure!(
        landed,
        "`claude mcp` did not update {} (is it writable?)",
        file.display()
    );
    let verb = if remove { "removed from" } else { "written to" };
    Ok(format!("{verb} {} (by `claude mcp`)", file.display()))
}

fn claude_cli(args: &[&str]) -> Result<()> {
    let out = std::process::Command::new("claude")
        .args(args)
        // `claude mcp ...` runs the SessionStart hooks too; without this each call is an empty
        // captured session.
        .env("OBOETE_SKIP", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .context("run claude")?;
    anyhow::ensure!(
        out.status.success(),
        "claude {}: {}{}",
        args[..2].join(" "),
        String::from_utf8_lossy(&out.stderr).trim(),
        String::from_utf8_lossy(&out.stdout).trim()
    );
    Ok(())
}

/// Codex and Grok Build both take `[mcp_servers.<name>]` with `command` and `args` in their
/// `config.toml`. Only those two keys are ours: other servers, the rest of the file and keys the
/// developer set on our entry (`enabled`, timeouts, `env`) are left as they are.
fn toml_mcp(file: &Path, cmd: &HookCommand, remove: bool) -> Result<String> {
    if remove && !file.exists() {
        return Ok("nothing to remove".into());
    }
    let mut doc: toml_edit::DocumentMut = read_text(file)?
        .parse()
        .with_context(|| format!("parse {}", file.display()))?;
    backup_once(file)?;
    let root = doc.as_table_mut();
    if remove {
        let emptied = root
            .get_mut("mcp_servers")
            .and_then(toml_edit::Item::as_table_like_mut)
            .is_some_and(|servers| {
                servers.remove(MCP_NAME);
                servers.is_empty()
            });
        // Only a header setup created (implicit) or a dotted prefix goes with our entry; an
        // explicit `[mcp_servers]` the developer wrote stays, with its comments.
        if emptied
            && root
                .get("mcp_servers")
                .and_then(toml_edit::Item::as_table)
                .is_some_and(|t| t.is_implicit() || t.is_dotted())
        {
            root.remove("mcp_servers");
        }
    } else {
        if !root.contains_key("mcp_servers") {
            let mut servers = toml_edit::Table::new();
            servers.set_implicit(true);
            root.insert("mcp_servers", toml_edit::Item::Table(servers));
        }
        let Some(servers) = root
            .get_mut("mcp_servers")
            .and_then(toml_edit::Item::as_table_like_mut)
        else {
            anyhow::bail!("{}: mcp_servers is not a table", file.display());
        };
        let command = toml_edit::value(cmd.exe.as_str());
        let args = toml_edit::value(cmd.mcp_args().iter().collect::<toml_edit::Array>());
        match servers
            .get_mut(MCP_NAME)
            .and_then(toml_edit::Item::as_table_like_mut)
        {
            Some(row) => {
                row.insert("command", command);
                row.insert("args", args);
            }
            None => {
                let mut row = toml_edit::Table::new();
                row["command"] = command;
                row["args"] = args;
                servers.insert(MCP_NAME, toml_edit::Item::Table(row));
            }
        }
    }
    write_atomic(file, &doc.to_string())?;
    let verb = if remove { "removed from" } else { "written to" };
    Ok(format!("{verb} {}", file.display()))
}

/// The command and args registered under our name, if any: Claude's JSON or a `config.toml`.
fn mcp_command(file: &Path) -> Option<(String, Vec<String>)> {
    let text = std::fs::read_to_string(file).ok()?;
    if file.extension().is_some_and(|e| e == "toml") {
        let doc: toml_edit::DocumentMut = text.parse().ok()?;
        let row = doc.get("mcp_servers")?.get(MCP_NAME)?;
        let args = row.get("args").and_then(|a| a.as_array()).map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });
        Some((row.get("command")?.as_str()?.to_string(), args?))
    } else {
        let v: Value = serde_json::from_str(&text).ok()?;
        let row = &v["mcpServers"][MCP_NAME];
        let args = serde_json::from_value(row["args"].clone()).ok()?;
        Some((row["command"].as_str()?.to_string(), args))
    }
}

/// Codex and Grok keep a turned-off server's table, with `enabled = false`.
fn mcp_disabled(file: &Path) -> bool {
    let doc = std::fs::read_to_string(file)
        .ok()
        .and_then(|t| t.parse::<toml_edit::DocumentMut>().ok());
    doc.and_then(|d| {
        d.get("mcp_servers")?
            .get(MCP_NAME)?
            .get("enabled")?
            .as_bool()
    }) == Some(false)
}

fn grok_config_file() -> PathBuf {
    crate::hook::grok_home().join("config.toml")
}

fn claude_settings_file() -> PathBuf {
    claude_dir().join("settings.json")
}

fn claude(cmd: &HookCommand, remove: bool) -> Result<Vec<String>> {
    let file = claude_settings_file();
    if remove && !file.exists() {
        return Ok(vec![]);
    }
    let mut root = read_json_object(&file)?;
    backup_once(&file)?;
    let wanted = if remove {
        vec![]
    } else {
        CLAUDE_EVENTS
            .iter()
            .map(|(event, timeout)| {
                (
                    event.to_string(),
                    json!({"hooks": [{"type": "command", "command": cmd.line("claude", event), "timeout": timeout}]}),
                )
            })
            .collect()
    };
    merge_groups(&mut root, wanted);
    write_json(&file, &root)?;
    Ok(vec![file.display().to_string()])
}

fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| config::home_dir().join(".codex"))
}

fn codex(cmd: &HookCommand, remove: bool) -> Result<Vec<String>> {
    let hooks_file = codex_home().join("hooks.json");
    let config_file = codex_home().join("config.toml");
    if remove && !hooks_file.exists() {
        return Ok(vec![]);
    }
    let mut root = read_json_object(&hooks_file)?;
    // The trust rows in config.toml follow handler positions in hooks.json, so one written
    // without the other cannot be repaired by a rerun: both are read first and staged together.
    let mut doc: toml_edit::DocumentMut = read_text(&config_file)?
        .parse()
        .with_context(|| format!("parse {}", config_file.display()))?;
    backup_once(&hooks_file)?;
    backup_once(&config_file)?;
    let before = codex_trust_keys(&hooks_file, &root);
    let wanted = if remove {
        vec![]
    } else {
        CODEX_EVENTS
            .iter()
            .map(|(event, matcher, timeout, injects)| {
                let mut handler = json!({"type": "command", "command": cmd.line("codex", event), "timeout": timeout});
                if *injects {
                    // 0 = no spill of long context to a file the model never reads.
                    handler["additionalContextLimit"] = json!(0);
                }
                let mut group = json!({"hooks": [handler]});
                if let Some(m) = matcher {
                    group["matcher"] = json!(m);
                }
                (event.to_string(), group)
            })
            .collect()
    };
    merge_groups(&mut root, wanted);
    let delta = codex_trust_delta(&before, &codex_trust_keys(&hooks_file, &root));
    codex_write_trust(&mut doc, &delta);
    let hooks = stage(&hooks_file, &json_text(&root)?)?;
    let config = stage(&config_file, &doc.to_string())?;
    hooks.commit()?;
    config.commit()?;
    Ok(vec![
        hooks_file.display().to_string(),
        config_file.display().to_string(),
    ])
}

struct TrustKey {
    key: String,
    hash: String,
    ours: bool,
}

/// `[hooks.state."<hooks.json>:<event>:<group>:<handler>"]` key and hash of every handler.
fn codex_trust_keys(hooks_file: &Path, root: &Value) -> Vec<TrustKey> {
    let path = hooks_file.to_string_lossy();
    let mut out = Vec::new();
    let Some(hooks) = root["hooks"].as_object() else {
        return out;
    };
    for (event, groups) in hooks {
        for (gi, group) in groups.as_array().into_iter().flatten().enumerate() {
            for (hi, handler) in group["hooks"].as_array().into_iter().flatten().enumerate() {
                out.push(TrustKey {
                    key: format!("{path}:{}:{gi}:{hi}", snake(event)),
                    hash: codex_trust_hash(event, group["matcher"].as_str(), handler),
                    ours: is_our_handler(handler),
                });
            }
        }
    }
    out
}

#[derive(Debug, Default, PartialEq)]
struct TrustDelta {
    /// Our rows at positions we no longer occupy.
    stale: Vec<String>,
    /// Other groups' rows whose position shifted: (old key, new key).
    moved: Vec<(String, String)>,
    /// Our rows at their current positions.
    fresh: Vec<(String, String)>,
}

/// Trust rows are keyed by position, so dropping or re-appending our groups shifts the groups
/// after them; their rows follow them to the new key. merge_groups only drops ours and appends
/// ours, so the k-th other handler before the merge is the k-th other handler after it.
fn codex_trust_delta(before: &[TrustKey], after: &[TrustKey]) -> TrustDelta {
    TrustDelta {
        stale: before
            .iter()
            .filter(|b| b.ours && !after.iter().any(|a| a.ours && a.key == b.key))
            .map(|b| b.key.clone())
            .collect(),
        moved: before
            .iter()
            .filter(|b| !b.ours)
            .zip(after.iter().filter(|a| !a.ours))
            .filter(|(b, a)| b.key != a.key)
            .map(|(b, a)| (b.key.clone(), a.key.clone()))
            .collect(),
        fresh: after
            .iter()
            .filter(|a| a.ours)
            .map(|a| (a.key.clone(), a.hash.clone()))
            .collect(),
    }
}

/// Applies a delta in order: drop stale, move shifted, write fresh (a moved row may land on a
/// key that was ours, and our new key may be one a moved row just left).
fn codex_write_trust(doc: &mut toml_edit::DocumentMut, delta: &TrustDelta) {
    let root = doc.as_table_mut();
    if delta.fresh.is_empty() && !root.contains_key("hooks") {
        return;
    }
    let hooks = root
        .entry("hooks")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    if let Some(t) = hooks.as_table_mut() {
        t.set_implicit(true);
    }
    let Some(state) = hooks
        .as_table_mut()
        .map(|h| {
            h.entry("state")
                .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        })
        .and_then(toml_edit::Item::as_table_mut)
    else {
        return;
    };
    state.set_implicit(true);
    for key in &delta.stale {
        state.remove(key);
    }
    // Take every moved row out before putting any back: moves can chain or swap.
    let rows: Vec<_> = delta
        .moved
        .iter()
        .map(|(old, new)| (new, state.remove(old)))
        .collect();
    for (new, row) in rows {
        if let Some(row) = row {
            state[new.as_str()] = row;
        }
    }
    for (key, hash) in &delta.fresh {
        let mut row = toml_edit::Table::new();
        row["trusted_hash"] = toml_edit::value(hash.as_str());
        state[key.as_str()] = toml_edit::Item::Table(row);
    }
}

/// Codex's trust hash: sha256 of the canonical (key-sorted, compact) JSON of the normalized
/// group holding this one handler. Verified against rows Codex 0.155 wrote on this machine.
pub fn codex_trust_hash(event: &str, matcher: Option<&str>, handler: &Value) -> String {
    let name = snake(event);
    let late = name == "session_end" || name == "interrupt";
    let timeout = match handler["timeout"].as_u64() {
        Some(t) if late => t.min(3),
        Some(t) => t,
        None if late => 1,
        None => 600,
    };
    let mut normalized = json!({
        "async": false,
        "command": handler["command"],
        "timeout": timeout,
        "type": handler["type"],
    });
    if let Some(s) = handler.get("statusMessage") {
        normalized["statusMessage"] = s.clone();
    }
    if let Some(limit) = handler["additionalContextLimit"].as_u64()
        && limit != 2500
    {
        normalized["additionalContextLimit"] = json!(limit);
    }
    let mut group = json!({"event_name": name, "hooks": [normalized]});
    if let Some(m) = matcher {
        group["matcher"] = json!(m);
    }
    let digest = Sha256::digest(canonical(&group).as_bytes());
    format!("sha256:{digest:x}")
}

/// Compact JSON with the keys of every object sorted (Codex's hashing input).
fn canonical(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let fields: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canonical(&map[k])))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", inner.join(","))
        }
        other => other.to_string(),
    }
}

fn snake(event: &str) -> String {
    let mut out = String::new();
    for (i, c) in event.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// Grok reads every `~/.grok/hooks/*.json`; ours is `oboete.json`. Anything the developer put
/// into it is kept, like the other agents' files; the file goes only when nothing else is left
/// (hook.rs reads its presence as "Grok delivers its own events").
fn grok(cmd: &HookCommand, remove: bool) -> Result<Vec<String>> {
    let file = crate::hook::grok_hooks_file();
    if remove && !file.exists() {
        return Ok(vec![]);
    }
    let mut root = read_json_object(&file)?;
    backup_once(&file)?;
    let wanted = if remove {
        vec![]
    } else {
        GROK_EVENTS
            .iter()
            .map(|(event, timeout)| {
                let handler = json!({"type": "command", "command": cmd.line("grok", event), "timeout": timeout});
                (event.to_string(), json!({"hooks": [handler]}))
            })
            .collect()
    };
    merge_groups(&mut root, wanted);
    let empty = root.as_object().is_some_and(|o| {
        o.iter()
            .all(|(k, v)| k == "hooks" && v.as_object().is_some_and(|h| h.is_empty()))
    });
    if empty {
        if file.exists() {
            std::fs::remove_file(&file)?;
        }
    } else {
        write_json(&file, &root)?;
    }
    Ok(vec![file.display().to_string()])
}

fn agy_dir() -> PathBuf {
    config::home_dir().join(".gemini")
}

fn agy_available(dir: &Path, on_path: bool) -> bool {
    dir.is_dir() || on_path
}

fn agy_spec(cmd: &HookCommand, windows: bool) -> Result<Value> {
    let mut spec = serde_json::Map::new();
    for event in AGY_FLAT_EVENTS {
        spec.insert(
            event.to_string(),
            json!([{"type":"command", "command":cmd.agy_line(event, windows)?, "timeout":10}]),
        );
    }
    spec.insert(
        "PostToolUse".to_string(),
        json!([{"matcher":"*", "hooks":[{"type":"command", "command":cmd.agy_line("PostToolUse", windows)?, "timeout":10}]}]),
    );
    Ok(Value::Object(spec))
}

/// agy reads two shared JSON files; only the named hook `oboete` and `mcpServers.oboete` are
/// ours. Both are staged before either is replaced. An `oboete` server entry keeps the fields
/// the developer set on it (`disabled`); an emptied file stays as `{}`.
fn agy_files(dir: &Path, cmd: &HookCommand, remove: bool, windows: bool) -> Result<Vec<String>> {
    let hooks = dir.join("config/hooks.json");
    let mcp = dir.join("config/mcp_config.json");
    let spec = if remove {
        None
    } else {
        Some(agy_spec(cmd, windows)?)
    };
    let mut hooks_root = read_json_object(&hooks)?;
    let mut mcp_root = read_json_object(&mcp)?;
    let (old_hooks, old_mcp) = (hooks_root.clone(), mcp_root.clone());

    let named = hooks_root.as_object_mut().expect("checked JSON object");
    match spec {
        Some(spec) => named.insert(MCP_NAME.to_string(), spec),
        None => named.remove(MCP_NAME),
    };
    let root = mcp_root.as_object_mut().expect("checked JSON object");
    if remove {
        if let Some(servers) = root.get_mut("mcpServers").and_then(Value::as_object_mut) {
            servers.remove(MCP_NAME);
        }
    } else {
        let servers = root
            .entry("mcpServers")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| anyhow!("{}: mcpServers is not a JSON object", mcp.display()))?;
        let entry = servers.entry(MCP_NAME).or_insert_with(|| json!({}));
        anyhow::ensure!(
            entry.is_object(),
            "{}: oboete server is not a JSON object",
            mcp.display()
        );
        entry["command"] = json!(cmd.exe);
        entry["args"] = json!(cmd.mcp_args());
    }

    let mut staged = Vec::new();
    for (file, new, old) in [
        (&hooks, &hooks_root, &old_hooks),
        (&mcp, &mcp_root, &old_mcp),
    ] {
        if new != old {
            staged.push((file, stage(file, &json_text(new)?)?));
        }
    }
    for (file, _) in &staged {
        backup_once(file)?;
    }
    let mut changed = Vec::new();
    for (file, s) in staged {
        s.commit()?;
        changed.push(file.display().to_string());
    }
    Ok(changed)
}

fn agy_hooks_status(file: &Path, cmd: &HookCommand, windows: bool) -> String {
    let root = match read_json_object(file) {
        Ok(root) => root,
        Err(e) => return format!("unreadable: {e}"),
    };
    let Some(ours) = root.get(MCP_NAME) else {
        return "not wired (run `oboete setup agy`)".into();
    };
    if ours["enabled"] == false {
        return "registered but turned off (`enabled: false`)".into();
    }
    let wanted = match agy_spec(cmd, windows) {
        Ok(wanted) => wanted,
        Err(e) => return format!("cannot run: {e}"),
    };
    if ours != &wanted {
        "hooks differ from expected (rerun `oboete setup agy`)".into()
    } else {
        "4 hooks wired".into()
    }
}

fn agy_mcp_disabled(file: &Path) -> bool {
    read_json_object(file)
        .ok()
        .is_some_and(|v| v["mcpServers"][MCP_NAME]["disabled"] == true)
}

/// `oboete doctor`: one screen of what is wired, what is stored and whether providers can run.
pub fn doctor(home: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    println!("oboete {} at {}", env!("CARGO_PKG_VERSION"), exe.display());
    let db_path = home.join("oboete.db");
    let size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
    println!("home {} (db {} KB)", home.display(), size / 1024);
    if db_path.exists() {
        let conn = db::open(home)?;
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0) };
        println!(
            "  sessions {} | raw events {} | observations {} | summaries {}",
            count("SELECT COUNT(*) FROM sessions"),
            count("SELECT COUNT(*) FROM events"),
            count("SELECT COUNT(*) FROM observations"),
            count("SELECT COUNT(*) FROM summaries")
        );
        let mut stmt = conn.prepare(
            "SELECT provider, outcome, ms, COALESCE(detail,'') FROM provider_calls ORDER BY id DESC LIMIT 5",
        )?;
        let rows: Vec<String> = stmt
            .query_map([], |r| {
                Ok(format!(
                    "{} {} {}ms {}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?.chars().take(60).collect::<String>()
                ))
            })?
            .collect::<Result<_, _>>()?;
        if !rows.is_empty() {
            println!("  last provider calls:");
            for r in rows {
                println!("    {r}");
            }
        }
    }
    let exe_str = exe
        .canonicalize()
        .unwrap_or(exe)
        .to_string_lossy()
        .into_owned();
    let wired = |file: &Path, agent: &str| -> String {
        match read_json_object(file) {
            Ok(root) => {
                let mut n = 0;
                let mut stale = 0;
                for groups in root["hooks"]
                    .as_object()
                    .into_iter()
                    .flat_map(|o| o.values())
                {
                    for g in groups
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|g| has_ours(g))
                    {
                        n += 1;
                        let cmd = g["hooks"][0]["command"].as_str().unwrap_or("");
                        if !cmd.contains(&exe_str) || !cmd.contains(&format!(" hook {agent} ")) {
                            stale += 1;
                        }
                    }
                }
                match (n, stale) {
                    (0, _) => "not wired (run `oboete setup`)".to_string(),
                    (_, 0) => format!("{n} hooks wired"),
                    _ => {
                        format!("{n} hooks, {stale} point at another binary (rerun `oboete setup`)")
                    }
                }
            }
            Err(e) => format!("unreadable: {e}"),
        }
    };
    let want = HookCommand::current(home)?;
    println!("agents:");
    println!("  claude  {}", wired(&claude_settings_file(), "claude"));
    let codex_hooks = codex_home().join("hooks.json");
    let mut codex_line = wired(&codex_hooks, "codex");
    if let Ok(root) = read_json_object(&codex_hooks) {
        let keys: Vec<_> = codex_trust_keys(&codex_hooks, &root)
            .into_iter()
            .filter(|k| k.ours)
            .collect();
        if !keys.is_empty() {
            let text =
                std::fs::read_to_string(codex_home().join("config.toml")).unwrap_or_default();
            let trusted = keys
                .iter()
                .filter(|k| text.contains(&format!("\"{}\"", k.key)) && text.contains(&k.hash))
                .count();
            codex_line.push_str(&format!(", trust rows {trusted}/{}", keys.len()));
        }
    }
    println!("  codex   {codex_line}");
    println!(
        "  grok    {}",
        wired(&crate::hook::grok_hooks_file(), "grok")
    );
    println!(
        "  agy     {}",
        agy_hooks_status(&agy_dir().join("config/hooks.json"), &want, cfg!(windows))
    );
    let mcp = |file: &Path| -> &str {
        match mcp_command(file) {
            Some(_) if mcp_disabled(file) => "registered but turned off (`enabled = false`)",
            Some((c, args)) if c == want.exe && args == want.mcp_args() => "registered",
            Some(_) => "registered with another binary or home (rerun `oboete setup`)",
            None => "not registered (run `oboete setup`)",
        }
    };
    println!("mcp server `{MCP_NAME}`:");
    println!("  claude  {}", mcp(&claude_mcp_file()));
    println!("  codex   {}", mcp(&codex_home().join("config.toml")));
    println!("  grok    {}", mcp(&grok_config_file()));
    let agy_mcp = agy_dir().join("config/mcp_config.json");
    let agy_status = if agy_mcp_disabled(&agy_mcp) {
        "registered but turned off (`disabled: true`)"
    } else {
        mcp(&agy_mcp)
    };
    println!("  agy     {agy_status}");
    println!("providers (chain order):");
    for p in config::load(home)?.providers {
        let state = match &p {
            config::Provider::Openai {
                key_file, model, ..
            } => match key_file {
                Some(f) if f.exists() => format!("key ok, model {model}"),
                Some(f) => format!("key file missing: {}", f.display()),
                None => format!("no key, model {model}"),
            },
            config::Provider::Cli { cli, .. } => {
                if on_path(cli) {
                    "on PATH".to_string()
                } else {
                    "not on PATH".to_string()
                }
            }
        };
        println!("  {:<11} {state}", p.name());
    }
    Ok(())
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths)
            // `.exe` only on Windows: WSL puts Windows' `claude.exe` on PATH, which `claude` does not run.
            .any(|d| {
                d.join(bin).is_file() || (cfg!(windows) && d.join(format!("{bin}.exe")).is_file())
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agy_setup_round_trip_preserves_other_entries() {
        let dir = std::env::temp_dir().join(format!("oboete-agy-setup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hooks = dir.join("config/hooks.json");
        let mcp = dir.join("config/mcp_config.json");
        std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
        std::fs::write(&hooks, "{\"other\":{\"Stop\":[]}}\n").unwrap();
        std::fs::write(&mcp, "{\"mcpServers\":{\"other\":{\"command\":\"x\"}}}\n").unwrap();
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: Some("/h".into()),
        };
        agy_files(&dir, &cmd, false, false).unwrap();
        let h = read_json_object(&hooks).unwrap();
        assert_eq!(h["other"], json!({"Stop": []}));
        assert!(h["oboete"].get("PreToolUse").is_none());
        assert_eq!(
            h["oboete"]["SessionStart"][0],
            json!({"type":"command","command":"/x/oboete --home /h hook agy SessionStart","timeout":10})
        );
        assert_eq!(h["oboete"]["PreInvocation"][0]["timeout"], 10);
        assert_eq!(h["oboete"]["Stop"][0]["timeout"], 10);
        assert_eq!(h["oboete"]["PostToolUse"][0]["matcher"], "*");
        assert_eq!(
            h["oboete"]["PostToolUse"][0]["hooks"][0]["command"],
            "/x/oboete --home /h hook agy PostToolUse"
        );
        let m = read_json_object(&mcp).unwrap();
        assert_eq!(m["mcpServers"]["other"], json!({"command":"x"}));
        assert_eq!(
            m["mcpServers"]["oboete"],
            json!({"command":"/x/oboete","args":["--home","/h","mcp"]})
        );
        let first_hooks = std::fs::read(&hooks).unwrap();
        let first_mcp = std::fs::read(&mcp).unwrap();
        agy_files(&dir, &cmd, false, false).unwrap();
        assert_eq!(std::fs::read(&hooks).unwrap(), first_hooks);
        assert_eq!(std::fs::read(&mcp).unwrap(), first_mcp);
        assert_eq!(
            std::fs::read_to_string(hooks.with_file_name("hooks.json.oboete.bak")).unwrap(),
            "{\"other\":{\"Stop\":[]}}\n"
        );
        assert_eq!(
            std::fs::read_to_string(mcp.with_file_name("mcp_config.json.oboete.bak")).unwrap(),
            "{\"mcpServers\":{\"other\":{\"command\":\"x\"}}}\n"
        );
        agy_files(&dir, &cmd, true, false).unwrap();
        assert_eq!(
            read_json_object(&hooks).unwrap(),
            json!({"other":{"Stop":[]}})
        );
        assert_eq!(
            read_json_object(&mcp).unwrap(),
            json!({"mcpServers":{"other":{"command":"x"}}})
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn agy_remove_refuses_read_only_owned_files_before_touching_either_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("oboete-agy-locked-{}", std::process::id()));
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        for locked in ["hooks.json", "mcp_config.json"] {
            let _ = std::fs::remove_dir_all(&dir);
            agy_files(&dir, &cmd, false, false).unwrap();
            let hooks = dir.join("config/hooks.json");
            let mcp = dir.join("config/mcp_config.json");
            let before_hooks = std::fs::read(&hooks).unwrap();
            let before_mcp = std::fs::read(&mcp).unwrap();
            let target = if locked == "hooks.json" { &hooks } else { &mcp };
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o444)).unwrap();
            assert!(agy_files(&dir, &cmd, true, false).is_err());
            assert_eq!(std::fs::read(&hooks).unwrap(), before_hooks);
            assert_eq!(std::fs::read(&mcp).unwrap(), before_mcp);
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agy_fresh_install_removes_to_empty_objects_and_missing_install_skips() {
        let dir = std::env::temp_dir().join(format!("oboete-agy-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!agy_available(&dir, false));
        assert!(agy_available(&dir, true));
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        let written = agy_files(&dir, &cmd, false, false).unwrap();
        assert_eq!(written.len(), 2);
        agy_files(&dir, &cmd, true, false).unwrap();
        assert_eq!(
            read_json_object(&dir.join("config/hooks.json")).unwrap(),
            json!({})
        );
        assert_eq!(
            read_json_object(&dir.join("config/mcp_config.json")).unwrap(),
            json!({"mcpServers": {}})
        );
        assert!(agy_files(&dir, &cmd, true, false).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agy_windows_command_is_raw_and_rejects_spaces_before_writing() {
        let dir = std::env::temp_dir().join(format!("oboete-agy-windows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cmd = HookCommand {
            exe: "C:/tools/oboete.exe".into(),
            home: Some("C:/data/oboete".into()),
        };
        agy_files(&dir, &cmd, false, true).unwrap();
        let hooks = read_json_object(&dir.join("config/hooks.json")).unwrap();
        assert_eq!(
            hooks["oboete"]["Stop"][0]["command"],
            "C:/tools/oboete.exe --home C:/data/oboete hook agy Stop"
        );
        let bad = HookCommand {
            exe: "C:/Program Files/oboete.exe".into(),
            home: None,
        };
        assert!(
            agy_files(&dir, &bad, false, true)
                .unwrap_err()
                .to_string()
                .contains("space-free")
        );
        assert_eq!(
            read_json_object(&dir.join("config/hooks.json")).unwrap(),
            hooks
        );
        let bad_home = HookCommand {
            exe: "C:/tools/oboete.exe".into(),
            home: Some("C:/My Data".into()),
        };
        assert!(agy_files(&dir, &bad_home, false, true).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agy_doctor_checks_expected_hooks_and_disabled_mcp() {
        let dir = std::env::temp_dir().join(format!("oboete-agy-doctor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: None,
        };
        agy_files(&dir, &cmd, false, false).unwrap();
        let hooks = dir.join("config/hooks.json");
        let mcp = dir.join("config/mcp_config.json");
        assert_eq!(agy_hooks_status(&hooks, &cmd, false), "4 hooks wired");
        assert!(!agy_mcp_disabled(&mcp));
        let mut root = read_json_object(&hooks).unwrap();
        root["oboete"]["PreInvocation"][0]["timeout"] = json!(10000);
        write_json(&hooks, &root).unwrap();
        assert!(agy_hooks_status(&hooks, &cmd, false).contains("rerun"));
        let mut root = read_json_object(&mcp).unwrap();
        root["mcpServers"]["oboete"]["disabled"] = json!(true);
        write_json(&mcp, &root).unwrap();
        assert!(agy_mcp_disabled(&mcp));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Rows Codex 0.155.1 itself wrote into the owner's config.toml for these two handlers.
    #[test]
    fn codex_trust_hash_matches_rows_codex_wrote() {
        let h = json!({"type": "command", "command": "node /home/jura/.codex/hooks/caveman.cjs", "timeout": 5, "statusMessage": "Loading caveman mode"});
        assert_eq!(
            codex_trust_hash("SessionStart", Some("startup|resume|clear|compact"), &h),
            "sha256:85139b0f4a7d3c7548afa35ac8377be0343cfb9b8657fe3854f5fd62d8241972"
        );
        let h = json!({"type": "command", "command": "node /home/jura/.codex/hooks/caveman.cjs", "timeout": 5, "statusMessage": "Tracking caveman mode"});
        assert_eq!(
            codex_trust_hash("UserPromptSubmit", None, &h),
            "sha256:d3b11f4d6ea1ff2411c3bc9953d9cac72e4f0cde061a95ffadb488139516289e"
        );
    }

    #[test]
    fn ownership_is_the_exact_hook_command_shape() {
        let ours = |c: &str| is_our_handler(&json!({"type": "command", "command": c}));
        assert!(ours("/x/oboete hook claude Stop"));
        assert!(ours("/x/oboete --home /h hook codex SessionStart"));
        assert!(ours(
            "'/my dir/it'\\''s/oboete' --home '/h o/me' hook grok PostToolUse"
        ));
        assert!(ours("C:/Users/x/.cargo/bin/oboete.exe hook claude Stop"));
        assert!(ours("/opt/oboete-0.1 hook claude Stop"));
        assert!(!ours("python /tools/oboete_report.py hook audit"));
        assert!(!ours("/x/oboete hook audit Stop"));
        assert!(!ours("/x/oboete observe"));
        assert!(!ours("/x/oboete hook claude"));
        assert!(!ours("/x/my-oboete hook claude Stop"));
        assert_eq!(
            shell_words("a 'b c' 'd'\\''e'  f"),
            vec!["a", "b c", "d'e", "f"]
        );
        let cmd = HookCommand {
            exe: "/my dir/oboete".into(),
            home: Some("/h o/me".into()),
        };
        assert!(ours(&cmd.line("codex", "Stop")));
    }

    #[test]
    fn mcp_registrations_round_trip_and_leave_other_servers() {
        let dir = std::env::temp_dir().join(format!("oboete-mcp-setup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cmd = HookCommand {
            exe: "/x/oboete".into(),
            home: Some("/h".into()),
        };
        let registered = |f: &Path| mcp_command(f).map(|(c, _)| c);
        assert_eq!(
            {
                let f = dir.join("args.toml");
                std::fs::write(
                    &f,
                    "[mcp_servers.oboete]\ncommand = \"/x/oboete\"\nargs = [\"mcp\"]\n",
                )
                .unwrap();
                mcp_command(&f)
            },
            Some(("/x/oboete".to_string(), vec!["mcp".to_string()]))
        );
        // TOML (Codex, Grok): other servers and sections survive, ours comes and goes.
        let toml = dir.join("config.toml");
        std::fs::write(&toml, "model = \"x\"\n\n[mcp_servers.other]\ncommand = \"o\"\n\n[hooks.state.\"k\"]\ntrusted_hash = \"h\"\n").unwrap();
        toml_mcp(&toml, &cmd, false).unwrap();
        let text = std::fs::read_to_string(&toml).unwrap();
        assert!(text.contains("[mcp_servers.oboete]\ncommand = \"/x/oboete\"\nargs = [\"--home\", \"/h\", \"mcp\"]"), "{text}");
        assert!(
            text.contains("[mcp_servers.other]")
                && text.contains("model = \"x\"")
                && text.contains("[hooks.state.\"k\"]")
        );
        assert!(!text.contains("\n[mcp_servers]\n"), "{text}");
        assert_eq!(registered(&toml).as_deref(), Some("/x/oboete"));
        toml_mcp(&toml, &cmd, true).unwrap();
        let text = std::fs::read_to_string(&toml).unwrap();
        assert!(
            !text.contains("oboete") && text.contains("[mcp_servers.other]"),
            "{text}"
        );
        assert!(mcp_command(&toml).is_none());
        // An empty file gains only our table; removing it leaves the file without mcp_servers.
        let fresh = dir.join("fresh.toml");
        toml_mcp(&fresh, &cmd, false).unwrap();
        assert_eq!(registered(&fresh).as_deref(), Some("/x/oboete"));
        toml_mcp(&fresh, &cmd, true).unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap().trim(), "");
        // A rerun sets only command/args: keys the developer put on our entry stay.
        let own = dir.join("own.toml");
        std::fs::write(&own, "[mcp_servers.oboete]\ncommand = \"/old/oboete\"\nargs = [\"mcp\"]\nenabled = false\nstartup_timeout_sec = 60\n").unwrap();
        toml_mcp(&own, &cmd, false).unwrap();
        assert!(mcp_disabled(&own) && !mcp_disabled(&toml));
        // Removing from a config that does not exist creates nothing.
        let absent = dir.join("absent").join("config.toml");
        toml_mcp(&absent, &cmd, true).unwrap();
        assert!(!absent.exists() && !dir.join("absent").exists());
        let text = std::fs::read_to_string(&own).unwrap();
        assert!(
            text.contains("command = \"/x/oboete\"")
                && text.contains("enabled = false")
                && text.contains("startup_timeout_sec = 60"),
            "{text}"
        );
        // An explicit [mcp_servers] header keeps its comment; the inline form is edited in place.
        let header = dir.join("header.toml");
        std::fs::write(
            &header,
            "# my servers\n[mcp_servers]\n\n[mcp_servers.a]\ncommand = \"x\"\n",
        )
        .unwrap();
        toml_mcp(&header, &cmd, false).unwrap();
        let text = std::fs::read_to_string(&header).unwrap();
        assert!(text.starts_with("# my servers\n[mcp_servers]\n"), "{text}");
        let inline = dir.join("inline.toml");
        std::fs::write(&inline, "mcp_servers = { other = { command = \"o\" } }\n").unwrap();
        toml_mcp(&inline, &cmd, false).unwrap();
        assert_eq!(registered(&inline).as_deref(), Some("/x/oboete"));
        assert!(std::fs::read_to_string(&inline).unwrap().contains("other"));
        toml_mcp(&inline, &cmd, true).unwrap();
        assert!(mcp_command(&inline).is_none());
        // Remove keeps an explicit [mcp_servers] header; a dotted prefix goes with our entry.
        std::fs::write(&header, "# my servers\n[mcp_servers]\n").unwrap();
        toml_mcp(&header, &cmd, false).unwrap();
        toml_mcp(&header, &cmd, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(&header).unwrap(),
            "# my servers\n[mcp_servers]\n"
        );
        let dotted = dir.join("dotted.toml");
        std::fs::write(&dotted, "a = 1\nmcp_servers.oboete.command = \"/old\"\n").unwrap();
        toml_mcp(&dotted, &cmd, true).unwrap();
        assert_eq!(std::fs::read_to_string(&dotted).unwrap(), "a = 1\n");
        // A file that cannot be read stops setup and stays as it was.
        let latin = dir.join("latin.toml");
        std::fs::write(&latin, b"# caf\xe9\n[cli]\nfoo = 1\n").unwrap();
        assert!(toml_mcp(&latin, &cmd, false).is_err());
        assert_eq!(
            std::fs::read(&latin).unwrap(),
            b"# caf\xe9\n[cli]\nfoo = 1\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_keeps_mode_and_symlink() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("oboete-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.json");
        std::fs::write(&real, "{}").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
        let link = dir.join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, "{\"a\": 1}\n").unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "{\"a\": 1}\n");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&real), 0o640);
        let new = dir.join("sub").join("new.json");
        write_atomic(&new, "{}\n").unwrap();
        assert_eq!(mode(&new), 0o600);
        assert_eq!(std::fs::read_dir(dir.join("sub")).unwrap().count(), 1);
        let chain = dir.join("chain.json");
        std::os::unix::fs::symlink("dangling.json", &chain).unwrap();
        // A link whose target does not exist yet: the target is created, the link stays.
        let dangling = dir.join("dangling.json");
        std::os::unix::fs::symlink("sub/later.json", &dangling).unwrap();
        write_atomic(&dangling, "{}\n").unwrap();
        assert!(
            dangling
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("sub/later.json")).unwrap(),
            "{}\n"
        );
        // Through a chain of links, too.
        std::fs::remove_file(dir.join("sub/later.json")).unwrap();
        write_atomic(&chain, "[]\n").unwrap();
        assert!(
            dangling
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("sub/later.json")).unwrap(),
            "[]\n"
        );
        // A staged file that is never committed leaves nothing behind.
        drop(stage(&dir.join("sub/never.json"), "{}").unwrap());
        assert_eq!(std::fs::read_dir(dir.join("sub")).unwrap().count(), 2);
        // A file locked read-only is not replaced.
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(write_atomic(&real, "{}").is_err());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "{\"a\": 1}\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn merge_keeps_user_groups_and_is_idempotent() {
        let user = json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "echo hi"}]});
        let mut root = json!({"env": {"A": "1"}, "hooks": {"PreToolUse": [user.clone()], "Stop": [user.clone()]}});
        let ours = || {
            vec![(
                "Stop".to_string(),
                json!({"hooks": [{"type": "command", "command": "/x/oboete hook claude Stop"}]}),
            )]
        };
        merge_groups(&mut root, ours());
        merge_groups(&mut root, ours());
        assert_eq!(root["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(root["hooks"]["Stop"][0], user);
        assert_eq!(root["hooks"]["PreToolUse"][0], user);
        merge_groups(&mut root, vec![]);
        assert_eq!(root["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(root["env"]["A"], "1");

        // A group shared with the developer's handler keeps that handler in place.
        let mixed = json!({"matcher": "Bash", "hooks": [
            {"type": "command", "command": "/x/oboete hook claude Stop"},
            {"type": "command", "command": "echo hi"}
        ]});
        let mut root = json!({"hooks": {"Stop": [mixed]}});
        merge_groups(&mut root, ours());
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(
            stop[0]["hooks"],
            json!([{"type": "command", "command": "echo hi"}])
        );
        assert_eq!(stop[0]["matcher"], "Bash");
        assert!(has_ours(&stop[1]));
        merge_groups(&mut root, vec![]);
        assert_eq!(root["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(!has_ours(&root["hooks"]["Stop"][0]));
    }

    #[test]
    fn codex_trust_rows_are_written_as_tables_and_removed() {
        let text = "[features]\nmemories = true\n\n[hooks.state.\"/h/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:aa\"\n\n[mcp_servers.x]\ncommand = \"y\"\n";
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        let fresh = vec![(
            "/h/hooks.json:session_start:1:0".to_string(),
            "sha256:bb".to_string(),
        )];
        codex_write_trust(
            &mut doc,
            &TrustDelta {
                fresh: fresh.clone(),
                ..Default::default()
            },
        );
        let out = doc.to_string();
        assert!(
            out.contains(
                "[hooks.state.\"/h/hooks.json:session_start:1:0\"]\ntrusted_hash = \"sha256:bb\""
            ),
            "{out}"
        );
        assert!(out.contains("[hooks.state.\"/h/hooks.json:stop:0:0\"]"));
        assert!(out.contains("[mcp_servers.x]"));
        codex_write_trust(
            &mut doc,
            &TrustDelta {
                stale: vec![fresh[0].0.clone()],
                ..Default::default()
            },
        );
        let out = doc.to_string();
        assert!(!out.contains("session_start:1:0"), "{out}");
        assert!(out.contains("[features]\nmemories = true"));
    }

    #[test]
    fn trust_rows_follow_user_groups_that_shift() {
        let file = Path::new("/h/hooks.json");
        let ours = json!({"hooks": [{"type": "command", "command": "/x/oboete hook codex Stop", "timeout": 5}]});
        let user = json!({"hooks": [{"type": "command", "command": "echo hi", "timeout": 5}]});
        let keys = |groups: Vec<Value>| codex_trust_keys(file, &json!({"hooks": {"Stop": groups}}));
        let before = keys(vec![ours.clone(), user.clone()]);
        let toml = "[hooks.state.\"/h/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:ours-old\"\n\n[hooks.state.\"/h/hooks.json:stop:1:0\"]\ntrusted_hash = \"sha256:user\"\n";
        let k = |g: usize| format!("/h/hooks.json:stop:{g}:0");
        let kh = |g: usize, h: usize| format!("/h/hooks.json:stop:{g}:{h}");

        // Rerun: ours is re-appended after the user's group, which moves from 1 to 0.
        let d = codex_trust_delta(&before, &keys(vec![user.clone(), ours.clone()]));
        assert_eq!(d.stale, vec![k(0)]);
        assert_eq!(d.moved, vec![(k(1), k(0))]);
        assert_eq!(d.fresh.len(), 1);
        assert_eq!(d.fresh[0].0, k(1));
        let mut doc: toml_edit::DocumentMut = toml.parse().unwrap();
        codex_write_trust(&mut doc, &d);
        let out = doc.to_string();
        assert!(
            out.contains(
                "[hooks.state.\"/h/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:user\""
            ),
            "{out}"
        );
        assert!(
            out.contains(&format!(
                "[hooks.state.\"/h/hooks.json:stop:1:0\"]\ntrusted_hash = \"{}\"",
                d.fresh[0].1
            )),
            "{out}"
        );
        assert!(!out.contains("ours-old"));

        // Remove: the user's group moves from 1 to 0 and keeps its row; ours is gone.
        let d = codex_trust_delta(&before, &keys(vec![user.clone()]));
        assert_eq!(d.stale, vec![k(0)]);
        assert_eq!(d.moved, vec![(k(1), k(0))]);
        assert!(d.fresh.is_empty());
        let mut doc: toml_edit::DocumentMut = toml.parse().unwrap();
        codex_write_trust(&mut doc, &d);
        let out = doc.to_string();
        assert!(
            out.contains(
                "[hooks.state.\"/h/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:user\""
            ),
            "{out}"
        );
        assert!(
            !out.contains("stop:1:0") && !out.contains("ours-old"),
            "{out}"
        );

        // Nothing shifts when ours already sits last.
        let last = keys(vec![user.clone(), ours.clone()]);
        let d = codex_trust_delta(&last, &last);
        assert!(d.stale.is_empty() && d.moved.is_empty() && d.fresh.len() == 1);

        // A handler of the developer's inside our group moves from 0:1 to 0:0 when ours leaves.
        let shared = json!({"hooks": [
            {"type": "command", "command": "/x/oboete hook codex Stop", "timeout": 5},
            {"type": "command", "command": "echo hi", "timeout": 5}
        ]});
        let d = codex_trust_delta(&keys(vec![shared]), &keys(vec![user.clone(), ours.clone()]));
        assert_eq!(d.stale, vec![kh(0, 0)]);
        assert_eq!(d.moved, vec![(kh(0, 1), kh(0, 0))]);
        assert_eq!(d.fresh[0].0, kh(1, 0));

        // Two identical user handlers (same hash) each keep their own row, by position.
        let twins = json!({"hooks": [
            {"type": "command", "command": "echo hi", "timeout": 5},
            {"type": "command", "command": "echo hi", "timeout": 5}
        ]});
        let same = keys(vec![twins.clone()]);
        assert_eq!(codex_trust_delta(&same, &same), TrustDelta::default());
        let d = codex_trust_delta(&keys(vec![ours.clone(), twins.clone()]), &same);
        assert_eq!(d.moved, vec![(kh(1, 0), kh(0, 0)), (kh(1, 1), kh(0, 1))]);
        let mut doc: toml_edit::DocumentMut = "[hooks.state.\"/h/hooks.json:stop:1:0\"]\ntrusted_hash = \"sha256:t0\"\n\n[hooks.state.\"/h/hooks.json:stop:1:1\"]\ntrusted_hash = \"sha256:t1\"\n".parse().unwrap();
        codex_write_trust(&mut doc, &d);
        let out = doc.to_string();
        assert!(
            out.contains("stop:0:0\"]\ntrusted_hash = \"sha256:t0\"")
                && out.contains("stop:0:1\"]\ntrusted_hash = \"sha256:t1\"")
                && !out.contains("stop:1:"),
            "{out}"
        );
    }
}
