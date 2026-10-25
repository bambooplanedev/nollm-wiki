//! End-to-end test: spawn `wiki serve` and speak raw newline-delimited
//! JSON-RPC (the MCP stdio transport) over its stdin/stdout.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

mod common;

/// How long a single response may take before the test fails instead of
/// hanging the whole `cargo test` run on a server that never answers.
const RESPONSE_DEADLINE: Duration = Duration::from_secs(10);

fn compile_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    common::seed_corpus(12)
}

/// Spawn `wiki serve` and hand back its stdin plus a channel of stdout
/// lines. A pipe has no read timeout, so a thread does the blocking read
/// and `read_response` waits on the channel with a deadline. The channel
/// closes when the server closes stdout.
fn spawn_server(dir: &std::path::Path) -> (Child, ChildStdin, Receiver<String>) {
    spawn_server_with(dir, &[])
}

/// `spawn_server` plus `extra` args after `--dir <dir>`.
fn spawn_server_with(
    dir: &std::path::Path,
    extra: &[&str],
) -> (Child, ChildStdin, Receiver<String>) {
    spawn("--dir", dir, extra)
}

/// Spawn `wiki serve <flag> <path>` plus `extra` args.
fn spawn(
    flag: &str,
    path: &std::path::Path,
    extra: &[&str],
) -> (Child, ChildStdin, Receiver<String>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wiki"))
        .args(["serve", flag])
        .arg(path)
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in stdout.lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    (child, stdin, rx)
}

fn send(stdin: &mut ChildStdin, msg: &Value) {
    let mut line = msg.to_string();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
}

/// Read lines until the response with the given id arrives (skips
/// notifications and unrelated messages).
fn read_response(stdout: &Receiver<String>, id: u64) -> Value {
    loop {
        let line = match stdout.recv_timeout(RESPONSE_DEADLINE) {
            Ok(line) => line,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("no response to id {id} within {RESPONSE_DEADLINE:?}")
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("server closed stdout before responding to id {id}")
            }
        };
        let v: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v["id"] == json!(id) {
            return v;
        }
    }
}

fn send_call(stdin: &mut ChildStdin, id: u64, tool: &str, args: &Value) {
    send(
        stdin,
        &json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {
            "name": tool, "arguments": args}}),
    );
}

/// `tools/call` and wait for its response.
fn call(
    stdin: &mut ChildStdin,
    stdout: &Receiver<String>,
    id: u64,
    tool: &str,
    args: &Value,
) -> Value {
    send_call(stdin, id, tool, args);
    read_response(stdout, id)
}

/// The text of a `tools/call` response.
fn text_of(resp: &Value) -> &str {
    resp["result"]["content"][0]["text"].as_str().unwrap()
}

fn initialize(stdin: &mut ChildStdin, stdout: &Receiver<String>) {
    send(
        stdin,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "test-client", "version": "0.0.0"}
        }}),
    );
    let resp = read_response(stdout, 1);
    assert_eq!(resp["result"]["serverInfo"]["name"], "wiki");
    assert_eq!(resp["result"]["instructions"], wiki::serve::INSTRUCTIONS);
    send(
        stdin,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    );
}

