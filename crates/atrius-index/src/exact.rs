//! The exact engine: literal and regex search with the same `(path, line)` results as ripgrep.
//!
//! Every file's content is held compressed in memory, beside its trigrams' posting lists. A search plans
//! a trigram query from the pattern (`plan`), intersects posting lists to get candidate files, and
//! verifies each candidate with `grep-searcher` and `grep-regex`, the crates ripgrep is built from, set
//! up as ripgrep sets them up for a directory search: line numbers on, binary detection that stops at
//! the first NUL, byte order mark sniffing, `\n` as the line terminator. A file named explicitly as the
//! search path is searched the way ripgrep searches a file named on its command line, with binary
//! detection that converts rather than stops.
//!
//! Nothing is walked and no process is started, so a search costs the candidates it verifies.
//!
//! **A posting list names blocks, not files.** A text file larger than one block is cut at line ends
//! into blocks of about 64 KB, each compressed and indexed on its own, so a minified bundle that holds
//! every common trigram costs the one block a pattern can match in rather than all nine megabytes of it.
//! A block's id is its file's id shifted left by `BLOCK_BITS` plus its number in the file, so no table
//! maps one to the other. A file with a NUL byte, or one ripgrep decodes from UTF-16, is one block,
//! because ripgrep's binary detection and its decoding work over the whole file.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::sinks::Bytes;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder};
use rayon::prelude::*;

use crate::files::{self, Found, Scope};
use crate::plan::{plan, Query};
use crate::trigram::{trigrams_of, Postings};

/// One file the index holds.
#[derive(Clone, Debug)]
pub struct FileRecord {
    /// Its path relative to the root, with forward slashes.
    pub rel: String,
    /// Its size in bytes.
    pub size: u64,
    /// Its modification time in nanoseconds since the epoch.
    pub mtime_ns: i64,
    /// The blake3 hash of its bytes.
    pub hash: [u8; 32],
    /// Its bytes in blocks, each lz4 compressed with the length in front.
    pub blocks: Vec<Block>,
    /// Whether it holds a NUL byte, which is how ripgrep decides a file is binary.
    pub binary: bool,
    /// How many lines it has.
    pub lines: u32,
}

impl FileRecord {
    /// Reads, hashes and packs a file.
    ///
    /// @param root - the root
    /// @param found - the file as the walk found it
    pub fn read(root: &Path, found: &Found) -> Option<(FileRecord, Vec<Vec<u32>>)> {
        let bytes = std::fs::read(root.join(&found.rel)).ok()?;
        Some(FileRecord::from_bytes(found.rel.clone(), found.mtime_ns, &bytes))
    }

    /// A record and its trigrams from a file's bytes.
    ///
    /// @param rel - the path relative to the root
    /// @param mtime_ns - the modification time
    /// @param bytes - the file's bytes
    pub fn from_bytes(rel: String, mtime_ns: i64, bytes: &[u8]) -> (FileRecord, Vec<Vec<u32>>) {
        let searched = searched_text(bytes);
        let binary = memchr::memchr(0, &searched).is_some();
        let whole = binary || matches!(searched, std::borrow::Cow::Owned(_));
        let spans = if whole { vec![(0, bytes.len(), 1)] } else { block_spans(bytes) };
        let mut blocks = Vec::with_capacity(spans.len());
        let mut trigrams = Vec::with_capacity(spans.len());
        for (start, end, first_line) in spans {
            blocks.push(Block {
                first_line,
                packed: lz4_flex::block::compress_prepend_size(&bytes[start..end]),
            });
            trigrams.push(if whole {
                trigrams_of(&searched)
            } else {
                trigrams_of(&bytes[start..end])
            });
        }
        let record = FileRecord {
            rel,
            size: bytes.len() as u64,
            mtime_ns,
            hash: *blake3::hash(bytes).as_bytes(),
            blocks,
            binary,
            lines: memchr::memchr_iter(b'\n', &searched).count() as u32
                + u32::from(!searched.ends_with(b"\n") && !searched.is_empty()),
        };
        (record, trigrams)
    }

    /// The file's bytes.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.size as usize);
        for block in &self.blocks {
            out.extend_from_slice(&block.bytes());
        }
        out
    }

    /// The bytes it holds in memory.
    pub fn heap_bytes(&self) -> usize {
        self.blocks.iter().map(|b| b.packed.capacity() + 16).sum::<usize>()
            + self.rel.capacity()
            + 96
    }
}

