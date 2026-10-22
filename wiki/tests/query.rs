//! `Wiki::load`, `search`, and `neighbors` against a compiled output directory, including the pack budget.

use std::fs;
use tempfile::tempdir;
use wiki::model::SourceKind;
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
        ids(w.search("zebra", Some(SourceKind::Text), 10)),
        ["guide"]
    );
    assert_eq!(
        ids(w.search(
            "zebra",
            Some(SourceKind::Code {
                lang: "rust".into()
            }),
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
