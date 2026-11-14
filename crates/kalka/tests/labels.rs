//! Markdown labels: ATX headings and bold step lines with their line ranges.

use kalka::labels::{doc_lines, doc_search, labels, DocHit, DOC_LIST_LIMIT, DOC_NO_MATCH};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
    // out of order only through the named any-order fallback
    let (h, _, _, fb) = doc_search(dir(), &["plan.md".into()], "matrix remaining", None);
    assert_eq!(
        (h[0].text.as_str(), fb.is_some()),
        ("T2d. Remaining matrix.", true)
    );
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
    let (h, _, _, _) = doc_search(dir(), &["plan.md".into()], "", Some(2));
    assert_eq!(h.len(), 2);
}
#[test]
fn bad_paths_get_one_line_each_and_others_answer() {
    let (h, e, _, _) = doc_search(
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
        doc_lines(&hits("T2d"), &[], 0, None),
        "plan.md\n  5-10  T2d. Remaining matrix.\n"
    );
    assert_eq!(doc_lines(&[], &[], 0, None), format!("{DOC_NO_MATCH}\n"));
}
#[test]
fn no_match_line_only_when_nothing_else_prints() {
    assert_eq!(
        doc_lines(&[], &["nope.md: not found".into()], 0, None),
        "nope.md: not found\n"
    );
    let cut = doc_lines(&[], &[], 3, None);
    assert!(
        cut.contains("… 3 more labels") && !cut.contains("no matches"),
        "{cut}"
    );
    // An empty query on docs with no labels gets the same line (accepted).
    let d = std::env::temp_dir().join(format!("wiki-nolabel-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("p.md"), "plain text, no labels\n").unwrap();
    let (h, e, more, _) = doc_search(&d, &["p.md".to_string()], "", None);
    assert_eq!(doc_lines(&h, &e, more, None), format!("{DOC_NO_MATCH}\n"));
}
#[test]
fn lines_print_each_path_once_and_errors_last() {
    let h = |p: &str, s| DocHit {
        path: p.into(),
        start: s,
        end: s,
        text: "x".into(),
    };
    let hs = [h("a.md", 1), h("a.md", 2), h("a.md", 4), h("b.md", 3)];
    assert_eq!(
        doc_lines(&hs, &["c.md: not found".into()], 0, None),
        "a.md\n  1-1  x\n  2-2  x\n  4-4  x\nb.md\n  3-3  x\nc.md: not found\n"
    );
}

fn texts_in(md: &str, q: &str) -> Vec<String> {
    let d = std::env::temp_dir().join(format!("wiki-hy-{}-{}", std::process::id(), md.len()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("p.md"), md).unwrap();
    let (h, _, _, _) = doc_search(&d, &["p.md".to_string()], q, None);
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
    let (hits, _, _, _) = doc_search(&dir, &["p.md".to_string()], "T5k", None);
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

fn write_tmp(name: &str, md: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("wiki-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("p.md"), md).unwrap();
    std::fs::write(d.join("q.md"), "## Task A\n").unwrap();
    d
}

#[test]
fn hits_are_grouped_by_doc_after_global_ranking() {
    // p.md: bold match ranks after q.md heading match, but output groups by doc.
    let d = write_tmp("group", "**Task B**\n## Task C\n");
    let docs = ["p.md".to_string(), "q.md".to_string()];
    let (h, _, _, _) = doc_search(&d, &docs, "task", None);
    let t: Vec<_> = h
        .iter()
        .map(|h| (h.path.as_str(), h.text.as_str()))
        .collect();
    assert_eq!(
        t,
        [("p.md", "Task C"), ("p.md", "Task B"), ("q.md", "Task A")]
    );
    assert_eq!(
        doc_lines(&h, &[], 0, None),
        "p.md\n  2-2  Task C\n  1-1  Task B\nq.md\n  1-1  Task A\n"
    );
    // the limit keeps the globally best hits, then groups them
    let (h, _, _, _) = doc_search(&d, &docs, "task", Some(2));
    let t: Vec<_> = h.iter().map(|h| h.text.as_str()).collect();
    assert_eq!(t, ["Task C", "Task A"]);
}

#[test]
fn duplicate_docs_are_read_once() {
    let d = write_tmp("dup", "**T2d. x**\n");
    let abs = d.join("p.md").to_string_lossy().into_owned();
    for docs in [
        vec!["p.md".to_string(), "p.md".to_string()],
        vec!["p.md".to_string(), abs],
    ] {
        assert_eq!(doc_search(&d, &docs, "T2d", None).0.len(), 1);
    }
}

#[test]
fn ordered_list_forms_are_labels() {
    let md = "1) **A. one**\n1. [ ] **B. two**\n2) [x] **C. three**\n";
    let t: Vec<_> = labels(md).into_iter().map(|l| l.text).collect();
    assert_eq!(t, ["A. one", "B. two", "C. three"]);
}

#[test]
fn empty_query_listing_is_capped_unless_limit_given() {
    let md: String = (0..250).fold(String::new(), |s, i| s + &format!("**L{i}. x**\n"));
    let d = write_tmp("cap", &md);
    let docs = ["p.md".to_string()];
    let (h, _, more, _) = doc_search(&d, &docs, "", None);
    assert_eq!((h.len(), more), (DOC_LIST_LIMIT, 50));
    let out = doc_lines(&h, &["z.md: not found".into()], more, None);
    assert!(out.ends_with("… 50 more labels; pass limit to list them\nz.md: not found\n"));
    let (h, _, more, _) = doc_search(&d, &docs, "", Some(300));
    assert_eq!((h.len(), more), (250, 0));
    assert!(!doc_lines(&h, &[], 0, None).contains("more label"));
    assert!(doc_lines(&h, &[], 1, None).ends_with("… 1 more label; pass limit to list them\n"));
    assert_eq!(doc_search(&d, &docs, "L1", None).2, 0);
}

#[test]
fn a_cut_repeated_label_keeps_its_first_and_last_entries() {
    let d = write_tmp("repeat", "## N11: a\n## Row X\n## N11: b\n## N11: c\n");
    let docs = ["p.md".to_string()];
    let t = |q: &str, limit| -> Vec<String> {
        let h = doc_search(&d, &docs, q, limit).0;
        h.into_iter().map(|h| h.text).collect()
    };
    assert_eq!(t("N11", None), ["N11: a", "N11: b", "N11: c"]);
    assert_eq!(t("N11", Some(2)), ["N11: a", "N11: c"]);
    assert_eq!(t("", None), ["N11: a", "Row X", "N11: b", "N11: c"]);
    let six = (1..=6).fold(String::new(), |s, i| s + &format!("## N11: {i}\n"));
    let d = write_tmp("six", &six);
    let h = doc_search(&d, &docs, "N11", Some(4)).0;
    let t: Vec<_> = h.iter().map(|h| h.text.as_str()).collect();
    assert_eq!(t, ["N11: 1", "N11: 2", "N11: 5", "N11: 6"]);
    let d = write_tmp("heads", "## Task 1: x\n## Task 2: y\n");
    let h = doc_search(&d, &docs, "Task", None).0;
    let t: Vec<_> = h.iter().map(|h| h.text.as_str()).collect();
    assert_eq!(t, ["Task 1: x", "Task 2: y"]);
    for (md, q) in [
        (
            "## https://a.com\n## https://b.org\n## https://c.net\n",
            "https",
        ),
        ("## `T5:a` x\n## `T5:b` y\n## `T5:c` z\n", "T5"),
    ] {
        let d = write_tmp("ownhead", md);
        let h = doc_search(&d, &docs, q, Some(2)).0;
        assert_eq!((h[0].start, h[1].start), (1, 2), "{md}");
    }
}
#[test]
fn a_cut_group_keeps_only_its_own_slots() {
    let d = write_tmp("slots", "## N11: a\n## N11 row\n## N11: b\n## N11: c\n");
    let docs = ["p.md".to_string()];
    let h = doc_search(&d, &docs, "N11", Some(3)).0;
    let t: Vec<_> = h.iter().map(|h| h.text.as_str()).collect();
    assert_eq!(t, ["N11: a", "N11 row", "N11: c"]);
}
#[test]
fn empty_query_keeps_document_order_across_levels() {
    let d = write_tmp("mixed", "**A1. bold**\n## H heading\n");
    let docs = ["p.md".to_string()];
    let h = doc_search(&d, &docs, "", None).0;
    let t: Vec<_> = h.iter().map(|h| h.text.as_str()).collect();
    assert_eq!(t, ["A1. bold", "H heading"]);
}

#[test]
fn labels_skip_bare_headings_and_strip_closing_hashes() {
    let md = "# Only\n\n#\n\n## Closing ##\n\n### Kept#\n\n## Tail ###   \n\n## ##\n\n  ## Indented\n    ## Code\n";
    let t: Vec<_> = labels(md).into_iter().map(|l| l.text).collect();
    assert_eq!(t, ["Only", "Closing", "Kept#", "Tail"]);
}

fn fallback_in(md: &str, q: &str) -> (Vec<String>, Option<String>) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = write_tmp(&format!("fb{n}"), md);
    let (h, _, _, fb) = doc_search(&d, &["p.md".to_string()], q, None);
    (h.into_iter().map(|h| h.text).collect(), fb)
}
const SPEC: &str = "## 11.4 The QX time line\n## 11.6 The crop-history export grammar (`tally --run`)\n**Step 5c: a flag above the stamp's per-batch N refuses.**\n**Task 9: brew_eval NOT SCORED, `--live-plots`**\n";
#[test]
fn leading_id_answers_when_the_words_miss() {
    let (t, fb) = fallback_in(SPEC, "11.4 QX time line");
    assert_eq!(t, ["11.4 The QX time line"]);
    assert!(fb.unwrap().contains("leading ID 11.4"));
    let (t, _) = fallback_in(SPEC, "11.6 export coverage tally --run");
    assert_eq!(t, ["11.6 The crop-history export grammar (`tally --run`)"]);
}
#[test]
fn all_words_in_any_order_answer_without_stop_words() {
    let (t, fb) = fallback_in(SPEC, "Step 5c flag above the stamp");
    assert_eq!(
        t,
        ["Step 5c: a flag above the stamp's per-batch N refuses."]
    );
    assert!(fb.unwrap().contains("any order"));
    let (t, _) = fallback_in(SPEC, "NOT SCORED live plots");
    assert_eq!(t, ["Task 9: brew_eval NOT SCORED, `--live-plots`"]);
    let (t, _) = fallback_in(SPEC, "time line QX");
    assert_eq!(t, ["11.4 The QX time line"]);
}
#[test]
fn normal_matches_and_misses_name_no_fallback() {
    assert_eq!(
        fallback_in(SPEC, "QX time line"),
        (vec!["11.4 The QX time line".to_string()], None)
    );
    // one word never falls back; nor do words a label lacks
    assert_eq!(fallback_in(SPEC, "QXV"), (vec![], None));
    assert_eq!(
        fallback_in(SPEC, "11.9 QX time line coverage"),
        (vec![], None)
    );
    assert_eq!(fallback_in(SPEC, "the a of"), (vec![], None));
    // the ID must open the label (first or second word); one word left is not enough
    let late = "## Row A: help is a usage error (QA-F Task 6 T3)\n**Step 5g: fee.**\n";
    assert_eq!(fallback_in(late, "T3 (i-c)"), (vec![], None));
    assert_eq!(fallback_in(late, "5g is complete").0, ["Step 5g: fee."]);
    assert_eq!(fallback_in(late, "in usage"), (vec![], None));
}
#[test]
fn the_fallback_is_named_first_in_lines() {
    let d = write_tmp("fbl", SPEC);
    let (h, e, more, fb) = doc_search(&d, &["p.md".to_string()], "11.4 QX", None);
    let json: serde_json::Value =
        serde_json::from_str(&kalka::serve::doc_json(&h, &e, more, fb.as_deref())).unwrap();
    assert_eq!(json["fallback"].as_str(), fb.as_deref());
    let plain: serde_json::Value =
        serde_json::from_str(&kalka::serve::doc_json(&h, &e, more, None)).unwrap();
    assert!(plain.get("fallback").is_none());
    let out = doc_lines(&h, &e, more, fb.as_deref());
    assert_eq!(
        out,
        format!("{}\np.md\n  1-1  11.4 The QX time line\n", fb.unwrap())
    );
}
const READING: &str = "# Plan\n## Global constraints\n**Quoted code pins**\n## Review Focus\n## Rows\n## Per-batch procedure (every batch)\n**Pins**\n**The saved baseline is never written**\n## Task T4: WIDTH-V2-DEV (lead R-S)\n**T4a. Check.**\n## Task T9: hand-off batches\n**T9a. End mark of an open batch.**\n**T9k. The reserved window ends.**\n**RF2, the widened window (T9a).**\n## 7. Tasks\n## 11.7 Halt order\n## 1. Intent\n## 2. Goals\n## Audit pass (2026-11-11), record\n## Git.\n";
#[test]
fn several_labels_in_one_query_answer_in_query_order() {
    let (t, fb) = fallback_in(READING, "Global constraints Per-batch procedure Pins");
    assert_eq!(
        t,
        [
            "Global constraints",
            "Per-batch procedure (every batch)",
            "Pins"
        ]
    );
    assert_eq!(
        fb.as_deref(),
        Some("no label holds the words in a row; labels each part of the query opens, in query order:")
    );
    let (t, _) = fallback_in(READING, "T9a T9k Review Focus Rows Task T9 Audit pass");
    assert_eq!(
        t,
        [
            "T9a. End mark of an open batch.",
            "T9k. The reserved window ends.",
            "Review Focus",
            "Rows",
            "Task T9: hand-off batches",
            "Audit pass (2026-11-11), record"
        ]
    );
    // an ID may be a label's second word; a part with a longer label is one part
    let (t, _) = fallback_in(READING, "T4 Rows");
    assert_eq!(t, ["Task T4: WIDTH-V2-DEV (lead R-S)", "Rows"]);
}
#[test]
fn words_naming_no_label_are_listed_first() {
    let (t, fb) = fallback_in(READING, "T9i-1 T9a Global constraints Audit pass record");
    assert_eq!(
        t,
        [
            "T9a. End mark of an open batch.",
            "Global constraints",
            "Audit pass (2026-11-11), record"
        ]
    );
    assert!(fb
        .unwrap()
        .ends_with("in query order (no label opens with T9i-1, record):"));
}
#[test]
fn bare_numbers_and_common_words_name_no_part() {
    // a bare number is not an ID: these keep the leading-ID fallback alone
    let (t, fb) = fallback_in(READING, "11.7 step 7");
    assert_eq!(t, ["11.7 Halt order"]);
    assert!(fb.unwrap().contains("leading ID 11.7"));
    assert_eq!(fallback_in(READING, "items 1, 2"), (vec![], None));
    // one plain word must be a whole label, so `the` opens no part
    let (t, fb) = fallback_in(READING, "T4a the window");
    assert_eq!(t, ["T4a. Check."]);
    assert!(fb.unwrap().contains("leading ID T4a"));
    // the same label named again is not a second part
    assert_eq!(
        fallback_in(READING, "git log main git branch git status"),
        (vec![], None)
    );
}
#[test]
fn single_label_answers_do_not_change() {
    assert_eq!(
        fallback_in(READING, "Global constraints"),
        (vec!["Global constraints".to_string()], None)
    );
    assert_eq!(
        fallback_in(READING, "T9a").0,
        [
            "T9a. End mark of an open batch.",
            "RF2, the widened window (T9a)."
        ]
    );
}
fn bold_texts(md: &str) -> Vec<(usize, usize, String)> {
    labels(md)
        .into_iter()
        .filter(|l| l.level == 7)
        .map(|l| (l.start, l.end, l.text))
        .collect()
}
#[test]
fn a_wrapped_step_label_is_one_line_from_its_first_line() {
    let md = "- [x] **T9j. Evidence is a file.**\n  body\n- [x] **T9o. `tally` refuses while a process holds store.db (rows N11,\n  UNMATCHED-REMAINDER).** Tests-only.\n  more\n- [x] **T9g. Next.**\n";
    assert_eq!(
        bold_texts(md),
        [
            (1, 2, "T9j. Evidence is a file.".to_string()),
            (
                3,
                5,
                "T9o. `tally` refuses while a process holds store.db (rows N11, UNMATCHED-REMAINDER)."
                    .to_string()
            ),
            (6, 6, "T9g. Next.".to_string()),
        ]
    );
}
#[test]
fn a_wrapped_label_closes_within_three_more_lines() {
    let close_on = |n: usize| {
        let mut md = String::from("- **Step 1 opens");
        for i in 2..n {
            md.push_str(&format!("\n  line {i}"));
        }
        md.push_str("\n  ends.** body\n");
        bold_texts(&md).into_iter().map(|l| l.2).collect::<Vec<_>>()
    };
    assert_eq!(close_on(2), ["Step 1 opens ends."]);
    assert_eq!(close_on(3), ["Step 1 opens line 2 ends."]);
    assert_eq!(close_on(4), ["Step 1 opens line 2 line 3 ends."]);
    assert_eq!(close_on(5), Vec::<String>::new());
}
#[test]
fn no_wrapped_label_across_breaks_code_or_an_open_span() {
    // Python outside a fence: `(**kwargs)` opens a span, it does not close one
    let code =
        "        **kwargs: Any,\n    ) -> None:\n        super().__init_subclass__(**kwargs)\n";
    assert_eq!(bold_texts(code), vec![]);
    for stop in ["", "## Heading", "- next item", "**Other.** text"] {
        let md = format!("- **T1 opens\n{stop}\n  ends.** body\n");
        let t: Vec<_> = bold_texts(&md).into_iter().map(|l| l.2).collect();
        assert!(!t.iter().any(|t| t.starts_with("T1")), "{stop:?}: {t:?}");
    }
    // never closed
    assert_eq!(bold_texts("- **T1 opens\n  and runs on\n"), vec![]);
    // a wrapped field is still a field
    assert_eq!(bold_texts("**Files\nof the task:** a.rs\n"), vec![]);
}
#[test]
fn one_line_labels_are_unchanged_by_wrapping() {
    assert_eq!(
        bold_texts("- **T1. A.** x\n  y **z** w\n**Files:**\n- **T2. B.**\n"),
        [(1, 3, "T1. A.".to_string()), (4, 4, "T2. B.".to_string())]
    );
}
