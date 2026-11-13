//! `wiki-code`: the code layer shared by the wiki compiler and its sibling tools: the item model, the source tree walk, tree-sitter code extraction, the candidate index calls resolve against, and import resolution.
//!
//! What a second consumer may rely on (the contract):
//!
//! - [`extract`], [`handles`], [`CodeFile`], [`CodeUnit`] and
//!   [`pair_declarations`]: one file's items, and the C/C++ pairing across
//!   files;
//! - [`analyze()`] with [`Analyzed`], [`Source`], [`Def`] and [`Call`]: the same
//!   items with the trees they came from, each item's node, the calls under a
//!   node with their literal text, and comment text;
//! - [`model`]: the item types;
//! - [`walk`]: the source tree walk;
//! - [`index`]: the candidate index a call site resolves against
//!   ([`index::Index`], [`index::Scope`], [`index::Family`], [`index::FnRef`],
//!   [`index::sees`], [`index::owner_key`]);
//! - [`imports`]: import resolution ([`imports::Imports`]);
//! - [`tree_sitter`], re-exported at the version the grammars are pinned to.
//!
//! [`support`] is not part of the contract: it holds text helpers the wiki
//! compiler shares with this crate and may change with the compiler.

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
