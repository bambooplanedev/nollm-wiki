//! `Wiki::load`, `search`, and `neighbors` against a compiled output directory, including the pack budget.

use std::fs;
use tempfile::tempdir;
use wiki::model::{KindFilter, SourceKind};
use wiki::query::{PackBudget, Wiki};
use wiki::{compile, CompileOptions};

fn build() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(
        input.join("alpha.txt"),
        "# Alpha\n\nAlpha mentions Beta and Gamma.\n",
    )
    .unwrap();
    fs::write(
        input.join("beta.txt"),
        "# Beta\n\nBeta is about beta things.\n",
    )
    .unwrap();
    fs::write(input.join("gamma.txt"), "# Gamma\n\nGamma stands alone.\n").unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();
    // stash output path by leaking dir via returning it
    dir
}

/// Mirror of the compiler's estimate (`manifest::token_estimate`): chars/4.
/// If this drifts from manifest.rs the ceiling tests below will catch it.
fn tok(s: &str) -> usize {
    s.chars().count() / 4
}

/// hub -> popular, rare; three fillers point at popular so its pagerank
/// beats rare's. Returns the tempdir; the compiled wiki is in `<dir>/out`.
fn build_hub_corpus() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(
        input.join("hub.txt"),
        "# Hub\n\nHub mentions Popular and Rare.\n",
    )
    .unwrap();
    fs::write(
        input.join("popular.txt"),
        "# Popular\n\nPopular is a popular topic.\n",
    )
    .unwrap();
    fs::write(
        input.join("rare.txt"),
        "# Rare\n\nRare stands alone here.\n",
    )
    .unwrap();
    for n in ["one", "two", "three"] {
        fs::write(
            input.join(format!("filler_{n}.txt")),
            format!("# Filler {n}\n\nFiller {n} talks about Popular.\n"),
        )
        .unwrap();
    }
    compile(&input, &output, &CompileOptions::default()).unwrap();
    dir
}

#[test]
fn search_finds_by_name_and_body() {
    let dir = build();
    let w = Wiki::load(&dir.path().join("out")).unwrap();
    let hits = w.search("beta", None, 10);
    assert!(hits.iter().any(|h| h.id == "beta"));
}

#[test]
fn page_is_served_from_the_loaded_snapshot() {
    let dir = build();
    let out = dir.path().join("out");
    let w = Wiki::load(&out).unwrap();
    let original = w.page("beta").unwrap();
    fs::write(out.join("beta.md"), "changed").unwrap();
    assert_eq!(w.page("beta").unwrap(), original);
    assert!(w.search("things", None, 10).iter().any(|h| h.id == "beta"));
}

#[test]
fn neighbors_pack_includes_target_first_and_respects_max_nodes() {
    let dir = build();
    let w = Wiki::load(&dir.path().join("out")).unwrap();
    let budget = PackBudget {
        max_nodes: Some(2),
        ..Default::default()
    };
    let pack = w.neighbors("alpha", 1, &budget).unwrap();
    assert_eq!(pack.included.first().map(String::as_str), Some("alpha"));
    assert!(pack.included.len() <= 2);
    assert!(pack.text.contains("# Alpha"));
}

#[test]
fn search_ignores_generated_chrome() {
    let dir = build();
    let w = Wiki::load(&dir.path().join("out")).unwrap();

    // "metadata" appears only inside the generated "## Metadata" chrome of every
    // page, never in a source body — it must not match after the fix.
    let chrome = w.search("metadata", None, 10);
    let ids: Vec<_> = chrome.iter().map(|h| h.id.clone()).collect();
    assert!(chrome.is_empty(), "chrome word matched pages: {ids:?}");

    // A genuine body word still matches (alpha's body: "Alpha mentions Beta and Gamma.").
    let body = w.search("mentions", None, 10);
    assert!(
        body.iter().any(|h| h.id == "alpha"),
        "body word should still match: {:?}",
        body.iter().map(|h| &h.id).collect::<Vec<_>>()
    );
}

