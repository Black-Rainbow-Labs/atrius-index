//! The symbol table: every definition the bundled language plugins can find, by name
//! (`tasks/task-2138-unluminous-code-index-tdd.md` §6.8).
//!
//! Built from the exact index's content, so it covers exactly the files a search covers, and held in
//! memory, so `search def` is a hash lookup. A name is looked up exactly, then without regard to case,
//! then by its parts, and the definitions found are ranked so the one an agent most likely meant is
//! first: a definition found by a definer keyword before one found by the brace heuristic, an exported
//! one before a private one, source before tests and generated files, a shallow path before a deep one.
//!
//! **Completion asks it three more things** (Unluminous `task-2231` §5.2): the members of a container
//! (`by_container`), the names that start with what has been typed (`by_name` is ordered, so that is a
//! range of it), and the names a stem reaches at a later word or by their initials (`word_starts` and
//! `humps`, the same kind of range). Each is a binary search and a walk of what matches, never a walk
//! of every name, so the cost of a keystroke does not grow with the project.

use std::collections::{BTreeMap, HashMap};

use rayon::prelude::*;

use crate::exact::{Exact, FileRecord};
use crate::outline::{self, Definition};

/// One definition and the file it is in.
#[derive(Clone, Debug)]
pub struct Defined {
    /// The file's path.
    pub path: String,
    /// The definition.
    pub definition: Definition,
}

/// Every definition, by lower case name, by file and by container, and every file's imports.
#[derive(Default)]
pub struct SymbolTable {
    by_name: BTreeMap<String, Vec<Defined>>,
    by_file: HashMap<String, Vec<Definition>>,
    /// A container's name and where each of its members is: the file and the index in `by_file`.
    by_container: HashMap<String, Vec<(String, usize)>>,
    /// Each lower case name under the part of it that starts at each later word: `layout_scroll` is
    /// under `scroll`, `selectionRectsIn` under `rectsin` and `in`.
    word_starts: BTreeMap<String, Vec<String>>,
    /// Each lower case name of two or more words under its initials: `layout_scroll_margin` under `lsm`.
    humps: BTreeMap<String, Vec<String>>,
    imports: HashMap<String, Vec<crate::structure::Import>>,
}

/// Where each word of a name starts, as character indices: the start, after a character that is not a
/// letter or a digit, at a lower case to capital step, at a letter to digit step, and at the last capital
/// of a run followed by a lower case letter (`HTTPServer` is `HTTP` and `Server`).
///
/// @param name - the name
pub fn word_starts_of(name: &str) -> Vec<usize> {
    let letters: Vec<char> = name.chars().collect();
    let mut starts = Vec::new();
    for (i, &c) in letters.iter().enumerate() {
        if !c.is_alphanumeric() {
            continue;
        }
        let starts_here = match i.checked_sub(1).map(|j| letters[j]) {
            None => true,
            Some(before) if !before.is_alphanumeric() => true,
            Some(before) if before.is_lowercase() && c.is_uppercase() => true,
            Some(before) if before.is_alphabetic() && c.is_ascii_digit() => true,
            Some(before) => {
                before.is_uppercase()
                    && c.is_uppercase()
                    && letters.get(i + 1).is_some_and(|n| n.is_lowercase())
            }
        };
        if starts_here {
            starts.push(i);
        }
    }
    starts
}

/// The keys a name is listed under in `word_starts` and `humps`, lower case.
///
/// @param name - the name
fn search_keys(name: &str) -> (Vec<String>, Option<String>) {
    let letters: Vec<char> = name.chars().collect();
    let starts = word_starts_of(name);
    let later: Vec<String> = starts
        .iter()
        .skip(1)
        .map(|&i| letters[i..].iter().collect::<String>().to_lowercase())
        .collect();
    let initials = (starts.len() >= 2)
        .then(|| starts.iter().map(|&i| letters[i]).collect::<String>().to_lowercase());
    (later, initials)
}