#[test]
fn serve_lists_and_calls_tools() {
    let (_tmp, out) = compile_fixture();
    let (mut child, mut stdin, stdout) = spawn_server(&out);
    initialize(&mut stdin, &stdout);

    // tools/list
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    );
    let resp = read_response(&stdout, 2);
    let names: Vec<&str> = resp["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    // `lint` is left out unless `--tools` names it.
    assert_eq!(
        sorted,
        ["callers", "neighbors", "page", "search"],
        "tools: {names:?}"
    );
    // Claude Code loads them up front instead of behind `ToolSearch`.
    for t in resp["result"]["tools"].as_array().unwrap() {
        assert_eq!(t["_meta"]["anthropic/alwaysLoad"], true, "{t}");
    }

    // tools/call search — seed-42 corpus contains gradient_descent
    let resp = call(
        &mut stdin,
        &stdout,
        3,
        "search",
        &json!({"query": "gradient"}),
    );
    let text = text_of(&resp);
    assert!(text.contains("gradient_descent"), "search result: {text}");

    // tools/call neighbors
    let resp = call(
        &mut stdin,
        &stdout,
        4,
        "neighbors",
        &json!({"id": "gradient_descent", "max_tokens": 800}),
    );
    let text = text_of(&resp);
    assert!(text.starts_with("# Gradient Descent"), "pack: {text}");

    // tools/call neighbors with unknown id -> tool error result
    let resp = call(
        &mut stdin,
        &stdout,
        5,
        "neighbors",
        &json!({"id": "no_such_page"}),
    );
    assert_eq!(resp["result"]["isError"], json!(true));

    drop(stdin); // close stdin -> server exits
    let _ = child.wait();
}

#[test]
fn serve_tools_flag_picks_the_tools() {
    let (_tmp, out) = compile_fixture();
    let (mut child, mut stdin, stdout) = spawn_server_with(&out, &["--tools", "search,lint"]);
    initialize(&mut stdin, &stdout);
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    );
    let resp = read_response(&stdout, 2);
    let mut names: Vec<&str> = resp["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["lint", "search"]);

    let resp = call(&mut stdin, &stdout, 3, "lint", &json!({}));
    let text = text_of(&resp);
    let report: Value = serde_json::from_str(text).unwrap();
    assert_eq!(report["broken_links"].as_array().unwrap().len(), 0);
    assert_eq!(report["large"], json!([]));

    // A tool left out is not callable.
    let resp = call(
        &mut stdin,
        &stdout,
        4,
        "page",
        &json!({"id": "gradient_descent"}),
    );
    assert!(
        resp.get("error").is_some() || resp["result"]["isError"] == json!(true),
        "resp: {resp}"
    );
    child.kill().ok();
}

#[test]
fn serve_refuses_an_unknown_tool() {
    let (_tmp, out) = compile_fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_wiki"))
        .args(["serve", "--tools", "search,nope", "--dir"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown tool \"nope\""), "stderr: {stderr}");
}

#[test]
fn mcp_search_returns_five_hits_unless_asked() {
    let (_tmp, out) = compile_fixture();
    let (mut child, mut stdin, stdout) = spawn_server(&out);
    initialize(&mut stdin, &stdout);
    let count = |stdin: &mut ChildStdin, id: u64, args: Value| {
        let resp = call(stdin, &stdout, id, "search", &args);
        serde_json::from_str::<Value>(text_of(&resp))
            .unwrap()
            .as_array()
            .unwrap()
            .len()
    };
    // `the` hits 10 of the 12 corpus pages.
    assert_eq!(
        count(&mut stdin, 2, json!({"query": "the", "format": "json"})),
        5
    );
    assert_eq!(
        count(
            &mut stdin,
            3,
            json!({"query": "the", "format": "json", "limit": 8})
        ),
        8
    );
    child.kill().ok();
}