/// One block of a file: the line it starts on and its bytes, compressed.
#[derive(Clone, Debug)]
pub struct Block {
    /// The line number of its first line, from one.
    pub first_line: u32,
    /// Its bytes, lz4 compressed with the length in front.
    pub packed: Vec<u8>,
}

impl Block {
    /// The block's bytes.
    pub fn bytes(&self) -> Vec<u8> {
        lz4_flex::block::decompress_size_prepended(&self.packed).unwrap_or_default()
    }
}

/// A file's blocks as one blob for the store: the count, then each block's first line, length and
/// compressed bytes, the numbers as variable length integers.
///
/// @param blocks - the blocks
pub fn pack_blocks(blocks: &[Block]) -> Vec<u8> {
    let mut out = Vec::new();
    crate::trigram::write_varint(&mut out, blocks.len() as u32);
    for block in blocks {
        crate::trigram::write_varint(&mut out, block.first_line);
        crate::trigram::write_varint(&mut out, block.packed.len() as u32);
        out.extend_from_slice(&block.packed);
    }
    out
}

/// The blocks a blob from `pack_blocks` holds.
///
/// @param bytes - the blob
pub fn unpack_blocks(bytes: &[u8]) -> Vec<Block> {
    fn next(bytes: &[u8], at: &mut usize) -> u32 {
        let (value, used) = crate::trigram::read_varint(&bytes[(*at).min(bytes.len())..]);
        *at += used;
        value
    }
    let mut at = 0usize;
    let count = next(bytes, &mut at);
    let mut blocks = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let first_line = next(bytes, &mut at);
        let length = next(bytes, &mut at) as usize;
        let start = at.min(bytes.len());
        let end = (at + length).min(bytes.len());
        blocks.push(Block { first_line, packed: bytes[start..end].to_vec() });
        at = end;
    }
    blocks
}

/// How many bits of a block id are the block's number inside its file.
pub const BLOCK_BITS: u32 = 8;
/// The size a block is cut at, at the next line end.
pub const BLOCK_BYTES: usize = 64 * 1024;

/// The id of one block of one file.
///
/// @param file - the file's id
/// @param block - the block's number inside it
pub fn block_id(file: u32, block: usize) -> u32 {
    (file << BLOCK_BITS) | block as u32
}

/// Where a file's blocks start and end: cut at the line end after every `BLOCK_BYTES`, larger blocks
/// for a file that would otherwise have more than 2^`BLOCK_BITS` of them. Each span is `(start byte,
/// end byte, first line)`.
///
/// @param bytes - the file's bytes
fn block_spans(bytes: &[u8]) -> Vec<(usize, usize, u32)> {
    let size = BLOCK_BYTES.max(bytes.len().div_ceil(1 << BLOCK_BITS) + 1);
    let mut spans = Vec::new();
    let (mut start, mut line) = (0usize, 1u32);
    while start < bytes.len() {
        let mut end = (start + size).min(bytes.len());
        if end < bytes.len() {
            end = memchr::memchr(b'\n', &bytes[end..]).map_or(bytes.len(), |i| end + i + 1);
        }
        spans.push((start, end, line));
        line += memchr::memchr_iter(b'\n', &bytes[start..end]).count() as u32;
        start = end;
    }
    if spans.is_empty() {
        spans.push((0, 0, 1));
    }
    spans
}

/// The text ripgrep actually searches in a file: a file starting with a UTF-16 byte order mark is
/// decoded to UTF-8 first, as ripgrep's default `--encoding auto` does; anything else is searched as it
/// is.
///
/// @param bytes - the file's bytes
pub fn searched_text(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let encoding = match bytes {
        [0xFF, 0xFE, ..] => encoding_rs::UTF_16LE,
        [0xFE, 0xFF, ..] => encoding_rs::UTF_16BE,
        _ => return std::borrow::Cow::Borrowed(bytes),
    };
    let (text, _, _) = encoding.decode(bytes);
    std::borrow::Cow::Owned(text.into_owned().into_bytes())
}

/// One matching line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// The file, relative to the root.
    pub path: String,
    /// The line number, from one.
    pub line: u64,
    /// The line's text, lossily decoded, without its line ending.
    pub text: String,
}

/// What one exact search did and found.
#[derive(Debug, Default)]
pub struct ExactAnswer {
    /// The matching lines, sorted by path then line.
    pub hits: Vec<Hit>,
    /// Files in scope.
    pub in_scope: usize,
    /// Files the trigram query let through.
    pub candidates: usize,
    /// Files the regex was run over.
    pub verified: usize,
    /// Whether the pattern had no usable trigrams, so every file in scope was verified.
    pub unbounded: bool,
}

