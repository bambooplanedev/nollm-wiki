//! Shell scripts through `compile()`: functions, docs, summary, parse-error fallback, page ids.

mod common;
use common::{compiled, compiled_wiki, entry, index_entries};
use std::collections::BTreeMap;

/// `(signature, kind, start, end, doc)` of a page's items.
fn items(
    wiki: &wiki::query::Wiki,
    id: &str,
) -> Vec<(String, String, usize, usize, Option<String>)> {
    wiki.items(id)
        .iter()
        .map(|i| {
            (
                i.signature.clone(),
                serde_json::to_value(i.kind)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string(),
                i.start_line,
                i.end_line,
                i.doc.clone(),
            )
        })
        .collect()
}

#[test]
fn shell_functions_have_spans_and_signatures() {
    let src = "foo() {\n  echo foo\n}\n\nfunction bar {\n  echo bar\n}\n\nfunction baz() (\n  echo baz\n)\n";
    let (_d, wiki) = compiled_wiki(&[("tools.sh", src)]);
    let got: Vec<_> = items(&wiki, "tools")
        .into_iter()
        .map(|(s, k, a, b, _)| (s, k, a, b))
        .collect();
    let fn_ = |s: &str, a, b| (s.to_string(), "fn".to_string(), a, b);
    assert_eq!(
        got,
        vec![fn_("foo()", 1, 3), fn_("bar()", 5, 7), fn_("baz()", 9, 11)]
    );
}

#[test]
fn bash_extension_is_a_code_page_too() {
    let (_d, wiki) = compiled_wiki(&[("lib.bash", "greet() {\n  echo hi\n}\n")]);
    assert_eq!(items(&wiki, "lib")[0].0, "greet()");
}

#[test]
fn comment_above_function_is_its_doc() {
    let src = "#!/bin/sh\n\nunrelated() { :; }\n\n# Old note, cut off by a blank line.\n\n# Deploy the build.\n# Second line.\nfoo() {\n  :\n}\n";
    let (_d, wiki) = compiled_wiki(&[("tools.sh", src)]);
    let it = items(&wiki, "tools");
    let foo = it.iter().find(|i| i.0 == "foo()").unwrap();
    assert_eq!(foo.4.as_deref(), Some("Deploy the build."));
    assert_eq!((foo.2, foo.3), (7, 11), "range starts at the doc run");
    assert_eq!(it.iter().find(|i| i.0 == "unrelated()").unwrap().4, None);
}

#[test]
fn leading_comment_is_page_summary() {
    let src = "#!/usr/bin/env bash\n# Builds and ships the release.\n\nfoo() { :; }\n";
    let (_d, _, out_dir) = compiled(&[("ship.sh", src)]);
    assert_eq!(
        entry(&out_dir, "ship")["summary"],
        "Builds and ships the release."
    );
}

#[test]
fn broken_file_keeps_its_functions() {
    let src = "#!/bin/bash\nif [ -z \"$X\" ; then\n  echo (\nfi\n\nnvm() {\n  echo nvm\n}\n\nfunction other {\n  echo o\n";
    let (_d, wiki) = compiled_wiki(&[("nvm.sh", src)]);
    let got: Vec<_> = items(&wiki, "nvm")
        .into_iter()
        .map(|(s, _, a, b, _)| (s, a, b))
        .collect();
    assert!(got.contains(&("nvm()".to_string(), 6, 8)), "{got:?}");
    assert!(got.iter().any(|g| g.0 == "other()"), "{got:?}");
}

/// Every non-shell page: id -> path.
fn non_shell(out: &std::path::Path) -> BTreeMap<String, String> {
    index_entries(out)
        .into_iter()
        .map(|e| {
            (
                e["id"].as_str().unwrap().to_string(),
                e["path"].as_str().unwrap().to_string(),
            )
        })
        .filter(|(_, p)| {
            let ext = std::path::Path::new(p).extension();
            ext != Some("sh".as_ref()) && ext != Some("bash".as_ref())
        })
        .collect()
}

