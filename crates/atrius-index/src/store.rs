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
use crate::trigram::{Posting, Postings};

/// The schema version of the tables this module writes. A different number rebuilds everything.
pub const SCHEMA_VERSION: &str = "2";
/// The trigram engine's version: how bytes are folded and which trigrams are kept.
pub const TRIGRAM_VERSION: &str = "1";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS file (id INTEGER PRIMARY KEY, path TEXT UNIQUE, lang TEXT, size INTEGER,
    mtime_ns INTEGER, content_hash BLOB, generation INTEGER, is_binary INTEGER, is_generated INTEGER,
    line_count INTEGER);
CREATE TABLE IF NOT EXISTS blob (content_hash BLOB PRIMARY KEY, bytes BLOB);
CREATE TABLE IF NOT EXISTS trigram (tri INTEGER PRIMARY KEY, postings BLOB);
CREATE TABLE IF NOT EXISTS trigram_delta (file_id INTEGER PRIMARY KEY, tris BLOB, tombstone INTEGER);
";

/// The index file of one checkout.
pub struct Store {
    database: Database,
    path: PathBuf,
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

impl Store {
    /// Opens or creates the index file in a folder, with its schema.
    ///
    /// @param folder - the index folder of one checkout
    pub fn open(folder: &Path) -> Result<Store, String> {
        std::fs::create_dir_all(folder)
            .map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
        let path = folder.join("index.rdb");
        let options = OpenOptions { create: true, ..OpenOptions::default() };
        let database = Database::open_with(&path, options).map_err(|e| e.to_string())?;
        database.session().execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        Ok(Store { database, path })
    }

    /// The file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A connection for one unit of work.
    pub fn session(&self) -> Connection<'_> {
        self.database.session()
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
                tx.execute(
                    "INSERT INTO blob (content_hash, bytes) VALUES (?1, ?2)",
                    &[Value::Blob(record.hash.to_vec()), Value::Blob(pack_blocks(&record.blocks))],
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
        let base = session
            .query_all("SELECT tri, postings FROM trigram", &[])
            .map_err(|e| e.to_string())?;
        for row in &base.rows {
            if let (Some(Value::Integer(tri)), Some(bytes)) =
                (row.first(), row.get(1).and_then(Value::bytes))
            {
                postings.insert(*tri as u32, Posting::from_bytes(bytes.to_vec()));
            }
        }
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
        let rows = session
            .query_all("SELECT f.id, f.path, f.size, f.mtime_ns, f.content_hash, f.is_binary, f.line_count, b.bytes FROM file f JOIN blob b ON b.content_hash = f.content_hash", &[])
            .map_err(|e| e.to_string())?;
        let mut files: Vec<Option<FileRecord>> = vec![None; next_id];
        for row in &rows.rows {
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
            });
        }
        Ok(Exact::restore(files, postings))
    }
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
