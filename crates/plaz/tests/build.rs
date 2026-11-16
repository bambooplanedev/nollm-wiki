mod common;

use std::path::Path;

const A: &str = "use std::collections::BTreeMap;\nuse crate::b::Store;\n\npub fn run(x: u8, cb: impl Fn(u8), m: BTreeMap<u8, u8>, s: &Store) {\n    helper(x);\n    cb(x);\n    m.get(&x);\n    s.get(x);\n    BTreeMap::<u8, u8>::new();\n    std::mem::drop(x);\n    other_crate::go();\n    let t = make();\n    t.get(x);\n}\nfn helper(_x: u8) {}\nfn make() -> crate::b::Store { todo!() }\n";
const B: &str = "pub struct Store;\nimpl Store {\n    pub fn get(&self, _k: u8) {}\n}\npub struct Cache;\nimpl Cache {\n    pub fn get(&self, _k: u8) {}\n}\n";
const LIB: &str = "mod a;\nmod b;\n";

fn headings(page: &str) -> Vec<&str> {
    page.lines().filter_map(|l| l.strip_prefix("## ")).collect()
}

#[test]
fn rust_calls_take_the_four_forms() {
    let (_d, vault) = common::built(&[("src/lib.rs", LIB), ("src/a.rs", A), ("src/b.rs", B)]);
    assert_eq!(
        common::fn_section(&common::map_page(&vault, "map/src/a_rs"), "run", "Calls"),
        [
            "`helper(x)` L5 → [[map/src/a_rs#helper|helper]]",
            "`cb(x)` L6 → target not visible",
            "`m.get(&x)` L7 → outside the project",
            "`s.get(x)` L8 → [[map/src/b_rs#store_get|Store::get]]",
            "`BTreeMap::<u8, u8>::new()` L9 → outside the project",
            "`std::mem::drop(x)` L10 → outside the project",
            "`other_crate::go()` L11 → target not visible",
            "`make()` L12 → [[map/src/a_rs#make|make]]",
            "`t.get(x)` L13 → one of: [[map/src/b_rs#store_get|Store::get]], [[map/src/b_rs#cache_get|Cache::get]], or outside the project",
        ]
    );
}

#[test]
fn called_from_splits_certain_and_may() {
    let (_d, vault) = common::built(&[("src/lib.rs", LIB), ("src/a.rs", A), ("src/b.rs", B)]);
    assert_eq!(
        common::fn_section(
            &common::map_page(&vault, "map/src/b_rs"),
            "store_get",
            "Called from"
        ),
        [
            "certainly: [[map/src/a_rs#run|run]] L8",
            "may call (if this `get` is this one): [[map/src/a_rs#run|run]] L13",
        ]
    );
    assert_eq!(
        common::fn_section(
            &common::map_page(&vault, "map/src/a_rs"),
            "run",
            "Called from"
        ),
        ["none found"]
    );
}

#[test]
fn a_closure_calling_an_outer_parameter_is_hidden_even_when_a_project_function_shares_the_name() {
    let src = "pub fn cb() {}\npub fn f(cb: impl Fn()) {\n    let run = || {\n        cb();\n    };\n    run();\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let calls = common::fn_section(&common::map_page(&vault, "map/src/a_rs"), "f", "Calls");
    assert!(calls.contains(&"`cb()` L4 → target not visible".to_string()));
}

fn cli_build(src: &Path, vault: &Path) -> assert_cmd::assert::Assert {
    assert_cmd::cargo_bin_cmd!()
        .args(["build", "--source"])
        .arg(src)
        .arg("--vault")
        .arg(vault)
        .assert()
}

#[test]
fn code_references_are_wikilinks_only_for_paths_the_vault_lists() {
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    common::write_files(
        &src,
        &[("src/lib.rs", LIB), ("src/a.rs", A), ("src/b.rs", B)],
    );
    common::write_files(
        &vault,
        &[
            (
                "index.json",
                r#"{"entries":[{"id":"a","path":"src/a.rs"}]}"#,
            ),
            ("a.md", "a wiki page\n"),
        ],
    );
    plaz::build(&src, &vault).unwrap();
    assert!(common::map_page(&vault, "map/src/a_rs").contains("\nCode: [[a|src/a.rs:1-16]]\n"));
    assert!(common::map_page(&vault, "map/src/b_rs").contains("\nCode: `src/b.rs:1-8`\n"));

    for bad in ["not json", r#"{"entries": 3}"#] {
        common::write_files(&vault, &[("index.json", bad)]);
        assert!(plaz::build(&src, &vault).is_ok());
        assert!(common::map_page(&vault, "map/src/a_rs").contains("\nCode: `src/a.rs:1-16`\n"));
        assert!(common::map_page(&vault, "map/src/b_rs").contains("\nCode: `src/b.rs:1-8`\n"));
        let out = cli_build(&src, &vault).success();
        let stderr = String::from_utf8(out.get_output().stderr.clone()).unwrap();
        assert_eq!(
            stderr
                .lines()
                .filter(|l| l.contains("not a wiki index"))
                .count(),
            1,
            "{stderr}"
        );
    }
}

#[test]
fn a_second_build_removes_only_its_own_stale_pages() {
    let (d, vault) = common::built(&[
        ("src/lib.rs", LIB),
        ("src/a.rs", A),
        ("src/b.rs", B),
        ("src/sub/c.rs", "fn c() {}\n"),
    ]);
    let src = d.path().join("src-root");
    assert!(vault.join("map/src/sub/c_rs.md").exists());
    let keep = [
        ("a.md", "---\ntitle: a wiki page\n---\nhello\n"),
        ("map/notes.md", "my notes\n"),
        ("map/src/mine.txt", "mine\n"),
    ];
    common::write_files(&vault, &keep);
    std::fs::remove_file(src.join("src/b.rs")).unwrap();
    std::fs::remove_file(src.join("src/sub/c.rs")).unwrap();
    common::write_files(&src, &[("src/a.rs", &A.replace("helper", "helper2"))]);

    let second = plaz::build(&src, &vault).unwrap();
    assert_eq!(second.removed, 2);
    assert!(!vault.join("map/src/b_rs.md").exists());
    assert!(
        !vault.join("map/src/sub").exists(),
        "an emptied directory is removed"
    );
    let a = common::map_page(&vault, "map/src/a_rs");
    assert!(a.contains("## helper2\n") && !a.contains("## helper\n"));
    for (path, text) in keep {
        assert_eq!(std::fs::read_to_string(vault.join(path)).unwrap(), text);
    }

    let mtime = || {
        std::fs::metadata(vault.join("map/src/a_rs.md"))
            .unwrap()
            .modified()
            .unwrap()
    };
    let before = mtime();
    let third = plaz::build(&src, &vault).unwrap();
    assert_eq!((third.written, third.removed), (0, 0));
    assert_eq!(mtime(), before);
}

#[test]
fn page_names_and_anchors_are_unique_and_stable() {
    // The `src/A.rs` / `src/a.rs` pair is `names::tests`: it needs a
    // case-sensitive disk here.
    let two = "fn a() {} fn a2() {}\n";
    let mods = "mod m { pub fn f() {} }\nmod n { pub fn f() {} }\n";
    let (_d, vault) = common::built(&[("x/a.b.rs", two), ("x/a_b.rs", mods)]);
    let (first, second) = (
        common::map_page(&vault, "map/x/a_b_rs"),
        common::map_page(&vault, "map/x/a_b_rs_2"),
    );
    assert!(first.contains("\nsource: \"x/a.b.rs\"\n"));
    assert!(second.contains("\nsource: \"x/a_b.rs\"\n"));
    assert_eq!(headings(&first), ["a", "a2"]);
    assert_eq!(headings(&second), ["f", "f_2"]);

    let (_d, moved) = common::built(&[
        ("x/a.b.rs", &format!("\n\n{two}")),
        ("x/a_b.rs", &format!("\n\n{mods}")),
    ]);
    assert_eq!(
        headings(&common::map_page(&moved, "map/x/a_b_rs")),
        ["a", "a2"]
    );
    assert_eq!(
        headings(&common::map_page(&moved, "map/x/a_b_rs_2")),
        ["f", "f_2"]
    );
}

/// The text the one code span `s` shows.
fn code_span_content(s: &str) -> &str {
    let ticks = s.len() - s.trim_start_matches('`').len();
    assert!(ticks > 0 && s.ends_with(&"`".repeat(ticks)), "{s}");
    let inner = &s[ticks..s.len() - ticks];
    match inner.strip_prefix(' ').and_then(|i| i.strip_suffix(' ')) {
        Some(i) if !inner.trim().is_empty() => i,
        _ => inner,
    }
}

#[test]
fn hostile_text_breaks_no_link_code_span_or_front_matter() {
    let src = "use std::fmt::Display;\ntrait Foo { fn m(&self); }\nfn helper() {}\nfn h(m: X) {\n    m.get(\"a`b\");\n}\nimpl<T> Display for T {\n    fn fmt(&self) { helper(); }\n}\nimpl Foo for Vec<[u8; 4]> {\n    fn m(&self) { helper(); }\n}\n";
    let path = "src/it's \"x\".rs";
    let d = tempfile::tempdir().unwrap();
    let (root, vault) = (d.path().join("src-root"), d.path().join("vault"));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join(path), src)
        .unwrap_or_else(|e| panic!("needs a file system that accepts quotes in file names: {e}"));
    plaz::build(&root, &vault).unwrap();
    let page = common::map_page(&vault, "map/src/it_s__x__rs");

    // Front matter, by exact string: these are the YAML double-quoted
    // scalars of `it's "x".rs · src` and `src/it's "x".rs`.
    assert!(page.starts_with(
        "---\ntitle: \"it's \\\"x\\\".rs · src\"\ntags: [\"plaz\"]\nkind: map\nsource: \"src/it's \\\"x\\\".rs\"\n---\n"
    ));
    assert!(page.contains("\nCode: `src/it's \"x\".rs:1-12`\n"));

    let calls = common::fn_section(&page, "h", "Calls");
    let span = calls[0].strip_suffix(" L5 → outside the project").unwrap();
    assert_eq!(span, "``m.get(\"a`b\")``");
    assert_eq!(code_span_content(span), "m.get(\"a`b\")");

    let callers = common::fn_section(&page, "helper", "Called from");
    assert_eq!(callers.len(), 2, "{callers:?}");
    for (line, anchor, name) in [
        (&callers[0], "t_as_display_fmt", "T as Display"),
        (&callers[1], "vec_u8_4_as_foo_m", "u8; 4"),
    ] {
        let link = line
            .strip_prefix("certainly: [[")
            .and_then(|l| l.rsplit_once("]] L"))
            .unwrap_or_else(|| panic!("{line}"))
            .0;
        let (target, text) = link.split_once('|').unwrap();
        assert_eq!(target, format!("map/src/it_s__x__rs#{anchor}"));
        assert!(text.contains(name), "{text}");
        assert!(!text.contains(['|', '[', ']', '\n']), "{text}");
        assert!(headings(&page).contains(&anchor));
    }
}