#[test]
fn shell_page_does_not_rename_other_pages() {
    let base = [
        ("run.py", "def go():\n    pass\n"),
        ("tool.ts", "export function t() {}\n"),
    ];
    let (_a, _, out_a) = compiled(&base);
    let mut with = base.to_vec();
    with.push(("run.sh", "go() { :; }\n"));
    with.push(("tool.sh", "t() { :; }\n"));
    let (_b, _, out_b) = compiled(&with);
    let ids_b = non_shell(&out_b);
    assert_eq!(non_shell(&out_a), ids_b);
    assert_eq!(index_entries(&out_b).len(), 4, "both shell pages exist");
}

#[test]
fn shell_pages_stay_out_of_import_resolution() {
    let base = [
        (
            "app.py",
            "import deploy\nfrom util import helper\n\ndef main():\n    deploy.run()\n",
        ),
        ("util.py", "def helper():\n    pass\n"),
    ];
    let (_a, _, out_a) = compiled(&base);
    let mut with = base.to_vec();
    // `deploy.sh` has the stem `deploy`; `helper` is also a shell function name.
    with.push(("deploy.sh", "helper() { :; }\nrun() { :; }\n"));
    let (_b, _, out_b) = compiled(&with);
    // The rendered pages carry the import and link edges.
    for page in ["app.md", "util.md"] {
        let read = |out: &std::path::Path| std::fs::read_to_string(out.join(page)).unwrap();
        assert_eq!(read(&out_a), read(&out_b), "{page} changed");
    }
    assert_eq!(non_shell(&out_a), non_shell(&out_b));
}

#[test]
fn shell_page_in_another_directory_does_not_rename_the_existing_page() {
    let base = [("a/run.py", "def go():\n    pass\n")];
    let (_a, _, out_a) = compiled(&base);
    let (_b, _, out_b) = compiled(&[base[0], ("b/run.sh", "go() { :; }\n")]);
    assert_eq!(non_shell(&out_a), non_shell(&out_b));
    assert_eq!(index_entries(&out_b).len(), 2);
}

#[test]
fn summary_is_the_header_comment_only() {
    let src = "#!/bin/sh\n# Ships the release.\n\nfoo() {\n  # inner note\n  :\n}\n";
    let (_d, _, out) = compiled(&[("ship.sh", src)]);
    assert_eq!(entry(&out, "ship")["summary"], "Ships the release.");
}

#[test]
fn a_comment_inside_a_function_body_is_no_summary() {
    let src = "foo() {\n  # inner note about things\n  :\n}\n";
    let (_d, _, out) = compiled(&[("ship.sh", src)]);
    assert_ne!(entry(&out, "ship")["summary"], "inner note about things");
}

fn callers_text(wiki: &wiki::query::Wiki, id: &str, symbol: &str, depth: Option<usize>) -> String {
    wiki.callers(id, symbol, depth, None).unwrap()
}

#[test]
fn shell_call_resolves_within_project() {
    let (_d, wiki) = compiled_wiki(&[
        ("a.sh", "helper() {\n  :\n}\n"),
        ("b.sh", "run_it() {\n  helper x\n}\n"),
    ]);
    let text = callers_text(&wiki, "a", "helper", None);
    assert!(
        text.contains("b.sh\n  1-3  fn  run_it  calls helper at 2"),
        "{text}"
    );
}

#[test]
fn top_level_call_has_top_level_caller() {
    let src = "helper start\n\nrun_it() {\n  helper x\n}\n\nhelper end\n";
    let (_d, wiki) = compiled_wiki(&[("a.sh", "helper() { :; }\n"), ("b.sh", src)]);
    let text = callers_text(&wiki, "a", "helper", None);
    assert!(
        text.contains("  1-7  top_level  <top level>  calls helper at 1, 7"),
        "{text}"
    );
    assert!(text.contains("run_it"), "positive control: {text}");
    // Nothing calls the top level: with depth 2 it stays a leaf.
    assert_eq!(text, callers_text(&wiki, "a", "helper", Some(2)));
    assert!(
        text.starts_with("callers of helper  a.sh:1-1 — 2 functions"),
        "{text}"
    );
}

#[test]
fn top_level_chain_stops_at_top_level() {
    // `inner` is called only by `outer`; `outer` only at top level.
    let src = "inner() { :; }\nouter() {\n  inner\n}\nouter\n";
    let (_d, wiki) = compiled_wiki(&[("s.sh", src)]);
    let text = callers_text(&wiki, "s", "inner", Some(3));
    assert!(text.contains("outer"), "{text}");
    assert!(text.contains("<top level>"), "{text}");
    assert!(text.contains("2 functions"), "{text}");
}

