use std::fs;
use tempfile::tempdir;
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

/// Spec §8: budgets must degrade by dropping the LOWEST-centrality
/// neighbors first, keeping the highest-centrality ones that fit.
///
/// Fixture: "hub" links to two direct neighbors, "popular" and "rare".
/// "popular" additionally has three filler entities pointing at it, so its
/// pagerank is unambiguously higher than "rare"'s (4 incoming links vs 1) —
/// none of the fillers link to "hub" itself, so they never enter the
/// depth-1 neighbor set.
///
/// hub's body ("Hub mentions Popular and Rare.") is 30 chars ->
/// token_estimate 7 (chars/4). Neighbor summaries cost a flat 20 tokens
/// each. A `max_tokens` of 27 admits the target plus exactly one neighbor
/// (7 + 20 = 27) but never two (7 + 40 = 47 > 27), so this asserts the
/// *single* neighbor kept is the highest-centrality one ("popular"), not
/// the lowest-centrality one ("rare").
#[test]
fn neighbors_pack_max_tokens_keeps_highest_centrality_neighbor() {
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
    fs::write(
        input.join("filler_one.txt"),
        "# Filler One\n\nFiller One talks about Popular.\n",
    )
    .unwrap();
    fs::write(
        input.join("filler_two.txt"),
        "# Filler Two\n\nFiller Two talks about Popular.\n",
    )
    .unwrap();
    fs::write(
        input.join("filler_three.txt"),
        "# Filler Three\n\nFiller Three talks about Popular.\n",
    )
    .unwrap();
    compile(&input, &output, &CompileOptions::default()).unwrap();
    let w = Wiki::load(&output).unwrap();

    let budget = PackBudget {
        max_tokens: Some(27),
        ..Default::default()
    };
    let pack = w.neighbors("hub", 1, &budget).unwrap();

    assert_eq!(pack.included.first().map(String::as_str), Some("hub"));
    assert_eq!(
        pack.included.len(),
        2,
        "expected target + exactly one neighbor, got {:?}",
        pack.included
    );
    assert!(
        pack.included.contains(&"popular".to_string()),
        "expected the highest-centrality neighbor (popular) to be kept, got {:?}",
        pack.included
    );
    assert!(
        !pack.included.contains(&"rare".to_string()),
        "expected the lowest-centrality neighbor (rare) to be dropped, got {:?}",
        pack.included
    );
}
