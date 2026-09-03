//! MCP server for a compiled wiki (`wiki serve`): exposes search/neighbors/
//! lint as tools and pages as resources over stdio.

use crate::lint::{lint, load_compiled_pages};
use crate::model::SourceKind;
use crate::query::PackBudget;
use crate::query::Wiki;
use crate::WikiError;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ListResourcesResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::transport::stdio;
use rmcp::{
    tool, tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::SystemTime;

/// Identity of the compiled output, cheap to compute: `index.json`'s
/// (mtime, len). A recompile rewrites index.json, changing at least one.
#[derive(PartialEq, Clone, Copy, Debug)]
struct Fingerprint {
    mtime: SystemTime,
    len: u64,
}

fn fingerprint(dir: &Path) -> std::io::Result<Fingerprint> {
    let meta = std::fs::metadata(dir.join("index.json"))?;
    Ok(Fingerprint {
        mtime: meta.modified()?,
        len: meta.len(),
    })
}

struct Loaded {
    wiki: Wiki,
    fingerprint: Fingerprint,
}

/// A lazily-reloading handle to a compiled wiki. Each access compares the
/// current `index.json` fingerprint against the loaded snapshot's and
/// reloads on change. A failed reload (mid-compile write, malformed index,
/// deleted file) keeps serving the previous snapshot; the reload is retried
/// on the next access.
pub(crate) struct WikiState {
    dir: PathBuf,
    inner: Mutex<Loaded>,
}

impl WikiState {
    pub fn load(dir: &Path) -> std::io::Result<WikiState> {
        let fp = fingerprint(dir)?;
        let wiki = Wiki::load(dir)?;
        Ok(WikiState {
            dir: dir.to_path_buf(),
            inner: Mutex::new(Loaded {
                wiki,
                fingerprint: fp,
            }),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn with_wiki<T>(&self, f: impl FnOnce(&Wiki) -> T) -> T {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(current) = fingerprint(&self.dir) {
            if current != guard.fingerprint {
                if let Ok(wiki) = Wiki::load(&self.dir) {
                    *guard = Loaded {
                        wiki,
                        fingerprint: current,
                    };
                }
                // Reload failure: keep old snapshot, retry next call.
            }
        }
        f(&guard.wiki)
    }
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct SearchParams {
    /// Search query, matched case-insensitively against title, aliases,
    /// summary, section headings, and body. A page matching any one token
    /// is a hit; rare tokens weigh far more than common ones.
    pub query: String,
    /// Filter by source kind: `text`, `markdown`, or `code:<lang>`
    /// (e.g. `code:rust`).
    pub kind: Option<String>,
    /// Maximum number of hits (default 10).
    pub limit: Option<usize>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct NeighborsParams {
    /// Page id (slug) to build the context pack around.
    pub id: String,
    /// BFS hop count from the target page (default 1).
    pub depth: Option<usize>,
    /// Token budget for the returned pack (default unbounded).
    pub max_tokens: Option<usize>,
    /// Node-count budget for the returned pack (default unbounded).
    pub max_nodes: Option<usize>,
    /// Include full neighbor bodies instead of one-line summaries.
    pub full: Option<bool>,
}

#[derive(Clone)]
pub(crate) struct WikiServer {
    state: Arc<WikiState>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl WikiServer {
    pub fn new(state: Arc<WikiState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Search the wiki for pages by keyword. Returns matching pages as JSON: [{id, title, summary, score, snippet}]. Use `neighbors` afterwards to pull a page's full context."
    )]
    fn search(&self, Parameters(p): Parameters<SearchParams>) -> Result<CallToolResult, McpError> {
        let kind = match p.kind.as_deref() {
            None => None,
            Some(s) => Some(SourceKind::parse(s).ok_or_else(|| {
                McpError::invalid_params(
                    format!("unknown kind {s:?}; expected {}", SourceKind::EXPECTED),
                    None,
                )
            })?),
        };
        let hits = self.state.with_wiki(|w| {
            w.search(&p.query, kind, p.limit.unwrap_or(10))
                .into_iter()
                .map(|h| {
                    serde_json::json!({
                        "id": h.id,
                        "title": h.title,
                        "summary": h.summary,
                        "score": h.score,
                        "snippet": h.snippet,
                    })
                })
                .collect::<Vec<_>>()
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::Value::Array(hits).to_string(),
        )]))
    }

    #[tool(
        description = "Build a budgeted context pack around a page: the page in full, then its graph neighbors ordered by centrality (most important last). Prefer this over reading pages one by one."
    )]
    fn neighbors(
        &self,
        Parameters(p): Parameters<NeighborsParams>,
    ) -> Result<CallToolResult, McpError> {
        let budget = PackBudget {
            max_tokens: p.max_tokens,
            max_nodes: p.max_nodes,
            full_neighbors: p.full.unwrap_or(false),
        };
        let pack = self
            .state
            .with_wiki(|w| w.neighbors(&p.id, p.depth.unwrap_or(1), &budget));
        match pack {
            Some(pack) => Ok(CallToolResult::success(vec![ContentBlock::text(pack.text)])),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "unknown page id {:?} — use the search tool to find valid ids",
                p.id
            ))])),
        }
    }

    #[tool(
        description = "Check wiki health: broken wikilinks and orphan pages. Returns JSON {total_pages, broken_links: [[page_id, link_text]], orphans: [page_id]}."
    )]
    fn lint(&self) -> Result<CallToolResult, McpError> {
        let pages = load_compiled_pages(self.state.dir())
            .map_err(|e| McpError::internal_error(format!("read wiki dir: {e}"), None))?;
        let r = lint(&pages);
        let report = serde_json::json!({
            "total_pages": r.total_pages,
            "broken_links": r.broken_links,
            "orphans": r.orphans,
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(
            report.to_string(),
        )]))
    }
}

