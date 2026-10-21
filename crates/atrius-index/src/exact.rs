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
        let reading = read_bounded(&root.join(&found.rel)).ok()?;
        Some(FileRecord::from_reading(found.rel.clone(), found.mtime_ns, reading))
    }

    /// A record and its trigrams from what [`read_bounded`] read of a file.
    ///
    /// @param rel - the path relative to the root
    /// @param mtime_ns - the modification time
    /// @param reading - what was read
    pub fn from_reading(
        rel: String,
        mtime_ns: i64,
        reading: Reading,
    ) -> (FileRecord, Vec<Vec<u32>>) {
        match reading {
            Reading::Whole(bytes) => FileRecord::from_bytes(rel, mtime_ns, &bytes),
            Reading::UpToNul { held, size } => {
                let (mut record, trigrams) = FileRecord::from_bytes(rel, mtime_ns, &held);
                record.size = size;
                record.hash = reading_hash(&held, size, mtime_ns, false);
                (record, trigrams)
            }
            Reading::Large { size } => {
                let record = FileRecord {
                    rel,
                    size,
                    mtime_ns,
                    hash: reading_hash(&[], size, mtime_ns, true),
                    blocks: Vec::new(),
                    binary: false,
                    lines: 0,
                };
                (record, Vec::new())
            }
        }
    }

    /// True for a text file larger than [`LARGEST_HELD`]: the record holds nothing of it and has no
    /// trigrams, and every search reads it from the disk as it goes. See [`read_bounded`].
    pub fn is_large(&self) -> bool {
        self.blocks.is_empty() && self.size > 0
    }

    /// A record and its trigrams from a file's bytes.
    ///
    /// @param rel - the path relative to the root
    /// @param mtime_ns - the modification time
    /// @param bytes - the file's bytes
    pub fn from_bytes(rel: String, mtime_ns: i64, bytes: &[u8]) -> (FileRecord, Vec<Vec<u32>>) {
        let searched = searched_text(bytes);
        let first_nul = memchr::memchr(0, &searched);
        let binary = first_nul.is_some();
        let decoded = matches!(searched, std::borrow::Cow::Owned(_));
        let whole = binary || decoded;
        // **A binary file is held only up to its first NUL** (Unluminous `task-2279`). A directory search
        // stops reading a file at its first NUL, which is ripgrep's rule and `verify_block`'s, so nothing
        // after it can ever be a hit, and a search naming the file reads it from the disk. The 60 MB of
        // screenshots in Unluminous's own repository were held whole in every window that hosted its index.
        // The NUL itself is kept, because the line it is on is not a hit either and the search has to see it.
        let held = match (first_nul, decoded) {
            (Some(at), false) => at + 1,
            _ => bytes.len(),
        };
        let spans = if whole { vec![(0, held, 1)] } else { block_spans(bytes) };
        let mut blocks = Vec::with_capacity(spans.len());
        let mut trigrams = Vec::with_capacity(spans.len());
        for (start, end, first_line) in spans {
            blocks.push(Block {
                first_line,
                packed: lz4_flex::block::compress_prepend_size(&bytes[start..end]),
            });
            trigrams.push(if decoded { trigrams_of(&searched) } else { trigrams_of(&bytes[start..end]) });
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

    /// True when the record holds only the part of a binary file up to its first NUL, so the whole of
    /// it is on the disk and nowhere else. See [`FileRecord::from_bytes`].
    pub fn is_partial(&self) -> bool {
        self.binary && self.held_len() < self.size
    }

    /// The record holding no more of a binary file than [`FileRecord::from_bytes`] would hold now: an index
    /// file written before 0.2.2 kept every byte, so a record loaded from one is cut at its first NUL as it
    /// is loaded, and its memory is given back without the file being built again.
    pub fn held_to_its_first_nul(mut self) -> FileRecord {
        if !self.binary || self.is_partial() || self.blocks.len() != 1 {
            return self;
        }
        let bytes = self.blocks[0].bytes();
        if matches!(searched_text(&bytes), std::borrow::Cow::Owned(_)) {
            return self;
        }
        if let Some(at) = memchr::memchr(0, &bytes) {
            self.blocks[0].packed = lz4_flex::block::compress_prepend_size(&bytes[..=at]);
        }
        self
    }

    /// How many of the file's bytes the record covers, read from the length each block carries.
    fn held_len(&self) -> u64 {
        self.blocks.iter().map(|block| u64::from(block.len())).sum()
    }

    /// Whether the record holds its content in memory. See [`set_holding_content`].
    pub fn is_held(&self) -> bool {
        self.blocks.iter().all(Block::is_held)
    }

    /// Lets go of the content, keeping each block's first line and length.
    pub fn let_go_of_content(&mut self) {
        for block in &mut self.blocks {
            block.let_go();
        }
    }

    /// The bytes the record covers: from memory when it holds them, and otherwise from the file, cut at
    /// its first NUL as [`FileRecord::from_bytes`] cut it. None when the file cannot be read.
    ///
    /// @param root - the root the record's path is under
    pub fn content(&self, root: Option<&Path>) -> Option<Vec<u8>> {
        if self.is_large() {
            return None;
        }
        if self.is_held() {
            return Some(self.bytes());
        }
        let mut bytes = match read_bounded(&root?.join(&self.rel)).ok()? {
            Reading::Whole(bytes) | Reading::UpToNul { held: bytes, .. } => bytes,
            Reading::Large { .. } => return None,
        };
        if self.binary && matches!(searched_text(&bytes), std::borrow::Cow::Borrowed(_)) {
            if let Some(at) = memchr::memchr(0, &bytes) {
                bytes.truncate(at + 1);
            }
        }
        Some(bytes)
    }

    /// One block's bytes: from memory when the record holds them, and otherwise the same span of the file
    /// as it is now, worked out by the rule the record was made with.
    ///
    /// @param block - the block
    /// @param root - the root the record's path is under
    pub fn block_bytes(&self, block: usize, root: Option<&Path>) -> Vec<u8> {
        if let Some(piece) = self.blocks.get(block).filter(|piece| piece.is_held()) {
            return piece.bytes();
        }
        let Some(bytes) = self.content(root) else { return Vec::new() };
        let whole = self.binary || matches!(searched_text(&bytes), std::borrow::Cow::Owned(_));
        if whole {
            return if block == 0 { bytes } else { Vec::new() };
        }
        block_spans(&bytes).get(block).map_or_else(Vec::new, |&(start, end, _)| bytes[start..end].to_vec())
    }

    /// The bytes the record holds: the whole file, or for a partial record the part up to its first NUL.
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

/// Whether the index holds every file's content in memory. Read from `ATRIUS_HOLD_CONTENT` the first time
/// it is asked, on unless that says `off`, and changed by [`set_holding_content`].
static HOLDING: std::sync::OnceLock<std::sync::atomic::AtomicBool> = std::sync::OnceLock::new();

/// The switch, made from `ATRIUS_HOLD_CONTENT` the first time.
fn holding_switch() -> &'static std::sync::atomic::AtomicBool {
    HOLDING.get_or_init(|| {
        let off = std::env::var("ATRIUS_HOLD_CONTENT")
            .is_ok_and(|v| v.trim().eq_ignore_ascii_case("off"));
        std::sync::atomic::AtomicBool::new(!off)
    })
}

/// Whether the index holds file content in memory: on unless [`set_holding_content`] or
/// `ATRIUS_HOLD_CONTENT=off` said otherwise.
pub fn holding_content() -> bool {
    holding_switch().load(std::sync::atomic::Ordering::SeqCst)
}

/// Whether an index keeps every file's content in memory, for the whole process.
///
/// On, which is how Atrius has always worked, a search verifies its candidates without touching the disk.
/// Off, a record keeps only its blocks' first lines and lengths once the store has written it and its
/// definitions and passages have been read, and a search reads each candidate file from the disk, the way
/// ripgrep reads every file. The answers are the same, because the gate that runs before every search has
/// already brought the index up to date with the files. An editor that hosts its project's index sets it
/// off (Unluminous `task-2279`, where the content was 11.4 MB of a 2,000 file checkout's index).
///
/// @param on - whether to hold content
pub fn set_holding_content(on: bool) {
    holding_switch().store(on, std::sync::atomic::Ordering::SeqCst);
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
    /// The block's bytes, or nothing when it does not hold them.
    pub fn bytes(&self) -> Vec<u8> {
        lz4_flex::block::decompress_size_prepended(&self.packed).unwrap_or_default()
    }

    /// How many bytes the block covers, from the length its packed form starts with.
    pub fn len(&self) -> u32 {
        self.packed.get(..4).map_or(0, |n| u32::from_le_bytes([n[0], n[1], n[2], n[3]]))
    }

    /// Whether the block covers nothing.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the block holds its bytes. A block that let go of them keeps only the four bytes of its
    /// length, and compressed bytes of anything at all are longer than that.
    pub fn is_held(&self) -> bool {
        self.packed.len() != 4 || self.is_empty()
    }

    /// Keeps the length and lets go of the bytes.
    fn let_go(&mut self) {
        if self.is_held() && !self.is_empty() {
            self.packed.truncate(4);
            self.packed.shrink_to_fit();
        }
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

/// The largest file read into memory whole. A text file larger than this is never read whole and never
/// held: it is a record with no blocks and no trigrams, every search is a candidate for it, and the
/// search reads it from the disk in pieces the way ripgrep reads a file it does not map. A binary file
/// larger than this is read only up to its first NUL, which is all a directory search reads of it.
///
/// Unluminous 1.3.3 on macOS (`task-2323`): a window whose project held large files read each one whole
/// on fourteen threads at once, and the process reached a footprint of 180 GB on a 64 GB Mac and stopped
/// answering. Nothing bounded a single read, so one disk image or log of tens of gigabytes was enough.
pub const LARGEST_HELD: u64 = 32 * 1024 * 1024;

/// What [`read_bounded`] read of a file.
#[derive(Debug, PartialEq, Eq)]
pub enum Reading {
    /// The whole file, which is no larger than the limit.
    Whole(Vec<u8>),
    /// A larger binary file, read up to and including its first NUL.
    UpToNul {
        /// The bytes up to and including the first NUL.
        held: Vec<u8>,
        /// The file's size.
        size: u64,
    },
    /// A larger file with no NUL within the limit, of which nothing is kept.
    Large {
        /// The file's size.
        size: u64,
    },
}

/// How much of a file is read at a time.
const READ_PIECE: usize = 1024 * 1024;

/// Reads a file without ever holding more than [`LARGEST_HELD`] bytes of it: the whole of a file that
/// size or smaller, the start of a larger binary file up to its first NUL, and nothing of any other larger
/// file. A file that grows while it is read is bounded the same way, because the read stops at the limit
/// rather than at the size the file had when it was listed.
///
/// @param path - the file
pub fn read_bounded(path: &Path) -> std::io::Result<Reading> {
    read_bounded_at(path, LARGEST_HELD)
}

/// [`read_bounded`] with the limit given, so a test can use a small one.
///
/// @param path - the file
/// @param limit - the most bytes read into memory
pub fn read_bounded_at(path: &Path, limit: u64) -> std::io::Result<Reading> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let listed = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut held = Vec::with_capacity(listed.min(limit) as usize);
    let mut piece = vec![0u8; READ_PIECE.min(limit as usize + 1)];
    while held.len() as u64 <= limit {
        let want = piece.len().min(limit as usize + 1 - held.len());
        let got = file.read(&mut piece[..want])?;
        if got == 0 {
            return Ok(Reading::Whole(held));
        }
        let start = held.len();
        held.extend_from_slice(&piece[..got]);
        // A file listed larger than the limit stops at its first NUL, so a disk image is read only as
        // far as that. A file that fits is read whole, so its record keeps the hash of every byte.
        if listed > limit && !utf16(&held) {
            if let Some(at) = memchr::memchr(0, &held[start..]) {
                held.truncate(start + at + 1);
                held.shrink_to_fit();
                return Ok(Reading::UpToNul { held, size: listed });
            }
        }
    }
    let size = file.metadata().map(|m| m.len()).unwrap_or(0).max(held.len() as u64);
    if !utf16(&held) {
        if let Some(at) = memchr::memchr(0, &held[..limit as usize]) {
            held.truncate(at + 1);
            held.shrink_to_fit();
            return Ok(Reading::UpToNul { held, size });
        }
    }
    Ok(Reading::Large { size })
}

/// Whether bytes start with a UTF-16 byte order mark, which ripgrep decodes before it searches, so a NUL
/// in them does not make the file binary.
///
/// @param bytes - the start of a file
fn utf16(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF])
}

