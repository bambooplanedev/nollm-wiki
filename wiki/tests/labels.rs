//! Markdown labels: ATX headings and bold step lines with their line ranges.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use wiki::labels::{doc_lines, doc_search, labels, DocHit};

const PLAN: &str = "# Plan\n\n## Task 2\n\n**T2d. Remaining matrix.**\n\n**Files:**\n- a.rs\n- **Mutants:** x\n\n**T2e (QA-H T5): next.**\nbody\n```sh\n## not a heading\n**T9. not a label**\n```\n## Task 3\n- **D-d, swap (H1).** ruling\n";

fn range_of(md: &str, text: &str) -> (usize, usize) {
    let l = labels(md).into_iter().find(|l| l.text == text).unwrap();
    (l.start, l.end)
}

#[test]
fn finds_headings_and_bold_labels_not_fields() {
    let t: Vec<_> = labels(PLAN)
        .into_iter()
        .map(|l| (l.start, l.text, l.level))
        .collect();
    assert_eq!(
        t,
        [
            (1, "Plan".into(), 1),
            (3, "Task 2".into(), 2),
            (5, "T2d. Remaining matrix.".into(), 7),
            (11, "T2e (QA-H T5): next.".into(), 7),
            (17, "Task 3".into(), 2),
            (18, "D-d, swap (H1).".into(), 7)
        ]
    );
}
#[test]
fn bold_range_runs_over_fields_to_next_bold() {
    assert_eq!(range_of(PLAN, "T2d. Remaining matrix."), (5, 10));
}
#[test]
fn bold_range_ends_at_next_heading_past_a_fence() {
    assert_eq!(range_of(PLAN, "T2e (QA-H T5): next."), (11, 16));
}
#[test]
fn heading_range_ends_at_same_level_heading() {
    assert_eq!(range_of(PLAN, "Task 2"), (3, 16));
}
#[test]
fn last_label_ends_at_last_line() {
    assert_eq!(range_of(PLAN, "D-d, swap (H1)."), (18, 18));
}
#[test]
fn top_heading_spans_file() {
    assert_eq!(range_of(PLAN, "Plan"), (1, 18));
}

#[test]
fn tilde_and_long_fences_hide_fake_labels() {
    let md = "# A\n~~~\n## fake\n**fake**\n```\n~~~\n````\n**x**\n```\n````\n**real**\n";
    let t: Vec<_> = labels(md)
        .into_iter()
        .map(|l| (l.start, l.end, l.text))
        .collect();
    assert_eq!(t, [(1, 11, "A".into()), (11, 11, "real".into())]);
}

const LEDGER: &str =
    "## Row NULL-FLUSH\nx\n**NULL-FLUSH/c0.** y\n- **Ledger:** append_row NULL-FLUSH\n";

fn dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("wiki-labels-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("plan.md"), PLAN).unwrap();
        std::fs::write(d.join("ledger.md"), LEDGER).unwrap();
        d
    })
}
fn hits(q: &str) -> Vec<DocHit> {
    doc_search(dir(), &["plan.md".into(), "ledger.md".into()], q, None).0
}
fn texts(q: &str) -> Vec<String> {
    hits(q).into_iter().map(|h| h.text).collect()
}