#[tool_handler(router = self.tool_router.clone())]
impl ServerHandler for WikiServer {
    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        ServerInfo::new(capabilities)
            .with_server_info(Implementation::new("wiki", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Query interface to a compiled wiki. Start with `search` to find \
                 page ids, then `neighbors` to pull a budgeted context pack around \
                 a page. Pages are also exposed as resources under wiki://page/<id>.",
            )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let mut resources = vec![
            Resource::new("wiki://index", "index.json (machine catalog)")
                .with_mime_type("application/json"),
            Resource::new("wiki://llms.txt", "llms.txt (compact LLM index)")
                .with_mime_type("text/plain"),
        ];
        self.state.with_wiki(|w| {
            for (id, title) in w.list_pages() {
                resources.push(
                    Resource::new(format!("wiki://page/{id}"), title)
                        .with_mime_type("text/markdown"),
                );
            }
        });
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let uri = request.uri.as_str();
        let not_found = || McpError::resource_not_found(format!("no such resource: {uri}"), None);
        // Only a missing file is "no such resource"; a permission or read
        // error on a file that exists is the server's problem, not the
        // client's.
        let read_file = |name: &str| {
            std::fs::read_to_string(self.state.dir().join(name)).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    not_found()
                } else {
                    McpError::internal_error(format!("read {name}: {e}"), None)
                }
            })
        };
        let (text, mime_type) = match uri {
            "wiki://index" => (read_file("index.json")?, "application/json"),
            "wiki://llms.txt" => (read_file("llms.txt")?, "text/plain"),
            _ => {
                let id = uri.strip_prefix("wiki://page/").ok_or_else(not_found)?;
                let page = self.state.with_wiki(|w| {
                    // Only touch the filesystem for ids the loaded index knows
                    // about — page ids are slugs `[a-z0-9_]+`, so index
                    // membership rejects any traversal sequence outright.
                    if w.has_page(id) {
                        w.page(id)
                    } else {
                        None
                    }
                });
                (page.ok_or_else(not_found)?, "text/markdown")
            }
        };
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, uri).with_mime_type(mime_type)
        ])
        .into())
    }
}

/// Start the MCP server over stdio and block until the client disconnects.
pub fn run(dir: &Path) -> Result<(), WikiError> {
    let state = WikiState::load(dir).map_err(|e| {
        WikiError::Serve(format!(
            "{} is not a compiled wiki ({e}) — run `wiki compile <input> {}` first",
            dir.display(),
            dir.display()
        ))
    })?;
    let server = WikiServer::new(Arc::new(state));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let service = server
                .serve(stdio())
                .await
                .map_err(|e| WikiError::Serve(e.to_string()))?;
            service
                .waiting()
                .await
                .map_err(|e| WikiError::Serve(e.to_string()))?;
            Ok(())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{compile, CompileOptions};

    fn fixture(files: usize) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let raw = tmp.path().join("raw");
        let out = tmp.path().join("out");
        crate::generator::generate_corpus(&raw, files, 42).unwrap();
        compile(&raw, &out, &CompileOptions::default()).unwrap();
        (tmp, out)
    }

    #[test]
    fn load_fails_without_index_json() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(WikiState::load(tmp.path()).is_err());
    }

    #[test]
    fn with_wiki_serves_loaded_snapshot() {
        let (_tmp, out) = fixture(3);
        let state = WikiState::load(&out).unwrap();
        let n = state.with_wiki(|w| w.list_pages().len());
        assert_eq!(n, 3);
    }

    #[test]
    fn with_wiki_reloads_after_recompile() {
        let (tmp, out) = fixture(3);
        let state = WikiState::load(&out).unwrap();
        assert_eq!(state.with_wiki(|w| w.list_pages().len()), 3);

        // Recompile with more source files -> index.json changes (len differs).
        let raw = tmp.path().join("raw");
        crate::generator::generate_corpus(&raw, 5, 42).unwrap();
        compile(&raw, &out, &CompileOptions::default()).unwrap();

        assert_eq!(state.with_wiki(|w| w.list_pages().len()), 5);
    }

    #[test]
    fn with_wiki_keeps_old_snapshot_when_reload_fails() {
        let (_tmp, out) = fixture(3);
        let state = WikiState::load(&out).unwrap();
        assert_eq!(state.with_wiki(|w| w.list_pages().len()), 3);

        // Corrupt index.json (fingerprint changes, load will fail).
        std::fs::write(out.join("index.json"), "{ not json").unwrap();

        // Old snapshot still serves.
        assert_eq!(state.with_wiki(|w| w.list_pages().len()), 3);
    }
}
