//! `wiki-code`: the code layer shared by the wiki compiler and its sibling tools: the item model, the source tree walk, tree-sitter code extraction, the candidate index calls resolve against, and import resolution.

pub mod analyze;
pub mod imports;
pub mod index;
pub mod model;
pub mod support;
pub mod walk;

mod formats {
    pub(crate) mod code;
    pub(crate) mod extract_cpp;
    pub(crate) mod extract_jsx;
    pub(crate) mod extract_python;
    pub(crate) mod extract_rust;
    pub(crate) mod extract_shell;
    pub(crate) mod extract_simple;
    pub(crate) mod extract_svelte;
    pub(crate) mod extract_vue;
}
mod summary;

pub use analyze::{analyze, Analyzed, Call, Def, Source};
pub use formats::code::{extract, handles, CodeFile, CodeUnit};
pub use formats::extract_cpp::pair_declarations;
pub use tree_sitter;