#[test]
fn the_cli_reports_counts_and_fails_cleanly() {
    let d = tempfile::tempdir().unwrap();
    common::write_files(
        d.path(),
        &[("src/lib.rs", LIB), ("src/a.rs", A), ("src/b.rs", B)],
    );
    assert_cmd::cargo_bin_cmd!()
        .args(["build", "--source"])
        .arg(d.path())
        .assert()
        .success()
        .stdout("map: 3 written, 0 unchanged, 0 removed\n")
        .stderr("");
    assert!(d.path().join(".kulman/map/src/a_rs.md").exists());

    let out = assert_cmd::cargo_bin_cmd!()
        .args(["build", "--source", "/no/such/dir"])
        .assert()
        .code(1);
    let stderr = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("/no/such/dir"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn the_cli_names_every_file_it_could_not_read() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    common::write_files(
        d.path(),
        &[("src/a.rs", "fn f() {}\n"), ("src/c.rs", "fn g() {}\n")],
    );
    let locked = d.path().join("src/c.rs");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert!(
        std::fs::read(&locked).is_err(),
        "needs a user that cannot read a mode 000 file (not root)"
    );
    assert_cmd::cargo_bin_cmd!()
        .args(["build", "--source"])
        .arg(d.path())
        .assert()
        .success()
        .stdout("map: 1 written, 0 unchanged, 0 removed\n")
        .stderr("warning: could not read src/c.rs\n");
}

fn calls_of(files: &[(&str, &str)], anchor: &str) -> Vec<String> {
    let (_d, vault) = common::built(files);
    common::fn_section(&common::map_page(&vault, "map/src/a_rs"), anchor, "Calls")
}

#[test]
fn a_self_member_of_an_external_brace_import_is_outside() {
    let a = "use std::io::{self, Write};\npub fn f() {\n    io::stdout();\n}\n";
    let files = [
        ("src/lib.rs", "mod a;\nmod io;\n"),
        ("src/io.rs", "pub fn stdout() {}\n"),
        ("src/a.rs", a),
    ];
    assert_eq!(
        calls_of(&files, "f"),
        ["`io::stdout()` L3 → outside the project"]
    );
}

#[test]
fn a_call_on_self_in_a_trait_default_method_may_reach_any_impl() {
    let a = "pub trait Store {\n    fn get(&self);\n    fn twice(&self) {\n        self.get();\n        Self::get(self);\n    }\n}\npub struct S;\nimpl Store for S {\n    fn get(&self) {}\n}\n";
    let to = "one of: [[map/src/a_rs#store_get|Store::get]], [[map/src/a_rs#s_as_store_get|<S as Store>::get]], or outside the project";
    assert_eq!(
        calls_of(&[("src/a.rs", a)], "store_twice"),
        [
            format!("`self.get()` L4 → {to}"),
            format!("`Self::get(self)` L5 → {to}")
        ]
    );
}

#[test]
fn a_receiver_of_a_generic_or_wrapping_type_may_reach_project_code() {
    let a = "use std::rc::Rc;\npub trait Store { fn get(&self); }\npub struct S;\nimpl S { pub fn own(&self) {} }\npub fn gen<T: Store>(x: T) {\n    x.get();\n}\npub fn boxed(x: Box<S>, r: Rc<S>) {\n    x.own();\n    r.own();\n}\n";
    let files = [("src/a.rs", a)];
    assert_eq!(
        calls_of(&files, "gen"),
        ["`x.get()` L6 → one of: [[map/src/a_rs#store_get|Store::get]], or outside the project"]
    );
    let own = "one of: [[map/src/a_rs#s_own|S::own]], or outside the project";
    assert_eq!(
        calls_of(&files, "boxed"),
        [
            format!("`x.own()` L9 → {own}"),
            format!("`r.own()` L10 → {own}")
        ]
    );
}

#[test]
fn a_recursive_call_is_resolved_to_its_caller() {
    let a = "fn rec(n: u8) {\n    rec(n);\n}\n";
    assert_eq!(
        calls_of(&[("src/a.rs", a)], "rec"),
        ["`rec(n)` L2 → [[map/src/a_rs#rec|rec]]"]
    );
}

#[test]
fn a_foreign_file_at_a_page_path_is_left_alone() {
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    common::write_files(
        &src,
        &[
            ("src/a.rs", "fn f() {}\n"),
            ("src/b.rs", "fn g() {}\n"),
            ("src/c.rs", "fn h() {}\n"),
        ],
    );
    // a.rs: a file of the user's own. b.rs: a page cut inside its front matter.
    // c.rs: no page yet; it is written although the two before it are left alone.
    let own = "my own notes\n";
    let cut = "---\ntitle: \"b.rs";
    common::write_files(
        &vault,
        &[("map/src/a_rs.md", own), ("map/src/b_rs.md", cut)],
    );
    let (a, b) = (vault.join("map/src/a_rs.md"), vault.join("map/src/b_rs.md"));
    cli_build(&src, &vault)
        .success()
        .stdout("map: 1 written, 0 unchanged, 0 removed, 2 left alone\n")
        .stderr(format!(
            "warning: {}: not a map page, left alone\nwarning: {}: not a map page, left alone\n",
            a.display(),
            b.display()
        ));
    assert_eq!(std::fs::read_to_string(&a).unwrap(), own);
    assert_eq!(std::fs::read_to_string(&b).unwrap(), cut);
    assert!(vault.join("map/src/c_rs.md").is_file());
}

#[test]
fn index_entries_without_a_page_or_with_a_link_breaking_id_are_dropped() {
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    common::write_files(
        &src,
        &[("src/lib.rs", LIB), ("src/a.rs", A), ("src/b.rs", B)],
    );
    common::write_files(
        &vault,
        &[
            (
                "index.json",
                r#"{"entries":[{"id":"a","path":"src/a.rs"},{"id":"b","path":"src/b.rs"},{"id":"c#x","path":"src/lib.rs"}]}"#,
            ),
            ("a.md", "a\n"),
            ("c#x.md", "c\n"),
        ],
    );
    plaz::build(&src, &vault).unwrap();
    assert!(common::map_page(&vault, "map/src/a_rs").contains("\nCode: [[a|src/a.rs:1-16]]\n"));
    assert!(common::map_page(&vault, "map/src/b_rs").contains("\nCode: `src/b.rs:1-8`\n"));
    assert!(common::map_page(&vault, "map/src/lib_rs").contains("\nCode: `src/lib.rs:1-2`\n"));
}

#[test]
fn a_receiver_typed_by_another_crates_path_is_not_the_project_type() {
    let a = "pub struct Value;\nimpl Value {\n    pub fn grab(&self) {}\n    pub fn new() {}\n}\npub fn shapes(v: serde_json::Value) {\n    v.grab();\n    serde_json::Value::new();\n}\n";
    assert_eq!(
        calls_of(&[("src/a.rs", a)], "shapes"),
        [
            "`v.grab()` L7 → one of: [[map/src/a_rs#value_grab|Value::grab]], or outside the project",
            "`serde_json::Value::new()` L8 → target not visible",
        ]
    );
}

#[test]
fn a_trait_whose_name_prefixes_an_earlier_word_is_still_a_trait() {
    let a = "pub trait tr {\n    fn get(&self);\n    fn twice(&self) {\n        self.get();\n    }\n}\npub struct S;\nimpl tr for S {\n    fn get(&self) {}\n}\n";
    assert_eq!(
        calls_of(&[("src/a.rs", a)], "tr_twice"),
        ["`self.get()` L4 → one of: [[map/src/a_rs#tr_get|tr::get]], [[map/src/a_rs#s_as_tr_get|<S as tr>::get]], or outside the project"]
    );
}

#[test]
fn a_generic_parameter_named_like_a_project_type_is_not_that_type() {
    let a = "pub struct S;\nimpl S {\n    pub fn own(&self) {}\n    pub fn make() {}\n}\npub struct W<S>(S);\nimpl<S> W<S> {\n    pub fn inner(&self, y: S) {\n        y.own();\n    }\n}\npub fn shadow<'a, S: Into<Vec<u8>>>(x: S) {\n    x.own();\n    S::make();\n}\n";
    let files = [("src/a.rs", a)];
    let own = "one of: [[map/src/a_rs#s_own|S::own]], or outside the project";
    assert_eq!(
        calls_of(&files, "shadow"),
        [
            format!("`x.own()` L13 → {own}"),
            "`S::make()` L14 → one of: [[map/src/a_rs#s_make|S::make]], or outside the project"
                .to_string(),
        ]
    );
    assert_eq!(
        calls_of(&files, "w_inner"),
        [format!("`y.own()` L9 → {own}")]
    );
}

#[test]
fn a_bare_call_of_an_external_import_is_outside() {
    let a = "use std::mem::drop;\nuse crate::b::*;\npub fn f(x: u8) {\n    drop(x);\n}\n";
    let files = [("src/a.rs", a), ("src/b.rs", "pub fn drop(_x: u8) {}\n")];
    assert_eq!(
        calls_of(&files, "f"),
        ["`drop(x)` L4 → outside the project"]
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_file_at_a_page_path_is_left_alone() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    common::write_files(&src, &[("src/a.rs", "fn f() {}\n")]);
    common::write_files(&vault, &[("map/src/a_rs.md", "secret\n")]);
    let page = vault.join("map/src/a_rs.md");
    std::fs::set_permissions(&page, std::fs::Permissions::from_mode(0o200)).unwrap();
    assert!(
        std::fs::read(&page).is_err(),
        "needs a user that cannot read a mode 200 file (not root)"
    );
    cli_build(&src, &vault)
        .success()
        .stdout("map: 0 written, 0 unchanged, 0 removed, 1 left alone\n")
        .stderr(format!(
            "warning: {}: not readable, left alone\n",
            page.display()
        ));
    std::fs::set_permissions(&page, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(std::fs::read_to_string(&page).unwrap(), "secret\n");
}

#[test]
fn a_name_an_external_glob_may_bring_in_is_not_certain() {
    let a = "use other::*;\nuse crate::b::Thing;\npub fn f(v: Value, t: Thing) {\n    helper();\n    v.grab();\n    Value::new();\n    t.go();\n}\n";
    let b = "pub struct Thing;\nimpl Thing {\n    pub fn go(&self) {}\n}\npub fn helper() {}\npub struct Value;\nimpl Value {\n    pub fn grab(&self) {}\n    pub fn new() {}\n}\n";
    let files = [
        ("src/lib.rs", "mod a;\nmod b;\n"),
        ("src/a.rs", a),
        ("src/b.rs", b),
    ];
    assert_eq!(
        calls_of(&files, "f"),
        [
            "`helper()` L4 → one of: [[map/src/b_rs#helper|helper]], or outside the project",
            "`v.grab()` L5 → one of: [[map/src/b_rs#value_grab|Value::grab]], or outside the project",
            "`Value::new()` L6 → one of: [[map/src/b_rs#value_new|Value::new]], or outside the project",
            "`t.go()` L7 → [[map/src/b_rs#thing_go|Thing::go]]",
        ]
    );
}

fn calls_in(files: &[(&str, &str)], page: &str, anchor: &str) -> Vec<String> {
    let (_d, vault) = common::built(files);
    common::fn_section(&common::map_page(&vault, page), anchor, "Calls")
}

const APP: &str = "pub fn run() {}\npub struct App;\nimpl App {\n    pub fn new() -> App { App }\n    pub fn go(&self) {}\n}\n";

// N1 and P1: `mod app; use app::…` names the child file of this file.
#[test]
fn a_name_imported_through_a_module_the_file_declares_is_certain() {
    let main = "mod app;\nuse app::{run, App};\nfn main() {\n    run();\n    App::new();\n}\nfn drive(a: App) {\n    a.go();\n}\n";
    let files = [("src/main.rs", main), ("src/app.rs", APP)];
    assert_eq!(
        calls_in(&files, "map/src/main_rs", "main"),
        [
            "`run()` L4 → [[map/src/app_rs#run|run]]",
            "`App::new()` L5 → [[map/src/app_rs#app_new|App::new]]",
        ]
    );
    assert_eq!(
        calls_in(&files, "map/src/main_rs", "drive"),
        ["`a.go()` L8 → [[map/src/app_rs#app_go|App::go]]"]
    );
}

// N1: a file module and an inline module as import roots.
#[test]
fn imports_rooted_in_a_file_module_or_an_inline_module_are_certain() {
    let lib = "mod b;\nmod inner { pub fn ihelp() {} }\nuse b::helper;\nuse inner::ihelp;\npub fn f() {\n    helper();\n    ihelp();\n}\n";
    let files = [("src/lib.rs", lib), ("src/b.rs", "pub fn helper() {}\n")];
    assert_eq!(
        calls_in(&files, "map/src/lib_rs", "f"),
        [
            "`helper()` L6 → [[map/src/b_rs#helper|helper]]",
            "`ihelp()` L7 → [[map/src/lib_rs#ihelp|ihelp]]",
        ]
    );
}

// N2: a value local does not shadow a module path root.
#[test]
fn a_parameter_named_like_a_module_does_not_hide_the_module() {
    let a = "use crate::store;\nuse crate::store::Store;\npub fn f(store: &Store) {\n    store::helper();\n}\n";
    let store = "pub struct Store;\npub fn helper() {}\n";
    let files = [("src/a.rs", a), ("src/store.rs", store)];
    assert_eq!(
        calls_of(&files, "f"),
        ["`store::helper()` L4 → [[map/src/store_rs#helper|helper]]"]
    );
}

// N3: a `use` in some body or module may or may not reach a given call.
#[test]
fn a_name_both_defined_and_imported_in_the_file_is_not_certain() {
    let a = "pub fn helper() {}\npub fn f() {\n    helper();\n}\nfn g() {\n    use other::helper;\n    helper();\n}\n#[cfg(test)]\nmod tests {\n    use mock::helper;\n}\n";
    let files = [("src/a.rs", a)];
    let to = "one of: [[map/src/a_rs#helper|helper]], or outside the project";
    assert_eq!(calls_of(&files, "f"), [format!("`helper()` L3 → {to}")]);
    assert_eq!(calls_of(&files, "g"), [format!("`helper()` L7 → {to}")]);
}

// P2: a constructor of a project type is no outside call.
#[test]
fn a_tuple_struct_or_variant_constructor_is_not_an_outside_call() {
    let a = "pub struct Meters(pub u32);\npub enum Shape { Circle(u8) }\nuse Shape::Circle;\npub fn f() {\n    Meters(5);\n    Circle(1);\n    Some(3);\n    Shape::Circle(2);\n}\n";
    assert_eq!(
        calls_of(&[("src/a.rs", a)], "f"),
        [
            "`Meters(5)` L5 → target not visible",
            "`Circle(1)` L6 → target not visible",
            "`Some(3)` L7 → outside the project",
            "`Shape::Circle(2)` L8 → target not visible",
        ]
    );
}

// P3: `super::f` and `crate::f` are not this file's `f` first.
#[test]
fn a_path_to_the_parent_or_the_crate_root_is_not_the_files_own_function() {
    let a = "pub fn f() {}\npub fn g() {\n    super::f();\n    crate::f();\n}\n";
    let files = [("src/lib.rs", "mod a;\npub fn f() {}\n"), ("src/a.rs", a)];
    let to = "one of: [[map/src/a_rs#f|f]], [[map/src/lib_rs#f|f]]";
    assert_eq!(
        calls_of(&files, "g"),
        [
            format!("`super::f()` L3 → {to}"),
            format!("`crate::f()` L4 → {to}")
        ]
    );
}

// P4: a method the own type gets from a trait is not another type's.
#[test]
fn a_method_on_self_that_the_own_type_lacks_is_not_a_namesakes() {
    let a = "pub trait Tr { fn get(&self); }\npub struct S;\nimpl Tr for S {\n    fn get(&self) {}\n}\nimpl S {\n    pub fn run(&self) {\n        self.get();\n        Self::get(self);\n    }\n}\n";
    let b = "pub struct S;\nimpl S {\n    pub fn get(&self) {}\n}\n";
    let to = "one of: [[map/src/a_rs#tr_get|Tr::get]], [[map/src/a_rs#s_as_tr_get|<S as Tr>::get]], [[map/src/b_rs#s_get|S::get]], or outside the project";
    assert_eq!(
        calls_of(&[("src/a.rs", a), ("src/b.rs", b)], "s_run"),
        [
            format!("`self.get()` L8 → {to}"),
            format!("`Self::get(self)` L9 → {to}")
        ]
    );
}

// P5: the import says which of two types named `S` is meant.
#[test]
fn an_imported_type_is_not_a_namesake_in_another_file() {
    let a = "use crate::c::{S, Tr};\npub fn f(s: S) {\n    s.get();\n    S::new();\n}\n";
    let b = "pub struct S;\nimpl S {\n    pub fn get(&self) {}\n    pub fn new() -> S { S }\n}\n";
    let c = "pub struct S;\npub trait Tr { fn get(&self); }\nimpl Tr for S {\n    fn get(&self) {}\n}\n";
    assert_eq!(
        calls_of(&[("src/a.rs", a), ("src/b.rs", b), ("src/c.rs", c)], "f"),
        [
            "`s.get()` L3 → one of: [[map/src/b_rs#s_get|S::get]], [[map/src/c_rs#tr_get|Tr::get]], [[map/src/c_rs#s_as_tr_get|<S as Tr>::get]], or outside the project",
            "`S::new()` L4 → one of: [[map/src/b_rs#s_new|S::new]], or outside the project",
        ]
    );
}

// P6: an alias is not the project type that happens to carry its name.
#[test]
fn an_import_alias_named_like_a_project_type_is_not_that_type() {
    let a = "use crate::b::Store as Cache;\npub fn f(c: Cache) {\n    c.get();\n    Cache::get(&c);\n}\n";
    let b = "pub struct Store;\nimpl Store {\n    pub fn get(&self) {}\n}\n";
    let c = "pub struct Cache;\nimpl Cache {\n    pub fn get(&self) {}\n}\n";
    assert_eq!(
        calls_of(&[("src/a.rs", a), ("src/b.rs", b), ("src/c.rs", c)], "f"),
        [
            "`c.get()` L3 → one of: [[map/src/b_rs#store_get|Store::get]], [[map/src/c_rs#cache_get|Cache::get]], or outside the project",
            "`Cache::get(&c)` L4 → one of: [[map/src/b_rs#store_get|Store::get]], [[map/src/c_rs#cache_get|Cache::get]], or outside the project",
        ]
    );
}

// The "already right" list of round 3 that no earlier test pins.
#[test]
fn reference_and_path_typed_receivers_are_certain_and_pattern_bound_ones_open() {
    let a = "pub struct S;\nimpl S {\n    pub fn own(&self) {}\n}\npub fn f<'a>(m: &mut S, r: &'a S, p: crate::a::S, o: Option<S>, v: Vec<S>) {\n    m.own();\n    r.own();\n    p.own();\n    if let Some(t) = o {\n        t.own();\n    }\n    for u in v {\n        u.own();\n    }\n}\n";
    let sure = "[[map/src/a_rs#s_own|S::own]]";
    let open = "one of: [[map/src/a_rs#s_own|S::own]], or outside the project";
    assert_eq!(
        calls_of(&[("src/lib.rs", "mod a;\n"), ("src/a.rs", a)], "f"),
        [
            format!("`m.own()` L6 → {sure}"),
            format!("`r.own()` L7 → {sure}"),
            format!("`p.own()` L8 → {sure}"),
            format!("`t.own()` L10 → {open}"),
            format!("`u.own()` L13 → {open}"),
        ]
    );
}

// Round-3 probes: shapes found by running the binary on scratch sources.

#[test]
fn a_call_that_may_be_recursion_keeps_the_caller_as_a_candidate() {
    let a = "pub struct Node { child: Box<Node> }\nimpl Node {\n    pub fn depth(&self) {\n        self.child.depth();\n        self.depth();\n    }\n}\n#[cfg(unix)]\nfn f() {\n    f();\n}\n#[cfg(windows)]\nfn f() {}\n";
    let files = [("src/a.rs", a)];
    assert_eq!(
        calls_of(&files, "node_depth"),
        [
            "`self.child.depth()` L4 → one of: [[map/src/a_rs#node_depth|Node::depth]], or outside the project",
            "`self.depth()` L5 → [[map/src/a_rs#node_depth|Node::depth]]",
        ]
    );
    assert_eq!(
        calls_of(&files, "f"),
        ["`f()` L10 → one of: [[map/src/a_rs#f|f]], [[map/src/a_rs#f_2|f]]"]
    );
}

#[test]
fn an_item_declared_in_a_body_hides_the_files_item_of_that_name() {
    let a = "pub struct S;\nimpl S {\n    pub fn new() -> S { S }\n}\nfn helper() {}\nfn f() {\n    fn helper() {}\n    struct S;\n    helper();\n    S::new();\n    let run = || {\n        helper();\n    };\n}\nfn g() {\n    let x = W { a: helper(), b: S::new() };\n    println!(\"{}\", helper());\n}\n";
    let files = [("src/a.rs", a)];
    assert_eq!(
        calls_of(&files, "f"),
        [
            "`helper()` L9 → target not visible",
            "`S::new()` L10 → one of: [[map/src/a_rs#s_new|S::new]], or outside the project",
            "`helper()` L12 → target not visible",
        ]
    );
    assert_eq!(
        calls_of(&files, "g"),
        [
            "`helper()` L16 → [[map/src/a_rs#helper|helper]]",
            "`S::new()` L16 → [[map/src/a_rs#s_new|S::new]]",
            "`helper()` L17 → [[map/src/a_rs#helper|helper]]",
        ]
    );
}

#[test]
fn another_crates_name_may_be_the_projects_own_library() {
    let main = "use mycrate::run;\nuse serde::to_vec;\nfn main() {\n    run();\n    mycrate::run();\n    to_vec();\n}\n";
    let files = [("src/lib.rs", "pub fn run() {}\n"), ("src/main.rs", main)];
    let run = "one of: [[map/src/lib_rs#run|run]], or outside the project";
    assert_eq!(
        calls_in(&files, "map/src/main_rs", "main"),
        [
            format!("`run()` L4 → {run}"),
            "`mycrate::run()` L5 → target not visible".to_string(),
            "`to_vec()` L6 → outside the project".to_string(),
        ]
    );
}

const WORKSPACE: [(&str, &str); 9] = [
    ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
    (
        "crates/app-model/Cargo.toml",
        "[package]\nname = \"app-model\"\nversion = \"0.1.0\"\n",
    ),
    (
        "crates/app-model/src/lib.rs",
        "pub mod value;\npub use value::{Mapping, Value};\npub fn run() {}\n",
    ),
    (
        "crates/app-model/src/value.rs",
        "pub struct Mapping;\nimpl Mapping {\n    pub fn new() -> Self { Mapping }\n}\npub enum Value {\n    Mapping(Mapping),\n}\n",
    ),
    // The directory is `cli`, the package `app-cli`.
    (
        "crates/cli/Cargo.toml",
        "[package]\nname = \"app-cli\" # the CLI\n\n[lib]\npath = \"src/lib.rs\"\n\n[[bin]]\nname = \"app\"\n",
    ),
    (
        "crates/cli/src/lib.rs",
        "pub mod tool;\npub fn run(_args: Vec<String>) {}\n",
    ),
    (
        "crates/cli/src/tool.rs",
        "use app_model::{Mapping, Value};\npub fn make() -> Value {\n    Value::Mapping(Mapping::new())\n}\n",
    ),
    (
        "crates/cli/src/main.rs",
        "fn main() {\n    app_cli::run(Vec::new());\n    other_crate::go();\n    cli::go();\n    std::mem::drop(1);\n}\n",
    ),
    ("crates/cli/src/other.rs", "pub fn go() {}\n"),
];

#[test]
fn a_path_rooted_at_an_own_crate_name_is_the_projects() {
    assert_eq!(
        calls_in(&WORKSPACE, "map/crates/cli/src/tool_rs", "make"),
        [
            "`Mapping::new()` L3 → [[map/crates/app-model/src/value_rs#mapping_new|Mapping::new]]",
            "`Value::Mapping(Mapping::new())` L3 → target not visible",
        ]
    );
    // Two `run`s in the tree: the one under the crate named.
    assert_eq!(
        calls_in(&WORKSPACE, "map/crates/cli/src/main_rs", "main"),
        [
            "`Vec::new()` L2 → target not visible",
            "`app_cli::run(Vec::new())` L2 → [[map/crates/cli/src/lib_rs#run|run]]",
            "`other_crate::go()` L3 → target not visible",
            "`cli::go()` L4 → target not visible",
            "`std::mem::drop(1)` L5 → outside the project",
        ]
    );
}

#[test]
fn a_binary_reaches_its_own_library_through_the_package_name() {
    let files = [
        (
            "Cargo.toml",
            "[package]\nname = \"wiki\"\n\n[dependencies]\nname = \"not-this\"\n",
        ),
        ("src/lib.rs", "pub mod labels;\n"),
        ("src/labels.rs", "pub fn doc_search() {}\n"),
        (
            "src/main.rs",
            "fn main() {\n    wiki::labels::doc_search();\n}\n",
        ),
    ];
    assert_eq!(
        calls_in(&files, "map/src/main_rs", "main"),
        ["`wiki::labels::doc_search()` L2 → [[map/src/labels_rs#doc_search|doc_search]]"]
    );
}

#[test]
fn a_local_module_shadows_an_own_crate_of_its_name() {
    let files = [
        ("utils/Cargo.toml", "[package]\nname = \"utils\"\n"),
        ("utils/src/lib.rs", "pub fn load() {}\n"),
        ("cli/Cargo.toml", "[package]\nname = \"cli\"\n"),
        (
            "cli/src/main.rs",
            "mod other;\nmod plain;\nmod utils;\nfn main() {\n    utils::load();\n}\n",
        ),
        ("cli/src/utils.rs", "pub fn load() {}\n"),
        (
            "cli/src/other.rs",
            "use crate::utils;\npub fn f() {\n    utils::load();\n}\n",
        ),
        ("cli/src/plain.rs", "pub fn g() {\n    utils::load();\n}\n"),
    ];
    let (_d, vault) = common::built(&files);
    let calls = |page: &str, anchor: &str| {
        common::fn_section(&common::map_page(&vault, page), anchor, "Calls")
    };
    let local = "`utils::load()` L5 → [[map/cli/src/utils_rs#load|load]]";
    assert_eq!(calls("map/cli/src/main_rs", "main"), [local]);
    assert_eq!(
        calls("map/cli/src/other_rs", "f"),
        ["`utils::load()` L3 → [[map/cli/src/utils_rs#load|load]]"]
    );
    assert_eq!(
        calls("map/cli/src/plain_rs", "g"),
        ["`utils::load()` L2 → [[map/utils/src/lib_rs#load|load]]"]
    );
}

#[test]
fn a_crate_path_links_only_into_the_crate_it_names() {
    let files = [
        (
            "Cargo.toml",
            "[package]\nname = \"app\"\n\n[workspace]\nmembers = [\"crates/*\"]\n",
        ),
        (
            "src/lib.rs",
            "pub use outdep::{helper, Thing};\npub fn run() {}\n",
        ),
        (
            "src/main.rs",
            "fn main() {\n    app::helper();\n    app::Thing::new();\n    app::run();\n}\n",
        ),
        ("crates/util/Cargo.toml", "[package]\nname = \"util\"\n"),
        (
            "crates/util/src/lib.rs",
            "pub fn helper() {}\npub struct Thing;\nimpl Thing {\n    pub fn new() {}\n}\n",
        ),
        ("crates/engine/Cargo.toml", "[package]\nname = \"engine\"\n"),
        ("crates/engine/src/lib.rs", "pub fn run() {}\n"),
    ];
    assert_eq!(
        calls_in(&files, "map/src/main_rs", "main"),
        [
            "`app::helper()` L2 → one of: [[map/crates/util/src/lib_rs#helper|helper]], or outside the project",
            "`app::Thing::new()` L3 → one of: [[map/crates/util/src/lib_rs#thing_new|Thing::new]], or outside the project",
            "`app::run()` L4 → [[map/src/lib_rs#run|run]]",
        ]
    );
    // A crate inside another crate's directory is not part of it.
    let nested = [
        ("wiki/Cargo.toml", "[package]\nname = \"wiki\"\n"),
        ("wiki/src/lib.rs", "pub mod walk;\n"),
        ("wiki/src/walk.rs", "pub fn mine() {}\n"),
        (
            "wiki/src/main.rs",
            "fn main() {\n    wiki::walk::mine();\n}\n",
        ),
        ("wiki/code/Cargo.toml", "[package]\nname = \"wiki-code\"\n"),
        ("wiki/code/src/lib.rs", "pub mod walk;\n"),
        ("wiki/code/src/walk.rs", "pub fn mine() {}\n"),
    ];
    assert_eq!(
        calls_in(&nested, "map/wiki/src/main_rs", "main"),
        ["`wiki::walk::mine()` L2 → [[map/wiki/src/walk_rs#mine|mine]]"]
    );
}

#[test]
fn a_name_imported_from_an_own_crate_is_that_crates() {
    let files = [
        ("a/Cargo.toml", "[package]\nname = \"alpha\"\n"),
        ("a/src/lib.rs", "pub fn resolve() {}\n"),
        ("b/Cargo.toml", "[package]\nname = \"beta\"\n"),
        ("b/src/lib.rs", "pub fn resolve() {}\n"),
        ("c/Cargo.toml", "[package]\nname = \"gamma\"\n"),
        (
            "c/src/lib.rs",
            "use beta::resolve;\npub fn f() {\n    resolve();\n}\n",
        ),
    ];
    assert_eq!(
        calls_in(&files, "map/c/src/lib_rs", "f"),
        ["`resolve()` L3 → [[map/b/src/lib_rs#resolve|resolve]]"]
    );
}

#[test]
fn a_project_trait_may_give_an_outside_type_a_method() {
    let a = "use std::collections::HashSet;\npub trait Ext { fn twice(&self); }\nimpl Ext for HashSet<u8> {\n    fn twice(&self) {}\n}\npub struct S;\nimpl S {\n    pub fn len(&self) {}\n}\npub fn f(v: HashSet<u8>) {\n    v.twice();\n    v.len();\n}\n";
    assert_eq!(
        calls_of(&[("src/a.rs", a)], "f"),
        [
            "`v.twice()` L11 → one of: [[map/src/a_rs#ext_twice|Ext::twice]], [[map/src/a_rs#hashset_u8_as_ext_twice|<HashSet<u8> as Ext>::twice]], or outside the project",
            "`v.len()` L12 → outside the project",
        ]
    );
}

#[test]
fn a_module_the_file_declares_as_core_is_not_the_core_crate() {
    let lib = "mod core;\nmod a;\npub fn f() {\n    core::start();\n}\n";
    let a = "pub fn g() {\n    core::mem::drop(1);\n    crate::core::start();\n}\n";
    let files = [
        ("src/lib.rs", lib),
        ("src/core.rs", "pub fn start() {}\n"),
        ("src/a.rs", a),
    ];
    assert_eq!(
        calls_in(&files, "map/src/lib_rs", "f"),
        ["`core::start()` L4 → [[map/src/core_rs#start|start]]"]
    );
    assert_eq!(
        calls_of(&files, "g"),
        [
            "`core::mem::drop(1)` L2 → outside the project",
            "`crate::core::start()` L3 → [[map/src/core_rs#start|start]]",
        ]
    );
}

#[test]
fn an_alias_of_a_project_module_or_type_names_the_original() {
    let a = "use crate::b as bee;\nuse crate::b::{helper as h, Store as St};\nuse std::mem::drop as d;\npub fn f(x: u8) {\n    h();\n    d(x);\n    bee::helper();\n    St::new();\n}\n";
    let b = "pub fn helper() {}\npub fn d(_x: u8) {}\npub struct Store;\nimpl Store {\n    pub fn new() -> Store { Store }\n}\n";
    assert_eq!(
        calls_of(&[("src/a.rs", a), ("src/b.rs", b)], "f"),
        [
            "`h()` L5 → [[map/src/b_rs#helper|helper]]",
            "`d(x)` L6 → outside the project",
            "`bee::helper()` L7 → [[map/src/b_rs#helper|helper]]",
            "`St::new()` L8 → [[map/src/b_rs#store_new|Store::new]]",
        ]
    );
}

#[test]
fn a_project_import_beside_an_outside_alias_of_the_same_name_stays_in_the_project() {
    let a = "use std::io::Error as IoError;\nuse std::fmt::Result as FmtResult;\nuse crate::error::Error;\npub fn f(e: Error, ie: IoError) {\n    Error::missing();\n    e.describe();\n}\n";
    let error = "pub struct Error;\nimpl Error {\n    pub fn missing() -> Self { Error }\n    pub fn describe(&self) -> u8 { 0 }\n}\n";
    assert_eq!(
        calls_of(&[("src/a.rs", a), ("src/error.rs", error)], "f"),
        [
            "`Error::missing()` L5 → one of: [[map/src/error_rs#error_missing|Error::missing]], or outside the project",
            "`e.describe()` L6 → one of: [[map/src/error_rs#error_describe|Error::describe]], or outside the project",
        ]
    );
}

#[test]
fn a_project_function_beside_an_outside_alias_of_the_same_name_stays_in_the_project() {
    let a = "use std::fs::read as fs_read;\nuse crate::io2::read;\npub fn f() {\n    read();\n}\n";
    assert_eq!(
        calls_of(
            &[("src/a.rs", a), ("src/io2.rs", "pub fn read() {}\n")],
            "f"
        ),
        ["`read()` L4 → one of: [[map/src/io2_rs#read|read]], or outside the project"]
    );
}

const F_SRC: &str = "fn f(x: u8, cb: impl Fn()) -> u8 {\n    // REQ-417. Small inputs skip the cache.\n    // The cache is cold at start.\n    if x < 3 {\n        cb();\n        return 0;\n    } else if let Some(y) = g(x) {\n        h(y)?;\n    } else {\n        k();\n    }\n    for i in 0..x {\n        if i == 2 { break; }\n    }\n    match x {\n        1 | 2 => m(),\n        n if n > 9 => panic!(\"big\"),\n        _ => {}\n    }\n    x\n}\n";
const LOOPS: &str = "fn g(xs: &[u8]) {\n    'outer: for x in xs {\n        for y in xs {\n            if y > x {\n                continue 'outer;\n            }\n            if y == x {\n                break 'outer;\n            }\n            if *y == 0 {\n                continue;\n            }\n            if *y == 9 {\n                return;\n            }\n            w();\n        }\n    }\n    z();\n}\n";
const LET_ELSE: &str = "fn f(xs: &[u8]) {\n    for x in xs {\n        // skip empties\n        let Some(v) = get(x) else {\n            continue;\n        };\n        use_it(v);\n    }\n}\n";
const HOSTILE: &str = "struct W<T>(T);\nimpl<T: std::fmt::Display> std::fmt::Display for W<T> {\n    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { if \"a|b\" != \"<x>\" { g(\"]] {} `y` #z\"); Ok(()) } else { Ok(()) } }\n}\n";

#[test]
fn mermaid_labels_escape_every_hostile_char() {
    assert_eq!(
        plaz::render::mermaid_label("a<b> && c|d {\"#x\"} `y`\nz"),
        "a#lt;b#gt; #amp;#amp; c#124;d #123;#quot;#35;x#quot;#125; #96;y#96;<br/>z"
    );
}

#[test]
fn rust_chart_snapshot() {
    let (_d, vault) = common::built(&[("src/a.rs", F_SRC)]);
    insta::assert_snapshot!(
        "rust_chart",
        common::fn_block(&common::map_page(&vault, "map/src/a_rs"), "f")
    );
}

#[test]
fn coverage_line_counts_forms_and_not_drawn() {
    let (_d, vault) = common::built(&[("src/lib.rs", LIB), ("src/a.rs", A), ("src/b.rs", B)]);
    let run = common::fn_block(&common::map_page(&vault, "map/src/a_rs"), "run");
    assert!(run.contains(
        "\nCoverage: 9 calls: 3 resolved, 1 one of, 3 outside, 2 hidden. Not drawn: none.\n"
    ));
    let src = "fn f(v: Vec<u8>) -> u8 {\n    let a = v.first().copied().unwrap_or(0);\n    let run = |z: u8| {\n        let b = g(z)?;\n        if z > a { log(|q| q); }\n    };\n    a\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let f = common::fn_block(&common::map_page(&vault, "map/src/a_rs"), "f");
    assert!(
        f.contains("\nCoverage: 5 calls: 0 resolved, 0 one of, 5 outside, 0 hidden. Not drawn: 1 ?, 1 unwrap_or, 1 closure.\n"),
        "{f}"
    );
}

#[test]
fn a_damaged_function_says_so_in_its_coverage_line() {
    let (_d, vault) = common::built(&[("src/a.rs", "fn ok() {}\nfn g() { if a() { b(; } }\n")]);
    let page = common::map_page(&vault, "map/src/a_rs");
    let g = common::fn_block(&page, "g");
    assert!(
        g.lines().any(|l| l.starts_with("Coverage: ")
            && l.ends_with(" Syntax errors: chart may be incomplete.")),
        "{g}"
    );
    assert!(!common::fn_block(&page, "ok").contains("Syntax errors: chart"));
}

#[test]
fn long_comments_become_a_line_reference() {
    let src = format!(
        "fn f(x: u8) {{\n{}    if x > 1 {{}}\n    // a\n    // b\n    if x > 2 {{}}\n}}\n",
        "    // l\n".repeat(7)
    );
    let (_d, vault) = common::built(&[("src/a.rs", &src)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    assert!(page.contains("comment: 7 lines at src/a.rs:2-8"), "{page}");
    assert!(page.contains("[/\"a<br/>b\"\\]"), "{page}");
}

#[test]
fn hostile_owner_names_keep_slug_anchors() {
    let (_d, vault) = common::built(&[("src/a.rs", HOSTILE)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    let heading = page.lines().find(|l| l.starts_with("## ")).unwrap();
    assert!(heading[3..]
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
    // diamond label escaped
    assert!(page.contains("{\"#quot;a#124;b#quot; != #quot;#lt;x#gt;#quot;\"}"));
    // call label escaped; nothing in the chart can end the fence
    assert!(
        page.contains("g(#quot;]] #123;#125; #96;y#96; #35;z#quot;)"),
        "{page}"
    );
    let chart = page.split("```mermaid\n").nth(1).unwrap();
    let chart = chart.split_once("\n```\n").unwrap().0;
    assert!(!chart.contains('`'), "{chart}");
    assert!(
        chart.contains("#lt;W#lt;T#gt; as std::fmt::Display#gt;::fmt"),
        "{chart}"
    );
}

#[test]
fn huge_charts_are_whole_and_point_to_the_calls_list() {
    let body = (0..600).fold(String::new(), |s, i| s + &format!("    g{i}();\n"));
    let (_d, vault) = common::built(&[("src/a.rs", &format!("fn f() {{\n{body}}}\n"))]);
    let page = common::map_page(&vault, "map/src/a_rs");
    let chart = page.split("```mermaid\n").nth(1).unwrap();
    assert!(chart.split_once("\n```\n").unwrap().0.contains("g599()"));
    assert!(page.contains("\n```\n\nThis chart exceeds Mermaid's default limits (500 edges, 50,000 characters); the Calls list below is the complete view.\n"));
    let (_d, vault) = common::built(&[("src/a.rs", F_SRC)]);
    assert!(!common::map_page(&vault, "map/src/a_rs").contains("Mermaid's default limits"));
}

#[test]
fn syntax_errors_are_stated_on_the_page() {
    let (_d, vault) = common::built(&[("src/a.rs", "fn ok() { if a() { b(); } }\nfn broken( {\n")]);
    let page = common::map_page(&vault, "map/src/a_rs");
    assert!(
        page.contains("Syntax errors in this file: some functions may be missing from the map.")
    );
    assert!(page.contains("## ok"));
}

#[test]
fn a_language_without_hooks_still_has_a_page() {
    let (_d, vault) = common::built(&[("a.go", "package a\nfunc F() {}\n")]);
    assert!(common::map_page(&vault, "map/a_go").contains("Not covered yet: go flowcharts."));
}

#[test]
fn a_click_from_a_nested_page_points_at_the_right_page() {
    let files = [
        ("src/lib.rs", "mod b;\nmod x;\n"),
        ("src/x.rs", "mod a;\n"),
        (
            "src/x/a.rs",
            "pub fn go() {\n    crate::b::helper();\n    crate::b::other();\n}\n",
        ),
        ("src/b.rs", "pub fn helper() {}\npub fn other() {}\n"),
    ];
    let (_d, vault) = common::built(&files);
    let page = common::map_page(&vault, "map/src/x/a_rs");
    let clicks: Vec<&str> = page
        .lines()
        .filter(|l| l.trim_start().starts_with("click "))
        .collect();
    assert_eq!(
        clicks,
        [
            "    click n1 href \"../b_rs#helper\"",
            "    click n2 href \"../b_rs#other\""
        ],
        "{page}"
    );
}

#[test]
#[ignore = "needs Node: npx -y @mermaid-js/mermaid-cli@12.2.0"]
fn every_chart_parses_with_mermaid_cli() {
    let files = [
        ("src/lib.rs", LIB),
        ("src/a.rs", A),
        ("src/b.rs", B),
        ("src/f.rs", F_SRC),
        ("src/w.rs", HOSTILE),
        ("src/l.rs", LOOPS),
        ("src/e.rs", LET_ELSE),
        ("py/f.py", PY_F),
        ("py/c.py", PY_CHART),
        ("py/t.py", PY_TRY),
    ];
    let (d, vault) = common::built(&files);
    let mut n = 0;
    for page in [
        "map/src/a_rs",
        "map/src/b_rs",
        "map/src/f_rs",
        "map/src/w_rs",
        "map/src/l_rs",
        "map/src/e_rs",
        "map/py/f_py",
        "map/py/c_py",
        "map/py/t_py",
    ] {
        let text = common::map_page(&vault, page);
        let blocks: Vec<&str> = text.split("```mermaid\n").skip(1).collect();
        // One chart per function heading: a page that lost a chart, or whose charts
        // were never fed to the renderer, fails here, and a new fixture needs no new count.
        let headings = text.lines().filter(|l| l.starts_with("## ")).count();
        assert_ne!(headings, 0, "{page}");
        assert_eq!(blocks.len(), headings, "{page}");
        for block in blocks {
            let chart = block.split_once("\n```\n").unwrap().0;
            let mmd = d.path().join(format!("{n}.mmd"));
            std::fs::write(&mmd, chart).unwrap();
            let out = std::process::Command::new("npx")
                .args(["-y", "@mermaid-js/mermaid-cli@12.2.0", "-q", "-i"])
                .arg(&mmd)
                .arg("-o")
                .arg(mmd.with_extension("svg"))
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{page}:\n{chart}\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
            n += 1;
        }
    }
    // The fixture's functions, one chart each (a nested `inner` is drawn inside `f`).
    assert_eq!(n, 12, "a function vanished from the map, or one was added");
}

#[test]
fn an_else_comment_hangs_off_the_else_body() {
    let src = "fn f(x: u8) {\n    if x > 1 {\n        a();\n    } else { // keep\n        k();\n    }\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    assert!(
        page.contains("    n3[/\"k()\"/]\n    n1 -->|no| n3\n    n4[/\"keep\"\\]\n    n3 -.- n4\n"),
        "{page}"
    );
}

/// The edges of the chart in `## {anchor}` as `from -label-> to` (`-->`
/// when unlabelled, `-.-` for a note), each node by its label as written.
fn chart_edges(page: &str, anchor: &str) -> Vec<String> {
    let block = common::fn_block(page, anchor);
    let chart = block.split("```mermaid\n").nth(1).unwrap();
    let chart = chart.split_once("```").unwrap().0;
    let mut labels = std::collections::BTreeMap::new();
    let mut edges = Vec::new();
    for l in chart.lines().map(str::trim) {
        if l.starts_with("subgraph ") {
            continue;
        }
        let id_end = l
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(l.len());
        let (id, rest) = l.split_at(id_end);
        if let (Some(a), Some(b)) = (rest.find('"'), rest.rfind('"')) {
            if a < b && !l.starts_with("click") && !l.starts_with("subgraph") {
                labels.insert(id.to_string(), rest[a + 1..b].to_string());
                continue;
            }
        }
        let Some((from, rest)) = l.split_once(' ') else {
            continue;
        };
        let edge = if let Some(r) = rest.strip_prefix("-->|") {
            r.split_once("| ")
                .map(|(lab, to)| (format!("-{lab}->"), to))
        } else if let Some(to) = rest.strip_prefix("--> ") {
            Some(("-->".to_string(), to))
        } else {
            rest.strip_prefix("-.- ").map(|to| ("-.-".to_string(), to))
        };
        if let Some((arrow, to)) = edge {
            edges.push((from.to_string(), arrow, to.to_string()));
        }
    }
    edges
        .into_iter()
        .map(|(f, a, t)| format!("{} {a} {}", labels[&f], labels[&t]))
        .collect()
}

#[test]
fn break_leaves_the_loop_and_a_bare_loop_has_no_done_edge() {
    let src = "fn f(n: u8) {\n    loop {\n        if n > 3 {\n            break;\n        }\n        tick();\n    }\n    after();\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    assert_eq!(
        chart_edges(&common::map_page(&vault, "map/src/a_rs"), "f"),
        [
            "f --> loop",
            "loop -loop-> n #gt; 3",
            "n #gt; 3 -yes-> break",
            "n #gt; 3 -no-> tick()",
            "tick() --> loop",
            "break --> after()",
            "after() --> end",
        ]
    );
}

#[test]
fn continue_returns_to_its_loop_and_labels_pick_the_loop() {
    let src = LOOPS;
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let edges = chart_edges(&common::map_page(&vault, "map/src/a_rs"), "g");
    for e in [
        "continue 'outer --> 'outer: for x in xs",
        "break 'outer --> z()",
        "continue --> for y in xs",
        "w() --> for y in xs",
        "for y in xs -done-> 'outer: for x in xs",
        "'outer: for x in xs -done-> z()",
        "z() --> end",
    ] {
        assert!(edges.contains(&e.to_string()), "{e} not in {edges:#?}");
    }
    assert!(
        !edges.iter().any(|e| e.starts_with("return ")),
        "{edges:#?}"
    );
}

#[test]
fn let_else_is_a_decision_with_its_comment() {
    let src = LET_ELSE;
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    assert_eq!(
        chart_edges(&common::map_page(&vault, "map/src/a_rs"), "f"),
        [
            "f --> for x in xs",
            "for x in xs -loop-> get(x)",
            "get(x) --> let Some(v) = get(x)",
            "let Some(v) = get(x) -.- skip empties",
            "let Some(v) = get(x) -no-> continue",
            "continue --> for x in xs",
            "let Some(v) = get(x) -yes-> use_it(v)",
            "use_it(v) --> for x in xs",
            "for x in xs -done-> end",
        ]
    );
}

#[test]
fn an_exhaustive_match_has_no_way_past_its_last_arm() {
    let src = "enum Kind { A, B }\nfn f(k: Kind) -> u8 {\n    match k {\n        Kind::A => a(),\n        Kind::B => return 2,\n    }\n    after();\n    0\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    assert_eq!(
        chart_edges(&common::map_page(&vault, "map/src/a_rs"), "f"),
        [
            "f --> match k",
            "match k --> Kind::A",
            "Kind::A -yes-> a()",
            "Kind::A -no-> Kind::B",
            "Kind::B -yes-> return 2",
            "a() --> after()",
            "after() --> end",
        ]
    );
}

#[test]
fn a_function_whose_every_path_returns_has_no_end_node() {
    let src = "fn r(x: u8) -> u8 {\n    if x > 1 {\n        return 1;\n    } else {\n        return 2;\n    }\n}\nfn e() {}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    assert!(!chart_edges(&page, "r").iter().any(|e| e.ends_with(" end")));
    assert_eq!(chart_edges(&page, "e"), ["e --> end"]);
}

#[test]
fn short_circuit_operators_are_stated_on_the_coverage_line() {
    let src = "fn f(x: bool) {\n    if a() && b() {\n        c();\n    }\n    let _ = x || y();\n}\nfn g() {\n    if a() {}\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    assert!(
        common::fn_block(&page, "f").contains(". Not drawn: 2 &&/||.\n"),
        "{page}"
    );
    assert!(common::fn_block(&page, "g").contains(". Not drawn: none.\n"));
}

#[test]
fn a_while_header_runs_on_every_iteration() {
    let src = "fn f(n: u8) {\n    while more() {\n        if n > 1 {\n            continue;\n        }\n        step();\n    }\n    done();\n}\nfn g(it: I) {\n    while let Some(x) = it.next() {\n        use_it(x);\n    }\n}\nfn h() {\n    for x in make() {\n        use_it(x);\n    }\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    assert_eq!(
        chart_edges(&page, "f"),
        [
            "f --> more()",
            "more() --> while more()",
            "while more() -loop-> n #gt; 1",
            "n #gt; 1 -yes-> continue",
            "continue --> more()",
            "n #gt; 1 -no-> step()",
            "step() --> more()",
            "while more() -done-> done()",
            "done() --> end",
        ]
    );
    assert_eq!(
        chart_edges(&page, "g"),
        [
            "g --> it.next()",
            "it.next() --> while let Some(x) = it.next()",
            "while let Some(x) = it.next() -loop-> use_it(x)",
            "use_it(x) --> it.next()",
            "while let Some(x) = it.next() -done-> end",
        ]
    );
    assert_eq!(
        chart_edges(&page, "h"),
        [
            "h --> make()",
            "make() --> for x in make()",
            "for x in make() -loop-> use_it(x)",
            "use_it(x) --> for x in make()",
            "for x in make() -done-> end",
        ]
    );
    assert_eq!(
        common::fn_section(&page, "f", "Calls"),
        [
            "`more()` L2 → outside the project",
            "`step()` L6 → outside the project",
            "`done()` L8 → outside the project",
        ]
    );
}

#[test]
fn and_in_a_let_chain_is_stated_on_the_coverage_line() {
    let src = "fn h(o: Option<u8>) {\n    if let Some(v) = o && ok(v) {\n        a(v);\n    }\n    while let Some(w) = o && ok(w) && w > 1 {\n        a(w);\n    }\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    assert!(
        common::fn_block(&page, "h").contains(". Not drawn: 3 &&/||.\n"),
        "{page}"
    );
}

#[test]
fn a_closure_first_in_a_while_header_or_else_body_is_not_where_the_path_goes() {
    let src = "fn f(v: u8) {\n    while any_of(|x| { check(x) }, v) {\n        if v > 1 {\n            continue;\n        }\n        go();\n    }\n}\nfn g(n: u8) {\n    if n > 1 {\n        a();\n    } else { // keep\n        let r = || { b() };\n        r();\n    }\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    let call = "any_of(#124;x#124; #123; check(x) #125;, v)";
    let head = format!("while {call}");
    assert_eq!(
        chart_edges(&page, "f"),
        [
            "closure L2 --> check(x)".to_string(),
            "check(x) --> end".to_string(),
            format!("f --> {call}"),
            format!("{call} --> {head}"),
            format!("{head} -loop-> v #gt; 1"),
            "v #gt; 1 -yes-> continue".to_string(),
            format!("continue --> {call}"),
            "v #gt; 1 -no-> go()".to_string(),
            format!("go() --> {call}"),
            format!("{head} -done-> end"),
        ]
    );
    assert_eq!(
        chart_edges(&page, "g"),
        [
            "g --> n #gt; 1",
            "n #gt; 1 -yes-> a()",
            "r --> b()",
            "b() --> end",
            "n #gt; 1 -no-> r()",
            "r() -.- keep",
            "a() --> end",
            "r() --> end",
        ]
    );
}

#[test]
fn an_endless_loop_followed_by_code_has_an_exit_not_drawn() {
    let src = "async fn f(mut rx: R, mut tick: T) {\n    loop {\n        tokio::select! {\n            msg = rx.recv() => {\n                match msg {\n                    Some(m) => { handle(m).await; }\n                    None => break,\n                }\n            }\n            _ = tick.tick() => { flush().await; }\n        }\n    }\n    finish().await;\n}\nfn g() {\n    loop {\n        tick();\n    }\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    let f = chart_edges(&page, "f");
    assert!(
        f.contains(&"loop -exit not drawn-> finish()".to_string()),
        "{f:#?}"
    );
    assert!(f.contains(&"finish() --> end".to_string()), "{f:#?}");
    assert_eq!(
        chart_edges(&page, "g"),
        ["g --> loop", "loop -loop-> tick()", "tick() --> loop"]
    );
}

#[test]
fn an_exit_inside_a_closure_or_async_block_is_not_the_functions() {
    let src = "async fn f(p: P) {\n    for t in [\"a\", \"b\"] {\n        if !has(t).await {\n            q(t).execute(p).await.unwrap_or_else(|e| panic!(\"add {t}: {e}\"));\n        }\n    }\n    done();\n}\nfn s(rx: R) {\n    spawn(async move {\n        if rx.closed() {\n            return;\n        }\n        work();\n    });\n    after();\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    let f = chart_edges(&page, "f");
    assert!(!f.iter().any(|e| e.contains("> panic!")), "{f:#?}");
    assert!(f.contains(&"q(t).execute(p).await.unwrap_or_else(#124;e#124; panic!(#quot;add #123;t#125;: #123;e#125;#quot;)) --> for t in [#quot;a#quot;, #quot;b#quot;]".to_string()), "{f:#?}");
    assert!(
        f.contains(&"for t in [#quot;a#quot;, #quot;b#quot;] -done-> done()".to_string()),
        "{f:#?}"
    );
    assert!(f.contains(&"done() --> end".to_string()), "{f:#?}");
    let spawn = "spawn(async move #123; if rx.closed() #123; return; #125; work(); #125;)";
    assert_eq!(
        chart_edges(&page, "s"),
        [
            "async L10 --> rx.closed()".to_string(),
            "rx.closed() --> rx.closed()".to_string(),
            "rx.closed() -yes-> return".to_string(),
            "rx.closed() -no-> work()".to_string(),
            "work() --> end".to_string(),
            format!("s --> {spawn}"),
            format!("{spawn} --> after()"),
            "after() --> end".to_string(),
        ]
    );
}

fn edges_of(src: &str, anchor: &str) -> Vec<String> {
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    chart_edges(&common::map_page(&vault, "map/src/a_rs"), anchor)
}

#[test]
fn a_loop_in_an_expression_closure_keeps_its_break_and_continue() {
    let src = "fn f() {\n    spawn(move || loop { if stop() { break; } step(); });\n    after();\n}\nfn m(xs: &[u8]) {\n    each(xs, |x| for y in 0..*x { if y == 1 { continue; } g(y); });\n}\n";
    let spawn = "spawn(move #124;#124; loop #123; if stop() #123; break; #125; step(); #125;)";
    assert_eq!(
        edges_of(src, "f"),
        [
            "closure L2 --> loop".to_string(),
            "loop -loop-> stop()".to_string(),
            "stop() --> stop()".to_string(),
            "stop() -yes-> break".to_string(),
            "stop() -no-> step()".to_string(),
            "step() --> loop".to_string(),
            "break --> end".to_string(),
            format!("f --> {spawn}"),
            format!("{spawn} --> after()"),
            "after() --> end".to_string(),
        ]
    );
    let each =
        "each(xs, #124;x#124; for y in 0..*x #123; if y == 1 #123; continue; #125; g(y); #125;)";
    assert_eq!(
        edges_of(src, "m"),
        [
            "closure L6 --> for y in 0..*x".to_string(),
            "for y in 0..*x -loop-> y == 1".to_string(),
            "y == 1 -yes-> continue".to_string(),
            "continue --> for y in 0..*x".to_string(),
            "y == 1 -no-> g(y)".to_string(),
            "g(y) --> for y in 0..*x".to_string(),
            "for y in 0..*x -done-> end".to_string(),
            format!("m --> {each}"),
            format!("{each} --> end"),
        ]
    );
}

#[test]
fn a_hidden_exit_of_a_loop_ending_an_arm_reaches_the_code_after_the_branch() {
    let src = "async fn f(c: bool, mut rx: R) {\n    if c {\n        loop {\n            tokio::select! {\n                m = rx.recv() => { if m.is_none() { break; } }\n            }\n        }\n    }\n    after();\n}\nasync fn g(k: u8, mut rx: R) {\n    match k {\n        1 => loop {\n            tokio::select! {\n                m = rx.recv() => { if m.is_none() { break; } }\n            }\n        },\n        _ => other(),\n    }\n    after();\n}\nasync fn h(xs: &[u8], mut rx: R) {\n    for x in xs {\n        if *x > 1 {\n            loop {\n                tokio::select! {\n                    m = rx.recv() => { if m.is_none() { break; } }\n                }\n            }\n        }\n    }\n}\nfn e(c: bool) {\n    if c {\n        loop {\n            tick();\n        }\n    }\n}\nfn d(c: bool) {\n    if c {\n        loop {\n            tick();\n        }\n    }\n    after();\n}\n";
    assert_eq!(
        edges_of(src, "f"),
        [
            "f --> c",
            "c -yes-> loop",
            "loop -loop-> rx.recv()",
            "rx.recv() --> m.is_none()",
            "m.is_none() --> loop",
            "loop -exit not drawn-> after()",
            "c -no-> after()",
            "after() --> end",
        ]
    );
    assert_eq!(
        edges_of(src, "g"),
        [
            "g --> match k",
            "match k --> 1",
            "1 -yes-> loop",
            "loop -loop-> rx.recv()",
            "rx.recv() --> m.is_none()",
            "m.is_none() --> loop",
            "1 -no-> _",
            "_ -yes-> other()",
            "loop -exit not drawn-> after()",
            "other() --> after()",
            "after() --> end",
        ]
    );
    // Going round the enclosing loop again is code that follows.
    assert_eq!(
        edges_of(src, "h"),
        [
            "h --> for x in xs",
            "for x in xs -loop-> *x #gt; 1",
            "*x #gt; 1 -yes-> loop",
            "loop -loop-> rx.recv()",
            "rx.recv() --> m.is_none()",
            "m.is_none() --> loop",
            "loop -exit not drawn-> for x in xs",
            "*x #gt; 1 -no-> for x in xs",
            "for x in xs -done-> end",
        ]
    );
    // No macro names `break`: endless, whatever follows the branch.
    assert_eq!(
        edges_of(src, "d"),
        [
            "d --> c",
            "c -yes-> loop",
            "loop -loop-> tick()",
            "tick() --> loop",
            "c -no-> after()",
            "after() --> end",
        ]
    );
    // Nothing follows anywhere: endless.
    assert_eq!(
        edges_of(src, "e"),
        [
            "e --> c",
            "c -yes-> loop",
            "loop -loop-> tick()",
            "tick() --> loop",
            "c -no-> end",
        ]
    );
}

#[test]
fn a_nested_item_after_an_endless_loop_is_not_code_that_follows() {
    let src = "fn f() {\n    loop {\n        if x() {\n            return;\n        }\n        step();\n    }\n    fn helper() {\n        h();\n    }\n}\n";
    assert_eq!(
        edges_of(src, "f"),
        [
            "f --> loop",
            "loop -loop-> x()",
            "x() --> x()",
            "x() -yes-> return",
            "x() -no-> step()",
            "step() --> loop",
            "helper --> h()",
            "h() --> end",
        ]
    );
}

#[test]
fn an_async_block_awaited_in_place_is_on_the_path() {
    let src = "async fn f() {\n    let v = async { compute().await }.await;\n    use_it(v);\n}\nasync fn s() {\n    let fut = async { later().await };\n    now();\n    fut.await;\n}\n";
    assert_eq!(
        edges_of(src, "f"),
        [
            "f --> compute()",
            "compute() --> use_it(v)",
            "use_it(v) --> end"
        ]
    );
    assert_eq!(
        edges_of(src, "s"),
        [
            "async L6 --> later()",
            "later() --> end",
            "s --> now()",
            "now() --> end",
        ]
    );
}

#[test]
fn exits_inside_a_closure_stay_inside_it() {
    let src = "fn f(xs: &[u8]) {\n    'o: for x in xs {\n        let ok = all(xs, |y| { if y > x { return false; } true });\n        if ok { continue; }\n        each(|y| loop { if y { break 'o; } t(); });\n    }\n}\nfn p(o: Option<u8>) {\n    let n = o.map(|x| match x { 1 => keep(), _ => panic!(\"no\") });\n    after(n);\n}\n";
    let all = "all(xs, #124;y#124; #123; if y #gt; x #123; return false; #125; true #125;)";
    let each = "each(#124;y#124; loop #123; if y #123; break 'o; #125; t(); #125;)";
    assert_eq!(
        edges_of(src, "f"),
        [
            "f --> 'o: for x in xs".to_string(),
            "closure L3 --> y #gt; x".to_string(),
            "y #gt; x -yes-> return false".to_string(),
            "y #gt; x -no-> end".to_string(),
            format!("'o: for x in xs -loop-> {all}"),
            format!("{all} --> ok"),
            "ok -yes-> continue".to_string(),
            "continue --> 'o: for x in xs".to_string(),
            "closure L5 --> loop".to_string(),
            "loop -loop-> y".to_string(),
            "y -yes-> break 'o".to_string(),
            "y -no-> t()".to_string(),
            "t() --> loop".to_string(),
            format!("ok -no-> {each}"),
            format!("{each} --> 'o: for x in xs"),
            "'o: for x in xs -done-> end".to_string(),
        ]
    );
    let map =
        "o.map(#124;x#124; match x #123; 1 =#gt; keep(), _ =#gt; panic!(#quot;no#quot;) #125;)";
    assert_eq!(
        edges_of(src, "p"),
        [
            "closure L9 --> match x".to_string(),
            "match x --> 1".to_string(),
            "1 -yes-> keep()".to_string(),
            "1 -no-> _".to_string(),
            "_ -yes-> panic!(#quot;no#quot;)".to_string(),
            "keep() --> end".to_string(),
            format!("p --> {map}"),
            format!("{map} --> after(n)"),
            "after(n) --> end".to_string(),
        ]
    );
}

#[test]
fn a_hidden_exit_reaches_the_end_of_its_flow_and_is_found_by_token() {
    let src = "async fn run(mut stop: R, mut rx: R) {\n    loop {\n        tokio::select! {\n            _ = stop.recv() => break,\n            m = rx.recv() => handle(m),\n        }\n    }\n}\nfn s(mut cancel: R) {\n    spawn(async move { loop { tokio::select! { _ = cancel.recv() => break, } } });\n}\nfn w(c: bool, breaker: B) {\n    if c {\n        loop {\n            // break on shutdown\n            log!(\"break here\", \"break\");\n            mac! { breaker.trip() /* break */ }\n        }\n    }\n    after();\n}\nasync fn o(mut rx: R) {\n    loop {\n        loop {\n            tokio::select! { m = rx.recv() => { if m.is_none() { break; } } }\n        }\n        if done() {\n            return;\n        }\n    }\n}\n";
    // The inner loop's hidden `break` is no exit of the loop around it.
    assert_eq!(
        edges_of(src, "o"),
        [
            "o --> loop",
            "loop -loop-> loop",
            "loop -loop-> rx.recv()",
            "rx.recv() --> m.is_none()",
            "m.is_none() --> loop",
            "loop -exit not drawn-> done()",
            "done() --> done()",
            "done() -yes-> return",
            "done() -no-> loop",
        ]
    );
    assert_eq!(
        edges_of(src, "run"),
        [
            "run --> loop",
            "loop -loop-> stop.recv()",
            "stop.recv() --> rx.recv()",
            "rx.recv() --> handle(m)",
            "handle(m) --> loop",
            "loop -exit not drawn-> end",
        ]
    );
    let spawn = "spawn(async move #123; loop #123; tokio::select! #123; _ = cancel.recv() =#gt; break, #125; #125; #125;)";
    assert_eq!(
        edges_of(src, "s"),
        [
            "async L10 --> loop".to_string(),
            "loop -loop-> cancel.recv()".to_string(),
            "cancel.recv() --> loop".to_string(),
            "loop -exit not drawn-> end".to_string(),
            format!("s --> {spawn}"),
            format!("{spawn} --> end"),
        ]
    );
    // The word in a string, a comment or a longer name is no `break`.
    assert_eq!(
        edges_of(src, "w"),
        [
            "w --> c",
            "c -yes-> loop",
            "loop -loop-> breaker.trip()",
            "breaker.trip() --> loop",
            "c -no-> after()",
            "after() --> end",
        ]
    );
}

#[test]
fn python_calls_to_parameters_and_nested_defs_are_hidden() {
    let a = "def decide(messages, ask):\n    return ask(messages)\n\ndef outer(x):\n    def inner(y):\n        return y\n    return inner(x)\n\ndef ask(m):\n    pass\n\ndef inner(y):\n    pass\n";
    let (_d, vault) = common::built(&[("a.py", a)]);
    let page = common::map_page(&vault, "map/a_py");
    assert_eq!(
        common::fn_section(&page, "decide", "Calls"),
        ["`ask(messages)` L2 → target not visible"]
    );
    assert_eq!(
        common::fn_section(&page, "outer", "Calls"),
        ["`inner(x)` L7 → target not visible"]
    );
}

#[test]
fn python_module_calls_are_outside_or_resolved() {
    let a = "import json\nimport os.path\nfrom .b import helper\nfrom . import b\nimport pkg.c\nfrom .b import Store\n\n\ndef run(s):\n    json.loads(s)\n    helper()\n    b.helper()\n    os.path.join(s)\n    len(s)\n    Store()\n    pkg.c.go()\n    t = Store()\n    t.get()\n    open(s)\n\ndef go():\n    pass\n\ndef body_import():\n    from .c import go\n    go()\n";
    let b = "def helper():\n    pass\n\nclass Store:\n    def get(self):\n        pass\n";
    let c = "def go():\n    pass\n\ndef open(p):\n    pass\n";
    let files = [
        ("pkg/__init__.py", ""),
        ("pkg/a.py", a),
        ("pkg/b.py", b),
        ("pkg/c.py", c),
    ];
    let (_d, vault) = common::built(&files);
    let page = common::map_page(&vault, "map/pkg/a_py");
    assert_eq!(
        common::fn_section(&page, "run", "Calls"),
        [
            "`json.loads(s)` L10 → outside the project",
            "`helper()` L11 → [[map/pkg/b_py#helper|helper]]",
            "`b.helper()` L12 → [[map/pkg/b_py#helper|helper]]",
            "`os.path.join(s)` L13 → outside the project",
            "`len(s)` L14 → outside the project",
            "`Store()` L15 → target not visible",
            "`pkg.c.go()` L16 → [[map/pkg/c_py#go|go]]",
            "`Store()` L17 → target not visible",
            "`t.get()` L18 → [[map/pkg/b_py#store_get|Store::get]]",
            "`open(s)` L19 → one of: [[map/pkg/c_py#open|open]], or outside the project",
        ]
    );
    // An import in the body names something else than the file's `go`.
    assert_eq!(
        common::fn_section(&page, "body_import", "Calls"),
        ["`go()` L26 → target not visible"]
    );
}

#[test]
fn python_self_call_with_a_subclass_override_is_one_of() {
    let a = "class A:\n    def run(self):\n        self.step()\n        self.only()\n        self.missing()\n\n    def step(self):\n        pass\n\n    def only(self):\n        pass\n\nclass B(A):\n    def step(self):\n        pass\n";
    let (_d, vault) = common::built(&[("a.py", a)]);
    assert_eq!(
        common::fn_section(&common::map_page(&vault, "map/a_py"), "a_run", "Calls"),
        [
            "`self.step()` L3 → one of: [[map/a_py#a_step|A::step]], [[map/a_py#b_step|B::step]]",
            "`self.only()` L4 → [[map/a_py#a_only|A::only]]",
            "`self.missing()` L5 → outside the project",
        ]
    );
}

#[test]
fn python_nested_defs_have_no_section_of_their_own() {
    let a = "def f():\n    def inner():\n        pass\n    inner()\n";
    let (_d, vault) = common::built(&[("a.py", a)]);
    let page = common::map_page(&vault, "map/a_py");
    assert_eq!(headings(&page), ["f"]);
    assert!(!page.contains("Not covered yet"), "{page}");
}

const PY_F: &str = "def f(x, ask):\n    # Tag-12. Retries only on 5xx.\n    if x > 500:\n        ask(x)\n    elif x == 404:\n        raise NotFound()\n    else:\n        pass\n    def inner(y):\n        if y:\n            return log(y)\n    match x:\n        case 1 | 2:\n            g()\n        case _:\n            pass\n    return inner(x) if x else None\n";
const PY_TRY: &str = "def k(xs):\n    for x in xs:\n        if x:\n            break\n    else:\n        none()\n    try:\n        return a(\"|<x>|\")\n    except (E1, E2) as e:\n        b(e)\n    else:\n        c()\n    finally:\n        d()\n";
const PY_CHART: &str = "def f(x):\n    while True:\n        if poll():\n            break\n    try:\n        return load(x)\n    except ValueError:\n        pass\n    match x:\n        case 1:\n            one()\n        case _:\n            other()\n";

#[test]
fn python_try_while_true_and_a_wildcard_case_draw_only_edges_the_code_can_take() {
    let (_d, vault) = common::built(&[("a.py", PY_CHART)]);
    assert_eq!(
        chart_edges(&common::map_page(&vault, "map/a_py"), "f"),
        [
            "f --> while True",
            "while True -loop-> poll()",
            "poll() --> poll()",
            "poll() -yes-> break",
            "poll() -no-> while True",
            "break --> try",
            "try -yes-> load(x)",
            "load(x) --> return load(x)",
            "try -no-> except ValueError",
            "except ValueError -yes-> match x",
            "match x --> 1",
            "1 -yes-> one()",
            "1 -no-> _",
            "_ -yes-> other()",
            "one() --> end",
            "other() --> end",
        ]
    );
}

#[test]
fn a_python_import_named_like_a_file_of_another_language_is_outside() {
    let a = "import json\nfrom time import sleep\n\ndef f(s):\n    json.loads(s)\n    sleep(1)\n";
    let files = [
        ("a.py", a),
        ("src/json.rs", "pub fn loads() {}\n"),
        ("src/time.rs", "pub fn sleep() {}\n"),
    ];
    let (_d, vault) = common::built(&files);
    assert_eq!(
        common::fn_section(&common::map_page(&vault, "map/a_py"), "f", "Calls"),
        [
            "`json.loads(s)` L5 → outside the project",
            "`sleep(1)` L6 → outside the project",
        ]
    );
}

#[test]
fn a_python_module_global_named_like_a_module_is_not_that_module() {
    let a = "config = load()\n\ndef f():\n    config.get()\n";
    let files = [("a.py", a), ("config.py", "def get():\n    pass\n")];
    let (_d, vault) = common::built(&files);
    assert_eq!(
        common::fn_section(&common::map_page(&vault, "map/a_py"), "f", "Calls"),
        ["`config.get()` L4 → outside the project"]
    );
}

#[test]
fn a_python_library_import_sharing_a_submodule_name_is_outside() {
    let main = "import os.path\nfrom requests.utils import quote\nfrom django.db.models import Q\n\ndef f(s):\n    quote(s)\n    Q(s)\n    os.path.join(s)\n";
    let files = [
        ("app/__init__.py", ""),
        ("app/main.py", main),
        ("app/utils.py", "def quote(s):\n    pass\n"),
        ("app/models.py", "def Q(s):\n    pass\n"),
        ("app/path.py", "def join(a):\n    pass\n"),
    ];
    assert_eq!(
        calls_in(&files, "map/app/main_py", "f"),
        [
            "`quote(s)` L6 → outside the project",
            "`Q(s)` L7 → outside the project",
            "`os.path.join(s)` L8 → outside the project",
        ]
    );
}

#[test]
fn a_python_import_rooted_in_a_project_package_is_the_projects() {
    let main = "import lib.deep.c\nfrom lib.deep import c as cc\n\ndef f():\n    lib.deep.c.go()\n    cc.go()\n";
    let files = [
        ("main.py", main),
        ("lib/deep/c.py", "def go():\n    pass\n"),
    ];
    assert_eq!(
        calls_in(&files, "map/main_py", "f"),
        [
            "`lib.deep.c.go()` L5 → [[map/lib/deep/c_py#go|go]]",
            "`cc.go()` L6 → [[map/lib/deep/c_py#go|go]]",
        ]
    );
}

#[test]
fn a_python_name_imported_from_one_module_is_looked_up_there() {
    let a =
        "from .mod import shared\nfrom . import util\n\ndef f():\n    shared()\n    util.other()\n";
    let files = [
        ("pkg/__init__.py", ""),
        ("pkg/a.py", a),
        ("pkg/mod.py", "def shared():\n    pass\n"),
        (
            "pkg/util.py",
            "def shared():\n    pass\n\ndef other():\n    pass\n",
        ),
    ];
    assert_eq!(
        calls_in(&files, "map/pkg/a_py", "f"),
        [
            "`shared()` L5 → [[map/pkg/mod_py#shared|shared]]",
            "`util.other()` L6 → [[map/pkg/util_py#other|other]]",
        ]
    );
}

#[test]
fn a_python_module_imported_by_name_is_that_file() {
    let shared = "def shared():\n    pass\n";
    let files = [
        ("pkg/__init__.py", ""),
        ("pkg/mod.py", shared),
        ("top/__init__.py", ""),
        ("top/mod.py", shared),
        ("pkg/a.py", "from pkg import mod\n\ndef f():\n    mod.shared()\n"),
        ("top/b.py", "from . import mod\n\ndef f():\n    mod.shared()\n"),
        (
            "c.py",
            "import pkg.mod\nimport top.mod as tm\n\ndef f():\n    pkg.mod.shared()\n    tm.shared()\n",
        ),
        ("d.py", "import mod\n\ndef f():\n    mod.shared()\n"),
    ];
    assert_eq!(
        calls_in(&files, "map/pkg/a_py", "f"),
        ["`mod.shared()` L4 → [[map/pkg/mod_py#shared|shared]]"]
    );
    assert_eq!(
        calls_in(&files, "map/top/b_py", "f"),
        ["`mod.shared()` L4 → [[map/top/mod_py#shared|shared]]"]
    );
    assert_eq!(
        calls_in(&files, "map/c_py", "f"),
        [
            "`pkg.mod.shared()` L5 → [[map/pkg/mod_py#shared|shared]]",
            "`tm.shared()` L6 → [[map/top/mod_py#shared|shared]]",
        ]
    );
    // A module inside a package is imported through the package.
    assert_eq!(
        calls_in(&files, "map/d_py", "f"),
        ["`mod.shared()` L4 → outside the project"]
    );
}

#[test]
fn python_class_relative_calls_walk_the_project_bases() {
    let base = "class Base:\n    def inherited(self):\n        pass\n\n    def hook(self):\n        pass\n";
    let a = "from base import Base\nfrom lib import Thing\n\nclass Child(Base):\n    def run(self):\n        self.inherited()\n        super().inherited()\n        self.hook()\n\n    @classmethod\n    def make(cls):\n        cls.build()\n\n    @classmethod\n    def build(cls):\n        pass\n\nclass Grand(Child):\n    def hook(self):\n        pass\n\n    def go(self):\n        self.inherited()\n\nclass Ext(Thing, Base):\n    def go(self):\n        self.inherited()\n\nclass Near:\n    def near(self):\n        pass\n\nclass Same(Near):\n    def go(self):\n        self.near()\n";
    let files = [("base.py", base), ("a.py", a)];
    let inherited = "[[map/base_py#base_inherited|Base::inherited]]";
    assert_eq!(
        calls_in(&files, "map/a_py", "child_run"),
        [
            format!("`self.inherited()` L6 → {inherited}"),
            "`super()` L7 → outside the project".to_string(),
            format!("`super().inherited()` L7 → {inherited}"),
            "`self.hook()` L8 → one of: [[map/a_py#grand_hook|Grand::hook]], [[map/base_py#base_hook|Base::hook]]".to_string(),
        ]
    );
    assert_eq!(
        calls_in(&files, "map/a_py", "child_make"),
        ["`cls.build()` L12 → [[map/a_py#child_build|Child::build]]"]
    );
    // Through two levels.
    assert_eq!(
        calls_in(&files, "map/a_py", "grand_go"),
        [format!("`self.inherited()` L23 → {inherited}")]
    );
    // A base from outside the project, named first, may have the method.
    assert_eq!(
        calls_in(&files, "map/a_py", "ext_go"),
        [format!(
            "`self.inherited()` L27 → one of: {inherited}, or outside the project"
        )]
    );
    assert_eq!(
        calls_in(&files, "map/a_py", "same_go"),
        ["`self.near()` L35 → [[map/a_py#near_near|Near::near]]"]
    );
}

#[test]
fn a_python_subclass_through_an_aliased_base_import_still_overrides() {
    let a =
        "class X:\n    def run(self):\n        self.step()\n\n    def step(self):\n        pass\n";
    let z = "from a import X as Base\n\nclass Z(Base):\n    def step(self):\n        pass\n\n    def go(self):\n        self.run()\n";
    let files = [("a.py", a), ("z.py", z)];
    assert_eq!(
        calls_in(&files, "map/a_py", "x_run"),
        ["`self.step()` L3 → one of: [[map/a_py#x_step|X::step]], [[map/z_py#z_step|Z::step]]"]
    );
    assert_eq!(
        calls_in(&files, "map/z_py", "z_go"),
        ["`self.run()` L8 → [[map/a_py#x_run|X::run]]"]
    );
}

#[test]
fn python_calls_are_listed_in_source_order_through_a_try_else() {
    let a = "def h():\n    try:\n        a()\n    except E:\n        b()\n    else:\n        c()\n    finally:\n        d()\n    for x in xs():\n        e()\n    else:\n        g()\n";
    let lines: Vec<String> = calls_in(&[("a.py", a)], "map/a_py", "h")
        .iter()
        .map(|l| l.split(" → ").next().unwrap().to_string())
        .collect();
    assert_eq!(
        lines,
        [
            "`a()` L3",
            "`b()` L5",
            "`c()` L7",
            "`d()` L9",
            "`xs()` L10",
            "`e()` L11",
            "`g()` L13"
        ]
    );
}

#[test]
fn a_python_loop_else_is_on_the_done_path_and_break_skips_it() {
    let a = "def f(rows):\n    for r in rows:\n        for c in r:\n            if hit(c):\n                break\n        else:\n            continue\n        found(r)\n";
    assert_eq!(
        edges_of_py(a, "f"),
        [
            "f --> for r in rows",
            "for r in rows -loop-> for c in r",
            "for c in r -loop-> hit(c)",
            "hit(c) --> hit(c)",
            "hit(c) -yes-> break",
            "hit(c) -no-> for c in r",
            "for c in r -done-> continue",
            "continue --> for r in rows",
            "break --> found(r)",
            "found(r) --> for r in rows",
            "for r in rows -done-> end",
        ]
    );
    let page = page_of_py(a);
    assert!(!page.contains("subgraph"), "{page}");
    assert!(page.contains("Not drawn: none."), "{page}");
}

#[test]
fn a_python_finally_is_on_every_path_that_falls_into_it() {
    let plain = "def w():\n    try:\n        work()\n    finally:\n        close()\n    after()\n";
    assert_eq!(
        edges_of_py(plain, "w"),
        [
            "w --> work()",
            "work() --> close()",
            "close() --> after()",
            "after() --> end",
        ]
    );
    // An exit in the `finally` ends the path: nothing leads to `after()`.
    let exit = "def g():\n    try:\n        pass\n    finally:\n        return z()\n    after()\n";
    assert_eq!(
        edges_of_py(exit, "g"),
        ["g --> z()", "z() --> return z()", "after() --> end"]
    );
    let full = "def h(x):\n    try:\n        a()\n    except E:\n        b()\n    except F:\n        return c()\n    else:\n        d()\n    finally:\n        e()\n    g()\n";
    assert_eq!(
        edges_of_py(full, "h"),
        [
            "h --> try",
            "try -yes-> a()",
            "a() --> d()",
            "try -no-> except E",
            "except E -yes-> b()",
            "except E -no-> except F",
            "except F -yes-> c()",
            "c() --> return c()",
            "d() --> e()",
            "b() --> e()",
            "e() --> g()",
            "g() --> end",
        ]
    );
    for src in [plain, exit, full] {
        let page = page_of_py(src);
        assert!(!page.contains("subgraph"), "{page}");
        assert!(page.contains("Not drawn: 1 try."), "{page}");
    }
}

#[test]
fn a_python_finally_no_path_falls_into_is_drawn_beside_the_path() {
    let a = "def k():\n    try:\n        return a()\n    finally:\n        b()\n";
    let page = page_of_py(a);
    assert_eq!(
        edges_of_py(a, "k"),
        [
            "k --> a()",
            "a() --> return a()",
            "finally L4 --> b()",
            "b() --> end"
        ]
    );
    assert!(page.contains("subgraph n3 [\"finally L4\"]"), "{page}");
}

#[test]
fn a_python_class_defined_in_the_function_is_not_visible() {
    let a = "def f():\n    class Local:\n        def m(self):\n            pass\n    Local().m()\n";
    assert_eq!(
        calls_in(&[("a.py", a)], "map/a_py", "f"),
        [
            "`Local()` L5 → target not visible",
            "`Local().m()` L5 → target not visible",
        ]
    );
}

#[test]
fn a_python_keyword_case_is_not_a_catch_all() {
    let a = "def f(x):\n    match x:\n        case 1:\n            one()\n        case None:\n            other()\n    after()\n";
    // `None` is a keyword, not a capture: it can fail to match, so it has a `-no->` edge.
    let edges = edges_of_py(a, "f");
    assert_eq!(
        edges,
        [
            "f --> match x",
            "match x --> 1",
            "1 -yes-> one()",
            "1 -no-> None",
            "None -yes-> other()",
            "one() --> after()",
            "other() --> after()",
            "None -no-> after()",
            "after() --> end",
        ]
    );
    let b = a.replace("case None", "case rest");
    let edges = edges_of_py(&b, "f");
    // A bare name matches anything: no `-no->` edge and `after()` is reached only through the bodies.
    assert_eq!(
        edges,
        [
            "f --> match x",
            "match x --> 1",
            "1 -yes-> one()",
            "1 -no-> rest",
            "rest -yes-> other()",
            "one() --> after()",
            "other() --> after()",
            "after() --> end",
        ]
    );
}

fn page_of_py(src: &str) -> String {
    let (_d, vault) = common::built(&[("a.py", src)]);
    common::map_page(&vault, "map/a_py")
}

fn edges_of_py(src: &str, anchor: &str) -> Vec<String> {
    chart_edges(&page_of_py(src), anchor)
}

#[test]
fn a_python_import_named_like_a_module_of_an_unrelated_directory_is_not_certain() {
    let logging = "def getLogger(n):\n    pass\n";
    let a = "import logging\nimport helpers\n\ndef f(n):\n    logging.getLogger(n)\n    logging.info(n)\n    helpers.getLogger(n)\n";
    let files = [
        ("app/logging.py", logging),
        ("pkg/a.py", a),
        ("pkg/helpers.py", logging),
        (
            "app/b.py",
            "import logging\n\ndef f(n):\n    logging.getLogger(n)\n",
        ),
    ];
    // Nothing puts `app/` on the import path of `pkg/a.py`.
    assert_eq!(
        calls_in(&files, "map/pkg/a_py", "f"),
        [
            "`logging.getLogger(n)` L5 → one of: [[map/app/logging_py#getlogger|getLogger]], or outside the project",
            "`logging.info(n)` L6 → outside the project",
            "`helpers.getLogger(n)` L7 → [[map/pkg/helpers_py#getlogger|getLogger]]",
        ]
    );
    assert_eq!(
        calls_in(&files, "map/app/b_py", "f"),
        ["`logging.getLogger(n)` L4 → [[map/app/logging_py#getlogger|getLogger]]"]
    );
}

#[test]
fn a_python_name_a_package_reexports_is_found_through_its_init() {
    let files = [
        ("top/__init__.py", "from .core import run\nfrom .star import *\n"),
        ("top/core.py", "def run():\n    pass\n"),
        ("top/star.py", "def starred():\n    pass\n"),
        ("other/core.py", "def run():\n    pass\n\ndef starred():\n    pass\n"),
        (
            "a.py",
            "import top\nfrom top import run as go, starred\n\ndef f():\n    top.run()\n    go()\n    starred()\n",
        ),
    ];
    assert_eq!(
        calls_in(&files, "map/a_py", "f"),
        [
            "`top.run()` L5 → [[map/top/core_py#run|run]]",
            "`go()` L6 → [[map/top/core_py#run|run]]",
            "`starred()` L7 → [[map/top/star_py#starred|starred]]",
        ]
    );
}

#[test]
fn a_python_name_its_module_does_not_define_is_not_a_namesake_elsewhere() {
    let files = [
        ("pkg/__init__.py", ""),
        ("pkg/mod.py", "def other():\n    pass\n"),
        ("top/mod.py", "def helper():\n    pass\n"),
        (
            "pkg/a.py",
            "from .mod import helper\nfrom . import mod\n\ndef f():\n    helper()\n    mod.helper()\n",
        ),
    ];
    assert_eq!(
        calls_in(&files, "map/pkg/a_py", "f"),
        [
            "`helper()` L5 → target not visible",
            "`mod.helper()` L6 → target not visible",
        ]
    );
}

#[test]
fn a_python_class_of_two_modules_is_the_imported_one() {
    let user = "class User:\n    def save(self):\n        pass\n";
    let files = [
        ("pkg/__init__.py", ""),
        ("pkg/models.py", user),
        ("top/models.py", user),
        (
            "pkg/a.py",
            "from .models import User\n\ndef f(u: User):\n    u.save()\n    User.save(u)\n",
        ),
    ];
    let save = "[[map/pkg/models_py#user_save|User::save]]";
    assert_eq!(
        calls_in(&files, "map/pkg/a_py", "f"),
        [
            format!("`u.save()` L4 → {save}"),
            format!("`User.save(u)` L5 → {save}"),
        ]
    );
}

#[test]
fn a_python_alias_imported_in_a_body_hides_the_name_it_stands_for() {
    let a = "from .b import go\n\ndef go_local():\n    pass\n\ndef f():\n    from .c import go as go_local\n    go_local()\n    import c as b\n    b.go()\n";
    let go = "def go():\n    pass\n";
    let files = [("pkg/a.py", a), ("pkg/b.py", go), ("pkg/c.py", go)];
    assert_eq!(
        calls_in(&files, "map/pkg/a_py", "f"),
        [
            "`go_local()` L8 → target not visible",
            "`b.go()` L10 → one of: [[map/pkg/c_py#go|go]], or outside the project",
        ]
    );
}

#[test]
fn python_super_under_an_outside_base_is_outside_and_a_static_cls_is_a_value() {
    let a = "class Base:\n    def __init__(self):\n        pass\n\n    def build(self):\n        pass\n\nclass Err(Exception):\n    def __init__(self):\n        super().__init__()\n\nclass Mixin:\n    def setup(self):\n        super().__init__()\n\nclass S(Base):\n    @staticmethod\n    def helper(cls):\n        cls.build()\n";
    let files = [("a.py", a)];
    assert_eq!(
        calls_in(&files, "map/a_py", "err_init"),
        [
            "`super()` L10 → outside the project",
            "`super().__init__()` L10 → outside the project",
        ]
    );
    // A mixin's `super()` is whatever class it is mixed into.
    assert_eq!(
        calls_in(&files, "map/a_py", "mixin_setup")[1],
        "`super().__init__()` L14 → one of: [[map/a_py#base_init|Base::__init__]], [[map/a_py#err_init|Err::__init__]], or outside the project"
    );
    assert_eq!(
        calls_in(&files, "map/a_py", "s_helper"),
        ["`cls.build()` L19 → one of: [[map/a_py#base_build|Base::build]], or outside the project"]
    );
}

#[test]
fn a_python_directory_inside_a_package_is_no_import_root() {
    let y = "from requests.utils import quote\nfrom app.missing import load\nimport email\nimport app.gone.utils\n\ndef f(o):\n    quote(o)\n    load(o)\n    email.message_from_string(o)\n    app.gone.utils.quote(o)\n";
    let files = [
        ("app/__init__.py", ""),
        ("app/requests/__init__.py", ""),
        (
            "app/email/__init__.py",
            "def message_from_string(s):\n    pass\n",
        ),
        ("app/utils.py", "def quote(s):\n    pass\n"),
        ("app/helpers.py", "def load(o):\n    pass\n"),
        ("svc/y.py", y),
        (
            "other/z.py",
            "from requests.utils import quote\n\ndef f(o):\n    quote(o)\n",
        ),
    ];
    assert_eq!(
        calls_in(&files, "map/svc/y_py", "f"),
        [
            "`quote(o)` L7 → outside the project",
            "`load(o)` L8 → target not visible",
            "`email.message_from_string(o)` L9 → outside the project",
            "`app.gone.utils.quote(o)` L10 → target not visible",
        ]
    );
    assert_eq!(
        calls_in(&files, "map/other/z_py", "f"),
        ["`quote(o)` L4 → outside the project"]
    );
}

#[test]
fn a_python_module_in_a_package_does_not_shadow_a_library() {
    let x =
        "import json\nimport logging\n\ndef f(o):\n    json.dumps(o)\n    logging.getLogger(o)\n";
    let y = "import logging\nimport types\n\ndef f(o):\n    logging.info(o)\n    types.SimpleNamespace(o)\n";
    let files = [
        ("app/__init__.py", ""),
        ("app/json.py", "def dumps(o):\n    pass\n"),
        ("app/logging.py", "def getLogger(n):\n    pass\n"),
        ("app/types/__init__.py", ""),
        ("app/x.py", x),
        ("app/api/__init__.py", ""),
        ("app/api/y.py", y),
    ];
    assert_eq!(
        calls_in(&files, "map/app/x_py", "f"),
        [
            "`json.dumps(o)` L5 → outside the project",
            "`logging.getLogger(o)` L6 → outside the project",
        ]
    );
    assert_eq!(
        calls_in(&files, "map/app/api/y_py", "f"),
        [
            "`logging.info(o)` L5 → outside the project",
            "`types.SimpleNamespace(o)` L6 → outside the project",
        ]
    );
}

#[test]
fn python_super_under_project_bases_ending_in_object_is_outside() {
    let a = "class P:\n    def run(self):\n        pass\n\nclass Q(P):\n    def __init__(self):\n        super().__init__()\n\nclass S(object):\n    def __init__(self):\n        super().__init__()\n";
    let files = [("a.py", a)];
    assert_eq!(
        calls_in(&files, "map/a_py", "q_init")[1],
        "`super().__init__()` L7 → outside the project"
    );
    assert_eq!(
        calls_in(&files, "map/a_py", "s_init")[1],
        "`super().__init__()` L11 → outside the project"
    );
}

#[test]
fn a_python_module_alias_keeps_its_links_through_dotted_calls() {
    let models = "class User:\n    @classmethod\n    def make(cls):\n        pass\n\n    def run(self):\n        pass\n\ndef func():\n    pass\n\nCONST = User()\n";
    let main = "import app.models as am\nimport shapes as sh\nfrom app import models as mm\nimport app as a\n\ndef f():\n    am.User.make()\n    sh.Sq.new()\n    mm.User.make()\n    am.func()\n    am.CONST.run()\n    mm.User().run()\n    a.models.User.make()\n";
    let w =
        "import utils\nfrom app import utils as au\n\ndef f():\n    au.u()\n    utils.dumps()\n";
    let files = [
        ("app/__init__.py", ""),
        ("app/models.py", models),
        ("app/utils.py", "def u():\n    pass\n"),
        ("shapes.py", "class Sq:\n    def new(self):\n        pass\n"),
        ("main.py", main),
        ("w.py", w),
    ];
    let make = "[[map/app/models_py#user_make|User::make]]";
    assert_eq!(
        calls_in(&files, "map/main_py", "f"),
        [
            format!("`am.User.make()` L7 → {make}"),
            "`sh.Sq.new()` L8 → [[map/shapes_py#sq_new|Sq::new]]".to_string(),
            format!("`mm.User.make()` L9 → {make}"),
            "`am.func()` L10 → [[map/app/models_py#func|func]]".to_string(),
            "`am.CONST.run()` L11 → target not visible".to_string(),
            "`mm.User()` L12 → target not visible".to_string(),
            "`mm.User().run()` L12 → one of: [[map/app/models_py#user_run|User::run]], or outside the project".to_string(),
            format!("`a.models.User.make()` L13 → {make}"),
        ]
    );
    assert_eq!(
        calls_in(&files, "map/w_py", "f"),
        [
            "`au.u()` L5 → [[map/app/utils_py#u|u]]",
            "`utils.dumps()` L6 → outside the project",
        ]
    );
}

#[test]
fn an_empty_file_at_a_page_path_is_rewritten() {
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    common::write_files(&src, &[("src/a.rs", "fn f() {}\n")]);
    common::write_files(&vault, &[("map/src/a_rs.md", "")]);
    cli_build(&src, &vault)
        .success()
        .stdout("map: 1 written, 0 unchanged, 0 removed\n")
        .stderr("");
    assert!(common::map_page(&vault, "map/src/a_rs").contains("## f\n"));
}

#[cfg(unix)]
#[test]
fn a_link_at_the_temporary_path_is_not_written_through() {
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    common::write_files(&src, &[("src/a.rs", "fn f() {}\n")]);
    let outside = d.path().join("outside.txt");
    std::fs::write(&outside, "mine").unwrap();
    std::fs::create_dir_all(vault.join("map/src")).unwrap();
    std::os::unix::fs::symlink(&outside, vault.join("map/src/.a_rs.md.plaz-tmp")).unwrap();
    cli_build(&src, &vault).success();
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "mine");
    let page = vault.join("map/src/a_rs.md");
    assert!(page.symlink_metadata().unwrap().file_type().is_file());
}

#[test]
fn a_leftover_temporary_file_is_removed_and_no_temporary_file_stays() {
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    common::write_files(&src, &[("src/a.rs", "fn f() {}\n")]);
    common::write_files(
        &vault,
        &[("map/old/.gone_rs.md.plaz-tmp", "---\ntitle: \"go")],
    );
    cli_build(&src, &vault)
        .success()
        .stdout("map: 1 written, 0 unchanged, 0 removed\n");
    assert!(!vault.join("map/old").exists());
    let names: Vec<_> = std::fs::read_dir(vault.join("map/src"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["a_rs.md"]);
}

#[test]
fn a_note_with_kind_map_but_no_plaz_tag_survives_a_build() {
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    common::write_files(&src, &[("src/a.rs", "fn f() {}\n")]);
    let note = "---\r\ntitle: My maps\r\nkind: map\r\n---\r\nmine\r\n";
    common::write_files(&vault, &[("map/my-note.md", note)]);
    cli_build(&src, &vault)
        .success()
        .stdout("map: 1 written, 0 unchanged, 0 removed\n");
    assert_eq!(
        std::fs::read_to_string(vault.join("map/my-note.md")).unwrap(),
        note
    );
}

#[test]
fn a_tree_with_no_code_file_warns_and_leaves_the_map_as_it_is() {
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    common::write_files(&src, &[("src/a.rs", "fn f() {}\n")]);
    cli_build(&src, &vault).success();
    std::fs::remove_file(src.join("src/a.rs")).unwrap();
    common::write_files(&src, &[("README.md", "x\n")]);
    cli_build(&src, &vault)
        .success()
        .stdout("map: 0 written, 0 unchanged, 0 removed\n")
        .stderr(format!(
            "warning: no code file found in {}, the map is left as it is\n",
            src.display()
        ));
    assert!(vault.join("map/src/a_rs.md").exists());
}

#[cfg(unix)]
#[test]
fn an_unreadable_directory_under_map_is_skipped_with_a_warning() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    common::write_files(&src, &[("src/a.rs", "fn f() {}\n")]);
    common::write_files(&vault, &[("map/locked/x.md", "x\n")]);
    let locked = vault.join("map/locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert!(
        std::fs::read_dir(&locked).is_err(),
        "needs a user that cannot list a mode 000 directory (not root)"
    );
    let out = cli_build(&src, &vault).success();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    out.stdout("map: 1 written, 0 unchanged, 0 removed\n")
        .stderr(format!(
            "warning: {}: not readable, left alone\n",
            locked.display()
        ));
    assert!(locked.join("x.md").exists());
}

#[test]
fn the_cli_prints_help_and_version_and_rejects_bad_usage() {
    let usage = "usage: plaz build [--source DIR] [--vault DIR] | plaz setup [--source DIR] --scope repo|local [--remove]\n";
    // Help wording is not pinned. What is: every spelling succeeds quietly, opens with
    // the usage line (the same line bad usage prints on stderr) and prints one text.
    // What each flag does is covered by the setup tests and the build tests.
    let mut helps = Vec::new();
    for args in [
        &["--help"][..],
        &["-h"],
        &["help"],
        &["build", "--help"],
        &["setup", "--help"],
        &["build", "-h"],
    ] {
        let out = assert_cmd::cargo_bin_cmd!()
            .args(args)
            .assert()
            .success()
            .stderr("")
            .get_output()
            .stdout
            .clone();
        assert!(out.starts_with(usage.as_bytes()), "{args:?}");
        helps.push(out);
    }
    assert!(helps.windows(2).all(|w| w[0] == w[1]), "{helps:?}");
    assert_cmd::cargo_bin_cmd!()
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("plaz {}\n", env!("CARGO_PKG_VERSION")));
    for bad in [
        &["nope"][..],
        &["build", "--source"],
        &["build", "--what", "x"],
    ] {
        assert_cmd::cargo_bin_cmd!()
            .args(bad)
            .assert()
            .code(2)
            .stdout("")
            .stderr(usage);
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        assert_cmd::cargo_bin_cmd!()
            .arg(std::ffi::OsStr::from_bytes(b"bu\xffild"))
            .assert()
            .code(2)
            .stderr(usage);
    }
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    common::write_files(&src, &[("src/a.rs", "fn f() {}\n")]);
    assert_cmd::cargo_bin_cmd!()
        .arg("build")
        .arg(format!("--source={}", src.display()))
        .arg(format!("--vault={}", vault.display()))
        .assert()
        .success()
        .stdout("map: 1 written, 0 unchanged, 0 removed\n");
}

#[test]
fn a_function_with_a_20000_term_expression_chain_builds() {
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    let body = vec!["g()"; 20_000].join(" + ");
    let a = format!("fn g() -> u8 {{ 1 }}\nfn f() -> u8 {{\n    {body}\n}}\n");
    common::write_files(&src, &[("src/a.rs", &a)]);
    cli_build(&src, &vault).success();
    assert!(common::map_page(&vault, "map/src/a_rs").contains("## f\n"));
}

#[test]
fn a_page_says_when_module_level_code_or_test_code_is_not_charted() {
    let script = "import os\n\"\"\"Doc.\"\"\"\nLIMIT = 3\ndata = load(os.environ)\n\ndef main():\n    pass\n\nif __name__ == \"__main__\":\n    main()\n";
    let plain = "\"\"\"Doc.\"\"\"\nfrom os import path\nimport json as j\nLIMIT: int = 3\nNAMES = [\"a\", (1, 2)]\nKEYS = {\"a\": -1}\n\n@deco\ndef f():\n    go()\n\nclass C:\n    def m(self):\n        pass\n";
    let it = "fn helper(x: u8) {\n    if x > 1 {}\n}\n#[test]\nfn t() {\n    helper(2);\n}\n";
    let lib = "pub fn f() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
    let (_d, vault) = common::built(&[
        ("script.py", script),
        ("call.py", "setup()\n"),
        ("plain.py", plain),
        ("pkg/test_thing.py", "def test_a():\n    assert go()\n"),
        ("tests/it.rs", it),
        ("src/lib.rs", lib),
        ("src/a.rs", "pub fn g() {}\n"),
    ]);
    let module = "\nModule-level code is not charted.\n";
    let tests = |n: &str| format!("\nTest code is not charted: {n}.\n");
    for (page, has_module, test_line) in [
        ("map/script_py", true, None),
        ("map/call_py", true, None),
        ("map/plain_py", false, None),
        ("map/pkg/test_thing_py", false, Some("1 function")),
        ("map/tests/it_rs", false, Some("2 functions")),
        ("map/src/lib_rs", false, Some("1 function")),
        ("map/src/a_rs", false, None),
    ] {
        let text = common::map_page(&vault, page);
        assert_eq!(text.contains(module), has_module, "{page}:\n{text}");
        match test_line {
            Some(n) => assert!(text.contains(&tests(n)), "{page}:\n{text}"),
            None => assert!(!text.contains("Test code"), "{page}:\n{text}"),
        }
    }
}

#[test]
fn build_leaves_a_compiled_wiki_alone() {
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    common::write_files(&src, &[("src/a.rs", "pub fn f() {}\n")]);
    let wiki = [
        ("index.md", "# Index\n"),
        ("AGENTS.md", "agents\n"),
        ("index.json", "{\"pages\": []}\n"),
        (".wiki/cache.json", "{\"v\": 1}\n"),
    ];
    common::write_files(&vault, &wiki);
    let w = plaz::build(&src, &vault).unwrap();
    assert_eq!((w.written, w.removed, w.left_alone), (1, 0, 0));
    for (path, text) in wiki {
        assert_eq!(std::fs::read_to_string(vault.join(path)).unwrap(), text);
    }
}

const KILN_H: &str = "class Kiln {\npublic:\n    Kiln();\n    void fire(int t);\n    void vent();\n    int heat() const {\n        if (hot_) { return 9; }\n        return 0;\n    }\nprivate:\n    bool hot_;\n};\n";
const KILN_CPP: &str = "#include \"kiln.h\"\nstatic int budget = start_budget();\nKiln::Kiln() : hot_(false) {}\nvoid Kiln::fire(int t) {\n    if (t == 3) { hot_ = true; }\n}\n";

#[test]
fn c_and_cpp_functions_are_charted_in_sources_and_headers() {
    let (_d, vault) = common::built(&[("src/kiln.h", KILN_H), ("src/kiln.cpp", KILN_CPP)]);
    let cpp = common::map_page(&vault, "map/src/kiln_cpp");
    assert!(!cpp.contains("Not covered yet"), "{cpp}");
    assert_eq!(headings(&cpp), ["kiln_kiln", "kiln_fire"]);
    let fire = common::fn_block(&cpp, "kiln_fire");
    assert!(fire.contains("    n1{\"t == 3\"}\n"), "{fire}");
    assert!(
        fire.contains(
            "\nCoverage: 0 calls: 0 resolved, 0 one of, 0 outside, 0 hidden. Not drawn: none.\n"
        ),
        "{fire}"
    );
    let h = common::map_page(&vault, "map/src/kiln_h");
    let heat = common::fn_block(&h, "kiln_heat");
    assert!(heat.contains("    n1{\"hot_\"}\n"), "{heat}");
}

#[test]
fn a_declaration_links_its_definition_or_says_there_is_none() {
    let (_d, vault) = common::built(&[("src/kiln.h", KILN_H), ("src/kiln.cpp", KILN_CPP)]);
    let h = common::map_page(&vault, "map/src/kiln_h");
    assert_eq!(
        headings(&h),
        ["kiln_kiln", "kiln_fire", "kiln_vent", "kiln_heat"]
    );
    assert_eq!(
        common::fn_block(&h, "kiln_fire"),
        "## kiln_fire\n`void fire(int t)` · `src/kiln.h:4-4`\n\nDefined at [[map/src/kiln_cpp#kiln_fire|Kiln::fire]]\n\n"
    );
    assert_eq!(
        common::fn_block(&h, "kiln_vent"),
        "## kiln_vent\n`void vent()` · `src/kiln.h:5-5`\n\nNo definition in the project.\n\n"
    );
    let cpp = common::map_page(&vault, "map/src/kiln_cpp");
    assert!(cpp.contains("\n## kiln_fire\n"), "{cpp}");
}

#[test]
fn a_constructor_says_constructions_are_not_traced_and_file_level_calls_are_noted() {
    let (_d, vault) = common::built(&[("src/kiln.h", KILN_H), ("src/kiln.cpp", KILN_CPP)]);
    let cpp = common::map_page(&vault, "map/src/kiln_cpp");
    assert_eq!(
        common::fn_section(&cpp, "kiln_kiln", "Called from"),
        ["constructions and operator uses are not traced"]
    );
    assert_eq!(
        common::fn_section(&cpp, "kiln_fire", "Called from"),
        ["none found"]
    );
    let module = "\nModule-level code is not charted.\n";
    assert!(cpp.contains(module), "{cpp}");
    assert!(!common::map_page(&vault, "map/src/kiln_h").contains(module));
}

#[test]
fn a_prototype_links_its_definition_in_the_same_file() {
    let calc = "static int clamp(int v);\nint run(int v) { return clamp(v); }\nstatic int clamp(int v) { return v > 9 ? 9 : v; }\n";
    // A same-named function elsewhere is not the prototype's definition.
    let other = "static int clamp(int v) { return v; }\n";
    let (_d, vault) = common::built(&[("src/calc.c", calc), ("src/other.c", other)]);
    let page = common::map_page(&vault, "map/src/calc_c");
    assert_eq!(headings(&page), ["clamp", "run", "clamp_2"]);
    assert_eq!(
        common::fn_block(&page, "clamp"),
        "## clamp\n`static int clamp(int v)` · `src/calc.c:1-1`\n\nDefined at [[map/src/calc_c#clamp_2|clamp]]\n\n"
    );
}

#[test]
fn same_arity_overloads_the_index_cannot_pair_are_one_of_their_definitions() {
    let h = "class Pan {\npublic:\n    void heat(int t);\n    void heat(double t);\n};\n";
    let cpp = "#include \"pan.h\"\nvoid Pan::heat(int t) { warm(t); }\nvoid Pan::heat(double t) { warm(t); }\n";
    let (_d, vault) = common::built(&[("src/pan.h", h), ("src/pan.cpp", cpp)]);
    let header = common::map_page(&vault, "map/src/pan_h");
    let one_of = "Defined at one of: [[map/src/pan_cpp#pan_heat|Pan::heat]], [[map/src/pan_cpp#pan_heat_2|Pan::heat]]";
    for (anchor, sig, line) in [
        ("pan_heat", "void heat(int t)", 3),
        ("pan_heat_2", "void heat(double t)", 4),
    ] {
        assert_eq!(
            common::fn_block(&header, anchor).trim_end(),
            format!("## {anchor}\n`{sig}` · `src/pan.h:{line}-{line}`\n\n{one_of}")
        );
    }
    assert_eq!(
        headings(&common::map_page(&vault, "map/src/pan_cpp")),
        ["pan_heat", "pan_heat_2"]
    );
}

#[test]
fn a_function_both_trees_list_is_one_call_target_with_a_heading() {
    let src = "#ifdef WIDE\nlong span(long a) {\n#else\nint span(int a) {\n#endif\n    return grow(a);\n}\nint use_it() { return span(1); }\n";
    let (_d, vault) = common::built(&[("a.cpp", src)]);
    let page = common::map_page(&vault, "map/a_cpp");
    assert_eq!(headings(&page), ["span", "use_it"]);
    assert_eq!(
        common::fn_section(&page, "use_it", "Calls"),
        ["`span(1)` L8 → [[map/a_cpp#span|span]]"]
    );
}

#[test]
fn a_defaulted_or_deleted_function_says_so_and_has_no_chart() {
    let h = "class Pot {\npublic:\n    Pot() = default;\n    Pot(const Pot&) = delete;\n    int size() const { return 1; }\n};\n";
    let (_d, vault) = common::built(&[("src/pot.h", h)]);
    let page = common::map_page(&vault, "map/src/pot_h");
    assert_eq!(headings(&page), ["pot_pot", "pot_pot_2", "pot_size"]);
    assert_eq!(
        common::fn_block(&page, "pot_pot"),
        "## pot_pot\n`Pot() = default` · `src/pot.h:3-3`\n\nDefaulted.\n\n"
    );
    assert_eq!(
        common::fn_block(&page, "pot_pot_2"),
        "## pot_pot_2\n`Pot(const Pot&) = delete` · `src/pot.h:4-4`\n\nDeleted.\n\n"
    );
}

#[test]
fn a_call_of_a_jumping_macro_from_another_file_is_on_the_coverage_line() {
    let guard = "#define RETURN_UNLESS(c) do { if (!(c)) return; } while (0)\n";
    let src = "void feed(int* p, int a) {\n    RETURN_UNLESS(a > 0);\n    use(p);\n}\n";
    let (_d, vault) = common::built(&[("inc/guard.h", guard), ("src/a.cpp", src)]);
    let feed = common::fn_block(&common::map_page(&vault, "map/src/a_cpp"), "feed");
    let coverage: Vec<&str> = feed
        .lines()
        .filter(|l| l.starts_with("Coverage: "))
        .collect();
    assert_eq!(
        coverage,
        ["Coverage: 2 calls: 0 resolved, 0 one of, 1 outside, 1 hidden. Not drawn: 1 macro jump."]
    );
}

/// Each call of the charted function `name` in `path`, in source order,
/// with its target: `path:line` of the target item, `one of: …`, `outside`
/// or `hidden`.
fn cpp_targets(files: &[(&str, &str)], path: &str, name: &str) -> Vec<String> {
    use plaz::model::Target;
    let sources = files.iter().map(|(p, t)| (p.to_string(), t.to_string()));
    let resolved = plaz::resolve::resolve(plaz::source::project(sources));
    let p = resolved.project();
    let at = |id: &plaz::model::FnId| {
        let line = p.files[&id.path].code.items[id.item].start_line;
        format!("{}:{line}", id.path)
    };
    let f = &p.files[path];
    let (_, flow) = (f.flows.iter())
        .find(|(&i, _)| f.code.items[i].name == name)
        .unwrap_or_else(|| panic!("no charted {name} in {path}"));
    let mut out = Vec::new();
    flow.each_call(&mut |_, c| {
        let target = match &c.target {
            Target::Resolved(id) => at(id),
            Target::OneOf {
                candidates,
                outside,
            } => {
                let all: Vec<String> = candidates.iter().map(at).collect();
                let or = if *outside { ", or outside" } else { "" };
                format!("one of: {}{or}", all.join(", "))
            }
            Target::Outside => "outside".to_string(),
            Target::Hidden => "hidden".to_string(),
        };
        out.push(format!("{} → {target}", c.text));
    });
    out
}

#[test]
fn a_cpp_call_of_a_bound_name_is_hidden_beside_a_same_named_function() {
    let src = "int pick(int v) { return v; }\nint run(int (*pick)(int)) { return pick(2); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.c", src)], "src/a.c", "run"),
        ["pick(2) → hidden"]
    );
}

