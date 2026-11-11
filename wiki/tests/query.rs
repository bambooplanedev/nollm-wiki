//! `Wiki::load`, `search`, and `neighbors` against a compiled output directory, including the pack budget.

use std::fs;

mod common;
use tempfile::tempdir;
use wiki::model::{KindFilter, SourceKind};
use wiki::query::{tag_filter, PackBudget, Wiki, OVER_BUDGET_NOTE};
use wiki::{compile, CompileOptions};

fn build() -> tempfile::TempDir {
    common::compiled(&[
        ("alpha.txt", "# Alpha\n\nAlpha mentions Beta and Gamma.\n"),
        ("beta.txt", "# Beta\n\nBeta is about beta things.\n"),
        ("gamma.txt", "# Gamma\n\nGamma stands alone.\n"),
    ])
    .0
}

/// Mirror of the compiler's estimate (`manifest::token_estimate`): chars/4.
/// If this drifts from manifest.rs the ceiling tests below will catch it.
fn tok(s: &str) -> usize {
    s.chars().count() / 4
}

/// hub -> popular, rare; three fillers point at popular so its pagerank
/// beats rare's. Returns the tempdir; the compiled wiki is in `<dir>/out`.
fn build_hub_corpus() -> tempfile::TempDir {
    let filler = |n: &str| {
        (
            format!("filler_{n}.txt"),
            format!("# Filler {n}\n\nFiller {n} talks about Popular.\n"),
        )
    };
    let fillers = ["one", "two", "three"].map(filler);
    let mut files = vec![
        ("hub.txt", "# Hub\n\nHub mentions Popular and Rare.\n"),
        ("popular.txt", "# Popular\n\nPopular is a popular topic.\n"),
        ("rare.txt", "# Rare\n\nRare stands alone here.\n"),
    ];
    files.extend(fillers.iter().map(|(n, b)| (n.as_str(), b.as_str())));
    common::compiled(&files).0
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
    let (_dir, _, output) = common::compiled(&[(
        "doc.md",
        "# Doc\n\nIntro paragraph.\n\n## Deep Section\n\nThe zanzibar rule lives here.\n",
    )]);
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
    let padding = "Padding sentence for sheer bulk in the body. ".repeat(40);
    let hub = format!("# Hub\n\nHub mentions Popular and Rare.\n\n{padding}\n");
    common::compiled(&[
        ("hub.txt", &hub),
        ("popular.txt", "# Popular\n\nPopular is a popular topic.\n"),
        ("rare.txt", "# Rare\n\nRare stands alone here.\n"),
    ])
    .0
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
    let (_dir, _, output) = common::compiled(&[
        ("guide.txt", "# Guide\n\nThe zebra token lives in prose.\n"),
        ("zoo.rs", "//! zebra module\npub fn zebra() -> u8 { 0 }\n"),
    ]);
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
    let (_dir, _, output) = common::compiled(&[
        ("alpha.txt", "# Alpha\n\nAlpha mentions Beta.\n"),
        ("beta.txt", "# Beta\n\nBeta mentions Gamma.\n"),
        ("gamma.txt", "# Gamma\n\nGamma stands alone.\n"),
    ]);
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
    common::compiled_wiki(&[
        ("a.rs", "//! zebra\npub fn zebra() {}\n"),
        ("b.cpp", "// zebra\nint zebra() { return 0; }\n"),
        ("c.md", "# C\n\nzebra in markdown.\n"),
        ("d.txt", "# D\n\nzebra in text.\n"),
    ])
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
    let (_dir, _, out) = common::compiled(&[("lib.rs", "pub fn alpha_beta() {}\n")]);

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
    common::compiled_wiki(files)
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
    let hits = w.search("zebra", None, 10);
    let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids, ["zz", "aa"]);
    let test_line = &hits[1].symbols;
    assert_eq!(test_line.len(), 1, "the test fn still shows");
    assert_eq!(test_line[0].name, "zebra_walk_works");
    assert!(test_line[0].test);
}

#[test]
fn a_query_spelling_out_a_name_finds_its_definer() {
    // `bin_stock.rs` carries two of the words in its title and all three in
    // its docs; the query spells out `getBinStock`, which only the large
    // `client.ts` defines.
    let mut client = String::new();
    for i in 0..30 {
        client.push_str(&format!(
            "export function fetchThing{i}() {{ return {i}; }}\n"
        ));
    }
    client.push_str("export function getBinStock() { return 1; }\n");
    let (_d, w) = code_fixture(&[
        ("src/client.ts", &client),
        (
            "src/bin_stock.rs",
            "/// Get risk limits.\npub fn limits() {}\n/// Get a slot.\npub fn slot() {}\n",
        ),
    ]);
    let hits = w.search("get bin stock", None, 10);
    assert_eq!(
        hits[0].id,
        "client",
        "{:?}",
        hits.iter().map(|h| &h.id).collect::<Vec<_>>()
    );
    assert_eq!(hits[0].symbols[0].name, "getBinStock");
}

#[test]
fn query_words_in_one_name_beat_words_spread_over_names() {
    // Both pages carry `order` and `reject` as name words; only `fsm.rs`
    // has them in one name.
    let (_d, w) = code_fixture(&[
        (
            "src/fsm.rs",
            "pub fn on_order_reject() {}\npub fn on_fill() {}\n",
        ),
        (
            "src/worker.rs",
            "pub fn synth_reject() {}\npub fn spawn_parcel_drain() {}\n",
        ),
    ]);
    let hits = w.search("order reject", None, 10);
    let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids, ["fsm", "worker"]);
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
fn a_weak_page_keeps_only_its_best_line() {
    // b's lines are below 0.3 of the best line in the response: b still
    // shows its own best line, but not the weaker doc-only one.
    let (_d, w) = code_fixture(&[
        ("src/a.rs", "pub fn zebra_quokka() {}\n"),
        (
            "src/b.rs",
            "fn quokka_thing() {}\n/// Mentions quokka.\nfn other() {}\n",
        ),
    ]);
    let hits = w.search("zebra quokka", None, 10);
    assert!(hits.iter().any(|h| h.id == "b"), "b is still a page hit");
    let names: Vec<String> = symbol_lines(&hits).into_iter().map(|l| l.3).collect();
    assert_eq!(names, ["zebra_quokka", "quokka_thing"]);
}