#[test]
fn serve_lists_and_reads_resources() {
    let (_tmp, out) = compile_fixture();
    let (mut child, mut stdin, stdout) = spawn_server(&out);
    initialize(&mut stdin, &stdout);

    // resources/list: 12 pages + index + llms.txt
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "resources/list"}),
    );
    let resp = read_response(&stdout, 2);
    let resources = resp["result"]["resources"].as_array().unwrap();
    let uris: Vec<&str> = resources
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    assert_eq!(uris.len(), 14, "uris: {uris:?}");
    assert!(uris.contains(&"wiki://index"));
    assert!(uris.contains(&"wiki://llms.txt"));
    assert!(uris.contains(&"wiki://page/gradient_descent"));

    // resources/list: MIME types are set per the spec.
    let index_entry = resources
        .iter()
        .find(|r| r["uri"] == json!("wiki://index"))
        .unwrap();
    assert_eq!(index_entry["mimeType"], json!("application/json"));
    let page_entry = resources
        .iter()
        .find(|r| r["uri"] == json!("wiki://page/gradient_descent"))
        .unwrap();
    assert_eq!(page_entry["mimeType"], json!("text/markdown"));

    // resources/read a page
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 3, "method": "resources/read", "params": {
            "uri": "wiki://page/gradient_descent"
        }}),
    );
    let resp = read_response(&stdout, 3);
    let text = resp["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("# Gradient Descent"));
    assert_eq!(
        resp["result"]["contents"][0]["mimeType"],
        json!("text/markdown")
    );

    // resources/read the index
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 4, "method": "resources/read", "params": {
            "uri": "wiki://index"
        }}),
    );
    let resp = read_response(&stdout, 4);
    let text = resp["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(
        serde_json::from_str::<Value>(text).is_ok(),
        "index.json should be valid JSON"
    );

    // unknown resource -> JSON-RPC error
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 5, "method": "resources/read", "params": {
            "uri": "wiki://page/no_such_page"
        }}),
    );
    let resp = read_response(&stdout, 5);
    assert!(resp["error"].is_object(), "expected error, got: {resp}");

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn serve_refuses_path_traversal_outside_index() {
    let (_tmp, out) = compile_fixture();
    // Plant a decoy file outside the served directory.
    std::fs::write(out.parent().unwrap().join("secret.md"), "# Secret").unwrap();

    let (mut child, mut stdin, stdout) = spawn_server(&out);
    initialize(&mut stdin, &stdout);

    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "resources/read", "params": {
            "uri": "wiki://page/../secret"
        }}),
    );
    let resp = read_response(&stdout, 2);
    assert!(resp["error"].is_object(), "expected error, got: {resp}");

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn serve_refuses_non_wiki_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_wiki"))
        .args(["serve", "--dir"])
        .arg(tmp.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("compile"), "stderr: {stderr}");
}

#[test]
fn search_hits_include_snippet_field() {
    let (_tmp, out) = compile_fixture();
    let (mut child, mut stdin, stdout) = spawn_server(&out);
    initialize(&mut stdin, &stdout);
    let resp = call(
        &mut stdin,
        &stdout,
        7,
        "search",
        &json!({"query": "gradient", "format": "json"}),
    );
    let text = text_of(&resp);
    let hits: Value = serde_json::from_str(text).unwrap();
    let arr = hits.as_array().expect("hits array");
    assert_ne!(*arr, [] as [Value; 0]);
    for h in arr {
        assert!(h.get("snippet").is_some(), "hit missing snippet key: {h}");
    }
    child.kill().ok();
}

/// The text of one `search` call against a compiled `source_project`.
fn search_text(args: &Value) -> String {
    let src = source_project();
    let out = src.path().join("out");
    wiki::compile(src.path(), &out, &wiki::CompileOptions::default()).unwrap();
    let (mut child, mut stdin, stdout) = spawn_server(&out);
    initialize(&mut stdin, &stdout);
    let resp = call(&mut stdin, &stdout, 9, "search", args);
    child.kill().ok();
    text_of(&resp).to_string()
}

#[test]
fn mcp_search_tag_only_lists_pages() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    std::fs::write(
        tmp.path().join("a.md"),
        "---\ntags: [mem]\n---\n# Tagged\n\nzebra\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join("b.md"), "# Plain\n\nzebra\n").unwrap();
    wiki::compile(tmp.path(), &out, &wiki::CompileOptions::default()).unwrap();
    let (mut child, mut stdin, stdout) = spawn_server(&out);
    initialize(&mut stdin, &stdout);
    let resp = call(
        &mut stdin,
        &stdout,
        9,
        "search",
        &json!({"query": "", "tag": "mem"}),
    );
    child.kill().ok();
    let text = text_of(&resp);
    assert!(text.contains("tagged  "), "text: {text}");
    assert!(!text.contains("plain  "), "text: {text}");
}