/// Spec §2: budgets must degrade by dropping the LOWEST-centrality
/// neighbors first, keeping the highest-centrality ones that fit.
///
/// Self-probing budget: a `max_nodes: 2` pack (no token budget) is exactly
/// target + the highest-centrality neighbor; its measured size is then the
/// token budget that must reproduce the same selection via `max_tokens`
/// alone. Adding rare's block would push past that budget, so only
/// popular survives — regardless of how block costs are computed.
#[test]
fn neighbors_pack_max_tokens_keeps_highest_centrality_neighbor() {
    let dir = build_hub_corpus();
    let w = Wiki::load(&dir.path().join("out")).unwrap();

    let probe = w
        .neighbors(
            "hub",
            1,
            &PackBudget {
                max_nodes: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        probe.included.contains(&"popular".to_string()),
        "probe must keep the highest-centrality neighbor: {:?}",
        probe.included
    );
    let b = tok(&probe.text);

    let pack = w
        .neighbors(
            "hub",
            1,
            &PackBudget {
                max_tokens: Some(b),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(pack.included.first().map(String::as_str), Some("hub"));
    assert_eq!(
        pack.included.len(),
        2,
        "expected target + exactly one neighbor at budget {b}, got {:?}",
        pack.included
    );
    assert!(
        pack.included.contains(&"popular".to_string()),
        "expected the highest-centrality neighbor (popular), got {:?}",
        pack.included
    );
}

/// `full_neighbors` + `max_tokens` must keep the highest-centrality neighbor even
/// when several lighter, lower-centrality neighbors would pack more nodes into
/// the same budget. Fixture: hub -> big, sa, sb. `big` gets three extra
/// incoming links (fillers) so its pagerank beats sa/sb, and a long body so
/// its rendered page outweighs sa's + sb's pages combined. Budget is derived
/// from a `max_nodes: 2` probe pack: the centrality-first greedy keeps
/// {big}; a cardinality-maximizer would instead keep {sa, sb}. Token
/// estimates are read from the rendered pages, not `index.json`, since page
/// size (the new cost basis) is what the budget accounting measures.
#[test]
fn full_neighbors_max_tokens_prefers_centrality_over_packing() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(
        input.join("hub.txt"),
        "# Hub\n\nHub mentions Big and Sa and Sb.\n",
    )
    .unwrap();
    let long = "Big has a deliberately long body so that its rendered page \
outweighs the two small neighbor pages combined, which is what forces the \
centrality versus packing distinction this test pins down. "
        .repeat(4);
    fs::write(input.join("big.txt"), format!("# Big\n\n{long}\n")).unwrap();
    fs::write(input.join("sa.txt"), "# Sa\n\nSa is short.\n").unwrap();
    fs::write(input.join("sb.txt"), "# Sb\n\nSb is short.\n").unwrap();
    for n in ["one", "two", "three"] {
        fs::write(
            input.join(format!("filler_{n}.txt")),
            format!("# Filler {n}\n\nFiller {n} talks about Big.\n"),
        )
        .unwrap();
    }
    compile(&input, &output, &CompileOptions::default()).unwrap();

    let page = |id: &str| fs::read_to_string(output.join(format!("{id}.md"))).unwrap();
    let (big, small_a, small_b) = (tok(&page("big")), tok(&page("sa")), tok(&page("sb")));
    assert!(
        small_a + small_b < big,
        "fixture invalid: need sa({small_a}) + sb({small_b}) < big({big}) in page tokens"
    );

    let w = Wiki::load(&output).unwrap();
    let probe = w
        .neighbors(
            "hub",
            1,
            &PackBudget {
                max_nodes: Some(2),
                full_neighbors: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        probe.included.contains(&"big".to_string()),
        "probe must keep the highest-centrality neighbor: {:?}",
        probe.included
    );

    let budget = PackBudget {
        max_tokens: Some(tok(&probe.text)),
        full_neighbors: true,
        ..Default::default()
    };
    let pack = w.neighbors("hub", 1, &budget).unwrap();
    assert!(
        pack.included.contains(&"big".to_string()),
        "highest-centrality neighbor dropped: {:?}",
        pack.included
    );
    assert!(
        !pack.included.contains(&"sa".to_string()) && !pack.included.contains(&"sb".to_string()),
        "packing beat centrality — lighter low-centrality neighbors kept: {:?}",
        pack.included
    );
}

#[test]
fn search_finds_text_under_embedded_subheadings() {
    // A markdown doc whose body has its own `## ` subheading: parse_sections
    // splits there, and pre-fix the text below it is invisible to search.
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(
        input.join("doc.md"),
        "# Doc\n\nIntro paragraph.\n\n## Deep Section\n\nThe zanzibar rule lives here.\n",
    )
    .unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();
    let w = Wiki::load(&output).unwrap();

    let hits = w.search("zanzibar", None, 10);
    assert!(
        hits.iter().any(|h| h.id == "doc"),
        "text under an embedded subheading must be searchable"
    );
}

/// Fixture for the degrade tests: hub's body is long enough that its
/// rendered page alone exceeds a 100-token budget, and hub links to two
/// small neighbors.
fn build_oversized_hub_corpus() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    let padding = "Padding sentence for sheer bulk in the body. ".repeat(40);
    fs::write(
        input.join("hub.txt"),
        format!("# Hub\n\nHub mentions Popular and Rare.\n\n{padding}\n"),
    )
    .unwrap();
    fs::write(
        input.join("popular.txt"),
        "# Popular\n\nPopular is a popular topic.\n",
    )
    .unwrap();
    fs::write(
        input.join("rare.txt"),
        "# Rare\n\nRare stands alone here.\n",
    )
    .unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();
    dir
}

/// Spec §2: `max_tokens` is a hard ceiling on `token_estimate(pack.text)` —
/// at every budget, with the single documented floor exception (a target-
/// only pack whose degraded block is itself over budget).
#[test]
fn max_tokens_is_a_hard_ceiling_on_pack_size() {
    let dir = build_oversized_hub_corpus();
    let w = Wiki::load(&dir.path().join("out")).unwrap();
    for b in [5usize, 10, 25, 50, 100, 200, 500, 2000] {
        let budget = PackBudget {
            max_tokens: Some(b),
            ..Default::default()
        };
        let pack = w.neighbors("hub", 1, &budget).unwrap();
        let is_floor =
            pack.included.len() == 1 && pack.text.contains(wiki::query::OVER_BUDGET_NOTE);
        assert!(
            tok(&pack.text) <= b || is_floor,
            "budget {b}: pack is {} tokens, included {:?}",
            tok(&pack.text),
            pack.included
        );
    }
}

/// Spec §3: a target too big for the budget degrades to title + summary +
/// pointer — and the neighborhood still fits.
#[test]
fn oversized_target_degrades_to_summary_and_keeps_neighborhood() {
    let dir = build_oversized_hub_corpus();
    let output = dir.path().join("out");
    let w = Wiki::load(&output).unwrap();

    let full_page = fs::read_to_string(output.join("hub.md")).unwrap();
    let b = 100usize;
    assert!(
        tok(&full_page) > b,
        "fixture invalid: hub page is only {} tokens",
        tok(&full_page)
    );

    let pack = w
        .neighbors(
            "hub",
            1,
            &PackBudget {
                max_tokens: Some(b),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        pack.text.contains(wiki::query::OVER_BUDGET_NOTE),
        "pack: {}",
        pack.text
    );
    assert!(
        pack.text.contains("wiki://page/hub"),
        "pack must point at the full page: {}",
        pack.text
    );
    assert!(
        !pack.text.contains("Padding sentence"),
        "full body must not be emitted on degrade"
    );
    assert!(
        pack.included.len() >= 2,
        "neighborhood must survive the degrade: {:?}",
        pack.included
    );
    assert!(tok(&pack.text) <= b, "pack is {} tokens", tok(&pack.text));
}

/// Spec §3 floor: even a budget the degraded block itself cannot fit
/// still returns the degraded block — never an empty pack.
#[test]
fn tiny_budget_still_returns_the_degraded_target_block() {
    let dir = build_oversized_hub_corpus();
    let w = Wiki::load(&dir.path().join("out")).unwrap();
    let pack = w
        .neighbors(
            "hub",
            1,
            &PackBudget {
                max_tokens: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(pack.included, vec!["hub".to_string()]);
    assert!(pack.text.contains(wiki::query::OVER_BUDGET_NOTE));
    assert!(pack.text.contains("# Hub"));
}

/// Spec §2: no `max_tokens` → unbudgeted behavior, full target, all
/// neighbors admitted.
#[test]
fn no_max_tokens_keeps_full_target_and_all_neighbors() {
    let dir = build_hub_corpus();
    let w = Wiki::load(&dir.path().join("out")).unwrap();
    let pack = w.neighbors("hub", 1, &PackBudget::default()).unwrap();
    assert!(
        !pack.text.contains(wiki::query::OVER_BUDGET_NOTE),
        "must not degrade without a budget"
    );
    assert!(
        pack.text.contains("## Body"),
        "full rendered target expected"
    );
    assert!(pack.included.contains(&"popular".to_string()));
    assert!(pack.included.contains(&"rare".to_string()));
}

/// The kind filter scopes ranking stats and results to one `SourceKind`:
/// a token shared by a text page and a Rust page must come back as only
/// the text page under `Text`, only the code page under `Code{rust}`, and
/// both unfiltered.
#[test]
fn search_kind_filter_restricts_to_that_kind() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(
        input.join("guide.txt"),
        "# Guide\n\nThe zebra token lives in prose.\n",
    )
    .unwrap();
    fs::write(
        input.join("zoo.rs"),
        "//! zebra module\npub fn zebra() -> u8 { 0 }\n",
    )
    .unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();
    let w = Wiki::load(&output).unwrap();
    let ids =
        |hits: Vec<wiki::query::Hit>| -> Vec<String> { hits.into_iter().map(|h| h.id).collect() };

    assert_eq!(
        ids(w.search("zebra", Some(&KindFilter::Exact(SourceKind::Text)), 10)),
        ["guide"]
    );
    assert_eq!(
        ids(w.search(
            "zebra",
            Some(&KindFilter::Exact(SourceKind::Code {
                lang: "rust".into()
            })),
            10
        )),
        ["zoo"]
    );
    let mut both = ids(w.search("zebra", None, 10));
    both.sort();
    assert_eq!(both, ["guide", "zoo"]);
}

/// alpha -> beta -> gamma, with no alpha -> gamma edge: depth 1 stops at
/// beta, depth 2 reaches gamma.
#[test]
fn neighbors_depth_two_reaches_the_second_hop() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("alpha.txt"), "# Alpha\n\nAlpha mentions Beta.\n").unwrap();
    fs::write(input.join("beta.txt"), "# Beta\n\nBeta mentions Gamma.\n").unwrap();
    fs::write(input.join("gamma.txt"), "# Gamma\n\nGamma stands alone.\n").unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();
    let w = Wiki::load(&output).unwrap();

    let one = w.neighbors("alpha", 1, &PackBudget::default()).unwrap();
    assert!(
        one.included.contains(&"beta".to_string()),
        "{:?}",
        one.included
    );
    assert!(
        !one.included.contains(&"gamma".to_string()),
        "depth 1 must not reach the second hop: {:?}",
        one.included
    );
    assert!(!one.text.contains("## Gamma (gamma)"), "pack: {}", one.text);

    let two = w.neighbors("alpha", 2, &PackBudget::default()).unwrap();
    assert_eq!(two.included.first().map(String::as_str), Some("alpha"));
    assert!(
        two.included.contains(&"gamma".to_string()),
        "depth 2 must reach the second hop: {:?}",
        two.included
    );
    // Non-full neighbor blocks are `## Title (id)` + summary.
    assert!(two.text.contains("## Gamma (gamma)"), "pack: {}", two.text);
}

fn kind_fixture() -> (tempfile::TempDir, Wiki) {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("a.rs"), "//! zebra\npub fn zebra() {}\n").unwrap();
    fs::write(input.join("b.cpp"), "// zebra\nint zebra() { return 0; }\n").unwrap();
    fs::write(input.join("c.md"), "# C\n\nzebra in markdown.\n").unwrap();
    fs::write(input.join("d.txt"), "# D\n\nzebra in text.\n").unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();
    let w = Wiki::load(&output).unwrap();
    (dir, w)
}

fn ids_for(w: &Wiki, f: &str) -> Vec<String> {
    let f = KindFilter::parse(f).unwrap();
    let mut v: Vec<String> = w
        .search("zebra", Some(&f), 10)
        .into_iter()
        .map(|h| h.id)
        .collect();
    v.sort();
    v
}

#[test]
fn kind_code_group_returns_only_code() {
    let (_d, w) = kind_fixture();
    // a.rs and b.cpp are the code pages; c.md and d.txt must not appear.
    assert_eq!(ids_for(&w, "code"), ["a", "b"]);
}

#[test]
fn kind_docs_group_returns_only_docs() {
    let (_d, w) = kind_fixture();
    assert_eq!(ids_for(&w, "docs"), ["c", "d"]);
}

#[test]
fn kind_exact_code_rust_still_works() {
    let (_d, w) = kind_fixture();
    assert_eq!(ids_for(&w, "code:rust"), ["a"]);
    assert_eq!(ids_for(&w, "markdown"), ["c"]);
    assert!(KindFilter::parse("nope").is_none());
}

#[test]
fn search_without_items_json_has_no_symbol_lines() {
    let dir = tempdir().unwrap();
    let (input, out) = (dir.path().join("raw"), dir.path().join("out"));
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("lib.rs"), "pub fn alpha_beta() {}\n").unwrap();
    compile(&input, &out, &CompileOptions::default()).unwrap();

    let wiki = Wiki::load(&out).unwrap();
    let ids: Vec<String> = wiki.list_pages().into_iter().map(|(id, _)| id).collect();
    assert!(ids.iter().any(|id| !wiki.items(id).is_empty()));

    for bad in [None, Some("not json")] {
        match bad {
            None => fs::remove_file(out.join("items.json")).unwrap(),
            Some(s) => fs::write(out.join("items.json"), s).unwrap(),
        }
        let wiki = Wiki::load(&out).unwrap();
        assert!(ids.iter().all(|id| wiki.items(id).is_empty()));
    }
}

