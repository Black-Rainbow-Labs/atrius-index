//! Result shaping: what an agent is shown, within a token budget (`tasks/task-2138-unluminous-code-index-tdd.md`
//! §6.9).
//!
//! Hits are grouped by file: the path once, then `line: text` rows, each line cut to 160 characters
//! around the match. There is no JSON punctuation in the text, because the agent pays for every
//! character. The shaper fills the budget best first and ends with one line saying how many hits in how
//! many files were left out, so an exact answer is never trimmed without saying so.

use std::collections::BTreeMap;

use crate::exact::Hit;

/// The longest line shown, in characters.
pub const LINE_WIDTH: usize = 160;
/// The budget when the caller names none, in tokens.
pub const DEFAULT_BUDGET: usize = 1500;

/// The approximate token count of a text: characters over 3.6, the rate the evaluation harness uses
/// until it is calibrated against the API's own count.
///
/// @param text - the text
pub fn tokens(text: &str) -> usize {
    (text.chars().count() as f64 / 3.6).ceil() as usize
}

/// A line cut to `LINE_WIDTH` characters around its first match of `needle` (case insensitive), or from
/// its start when the needle is not found, with surrounding white space removed.
///
/// @param text - the line
/// @param needle - what was searched for, as a plain string, or empty
pub fn trim_line(text: &str, needle: &str) -> String {
    let text = text.trim();
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= LINE_WIDTH {
        return text.to_owned();
    }
    let lower = text.to_lowercase();
    let at = if needle.is_empty() {
        0
    } else {
        lower.find(&needle.to_lowercase()).map_or(0, |b| text[..b.min(text.len())].chars().count())
    };
    let start = at.saturating_sub(LINE_WIDTH / 3).min(chars.len() - LINE_WIDTH);
    let mut out: String = chars[start..start + LINE_WIDTH].iter().collect();
    if start > 0 {
        out.insert(0, '…');
    }
    if start + LINE_WIDTH < chars.len() {
        out.push('…');
    }
    out
}

/// What the shaper produced.
pub struct Shaped {
    /// The text an agent is shown.
    pub text: String,
    /// The hits that were shown, in the order shown.
    pub shown: Vec<Hit>,
    /// Hits left out.
    pub omitted_hits: usize,
    /// Files left out entirely.
    pub omitted_files: usize,
}

/// Groups hits by file in the order given, and fills the budget with them.
///
/// @param hits - the hits, best first
/// @param needle - what to centre long lines on
/// @param budget - the budget in tokens; zero means no budget
pub fn shape(hits: &[Hit], needle: &str, budget: usize) -> Shaped {
    let mut order: Vec<&str> = Vec::new();
    let mut by_file: BTreeMap<&str, Vec<&Hit>> = BTreeMap::new();
    for hit in hits {
        let entry = by_file.entry(hit.path.as_str()).or_default();
        if entry.is_empty() {
            order.push(hit.path.as_str());
        }
        entry.push(hit);
    }
    let mut text = String::new();
    let mut shown = Vec::new();
    let mut used = 0usize;
    'files: for (files_shown, path) in order.iter().enumerate() {
        let header = format!("{path}\n");
        if budget > 0 && used + tokens(&header) > budget && files_shown > 0 {
            break;
        }
        text.push_str(&header);
        used += tokens(&header);
        for hit in &by_file[path] {
            let row = format!("  {}: {}\n", hit.line, trim_line(&hit.text, needle));
            if budget > 0 && used + tokens(&row) > budget {
                break 'files;
            }
            used += tokens(&row);
            text.push_str(&row);
            shown.push((*hit).clone());
        }
    }
    let omitted_hits = hits.len() - shown.len();
    let mut shown_per_file: BTreeMap<&str, usize> = BTreeMap::new();
    for hit in &shown {
        *shown_per_file.entry(hit.path.as_str()).or_default() += 1;
    }
    // Every file with at least one hit left out, whether or not some of its hits were shown.
    let files_with_omissions = order
        .iter()
        .filter(|p| shown_per_file.get(*p).copied().unwrap_or(0) < by_file[*p].len())
        .count();
    let omitted_files = order.iter().filter(|p| !shown_per_file.contains_key(*p)).count();
    if omitted_hits > 0 {
        text.push_str(&format!("+{omitted_hits} hits in {files_with_omissions} files not shown; narrow with path= or glob=, or raise budget=\n"));
    }
    if hits.is_empty() {
        text.push_str("no matches\n");
    }
    Shaped { text, shown, omitted_hits, omitted_files }
}
