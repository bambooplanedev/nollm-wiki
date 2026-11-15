//! The code page: `wiki_code::extract`'s result as an `Entity`.

use crate::formats::Extractor;
use crate::model::{slugify, Entity, SourceKind};
pub(crate) use wiki_code::support::{is_shell, module_stem, source_rank, validate_queries};

pub struct CodeExtractor;

impl Extractor for CodeExtractor {
    fn extensions(&self) -> &[&str] {
        wiki_code::support::CODE_EXTENSIONS
    }

    fn extract(&self, rel_path: &str, text: &str) -> Entity {
        let f = wiki_code::extract(rel_path, text);
        Entity {
            id: slugify(&f.name),
            name: f.name,
            body: f.body,
            kind: SourceKind::Code { lang: f.lang },
            summary: f.summary,
            symbols: f.symbols,
            imports: f.imports,
            defined: f.defined,
            methods: f.methods,
            items: f.items,
            ..Entity::default()
        }
    }
}

#[cfg(test)]
mod tests;
