//! A file's definitions and its chunks (`tasks/task-2138-unluminous-code-index-tdd.md` §6.5).
//!
//! Unluminous has no parser and task-1675 refused tree-sitter, so this uses what `unluminous-core`
//! already has. Definitions come from `crate::lang::symbols`, which reads each language plugin's
//! `language.definers`. Where a definition ends comes from `crate::lang::folding`, whose bracket
//! matching runs over the tokeniser's output and so ignores brackets in strings and comments, and whose
//! indentation regions cover Python and YAML: the definition ends where the block that opens on its line,
//! or within the next two lines, ends.
//!
//! **Chunks follow the cAST method.** Top level definitions, each with the comments and attributes above
//! it, are units; code between them is a unit too. Neighbouring units are merged while the chunk stays
//! within the budget, counted in characters that are not white space so the budget means the same in
//! every language, and a unit over the budget is split at the definitions or blank lines inside it.
//! Each chunk's header names its file and the signatures of the definitions it holds, because "which file
//! and which function" is often the most useful thing a search word can match.

use crate::lang::folding::{self, Kind, Reading};
use crate::lang::symbols::{Confidence, FileSymbols};

use crate::grammars;

/// The chunk budget in characters that are not white space (§6.5: start at 2,000).
pub const CHUNK_BUDGET: usize = 2000;

/// One definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    /// The defined name.
    pub name: String,
    /// What kind of thing: function, type, constant, variable or module.
    pub kind: &'static str,
    /// The line the name is on, from one.
    pub line: u32,
    /// The last line of its body, from one; the same as `line` when it has none.
    pub end: u32,
    /// The first line of the comments and attributes above it, from one.
    pub start: u32,
    /// The line the name is on, trimmed: its signature.
    pub signature: String,
    /// Whether it was found by the brace heuristic rather than a definer keyword.
    pub likely: bool,
    /// Whether another file could import it.
    pub exported: bool,
    /// How many other definitions it sits inside.
    pub depth: u32,
}

/// One chunk of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// The first line, from one.
    pub start: u32,
    /// The last line, from one.
    pub end: u32,
    /// `definition`, `section` or `other`.
    pub kind: &'static str,
    /// The first definition it holds, if any.
    pub symbol: Option<String>,
    /// The file's path and the signatures of what it holds.
    pub header: String,
    /// Its text.
    pub body: String,
}

/// What reading one file produced.
#[derive(Debug, Default)]
pub struct Outline {
    /// Every definition, in line order.
    pub definitions: Vec<Definition>,
    /// The chunks, in line order, covering the whole file.
    pub chunks: Vec<Chunk>,
}

/// Whether a definition is one worth listing and looking up by name: anything at the top level, and
/// anything nested that is not a variable. A `let` inside a function body is a definition to the
/// tokeniser and nothing an agent asks for by name.
///
/// @param d - the definition
pub fn is_listed(d: &Definition) -> bool {
    d.depth == 0 || d.kind != "variable"
}

/// Whether a line, trimmed, is a comment or an attribute that belongs to the definition under it.
///
/// @param line - the line
fn is_preamble(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("///")
        || t.starts_with("//")
        || t.starts_with("/*")
        || t.starts_with('*')
        || t.starts_with("#[")
        || t.starts_with('@')
        || (t.starts_with('#') && !t.starts_with("#!"))
}

/// The characters of a line that are not white space.
///
/// @param line - the line
fn weight(line: &str) -> usize {
    line.chars().filter(|c| !c.is_whitespace()).count()
}

/// Reads a file's definitions and chunks.
///
/// @param rel - the file's path, which decides its language and heads its chunks
/// @param text - the file's text
/// @param budget - the chunk budget in characters that are not white space
pub fn read(rel: &str, text: &str, budget: usize) -> Outline {
    let lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let language = grammars::for_path(rel);
    let definitions = match &language {
        Some(language) if language.grammar.defines_symbols() => {
            definitions(text, &lines, &language.grammar)
        }
        _ => Vec::new(),
    };
    let units = match &language {
        Some(_) => code_units(&lines, &definitions),
        None if rel.to_ascii_lowercase().ends_with(".md") => section_units(text, &lines),
        None => paragraph_units(&lines),
    };
    let chunks = merge(rel, &lines, &units, &definitions, budget);
    Outline { definitions, chunks }
}

