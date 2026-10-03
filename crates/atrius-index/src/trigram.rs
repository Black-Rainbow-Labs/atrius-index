//! Trigram posting lists: for every three byte sequence, the files it appears in.
//!
//! **Bytes are folded to lower case for ASCII and kept as they are otherwise**, so one list answers a
//! case sensitive and a case insensitive search; the case sensitive one simply gets a few more
//! candidates to verify. **A trigram that spans a line break is not kept**, because a match never
//! spans a line (the Grep tool does not pass `--multiline`). **Nothing after a file's first NUL byte is
//! indexed**: ripgrep stops searching a file at its first NUL, so nothing there can ever match.
//!
//! A list is the delta of each file id from the one before, as a variable length integer, which is
//! what Zoekt and the reference editor's trigram index keep too. Ids are handed out in increasing order, so a
//! changed file is appended to the lists with a new id and its old id is tombstoned; the lists are
//! rebuilt when tombstones pile up.

use std::collections::HashMap;

/// A trigram as an integer: the three folded bytes, high to low.
pub type Trigram = u32;

/// Folds one byte the way the index does: ASCII upper case to lower case, anything else unchanged.
///
/// @param byte - the byte
pub fn fold(byte: u8) -> u8 {
    byte.to_ascii_lowercase()
}

/// The trigram of three bytes, folded.
///
/// @param a - the first byte
/// @param b - the second byte
/// @param c - the third byte
pub fn trigram(a: u8, b: u8, c: u8) -> Trigram {
    (u32::from(fold(a)) << 16) | (u32::from(fold(b)) << 8) | u32::from(fold(c))
}

/// The length of the part of a file that can match: everything before its first NUL byte.
///
/// @param bytes - the file's bytes
pub fn searchable_len(bytes: &[u8]) -> usize {
    memchr::memchr(0, bytes).unwrap_or(bytes.len())
}

/// Every distinct trigram of the searchable part of a file, sorted.
///
/// @param bytes - the file's bytes
pub fn trigrams_of(bytes: &[u8]) -> Vec<Trigram> {
    let bytes = &bytes[..searchable_len(bytes)];
    let mut seen = vec![0u64; 1 << 18];
    let mut out = Vec::new();
    for w in bytes.windows(3) {
        if w[0] == b'\n' || w[1] == b'\n' || w[2] == b'\n' {
            continue;
        }
        let t = trigram(w[0], w[1], w[2]);
        let (word, bit) = ((t >> 6) as usize, 1u64 << (t & 63));
        if seen[word] & bit == 0 {
            seen[word] |= bit;
            out.push(t);
        }
    }
    out.sort_unstable();
    out
}