#[test]
fn a_c_definition_in_the_callers_file_wins_over_an_included_declaration() {
    // Valid C: one function, its header declaration spelled differently.
    let h = "int count(char *argv[]);\n";
    let src = "#include \"count.h\"\nint count(char **argv) { return argv != 0; }\nint run(char **v) { return count(v); }\n";
    assert_eq!(
        cpp_targets(&[("src/count.h", h), ("src/a.c", src)], "src/a.c", "run"),
        ["count(v) → src/a.c:2"]
    );
}

#[test]
fn a_cpp_callers_own_overload_and_an_included_one_are_one_candidate_set() {
    let h = "int step(double d);\n";
    let src = "#include \"step.h\"\nstatic int step(int a) { return a; }\nint run() { return step(1.5); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/step.h", h), ("src/step.cpp", src)],
            "src/step.cpp",
            "run"
        ),
        ["step(1.5) → one of: src/step.cpp:2, src/step.h:1"]
    );
}

#[test]
fn a_forward_declaration_folds_onto_its_definition_in_the_same_file() {
    // REPORT T12, first bullet.
    let src = "static int twice(int);\nint run(int a) { return twice(a); }\nstatic int twice(int a) { return a * 2; }\n";
    assert_eq!(
        cpp_targets(&[("src/a.c", src)], "src/a.c", "run"),
        ["twice(a) → src/a.c:3"]
    );
}

