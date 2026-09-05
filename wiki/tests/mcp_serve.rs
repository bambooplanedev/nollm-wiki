//! End-to-end test: spawn `wiki serve` and speak raw newline-delimited
//! JSON-RPC (the MCP stdio transport) over its stdin/stdout.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// How long a single response may take before the test fails instead of
/// hanging the whole `cargo test` run on a server that never answers.
const RESPONSE_DEADLINE: Duration = Duration::from_secs(10);

fn compile_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path().join("raw");
    let out = tmp.path().join("out");
    wiki::generator::generate_corpus(&raw, 12, 42).unwrap();
    wiki::compile(&raw, &out, &wiki::CompileOptions::default()).unwrap();
    (tmp, out)
}

/// Spawn `wiki serve` and hand back its stdin plus a channel of stdout
/// lines. A pipe has no read timeout, so a thread does the blocking read
/// and `read_response` waits on the channel with a deadline. The channel
/// closes when the server closes stdout.
fn spawn_server(dir: &std::path::Path) -> (Child, ChildStdin, Receiver<String>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wiki"))
        .args(["serve", "--dir"])
        .arg(dir)
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
    assert!(names.contains(&"search"), "tools: {names:?}");
    assert!(names.contains(&"neighbors"), "tools: {names:?}");
    assert!(names.contains(&"lint"), "tools: {names:?}");

    // tools/call search — seed-42 corpus contains gradient_descent
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
            "name": "search", "arguments": {"query": "gradient"}
        }}),
    );
    let resp = read_response(&stdout, 3);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("gradient_descent"), "search result: {text}");

    // tools/call neighbors
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {
            "name": "neighbors", "arguments": {"id": "gradient_descent", "max_tokens": 800}
        }}),
    );
    let resp = read_response(&stdout, 4);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("# Gradient Descent"), "pack: {text}");

    // tools/call neighbors with unknown id -> tool error result
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {
            "name": "neighbors", "arguments": {"id": "no_such_page"}
        }}),
    );
    let resp = read_response(&stdout, 5);
    assert_eq!(resp["result"]["isError"], json!(true));

    // tools/call lint
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {
            "name": "lint", "arguments": {}
        }}),
    );
    let resp = read_response(&stdout, 6);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let report: Value = serde_json::from_str(text).unwrap();
    assert_eq!(report["broken_links"].as_array().unwrap().len(), 0);

    drop(stdin); // close stdin -> server exits
    let _ = child.wait();
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
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {
            "name": "search", "arguments": {"query": "gradient"}}}),
    );
    let resp = read_response(&stdout, 7);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let hits: Value = serde_json::from_str(text).unwrap();
    let arr = hits.as_array().expect("hits array");
    assert!(!arr.is_empty());
    for h in arr {
        assert!(h.get("snippet").is_some(), "hit missing snippet key: {h}");
    }
    child.kill().ok();
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
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "search", "arguments": {"kind": "bogus", "query": "x"}
        }}),
    );
    let resp = read_response(&stdout, 2);
    assert_eq!(resp["error"]["code"], json!(-32602), "got: {resp}");
    let msg = resp["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("expected text, markdown, or code:<lang>"),
        "message: {msg}"
    );
    assert!(resp.get("result").is_none(), "got: {resp}");

    // (b) A missing required field fails parameter deserialization, which
    // rmcp reports as a tool error result naming the field.
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
            "name": "search", "arguments": {}
        }}),
    );
    let resp = read_response(&stdout, 3);
    assert_eq!(resp["result"]["isError"], json!(true), "got: {resp}");
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("query"), "text: {text}");

    // (c) Unknown tool name -> JSON-RPC invalid params.
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {
            "name": "nope", "arguments": {}
        }}),
    );
    let resp = read_response(&stdout, 4);
    assert_eq!(resp["error"]["code"], json!(-32602), "got: {resp}");

    // (d) A budget the target's own page cannot fit degrades the target
    // block instead of failing.
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {
            "name": "neighbors", "arguments": {"id": "gradient_descent", "max_tokens": 1}
        }}),
    );
    let resp = read_response(&stdout, 5);
    assert_eq!(resp["result"]["isError"], json!(false), "got: {resp}");
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
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
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "neighbors", "arguments": {"id": "huge"}
        }}),
    );
    let resp = read_response(&stdout, 2);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
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