/// Every trigram of a literal string as it would be searched for, skipping those across a line break.
///
/// @param text - the literal's bytes
pub fn trigrams_of_literal(text: &[u8]) -> Vec<Trigram> {
    let mut out: Vec<Trigram> = text
        .windows(3)
        .filter(|w| !w.contains(&b'\n'))
        .map(|w| trigram(w[0], w[1], w[2]))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// One trigram's list of file ids, delta and varint encoded.
#[derive(Clone, Debug, Default)]
pub struct Posting {
    last: u32,
    count: u32,
    bytes: Vec<u8>,
}

impl Posting {
    /// Appends a file id, which must be larger than every id already in the list.
    ///
    /// @param id - the file id
    pub fn push(&mut self, id: u32) {
        let delta = if self.count == 0 { id } else { id - self.last };
        write_varint(&mut self.bytes, delta);
        self.last = id;
        self.count += 1;
    }

    /// How many files the list holds, tombstoned ones included.
    pub fn len(&self) -> usize {
        self.count as usize
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The file ids, decoded, in increasing order.
    pub fn ids(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.count as usize);
        let mut at = 0;
        let mut id = 0u32;
        while at < self.bytes.len() {
            let (delta, used) = read_varint(&self.bytes[at..]);
            at += used;
            id = if out.is_empty() { delta } else { id + delta };
            out.push(id);
        }
        out
    }

    /// The encoded bytes, for storage.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// A list read back from storage.
    ///
    /// @param bytes - the encoded bytes
    pub fn from_bytes(bytes: Vec<u8>) -> Posting {
        let mut posting = Posting { last: 0, count: 0, bytes };
        let ids = posting.ids();
        posting.count = ids.len() as u32;
        posting.last = ids.last().copied().unwrap_or(0);
        posting
    }

    /// Bytes held, for the memory figure `search status` reports.
    pub fn heap_bytes(&self) -> usize {
        self.bytes.capacity()
    }
}

/// Every trigram's posting list.
#[derive(Default, Clone)]
pub struct Postings {
    lists: HashMap<Trigram, Posting>,
}

impl Postings {
    /// Adds one file's trigrams under its id.
    ///
    /// @param id - the file id, larger than any added before
    /// @param trigrams - the file's trigrams
    pub fn add(&mut self, id: u32, trigrams: &[Trigram]) {
        for &t in trigrams {
            self.lists.entry(t).or_default().push(id);
        }
    }

    /// The ids of the files holding a trigram, in increasing order.
    ///
    /// @param t - the trigram
    pub fn files_with(&self, t: Trigram) -> Vec<u32> {
        self.lists.get(&t).map_or_else(Vec::new, Posting::ids)
    }

    /// How many files hold a trigram, without decoding the list.
    ///
    /// @param t - the trigram
    pub fn count(&self, t: Trigram) -> usize {
        self.lists.get(&t).map_or(0, Posting::len)
    }

    /// How many distinct trigrams are indexed.
    pub fn len(&self) -> usize {
        self.lists.len()
    }

    /// Whether nothing is indexed.
    pub fn is_empty(&self) -> bool {
        self.lists.is_empty()
    }

    /// The lists, for storage.
    pub fn iter(&self) -> impl Iterator<Item = (&Trigram, &Posting)> {
        self.lists.iter()
    }

    /// Puts one stored list back.
    ///
    /// @param t - the trigram
    /// @param posting - its list
    pub fn insert(&mut self, t: Trigram, posting: Posting) {
        self.lists.insert(t, posting);
    }

    /// Bytes held by every list.
    pub fn heap_bytes(&self) -> usize {
        self.lists.values().map(Posting::heap_bytes).sum::<usize>() + self.lists.capacity() * 48
    }
}

/// Writes an unsigned integer in the variable length form: seven bits a byte, high bit set on all but
/// the last.
///
/// @param out - where to write
/// @param value - the integer
pub fn write_varint(out: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Reads one variable length integer and how many bytes it took.
///
/// @param bytes - the bytes starting at the integer
pub fn read_varint(bytes: &[u8]) -> (u32, usize) {
    let mut value = 0u32;
    let mut shift = 0;
    for (i, &b) in bytes.iter().enumerate() {
        value |= u32::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return (value, i + 1);
        }
        shift += 7;
    }
    (value, bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_posting_list_gives_back_the_ids_it_was_given() {
        let mut p = Posting::default();
        for id in [0, 1, 5, 300, 70_000, 70_001] {
            p.push(id);
        }
        assert_eq!(p.ids(), [0, 1, 5, 300, 70_000, 70_001]);
        assert_eq!(Posting::from_bytes(p.as_bytes().to_vec()).ids(), p.ids());
    }

    #[test]
    fn nothing_after_the_first_nul_is_indexed_and_case_is_folded() {
        let t = trigrams_of(b"ABCd\0xyz");
        assert!(t.contains(&trigram(b'a', b'b', b'c')));
        assert!(!t.contains(&trigram(b'x', b'y', b'z')));
        assert!(trigrams_of(b"ab\ncd").is_empty(), "no trigram crosses a line");
    }
}
