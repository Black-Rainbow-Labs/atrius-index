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
use std::sync::Arc;

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

/// One definition and the file it is in, borrowed from the table: what [`SymbolTable::named`] and
/// [`SymbolTable::prefixed`] hand out, so a lookup copies nothing.
#[derive(Clone, Copy, Debug)]
pub struct DefinedRef<'a> {
    /// The file's path.
    pub path: &'a str,
    /// The definition.
    pub definition: &'a Definition,
}

impl DefinedRef<'_> {
    /// A copy that outlives the table's lock.
    pub fn to_owned(self) -> Defined {
        Defined { path: self.path.to_owned(), definition: self.definition.clone() }
    }
}

/// Where a definition is kept: its file, shared with every other place in that file, and its index in
/// the file's list.
#[derive(Clone, Debug)]
struct Place {
    path: Arc<str>,
    at: u32,
}

/// Every definition, by lower case name, by file and by container, and every file's imports.
///
/// **Each definition is held once** (Unluminous `task-2279`), in `by_file`. The other maps hold where it
/// is, and a name is one allocation shared by `by_name`, `word_starts` and `humps`. Before, every
/// definition was cloned into `by_name` with its own copy of its path and every name was copied into
/// each word it starts, and the table was 62.6 MB for 28,979 definitions whose own text is 10.7 MB.
#[derive(Default)]
pub struct SymbolTable {
    by_name: BTreeMap<Arc<str>, Vec<Place>>,
    by_file: HashMap<Arc<str>, Vec<Definition>>,
    /// A container's name and where each of its members is.
    by_container: HashMap<Box<str>, Vec<Place>>,
    /// Each lower case name under the part of it that starts at each later word: `layout_scroll` is
    /// under `scroll`, `selectionRectsIn` under `rectsin` and `in`.
    word_starts: Listings,
    /// Each lower case name of two or more words under its initials: `layout_scroll_margin` under `lsm`.
    humps: Listings,
    imports: HashMap<Arc<str>, Vec<crate::structure::Import>>,
}

/// A key a name is listed under, held without an allocation of its own wherever it can be.
#[derive(Clone, Debug)]
enum Key {
    /// The lower case name itself from this byte on, which is what a later word's key always is.
    Tail(u32),
    /// A short key, held in place: the bytes and how many of them there are.
    Inline([u8; 22], u8),
    /// Any other key.
    Owned(Box<str>),
}

/// One name under one key.
#[derive(Clone, Debug)]
struct Listing {
    name: Arc<str>,
    key: Key,
}

impl Listing {
    /// The key's text.
    fn key(&self) -> &str {
        match &self.key {
            Key::Tail(at) => self.name.get(*at as usize..).unwrap_or(""),
            Key::Inline(bytes, len) => std::str::from_utf8(&bytes[..*len as usize]).unwrap_or(""),
            Key::Owned(text) => text,
        }
    }
}

/// Names under keys, in one list sorted by key (Unluminous `task-2279`).
///
/// It replaced two `BTreeMap<String, Vec<String>>`, which made two allocations for every key and one
/// for every name under it: about 80,000 small allocations on a 2,000 file checkout, each with the
/// allocator's own header beside it. A key under a later word is the tail of the lower case name, so it
/// is held as where the tail starts, and an initials key is short enough to hold in place.
///
/// The list is sorted once after a whole project is read ([`Listings::settle`]); a name added after that
/// is put in its place. A list that has not been settled is still answered correctly, by reading all of
/// it, so nothing depends on a caller remembering to settle it.
#[derive(Default)]
struct Listings {
    entries: Vec<Listing>,
    sorted: bool,
}

impl Listings {
    /// Lists a name under a key.
    ///
    /// @param name - the lower case name
    /// @param key - its key
    fn add(&mut self, name: &Arc<str>, key: &str) {
        let lower_before = name.len().saturating_sub(key.len());
        let key = if name.get(lower_before..) == Some(key) {
            Key::Tail(lower_before as u32)
        } else if key.len() <= 22 {
            let mut bytes = [0u8; 22];
            bytes[..key.len()].copy_from_slice(key.as_bytes());
            Key::Inline(bytes, key.len() as u8)
        } else {
            Key::Owned(key.into())
        };
        let listing = Listing { name: Arc::clone(name), key };
        if self.sorted {
            let at = self.entries.partition_point(|e| (e.key(), &*e.name) < (listing.key(), &*listing.name));
            self.entries.insert(at, listing);
        } else {
            self.entries.push(listing);
        }
    }

