//! Atrius: a code search index for agents and people.
//!
//! One index per checkout, kept in one Inillucent file outside the repository, owned by one host
//! process that also watches the files. It answers four kinds of question: exact text and regex with
//! the same results as ripgrep (`exact`), where a name is defined and used (`symbols`), questions written
//! in plain English (`passages`), and "show me just this function" (`outline`).
//!
//! The `atrius` command line and the `atrius-mcp` server both reach an index through [`host::ask`], and
//! both answer through [`verbs::answer`], so a search typed at a terminal and a search an agent makes are
//! the same function. `docs/architecture.md` in the repository describes the design.

pub mod direct;
pub mod exact;
pub mod files;
pub mod freshness;
pub mod grammars;
pub mod host;
pub mod index;
pub mod lang;
pub mod outline;
pub mod passages;
pub mod paths;
pub mod plan;
pub mod protocol;
pub mod shape;
pub mod store;
pub mod structure;
pub mod symbols;
pub mod trigram;
pub mod verbs;
pub mod words;

/// The version of this crate, which the command line, the MCP server and the host file report.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