#[test]
fn mcp_search_defaults_to_lines() {
    let text = search_text(&json!({"query": "add"}));
    let first = text.lines().next().unwrap();
    let (id, rest) = first.split_once("  ").expect("id, two spaces, path");
    assert!(
        !id.contains(' ') && rest.contains("lib.rs"),
        "first: {first}"
    );
    let sym = regex::Regex::new(r"(?m)^  \S+:\d+-\d+  ").unwrap();
    assert!(sym.is_match(&text), "no symbol line: {text}");
}

#[test]
fn mcp_search_json_on_request() {
    let text = search_text(&json!({"query": "add", "format": "json"}));
    let hits: Value = serde_json::from_str(&text).unwrap();
    let hit = &hits.as_array().unwrap()[0];
    assert!(
        hit["symbols"].is_array() && hit["path"].is_string(),
        "{hit}"
    );
}

/// Bad-parameter paths and the over-budget neighbors block, then a final
/// `tools/list` to prove none of them took the server down.
#[test]
fn serve_reports_bad_params_and_survives() {
    let (_tmp, out) = compile_fixture();
    let (mut child, mut stdin, stdout) = spawn_server(&out);
    initialize(&mut stdin, &stdout);

    // (a) An unknown kind is rejected inside the handler with
    // `McpError::invalid_params`, which rmcp surfaces as a JSON-RPC error
    // (not an `isError` result).
    let resp = call(
        &mut stdin,
        &stdout,
        2,
        "search",
        &json!({"kind": "bogus", "query": "x"}),
    );
    assert_eq!(resp["error"]["code"], json!(-32602), "got: {resp}");
    let msg = resp["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("expected code, docs, text, markdown, or code:<lang>"),
        "message: {msg}"
    );
    assert!(resp.get("result").is_none(), "got: {resp}");

    // An empty tag is rejected the same way.
    let resp = call(
        &mut stdin,
        &stdout,
        7,
        "search",
        &json!({"query": "x", "tag": "#"}),
    );
    assert_eq!(resp["error"]["code"], json!(-32602), "got: {resp}");
    let msg = resp["error"]["message"].as_str().unwrap();
    assert!(msg.contains("empty tag"), "message: {msg}");

    // (b) A missing required field fails parameter deserialization, which
    // rmcp reports as a tool error result naming the field.
    let resp = call(&mut stdin, &stdout, 3, "search", &json!({}));
    assert_eq!(resp["result"]["isError"], json!(true), "got: {resp}");
    let text = text_of(&resp);
    assert!(text.contains("query"), "text: {text}");

    // (c) Unknown tool name -> JSON-RPC invalid params.
    let resp = call(&mut stdin, &stdout, 4, "nope", &json!({}));
    assert_eq!(resp["error"]["code"], json!(-32602), "got: {resp}");

    // (d) A budget the target's own page cannot fit degrades the target
    // block instead of failing.
    let resp = call(
        &mut stdin,
        &stdout,
        5,
        "neighbors",
        &json!({"id": "gradient_descent", "max_tokens": 1}),
    );
    assert_eq!(resp["result"]["isError"], json!(false), "got: {resp}");
    let text = text_of(&resp);
    assert!(text.contains(wiki::query::OVER_BUDGET_NOTE), "pack: {text}");
    assert!(text.starts_with("# Gradient Descent"), "pack: {text}");

    // (e) Still answering after every error above.
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 6, "method": "tools/list"}),
    );
    let resp = read_response(&stdout, 6);
    assert!(
        resp["result"]["tools"]
            .as_array()
            .is_some_and(|t| !t.is_empty()),
        "server did not survive the error sequence: {resp}"
    );

    drop(stdin);
    let _ = child.wait();
}

