//! How the index reads a language: the manifest format, the tokeniser, definitions and folding.
//!
//! These four modules started as copies of the same modules in `unluminous-core`, the editor that the
//! index was first built for. They are kept here so the index has no dependency on the editor. Each
//! language is described by a manifest in `languages/` at the root of this crate, in the same
//! `name = value` format the editor's language plugins use, so a manifest written for one reads in the
//! other. Comments in these files still say "Unluminous" where they describe the editor's behaviour.

pub mod folding;
pub mod manifest;
pub mod symbols;
pub mod syntax;
