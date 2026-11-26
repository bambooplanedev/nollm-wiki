mod common;

use common::headings;

use std::path::Path;

const A: &str = "use std::collections::BTreeMap;\nuse crate::b::Store;\n\npub fn run(x: u8, cb: impl Fn(u8), m: BTreeMap<u8, u8>, s: &Store) {\n    helper(x);\n    cb(x);\n    m.get(&x);\n    s.get(x);\n    BTreeMap::<u8, u8>::new();\n    std::mem::drop(x);\n    other_crate::go();\n    let t = make();\n    t.get(x);\n}\nfn helper(_x: u8) {}\nfn make() -> crate::b::Store { todo!() }\n";
const B: &str = "pub struct Store;\nimpl Store {\n    pub fn get(&self, _k: u8) {}\n}\npub struct Cache;\nimpl Cache {\n    pub fn get(&self, _k: u8) {}\n}\n";
const LIB: &str = "mod a;\nmod b;\n";

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
fn a_shell_script_with_no_extension_gets_a_map_page() {
    let src = "#!/bin/sh\nhelper() { :; }\nrun() {\n  helper\n}\n";
    let readme = "helper() { :; }\n";
    let (_d, vault) = common::built(&[("scripts/deploy", src), ("scripts/README", readme)]);
    assert_eq!(
        common::map_page(&vault, "map/scripts/deploy"),
        "---\ntitle: \"deploy · scripts\"\ntags: [\"plaz\"]\nkind: map\nsource: \"scripts/deploy\"\n---\nCode: `scripts/deploy:1-5`\n\nNot covered yet: shell flowcharts.\n"
    );
    // A file with no extension and no shebang gets no page.
    let mut pages: Vec<_> = std::fs::read_dir(vault.join("map/scripts"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    pages.sort();
    assert_eq!(pages, ["deploy.md"]);
}

#[test]
fn a_closure_calling_an_outer_parameter_is_hidden_even_when_a_project_function_shares_the_name() {
    let src = "pub fn cb() {}\npub fn f(cb: impl Fn()) {\n    let run = || {\n        cb();\n    };\n    run();\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let calls = common::fn_section(&common::map_page(&vault, "map/src/a_rs"), "f", "Calls");
    assert_eq!(
        calls,
        [
            "`cb()` L4 → target not visible",
            "`run()` L6 → the closure `run`, drawn in this chart"
        ]
    );
}

#[test]
fn a_let_does_not_see_its_own_name_in_its_initializer() {
    let src = "pub fn level(n: u8) -> u8 { n }\npub fn grow(v: u8, w: u8) -> u8 { v + w }\npub fn f(node: u8) -> u8 {\n    let level = level(node);\n    let grow = |v| grow(v, 1);\n    grow(level)\n}\npub fn g(level: fn(u8) -> u8) -> u8 {\n    let level = level(2);\n    level\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    assert_eq!(
        common::fn_section(&page, "f", "Calls"),
        [
            "`level(node)` L4 → [[map/src/a_rs#level|level]]",
            "`grow(v, 1)` L5 → [[map/src/a_rs#grow|grow]]",
            // The closure bound by the `let` above.
            "`grow(level)` L6 → target not visible",
        ]
    );
    assert_eq!(
        common::fn_section(&page, "level", "Called from"),
        ["certainly: [[map/src/a_rs#f|f]] L4"]
    );
    // A parameter of that name is what the initializer calls.
    assert_eq!(
        common::fn_section(&page, "g", "Calls"),
        ["`level(2)` L9 → target not visible"]
    );
}

#[test]
fn closures_and_nested_functions_are_linked_to_the_calls_that_take_or_call_them() {
    let src = "pub fn sort_all(v: &mut Vec<u8>, w: &mut Vec<u8>) {\n    let fits = |x: &u8| {\n        if *x > 3 { return 1; }\n        0\n    };\n    v.sort_by_key(fits);\n    w.retain(|x| {\n        if *x > 2 { return true; }\n        false\n    });\n    fits(1);\n    fn tidy(n: u8) -> u8 { if n > 1 { tidy(n - 1) } else { 0 } }\n    tidy(2);\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let block = common::fn_block(&common::map_page(&vault, "map/src/a_rs"), "sort_all");
    assert_eq!(
        common::dotted_edges(&block),
        [
            "[fits] -passed to-> v.sort_by_key(fits)",
            "[closure L7] -passed to-> w.retain(#124;x#124; #123; if *x #gt; 2 #123; return true; #125; false #125;)",
            "fits(1) -calls-> [fits]",
            "tidy(2) -calls-> [tidy]",
        ]
    );
    assert!(block.contains("(\"fits(1)\")\n"), "{block}");
    assert_eq!(
        common::fn_section(&block, "sort_all", "Calls"),
        [
            "`v.sort_by_key(fits)` L6 → outside the project",
            "`w.retain(|x| { if *x > 2 { return true; } false })` L7 → outside the project",
            "`fits(1)` L11 → the closure `fits`, drawn in this chart",
            // Calling itself: no edge from inside its subgraph to it.
            "`tidy(n - 1)` L12 → the nested function `tidy`, drawn in this chart",
            "`tidy(2)` L13 → the nested function `tidy`, drawn in this chart",
        ]
    );
    assert!(
        block.contains("\nCoverage: 5 calls: 0 resolved, 0 one of, 2 outside, 0 hidden, 3 local. Not drawn: none.\n"),
        "{block}"
    );
}

#[test]
fn a_name_bound_twice_leaves_its_calls_unlinked() {
    let src = "pub fn twice(flag: bool) {\n    let pick = |x: u8| { if x > 1 { return 1; } 0 };\n    pick(1);\n    let pick = |x: u8| { if x > 2 { return 2; } 0 };\n    pick(2);\n}\npub fn param(pick: u8) {\n    fn pick() { if true {} }\n    pick();\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    for (f, calls) in [
        (
            "twice",
            vec![
                "`pick(1)` L3 → target not visible",
                "`pick(2)` L5 → target not visible",
            ],
        ),
        ("param", vec!["`pick()` L9 → target not visible"]),
    ] {
        let block = common::fn_block(&page, f);
        assert!(common::dotted_edges(&block).is_empty(), "{block}");
        assert_eq!(common::fn_section(&block, f, "Calls"), calls);
    }
}

#[test]
fn a_python_nested_def_is_linked_to_the_calls_that_take_or_call_it() {
    let src = "def outer(rows):\n    def keep(r):\n        if r > 2:\n            return True\n        return False\n    picked = list(filter(keep, rows))\n    if keep(1):\n        return picked\n    return []\n";
    let (_d, vault) = common::built(&[("app/m.py", src)]);
    let block = common::fn_block(&common::map_page(&vault, "map/app/m_py"), "outer");
    assert_eq!(
        common::dotted_edges(&block),
        [
            "[keep] -passed to-> filter(keep, rows)",
            "keep(1) -calls-> [keep]",
        ]
    );
    assert_eq!(
        common::fn_section(&block, "outer", "Calls"),
        [
            "`filter(keep, rows)` L6 → outside the project",
            "`list(filter(keep, rows))` L6 → outside the project",
            "`keep(1)` L7 → the nested function `keep`, drawn in this chart",
        ]
    );
}

