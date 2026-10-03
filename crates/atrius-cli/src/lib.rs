//! The `atrius` command line and the `atrius-mcp` server.
//!
//! Both answer queries through `atrius_index::host::ask` and therefore through
//! `atrius_index::verbs::answer`, so the command line and an agent see the same text. Every command is
//! listed once, in [`catalogue`].

pub mod catalogue;
pub mod daemon;
pub mod docs;
pub mod doctor;
pub mod mcp;
pub mod parse;
pub mod process;
pub mod run;
pub mod service;
pub mod watchlist;