    /// Takes every listing of these names out.
    ///
    /// @param names - the lower case names
    fn remove(&mut self, names: &std::collections::HashSet<&str>) {
        if !names.is_empty() {
            self.entries.retain(|e| !names.contains(&*e.name));
        }
    }

    /// Sorts the list and gives back the room it was grown into.
    fn settle(&mut self) {
        if !self.sorted {
            self.entries.sort_unstable_by(|a, b| (a.key(), &*a.name).cmp(&(b.key(), &*b.name)));
            self.sorted = true;
        }
        self.entries.shrink_to_fit();
    }

    /// The names under every key that starts with a prefix.
    ///
    /// @param prefix - the prefix, lower case
    fn starting_with<'a>(&'a self, prefix: &'a str) -> Box<dyn Iterator<Item = &'a str> + 'a> {
        if !self.sorted {
            return Box::new(
                self.entries.iter().filter(move |e| e.key().starts_with(prefix)).map(|e| &*e.name),
            );
        }
        let from = self.entries.partition_point(|e| e.key() < prefix);
        Box::new(
            self.entries[from..]
                .iter()
                .take_while(move |e| e.key().starts_with(prefix))
                .map(|e| &*e.name),
        )
    }
}

/// The definitions of one name, borrowed from the table, as [`SymbolTable::named`] answers.
#[derive(Clone)]
pub struct Named<'a> {
    table: &'a SymbolTable,
    places: std::slice::Iter<'a, Place>,
}

impl<'a> Iterator for Named<'a> {
    type Item = DefinedRef<'a>;