#[test]
fn chain_steps_and_wrappers_outside_the_project_are_not_drawn() {
    let src = "pub struct Store;\nimpl Store { pub fn load(&self) -> Vec<u8> { Vec::new() } }\npub fn helper(n: usize) -> usize { n }\npub fn shape(v: Vec<u8>, s: &Store) -> Option<usize> {\n    let n = v.iter().map(|x| x + 1).filter(|x| *x > 2).count();\n    let k = s.load().iter().count();\n    if n > k {\n        return Some(helper(n));\n    }\n    let e: Result<u8, u8> = Err(2);\n    None\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let block = common::fn_block(&common::map_page(&vault, "map/src/a_rs"), "shape");
    assert_eq!(
        common::node_labels(&block),
        [
            "shape",
            "v.iter().map(#124;x#124; x + 1).filter(#124;x#124; *x #gt; 2).count()",
            // A project call inside a chain stays a node of its own.
            "s.load()",
            "s.load().iter().count()",
            "n #gt; k",
            "helper(n)",
            "return Some(helper(n))",
            "end",
        ]
    );
    // The Calls list keeps every call.
    assert_eq!(common::fn_section(&block, "shape", "Calls").len(), 10);
    assert!(
        block.contains("\nCoverage: 10 calls: 2 resolved, 0 one of, 8 outside, 0 hidden. Not drawn: 2 closure, 4 chain step, 2 Some/Ok/Err.\n"),
        "{block}"
    );
}

#[test]
fn a_python_chain_is_drawn_as_its_outermost_call() {
    let src = "def tidy(parts):\n    return \" \".join(parts).strip().lower()\n";
    let (_d, vault) = common::built(&[("app/m.py", src)]);
    let block = common::fn_block(&common::map_page(&vault, "map/app/m_py"), "tidy");
    assert_eq!(
        common::node_labels(&block),
        [
            "tidy",
            "#quot; #quot;.join(parts).strip().lower()",
            "return #quot; #quot;.join(parts).strip().lower()"
        ]
    );
    assert_eq!(common::fn_section(&block, "tidy", "Calls").len(), 3);
}

#[test]
fn a_call_is_resolved_from_its_whole_text_not_the_text_shown() {
    let inner = "def measure_everything_carefully_at_once(*a):\n    return a\n";
    let outer = "import pkg.gauges\n\ndef run(v):\n    return pkg.gauges.measure_everything_carefully_at_once(v, v + 1, v + 2, v + 3, v + 4, v + 5)\n";
    let (_d, vault) = common::built(&[
        ("pkg/__init__.py", ""),
        ("pkg/gauges.py", inner),
        ("app.py", outer),
    ]);
    assert_eq!(
        common::fn_section(&common::map_page(&vault, "map/app_py"), "run", "Calls"),
        ["`pkg.gauges.measure_everything_carefully_…e(v, v + 1, v + 2, v + 3, v + 4, v + 5)` L4 → [[map/pkg/gauges_py#measure_everything_carefully_at_once|measure_everything_carefully_at_once]]"]
    );
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
        .stdout("map: 5 written, 0 unchanged, 0 removed\n")
        .stderr("");
    assert!(d.path().join(".nollm-wiki/map/src/a_rs.md").exists());

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
        .stdout("map: 3 written, 0 unchanged, 0 removed\n")
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
        .stdout("map: 3 written, 0 unchanged, 0 removed, 2 left alone\n")
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
        .stdout("map: 2 written, 0 unchanged, 0 removed, 1 left alone\n")
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

#[test]
fn a_path_on_a_prelude_or_primitive_type_is_outside() {
    let a = "pub fn f(x: u8) {\n    Vec::<u8>::new();\n    String::new();\n    Box::new(x);\n    Option::Some(x);\n    usize::from(x);\n    char::from_u32(1);\n}\n";
    let calls = calls_of(&[("src/a.rs", a)], "f");
    assert_eq!(calls.len(), 6, "{calls:#?}");
    assert!(
        calls.iter().all(|c| c.ends_with("→ outside the project")),
        "{calls:#?}"
    );
}

#[test]
fn a_path_on_a_prelude_type_lists_only_trait_impls_for_that_type() {
    // `impl From<Name> for Wrap` is no `usize::from`; a blanket impl on a
    // generic parameter may be, which its trait's own method stands for.
    let a = "pub struct Name;\npub struct Wrap;\nimpl From<Name> for Wrap {\n    fn from(_n: Name) -> Wrap { Wrap }\n}\nimpl From<Name> for Box<u8> {\n    fn from(_n: Name) -> Box<u8> { todo!() }\n}\npub trait Show {\n    fn show(&self);\n}\nimpl<T: Copy> Show for T {\n    fn show(&self) {}\n}\nimpl Show for Wrap {\n    fn show(&self) {}\n}\npub fn f(n: Name) {\n    usize::from(1u8);\n    Box::from(n);\n    u8::show(&1);\n}\n";
    let calls = calls_of(&[("src/a.rs", a)], "f");
    assert_eq!(calls[0], "`usize::from(1u8)` L19 → outside the project");
    assert_eq!(calls[1], "`Box::from(n)` L20 → one of: [[map/src/a_rs#box_u8_as_from_name_from|<Box<u8> as From<Name>>::from]], or outside the project");
    assert_eq!(calls[2], "`u8::show(&1)` L21 → one of: [[map/src/a_rs#show_show|Show::show]], or outside the project");
}

#[test]
fn a_prelude_type_name_the_project_has_is_not_outside() {
    // A project type of the name and an alias of the name read as before;
    // a project trait method, or a trait impl on the type, may be called.
    let a = "use crate::b::Gear as Vec;\npub trait Tally {\n    fn tally(&self) -> usize { 0 }\n}\nimpl Tally for Option<u8> {}\npub struct Name;\nimpl From<Name> for Box<u8> {\n    fn from(_n: Name) -> Box<u8> { todo!() }\n}\npub fn f(n: Name, o: Option<u8>) {\n    String::new();\n    Vec::new();\n    Option::tally(&o);\n    Box::from(n);\n}\n";
    let b = "pub struct String;\npub struct Gear;\n";
    let files = [
        ("src/lib.rs", "mod a;\nmod b;\n"),
        ("src/a.rs", a),
        ("src/b.rs", b),
    ];
    assert_eq!(
        calls_of(&files, "f"),
        [
            "`String::new()` L11 → target not visible",
            "`Vec::new()` L12 → target not visible",
            "`Option::tally(&o)` L13 → one of: [[map/src/a_rs#tally_tally|Tally::tally]], or outside the project",
            "`Box::from(n)` L14 → one of: [[map/src/a_rs#box_u8_as_from_name_from|<Box<u8> as From<Name>>::from]], or outside the project",
        ]
    );
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
            "`helper()` L9 → the nested function `helper`, drawn in this chart",
            "`S::new()` L10 → one of: [[map/src/a_rs#s_new|S::new]], or outside the project",
            "`helper()` L12 → the nested function `helper`, drawn in this chart",
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
            "`Vec::new()` L2 → outside the project",
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
    assert_eq!(
        plaz::render::mermaid_label("w(s, \"\\n\")"),
        "w(s, #quot;#92;n#quot;)"
    );
    assert_eq!(plaz::render::mermaid_label("a: b"), "a#58; b");
    assert_eq!(plaz::render::mermaid_label("$$x$$"), "#36;#36;x#36;#36;");
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
        f.contains("\nCoverage: 5 calls: 0 resolved, 0 one of, 5 outside, 0 hidden. Not drawn: 1 ?, 1 unwrap_or, 1 closure, 2 chain step.\n"),
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
    assert!(
        page.contains("comment#58; 7 lines at src/a.rs#58;2-8"),
        "{page}"
    );
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
        chart.contains("#lt;W#lt;T#gt; as std#58;#58;fmt#58;#58;Display#gt;#58;#58;fmt"),
        "{chart}"
    );
}

