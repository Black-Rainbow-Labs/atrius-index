//! The symbol table: every definition the bundled language plugins can find, by name
//! (`tasks/task-2138-unluminous-code-index-tdd.md` §6.8).
//!
//! Built from the exact index's content, so it covers exactly the files a search covers, and held in
//! memory, so `search def` is a hash lookup. A name is looked up exactly, then without regard to case,
//! then by its parts, and the definitions found are ranked so the one an agent most likely meant is
//! first: a definition found by a definer keyword before one found by the brace heuristic, an exported
//! one before a private one, source before tests and generated files, a shallow path before a deep one.

use std::collections::HashMap;

use rayon::prelude::*;

use crate::exact::{Exact, FileRecord};
use crate::outline::{self, Definition, CHUNK_BUDGET};

/// One definition and the file it is in.
#[derive(Clone, Debug)]
pub struct Defined {
    /// The file's path.
    pub path: String,
    /// The definition.
    pub definition: Definition,
}

/// Every definition, by lower case name.
#[derive(Default)]
pub struct SymbolTable {
    by_name: HashMap<String, Vec<Defined>>,
    by_file: HashMap<String, Vec<Definition>>,
}

/// Whether a path is a test, generated or vendored rather than the project's own source.
///
/// @param path - the path
pub fn is_secondary(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/test")
        || lower.starts_with("test")
        || lower.contains("tests/")
        || lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.contains("_test.")
        || lower.contains("/examples/")
        || lower.contains("/benches/")
        || lower.contains(".min.")
        || lower.contains("/dist/")
        || lower.contains("/vendor/")
        || lower.contains("generated")
        || lower.contains("/fixtures/")
}

impl SymbolTable {
    /// Reads the definitions of every file the exact index holds, in parallel.
    ///
    /// @param exact - the exact index
    pub fn build(exact: &Exact) -> SymbolTable {
        let read: Vec<(String, Vec<Definition>)> = exact
            .files
            .par_iter()
            .flatten()
            .filter_map(|record| definitions_of(record).map(|defs| (record.rel.clone(), defs)))
            .collect();
        let mut table = SymbolTable::default();
        for (path, defs) in read {
            table.set_file(&path, defs);
        }
        table
    }

    /// Replaces one file's definitions, as after the gate read it again.
    ///
    /// @param path - the file's path
    /// @param defs - its definitions now
    pub fn set_file(&mut self, path: &str, defs: Vec<Definition>) {
        self.remove_file(path);
        for d in &defs {
            self.by_name
                .entry(d.name.to_lowercase())
                .or_default()
                .push(Defined { path: path.to_owned(), definition: d.clone() });
        }
        if !defs.is_empty() {
            self.by_file.insert(path.to_owned(), defs);
        }
    }

    /// Takes one file's definitions out.
    ///
    /// @param path - the file's path
    pub fn remove_file(&mut self, path: &str) {
        if let Some(old) = self.by_file.remove(path) {
            for d in old {
                let key = d.name.to_lowercase();
                if let Some(list) = self.by_name.get_mut(&key) {
                    list.retain(|x| x.path != path);
                    if list.is_empty() {
                        self.by_name.remove(&key);
                    }
                }
            }
        }
    }

    /// A file's definitions, in line order.
    ///
    /// @param path - the file's path
    pub fn of_file(&self, path: &str) -> &[Definition] {
        self.by_file.get(path).map_or(&[], Vec::as_slice)
    }

    /// How many names are defined.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether nothing is defined.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// The definitions of a name, best first: the exact spelling, then any case, then names whose parts
    /// hold it, each ranked.
    ///
    /// @param name - the name asked for
    /// @param limit - how many to return
    pub fn lookup(&self, name: &str, limit: usize) -> Vec<Defined> {
        let lower = name.to_lowercase();
        let mut found: Vec<Defined> = self.by_name.get(&lower).cloned().unwrap_or_default();
        if found.is_empty() && lower.len() >= 3 {
            let mut partial: Vec<&Defined> = self
                .by_name
                .iter()
                .filter(|(k, _)| k.contains(&lower))
                .flat_map(|(_, v)| v.iter())
                .collect();
            partial.sort_by_key(|d| d.definition.name.len());
            found = partial.into_iter().take(limit * 4).cloned().collect();
        }
        found.sort_by_key(|d| rank(d, name));
        found.truncate(limit);
        found
    }
}

/// The ranking key of a definition for a name: lower is better.
///
/// @param d - the definition
/// @param asked - the name as asked
fn rank(d: &Defined, asked: &str) -> (u8, u8, u8, u8, u8, usize, u32) {
    let def = &d.definition;
    (
        u8::from(def.name != asked),
        u8::from(def.name.to_lowercase() != asked.to_lowercase()),
        u8::from(is_secondary(&d.path)),
        u8::from(def.likely),
        match def.kind {
            "type" | "function" => 0,
            "module" => 1,
            "constant" => 2,
            _ => 3,
        },
        d.path.matches('/').count(),
        def.depth,
    )
}

/// One file's definitions, when a bundled language claims it and it is not too large to be source.
///
/// @param record - the file
pub fn definitions_of(record: &FileRecord) -> Option<Vec<Definition>> {
    if record.binary || record.size > 1_048_576 {
        return None;
    }
    let language = crate::grammars::for_path(&record.rel)?;
    if !language.grammar.defines_symbols() {
        return None;
    }
    let bytes = record.bytes();
    let text = String::from_utf8_lossy(&bytes);
    Some(
        outline::read(&record.rel, &text, CHUNK_BUDGET)
            .definitions
            .into_iter()
            .filter(outline::is_listed)
            .collect(),
    )
}
