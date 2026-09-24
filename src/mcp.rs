//! `oboete mcp`: the memory as an MCP server over stdio, for the agent to search from inside a
//! session. Three tools, thin over `search`. The tokio runtime is built here and nowhere near
//! the hook path.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorData, Implementation, ServerCapabilities, ServerConfig,
};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::{db, repo, search};

#[derive(Clone)]
pub struct Oboete {
    home: PathBuf,
    /// The directory the agent launched us from. Its repository key is read per call: it
    /// changes when the repository gets an origin (and `observe` re-keys the rows).
    cwd: PathBuf,
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Terms, all required. A term of 3+ characters matches as a substring with Unicode case
    /// folding; a shorter one as a literal substring.
    query: String,
    /// Search every repository instead of the current one.
    #[serde(default)]
    all: Option<bool>,
    /// A repository to search instead of the current one: its path, or its key as results show
    /// it (e.g. `github.com/owner/name`).
    #[serde(default)]
    repo: Option<String>,
    /// Maximum number of hits (default 10, at most 100).
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetArgs {
    /// A document id from `search`: `o12` (observation), `s5` (session summary) or `p7` (prompt).
    id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct TimelineArgs {
    /// Every repository instead of the current one.
    #[serde(default)]
    all: Option<bool>,
    /// A repository instead of the current one: its path or its key.
    #[serde(default)]
    repo: Option<String>,
    /// Maximum number of sessions (default 20, at most 100).
    #[serde(default)]
    limit: Option<usize>,
}

/// A model can ask for any `limit`; the store is not dumped into one reply.
const MAX_LIMIT: usize = 100;

fn text(s: String) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(s)]))
}

/// A failure the model can act on (a wrong argument, an unknown id) is a tool result with
/// `isError`, not a protocol error, so the client hands it back to the model.
fn failed(s: String) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::error(vec![ContentBlock::text(s)]))
}

fn internal(e: anyhow::Error) -> ErrorData {
    ErrorData::internal_error(format!("{e:#}"), None)
}

#[tool_router]
impl Oboete {
    pub fn new(home: &Path, cwd: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
            cwd: cwd.to_path_buf(),
            tool_router: Self::tool_router(),
        }
    }

    /// `None` = every repository. Models send `null` and `""` for arguments they mean to leave
    /// out. `repo` is a directory or a repository key the store knows (as `timeline --all` and
    /// hits show it); anything else is an error, not a scope that matches nothing.
    fn scope(
        &self,
        conn: &rusqlite::Connection,
        all: Option<bool>,
        repo: Option<&str>,
    ) -> Result<Option<String>, String> {
        if all == Some(true) {
            return Ok(None);
        }
        match repo.filter(|r| !r.is_empty()) {
            None => Ok(Some(repo::key(&self.cwd))),
            Some(r) if Path::new(r).is_dir() => Ok(Some(crate::repo::key(Path::new(r)))),
            Some(r)
                if search::repos(conn)
                    .map_err(|e| e.to_string())?
                    .iter()
                    .any(|row| row.repo == r) =>
            {
                Ok(Some(r.to_string()))
            }
            Some(r) => Err(format!(
                "repo {r:?} is neither a directory nor a repository oboete knows: pass a repository's path or key"
            )),
        }
    }

    #[tool(
        name = "search",
        description = "Full-text search over what oboete remembers about this repository: observations (decisions, bug fixes, discoveries, preferences), session summaries and the developer's prompts from earlier coding sessions. Returns one hit per line: id, local time, kind, title, snippet. Use `get` for the full text."
    )]
    fn search(&self, Parameters(a): Parameters<SearchArgs>) -> Result<CallToolResult, ErrorData> {
        let conn = db::open(&self.home).map_err(internal)?;
        let scope = match self.scope(&conn, a.all, a.repo.as_deref()) {
            Ok(s) => s,
            Err(m) => return failed(m),
        };
        let hits = search::search(
            &conn,
            &a.query,
            scope.as_deref(),
            a.limit.unwrap_or(10).min(MAX_LIMIT),
        )
        .map_err(internal)?;
        let terms: Vec<&str> = a.query.split_whitespace().collect();
        let mut out = String::new();
        for h in hits {
            let snippet = search::snippet(&h.body, &terms, 160);
            if h.title.is_empty() {
                out.push_str(&format!("{} {} {} — {snippet}\n", h.doc, h.when, h.kind));
            } else {
                out.push_str(&format!(
                    "{} {} {} — {}: {snippet}\n",
                    h.doc, h.when, h.kind, h.title
                ));
            }
        }
        if out.is_empty() {
            out.push_str("no hits");
        }
        text(out)
    }

    #[tool(
        name = "get",
        description = "The full text of one remembered document by the id `search` returned (o12 = observation, s5 = session summary, p7 = prompt)."
    )]
    fn get(&self, Parameters(a): Parameters<GetArgs>) -> Result<CallToolResult, ErrorData> {
        let conn = db::open(&self.home).map_err(internal)?;
        match search::get(&conn, &a.id).map_err(internal)? {
            Some(h) => text(format!(
                "{} {} {} {}\n{}{}",
                h.doc,
                h.when,
                h.kind,
                h.repo,
                if h.title.is_empty() {
                    String::new()
                } else {
                    format!("{}\n\n", h.title)
                },
                h.body
            )),
            None => failed(format!("no document {} (ids come from search)", a.id)),
        }
    }

    #[tool(
        name = "timeline",
        description = "Earlier coding sessions in this repository, newest first, each with its summary: when, which agent, session id, summary."
    )]
    fn timeline(
        &self,
        Parameters(a): Parameters<TimelineArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let conn = db::open(&self.home).map_err(internal)?;
        let scope = match self.scope(&conn, a.all, a.repo.as_deref()) {
            Ok(s) => s,
            Err(m) => return failed(m),
        };
        let rows = search::timeline(
            &conn,
            scope.as_deref(),
            a.limit.unwrap_or(20).min(MAX_LIMIT),
        )
        .map_err(internal)?;
        let mut out = String::new();
        for r in rows {
            let summary = if r.summary.is_empty() {
                "(not summarized yet)".to_string()
            } else {
                r.summary.replace('\n', " ")
            };
            out.push_str(&format!(
                "{} {} {} {} — {summary}\n",
                r.when, r.agent, r.id, r.repo
            ));
        }
        if out.is_empty() {
            out.push_str("no sessions");
        }
        text(out)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Oboete {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("oboete", env!("CARGO_PKG_VERSION")))
            .with_instructions(
            "oboete is this developer's memory across coding sessions and agents. Call `search` when a task touches earlier decisions, bugs or preferences in this repository; `timeline` for what happened recently; `get` for a document's full text.",
        )
    }
}