#[test]
fn a_header_declaration_folds_onto_its_definition_with_names_and_defaults_dropped() {
    let h = "int scale(const struct grid* g, const std::vector<enum mode>& ms, int value, int by = 2);\n";
    let def =
        "#include \"scale.h\"\nint scale(const grid *g, const std::vector<mode> &m, const int v, int k) { return v * k; }\n";
    let main = "#include \"scale.h\"\nint run() { return scale(nullptr, {}, 3); }\n";
    assert_eq!(
        cpp_targets(
            &[
                ("src/scale.h", h),
                ("src/scale.cpp", def),
                ("src/main.cpp", main)
            ],
            "src/main.cpp",
            "run"
        ),
        ["scale(nullptr, {}, 3) → src/scale.cpp:2"]
    );
}

#[test]
fn a_declaration_does_not_fold_onto_a_definition_of_other_parameter_types() {
    let h = "void put(int n);\n";
    let def = "void put(double d) {}\n";
    let main = "#include \"put.h\"\nvoid run() { put(1); }\n";
    assert_eq!(
        cpp_targets(
            &[
                ("src/put.h", h),
                ("src/put.cpp", def),
                ("src/main.cpp", main)
            ],
            "src/main.cpp",
            "run"
        ),
        ["put(1) → src/put.h:1"]
    );
}