/// The hash of a file that was not read whole: the part that was read, the size and, for a file of which
/// nothing was kept, the modification time, so a change to any of them is a change to the record.
///
/// @param held - the bytes read
/// @param size - the file's size
/// @param mtime_ns - its modification time
/// @param large - whether nothing of it was kept
fn reading_hash(held: &[u8], size: u64, mtime_ns: i64, large: bool) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(if large { b"atrius-large" } else { b"atrius-upto0" });
    hasher.update(&size.to_le_bytes());
    if large {
        hasher.update(&mtime_ns.to_le_bytes());
    }
    hasher.update(held);
    *hasher.finalize().as_bytes()
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

/// How many files [`Exact::build`] reads before it adds them to the index.
const BUILD_SLICE: usize = 1024;

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
    /// The root, which a record that let go of its content is read from.
    pub root: Option<std::path::PathBuf>,
}

impl Exact {
    /// Builds the index of a root from scratch: walks it, reads every file in parallel, and adds them
    /// in path order.
    ///
    /// @param root - the root
    pub fn build(root: &Path) -> Exact {
        let found = files::walk(root);
        let mut exact = Exact { root: Some(root.to_path_buf()), ..Exact::default() };
        // A slice of the files is read and added before the next is read, so the trigram lists waiting
        // to be added are those of one slice rather than of the whole project.
        for slice in found.chunks(BUILD_SLICE) {
            let read: Vec<Option<(FileRecord, Vec<Vec<u32>>)>> =
                slice.par_iter().map(|f| FileRecord::read(root, f)).collect();
            for (record, trigrams) in read.into_iter().flatten() {
                exact.insert(record, &trigrams);
            }
        }
        exact
    }

