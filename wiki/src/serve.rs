//! MCP server for a compiled wiki (`wiki serve`): exposes
//! search/page/neighbors (and, on request, lint) as tools and pages as
//! resources over stdio.

use crate::lint::{lint, load_compiled_pages};
use crate::live::Live;
use crate::model::{one_line as one, KindFilter};
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

/// Hits the `search` tool returns when the caller omits `limit` (the CLI
/// keeps 10): every hit line is re-read on every later turn of the agent.
pub const DEFAULT_SEARCH_LIMIT: usize = 5;

/// Every tool the server has.
pub const TOOLS: [&str; 4] = ["search", "page", "neighbors", "lint"];

/// The tools exposed without `--tools`. `lint` serves the person keeping
/// the wiki, not an agent mid-task, and each exposed tool costs schema
/// tokens on every request.
pub const DEFAULT_TOOLS: [&str; 3] = ["search", "page", "neighbors"];

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

/// A text result: success for `Ok`, an error result for `Err`.
fn text_result(r: Result<String, String>) -> CallToolResult {
    match r {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(text) => CallToolResult::error(vec![ContentBlock::text(text)]),
    }
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
    /// Words or a symbol name, e.g. `order reject` or `onOrderReject`.
    pub query: String,
    /// `code`, `docs`, or one kind such as `code:rust` or `markdown`.
    pub kind: Option<String>,
    /// Only pages with this tag (`a` also matches `a/b`); an empty query lists them.
    pub tag: Option<String>,
    /// Maximum pages (default 5).
    pub limit: Option<usize>,
    /// Output format: `lines` (default) or `json`.
    pub format: Option<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct NeighborsParams {
    /// Page id, title, or source path.
    pub id: String,
    /// Link hops from the page (default 1); with `symbol`, calls deep
    /// (default no limit).
    pub depth: Option<usize>,
    /// Token budget (default 4000).
    pub max_tokens: Option<usize>,
    /// Maximum pages in the pack (default no limit).
    pub max_nodes: Option<usize>,
    /// Whole linked pages (outlines for code) instead of one-line summaries.
    pub full: Option<bool>,
    /// Only linked pages of this kind: `code`, `docs`, or one kind such as
    /// `code:rust` or `markdown`.
    pub kind: Option<String>,
    /// A function on the page, e.g. `canOpenPosition` or
    /// `RiskManager::canOpenPosition`: list every function it calls, through
    /// all levels, instead of linked pages.
    pub symbol: Option<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct PageParams {
    /// Page id, title, or source path.
    pub id: String,
    /// The whole page instead of the outline.
    pub full: Option<bool>,
    /// Token budget (default 4000 for the outline, none with `full`).
    pub max_tokens: Option<usize>,
}

#[derive(Clone)]
pub(crate) struct WikiServer {
    state: Arc<WikiState>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl WikiServer {
    /// A server exposing only `tools` (names from [`TOOLS`]).
    pub fn new(state: Arc<WikiState>, tools: &[&str]) -> Self {
        let mut tool_router = Self::tool_router();
        for name in TOOLS.iter().filter(|t| !tools.contains(t)) {
            tool_router.remove_route(name);
        }
        Self { state, tool_router }
    }

    #[tool(
        description = "Find where code or docs live in this project. Use it before grep or reading files. Returns one line per page (`id  path  title`) and indented `path:start-end  name  signature` lines for matching symbols: read only those lines with your file tool. Paths are relative to the project root; line ranges are inclusive."
    )]
    async fn search(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        let kind = p
            .kind
            .as_deref()
            .map(KindFilter::parse_arg)
            .transpose()
            .map_err(|m| McpError::invalid_params(m, None))?;
        let tag = p
            .tag
            .as_deref()
            .map(crate::query::tag_arg)
            .transpose()
            .map_err(|m| McpError::invalid_params(m, None))?;
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
        // A tag-only listing (no query words) stays unbounded by default.
        let has_words = p.query.chars().any(char::is_alphanumeric);
        let limit = p.limit.or(has_words.then_some(DEFAULT_SEARCH_LIMIT));
        let hits = wiki.search_tagged(&p.query, kind.as_ref(), tag.as_deref(), limit);
        let text = if json {
            search_json(hits)
        } else {
            search_lines(&hits)
        };
        Ok(with_stale(text_result(Ok(text)), stale.as_deref()))
    }

    #[tool(
        description = "Show a page with the pages linked to it, within a token budget: a module's outline with the outlines of the modules it imports or is imported by, then one-line summaries of the rest. Use it to see what a module connects to; `kind: \"code\"` keeps it to code. With `symbol`, it lists every function that function calls, through all levels, as `start-end` lines grouped by file."
    )]
    async fn neighbors(
        &self,
        Parameters(p): Parameters<NeighborsParams>,
    ) -> Result<CallToolResult, McpError> {
        let kind = p
            .kind
            .as_deref()
            .map(KindFilter::parse_arg)
            .transpose()
            .map_err(|m| McpError::invalid_params(m, None))?;
        let budget = PackBudget {
            max_tokens: Some(p.max_tokens.unwrap_or(DEFAULT_NEIGHBORS_MAX_TOKENS)),
            max_nodes: p.max_nodes,
            full_neighbors: p.full.unwrap_or(false),
            kind,
        };
        let (wiki, stale) = self.state.snapshot().await;
        let result = text_result(wiki.resolve(&p.id).and_then(|id| {
            match &p.symbol {
                Some(symbol) => wiki.calls(&id, symbol, p.depth, budget.max_tokens),
                None => wiki
                    .neighbors(&id, p.depth.unwrap_or(1), &budget)
                    .map(|pack| pack.text)
                    .ok_or_else(|| crate::query::unknown_page(&p.id, &[])),
            }
        }));
        Ok(with_stale(result, stale.as_deref()))
    }

    #[tool(
        description = "Show a module's outline: one `start-end  kind  signature` line per symbol, or a docs page's headings. Then read only the lines you need with your file tool. `full: true` returns the whole page, which can be large."
    )]
    async fn page(&self, Parameters(p): Parameters<PageParams>) -> CallToolResult {
        let full = p.full.unwrap_or(false);
        let max_tokens = p
            .max_tokens
            .or((!full).then_some(DEFAULT_NEIGHBORS_MAX_TOKENS));
        let (wiki, stale) = self.state.snapshot().await;
        let result = text_result(wiki.resolve(&p.id).and_then(|id| {
            wiki.page_text(&id, full, max_tokens)
                .ok_or_else(|| crate::query::unknown_page(&p.id, &[]))
        }));
        with_stale(result, stale.as_deref())
    }

    #[tool(
        description = "Check the wiki for broken links, orphan pages and pages too large for a neighbors pack. Returns JSON {total_pages, broken_links: [[page_id, link_text]], orphans: [page_id], large: [[page_id, tokens]]}."
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
            "large": r.large,
        });
        Ok(with_stale(
            text_result(Ok(report.to_string())),
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
                "Map of this project. Find code with `search` (symbols with \
                 path:start-end) and `page` (a module's outline), then read only \
                 those lines with your file tool. `neighbors` shows linked pages, or with `symbol` what a function calls. \
                 Paths are relative to the project root; line ranges are 1-based \
                 and inclusive. Pages are also resources under wiki://page/<id>.",
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
/// `format: "json"`: `[{id, title, summary, updated, score, snippet, path, symbols}]`;
/// `updated` is present only when the page has a date.
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
            let mut hit = serde_json::json!({
                "id": h.id,
                "title": h.title,
                "summary": h.summary,
                "score": h.score,
                "snippet": h.snippet,
                "path": h.path,
                "symbols": symbols,
            });
            if let Some(u) = h.updated {
                hit["updated"] = u.into();
            }
            if h.memory {
                hit["memory"] = true.into();
            }
            if let Some(by) = h.superseded_by {
                hit["superseded_by"] = by.into();
            }
            hit
        })
        .collect();
    serde_json::Value::Array(hits).to_string()
}

