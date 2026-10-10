//! The structural symbol table (Unluminous `task-2231` §5.2): it is written to the index file and read
//! back as it was, a changed file replaces its own rows, and the container, prefix, word start and
//! initials lookups give exactly what a walk of every definition gives.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

use atrius_index::index::Index;
use atrius_index::outline::Definition;
use atrius_index::structure::{Kind, Visibility};
use atrius_index::symbols::{word_starts_of, SymbolTable};

/// Writes a file, creating its folder.
fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// One scratch cache for the whole file, set once, as `tests/store.rs` does.
fn scratch_cache() {
    static CACHE: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| tempfile::tempdir().unwrap());
    std::env::set_var("ATRIUS_INDEX_CACHE", cache.path());
    std::env::set_var("ATRIUS_EMBED", "off");
}

/// A small Rust project with a struct, its methods and an import.
fn a_project(root: &Path) {
    write(root, "src/layout.rs", "/// Lays text out.\npub struct Layout {\n    pub width: f32,\n}\n\nimpl Layout {\n    pub fn new(width: f32) -> Self { Layout { width } }\n    pub fn caret_at(&self, offset: usize) -> f32 { 0.0 }\n}\n");
    write(root, "src/main.rs", "use crate::layout::Layout;\n\nfn main() {\n    let layout = Layout::new(3.0);\n}\n");
}

/// Every definition of a table, as comparable rows.
fn every_row(table: &SymbolTable) -> BTreeSet<String> {
    let mut rows = BTreeSet::new();
    for (path, defs) in table.files() {
        for d in defs {
            rows.insert(format!(
                "{path} {} {} {:?} {} {:?} {:?} {} {:?}",
                d.name,
                d.symbol_kind.name(),
                d.container,
                d.parameter_text(),
                d.returns,
                d.type_text,
                d.visibility.name(),
                d.doc
            ));
        }
    }
    for (path, imports) in table.all_imports() {
        for i in imports {
            rows.insert(format!("{path} import {:?} {:?} {}", i.path, i.alias, i.glob));
        }
    }
    rows
}