#[test]
fn a_function_in_the_include_closure_is_certain_when_one_and_one_of_when_several() {
    let top = "#include \"../lib/mid.h\"\n";
    let mid = "#include \"deep.h\"\nint pair_of(int a);\n";
    let deep = "int sink(int a);\nint pair_of(long a);\n";
    // The definitions, in a file no include reaches; it includes the
    // declaring header, so they are the declaration's.
    let elsewhere = "#include \"../lib/deep.h\"\nint sink(int a) { return a; }\nint sink(long a) { return 1; }\n";
    let src = "#include \"top.h\"\nint run() { return sink(1) + pair_of(2); }\n";
    assert_eq!(
        cpp_targets(
            &[
                ("app/top.h", top),
                ("lib/mid.h", mid),
                ("lib/deep.h", deep),
                ("other/sink.c", elsewhere),
                // Same suffix as the include, not the file it names.
                ("vendor/lib/mid.h", "int sink(short a);\n"),
                ("app/main.c", src),
            ],
            "app/main.c",
            "run"
        ),
        [
            "sink(1) → other/sink.c:2",
            "pair_of(2) → one of: lib/deep.h:2, lib/mid.h:2"
        ]
    );
}

#[test]
fn a_static_inline_helper_in_a_header_is_certain_only_where_the_header_is_included() {
    let h = "static inline int clampi(int v) { return v < 0 ? 0 : v; }\n";
    let with = "#include \"util.h\"\nint run(int v) { return clampi(v); }\n";
    let without = "int other(int v) { return clampi(v); }\n";
    let files = [
        ("src/util.h", h),
        ("src/with.c", with),
        ("src/without.c", without),
    ];
    assert_eq!(
        cpp_targets(&files, "src/with.c", "run"),
        ["clampi(v) → src/util.h:1"]
    );
    assert_eq!(
        cpp_targets(&files, "src/without.c", "other"),
        ["clampi(v) → outside"]
    );
}