#[test]
fn top_level_item_is_not_a_symbol() {
    let src = "helper() { :; }\ntop_call_marker() { :; }\nhelper\ntop_call_marker\n";
    let (_d, out_wiki) = {
        let (d, _, out) = compiled(&[("s.sh", src)]);
        let w = wiki::query::Wiki::load(&out).unwrap();
        let idx = index_entries(&out);
        // `defined` and the page's exports never name it.
        let e = idx.iter().find(|e| e["id"] == "s").unwrap();
        let defined = e["defined"].to_string();
        assert!(defined.contains("helper"), "positive control: {defined}");
        assert!(!defined.contains("top level"), "{defined}");
        let md = std::fs::read_to_string(out.join("s.md")).unwrap();
        assert!(md.contains("helper()") && !md.contains("top level"), "{md}");
        (d, w)
    };
    let wiki = out_wiki;
    // The item exists for `callers` ...
    assert!(wiki.items("s").iter().any(|i| i.name == "<top level>"));
    // ... but search lists the real function and not it.
    for q in ["helper", "top", "top level", "<top level>"] {
        for h in wiki.search(q, None, 10) {
            assert!(
                h.symbols.iter().all(|s| !s.name.contains("top level")),
                "{q}"
            );
        }
    }
    let hit = wiki.search("helper", None, 10);
    assert!(hit
        .iter()
        .any(|h| h.symbols.iter().any(|s| s.name == "helper")));
    // The outline lists the function and not the top level.
    let outline = wiki.outline("s", None).unwrap();
    assert!(outline.contains("helper()"), "{outline}");
    assert!(!outline.contains("top level"), "{outline}");
    // Cut by a budget the outline still hides it.
    let tight = wiki.outline("s", Some(30)).unwrap();
    assert!(!tight.contains("top level"), "{tight}");
}

#[test]
fn script_with_only_top_level_calls_has_no_outline_items() {
    let (_d, wiki) = compiled_wiki(&[("a.sh", "helper() { :; }\n"), ("run.sh", "helper\n")]);
    assert!(!wiki.outline("run", None).unwrap().contains("top level"));
    assert!(wiki
        .search("top level", None, 10)
        .iter()
        .all(|h| h.symbols.is_empty()));
    assert!(callers_text(&wiki, "a", "helper", None).contains("<top level>"));
}

#[test]
fn top_level_only_script_is_not_told_to_recompile() {
    let (_d, wiki) = compiled_wiki(&[("run.sh", "set -e\nsource ./a.sh\n")]);
    let text = wiki.outline("run", None).unwrap();
    assert!(
        text.contains("no functions") && !text.contains("recompile"),
        "{text}"
    );
}

#[test]
fn neighbors_symbol_never_lists_top_level() {
    let src = "leaf() { :; }\nmid() {\n  leaf\n}\nmid\n";
    let (_d, wiki) = compiled_wiki(&[("s.sh", src)]);
    let text = wiki.calls("s", "mid", None, None).unwrap();
    assert!(text.contains("leaf"), "positive control: {text}");
    assert!(!text.contains("top level"), "{text}");
}

#[test]
fn bash_log_does_not_resolve_to_ts() {
    let (_d, wiki) = compiled_wiki(&[
        ("x.sh", "go() {\n  log hi\n}\nlog hi\n"),
        ("y.ts", "export function log(m: string) {}\n"),
    ]);
    let text = callers_text(&wiki, "y", "log", None);
    assert!(text.contains("0 functions"), "{text}");
    // Positive control: a shell `log` is found by shell.
    let (_d, wiki) = compiled_wiki(&[("x.sh", "log() { :; }\ngo() {\n  log hi\n}\n")]);
    assert!(callers_text(&wiki, "x", "log", None).contains("go"));
}