/// Waits until a condition holds, or a minute passes.
fn wait_for(mut done: impl FnMut() -> bool) -> bool {
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(60) {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn the_symbol_table_is_written_down_and_read_back_as_it_was() {
    scratch_cache();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    a_project(&root);
    let root = atrius_index::host::canonical(&root);
    let built = {
        let index = Index::open_quietly(&root);
        assert!(index.wait_ready(Duration::from_secs(60)));
        assert!(index.wait_for_symbols(Duration::from_secs(60)));
        let rows = index.try_symbols(every_row).expect("the table is there");
        // The rows are written on the store thread once the index answers; wait until the file has them.
        let store = atrius_index::store::index_folder(&root);
        assert!(wait_for(|| store.join("index.rdb").exists()));
        std::thread::sleep(Duration::from_millis(1500));
        rows
    };
    assert!(built.iter().any(|r| r.contains("caret_at method Some(\"Layout\")")), "{built:#?}");
    assert!(built.iter().any(|r| r.contains("import [\"crate\", \"layout\", \"Layout\"]")), "{built:#?}");
    let store = atrius_index::store::Store::open(&atrius_index::store::index_folder(&root)).unwrap();
    let loaded = store.load_symbols().unwrap().expect("the table is current");
    assert_eq!(every_row(&loaded), built, "what is read back is what was written");
}

#[test]
fn a_changed_file_replaces_its_own_rows_and_nothing_else() {
    scratch_cache();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    a_project(&root);
    let root = atrius_index::host::canonical(&root);
    let index = std::sync::Arc::new(Index::open_quietly(&root));
    assert!(index.wait_for_symbols(Duration::from_secs(60)));
    Index::keep_fresh(&index, Duration::from_millis(50));
    write(&root, "src/layout.rs", "pub struct Layout {\n    pub height: f32,\n}\nimpl Layout {\n    pub fn line_at(&self) -> usize { 0 }\n}\n");
    let fresh = wait_for(|| {
        index
            .try_symbols(|t| t.members_of("Layout").any(|(_, d)| d.name == "line_at"))
            .unwrap_or(false)
    });
    assert!(fresh, "keep_fresh ran the gate and the table has the new method");
    let members: BTreeSet<String> = index
        .try_symbols(|t| t.members_of("Layout").map(|(_, d)| d.name.clone()).collect())
        .unwrap();
    assert_eq!(members, ["height", "line_at"].iter().map(|s| (*s).to_owned()).collect());
    let main_still_there = index.try_symbols(|t| t.named("main").len()).unwrap();
    assert_eq!(main_still_there, 1, "the other file's rows are untouched");
    // And the file holds the same thing once the store thread has written the batch.
    let store_folder = atrius_index::store::index_folder(&root);
    let written = wait_for(|| {
        let Ok(store) = atrius_index::store::Store::open(&store_folder) else { return false };
        store
            .load_symbols()
            .ok()
            .flatten()
            .is_some_and(|t| t.members_of("Layout").any(|(_, d)| d.name == "line_at"))
    });
    assert!(written, "the changed rows reached the file");
}

/// A definition with only what the lookups read.
fn definition(name: &str, container: Option<&str>) -> Definition {
    Definition {
        name: name.to_owned(),
        kind: "function",
        line: 1,
        end: 1,
        start: 1,
        signature: String::new(),
        likely: false,
        exported: true,
        depth: 0,
        symbol_kind: Kind::Function,
        container: container.map(str::to_owned),
        parameters: Vec::new(),
        returns: None,
        type_text: None,
        visibility: Visibility::Public,
        doc: None,
        range: 0..name.len(),
        member: false,
    }
}

/// A small seeded random number generator, so the test is the same on every run.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

#[test]
fn the_lookups_agree_with_a_walk_of_every_definition() {
    let parts = ["layout", "Scroll", "margin", "caret", "At", "draw", "HTTP", "Server", "x2", "rects"];
    let containers = ["Layout", "Painter", "Board"];
    let mut random = Seeded(2231);
    let mut table = SymbolTable::default();
    let mut all: Vec<(String, Option<String>)> = Vec::new();
    for file in 0..20 {
        let mut defs = Vec::new();
        for _ in 0..30 {
            let words = 1 + random.next() as usize % 3;
            let mut name = String::new();
            for w in 0..words {
                let part = parts[random.next() as usize % parts.len()];
                if w > 0 && random.next() % 2 == 0 {
                    name.push('_');
                }
                name.push_str(part);
            }
            let container = (random.next() % 3 == 0).then(|| containers[random.next() as usize % 3]);
            defs.push(definition(&name, container));
        }
        table.set_file(&format!("src/f{file}.rs"), defs);
    }
    // Replace a few files, so removal is exercised as well as insertion.
    for file in [3, 7, 11] {
        table.set_file(&format!("src/f{file}.rs"), vec![definition("replacement_name", Some("Board"))]);
    }
    for (_, defs) in table.files() {
        for d in defs {
            all.push((d.name.clone(), d.container.clone()));
        }
    }
    for container in containers {
        let mut fast: Vec<String> = table.members_of(container).map(|(_, d)| d.name.clone()).collect();
        let mut slow: Vec<String> =
            all.iter().filter(|(_, c)| c.as_deref() == Some(container)).map(|(n, _)| n.clone()).collect();
        fast.sort();
        slow.sort();
        assert_eq!(fast, slow, "members of {container}");
    }
    let names: BTreeSet<String> = all.iter().map(|(n, _)| n.to_lowercase()).collect();
    for prefix in ["l", "la", "scr", "x", "http", "draw_", "zz", "rect"] {
        let fast: BTreeSet<String> = table.prefixed(prefix).map(|(k, _)| k.to_owned()).collect();
        let slow: BTreeSet<String> = names.iter().filter(|n| n.starts_with(prefix)).cloned().collect();
        assert_eq!(fast, slow, "names starting {prefix}");
        let fast: BTreeSet<String> = table.word_started(prefix).map(str::to_owned).collect();
        let slow: BTreeSet<String> = all
            .iter()
            .filter(|(n, _)| {
                let letters: Vec<char> = n.chars().collect();
                word_starts_of(n).iter().skip(1).any(|&i| {
                    letters[i..].iter().collect::<String>().to_lowercase().starts_with(prefix)
                })
            })
            .map(|(n, _)| n.to_lowercase())
            .collect();
        assert_eq!(fast, slow, "names with a later word starting {prefix}");
    }
    for initials in ["ls", "lsm", "hs", "dr", "ca"] {
        let fast: BTreeSet<String> = table.humped(initials).map(str::to_owned).collect();
        let slow: BTreeSet<String> = all
            .iter()
            .filter(|(n, _)| {
                let starts = word_starts_of(n);
                let letters: Vec<char> = n.chars().collect();
                starts.len() >= 2
                    && starts.iter().map(|&i| letters[i]).collect::<String>().to_lowercase().starts_with(initials)
            })
            .map(|(n, _)| n.to_lowercase())
            .collect();
        assert_eq!(fast, slow, "names with initials {initials}");
    }
}
