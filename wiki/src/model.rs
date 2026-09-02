//! Core data types: `SourceKind`, `Entity`, `Graph`, `LintReport`, plus `slugify` for page ids and path normalisation.

use crate::hash::{hash_str, to_hex};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    Text,
    Markdown,
    Code { lang: String },
}

impl SourceKind {
    pub fn label(&self) -> String {
        match self {
            SourceKind::Text => "text".into(),
            SourceKind::Markdown => "markdown".into(),
            SourceKind::Code { lang } => format!("code:{lang}"),
        }
    }

    pub fn parse(s: &str) -> Option<SourceKind> {
        match s {
            "text" => Some(SourceKind::Text),
            "markdown" => Some(SourceKind::Markdown),
            other => other.strip_prefix("code:").map(|lang| SourceKind::Code {
                lang: lang.to_string(),
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub created: String,
    pub body: String,
    pub source_path: String,
    pub kind: SourceKind,
    pub content_hash: [u8; 32],
    pub summary: Option<String>,
    pub symbols: Vec<String>,
    pub imports: Vec<String>,
}

#[derive(Default, Clone, Debug)]
pub struct Edges {
    pub outgoing: BTreeSet<String>,
    pub incoming: BTreeSet<String>,
}

#[derive(Default, Clone, Debug)]
pub struct Graph {
    pub edges: BTreeMap<String, Edges>,
    pub pagerank: BTreeMap<String, f64>,
}

#[derive(Default, Clone, Debug)]
pub struct LintReport {
    pub total_pages: usize,
    pub broken_links: Vec<(String, String)>,
    pub orphans: Vec<String>,
}

/// A filesystem/URL-clean id: lowercase, every run of non-`[a-z0-9]` collapsed
/// to a single `_`, no leading/trailing `_`. A name that is entirely non-alnum
/// falls back to a deterministic `page_<hash>` so it is never empty (and never
/// silently dropped by slug dedup). Uses `char::to_ascii_lowercase` (ASCII-only,
/// no Unicode case-expansion) and a single pass (no `String::replace` loop).
/// Capitalize each whitespace-separated word and lowercase the rest of it.
/// Shared by every name-deriving path — the text extractor's ALL-CAPS first
/// line, the markdown/code filename fallbacks, and `disambiguate_ids`'
/// directory prefixes — so that a page named from a path and a page named
/// from its own content are cased by one rule rather than three copies of it.
pub(crate) fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut ch = w.chars();
            match ch.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &ch.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending_sep = false;
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('_');
            }
            out.push(c);
            pending_sep = false;
        } else {
            pending_sep = true; // a run of non-alnum → at most one '_', added lazily
        }
    }
    if out.is_empty() {
        format!("page_{}", &to_hex(&hash_str(name))[..16])
    } else {
        out
    }
}

/// Repo-relative, forward-slash-normalized path string. Falls back to the raw
/// path (lossy) if `path` is not under `root`.
pub fn normalize_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn slugify_lowercases_and_replaces_separators() {
        assert_eq!(slugify("Gradient Descent"), "gradient_descent");
        assert_eq!(slugify("Top-K Sampling"), "top_k_sampling");
        assert_eq!(slugify("KV Cache"), "kv_cache");
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
        assert_eq!(slugify("café"), "caf");
        // A `|` in a title MUST fold away — the id is both the `<id>.md`
        // filename and the `[[id|display]]` link target, and a pipe in either
        // would break Task 1's split-on-`|` resolution (Task 1 review finding).
        assert_eq!(slugify("Chapter 1 | Overview"), "chapter_1_overview");
        // 'İ' (U+0130) is non-ASCII; ASCII-only lowercasing leaves it, then it
        // folds to a single leading separator that is suppressed.
        assert_eq!(slugify("İstanbul"), "stanbul");
    }

    #[test]
    fn slugify_all_non_alnum_falls_back_to_hash() {
        let s = slugify("→→→");
        assert!(s.starts_with("page_"), "got {s}");
        assert_eq!(s.len(), "page_".len() + 16);
        assert_eq!(s, slugify("→→→")); // deterministic
        assert_ne!(slugify("→→→"), slugify("★★★")); // distinct names → distinct slugs
    }

    #[test]
    fn normalize_path_is_relative_and_forward_slashed() {
        let root = Path::new("/tmp/proj");
        let p = Path::new("/tmp/proj/src/graph.rs");
        assert_eq!(normalize_path(root, p), "src/graph.rs");
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
    }
}
