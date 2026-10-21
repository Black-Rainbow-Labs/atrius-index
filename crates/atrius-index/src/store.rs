//! The Inillucent file an index is kept in (`tasks/task-2138-unluminous-code-index-tdd.md` §6.2).
//!
//! One file per checkout, under the user's cache folder and never inside the repository, opened only by
//! the host. `inillucent_driver::Database` is neither `Send` nor `Sync`, so a `Store` lives on one
//! thread, the host's writer, and everything else talks to it through the in-memory index.
//!
//! What is written: every file's row, its content once per distinct hash in `blob`, and the trigram
//! posting lists, as one base row per trigram plus a row per file changed since the base was written.
//! Loading reads them back in id order, so the in-memory lists come back sorted without a sort. The
//! version of each engine is in `meta`, and a version that does not match rebuilds that engine's tables.

use std::path::{Path, PathBuf};

use inillucent_driver::{Connection, Database, OpenOptions, Value};

use crate::exact::{block_id, pack_blocks, unpack_blocks, Exact, FileRecord};
use crate::outline::Definition;
use crate::structure::{Import, Kind, Parameter, Visibility};
use crate::symbols::SymbolTable;
use crate::trigram::{Posting, Postings};
use std::collections::HashMap;

/// The schema version of the tables this module writes. A different number rebuilds everything.
///
/// `3` added the `symbol` and `import` tables (Unluminous `task-2231` §5.2), so an index written by an
/// older version is rebuilt, which the recorded numbers put at 0.6 s on vite and 1.5 s on django.
///
/// `4` is the same tables. It is raised so every index written before the watcher obeyed the ignore rules
/// of the folders above a change is replaced: those hold a project's whole build output and every version
/// of every file, and the Unluminous checkout's had reached 11.7 GB (Unluminous `task-2247`). A file of an
/// older version is deleted and made again rather than emptied, because emptying it leaves the file its
/// size on disk.
///
/// `5` is the same tables. It is raised so every index written before a file larger than
/// `exact::LARGEST_HELD` stopped being held is replaced: such an index holds those files whole, and
/// loading it reads them into memory (Unluminous `task-2323`).
pub const SCHEMA_VERSION: &str = "5";
/// The trigram engine's version: how bytes are folded and which trigrams are kept.
pub const TRIGRAM_VERSION: &str = "1";
/// The structural reading's version: what `crate::structure` takes out of a definition. A different
/// number reads every file's definitions again rather than loading the `symbol` table.
pub const SYMBOL_VERSION: &str = "1";

/// The tables. `symbol` and `import` are keyed by the file's path rather than its id, because
/// compaction gives every file a new id and the rows would otherwise all have to be written again.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS file (id INTEGER PRIMARY KEY, path TEXT UNIQUE, lang TEXT, size INTEGER,
    mtime_ns INTEGER, content_hash BLOB, generation INTEGER, is_binary INTEGER, is_generated INTEGER,
    line_count INTEGER);
CREATE INDEX IF NOT EXISTS file_content ON file(content_hash);
CREATE TABLE IF NOT EXISTS blob (content_hash BLOB PRIMARY KEY, bytes BLOB);
CREATE TABLE IF NOT EXISTS trigram (tri INTEGER PRIMARY KEY, postings BLOB);
CREATE TABLE IF NOT EXISTS trigram_delta (file_id INTEGER PRIMARY KEY, tris BLOB, tombstone INTEGER);
CREATE TABLE IF NOT EXISTS symbol (id INTEGER PRIMARY KEY, path TEXT, ordinal INTEGER, name TEXT,
    kind TEXT, container TEXT, signature TEXT, parameters TEXT, returns TEXT, type_text TEXT,
    visibility TEXT, doc TEXT, line INTEGER, end_line INTEGER, start_line INTEGER, byte_start INTEGER,
    byte_end INTEGER, likely INTEGER, depth INTEGER, exported INTEGER, member INTEGER);
CREATE INDEX IF NOT EXISTS symbol_path ON symbol(path);
CREATE INDEX IF NOT EXISTS symbol_name ON symbol(name);
CREATE INDEX IF NOT EXISTS symbol_container ON symbol(container);
CREATE TABLE IF NOT EXISTS import (id INTEGER PRIMARY KEY, path TEXT, ordinal INTEGER, segments TEXT,
    alias TEXT, is_glob INTEGER, byte_start INTEGER, byte_end INTEGER);