/// A search request, as the Grep tool's arguments put it.
pub struct ExactRequest<'a> {
    /// The pattern, a regex.
    pub pattern: &'a str,
    /// `-i`.
    pub case_insensitive: bool,
    /// The path, globs and types narrowing the file set.
    pub scope: &'a Scope,
}

/// The in-memory exact index of one root.
#[derive(Default, Clone)]
pub struct Exact {
    /// Files by id; a tombstoned id holds None.
    pub files: Vec<Option<FileRecord>>,
    /// Ids by path.
    pub by_path: HashMap<String, u32>,
    /// The posting lists.
    pub postings: Postings,
    tombstones: usize,
    /// Counts every insert and removal, so a copy taken for compaction can tell whether the index it
    /// was taken from changed while it was being rebuilt.
    pub generation: u64,
}

impl Exact {
    /// Builds the index of a root from scratch: walks it, reads every file in parallel, and adds them
    /// in path order.
    ///
    /// @param root - the root
    pub fn build(root: &Path) -> Exact {
        let found = files::walk(root);
        let read: Vec<Option<(FileRecord, Vec<Vec<u32>>)>> =
            found.par_iter().map(|f| FileRecord::read(root, f)).collect();
        let mut exact = Exact::default();
        for (record, trigrams) in read.into_iter().flatten() {
            exact.insert(record, &trigrams);
        }
        exact
    }

    /// A fresh index of the given files, numbered from zero with no tombstones: what compaction builds.
    /// Each file's trigrams are taken again from its bytes, in parallel.
    ///
    /// @param records - the live files
    pub fn from_records(records: Vec<FileRecord>) -> Exact {
        let read: Vec<(FileRecord, Vec<Vec<u32>>)> = records
            .into_par_iter()
            .map(|r| FileRecord::from_bytes(r.rel.clone(), r.mtime_ns, &r.bytes()))
            .collect();
        let mut exact = Exact::default();
        for (record, trigrams) in read {
            exact.insert(record, &trigrams);
        }
        exact
    }

    /// An index put back together from what the store held.
    ///
    /// @param files - records by id, None for a tombstone
    /// @param postings - the posting lists, which already cover every id
    pub fn restore(files: Vec<Option<FileRecord>>, postings: Postings) -> Exact {
        let by_path = files
            .iter()
            .enumerate()
            .filter_map(|(id, f)| f.as_ref().map(|f| (f.rel.clone(), id as u32)))
            .collect();
        let tombstones = files.iter().filter(|f| f.is_none()).count();
        Exact { files, by_path, postings, tombstones, generation: 0 }
    }

    /// Adds a file under the next id, tombstoning any older copy of the same path.
    ///
    /// @param record - the file
    /// @param trigrams - its trigrams
    pub fn insert(&mut self, record: FileRecord, trigrams: &[Vec<u32>]) {
        self.remove(&record.rel);
        self.generation += 1;
        let id = self.files.len() as u32;
        for (block, tris) in trigrams.iter().enumerate() {
            self.postings.add(block_id(id, block), tris);
        }
        self.by_path.insert(record.rel.clone(), id);
        self.files.push(Some(record));
    }

    /// Tombstones a path.
    ///
    /// @param rel - the path relative to the root
    pub fn remove(&mut self, rel: &str) {
        if let Some(id) = self.by_path.remove(rel) {
            self.files[id as usize] = None;
            self.tombstones += 1;
            self.generation += 1;
        }
    }

    /// How many ids are tombstoned, which is when the lists are worth rebuilding.
    pub fn tombstones(&self) -> usize {
        self.tombstones
    }

    /// How many live files the index holds.
    pub fn live(&self) -> usize {
        self.by_path.len()
    }

    /// The bytes the index holds in memory.
    pub fn heap_bytes(&self) -> usize {
        self.files.iter().flatten().map(FileRecord::heap_bytes).sum::<usize>()
            + self.postings.heap_bytes()
    }

