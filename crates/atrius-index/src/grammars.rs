//! The languages the index reads definitions and chunks in.
//!
//! Each language is a manifest in `languages/` at the root of this crate, compiled in, in the
//! `name = value` format described in `docs/languages.md`. A program that embeds the index can add a
//! language or replace one with [`register`], and a person can add one by putting a `.conf` file in the
//! folder `ATRIUS_LANGUAGES` names. A file in a language nobody describes is still searched exactly and
//! chunked as plain text; it only has no definitions.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use crate::lang::manifest::{language_extensions, language_grammar, Values};
use crate::lang::syntax::Grammar;

/// The languages compiled into the index, as `(id, manifest)`.
pub const BUNDLED: &[(&str, &str)] = &[
    ("c", include_str!("../languages/c.conf")),
    ("cpp", include_str!("../languages/cpp.conf")),
    ("csharp", include_str!("../languages/csharp.conf")),
    ("css", include_str!("../languages/css.conf")),
    ("go", include_str!("../languages/go.conf")),
    ("html", include_str!("../languages/html.conf")),
    ("java", include_str!("../languages/java.conf")),
    ("javascript", include_str!("../languages/javascript.conf")),
    ("json", include_str!("../languages/json.conf")),
    ("kotlin", include_str!("../languages/kotlin.conf")),
    ("lua", include_str!("../languages/lua.conf")),
    ("mermaid", include_str!("../languages/mermaid.conf")),
    ("php", include_str!("../languages/php.conf")),
    ("python", include_str!("../languages/python.conf")),
    ("ruby", include_str!("../languages/ruby.conf")),
    ("rust", include_str!("../languages/rust.conf")),
    ("scala", include_str!("../languages/scala.conf")),
    ("shell", include_str!("../languages/shell.conf")),
    ("sql", include_str!("../languages/sql.conf")),
    ("swift", include_str!("../languages/swift.conf")),
    ("toml", include_str!("../languages/toml.conf")),
    ("typescript", include_str!("../languages/typescript.conf")),
    ("yaml", include_str!("../languages/yaml.conf")),
    ("zig", include_str!("../languages/zig.conf")),
];

/// One language: its id and its grammar.
pub struct Language {
    /// The manifest's id, such as `rust`.
    pub id: String,
    /// The grammar the manifest describes.
    pub grammar: Grammar,
}

/// Every known language by file extension.
type Table = HashMap<String, Arc<Language>>;

/// The table, built on first use from the bundled manifests and the `ATRIUS_LANGUAGES` folder.
fn table() -> &'static RwLock<Arc<Table>> {
    static TABLE: OnceLock<RwLock<Arc<Table>>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = HashMap::new();
        for (id, text) in BUNDLED {
            add(&mut out, id, text);
        }
        for (id, text) in folder_manifests() {
            add(&mut out, &id, &text);
        }
        RwLock::new(Arc::new(out))
    })
}

/// The manifests in the folder `ATRIUS_LANGUAGES` names, as `(file stem, text)`.
fn folder_manifests() -> Vec<(String, String)> {
    let Some(folder) = std::env::var_os("ATRIUS_LANGUAGES") else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut out: Vec<(String, String)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "conf"))
        .filter_map(|e| {
            let id = e.path().file_stem()?.to_string_lossy().into_owned();
            Some((id, std::fs::read_to_string(e.path()).ok()?))
        })
        .collect();
    out.sort();
    out
}

/// Reads one manifest into the table. A later manifest claims an extension from an earlier one.
///
/// @param out - the table
/// @param id - the language's id, used when the manifest names none
/// @param text - the manifest
fn add(out: &mut Table, id: &str, text: &str) -> bool {
    let values = Values::parse(text);
    let id = values.text("plugin.id").unwrap_or(id).to_owned();
    let name = values.text("plugin.name").unwrap_or(&id).to_owned();
    let Ok(grammar) = language_grammar(&values, &name) else { return false };
    let language = Arc::new(Language { id, grammar });
    for extension in language_extensions(&values) {
        out.insert(extension, Arc::clone(&language));
    }
    true
}

/// Adds a language, or replaces the one that claims the same extensions. Answers whether the manifest
/// read. Call it before opening an index: a file already chunked is not read again.
///
/// @param id - the language's id, used when the manifest names none
/// @param manifest - the manifest text
pub fn register(id: &str, manifest: &str) -> bool {
    let lock = table();
    let mut guard = lock.write().unwrap_or_else(|e| e.into_inner());
    let mut next: Table = (**guard).clone();
    let read = add(&mut next, id, manifest);
    *guard = Arc::new(next);
    read
}

/// Every known language by file extension.
pub fn by_extension() -> Arc<Table> {
    Arc::clone(&table().read().unwrap_or_else(|e| e.into_inner()))
}

/// The language of a file, by its extension, if a manifest claims it.
///
/// @param rel - the file's path
pub fn for_path(rel: &str) -> Option<Arc<Language>> {
    let extension = crate::store::extension(rel);
    by_extension().get(&extension).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_language_reads() {
        for (id, text) in BUNDLED {
            let values = Values::parse(text);
            let name = values.text("plugin.name").unwrap_or(id).to_owned();
            assert!(language_grammar(&values, &name).is_ok(), "{id} does not read");
            assert!(!language_extensions(&values).is_empty(), "{id} claims no extension");
        }
        // Every bundled language keeps at least one extension of its own. Another test registers a
        // language at the same time, so the table may hold one more.
        let ids: std::collections::HashSet<String> =
            by_extension().values().map(|l| l.id.clone()).collect();
        for (id, _) in BUNDLED {
            assert!(ids.contains(*id), "{id} lost every extension to another language");
        }
    }

    #[test]
    fn the_code_languages_define_symbols() {
        for path in ["a.rs", "a.go", "a.java", "a.c", "a.cpp", "a.cs", "a.py", "a.rb", "a.php", "a.ts"] {
            let language = for_path(path).unwrap_or_else(|| panic!("no language for {path}"));
            assert!(language.grammar.defines_symbols(), "{path}");
        }
        assert!(for_path("README.md").is_none());
    }

    #[test]
    fn a_registered_manifest_claims_its_extensions() {
        let manifest = "plugin.id = demo\nplugin.name = Demo\nlanguage.extensions = .demo\nlanguage.definers = proc=function\n";
        assert!(register("demo", manifest));
        assert_eq!(for_path("x/y.demo").map(|l| l.id.clone()).as_deref(), Some("demo"));
    }
}