#[test]
fn call_in_parse_error_has_no_caller() {
    // The unclosed `foo=(` makes the rest of the file an ERROR node.
    let src = "helper() { :; }\nok() {\n  helper fine\n}\nfoo=(\n  helper broken\n";
    let (_d, wiki) = compiled_wiki(&[("s.sh", src)]);
    let text = callers_text(&wiki, "s", "helper", None);
    assert!(
        text.contains("ok  calls helper at 3"),
        "positive control: {text}"
    );
    assert!(
        !text.contains("top level") && !text.contains("at 6"),
        "{text}"
    );
}

#[test]
fn call_in_parsed_function_inside_error_keeps_its_caller() {
    // The unclosed `x() {` makes the rest of the file an ERROR node holding
    // the well formed `f`, so its call keeps `f` as caller.
    let src = "g() { :; }\nx() {\nf() {\n  g\n}\ng broken\n";
    let (_d, wiki) = compiled_wiki(&[("s.sh", src)]);
    let text = callers_text(&wiki, "s", "g", None);
    assert!(text.contains("f  calls g at 4"), "{text}");
    assert!(
        !text.contains("top level") && !text.contains("at 6"),
        "{text}"
    );
}

#[test]
fn source_and_dot_arguments_are_imports() {
    let src = "\\. d.sh\nsource lib/a.sh\n. \"${R}\"/b.sh\nbuiltin source c.sh\necho source e.sh\nsource\n";
    let (_d, _, out) = compiled(&[("m.sh", src)]);
    let page = std::fs::read_to_string(out.join("m.md")).unwrap();
    let got = common::section(&page, "Imports");
    assert_eq!(got, ["${R}\"/b.sh", "c.sh", "d.sh", "lib/a.sh"], "{page}");
}

/// `trigger` is a line tree-sitter-bash 0.25 rejects though bash accepts it.
/// After it, `f`'s call keeps `f` as caller, and the doc and the span of the
/// indented-brace `f` come from the tree, not the line-regex fallback.
fn grammar_gap_is_rewritten(trigger: &str) {
    let src = format!("g() {{ :; }}\n{trigger}\n# Calls g.\nf() {{\n  g\n  }}\nh() {{ :; }}\n");
    let (_d, wiki) = compiled_wiki(&[("s.sh", &src)]);
    let text = callers_text(&wiki, "s", "g", None);
    let k = trigger.lines().count() - 1;
    assert!(
        text.contains(&format!("{}-{}  fn  f  calls g at {}", 3 + k, 6 + k, 5 + k)),
        "{trigger}: {text}"
    );
    let f = items(&wiki, "s")
        .into_iter()
        .find(|i| i.0 == "f()")
        .unwrap();
    assert_eq!(
        (f.2, f.3, f.4.as_deref()),
        (3 + k, 6 + k, Some("Calls g.")),
        "{trigger}"
    );
}

#[test]
fn case_pattern_with_glob_and_quote_is_rewritten() {
    grammar_gap_is_rewritten("case $P in a*' '*) x ;; esac");
}

#[test]
fn multi_line_case_pattern_is_rewritten() {
    // The nvm.sh form: the glob-and-quote pattern sits on its own line.
    grammar_gap_is_rewritten("case $P in\n  --only=*' '*)\n    x ;;\nesac");
}

#[test]
fn bracket_in_expansion_pattern_is_rewritten() {
    grammar_gap_is_rewritten("r=${r%%]*}");
}

#[test]
fn semicolon_in_expansion_word_is_rewritten() {
    grammar_gap_is_rewritten("echo \"${out:+$out;}1\"");
}

#[test]
fn clean_file_is_not_rewritten() {
    // The grammar takes this file; a rewrite would blank the `;` and merge
    // the two calls into one word.
    let src = "g() { :; }\nh() { :; }\nf() {\n  o=${o:-\"`g;h`\"}\n}\n";
    let (_d, wiki) = compiled_wiki(&[("s.sh", src)]);
    let text = callers_text(&wiki, "s", "h", None);
    assert!(text.contains("f  calls h at 4"), "{text}");
}

#[test]
fn backslash_dot_import_links_the_sourced_page() {
    let (_d, _, out) = compiled(&[("main.sh", "\\. ./lib.sh\n"), ("lib.sh", "x() { :; }\n")]);
    let lib = std::fs::read_to_string(out.join("lib.md")).unwrap();
    assert!(
        lib.contains("## Imported by") && lib.contains("[[main"),
        "{lib}"
    );
}