#[test]
fn a_project_function_no_include_reaches_may_be_a_c_library_name() {
    // REPORT T13.
    let net = "#include <unistd.h>\nvoid hang_up(int fd) { close(fd); }\n";
    let pool = "void close(int slot) {}\n";
    assert_eq!(
        cpp_targets(
            &[("src/net.cpp", net), ("src/pool.cpp", pool)],
            "src/net.cpp",
            "hang_up"
        ),
        ["close(fd) → one of: src/pool.cpp:1, or outside"]
    );
}

#[test]
fn a_static_function_of_another_file_is_no_candidate() {
    // REPORT T12, second bullet.
    let a = "static int twice(int a) { return a * 2; }\n";
    let b = "int run() { return twice(1); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.c", a), ("src/b.c", b)], "src/b.c", "run"),
        ["twice(1) → outside"]
    );
}

#[test]
fn a_function_in_an_anonymous_namespace_of_another_file_is_no_candidate() {
    let a = "namespace {\nint twice(int a) { return a * 2; }\n}\nint keep() { return twice(2); }\n";
    let b = "int run() { return twice(1); }\n";
    let files = [("src/a.cpp", a), ("src/b.cpp", b)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "keep"),
        ["twice(2) → src/a.cpp:2"]
    );
    assert_eq!(
        cpp_targets(&files, "src/b.cpp", "run"),
        ["twice(1) → outside"]
    );
}

#[test]
fn a_variadic_function_is_a_candidate_for_any_argument_count() {
    // REPORT T12, third bullet.
    let src = "int say(const char* f, ...) { return 0; }\nint run() { return say(\"a\", 1, 2); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.c", src)], "src/a.c", "run"),
        ["say(\"a\", 1, 2) → src/a.c:1"]
    );
}

#[test]
fn a_bare_call_of_a_project_macro_or_type_is_hidden_anything_else_outside() {
    let h = "#define CHECKED(x) (x)\nstruct Point { int x; };\n";
    let src = "#include \"defs.h\"\nint run() { CHECKED(1); Point(2); return puts(\"x\"); }\n";
    assert_eq!(
        cpp_targets(&[("inc/defs.h", h), ("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "CHECKED(1) → hidden",
            "Point(2) → hidden",
            "puts(\"x\") → outside"
        ]
    );
}

#[test]
fn a_function_a_project_macro_generates_is_outside() {
    // A known limitation pinned on purpose: no item stands for `get_depth`.
    let src = "#define DEFINE_GETTER(x) int get_##x() { return 0; }\nDEFINE_GETTER(depth)\nint run() { return get_depth(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["get_depth() → outside"]
    );
}

#[test]
fn a_std_call_is_outside_and_a_root_the_project_does_not_have_is_outside() {
    let src = "int run() { std::abs(1); return fmtlib::print(2); }\n";
    // A same-named project function changes neither, nor a project
    // specialization in `namespace std`.
    let other = "int print(int a) { return a; }\nint abs(int a) { return a; }\nnamespace std {\ntemplate <> struct hash<int>;\n}\n";
    assert_eq!(
        cpp_targets(
            &[("src/a.cpp", src), ("src/print.cpp", other)],
            "src/a.cpp",
            "run"
        ),
        ["std::abs(1) → outside", "fmtlib::print(2) → outside"]
    );
}

#[test]
fn a_call_qualified_by_a_project_type_is_its_member_not_dispatched() {
    let h = "struct Base {\n    virtual void fill(int n);\n};\nstruct Child : Base {\n    void fill(int n) override { Base::fill(n); }\n};\n";
    let cpp = "#include \"base.h\"\nvoid Base::fill(int k) {}\n";
    assert_eq!(
        cpp_targets(
            &[("src/base.h", h), ("src/base.cpp", cpp)],
            "src/base.h",
            "fill"
        ),
        ["Base::fill(n) → src/base.cpp:2"]
    );
}

#[test]
fn a_call_qualified_by_a_project_namespace_reaches_its_free_function() {
    let h = "namespace core {\nint run(int n);\n}\n";
    let def = "#include \"core.h\"\nint core::run(int count) { return count; }\n";
    let main = "#include \"core.h\"\nint start() { return core::run(1); }\n";
    assert_eq!(
        cpp_targets(
            &[
                ("src/core.h", h),
                ("src/core.cpp", def),
                ("src/main.cpp", main)
            ],
            "src/main.cpp",
            "start"
        ),
        ["core::run(1) → src/core.cpp:2"]
    );
}

#[test]
fn a_prototype_offers_no_static_definition_of_another_file() {
    let proto = "int measure(int v);\nint run(int v) { return measure(v); }\n";
    let shared = "int measure(int v) { return v; }\n";
    let hidden = "static int measure(int v) { return -v; }\n";
    let (_d, vault) = common::built(&[
        ("src/proto.c", proto),
        ("src/shared.c", shared),
        ("src/hidden.c", hidden),
    ]);
    assert_eq!(
        common::fn_block(&common::map_page(&vault, "map/src/proto_c"), "measure"),
        "## measure\n`int measure(int v)` · `src/proto.c:1-1`\n\nDefined at [[map/src/shared_c#measure|measure]]\n\n"
    );
}

#[test]
fn a_headers_call_of_a_static_inline_in_a_header_it_does_not_include_may_be_it() {
    let b = "static inline int halve(int v) { return v / 2; }\n";
    let a = "static inline int quarter(int v) { return halve(halve(v)); }\n";
    let src = "int run(int v) { return halve(v); }\n";
    let files = [("inc/b.h", b), ("inc/a.h", a), ("src/run.c", src)];
    assert_eq!(
        cpp_targets(&files, "inc/a.h", "quarter"),
        [
            "halve(v) → one of: inc/b.h:1, or outside",
            "halve(halve(v)) → one of: inc/b.h:1, or outside"
        ]
    );
    assert_eq!(
        cpp_targets(&files, "src/run.c", "run"),
        ["halve(v) → outside"]
    );
}

#[test]
fn a_call_qualified_by_a_template_parameter_is_any_project_method_of_that_name() {
    let h = "struct Cell {\n    static int make(int n);\n};\n";
    let src = "#include \"cell.h\"\ntemplate <typename T>\nint build(int n) { return T::make(n) + T::drop(n) + Cell::make(n) + std::abs(n); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/cell.h", h), ("src/build.cpp", src)],
            "src/build.cpp",
            "build"
        ),
        [
            "T::make(n) → one of: src/cell.h:2, or outside",
            "T::drop(n) → outside",
            "Cell::make(n) → src/cell.h:2",
            "std::abs(n) → outside"
        ]
    );
}

#[test]
fn a_call_qualified_by_a_block_scope_alias_is_any_project_method_of_that_name() {
    let h = "template <typename T>\nstruct Box {\n    static T fill(T v);\n};\n";
    let src = "#include \"box.h\"\nint run() {\n    using X = Box<int>;\n    return X::fill(1) + Box<int>::fill(2);\n}\n";
    assert_eq!(
        cpp_targets(
            &[("src/box.h", h), ("src/run.cpp", src)],
            "src/run.cpp",
            "run"
        ),
        [
            "X::fill(1) → one of: src/box.h:3, or outside",
            "Box<int>::fill(2) → src/box.h:3"
        ]
    );
}

#[test]
fn a_bare_call_reaches_a_namespace_function_only_from_that_namespace_or_a_using() {
    let h = "namespace io {\nint close(int fd);\nstruct Pipe { void drain(int fd); };\n}\n";
    let shut = "#include <unistd.h>\n#include \"fdutil.h\"\nvoid shut(int fd) { close(fd); }\nnamespace io {\nvoid again(int fd) { close(fd); }\n}\nvoid io::Pipe::drain(int fd) { close(fd); }\n";
    let used = "#include \"fdutil.h\"\nusing namespace io;\nvoid done(int fd) { close(fd); }\n";
    let files = [
        ("src/fdutil.h", h),
        ("src/shut.cpp", shut),
        ("src/used.cpp", used),
    ];
    let decl = "close(fd) → src/fdutil.h:2";
    assert_eq!(
        cpp_targets(&files, "src/shut.cpp", "shut"),
        ["close(fd) → one of: src/fdutil.h:2, or outside"]
    );
    assert_eq!(cpp_targets(&files, "src/shut.cpp", "again"), [decl]);
    assert_eq!(cpp_targets(&files, "src/shut.cpp", "drain"), [decl]);
    assert_eq!(cpp_targets(&files, "src/used.cpp", "done"), [decl]);
}

#[test]
fn an_angle_include_with_a_directory_reaches_a_project_header() {
    let h = "static inline int clampi(int v) { return v < 0 ? 0 : v; }\n";
    let with = "#include <proj/util.h>\nint run(int v) { return clampi(v); }\n";
    let bare = "#include <util.h>\nint other(int v) { return clampi(v); }\n";
    let files = [
        ("include/proj/util.h", h),
        ("src/angle.c", with),
        ("src/bare.c", bare),
    ];
    assert_eq!(
        cpp_targets(&files, "src/angle.c", "run"),
        ["clampi(v) → include/proj/util.h:1"]
    );
    assert_eq!(
        cpp_targets(&files, "src/bare.c", "other"),
        ["clampi(v) → outside"]
    );
}

#[test]
fn a_forward_declaration_spelled_otherwise_folds_onto_the_files_one_definition() {
    let c = "static int count(char *argv[]);\nint run(char **v) { return count(v); }\nstatic int count(char **argv) { return argv != 0; }\n";
    // C++: the definition of the same arity, not the overload.
    let cpp = "int tally(int v[]);\nint run(int *v) { return tally(v); }\nint tally(int *v) { return *v; }\nint tally() { return 0; }\n";
    assert_eq!(
        cpp_targets(&[("src/fwd.c", c)], "src/fwd.c", "run"),
        ["count(v) → src/fwd.c:3"]
    );
    assert_eq!(
        cpp_targets(&[("src/fwd.cpp", cpp)], "src/fwd.cpp", "run"),
        ["tally(v) → src/fwd.cpp:3"]
    );
}

#[test]
fn a_namespace_named_like_inline_keeps_its_name() {
    let h = "namespace inline_math {\nint sq(int v);\n}\n";
    let src = "#include \"m.h\"\nint run() { return inline_math::sq(2); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/m.h", h), ("src/run.cpp", src)],
            "src/run.cpp",
            "run"
        ),
        ["inline_math::sq(2) → src/m.h:2"]
    );
}

#[test]
fn a_cpp_lambda_capturing_by_default_calls_a_local_of_the_enclosing_function_unseen() {
    let src = "int pick() { return 0; }\nint handler(int v) { return v; }\nvoid run() {\n    auto handler = pick();\n    auto go = [&] { handler(1); };\n    go();\n}\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "pick() → src/a.cpp:1",
            "handler(1) → hidden",
            "go() → hidden"
        ]
    );
}

#[test]
fn a_cpp_method_reaches_members_free_functions_and_typed_receivers() {
    // REPORT T11, with names of its own.
    let h = "#pragma once\nnamespace acct { namespace ledger {\nint tally(int x);\nint pick(int a);\nint pick(double a);\nint pick(int a, int b);\nclass Ledger {\npublic:\n    virtual void post(int q);\n    void close_day(int q, int (*hook)(int));\n    void settle(int n);\nprivate:\n    void recount_();\n};\nstruct Branch : public Ledger {\n    void post(int q) override;\n};\n}}\n";
    let src = "#include \"ledger.h\"\n#include \"util/note.h\"\n#include <fmtx/core.h>\n#include <algorithm>\nnamespace acct { namespace ledger {\nvoid Ledger::recount_() {}\nvoid Ledger::post(int q) {}\nvoid Branch::post(int q) { Ledger::post(q); }\nint tally(int x) { return x; }\nint pick(int a) { return a; }\nint pick(double a) { return 0; }\nint pick(int a, int b) { return a + b; }\nvoid Ledger::settle(int n) {}\nvoid Ledger::close_day(int q, int (*hook)(int)) {\n    auto dbl = [](int v) { return v; };\n    Ledger& other = *this;\n    Branch c;\n    recount_();\n    this->recount_();\n    post(q);\n    dbl(3);\n    hook(7);\n    tally(q);\n    util::note(\"day\");\n    acct::ledger::tally(1);\n    std::sort(nullptr, nullptr);\n    fmtx::print(\"x\");\n    other.settle(1);\n    c.post(1);\n    pick(1);\n}\n}}\n";
    let files = [
        ("src/ledger.h", h),
        ("src/ledger.cpp", src),
        (
            "src/util/note.h",
            "namespace util {\nvoid note(const char* m);\n}\n",
        ),
        (
            "src/util/note.cpp",
            "#include \"note.h\"\nnamespace util {\nvoid note(const char* m) {}\n}\n",
        ),
    ];
    assert_eq!(
        cpp_targets(&files, "src/ledger.cpp", "close_day"),
        [
            "recount_() → src/ledger.cpp:6",
            "this->recount_() → src/ledger.cpp:6",
            "post(q) → one of: src/ledger.cpp:7, src/ledger.cpp:8",
            "dbl(3) → hidden",
            "hook(7) → hidden",
            "tally(q) → src/ledger.cpp:9",
            "util::note(\"day\") → src/util/note.cpp:3",
            "acct::ledger::tally(1) → src/ledger.cpp:9",
            "std::sort(nullptr, nullptr) → outside",
            "fmtx::print(\"x\") → outside",
            "other.settle(1) → src/ledger.cpp:13",
            "c.post(1) → src/ledger.cpp:8",
            "pick(1) → one of: src/ledger.cpp:10, src/ledger.cpp:11, src/ledger.cpp:12",
        ]
    );
}

#[test]
fn a_bare_call_in_a_method_reaches_the_nearest_base_that_defines_it() {
    let src = "struct Top {\n    void lift() {}\n    void drop() {}\n};\nstruct Mid : Top {\n    void lift() {}\n};\nstruct Low : public Mid {\n    void run() { lift(); drop(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["lift() → src/a.cpp:6", "drop() → src/a.cpp:3"]
    );
}

#[test]
fn a_member_a_subclass_of_the_base_also_defines_is_one_of_both() {
    let src = "struct Top {\n    virtual void lift() {}\n};\nstruct Mid : Top {\n    void run() { lift(); }\n};\nstruct Low : Mid {\n    void lift() override {}\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["lift() → one of: src/a.cpp:2, src/a.cpp:8"]
    );
}

#[test]
fn a_class_with_an_outside_base_calls_a_bare_name_as_maybe_inherited() {
    let h = "int flush(int n);\n";
    let src = "#include \"flush.h\"\nstruct Sink : public lib::Stream<int> {\n    void run() { flush(1); sync(); }\n};\n";
    assert_eq!(
        cpp_targets(
            &[("src/flush.h", h), ("src/sink.cpp", src)],
            "src/sink.cpp",
            "run"
        ),
        [
            "flush(1) → one of: src/flush.h:1, or outside",
            "sync() → outside"
        ]
    );
}

#[test]
fn a_base_outside_the_project_counts_through_a_project_base() {
    let h = "int flush(int n);\n";
    let src = "#include \"flush.h\"\nstruct Mid : std::streambuf {};\nstruct Sink : Mid {\n    void run() { flush(1); }\n};\n";
    assert_eq!(
        cpp_targets(
            &[("src/flush.h", h), ("src/sink.cpp", src)],
            "src/sink.cpp",
            "run"
        ),
        ["flush(1) → one of: src/flush.h:1, or outside"]
    );
}

#[test]
fn this_arrow_a_name_the_class_lacks_is_outside_under_an_outside_base() {
    let src = "struct Pane : ui::Widget {\n    void draw() { this->paint(); }\n};\nstruct Ink {\n    void paint() {}\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "draw"),
        ["this->paint() → outside"]
    );
}

#[test]
fn this_arrow_a_name_the_class_lacks_under_project_bases_is_any_project_method() {
    let src = "struct Card {\n    void show() { this->paint(); }\n};\nstruct Ink {\n    void paint() {}\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "show"),
        ["this->paint() → one of: src/a.cpp:5, or outside"]
    );
}

#[test]
fn a_namespace_owner_is_no_class_even_when_a_type_shares_its_name() {
    let h = "namespace text {\nstruct text {\n    void trim();\n};\nint lines();\n}\n";
    let src = "#include \"text.h\"\nint text::lines() { trim(); return 0; }\n";
    assert_eq!(
        cpp_targets(
            &[("src/text.h", h), ("src/text.cpp", src)],
            "src/text.cpp",
            "lines"
        ),
        ["trim() → outside"]
    );
}

#[test]
fn a_macro_before_a_return_type_naming_a_project_class_makes_no_member_call() {
    let h = "struct Store {\n    void stock();\n    struct Shelf {};\n};\n";
    let src = "#include \"shelf.h\"\nAPI Store::Shelf restock() { stock(); return {}; }\n";
    assert_eq!(
        cpp_targets(
            &[("src/shelf.h", h), ("src/shelf.cpp", src)],
            "src/shelf.cpp",
            "restock"
        ),
        ["stock() → outside"]
    );
}

#[test]
fn a_smart_pointer_receiver_is_outside_with_a_dot_and_its_pointee_with_an_arrow() {
    let src = "struct Book {\n    void apply() {}\n    void reset() {}\n};\nvoid run() {\n    auto p = std::make_unique<Book>();\n    p.reset();\n    p->apply();\n}\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "std::make_unique<Book>() → outside",
            "p.reset() → outside",
            "p->apply() → src/a.cpp:2"
        ]
    );
}

#[test]
fn a_typed_receiver_of_a_class_another_class_overrides_is_one_of_both() {
    let src = "struct Shape {\n    virtual int area() { return 0; }\n};\nstruct Square : Shape {\n    int area() override { return 1; }\n};\nint total(Shape& s) { return s.area(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "total"),
        ["s.area() → one of: src/a.cpp:2, src/a.cpp:5"]
    );
}

#[test]
fn a_pure_virtual_method_called_on_a_typed_receiver_is_its_override() {
    let src = "struct Shape {\n    virtual int area() = 0;\n};\nstruct Square : Shape {\n    int area() override { return 1; }\n};\nint total(Shape* s) { return s->area(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "total"),
        ["s->area() → src/a.cpp:5"]
    );
}

#[test]
fn a_typed_receiver_lacking_the_method_is_outside_under_an_outside_base() {
    let src = "struct Shape : gfx::Node {};\nstruct Ink {\n    void paint() {}\n};\nvoid run(Shape& s) { s.paint(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["s.paint() → outside"]
    );
}

#[test]
fn a_typed_receiver_lacking_the_method_under_project_bases_is_any_project_method() {
    let src = "struct Shape {};\nstruct Ink {\n    void paint() {}\n};\nvoid run(Shape& s) { s.paint(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["s.paint() → one of: src/a.cpp:3, or outside"]
    );
}

#[test]
fn a_std_typed_receiver_is_outside_beside_a_project_method_of_that_name() {
    let src = "struct Stack {\n    void push_back(int v) {}\n};\nvoid run(std::vector<int>& v) { v.push_back(1); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["v.push_back(1) → outside"]
    );
}

#[test]
fn an_outside_typed_receiver_may_be_a_project_class_deriving_from_it() {
    let src = "struct Fault : std::exception {\n    const char* what() const noexcept override { return \"f\"; }\n};\nvoid run() {\n    try {\n    } catch (const std::exception& e) {\n        e.what();\n    }\n}\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["e.what() → one of: src/a.cpp:2, or outside"]
    );
}

#[test]
fn a_receiver_with_no_type_is_any_project_method_of_that_name() {
    let src = "struct Ink {\n    void paint() {}\n    void show() { auto x = make(); x.paint(); (*this).paint(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "show"),
        [
            "make() → outside",
            "x.paint() → one of: src/a.cpp:2, or outside",
            "(*this).paint() → one of: src/a.cpp:2, or outside"
        ]
    );
}

#[test]
fn a_nested_class_reaches_a_member_of_its_project_base() {
    let h = "struct Ctx {\n    int make() const;\n};\nstruct Model {\n    struct Graph : public Ctx {\n        Graph(int p);\n    };\n};\n";
    let src = "#include \"model.h\"\nint Ctx::make() const { return 0; }\nModel::Graph::Graph(int p) { make(); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/model.h", h), ("src/model.cpp", src)],
            "src/model.cpp",
            "Graph"
        ),
        ["make() → src/model.cpp:2"]
    );
}

#[test]
fn a_method_of_a_class_whose_head_no_file_shows_may_call_an_inherited_name() {
    let src = "int flush(int n) { return n; }\nvoid Gen::run() { flush(1); }\n";
    assert_eq!(
        cpp_targets(&[("src/gen.cpp", src)], "src/gen.cpp", "run"),
        ["flush(1) → one of: src/gen.cpp:1, or outside"]
    );
}

#[test]
fn a_receiver_typed_by_a_project_alias_is_any_project_method_of_that_name() {
    let src = "struct Book {\n    void apply() {}\n};\nusing BookPtr = std::unique_ptr<Book>;\nvoid run(BookPtr p) { p->apply(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["p->apply() → one of: src/a.cpp:2, or outside"]
    );
}

#[test]
fn a_class_is_told_from_a_namesake_in_another_class_or_namespace() {
    let src = "struct Base {\n    void fold() {}\n};\nstruct Left {\n    struct Node {\n        void fold() {}\n    };\n};\nstruct Right {\n    struct Node : Base {\n        void run() { fold(); }\n    };\n};\nnamespace x {\nstruct Cfg {\n    void fold() {}\n};\n}\nnamespace y {\nstruct Cfg : Base {\n    void load() { fold(); }\n};\n}\n";
    let files = [("src/a.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        ["fold() → src/a.cpp:2"]
    );
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "load"),
        ["fold() → src/a.cpp:2"]
    );
}

#[test]
fn a_call_qualified_by_the_global_scope_is_no_member_call() {
    let src = "int flush(int n) { return n; }\nstruct Sink : lib::Stream {\n    void run() { ::flush(1); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["::flush(1) → src/a.cpp:1"]
    );
}

#[test]
fn a_call_qualified_by_a_class_name_two_classes_share_is_the_one_in_view() {
    let src = "namespace x {\nstruct Cfg {\n    static int load() { return 1; }\n};\nint run() { return Cfg::load(); }\n}\nnamespace y {\nstruct Cfg {\n    static int load() { return 2; }\n};\n}\nint other() { return Cfg::load(); }\n";
    let files = [("src/a.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        ["Cfg::load() → src/a.cpp:3"]
    );
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "other"),
        ["Cfg::load() → one of: src/a.cpp:3, src/a.cpp:9, or outside"]
    );
}

#[test]
fn a_call_qualified_by_a_shared_class_name_only_one_class_answers_is_open() {
    let src = "namespace x {\nstruct Cfg {\n    static int load() { return 1; }\n};\n}\nnamespace y {\nstruct Cfg {};\n}\nint other() { return Cfg::load(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "other"),
        ["Cfg::load() → one of: src/a.cpp:3, or outside"]
    );
}

#[test]
fn a_call_qualified_by_an_owner_no_head_shows_is_its_member_or_outside() {
    let src = "int Gen::make() { return 1; }\nint run() { return Gen::make(); }\n";
    assert_eq!(
        cpp_targets(&[("src/gen.cpp", src)], "src/gen.cpp", "run"),
        ["Gen::make() → one of: src/gen.cpp:1, or outside"]
    );
}

#[test]
fn a_call_qualified_by_a_namespace_a_class_elsewhere_is_named_like_reaches_its_function() {
    let src = "namespace ui {\nnamespace dial {\nvoid start() {}\n}\n}\nstruct dial {};\nvoid run() { ui::dial::start(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["ui::dial::start() → src/a.cpp:3"]
    );
}

#[test]
fn a_call_qualified_by_a_project_alias_is_any_project_method_of_that_name() {
    let src = "struct Doc {\n    static int parse() { return 0; }\n};\nusing Text = Doc;\nint run() { return Text::parse(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["Text::parse() → one of: src/a.cpp:2, or outside"]
    );
}

#[test]
fn classes_in_two_files_anonymous_namespaces_are_two_classes() {
    let a =
        "namespace {\nstruct Printer {\n    void line() {}\n    void run() { line(); }\n};\n}\n";
    let b = "namespace {\nstruct Printer : public lib::Out {\n    void go() { line(); }\n};\n}\n";
    let files = [("src/a.cpp", a), ("src/b.cpp", b)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        ["line() → src/a.cpp:3"]
    );
    assert_eq!(cpp_targets(&files, "src/b.cpp", "go"), ["line() → outside"]);
}

#[test]
fn a_class_name_two_programs_define_is_the_one_the_callers_file_sees() {
    let h = "struct Config {\n    void load();\n    void run();\n};\n";
    let src = "#include \"config.h\"\nvoid Config::load() {}\nvoid Config::run() { load(); }\n";
    let tool = "struct Config : lib::Out {\n    void go() { load(); }\n};\n";
    let files = [
        ("app/config.h", h),
        ("app/config.cpp", src),
        ("tool/tool.cpp", tool),
    ];
    assert_eq!(
        cpp_targets(&files, "app/config.cpp", "run"),
        ["load() → app/config.cpp:2"]
    );
    assert_eq!(
        cpp_targets(&files, "tool/tool.cpp", "go"),
        ["load() → outside"]
    );
}