/// Compiles `files` (relative path, contents) and loads the wiki.
fn code_fixture(files: &[(&str, &str)]) -> (tempfile::TempDir, Wiki) {
    let dir = tempdir().unwrap();
    let (input, out) = (dir.path().join("raw"), dir.path().join("out"));
    for (path, text) in files {
        let p = input.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    compile(&input, &out, &CompileOptions::default()).unwrap();
    let w = Wiki::load(&out).unwrap();
    (dir, w)
}

/// Every symbol line of a response in output order: `(path, start, end, name)`.
fn symbol_lines(hits: &[wiki::query::Hit]) -> Vec<(String, usize, usize, String)> {
    hits.iter()
        .flat_map(|h| &h.symbols)
        .map(|s| (s.path.clone(), s.start_line, s.end_line, s.name.clone()))
        .collect()
}

#[test]
fn private_fn_is_found_by_name_with_its_lines() {
    let (_d, w) = code_fixture(&[("src/a.rs", "fn helper_thing() {}\n")]);
    let hits = w.search("helper_thing", None, 10);
    let a = hits.iter().find(|h| h.id == "a").expect("page a is a hit");
    assert_eq!(a.path, "src/a.rs");
    assert_eq!(
        symbol_lines(std::slice::from_ref(a)),
        [("src/a.rs".to_string(), 1, 1, "helper_thing".to_string())]
    );
}

#[test]
fn method_is_found_by_split_words() {
    let (_d, w) = code_fixture(&[
        (
            "src/s.rs",
            "pub struct S;\nimpl S {\n    fn on_order_reject(&self) {}\n}\n",
        ),
        ("src/other.rs", "pub fn unrelated() {}\n"),
    ]);
    let lines = symbol_lines(&w.search("order reject", None, 10));
    assert_eq!(
        lines.first(),
        Some(&(
            "src/s.rs".to_string(),
            3,
            3,
            "S::on_order_reject".to_string()
        )),
        "{lines:?}"
    );
}

#[test]
fn module_items_never_show() {
    let (_d, w) = code_fixture(&[
        ("src/lib.rs", "mod formats;\npub fn list_formats() {}\n"),
        ("src/formats.rs", "pub fn formats_all() {}\n"),
    ]);
    let lines = symbol_lines(&w.search("formats", None, 10));
    assert!(!lines.is_empty(), "the fns still show");
    assert!(lines.iter().all(|l| l.3 != "formats"), "{lines:?}");
}

#[test]
fn test_items_do_not_lift_a_page_but_still_show() {
    // `aa` says the words more often, so a lifted test fn would put it first.
    let (_d, w) = code_fixture(&[
        ("src/zz.rs", "fn zebra_walk() {}\n"),
        (
            "tests/aa.rs",
            "#[test]\nfn zebra_walk_works() {\n    zebra_walk();\n    zebra_walk();\n}\n",
        ),
    ]);
    let hits = w.search("zebra walk", None, 10);
    let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids, ["zz", "aa"]);
    let test_line = &hits[1].symbols;
    assert_eq!(test_line.len(), 1, "the test fn still shows");
    assert_eq!(test_line[0].name, "zebra_walk_works");
    assert!(test_line[0].test);
}