#[test]
fn huge_charts_are_whole_and_point_to_the_calls_list() {
    // 600 edges: over what mermaid.js draws, under the character limit.
    // Calls of a project function: each a node of its own.
    let body = (0..600).fold(String::new(), |s, i| s + &format!("    g({i});\n"));
    let src = |body| format!("fn f() {{\n{body}}}\nfn g(_: u32) {{}}\n");
    let (_d, vault) = common::built(&[("src/a.rs", &src(body))]);
    let page = common::map_page(&vault, "map/src/a_rs");
    let chart = page.split("```mermaid\n").nth(1).unwrap();
    assert!(chart.split_once("\n```\n").unwrap().0.contains("g(599)"));
    assert!(page.contains("\n```\n\nThis chart exceeds the 500-edge limit of Mermaid.js renderers other than Obsidian's; the Calls list below is the complete view.\n"));
    // over 50,000 characters no renderer draws it: no block, only the notice
    let body = (0..3000).fold(String::new(), |s, i| s + &format!("    g({i});\n"));
    let (_d, vault) = common::built(&[("src/a.rs", &src(body))]);
    let page = common::map_page(&vault, "map/src/a_rs");
    assert!(
        !common::fn_block(&page, "f").contains("```mermaid"),
        "{page}"
    );
    assert!(page.contains("\nThis chart exceeds Mermaid's 50,000-character limit and is not drawn; the Calls list below is the complete view.\n"));
    assert!(page.contains("g(2999)"), "the Calls list stays");
    let (_d, vault) = common::built(&[("src/a.rs", F_SRC)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    assert!(page.contains("```mermaid") && !page.contains("This chart exceeds"));
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
    assert_eq!(
        edges,
        [
            "g --> 'outer#58; for x in xs",
            "'outer#58; for x in xs -loop-> for y in xs",
            "for y in xs -loop-> y #gt; x",
            "y #gt; x -yes-> continue 'outer",
            "continue 'outer --> 'outer#58; for x in xs",
            "y #gt; x -no-> y == x",
            "y == x -yes-> break 'outer",
            "y == x -no-> *y == 0",
            "*y == 0 -yes-> continue",
            "continue --> for y in xs",
            "*y == 0 -no-> *y == 9",
            "*y == 9 -yes-> return",
            "*y == 9 -no-> w()",
            "w() --> for y in xs",
            "for y in xs -done-> 'outer#58; for x in xs",
            "break 'outer --> z()",
            "'outer#58; for x in xs -done-> z()",
            "z() --> end",
        ]
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
            "match k --> Kind#58;#58;A",
            "Kind#58;#58;A -yes-> a()",
            "Kind#58;#58;A -no-> Kind#58;#58;B",
            "Kind#58;#58;B -yes-> return 2",
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
    assert!(f.contains(&"q(t).execute(p).await.unwrap_or_else(#124;e#124; panic!(#quot;add #123;t#125;#58; #123;e#125;#quot;)) --> for t in [#quot;a#quot;, #quot;b#quot;]".to_string()), "{f:#?}");
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

/// The edges of `anchor`'s chart on the page of `path`, built from `files`.
fn edges_in(files: &[(&str, &str)], path: &str, anchor: &str) -> Vec<String> {
    let (_d, vault) = common::built(files);
    let id = format!("map/{}", path.replace('.', "_"));
    chart_edges(&common::map_page(&vault, &id), anchor)
}

#[test]
fn a_certain_call_of_a_c_function_that_never_returns_ends_the_path() {
    let h = "#define NORETURN __attribute__((noreturn))\nNORETURN void die(const char *m);\n[[noreturn]] void halt(int c);\n_Noreturn void stop(int c);\nvoid quit(int c) __attribute__((noreturn));\nint fail(int c);\n";
    let c = "#include \"a.h\"\nvoid die(const char *m) { report(m); }\nvoid halt(int c) { report(c); }\nint run(int v) {\n    if (v < 0) die(\"neg\");\n    if (v == 1) halt(1);\n    if (v == 2) stop(1);\n    if (v == 3) quit(1);\n    if (v == 4) fail(1);\n    return v * 2;\n}\n";
    let e = edges_in(&[("src/a.h", h), ("src/a.c", c)], "src/a.c", "run");
    for (call, next) in [
        ("die(#quot;neg#quot;)", "v == 1"),
        ("halt(1)", "v == 2"),
        ("stop(1)", "v == 3"),
        ("quit(1)", "v == 4"),
    ] {
        assert!(e.contains(&format!("{call} --> {call}")), "{call}: {e:#?}");
        assert!(!e.contains(&format!("{call} --> {next}")), "{call}: {e:#?}");
    }
    // A function with no such attribute returns.
    assert!(
        e.contains(&"fail(1) --> return v * 2;".to_string()),
        "{e:#?}"
    );
}

#[test]
fn a_noreturn_call_that_is_one_of_several_or_inside_an_expression_continues() {
    let crash = "__attribute__((noreturn)) void crash(int c) { abort(); }\n";
    let c = "[[noreturn]] void die(int c);\nvoid on_fault(void (*h)(int) __attribute__((noreturn)));\nint run(int v) {\n    crash(1);\n    on_fault(die);\n    ok(v) || die(2);\n    return v;\n}\n";
    let files = [("src/b.c", crash), ("src/d.c", crash), ("src/a.c", c)];
    let e = edges_in(&files, "src/a.c", "run");
    assert!(
        e.contains(&"crash(1) --> on_fault(die)".to_string()),
        "{e:#?}"
    );
    // A parameter that never returns says nothing of the function.
    assert!(e.contains(&"on_fault(die) --> ok(v)".to_string()), "{e:#?}");
    assert!(e.contains(&"die(2) --> return v;".to_string()), "{e:#?}");
}

#[test]
fn a_python_exit_ends_the_path_unless_its_name_is_rebound() {
    let src = "import sys\nimport os\n\ndef f(v):\n    if v < 0:\n        sys.exit(1)\n    if v == 1:\n        os._exit(2)\n    if v == 2:\n        exit(3)\n    if v == 3:\n        quit(4)\n    return v * 2\n\ndef g(v):\n    sys = make(v)\n    sys.exit(1)\n    return v\n";
    let page = page_of_py(src);
    let f = chart_edges(&page, "f");
    for call in ["sys.exit(1)", "os._exit(2)", "exit(3)", "quit(4)"] {
        assert!(f.contains(&format!("{call} --> {call}")), "{call}: {f:#?}");
    }
    assert!(!f.contains(&"sys.exit(1) --> v == 1".to_string()), "{f:#?}");
    let g = chart_edges(&page, "g");
    assert!(
        g.contains(&"make(v)<br/>sys.exit(1) --> return v".to_string()),
        "{g:#?}"
    );
    // A project function named `exit` is the one called.
    let own = "def exit(c):\n    return c\n\ndef h(v):\n    exit(1)\n    return v\n";
    let h = edges_of_py(own, "h");
    assert!(h.contains(&"exit(1) --> return v".to_string()), "{h:#?}");
}

#[test]
fn a_rust_process_exit_or_a_never_returning_fn_ends_the_path() {
    let src = "use std::process;\nfn fail(m: &str) -> ! { panic!(\"{m}\") }\nfn f(v: i32) -> i32 {\n    if v < 0 { std::process::exit(1); }\n    if v == 1 { process::exit(2) }\n    if v == 2 { fail(\"x\"); }\n    v * 2\n}\nfn g(o: Option<i32>) {\n    let n = o.map(|x| match x { 1 => keep(), _ => process::exit(x) });\n    after(n);\n}\n";
    let f = edges_of(src, "f");
    for call in [
        "std#58;#58;process#58;#58;exit(1)",
        "process#58;#58;exit(2)",
        "fail(#quot;x#quot;)",
    ] {
        assert!(f.contains(&format!("{call} --> {call}")), "{call}: {f:#?}");
    }
    assert!(
        !f.contains(&"std#58;#58;process#58;#58;exit(1) --> v == 1".to_string()),
        "{f:#?}"
    );
    // In a closure the exit ends the closure's own flow, not the path.
    let g = edges_of(src, "g");
    let exit = "process#58;#58;exit(x)";
    assert!(g.contains(&format!("{exit} --> {exit}")), "{g:#?}");
    assert!(g.iter().any(|e| e.ends_with("--> after(n)")), "{g:#?}");
}

#[test]
fn a_project_function_named_exit_that_returns_continues() {
    let lib = "mod process;\nfn h() {\n    process::exit(1);\n    after();\n}\n";
    let process = "pub fn exit(_c: i32) {}\n";
    let e = edges_in(
        &[("src/lib.rs", lib), ("src/process.rs", process)],
        "src/lib.rs",
        "h",
    );
    assert!(
        e.contains(&"process#58;#58;exit(1) --> after()".to_string()),
        "{e:#?}"
    );
}

/// Is the call labelled `call` drawn as an end of the path in `edges`?
fn ends_at(edges: &[String], call: &str) -> bool {
    edges.contains(&format!("{call} --> {call}"))
}

#[test]
fn a_noreturn_call_in_a_macro_argument_does_not_end_the_path() {
    let c = "[[noreturn]] void raise_now(void);\nint run(int v) {\n    EXPECT_FAILS({ count++; raise_now(); }, \"msg\");\n    after(v);\n    return v;\n}\n";
    let e = edges_in(&[("src/a.c", c)], "src/a.c", "run");
    assert!(!ends_at(&e, "raise_now()"), "{e:#?}");
    // Folded or not, the path goes on to `after(v)`.
    assert!(
        e.iter()
            .any(|l| l.contains(" --> ") && l.ends_with("after(v)")),
        "{e:#?}"
    );
}

#[test]
fn a_rust_never_returning_call_in_a_macro_or_an_argument_closure_does_not_end_the_path() {
    let src = "fn fail() -> ! { panic!() }\nfn f() {\n    assert_panics!({ fail(); });\n    after();\n}\nfn g() {\n    let r = std::panic::catch_unwind(|| { fail(); });\n    after(r);\n}\n";
    let f = edges_of(src, "f");
    assert!(!ends_at(&f, "fail()"), "{f:#?}");
    assert!(f.iter().any(|l| l.ends_with("--> after()")), "{f:#?}");
    let g = edges_of(src, "g");
    assert!(g.iter().any(|l| l.ends_with("--> after(r)")), "{g:#?}");
}

#[test]
fn a_python_exit_in_a_lambda_or_a_try_catching_it_does_not_end_the_path() {
    let src = "import sys\n\ndef f(v):\n    try:\n        sys.exit(1)\n    except SystemExit:\n        pass\n    after(v)\n\ndef g(v):\n    run(lambda: sys.exit(1))\n    after(v)\n\ndef h(v):\n    try:\n        sys.exit(2)\n    except:\n        pass\n    try:\n        sys.exit(3)\n    except ValueError:\n        pass\n";
    let page = page_of_py(src);
    let f = chart_edges(&page, "f");
    assert!(!ends_at(&f, "sys.exit(1)"), "{f:#?}");
    let g = chart_edges(&page, "g");
    assert!(
        g.iter()
            .any(|l| l.contains(" --> ") && l.ends_with("after(v)")),
        "{g:#?}"
    );
    // A bare `except:` catches the exit; `except ValueError` does not.
    let h = chart_edges(&page, "h");
    assert!(!ends_at(&h, "sys.exit(2)"), "{h:#?}");
    assert!(ends_at(&h, "sys.exit(3)"), "{h:#?}");
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
            "loop -loop-> rx.recv()<br/>m.is_none()",
            "rx.recv()<br/>m.is_none() --> loop",
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
            "loop -loop-> rx.recv()<br/>m.is_none()",
            "rx.recv()<br/>m.is_none() --> loop",
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
            "loop -loop-> rx.recv()<br/>m.is_none()",
            "rx.recv()<br/>m.is_none() --> loop",
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
            "f --> compute()<br/>use_it(v)",
            "compute()<br/>use_it(v) --> end"
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
            "f --> 'o#58; for x in xs".to_string(),
            "closure L3 --> y #gt; x".to_string(),
            "y #gt; x -yes-> return false".to_string(),
            "y #gt; x -no-> end".to_string(),
            format!("'o#58; for x in xs -loop-> {all}"),
            format!("{all} --> ok"),
            "ok -yes-> continue".to_string(),
            "continue --> 'o#58; for x in xs".to_string(),
            "closure L5 --> loop".to_string(),
            "loop -loop-> y".to_string(),
            "y -yes-> break 'o".to_string(),
            "y -no-> t()".to_string(),
            "t() --> loop".to_string(),
            format!("ok -no-> {each}"),
            format!("{each} --> 'o#58; for x in xs"),
            "'o#58; for x in xs -done-> end".to_string(),
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
            "loop -loop-> rx.recv()<br/>m.is_none()",
            "rx.recv()<br/>m.is_none() --> loop",
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
            "loop -loop-> stop.recv()<br/>rx.recv()<br/>handle(m)",
            "stop.recv()<br/>rx.recv()<br/>handle(m) --> loop",
            "loop -exit not drawn-> end",
        ]
    );
    let spawn = "spawn(async move #123; loop #123; tokio#58;#58;select! #123; _ = cancel.recv() =#gt; break, #125; #125; #125;)";
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
fn python_calls_to_parameters_are_hidden_and_to_nested_defs_local() {
    let a = "def decide(messages, ask):\n    return ask(messages)\n\ndef outer(x):\n    def inner(y):\n        return y\n    return inner(x)\n\ndef ask(m):\n    pass\n\ndef inner(y):\n    pass\n";
    let (_d, vault) = common::built(&[("a.py", a)]);
    let page = common::map_page(&vault, "map/a_py");
    assert_eq!(
        common::fn_section(&page, "decide", "Calls"),
        ["`ask(messages)` L2 → target not visible"]
    );
    assert_eq!(
        common::fn_section(&page, "outer", "Calls"),
        ["`inner(x)` L7 → the nested function `inner`, drawn in this chart"]
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

const GAUGE: &str = "class Gauge:\n    def __init__(self, v):\n        self.v = v\n\nclass Base:\n    def __init__(self, v):\n        self.v = v\n\nclass Dial(Base):\n    def turn(self):\n        pass\n\ndef make(v):\n    Gauge(v)\n    Dial(v)\n";

#[test]
fn a_python_construction_of_a_project_class_links_its_init() {
    let b = "from a import Gauge\n\ndef build(v):\n    Gauge(v)\n";
    let files = [("a.py", GAUGE), ("b.py", b)];
    assert_eq!(
        calls_in(&files, "map/a_py", "make"),
        [
            "`Gauge(v)` L14 → [[map/a_py#gauge_init|Gauge::__init__]]",
            "`Dial(v)` L15 → one of: [[map/a_py#base_init|Base::__init__]], or outside the project",
        ]
    );
    assert_eq!(
        calls_in(&files, "map/b_py", "build"),
        ["`Gauge(v)` L4 → [[map/a_py#gauge_init|Gauge::__init__]]"]
    );
    let (_d, vault) = common::built(&files);
    let called = common::fn_section(
        &common::map_page(&vault, "map/a_py"),
        "gauge_init",
        "Called from",
    );
    assert_eq!(
        called,
        [
            "certainly: [[map/a_py#make|make]] L14",
            "certainly: [[map/b_py#build|build]] L4"
        ]
    );
}

#[test]
fn a_python_construction_that_may_not_run_the_init_stays_hidden() {
    let a = "from enum import Enum\n\nclass Meta(type):\n    pass\n\nclass Pooled:\n    def __new__(cls, v):\n        return cls\n\n    def __init__(self, v):\n        self.v = v\n\nclass Typed(metaclass=Meta):\n    def __init__(self, v):\n        self.v = v\n\nclass Color(Enum):\n    def __init__(self, v):\n        self.v = v\n\nclass Bare:\n    pass\n\ndef make(v):\n    Pooled(v)\n    Typed(v)\n    Color(v)\n    Bare()\n";
    assert_eq!(
        calls_in(&[("a.py", a)], "map/a_py", "make"),
        [
            "`Pooled(v)` L25 → target not visible",
            "`Typed(v)` L26 → target not visible",
            "`Color(v)` L27 → target not visible",
            "`Bare()` L28 → target not visible",
        ]
    );
    // Two classes of the name and no import saying which: as before.
    let two = "class Gauge:\n    def __init__(self, v):\n        pass\n";
    let c = "def build(v):\n    Gauge(v)\n";
    let files = [("a.py", two), ("d.py", two), ("c.py", c)];
    let built = calls_in(&files, "map/c_py", "build");
    assert!(!built[0].contains("__init__"), "{built:?}");
}

#[test]
fn a_python_class_its_module_may_rebind_from_outside_stays_hidden() {
    // A star import from outside, after the class, may replace it.
    let starred = "class Getter:\n    def __init__(self, k):\n        self.k = k\n\ndef make(k):\n    Getter(k)\n\nfrom _speedups import *\n";
    // An outside import binds the name; the class is only the fallback.
    let fallback = "try:\n    from types import SimpleNamespace as Holder\nexcept ImportError:\n    class Holder:\n        def __init__(self, **kw):\n            pass\n";
    let user = "from starred import Getter\nfrom fallback import Holder\n\ndef build(k):\n    Getter(k)\n    Holder(k=k)\n";
    let files = [
        ("starred.py", starred),
        ("fallback.py", fallback),
        ("user.py", user),
    ];
    assert_eq!(
        calls_in(&files, "map/user_py", "build"),
        [
            "`Getter(k)` L5 → target not visible",
            "`Holder(k=k)` L6 → target not visible",
        ]
    );
    assert_eq!(
        calls_in(&files, "map/starred_py", "make"),
        ["`Getter(k)` L6 → target not visible"]
    );
}

#[test]
fn a_python_class_imported_through_a_module_that_may_rebind_it_stays_hidden() {
    // `kit` takes its names from a C module when there is one.
    let kit = "try:\n    from _ckit import *\nexcept ImportError:\n    from _pykit import *\n";
    let pykit = "class Base:\n    def __init__(self):\n        pass\n\nclass Thing:\n    def __init__(self, v):\n        pass\n";
    let user = "from kit import Thing, Base\n\nclass Mine(Base):\n    def __init__(self):\n        pass\n\ndef build(v):\n    Thing(v)\n    Mine()\n";
    let files = [("kit.py", kit), ("_pykit.py", pykit), ("user.py", user)];
    assert_eq!(
        calls_in(&files, "map/user_py", "build"),
        [
            "`Thing(v)` L8 → target not visible",
            "`Mine()` L9 → target not visible",
        ]
    );
}

#[test]
fn a_python_class_with_a_base_the_caller_does_not_see_stays_hidden() {
    // `enum.Enum` is no project class, whatever project class `Enum` exists;
    // nor is a base no import or definition of the file names.
    let a = "import enum\n\nclass Color(enum.Enum):\n    def __init__(self, v):\n        self.v = v\n\nclass Child(Parent):\n    def __init__(self, v):\n        self.v = v\n\ndef make(v):\n    Color(v)\n    Child(v)\n";
    let b = "class Enum:\n    def __init__(self):\n        pass\n\nclass Parent:\n    def __init__(self, v):\n        pass\n";
    assert_eq!(
        calls_in(&[("a.py", a), ("b.py", b)], "map/a_py", "make"),
        [
            "`Color(v)` L12 → target not visible",
            "`Child(v)` L13 → target not visible",
        ]
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
            "work() --> close()<br/>after()",
            "close()<br/>after() --> end",
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
            "d() --> e()<br/>g()",
            "b() --> e()<br/>g()",
            "e()<br/>g() --> end",
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
        .stdout("map: 3 written, 0 unchanged, 0 removed\n")
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
        .stdout("map: 3 written, 0 unchanged, 0 removed\n");
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
        .stdout("map: 3 written, 0 unchanged, 0 removed\n");
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
    out.stdout("map: 3 written, 0 unchanged, 0 removed\n")
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
        .stdout("map: 3 written, 0 unchanged, 0 removed\n");
}

// Pins the 512 MiB build stack: the flow walker recurses once per tree level.
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
    assert_eq!((w.written, w.removed, w.left_alone), (3, 0, 0));
    for (path, text) in wiki {
        assert_eq!(std::fs::read_to_string(vault.join(path)).unwrap(), text);
    }
}

#[test]
fn an_empty_comment_draws_no_note() {
    for (path, src) in [
        (
            "src/a.rs",
            "fn f(x: u8) {\n    //\n    if x > 1 { g(); }\n}\n",
        ),
        ("a.py", "def f(x):\n    #\n    if x > 1:\n        g()\n"),
        (
            "a.cpp",
            "void f(int x) {\n    //\n    if (x > 1) { g(); }\n}\n",
        ),
    ] {
        let (_d, vault) = common::built(&[(path, src)]);
        let id = format!("map/{}", path.replace('.', "_"));
        let page = common::map_page(&vault, &id);
        assert!(page.contains("x #gt; 1"), "{path}: {page}");
        assert!(!page.contains("\"\""), "{path}: {page}");
    }
}

/// The line under the signature of `anchor`, if it is a locals line.
fn locals_of(page: &str, anchor: &str) -> Vec<String> {
    let block = common::fn_block(page, anchor);
    block
        .lines()
        .skip(2)
        .take_while(|l| !l.starts_with("**Called from**"))
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

#[test]
fn rust_locals_line_lists_body_names_not_parameters_or_unit_patterns() {
    let src = "fn tally(width: u8, ready: bool) -> u8 {\n    let count = 1;\n    let item: Entry = make();\n    let bag: BTreeMap<u8, u8> = make();\n    let rest = width;\n    match find() {\n        Some(got) => {}\n        None => {}\n        LIMIT => {}\n    }\n    if let Some(hit) = look() {}\n    if let None = look() {}\n    fn deep() { let own = 1; }\n    count\n}\nfn bare(a: u8) { a; }\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    assert_eq!(
        locals_of(&page, "tally"),
        ["Locals: `bag: BTreeMap<u8, u8>`, `count`, `got`, `hit`, `item: Entry`, `rest`"]
    );
    assert_eq!(locals_of(&page, "bare"), Vec::<String>::new());
}

#[test]
fn a_local_shadowing_a_parameter_is_on_the_locals_line() {
    let rust =
        "fn open(original: &str) {\n    let original = original.trim();\n    keep(original);\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", rust)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    assert_eq!(locals_of(&page, "open"), ["Locals: `original`"]);
    // Python: assigning a parameter rebinds it; no new name.
    let py = "def open(items):\n    items = items or []\n    return items\n";
    let (_d, vault) = common::built(&[("a.py", py)]);
    let page = common::map_page(&vault, "map/a_py");
    assert_eq!(locals_of(&page, "open"), Vec::<String>::new());
}

#[test]
fn a_python_case_pattern_binds_its_captures_only() {
    // A class pattern's head, a dotted value and a keyword bind nothing.
    let py = "def sort(v):\n    match v:\n        case str():\n            pass\n        case Color.RED:\n            pass\n        case Point(x=0, y=depth):\n            pass\n        case [first, *rest] as whole:\n            pass\n";
    let (_d, vault) = common::built(&[("a.py", py)]);
    let page = common::map_page(&vault, "map/a_py");
    assert_eq!(
        locals_of(&page, "sort"),
        ["Locals: `depth`, `first`, `rest`, `whole`"]
    );
}

#[test]
fn python_locals_line_moves_global_names_to_outside_names() {
    let src = "def tally(width, ready: bool):\n    global COUNT\n    count = 1\n    item: Entry = make()\n    for rest in width:\n        pass\n    def deep():\n        own = 2\n    COUNT = 3\n\ndef bare(a):\n    return a\n";
    let (_d, vault) = common::built(&[("a.py", src)]);
    let page = common::map_page(&vault, "map/a_py");
    assert_eq!(
        locals_of(&page, "tally"),
        [
            "Locals: `count`, `item: Entry`, `rest`",
            "Outside names: `COUNT`"
        ]
    );
    assert_eq!(locals_of(&page, "bare"), Vec::<String>::new());
}

#[test]
fn asserts_are_counted_as_not_drawn() {
    let rs = "fn f(a: u8) {\n    assert!(a > 0);\n    assert_eq!(a, 1);\n    debug_assert_ne!(a, 2);\n    std::assert!(a);\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", rs)]);
    let f = common::fn_block(&common::map_page(&vault, "map/src/a_rs"), "f");
    assert!(f.contains(" Not drawn: 4 assert.\n"), "{f}");
    let py = "def f(a):\n    assert a\n    assert a > 1, 'big'\n";
    let (_d, vault) = common::built(&[("a.py", py)]);
    let f = common::fn_block(&common::map_page(&vault, "map/a_py"), "f");
    assert!(f.contains(" Not drawn: 2 assert.\n"), "{f}");
}

#[test]
fn a_module_file_declared_under_cfg_test_is_test_code() {
    let helpers =
        "use super::*;\nfn fixture() {\n    keep();\n}\n#[test]\nfn t() {\n    fixture();\n}\n";
    let (_d, vault) = common::built(&[
        (
            "src/lib.rs",
            "mod store;\nmod shelf;\nmod plain;\n#[cfg(test)]\nmod tests;\n",
        ),
        ("src/tests.rs", helpers),
        (
            "src/store.rs",
            "pub fn keep() {}\n#[cfg(test)]\nmod tests;\n",
        ),
        ("src/store/tests.rs", helpers),
        (
            "src/shelf/mod.rs",
            "pub fn keep() {}\n#[cfg(test)]\nmod checks;\n",
        ),
        ("src/shelf/checks.rs", helpers),
        // Declared with no `cfg(test)`: charted as any module.
        ("src/plain.rs", "pub fn other() {}\nmod tests;\n"),
        (
            "src/plain/tests.rs",
            "fn fixture() {\n    super::other();\n}\n",
        ),
    ]);
    for page in [
        "map/src/tests_rs",
        "map/src/store/tests_rs",
        "map/src/shelf/checks_rs",
    ] {
        let text = common::map_page(&vault, page);
        assert!(common::headings(&text).is_empty(), "{page}:\n{text}");
        assert!(
            text.contains("\nTest code is not charted: 2 functions.\n"),
            "{page}:\n{text}"
        );
    }
    for page in ["map/src/store_rs", "map/src/shelf/mod_rs"] {
        let text = common::map_page(&vault, page);
        assert_eq!(
            common::fn_section(&text, "keep", "Called from"),
            ["none found"],
            "{page}"
        );
    }
    let plain = common::map_page(&vault, "map/src/plain/tests_rs");
    assert_eq!(common::headings(&plain), ["fixture"]);
}

/// Builds `lib` as `src/lib.rs` beside `src/<stem>.rs` holding a live
/// `pub fn ping() {}`, plus `extra`, and checks that `ping` keeps its
/// section and `go`'s call to it, `<stem>::ping()` on line `line`, stays a
/// certain link.
fn live_module_stays_charted(lib: &str, stem: &str, line: usize, extra: &[(&str, &str)]) {
    let path = format!("src/{stem}.rs");
    let mut files = vec![("src/lib.rs", lib), (path.as_str(), "pub fn ping() {}\n")];
    files.extend_from_slice(extra);
    let (_d, vault) = common::built(&files);
    let page = common::map_page(&vault, &format!("map/src/{stem}_rs"));
    assert_eq!(common::headings(&page), ["ping"], "{page}");
    let lib = common::map_page(&vault, "map/src/lib_rs");
    assert_eq!(
        common::fn_section(&lib, "go", "Calls"),
        [format!(
            "`{stem}::ping()` L{line} → [[map/src/{stem}_rs#ping|ping]]"
        )]
    );
}

#[test]
fn a_cfg_test_module_inside_an_inline_module_leaves_a_live_namesake_charted() {
    live_module_stays_charted(
        "mod probe;\npub mod client {\n    #[cfg(test)]\n    mod probe;\n}\npub fn go() {\n    probe::ping();\n}\n",
        "probe",
        7,
        &[],
    );
}

#[test]
fn a_module_declared_inside_an_inline_test_module_leaves_a_live_namesake_charted() {
    live_module_stays_charted(
        "mod m;\npub fn go() {\n    m::ping();\n}\n#[cfg(test)]\nmod tests {\n    mod m;\n}\n",
        "m",
        3,
        &[],
    );
}

#[test]
fn a_cfg_test_module_with_a_path_leaves_the_live_module_charted() {
    live_module_stays_charted(
        "#[cfg(not(test))]\nmod clock;\n#[cfg(test)]\n#[path = \"mock_clock.rs\"]\nmod clock;\npub fn go() {\n    clock::ping();\n}\n",
        "clock",
        7,
        &[("src/mock_clock.rs", "pub fn fake() {}\n")],
    );
}

#[test]
fn a_test_module_of_a_crate_root_is_beside_it_and_its_children_are_test_code() {
    let helper = "fn fixture() {}\n";
    let (_d, vault) = common::built(&[
        ("Cargo.toml", "[package]\nname = \"tool\"\n"),
        ("src/lib.rs", "pub fn keep() {}\n"),
        (
            "src/bin/tool.rs",
            "fn main() {}\n#[cfg(test)]\nmod checks;\n",
        ),
        ("src/bin/checks.rs", "mod deep;\nfn fixture() {}\n"),
        ("src/bin/checks/deep.rs", helper),
        ("build.rs", "fn main() {}\n#[cfg(test)]\nmod probe;\n"),
        ("probe.rs", helper),
    ]);
    for page in [
        "map/src/bin/checks_rs",
        "map/src/bin/checks/deep_rs",
        "map/probe_rs",
    ] {
        let text = common::map_page(&vault, page);
        assert!(common::headings(&text).is_empty(), "{page}:\n{text}");
    }
}

const INDEX_MAIN: &str = "mod aux;\n#[cfg(test)]\nmod tests;\n\nfn main() {\n    prepare();\n    shared();\n}\nfn prepare() {\n    shared();\n    again();\n}\nfn shared() {\n    leaf();\n}\nfn leaf() {}\nfn again() {\n    again();\n}\nfn spare() {}\n";
const INDEX_AUX: &str = "pub fn begin() {\n    ping();\n}\nfn ping() {\n    pong();\n}\nfn pong() {\n    ping();\n}\nfn lone() {}\nfn ring_a() {\n    ring_b();\n}\nfn ring_b() {\n    ring_a();\n}\npub struct Store;\nimpl Store {\n    pub fn get(&self) {}\n}\npub struct Cache;\nimpl Cache {\n    pub fn get(&self) {}\n}\npub fn fetch(t: &Holder) {\n    t.get();\n}\n";
const INDEX_TESTS: &str =
    "use super::*;\nfn fixture() {\n    spare();\n}\n#[test]\nfn t() {\n    fixture();\n}\n";

fn index_tree() -> [(&'static str, &'static str); 3] {
    [
        ("src/main.rs", INDEX_MAIN),
        ("src/aux.rs", INDEX_AUX),
        ("src/tests.rs", INDEX_TESTS),
    ]
}

const INTRO: &str = "Every charted function, from the top down, following only the calls the map reads as certain (a single link in a page's **Calls**); a call that may be one of several functions is not followed. Each function is expanded once, at the shallowest depth the first section reaches it, or else where a later section first reaches it; its last line, \"also calls\", lists the functions it calls that are not expanded under it. A function listed as not called may still be called through a value, a trait or virtual call, a macro, an attribute, or from outside the project.";

#[test]
fn the_index_lists_every_charted_function_from_the_top_down() {
    let (_d, vault) = common::built(&index_tree());
    assert_eq!(
        common::map_page(&vault, "map/index"),
        format!("---\ntitle: \"Index\"\ntags: [\"plaz\"]\nkind: map\n---\n{INTRO}\n\n## Directories\n- [[map/index/src|src]] · 15 functions listed\n")
    );
    let m = |a: &str, t: &str| format!("[[map/src/main_rs#{a}|{t}]]");
    let x = |a: &str, t: &str| format!("[[map/src/aux_rs#{a}|{t}]]");
    let want = [
        "---".to_string(),
        "title: \"Index · src\"".into(),
        "tags: [\"plaz\"]".into(),
        "kind: map".into(),
        "---".into(),
        "The trees that start in a file under `src/`, from the [[map/index|index]]. A tree follows certain calls into other directories; a function of `src/` first reached from another directory is listed on that directory's page. ".to_string() + INTRO,
        String::new(),
        "## Not called by any charted function".into(),
        String::new(),
        "### [[map/src/main_rs|src/main.rs]]".into(),
        format!("- {}", m("main", "main")),
        format!("  - {}", m("prepare", "prepare")),
        format!("    - {}", m("again", "again")),
        format!("      - also calls: {}", m("again", "again")),
        format!("    - also calls: {}", m("shared", "shared")),
        format!("  - {}", m("shared", "shared")),
        format!("    - {}", m("leaf", "leaf")),
        String::new(),
        "### [[map/src/aux_rs|src/aux.rs]]".into(),
        format!("- {}", x("begin", "begin")),
        format!("  - {}", x("ping", "ping")),
        format!("    - {}", x("pong", "pong")),
        format!("      - also calls: {}", x("ping", "ping")),
        String::new(),
        "## Called only by calls that may be another function".into(),
        String::new(),
        "### [[map/src/aux_rs|src/aux.rs]]".into(),
        format!("- {}", x("store_get", "Store::get")),
        format!("- {}", x("cache_get", "Cache::get")),
        String::new(),
        "## In a call cycle no function above reaches".into(),
        String::new(),
        "### [[map/src/aux_rs|src/aux.rs]]".into(),
        format!("- {}", x("ring_a", "ring_a")),
        format!("  - {}", x("ring_b", "ring_b")),
        format!("    - also calls: {}", x("ring_a", "ring_a")),
        String::new(),
        "## Not called, calling nothing certain".into(),
        String::new(),
        "### [[map/src/main_rs|src/main.rs]]".into(),
        format!("- {}", m("spare", "spare")),
        String::new(),
        "### [[map/src/aux_rs|src/aux.rs]]".into(),
        format!("- {}", x("lone", "lone")),
        format!("- {}", x("fetch", "fetch")),
    ];
    assert_eq!(
        common::map_page(&vault, "map/index/src"),
        want.join("\n") + "\n"
    );
}

#[test]
fn a_cycle_tree_starts_on_the_cycle_not_at_a_leaf_it_calls() {
    let (_d, vault) = common::built(&[(
        "src/lib.rs",
        "fn leaf() {}\nfn ring_a() {\n    ring_b();\n    leaf();\n}\nfn ring_b() {\n    ring_a();\n}\n",
    )]);
    let x = |a: &str| format!("[[map/src/lib_rs#{a}|{a}]]");
    assert_eq!(
        common::index_section(
            &vault,
            "map/index/src",
            "In a call cycle no function above reaches"
        ),
        [
            "### [[map/src/lib_rs|src/lib.rs]]".to_string(),
            format!("- {}", x("ring_a")),
            format!("  - {}", x("ring_b")),
            format!("    - also calls: {}", x("ring_a")),
            format!("  - {}", x("leaf")),
        ]
    );
}

#[test]
fn a_function_section_names_its_level_in_the_index() {
    let (_d, vault) = common::built(&index_tree());
    let level = |page: &str, anchor: &str| {
        let block = common::fn_block(&common::map_page(&vault, page), anchor);
        block
            .lines()
            .find(|l| l.starts_with("Level "))
            .map(str::to_string)
    };
    for (page, anchor, n) in [
        ("map/src/main_rs", "main", Some(0)),
        ("map/src/main_rs", "prepare", Some(1)),
        ("map/src/main_rs", "shared", Some(1)),
        ("map/src/main_rs", "again", Some(2)),
        ("map/src/main_rs", "leaf", Some(2)),
        ("map/src/aux_rs", "pong", Some(2)),
        ("map/src/main_rs", "spare", None),
        ("map/src/aux_rs", "ring_b", None),
        ("map/src/aux_rs", "store_get", None),
    ] {
        let want = n.map(|n| format!("Level {n} in the [[map/index/src|index]]"));
        assert_eq!(level(page, anchor), want, "{anchor}");
    }
    let block = common::fn_block(&common::map_page(&vault, "map/src/main_rs"), "prepare");
    assert!(
        block.starts_with("## prepare\n`fn prepare()` · `src/main.rs:9-12`\nLevel 1 in the [[map/index/src|index]]\n\n**Called from**\n"),
        "{block}"
    );
}

#[test]
fn the_index_puts_main_first_and_follows_calls_across_files() {
    let (_d, vault) = common::built(&[
        ("app/a.py", "def start():\n    helper()\n\ndef helper():\n    pass\n"),
        ("app/tool.py", "from app.a import helper\n\ndef main():\n    helper()\n    finish()\n\ndef finish():\n    pass\n"),
        ("tests/test_a.py", "from app.a import start\n\ndef test_start():\n    start()\n"),
        ("cli.py", "def run():\n    pass\n"),
    ]);
    assert_eq!(
        common::index_section(
            &vault,
            "map/index/app",
            "Not called by any charted function"
        ),
        [
            "### [[map/app/tool_py|app/tool.py]]",
            "- [[map/app/tool_py#main|main]]",
            "  - [[map/app/a_py#helper|helper]]",
            "  - [[map/app/tool_py#finish|finish]]",
            "### [[map/app/a_py|app/a.py]]",
            "- [[map/app/a_py#start|start]]",
            "  - also calls: [[map/app/a_py#helper|helper]]",
        ]
    );
    // The top of the tree is on the index itself, after the directories.
    let index = common::map_page(&vault, "map/index");
    assert!(
        index.ends_with("\n## Directories\n- [[map/index/app|app]] · 4 functions listed\n\n## Not called, calling nothing certain\n\n### [[map/cli_py|cli.py]]\n- [[map/cli_py#run|run]]\n"),
        "{index}"
    );
}

#[test]
fn the_index_takes_its_page_ids_before_any_source_file() {
    let script = "#!/bin/sh\nhelper() { :; }\n";
    let (_d, vault) = common::built(&[("index", script), ("src/a.rs", "fn f() {}\n")]);
    let index = common::map_page(&vault, "map/index");
    assert!(
        index.contains("\nkind: map\n---\nEvery charted function"),
        "{index}"
    );
    assert!(
        index.contains("\n- [[map/index/src|src]] · 1 function listed\n"),
        "{index}"
    );
    assert!(common::map_page(&vault, "map/index_2").contains("\nsource: \"index\"\n"));

    let (_d, vault) = common::built(&[("index/src", script), ("src/a.rs", "fn f() {}\n")]);
    let src = common::map_page(&vault, "map/index/src");
    assert!(src.starts_with("---\ntitle: \"Index · src\"\n"), "{src}");
    assert!(common::map_page(&vault, "map/index/src_2").contains("\nsource: \"index/src\"\n"));
}

#[test]
fn the_index_is_rewritten_only_when_it_changes_and_goes_with_the_functions() {
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    common::write_files(&src, &index_tree());
    let first = plaz::build(&src, &vault).unwrap();
    let before = common::map_page(&vault, "map/index");
    let again = plaz::build(&src, &vault).unwrap();
    assert_eq!((again.written, again.unchanged), (0, first.written));
    assert_eq!(common::map_page(&vault, "map/index"), before);
    let src_index = common::map_page(&vault, "map/index/src");
    plaz::build(&src, &vault).unwrap();
    assert_eq!(common::map_page(&vault, "map/index/src"), src_index);

    // A tree left with no function: no index.
    std::fs::remove_dir_all(&src).unwrap();
    common::write_files(&src, &[("src/a.rs", "pub struct Plain;\n")]);
    plaz::build(&src, &vault).unwrap();
    assert!(!vault.join("map/index.md").exists());
    assert!(!vault.join("map/index").exists());
}

#[test]
fn a_file_of_the_users_at_the_index_path_is_left_alone() {
    let d = tempfile::tempdir().unwrap();
    let (src, vault) = (d.path().join("src-root"), d.path().join("vault"));
    common::write_files(&src, &index_tree());
    common::write_files(&vault, &[("map/index.md", "my own notes\n")]);
    let n = plaz::build(&src, &vault).unwrap();
    assert_eq!(n.left_alone, 1);
    assert_eq!(common::map_page(&vault, "map/index"), "my own notes\n");
}

#[test]
fn a_directory_page_has_each_section_once_when_main_files_sort_between() {
    let (_d, vault) = common::built(&[
        ("src/main.rs", "fn main() {\n    go();\n}\nfn go() {}\n"),
        (
            "src/aux.rs",
            "pub fn begin() {\n    step();\n}\nfn step() {}\n",
        ),
        ("tools/main.rs", "fn main() {\n    run();\n}\nfn run() {}\n"),
    ]);
    let page = common::map_page(&vault, "map/index/src");
    assert_eq!(
        common::headings(&page),
        ["Not called by any charted function"]
    );
    assert_eq!(
        common::index_section(
            &vault,
            "map/index/src",
            "Not called by any charted function"
        ),
        [
            "### [[map/src/main_rs|src/main.rs]]",
            "- [[map/src/main_rs#main|main]]",
            "  - [[map/src/main_rs#go|go]]",
            "### [[map/src/aux_rs|src/aux.rs]]",
            "- [[map/src/aux_rs#begin|begin]]",
            "  - [[map/src/aux_rs#step|step]]",
        ]
    );
}

/// The line of the Calls list of `f` that starts with `call`.
fn call_line(page: &str, f: &str, call: &str) -> String {
    let calls = common::fn_section(page, f, "Calls");
    let line = calls.iter().find(|l| l.starts_with(&format!("`{call}`")));
    line.unwrap_or_else(|| panic!("{call}: {calls:#?}")).clone()
}

#[test]
fn a_let_mut_closure_is_not_linked() {
    let src = "fn spare(x: u8) -> u8 { x }\npub fn choose(n: u8) -> u8 {\n    let mut pick: fn(u8) -> u8 = |x| { if x > 1 { x } else { 0 } };\n    if n > 3 { pick = spare; }\n    pick(n)\n}\npub fn keep(n: u8) -> u8 {\n    let pick = |x: u8| { if x > 1 { x } else { 0 } };\n    pick(n)\n}\n";
    let (_d, vault) = common::built(&[("src/a.rs", src)]);
    let page = common::map_page(&vault, "map/src/a_rs");
    assert_eq!(
        call_line(&page, "choose", "pick(n)"),
        "`pick(n)` L5 → target not visible"
    );
    assert_eq!(
        call_line(&page, "keep", "pick(n)"),
        "`pick(n)` L9 → the closure `pick`, drawn in this chart"
    );
}

#[test]
fn a_reassigned_python_nested_def_is_not_linked() {
    let py = "def spare(x):\n    return x\n\ndef choose(n):\n    def helper(x):\n        if x > 1:\n            return x\n        return 0\n    if n > 3:\n        helper = spare\n    return helper(n)\n";
    let (_d, vault) = common::built(&[("app/m.py", py)]);
    let page = common::map_page(&vault, "map/app/m_py");
    assert_eq!(
        call_line(&page, "choose", "helper(n)"),
        "`helper(n)` L11 → target not visible"
    );
}