#[test]
fn a_page_keeps_a_second_line_above_the_response_cut() {
    // `zebra_quokka_cache` is not a's best line, but it scores well above
    // 0.3 of the response's best, so the response-wide rule keeps it.
    let (_d, w) = code_fixture(&[(
        "src/a.rs",
        "pub fn zebra_quokka() {}\npub fn zebra_quokka_cache() {}\n",
    )]);
    let hits = w.search("zebra quokka", None, 10);
    let names: Vec<String> = symbol_lines(&hits).into_iter().map(|l| l.3).collect();
    assert!(names.iter().any(|n| n == "zebra_quokka_cache"), "{names:?}");
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

/// A C++ header declaring `onOrderReject` (no doc), a test file that mocks
/// it and names a test after it, a dispatch site and a docs spec. Before
/// test-only names left `defined_terms`, the test page ranked first.
fn fsm_fixture() -> (tempfile::TempDir, Wiki) {
    code_fixture(&[
        (
            "src/core_parcel_sorter.h",
            "#pragma once\n/// Parcel sorting machine.\nclass ParcelSorter {\npublic:\n    void onOrderReject(int code);\n    void onFill(int qty);\n};\n",
        ),
        (
            "tests/test_fsm.cpp",
            "#include \"../src/core_parcel_sorter.h\"\nclass MockFsm : public ParcelSorter {\npublic:\n    void onOrderReject(int code) { rejects++; }\n    int rejects = 0;\n};\nvoid test_order_reject_counts() {\n    MockFsm m;\n    m.onOrderReject(1);\n    m.onOrderReject(2);\n}\n",
        ),
        (
            "src/dispatch.cpp",
            "#include \"core_parcel_sorter.h\"\nvoid dispatch(ParcelSorter& f, int ev) {\n    if (ev == 3) f.onOrderReject(ev);\n}\n",
        ),
        (
            "docs/spec.md",
            "# Fsm spec\n\nAn order reject halts the strategy until the next order.\n",
        ),
    ])
}

#[test]
fn test_only_names_do_not_lift_a_cpp_test_page() {
    let (_d, w) = fsm_fixture();
    let hits = w.search("order reject", None, 10);
    let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    let rank = |id: &str| ids.iter().position(|i| *i == id);
    let (def, test) = (rank("core_parcel_sorter"), rank("test_fsm"));
    assert!(def.is_some() && test.is_some() && def < test, "{ids:?}");
    let lines = symbol_lines(&hits);
    assert_eq!(
        lines.first(),
        Some(&(
            "src/core_parcel_sorter.h".to_string(),
            5,
            5,
            "ParcelSorter::onOrderReject".to_string()
        )),
        "{lines:?}"
    );
}

#[test]
fn owner_qualified_token_finds_the_method() {
    let (_d, w) = fsm_fixture();
    let lines = symbol_lines(&w.search("ParcelSorter::onOrderReject", None, 10));
    assert_eq!(
        lines.first(),
        Some(&(
            "src/core_parcel_sorter.h".to_string(),
            5,
            5,
            "ParcelSorter::onOrderReject".to_string()
        )),
        "{lines:?}"
    );
}

#[test]
fn search_lines_says_no_matches_when_empty() {
    assert_eq!(wiki::serve::search_lines(&[]), "no matches\n");
}

#[test]
fn snippet_drops_comment_markers() {
    let (_dir, _, output) = common::compiled(&[
        ("a.rs", "//! Foo bar\n/// Foo baz\npub fn f() {}\n"),
        ("b.py", "# Foo qux\ndef g(): pass\n"),
    ]);
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
    let (_dir, _, output) = common::compiled(&[
        (
            "a.txt",
            "# T\nintro\n#include zork.h\n#[derive(Debug)] zork\n*ptr = zork;\n*note* zork\n",
        ),
        ("c.txt", "# C\n/*\n * Zork block\n */\n"),
    ]);
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
        updated: None,
        score: 1.0,
        memory: false,
        origin: wiki::model::Origin::Local,
        superseded_by: None,
        snippet: None,
        symbols: vec![SymbolLine {
            path: "a.rs".into(),
            start_line: 1,
            end_line: 2,
            name: "f".into(),
            signature: "fn f(\n)".into(),
            doc: Some("do\tc\nline".into()),
            test: false,
            nested: false,
            also: None,
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

/// `(id, title)` of the page whose path is `path`.
fn id_title(w: &Wiki, path: &str) -> (String, String) {
    let h = w
        .search("zebra", None, 50)
        .into_iter()
        .find(|h| h.path == path)
        .unwrap_or_else(|| panic!("no page at {path}"));
    (h.id, h.title)
}

fn resolve_fixture() -> (tempfile::TempDir, Wiki) {
    code_fixture(&[
        ("src/manager.cpp", "// zebra\nint zebra() { return 1; }\n"),
        ("a/mod.rs", "//! zebra\npub fn zebra() {}\n"),
        ("b/mod.rs", "//! zebra\npub fn zebra_two() {}\n"),
    ])
}

#[test]
fn title_resolves_to_id() {
    let (_d, w) = resolve_fixture();
    let (id, title) = id_title(&w, "src/manager.cpp");
    assert_eq!(w.resolve_id(&title.to_uppercase()), Ok(id.clone()));
    assert_eq!(w.resolve_id(&id), Ok(id));
}

#[test]
fn slug_of_title_resolves() {
    let (_d, w) = resolve_fixture();
    let (id, _) = id_title(&w, "src/manager.cpp");
    assert_eq!(w.resolve_id(&id.replace('_', " ").to_uppercase()), Ok(id));
}

#[test]
fn unique_path_suffix_resolves() {
    let (_d, w) = resolve_fixture();
    let (id, _) = id_title(&w, "src/manager.cpp");
    assert_eq!(w.resolve_id("manager.cpp"), Ok(id.clone()));
    assert_eq!(w.resolve_id("src/manager.cpp"), Ok(id));
}

#[test]
fn unknown_path_suggests_pages_of_the_same_file_name() {
    let (_d, w) = resolve_fixture();
    let (a, _) = id_title(&w, "a/mod.rs");
    let (b, _) = id_title(&w, "b/mod.rs");
    let (m, _) = id_title(&w, "src/manager.cpp");
    let err = w.resolve_id("src/x/mod.rs").unwrap_err();
    assert!(
        err.contains(&a) && err.contains(&b) && !err.contains(&m),
        "{err:?}"
    );
}

#[test]
fn ambiguous_path_suffix_lists_candidates() {
    let (_d, w) = resolve_fixture();
    let (a, _) = id_title(&w, "a/mod.rs");
    let (b, _) = id_title(&w, "b/mod.rs");
    let err = w.resolve_id("mod.rs").unwrap_err();
    assert!(err.contains(&a) && err.contains(&b), "{err:?}");
}

#[test]
fn resolve_names_a_skipped_or_hidden_folder() {
    let (_d, raw, out) = common::compiled(&[
        ("src/a.rs", "pub fn a() {}\n"),
        ("docs/superpowers/plans/p.md", "# P\n\n## Step 1\n"),
        ("third_party/x/lib.cpp", "int f();\n"),
    ]);
    let w = Wiki::load(&out).unwrap();
    let r = |a: &str| w.resolve(a).unwrap_err();
    assert_eq!(
        r("docs/superpowers/plans/p.md"),
        "\"docs/superpowers/plans/p.md\" is under a skipped `superpowers` folder, which the wiki does not compile; read it with your file tool, or pass \"docs/superpowers/plans/p.md\" in search's `docs` for its headings' line ranges"
    );
    assert_eq!(
        r("./third_party/x/lib.cpp"),
        "\"./third_party/x/lib.cpp\" is under a skipped `third_party` folder, which the wiki does not compile; read it with your file tool"
    );
    let e = r(".superpowers/specs/s.MD");
    assert!(
        e.contains("under a hidden `.superpowers` folder") && e.contains("in search's"),
        "{e}"
    );
    let e = r(".github/vendor/x.js");
    assert!(e.contains("hidden `.github` "), "{e}");
    for a in [
        "src/missing/a.rs",
        "../superpowers/p.md",
        "/abs/docs/superpowers/p.md",
        ".env",
        "Superpowers Plan",
    ] {
        assert!(r(a).starts_with("unknown page"), "{a}: {}", r(a));
    }
    assert!(
        r("src/missing/a.rs").contains("closest: a"),
        "{}",
        r("src/missing/a.rs")
    );
    // The `docs` advice works: it reads the file from disk, not the index.
    let docs = wiki::labels::doc_search(&raw, &["docs/superpowers/plans/p.md".into()], "", None);
    assert!(docs
        .0
        .iter()
        .any(|h| h.path == "docs/superpowers/plans/p.md"));
}

#[test]
fn resolve_keeps_unknown_page_when_the_folder_was_compiled() {
    let tmp = tempdir().unwrap();
    // (a) The compile input is itself a skipped folder: root == "superpowers".
    let input = tmp.path().join("docs/superpowers");
    common::write(&input, "plans/p.md", "# P\n\n## Step 1\n");
    let out = tmp.path().join("out-a");
    compile(&input, &out, &CompileOptions::default()).unwrap();
    let e = Wiki::load(&out)
        .unwrap()
        .resolve("superpowers/plans/p.md")
        .unwrap_err();
    assert!(
        e.starts_with("unknown page") && e.contains("closest: p"),
        "{e}"
    );
    // (b) `--no-ignore` compiled `vendor/`.
    let input = tmp.path().join("raw");
    common::write(&input, "vendor/y.js", "function y() {}\n");
    common::write(&input, "src/a.rs", "pub fn a() {}\n");
    let out = tmp.path().join("out-b");
    let opts = CompileOptions {
        respect_ignore: false,
        ..Default::default()
    };
    compile(&input, &out, &opts).unwrap();
    let e = Wiki::load(&out)
        .unwrap()
        .resolve("vendor/typo.js")
        .unwrap_err();
    assert!(e.starts_with("unknown page"), "{e}");
    // (c) The critique's repro: a path typed from above an input named `wiki`.
    let input = tmp.path().join("wiki");
    common::write(&input, "src/query.rs", "pub fn q() {}\n");
    common::write(&input, "vendor/v.rs", "pub fn v() {}\n");
    let out = tmp.path().join("out-c");
    compile(&input, &out, &CompileOptions::default()).unwrap();
    let e = Wiki::load(&out)
        .unwrap()
        .resolve("wiki/src/query.rs")
        .unwrap_err();
    assert!(e.contains("closest: query"), "{e}");
    // (d) A hidden folder an ignore file whitelists is compiled.
    let input = tmp.path().join("gh");
    common::write(&input, ".ignore", "!.github/\n");
    common::write(&input, ".github/w/x.rs", "pub fn x() {}\n");
    let out = tmp.path().join("out-d");
    compile(&input, &out, &CompileOptions::default()).unwrap();
    let e = Wiki::load(&out)
        .unwrap()
        .resolve(".github/w/typo.rs")
        .unwrap_err();
    assert!(e.starts_with("unknown page"), "{e}");
}

fn readme_fixture() -> (tempfile::TempDir, Wiki) {
    code_fixture(&[
        ("README.md", "# Root zebra\n\nzebra root.\n"),
        ("docs/README.md", "# Docs zebra\n\nzebra docs.\n"),
        ("src/lib.rs", "//! zebra\npub fn zebra() {}\n"),
    ])
}

#[test]
fn exact_path_beats_suffix() {
    let (_d, w) = readme_fixture();
    let (root, _) = id_title(&w, "README.md");
    let (docs, _) = id_title(&w, "docs/README.md");
    assert_eq!(w.resolve_id("README.md"), Ok(root));
    assert_eq!(w.resolve_id("docs/README.md"), Ok(docs));
}

#[test]
fn absolute_and_normalised_paths_resolve() {
    let (_d, w) = readme_fixture();
    let (root, _) = id_title(&w, "README.md");
    let (docs, _) = id_title(&w, "docs/README.md");
    // `code_fixture` compiles a directory named `raw`.
    for arg in [
        "./docs/README.md",
        "docs\\README.md",
        "/home/u/raw/docs/README.md",
    ] {
        assert_eq!(w.resolve_id(arg), Ok(docs.clone()), "{arg}");
    }
    assert_eq!(w.resolve_id("/home/u/raw/src/lib.rs"), Ok("lib".into()));
    assert_eq!(w.resolve_id("/home/u/raw/README.md"), Ok(root));
    assert!(w.resolve_id("/home/u/other.rs").is_err());
}

#[test]
fn a_path_under_another_root_does_not_resolve() {
    let (_d, w) = readme_fixture();
    for arg in [
        "/home/u/proj/src/lib.rs",
        "/some/other/proj/README.md",
        "/home/u/raw/x/src/lib.rs",
    ] {
        assert!(w.resolve_id(arg).is_err(), "{arg}");
    }
}

#[test]
fn index_json_without_tags_still_loads() {
    let dir = build();
    let path = dir.path().join("out").join("index.json");
    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    for e in v["entries"].as_array_mut().unwrap() {
        e.as_object_mut().unwrap().remove("tags");
    }
    fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
    let w = Wiki::load(&dir.path().join("out")).unwrap();
    assert!(!w.search("beta", None, 10).is_empty());
}

fn build_tag_corpus() -> (tempfile::TempDir, Wiki) {
    let notes: Vec<_> = (1..=12)
        .map(|i| {
            let body = format!("---\ntags: [project/wiki]\n---\n# N{i}\n\nquokka page.\n");
            (format!("n{i}.md"), body)
        })
        .collect();
    let mut files: Vec<(&str, &str)> = notes
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_str()))
        .collect();
    files.extend([
        (
            "root.md",
            "---\ntags: [Project]\n---\n# Root\n\nquokka page.\n",
        ),
        (
            "other.md",
            "---\ntags: [projects]\n---\n# Other\n\nquokka page.\n",
        ),
        ("z.rs", "//! quokka\npub fn z() {}\n"),
    ]);
    common::compiled_wiki(&files)
}

#[test]
fn tag_only_lists_every_tagged_page_in_path_order() {
    let (_d, w) = build_tag_corpus();
    let hits = w.search_tagged("", None, Some("project"), None);
    let paths: Vec<_> = hits.iter().map(|h| h.path.as_str()).collect();
    assert_eq!(hits.len(), 13);
    assert!(paths.windows(2).all(|p| p[0] <= p[1]));
    assert!(hits.iter().all(|h| h.score == 0.0 && h.snippet.is_none()));
    assert_eq!(w.search_tagged("", None, Some("project"), Some(3)).len(), 3);
}

#[test]
fn tag_filters_a_query_and_combines_with_kind() {
    let (_d, w) = build_tag_corpus();
    let hits = w.search_tagged("quokka", None, Some("project"), Some(50));
    assert_eq!(hits.len(), 13);
    assert!(hits.iter().all(|h| std::path::Path::new(&h.path)
        .extension()
        .is_some_and(|e| e == "md")
        && h.path != "other.md"));
    let code = KindFilter::parse("code").unwrap();
    assert!(w
        .search_tagged("quokka", Some(&code), Some("project"), None)
        .is_empty());
}

#[test]
fn tag_filter_normalizes_and_rejects_empty() {
    assert_eq!(tag_filter(" #Project/ "), Some("project".into()));
    assert_eq!(tag_filter("##deep"), Some("deep".into()));
    assert_eq!(tag_filter("# deep"), Some("deep".into()));
    assert_eq!(tag_filter("deep//"), Some("deep".into()));
    assert_eq!(tag_filter("#"), None);
    assert_eq!(tag_filter(""), None);
}

#[test]
fn punctuation_query_with_tag_is_the_listing() {
    let (_d, w) = build_tag_corpus();
    assert_eq!(
        w.search_tagged("!!!", None, Some("project"), None).len(),
        13
    );
    assert!(w.search("!!!", None, 10).is_empty());
}

/// consumer.rs imports text.rs; guide.md only mentions it.
fn import_fixture() -> (tempfile::TempDir, Wiki) {
    common::compiled_wiki(&[
        (
            "src/text.rs",
            "//! Text rendering.\n\n/// Renders a page.\npub fn render() {}\n",
        ),
        (
            "src/consumer.rs",
            "//! Uses text.\nuse crate::text::render;\n\npub fn run() {\n    render();\n}\n",
        ),
        (
            "guide.md",
            "# Guide\n\nSee `text.rs` for how pages render.\n",
        ),
    ])
}

#[test]
fn a_code_target_is_its_outline_with_its_imports_as_outlines() {
    let (_dir, w) = import_fixture();
    let pack = w.neighbors("consumer", 1, &PackBudget::default()).unwrap();
    assert!(
        pack.text.starts_with("# Consumer (consumer)\n"),
        "{}",
        pack.text
    );
    assert!(!pack.text.contains("## Body"), "{}", pack.text);
    assert!(pack.text.contains("# Text (text)\n"), "{}", pack.text);
    assert!(pack.text.contains("fn render()"), "{}", pack.text);
}

#[test]
fn neighbors_kind_keeps_the_pack_to_that_kind() {
    let (_dir, w) = import_fixture();
    let mut all = w
        .neighbors("text", 1, &PackBudget::default())
        .unwrap()
        .included;
    all[1..].sort();
    assert_eq!(all, ["text", "consumer", "guide"]);
    let code = PackBudget {
        kind: KindFilter::parse("code"),
        ..Default::default()
    };
    assert_eq!(
        w.neighbors("text", 1, &code).unwrap().included,
        ["text", "consumer"]
    );
    let full = PackBudget {
        full_neighbors: true,
        ..Default::default()
    };
    let text = w.neighbors("text", 1, &full).unwrap().text;
    assert!(text.contains("# Consumer (consumer)\n"), "{text}");
    assert!(text.contains("See `text.rs`"), "{text}");
}

const CALLS_TEXT_RS: &str = "pub fn render() {\n    helper();\n}\nfn helper() {}\npub struct Doc;\nimpl Doc {\n    pub fn new() -> Doc {\n        Doc\n    }\n    pub fn show(&self) {\n        self.paint();\n        render();\n    }\n    fn paint(&self) {}\n}\n";
const CALLS_MAIN_RS: &str = "use crate::text::{render, Doc};\nfn main() {\n    let d = Doc::new();\n    d.show();\n    std::mem::drop(1);\n}\n";

#[test]
fn calls_follow_every_level_grouped_by_file() {
    let (_dir, w) = code_fixture(&[
        ("src/text.rs", CALLS_TEXT_RS),
        ("src/main.rs", CALLS_MAIN_RS),
        ("src/tool.py", "def run():\n    render()\n"),
    ]);
    // `Doc::new` by type, `d.show` as a method the file sees, `self.paint`
    // by owner, `render` and `helper` as free functions; `std::mem::drop`
    // is outside the project.
    assert_eq!(
        w.calls("main", "main", None, None).unwrap(),
        "calls from main  src/main.rs:2-6 — 5 functions in 1 files\nsrc/text.rs\n  1-3  fn  render\n  4-4  fn  helper\n  7-9  method  Doc::new\n  10-13  method  Doc::show\n  14-14  method  Doc::paint\n"
    );
    assert_eq!(
        w.calls("main", "main", Some(1), None).unwrap(),
        "calls from main  src/main.rs:2-6 — 2 functions in 1 files\nsrc/text.rs\n  7-9  method  Doc::new\n  10-13  method  Doc::show\n"
    );
    assert_eq!(
        w.calls("text", "Doc::show", None, None).unwrap(),
        "calls from Doc::show  src/text.rs:10-13 — 3 functions in 1 files\nsrc/text.rs\n  1-3  fn  render\n  4-4  fn  helper\n  14-14  method  Doc::paint\n"
    );
    // A Python call never reaches a Rust function of the same name.
    assert_eq!(
        w.calls("tool", "run", None, None).unwrap(),
        "calls from run  src/tool.py:1-2 — 0 functions in 0 files\n"
    );
    assert_eq!(
        w.calls("text", "nope", None, None).unwrap_err(),
        "no function \"nope\" on page text; its functions: Doc::new, Doc::paint, Doc::show, helper, render"
    );
}

#[test]
fn calls_cut_to_the_budget_say_how_to_see_all() {
    let (_dir, w) = code_fixture(&[
        ("src/text.rs", CALLS_TEXT_RS),
        ("src/main.rs", CALLS_MAIN_RS),
    ]);
    let full = w.calls("main", "main", None, None).unwrap();
    let cut = w.calls("main", "main", None, Some(30)).unwrap();
    assert!(tok(&cut) <= 30, "{cut}");
    let marker = format!("— pass max_tokens: {} for all\n", tok(&full));
    assert!(cut.ends_with(&marker), "{cut}");
    assert_eq!(
        w.calls("main", "main", None, Some(tok(&full))).unwrap(),
        full
    );
}

/// The `path:start-end` of what function `name` on page `id` calls.
fn callees(w: &Wiki, id: &str, name: &str) -> Vec<String> {
    let it = w.items(id).iter().find(|i| i.name == name);
    it.expect("the function").calls.clone()
}

#[test]
fn a_call_whose_argument_count_cannot_fit_is_no_match() {
    let (_dir, w) = code_fixture(&[
        (
            "src/m.rs",
            "pub struct M;\nimpl M {\n    pub fn map(&self, k: u8, v: u8) {}\n    pub fn get(&self, k: u8) {}\n}\npub fn user(v: Vec<u8>, m: M) {\n    v.iter().map(|x| x);\n    m.get(1);\n}\npub fn by_path(m: M) {\n    M::get(&m, 2);\n}\n",
        ),
        (
            "src/p.py",
            "import functools\n\ndef two(a, b=0):\n    pass\n\ndef star(*xs: int):\n    pass\n\n@functools.cache\ndef memo(a):\n    pass\n\nclass K:\n    def m(self, a):\n        pass\n\n    def u(self):\n        self.m(1)\n\ndef ok():\n    two(1)\n\ndef bad():\n    two(1, 2, 3)\n\ndef splat():\n    star(1, 2, 3, 4)\n    memo(1, 2)\n\ndef unbound(k):\n    K.m(k, 2)\n",
        ),
        (
            "src/c.cpp",
            "void f(int a, int b = 0) {}\nvoid g() { f(); }\nvoid h() { f(1, 2, 3); }\n",
        ),
        (
            "src/a.go",
            "package a\n\nfunc Two(a, b int) {}\nfunc V(xs ...int) {}\nfunc A() { Two(1, 2) }\nfunc B() { Two(1) }\nfunc C() { V(1, 2, 3) }\n",
        ),
    ]);
    // `.map(|x| x)` passes one argument to a two-parameter `map`; a
    // `Type::f` call may pass the receiver.
    assert_eq!(callees(&w, "m", "user"), ["src/m.rs:4-4"]);
    assert_eq!(callees(&w, "m", "by_path"), ["src/m.rs:4-4"]);
    assert_eq!(callees(&w, "p", "ok"), ["src/p.py:3-4"]);
    assert_eq!(callees(&w, "p", "bad"), [] as [String; 0]);
    // A star parameter takes any count; a decorator may change the signature.
    assert_eq!(callees(&w, "p", "splat"), ["src/p.py:6-7", "src/p.py:9-11"]);
    assert_eq!(callees(&w, "p", "u"), ["src/p.py:14-15"]);
    assert_eq!(callees(&w, "p", "unbound"), ["src/p.py:14-15"]);
    // C++ sets no minimum: a default may sit on another declaration.
    assert_eq!(callees(&w, "c", "g"), ["src/c.cpp:1-1"]);
    assert_eq!(callees(&w, "c", "h"), [] as [String; 0]);
    assert_eq!(callees(&w, "a", "A"), ["src/a.go:3-3"]);
    assert_eq!(callees(&w, "a", "B"), [] as [String; 0]);
    assert_eq!(callees(&w, "a", "C"), ["src/a.go:4-4"]);
}

const CALLERS_LIB_RS: &str = "pub fn target() {}\npub fn a() {\n    target();\n}\npub fn b() {\n    target();\n}\npub fn c() {\n    target();\n}\npub fn d() {\n    target();\n    a();\n}\n#[test]\nfn checks() {\n    target();\n}\n";

#[test]
fn callers_list_one_level_by_default_with_tests_last() {
    let (_dir, w) = code_fixture(&[("src/lib.rs", CALLERS_LIB_RS)]);
    assert_eq!(
        w.callers("lib", "target", Some(1), None).unwrap(),
        "callers of target  src/lib.rs:1-1 — 5 functions in 1 files\nsrc/lib.rs\n  2-4  fn  a  calls target at 3\n  5-7  fn  b  calls target at 6\n  8-10  fn  c  calls target at 9\n  11-14  fn  d  calls target at 12\n  15-18  fn [test]  checks  calls target at 17\n"
    );
    // With no depth limit the walk ends where nothing calls: `d`.
    assert_eq!(
        w.callers("lib", "a", None, None).unwrap(),
        "callers of a  src/lib.rs:2-4 — 1 functions in 1 files\nsrc/lib.rs\n  11-14  fn  d  calls a at 13\n"
    );
}

const CALLBACK_TS: &str = "export function renderCartLine(x: number): string {\n  return String(x);\n}\nexport function renderCart(xs: number[]): string {\n  return xs.map(renderCartLine).join(\",\");\n}\n";

#[test]
fn empty_callers_say_what_is_not_seen() {
    const CLAUSE: &str = " (not seen: a call outside any function, a function passed as a value or called by a computed name, or a call from another language or an unindexed file; grep for ";
    // The b3 case: a function passed as a value has no seen callers.
    let (_d, w) = code_fixture(&[("src/cart.ts", CALLBACK_TS)]);
    assert_eq!(
        w.callers("cart", "renderCartLine", Some(1), None).unwrap(),
        format!("callers of renderCartLine  src/cart.ts:1-3 — 0 functions in 0 files{CLAUSE}renderCartLine)\n")
    );
    // A call at module level, outside any function, has no caller to list.
    let (_t, w) = code_fixture(&[("src/app.ts", "export function init(): void {}\ninit();\n")]);
    assert_eq!(
        w.callers("app", "init", Some(1), None).unwrap(),
        format!("callers of init  src/app.ts:1-1 — 0 functions in 0 files{CLAUSE}init)\n")
    );
    let (_dir, w) = code_fixture(&[("src/lib.rs", CALLERS_LIB_RS)]);
    // A true zero: nothing calls the test `checks`. A small budget keeps the line whole.
    let empty =
        format!("callers of checks  src/lib.rs:15-18 — 0 functions in 0 files{CLAUSE}checks)\n");
    assert_eq!(w.callers("lib", "checks", Some(1), None).unwrap(), empty);
    assert_eq!(w.callers("lib", "checks", Some(1), Some(5)).unwrap(), empty);
    // Regression guards (pass before the change): depth 0 and a budget cut get no clause.
    assert!(!w
        .callers("lib", "target", Some(0), None)
        .unwrap()
        .contains("not seen"));
    let cut = w.callers("lib", "target", Some(1), Some(1)).unwrap();
    assert!(cut.contains("more functions — pass max_tokens"), "{cut}");
    assert!(!cut.contains("not seen"), "{cut}");
}

#[test]
fn a_header_declaration_stands_for_its_definition() {
    let (_dir, w) = code_fixture(&[
        (
            "src/a.h",
            "class Base {\npublic:\n  void log() {}\n};\nclass A : public Base {\npublic:\n  void run();\n  void step();\n};\n",
        ),
        (
            "src/a.cpp",
            "#include \"a.h\"\nvoid A::run() { step(); log(); }\nvoid A::step() {}\n",
        ),
        (
            "src/main.cpp",
            "#include \"a.h\"\nint main() {\n  A a;\n  a.run();\n}\n",
        ),
    ]);
    // `a` is also a module name; with no `a::run` it is read as a value.
    // `log()` is bare, as a base class's method is called in C++.
    assert_eq!(
        w.calls("main", "main", None, None).unwrap(),
        "calls from main  src/main.cpp:2-5 — 3 functions in 2 files\nsrc/a.cpp\n  2-2  method  A::run\n  3-3  method  A::step\nsrc/a.h\n  3-3  method  Base::log\n"
    );
    // Asked on the header, the walk starts from the definition too.
    let header = w.resolve_id("src/a.h").unwrap();
    assert_eq!(
        w.calls(&header, "A::run", None, None).unwrap(),
        "calls from A::run  src/a.h:7-7, src/a.cpp:2-2 — 2 functions in 2 files\nsrc/a.cpp\n  3-3  method  A::step\nsrc/a.h\n  3-3  method  Base::log\n"
    );
}

#[test]
fn index_json_without_updated_still_loads() {
    let dir = build();
    let path = dir.path().join("out").join("index.json");
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert!(v["entries"][0].get("updated").is_none());
    let w = Wiki::load(&dir.path().join("out")).unwrap();
    assert!(w
        .search("beta", None, 10)
        .iter()
        .all(|h| h.updated.is_none()));
}

#[test]
fn search_lines_shows_date_after_path() {
    use wiki::query::Hit;
    let hit = |updated: Option<&str>| Hit {
        id: "a".into(),
        title: "Title".into(),
        path: "a.md".into(),
        summary: Some("sum".into()),
        updated: updated.map(String::from),
        score: 1.0,
        snippet: None,
        symbols: vec![],
        memory: false,
        origin: wiki::model::Origin::Local,
        superseded_by: None,
    };
    assert_eq!(
        wiki::serve::search_lines(&[hit(Some("2026-10-23"))]),
        "a  a.md  2026-10-23  Title — sum\n"
    );
    assert_eq!(
        wiki::serve::search_lines(&[hit(None)]),
        "a  a.md  Title — sum\n"
    );
    let j: serde_json::Value = serde_json::from_str(&wiki::serve::search_json(vec![
        hit(Some("2026-10-23")),
        hit(None),
    ]))
    .unwrap();
    assert_eq!(j[0]["updated"], "2026-10-23");
    assert!(j[1].get("updated").is_none());
}

#[test]
fn title_resolves_case_insensitively_in_any_script() {
    let (_dir, _, output) =
        common::compiled(&[("note.md", "---\ntitle: Морський Маяк\n---\n\nBody.\n")]);
    let wiki = Wiki::load(&output).unwrap();
    assert_eq!(wiki.resolve_id("морський маяк").unwrap(), "morskyi_maiak");
    assert_eq!(wiki.resolve_id("МОРСЬКИЙ МАЯК").unwrap(), "morskyi_maiak");
}

#[test]
fn cyrillic_query_words_match_another_grammatical_form() {
    let (_dir, w) = common::compiled_wiki(&[
        ("a.md", "# Плани агента\n\nНотатка.\n"),
        ("b.md", "# Плани агентів\n\nНотатка.\n"),
        ("c.md", "# Ринок\n\nОгляд конкурентів.\n"),
        ("d.md", "# Шлюз\n\nПідключає mcp-сервер.\n"),
    ]);
    // The whole-title bonus compares the words as typed.
    assert_eq!(w.search("плани агентів", None, 10)[0].id, "plany_ahentiv");
    assert!(w
        .search("конкуренти", None, 10)
        .iter()
        .any(|h| h.id == "rynok"));
    assert!(w
        .search("mcp-сервери", None, 10)
        .iter()
        .any(|h| h.id == "shliuz"));
}

#[test]
fn an_english_word_form_finds_the_page_and_the_exact_form_still_wins() {
    let (_dir, w) = common::compiled_wiki(&[
        (
            "provision.md",
            "# Brew Provision

Arms a brew run.
",
        ),
        (
            "sorted.md",
            "# Sorted Walk

The tree, in order.
",
        ),
        (
            "sorting.md",
            "# Sorting Graph

Ranks by sorting.
",
        ),
        (
            "other.md",
            "# Other

Nothing here.
",
        ),
    ]);
    // `provisioning` shares its stem with `provision`; no prefix connects them.
    assert!(w
        .search("provisioning", None, 10)
        .iter()
        .any(|h| h.id == "brew_provision"));
    // `sorted` says exactly what one page says; the stem-only page follows.
    let ids: Vec<_> = w
        .search("sorted", None, 10)
        .into_iter()
        .map(|h| h.id)
        .collect();
    assert_eq!(ids, ["sorted_walk", "sorting_graph"]);
}

#[test]
fn a_ukrainian_word_form_finds_the_page() {
    let (_dir, w) = common::compiled_wiki(&[
        (
            "a.md",
            "# Сторінка

Нотатка.
",
        ),
        (
            "b.md",
            "# Ринок

Огляд.
",
        ),
    ]);
    // Genitive plural: the stem joins it, no prefix does.
    assert!(w
        .search("сторінок", None, 10)
        .iter()
        .any(|h| h.id == "storinka"));
}

/// `(id, score, symbol lines)` of the non-memory hits, for exact comparison.
fn code_part(hits: &[wiki::query::Hit]) -> Vec<CodeHitKey> {
    hits.iter()
        .filter(|h| !h.memory)
        .map(|h| (h.id.clone(), h.score, symbol_lines(std::slice::from_ref(h))))
        .collect()
}

type CodeHitKey = (String, f64, Vec<(String, usize, usize, String)>);

#[test]
fn code_hits_are_identical_with_and_without_notes() {
    let code = "//! zebra\npub fn zebra() { /* zebra zebra */ }\n";
    let body = "---\ntitle: Note\n---\n\nOnce a zebra, then filler words follow.\n";
    // As a project doc the note is in the index: it is a code-part hit.
    let (_d, doc) = code_fixture(&[("a.rs", code), ("note.md", body)]);
    assert!(doc
        .search("zebra", None, 10)
        .iter()
        .any(|h| h.id == "note" && !h.memory));
    let (_d, with) = code_fixture(&[("a.rs", code), (".agents/memory/note.md", body)]);
    let (_d, without) = code_fixture(&[("a.rs", code)]);
    assert_eq!(
        code_part(&with.search("zebra", None, 10)),
        code_part(&without.search("zebra", None, 10))
    );
}

/// A code page and three notes whose titles say `elephant`: two dated with
/// equal text, one undated.
fn elephant_fixture() -> (tempfile::TempDir, Wiki) {
    code_fixture(&[
        ("a.rs", "//! elephant\npub fn elephant() {}\n"),
        (
            ".agents/memory/a.md",
            &note("Elephant Aaa", "updated: 2026-01-01\n", "x"),
        ),
        (
            ".agents/memory/b.md",
            &note("Elephant Bbb", "updated: 2026-03-01\n", "x"),
        ),
        (".agents/memory/c.md", &note("Elephant Ccc", "", "x")),
    ])
}

#[test]
fn notes_come_after_code_as_a_tail_of_at_most_two() {
    let (_d, w) = elephant_fixture();
    let hits = w.search("elephant", None, 10);
    let rows: Vec<_> = hits.iter().map(|h| (h.id.as_str(), h.memory)).collect();
    // Equal scores: the newer `updated` first, the undated note cut.
    assert_eq!(
        rows,
        [("a", false), ("elephant_bbb", true), ("elephant_aaa", true)]
    );
}

#[test]
fn tail_is_independent_of_kind_and_limit() {
    let (_d, w) = elephant_fixture();
    let code = KindFilter::parse("code").unwrap();
    let ids: Vec<_> = w
        .search("elephant", Some(&code), 1)
        .into_iter()
        .map(|h| h.id)
        .collect();
    assert_eq!(ids, ["a", "elephant_bbb", "elephant_aaa"]);
}

/// Loud code (`zebra` five times) and four notes: `rotation` only in the
/// title, an alias, the summary, or the body.
fn rotation_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    common::compiled(&[
        (
            "src/lib.rs",
            "/// zebra zebra zebra zebra zebra\npub fn zebra_loud() {}\n",
        ),
        (".agents/memory/t.md", &note("Rotation plan", "", "x")),
        (
            ".agents/memory/a.md",
            &note("Alpha", "aliases: [rotation]\n", "x"),
        ),
        (
            ".agents/memory/s.md",
            &note("Sigma", "", "Rotation weekly.\n\nfiller"),
        ),
        (
            ".agents/memory/b.md",
            &note("Bee", "", "Filler first.\n\nThe rotation happens here."),
        ),
    ])
}

#[test]
fn tail_gate_long_word_in_title_alias_or_summary() {
    let (_d, _raw, out) = rotation_fixture();
    let w = Wiki::load(&out).unwrap();
    let named = ["rotation_plan", "alpha", "sigma"];
    let tail: Vec<_> = w
        .search("rotation zebra", None, 10)
        .into_iter()
        .filter(|h| h.memory)
        .map(|h| h.id)
        .collect();
    assert_eq!(tail.len(), 2, "{tail:?}");
    assert!(
        tail.iter().all(|id| named.contains(&id.as_str())),
        "{tail:?}"
    );
    // A four-character word passes the default gate (4) and fails under
    // `WIKI_MEMORY_GATE=8`, read once per process, so through the binary.
    let rows = |gate: Option<&str>| {
        let mut cmd = assert_cmd::Command::cargo_bin("wiki").unwrap();
        cmd.args(["search", "rota zebra", "--dir"]).arg(&out);
        if let Some(g) = gate {
            cmd.env("WIKI_MEMORY_GATE", g);
        }
        let lines = cmd.assert().success();
        let lines = String::from_utf8(lines.get_output().stdout.clone()).unwrap();
        let tail: Vec<_> = lines
            .lines()
            .filter(|l| l.starts_with("memory ("))
            .map(str::to_owned)
            .collect();
        (lines, tail)
    };
    let (lines, tail) = rows(None);
    assert_eq!(tail.len(), 2, "{lines}");
    assert!(
        tail.iter()
            .all(|l| ["t.md", "a.md", "s.md"].iter().any(|f| l.contains(f))),
        "{lines}"
    );
    let (lines, tail) = rows(Some("8"));
    assert!(tail.is_empty(), "{lines}");
}

#[test]
fn tail_ratio_branch_needs_a_positive_note_score() {
    let (_d, w) = code_fixture(&[
        ("a.rs", "pub fn other() {}\n"),
        (
            ".agents/memory/b.md",
            &note("Bee", "", "Filler first.\n\nThe rotation happens here."),
        ),
    ]);
    let ids: Vec<_> = w
        .search("rotation", None, 10)
        .into_iter()
        .map(|h| h.id)
        .collect();
    assert_eq!(ids, ["bee"]);
    assert!(w.search("walrus", None, 10).is_empty());
}

#[test]
fn tail_scores_use_the_project_scale() {
    // Same page count; only the project page that also says `migration`
    // differs, so a notes-only scale would give the note the same score.
    let tail_score = |extra: &str| {
        let (_d, w) = code_fixture(&[
            ("a.rs", "pub fn other() {}\n"),
            ("b.rs", extra),
            (".agents/memory/m.md", &note("Migration plan", "", "x")),
        ]);
        let hits = w.search("migration", None, 10);
        hits.iter().find(|h| h.memory).expect("tail note").score
    };
    let alone = tail_score("pub fn unrelated() {}\n");
    let shared = tail_score("/// migration\npub fn migrate() {}\n");
    assert!(shared < alone, "{shared} vs {alone}");
}

#[test]
fn tail_ratio_branch_skips_a_note_the_code_outscores() {
    let (_d, w) = code_fixture(&[
        (
            "src/rotation.rs",
            "//! Rotation rotation rotation.\n/// Rotation of rotation keys.\npub fn rotation() {}\n",
        ),
        (
            ".agents/memory/b.md",
            &note("Bee", "", "Filler first.\n\nThe rotation happens here."),
        ),
    ]);
    let hits = w.search("rotation", None, 10);
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|h| !h.memory), "{:?}", ids_of(&hits));
}