    /// A fresh index of the given files, numbered from zero with no tombstones: what compaction builds.
    /// Each file's trigrams are taken again from its bytes, in parallel.
    ///
    /// @param records - the live files
    /// @param root - the root, which a record that let go of its content is read from
    pub fn from_records(records: Vec<FileRecord>, root: Option<&Path>) -> Exact {
        let read: Vec<(FileRecord, Vec<Vec<u32>>)> = records
            .into_par_iter()
            .filter_map(|r| match (r.is_held(), r.is_partial()) {
                // A large record has nothing to take trigrams from.
                _ if r.is_large() => Some((r, Vec::new())),
                // Taken from its bytes again, a partial record would be a whole one of a shorter file.
                (true, true) => {
                    let trigrams = vec![trigrams_of(&r.bytes())];
                    Some((r, trigrams))
                }
                (true, false) => Some(FileRecord::from_bytes(r.rel.clone(), r.mtime_ns, &r.bytes())),
                // A record that let go of its content is read again from the file.
                (false, _) => {
                    let reading = read_bounded(&root?.join(&r.rel)).ok()?;
                    Some(FileRecord::from_reading(r.rel.clone(), r.mtime_ns, reading))
                }
            })
            .collect();
        let mut exact = Exact { root: root.map(Path::to_path_buf), ..Exact::default() };
        for (record, trigrams) in read {
            exact.insert(record, &trigrams);
        }
        exact
    }

