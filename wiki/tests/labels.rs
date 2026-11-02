//! Markdown labels: ATX headings and bold step lines with their line ranges.

use wiki::labels::labels;

const PLAN: &str = "# Plan\n\n## Task 2\n\n**T2d. Remaining matrix.**\n\n**Files:**\n- a.rs\n- **Mutants:** x\n\n**T2e (QA-H T5): next.**\nbody\n```sh\n## not a heading\n**T9. not a label**\n```\n## Task 3\n- **D-d, swap (H1).** ruling\n";

fn range_of(md: &str, text: &str) -> (usize, usize) {
    let l = labels(md).into_iter().find(|l| l.text == text).unwrap();
    (l.start, l.end)
}

#[test]
fn finds_headings_and_bold_labels_not_fields() {
    let t: Vec<_> = labels(PLAN).into_iter().map(|l| (l.start, l.text, l.level)).collect();
    assert_eq!(t, [(1,"Plan".into(),1),(3,"Task 2".into(),2),(5,"T2d. Remaining matrix.".into(),7),
                   (11,"T2e (QA-H T5): next.".into(),7),(17,"Task 3".into(),2),(18,"D-d, swap (H1).".into(),7)]);
}
#[test] fn bold_range_runs_over_fields_to_next_bold() { assert_eq!(range_of(PLAN, "T2d. Remaining matrix."), (5, 10)); }
#[test] fn bold_range_ends_at_next_heading_past_a_fence() { assert_eq!(range_of(PLAN, "T2e (QA-H T5): next."), (11, 16)); }
#[test] fn heading_range_ends_at_same_level_heading() { assert_eq!(range_of(PLAN, "Task 2"), (3, 16)); }
#[test] fn last_label_ends_at_last_line() { assert_eq!(range_of(PLAN, "D-d, swap (H1)."), (18, 18)); }
#[test] fn top_heading_spans_file() { assert_eq!(range_of(PLAN, "Plan"), (1, 18)); }
