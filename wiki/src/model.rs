use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    Text,
    Markdown,
    Code { lang: String },
    Pdf,
    Image,
    Audio,
}

impl SourceKind {
    pub fn label(&self) -> String {
        match self {
            SourceKind::Text => "text".into(),
            SourceKind::Markdown => "markdown".into(),
            SourceKind::Code { lang } => format!("code:{lang}"),
            SourceKind::Pdf => "pdf".into(),
            SourceKind::Image => "image".into(),
            SourceKind::Audio => "audio".into(),
        }
    }

    pub fn parse(s: &str) -> Option<SourceKind> {
        match s {
            "text" => Some(SourceKind::Text),
            "markdown" => Some(SourceKind::Markdown),
            "pdf" => Some(SourceKind::Pdf),
            "image" => Some(SourceKind::Image),
            "audio" => Some(SourceKind::Audio),
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

impl LintReport {
    pub fn is_clean(&self) -> bool {
        self.broken_links.is_empty() && self.orphans.is_empty()
    }
}

pub fn slugify(name: &str) -> String {
    name.trim().to_lowercase().replace([' ', '-'], "_")
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
