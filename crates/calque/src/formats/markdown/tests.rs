//! Tests for `markdown`.
use super::*;

#[test]
fn tag_list_comment_and_quote_edge_cases() {
    let fm = |s: &str| s.lines().map(String::from).collect::<Vec<_>>();
    let tags = |s: &str| fm_list(&fm(s), "tags");
    assert_eq!(
        tags("tags: # my tags\n  - alpha\n  - beta"),
        vec!["alpha", "beta"]
    );
    assert_eq!(tags("tags: # none"), Vec::<String>::new());
    assert_eq!(
        tags("tags:\n  - \"Issue #42\" # c\n  - Peoples'"),
        vec!["Issue #42", "Peoples'"]
    );
    assert_eq!(tags("tags: [\"a b\", 'c']"), vec!["a b", "c"]);
}

fn extract_fm(line: &str) -> Entity {
    MarkdownExtractor.extract("n.md", &format!("---\n{line}\n---\n\nBody.\n"))
}

fn h1(md: &str) -> Entity {
    MarkdownExtractor.extract("docs/readme.md", md)
}

#[test]
fn h1_badges_and_links_are_cleaned() {
    let e = h1("# Node Version Manager [![build status](https://x/y.svg)](https://x/z) [![ci][3]](https://q) [![a](i.svg)][4]\n\n[3]: u\n[4]: u\n");
    assert_eq!(
        (e.name.as_str(), e.id.as_str()),
        ("Node Version Manager", "node_version_manager")
    );
    assert_eq!(h1("# Using [Foo](x) here\n").name, "Using Foo here");
    assert_eq!(
        h1("# Using [Foo][r] here ![img][r]\n\n[r]: u\n").name,
        "Using Foo here"
    );
    assert_eq!(h1("# Call `a[0](x)` now\n").name, "Call `a[0](x)` now");
    assert_eq!(h1("# <b>Hi</b>\n").name, "<b>Hi</b>");
}

#[test]
fn plain_brackets_and_escapes_keep_the_h1() {
    for h in [
        "Array[int] notes",
        "Matrix a[i][j]",
        "See [1][2]",
        "[x] todo",
        r"Esc \[a](b)",
    ] {
        assert_eq!(h1(&format!("# {h}\n")).name, h, "{h}");
    }
    // a reference is a link only when the document defines it (case-insensitive)
    assert_eq!(
        h1("# See [one][R] ![i][R]\n\n[r]: http://x\n").name,
        "See one"
    );
    assert_eq!(
        h1("# See [one][R2]\n\n[r]: http://x\n").name,
        "See [one][R2]"
    );
}

#[test]
fn badge_only_h1_falls_back_to_file_stem_and_front_matter_wins() {
    assert_eq!(
        h1("# [![a](i.svg)](l)\n").name,
        derive_name_from_path("docs/readme.md")
    );
    assert_eq!(h1("---\ntitle: A [b](c)\n---\n# X\n").name, "A [b](c)");
}