/// Whether a path is a test, generated or vendored rather than the project's own source.
///
/// @param path - the path
pub fn is_secondary(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    // A folder of examples, demonstrations or documentation at any depth. Measured on vite 6.0.0:
    // `createServer` is defined once in `packages/vite/src/node/server/index.ts` and again in eleven
    // `playground/*/server.js` files, which ranked ahead of it.
    const FOLDERS: &[&str] = &[
        "example", "examples", "playground", "playgrounds", "demo", "demos", "sample", "samples",
        "docs", "doc", "bench", "benches", "benchmark", "benchmarks", "third_party", "thirdparty",
        "node_modules", "spec", "specs", "__tests__", "testdata", "test-data",
    ];
    let mut folders = lower.split('/').rev().skip(1);
    if folders.any(|f| FOLDERS.contains(&f)) {
        return true;
    }
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
        let read: Vec<(String, Read)> = exact
            .files
            .par_iter()
            .flatten()
            .filter_map(|record| structure_of(record).map(|read| (record.rel.clone(), read)))
            .collect();
        let mut table = SymbolTable::default();
        for (path, read) in read {
            table.set_file_with_imports(&path, read.definitions, read.imports);
        }
        table
    }

    /// Replaces one file's definitions, as after the gate read it again.
    ///
    /// @param path - the file's path
    /// @param defs - its definitions now
    pub fn set_file(&mut self, path: &str, defs: Vec<Definition>) {
        self.set_file_with_imports(path, defs, Vec::new());
    }

    /// Replaces one file's definitions and imports.
    ///
    /// @param path - the file's path
    /// @param defs - its definitions now
    /// @param imports - its imports now
    pub fn set_file_with_imports(
        &mut self,
        path: &str,
        defs: Vec<Definition>,
        imports: Vec<crate::structure::Import>,
    ) {
        self.remove_file(path);
        for (at, d) in defs.iter().enumerate() {
            let key = d.name.to_lowercase();
            let fresh = !self.by_name.contains_key(&key);
            self.by_name
                .entry(key.clone())
                .or_default()
                .push(Defined { path: path.to_owned(), definition: d.clone() });
            if fresh {
                let (later, initials) = search_keys(&d.name);
                for part in later {
                    self.word_starts.entry(part).or_default().push(key.clone());
                }
                if let Some(initials) = initials {
                    self.humps.entry(initials).or_default().push(key.clone());
                }
            }
            if let Some(container) = &d.container {
                self.by_container.entry(container.clone()).or_default().push((path.to_owned(), at));
            }
        }
        if !defs.is_empty() {
            self.by_file.insert(path.to_owned(), defs);
        }
        if !imports.is_empty() {
            self.imports.insert(path.to_owned(), imports);
        }
    }

    /// Takes one file's definitions and imports out.
    ///
    /// @param path - the file's path
    pub fn remove_file(&mut self, path: &str) {
        self.imports.remove(path);
        let Some(old) = self.by_file.remove(path) else { return };
        for d in old {
            if let Some(container) = &d.container {
                if let Some(list) = self.by_container.get_mut(container) {
                    list.retain(|(p, _)| p != path);
                    if list.is_empty() {
                        self.by_container.remove(container);
                    }
                }
            }
            let key = d.name.to_lowercase();
            let Some(list) = self.by_name.get_mut(&key) else { continue };
            list.retain(|x| x.path != path);
            if !list.is_empty() {
                continue;
            }
            self.by_name.remove(&key);
            let (later, initials) = search_keys(&d.name);
            for part in later.into_iter().chain(initials) {
                for map in [&mut self.word_starts, &mut self.humps] {
                    if let Some(names) = map.get_mut(&part) {
                        names.retain(|n| n != &key);
                        if names.is_empty() {
                            map.remove(&part);
                        }
                    }
                }
            }
        }
    }

    /// The members of a container: every definition whose block is the container's, from every file
    /// that has one.
    ///
    /// @param container - the container's name, as written
    pub fn members_of<'a>(
        &'a self,
        container: &str,
    ) -> impl Iterator<Item = (&'a str, &'a Definition)> + 'a {
        self.by_container
            .get(container)
            .into_iter()
            .flatten()
            .filter_map(|(path, at)| Some((path.as_str(), self.by_file.get(path)?.get(*at)?)))
    }

    /// The definitions of a lower case name, exactly.
    ///
    /// @param lower - the name, lower case
    pub fn named(&self, lower: &str) -> &[Defined] {
        self.by_name.get(lower).map_or(&[], Vec::as_slice)
    }

    /// The lower case names that start with a lower case prefix, in order, each with its definitions.
    ///
    /// @param prefix - the prefix, lower case
    pub fn prefixed<'a>(
        &'a self,
        prefix: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a [Defined])> + 'a {
        let from = (std::ops::Bound::Included(prefix), std::ops::Bound::Unbounded);
        self.by_name
            .range::<str, _>(from)
            .take_while(move |(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.as_str(), v.as_slice()))
    }

    /// The lower case names with a later word that starts with a lower case prefix: `scroll` reaches
    /// `layout_scroll_margin`.
    ///
    /// @param prefix - the prefix, lower case
    pub fn word_started<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        ranged(&self.word_starts, prefix)
    }

    /// The lower case names whose initials start with a lower case prefix: `lsm` reaches
    /// `layout_scroll_margin`.
    ///
    /// @param prefix - the prefix, lower case
    pub fn humped<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        ranged(&self.humps, prefix)
    }

    /// A file's imports, in the order they are written.
    ///
    /// @param path - the file's path
    pub fn imports_of(&self, path: &str) -> &[crate::structure::Import] {
        self.imports.get(path).map_or(&[], Vec::as_slice)
    }

    /// Every file that defines something, with its definitions.
    pub fn files(&self) -> impl Iterator<Item = (&str, &[Definition])> {
        self.by_file.iter().map(|(p, d)| (p.as_str(), d.as_slice()))
    }

    /// Every file's imports.
    pub fn all_imports(&self) -> impl Iterator<Item = (&str, &[crate::structure::Import])> {
        self.imports.iter().map(|(p, i)| (p.as_str(), i.as_slice()))
    }

    /// A copy of every file's definitions and imports, for the store to write with no lock held.
    #[allow(clippy::type_complexity)]
    pub fn rows(&self) -> Vec<(String, Vec<Definition>, Vec<crate::structure::Import>)> {
        let mut paths: Vec<&String> = self.by_file.keys().chain(self.imports.keys()).collect();
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .map(|p| {
                let defs = self.by_file.get(p).cloned().unwrap_or_default();
                (p.clone(), defs, self.imports.get(p).cloned().unwrap_or_default())
            })
            .collect()
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

/// The lower case names under every key of a map that starts with a prefix.
///
/// @param map - `word_starts` or `humps`
/// @param prefix - the prefix, lower case
fn ranged<'a>(
    map: &'a BTreeMap<String, Vec<String>>,
    prefix: &'a str,
) -> impl Iterator<Item = &'a str> + 'a {
    let from = (std::ops::Bound::Included(prefix), std::ops::Bound::Unbounded);
    map.range::<str, _>(from)
        .take_while(move |(k, _)| k.starts_with(prefix))
        .flat_map(|(_, names)| names.iter().map(String::as_str))
}