/// Render search hits as the default line output: `id  path  [date  ]title — summary`
/// per page, then one `  path:start-end  name  signature  — doc` line per
/// symbol, or the snippet (four-space indent) when the page has no symbols.
/// No hits render as one `no matches` line.
pub fn search_lines(hits: &[Hit]) -> String {
    if hits.is_empty() {
        return "no matches\n".to_string();
    }
    // Fields are one-line text; collapse any newline or tab they carry.
    let mut out = String::new();
    for h in hits {
        out.push_str(&format!("{}  {}  ", h.id, h.path));
        if let Some(u) = &h.updated {
            out.push_str(&format!("{u}  "));
        }
        out.push_str(&one(&h.title));
        if let Some(s) = &h.summary {
            out.push_str(&format!(" — {}", one(s)));
        }
        if let Some(by) = &h.superseded_by {
            out.push_str(&format!(" (superseded by {by})"));
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
/// `tools` picks the exposed tools (default [`DEFAULT_TOOLS`]).
pub fn run(
    dir: &Path,
    source: Option<&Path>,
    jobs: Option<usize>,
    tools: Option<&[String]>,
    memory: Option<&str>,
) -> Result<(), WikiError> {
    let tools: Vec<&str> = match tools {
        None => DEFAULT_TOOLS.to_vec(),
        Some(names) => {
            let mut picked = Vec::new();
            for name in names {
                let Some(&t) = TOOLS.iter().find(|t| **t == name.trim()) else {
                    return Err(WikiError::Serve(format!(
                        "unknown tool {name:?}; expected {}",
                        TOOLS.join(", ")
                    )));
                };
                picked.push(t);
            }
            picked
        }
    };
    let state = match source {
        Some(source) => {
            // The repo's local folder unless `none`; `DIR` replaces it.
            let memory = match memory {
                Some("none") => None,
                Some(d) => Some(crate::memory::resolve(Path::new(d))?),
                None => crate::memory::local_dir(source)
                    .map(|d| crate::memory::resolve(&d))
                    .transpose()?,
            };
            let opts = CompileOptions {
                incremental: true,
                respect_ignore: true,
                emit_json: dir.join("graph.json").exists(),
                jobs,
                project: None,
                memory,
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
    let server = WikiServer::new(Arc::new(state), &tools);
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