#[test]
fn shellcheck_directive_is_no_summary() {
    let src = "#!/bin/bash\n# shellcheck disable=SC2034\n# Deploys things.\n\nfoo() { :; }\n";
    let (_d, _, out) = compiled(&[("d.sh", src)]);
    assert_eq!(entry(&out, "d")["summary"], "Deploys things.");
}

/// `(signature, start, end)` of a page's top-level variable items.
fn globals(wiki: &wiki::query::Wiki, id: &str) -> Vec<(String, usize, usize)> {
    wiki.items(id)
        .iter()
        .filter(|i| i.nested && i.kind == wiki::model::CodeItemKind::Value)
        .map(|i| (i.signature.clone(), i.start_line, i.end_line))
        .collect()
}

fn g(sig: &str, a: usize, b: usize) -> (String, usize, usize) {
    (sig.to_string(), a, b)
}

#[test]
fn top_level_uppercase_assignment_is_a_search_only_value() {
    let (_d, wiki) = compiled_wiki(&[("env.sh", "X=1\nf() { :; }\n")]);
    let it = wiki.items("env").iter().find(|i| i.name == "X").unwrap();
    assert_eq!(
        (it.kind, it.public, it.nested),
        (wiki::model::CodeItemKind::Value, false, true)
    );
    assert_eq!(globals(&wiki, "env"), [g("X=1", 1, 1)]);
    let page = wiki.page_text("env", false, None).unwrap();
    assert!(!page.contains("X=1"), "{page}");
    let full = wiki.page_text("env", true, None).unwrap();
    assert!(
        !common::exports(&full).iter().any(|s| s.contains('X')),
        "{full}"
    );
}

#[test]
fn export_readonly_and_declare_assignments_are_values() {
    let src = "export A=1\nreadonly B=2\ndeclare -r C=3\nexport D\n";
    let (_d, wiki) = compiled_wiki(&[("env.sh", src)]);
    assert_eq!(
        globals(&wiki, "env"),
        [
            g("export A=1", 1, 1),
            g("readonly B=2", 2, 2),
            g("declare -r C=3", 3, 3)
        ]
    );
}

#[test]
fn heredoc_command_substitution_value_spans_to_its_close() {
    let src = "QX_PY=\"$(cat <<'PY'\nimport sys\nprint(sys.argv)\nPY\n)\"\nNEXT=1\n";
    let (_d, wiki) = compiled_wiki(&[("env.sh", src)]);
    assert_eq!(
        globals(&wiki, "env"),
        [g("QX_PY=\"$(cat <<'PY'", 1, 5), g("NEXT=1", 6, 6)]
    );
}

#[test]
fn multi_line_quoted_value_spans_to_its_closing_quote() {
    let src = "USAGE=\"usage:\n  run it\n\"\nNEXT=1\n";
    let (_d, wiki) = compiled_wiki(&[("env.sh", src)]);
    assert_eq!(
        globals(&wiki, "env"),
        [g("USAGE=\"usage:", 1, 3), g("NEXT=1", 4, 4)]
    );
}

#[test]
fn only_plain_top_level_uppercase_assignments_are_values() {
    let src = "f() {\n  local A=1\n  B=2\n}\nif true; then\n  C=3\nfi\nfor i in 1; do\n  D=4\ndone\nE=5 cmd\nout=6\n";
    let (_d, wiki) = compiled_wiki(&[("env.sh", src)]);
    assert_eq!(globals(&wiki, "env"), []);
}

#[test]
fn only_the_first_assignment_of_a_name_is_an_item() {
    let (_d, wiki) = compiled_wiki(&[("env.sh", "X=1\nX=2\n")]);
    assert_eq!(globals(&wiki, "env"), [g("X=1", 1, 1)]);
}

#[test]
fn broken_file_keeps_its_assignments() {
    let src = "A=1\ncase $P in a*' '*) x ;; esac\nB=2\n";
    let (_d, wiki) = compiled_wiki(&[("env.sh", src)]);
    assert_eq!(globals(&wiki, "env"), [g("A=1", 1, 1), g("B=2", 3, 3)]);
}