#[test]
fn symbol_lines_are_unique_per_page() {
    // Overloads in a header and their definitions in the `.cpp` twin: one
    // line per item, every line on its own page's path, a doc shown once.
    let (_d, w) = code_fixture(&[
        (
            "src/fsm.h",
            "class Fsm {\npublic:\n    /// Handles an order reject.\n    void onOrderReject();\n    /// Handles an order reject.\n    void onOrderReject(int code);\n};\n",
        ),
        (
            "src/fsm.cpp",
            "#include \"fsm.h\"\nvoid Fsm::onOrderReject() {}\nvoid Fsm::onOrderReject(int code) {}\n",
        ),
    ]);
    let hits = w.search("order reject", None, 10);
    assert!(
        hits.iter().any(|h| h.symbols.len() == 2),
        "both overloads show"
    );
    for h in &hits {
        let keys: std::collections::BTreeSet<_> =
            symbol_lines(std::slice::from_ref(h)).into_iter().collect();
        assert_eq!(
            keys.len(),
            h.symbols.len(),
            "duplicate lines under {}",
            h.id
        );
        assert!(h.symbols.iter().all(|s| s.path == h.path));
        let docs: Vec<&str> = h.symbols.iter().filter_map(|s| s.doc.as_deref()).collect();
        let unique: std::collections::BTreeSet<_> = docs.iter().collect();
        assert_eq!(unique.len(), docs.len(), "a doc repeats under {}", h.id);
    }
}

