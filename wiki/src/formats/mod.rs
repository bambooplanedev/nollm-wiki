pub mod summary;
pub mod text;

pub use summary::summarize;
pub use text::TextExtractor;

use crate::hash::hash_bytes;
use crate::model::Entity;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("parse error in {path}: {msg}")]
    Parse { path: String, msg: String },
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
        // Task 6 adds MarkdownExtractor; Task 7 adds CodeExtractor.
        // Feature seams (pdf/ocr/audio) register here under #[cfg(feature = ...)].
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