fn ids_of(hits: &[wiki::query::Hit]) -> Vec<&str> {
    hits.iter().map(|h| h.id.as_str()).collect()
}

#[test]
fn newer_of_two_equal_memory_notes_is_first_and_dated_beats_undated() {
    let note =
        |title: &str, updated: &str| format!("---\ntitle: {title}\n{updated}---\n\nzebra rule.\n");
    let (_d, w) = code_fixture(&[
        (".agents/memory/a.md", &note("Aaa", "updated: 2026-01-01\n")),
        (".agents/memory/c.md", &note("Ccc", "")),
        (".agents/memory/b.md", &note("Bbb", "updated: 2026-03-01\n")),
    ]);
    let ids: Vec<_> = w
        .search("zebra", None, 10)
        .into_iter()
        .filter(|h| h.memory)
        .map(|h| h.id)
        .collect();
    // The undated note loses the tie to both and falls out of the tail of two.
    assert_eq!(ids, ["bbb", "aaa"]);
}

#[test]
fn memory_tail_ignores_kind_and_the_code_part_respects_it() {
    let (_d, w) = code_fixture(&[
        ("a.rs", "//! elephant\npub fn elephant() {}\n"),
        (
            ".agents/memory/note.md",
            "---\ntitle: Elephant note\n---\n\nelephant\n",
        ),
    ]);
    let ids = |f: &str| {
        let f = KindFilter::parse(f).unwrap();
        w.search("elephant", Some(&f), 10)
            .into_iter()
            .map(|h| h.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids("code"), ["a", "elephant_note"]);
    assert_eq!(ids("docs"), ["elephant_note"]);
    assert_eq!(ids("markdown"), ["elephant_note"]);
    assert_eq!(ids("code:rust"), ["a", "elephant_note"]);
}

#[test]
fn memory_flag_shows_in_json_only_on_memory_hits() {
    let (_d, w) = code_fixture(&[
        ("a.rs", "//! elephant\npub fn elephant() {}\n"),
        (
            ".agents/memory/note.md",
            "---\ntitle: Elephant note\n---\n\nelephant\n",
        ),
    ]);
    let j: serde_json::Value =
        serde_json::from_str(&wiki::serve::search_json(w.search("elephant", None, 10))).unwrap();
    assert!(j[0].get("memory").is_none());
    assert_eq!(j[1]["memory"], true);
}

#[test]
fn index_json_without_memory_still_loads() {
    let (d, _w) = code_fixture(&[("a.md", "# A\n\nzebra\n")]);
    let out = d.path().join("out");
    assert!(common::index(&out)["entries"][0].get("memory").is_none());
    let w = Wiki::load(&out).unwrap();
    assert!(w.search("zebra", None, 10).iter().all(|h| !h.memory));
}

fn note(title: &str, extra: &str, body: &str) -> String {
    format!("---\ntitle: {title}\n{extra}---\n\n{body}\n")
}

fn superseded_by(d: &tempfile::TempDir) -> std::collections::BTreeMap<String, Option<String>> {
    common::index_entries(&d.path().join("out"))
        .iter()
        .map(|e| {
            (
                e["id"].as_str().unwrap().to_string(),
                e.get("superseded_by")
                    .map(|s| s.as_str().unwrap().to_string()),
            )
        })
        .collect()
}

#[test]
fn supersedes_marks_a_memory_note_by_title_alias_or_slug_only() {
    let (d, _w) = code_fixture(&[
        (".agents/memory/t.md", &note("Old Title", "", "x")),
        (".agents/memory/a.md", &note("Aliased", "aliases: [Former Name]\n", "x")),
        (".agents/memory/s.md", &note("Slugged Note", "", "x")),
        ("proj.md", &note("Project Page", "", "x")),
        (
            ".agents/memory/new.md",
            &note(
                "New",
                "supersedes:\n  - old title\n  - FORMER NAME\n  - slugged_note\n  - Project Page\n  - Nothing Here\n",
                "x",
            ),
        ),
    ]);
    let m = superseded_by(&d);
    for id in ["old_title", "aliased", "slugged_note"] {
        assert_eq!(m[id].as_deref(), Some("new"), "{id}");
    }
    assert_eq!(m["project_page"], None);
    assert_eq!(m["new"], None);
}

#[test]
fn two_superseders_resolve_to_the_smaller_id() {
    let (d, _w) = code_fixture(&[
        (".agents/memory/o.md", &note("Old", "", "x")),
        (
            ".agents/memory/z.md",
            &note("Zed", "supersedes: Old\n", "x"),
        ),
        (
            ".agents/memory/b.md",
            &note("Bee", "supersedes: [Old]\n", "x"),
        ),
    ]);
    assert_eq!(superseded_by(&d)["old"].as_deref(), Some("bee"));
}

#[test]
fn superseded_note_never_enters_the_tail() {
    let (_d, w) = code_fixture(&[
        (".agents/memory/t.md", &note("Old Title", "", "rotation")),
        (
            ".agents/memory/a.md",
            &note("Aliased", "aliases: [Former Name]\n", "rotation"),
        ),
        (".agents/memory/s.md", &note("Slugged Note", "", "rotation")),
        ("proj.md", &note("Project Page", "", "x")),
        (
            ".agents/memory/new.md",
            &note(
                "New",
                "supersedes:\n  - old title\n  - FORMER NAME\n  - slugged_note\n",
                "rotation",
            ),
        ),
    ]);
    let ids: Vec<_> = w
        .search("rotation", None, 10)
        .into_iter()
        .map(|h| h.id)
        .collect();
    assert_eq!(ids, ["new"]);
}

#[test]
fn tag_listing_is_unchanged_by_the_tail() {
    let (_d, w) = code_fixture(&[
        ("proj.md", &note("Proj", "tags: [ops]\n", "zebra")),
        (
            ".agents/memory/old.md",
            &note("Old", "tags: [ops]\n", "zebra"),
        ),
        (
            ".agents/memory/new.md",
            &note("New", "supersedes: Old\n", "zebra"),
        ),
    ]);
    let hits = w.search_tagged("", None, Some("ops"), None);
    let rows: Vec<_> = hits
        .iter()
        .map(|h| (h.id.as_str(), h.superseded_by.as_deref()))
        .collect();
    assert_eq!(rows, [("old", Some("new")), ("proj", None)]);
    let lines = wiki::serve::search_lines(&hits);
    assert!(
        lines.lines().any(|l| l.ends_with(" (superseded by new)")),
        "{lines}"
    );
    let j: serde_json::Value = serde_json::from_str(&wiki::serve::search_json(hits)).unwrap();
    assert_eq!(j[0]["superseded_by"], "new");
    assert!(j[1].get("superseded_by").is_none());
}

fn journals() -> (tempfile::TempDir, Wiki) {
    // The day number appears nowhere in the note's text, only in its date.
    let j = |day: &str, name: &str| {
        (
            format!(".agents/memory/journal/2026-10-{day}.md"),
            format!("---\ntitle: Notebook {name}\n---\n\nstand run notes\n"),
        )
    };
    let files = [j("05", "alpha"), j("12", "beta"), j("19", "gamma")];
    let mut refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    refs.push(("src/stand.rs", "pub fn stand() {}\n"));
    code_fixture(&refs)
}

#[test]
fn a_date_in_the_query_ranks_the_note_of_that_day_first() {
    let (_d, w) = journals();
    for q in [
        "що сталося зі stand 12 жовтня",
        "stand October 12",
        "stand 12.10",
        "stand 2026-10-12",
    ] {
        let hits = w.search(q, None, 10);
        let first_note = hits.iter().find(|h| h.memory).unwrap();
        assert_eq!(first_note.updated.as_deref(), Some("2026-10-12"), "{q}");
    }
    // No date: the three journals tie, newest first.
    let hits = w.search("notebook stand", None, 10);
    let first_note = hits.iter().find(|h| h.memory).unwrap();
    assert_eq!(first_note.updated.as_deref(), Some("2026-10-19"));
}

#[test]
fn a_date_without_a_year_hits_every_year_newer_first() {
    let note = |u: &str| format!("---\ntitle: N{u}\nupdated: {u}\n---\n\nstand\n");
    let (a, b) = (note("2025-10-12"), note("2026-10-12"));
    let (_d, w) = code_fixture(&[
        (".agents/memory/a.md", &a),
        (".agents/memory/b.md", &b),
        (".agents/memory/c.md", &note("2026-10-13")),
    ]);
    let hits = w.search("12 жовтня", None, 10);
    let ups: Vec<_> = hits.iter().map(|h| h.updated.clone().unwrap()).collect();
    assert_eq!(ups, ["2026-10-12", "2025-10-12"]);
    let hits = w.search("12 жовтня 2025", None, 10);
    assert_eq!(hits[0].updated.as_deref(), Some("2025-10-12"));
}

#[test]
fn a_project_page_with_a_date_in_its_name_is_not_date_matched() {
    let (_d, w) = code_fixture(&[(
        "docs/2026-10-12-spec.md",
        "---\ntitle: Spec\nupdated: 2026-10-12\n---\n\nplain\n",
    )]);
    assert!(w.search("12 жовтня", None, 10).is_empty());
}

#[test]
fn a_date_lifts_only_the_notes_it_hits_and_a_missed_date_changes_nothing() {
    let note = |name: &str, u: &str| {
        (
            format!(".agents/memory/{name}.md"),
            format!("---\ntitle: {name}\nupdated: {u}\n---\n\nstand run\n"),
        )
    };
    let files = [
        note("alpha", "2026-10-12"),
        note("beta", "2026-10-13"),
        note("gamma", "2026-10-14"),
    ];
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let (_d, w) = code_fixture(&refs);
    let scores = |q: &str| -> Vec<(String, f64)> {
        w.search(q, None, 10)
            .into_iter()
            .map(|h| (h.id, h.score))
            .collect()
    };
    let hit = scores("stand 12 жовтня");
    assert_eq!(hit[0].0, "alpha");
    assert!(hit[0].1 > hit[1].1);
    // Beta and gamma tie unlifted; the newer is the second of the tail of two.
    assert_eq!(hit.len(), 2);
    assert_eq!(hit[1].0, "gamma");
    // Dated 25 October: nothing; same scores as the non-date "250".
    assert_eq!(scores("stand 25 жовтня"), scores("stand 250 жовтня"));
}

#[test]
fn tail_keeps_the_newest_of_many_equal_notes() {
    // Twelve journals with the same text: the smallest ids are the oldest.
    let files: Vec<(String, String)> = (1..=12)
        .map(|d| {
            (
                format!(".agents/memory/day{d:02}.md"),
                format!("---\ntitle: Day {d:02}\nupdated: 2026-10-{d:02}\n---\n\nstand run\n"),
            )
        })
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let (_d, w) = code_fixture(&refs);
    let ups: Vec<_> = w
        .search("stand", None, 10)
        .into_iter()
        .map(|h| h.updated.unwrap())
        .collect();
    assert_eq!(ups, ["2026-10-12", "2026-10-11"]);
}

#[test]
fn a_memory_note_earns_nothing_for_its_file_name() {
    let (_d, w) = code_fixture(&[
        ("a.rs", "pub fn plain() {}\n"),
        (
            ".agents/memory/elephant.md",
            &note("Elephant herd", "", "x"),
        ),
    ]);
    let score = |q: &str| {
        w.search(q, None, 10)
            .iter()
            .find(|h| h.memory)
            .map(|h| h.score)
    };
    assert_eq!(score("elephant"), score("elephant elephant.md"));
}

/// A distinct email per author name.
fn email_of(name: &str) -> String {
    format!("{}@t", name.replace(' ', "."))
}

/// Run git in `dir` as author `name` (with `email_of(name)`), unsigned.
fn git_as(dir: &std::path::Path, name: &str, args: &[&str]) {
    let s = std::process::Command::new("git")
        .args(["-c", &format!("user.name={name}")])
        .args(["-c", &format!("user.email={}", email_of(name))])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(s.status.success(), "git {args:?}");
}

#[test]
fn compile_stores_note_origin_in_index() {
    let d = tempdir().unwrap();
    let root = d.path();
    let note = |t: &str| format!("---\ntitle: {t}\n---\n\nzebra\n");
    common::write(root, "proj/a.rs", "pub fn a() {}\n");
    common::write(root, "proj/.agents/memory/kept.md", &note("Kept"));
    common::write(root, "agent/mine.md", &note("Mine"));
    git_as(root, "t", &["init", "-q"]);
    git_as(root, "t", &["config", "user.email", &email_of("Jane Doe")]);
    git_as(root, "Jane Doe", &["add", "."]);
    git_as(root, "Jane Doe", &["commit", "-qm", "x"]);
    common::write(root, "proj/.agents/memory/fresh.md", &note("Fresh"));
    let out = root.join("out");
    let opts = CompileOptions {
        memory: vec![root.join("agent")],
        ..Default::default()
    };
    compile(&root.join("proj"), &out, &opts).unwrap();
    let raw = std::fs::read_to_string(out.join("index.json")).unwrap();
    assert!(!raw.contains(&email_of("Jane Doe")), "email in index.json");
    let entries = common::index_entries(&out);
    let by_id = |id: &str| {
        entries
            .iter()
            .find(|e| e["id"] == id)
            .unwrap_or_else(|| panic!("no {id}"))
            .clone()
    };
    let keys = |id: &str| {
        let e = by_id(id);
        ["origin", "author", "edited", "mine"].map(|k| e.get(k).cloned())
    };
    assert_eq!(
        keys("kept"),
        [
            Some(serde_json::json!("repo")),
            Some(serde_json::json!("Jane Doe")),
            None,
            Some(serde_json::json!(true))
        ]
    );
    assert_eq!(
        keys("fresh"),
        [Some(serde_json::json!("local")), None, None, None]
    );
    assert_eq!(
        keys("mine"),
        [Some(serde_json::json!("local")), None, None, None]
    );
    assert_eq!(keys("a"), [None, None, None, None]);
}

#[test]
fn old_index_guesses_origin_from_path() {
    use wiki::model::Origin;
    let d = tempdir().unwrap();
    let (raw, agent, out) = (
        d.path().join("raw"),
        d.path().join("agent"),
        d.path().join("out"),
    );
    common::write(
        &raw,
        ".agents/memory/x.md",
        "---\ntitle: Repo note\n---\n\nzebra\n",
    );
    common::write(&agent, "y.md", "---\ntitle: Agent note\n---\n\nzebra\n");
    let opts = CompileOptions {
        memory: vec![agent],
        ..Default::default()
    };
    compile(&raw, &out, &opts).unwrap();
    let path = out.join("index.json");
    let mut index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    for e in index["entries"].as_array_mut().unwrap() {
        let e = e.as_object_mut().unwrap();
        for k in ["origin", "author", "edited", "mine"] {
            e.remove(k);
        }
    }
    fs::write(&path, index.to_string()).unwrap();
    let hits = Wiki::load(&out).unwrap().search("zebra", None, 10);
    let origin = |id: &str| hits.iter().find(|h| h.id == id).unwrap().origin.clone();
    assert_eq!(
        origin("repo_note"),
        Origin::Repo {
            author: None,
            edited: false,
            mine: false
        }
    );
    assert_eq!(origin("agent_note"), Origin::Local);
}

fn origin_fixture(
    committed: &[(&str, String)],
    local: &[(&str, String)],
) -> (tempfile::TempDir, Wiki) {
    origin_fixture_by("Jane Doe", committed, local)
}

/// A temp git repo: `committed` `.agents/memory/<name>` notes committed by
/// `author` (the repo's own `user.email` is Jane Doe's), then `local`
/// agent-folder notes. Compiled into `<tmp>/out`; returns the loaded wiki.
fn origin_fixture_by(
    author: &str,
    committed: &[(&str, String)],
    local: &[(&str, String)],
) -> (tempfile::TempDir, Wiki) {
    let d = tempdir().unwrap();
    let root = d.path();
    common::write(root, "proj/a.rs", "pub fn a() {}\n");
    for (n, t) in committed {
        common::write(root, &format!("proj/.agents/memory/{n}.md"), t);
    }
    git_as(root, "t", &["init", "-q"]);
    git_as(root, "t", &["config", "user.email", &email_of("Jane Doe")]);
    git_as(root, author, &["add", "."]);
    git_as(root, author, &["commit", "-qm", "x"]);
    for (n, t) in local {
        common::write(root, &format!("agent/{n}.md"), t);
    }
    let opts = CompileOptions {
        memory: vec![root.join("agent")],
        ..Default::default()
    };
    compile(&root.join("proj"), &root.join("out"), &opts).unwrap();
    let w = Wiki::load(&root.join("out")).unwrap();
    (d, w)
}

/// A local note and a co-author's committed note on the same word; the
/// committed one scores higher (the word is also its title).
fn trust_fixture() -> (tempfile::TempDir, Wiki) {
    origin_fixture_by(
        "Mallory Example",
        &[("theirs", note("Rotation", "", "rotation rotation rotation"))],
        &[("ours", note("Keys", "", "rotation"))],
    )
}

#[test]
fn tail_orders_notes_by_trust() {
    let (_d, w) = trust_fixture();
    let notes: Vec<_> = w
        .search("rotation", None, 10)
        .into_iter()
        .filter(|h| h.memory)
        .collect();
    let ids: Vec<_> = notes.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids, ["keys", "rotation"]);
    assert!(
        notes[1].score > notes[0].score,
        "Mallory's note must score higher"
    );
}

