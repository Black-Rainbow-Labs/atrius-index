//! Passage search: questions written in plain English, answered from chunks
//! (`tasks/task-2138-unluminous-code-index-tdd.md` §6.2, §6.8, R2, R3, R8).
//!
//! Every chunk of every file a bundled language claims, and of Markdown and plain text, is a row of an
//! `inillucent_search` table: its header (the path and the enclosing signatures), its body, and the
//! split identifier words the engine does not make itself. Three facets narrow a search inside it:
//! `lang` (the extension), `kind` (code, test or docs) and `path_prefix` (the top folder). The
//! `chunk` table says where each row came from.
//!
//! Files the index holds for exact search only are left out: binary files, files over 1 MB, minified
//! and lock files, and files that look like secrets (`.env*`, `*.pem`, `id_*`), so none of them can
//! outrank source and none of their text reaches an answer about meaning (§6.3, §6.12).
//!
//! The table lives in the store's file and is only touched on the store thread, because the engine's
//! `Database` cannot leave the thread that opened it.

use inillucent_driver::{Connection, Value};

use crate::exact::FileRecord;
use crate::outline::{self, CHUNK_BUDGET};
use crate::symbols::is_secondary;
use crate::words;

/// The version of how chunks are cut and what a row holds. A different value rebuilds the table.
pub const PASSAGE_VERSION: &str = "2";

/// How the passage table combines its word ranking with its vector ranking, as the words of its
/// declaration. A fixed weight of 0.5 by default (R8). `ATRIUS_PASSAGE_FUSION` names another while
/// the improvement loop compares them (lever 7): `adaptive`, `rrf` or `weighted:<weight>`. The vectors
/// are cached by chunk hash, so trying one rebuilds the table and embeds nothing.
pub fn fusion() -> String {
    match std::env::var("ATRIUS_PASSAGE_FUSION").ok().as_deref().map(str::trim) {
        Some("adaptive") => String::new(),
        Some("rrf") => ", fusion = 'rrf'".to_owned(),
        Some(other) if other.starts_with("weighted:") => {
            match other["weighted:".len()..].parse::<f64>() {
                Ok(weight) if (0.0..=1.0).contains(&weight) => {
                    format!(", fusion = 'weighted', vector_weight = {weight}")
                }
                _ => ", fusion = 'weighted', vector_weight = 0.5".to_owned(),
            }
        }
        _ => ", fusion = 'weighted', vector_weight = 0.5".to_owned(),
    }
}

/// The passage version written beside the table: `PASSAGE_VERSION` and the fusion it was made with,
/// so a change of either rebuilds it.
pub fn passage_version() -> String {
    format!("{PASSAGE_VERSION}{}", fusion())
}

/// The embedding model's width: `nomic-embed-text-v1.5`, the one model `embed()` serves (R6).
pub const DIMENSIONS: usize = 768;
/// The model a cached vector was made by, so a change of model never reuses a vector.
pub const MODEL: &str = "nomic-embed-text-v1.5";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS chunk (id INTEGER PRIMARY KEY, path TEXT, start_line INTEGER, end_line INTEGER,
    kind TEXT, symbol TEXT, header TEXT, chunk_hash BLOB, embedded INTEGER DEFAULT 0);
CREATE INDEX IF NOT EXISTS chunk_path ON chunk(path);
CREATE INDEX IF NOT EXISTS chunk_embedded ON chunk(embedded);
";

/// One chunk a passage search found.
#[derive(Clone, Debug)]
pub struct PassageHit {
    /// The file.
    pub path: String,
    /// Its first line.
    pub start: u32,
    /// Its last line.
    pub end: u32,
    /// Its header.
    pub header: String,
    /// The engine's score.
    pub score: f64,
    /// The engine's confidence, 0 to 1.
    pub confidence: f64,
}