    /// Lets every record go of its content, when [`holding_content`] is off.
    pub fn let_go_of_content(&mut self) {
        if holding_content() {
            return;
        }
        for record in self.files.iter_mut().flatten() {
            record.let_go_of_content();
        }
    }

    /// Lets the given records go of their content, when [`holding_content`] is off.
    ///
    /// @param ids - the records
    pub fn let_go_of(&mut self, ids: impl IntoIterator<Item = u32>) {
        if holding_content() {
            return;
        }
        for id in ids {
            if let Some(Some(record)) = self.files.get_mut(id as usize) {
                record.let_go_of_content();
            }
        }
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
        Exact { files, by_path, postings, tombstones, generation: 0, root: None }
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
        if record.is_large() {
            let Some(root) = self.root.as_deref() else { return Vec::new() };
            return verify_path(matcher, &record.rel, &root.join(&record.rel));
        }
        let Some(piece) = record.blocks.get(block) else { return Vec::new() };
        let bytes = record.block_bytes(block, self.root.as_deref());
        let mut hits = verify(matcher, &record.rel, &bytes, BinaryDetection::quit(b'\x00'));
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
        let mut allowed = self.evaluate(&query);
        if query != Query::All {
            // A large file has no trigrams, so no query lets it through, and every search reads it.
            let before = allowed.len();
            allowed.extend(
                self.by_path
                    .values()
                    .filter(|&&file| {
                        self.files[file as usize].as_ref().is_some_and(FileRecord::is_large)
                    })
                    .map(|&file| block_id(file, 0)),
            );
            if allowed.len() > before {
                allowed.sort_unstable();
                allowed.dedup();
            }
        }
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
            Some(record) if !record.is_partial() && !record.is_large() && record.is_held() => {
                record.bytes()
            }
            _ => std::fs::read(path).unwrap_or_default(),
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
                        self.files[file as usize].as_ref().map(|r| (file, r.blocks.len().max(1)))
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

/// Runs the regex over a file read from the disk in pieces, as ripgrep searches a file it does not map,
/// stopping at the first NUL. It is how a file larger than [`LARGEST_HELD`] is searched, so the file is
/// never in memory whole.
///
/// @param matcher - the regex
/// @param rel - the file's path, for the hits
/// @param path - the file
pub fn verify_path(matcher: &RegexMatcher, rel: &str, path: &Path) -> Vec<Hit> {
    let mut searcher: Searcher = SearcherBuilder::new()
        .line_number(true)
        .binary_detection(BinaryDetection::quit(b'\x00'))
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
    let _ = searcher.search_path(matcher, path, sink);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `task-2279`: a binary file is held up to and including its first NUL, which is all a directory
    /// search reads of it, and a record an older index file held whole is cut the same way as it loads.
    #[test]
    fn a_binary_file_is_held_up_to_its_first_nul() {
        let bytes = b"match here\0match after\nand more after\n";
        let (record, _) = FileRecord::from_bytes("bin/a.bin".into(), 0, bytes);
        assert!(record.binary && record.is_partial());
        assert_eq!(record.bytes(), b"match here\0");
        assert_eq!(record.size, bytes.len() as u64, "the size is the file's");

        let whole = FileRecord {
            blocks: vec![Block { first_line: 1, packed: lz4_flex::block::compress_prepend_size(bytes) }],
            ..record.clone()
        };
        assert!(!whole.is_partial(), "an older index file held every byte");
        assert_eq!(whole.held_to_its_first_nul().bytes(), b"match here\0");

        let (text, _) = FileRecord::from_bytes("src/a.rs".into(), 0, b"fn main() {}\n");
        assert!(!text.is_partial() && text.bytes() == b"fn main() {}\n", "text is held whole");
    }

    /// `task-2323`: a read never holds more than its limit. A file within it is read whole, a larger
    /// binary file up to its first NUL, a larger text file not at all, and a UTF-16 file's NULs do not
    /// make it binary.
    #[test]
    fn a_read_holds_no_more_than_its_limit() {
        let dir = tempfile::tempdir().expect("a folder");
        let write = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        let small = write("small.txt", b"0123456789");
        assert_eq!(read_bounded_at(&small, 10).unwrap(), Reading::Whole(b"0123456789".to_vec()));

        let text = write("text.txt", &b"line\n".repeat(100));
        assert_eq!(read_bounded_at(&text, 64).unwrap(), Reading::Large { size: 500 });

        let mut image = b"head\n\0".to_vec();
        image.resize(500, 0);
        let image = write("disk.img", &image);
        assert_eq!(
            read_bounded_at(&image, 64).unwrap(),
            Reading::UpToNul { held: b"head\n\0".to_vec(), size: 500 }
        );

        let mut late = b"text\n".repeat(100);
        late.push(0);
        let late = write("late.bin", &late);
        assert_eq!(read_bounded_at(&late, 64).unwrap(), Reading::Large { size: 501 });

        let mut wide = vec![0xFF, 0xFE];
        for unit in "wide text\n".repeat(20).encode_utf16() {
            wide.extend_from_slice(&unit.to_le_bytes());
        }
        let wide = write("wide.txt", &wide);
        assert!(matches!(read_bounded_at(&wide, 64).unwrap(), Reading::Large { .. }));

        let (record, trigrams) =
            FileRecord::from_reading("t.txt".into(), 7, Reading::Large { size: 500 });
        assert!(
            record.is_large() && trigrams.is_empty() && record.content(Some(dir.path())).is_none()
        );
    }
}