#[test]
fn hint_picks_the_most_trusted_note() {
    let (_d, w) = trust_fixture();
    let hint = wiki::hint::hint_lines(&w, "check the rotation schedule before we ship");
    assert!(hint.contains("memory (local):"), "{hint}");
    assert!(hint.contains("ours.md"), "{hint}");
    assert!(!hint.contains("theirs.md"), "{hint}");
    // The committed note passes the gate on its own, so the pick above is
    // a ranking, not a gate failure.
    let (_d, w) = origin_fixture_by(
        "Mallory Example",
        &[("theirs", note("Rotation", "", "rotation rotation rotation"))],
        &[],
    );
    let hint = wiki::hint::hint_lines(&w, "check the rotation schedule before we ship");
    assert!(hint.contains("theirs.md"), "{hint}");
}

#[test]
fn search_output_never_shows_the_author_email() {
    let (_d, w) = trust_fixture();
    let search = || w.search("rotation", None, 10);
    let json = wiki::serve::search_json(search());
    assert!(json.contains("Mallory Example"), "{json}");
    for out in [json, wiki::serve::search_lines(&search())] {
        assert!(!out.contains(&email_of("Mallory Example")), "{out}");
    }
}

#[test]
fn committed_note_cannot_supersede_a_local_note() {
    let (d, w) = origin_fixture(
        &[("x", note("New", "supersedes: Deploy key\n", "rotation"))],
        &[("deploy", note("Deploy key", "", "rotation"))],
    );
    assert_eq!(superseded_by(&d)["deploy_key"], None);
    let ids: Vec<_> = w
        .search("rotation", None, 10)
        .into_iter()
        .map(|h| h.id)
        .collect();
    assert!(ids.contains(&"deploy_key".to_string()), "{ids:?}");
    let hint = wiki::hint::hint_lines(&w, "rotation of the Deploy key needs checking");
    assert!(hint.contains("deploy"), "{hint}");
}

