//! MCP server for a compiled wiki (`wiki serve`): exposes
//! search/neighbors/lint as tools and pages as resources over stdio.

use crate::lint::{lint, load_compiled_pages};
use crate::live::Live;
use crate::model::KindFilter;
use crate::query::Wiki;
use crate::query::{Hit, PackBudget};
use crate::{CompileOptions, WikiError};
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

/// Token ceiling applied to the `neighbors` tool when the caller omits
/// `max_tokens`. The CLI stays unbounded: a person passes flags on purpose,
/// an agent calling with defaults should not receive a 30 KB dump.
pub const DEFAULT_NEIGHBORS_MAX_TOKENS: usize = 4000;

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
    wiki: Arc<Wiki>,
    fingerprint: Fingerprint,
}

/// How the served snapshot keeps up with the source.
enum Freshness {
    /// Another process compiles; reload when `index.json` changes. A failed
    /// reload (mid-compile write, malformed index, deleted file) keeps
    /// serving the previous snapshot and is retried on the next access.
    Fingerprint(Mutex<Loaded>),
    /// `serve --source` compiles itself (see [`Live`]).
    Live(Arc<Live>),
}

/// A handle to the compiled wiki a server answers from.
pub(crate) struct WikiState {
    dir: PathBuf,
    freshness: Freshness,
}

impl WikiState {
    pub fn load(dir: &Path) -> std::io::Result<WikiState> {
        let fp = fingerprint(dir)?;
        let wiki = Wiki::load(dir)?;
        Ok(WikiState {
            dir: dir.to_path_buf(),
            freshness: Freshness::Fingerprint(Mutex::new(Loaded {
                wiki: Arc::new(wiki),
                fingerprint: fp,
            })),
        })
    }