    fn next(&mut self) -> Option<DefinedRef<'a>> {
        for place in self.places.by_ref() {
            if let Some(found) = self.table.place(place) {
                return Some(found);
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.places.size_hint()
    }
}

impl ExactSizeIterator for Named<'_> {}

impl Named<'_> {
    /// Whether the name has no definitions.
    pub fn is_empty(&self) -> bool {
        self.places.len() == 0
    }
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
        table.shrink();
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
        let shared: Arc<str> = Arc::from(path);
        for (at, d) in defs.iter().enumerate() {
            let place = Place { path: Arc::clone(&shared), at: at as u32 };
            let lower = d.name.to_lowercase();
            match self.by_name.get_mut(lower.as_str()) {
                Some(places) => places.push(place),
                None => {
                    let key: Arc<str> = Arc::from(lower);
                    let (later, initials) = search_keys(&d.name);
                    for part in later {
                        self.word_starts.add(&key, &part);
                    }
                    if let Some(initials) = initials {
                        self.humps.add(&key, &initials);
                    }
                    self.by_name.insert(key, vec![place]);
                }
            }
            if let Some(container) = &d.container {
                let place = Place { path: Arc::clone(&shared), at: at as u32 };
                match self.by_container.get_mut(container.as_str()) {
                    Some(places) => places.push(place),
                    None => {
                        self.by_container.insert(container.as_str().into(), vec![place]);
                    }
                }
            }
        }
        if !defs.is_empty() {
            let mut defs = defs;
            defs.shrink_to_fit();
            self.by_file.insert(Arc::clone(&shared), defs);
        }
        if !imports.is_empty() {
            self.imports.insert(shared, imports);
        }
    }

    /// Takes one file's definitions and imports out.
    ///
    /// @param path - the file's path
    pub fn remove_file(&mut self, path: &str) {
        self.imports.remove(path);
        let Some(old) = self.by_file.remove(path) else { return };
        let mut gone: Vec<String> = Vec::new();
        for d in old {
            if let Some(container) = &d.container {
                if let Some(list) = self.by_container.get_mut(container.as_str()) {
                    list.retain(|p| &*p.path != path);
                    if list.is_empty() {
                        self.by_container.remove(container.as_str());
                    }
                }
            }
            let key = d.name.to_lowercase();
            let Some(list) = self.by_name.get_mut(key.as_str()) else { continue };
            list.retain(|x| &*x.path != path);
            if !list.is_empty() {
                continue;
            }
            self.by_name.remove(key.as_str());
            gone.push(key);
        }
        let gone: std::collections::HashSet<&str> = gone.iter().map(String::as_str).collect();
        self.word_starts.remove(&gone);
        self.humps.remove(&gone);
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
            .filter_map(|place| self.place(place).map(|found| (found.path, found.definition)))
    }

    /// The definition a place names, if its file still holds it.
    ///
    /// @param place - the place
    fn place(&self, place: &Place) -> Option<DefinedRef<'_>> {
        let (path, defs) = self.by_file.get_key_value(&*place.path)?;
        Some(DefinedRef { path, definition: defs.get(place.at as usize)? })
    }

    /// Gives back the room the maps were grown into, once a whole project has been read.
    pub fn shrink(&mut self) {
        for places in self.by_name.values_mut() {
            places.shrink_to_fit();
        }
        for places in self.by_container.values_mut() {
            places.shrink_to_fit();
        }
        self.word_starts.settle();
        self.humps.settle();
        self.by_file.shrink_to_fit();
        self.by_container.shrink_to_fit();
        self.imports.shrink_to_fit();
    }

    /// The definitions of a lower case name, exactly.
    ///
    /// @param lower - the name, lower case
    pub fn named(&self, lower: &str) -> Named<'_> {
        let places = self.by_name.get(lower).map_or(&[][..], Vec::as_slice);
        Named { table: self, places: places.iter() }
    }

    /// The lower case names that start with a lower case prefix, in order, each with its definitions.
    ///
    /// @param prefix - the prefix, lower case
    pub fn prefixed<'a>(
        &'a self,
        prefix: &'a str,
    ) -> impl Iterator<Item = (&'a str, Named<'a>)> + 'a {
        let from = (std::ops::Bound::Included(prefix), std::ops::Bound::Unbounded);
        self.by_name
            .range::<str, _>(from)
            .take_while(move |(k, _)| k.starts_with(prefix))
            .map(move |(k, v)| (&**k, Named { table: self, places: v.iter() }))
    }

    /// The lower case names with a later word that starts with a lower case prefix: `scroll` reaches
    /// `layout_scroll_margin`.
    ///
    /// @param prefix - the prefix, lower case
    pub fn word_started<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.word_starts.starting_with(prefix)
    }

    /// The lower case names whose initials start with a lower case prefix: `lsm` reaches
    /// `layout_scroll_margin`.
    ///
    /// @param prefix - the prefix, lower case
    pub fn humped<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.humps.starting_with(prefix)
    }

    /// A file's imports, in the order they are written.
    ///
    /// @param path - the file's path
    pub fn imports_of(&self, path: &str) -> &[crate::structure::Import] {
        self.imports.get(path).map_or(&[], Vec::as_slice)
    }

    /// Every file that defines something, with its definitions.
    pub fn files(&self) -> impl Iterator<Item = (&str, &[Definition])> {
        self.by_file.iter().map(|(p, d)| (&**p, d.as_slice()))
    }

    /// Every file's imports.
    pub fn all_imports(&self) -> impl Iterator<Item = (&str, &[crate::structure::Import])> {
        self.imports.iter().map(|(p, i)| (&**p, i.as_slice()))
    }

    /// A copy of every file's definitions and imports, for the store to write with no lock held.
    #[allow(clippy::type_complexity)]
    pub fn rows(&self) -> Vec<(String, Vec<Definition>, Vec<crate::structure::Import>)> {
        let mut paths: Vec<&Arc<str>> = self.by_file.keys().chain(self.imports.keys()).collect();
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .map(|p| {
                let defs = self.by_file.get(p).cloned().unwrap_or_default();
                (p.to_string(), defs, self.imports.get(p).cloned().unwrap_or_default())
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
        let mut found: Vec<Defined> = self.named(&lower).map(DefinedRef::to_owned).collect();
        if found.is_empty() && lower.len() >= 3 {
            let mut partial: Vec<DefinedRef<'_>> = self
                .by_name
                .iter()
                .filter(|(k, _)| k.contains(lower.as_str()))
                .flat_map(|(_, v)| v.iter().filter_map(|place| self.place(place)))
                .collect();
            partial.sort_by_key(|d| d.definition.name.len());
            found = partial.into_iter().take(limit * 4).map(DefinedRef::to_owned).collect();
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