#[test]
fn local_note_still_supersedes_a_committed_one() {
    let (d, _w) = origin_fixture(
        &[("old", note("Old", "", "rotation"))],
        &[("new", note("New", "supersedes: Old\n", "rotation"))],
    );
    assert_eq!(superseded_by(&d)["old"].as_deref(), Some("new"));
}

#[test]
fn committed_note_still_supersedes_a_committed_one() {
    let (d, _w) = origin_fixture(
        &[
            ("old", note("Old", "", "rotation")),
            ("new", note("New", "supersedes: Old\n", "rotation")),
        ],
        &[],
    );
    assert_eq!(superseded_by(&d)["old"].as_deref(), Some("new"));
}

#[test]
fn callers_reach_a_pointer_returning_definition() {
    let (_d, w) = code_fixture(&[
        (
            "src/router.h",
            "class Router {\npublic:\n  const Venue* pick_venue(int id);\n  int cost(int id);\n};\n",
        ),
        (
            "src/router.cpp",
            "#include \"router.h\"\nconst Venue* Router::pick_venue(int id) { return nullptr; }\nint Router::cost(int id) {\n  pick_venue(id);\n  return 0;\n}\n",
        ),
    ]);
    let id = w.resolve("src/router.cpp").unwrap();
    let out = w
        .callers(&id, "pick_venue", Some(1), None)
        .unwrap_or_else(|e| e);
    assert!(out.contains("cost  calls pick_venue at 4"), "{out}");
}

