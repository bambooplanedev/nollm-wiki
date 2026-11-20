//! Tests for `edit`.
use super::*;
use serde_json::json;
use std::fs;

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap() + "\n"
}

fn is_foreign(o: &Outcome) -> bool {
    matches!(o, Outcome::Foreign(_))
}

#[test]
fn upsert_creates_file_and_parents() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join(".mcp.json");
    let v = json!({"command": "wiki", "args": ["serve"]});
    assert_eq!(
        upsert_entry(&p, &["mcpServers"], "wiki", std::slice::from_ref(&v)).unwrap(),
        Outcome::Done
    );
    let want = pretty(&json!({"mcpServers": {"wiki": v}}));
    assert_eq!(fs::read_to_string(&p).unwrap(), want);
    assert_eq!(
        upsert_entry(&p, &["mcpServers"], "wiki", std::slice::from_ref(&v)).unwrap(),
        Outcome::AlreadyThere
    );
    assert_eq!(fs::read_to_string(&p).unwrap(), want);
}

#[test]
fn upsert_keeps_other_keys_and_order() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.json");
    let orig = r#"{"zeta":1,"mcpServers":{"other":{"command":"x"}},"alpha":2}"#;
    fs::write(&p, orig).unwrap();
    upsert_entry(&p, &["mcpServers"], "wiki", &[json!({"command": "wiki"})]).unwrap();
    let v: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(keys(&v), ["zeta", "mcpServers", "alpha"]);
    assert_eq!(keys(&v["mcpServers"]), ["other", "wiki"]);
}

#[test]
fn upsert_reports_foreign_value() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.json");
    let orig = pretty(&json!({"mcpServers": {"wiki": {"command": "other"}}}));
    fs::write(&p, &orig).unwrap();
    let o = upsert_entry(&p, &["mcpServers"], "wiki", &[json!({"command": "wiki"})]).unwrap();
    assert!(is_foreign(&o));
    assert_eq!(fs::read_to_string(&p).unwrap(), orig);
}

#[test]
fn upsert_refuses_malformed_json() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.json");
    fs::write(&p, "{ not json").unwrap();
    let o = upsert_entry(&p, &["mcpServers"], "wiki", &[json!(1)]).unwrap();
    assert!(is_foreign(&o));
    assert_eq!(fs::read_to_string(&p).unwrap(), "{ not json");
}

#[test]
fn remove_deletes_a_file_left_empty() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join(".pi/a.json");
    let v = json!({"command": "wiki"});
    upsert_entry(&p, &["mcpServers"], "wiki", std::slice::from_ref(&v)).unwrap();
    assert_eq!(
        remove_entry(
            d.path(),
            &p,
            &["mcpServers"],
            "wiki",
            std::slice::from_ref(&v)
        )
        .unwrap(),
        Outcome::Done
    );
    assert!(!p.exists());
    assert!(!d.path().join(".pi").exists());
    assert_eq!(
        remove_entry(
            d.path(),
            &p,
            &["mcpServers"],
            "wiki",
            std::slice::from_ref(&v)
        )
        .unwrap(),
        Outcome::AlreadyThere
    );
}

#[test]
fn remove_restores_a_pretty_file_byte_for_byte() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.json");
    let orig = pretty(&json!({"b": 1, "mcpServers": {"other": {"command": "x"}}, "a": [1]}));
    fs::write(&p, &orig).unwrap();
    let v = json!({"command": "wiki"});
    upsert_entry(&p, &["mcpServers"], "wiki", std::slice::from_ref(&v)).unwrap();
    remove_entry(
        d.path(),
        &p,
        &["mcpServers"],
        "wiki",
        std::slice::from_ref(&v),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&p).unwrap(), orig);
}

#[test]
fn append_and_remove_item_keep_other_items() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("s.json");
    let (a, b) = (json!({"hooks": "A"}), json!({"hooks": "B"}));
    let path = ["hooks", "UserPromptSubmit"];
    fs::write(&p, pretty(&json!({"hooks": {"UserPromptSubmit": [a]}}))).unwrap();
    assert_eq!(
        append_item(&p, &path, std::slice::from_ref(&b)).unwrap(),
        Outcome::Done
    );
    let read = || serde_json::from_str::<Value>(&fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(read()["hooks"]["UserPromptSubmit"], json!([a, b]));
    assert_eq!(
        append_item(&p, &path, std::slice::from_ref(&b)).unwrap(),
        Outcome::AlreadyThere
    );
    assert_eq!(
        remove_item(d.path(), &p, &path, std::slice::from_ref(&b)).unwrap(),
        Outcome::Done
    );
    assert_eq!(read()["hooks"]["UserPromptSubmit"], json!([a]));
    assert_eq!(
        remove_item(d.path(), &p, &path, std::slice::from_ref(&a)).unwrap(),
        Outcome::Done
    );
    assert!(!p.exists());
}