/// Every definition in a file with its extent.
///
/// @param text - the file's text
/// @param lines - its lines
/// @param grammar - its language
fn definitions(
    text: &str,
    lines: &[&str],
    grammar: &crate::lang::syntax::Grammar,
) -> Vec<Definition> {
    let symbols = FileSymbols::read(text, grammar);
    let regions = folding::regions(text, Reading::Code(grammar));
    let line_starts: Vec<usize> =
        std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let mut out: Vec<Definition> = Vec::new();
    for d in symbols.definitions() {
        let line0 = line_starts.partition_point(|&s| s <= d.name_range.start).saturating_sub(1);
        let end0 = regions
            .iter()
            .filter(|r| {
                matches!(r.kind, Kind::Block | Kind::Indent)
                    && r.head >= line0
                    && r.head <= line0 + 2
            })
            .map(|r| r.last())
            .next()
            .unwrap_or(line0);
        let mut start0 = line0;
        while start0 > 0 && is_preamble(lines[start0 - 1]) {
            start0 -= 1;
        }
        out.push(Definition {
            name: text[d.name_range.clone()].to_owned(),
            kind: d.kind.name(),
            line: line0 as u32 + 1,
            end: end0.max(line0) as u32 + 1,
            start: start0 as u32 + 1,
            signature: lines
                .get(line0)
                .map(|l| l.trim().chars().take(200).collect())
                .unwrap_or_default(),
            likely: d.confidence == Confidence::Likely,
            exported: d.exported,
            depth: 0,
        });
    }
    out.sort_by_key(|d| (d.line, std::cmp::Reverse(d.end)));
    for i in 0..out.len() {
        let (line, end) = (out[i].line, out[i].end);
        out[i].depth =
            out[..i].iter().filter(|o| o.line < line && o.end >= end && o.end > o.line).count()
                as u32;
    }
    out
}

/// A run of lines that belongs together: a definition, a heading's section, or what is between them.
#[derive(Clone, Debug)]
struct Unit {
    start: usize,
    end: usize,
    kind: &'static str,
}

/// The units of a code file: each top level definition from its preamble to its end, and the code
/// between them.
///
/// @param lines - the file's lines
/// @param definitions - its definitions
fn code_units(lines: &[&str], definitions: &[Definition]) -> Vec<Unit> {
    let mut units = Vec::new();
    let mut at = 0usize;
    for d in definitions.iter().filter(|d| d.depth == 0) {
        let (start, end) = (d.start as usize - 1, d.end as usize - 1);
        if start < at {
            continue;
        }
        if start > at {
            units.push(Unit { start: at, end: start - 1, kind: "other" });
        }
        units.push(Unit { start, end, kind: "definition" });
        at = end + 1;
    }
    if at < lines.len() {
        units.push(Unit { start: at, end: lines.len() - 1, kind: "other" });
    }
    units
}

/// The units of a Markdown file: each top level heading's section, and the text before the first.
///
/// @param text - the file's text
/// @param lines - its lines
fn section_units(text: &str, lines: &[&str]) -> Vec<Unit> {
    let regions = folding::regions(text, Reading::Markdown);
    let mut units = Vec::new();
    let mut at = 0usize;
    for r in regions.iter().filter(|r| r.kind == Kind::Heading) {
        if r.head < at {
            continue;
        }
        if r.head > at {
            units.push(Unit { start: at, end: r.head - 1, kind: "other" });
        }
        let end = r.last().max(r.head).min(lines.len().saturating_sub(1));
        units.push(Unit { start: r.head, end, kind: "section" });
        at = end + 1;
    }
    if at < lines.len() {
        units.push(Unit { start: at, end: lines.len() - 1, kind: "other" });
    }
    units
}

/// The units of a plain text file: paragraphs, split at blank lines.
///
/// @param lines - the file's lines
fn paragraph_units(lines: &[&str]) -> Vec<Unit> {
    let mut units = Vec::new();
    let mut start = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() && i > start {
            units.push(Unit { start, end: i, kind: "other" });
            start = i + 1;
        }
    }
    if start < lines.len() {
        units.push(Unit { start, end: lines.len() - 1, kind: "other" });
    }
    units
}

