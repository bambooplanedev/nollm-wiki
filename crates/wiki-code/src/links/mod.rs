#![warn(clippy::wildcard_enum_match_arm)]
//! Per-function flows and call targets of a codebase: the link extraction
//! plaz draws its charts from.

pub mod flow;
pub mod model;
pub mod resolve;
pub mod source;
mod uses;