#[test]
fn id_matches_whole_word_only() {
    assert_eq!(texts("T2d"), ["T2d. Remaining matrix."]);
    assert_eq!(texts("T2").len(), 0);
}
#[test]
fn id_inside_a_label_matches_after_first_word_hits() {
    assert_eq!(texts("T5"), ["T2e (QA-H T5): next."]);
}
#[test]
fn title_words_consecutive_case_insensitive() {
    assert_eq!(texts("remaining MATRIX"), ["T2d. Remaining matrix."]);
    assert_eq!(texts("matrix remaining").len(), 0);
}
#[test]
fn heading_ranks_before_bold_and_fields_never_match() {
    assert_eq!(
        hits("NULL-FLUSH")
            .iter()
            .map(|h| (h.path.as_str(), h.start, h.end))
            .collect::<Vec<_>>(),
        [("ledger.md", 1, 4), ("ledger.md", 3, 4)]
    );
}
#[test]
fn first_word_match_ranks_before_later_word() {
    assert_eq!(texts("task"), ["Task 2", "Task 3"]);
    assert_eq!(texts("remaining")[0], "T2d. Remaining matrix.");
}
#[test]
fn empty_query_lists_every_label_unbounded() {
    assert_eq!(hits("").len(), 6 + 2);
    let (h, _) = doc_search(dir(), &["plan.md".into()], "", Some(2));
    assert_eq!(h.len(), 2);
}
#[test]
fn bad_paths_get_one_line_each_and_others_answer() {
    let (h, e) = doc_search(
        dir(),
        &["nope.md".into(), "x.txt".into(), "plan.md".into()],
        "T2d",
        None,
    );
    assert_eq!(h.len(), 1);
    assert_eq!(e, ["nope.md: not found", "x.txt: not a markdown file"]);
}
#[test]
fn lines_render_like_symbol_ranges() {
    assert_eq!(
        doc_lines(&hits("T2d"), &[]),
        "plan.md\n  5-10  T2d. Remaining matrix.\n"
    );
    assert_eq!(doc_lines(&[], &[]), "no matches\n");
}
#[test]
fn lines_print_each_path_once_and_errors_last() {
    let h = |p: &str, s| DocHit {
        path: p.into(),
        start: s,
        end: s,
        text: "x".into(),
    };
    let hs = [h("a.md", 1), h("a.md", 2), h("b.md", 3), h("a.md", 4)];
    assert_eq!(
        doc_lines(&hs, &["c.md: not found".into()]),
        "a.md\n  1-1  x\n  2-2  x\nb.md\n  3-3  x\na.md\n  4-4  x\nc.md: not found\n"
    );
}

fn texts_in(md: &str, q: &str) -> Vec<String> {
    let d = std::env::temp_dir().join(format!("wiki-hy-{}-{}", std::process::id(), md.len()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("p.md"), md).unwrap();
    let (h, _) = doc_search(&d, &["p.md".to_string()], q, None);
    h.into_iter().map(|h| h.text).collect()
}
#[test]
fn hyphen_insensitive_fallback_both_ways() {
    assert_eq!(
        texts_in("## 4.7 Load profile from result.txt\n", "load-profile"),
        ["4.7 Load profile from result.txt"]
    );
    assert_eq!(
        texts_in("**Load-profile analyzer task**\n", "load profile"),
        ["Load-profile analyzer task"]
    );
}
#[test]
fn one_word_query_never_uses_the_fallback() {
    let md = "**T5m-a. Execution split.**\n**T5m. purge.**\n";
    assert_eq!(texts_in(md, "T5m"), ["T5m. purge."]);
}
#[test]
fn exact_matches_rank_before_hyphen_fallback() {
    let md = "## Row TESTS-BREW-RED\n## Row TESTS-BREW\n";
    assert_eq!(
        texts_in(md, "TESTS-BREW"),
        ["Row TESTS-BREW", "Row TESTS-BREW-RED"]
    );
    let md2 = "## Row TESTS-BREW\n## Row TESTS-BREW-RED\n";
    assert_eq!(texts_in(md2, "TESTS-BREW")[0], "Row TESTS-BREW");
}

const CHECKBOX_PLAN: &str = "# P\n- [ ] **T5k. job_start + finalize step 10 (H2).**\n  - **Files:**\nbody\n- [x] **T5l. next.**\n";

#[test]
fn checkbox_steps_are_labels() {
    let t: Vec<_> = labels(CHECKBOX_PLAN)
        .into_iter()
        .map(|l| (l.start, l.end, l.text))
        .collect();
    assert_eq!(
        t[1..],
        [
            (2, 4, "T5k. job_start + finalize step 10 (H2).".into()),
            (5, 5, "T5l. next.".into())
        ]
    );
    let dir = std::env::temp_dir().join(format!("wiki-cb-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("p.md"), CHECKBOX_PLAN).unwrap();
    let (hits, _) = doc_search(&dir, &["p.md".to_string()], "T5k", None);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].start, hits[0].end), (2, 4));
}

#[test]
fn query_matches_whole_words_only() {
    let md = "**T5k. a**\n**T5.0 b**\n**T5g, c**\n";
    let dir = std::env::temp_dir().join(format!("wiki-ww-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("p.md"), md).unwrap();
    let d = ["p.md".to_string()];
    assert_eq!(doc_search(&dir, &d, "T5", None).0, []);
    let hits = doc_search(&dir, &d, "T5.0", None).0;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].text, "T5.0 b");
}

#[test]
fn inline_code_at_line_start_does_not_open_a_fence() {
    let t: Vec<_> = labels("```x``` y\n**T1. a**\n")
        .into_iter()
        .map(|l| l.text)
        .collect();
    assert_eq!(t, ["T1. a"]);
}
