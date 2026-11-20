//! MCP server for a compiled wiki (`wiki serve`): exposes
//! search/page/callers (and, on request, neighbors and lint) as tools and
//! pages as resources over stdio.

use crate::lint::{lint, load_compiled_pages};
use crate::live::{Live, EXE, OUTDATED, RUNNING, WAIT};
use crate::model::KindFilter;
use crate::query::PackBudget;
use crate::query::Wiki;
use crate::{CompileOptions, WikiError};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Instant, SystemTime};

mod render;

pub use crate::query::DEFAULT_NEIGHBORS_MAX_TOKENS;
pub use render::{doc_json, memory_prefix, search_json, search_lines, search_lines_missing};

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
            // The fingerprint is taken first (a later change is then seen), but
            // its error is reported after the load's, which names the file.
            let fp = fingerprint(dir);
            let wiki = Wiki::load(dir)?;
            Ok::<_, std::io::Error>((fp?, wiki))
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

    /// The wiki to answer from, and why it may be stale. A live wait blocks
    /// only the request's own thread, so pings and other calls keep working.
    pub fn snapshot(&self) -> (Arc<Wiki>, Option<String>) {
        let deadline = Instant::now() + WAIT;
        let outdated = EXE.replaced().then(|| OUTDATED.to_string());
        match &self.freshness {
            Freshness::Live(live, _) if outdated.is_none() => live.fresh(),
            Freshness::Live(live, frozen) => {
                frozen.get_or_init(|| {
                    Mutex::new(Loaded {
                        wiki: live.wiki(),
                        fingerprint: None,
                    })
                });
                self.reload_or_running(deadline, outdated)
            }
            Freshness::Fingerprint(_) => self.reload_or_running(deadline, outdated),
        }
    }

    /// [`Self::reload`]; past `deadline`, `stale` falls back to `RUNNING`.
    fn reload_or_running(
        &self,
        deadline: Instant,
        stale: Option<String>,
    ) -> (Arc<Wiki>, Option<String>) {
        let (wiki, done) = self.reload(deadline);
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

/// A JSON-RPC error: code and message.
pub(crate) type RpcError = (i64, String);
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
const RESOURCE_NOT_FOUND: i64 = -32002;

/// Versions `initialize` agrees to, oldest first: the client's own if
/// listed, else the newest (what rmcp 3.10 answered).
const PROTOCOL_VERSIONS: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

/// Every tool's `tools/list` entry, sorted by name. `_meta.anthropic/alwaysLoad`:
/// Claude Code otherwise defers MCP tools behind a `ToolSearch` call, one
/// more turn before the first search (46 of 48 field runs).
const TOOLS_JSON: &str = include_str!("serve/tools.json");

fn invalid_params(message: impl Into<String>) -> RpcError {
    (INVALID_PARAMS, message.into())
}

/// A `tools/call` result: the text (an error result for `Err`), then the
/// stale line, if any, as a second text block.
fn tool_result(r: Result<String, String>, stale: Option<&str>) -> Value {
    let (text, is_error) = match r {
        Ok(text) => (text, false),
        Err(text) => (text, true),
    };
    let mut content = vec![json!({"type": "text", "text": text})];
    if let Some(reason) = stale {
        content.push(json!({"type": "text", "text": stale_line(reason)}));
    }
    json!({"content": content, "isError": is_error})
}

/// Tool arguments; one that does not fit is a tool error result, not a
/// JSON-RPC error, so the model sees which field is wrong.
fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, Value> {
    serde_json::from_value(args)
        .map_err(|e| tool_result(Err(format!("failed to deserialize parameters: {e}")), None))
}

// Field docs are the schemas in `serve/tools.json`.
#[derive(Deserialize)]
pub(crate) struct SearchParams {
    pub query: String,
    pub kind: Option<String>,
    pub tag: Option<String>,
    pub limit: Option<usize>,
    pub format: Option<String>,
    pub docs: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub(crate) struct NeighborsParams {
    pub id: String,
    pub depth: Option<usize>,
    pub max_tokens: Option<usize>,
    pub max_nodes: Option<usize>,
    pub full: Option<bool>,
    pub kind: Option<String>,
    pub symbol: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct CallersParams {
    pub id: String,
    pub symbol: String,
    pub depth: Option<usize>,
    pub max_tokens: Option<usize>,
}

#[derive(Deserialize)]
pub(crate) struct PageParams {
    pub id: String,
    pub full: Option<bool>,
    pub max_tokens: Option<usize>,
}

pub(crate) struct WikiServer {
    state: Arc<WikiState>,
    /// Where `docs` paths are resolved: the `--source` dir, else the cwd.
    root: PathBuf,
    /// The `tools/list` entries of the exposed tools.
    tools: Vec<Value>,
}

impl WikiServer {
    /// A server exposing only `tools` (names from [`TOOLS`]).
    pub fn new(state: Arc<WikiState>, root: PathBuf, tools: &[&str]) -> Self {
        let all: Vec<Value> = serde_json::from_str(TOOLS_JSON).expect("tools.json");
        let tools = all
            .into_iter()
            .filter(|t| tools.iter().any(|name| t["name"] == *name))
            .collect();
        Self { state, root, tools }
    }

    /// The reply to one message from the client: `None` for a notification
    /// or a response.
    fn handle(&self, line: &str) -> Option<Value> {
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            return Some(reply(None, Err((-32700, "Parse error".into()))));
        };
        if msg["jsonrpc"] != "2.0" {
            return Some(reply(None, Err((-32600, "Invalid request".into()))));
        }
        let (Some(id), Some(method)) = (msg.get("id"), msg["method"].as_str()) else {
            return None;
        };
        let params = &msg["params"];
        let r = match method {
            "initialize" => {
                let asked = params["protocolVersion"].as_str();
                let version = asked
                    .filter(|v| PROTOCOL_VERSIONS.contains(v))
                    .unwrap_or(PROTOCOL_VERSIONS[3]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"resources": {}, "tools": {}},
                    "serverInfo": {"name": "wiki", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": self.tools})),
            "tools/call" => self.call_tool(params),
            "resources/list" => Ok(self.list_resources()),
            // Clients with the resources capability may ask; rmcp answered.
            "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
            "resources/read" => match params["uri"].as_str() {
                Some(uri) => self.read_resource(uri),
                None => Err(invalid_params("missing uri")),
            },
            _ => Err((METHOD_NOT_FOUND, method.to_string())),
        };
        Some(reply(Some(id), r))
    }

    fn call_tool(&self, params: &Value) -> Result<Value, RpcError> {
        let name = params["name"]
            .as_str()
            .filter(|name| self.tools.iter().any(|t| t["name"] == *name))
            .ok_or_else(|| invalid_params("tool not found"))?;
        let args = match &params["arguments"] {
            Value::Null => json!({}),
            args => args.clone(),
        };
        let r = match name {
            "search" => parse(args).map(|p| self.search(&p)),
            "neighbors" => parse(args).map(|p| self.neighbors(&p)),
            "callers" => parse(args).map(|p| Ok(self.callers(&p))),
            "page" => parse(args).map(|p| Ok(self.page(&p))),
            _ => Ok(self.lint()),
        };
        r.unwrap_or_else(Ok)
    }

    fn search(&self, p: &SearchParams) -> Result<Value, RpcError> {
        let kind = p
            .kind
            .as_deref()
            .map(KindFilter::parse_arg)
            .transpose()
            .map_err(invalid_params)?;
        let tag = p
            .tag
            .as_deref()
            .map(crate::query::tag_arg)
            .transpose()
            .map_err(invalid_params)?;
        let json = match p.format.as_deref() {
            None | Some("lines") => false,
            Some("json") => true,
            Some(f) => {
                return Err(invalid_params(format!(
                    "unknown format {f:?}; expected lines or json"
                )))
            }
        };
        if let Some(docs) = p.docs.as_ref().filter(|d| !d.is_empty()) {
            if kind.is_some() || tag.is_some() {
                return Err(invalid_params("`docs` cannot be combined with kind or tag"));
            }
            let (hits, errors, more, fallback) =
                crate::labels::doc_search(&self.root, docs, &p.query, p.limit);
            let text = if json {
                doc_json(&hits, &errors, more, fallback.as_deref())
            } else {
                crate::labels::doc_lines(&hits, &errors, more, fallback.as_deref())
            };
            return Ok(tool_result(Ok(text), None));
        }
        let (wiki, stale) = self.state.snapshot();
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
        Ok(tool_result(Ok(text), stale.as_deref()))
    }

    fn neighbors(&self, p: &NeighborsParams) -> Result<Value, RpcError> {
        let kind = p
            .kind
            .as_deref()
            .map(KindFilter::parse_arg)
            .transpose()
            .map_err(invalid_params)?;
        let budget = PackBudget {
            max_tokens: Some(p.max_tokens.unwrap_or(DEFAULT_NEIGHBORS_MAX_TOKENS)),
            max_nodes: p.max_nodes,
            full_neighbors: p.full.unwrap_or(false),
            kind,
        };
        let (wiki, stale) = self.state.snapshot();
        let r = wiki.resolve(&p.id).and_then(|id| match &p.symbol {
            Some(symbol) => wiki.calls(&id, symbol, p.depth, budget.max_tokens),
            None => wiki
                .neighbors(&id, p.depth.unwrap_or(1), &budget)
                .map(|pack| pack.text)
                .ok_or_else(|| crate::query::unknown_page(&p.id, &[])),
        });
        Ok(tool_result(r, stale.as_deref()))
    }

    fn callers(&self, p: &CallersParams) -> Value {
        let (wiki, stale) = self.state.snapshot();
        let r = wiki.resolve(&p.id).and_then(|id| {
            wiki.callers(
                &id,
                &p.symbol,
                Some(p.depth.unwrap_or(1)),
                Some(p.max_tokens.unwrap_or(DEFAULT_NEIGHBORS_MAX_TOKENS)),
            )
        });
        tool_result(r, stale.as_deref())
    }

    fn page(&self, p: &PageParams) -> Value {
        let full = p.full.unwrap_or(false);
        let (wiki, stale) = self.state.snapshot();
        let r = wiki.resolve(&p.id).and_then(|id| {
            let max_tokens = p
                .max_tokens
                .or((!full).then_some(DEFAULT_NEIGHBORS_MAX_TOKENS));
            wiki.page_text(&id, full, max_tokens)
                .ok_or_else(|| wiki.page_error(&id, &p.id))
        });
        tool_result(r, stale.as_deref())
    }

    fn lint(&self) -> Result<Value, RpcError> {
        let deadline = Instant::now() + WAIT;
        let (wiki, stale) = self.state.snapshot();
        let dir = self.state.dir();
        let pages = crate::dirlock::shared_until(dir, deadline, || load_compiled_pages(dir))
            .transpose()
            .map_err(|e| (INTERNAL_ERROR, format!("read wiki dir: {e}")))?;
        // A compile holding the lock past the call: lint the snapshot.
        let (r, stale) = match pages {
            Some(pages) => (lint(&pages), stale),
            None => (lint(wiki.bodies()), stale.or(Some(RUNNING.to_string()))),
        };
        let report = json!({
            "total_pages": r.total_pages,
            "broken_links": r.broken_links,
            "orphans": r.orphans,
            "large": r.large,
        });
        Ok(tool_result(Ok(report.to_string()), stale.as_deref()))
    }

    fn list_resources(&self) -> Value {
        let resource = |uri: String, name: &str, mime: &str| json!({"uri": uri, "name": name, "mimeType": mime});
        let mut resources = vec![
            resource(
                "wiki://index".into(),
                "index.json (machine catalog)",
                "application/json",
            ),
            resource(
                "wiki://llms.txt".into(),
                "llms.txt (compact LLM index)",
                "text/plain",
            ),
        ];
        let (wiki, _) = self.state.snapshot();
        for (id, title) in wiki.list_pages() {
            resources.push(resource(
                format!("wiki://page/{id}"),
                &title,
                "text/markdown",
            ));
        }
        json!({"resources": resources})
    }

    fn read_resource(&self, uri: &str) -> Result<Value, RpcError> {
        let deadline = Instant::now() + WAIT;
        let (wiki, stale) = self.state.snapshot();
        let not_found = || (RESOURCE_NOT_FOUND, format!("no such resource: {uri}"));
        // Only a missing file is "no such resource"; a permission or read
        // error on a file that exists is the server's problem, not the
        // client's.
        let read_file = |name: &str| {
            let dir = self.state.dir();
            crate::dirlock::shared_until(dir, deadline, || std::fs::read_to_string(dir.join(name)))
                .ok_or_else(|| (INTERNAL_ERROR, format!("read {name}: {RUNNING}")))?
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        not_found()
                    } else {
                        (INTERNAL_ERROR, format!("read {name}: {e}"))
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
                let page = match wiki.page(id) {
                    Some(page) => page,
                    None if wiki.has_page(id) => {
                        return Err((RESOURCE_NOT_FOUND, wiki.page_error(id, id)))
                    }
                    None => return Err(not_found()),
                };
                (page, "text/markdown")
            }
        };
        let mut contents = vec![json!({"uri": uri, "mimeType": mime_type, "text": text})];
        if let Some(reason) = stale {
            contents
                .push(json!({"uri": uri, "mimeType": "text/plain", "text": stale_line(&reason)}));
        }
        Ok(json!({"contents": contents}))
    }
}

/// A JSON-RPC response; `id` is `None` when the message had none to echo.
fn reply(id: Option<&Value>, r: Result<Value, RpcError>) -> Value {
    let mut msg = json!({"jsonrpc": "2.0"});
    if let Some(id) = id {
        msg["id"] = id.clone();
    }
    match r {
        Ok(result) => msg["result"] = result,
        Err((code, message)) => msg["error"] = json!({"code": code, "message": message}),
    }
    msg
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
            WikiError::Serve(format!("{} is not a compiled wiki ({e})", dir.display()))
        })?,
    };
    let root = match source {
        Some(source) => source.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let server = WikiServer::new(state, root, &tools);
    // Newline-delimited JSON-RPC, one thread per message: a call waiting
    // on a compile holds up nothing else. Closing stdin waits for the
    // calls in flight, then returns.
    let read_any = std::thread::scope(|s| -> std::io::Result<bool> {
        let mut read_any = false;
        for line in std::io::stdin().lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            read_any = true;
            let server = &server;
            s.spawn(move || {
                if let Some(reply) = server.handle(&line) {
                    // A locked write keeps each reply on its own line.
                    let _ = writeln!(std::io::stdout().lock(), "{reply}");
                }
            });
        }
        Ok(read_any)
    })?;
    if !read_any {
        return Err(WikiError::Serve(
            "connection closed: initialize request".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
