//! MCP server for a compiled wiki (`kalka serve`): exposes
//! search/page/callers (and, on request, neighbors and lint) as tools and
//! pages as resources over stdio.

use crate::lint::{lint, load_compiled_pages};
use crate::live::{Live, EXE, OUTDATED, RUNNING, WAIT};
use crate::model::{clip, one_line, KindFilter, Origin, LINE_CHARS};
use crate::query::Wiki;
use crate::query::{Hit, PackBudget};
use crate::{CompileOptions, WikiError};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ListResourcesResult, MetaObject,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    Resource, ResourceContents, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::transport::stdio;
use rmcp::{
    tool, tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Instant, SystemTime};

pub use crate::query::DEFAULT_NEIGHBORS_MAX_TOKENS;

/// Hits the `search` tool returns when the caller omits `limit` (the CLI
/// keeps 10): every hit line is re-read on every later turn of the agent.
pub const DEFAULT_SEARCH_LIMIT: usize = 5;

/// What the client shows the model at session start (Claude Code does; pi
/// does not, so pi gets the same steps from its skill). Claude Code gets no skill:
/// a navigation skill cost one turn per task.
pub const INSTRUCTIONS: &str =
    "Map of this project. To find code, call `search` with the words of \
what you are looking for and `kind: \"code\"`; each symbol line is \
`path:start-end  name  signature`. Read only those lines with your file tool: \
the ranges are exact, do not re-derive them with grep. `page` with a path \
lists a module's symbols with their lines. Do not call grep, find or ls in \
the same turn as \
`search`: read its answer first. Use `search` for where something is defined \
or handled, `callers` for who calls a function, grep for every occurrence of a \
literal string (an env var, a message, a config key). If search finds \
nothing relevant, try other words (a name, an abbreviation) before grep. \
Paths are relative to the project root; line ranges are 1-based and \
inclusive. Pages are also resources under wiki://page/<id>.";

/// Every tool the server has.
pub const TOOLS: [&str; 5] = ["search", "page", "neighbors", "callers", "lint"];

/// The tools exposed without `--tools`. `lint` serves the person keeping
/// the wiki, not an agent mid-task, and each exposed tool costs schema
/// tokens on every request. `neighbors` is left out: 16 of 11,056 rig calls
/// and 0 of 37 on the bench, for about a quarter of the per-turn schema
/// tokens.
pub const DEFAULT_TOOLS: [&str; 3] = ["search", "page", "callers"];

/// Identity of the compiled output, cheap to compute: `index.json`'s
/// (mtime, len). A recompile rewrites index.json, changing at least one.
#[derive(PartialEq, Clone, Copy, Debug)]
pub(crate) struct Fingerprint {
    mtime: SystemTime,
    len: u64,
}

pub(crate) fn fingerprint(dir: &Path) -> std::io::Result<Fingerprint> {
    let meta = std::fs::metadata(dir.join("index.json"))?;
    Ok(Fingerprint {
        mtime: meta.modified()?,
        len: meta.len(),
    })
}

struct Loaded {
    wiki: Arc<Wiki>,
    /// `None` until the first reload from disk.
    fingerprint: Option<Fingerprint>,
}

/// How the served snapshot keeps up with the source.
enum Freshness {
    /// Another process compiles; reload when `index.json` changes. A failed
    /// reload (mid-compile write, malformed index, deleted file) keeps
    /// serving the previous snapshot and is retried on the next access.
    Fingerprint(Mutex<Loaded>),
    /// `serve --source` compiles itself (see [`Live`]). Once its binary is
    /// replaced it stops, and the second field takes over as `Fingerprint`.
    Live(Arc<Live>, OnceLock<Mutex<Loaded>>),
}

/// A handle to the compiled wiki a server answers from.
pub(crate) struct WikiState {
    dir: PathBuf,
    freshness: Freshness,
}