#[test]
fn outline_header_and_neighbors_cap_and_clean_the_summary() {
    let para = format!(
        "zebrafish \x1b[1mbold\x1b[0m bell\x07 {}",
        "word ".repeat(240)
    );
    let (_d, w) = code_fixture(&[
        ("docs/long.md", &format!("# Long\n\n{para}\n")),
        ("docs/hub.md", "# Hub\n\nHub links [L](long.md).\n"),
    ]);
    let clean = |t: &str| !t.contains('\x1b') && !t.contains('\x07');
    let outline = w.outline("long", None).unwrap();
    let third = outline.lines().nth(2).unwrap();
    assert_eq!(third.chars().count(), 1000, "{third}");
    assert!(clean(&outline));

    let pack = w.neighbors("hub", 1, &PackBudget::default()).unwrap();
    assert!(pack.text.contains("## Long (long)"), "{}", pack.text);
    assert!(clean(&pack.text));
    assert!(pack.text.lines().all(|l| l.chars().count() <= 1000));

    let small = PackBudget {
        max_tokens: Some(100),
        ..Default::default()
    };
    let pack = w.neighbors("long", 1, &small).unwrap();
    assert!(pack.text.contains(OVER_BUDGET_NOTE), "{}", pack.text);
    assert!(clean(&pack.text));
    assert!(pack.text.lines().all(|l| l.chars().count() <= 1000));

    let lines = wiki::serve::search_lines(&w.search("zebrafish", None, 10));
    assert!(clean(&lines), "{lines}");
}

