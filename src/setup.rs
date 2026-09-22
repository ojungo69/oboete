//! `oboete setup <agent>` wires Claude Code, Codex and Grok Build hooks to this binary and
//! `--remove` takes exactly those entries out again; `oboete doctor` reports the state.
//! Only oboete's own entries are ever touched. The first write to a file the developer owned
//! leaves a `.oboete.bak` copy next to it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{config, db};

pub const AGENTS: [&str; 3] = ["claude", "codex", "grok"];
const BACKUP_SUFFIX: &str = ".oboete.bak";

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

pub fn run(home: &Path, agent: &str, remove: bool) -> Result<()> {
    let agents: Vec<&str> = if agent == "all" {
        AGENTS.to_vec()
    } else if AGENTS.contains(&agent) {
        vec![agent]
    } else {
        return Err(anyhow!(
            "unknown agent {agent}: use claude | codex | grok | all"
        ));
    };
    let cmd = HookCommand::current(home)?;
    for a in agents {
        let files = match a {
            "claude" => claude(&cmd, remove)?,
            "codex" => codex(&cmd, remove)?,
            _ => grok(&cmd, remove)?,
        };
        let verb = if remove { "removed from" } else { "written to" };
        println!("{a}: hooks {verb} {}", files.join(", "));
    }
    if !remove {
        println!("Hook files are read when an agent starts: restart running sessions.");
    }
    Ok(())
}

/// The command line every hook entry runs: this binary's absolute path plus `hook <agent> <event>`.
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
    fn line(&self, agent: &str, event: &str) -> String {
        let mut s = shell_quote(&self.exe);
        if let Some(h) = &self.home {
            s.push_str(" --home ");
            s.push_str(&shell_quote(h));
        }
        format!("{s} hook {agent} {event}")
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
        .and_then(|f| f.to_str())
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
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, format!("{}\n", serde_json::to_string_pretty(v)?))
        .with_context(|| format!("write {}", file.display()))
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

fn claude_settings_file() -> PathBuf {
    config::home_dir().join(".claude").join("settings.json")
}

fn claude(cmd: &HookCommand, remove: bool) -> Result<Vec<String>> {
    let file = claude_settings_file();
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
    let mut root = read_json_object(&hooks_file)?;
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
    write_json(&hooks_file, &root)?;
    let delta = codex_trust_delta(&before, &codex_trust_keys(&hooks_file, &root));

    let text = std::fs::read_to_string(&config_file).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("parse {}", config_file.display()))?;
    codex_write_trust(&mut doc, &delta);
    if let Some(dir) = config_file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&config_file, doc.to_string())
        .with_context(|| format!("write {}", config_file.display()))?;
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
            .any(|d| d.join(bin).is_file() || d.join(format!("{bin}.exe")).is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