    pub fn live(live: Arc<Live>) -> WikiState {
        WikiState {
            dir: live.dir().to_path_buf(),
            freshness: Freshness::Live(live),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The wiki to answer from, and why it may be stale. A live wait runs off
    /// the MCP runtime thread, so pings and cancels keep working meanwhile.
    pub async fn snapshot(&self) -> (Arc<Wiki>, Option<String>) {
        match &self.freshness {
            Freshness::Fingerprint(inner) => (self.reload(inner), None),
            Freshness::Live(live) => {
                let live = Arc::clone(live);
                tokio::task::spawn_blocking(move || live.fresh())
                    .await
                    .expect("freshness wait panicked")
            }
        }
    }

    fn reload(&self, inner: &Mutex<Loaded>) -> Arc<Wiki> {
        let mut guard = inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Ok(current) = fingerprint(&self.dir) {
            if current != guard.fingerprint {
                if let Ok(wiki) = Wiki::load(&self.dir) {
                    *guard = Loaded {
                        wiki: Arc::new(wiki),
                        fingerprint: current,
                    };
                }
                // Reload failure: keep old snapshot, retry next call.
            }
        }
        Arc::clone(&guard.wiki)
    }
}

/// The line a result carries while the snapshot may be behind the source.
fn stale_line(reason: &str) -> String {
    format!("wiki may be stale: {reason}")
}

/// Append the stale line, if any, as a text block after a tool's own result.
fn with_stale(mut result: CallToolResult, stale: Option<&str>) -> CallToolResult {
    if let Some(reason) = stale {
        result.content.push(ContentBlock::text(stale_line(reason)));
    }
    result
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct SearchParams {
    /// Search query, matched case-insensitively: title, aliases, summary and
    /// section headings by word prefix (every part of the token), defined
    /// and method names by whole term, body by substring. A page matching
    /// any one token is a hit; rare tokens weigh far more than common ones.
    pub query: String,
    /// Filter by source kind: the groups `code` and `docs`, or one exact
    /// kind: `text`, `markdown`, or `code:<lang>` (e.g. `code:rust`).
    pub kind: Option<String>,
    /// Only pages carrying this tag; `a` also matches `a/b`. With an empty
    /// query, lists every such page.
    pub tag: Option<String>,
    /// Maximum number of hits (default 10 with a query; none for a tag-only listing).
    pub limit: Option<usize>,
    /// Output format: `lines` (default) or `json`.
    pub format: Option<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct NeighborsParams {
    /// The page to build the context pack around: its id, its title, or its
    /// source path (a path suffix works when only one page ends with it).
    pub id: String,
    /// BFS hop count from the target page (default 1).
    pub depth: Option<usize>,
    /// Token budget for the returned pack (default 4000). Pass a larger
    /// value for more context; there is no way to make it unbounded here.
    pub max_tokens: Option<usize>,
    /// Node-count budget for the returned pack (default unbounded).
    pub max_nodes: Option<usize>,
    /// Include full neighbor bodies instead of one-line summaries.
    pub full: Option<bool>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct PageParams {
    /// The page: its id, its title, or its source path (relative, ./, or absolute).
    pub id: String,
    /// Return the whole rendered page instead of the outline.
    pub full: Option<bool>,
    /// Token budget (default 4000 for the outline; none for full).
    pub max_tokens: Option<usize>,
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
        description = "Search the wiki for pages by keyword, or list pages by tag. Returns one line per page (`id  path  title — summary`) with indented `path:start-end  name  signature  — doc` lines for matching code symbols; `format: json` returns JSON. Use `page` for a module's outline."
    )]
    async fn search(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        let kind = match p.kind.as_deref() {
            None => None,
            Some(s) => Some(KindFilter::parse(s).ok_or_else(|| {
                McpError::invalid_params(
                    format!("unknown kind {s:?}; expected {}", KindFilter::EXPECTED),
                    None,
                )
            })?),
        };
        let tag = match p.tag.as_deref() {
            None => None,
            Some(s) => Some(
                crate::query::tag_filter(s)
                    .ok_or_else(|| McpError::invalid_params(format!("empty tag {s:?}"), None))?,
            ),
        };
        let (wiki, stale) = self.state.snapshot().await;
        let json = match p.format.as_deref() {
            None | Some("lines") => false,
            Some("json") => true,
            Some(f) => {
                return Err(McpError::invalid_params(
                    format!("unknown format {f:?}; expected lines or json"),
                    None,
                ))
            }
        };
        let hits = wiki.search_tagged(&p.query, kind.as_ref(), tag.as_deref(), p.limit);
        let text = if json {
            search_json(hits)
        } else {
            search_lines(&hits)
        };
        Ok(with_stale(
            CallToolResult::success(vec![ContentBlock::text(text)]),
            stale.as_deref(),
        ))
    }

    #[tool(
        description = "Build a budgeted context pack around a page: the page in full, then its graph neighbors ordered by centrality (most important last). Prefer this over reading pages one by one."
    )]
    async fn neighbors(&self, Parameters(p): Parameters<NeighborsParams>) -> CallToolResult {
        let budget = PackBudget {
            max_tokens: Some(p.max_tokens.unwrap_or(DEFAULT_NEIGHBORS_MAX_TOKENS)),
            max_nodes: p.max_nodes,
            full_neighbors: p.full.unwrap_or(false),
        };
        let (wiki, stale) = self.state.snapshot().await;
        let result = match wiki.resolve_id(&p.id) {
            Ok(id) => match wiki.neighbors(&id, p.depth.unwrap_or(1), &budget) {
                Some(pack) => CallToolResult::success(vec![ContentBlock::text(pack.text)]),
                None => CallToolResult::error(vec![ContentBlock::text(
                    crate::query::unknown_page(&p.id, &[]),
                )]),
            },
            Err(near) => CallToolResult::error(vec![ContentBlock::text(
                crate::query::unknown_page(&p.id, &near),
            )]),
        };
        with_stale(result, stale.as_deref())
    }

    #[tool(
        description = "Show a page's outline: one line per code symbol (start-end kind signature) — then read just those lines with your file tool. Docs pages list their headings. full: true returns the whole page."
    )]
    async fn page(&self, Parameters(p): Parameters<PageParams>) -> CallToolResult {
        let full = p.full.unwrap_or(false);
        let max_tokens = p
            .max_tokens
            .or((!full).then_some(DEFAULT_NEIGHBORS_MAX_TOKENS));
        let (wiki, stale) = self.state.snapshot().await;
        let result = match wiki.resolve_id(&p.id) {
            Ok(id) => match wiki.page_text(&id, full, max_tokens) {
                Some(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
                None => CallToolResult::error(vec![ContentBlock::text(
                    crate::query::unknown_page(&p.id, &[]),
                )]),
            },
            Err(near) => CallToolResult::error(vec![ContentBlock::text(
                crate::query::unknown_page(&p.id, &near),
            )]),
        };
        with_stale(result, stale.as_deref())
    }

    #[tool(
        description = "Check wiki health: broken wikilinks and orphan pages. Returns JSON {total_pages, broken_links: [[page_id, link_text]], orphans: [page_id]}."
    )]
    async fn lint(&self) -> Result<CallToolResult, McpError> {
        let (_, stale) = self.state.snapshot().await;
        let pages = load_compiled_pages(self.state.dir())
            .map_err(|e| McpError::internal_error(format!("read wiki dir: {e}"), None))?;
        let r = lint(&pages);
        let report = serde_json::json!({
            "total_pages": r.total_pages,
            "broken_links": r.broken_links,
            "orphans": r.orphans,
        });
        Ok(with_stale(
            CallToolResult::success(vec![ContentBlock::text(report.to_string())]),
            stale.as_deref(),
        ))
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
                "Query interface to a compiled wiki. `search` finds pages and code \
                 symbols with path:start-end; `page` gives a module's outline; read \
                 only those lines with your file tool. `neighbors` packs related \
                 pages. Paths are relative to the compiled project root; line ranges \
                 are 1-based and inclusive. Pages are also resources under \
                 wiki://page/<id>.",
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
        let (wiki, _) = self.state.snapshot().await;
        for (id, title) in wiki.list_pages() {
            resources.push(
                Resource::new(format!("wiki://page/{id}"), title).with_mime_type("text/markdown"),
            );
        }
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let uri = request.uri.as_str();
        let (wiki, stale) = self.state.snapshot().await;
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
                // Served from memory, keyed by the index's own ids: an id
                // with a traversal sequence is simply not found.
                let page = wiki.page(id);
                (page.ok_or_else(not_found)?, "text/markdown")
            }
        };
        let mut contents = vec![ResourceContents::text(text, uri).with_mime_type(mime_type)];
        if let Some(reason) = stale {
            contents.push(
                ResourceContents::text(stale_line(&reason), uri).with_mime_type("text/plain"),
            );
        }
        Ok(ReadResourceResult::new(contents).into())
    }
}