fn twin_note(files: &[(&str, &str)]) -> String {
    let (_d, w) = code_fixture(files);
    wiki::serve::search_lines(&w.search("slice_cap_for", None, 1))
}

#[test]
fn search_notes_the_one_other_definition_of_a_name() {
    let out = twin_note(&[
        ("src/a.rs", "pub fn slice_cap_for() {}\n"),
        ("src/b.rs", "pub fn slice_cap_for() {}\n"),
    ]);
    assert!(
        out.contains("    also defined: src/b.rs:1-1  slice_cap_for\n"),
        "{out}"
    );
}

#[test]
fn search_stays_silent_when_a_name_has_many_copies() {
    let out = twin_note(&[
        ("src/a.rs", "pub fn slice_cap_for() {}\n"),
        ("src/b.rs", "pub fn slice_cap_for() {}\n"),
        ("src/c.rs", "pub fn slice_cap_for() {}\n"),
    ]);
    assert!(!out.contains("also defined"), "{out}");
}

#[test]
fn search_does_not_note_a_test_copy() {
    let out = twin_note(&[
        ("src/a.rs", "pub fn slice_cap_for() {}\n"),
        ("tests/b.rs", "fn slice_cap_for() {}\n"),
    ]);
    assert!(!out.contains("also defined"), "{out}");
}

