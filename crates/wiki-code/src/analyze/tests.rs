//! Tests for `analyze`.
use super::*;

fn def_text(a: &Analyzed, name: &str) -> String {
    let i = a.file.items.iter().position(|it| it.name == name).unwrap();
    let d = a.defs[i].as_ref().unwrap();
    a.sources[d.source].text()[d.start_byte..d.end_byte].to_string()
}

#[test]
fn every_function_item_points_at_its_own_node() {
    let a = analyze(
        "src/a.rs",
        "fn a() {} fn a2() { g(); }\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n",
    )
    .unwrap();
    assert_eq!(def_text(&a, "a"), "fn a() {}");
    assert_eq!(def_text(&a, "a2"), "fn a2() { g(); }");
    assert_eq!(def_text(&a, "t"), "fn t() {}"); // items of a stripped test module still have nodes
    assert!(analyze("notes.txt", "x").is_none());
}

#[test]
fn defs_hold_in_every_family() {
    for (path, src, name, want) in [
            ("a.py", "def f(x):\n    return g(x)\n", "f", "def f(x):\n    return g(x)"),
            ("a.go", "package a\nfunc F(x int) int { return g(x) }\n", "F", "func F(x int) int { return g(x) }"),
            ("a.ts", "export function f(n: number) { return g(n) }\n", "f", "function f(n: number) { return g(n) }"),
            ("a.sh", "f() {\n  g \"$1\"\n}\nf x\n", "f", "f() {\n  g \"$1\"\n}"),
            ("a.cpp", "#if X\nint f(int a) { return g(a); }\n#else\nint h(int a) { return a; }\n#endif\n", "h", "int h(int a) { return a; }"),
            ("a.vue", "<template><div/></template>\n<script setup lang=\"ts\">\nfunction go(n: number) { emit(n) }\n</script>\n", "go", "function go(n: number) { emit(n) }"),
        ] {
            let a = analyze(path, src).unwrap();
            let i = a.file.items.iter().position(|it| it.name == name).unwrap();
            let d = a.defs[i].as_ref().unwrap();
            let s = &a.sources[d.source];
            assert!(s.is_function(s.node(d).unwrap()), "{path}");
            assert_eq!(def_text(&a, name), want, "{path}");
        }
}

#[test]
fn a_function_split_by_the_preprocessor_has_a_def_in_each_tree() {
    let src = "#ifdef A\nint f(int a) {\n#else\nint f(long a) {\n#endif\n  return g(a);\n}\n";
    let a = analyze("a.cpp", src).unwrap();
    assert_eq!(a.sources.len(), 2);
    assert_eq!(a.defs.len(), a.file.items.len());
    let trees: Vec<usize> = a.defs.iter().flatten().map(|d| d.source).collect();
    assert!(trees.contains(&0) && trees.contains(&1), "{trees:?}");
}

#[test]
fn shell_fallback_items_have_no_node() {
    let src = "FOO=1\nf() {\n echo\n}\nh() { ((( ; }\nBAZ=3\nf\n";
    let a = analyze("a.sh", src).unwrap();
    assert_eq!(a.defs.len(), a.file.items.len());
    let none: Vec<&str> = a
        .file
        .items
        .iter()
        .zip(&a.defs)
        .filter(|(_, d)| d.is_none())
        .map(|(it, _)| it.name.as_str())
        .collect();
    assert_eq!(none, ["h", "<top level>"]);
    let foo = a.file.items.iter().position(|it| it.name == "FOO").unwrap();
    assert!(a.defs[foo].is_some());
}

#[test]
fn calls_at_gives_literal_text_also_inside_macros() {
    let src = "use a::b as c;\nfn f(x: u8) { g(h(x)); c::k(); let s = format!(\"{}\", crate::port(\"X\", 1)); }\n";
    let a = analyze("src/a.rs", src).unwrap();
    let s = &a.sources[0];
    let mut texts = Vec::new();
    let mut stack = vec![s.tree().root_node()];
    while let Some(n) = stack.pop() {
        let mut c = n.walk();
        stack.extend(n.named_children(&mut c));
        texts.extend(
            s.calls_at(n)
                .into_iter()
                .map(|c| s.text()[c.start_byte..c.end_byte].to_string()),
        );
    }
    texts.sort();
    assert_eq!(
        texts,
        ["c::k()", "crate::port(\"X\", 1)", "g(h(x))", "h(x)"]
    );
    // and they are the sites the item got
    assert_eq!(a.file.items[0].call_sites.len(), 4);
}

#[test]
fn a_call_in_quoted_shell_code_points_at_its_own_text() {
    let src = "f() {\n  trap 'cleanup now' EXIT\n}\n";
    let a = analyze("a.sh", src).unwrap();
    let s = &a.sources[0];
    let mut texts = Vec::new();
    let mut stack = vec![s.tree().root_node()];
    while let Some(n) = stack.pop() {
        stack.extend(n.named_children(&mut n.walk()));
        for c in s.calls_at(n) {
            texts.push((c.site.name, s.text()[c.start_byte..c.end_byte].to_string()));
        }
    }
    texts.sort();
    assert_eq!(
        texts,
        [
            ("cleanup".to_string(), "cleanup now".to_string()),
            ("trap".to_string(), "trap 'cleanup now' EXIT".to_string()),
        ]
    );
}

#[test]
fn comment_text_strips_the_markers_of_each_language() {
    for (path, src, want) in [
        ("a.rs", "// Tag. Why.\nfn f() {}\n", "Tag. Why."),
        ("a.py", "# Tag. Why.\ndef f():\n    pass\n", "Tag. Why."),
        ("a.sh", "# Tag. Why.\nf() { :; }\n", "Tag. Why."),
        (
            "a.cpp",
            "/* Tag. Why. */\nint f() { return 0; }\n",
            "Tag. Why.",
        ),
    ] {
        let a = analyze(path, src).unwrap();
        let s = &a.sources[0];
        let root = s.tree().root_node();
        let mut c = root.walk();
        let n = root
            .named_children(&mut c)
            .find(|n| s.is_comment(*n))
            .unwrap();
        assert_eq!(s.comment_text(n), want, "{path}");
    }
}
