pub mod code;
pub mod extract_python;
pub mod extract_rust;
pub mod extract_simple;
pub mod markdown;
pub mod summary;
pub mod text;

pub use code::CodeExtractor;
pub use markdown::MarkdownExtractor;
pub use summary::summarize;
pub use text::TextExtractor;

use crate::hash::hash_bytes;
use crate::model::Entity;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A page name derived from the file path: the basename's stem, with `_` and
/// `-` turned into spaces, title-cased.
///
/// Shared by `CodeExtractor` and `MarkdownExtractor`, which each carried a
/// private copy. The copies had already drifted — markdown's replaced only
/// `_`, so `my-notes.md` rendered "My-notes" while `my-code.rs` rendered
/// "My Code". Cosmetic (both slugify to one id, and the phrase index
/// tokenizes them identically), but it is the drift a verbatim duplicate
/// invites.
pub(crate) fn derive_name_from_path(rel_path: &str) -> String {
    let base = rel_path.rsplit('/').next().unwrap_or(rel_path);
    let stem = base.split('.').next().unwrap_or(base);
    crate::model::title_case(&stem.replace(['_', '-'], " "))
}

/// Every extractor turns already-decoded text into a semantic `Entity`.
/// `source_path` and `content_hash` are filled by the `Registry`, not the extractor.
pub trait Extractor: Send + Sync {
    fn extensions(&self) -> &[&str];
    fn extract(&self, rel_path: &str, text: &str) -> Entity;
}

pub struct Registry {
    by_ext: BTreeMap<String, Arc<dyn Extractor>>,
}

impl Registry {
    pub fn new() -> Self {
        Registry {
            by_ext: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, extractor: Arc<dyn Extractor>) {
        for ext in extractor.extensions() {
            self.by_ext.insert((*ext).to_string(), extractor.clone());
        }
    }

    pub fn with_defaults() -> Self {
        let mut reg = Registry::new();
        reg.register(Arc::new(TextExtractor));
        reg.register(Arc::new(MarkdownExtractor));
        reg.register(Arc::new(CodeExtractor));
        reg
    }

    fn ext_of(rel_path: &str) -> Option<String> {
        rel_path
            .rsplit('.')
            .next()
            .filter(|e| *e != rel_path)
            .map(|e| e.to_lowercase())
    }

    /// Decode bytes lossily, dispatch by extension, fill source_path + content_hash.
    pub fn extract(&self, rel_path: &str, bytes: &[u8]) -> Option<Entity> {
        let ext = Self::ext_of(rel_path)?;
        let extractor = self.by_ext.get(&ext)?;
        let text = String::from_utf8_lossy(bytes);
        let mut entity = extractor.extract(rel_path, &text);
        entity.source_path = rel_path.to_string();
        entity.content_hash = hash_bytes(bytes);
        Some(entity)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::with_defaults()
    }
}