#[test]
fn weak_lines_are_cut_response_wide() {
    // `other` matches one token by doc only: below 0.3 of the best line in
    // the response, though it is the best line of its own page.
    let (_d, w) = code_fixture(&[
        ("src/a.rs", "pub fn zebra_quokka() {}\n"),
        ("src/b.rs", "/// Mentions quokka.\nfn other() {}\n"),
    ]);
    let hits = w.search("zebra quokka", None, 10);
    assert!(hits.iter().any(|h| h.id == "b"), "b is still a page hit");
    let names: Vec<String> = symbol_lines(&hits).into_iter().map(|l| l.3).collect();
    assert_eq!(names, ["zebra_quokka"]);
}

#[test]
fn whole_name_outranks_an_earlier_word_match() {
    // `resolve_link` matches `resolve` by a word and sits first in the file;
    // the method named exactly `resolve` must still be the first line.
    let (_d, w) = code_fixture(&[(
        "src/g.rs",
        "fn resolve_link() {}\npub struct G;\nimpl G {\n    fn resolve(&self) {}\n}\n",
    )]);
    let lines = symbol_lines(&w.search("resolve", None, 10));
    assert_eq!(
        lines.first(),
        Some(&("src/g.rs".to_string(), 4, 4, "G::resolve".to_string())),
        "{lines:?}"
    );
}