#[test]
fn an_out_of_line_method_written_short_after_a_using_is_its_namespaces_class() {
    let a = "namespace a {\nstruct Node {\n    void tick() {}\n    void step();\n};\n}\n";
    let b = "namespace b {\nstruct Node {\n    void tick();\n};\n}\n";
    let a_src = "#include \"a.h\"\nusing namespace a;\nvoid Node::step() { tick(); }\n";
    let b_src = "#include \"b.h\"\nusing namespace b;\nvoid Node::tick() {}\n";
    let files = [
        ("src/a.h", a),
        ("src/b.h", b),
        ("src/a.cpp", a_src),
        ("src/b.cpp", b_src),
    ];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "step"),
        ["tick() → src/a.h:3"]
    );
}

#[test]
fn a_type_name_in_a_class_is_first_a_class_nested_in_its_bases() {
    let src = "struct Cfg {\n    static int load() { return 1; }\n};\nnamespace n {\nstruct Base {\n    struct Cfg {\n        static int load() { return 2; }\n    };\n};\nstruct App : Base {\n    int run() { return Cfg::load(); }\n    struct Part {\n        int read() { return Cfg::load(); }\n    };\n};\n}\n";
    let files = [("src/a.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        ["Cfg::load() → src/a.cpp:7"]
    );
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "read"),
        ["Cfg::load() → src/a.cpp:7"]
    );
}

#[test]
fn a_typed_receiver_in_a_class_with_an_outside_base_is_its_project_class() {
    let src = "struct Cfg {\n    int load() { return 1; }\n};\nstruct App : lib::Base {\n    int run(Cfg& c) { return c.load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["c.load() → src/a.cpp:2"]
    );
}

#[test]
fn an_anonymous_namespace_class_is_not_a_global_namesake_its_file_sees() {
    let b = "struct Helper : lib::Out {};\n";
    let a = "#include \"b.h\"\nnamespace {\nstruct Helper {\n    void go() {}\n    void run() { go(); }\n};\n}\n";
    assert_eq!(
        cpp_targets(&[("src/b.h", b), ("src/a.h", a)], "src/a.h", "run"),
        ["go() → src/a.h:4"]
    );
}

#[test]
fn a_bare_call_reaching_only_a_dependent_base_may_be_the_free_function() {
    let src = "void flush() {}\ntemplate <class T>\nstruct Base {\n    void flush() {}\n};\ntemplate <class T>\nstruct Sink : Base<T> {\n    void run() { flush(); }\n};\nstruct Pipe : Base<int> {\n    void pump() { flush(); }\n};\n";
    let files = [("src/a.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        ["flush() → one of: src/a.cpp:1, src/a.cpp:4"]
    );
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "pump"),
        ["flush() → src/a.cpp:4"]
    );
}

#[test]
fn a_base_is_named_from_outside_its_class() {
    let src = "struct Outer {\n    struct Part {\n        void fit() {}\n    };\n    struct Whole : Part {\n        struct Part {};\n        void make() { fit(); }\n    };\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "make"),
        ["fit() → src/a.cpp:3"]
    );
}

#[test]
fn an_out_of_line_method_of_a_class_no_head_settles_has_no_certain_member() {
    let a = "namespace a {\nstruct Node {\n    void tick() {}\n    void step();\n};\n}\n";
    let b = "namespace b {\nstruct Node {\n    void tick();\n};\n}\n";
    let a_src = "#include \"a.h\"\n#include \"b.h\"\nvoid Node::step() { tick(); }\n";
    let b_src = "#include \"b.h\"\nvoid Node::tick() {}\n";
    let files = [
        ("src/a.h", a),
        ("src/b.h", b),
        ("src/a.cpp", a_src),
        ("src/b.cpp", b_src),
    ];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "step"),
        ["tick() → one of: src/a.h:3, src/b.cpp:2, or outside"]
    );
}

#[test]
fn a_class_two_programs_split_into_header_and_source_is_the_callers_programs() {
    let h = "struct Config {\n    void load();\n    void run();\n};\n";
    let src = "#include \"config.h\"\nvoid Config::load() {}\nvoid Config::run() { load(); }\n";
    let other = "#include \"config.h\"\nvoid Config::load() {}\n";
    let files = [
        ("app/config.h", h),
        ("app/config.cpp", src),
        ("tool/config.h", "struct Config {\n    void load();\n};\n"),
        ("tool/config.cpp", other),
    ];
    assert_eq!(
        cpp_targets(&files, "app/config.cpp", "run"),
        ["load() → app/config.cpp:2"]
    );
}

#[test]
fn a_class_names_itself_inside_its_body_under_an_outside_base() {
    let src = "struct Reader : lib::Base {\n    static int peek() { return 0; }\n    int next() { return Reader::peek(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "next"),
        ["Reader::peek() → src/a.cpp:2"]
    );
}

#[test]
fn a_type_name_in_a_class_whose_base_no_head_places_is_not_placed_certainly() {
    let src = "struct Cfg {\n    static int load() { return 1; }\n};\nusing Mixin = lib::Mixin;\nstruct App : Mixin {\n    int run() { return Cfg::load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["Cfg::load() → one of: src/a.cpp:2, or outside"]
    );
}

#[test]
fn a_base_named_like_the_class_but_qualified_by_an_outer_namespace_is_outside() {
    let src = "struct Cfg {\n    int load() { return 1; }\n};\nnamespace lib {\nnamespace front {\nnamespace mine {\nstruct Ctx : front::Ctx {\n    int run(Cfg& c) { return c.load(); }\n};\n}\n}\n}\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["c.load() → src/a.cpp:2"]
    );
}

#[test]
fn a_base_several_project_classes_may_be_hides_type_names_from_certainty() {
    let src = "struct Cfg {\n    static int load() { return 1; }\n};\nnamespace a {\nstruct Node {};\n}\nnamespace b {\nstruct Node {};\n}\nstruct App : Node {\n    int run() { return Cfg::load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["Cfg::load() → one of: src/a.cpp:2, or outside"]
    );
}

#[test]
fn an_out_of_line_method_of_an_anonymous_class_is_not_a_global_namesakes() {
    let a = "namespace {\nstruct Unit {\n    void step();\n    void tick() {}\n};\n}\nvoid Unit::step() { tick(); }\n";
    let b = "struct Unit {\n    void tick() {}\n};\n";
    let files = [("src/a.cpp", a), ("src/b.cpp", b)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "step"),
        ["tick() → src/a.cpp:4"]
    );
}

#[test]
fn a_type_name_in_a_files_code_is_its_own_anonymous_class_not_an_unseen_global() {
    let a = "struct Pen {\n    void draw() {}\n};\n";
    let b = "namespace {\nstruct Pen {\n    void draw() {}\n};\nvoid first() { Pen p; p.draw(); }\n}\nvoid second() { Pen q; q.draw(); }\n";
    let files = [("src/a.cpp", a), ("src/b.cpp", b)];
    assert_eq!(
        cpp_targets(&files, "src/b.cpp", "first"),
        ["p.draw() → src/b.cpp:3"]
    );
    assert_eq!(
        cpp_targets(&files, "src/b.cpp", "second"),
        ["q.draw() → src/b.cpp:3"]
    );
}

#[test]
fn a_type_name_no_file_in_view_defines_is_outside() {
    let b = "namespace {\nstruct Pen {\n    void draw() {}\n};\n}\n";
    let c = "#include <lib/pen.h>\nvoid run(Pen& p) { p.draw(); }\n";
    assert_eq!(
        cpp_targets(&[("src/b.cpp", b), ("src/c.cpp", c)], "src/c.cpp", "run"),
        ["p.draw() → outside"]
    );
}

#[test]
fn a_qualifier_is_placed_through_the_bases_the_callers_file_sees() {
    let common =
        "struct Base {\n    struct Cfg {\n        static int load() { return 2; }\n    };\n};\n";
    let app = "#include \"common.h\"\nstruct Cfg {\n    static int load() { return 1; }\n};\nstruct App {\n    int run() { return Cfg::load(); }\n};\n";
    let tool = "#include \"common.h\"\nstruct App : Base {\n    int go() { return 0; }\n};\n";
    let files = [
        ("src/common.h", common),
        ("src/app.cpp", app),
        ("src/tool.cpp", tool),
    ];
    assert_eq!(
        cpp_targets(&files, "src/app.cpp", "run"),
        ["Cfg::load() → src/app.cpp:3"]
    );
}

#[test]
fn a_files_anonymous_class_and_a_global_namesake_it_sees_are_not_told_apart() {
    let g = "struct Pen {\n    void draw() {}\n};\n";
    let b = "#include \"pen.h\"\nnamespace {\nstruct Pen {\n    void draw() {}\n};\nvoid first() { Pen p; p.draw(); }\n}\n";
    assert_eq!(
        cpp_targets(&[("src/pen.h", g), ("src/b.cpp", b)], "src/b.cpp", "first"),
        ["p.draw() → one of: src/b.cpp:4, src/pen.h:2, or outside"]
    );
}

#[test]
fn a_base_the_project_declares_as_an_alias_is_not_read_as_outside() {
    let h = "namespace detail {\nstruct Impl {\n    struct Cfg {\n        static int load() { return 2; }\n    };\n};\n}\nnamespace lib {\nusing Base = detail::Impl;\n}\n";
    let src = "#include \"a.h\"\nstruct Cfg {\n    static int load() { return 1; }\n};\nnamespace other {\nstruct Base {};\n}\nstruct App : lib::Base {\n    int run() { return Cfg::load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.h", h), ("src/b.cpp", src)], "src/b.cpp", "run"),
        ["Cfg::load() → one of: src/a.h:4, src/b.cpp:3, or outside"]
    );
}

#[test]
fn anonymous_classes_of_two_files_with_out_of_line_methods_are_each_their_own() {
    let a = "namespace {\nstruct Impl {\n    void tick();\n    void step();\n};\nvoid Impl::tick() {}\nvoid Impl::step() { tick(); }\n}\n";
    let b = "namespace {\nstruct Impl {\n    void tick();\n    void step();\n};\nvoid Impl::tick() {}\nvoid Impl::step() { tick(); }\n}\n";
    let files = [("src/a.cpp", a), ("src/b.cpp", b)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "step"),
        ["tick() → src/a.cpp:6"]
    );
    assert_eq!(
        cpp_targets(&files, "src/b.cpp", "step"),
        ["tick() → src/b.cpp:6"]
    );
}

#[test]
fn a_class_an_includer_provides_is_not_hidden_by_a_namesake_the_header_sees() {
    let n = "namespace n {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let g = "struct Cfg {\n    void load() {}\n};\n";
    let usage = "#include \"g.h\"\nnamespace n {\ninline void f() { Cfg c; c.load(); }\n}\n";
    let main = "#include \"n.h\"\n#include \"use.h\"\nint main() { return 0; }\n";
    let files = [
        ("src/n.h", n),
        ("src/g.h", g),
        ("src/use.h", usage),
        ("src/main.cpp", main),
    ];
    assert_eq!(
        cpp_targets(&files, "src/use.h", "f"),
        ["c.load() → one of: src/g.h:2, src/n.h:3"]
    );
}

#[test]
fn a_class_out_of_view_that_lookup_meets_first_keeps_the_one_in_view_uncertain() {
    let n = "namespace n {\nstruct Cfg {\n    static int load() { return 2; }\n    int size() { return 0; }\n};\n}\n";
    let b = "#include <n.h>\nstruct Cfg {\n    static int load() { return 1; }\n    int size() { return 1; }\n};\nnamespace n {\nint run() { return Cfg::load(); }\nint go() { Cfg c; return c.size(); }\n}\n";
    let files = [("src/n.h", n), ("src/b.cpp", b)];
    assert_eq!(
        cpp_targets(&files, "src/b.cpp", "run"),
        ["Cfg::load() → one of: src/b.cpp:3, src/n.h:3"]
    );
    assert_eq!(
        cpp_targets(&files, "src/b.cpp", "go"),
        ["c.size() → one of: src/b.cpp:4, src/n.h:4"]
    );
}

#[test]
fn a_base_through_an_alias_template_leaves_a_type_name_open() {
    let h = "namespace detail {\nstruct Impl {\n    struct Cfg {\n        static int load() { return 2; }\n    };\n};\n}\nnamespace lib {\ntemplate <class T>\nusing Base = detail::Impl;\n}\n";
    let src = "#include \"a.h\"\nstruct Cfg {\n    static int load() { return 1; }\n};\nstruct App : lib::Base<int> {\n    int run() { return Cfg::load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.h", h), ("src/b.cpp", src)], "src/b.cpp", "run"),
        ["Cfg::load() → one of: src/a.h:4, src/b.cpp:3, or outside"]
    );
}

#[test]
fn a_base_through_an_alias_out_of_view_leaves_a_type_name_open() {
    let h = "namespace detail {\nstruct Impl {\n    struct Cfg {\n        static int load() { return 2; }\n    };\n};\n}\nnamespace lib {\nusing Base = detail::Impl;\n}\n";
    let src = "#include <a.h>\nstruct Cfg {\n    static int load() { return 1; }\n};\nstruct App : lib::Base {\n    int run() { return Cfg::load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.h", h), ("src/b.cpp", src)], "src/b.cpp", "run"),
        ["Cfg::load() → one of: src/a.h:4, src/b.cpp:3, or outside"]
    );
}

#[test]
fn a_base_through_an_alias_of_an_outside_class_leaves_a_type_name_open() {
    let h = "#include <ext/thing.h>\nnamespace lib {\nusing Base = ext::Thing;\n}\nnamespace other {\nstruct Cfg {\n    static int load() { return 2; }\n};\n}\n";
    let src = "#include \"a.h\"\nstruct Cfg {\n    static int load() { return 1; }\n};\nstruct App : lib::Base {\n    int run() { return Cfg::load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.h", h), ("src/b.cpp", src)], "src/b.cpp", "run"),
        ["Cfg::load() → one of: src/a.h:7, src/b.cpp:3, or outside"]
    );
}

#[test]
fn an_alias_met_before_a_global_class_of_its_name_leaves_the_base_unplaced() {
    let src = "struct Base {};\nstruct Cfg {\n    static int load() { return 1; }\n};\nnamespace detail {\nstruct Impl {\n    struct Cfg {\n        static int load() { return 2; }\n    };\n};\n}\nnamespace lib {\nusing Base = detail::Impl;\nstruct App : Base {\n    int run() { return Cfg::load(); }\n};\n}\n";
    assert_eq!(
        cpp_targets(&[("src/b.cpp", src)], "src/b.cpp", "run"),
        ["Cfg::load() → one of: src/b.cpp:3, src/b.cpp:8, or outside"]
    );
}

#[test]
fn a_qualifier_either_class_may_be_is_never_certain() {
    let n = "#include <ext/base.h>\nnamespace n {\nstruct Cfg : ext::Base {};\n}\n";
    let b = "#include <n.h>\nstruct Cfg {\n    static int load() { return 1; }\n};\nnamespace n {\nint run() { return Cfg::load(); }\n}\n";
    assert_eq!(
        cpp_targets(&[("src/n.h", n), ("src/b.cpp", b)], "src/b.cpp", "run"),
        ["Cfg::load() → one of: src/b.cpp:3, or outside"]
    );
}

#[test]
fn a_qualified_call_of_a_member_an_outside_base_gives_is_outside() {
    let n = "#include <ext/base.h>\nnamespace n {\nstruct Cfg : ext::Base {};\n}\n";
    let b = "#include \"n.h\"\nnamespace n {\nint run() { return Cfg::load(); }\n}\n";
    assert_eq!(
        cpp_targets(&[("src/n.h", n), ("src/b.cpp", b)], "src/b.cpp", "run"),
        ["Cfg::load() → outside"]
    );
}

#[test]
fn a_typedef_of_a_qualified_name_is_an_alias() {
    let src = "struct Cfg {\n    void load() {}\n};\nstruct App {\n    typedef ext::Cfg Cfg;\n    void run() { Cfg c; c.load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["c.load() → one of: src/a.cpp:2, or outside"]
    );
}

#[test]
fn an_alias_whose_path_ends_in_the_name_leaves_the_last_step_open() {
    let u = "namespace lib {\nusing Cfg = ext::Thing;\n}\nusing namespace lib;\n";
    let o = "namespace other {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let main = "#include \"u.h\"\n#include \"o.h\"\nvoid run() { Cfg c; c.load(); }\n";
    let files = [("src/u.h", u), ("src/o.h", o), ("src/main.cpp", main)];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "run"),
        ["c.load() → one of: src/o.h:3, or outside"]
    );
}

#[test]
fn an_alias_in_another_source_file_does_not_reach_the_caller() {
    let h = "struct Buffer {\n    void clear() {}\n};\n";
    let a = "#include \"buf.h\"\nvoid run(Buffer& b) { b.clear(); }\n";
    let other = "using Buffer = std::vector<char>;\n";
    let files = [("src/buf.h", h), ("src/a.cpp", a), ("src/other.cpp", other)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        ["b.clear() → src/buf.h:2"]
    );
}

#[test]
fn a_using_declaration_of_a_class_places_its_name() {
    let a = "namespace a {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let g = "struct Cfg {\n    void load() {}\n};\n";
    let main = "#include \"a.h\"\nusing a::Cfg;\nvoid run() { Cfg c; c.load(); }\n";
    let files = [("src/a.h", a), ("src/g.h", g), ("src/main.cpp", main)];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "run"),
        ["c.load() → src/a.h:3"]
    );
}

#[test]
fn a_qualifier_either_class_may_be_stays_open_when_one_class_has_none_of_its_own() {
    let n = "#include <ext/base.h>\nnamespace n {\nstruct Cfg : ext::Base {};\n}\n";
    let b = "#include <n.h>\nstruct Cfg {\n    static int load(int a) { return a; }\n    static int load(double a) { return 0; }\n};\nnamespace n {\nint run() { return Cfg::load(1); }\n}\n";
    assert_eq!(
        cpp_targets(&[("src/n.h", n), ("src/b.cpp", b)], "src/b.cpp", "run"),
        ["Cfg::load(1) → one of: src/b.cpp:3, src/b.cpp:4, or outside"]
    );
}

#[test]
fn a_using_declaration_inside_a_function_does_not_place_the_name_for_the_file() {
    let a = "namespace a {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let g = "struct Cfg {\n    void load() {}\n};\n";
    let main = "#include \"a.h\"\n#include \"g.h\"\nvoid f() {\n    using a::Cfg;\n    Cfg c;\n    c.load();\n}\nvoid g() {\n    Cfg c;\n    c.load();\n}\n";
    let files = [("src/a.h", a), ("src/g.h", g), ("src/main.cpp", main)];
    let either = ["c.load() → one of: src/a.h:3, src/g.h:2"];
    assert_eq!(cpp_targets(&files, "src/main.cpp", "f"), either);
    assert_eq!(cpp_targets(&files, "src/main.cpp", "g"), either);
}

#[test]
fn a_using_declaration_inside_a_namespace_does_not_place_the_name_for_the_file() {
    let a = "namespace a {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let g = "struct Cfg {\n    void load() {}\n};\n";
    let main = "#include \"a.h\"\n#include \"g.h\"\nnamespace x {\nusing a::Cfg;\n}\nvoid g() {\n    Cfg c;\n    c.load();\n}\n";
    let files = [("src/a.h", a), ("src/g.h", g), ("src/main.cpp", main)];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "g"),
        ["c.load() → one of: src/a.h:3, src/g.h:2"]
    );
}

#[test]
fn a_file_scope_using_declaration_of_an_outside_class_makes_the_name_outside() {
    let g = "struct Cfg {\n    void load() {}\n};\n";
    let main = "#include <ext/cfg.h>\nusing ext::Cfg;\nvoid run() { Cfg c; c.load(); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/g.h", g), ("src/main.cpp", main)],
            "src/main.cpp",
            "run"
        ),
        ["c.load() → outside"]
    );
}

#[test]
fn a_using_directive_inside_a_function_does_not_place_the_name_for_the_file() {
    let a = "namespace a {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let b = "namespace b {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let main = "#include \"a.h\"\n#include \"b.h\"\nvoid f() {\n    using namespace a;\n    Cfg c;\n    c.load();\n}\nvoid g() {\n    Cfg c;\n    c.load();\n}\n";
    let files = [("src/a.h", a), ("src/b.h", b), ("src/main.cpp", main)];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "g"),
        ["c.load() → one of: src/a.h:3, src/b.h:3, or outside"]
    );
}

#[test]
fn an_inherited_constructor_using_leaves_the_base_placed() {
    let src = "struct Base {\n    void go() {}\n};\nstruct Kid : Base {\n    using Base::Base;\n    void run() { go(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["go() → src/a.cpp:2"]
    );
}

#[test]
fn a_using_declaration_of_a_global_class_inside_a_namespace_places_it() {
    let g = "struct Cfg {\n    void load() {}\n};\n";
    let main = "#include \"g.h\"\nnamespace n {\nusing ::Cfg;\nvoid f() { Cfg c; c.load(); }\n}\n";
    assert_eq!(
        cpp_targets(
            &[("src/g.h", g), ("src/main.cpp", main)],
            "src/main.cpp",
            "f"
        ),
        ["c.load() → src/g.h:2"]
    );
}

#[test]
fn a_using_declaration_through_a_project_namespace_alias_is_not_outside() {
    let d = "namespace proj { namespace detail {\nstruct Cfg {\n    void load() {}\n};\n} }\n";
    let main = "#include \"d.h\"\nnamespace p = proj::detail;\nusing p::Cfg;\nvoid f() { Cfg c; c.load(); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/d.h", d), ("src/main.cpp", main)],
            "src/main.cpp",
            "f"
        ),
        ["c.load() → one of: src/d.h:3, or outside"]
    );
}

#[test]
fn a_field_of_a_project_type_is_certain_with_or_without_this() {
    let h =
        "#include \"book.h\"\nstruct Shelf {\n    void run();\n    const Book* top_, *low_;\n};\n";
    let book = "struct Book {\n    void apply() {}\n};\n";
    let src = "#include \"shelf.h\"\nvoid Shelf::run() {\n    top_->apply();\n    this->low_->apply();\n    auto go = [this] { top_->apply(); };\n}\n";
    let files = [
        ("src/book.h", book),
        ("src/shelf.h", h),
        ("src/shelf.cpp", src),
    ];
    assert_eq!(
        cpp_targets(&files, "src/shelf.cpp", "run"),
        [
            "top_->apply() → src/book.h:2",
            "this->low_->apply() → src/book.h:2",
            "top_->apply() → src/book.h:2"
        ]
    );
}

#[test]
fn a_field_of_a_type_a_subclass_overrides_is_one_of_both() {
    let src = "struct Shape {\n    virtual int area() { return 0; }\n};\nstruct Square : Shape {\n    int area() override { return 1; }\n};\nstruct Canvas {\n    Shape* shape_;\n    int run() { return shape_->area(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["shape_->area() → one of: src/a.cpp:2, src/a.cpp:5"]
    );
}

#[test]
fn a_smart_pointer_field_is_outside_with_a_dot_and_its_pointee_with_an_arrow() {
    let src = "struct Book {\n    void apply() {}\n    void reset() {}\n};\nstruct Shelf {\n    std::unique_ptr<Book> book_ = nullptr;\n    void run() {\n        book_->apply();\n        this->book_->apply();\n        book_.reset();\n    }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "book_->apply() → src/a.cpp:2",
            "this->book_->apply() → src/a.cpp:2",
            "book_.reset() → outside"
        ]
    );
}

#[test]
fn a_field_a_project_base_declares_is_typed_by_it() {
    let src = "struct Book {\n    void apply() {}\n};\nstruct Ink {\n    void apply() {}\n};\nstruct Base {\n    Book book_;\n};\nstruct Mid : Base {};\nstruct Shelf : public Mid {\n    void run() { book_.apply(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["book_.apply() → src/a.cpp:2"]
    );
}

#[test]
fn a_field_behind_a_nearer_base_no_head_places_is_not_certain() {
    let src = "struct Book {\n    void apply() {}\n};\nstruct Ink {\n    void apply() {}\n};\nstruct Top {\n    Book book_;\n};\nstruct Mid : Top {};\nusing Other = Ink;\nstruct Shelf : Other, Mid {\n    void run() { book_.apply(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["book_.apply() → one of: src/a.cpp:2, src/a.cpp:5, or outside"]
    );
}

#[test]
fn a_function_field_called_bare_is_hidden_beside_a_free_function_of_its_name() {
    let src = "void notify(int v) {}\nstruct Shelf {\n    std::function<void(int)> notify;\n    void run() { notify(1); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["notify(1) → hidden"]
    );
}

#[test]
fn a_local_named_like_a_field_is_the_local() {
    let src = "struct Book {\n    void apply() {}\n};\nstruct Ink {\n    void apply() {}\n};\nstruct Shelf {\n    Book book_;\n    void run(Ink book_) { book_.apply(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["book_.apply() → src/a.cpp:5"]
    );
}

#[test]
fn a_field_type_is_placed_from_the_class_not_from_the_callers_file() {
    let h = "struct Cfg {\n    void load() {}\n};\nstruct Shelf {\n    Cfg cfg_;\n    void run();\n};\n";
    let b = "namespace b {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let src = "#include \"shelf.h\"\n#include \"b.h\"\nusing namespace b;\nvoid Shelf::run() { cfg_.load(); }\n";
    let files = [("src/shelf.h", h), ("src/b.h", b), ("src/shelf.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/shelf.cpp", "run"),
        ["cfg_.load() → src/shelf.h:2"]
    );
}