/// Render search hits as the JSON array `search` returns with
/// `format: "json"`: `[{id, title, summary, score, snippet, path, symbols}]`.
pub fn search_json(hits: Vec<Hit>) -> String {
    let hits = hits
        .into_iter()
        .map(|h| {
            let symbols: Vec<_> = h
                .symbols
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "path": s.path,
                        "start_line": s.start_line,
                        "end_line": s.end_line,
                        "name": s.name,
                        "signature": s.signature,
                        "doc": s.doc,
                    })
                })
                .collect();
            serde_json::json!({
                "id": h.id,
                "title": h.title,
                "summary": h.summary,
                "score": h.score,
                "snippet": h.snippet,
                "path": h.path,
                "symbols": symbols,
            })
        })
        .collect();
    serde_json::Value::Array(hits).to_string()
}

/// Render search hits as the default line output: `id  path  title — summary`
/// per page, then one `  path:start-end  name  signature  — doc` line per
/// symbol, or the snippet (four-space indent) when the page has no symbols.
/// No hits render as one `no matches` line.
pub fn search_lines(hits: &[Hit]) -> String {
    if hits.is_empty() {
        return "no matches\n".to_string();
    }
    // Fields are one-line text; collapse any newline or tab they carry.
    let one = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for h in hits {
        out.push_str(&format!("{}  {}  {}", h.id, h.path, one(&h.title)));
        if let Some(s) = &h.summary {
            out.push_str(&format!(" — {}", one(s)));
        }
        out.push('\n');
        for s in &h.symbols {
            out.push_str(&format!(
                "  {}:{}-{}  {}  {}",
                s.path,
                s.start_line,
                s.end_line,
                s.name,
                one(&s.signature)
            ));
            if let Some(d) = &s.doc {
                out.push_str(&format!("  — {}", one(d)));
            }
            out.push('\n');
        }
        if let (true, Some(sn)) = (h.symbols.is_empty(), &h.snippet) {
            out.push_str(&format!("    {}\n", one(sn)));
        }
    }
    out
}

/// Start the MCP server over stdio and block until the client disconnects.
/// With `source`, the server compiles it into `dir` and keeps recompiling.
pub fn run(dir: &Path, source: Option<&Path>, jobs: Option<usize>) -> Result<(), WikiError> {
    let state = match source {
        Some(source) => {
            let opts = CompileOptions {
                incremental: true,
                respect_ignore: true,
                emit_json: dir.join("graph.json").exists(),
                jobs,
                project: None,
            };
            WikiState::live(Live::start(source, dir, opts)?)
        }
        None => WikiState::load(dir).map_err(|e| {
            WikiError::Serve(format!(
                "{} is not a compiled wiki ({e}) — run `wiki compile <input> {}` first",
                dir.display(),
                dir.display()
            ))
        })?,
    };
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

    /// Page count of the snapshot `state` answers from now.
    fn pages(state: &WikiState) -> usize {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(state.snapshot()).0.list_pages().len()
    }

    #[test]
    fn load_fails_without_index_json() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(WikiState::load(tmp.path()).is_err());
    }

    #[test]
    fn snapshot_serves_loaded_snapshot() {
        let (_tmp, out) = fixture(3);
        let state = WikiState::load(&out).unwrap();
        let n = pages(&state);
        assert_eq!(n, 3);
    }

    #[test]
    fn snapshot_reloads_after_recompile() {
        let (tmp, out) = fixture(3);
        let state = WikiState::load(&out).unwrap();
        assert_eq!(pages(&state), 3);

        // Recompile with more source files -> index.json changes (len differs).
        let raw = tmp.path().join("raw");
        crate::generator::generate_corpus(&raw, 5, 42).unwrap();
        compile(&raw, &out, &CompileOptions::default()).unwrap();

        assert_eq!(pages(&state), 5);
    }

    #[test]
    fn snapshot_keeps_old_snapshot_when_reload_fails() {
        let (_tmp, out) = fixture(3);
        let state = WikiState::load(&out).unwrap();
        assert_eq!(pages(&state), 3);

        // Corrupt index.json (fingerprint changes, load will fail).
        std::fs::write(out.join("index.json"), "{ not json").unwrap();

        // Old snapshot still serves.
        assert_eq!(pages(&state), 3);
    }
}