    /// Runs one exact search.
    ///
    /// @param root - the root, for an explicit file the index does not hold
    /// @param request - the pattern, case and scope
    pub fn search(&self, root: &Path, request: &ExactRequest) -> Result<ExactAnswer, String> {
        let matcher = matcher(request.pattern, request.case_insensitive)?;
        if let Some(path) = request.scope.explicit_path(root) {
            return Ok(self.search_explicit(&matcher, &request.scope.path, &path));
        }
        let (candidates, in_scope, unbounded) = self.candidates(request)?;
        let mut hits: Vec<Hit> = candidates
            .par_iter()
            .flat_map_iter(|&unit| self.verify_block(&matcher, unit))
            .collect();
        hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
        Ok(ExactAnswer {
            hits,
            in_scope,
            candidates: candidates.len(),
            verified: candidates.len(),
            unbounded,
        })
    }

    /// Runs the regex over one block. A file of one block is searched whole, as ripgrep searches it; a
    /// block of a larger file is searched on its own and its line numbers moved to where it starts.
    ///
    /// @param matcher - the regex
    /// @param unit - the block's id
    fn verify_block(&self, matcher: &RegexMatcher, unit: u32) -> Vec<Hit> {
        let (file, block) = (unit >> BLOCK_BITS, (unit & ((1 << BLOCK_BITS) - 1)) as usize);
        let Some(record) = self.files.get(file as usize).and_then(Option::as_ref) else {
            return Vec::new();
        };
        let Some(piece) = record.blocks.get(block) else { return Vec::new() };
        let mut hits = verify(matcher, &record.rel, &piece.bytes(), BinaryDetection::quit(b'\x00'));
        if piece.first_line > 1 {
            for hit in &mut hits {
                hit.line += u64::from(piece.first_line - 1);
            }
        }
        hits
    }

    /// The blocks a search has to verify: those the trigram query lets through, of files inside the
    /// scope, in id order. Also how many files the scope holds, and whether the pattern had no usable
    /// trigrams.
    ///
    /// @param request - the pattern, case and scope
    pub fn candidates(&self, request: &ExactRequest) -> Result<(Vec<u32>, usize, bool), String> {
        let query = plan(request.pattern, request.case_insensitive)?;
        let unbounded = query == Query::All;
        let everything = request.scope.is_everything();
        let in_scope = if everything {
            self.by_path.len()
        } else {
            self.by_path.keys().filter(|rel| request.scope.contains(rel)).count()
        };
        let allowed = self.evaluate(&query);
        let ids = allowed
            .into_iter()
            .filter(|&unit| {
                self.files
                    .get((unit >> BLOCK_BITS) as usize)
                    .and_then(Option::as_ref)
                    .is_some_and(|r| everything || request.scope.contains(&r.rel))
            })
            .collect();
        Ok((ids, in_scope, unbounded))
    }

    /// Searches one file named as the search path, the way ripgrep searches a file named on its
    /// command line: whether or not an ignore rule covers it, and converting binary data rather than
    /// stopping at it.
    ///
    /// @param matcher - the regex
    /// @param rel - the path relative to the root
    /// @param path - the absolute path
    fn search_explicit(&self, matcher: &RegexMatcher, rel: &str, path: &PathBuf) -> ExactAnswer {
        let bytes = match self.by_path.get(rel).and_then(|&id| self.files[id as usize].as_ref()) {
            Some(record) => record.bytes(),
            None => std::fs::read(path).unwrap_or_default(),
        };
        let hits = verify(matcher, rel, &bytes, BinaryDetection::convert(b'\x00'));
        ExactAnswer { hits, in_scope: 1, candidates: 1, verified: 1, unbounded: false }
    }

    /// The sorted ids of the files a trigram query lets through.
    ///
    /// @param query - the query, never `All` at the top
    fn evaluate(&self, query: &Query) -> Vec<u32> {
        match query {
            Query::All => {
                let mut ids: Vec<u32> = self
                    .by_path
                    .values()
                    .filter_map(|&file| {
                        self.files[file as usize].as_ref().map(|r| (file, r.blocks.len()))
                    })
                    .flat_map(|(file, blocks)| (0..blocks).map(move |b| block_id(file, b)))
                    .collect();
                ids.sort_unstable();
                ids
            }
            Query::Tri(t) => self.postings.files_with(*t),
            Query::And(parts) => {
                let mut parts: Vec<&Query> = parts.iter().collect();
                parts.sort_by_key(|q| match q {
                    Query::Tri(t) => self.postings.count(*t),
                    _ => usize::MAX,
                });
                let mut acc = self.evaluate(parts[0]);
                for p in &parts[1..] {
                    if acc.is_empty() {
                        break;
                    }
                    acc = intersect(&acc, &self.evaluate(p));
                }
                acc
            }
            Query::Or(parts) => {
                let mut acc: Vec<u32> = parts.iter().flat_map(|p| self.evaluate(p)).collect();
                acc.sort_unstable();
                acc.dedup();
                acc
            }
        }
    }
}