/// Serve on stdin/stdout until the client disconnects.
pub fn run(home: &Path) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let server = Oboete::new(home, &cwd);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let service = server.serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> (PathBuf, Oboete) {
        let dir = std::env::temp_dir().join(format!("oboete-mcp-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("r/.git")).unwrap();
        std::fs::write(
            dir.join("r/.git/config"),
            "[remote \"origin\"]\n\turl = git@github.com:o/r.git\n",
        )
        .unwrap();
        let repo_key = repo::key(&dir.join("r"));
        assert_eq!(repo_key, "github.com/o/r");
        let mut conn = db::open(&dir).unwrap();
        db::upsert_session(
            &conn,
            "s1",
            "claude",
            &repo_key,
            &repo_key,
            1_700_000_000_000,
        )
        .unwrap();
        db::apply_batch(
            &mut conn,
            &db::PendingSession {
                id: "s1".into(),
                agent: "claude".into(),
                repo: repo_key.clone(),
                last_event_at: 1_700_000_000_000,
            },
            "test",
            "要約: 検索を実装した",
            &[db::Observation {
                kind: "decision".into(),
                title: "use the trigram tokenizer".into(),
                body: "FTS5 trigram indexes CJK by character".into(),
            }],
            i64::MAX,
        )
        .unwrap();
        let server = Oboete::new(&dir, &dir.join("r"));
        (dir, server)
    }

    fn body(r: CallToolResult) -> String {
        r.content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn tools_answer_from_the_store() {
        let (dir, s) = seeded();
        let search = |all: Option<bool>, repo: Option<&str>| {
            s.search(Parameters(SearchArgs {
                query: "trigram".into(),
                all,
                repo: repo.map(String::from),
                limit: None,
            }))
            .map(body)
        };
        let hits = search(None, None).unwrap();
        assert!(
            hits.starts_with("o1 ") && hits.contains("use the trigram tokenizer"),
            "{hits}"
        );
        // `null` / `""` stand for "left out"; another existing directory is another scope; a
        // known key names its repository; anything else is an error rather than a scope that
        // matches nothing.
        assert_eq!(search(Some(false), Some("")).unwrap(), hits);
        assert_eq!(search(None, Some("github.com/o/r")).unwrap(), hits);

        assert!(search(Some(true), None).unwrap().starts_with("o1 "));
        let elsewhere = std::env::temp_dir();
        assert_eq!(
            search(None, Some(elsewhere.to_str().unwrap())).unwrap(),
            "no hits"
        );
        let bad = s
            .search(Parameters(SearchArgs {
                query: "trigram".into(),
                all: None,
                repo: Some("/elsewhere/not/a/dir".into()),
                limit: None,
            }))
            .unwrap();
        assert_eq!(bad.is_error, Some(true));
        let unknown = s
            .search(Parameters(SearchArgs {
                query: "trigram".into(),
                all: None,
                repo: Some("github.com/o/unknown".into()),
                limit: None,
            }))
            .unwrap();
        assert_eq!(unknown.is_error, Some(true));
        let doc = body(s.get(Parameters(GetArgs { id: "s1".into() })).unwrap());
        assert!(doc.contains("要約: 検索を実装した"), "{doc}");
        let missing = s.get(Parameters(GetArgs { id: "o9".into() })).unwrap();
        assert_eq!(missing.is_error, Some(true));
        let tl = body(
            s.timeline(Parameters(TimelineArgs {
                all: Some(true),
                repo: None,
                limit: None,
            }))
            .unwrap(),
        );
        assert!(tl.contains("claude s1") && tl.contains("要約"), "{tl}");
        let tools = s.tool_router.list_all();
        let mut names: Vec<_> = tools.iter().map(|t| t.name.to_string()).collect();
        names.sort();
        assert_eq!(names, ["get", "search", "timeline"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
