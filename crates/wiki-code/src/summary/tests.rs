//! Tests for `summary`.
use super::*;

#[test]
fn prefers_frontmatter_description() {
    let s = summarize(Some("Short desc."), None, "body sentence.", None);
    assert_eq!(s.as_deref(), Some("Short desc."));
}

#[test]
fn skips_headings_and_boilerplate_first_sentence() {
    let body = "# Title\nThis document describes stuff.\nGradient descent optimizes loss.";
    let s = summarize(None, None, body, None);
    assert_eq!(s.as_deref(), Some("Gradient descent optimizes loss."));
}

#[test]
fn a_dot_inside_a_word_does_not_end_the_sentence() {
    let s = summarize(
        None,
        Some("Renders index.json and llms.txt for agents. Second sentence."),
        "",
        None,
    );
    assert_eq!(
        s.as_deref(),
        Some("Renders index.json and llms.txt for agents.")
    );
    assert_eq!(first_sentence("Bump to v1.2"), "Bump to v1.2");
    assert_eq!(first_sentence("Done!Next"), "Done!Next");
}

#[test]
fn an_initialism_does_not_end_the_sentence() {
    assert_eq!(
        first_sentence("Price in admin tables, e.g. 12.50 EUR. Uses a dot."),
        "Price in admin tables, e.g. 12.50 EUR."
    );
    assert_eq!(
        first_sentence("Ships to the U.S. only. Rest."),
        "Ships to the U.S. only."
    );
    assert_eq!(first_sentence("Read a file. Then go."), "Read a file.");
    assert_eq!(first_sentence("Use plan B. Then go."), "Use plan B.");
    assert_eq!(
        first_sentence("Logs, caches, etc. Then go."),
        "Logs, caches, etc."
    );
    // A sentence that really ends on an initialism runs on: after `e.g.` a
    // capital is as common as after a real end, so the two look the same.
    assert_eq!(
        first_sentence("Made in the U.S. Next one."),
        "Made in the U.S. Next one."
    );
}

#[test]
fn a_tag_line_ends_the_first_sentence() {
    let doc = "Pin the current thread to a core\n@param coreId The core\n@return true on success";
    let s = summarize(None, Some(doc), "", None);
    assert_eq!(s.as_deref(), Some("Pin the current thread to a core"));
    let doc = "Async write callback\n\\param ec The status code";
    let s = summarize(None, Some(doc), "", None);
    assert_eq!(s.as_deref(), Some("Async write callback"));
    // An `@` that opens no tag stays in the paragraph.
    let doc = "Mails the owner\n@ 9am daily";
    let s = summarize(None, Some(doc), "", None);
    assert_eq!(s.as_deref(), Some("Mails the owner @ 9am daily"));
}

#[test]
fn a_sentence_wrapped_across_lines_is_joined_before_the_cut() {
    let doc = "Rust extraction: bare-`pub` gating, owner qualification through\nimpl scopes. Second sentence\nwraps too.";
    assert_eq!(
        summarize(None, Some(doc), "", None).as_deref(),
        Some("Rust extraction: bare-`pub` gating, owner qualification through impl scopes.")
    );
    // The paragraph ends at a blank line: a following paragraph is never
    // glued on when the first has no terminator.
    let body = "first paragraph without a stop\n\nSecond paragraph.";
    assert_eq!(
        summarize(None, None, body, None).as_deref(),
        Some("first paragraph without a stop")
    );
}

#[test]
fn falls_back_to_signature_then_none() {
    assert_eq!(
        summarize(None, None, "", Some("fn foo(a: i32)")).as_deref(),
        Some("fn foo(a: i32)")
    );
    assert_eq!(summarize(None, None, "   \n\n", None), None);
}

#[test]
fn a_list_yields_its_first_item() {
    let s = summarize(
        None,
        None,
        "- [A](a.md) — first\n- [B](b.md) — second",
        None,
    );
    assert_eq!(s.as_deref(), Some("[A](a.md) — first"));
}

#[test]
fn a_list_item_ends_the_paragraph() {
    let s = summarize(None, None, "Intro line\n- item", None);
    assert_eq!(s.as_deref(), Some("Intro line"));
}

#[test]
fn numbered_items() {
    let s = summarize(None, None, "1. one\n2. two", None);
    assert_eq!(s.as_deref(), Some("one"));
    let s = summarize(None, None, "10. ten\n11. eleven", None);
    assert_eq!(s.as_deref(), Some("ten"));
}

#[test]
fn not_list_items() {
    for (body, want) in [
        ("-foo bar\nmore.", "-foo bar more."),
        ("*emphasis* text\nnext.", "*emphasis* text next."),
        ("Intro\n---\nmore.", "Intro --- more."),
    ] {
        assert_eq!(summarize(None, None, body, None).as_deref(), Some(want));
    }
}

#[test]
fn wrapped_item_continues() {
    let s = summarize(None, None, "- first item\n  wraps here\n- second", None);
    assert_eq!(s.as_deref(), Some("first item wraps here"));
}
