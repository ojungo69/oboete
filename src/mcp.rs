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
    /// The repository key of the directory the agent launched us from.
    cwd_repo: String,
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Terms, all required. A term of 3+ characters matches as a substring with Unicode case
    /// folding; a shorter one as a literal substring.
    query: String,
    /// Search every repository instead of the current one.
    #[serde(default)]
    all: bool,
    /// A repository root to search instead of the current one.
    #[serde(default)]
    repo: Option<String>,
    /// Maximum number of hits (default 10).
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetArgs {
    /// A document id from `search`: `o12` (observation) or `s5` (session summary).
    id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct TimelineArgs {
    /// Every repository instead of the current one.
    #[serde(default)]
    all: bool,
    /// A repository root instead of the current one.
    #[serde(default)]
    repo: Option<String>,
    /// Maximum number of sessions (default 20).
    #[serde(default)]
    limit: Option<usize>,
}

fn text(s: String) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(s)]))
}

fn internal(e: anyhow::Error) -> ErrorData {
    ErrorData::internal_error(format!("{e:#}"), None)
}

#[tool_router]
impl Oboete {
    pub fn new(home: &Path, cwd: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
            cwd_repo: repo::key(cwd),
            tool_router: Self::tool_router(),
        }
    }

    fn scope(&self, all: bool, repo: Option<&str>) -> Option<String> {
        if all {
            None
        } else {
            Some(match repo {
                Some(r) => crate::repo::key(Path::new(r)),
                None => self.cwd_repo.clone(),
            })
        }
    }

    #[tool(
        name = "search",
        description = "Full-text search over what oboete remembers about this repository: observations (decisions, bug fixes, discoveries, preferences) and session summaries from earlier coding sessions. Returns one hit per line: id, local time, kind, title, snippet. Use `get` for the full text."
    )]
    fn search(&self, Parameters(a): Parameters<SearchArgs>) -> Result<CallToolResult, ErrorData> {
        let conn = db::open(&self.home).map_err(internal)?;
        let scope = self.scope(a.all, a.repo.as_deref());
        let hits = search::search(&conn, &a.query, scope.as_deref(), a.limit.unwrap_or(10))
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
        description = "The full text of one remembered document by the id `search` returned (o12 = observation, s5 = session summary)."
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
            None => Err(ErrorData::invalid_params(
                format!("no document {} (ids come from search)", a.id),
                None,
            )),
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
        let scope = self.scope(a.all, a.repo.as_deref());
        let rows =
            search::timeline(&conn, scope.as_deref(), a.limit.unwrap_or(20)).map_err(internal)?;
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
        let repo_key = repo::key(&dir.join("r"));
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
        let hits = body(
            s.search(Parameters(SearchArgs {
                query: "trigram".into(),
                all: false,
                repo: None,
                limit: None,
            }))
            .unwrap(),
        );
        assert!(
            hits.starts_with("o1 ") && hits.contains("use the trigram tokenizer"),
            "{hits}"
        );
        let none = body(
            s.search(Parameters(SearchArgs {
                query: "trigram".into(),
                all: false,
                repo: Some("/elsewhere".into()),
                limit: None,
            }))
            .unwrap(),
        );
        assert_eq!(none, "no hits");
        let doc = body(s.get(Parameters(GetArgs { id: "s1".into() })).unwrap());
        assert!(doc.contains("要約: 検索を実装した"), "{doc}");
        assert!(s.get(Parameters(GetArgs { id: "o9".into() })).is_err());
        let tl = body(
            s.timeline(Parameters(TimelineArgs {
                all: true,
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