#[test]
fn assignment_in_a_top_level_and_or_list_is_a_value() {
    let (_d, wiki) = compiled_wiki(&[("env.sh", "T=\"$(date)\" || T=\"\"\n")]);
    assert_eq!(globals(&wiki, "env"), [g("T=\"$(date)\" || T=\"\"", 1, 1)]);
}

#[test]
fn a_page_of_values_only_reads_as_before() {
    for body in ["X=1\n", "X=1\necho hi\n"] {
        let (_a, with) = compiled_wiki(&[("env.sh", body)]);
        let (_b, without) = compiled_wiki(&[("env.sh", &body.replace('X', "x"))]);
        assert_eq!(globals(&with, "env").len(), 1);
        assert_eq!(
            with.page_text("env", false, None),
            without.page_text("env", false, None),
            "{body}"
        );
    }
}

#[test]
fn function_body_in_a_broken_file_is_no_value() {
    // The backquoted `$//` makes the parser lose `function f`: its body lands
    // at the top level.
    let src =
        "A=1\nfunction f() {\nT=$1\n    N=`echo $T | sed s/\\.x$//`\n    print \"$N\"\n}\nC=2\n";
    let (_d, wiki) = compiled_wiki(&[("env.sh", src)]);
    assert_eq!(globals(&wiki, "env"), [g("A=1", 1, 1), g("C=2", 7, 7)]);
}

#[test]
fn assignment_with_a_parse_error_spans_its_first_line_only() {
    let (_d, wiki) = compiled_wiki(&[("env.sh", "X=\"a\nY=1\n")]);
    assert_eq!(globals(&wiki, "env"), [g("X=\"a", 1, 1)]);
}

#[test]
fn assignment_inside_an_error_node_is_a_value() {
    // zsh `${${(@k)T}:|M}` turns the whole file into one ERROR node.
    let src = "cd \"$Z\"\nlocal -a M\nM=(a b)\nI=(${${(@k)T}:|M})\nf() {\n  :\n}\n";
    let (_d, wiki) = compiled_wiki(&[("env.sh", src)]);
    assert_eq!(globals(&wiki, "env"), [g("M=(a b)", 3, 3)]);
}

#[test]
fn trap_string_command_is_a_call_from_its_function() {
    let src = "cleanup() { :; }\nrun() {\n  trap 'cleanup now' EXIT\n}\ntrap \"cleanup $X\" INT\n";
    let (_d, wiki) = compiled_wiki(&[("s.sh", src)]);
    let text = callers_text(&wiki, "s", "cleanup", None);
    assert!(text.contains("run  calls cleanup at 3"), "{text}");
    assert!(text.contains("<top level>  calls cleanup at 5"), "{text}");
}

#[test]
fn every_command_of_a_dash_c_string_is_a_call() {
    let src = "a() { :; }\nb() { :; }\nc() { :; }\nd() { :; }\n\
               run() {\n  bash -c 'a; b && c x | d'\n  sh -ec \"a\"\n}\n";
    let (_d, wiki) = compiled_wiki(&[("s.sh", src)]);
    for f in ["a", "b", "c", "d"] {
        let text = callers_text(&wiki, "s", f, None);
        assert!(text.contains(&format!("run  calls {f} at 6")), "{text}");
    }
    assert!(callers_text(&wiki, "s", "a", None).contains("at 6, 7"));
}

#[test]
fn eval_string_command_is_a_call_once() {
    let src = "f() { :; }\ng() { :; }\nrun() {\n  eval 'f'\n  eval \"$(g)\"\n  eval \"$cmd\"\n}\n";
    let (_d, wiki) = compiled_wiki(&[("s.sh", src)]);
    assert!(callers_text(&wiki, "s", "f", None).contains("run  calls f at 4"));
    // `$(g)` is already a call in the outer tree: not counted twice.
    let text = callers_text(&wiki, "s", "g", None);
    assert!(
        text.contains("run  calls g at 5\n") || text.ends_with("calls g at 5"),
        "{text}"
    );
}

#[test]
fn a_string_that_is_no_code_names_no_call() {
    let src = "f() { :; }\nrun() {\n  echo 'f'\n  trap - EXIT\n  bash 'f'\n}\n";
    let (_d, wiki) = compiled_wiki(&[("s.sh", src)]);
    assert!(callers_text(&wiki, "s", "f", None).contains("0 functions"));
}