#[test]
fn another_own_item_is_replaced_in_its_place_and_removed() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("s.json");
    let (old, new, other) = (json!("old"), json!("new"), json!("other"));
    let path = ["hooks", "UserPromptSubmit"];
    let own = [new.clone(), old.clone()];
    fs::write(
        &p,
        pretty(&json!({"hooks": {"UserPromptSubmit": [old, other]}})),
    )
    .unwrap();
    assert_eq!(append_item(&p, &path, &own).unwrap(), Outcome::Done);
    let read = || serde_json::from_str::<Value>(&fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(read()["hooks"]["UserPromptSubmit"], json!([new, other]));
    fs::write(
        &p,
        pretty(&json!({"hooks": {"UserPromptSubmit": [other, old]}})),
    )
    .unwrap();
    assert_eq!(
        remove_item(d.path(), &p, &path, &own).unwrap(),
        Outcome::Done
    );
    assert_eq!(read()["hooks"]["UserPromptSubmit"], json!([other]));
}

#[test]
fn write_and_remove_file_prune_empty_dirs() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let p = root.join(".claude/skills/wiki-navigation/SKILL.md");
    fs::create_dir_all(root.join(".claude")).unwrap();
    fs::write(root.join(".claude/settings.json"), "{}").unwrap();
    assert_eq!(write_file(&p, &["body"]).unwrap(), Outcome::Done);
    assert_eq!(write_file(&p, &["body"]).unwrap(), Outcome::AlreadyThere);
    assert!(is_foreign(&write_file(&p, &["other"]).unwrap()));
    assert!(is_foreign(&remove_file(root, &p, &["other"]).unwrap()));
    assert_eq!(remove_file(root, &p, &["body"]).unwrap(), Outcome::Done);
    assert!(!root.join(".claude/skills").exists());
    assert!(root.join(".claude").exists());
    assert_eq!(
        remove_file(root, &p, &["body"]).unwrap(),
        Outcome::AlreadyThere
    );

    fs::remove_file(root.join(".claude/settings.json")).unwrap();
    write_file(&p, &["body"]).unwrap();
    remove_file(root, &p, &["body"]).unwrap();
    assert!(!root.join(".claude").exists());
    assert!(root.exists());
}

#[test]
fn a_block_goes_after_other_text_and_comes_out_byte_for_byte() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("AGENTS.md");
    let (begin, block) = ("<!-- b -->", "<!-- b -->\nrules\n<!-- /b -->\n");
    fs::write(&p, "# Mine\n").unwrap();
    assert_eq!(upsert_block(&p, begin, block).unwrap(), Outcome::Done);
    assert_eq!(
        fs::read_to_string(&p).unwrap(),
        format!("# Mine\n\n{block}")
    );
    assert_eq!(
        upsert_block(&p, begin, block).unwrap(),
        Outcome::AlreadyThere
    );
    let other = "<!-- b -->\nold\n<!-- /b -->\n";
    assert!(is_foreign(&upsert_block(&p, begin, other).unwrap()));
    assert_eq!(remove_block(d.path(), &p, block).unwrap(), Outcome::Done);
    assert_eq!(fs::read_to_string(&p).unwrap(), "# Mine\n");
    assert_eq!(
        remove_block(d.path(), &p, block).unwrap(),
        Outcome::AlreadyThere
    );

    let q = d.path().join("new/AGENTS.md");
    upsert_block(&q, begin, block).unwrap();
    assert_eq!(fs::read_to_string(&q).unwrap(), block);
    remove_block(d.path(), &q, block).unwrap();
    assert!(!d.path().join("new").exists());
}

#[test]
fn ensure_line_appends_once() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join(".gitignore");
    fs::write(&p, "target/").unwrap();
    assert_eq!(ensure_line(&p, ".nollm-wiki/").unwrap(), Outcome::Done);
    assert_eq!(fs::read_to_string(&p).unwrap(), "target/\n.nollm-wiki/\n");
    assert_eq!(
        ensure_line(&p, ".nollm-wiki/").unwrap(),
        Outcome::AlreadyThere
    );
}