impl WikiState {
    /// While a compile holds `compile.lock`, start empty rather than wait:
    /// the first call loads the wiki.
    pub fn load(dir: &Path) -> std::io::Result<Arc<WikiState>> {
        let loaded = crate::dirlock::try_shared(dir, || {
            Ok::<_, std::io::Error>((fingerprint(dir)?, Wiki::load(dir)?))
        })
        .transpose()?;
        let (wiki, fingerprint) = match loaded {
            Some((fp, wiki)) => (wiki, Some(fp)),
            None => (Wiki::default(), None),
        };
        Ok(Arc::new(WikiState {
            dir: dir.to_path_buf(),
            freshness: Freshness::Fingerprint(Mutex::new(Loaded {
                wiki: Arc::new(wiki),
                fingerprint,
            })),
        }))
    }

    pub fn live(live: Arc<Live>) -> Arc<WikiState> {
        Arc::new(WikiState {
            dir: live.dir().to_path_buf(),
            freshness: Freshness::Live(live, OnceLock::new()),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The wiki to answer from, and why it may be stale. A live wait runs off
    /// the MCP runtime thread, so pings and cancels keep working meanwhile.
    pub async fn snapshot(self: &Arc<Self>) -> (Arc<Wiki>, Option<String>) {
        let deadline = Instant::now() + WAIT;
        let outdated = EXE.replaced().then(|| OUTDATED.to_string());
        match &self.freshness {
            Freshness::Fingerprint(_) => self.reload_blocking(deadline, outdated).await,
            Freshness::Live(live, frozen) if outdated.is_some() => {
                frozen.get_or_init(|| {
                    Mutex::new(Loaded {
                        wiki: live.wiki(),
                        fingerprint: None,
                    })
                });
                self.reload_blocking(deadline, outdated).await
            }
            Freshness::Live(live, _) => {
                let live = Arc::clone(live);
                tokio::task::spawn_blocking(move || live.fresh())
                    .await
                    .expect("freshness wait panicked")
            }
        }
    }

    /// `reload` waits until `deadline` for a compile's file lock: off the
    /// runtime thread. Past it, `stale` falls back to `RUNNING`.
    async fn reload_blocking(
        self: &Arc<Self>,
        deadline: Instant,
        stale: Option<String>,
    ) -> (Arc<Wiki>, Option<String>) {
        let this = Arc::clone(self);
        let (wiki, done) = tokio::task::spawn_blocking(move || this.reload(deadline))
            .await
            .expect("reload panicked");
        (wiki, stale.or((!done).then(|| RUNNING.to_string())))
    }

    /// The current snapshot, and whether the reload check ran: `false` when a
    /// compile held the lock past `deadline`, so the old snapshot stays.
    fn reload(&self, deadline: Instant) -> (Arc<Wiki>, bool) {
        let inner = match &self.freshness {
            Freshness::Fingerprint(m) => m,
            Freshness::Live(_, frozen) => frozen.get().expect("frozen set before reload"),
        };
        let mut guard = inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let done = crate::dirlock::shared_until(&self.dir, deadline, || {
            if let Ok(current) = fingerprint(&self.dir) {
                if Some(current) != guard.fingerprint {
                    if let Ok(wiki) = Wiki::load(&self.dir) {
                        *guard = Loaded {
                            wiki: Arc::new(wiki),
                            fingerprint: Some(current),
                        };
                    }
                    // Reload failure: keep old snapshot, retry next call.
                }
            }
        });
        (Arc::clone(&guard.wiki), done.is_some())
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
    /// Maximum pages (default 5; with `docs` default 10, and an empty query lists the first 200 labels).
    pub limit: Option<usize>,
    /// Output format: `lines` (default) or `json`.
    pub format: Option<String>,
    /// Markdown files to search instead of the code: the plan, spec or ledger
    /// you were handed. Returns each path on its own line, then `  start-end  label` for
    /// matching headings and bold step labels (`-` and spaces match alike); an empty query lists the first 200 (pass `limit` for more).
    /// Pass the plan, spec and ledger together: an ID is often a label in only one of
    /// them. Only labels are searched, not body text; use grep for text.
    pub docs: Option<Vec<String>>,
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
pub(crate) struct CallersParams {
    /// Page id, title, or source path.
    pub id: String,
    /// A function on the page, e.g. `canOpenPosition` or
    /// `RiskManager::canOpenPosition`.
    pub symbol: String,
    /// Caller levels (default 1: the direct callers).
    pub depth: Option<usize>,
    /// Token budget (default 4000).
    pub max_tokens: Option<usize>,
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
    /// Where `docs` paths are resolved: the `--source` dir, else the cwd.
    root: PathBuf,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl WikiServer {
    /// A server exposing only `tools` (names from [`TOOLS`]).
    pub fn new(state: Arc<WikiState>, root: PathBuf, tools: &[&str]) -> Self {
        let mut tool_router = Self::tool_router();
        for name in TOOLS.iter().filter(|t| !tools.contains(t)) {
            tool_router.remove_route(name);
        }
        // Claude Code otherwise defers MCP tools behind a `ToolSearch`
        // call: one more turn before the first search (46 of 48 field runs).
        for route in tool_router.map.values_mut() {
            let mut meta = MetaObject::new();
            meta.0.insert("anthropic/alwaysLoad".into(), true.into());
            route.attr.meta = Some(meta);
        }
        Self {
            state,
            root,
            tool_router,
        }
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "Find where code or docs live in this project. Use it before grep or reading files, and read its answer before any grep. Returns one line per page (`id  path  title`) and indented `path:start-end  name  signature` lines for matching symbols: read only those lines with your file tool. A query of identifiers alone (`_ack`, `OrderBook`) lists each one's definition, or `not found: <name>`. Paths are relative to the project root; line ranges are inclusive. When you were handed a plan, spec or ledger, pass its paths in `docs` to get a step's line range (`T5k`, `NULL-FLUSH`, or its title words; several labels in one query each come back); each path is printed once, then `  start-end  label` lines. Pass the plan, spec and ledger together; only labels are searched, so use grep for text inside a doc."
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
        if let Some(docs) = p.docs.as_ref().filter(|d| !d.is_empty()) {
            if kind.is_some() || tag.is_some() {
                return Err(McpError::invalid_params(
                    "`docs` cannot be combined with kind or tag",
                    None,
                ));
            }
            let (hits, errors, more, fallback) =
                crate::labels::doc_search(&self.root, docs, &p.query, p.limit);
            let text = if json {
                doc_json(&hits, &errors, more, fallback.as_deref())
            } else {
                crate::labels::doc_lines(&hits, &errors, more, fallback.as_deref())
            };
            return Ok(text_result(Ok(text)));
        }
        let (wiki, stale) = self.state.snapshot().await;
        // A tag-only listing (no query words) stays unbounded by default.
        let has_words = p.query.chars().any(char::is_alphanumeric);
        let limit = p.limit.or(has_words.then_some(DEFAULT_SEARCH_LIMIT));
        let hits = wiki.search_tagged(&p.query, kind.as_ref(), tag.as_deref(), limit);

        let text = if json {
            search_json(hits)
        } else {
            let missing = wiki.not_found(&p.query, kind.as_ref(), tag.as_deref());
            search_lines_missing(&hits, &missing)
        };
        Ok(with_stale(text_result(Ok(text)), stale.as_deref()))
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "Show a page with the pages linked to it, within a token budget: a module's outline, the outlines of the modules it imports or is imported by (admitted first), and one-line summaries of the rest, the most central last. Use it to see what a module connects to, not grep; `kind: \"code\"` keeps it to code. With `symbol`, it lists every function that function calls, through all levels, as `start-end` lines grouped by file."
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
        annotations(read_only_hint = true),
        description = "List the functions that call a function, as `start-end  kind  name` lines grouped by file, test code last: read a caller's lines with your file tool instead of grepping for the name; use it, not grep, to find who calls a function. Calls are matched by name and argument count, not by type. `depth` goes up more levels."
    )]
    async fn callers(
        &self,
        Parameters(p): Parameters<CallersParams>,
    ) -> Result<CallToolResult, McpError> {
        let (wiki, stale) = self.state.snapshot().await;
        let result = text_result(wiki.resolve(&p.id).and_then(|id| {
            wiki.callers(
                &id,
                &p.symbol,
                Some(p.depth.unwrap_or(1)),
                Some(p.max_tokens.unwrap_or(DEFAULT_NEIGHBORS_MAX_TOKENS)),
            )
        }));
        Ok(with_stale(result, stale.as_deref()))
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "Show a module's outline: one `start-end  kind  signature` line per symbol, or a docs page's headings; a memory note's `page` is its text. Then read only the lines you need with your file tool. `full: true` returns the whole page, which can be large."
    )]
    async fn page(&self, Parameters(p): Parameters<PageParams>) -> CallToolResult {
        let full = p.full.unwrap_or(false);
        let (wiki, stale) = self.state.snapshot().await;
        let result = text_result(wiki.resolve(&p.id).and_then(|id| {
            let max_tokens = p
                .max_tokens
                .or((!full).then_some(DEFAULT_NEIGHBORS_MAX_TOKENS));
            wiki.page_text(&id, full, max_tokens)
                .ok_or_else(|| wiki.page_error(&id, &p.id))
        }));
        with_stale(result, stale.as_deref())
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "Check the wiki for broken links, orphan pages and pages too large for a neighbors pack. Returns JSON {total_pages, broken_links: [[page_id, link_text]], orphans: [page_id], large: [[page_id, tokens]]}."
    )]
    async fn lint(&self) -> Result<CallToolResult, McpError> {
        let deadline = Instant::now() + WAIT;
        let (wiki, stale) = self.state.snapshot().await;
        let ws = Arc::clone(&self.state);
        let pages = tokio::task::spawn_blocking(move || {
            crate::dirlock::shared_until(ws.dir(), deadline, || load_compiled_pages(ws.dir()))
        })
        .await
        .expect("lint read panicked")
        .transpose()
        .map_err(|e| McpError::internal_error(format!("read wiki dir: {e}"), None))?;
        // A compile holding the lock past the call: lint the snapshot.
        let (r, stale) = match pages {
            Some(pages) => (lint(&pages), stale),
            None => (lint(wiki.bodies()), stale.or(Some(RUNNING.to_string()))),
        };
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
    fn get_info(&self) -> ServerConfig {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        ServerConfig::new(capabilities)
            .with_server_info(Implementation::new("kalka", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
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
        let deadline = Instant::now() + WAIT;
        let (wiki, stale) = self.state.snapshot().await;
        let not_found = || McpError::resource_not_found(format!("no such resource: {uri}"), None);
        // Only a missing file is "no such resource"; a permission or read
        // error on a file that exists is the server's problem, not the
        // client's.
        let read_file = |name: &'static str| {
            let ws = Arc::clone(&self.state);
            async move {
                tokio::task::spawn_blocking(move || {
                    crate::dirlock::shared_until(ws.dir(), deadline, || {
                        std::fs::read_to_string(ws.dir().join(name))
                    })
                })
                .await
                .expect("read panicked")
                .ok_or_else(|| McpError::internal_error(format!("read {name}: {RUNNING}"), None))?
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        not_found()
                    } else {
                        McpError::internal_error(format!("read {name}: {e}"), None)
                    }
                })
            }
        };
        let (text, mime_type) = match uri {
            "wiki://index" => (read_file("index.json").await?, "application/json"),
            "wiki://llms.txt" => (read_file("llms.txt").await?, "text/plain"),
            _ => {
                let id = uri.strip_prefix("wiki://page/").ok_or_else(not_found)?;
                // Served from memory, keyed by the index's own ids: an id
                // with a traversal sequence is simply not found.
                let page = match wiki.page(id) {
                    Some(page) => page,
                    None if wiki.has_page(id) => {
                        return Err(McpError::resource_not_found(wiki.page_error(id, id), None))
                    }
                    None => return Err(not_found()),
                };
                (page, "text/markdown")
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

/// `docs` hits as JSON: `{"hits": [{path, start_line, end_line, label}], "errors": [..]}`, plus `"more": n` when the limit (or an empty query's default cap) cut n labels and `"fallback": line` when a fallback answered.
pub fn doc_json(
    hits: &[crate::labels::DocHit],
    errors: &[String],
    more: usize,
    fallback: Option<&str>,
) -> String {
    let hits: Vec<_> = hits
        .iter()
        .map(|h| {
            serde_json::json!({
                "path": h.path,
                "start_line": h.start,
                "end_line": h.end,
                "label": h.text,
            })
        })
        .collect();
    let mut out = serde_json::json!({"hits": hits, "errors": errors});
    if more > 0 {
        out["more"] = more.into();
    }
    if let Some(f) = fallback {
        out["fallback"] = f.into();
    }
    out.to_string()
}

/// Render search hits as the JSON array `search` returns with
/// `format: "json"`: `[{id, title, summary, updated, score, snippet, path, symbols}]`;
/// `updated` is present only when the page has a date. A memory note adds
/// `memory: true` and `origin` (`local` or `repo`), plus `author` when known
/// and `edited: true` when the file differs from its last commit.
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
                match h.origin {
                    Origin::Local => hit["origin"] = "local".into(),
                    Origin::Repo {
                        author,
                        edited,
                        mine,
                    } => {
                        hit["origin"] = "repo".into();
                        if let Some(a) = author {
                            hit["author"] = a.into();
                        }
                        if edited {
                            hit["edited"] = true.into();
                        }
                        if mine {
                            hit["mine"] = true.into();
                        }
                    }
                }
            }
            if let Some(by) = h.superseded_by {
                hit["superseded_by"] = by.into();
            }
            hit
        })
        .collect();
    serde_json::Value::Array(hits).to_string()
}

/// `memory (local)`, `memory (repo, Jane Doe)`: how a note row opens in
/// `search` lines and in the hook.
#[must_use]
pub fn memory_prefix(o: &Origin) -> String {
    format!("memory ({})", o.label())
}

/// Render search hits as the default line output: `id  path  [date  ]title — summary`
/// per page, then one `  path:start-end  name  signature  — doc` line per
/// symbol (`SymbolLine::called_by` adds `  [called by N: callers]`, callers in backticks,
/// before the doc; a function with exactly one other definition elsewhere adds
/// `    also defined: path:start-end  name`), or
/// the snippet (four-space indent) when the page has no symbols. A summary,
/// snippet or doc is cut at `LINE_CHARS`, as a hint line is.
/// A memory note renders as the hook's one line, `memory (origin): path — summary`
/// (the title when there is no summary; see [`memory_prefix`]). No hits render as one `no matches` line.
pub fn search_lines(hits: &[Hit]) -> String {
    if hits.is_empty() {
        return "no matches\n".to_string();
    }
    // Fields are one-line text; collapse any newline or tab they carry.
    let mut out = String::new();
    let shown: std::collections::HashSet<_> = hits
        .iter()
        .flat_map(|h| &h.symbols)
        .map(|s| (&s.path, s.start_line, s.end_line))
        .collect();
    for h in hits {
        if h.memory {
            let said = h.summary.as_deref().filter(|s| !s.trim().is_empty());
            out.push_str(&format!(
                "{}: {} — {}",
                memory_prefix(&h.origin),
                h.path,
                clip(said.unwrap_or(&h.title), LINE_CHARS)
            ));
            // Only a tag listing shows a superseded note.
            if let Some(by) = &h.superseded_by {
                out.push_str(&format!(" (superseded by {by})"));
            }
            out.push('\n');
            if let Some(sn) = &h.snippet {
                out.push_str(&format!("    {}\n", clip(sn, LINE_CHARS)));
            }
            continue;
        }
        out.push_str(&format!("{}  {}  ", h.id, h.path));
        if let Some(u) = &h.updated {
            out.push_str(&format!("{u}  "));
        }
        out.push_str(&one_line(&h.title));
        if let Some(s) = &h.summary {
            out.push_str(&format!(" — {}", clip(s, LINE_CHARS)));
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
                one_line(&s.signature)
            ));
            if let Some(n) = s.called_by {
                out.push_str(&format!("  [called by {n}: `callers`]"));
            }
            if let Some(d) = &s.doc {
                out.push_str(&format!("  — {}", clip(d, LINE_CHARS)));
            }
            out.push('\n');
            if let Some((p, a, b, name)) = &s.also {
                if !shown.contains(&(p, *a, *b)) {
                    out.push_str(&format!("    also defined: {p}:{a}-{b}  {name}\n"));
                }
            }
        }
        if let (true, Some(sn)) = (h.symbols.is_empty(), &h.snippet) {
            out.push_str(&format!("    {}\n", clip(sn, LINE_CHARS)));
        }
    }
    out
}