#[test]
fn snippet_drops_comment_markers() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(
        input.join("a.rs"),
        "//! Foo bar\n/// Foo baz\npub fn f() {}\n",
    )
    .unwrap();
    fs::write(input.join("b.py"), "# Foo qux\ndef g(): pass\n").unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();
    let w = Wiki::load(&output).unwrap();
    let hits = w.search("foo", None, 10);
    assert_eq!(hits.len(), 2);
    for h in hits {
        let s = h.snippet.unwrap();
        assert!(s.starts_with("Foo "), "snippet: {s}");
        assert!(!s.contains("//") && !s.contains('#'), "snippet: {s}");
    }
}

#[test]
fn snippet_keeps_markers_that_are_content() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("raw");
    let output = dir.path().join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(
        input.join("a.txt"),
        "# T\nintro\n#include zork.h\n#[derive(Debug)] zork\n*ptr = zork;\n*note* zork\n",
    )
    .unwrap();
    fs::write(input.join("c.txt"), "# C\n/*\n * Zork block\n */\n").unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();
    let w = Wiki::load(&output).unwrap();
    let hits = w.search("zork", None, 10);
    let snips: Vec<String> = hits.into_iter().filter_map(|h| h.snippet).collect();
    let all = snips.join("\n");
    for kept in [
        "#include zork.h",
        "#[derive(Debug)]",
        "*ptr = zork",
        "*note* zork",
    ] {
        assert!(all.contains(kept), "{kept} lost: {all}");
    }
    assert!(
        all.contains("Zork block") && !all.contains("* Zork"),
        "{all}"
    );
}

#[test]
fn search_lines_keeps_one_line_per_hit_and_symbol() {
    use wiki::query::{Hit, SymbolLine};
    let hit = Hit {
        id: "a".into(),
        title: "Ti\ntle\tx".into(),
        path: "a.rs".into(),
        summary: Some("sum\nmary".into()),
        score: 1.0,
        snippet: None,
        symbols: vec![SymbolLine {
            path: "a.rs".into(),
            start_line: 1,
            end_line: 2,
            name: "f".into(),
            signature: "fn f(\n)".into(),
            doc: Some("do\tc\nline".into()),
            test: false,
        }],
    };
    let out = wiki::serve::search_lines(&[hit]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        [
            "a  a.rs  Ti tle x — sum mary",
            "  a.rs:1-2  f  fn f( )  — do c line"
        ]
    );
}