CREATE INDEX IF NOT EXISTS import_path ON import(path);
";

/// The index file of one checkout.
pub struct Store {
    /// None only for the moment between closing the file and opening it again with another pool.
    database: Option<Database>,
    path: PathBuf,
    /// How many pages the file's pool was opened with.
    frames: usize,
}

/// Where the index of a root lives: `<cache>/atrius-index/index/<blake3 of the canonical root>/index.rdb`.
///
/// @param root - the checkout's root
pub fn index_folder(root: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let text = canonical.to_string_lossy().replace("\\\\?\\", "").to_lowercase();
    let hash = blake3::hash(text.as_bytes()).to_hex();
    cache_folder().join("atrius-index").join("index").join(&hash.as_str()[..24])
}

/// The user's local cache folder: `%LOCALAPPDATA%` on Windows, `~/Library/Caches` on macOS,
/// `$XDG_CACHE_HOME` or `~/.cache` elsewhere. `ATRIUS_INDEX_CACHE` overrides it, which is what the
/// tests and the evaluation harness use so they never touch a person's own cache.
pub fn cache_folder() -> PathBuf {
    if let Some(dir) = std::env::var_os("ATRIUS_INDEX_CACHE") {
        return PathBuf::from(dir);
    }
    if cfg!(windows) {
        if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(dir);
        }
    }
    if cfg!(target_os = "macos") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join("Library").join("Caches");
        }
    }
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(dir);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".cache"))
        .unwrap_or_else(std::env::temp_dir)
}

/// Opens the index file with a pool of this many pages.
///
/// @param path - the file
/// @param frames - how many 32 KiB pages
fn open_database(path: &Path, frames: usize) -> Result<Database, String> {
    let options = OpenOptions { create: true, cache_frames: frames, ..OpenOptions::default() };
    Database::open_with(path, options).map_err(|e| e.to_string())
}

/// The pool a store opens with and builds with: 1,024 pages, 32 MiB, which is what building the passage
/// table needs (see [`Store::reopen_with`]).
pub const BUILD_FRAMES: usize = 1024;

/// How many rows one page of a load reads.
const LOAD_PAGE: usize = 1024;

/// Runs a query a page of rows at a time and hands each row over, so a load never holds the whole of a
/// table's result beside what is built from it (Unluminous `task-2279`: a window that loaded its index
/// peaked 25 MB above what it kept, and the Windows heap keeps a peak's pages). The query names its key
/// first and filters `> ?1` on it in key order.
///
/// @param session - the connection
/// @param sql - `SELECT key, ... WHERE key > ?1 ORDER BY key`
/// @param each - what to do with a row
fn in_pages(
    session: &inillucent_driver::Connection<'_>,
    sql: &str,
    mut each: impl FnMut(&[Value]),
) -> Result<(), String> {
    let mut after = i64::MIN;
    loop {
        let rows = session
            .query(sql, &[Value::Integer(after)], LOAD_PAGE)
            .map_err(|e| e.to_string())?;
        for row in &rows.rows {
            if let Some(Value::Integer(key)) = row.first() {
                after = *key;
            }
            each(row);
        }
        if rows.rows.len() < LOAD_PAGE {
            return Ok(());
        }
    }
}