/// An agent that omits `max_tokens` must not get an unbounded dump: the
/// server applies a default ceiling that the CLI, driven by a person passing
/// flags, deliberately does not.
#[test]
fn neighbors_defaults_to_a_token_ceiling() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path().join("raw");
    let out = tmp.path().join("out");
    std::fs::create_dir_all(&raw).unwrap();
    // ~40k chars, ~10k tokens: far over the default ceiling on its own.
    let huge = "# Huge\n\nHuge mentions Small. ".to_string() + &"filler text. ".repeat(3000);
    std::fs::write(raw.join("huge.txt"), huge).unwrap();
    std::fs::write(raw.join("small.txt"), "# Small\n\nSmall body.\n").unwrap();
    wiki::compile(&raw, &out, &wiki::CompileOptions::default()).unwrap();

    let (mut child, mut stdin, stdout) = spawn_server(&out);
    initialize(&mut stdin, &stdout);
    let resp = call(&mut stdin, &stdout, 2, "neighbors", &json!({"id": "huge"}));
    let text = text_of(&resp);
    assert!(
        text.chars().count() / 4 <= wiki::serve::DEFAULT_NEIGHBORS_MAX_TOKENS,
        "defaults-only pack exceeds the default ceiling: {} chars",
        text.len()
    );
    assert!(
        text.contains("wiki://page/huge"),
        "target should degrade: {text}"
    );

    drop(stdin);
    let _ = child.wait();
}

/// Spawn `wiki serve --source <source>` plus `extra` args.
fn spawn_server_source(
    source: &std::path::Path,
    extra: &[&str],
) -> (Child, ChildStdin, Receiver<String>) {
    spawn("--source", source, extra)
}

/// A project with one note and one Rust file, not yet compiled.
fn source_project() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("notes")).unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("notes/alpha.md"),
        "# Alpha\n\noldword here\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("src/lib.rs"),
        "/// Adds.\npub fn add() {}\n",
    )
    .unwrap();
    tmp
}

fn send_search(stdin: &mut ChildStdin, id: u64, query: &str) {
    send_call(
        stdin,
        id,
        "search",
        &json!({"query": query, "format": "json"}),
    );
}

/// Ids of the hits in a `search` response.
fn hit_ids(resp: &Value) -> Vec<String> {
    let text = text_of(resp);
    let hits: Value = serde_json::from_str(text).unwrap();
    hits.as_array()
        .unwrap()
        .iter()
        .map(|h| h["id"].as_str().unwrap().to_string())
        .collect()
}

/// The `wiki may be stale: …` block of a tool response, if any.
fn stale_block(resp: &Value) -> Option<String> {
    resp["result"]["content"]
        .as_array()?
        .iter()
        .filter_map(|c| c["text"].as_str())
        .find(|t| t.starts_with("wiki may be stale:"))
        .map(str::to_string)
}

fn search(stdin: &mut ChildStdin, stdout: &Receiver<String>, id: u64, query: &str) -> Value {
    send_search(stdin, id, query);
    read_response(stdout, id)
}