/// One file's listed definitions and its imports.
pub struct Read {
    pub definitions: Vec<Definition>,
    pub imports: Vec<crate::structure::Import>,
}

/// One file's definitions, when a bundled language claims it and it is not too large to be source.
///
/// @param record - the file
pub fn definitions_of(record: &FileRecord) -> Option<Vec<Definition>> {
    structure_of(record).map(|read| read.definitions)
}

/// One file's listed definitions and imports, when a language claims it and it is not too large to be
/// source.
///
/// @param record - the file
pub fn structure_of(record: &FileRecord) -> Option<Read> {
    if record.binary || record.size > 1_048_576 {
        return None;
    }
    let language = crate::grammars::for_path(&record.rel)?;
    if !language.grammar.defines_symbols() {
        return None;
    }
    let bytes = record.bytes();
    let text = String::from_utf8_lossy(&bytes);
    Some(read_text(&record.rel, &text))
}

/// The listed definitions and imports of a file's text, read with the language its path names.
///
/// What the window reads an open tab's live text with, so an open file and a closed one are read by the
/// same rules.
///
/// @param rel - the file's path, which decides its language
/// @param text - its text
pub fn read_text(rel: &str, text: &str) -> Read {
    let (definitions, imports) = outline::read_structure(rel, text);
    Read { definitions: definitions.into_iter().filter(outline::is_listed).collect(), imports }
}