/// Whether a file's chunks belong in the passage table.
///
/// @param record - the file
pub fn is_passage_source(record: &FileRecord) -> bool {
    let lower = record.rel.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let generated = name.ends_with(".lock")
        || name.contains(".min.")
        || name == "package-lock.json"
        || name.ends_with(".map");
    let secret = name.starts_with(".env")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.starts_with("id_");
    let readable = crate::grammars::for_path(&record.rel).is_some()
        || name.ends_with(".md")
        || name.ends_with(".txt");
    !record.binary && record.size <= 1_048_576 && !generated && !secret && readable
}

/// What kind of file a path is, for the `kind` facet.
///
/// @param path - the path
pub fn kind_of(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".txt") {
        "docs"
    } else if is_secondary(path) {
        "test"
    } else {
        "code"
    }
}

/// The top folder of a path, for the `path_prefix` facet; empty for a file at the root.
///
/// @param path - the path
pub fn prefix_of(path: &str) -> &str {
    path.split_once('/').map_or("", |(top, _)| top)
}

/// Creates the passage and chunk tables when they are missing.
///
/// @param conn - a connection on the store thread
pub fn create(conn: &Connection<'_>) -> Result<(), String> {
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    let exists = conn
        .query("SELECT name FROM sqlite_schema WHERE name = 'passage'", &[], 1)
        .map_err(|e| e.to_string())?;
    if exists.rows.is_empty() {
        conn.execute(&format!("CREATE VIRTUAL TABLE passage USING inillucent_search(header, body, words, lang FACET, kind FACET, path_prefix FACET, dims = {DIMENSIONS}{}, tokenize = 'porter')", fusion()), &[])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Writes every chunk of every passage source, replacing what the tables held, in one transaction.
///
/// @param conn - a connection on the store thread
/// @param records - the files, copied out of the index so no lock is held while this runs
pub fn rebuild(conn: &Connection<'_>, records: &[FileRecord]) -> Result<usize, String> {
    // Dropped rather than emptied, because a version change can change the table's own declaration.
    let _ = conn.execute("DROP TABLE IF EXISTS passage", &[]);
    let _ = conn.execute("DROP TABLE IF EXISTS chunk", &[]);
    create(conn)?;
    let tx = conn.begin().map_err(|e| e.to_string())?;
    let mut next = 1i64;
    for record in records.iter().filter(|r| is_passage_source(r)) {
        next = write_file(&tx, record, next)?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    let _ = conn.execute("INSERT INTO passage(passage) VALUES('compact')", &[]);
    Ok((next - 1) as usize)
}

/// Replaces the chunks of files that changed and drops those of files that are gone.
///
/// @param conn - a connection on the store thread
/// @param removed - paths gone from the set
/// @param changed - files read again
pub fn update(
    conn: &Connection<'_>,
    removed: &[String],
    changed: &[&FileRecord],
) -> Result<(), String> {
    let tx = conn.begin().map_err(|e| e.to_string())?;
    let next =
        match tx.query("SELECT max(id) FROM chunk", &[], 1).map_err(|e| e.to_string())?.value(0, 0)
        {
            Some(Value::Integer(n)) => n + 1,
            _ => 1,
        };
    for path in removed.iter().map(String::as_str).chain(changed.iter().map(|r| r.rel.as_str())) {
        let ids = tx
            .query(
                "SELECT id FROM chunk WHERE path = ?1",
                &[Value::Text(path.to_owned())],
                usize::MAX,
            )
            .map_err(|e| e.to_string())?;
        for row in &ids.rows {
            if let Some(Value::Integer(id)) = row.first() {
                tx.execute("DELETE FROM passage WHERE rowid = ?1", &[Value::Integer(*id)])
                    .map_err(|e| e.to_string())?;
            }
        }
        tx.execute("DELETE FROM chunk WHERE path = ?1", &[Value::Text(path.to_owned())])
            .map_err(|e| e.to_string())?;
    }
    let mut next = next;
    for record in changed.iter().filter(|r| is_passage_source(r)) {
        next = write_file(&tx, record, next)?;
    }
    tx.commit().map_err(|e| e.to_string())
}

/// Writes one file's chunks from a given id, and returns the next free id.
///
/// @param tx - the open transaction
/// @param record - the file
/// @param first - the first id to use
fn write_file(
    tx: &inillucent_driver::Transaction<'_>,
    record: &FileRecord,
    first: i64,
) -> Result<i64, String> {
    let bytes = record.bytes();
    let text = String::from_utf8_lossy(&bytes);
    let read = outline::read(&record.rel, &text, CHUNK_BUDGET);
    let mut id = first;
    let (lang, kind, prefix) = (
        crate::store::extension(&record.rel),
        kind_of(&record.rel),
        prefix_of(&record.rel).to_owned(),
    );
    for chunk in &read.chunks {
        let hash = blake3::hash(format!("{}\n{}", chunk.header, chunk.body).as_bytes());
        tx.execute(
            "INSERT INTO chunk (id, path, start_line, end_line, kind, symbol, header, chunk_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            &[Value::Integer(id), Value::Text(record.rel.clone()), Value::Integer(i64::from(chunk.start)), Value::Integer(i64::from(chunk.end)), Value::Text(chunk.kind.into()), chunk.symbol.clone().map_or(Value::Null, Value::Text), Value::Text(chunk.header.clone()), Value::Blob(hash.as_bytes().to_vec())],
        )
        .map_err(|e| e.to_string())?;
        let searched_words =
            format!("{} {}", words::search_words(&chunk.header), words::search_words(&chunk.body));
        tx.execute(
            "INSERT INTO passage (rowid, header, body, words, lang, kind, path_prefix) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            &[Value::Integer(id), Value::Text(chunk.header.clone()), Value::Text(chunk.body.clone()), Value::Text(searched_words), Value::Text(lang.clone()), Value::Text(kind.into()), Value::Text(prefix.clone())],
        )
        .map_err(|e| e.to_string())?;
        id += 1;
    }
    Ok(id)
}

/// Whether `embed()` can run here: the engine was built with it and the person installed the model.
/// Unluminous never downloads one (R6).
///
/// @param conn - a connection on the store thread
pub fn can_embed(conn: &Connection<'_>) -> bool {
    conn.query("SELECT length(embed('search_query: ready'))", &[], 1).is_ok_and(
        |rows| matches!(rows.value(0, 0), Some(Value::Integer(n)) if *n as usize == DIMENSIONS * 4),
    )
}

/// How many chunks have a vector, and how many there are.
///
/// @param conn - a connection on the store thread
pub fn vector_counts(conn: &Connection<'_>) -> (usize, usize) {
    let count = |sql: &str| {
        conn.query(sql, &[], 1)
            .ok()
            .and_then(|r| match r.value(0, 0) {
                Some(Value::Integer(n)) => Some(*n as usize),
                _ => None,
            })
            .unwrap_or(0)
    };
    (count("SELECT count(*) FROM chunk WHERE embedded = 1"), count("SELECT count(*) FROM chunk"))
}

/// Embeds up to `batch` chunks that have no vector yet, taking each from the shared cache when another
/// checkout, an earlier edit or another repository already embedded the same text. Returns how many
/// chunks were given a vector, so the caller knows when there is nothing left.
///
/// The model runs first, with no transaction open, so a search is never kept waiting while it works.
/// The vectors are then written in one transaction, and the new ones go into the cache in one more.
/// Writing each row as its own commit cost two commits a chunk and held the hosts at about four
/// chunks a second with the card nearly idle.
///
/// @param conn - a connection on the store thread
/// @param cache - the shared embedding cache, if it could be opened
/// @param batch - how many chunks at most
pub fn embed_pending(
    conn: &Connection<'_>,
    cache: Option<&Connection<'_>>,
    batch: usize,
) -> Result<usize, String> {
    let rows = conn
        .query("SELECT c.id, c.chunk_hash, p.header, p.body FROM chunk c JOIN passage p ON p.rowid = c.id WHERE c.embedded = 0 LIMIT ?1", &[Value::Integer(batch as i64)], batch)
        .map_err(|e| e.to_string())?;
    let mut made: Vec<(i64, Vec<u8>, Vec<u8>, bool)> = Vec::new();
    for row in &rows.rows {
        let (Some(Value::Integer(id)), Some(hash)) =
            (row.first(), row.get(1).and_then(Value::bytes))
        else {
            continue;
        };
        if let Some(vector) = cached_vector(cache, hash) {
            made.push((*id, hash.to_vec(), vector, false));
            continue;
        }
        let text = format!(
            "search_document: {}\n{}",
            row.get(2).and_then(Value::text).unwrap_or_default(),
            row.get(3).and_then(Value::text).unwrap_or_default()
        );
        let answer = conn
            .query("SELECT embed(?1)", &[Value::Text(text.chars().take(6000).collect())], 1)
            .map_err(|e| e.to_string())?;
        if let Some(vector) = answer.value(0, 0).and_then(Value::bytes) {
            made.push((*id, hash.to_vec(), vector.to_vec(), true));
        }
    }
    write_vectors(conn, &made)?;
    if let Some(cache) = cache {
        remember_vectors(cache, &made);
    }
    Ok(made.len())
}

/// The vector the shared cache holds for a chunk's text, if any.
///
/// @param cache - the shared embedding cache, if it could be opened
/// @param hash - the chunk's hash
fn cached_vector(cache: Option<&Connection<'_>>, hash: &[u8]) -> Option<Vec<u8>> {
    let rows = cache?
        .query(
            "SELECT vector FROM emb WHERE hash = ?1 AND model = ?2",
            &[Value::Blob(hash.to_vec()), Value::Text(MODEL.into())],
            1,
        )
        .ok()?;
    rows.value(0, 0).and_then(Value::bytes).map(<[u8]>::to_vec)
}

/// Writes a batch of vectors into the passage table and marks their chunks embedded, in one transaction.
///
/// @param conn - a connection on the store thread
/// @param made - (chunk id, hash, vector, newly made) for each chunk
fn write_vectors(
    conn: &Connection<'_>,
    made: &[(i64, Vec<u8>, Vec<u8>, bool)],
) -> Result<(), String> {
    if made.is_empty() {
        return Ok(());
    }
    let tx = conn.begin().map_err(|e| e.to_string())?;
    for (id, _, vector, _) in made {
        tx.execute(
            "UPDATE passage SET vector = ?1 WHERE rowid = ?2",
            &[Value::Blob(vector.clone()), Value::Integer(*id)],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("UPDATE chunk SET embedded = 1 WHERE id = ?1", &[Value::Integer(*id)])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}

/// Puts the vectors this batch made into the shared cache, in one transaction. A failure here costs a
/// later checkout an embedding and nothing else, so it is not reported.
///
/// @param cache - the shared embedding cache
/// @param made - (chunk id, hash, vector, newly made) for each chunk
fn remember_vectors(cache: &Connection<'_>, made: &[(i64, Vec<u8>, Vec<u8>, bool)]) {
    if !made.iter().any(|m| m.3) {
        return;
    }
    let Ok(tx) = cache.begin() else { return };
    for (_, hash, vector, _) in made.iter().filter(|m| m.3) {
        let _ = tx.execute(
            "INSERT OR REPLACE INTO emb (hash, model, vector) VALUES (?1, ?2, ?3)",
            &[Value::Blob(hash.clone()), Value::Text(MODEL.into()), Value::Blob(vector.clone())],
        );
    }
    let _ = tx.commit();
}

/// The words of a question as an engine query: each word quoted, any of them may match, so a chunk
/// holding more of them ranks higher.
///
/// @param question - the question
pub fn match_query(question: &str) -> Option<String> {
    let terms = words::query_words(question);
    if terms.is_empty() {
        return None;
    }
    Some(
        terms
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" OR "),
    )
}

/// The chunks that best answer a question, best first.
///
/// With no kind given, code and tests come before documents. A question asked in English shares far
/// more words with a design document than with the code it describes, so on the dev split a search
/// over every kind put `tasks/*.md` above the function being asked about in most answers. So code
/// and tests come first, in the engine's order, and documents follow them.
///
/// @param conn - a connection on the store thread
/// @param question - the question
/// @param k - how many to retrieve
/// @param kind - only chunks of this kind, if given
/// @param vectors - whether the table has vectors, so the search is hybrid
pub fn search(
    conn: &Connection<'_>,
    question: &str,
    k: usize,
    kind: Option<&str>,
    vectors: bool,
) -> Result<Vec<PassageHit>, String> {
    let Some(query) = match_query(question) else { return Ok(Vec::new()) };
    // The question as the model is asked it. Embedded once and bound as bytes, the vector was taken
    // as something other than a query vector and every search fell back to its words (F6 went from
    // 0.15 to 0.02 on the dev split), so each search embeds it in its own statement.
    let vector = vectors.then(|| Value::Text(format!("search_query: {question}")));
    if let Some(kind) = kind {
        return search_kind(conn, &query, vector.as_ref(), k, Some(kind));
    }
    // One search over every kind, with documents moved after code and tests here. Filtering on the
    // `kind` facet in a hybrid search returned the same few large files for every question, the
    // ranking of the words alone (F6 0.15 to 0.04 on the dev split), so the facet is not used for this.
    let found = search_kind(conn, &query, vector.as_ref(), k * 2, None)?;
    let (mut first, docs): (Vec<_>, Vec<_>) =
        found.into_iter().partition(|hit| kind_of(&hit.path) != "docs");
    first.truncate(k);
    first.extend(docs.into_iter().take(10));
    Ok(first)
}

/// One search of the passage table: the words, and the question's vector when there is one.
///
/// @param conn - a connection on the store thread
/// @param query - the words as an engine query
/// @param vector - the question as the model is asked it, if the search is hybrid
/// @param k - how many to retrieve
/// @param kind - only chunks of this kind, if given
fn search_kind(
    conn: &Connection<'_>,
    query: &str,
    vector: Option<&Value>,
    k: usize,
    kind: Option<&str>,
) -> Result<Vec<PassageHit>, String> {
    let mut params = vec![Value::Text(query.to_owned()), Value::Integer(k as i64)];
    let mut filter = String::new();
    if let Some(kind) = kind {
        params.push(Value::Text(kind.to_owned()));
        filter.push_str(&format!(" AND p.kind = ?{}", params.len()));
    }
    // With a vector the search is hybrid and the engine blends the two rankings (R2, R8). Without one
    // it is the words alone.
    if let Some(vector) = vector {
        params.push(vector.clone());
        filter.push_str(&format!(" AND vector = embed(?{})", params.len()));
    }
    let sql = format!(
        "SELECT p.rowid, score(passage), confidence(passage), c.path, c.start_line, c.end_line, c.header FROM passage p JOIN chunk c ON c.id = p.rowid WHERE passage MATCH ?1 AND k = ?2{filter} ORDER BY rank"
    );
    let rows = conn.query(&sql, &params, k).map_err(|e| e.to_string())?;
    let real = |v: Option<&Value>| match v {
        Some(Value::Real(r)) => *r,
        Some(Value::Integer(i)) => *i as f64,
        _ => 0.0,
    };
    let int = |v: Option<&Value>| match v {
        Some(Value::Integer(i)) => *i as u32,
        _ => 0,
    };
    Ok(rows
        .rows
        .iter()
        .map(|row| PassageHit {
            path: row.get(3).and_then(Value::text).unwrap_or_default().to_owned(),
            start: int(row.get(4)),
            end: int(row.get(5)),
            header: row.get(6).and_then(Value::text).unwrap_or_default().to_owned(),
            score: real(row.get(1)),
            confidence: real(row.get(2)),
        })
        .collect())
}