/// The intersection of two sorted id lists.
///
/// @param a - a sorted list
/// @param b - a sorted list
fn intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}

/// The regex matcher ripgrep builds for a pattern with the Grep tool's arguments.
///
/// @param pattern - the pattern
/// @param case_insensitive - `-i`
pub fn matcher(pattern: &str, case_insensitive: bool) -> Result<RegexMatcher, String> {
    RegexMatcherBuilder::new()
        .case_insensitive(case_insensitive)
        .case_smart(false)
        .multi_line(false)
        .unicode(true)
        .octal(false)
        .line_terminator(Some(b'\n'))
        .build(pattern)
        .map_err(|e| e.to_string())
}

/// Runs the regex over one file's bytes as ripgrep's searcher would, and returns the matching lines.
///
/// @param matcher - the regex
/// @param rel - the file's path, for the hits
/// @param bytes - the file's bytes
/// @param binary - how binary data is treated
pub fn verify(
    matcher: &RegexMatcher,
    rel: &str,
    bytes: &[u8],
    binary: BinaryDetection,
) -> Vec<Hit> {
    if binary.convert_byte().is_some() && memchr::memchr(0, bytes).is_some() {
        return verify_converted(matcher, rel, bytes, binary);
    }
    let mut searcher: Searcher = SearcherBuilder::new()
        .line_number(true)
        .binary_detection(binary)
        .bom_sniffing(true)
        .build();
    let mut hits = Vec::new();
    let sink = Bytes(|line, text| {
        let text = String::from_utf8_lossy(text);
        hits.push(Hit {
            path: rel.to_owned(),
            line,
            text: text.trim_end_matches(['\n', '\r']).to_owned(),
        });
        Ok(true)
    });
    // Bytes with no NUL and no byte order mark are searched in place, as ripgrep searches a memory
    // map, which avoids copying a long line into the reader's buffer. Anything else goes through the
    // reader, whose buffering decides which matches before a NUL ripgrep reports.
    let plain = memchr::memchr(0, bytes).is_none()
        && !bytes.starts_with(&[0xFF, 0xFE])
        && !bytes.starts_with(&[0xFE, 0xFF])
        && !bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let _ = if plain {
        searcher.search_slice(matcher, bytes, sink)
    } else {
        searcher.search_reader(matcher, Cursor::new(bytes), sink)
    };
    hits
}

/// Searches a binary file named as the search path. The searcher turns each NUL into a line break to
/// keep going, so the line numbers it counts include those breaks; ripgrep reports the file's real line,
/// so each match's byte offset is mapped back to the line it is on, and a real line is reported once.
///
/// @param matcher - the regex
/// @param rel - the file's path
/// @param bytes - the file's bytes
/// @param binary - convert mode
fn verify_converted(
    matcher: &RegexMatcher,
    rel: &str,
    bytes: &[u8],
    binary: BinaryDetection,
) -> Vec<Hit> {
    struct Offsets(Vec<u64>);
    impl grep_searcher::Sink for Offsets {
        type Error = std::io::Error;
        fn matched(
            &mut self,
            _: &Searcher,
            m: &grep_searcher::SinkMatch<'_>,
        ) -> Result<bool, std::io::Error> {
            self.0.push(m.absolute_byte_offset());
            Ok(true)
        }
    }
    let mut searcher: Searcher = SearcherBuilder::new()
        .line_number(false)
        .binary_detection(binary)
        .bom_sniffing(true)
        .build();
    let mut offsets = Offsets(Vec::new());
    let _ = searcher.search_reader(matcher, Cursor::new(bytes), &mut offsets);
    let mut hits: Vec<Hit> = Vec::new();
    for offset in offsets.0 {
        let at = (offset as usize).min(bytes.len());
        let line = memchr::memchr_iter(b'\n', &bytes[..at]).count() as u64 + 1;
        if hits.last().is_some_and(|h| h.line == line) {
            continue;
        }
        let start = memchr::memrchr(b'\n', &bytes[..at]).map_or(0, |i| i + 1);
        let end = memchr::memchr(b'\n', &bytes[at..]).map_or(bytes.len(), |i| at + i);
        let text = String::from_utf8_lossy(&bytes[start..end]).replace('\0', " ");
        hits.push(Hit { path: rel.to_owned(), line, text: text.trim_end_matches('\r').to_owned() });
    }
    hits
}