/// `search_lines` of `hits`, then one `not found: <name>` line per name in
/// `missing` (`Wiki::not_found`).
pub fn search_lines_missing(hits: &[Hit], missing: &[String]) -> String {
    let mut out = search_lines(hits);
    for name in missing {
        out.push_str(&format!("not found: {name}\n"));
    }
    out
}

/// Start the MCP server over stdio and block until the client disconnects.
/// With `source`, the server compiles it into `dir` and keeps recompiling.
/// `tools` picks the exposed tools (default [`DEFAULT_TOOLS`]). `memory`
/// lists the external memory folders: `["none"]` for no memory at all (the
/// repo's `.agents/memory` included), `[]` for
/// the discovered [`crate::memory::agent_dirs`], else exactly those folders.
pub fn run(
    dir: &Path,
    source: Option<&Path>,
    jobs: Option<usize>,
    tools: Option<&[String]>,
    memory: &[String],
) -> Result<(), WikiError> {
    LazyLock::force(&EXE);
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
            let repo_memory = memory != ["none"];
            let memory = match memory {
                [none] if none == "none" => Vec::new(),
                [] => crate::memory::agent_dirs(source),
                dirs => dirs
                    .iter()
                    .map(|d| crate::memory::resolve(Path::new(d)))
                    .collect::<Result<_, _>>()?,
            };
            let opts = CompileOptions {
                incremental: true,
                respect_ignore: true,
                emit_json: dir.join("graph.json").exists(),
                jobs,
                project: None,
                memory,
                repo_memory,
            };
            WikiState::live(Live::start(source, dir, opts)?)
        }
        None => WikiState::load(dir).map_err(|e| {
            WikiError::Serve(format!(
                "{} is not a compiled wiki ({e}) — run `kalka compile <input> {}` first",
                dir.display(),
                dir.display()
            ))
        })?,
    };
    let root = match source {
        Some(source) => source.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let server = WikiServer::new(state, root, &tools);
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
    use std::time::{Duration, Instant};

    fn fixture(files: usize) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let raw = tmp.path().join("raw");
        let out = tmp.path().join("out");
        crate::generator::generate_corpus(&raw, files, 42).unwrap();
        compile(&raw, &out, &CompileOptions::default()).unwrap();
        (tmp, out)
    }

    /// Page count of the snapshot `state` answers from now.
    fn pages(state: &Arc<WikiState>) -> usize {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(state.snapshot()).0.list_pages().len()
    }

    #[test]
    fn search_lines_prints_a_memory_hit_as_a_hook_line() {
        let hit = |memory: bool, summary: Option<&str>| Hit {
            id: "n".into(),
            title: "Vault".into(),
            path: "/m/n.md".into(),
            summary: summary.map(String::from),
            updated: Some("2026-10-23".into()),
            score: 1.0,
            snippet: Some("snip".into()),
            symbols: vec![],
            memory,
            origin: crate::model::Origin::Local,
            superseded_by: None,
        };
        assert_eq!(
            search_lines(&[hit(true, Some("Rotate nightly."))]),
            "memory (local): /m/n.md — Rotate nightly.\n    snip\n"
        );
        assert_eq!(
            search_lines(&[hit(true, None)]),
            "memory (local): /m/n.md — Vault\n    snip\n"
        );
        let mut bare = hit(true, Some("Rotate nightly."));
        bare.snippet = None;
        assert_eq!(
            search_lines(&[bare]),
            "memory (local): /m/n.md — Rotate nightly.\n"
        );
        let mut long = hit(true, Some("Rotate nightly."));
        long.snippet = Some("…the port is\n4471…".into());
        assert_eq!(
            search_lines(&[long]),
            "memory (local): /m/n.md — Rotate nightly.\n    …the port is 4471…\n"
        );
        let repo = |memory: bool| Hit {
            origin: crate::model::Origin::Repo {
                author: Some("Jane Doe".into()),
                edited: true,
                mine: true,
            },
            ..hit(memory, Some("Rotate nightly."))
        };
        assert_eq!(
            search_lines(&[repo(true)]),
            "memory (repo, Jane Doe, you, edited): /m/n.md — Rotate nightly.\n    snip\n"
        );
        let json: serde_json::Value = serde_json::from_str(&search_json(vec![repo(true)])).unwrap();
        assert_eq!(json[0]["origin"], "repo");
        assert_eq!(json[0]["author"], "Jane Doe");
        assert_eq!(json[0]["edited"], true);
        assert_eq!(json[0]["mine"], true);
        assert_eq!(json[0]["memory"], true);
        let json: serde_json::Value =
            serde_json::from_str(&search_json(vec![hit(true, None)])).unwrap();
        assert_eq!(json[0]["origin"], "local");
        for k in ["author", "edited", "mine"] {
            assert!(json[0].get(k).is_none(), "{k}");
        }
        let json: serde_json::Value =
            serde_json::from_str(&search_json(vec![repo(false)])).unwrap();
        for k in ["origin", "author", "edited", "mine", "memory"] {
            assert!(json[0].get(k).is_none(), "{k}");
        }
        let mut plain = hit(false, Some("sum"));
        plain.snippet = None;
        assert_eq!(
            search_lines(&[plain]),
            "n  /m/n.md  2026-10-23  Vault — sum\n"
        );
        let mut long = hit(false, Some(&"word ".repeat(100)));
        long.snippet = None;
        let line = search_lines(&[long]);
        let head = "n  /m/n.md  2026-10-23  Vault — ";
        assert_eq!(line.chars().count(), head.chars().count() + 160 + 1);
        assert!(line.ends_with("…\n"), "{line}");
    }

    #[test]
    fn load_fails_without_index_json() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(WikiState::load(tmp.path()).is_err());
    }

    #[test]
    fn load_does_not_wait_for_a_compile() {
        let (_tmp, out) = fixture(3);
        std::fs::create_dir(out.join(".serve")).unwrap();
        let (release, cli) = crate::dirlock::hold(&out, Duration::from_secs(10), || ());
        let state = WikiState::load(&out).unwrap();
        release.send(()).unwrap();
        cli.join().unwrap();
        assert_eq!(pages(&state), 3);
    }

    /// A compile that holds `compile.lock` on `out`, recompiling `raw` with 5
    /// files inside the hold; returns once the disk has moved on. It releases
    /// on a message, or after `WAIT` + 3 s.
    fn hold(
        tmp: &tempfile::TempDir,
        out: &Path,
    ) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
        std::fs::create_dir_all(out.join(".serve")).unwrap();
        let (raw, dir) = (tmp.path().join("raw"), out.to_path_buf());
        let (o, max) = (
            out.to_path_buf(),
            crate::live::WAIT + Duration::from_secs(3),
        );
        crate::dirlock::hold(&dir, max, move || {
            crate::generator::generate_corpus(&raw, 5, 42).unwrap();
            compile(&raw, &o, &CompileOptions::default()).unwrap();
        })
    }

    #[test]
    fn a_timed_out_snapshot_says_recompile_still_running() {
        let (tmp, out) = fixture(3);
        let state = WikiState::load(&out).unwrap();
        let (release, cli) = hold(&tmp, &out);
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let t = Instant::now();
        let (wiki, reason) = rt.block_on(state.snapshot());
        assert!(
            t.elapsed() < crate::live::WAIT + Duration::from_secs(1),
            "{:?}",
            t.elapsed()
        );
        assert_eq!(wiki.list_pages().len(), 3);
        assert_eq!(reason.as_deref(), Some(crate::live::RUNNING));
        release.send(()).unwrap();
        cli.join().unwrap();
        let (wiki, reason) = rt.block_on(state.snapshot());
        assert_eq!(wiki.list_pages().len(), 5);
        assert_eq!(reason, None);
    }

    #[test]
    fn concurrent_snapshots_share_one_wait() {
        let (tmp, out) = fixture(3);
        let state = WikiState::load(&out).unwrap();
        let (release, cli) = hold(&tmp, &out);
        let t = Instant::now();
        let calls: Vec<_> = (0..2)
            .map(|_| {
                let state = Arc::clone(&state);
                std::thread::spawn(move || {
                    let n = pages(&state);
                    (n, t.elapsed())
                })
            })
            .collect();
        let done: Vec<_> = calls.into_iter().map(|c| c.join().unwrap()).collect();
        let _ = release.send(());
        cli.join().unwrap();
        for (n, took) in done {
            assert!(
                took < crate::live::WAIT + Duration::from_secs(1),
                "{took:?}"
            );
            assert_eq!(n, 3);
        }
    }

    #[test]
    fn lint_answers_from_the_snapshot_while_a_compile_holds_the_lock() {
        let (tmp, out) = fixture(3);
        let state = WikiState::load(&out).unwrap();
        let server = WikiServer::new(Arc::clone(&state), out.clone(), &["lint"]);
        let (release, cli) = hold(&tmp, &out);
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let t = Instant::now();
        let r = rt.block_on(server.lint()).unwrap();
        // One wait, not two.
        assert!(
            t.elapsed() < crate::live::WAIT + Duration::from_secs(1),
            "{:?}",
            t.elapsed()
        );
        release.send(()).unwrap();
        cli.join().unwrap();
        let texts: Vec<&str> = r
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
            .collect();
        let report: serde_json::Value = serde_json::from_str(texts[0]).unwrap();
        assert_eq!(report["total_pages"], 3);
        assert_eq!(
            texts.last().copied(),
            Some(stale_line(crate::live::RUNNING).as_str())
        );
    }

    #[test]
    fn snapshot_lint_matches_the_disk_lint() {
        let (_tmp, out) = fixture(3);
        let wiki = Wiki::load(&out).unwrap();
        assert_eq!(
            format!("{:?}", lint(wiki.bodies())),
            format!("{:?}", lint(&load_compiled_pages(&out).unwrap()))
        );
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

    /// What a newer version may write: an old server must load or refuse it,
    /// never fall over.
    #[test]
    fn snapshot_survives_the_index_of_another_version() {
        let (_tmp, out) = fixture(3);
        let state = WikiState::load(&out).unwrap();
        let index = std::fs::read_to_string(out.join("index.json")).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&index).unwrap();

        // An added field and a garbled items.json: read as far as understood.
        value["entries"][0]["added_later"] = true.into();
        value["added_later"] = 1.into();
        std::fs::write(out.join("index.json"), value.to_string()).unwrap();
        std::fs::write(out.join("items.json"), r#"{"p":[{"kind":"new"}]}"#).unwrap();
        assert_eq!(pages(&state), 3);

        // A dropped or retyped field: the old snapshot stays.
        value["entries"][0]["pagerank"] = "high".into();
        value["entries"][1].as_object_mut().unwrap().remove("title");
        std::fs::write(out.join("index.json"), value.to_string()).unwrap();
        assert_eq!(pages(&state), 3);
    }
}