#[test]
fn a_field_typed_by_a_template_parameter_is_no_project_class() {
    let src = "struct Book {\n    void apply() {}\n};\ntemplate <class Book>\nstruct Box {\n    Book item_;\n    void run() { item_.apply(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["item_.apply() → one of: src/a.cpp:2, or outside"]
    );
}

#[test]
fn a_method_returning_a_pointer_or_reference_is_no_data_member() {
    let src = "struct Shelf {\n    int* top();\n    const int& low() const;\n    int (*pick_)(int);\n    void run() { top(); low(); pick_(1); }\n};\nint* Shelf::top() { return nullptr; }\nconst int& Shelf::low() const { return *top(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "top() → src/a.cpp:7",
            "low() → src/a.cpp:8",
            "pick_(1) → hidden"
        ]
    );
}

#[test]
fn a_smart_pointer_of_a_template_parameter_is_no_project_class() {
    let src = "struct Engine {\n    void start() {}\n};\ntemplate <class Engine>\nvoid boot(std::unique_ptr<Engine> e) { e->start(); }\ntemplate <class Engine>\nstruct Pad {\n    void run(std::shared_ptr<Engine> e) { e->start(); }\n};\n";
    let open = ["e->start() → one of: src/a.cpp:2, or outside"];
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "boot"),
        open
    );
    assert_eq!(cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"), open);
    let late = "struct Engine {\n    void start() {}\n};\ntemplate <class Engine>\nvoid boot(std::unique_ptr<Engine const> e) { e->start(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", late)], "src/a.cpp", "boot"),
        open
    );
}

#[test]
fn a_project_class_with_a_template_parameter_argument_keeps_its_type() {
    let src = "template <class T>\nstruct Box {\n    int size() { return 0; }\n};\nstruct Tray {\n    int size() { return 1; }\n};\ntemplate <class T>\nint count(Box<T>& b) { return b.size(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "count"),
        ["b.size() → src/a.cpp:3"]
    );
}

#[test]
fn a_member_in_a_class_local_to_a_method_is_not_the_methods_class_field() {
    let src = "struct Lock {\n    void unlock() {}\n};\nstruct Spin {\n    void unlock() {}\n};\nstruct Shelf {\n    Lock mutex_;\n    std::function<void()> notify;\n    void run() {\n        struct Guard {\n            Spin& mutex_;\n            void notify() {}\n            ~Guard() { mutex_.unlock(); notify(); }\n        };\n        mutex_.unlock();\n    }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "mutex_.unlock() → one of: src/a.cpp:2, src/a.cpp:5, or outside",
            "notify() → outside",
            "mutex_.unlock() → src/a.cpp:2"
        ]
    );
}

#[test]
fn an_arrow_on_a_class_with_an_arrow_operator_is_not_its_own_method() {
    let src = "struct Book {\n    void reset() {}\n};\nstruct Handle {\n    Book* operator->();\n    void reset() {}\n};\nstruct Base {\n    Book* operator->();\n};\nstruct Ref : Base {\n    void reset() {}\n};\nstruct Shelf {\n    Handle h_;\n    void run(Ref r) { h_->reset(); h_.reset(); r->reset(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "h_->reset() → one of: src/a.cpp:2, src/a.cpp:6, src/a.cpp:12, or outside",
            "h_.reset() → src/a.cpp:6",
            "r->reset() → one of: src/a.cpp:2, src/a.cpp:6, src/a.cpp:12, or outside"
        ]
    );
}

#[test]
fn a_smart_pointer_to_a_const_project_class_is_its_pointee() {
    let src = "struct Book {\n    void apply() {}\n};\nstruct Shelf {\n    std::shared_ptr<const Book> w_;\n    std::unique_ptr<Book const> u_;\n    void run(std::unique_ptr<const Book> p) { w_->apply(); u_->apply(); p->apply(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "w_->apply() → src/a.cpp:2",
            "u_->apply() → src/a.cpp:2",
            "p->apply() → src/a.cpp:2"
        ]
    );
}

#[test]
fn a_function_like_macro_block_in_a_method_keeps_the_methods_fields() {
    let src = "struct Book {\n    void apply() {}\n};\nstruct Shelf {\n    Book book_;\n    std::vector<int> items_;\n    void run() {\n        EACH_ITEM(int i, items_) { book_.apply(); }\n    }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["book_.apply() → src/a.cpp:2"]
    );
}

#[test]
fn a_call_in_a_local_class_method_is_never_certainly_the_enclosing_classs_method() {
    let src = "void tidy() {}\nstruct Shelf {\n    void flush() {}\n    void tidy() {}\n    void run() {\n        struct Guard {\n            void flush() {}\n            ~Guard() { flush(); this->flush(); ::tidy(); tidy(); }\n        };\n    }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "flush() → one of: src/a.cpp:3, or outside",
            "this->flush() → one of: src/a.cpp:3, or outside",
            "::tidy() → src/a.cpp:1",
            "tidy() → one of: src/a.cpp:1, src/a.cpp:4, or outside"
        ]
    );
}

#[test]
fn an_arrow_operator_declared_with_spaces_is_found() {
    let src = "struct Book {\n    void reset() {}\n};\nstruct Spaced {\n    Book* operator -> ();\n    void reset() {}\n};\nvoid run(Spaced s) { s->reset(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["s->reset() → one of: src/a.cpp:2, src/a.cpp:6, or outside"]
    );
}

#[test]
fn an_items_own_flow_marked_as_a_local_class_method_still_reaches_its_class() {
    // Regression pin: in a file whose braces split across `#if`/`#else`, a
    // method charted from the tree that misreads the split sits in a body,
    // so its own flow carries `local_class`; the item is still its class's.
    let src = "struct Turn {\n    void flush() {}\n    void push() { flush(); }\n};\n";
    let mut p = plaz::source::project([("src/a.h".to_string(), src.to_string())]);
    let f = p.files.get_mut("src/a.h").unwrap();
    let at = |name: &str| (f.code.items.iter()).position(|it| it.name == name);
    let (push, flush) = (at("push").unwrap(), at("flush").unwrap());
    f.flows.get_mut(&push).unwrap().local_class = true;
    let resolved = plaz::resolve::resolve(p);
    let mut out = Vec::new();
    resolved.project().files["src/a.h"].flows[&push]
        .each_call(&mut |_, c| out.push(c.target.clone()));
    let flush = plaz::model::FnId {
        path: "src/a.h".to_string(),
        item: flush,
    };
    assert_eq!(out, [plaz::model::Target::Resolved(flush)]);
}

#[test]
fn a_local_class_method_under_if_or_as_a_template_is_still_the_local_classs() {
    let src = "struct Book {\n    void apply() {}\n};\nstruct Ink {\n    void apply() {}\n};\nstruct Shelf {\n    Book book_;\n    void flush() {}\n    void run() {\n        struct Guard {\n            Ink book_;\n#ifdef DEBUG\n            void go() { flush(); book_.apply(); this->flush(); }\n#endif\n            template <class T>\n            void put(T t) { book_.apply(); }\n            void flush() {}\n        };\n    }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "flush() → one of: src/a.cpp:9, or outside",
            "book_.apply() → one of: src/a.cpp:2, src/a.cpp:5, or outside",
            "this->flush() → one of: src/a.cpp:9, or outside",
            "book_.apply() → one of: src/a.cpp:2, src/a.cpp:5, or outside"
        ]
    );
}

#[test]
fn a_class_head_with_a_macro_before_its_name_is_a_class() {
    let h = "#define KIT_API\nnamespace kit {\nclass KIT_API Base {\npublic:\n    bool isLit() const;\n};\nclass KIT_API Pane final : public Base {\npublic:\n    class KIT_API Part {\n    public:\n        void tick();\n    };\n    void setQuiet(bool on);\n    void open(int h);\n};\n}\n";
    let src = "#include \"pane.h\"\nnamespace kit {\nbool Base::isLit() const { return true; }\nvoid Pane::setQuiet(bool on) {}\nvoid Pane::open(int h) { setQuiet(h > 0); }\nvoid Pane::Part::tick() {}\n}\nvoid setQuiet(int fd, bool on) {}\nvoid run() {\n    kit::Pane p;\n    p.isLit();\n    kit::Pane::Part q;\n    q.tick();\n}\n";
    let files = [("src/pane.h", h), ("src/pane.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/pane.cpp", "open"),
        ["setQuiet(h > 0) → src/pane.cpp:4"]
    );
    assert_eq!(
        cpp_targets(&files, "src/pane.cpp", "run"),
        ["p.isLit() → src/pane.cpp:3", "q.tick() → src/pane.cpp:6"]
    );
}

#[test]
fn a_methods_declaration_stays_its_classs_beside_a_namesake_classs_definition() {
    let h = "struct Shape {\n    int area() const;\n    int twice() const;\n};\n";
    let src = "#include \"shape.h\"\nint Shape::twice() const { return 2 * area(); }\n";
    let other = "namespace {\nstruct Shape {\n    int area() const { return 1; }\n};\n}\n";
    let files = [
        ("src/shape.h", h),
        ("src/shape.cpp", src),
        ("src/other.cpp", other),
    ];
    assert_eq!(
        cpp_targets(&files, "src/shape.cpp", "twice"),
        ["area() → src/shape.h:2"]
    );
}

#[test]
fn a_field_of_a_nested_class_the_head_only_declares_is_placed_where_it_is_defined() {
    let h = "#include <memory>\nclass Pane {\npublic:\n    void show();\nprivate:\n    struct Core;\n    std::unique_ptr<Core> m_core;\n};\n";
    let src = "#include \"pane.h\"\nstruct Pane::Core {\n    void paint() {}\n};\nvoid Pane::show() { m_core->paint(); }\n";
    let files = [("src/pane.h", h), ("src/pane.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/pane.cpp", "show"),
        ["m_core->paint() → src/pane.cpp:3"]
    );
    // An outside type the caller's file has a namesake of elsewhere is not it.
    let h = "class Pane {\npublic:\n    void show();\nprivate:\n    Gadget* m_gadget;\n};\n";
    let src = "#include \"pane.h\"\nnamespace detail {\nstruct Gadget {\n    void paint() {}\n};\n}\nvoid Pane::show() { m_gadget->paint(); }\n";
    let files = [("src/pane.h", h), ("src/pane.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/pane.cpp", "show"),
        ["m_gadget->paint() → outside"]
    );
}

#[test]
fn an_elaborated_type_with_a_brace_initialiser_is_no_class_head() {
    let src = "struct Clock {\n    void tick();\n};\nvoid Clock::tick() {}\nvoid run() {\n    struct Clock c{};\n    c.tick();\n}\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["c.tick() → src/a.cpp:4"]
    );
}

#[test]
fn a_bare_call_in_a_method_of_a_class_no_head_shows_lists_its_methods_beside_the_free_ones() {
    let src = "#include <kit/pane.h>\nvoid Pane::setQuiet(bool on) {}\nvoid Pane::open(int h) { setQuiet(h > 0); }\nvoid setQuiet(int fd, bool on) {}\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "open"),
        ["setQuiet(h > 0) → one of: src/a.cpp:2, src/a.cpp:4, or outside"]
    );
}

#[test]
fn a_declaration_no_definitions_file_includes_is_any_programs_definition() {
    let entry =
        "extern int main(int argc, char* argv[]);\nint start() { return main(0, nullptr); }\n";
    let one = "int main(int argc, char* argv[]) { return 0; }\n";
    let two = "int main(int argc, char** argv) { return 1; }\n";
    let none = "int main() { return 2; }\n";
    let files = [
        ("lib/entry.cpp", entry),
        ("demo/one.cpp", one),
        ("demo/two.cpp", two),
        ("demo/none.cpp", none),
    ];
    assert_eq!(
        cpp_targets(&files, "lib/entry.cpp", "start"),
        ["main(0, nullptr) → one of: demo/one.cpp:1, demo/two.cpp:1, or outside"]
    );
    // The header and source split: the defining file includes the header.
    let h = "int count(int n);\n";
    let def = "#include \"count.h\"\nint count(int n) { return n; }\n";
    let call = "#include \"count.h\"\nint run() { return count(1); }\n";
    let files = [
        ("src/count.h", h),
        ("src/count.cpp", def),
        ("src/run.cpp", call),
    ];
    assert_eq!(
        cpp_targets(&files, "src/run.cpp", "run"),
        ["count(1) → src/count.cpp:2"]
    );
}

#[test]
fn a_namespace_alias_is_the_namespace_it_names() {
    let h = "namespace kit {\nnamespace ui {\nvoid paint(int n);\n}\n}\n";
    let def = "#include \"ui.h\"\nnamespace kit {\nnamespace ui {\nvoid paint(int n) {}\n}\n}\n";
    let src = "#include \"ui.h\"\nnamespace fs = std::filesystem;\nnamespace kui = ::kit::ui;\nvoid run() {\n    fs::create_directories(\"x\");\n    kui::paint(1);\n}\n";
    let files = [("src/ui.h", h), ("src/ui.cpp", def), ("src/a.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        [
            "fs::create_directories(\"x\") → outside",
            "kui::paint(1) → src/ui.cpp:4"
        ]
    );
}

#[test]
fn a_member_or_local_of_a_project_class_called_bare_is_its_call_operator() {
    let src = "#include <functional>\nstruct Less {\n    bool operator()(int a, int b) const { return a < b; }\n};\nstruct Plain {\n    int v;\n};\nstruct Sorter {\n    Less less_;\n    std::function<bool(int, int)> fn_;\n    bool (*ptr_)(int, int);\n    Plain plain_;\n    bool run(int a, int b) { return less_(a, b) && fn_(a, b) && ptr_(a, b) && plain_(a, b); }\n};\nbool pick(int a, int b, Plain p) {\n    Less cmp;\n    return cmp(a, b) && p(a, b);\n}\n";
    let files = [("src/a.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        [
            "less_(a, b) → src/a.cpp:3",
            "fn_(a, b) → hidden",
            "ptr_(a, b) → hidden",
            "plain_(a, b) → hidden"
        ]
    );
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "pick"),
        ["cmp(a, b) → src/a.cpp:3", "p(a, b) → hidden"]
    );
}

#[test]
fn a_class_head_with_a_macro_after_its_name_keeps_its_name() {
    let h = "#define TOY_FINAL final\nstruct Gear {\n    void spin() {}\n};\nclass Crank TOY_FINAL : public Gear {\npublic:\n    void start() { spin(); }\n};\nclass Wheel TOY_FINAL {\npublic:\n    void spin() {}\n};\nvoid roll() {\n    Wheel w;\n    w.spin();\n}\n#define toy_api\nclass toy_api Lever : public Gear {\npublic:\n    void pull() { spin(); }\n};\n";
    let files = [("src/toy.h", h)];
    assert_eq!(
        cpp_targets(&files, "src/toy.h", "start"),
        ["spin() → src/toy.h:3"]
    );
    assert_eq!(
        cpp_targets(&files, "src/toy.h", "roll"),
        ["w.spin() → src/toy.h:11"]
    );
    // A lower-case word a project file defines is the macro too.
    assert_eq!(
        cpp_targets(&files, "src/toy.h", "pull"),
        ["spin() → src/toy.h:3"]
    );
}

#[test]
fn a_class_head_whose_macro_word_cannot_be_told_is_left_unread() {
    // Neither word is a project macro or in capitals: the head stays
    // misread, so its body's method is no item.
    let h = "struct Gear {\n    void spin() {}\n};\nclass Crank quiet : public Gear {\npublic:\n    void start() { spin(); }\n};\nclass Wheel {\npublic:\n    void spin() {}\n};\n";
    let p = plaz::source::project([("src/toy.h".to_string(), h.to_string())]);
    let lines: Vec<usize> = (p.files["src/toy.h"].code.items.iter())
        .filter(|it| it.name == "spin" || it.name == "start")
        .map(|it| it.start_line)
        .collect();
    assert_eq!(lines, [2, 10]);
}

#[test]
fn a_declaration_whose_header_has_an_unread_objective_c_twin_is_open() {
    let h =
        "bool touched(int f);\nstruct Pad {\n    bool held(int f);\n    bool both(int f);\n};\n";
    let def = "#include \"../pad.hpp\"\nbool touched(int f) { return f > 0; }\nbool Pad::held(int f) { return f < 0; }\nbool Pad::both(int f) { return touched(f) && held(f); }\n";
    let call = "#include \"pad.hpp\"\nbool any(Pad p) { return touched(1) || p.held(2); }\n";
    let twin = "bool touched(int f) { return false; }\n";
    let files = [
        ("src/pad.hpp", h),
        ("src/linux/pad.cpp", def),
        ("src/use.cpp", call),
    ];
    assert_eq!(
        cpp_targets(&files, "src/use.cpp", "any"),
        [
            "touched(1) → src/linux/pad.cpp:2",
            "p.held(2) → src/linux/pad.cpp:3"
        ]
    );
    let files = [files[0], files[1], files[2], ("src/mac/pad.mm", twin)];
    assert_eq!(
        cpp_targets(&files, "src/use.cpp", "any"),
        [
            "touched(1) → one of: src/linux/pad.cpp:2, or outside",
            "p.held(2) → one of: src/linux/pad.cpp:3, or outside"
        ]
    );
    // A call in the defining file reaches its own unit's definition.
    assert_eq!(
        cpp_targets(&files, "src/linux/pad.cpp", "both"),
        [
            "touched(f) → src/linux/pad.cpp:2",
            "held(f) → src/linux/pad.cpp:3"
        ]
    );
}

#[test]
fn a_parameter_of_a_callable_project_class_called_bare_is_its_call_operator() {
    let src = "struct Less {\n    bool operator()(int a, int b) const { return a < b; }\n};\nbool pick(Less& cmp, int a, int b) { return cmp(a, b); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "pick"),
        ["cmp(a, b) → src/a.cpp:2"]
    );
}

#[test]
fn a_bare_call_of_an_alias_is_a_construction_of_its_type_wherever_the_caller_stands() {
    let h = "#include <vector>\nnamespace geo {\ntemplate <typename T>\nstruct Duo {\n    T x, y;\n    Duo(T a, T b) : x(a), y(b) {}\n};\nusing DuoF = Duo<float>;\ntypedef Duo<int> DuoI;\ntemplate <typename T>\nusing Twin = Duo<T>;\n}\ntypedef std::vector<int> Ints;\n";
    let src = "#include \"vec.h\"\n#include <string>\nusing Str = std::string;\nstruct Outer : std::vector<int> {\n    void m();\n};\nvoid f() { geo::DuoF(1, 2); Str(\"x\"); Ints(3); }\nvoid Outer::m() { using namespace geo; DuoF(1, 2); DuoI(1, 2); Twin<float>(1, 2); Str(\"x\"); }\nvoid Loose::h() { using namespace geo; DuoF(1, 2); Ints(3); }\n";
    // Another file's alias of the name is out of this file's reach.
    let other = "#include \"vec.h\"\nusing Str = geo::Duo<char>;\n";
    let files = [("src/vec.h", h), ("src/a.cpp", src), ("src/b.cpp", other)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "f"),
        [
            "geo::DuoF(1, 2) → hidden",
            "Str(\"x\") → outside",
            "Ints(3) → outside"
        ]
    );
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "m"),
        [
            "DuoF(1, 2) → hidden",
            "DuoI(1, 2) → hidden",
            "Twin<float>(1, 2) → hidden",
            "Str(\"x\") → outside"
        ]
    );
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "h"),
        ["DuoF(1, 2) → hidden", "Ints(3) → outside"]
    );
}

#[test]
fn a_bare_call_of_an_alias_reads_outside_only_on_evidence_its_type_is_from_outside() {
    let h = "template <class T>\nstruct Box {\n    using value_type = T;\n    value_type zero() { return value_type(0); }\n};\ntemplate <class A>\nstruct Holder {\n    using size_type = typename A::size_type;\n    size_type count() { return size_type(1); }\n};\nstruct Buf {\n    Buf(int n);\n};\n";
    let other = "#include <vector>\nnamespace other {\nusing Buf = std::vector<char>;\n}\n";
    let src = "#include \"box.h\"\n#include \"other.h\"\nstruct Walk : std::vector<int> {\n    void go();\n};\nvoid Walk::go() { Buf(3); }\n";
    let files = [
        ("src/box.h", h),
        ("src/other.h", other),
        ("src/use.cpp", src),
    ];
    assert_eq!(
        cpp_targets(&files, "src/box.h", "zero"),
        ["value_type(0) → hidden"]
    );
    assert_eq!(
        cpp_targets(&files, "src/box.h", "count"),
        ["size_type(1) → hidden"]
    );
    // A class of the name in reach beats a namesake alias of an outside type.
    assert_eq!(
        cpp_targets(&files, "src/use.cpp", "go"),
        ["Buf(3) → hidden"]
    );
}

#[test]
fn a_function_with_a_macro_before_its_return_type_is_a_function() {
    // A long return type name (`tap_outcome_kind`) is what makes the
    // grammar read `TAP_API tap_outcome_kind tap_open` as `tap_outcome_kind::tap_open`.
    let h = "#define TAP_API extern\ntypedef int tap_outcome_kind;\ntypedef struct tap_dev tap_dev;\nTAP_API tap_outcome_kind tap_open(const tap_dev* d);\n";
    let src = "#include \"tap.h\"\nTAP_API tap_outcome_kind tap_open(const tap_dev* d)\n{\n    return d == 0;\n}";
    let call =
        "#include <tap.h>\nint use(const tap_dev* d) { return tap_open(d) + tap_shut(d); }\n";
    // Neither word tells which is the macro: no stronger than open.
    let other = "tap_layout tap_outcome_kind tap_shut(const tap_dev* d)\n{\n    return 0;\n}\n";
    let near = "#include \"../lib/tap.h\"\nint near(const tap_dev* d) { return tap_open(d); }\n";
    let files = [
        ("lib/tap.h", h),
        ("lib/tap.c", src),
        ("app/use.c", call),
        ("lib/shut.c", other),
        ("app/near.c", near),
    ];
    assert_eq!(
        cpp_targets(&files, "app/near.c", "near"),
        ["tap_open(d) → lib/tap.c:2"]
    );
    assert_eq!(
        cpp_targets(&files, "app/use.c", "use"),
        [
            "tap_open(d) → one of: lib/tap.c:2, or outside",
            "tap_shut(d) → one of: lib/shut.c:1, or outside"
        ]
    );
}

#[test]
fn a_method_with_a_macro_before_its_return_type_is_its_classs() {
    let h = "#define RUN_API\ntypedef unsigned int knob_u32;\ntypedef int outcome_kind_long;\nstruct Kern {\n    knob_u32 step(int a);\n    knob_u32 other_one(int a);\n};\nstruct Runner {\n    RUN_API outcome_kind_long run();\n    outcome_kind_long run_it();\n    outcome_kind_long run_short();\n};\n";
    let src = "#include \"kern.h\"\nknob_u32 Kern::other_one(int a) { return a; }\nFORCE_INLINE knob_u32 Kern::step(int a) { return other_one(a) + this->other_one(a); }\noutcome_kind_long Runner::run_short() { return 0; }\nRUN_API outcome_kind_long Runner::run_it() { return run_short(); }\nRUN_API outcome_kind_long Runner::run() { return 1; }\nint use(Kern k, Runner r) { return k.step(1) + r.run(); }\n";
    let files = [("src/kern.h", h), ("src/kern.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/kern.cpp", "step"),
        [
            "other_one(a) → src/kern.cpp:2",
            "this->other_one(a) → src/kern.cpp:2"
        ]
    );
    assert_eq!(
        cpp_targets(&files, "src/kern.cpp", "run_it"),
        ["run_short() → src/kern.cpp:4"]
    );
    assert_eq!(
        cpp_targets(&files, "src/kern.cpp", "use"),
        ["k.step(1) → src/kern.cpp:3", "r.run() → src/kern.cpp:6"]
    );
}

#[test]
fn a_misread_methods_bare_call_is_never_certain_to_a_free_namesake() {
    // Neither word is told the macro: the method stays misread, with no class.
    let h = "typedef int outcome_kind;\ntypedef int outcome_kind_long;\nstruct Runner {\n    outcome_kind_long run_it();\n    outcome_kind_long run_short();\n};\noutcome_kind_long run_short();\n";
    let src = "#include \"run.h\"\noutcome_kind_long Runner::run_short() { return 0; }\noutcome_kind_long run_short() { return 1; }\noutcome_kind outcome_kind_long Runner::run_it() { return run_short(); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/run.h", h), ("src/run.cpp", src)],
            "src/run.cpp",
            "run_it"
        ),
        ["run_short() → one of: src/run.cpp:2, src/run.cpp:3, or outside"]
    );
}

#[test]
fn a_misread_call_operators_bare_call_is_never_certain_to_a_free_namesake() {
    let h = "typedef int outcome_kind;\ntypedef int outcome_kind_long;\nstruct Runner {\n    outcome_kind_long operator()(int a);\n    outcome_kind_long run_short();\n};\noutcome_kind_long run_short();\n";
    let src = "#include \"run.h\"\noutcome_kind_long Runner::run_short() { return 0; }\noutcome_kind_long run_short() { return 1; }\noutcome_kind outcome_kind_long Runner::operator()(int a) { return run_short(); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/run.h", h), ("src/run.cpp", src)],
            "src/run.cpp",
            "operator()"
        ),
        ["run_short() → one of: src/run.cpp:2, src/run.cpp:3, or outside"]
    );
}
