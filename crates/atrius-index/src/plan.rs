//! Turning a regular expression into a trigram query (Russ Cox's method from Google Code Search).
//!
//! Every match of the pattern must contain certain strings. A literal must appear whole, so all of its
//! trigrams must be in the file; an alternation needs one of its branches; a part that can match
//! anything, or match nothing, says nothing. The result is an AND/OR tree of trigrams, and only files
//! whose posting lists satisfy it are verified with the real regex.
//!
//! This is a simplified form of Cox's analysis: each node knows either the exact small set of strings
//! it can match, or only a query its matches satisfy. Concatenation multiplies exact sets while they
//! stay small and otherwise ANDs the queries. The result is never too narrow: it may let through a file
//! that does not match, never keep out one that does, which is what parity with ripgrep needs.

use std::collections::BTreeSet;

use regex_syntax::hir::{Class, Hir, HirKind};
use regex_syntax::ParserBuilder;

use crate::trigram::{fold, trigrams_of_literal, Trigram};

/// The largest exact set a node keeps before it is turned into a query.
const MAX_EXACT: usize = 16;
/// The largest class turned into alternatives; a wider class matches too much to say anything.
const MAX_CLASS: u32 = 8;

/// A boolean query over trigrams.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Query {
    /// Every file could match.
    All,
    /// The file must hold this trigram.
    Tri(Trigram),
    /// The file must satisfy every part.
    And(Vec<Query>),
    /// The file must satisfy at least one part.
    Or(Vec<Query>),
}

impl Query {
    /// An AND that drops `All` parts and flattens nested ANDs.
    ///
    /// @param parts - the parts
    pub fn and(parts: Vec<Query>) -> Query {
        let mut out = Vec::new();
        for p in parts {
            match p {
                Query::All => {}
                Query::And(inner) => out.extend(inner),
                other => out.push(other),
            }
        }
        out.sort_by_key(|q| format!("{q:?}"));
        out.dedup();
        match out.len() {
            0 => Query::All,
            1 => out.pop().expect("one part"),
            _ => Query::And(out),
        }
    }

    /// An OR that is `All` when any part is, and flattens nested ORs.
    ///
    /// @param parts - the parts
    pub fn or(parts: Vec<Query>) -> Query {
        let mut out = Vec::new();
        for p in parts {
            match p {
                Query::All => return Query::All,
                Query::Or(inner) => out.extend(inner),
                other => out.push(other),
            }
        }
        out.sort_by_key(|q| format!("{q:?}"));
        out.dedup();
        match out.len() {
            0 => Query::All,
            1 => out.pop().expect("one part"),
            _ => Query::Or(out),
        }
    }
}

/// What is known about the strings one part of a pattern matches.
struct Info {
    /// The exact set of (folded) strings it can match, when that set is small.
    exact: Option<BTreeSet<Vec<u8>>>,
    /// A query every match satisfies, beyond what `exact` says.
    query: Query,
}

impl Info {
    fn exact(set: BTreeSet<Vec<u8>>) -> Info {
        Info { exact: Some(set), query: Query::All }
    }

    fn anything() -> Info {
        Info { exact: None, query: Query::All }
    }

    /// Everything this part's matches must satisfy, as a query.
    fn to_query(&self) -> Query {
        let from_exact = match &self.exact {
            Some(set) => Query::or(set.iter().map(|s| literal_query(s)).collect()),
            None => Query::All,
        };
        Query::and(vec![from_exact, self.query.clone()])
    }
}

/// The query a literal string satisfies: all of its trigrams, or `All` when it is shorter than three.
///
/// @param text - the folded literal
fn literal_query(text: &[u8]) -> Query {
    if text.len() < 3 {
        return Query::All;
    }
    Query::and(trigrams_of_literal(text).into_iter().map(Query::Tri).collect())
}

/// Parses a pattern the way ripgrep's default regex engine does and plans its trigram query.
///
/// @param pattern - the pattern
/// @param case_insensitive - whether `-i` was given
pub fn plan(pattern: &str, case_insensitive: bool) -> Result<Query, String> {
    let hir = ParserBuilder::new()
        .case_insensitive(case_insensitive)
        .unicode(true)
        .utf8(false)
        .build()
        .parse(pattern)
        .map_err(|e| e.to_string())?;
    Ok(analyse(&hir).to_query())
}

/// Analyses one node of the parsed pattern.
///
/// @param hir - the node
fn analyse(hir: &Hir) -> Info {
    match hir.kind() {
        HirKind::Empty | HirKind::Look(_) => Info::exact(BTreeSet::from([Vec::new()])),
        HirKind::Literal(lit) => {
            Info::exact(BTreeSet::from([lit.0.iter().map(|&b| fold(b)).collect()]))
        }
        HirKind::Class(class) => class_info(class),
        HirKind::Capture(cap) => analyse(&cap.sub),
        HirKind::Repetition(rep) => {
            if rep.min == 0 {
                return Info::anything();
            }
            let sub = analyse(&rep.sub);
            Info { exact: None, query: sub.to_query() }
        }
        HirKind::Concat(parts) => concat(parts.iter().map(analyse)),
        HirKind::Alternation(parts) => alternation(parts.iter().map(analyse).collect()),
    }
}