#[test]
fn serve_source_compiles_missing_wiki_and_answers() {
    let project = source_project();
    let (mut child, mut stdin, stdout) = spawn_server_source(project.path(), &[]);
    initialize(&mut stdin, &stdout);

    let resp = search(&mut stdin, &stdout, 2, "oldword");
    let ids = hit_ids(&resp);
    assert!(ids.iter().any(|id| id.contains("alpha")), "ids: {ids:?}");
    assert!(project.path().join(".wiki/index.json").exists());
    assert_eq!(stale_block(&resp), None);

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn serve_source_catches_up_an_existing_wiki() {
    let project = source_project();
    let opts = wiki::CompileOptions {
        incremental: true,
        ..Default::default()
    };
    wiki::compile(project.path(), &project.path().join(".wiki"), &opts).unwrap();
    // Edits made while no server ran: the start-up compile must pick them up.
    std::fs::write(
        project.path().join("notes/alpha.md"),
        "# Alpha\n\nzebrafish here\n",
    )
    .unwrap();
    std::fs::write(project.path().join("notes/gamma.md"), "# Gamma\n\nquokka\n").unwrap();

    let (mut child, mut stdin, stdout) = spawn_server_source(project.path(), &[]);
    initialize(&mut stdin, &stdout);
    for (id, word) in [(2, "zebrafish"), (3, "quokka")] {
        let resp = search(&mut stdin, &stdout, id, word);
        assert!(!hit_ids(&resp).is_empty(), "{word} not seen: {resp}");
        assert_eq!(stale_block(&resp), None);
    }

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn serve_source_sees_edit_without_sleep() {
    let project = source_project();
    let (mut child, mut stdin, stdout) = spawn_server_source(project.path(), &[]);
    initialize(&mut stdin, &stdout);
    assert_ne!(
        hit_ids(&search(&mut stdin, &stdout, 2, "oldword")),
        [] as [String; 0]
    );

    std::fs::write(
        project.path().join("notes/alpha.md"),
        "# Alpha\n\nzebrafish here\n",
    )
    .unwrap();
    let resp = search(&mut stdin, &stdout, 3, "zebrafish");
    assert!(!hit_ids(&resp).is_empty(), "edit not seen: {resp}");
    assert_eq!(stale_block(&resp), None);

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn serve_source_sees_file_in_new_subdir() {
    let project = source_project();
    let (mut child, mut stdin, stdout) = spawn_server_source(project.path(), &[]);
    initialize(&mut stdin, &stdout);
    assert_ne!(
        hit_ids(&search(&mut stdin, &stdout, 2, "oldword")),
        [] as [String; 0]
    );

    std::fs::create_dir_all(project.path().join("notes/deep")).unwrap();
    std::fs::write(
        project.path().join("notes/deep/beta.md"),
        "# Beta\n\nquokka\n",
    )
    .unwrap();
    let resp = search(&mut stdin, &stdout, 3, "quokka");
    let ids = hit_ids(&resp);
    assert!(ids.iter().any(|id| id.contains("beta")), "ids: {ids:?}");

    drop(stdin);
    let _ = child.wait();
}

#[cfg(unix)]
#[test]
fn serve_source_failed_compile_keeps_snapshot_and_says_stale() {
    use std::os::unix::fs::PermissionsExt;
    let project = source_project();
    let wiki_dir = project.path().join(".wiki");
    let (mut child, mut stdin, stdout) = spawn_server_source(project.path(), &[]);
    initialize(&mut stdin, &stdout);
    assert_ne!(
        hit_ids(&search(&mut stdin, &stdout, 2, "oldword")),
        [] as [String; 0]
    );

    std::fs::set_permissions(&wiki_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    std::fs::write(
        project.path().join("notes/alpha.md"),
        "# Alpha\n\nzebrafish here\n",
    )
    .unwrap();
    let resp = search(&mut stdin, &stdout, 3, "oldword");
    std::fs::set_permissions(&wiki_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    let ids = hit_ids(&resp);
    assert!(ids.iter().any(|id| id.contains("alpha")), "ids: {ids:?}");
    let stale = stale_block(&resp);
    assert!(stale.is_some(), "no stale line: {resp}");
    assert!(
        !stale.unwrap().contains("recompile still running"),
        "the compile should have failed, not timed out: {resp}"
    );

    drop(stdin);
    let _ = child.wait();
}

#[cfg(unix)]
#[test]
fn serve_source_through_symlink_answers_fresh() {
    let project = source_project();
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("project");
    std::os::unix::fs::symlink(project.path(), &link).unwrap();
    let (mut child, mut stdin, stdout) = spawn_server_source(&link, &[]);
    initialize(&mut stdin, &stdout);
    assert_ne!(
        hit_ids(&search(&mut stdin, &stdout, 2, "oldword")),
        [] as [String; 0]
    );

    std::fs::write(link.join("notes/alpha.md"), "# Alpha\n\nzebrafish here\n").unwrap();
    let resp = search(&mut stdin, &stdout, 3, "zebrafish");
    assert!(!hit_ids(&resp).is_empty(), "edit not seen: {resp}");
    assert_eq!(stale_block(&resp), None, "a recompile loop would show here");

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn concurrent_calls_after_edit_both_fresh() {
    let project = source_project();
    let (mut child, mut stdin, stdout) = spawn_server_source(project.path(), &[]);
    initialize(&mut stdin, &stdout);
    assert_ne!(
        hit_ids(&search(&mut stdin, &stdout, 2, "oldword")),
        [] as [String; 0]
    );

    std::fs::write(
        project.path().join("notes/alpha.md"),
        "# Alpha\n\nzebrafish here\n",
    )
    .unwrap();
    send_search(&mut stdin, 3, "zebrafish");
    send_search(&mut stdin, 4, "zebrafish");
    // The calls run concurrently, so their responses come in either order;
    // `read_response` would drop the other one while looking for its own.
    let mut responses = std::collections::BTreeMap::new();
    while responses.len() < 2 {
        let line = stdout
            .recv_timeout(RESPONSE_DEADLINE)
            .expect("both calls must answer");
        if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
            if let Some(id) = v["id"].as_u64() {
                responses.insert(id, v);
            }
        }
    }
    for (id, resp) in responses {
        assert!(!hit_ids(&resp).is_empty(), "call {id} not fresh: {resp}");
        assert_eq!(stale_block(&resp), None, "call {id}: {resp}");
    }

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn serve_source_fails_when_first_compile_fails_and_no_wiki() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("not_a_dir.txt");
    std::fs::write(&file, "x").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_wiki"))
        .args(["serve", "--source"])
        .arg(&file)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not_a_dir.txt"), "stderr: {stderr}");
}

#[test]
fn mcp_neighbors_unknown_id_names_candidates() {
    let src = source_project();
    let out = src.path().join("out");
    wiki::compile(src.path(), &out, &wiki::CompileOptions::default()).unwrap();
    let id = wiki::query::Wiki::load(&out)
        .unwrap()
        .search("add", None, 1)
        .remove(0)
        .id;
    let (mut child, mut stdin, stdout) = spawn_server(&out);
    initialize(&mut stdin, &stdout);
    let resp = call(
        &mut stdin,
        &stdout,
        9,
        "neighbors",
        &json!({"id": "lib.rs nonsense"}),
    );
    child.kill().ok();
    assert_eq!(resp["result"]["isError"], true);
    let text = text_of(&resp);
    assert!(text.contains(&id), "no candidate {id}: {text}");
}

/// Call tool `page` and return `(isError, text)`.
fn page(
    stdin: &mut ChildStdin,
    stdout: &Receiver<String>,
    id: u64,
    args: &Value,
) -> (bool, String) {
    let resp = call(stdin, stdout, id, "page", args);
    (
        resp["result"]["isError"] == json!(true),
        text_of(&resp).to_string(),
    )
}

/// Compiled project whose `src/big.rs` has 600 documented fns; returns the
/// project dir, the output dir and the big page's id.
fn big_project() -> (tempfile::TempDir, std::path::PathBuf, String) {
    let src = source_project();
    let mut body = String::new();
    for i in 0..600 {
        body += &format!("/// Does thing number {i}.\npub fn thing_{i}() {{}}\n");
    }
    std::fs::write(src.path().join("src/big.rs"), body).unwrap();
    let out = src.path().join("out");
    wiki::compile(src.path(), &out, &wiki::CompileOptions::default()).unwrap();
    let id = wiki::query::Wiki::load(&out)
        .unwrap()
        .resolve_id("src/big.rs")
        .unwrap();
    (src, out, id)
}

#[test]
fn mcp_page_defaults_to_a_budgeted_outline() {
    let (_src, out, id) = big_project();
    let (mut child, mut stdin, stdout) = spawn_server(&out);
    initialize(&mut stdin, &stdout);
    let (err, text) = page(&mut stdin, &stdout, 2, &json!({"id": id}));
    assert!(!err);
    assert!(text.chars().count() / 4 <= 4000, "{} chars", text.len());
    assert!(text.lines().last().unwrap().starts_with("… "), "{text}");
    let all = page(
        &mut stdin,
        &stdout,
        3,
        &json!({"id": id, "max_tokens": 100_000}),
    )
    .1;
    child.kill().ok();
    assert!(!all.contains("… "), "marker in unbudgeted outline");
    assert!(all.contains("thing_599"));
}

#[test]
fn mcp_page_resolves_a_path_and_reports_unknown() {
    let (_src, out, _id) = big_project();
    let (mut child, mut stdin, stdout) = spawn_server(&out);
    initialize(&mut stdin, &stdout);
    let (err, text) = page(&mut stdin, &stdout, 2, &json!({"id": "./src/lib.rs"}));
    assert!(!err, "{text}");
    assert!(text.contains("add"), "{text}");
    let (err, text) = page(&mut stdin, &stdout, 3, &json!({"id": "nope"}));
    child.kill().ok();
    assert!(err);
    assert!(text.contains("unknown page"), "{text}");
}

#[test]
fn mcp_page_full_matches_resource() {
    let (_src, out, id) = big_project();
    let (mut child, mut stdin, stdout) = spawn_server(&out);
    initialize(&mut stdin, &stdout);
    let (err, full) = page(&mut stdin, &stdout, 2, &json!({"id": id, "full": true}));
    assert!(!err);
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 3, "method": "resources/read",
            "params": {"uri": format!("wiki://page/{id}")}}),
    );
    let resp = read_response(&stdout, 3);
    child.kill().ok();
    assert_eq!(
        resp["result"]["contents"][0]["text"].as_str().unwrap(),
        full
    );
}

#[test]
fn serve_source_sees_first_repo_memory_note_and_its_removal() {
    let project = source_project();
    let (mut child, mut stdin, stdout) = spawn_server_source(project.path(), &["--memory", "none"]);
    initialize(&mut stdin, &stdout);
    assert_eq!(
        hit_ids(&search(&mut stdin, &stdout, 2, "wombatnote")),
        [] as [String; 0]
    );

    let mem = project.path().join(".agents/memory");
    std::fs::create_dir_all(&mem).unwrap();
    std::fs::write(mem.join("first.md"), "# First\n\nwombatnote\n").unwrap();
    let ids = hit_ids(&search(&mut stdin, &stdout, 3, "wombatnote"));
    assert!(ids.iter().any(|id| id.contains("first")), "ids: {ids:?}");

    std::fs::remove_dir_all(project.path().join(".agents")).unwrap();
    let resp = search(&mut stdin, &stdout, 4, "wombatnote");
    assert_eq!(hit_ids(&resp), [] as [String; 0]);
    assert!(stale_block(&resp).is_none(), "{resp}");

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn serve_source_watches_an_external_memory_folder() {
    let project = source_project();
    let ext = tempfile::tempdir().unwrap();
    let mem = ext.path().join("mem");
    std::fs::create_dir_all(&mem).unwrap();
    std::fs::write(mem.join("a.md"), "# Ext A\n\nplatypusone\n").unwrap();
    let (mut child, mut stdin, stdout) =
        spawn_server_source(project.path(), &["--memory", mem.to_str().unwrap()]);
    initialize(&mut stdin, &stdout);
    assert_ne!(
        hit_ids(&search(&mut stdin, &stdout, 2, "platypusone")),
        [] as [String; 0]
    );

    std::fs::write(mem.join("b.md"), "# Ext B\n\nplatypustwo\n").unwrap();
    assert_ne!(
        hit_ids(&search(&mut stdin, &stdout, 3, "platypustwo")),
        [] as [String; 0]
    );

    drop(stdin);
    let _ = child.wait();
}
