//! Phase counters for the performance benchmark (`task-2696`): how much time each part of a query takes,
//! and how many candidate blocks a search verified against how many held a hit.
//!
//! The counters exist only in a build with the `phases` feature. In every other build [`start`] returns
//! `None` and [`stop`] and [`count`] do nothing, so the compiler removes them and a released binary pays
//! nothing for them. A benchmark built with the feature reads and clears the counters through `status`,
//! which then carries a `phases` field.
//!
//! Time spent on several threads at once, such as decompressing and matching candidate blocks in
//! parallel, is the sum over the threads, so those phases can add up to more than the query's own time.

/// A part of a query.
#[derive(Clone, Copy, Debug)]
pub enum Phase {
    /// The freshness gate: the fence, the dirty set and any files read again.
    Gate,
    /// Turning the pattern into a trigram query.
    Plan,
    /// Decoding posting lists into block ids.
    Decode,
    /// Intersecting and joining block id lists.
    Intersect,
    /// Narrowing the candidates to the scope, and adding large records.
    Scope,
    /// Building the regex matcher.
    Matcher,
    /// Getting each candidate block's bytes: lz4 decompression or a read from the disk.
    Decompress,
    /// Running the searcher over the candidate blocks.
    Match,
    /// Sorting the hits.
    Sort,
    /// Ordering and cutting the hits to the token budget, and writing the answer's text.
    Shape,
    /// The whole of `verbs::answer`, the part of a call that happens inside the host.
    Answer,
}

/// A counted quantity.
#[derive(Clone, Copy, Debug)]
pub enum Count {
    /// Searches run by the exact engine.
    Searches,
    /// Candidate blocks verified.
    CandidateBlocks,
    /// Candidate blocks that held at least one hit.
    BlocksWithHit,
    /// Hits found, before shaping.
    Hits,
}

#[cfg(feature = "phases")]
mod counters {
    use std::sync::atomic::{AtomicU64, Ordering};

    pub const PHASES: [&str; 11] = [
        "gate", "plan", "decode", "intersect", "scope", "matcher", "decompress", "match", "sort",
        "shape", "answer",
    ];
    pub const COUNTS: [&str; 4] = ["searches", "candidateBlocks", "blocksWithHit", "hits"];
    pub static NANOS: [AtomicU64; 11] = [const { AtomicU64::new(0) }; 11];
    pub static TALLY: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

    /// Reads every counter and sets it back to zero.
    pub fn take() -> serde_json::Value {
        let mut out = serde_json::Map::new();
        for (name, cell) in PHASES.iter().zip(NANOS.iter()) {
            out.insert(format!("{name}Micros"), (cell.swap(0, Ordering::Relaxed) / 1000).into());
        }
        for (name, cell) in COUNTS.iter().zip(TALLY.iter()) {
            out.insert((*name).to_owned(), cell.swap(0, Ordering::Relaxed).into());
        }
        serde_json::Value::Object(out)
    }
}

/// Whether this build counts phases.
pub const ENABLED: bool = cfg!(feature = "phases");

/// The start of a timed phase: the time now in a build with the feature, nothing otherwise.
#[inline(always)]
pub fn start() -> Option<std::time::Instant> {
    if ENABLED {
        Some(std::time::Instant::now())
    } else {
        None
    }
}

/// Adds the time since `started` to a phase.
///
/// @param phase - the phase
/// @param started - what [`start`] returned
#[inline(always)]
pub fn stop(phase: Phase, started: Option<std::time::Instant>) {
    #[cfg(feature = "phases")]
    if let Some(at) = started {
        counters::NANOS[phase as usize]
            .fetch_add(at.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    }
    #[cfg(not(feature = "phases"))]
    let _ = (phase, started);
}

/// Adds to a counted quantity.
///
/// @param what - the quantity
/// @param n - how much to add
#[inline(always)]
pub fn count(what: Count, n: u64) {
    #[cfg(feature = "phases")]
    counters::TALLY[what as usize].fetch_add(n, std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(feature = "phases"))]
    let _ = (what, n);
}

/// Every counter as JSON, set back to zero, or None in a build without the feature.
pub fn take() -> Option<serde_json::Value> {
    #[cfg(feature = "phases")]
    return Some(counters::take());
    #[cfg(not(feature = "phases"))]
    None
}
