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
    let (_d, out_dir, _) = {
        let (d, _, out) = compiled(&[("ship.sh", src)]);
        (d, out.clone(), ())
    };
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
            !std::path::Path::new(p)
                .extension()
                .is_some_and(|e| e == "sh")
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