/// A class becomes the set of its members, folded, when it is small, and says nothing otherwise.
///
/// @param class - the class
fn class_info(class: &Class) -> Info {
    let mut set = BTreeSet::new();
    match class {
        Class::Unicode(c) => {
            let size: u32 =
                c.ranges().iter().map(|r| u32::from(r.end()) - u32::from(r.start()) + 1).sum();
            if size > MAX_CLASS {
                return Info::anything();
            }
            for r in c.ranges() {
                for ch in r.start()..=r.end() {
                    let mut buf = [0u8; 4];
                    set.insert(ch.encode_utf8(&mut buf).bytes().map(fold).collect::<Vec<u8>>());
                }
            }
        }
        Class::Bytes(c) => {
            let size: u32 =
                c.ranges().iter().map(|r| u32::from(r.end()) - u32::from(r.start()) + 1).sum();
            if size > MAX_CLASS {
                return Info::anything();
            }
            for r in c.ranges() {
                for b in r.start()..=r.end() {
                    set.insert(vec![fold(b)]);
                }
            }
        }
    }
    Info::exact(set)
}

/// A concatenation: exact sets are multiplied while they stay small; otherwise what is known is
/// ANDed and adjacency is given up, which only ever widens the query.
///
/// @param parts - the analysed parts in order
fn concat(parts: impl Iterator<Item = Info>) -> Info {
    let mut current: Option<BTreeSet<Vec<u8>>> = Some(BTreeSet::from([Vec::new()]));
    let mut queries = Vec::new();
    // Whether `current` still describes the whole concatenation from its first part. Once a part
    // that is not exact has been passed, or a product has been given up, `current` describes only
    // what comes after it, so it can say what a match must contain but not what a match is.
    let mut whole = true;
    for part in parts {
        queries.push(part.query.clone());
        match (current.take(), part.exact) {
            (Some(left), Some(right)) if left.len() * right.len() <= MAX_EXACT => {
                let mut product = BTreeSet::new();
                for l in &left {
                    for r in &right {
                        let mut s = l.clone();
                        s.extend_from_slice(r);
                        product.insert(s);
                    }
                }
                current = Some(product);
            }
            (Some(left), Some(right)) => {
                queries.push(Info::exact(left).to_query());
                current = Some(right);
                whole = false;
            }
            (Some(left), None) => {
                queries.push(Info::exact(left).to_query());
                current = None;
                whole = false;
            }
            (None, Some(right)) => current = Some(right),
            (None, None) => {}
        }
    }
    if whole {
        return Info { exact: current, query: Query::and(queries) };
    }
    if let Some(rest) = current {
        queries.push(Info::exact(rest).to_query());
    }
    Info { exact: None, query: Query::and(queries) }
}

/// An alternation: the union of exact sets when every branch has one and the union is small,
/// otherwise the OR of each branch's query.
///
/// @param branches - the analysed branches
fn alternation(branches: Vec<Info>) -> Info {
    if branches.iter().all(|b| b.exact.is_some() && b.query == Query::All) {
        let union: BTreeSet<Vec<u8>> =
            branches.iter().flat_map(|b| b.exact.clone().unwrap_or_default()).collect();
        if union.len() <= MAX_EXACT {
            return Info::exact(union);
        }
    }
    Info { exact: None, query: Query::or(branches.iter().map(Info::to_query).collect()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigram::trigram;

    fn tri(s: &str) -> Query {
        let b = s.as_bytes();
        Query::Tri(trigram(b[0], b[1], b[2]))
    }

    #[test]
    fn a_literal_needs_all_of_its_trigrams() {
        assert_eq!(plan("abcd", false).unwrap(), Query::And(vec![tri("abc"), tri("bcd")]));
    }

    #[test]
    fn an_alternation_needs_one_branch() {
        let q = plan("fooo|barr", false).unwrap();
        assert!(matches!(q, Query::Or(_)), "{q:?}");
    }

    #[test]
    fn a_pattern_with_nothing_bounded_matches_everything() {
        assert_eq!(plan(r"\w+", false).unwrap(), Query::All);
        assert_eq!(plan("ab", false).unwrap(), Query::All);
    }

    #[test]
    fn case_insensitive_folds_to_the_same_trigrams() {
        assert_eq!(plan("ABCD", true).unwrap(), plan("abcd", false).unwrap());
    }

    /// `qwen38|qwen-3.8|qwen 3.8` is parsed with `qwen` factored out of the alternation, and the two
    /// branches with a `.` in them end with the exact set `{"8"}`, which describes only what follows
    /// the dot. Treated as the whole branch, it made the query ask for `qwen8` and lost every file
    /// holding "Qwen 3.8" (found by the F2 parity run on ai-service).
    #[test]
    fn what_follows_a_wildcard_is_never_glued_to_what_came_before_it() {
        let q = plan("qwen38|qwen-3.8|qwen 3.8", true).unwrap();
        let text = format!("{q:?}");
        assert!(!text.contains(&format!("{:?}", tri("en8"))), "{text}");
    }

    #[test]
    fn a_wildcard_between_two_literals_keeps_both() {
        let q = plan("fn relayout.*width", false).unwrap();
        let text = format!("{q:?}");
        assert!(
            text.contains(&format!("{:?}", tri("rel")))
                && text.contains(&format!("{:?}", tri("wid"))),
            "{text}"
        );
    }
}
