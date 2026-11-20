//! Tests for `serve`.
use super::*;
use crate::query::Hit;
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
    state.snapshot().0.list_pages().len()
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
    let json: serde_json::Value = serde_json::from_str(&search_json(vec![repo(false)])).unwrap();
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
    // The missing index.json is reported as missing (its text names the file and the compile command).
    let e = WikiState::load(tmp.path()).err().unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "{e}");
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
    let t = Instant::now();
    let (wiki, reason) = state.snapshot();
    assert!(
        t.elapsed() < crate::live::WAIT + Duration::from_secs(1),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(wiki.list_pages().len(), 3);
    assert_eq!(reason.as_deref(), Some(crate::live::RUNNING));
    release.send(()).unwrap();
    cli.join().unwrap();
    let (wiki, reason) = state.snapshot();
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
    let t = Instant::now();
    let r = server.lint().unwrap();
    // One wait, not two.
    assert!(
        t.elapsed() < crate::live::WAIT + Duration::from_secs(1),
        "{:?}",
        t.elapsed()
    );
    release.send(()).unwrap();
    cli.join().unwrap();
    let texts: Vec<&str> = r["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["text"].as_str())
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

/// With two workers and a compile holding the lock, two calls share one
/// wait, the third starts only when a worker frees, and a ping answers at
/// once.
#[test]
fn calls_beyond_the_workers_queue_and_ping_does_not() {
    let (tmp, out) = fixture(3);
    let state = WikiState::load(&out).unwrap();
    let server = WikiServer::new(state, out.clone(), &["search"]);
    let (release, cli) = hold(&tmp, &out);
    let call = |id: u64| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": "search", "arguments": {"query": "x"}}})
        .to_string()
    };
    let ping = r#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#.to_string();
    let input = [call(1), call(2), call(3), ping].join("\n");
    let t = Instant::now();
    let replies = Mutex::new(Vec::new());
    let read = serve(&server, input.as_bytes(), 2, &|r: Value| {
        replies
            .lock()
            .unwrap()
            .push((r["id"].as_u64().unwrap(), t.elapsed()));
    })
    .unwrap();
    let _ = release.send(());
    cli.join().unwrap();
    assert!(read);
    let replies = replies.into_inner().unwrap();
    let at = |id| replies.iter().find(|r| r.0 == id).unwrap().1;
    assert!(at(4) < Duration::from_secs(1), "{replies:?}");
    let first = at(1).max(at(2));
    assert!(
        first < crate::live::WAIT + Duration::from_secs(1),
        "{replies:?}"
    );
    assert!(at(3) > first + Duration::from_secs(1), "{replies:?}");
}

/// A sample value of a schema property's type.
fn sample(schema: &Value) -> Value {
    let ty = match &schema["type"] {
        Value::Array(types) => types[0].clone(),
        ty => ty.clone(),
    };
    match ty.as_str() {
        Some("string") => "x".into(),
        Some("integer") => 1.into(),
        Some("boolean") => true.into(),
        Some("array") => serde_json::json!(["x"]),
        other => panic!("no sample for type {other:?}"),
    }
}

/// `tool`'s schema in `tools.json` and the struct `T` its arguments
/// deserialize into name the same fields, and the required ones are
/// exactly those without a default.
fn schema_matches<T: serde::de::DeserializeOwned + serde::Serialize>(tool: &str) {
    let tools: Vec<Value> = serde_json::from_str(TOOLS_JSON).unwrap();
    let schema = &tools.iter().find(|t| t["name"] == tool).unwrap()["inputSchema"];
    let props = schema["properties"].as_object().unwrap();
    let full: serde_json::Map<String, Value> =
        props.iter().map(|(k, v)| (k.clone(), sample(v))).collect();
    let parsed: T = serde_json::from_value(Value::Object(full.clone())).unwrap();
    let back = serde_json::to_value(parsed).unwrap();
    let mut fields: Vec<&String> = back.as_object().unwrap().keys().collect();
    let mut names: Vec<&String> = props.keys().collect();
    fields.sort();
    names.sort();
    assert_eq!(fields, names, "{tool}: struct fields vs schema properties");
    let required = schema["required"].as_array().cloned().unwrap_or_default();
    for name in props.keys() {
        let mut without = full.clone();
        without.remove(name);
        assert_eq!(
            serde_json::from_value::<T>(Value::Object(without)).is_err(),
            required.contains(&name.as_str().into()),
            "{tool}.{name}: required in the schema iff the struct needs it"
        );
    }
}

#[test]
fn tool_schemas_match_the_params_structs() {
    schema_matches::<SearchParams>("search");
    schema_matches::<NeighborsParams>("neighbors");
    schema_matches::<CallersParams>("callers");
    schema_matches::<PageParams>("page");
    let tools: Vec<Value> = serde_json::from_str(TOOLS_JSON).unwrap();
    let lint = tools.iter().find(|t| t["name"] == "lint").unwrap();
    assert_eq!(lint["inputSchema"]["properties"], serde_json::json!({}));
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    let mut all = TOOLS.to_vec();
    all.sort_unstable();
    assert_eq!(names, all);
}
