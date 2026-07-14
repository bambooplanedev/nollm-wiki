//! End-to-end test: spawn `wiki serve` and speak raw newline-delimited
//! JSON-RPC (the MCP stdio transport) over its stdin/stdout.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

fn compile_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path().join("raw");
    let out = tmp.path().join("out");
    wiki::generator::generate_corpus(&raw, 12, 42).unwrap();
    wiki::compile(&raw, &out, &wiki::CompileOptions::default()).unwrap();
    (tmp, out)
}

fn spawn_server(dir: &std::path::Path) -> (Child, ChildStdin, BufReader<ChildStdout>) {
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
    (child, stdin, stdout)
}

fn send(stdin: &mut ChildStdin, msg: Value) {
    let mut line = msg.to_string();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
}

/// Read lines until the response with the given id arrives (skips
/// notifications and unrelated messages).
fn read_response(stdout: &mut BufReader<ChildStdout>, id: u64) -> Value {
    loop {
        let mut line = String::new();
        let n = stdout.read_line(&mut line).unwrap();
        assert!(n > 0, "server closed stdout before responding to id {id}");
        let v: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v["id"] == json!(id) {
            return v;
        }
    }
}

fn initialize(stdin: &mut ChildStdin, stdout: &mut BufReader<ChildStdout>) {
    send(
        stdin,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "test-client", "version": "0.0.0"}
        }}),
    );
    let resp = read_response(stdout, 1);
    assert_eq!(resp["result"]["serverInfo"]["name"], "wiki");
    send(
        stdin,
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    );
}

#[test]
fn serve_lists_and_calls_tools() {
    let (_tmp, out) = compile_fixture();
    let (mut child, mut stdin, mut stdout) = spawn_server(&out);
    initialize(&mut stdin, &mut stdout);

    // tools/list
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    );
    let resp = read_response(&mut stdout, 2);
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
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
            "name": "search", "arguments": {"query": "gradient"}
        }}),
    );
    let resp = read_response(&mut stdout, 3);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("gradient_descent"), "search result: {text}");

    // tools/call neighbors
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {
            "name": "neighbors", "arguments": {"id": "gradient_descent", "max_tokens": 800}
        }}),
    );
    let resp = read_response(&mut stdout, 4);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("# Gradient Descent"), "pack: {text}");

    // tools/call neighbors with unknown id -> tool error result
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {
            "name": "neighbors", "arguments": {"id": "no_such_page"}
        }}),
    );
    let resp = read_response(&mut stdout, 5);
    assert_eq!(resp["result"]["isError"], json!(true));

    // tools/call lint
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {
            "name": "lint", "arguments": {}
        }}),
    );
    let resp = read_response(&mut stdout, 6);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let report: Value = serde_json::from_str(text).unwrap();
    assert_eq!(report["broken_links"].as_array().unwrap().len(), 0);

    drop(stdin); // close stdin -> server exits
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
