//! Tests for `model`.
use super::*;

#[test]
fn origin_label_covers_every_form() {
    let repo = |author: Option<&str>, edited| Origin::Repo {
        author: author.map(String::from),
        edited,
        mine: false,
    };
    assert_eq!(Origin::Local.label(), "local");
    assert_eq!(repo(None, false).label(), "repo");
    assert_eq!(repo(Some("Jane Doe"), false).label(), "repo, Jane Doe");
    assert_eq!(repo(None, true).label(), "repo, edited");
    assert_eq!(
        repo(Some("Jane Doe"), true).label(),
        "repo, Jane Doe, edited"
    );
    let mine = |edited| Origin::Repo {
        author: Some("Jane Doe".into()),
        edited,
        mine: true,
    };
    assert_eq!(mine(false).label(), "repo, Jane Doe, you");
    assert_eq!(mine(true).label(), "repo, Jane Doe, you, edited");
}

#[test]
fn origin_trust_scores() {
    let repo = |edited, mine| Origin::Repo {
        author: Some("Jane Doe".into()),
        edited,
        mine,
    };
    assert!((Origin::Local.trust() - 1.0).abs() < 1e-9);
    assert!((repo(false, true).trust() - 0.8).abs() < 1e-9);
    assert!((repo(true, false).trust() - 0.8).abs() < 1e-9);
    assert!((repo(false, false).trust() - 0.5).abs() < 1e-9);
    let anonymous = Origin::Repo {
        author: None,
        edited: false,
        mine: false,
    };
    assert!((anonymous.trust() - 0.5).abs() < 1e-9);
}

#[test]
fn slugify_folds_punctuation_and_unicode() {
    assert_eq!(
        slugify("AI News RSS Aggregator → Telegram"),
        "ai_news_rss_aggregator_telegram"
    );
    assert_eq!(slugify("a  --  b"), "a_b");
    assert_eq!(slugify("  →Foo→  "), "foo");
    assert_eq!(slugify("Rate: 5&6"), "rate_5_6");
    assert_eq!(slugify("café"), "cafe");
    // A `|` in a title MUST fold away — the id is both the `<id>.md`
    // filename and the `[[id|display]]` link target, and a pipe in either
    // would break the split-on-`|` link resolution.
    assert_eq!(slugify("Chapter 1 | Overview"), "chapter_1_overview");
    // 'İ' lowercases to `i` plus a combining dot, which is dropped.
    assert_eq!(slugify("İstanbul"), "istanbul");
}

#[test]
fn slugify_treats_decomposed_short_i_like_precomposed() {
    // `й` written as `и` + combining breve, as macOS tools write file names.
    assert_eq!(slugify("Морськии\u{306} маяк"), "morskyi_maiak");
    assert_eq!(slugify("МОРСЬКИИ\u{306} МАЯК"), "morskyi_maiak");
}

#[test]
fn slugify_transliterates_cyrillic() {
    for (name, id) in [
        ("Морський маяк", "morskyi_maiak"),
        ("Щука й Їжак", "shchuka_i_izhak"),
        ("Ґанок", "ganok"),
        ("Юля", "iulia"),
        ("Єва", "ieva"),
        ("Хата", "khata"),
        ("Цех", "tsekh"),
        ("Жук", "zhuk"),
        ("Чашка", "chashka"),
        ("Объём", "obem"),
        ("Мыло", "mylo"),
        ("Эхо", "ekho"),
        ("MCP-сервер v2", "mcp_server_v2"),
    ] {
        assert_eq!(slugify(name), id, "{name}");
    }
}

#[test]
fn slugify_drops_apostrophes_between_cyrillic_letters_only() {
    for name in ["пам'ять", "пам’ять", "памʼять", "'пам'ять'"] {
        assert_eq!(slugify(name), "pamiat", "{name}");
    }
    assert_eq!(slugify("don't"), "don_t");
    assert_eq!(slugify("Пам'ять агента"), "pamiat_ahenta");
}

#[test]
fn slugify_folds_latin_diacritics() {
    assert_eq!(slugify("Straße"), "strasse");
    assert_eq!(slugify("Łódź"), "lodz");
    assert_eq!(slugify("Ærø"), "aero");
    assert_eq!(slugify("naïve"), "naive");
}

#[test]
fn slugify_all_non_alnum_falls_back_to_hash() {
    for name in ["→→→", "—", "ьъ"] {
        let s = slugify(name);
        assert!(s.starts_with("page_"), "{name}: got {s}");
        assert_eq!(s.len(), "page_".len() + 16, "{name}");
    }
    let s = slugify("→→→");
    assert_eq!(s, slugify("→→→")); // deterministic
    assert_ne!(slugify("→→→"), slugify("★★★")); // distinct names → distinct slugs
}

#[test]
fn slugify_caps_long_slugs_with_a_hash_of_the_whole() {
    let fits = "a".repeat(MAX_ID_LEN);
    assert_eq!(slugify(&fits), fits); // a slug that fits is untouched
    let long = "word ".repeat(60);
    let s = slugify(&long);
    assert_eq!(s.len(), MAX_ID_LEN);
    assert!(s.starts_with("word_word_"), "{s}");
    assert_eq!(s, slugify(&long)); // deterministic
    assert_ne!(s, slugify(&format!("{long} x"))); // same prefix, distinct id
    assert_eq!(slugify(&s), s); // an id resolves to itself
}

#[test]
fn sourcekind_label_and_parse_roundtrip() {
    assert_eq!(SourceKind::Text.label(), "text");
    assert_eq!(
        SourceKind::Code {
            lang: "rust".into()
        }
        .label(),
        "code:rust"
    );
    assert_eq!(SourceKind::parse("markdown"), Some(SourceKind::Markdown));
    for k in [
        SourceKind::Text,
        SourceKind::Markdown,
        SourceKind::Code {
            lang: "rust".into(),
        },
    ] {
        assert_eq!(SourceKind::parse(&k.label()), Some(k));
    }
    assert_eq!(SourceKind::parse("pdf"), None);
}