/// How many 32 KiB pages the index file's buffer pool holds once the index is loaded or built: 128,
/// which is 4 MiB, or `ATRIUS_STORE_CACHE_FRAMES`. A build, a batch of changes and a compaction reopen
/// the file with [`BUILD_FRAMES`] for as long as they take.
///
/// Inillucent's default is 4,096, 128 MiB, which is right for a database that answers queries from its
/// pages. This one is read whole once when a host opens it, into the exact index and the symbol table held
/// in memory, and after that it is written a batch at a time and asked a passage search now and then. The
/// pool filled to its 128 MiB during that one read and kept it for the life of the host, which in an
/// editor that hosts its project's index is the editor's whole life (Unluminous `task-2279`). 32 MiB in
/// 0.2.2 and 4 MiB since 0.3.0, measured 33 MB less in a window: the pages a passage search or a batch of
/// changes reads are in the operating system's file cache whatever the pool holds.
pub fn cache_frames() -> usize {
    std::env::var("ATRIUS_STORE_CACHE_FRAMES")
        .ok()
        .and_then(|n| n.trim().parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(128)
}

impl Store {
    /// Opens or creates the index file in a folder, with its schema.
    ///
    /// @param folder - the index folder of one checkout
    pub fn open(folder: &Path) -> Result<Store, String> {
        std::fs::create_dir_all(folder)
            .map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
        let path = folder.join("index.rdb");
        let database = open_database(&path, BUILD_FRAMES)?;
        database.session().execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        Ok(Store { database: Some(database), path, frames: BUILD_FRAMES })
    }

    /// Opens the file again with another pool size. The file is closed first, because one process holds
    /// it once; when it cannot be opened again with the size asked for, it is opened as it was.
    ///
    /// The pool has to be large while the passage table is built or written: Inillucent 2.0 pins pages
    /// while it builds a search table, and with 512 pages or fewer the build failed with "out of memory",
    /// so the passages were never written and every open built them again (Unluminous `task-2279`). It
    /// only has to be small the rest of the time, which is nearly all of it.
    ///
    /// @param frames - how many 32 KiB pages
    pub fn reopen_with(&mut self, frames: usize) -> Result<(), String> {
        if self.frames == frames && self.database.is_some() {
            return Ok(());
        }
        drop(self.database.take());
        match open_database(&self.path, frames) {
            Ok(database) => {
                self.database = Some(database);
                self.frames = frames;
                Ok(())
            }
            Err(e) => {
                self.database = Some(open_database(&self.path, self.frames)?);
                Err(e)
            }
        }
    }

    /// Opens the file with room to build, runs the work, and goes back to the small pool.
    ///
    /// @param work - what to do
    pub fn with_room<T>(&mut self, work: impl FnOnce(&Store) -> T) -> T {
        let _ = self.reopen_with(BUILD_FRAMES);
        let done = work(self);
        let _ = self.reopen_with(cache_frames());
        done
    }

    /// How many pages the file's pool holds now.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Opens the index file in a folder, first deleting it when an older schema wrote it, so a rebuild
    /// starts from an empty file rather than from one the size of everything it ever held. A file that
    /// cannot be deleted, because another process still has it open, is opened as it is and emptied by
    /// the rebuild instead.
    ///
    /// @param folder - the index folder of one checkout
    pub fn open_replacing_an_old_one(folder: &Path) -> Result<Store, String> {
        let store = Store::open(folder)?;
        let written_by = store.meta("schema_version");
        if written_by.is_none() || written_by.as_deref() == Some(SCHEMA_VERSION) {
            return Ok(store);
        }
        drop(store);
        let Ok(entries) = std::fs::read_dir(folder) else { return Store::open(folder) };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("index.rdb") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Store::open(folder)
    }

    /// The file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A connection for one unit of work.
    pub fn session(&self) -> Connection<'_> {
        self.database.as_ref().expect("the index file is open").session()
    }

    /// One value of `meta`.
    ///
    /// @param key - the key
    pub fn meta(&self, key: &str) -> Option<String> {
        let rows = self
            .session()
            .query("SELECT value FROM meta WHERE key = ?1", &[Value::Text(key.into())], 1)
            .ok()?;
        rows.value(0, 0).and_then(Value::text).map(str::to_owned)
    }

    /// Writes one value of `meta`.
    ///
    /// @param key - the key
    /// @param value - the value
    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), String> {
        self.session()
            .execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
                &[Value::Text(key.into()), Value::Text(value.into())],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Whether the stored trigram index was written by this version and can be loaded.
    pub fn exact_is_current(&self) -> bool {
        self.meta("schema_version").as_deref() == Some(SCHEMA_VERSION)
            && self.meta("trigram_version").as_deref() == Some(TRIGRAM_VERSION)
    }

    /// Writes the whole exact index, replacing what was there, in one transaction.
    ///
    /// @param exact - the in-memory index
    /// @param root - the root it indexes, recorded in `meta`
    pub fn save_exact(&self, exact: &Exact, root: &Path) -> Result<(), String> {
        let session = self.session();
        let tx = session.begin().map_err(|e| e.to_string())?;
        for table in ["file", "blob", "trigram", "trigram_delta"] {
            tx.execute(&format!("DELETE FROM {table}"), &[]).map_err(|e| e.to_string())?;
        }
        let mut blobs = std::collections::HashSet::new();
        for (id, record) in exact.files.iter().enumerate() {
            let Some(record) = record else { continue };
            write_file_row(&tx, id as u32, record)?;
            if blobs.insert(record.hash) {
                // A record that let go of its content is written from the file it came from.
                let read;
                let blocks = match record.is_held() {
                    true => &record.blocks,
                    false => {
                        let bytes = record.content(Some(root)).unwrap_or_default();
                        read = FileRecord::from_bytes(record.rel.clone(), record.mtime_ns, &bytes).0;
                        &read.blocks
                    }
                };
                tx.execute(
                    "INSERT INTO blob (content_hash, bytes) VALUES (?1, ?2)",
                    &[Value::Blob(record.hash.to_vec()), Value::Blob(pack_blocks(blocks))],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        for (tri, posting) in exact.postings.iter() {
            tx.execute(
                "INSERT INTO trigram (tri, postings) VALUES (?1, ?2)",
                &[Value::Integer(i64::from(*tri)), Value::Blob(posting.as_bytes().to_vec())],
            )
            .map_err(|e| e.to_string())?;
        }
        for (key, value) in [
            ("schema_version", SCHEMA_VERSION),
            ("trigram_version", TRIGRAM_VERSION),
            ("next_id", &exact.files.len().to_string()),
            ("root", &root.to_string_lossy()),
        ] {
            tx.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
                &[Value::Text(key.into()), Value::Text(value.into())],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    /// Records changed and removed files since the base was written: their rows, their blobs, and their
    /// trigrams as delta rows. One transaction for the whole batch, which is kept short so a query never
    /// waits long behind it.
    ///
    /// @param changed - `(id, record, trigrams)` for each file added or rewritten
    /// @param removed - ids tombstoned
    /// @param next_id - the next id the in-memory index will hand out
    pub fn save_changes(
        &self,
        changed: &[(u32, FileRecord, Vec<Vec<u32>>)],
        removed: &[u32],
        next_id: usize,
    ) -> Result<(), String> {
        let session = self.session();
        // The content of every file row this batch takes away, so a blob nothing names any more is
        // deleted with it. Nothing did that before, and a blob is a whole file's content: a project
        // whose files changed often enough kept every version of every one of them (`task-2247`).
        let mut orphans: Vec<Vec<u8>> = Vec::new();
        let mut note_the_content = |sql: &str, key: Value| {
            if let Ok(rows) = session.query_all(sql, &[key]) {
                orphans.extend(rows.rows.iter().filter_map(|row| row.first()?.bytes().map(<[u8]>::to_vec)));
            }
        };
        for &id in removed {
            note_the_content(
                "SELECT content_hash FROM file WHERE id = ?1",
                Value::Integer(i64::from(id)),
            );
        }
        for (_, record, _) in changed {
            note_the_content(
                "SELECT content_hash FROM file WHERE path = ?1",
                Value::Text(record.rel.clone()),
            );
        }
        let tx = session.begin().map_err(|e| e.to_string())?;
        for &id in removed {
            tx.execute("DELETE FROM file WHERE id = ?1", &[Value::Integer(i64::from(id))])
                .map_err(|e| e.to_string())?;
            tx.execute("INSERT OR REPLACE INTO trigram_delta (file_id, tris, tombstone) VALUES (?1, NULL, 1)", &[Value::Integer(i64::from(id))]).map_err(|e| e.to_string())?;
        }
        for (id, record, trigrams) in changed {
            tx.execute("DELETE FROM file WHERE path = ?1", &[Value::Text(record.rel.clone())])
                .map_err(|e| e.to_string())?;
            write_file_row(&tx, *id, record)?;
            tx.execute(
                "INSERT OR IGNORE INTO blob (content_hash, bytes) VALUES (?1, ?2)",
                &[Value::Blob(record.hash.to_vec()), Value::Blob(pack_blocks(&record.blocks))],
            )
            .map_err(|e| e.to_string())?;
            let tris = pack_trigrams(trigrams);
            tx.execute("INSERT OR REPLACE INTO trigram_delta (file_id, tris, tombstone) VALUES (?1, ?2, 0)", &[Value::Integer(i64::from(*id)), Value::Blob(tris)]).map_err(|e| e.to_string())?;
        }
        orphans.sort();
        orphans.dedup();
        for hash in orphans {
            tx.execute(
                "DELETE FROM blob WHERE content_hash = ?1 AND NOT EXISTS (SELECT 1 FROM file WHERE content_hash = ?1)",
                &[Value::Blob(hash)],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('next_id', ?1)",
            &[Value::Text(next_id.to_string())],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    /// Reads the exact index back: base posting lists, then each delta in id order, then the file rows
    /// and their content. Ids with no file row are tombstones.
    pub fn load_exact(&self) -> Result<Exact, String> {
        let session = self.session();
        let next_id: usize = self.meta("next_id").and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut postings = Postings::default();
        in_pages(&session, "SELECT tri, postings FROM trigram WHERE tri > ?1 ORDER BY tri", |row| {
            if let (Some(Value::Integer(tri)), Some(bytes)) =
                (row.first(), row.get(1).and_then(Value::bytes))
            {
                postings.insert(*tri as u32, Posting::from_bytes(bytes.to_vec()));
            }
        })?;
        let deltas = session
            .query_all(
                "SELECT file_id, tris FROM trigram_delta WHERE tombstone = 0 ORDER BY file_id",
                &[],
            )
            .map_err(|e| e.to_string())?;
        for row in &deltas.rows {
            if let (Some(Value::Integer(id)), Some(bytes)) =
                (row.first(), row.get(1).and_then(Value::bytes))
            {
                for (block, tris) in unpack_trigrams(bytes).iter().enumerate() {
                    postings.add(block_id(*id as u32, block), tris);
                }
            }
        }
        let mut files: Vec<Option<FileRecord>> = vec![None; next_id];
        in_pages(&session, "SELECT f.id, f.path, f.size, f.mtime_ns, f.content_hash, f.is_binary, f.line_count, b.bytes FROM file f JOIN blob b ON b.content_hash = f.content_hash WHERE f.id > ?1 ORDER BY f.id", |row| {
            let int = |i: usize| match row.get(i) {
                Some(Value::Integer(v)) => *v,
                _ => 0,
            };
            let id = int(0) as usize;
            if id >= files.len() {
                files.resize(id + 1, None);
            }
            let mut hash = [0u8; 32];
            hash.copy_from_slice(
                row.get(4).and_then(Value::bytes).unwrap_or(&[0; 32]).get(..32).unwrap_or(&[0; 32]),
            );
            files[id] = Some(FileRecord {
                rel: row.get(1).and_then(Value::text).unwrap_or_default().to_owned(),
                size: int(2) as u64,
                mtime_ns: int(3),
                hash,
                blocks: unpack_blocks(row.get(7).and_then(Value::bytes).unwrap_or_default()),
                binary: int(5) != 0,
                lines: int(6) as u32,
            }
            .held_to_its_first_nul());
        })?;
        Ok(Exact::restore(files, postings))
    }
}

impl Store {
    /// Whether the stored `symbol` and `import` tables were written by this structural reading.
    pub fn symbols_are_current(&self) -> bool {
        self.exact_is_current() && self.meta("symbol_version").as_deref() == Some(SYMBOL_VERSION)
    }

    /// Writes every file's definitions and imports, replacing what was there, in one transaction.
    ///
    /// @param rows - each file's path, definitions and imports, as `SymbolTable::rows` copies them
    pub fn save_symbols(&self, rows: &[(String, Vec<Definition>, Vec<Import>)]) -> Result<(), String> {
        let session = self.session();
        let tx = session.begin().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM symbol", &[]).map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM import", &[]).map_err(|e| e.to_string())?;
        for (path, defs, imports) in rows {
            write_symbol_rows(&tx, path, defs)?;
            write_import_rows(&tx, path, imports)?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('symbol_version', ?1)",
            &[Value::Text(SYMBOL_VERSION.into())],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    /// Replaces the rows of the files a gate read again and removes those of the files it removed.
    ///
    /// @param changed - each changed file's path, definitions and imports
    /// @param removed - the paths of the removed files
    pub fn save_symbol_changes(
        &self,
        changed: &[(String, Vec<Definition>, Vec<Import>)],
        removed: &[String],
    ) -> Result<(), String> {
        let session = self.session();
        let tx = session.begin().map_err(|e| e.to_string())?;
        for path in removed.iter().chain(changed.iter().map(|c| &c.0)) {
            for table in ["symbol", "import"] {
                tx.execute(&format!("DELETE FROM {table} WHERE path = ?1"), &[Value::Text(path.clone())])
                    .map_err(|e| e.to_string())?;
            }
        }
        for (path, defs, imports) in changed {
            write_symbol_rows(&tx, path, defs)?;
            write_import_rows(&tx, path, imports)?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    /// Reads the symbol table back from the `symbol` and `import` tables, or `None` when they were
    /// written by another structural reading and the definitions have to be read again.
    pub fn load_symbols(&self) -> Result<Option<SymbolTable>, String> {
        if !self.symbols_are_current() {
            return Ok(None);
        }
        let session = self.session();
        // In the order the rows were written, which keeps each file's rows together and in ordinal order:
        // `save_symbols` writes the files one after another and `save_symbol_changes` deletes a file's rows
        // before it writes them again. Paged by id, so the whole table is never in memory twice.
        let mut by_file: Vec<(String, Vec<Definition>)> = Vec::new();
        in_pages(&session, "SELECT id, path, name, kind, container, signature, parameters, returns, type_text, visibility, doc, line, end_line, start_line, byte_start, byte_end, likely, depth, exported, member FROM symbol WHERE id > ?1 ORDER BY id", |row| {
            let row = &row[1..];
            let path = text_at(row, 0).unwrap_or_default();
            let definition = definition_from_row(row);
            match by_file.last_mut() {
                Some((last, defs)) if *last == path => defs.push(definition),
                _ => by_file.push((path, vec![definition])),
            }
        })?;
        by_file.sort_by(|a, b| a.0.cmp(&b.0));
        let mut imports: HashMap<String, Vec<Import>> = HashMap::new();
        let rows = session
            .query_all("SELECT path, segments, alias, is_glob, byte_start, byte_end FROM import ORDER BY path, ordinal", &[])
            .map_err(|e| e.to_string())?;
        for row in &rows.rows {
            let segments: Vec<String> =
                serde_json::from_str(&text_at(row, 1).unwrap_or_default()).unwrap_or_default();
            imports.entry(text_at(row, 0).unwrap_or_default()).or_default().push(Import {
                path: segments,
                alias: text_at(row, 2),
                glob: int_at(row, 3) != 0,
                range: int_at(row, 4) as usize..int_at(row, 5) as usize,
            });
        }
        let mut table = SymbolTable::default();
        for (path, defs) in by_file {
            let own = imports.remove(&path).unwrap_or_default();
            table.set_file_with_imports(&path, defs, own);
        }
        for (path, own) in imports {
            table.set_file_with_imports(&path, Vec::new(), own);
        }
        table.shrink();
        Ok(Some(table))
    }
}

/// The text in a column of a row, if it holds text.
///
/// @param row - the row
/// @param at - the column
fn text_at(row: &[Value], at: usize) -> Option<String> {
    row.get(at).and_then(Value::text).map(str::to_owned)
}

/// The integer in a column of a row, or zero.
///
/// @param row - the row
/// @param at - the column
fn int_at(row: &[Value], at: usize) -> i64 {
    match row.get(at) {
        Some(Value::Integer(v)) => *v,
        _ => 0,
    }
}

/// A definition from a `symbol` row, in the column order `load_symbols` selects.
///
/// @param row - the row
fn definition_from_row(row: &[Value]) -> Definition {
    let kind = text_at(row, 2).and_then(|k| Kind::parse(&k)).unwrap_or(Kind::Variable);
    let parameters: Vec<(String, Option<String>)> =
        serde_json::from_str(&text_at(row, 5).unwrap_or_default()).unwrap_or_default();
    Definition {
        name: text_at(row, 1).unwrap_or_default(),
        kind: kind.family(),
        line: int_at(row, 10) as u32,
        end: int_at(row, 11) as u32,
        start: int_at(row, 12) as u32,
        signature: text_at(row, 4).unwrap_or_default(),
        likely: int_at(row, 15) != 0,
        exported: int_at(row, 17) != 0,
        depth: int_at(row, 16) as u32,
        symbol_kind: kind,
        container: text_at(row, 3),
        parameters: parameters
            .into_iter()
            .map(|(name, type_text)| Parameter { name, type_text })
            .collect(),
        returns: text_at(row, 6),
        type_text: text_at(row, 7),
        visibility: text_at(row, 8)
            .and_then(|v| Visibility::parse(&v))
            .unwrap_or(Visibility::Private),
        doc: text_at(row, 9),
        range: int_at(row, 13) as usize..int_at(row, 14) as usize,
        member: int_at(row, 18) != 0,
    }
}

/// A value for an optional text column.
///
/// @param text - the text, if any
fn optional(text: &Option<String>) -> Value {
    match text {
        Some(t) => Value::Text(t.clone()),
        None => Value::Null,
    }
}

/// Writes one file's `symbol` rows.
///
/// @param tx - the open transaction
/// @param path - the file's path
/// @param defs - its definitions
fn write_symbol_rows(
    tx: &inillucent_driver::Transaction<'_>,
    path: &str,
    defs: &[Definition],
) -> Result<(), String> {
    for (ordinal, d) in defs.iter().enumerate() {
        let parameters: Vec<(&str, Option<&str>)> =
            d.parameters.iter().map(|p| (p.name.as_str(), p.type_text.as_deref())).collect();
        tx.execute(
            "INSERT INTO symbol (path, ordinal, name, kind, container, signature, parameters, returns, type_text, visibility, doc, line, end_line, start_line, byte_start, byte_end, likely, depth, exported, member) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
            &[
                Value::Text(path.to_owned()),
                Value::Integer(ordinal as i64),
                Value::Text(d.name.clone()),
                Value::Text(d.symbol_kind.name().to_owned()),
                optional(&d.container),
                Value::Text(d.signature.clone()),
                Value::Text(serde_json::to_string(&parameters).unwrap_or_default()),
                optional(&d.returns),
                optional(&d.type_text),
                Value::Text(d.visibility.name().to_owned()),
                optional(&d.doc),
                Value::Integer(i64::from(d.line)),
                Value::Integer(i64::from(d.end)),
                Value::Integer(i64::from(d.start)),
                Value::Integer(d.range.start as i64),
                Value::Integer(d.range.end as i64),
                Value::Integer(i64::from(d.likely)),
                Value::Integer(i64::from(d.depth)),
                Value::Integer(i64::from(d.exported)),
                Value::Integer(i64::from(d.member)),
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Writes one file's `import` rows.
///
/// @param tx - the open transaction
/// @param path - the file's path
/// @param imports - its imports
fn write_import_rows(
    tx: &inillucent_driver::Transaction<'_>,
    path: &str,
    imports: &[Import],
) -> Result<(), String> {
    for (ordinal, i) in imports.iter().enumerate() {
        tx.execute(
            "INSERT INTO import (path, ordinal, segments, alias, is_glob, byte_start, byte_end) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            &[
                Value::Text(path.to_owned()),
                Value::Integer(ordinal as i64),
                Value::Text(serde_json::to_string(&i.path).unwrap_or_default()),
                optional(&i.alias),
                Value::Integer(i64::from(i.glob)),
                Value::Integer(i.range.start as i64),
                Value::Integer(i.range.end as i64),
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Writes one `file` row.
///
/// @param tx - the open transaction
/// @param id - the file id
/// @param record - the file
fn write_file_row(
    tx: &inillucent_driver::Transaction<'_>,
    id: u32,
    record: &FileRecord,
) -> Result<(), String> {
    tx.execute(
        "INSERT INTO file (id, path, lang, size, mtime_ns, content_hash, generation, is_binary, is_generated, line_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, 0, ?8)",
        &[
            Value::Integer(i64::from(id)),
            Value::Text(record.rel.clone()),
            Value::Text(extension(&record.rel)),
            Value::Integer(record.size as i64),
            Value::Integer(record.mtime_ns),
            Value::Blob(record.hash.to_vec()),
            Value::Integer(i64::from(record.binary)),
            Value::Integer(i64::from(record.lines)),
        ],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// One file's trigrams, block by block, as a blob: the block count, then each block's count and its
/// trigrams as four little endian bytes each.
///
/// @param trigrams - the trigrams of each block
fn pack_trigrams(trigrams: &[Vec<u32>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(trigrams.len() as u32).to_le_bytes());
    for block in trigrams {
        out.extend_from_slice(&(block.len() as u32).to_le_bytes());
        for t in block {
            out.extend_from_slice(&t.to_le_bytes());
        }
    }
    out
}

/// The trigrams a blob from `pack_trigrams` holds, block by block.
///
/// @param bytes - the blob
fn unpack_trigrams(bytes: &[u8]) -> Vec<Vec<u32>> {
    let word = |at: usize| {
        bytes.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut out = Vec::new();
    let mut at = 4;
    for _ in 0..word(0) {
        let count = word(at) as usize;
        at += 4;
        out.push((0..count).map(|i| word(at + i * 4)).collect());
        at += count * 4;
    }
    out
}

/// A file's extension, lower case, which is the language key until a plugin names one.
///
/// @param rel - the path
pub fn extension(rel: &str) -> String {
    rel.rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}