#[test]
fn quoted_description_is_unquoted_and_unescaped() {
    let e = extract_fm(r#"description: "A \"quoted\" word.""#);
    assert_eq!(e.summary.as_deref(), Some("A \"quoted\" word."));
}

#[test]
fn single_quoted_title_doubles_its_quote() {
    assert_eq!(extract_fm("title: 'It''s mine'").name, "It's mine");
}

#[test]
fn backslash_escape_in_double_quotes() {
    assert_eq!(extract_fm(r#"title: "a\\b""#).name, "a\\b");
}

#[test]
fn unmatched_quote_stays() {
    assert_eq!(extract_fm("title: \"abc").name, "\"abc");
}

#[test]
fn quoted_created() {
    assert_eq!(extract_fm("created: \"2026-02-02\"").created, "2026-02-02");
}

#[test]
fn markdown_pages_define_no_names() {
    let e = MarkdownExtractor.extract("a.md", "# A\n\nSome prose about Beta.\n");
    assert!(e.defined.is_empty(), "defined: {:?}", e.defined);
}

#[test]
fn frontmatter_fields_parsed() {
    let src = "---\ntitle: My Note\naliases: foo, bar\ncreated: 2026-02-02\ndescription: A short desc.\n---\n\nBody line one.\n";
    let e = MarkdownExtractor.extract("n.md", src);
    assert_eq!(e.name, "My Note");
    assert_eq!(e.aliases, vec!["foo", "bar"]);
    assert_eq!(e.created, "2026-02-02");
    assert_eq!(e.summary.as_deref(), Some("A short desc."));
    assert!(e.body.starts_with("Body line one."));
}

fn aliases_of(fm: &str) -> Vec<String> {
    MarkdownExtractor
        .extract("n.md", &format!("---\n{fm}---\nbody\n"))
        .aliases
}

#[test]
fn tags_normalized_from_front_matter() {
    let e = MarkdownExtractor.extract(
        "n.md",
        "---\ntags:\n  - '#Project/Wiki'\n  - draft\n  - DRAFT\n---\nb\n",
    );
    assert_eq!(e.tags, vec!["project/wiki", "draft"]);
    let e = MarkdownExtractor.extract("n.md", "---\ntags: [a, '#b']\n---\nb\n");
    assert_eq!(e.tags, vec!["a", "b"]);
}

#[test]
fn aliases_block_list_forms() {
    assert_eq!(aliases_of("aliases:\n  - AI\n  - ШІ\n"), vec!["AI", "ШІ"]);
    assert_eq!(aliases_of("aliases:\n- a\n- b\ntitle: T\n"), vec!["a", "b"]);
    assert_eq!(
        aliases_of("aliases:\n  # group\n  - a\n\n  - b # note\n"),
        vec!["a", "b"]
    );
    assert_eq!(aliases_of("aliases:\n  -\n  - real\n"), vec!["real"]);
    assert_eq!(
        aliases_of("aliases:\n  - \"q1\"\n  - 'q2'\n"),
        vec!["q1", "q2"]
    );
}

#[test]
fn aliases_one_line_forms_and_empties() {
    assert_eq!(aliases_of("aliases: a, b\n"), vec!["a", "b"]);
    assert_eq!(aliases_of("aliases: ['a', \"b\"]\n"), vec!["a", "b"]);
    assert_eq!(aliases_of("aliases: []\n"), [] as [String; 0]);
    assert_eq!(aliases_of("aliases: null\n"), [] as [String; 0]);
    assert_eq!(aliases_of("aliases: ~\n"), [] as [String; 0]);
}

#[test]
fn indented_keys_are_not_top_level() {
    assert_eq!(
        aliases_of("project:\n  aliases:\n    - inner\n"),
        [] as [String; 0]
    );
    let e = MarkdownExtractor.extract("n.md", "---\ndescription: |\n  title: oops\n---\n# Real\n");
    assert_eq!(e.name, "Real");
}

#[test]
fn crlf_block_list() {
    let e = MarkdownExtractor.extract(
        "n.md",
        "---\r\naliases:\r\n  - a\r\n  - b\r\n---\r\nbody\r\n",
    );
    assert_eq!(e.aliases, vec!["a", "b"]);
}

#[test]
fn h1_fallback_when_no_frontmatter_title() {
    let e = MarkdownExtractor.extract("n.md", "# Heading Title\n\nprose here.\n");
    assert_eq!(e.name, "Heading Title");
}

#[test]
fn filename_fallback_when_no_title() {
    let e = MarkdownExtractor.extract("my_file.md", "just prose\n");
    assert_eq!(e.name, "My File");
}

#[test]
fn crlf_frontmatter_body_is_clean() {
    // Same content as frontmatter_fields_parsed, but CRLF line endings.
    let src = "---\r\ntitle: My Note\r\naliases: foo, bar\r\ncreated: 2026-02-02\r\n---\r\n\r\nBody line one.\r\nBody line two.\r\n";
    let e = MarkdownExtractor.extract("n.md", src);
    assert_eq!(e.name, "My Note");
    assert_eq!(e.aliases, vec!["foo", "bar"]);
    assert_eq!(e.created, "2026-02-02");
    assert!(
        e.body.starts_with("Body line one."),
        "body should start at content, got: {:?}",
        e.body
    );
    assert!(
        !e.body.contains("---"),
        "closing delimiter leaked into body: {:?}",
        e.body
    );
}

fn updated_of(fm: &str) -> String {
    extract_fm(fm).updated
}

#[test]
fn key_order_updated_then_modified_then_date() {
    let all = "date: 2026-01-01\nmodified: 2026-01-02\nupdated: 2026-01-03";
    assert_eq!(updated_of(all), "2026-01-03");
    assert_eq!(
        updated_of("date: 2026-01-01\nmodified: 2026-01-02"),
        "2026-01-02"
    );
}

#[test]
fn metadata_block_one_level() {
    let t = "metadata:\n  type: user\n  modified: 2026-10-24T04:57:37.464Z";
    assert_eq!(updated_of(t), "2026-10-24");
    assert_eq!(
        updated_of("metadata:\n  sub:\n    modified: 2026-01-01"),
        ""
    );
}

#[test]
fn top_level_beats_metadata() {
    let t = "updated: 2026-03-03\nmetadata:\n  modified: 2026-04-04";
    assert_eq!(updated_of(t), "2026-03-03");
}

#[test]
fn quoted_date() {
    assert_eq!(updated_of("updated: \"2026-03-04\""), "2026-03-04");
}

#[test]
fn file_name_date() {
    let at = |path: &str, text: &str| MarkdownExtractor.extract(path, text).updated;
    assert_eq!(at("digests/2026-05-06.md", "# A\n"), "2026-05-06");
    assert_eq!(at("2026-05-06-notes.md", "# A\n"), "2026-05-06");
    let fm = "---\ndate: 2026-07-08\n---\n# A\n";
    assert_eq!(at("2026-05-06.md", fm), "2026-07-08");
}

#[test]
fn date_with_zone_keeps_own_date() {
    assert_eq!(
        updated_of("updated: 2026-10-23T23:59:00-05:00"),
        "2026-10-23"
    );
}

#[test]
fn invalid_dates_dropped() {
    for bad in [
        "2026-02-30",
        "2023-02-29",
        "2026-13-01",
        "2026-1-5",
        "20260101",
        "2026-10-231",
        "soon",
    ] {
        assert_eq!(updated_of(&format!("updated: {bad}")), "", "{bad}");
    }
    assert_eq!(updated_of("updated: 2024-02-29"), "2024-02-29");
}

#[test]
fn created_is_untouched() {
    let e = extract_fm("created: 2026-02-02");
    assert_eq!((e.created.as_str(), e.updated.as_str()), ("2026-02-02", ""));
}
