//! `wiki-code`: the code layer shared by the wiki compiler and its sibling tools: the item model and the source tree walk.

pub mod model;
pub mod support;
pub mod walk;

mod summary;

pub use tree_sitter;