/// Names of the symbol lines of a response, in output order.
fn line_names(hits: &[wiki::query::Hit]) -> Vec<String> {
    symbol_lines(hits).into_iter().map(|l| l.3).collect()
}

const HARVEST: &str = "\
def _parse_qx_bundle(): pass
def _raw_artifact(): pass
def _artifact(): pass
def _ack(): pass
def _tidy_stop_with(): pass
def _stop_harvest(): pass
def _set_engine(): pass
";

#[test]
fn a_name_keeps_its_leading_underscore() {
    let (_d, w) = code_fixture(&[
        (
            "workbench/src/store/events.ts",
            "export function events() {}\n",
        ),
        ("scripts/state.py", "def _events():\n    pass\n"),
    ]);
    assert_eq!(line_names(&w.search("_events", None, 10)), ["_events"]);
}

#[test]
fn an_exact_name_beats_its_look_alikes() {
    let (_d, w) = code_fixture(&[("scripts/test_harvest_state.py", HARVEST)]);
    assert_eq!(line_names(&w.search("_artifact", None, 3)), ["_artifact"]);
}

#[test]
fn a_word_query_shows_a_named_definition_without_moving_the_ranking() {
    let (_d, w) = code_fixture(&[
        (
            "workbench/src/store/events.ts",
            "export function events() {}\n",
        ),
        ("scripts/state.py", "def _events():\n    pass\n"),
    ]);
    let names = line_names(&w.search("_events store", None, 10));
    assert_eq!(names[0], "events");
    assert!(names.contains(&"_events".to_string()), "{names:?}");
    let (_d, w) = code_fixture(&[(
        "scripts/state.py",
        "def _parse_raw_artifact(): pass\ndef _raw_artifact_parse(): pass\n\
         def _artifact_raw_parse(): pass\ndef _artifact(): pass\n",
    )]);
    let names = line_names(&w.search("_artifact parse raw", None, 3));
    assert!(names.contains(&"_artifact".to_string()), "{names:?}");
}

#[test]
fn a_query_of_names_gets_one_line_per_name() {
    let (_d, w) = code_fixture(&[("scripts/test_harvest_state.py", HARVEST)]);
    let q = "_tidy_stop_with _artifact _stop_harvest _set_engine _ack test_eh";
    assert_eq!(
        line_names(&w.search(q, None, 3)),
        [
            "_tidy_stop_with",
            "_artifact",
            "_stop_harvest",
            "_set_engine",
            "_ack"
        ]
    );
    assert_eq!(w.not_found(q, None, None), ["test_eh"]);
}

#[test]
fn a_word_query_reports_nothing_missing() {
    let (_d, w) = code_fixture(&[("scripts/test_harvest_state.py", HARVEST)]);
    assert_eq!(
        w.not_found("stop harvest", None, None),
        Vec::<String>::new()
    );
    assert_eq!(w.not_found("_ack stop", None, None), Vec::<String>::new());
}

#[test]
fn an_exact_case_match_is_preferred() {
    let (_d, w) = code_fixture(&[
        ("src/book.py", "class OrderBook:\n    pass\n"),
        ("src/util.py", "def orderbook():\n    pass\n"),
    ]);
    assert_eq!(line_names(&w.search("OrderBook", None, 10)), ["OrderBook"]);
    let any_case = line_names(&w.search("orderBook", None, 10));
    assert!(any_case.contains(&"OrderBook".to_string()), "{any_case:?}");
    assert!(any_case.contains(&"orderbook".to_string()), "{any_case:?}");
}

const NESTED_PY: &str = "\
def outer(x):
    def mk_run(y):
        return helper(y)
    class Local:
        def in_local(self):
            pass
    return mk_run(x)

def helper(y):
    return y
";

#[test]
fn a_nested_python_function_is_found_by_search() {
    let (_d, w) = code_fixture(&[("src/runs.py", NESTED_PY)]);
    assert_eq!(
        symbol_lines(&w.search("mk_run", None, 10)),
        [("src/runs.py".to_string(), 2, 3, "mk_run".to_string())]
    );
    // A method of a class defined inside a function stays out.
    assert_eq!(w.not_found("in_local", None, None), ["in_local"]);
}

#[test]
fn a_nested_python_function_stays_off_the_page() {
    let (_d, w) = code_fixture(&[("src/runs.py", NESTED_PY)]);
    let outline = w.outline("runs", None).unwrap();
    assert!(!outline.contains("mk_run"), "{outline}");
}

#[test]
fn a_nested_python_function_is_not_in_the_call_graph() {
    let (_d, w) = code_fixture(&[("src/runs.py", NESTED_PY)]);
    // The call inside `mk_run` is its enclosing function's; `mk_run(x)` is
    // no edge.
    assert_eq!(callees(&w, "runs", "outer"), ["src/runs.py:9-10"]);
    assert_eq!(callees(&w, "runs", "mk_run"), [] as [String; 0]);
    assert!(w.callers("runs", "mk_run", None, None).is_err());
}

#[test]
fn search_does_not_note_a_nested_copy() {
    let out = twin_note(&[
        ("src/a.py", "def slice_cap_for():\n    pass\n"),
        (
            "src/b.py",
            "def run():\n    def slice_cap_for():\n        pass\n",
        ),
    ]);
    assert!(!out.contains("also defined"), "{out}");
}

#[test]
fn a_nested_python_function_stays_out_of_the_hint() {
    let (_d, w) = code_fixture(&[(
        "src/rotation.py",
        "def schedule_rotation(keys):\n    def rotation_step(k):\n        return k\n    return [rotation_step(k) for k in keys]\n",
    )]);
    let hint = wiki::hint::hint_lines(&w, "where is the rotation schedule step for keys");
    assert!(hint.contains("src/rotation.py:1-4"), "{hint}");
    assert!(!hint.contains("rotation_step"), "{hint}");
}

const GLOBALS_SH: &str = "\
#!/usr/bin/env bash
QX_PY_DERIVE=\"$(cat <<'PY'
import sys
PY
)\"

_qx_py_run() {
  python3 -c \"$QX_PY_DERIVE\" \"$@\"
}

derive_python_step() {
  _qx_py_run step
}
_qx_py_run go
";

#[test]
fn a_shell_global_is_found_by_its_exact_name() {
    let (_d, w) = code_fixture(&[("lib/qx.sh", GLOBALS_SH)]);
    assert_eq!(
        symbol_lines(&w.search("QX_PY_DERIVE", None, 10)),
        [("lib/qx.sh".to_string(), 2, 5, "QX_PY_DERIVE".to_string())]
    );
    let mixed = symbol_lines(&w.search("_qx_py_run QX_PY_DERIVE", None, 10));
    assert_eq!(
        mixed,
        [
            ("lib/qx.sh".to_string(), 7, 9, "_qx_py_run".to_string()),
            ("lib/qx.sh".to_string(), 2, 5, "QX_PY_DERIVE".to_string()),
        ]
    );
    assert_eq!(
        w.not_found("_qx_py_run QX_PY_DERIVE", None, None),
        Vec::<String>::new()
    );
}

#[test]
fn a_shell_global_leaves_word_queries_unchanged() {
    // Lowercase `qx_py_derive=` is no item; the body reads the same.
    let lower = GLOBALS_SH.replace("QX_PY_DERIVE", "qx_py_derive");
    let (_a, with) = code_fixture(&[("lib/qx.sh", GLOBALS_SH)]);
    let (_b, without) = code_fixture(&[("lib/qx.sh", &lower)]);
    assert!(with.items("qx").iter().any(|i| i.name == "QX_PY_DERIVE"));
    for q in ["derive python step", "py derive", "derive"] {
        let show = |w: &Wiki| wiki::serve::search_lines(&w.search(q, None, 10));
        assert_eq!(show(&with), show(&without), "{q}");
    }
}

#[test]
fn a_shell_global_stays_out_of_page_graph_and_hint() {
    let (_d, w) = code_fixture(&[("lib/qx.sh", GLOBALS_SH)]);
    let outline = w.outline("qx", None).unwrap();
    assert!(!outline.contains("QX_PY_DERIVE"), "{outline}");
    assert!(w.callers("qx", "QX_PY_DERIVE", None, None).is_err());
    let callers = w.callers("qx", "_qx_py_run", None, None).unwrap();
    assert!(!callers.contains("QX_PY_DERIVE"), "{callers}");
    let pack = w.neighbors("qx", 1, &PackBudget::default()).unwrap().text;
    assert!(!pack.contains("QX_PY_DERIVE"), "{pack}");
    let hint = wiki::hint::hint_lines(&w, "where is the zb py derive script run");
    assert!(!hint.contains("QX_PY_DERIVE"), "{hint}");
}