/// Merges units into chunks within the budget and splits any unit over it.
///
/// @param rel - the file's path
/// @param lines - its lines
/// @param units - the units in line order
/// @param definitions - its definitions, for headers and split points
/// @param budget - the budget
fn merge(
    rel: &str,
    lines: &[&str],
    units: &[Unit],
    definitions: &[Definition],
    budget: usize,
) -> Vec<Chunk> {
    let weight_of = |s: usize, e: usize| {
        lines[s..=e.min(lines.len() - 1)].iter().map(|l| weight(l)).sum::<usize>()
    };
    let mut pieces: Vec<Unit> = Vec::new();
    for unit in units {
        if weight_of(unit.start, unit.end) <= budget {
            pieces.push(unit.clone());
        } else {
            pieces.extend(split(lines, unit, definitions, budget));
        }
    }
    let mut chunks = Vec::new();
    let mut current: Option<Unit> = None;
    for piece in pieces {
        current = match current {
            Some(c) if weight_of(c.start, piece.end) <= budget => Some(Unit {
                start: c.start,
                end: piece.end,
                kind: if c.kind == "other" { piece.kind } else { c.kind },
            }),
            Some(c) => {
                chunks.push(make_chunk(rel, lines, &c, definitions));
                Some(piece)
            }
            None => Some(piece),
        };
    }
    if let Some(c) = current {
        chunks.push(make_chunk(rel, lines, &c, definitions));
    }
    chunks.retain(|c| !c.body.trim().is_empty());
    chunks
}

/// Splits a unit over the budget at the definitions and blank lines inside it, or at the budget when it
/// has neither.
///
/// @param lines - the file's lines
/// @param unit - the unit
/// @param definitions - the file's definitions
/// @param budget - the budget
fn split(lines: &[&str], unit: &Unit, definitions: &[Definition], budget: usize) -> Vec<Unit> {
    let cuts: Vec<usize> = (unit.start + 1..=unit.end)
        .filter(|&i| {
            lines[i].trim().is_empty()
                || definitions.iter().any(|d| d.start as usize - 1 == i && d.depth > 0)
        })
        .collect();
    let mut out = Vec::new();
    let mut start = unit.start;
    let mut used = 0usize;
    for i in unit.start..=unit.end {
        let w = weight(lines[i]);
        if used + w > budget && i > start {
            let cut = cuts.iter().rev().find(|&&c| c > start && c <= i).copied().unwrap_or(i);
            out.push(Unit { start, end: cut - 1, kind: unit.kind });
            used = lines[cut..=i].iter().map(|l| weight(l)).sum();
            start = cut;
            continue;
        }
        used += w;
    }
    out.push(Unit { start, end: unit.end, kind: unit.kind });
    out
}

/// A chunk from a unit: its text and its header.
///
/// @param rel - the file's path
/// @param lines - the file's lines
/// @param unit - the lines it covers
/// @param definitions - the file's definitions
fn make_chunk(rel: &str, lines: &[&str], unit: &Unit, definitions: &[Definition]) -> Chunk {
    let (s, e) = (unit.start as u32 + 1, unit.end as u32 + 1);
    let inside: Vec<&Definition> =
        definitions.iter().filter(|d| d.line >= s && d.line <= e).collect();
    let enclosing: Vec<&Definition> =
        definitions.iter().filter(|d| d.line < s && d.end >= s).collect();
    let mut signatures: Vec<String> = enclosing.iter().map(|d| d.signature.clone()).collect();
    signatures.extend(
        inside
            .iter()
            .filter(|d| d.depth == 0 || enclosing.is_empty())
            .take(4)
            .map(|d| d.signature.clone()),
    );
    let header = if signatures.is_empty() {
        rel.to_owned()
    } else {
        format!("{rel} | {}", signatures.join(" ; "))
    };
    Chunk {
        start: s,
        end: e,
        kind: unit.kind,
        symbol: inside.first().or(enclosing.last()).map(|d| d.name.clone()),
        header,
        body: lines[unit.start..=unit.end.min(lines.len() - 1)].join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rust_function_ends_where_its_block_ends_and_carries_its_doc_comment() {
        let text = "use std::fmt;\n\n/// Adds.\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn other() {}\n";
        let outline = read("src/lib.rs", text, CHUNK_BUDGET);
        let add = outline.definitions.iter().find(|d| d.name == "add").expect("add");
        assert_eq!((add.start, add.line, add.end), (3, 4, 6));
        assert!(outline.chunks.iter().any(|c| c.header.contains("pub fn add")));
    }

    #[test]
    fn a_function_over_the_budget_is_split_and_every_line_is_in_a_chunk() {
        let mut text = String::from("fn big() {\n");
        for i in 0..400 {
            text.push_str(&format!("    let value_{i} = compute_something_long({i});\n"));
            if i % 50 == 49 {
                text.push('\n');
            }
        }
        text.push_str("}\n");
        let outline = read("src/big.rs", &text, CHUNK_BUDGET);
        assert!(outline.chunks.len() > 3, "{}", outline.chunks.len());
        let covered: usize = outline.chunks.iter().map(|c| (c.end - c.start + 1) as usize).sum();
        assert!(covered >= text.lines().count() - 1);
        assert!(outline.chunks.iter().all(|c| c.header.contains("fn big")));
    }
}
