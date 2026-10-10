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
    /// What it is, finer than `kind`: a method, a struct, a field (Unluminous `task-2231` §5.2).
    pub symbol_kind: crate::structure::Kind,
    /// The type or module whose block holds it.
    pub container: Option<String>,
    /// A callable's parameters.
    pub parameters: Vec<crate::structure::Parameter>,
    /// A callable's return type, as written.
    pub returns: Option<String>,
    /// A field's or variable's type, as written or as its constructor names it.
    pub type_text: Option<String>,
    /// Who may use it.
    pub visibility: crate::structure::Visibility,
    /// The first line of its documentation.
    pub doc: Option<String>,
    /// The name's own bytes.
    pub range: std::ops::Range<usize>,
    /// True for a field, a variant or an interface's method: a member the structural reading found
    /// rather than a definer keyword. The chunking and the chunk headers leave these out, so the passage
    /// table is exactly what it was before they existed.
    pub member: bool,
}

impl Definition {
    /// The parameter list as a person writes it: `ui: &mut Ui, at: Pos2`.
    pub fn parameter_text(&self) -> String {
        self.parameters
            .iter()
            .map(|p| match &p.type_text {
                Some(t) => format!("{}: {t}", p.name),
                None => p.name.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
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

/// One heading of a Markdown file and the section under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// The heading's line, from one.
    pub line: u32,
    /// The last line of the section, from one: the line before the next heading at the same level or
    /// higher, less any blank lines at its end. The same as `line` for a heading with nothing under it.
    pub end: u32,
    /// From one for `#` to six for `######`.
    pub level: u32,
    /// The heading's line, trimmed, with its hashes: `## Context`.
    pub heading: String,
}

/// What reading one file produced.
#[derive(Debug, Default)]
pub struct Outline {
    /// Every definition, in line order.
    pub definitions: Vec<Definition>,
    /// Every heading of a Markdown file, in line order. Empty for any other file.
    pub sections: Vec<Section>,
    /// The chunks, in line order, covering the whole file.
    pub chunks: Vec<Chunk>,
    /// The file's import statements, taken apart (Unluminous `task-2231` §5.2).
    pub imports: Vec<crate::structure::Import>,
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
    let (definitions, imports) = match &language {
        Some(language) if language.grammar.defines_symbols() => {
            definitions(text, &lines, &language.grammar)
        }
        _ => (Vec::new(), Vec::new()),
    };
    let markdown = language.is_none() && rel.to_ascii_lowercase().ends_with(".md");
    let sections = match markdown {
        true => sections(text, &lines),
        false => Vec::new(),
    };
    let units = match &language {
        Some(_) => code_units(&lines, &definitions),
        None if markdown => section_units(&lines, &sections),
        None => paragraph_units(&lines),
    };
    let mut chunks = merge(rel, &lines, &units, &definitions, budget);
    for chunk in &mut chunks {
        name_the_sections(rel, chunk, &sections);
    }
    Outline { definitions, sections, chunks, imports }
}

/// A file's definitions and imports alone, without the chunks and sections [`read`] also makes.
///
/// What an editor reads an open tab with on each change of its text (Unluminous `task-2231` §5.2), where
/// chunking the file for passage search would be work nobody asked for.
///
/// @param rel - the file's path, which decides its language
/// @param text - the file's text
pub fn read_structure(rel: &str, text: &str) -> (Vec<Definition>, Vec<crate::structure::Import>) {
    match grammars::for_path(rel) {
        Some(language) if language.grammar.defines_symbols() => {
            let lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
            definitions(text, &lines, &language.grammar)
        }
        _ => (Vec::new(), Vec::new()),
    }
}

/// Every heading of a Markdown file with the section under it.
///
/// @param text - the file's text
/// @param lines - its lines
fn sections(text: &str, lines: &[&str]) -> Vec<Section> {
    let headings = folding::markdown_headings(text);
    headings
        .iter()
        .enumerate()
        .map(|(at, &(line, level))| {
            let mut end = headings[at + 1..]
                .iter()
                .find(|(_, below)| *below <= level)
                .map_or(lines.len(), |(next, _)| *next);
            while end > line + 1 && lines[end - 1].trim().is_empty() {
                end -= 1;
            }
            Section {
                line: line as u32 + 1,
                end: end.max(line + 1) as u32,
                level: level as u32,
                heading: lines[line].trim().to_owned(),
            }
        })
        .collect()
}

/// Names the sections a Markdown chunk sits in and holds in its header, and its first heading as its
/// symbol, so a search word can match "which section" as a code chunk's header lets it match "which
/// function". Nothing for a chunk of any other file, which has no sections.
///
/// @param rel - the file's path
/// @param chunk - the chunk
/// @param sections - the file's sections
fn name_the_sections(rel: &str, chunk: &mut Chunk, sections: &[Section]) {
    if sections.is_empty() {
        return;
    }
    let mut named: Vec<&str> = sections
        .iter()
        .filter(|s| s.line < chunk.start && s.end >= chunk.start)
        .map(|s| s.heading.as_str())
        .collect();
    let inside = sections.iter().filter(|s| s.line >= chunk.start && s.line <= chunk.end);
    named.extend(inside.clone().take(4).map(|s| s.heading.as_str()));
    if !named.is_empty() {
        chunk.header = format!("{rel} | {}", named.join(" ; "));
    }
    if let Some(first) = inside.clone().next() {
        chunk.kind = "section";
        chunk.symbol = Some(first.heading.trim_start_matches('#').trim().to_owned());
    }
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
) -> (Vec<Definition>, Vec<crate::structure::Import>) {
    let symbols = FileSymbols::read(text, grammar);
    let regions = folding::regions_from(text, Reading::Code(grammar), &symbols.folding_tokens());
    let line_starts: Vec<usize> =
        std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let structure = crate::structure::read(text, grammar, &symbols, &regions);
    let extent = |line0: usize| {
        // The regions are sorted by head line, so the ones that open on this line or the next two are a
        // binary search away rather than a walk of every block in the file.
        let from = regions.partition_point(|r| r.head < line0);
        let end0 = regions[from..]
            .iter()
            .take_while(|r| r.head <= line0 + 2)
            .find(|r| matches!(r.kind, Kind::Block | Kind::Indent))
            .map(|r| r.last())
            .unwrap_or(line0);
        let mut start0 = line0;
        while start0 > 0 && is_preamble(lines[start0 - 1]) {
            start0 -= 1;
        }
        (start0, end0.max(line0))
    };
    let signature_of = |line0: usize| -> String {
        lines.get(line0).map(|l| l.trim().chars().take(200).collect()).unwrap_or_default()
    };
    let mut out: Vec<Definition> = Vec::new();
    for (d, shape) in symbols.definitions().iter().zip(structure.shapes) {
        // `import { type ReactNode }` names something defined elsewhere; the definer keyword inside an
        // import is not a definition.
        if structure.imports.iter().any(|i| i.range.contains(&d.name_range.start)) {
            continue;
        }
        let line0 = line_starts.partition_point(|&s| s <= d.name_range.start).saturating_sub(1);
        let (start0, end0) = extent(line0);
        out.push(Definition {
            name: text[d.name_range.clone()].to_owned(),
            kind: shape.kind.family(),
            line: line0 as u32 + 1,
            end: end0 as u32 + 1,
            start: start0 as u32 + 1,
            signature: signature_of(line0),
            likely: d.confidence == Confidence::Likely,
            exported: d.exported,
            depth: 0,
            symbol_kind: shape.kind,
            container: shape.container,
            parameters: shape.parameters,
            returns: shape.returns,
            type_text: shape.type_text,
            visibility: shape.visibility,
            doc: shape.doc,
            range: d.name_range.clone(),
            member: false,
        });
    }
    for member in structure.members {
        let line0 = member.line;
        out.push(Definition {
            name: member.name,
            kind: member.shape.kind.family(),
            line: line0 as u32 + 1,
            end: line0 as u32 + 1,
            start: extent(line0).0 as u32 + 1,
            signature: signature_of(line0),
            likely: false,
            exported: member.shape.visibility == crate::structure::Visibility::Public,
            depth: 0,
            symbol_kind: member.shape.kind,
            container: member.shape.container,
            parameters: member.shape.parameters,
            returns: member.shape.returns,
            type_text: member.shape.type_text,
            visibility: member.shape.visibility,
            doc: member.shape.doc,
            range: member.range,
            member: true,
        });
    }
    out.sort_by_key(|d| (d.line, std::cmp::Reverse(d.end)));
    for i in 0..out.len() {
        let (line, end) = (out[i].line, out[i].end);
        out[i].depth =
            out[..i].iter().filter(|o| o.line < line && o.end >= end && o.end > o.line).count()
                as u32;
    }
    (out, structure.imports)
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
    for d in definitions.iter().filter(|d| d.depth == 0 && !d.member) {
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

/// The units of a Markdown file: each heading down to the next heading of any level, and the text before
/// the first.
///
/// **Every heading starts a unit, not only the top level ones.** A `#` title's section is the whole of a
/// document, so taking only the outermost sections left one unit for the file: over the budget, it was
/// split at blank lines, and every `##` heading under the title was lost from the outline. That is
/// `task-2207`'s report: an outline of a ticket with a title and five `##` sections answered the title on
/// line 1 and a section with no name starting at a blank line 33. Small neighbouring sections are joined
/// again by the merge, within the budget.
///
/// @param lines - the file's lines
/// @param sections - its sections
fn section_units(lines: &[&str], sections: &[Section]) -> Vec<Unit> {
    let mut units = Vec::new();
    let mut at = 0usize;
    for (i, section) in sections.iter().enumerate() {
        let head = section.line as usize - 1;
        if head > at {
            units.push(Unit { start: at, end: head - 1, kind: "other" });
        }
        let next = sections.get(i + 1).map_or(lines.len(), |s| s.line as usize - 1);
        units.push(Unit { start: head, end: next.max(head + 1) - 1, kind: "section" });
        at = next.max(head + 1);
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
                || definitions.iter().any(|d| d.start as usize - 1 == i && d.depth > 0 && !d.member)
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
        definitions.iter().filter(|d| !d.member && d.line >= s && d.line <= e).collect();
    let enclosing: Vec<&Definition> =
        definitions.iter().filter(|d| !d.member && d.line < s && d.end >= s).collect();
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

    /// `task-2207`: a ticket with a `#` title and five `##` sections outlined as the title and a section
    /// with no name at a blank line. Every heading is a section now, nested by level, and each ends at the
    /// line before the next heading at its own level or higher.
    #[test]
    fn every_markdown_heading_is_a_section_whatever_its_level() {
        let mut text = String::from("# OBWV2-3 Max exposure\n\nA line about the ticket.\n\n");
        for name in ["Context", "Scope", "Deliverables", "Acceptance Criteria", "Links"] {
            text.push_str(&format!("## {name}\n\n"));
            for i in 0..30 {
                text.push_str(&format!("- {name} item {i} with enough words to weigh something\n"));
            }
            text.push('\n');
        }
        text.push_str("### Nested\nunder links\n");
        let read = read("tasks/ticket.md", &text, CHUNK_BUDGET);
        let headings: Vec<(&str, u32)> =
            read.sections.iter().map(|s| (s.heading.as_str(), s.level)).collect();
        assert_eq!(
            headings,
            vec![
                ("# OBWV2-3 Max exposure", 1),
                ("## Context", 2),
                ("## Scope", 2),
                ("## Deliverables", 2),
                ("## Acceptance Criteria", 2),
                ("## Links", 2),
                ("### Nested", 3),
            ]
        );
        let lines = text.lines().count() as u32;
        assert_eq!(read.sections[0].end, lines, "the title's section is the whole document");
        assert_eq!(read.sections[1].line, 5);
        assert_eq!(read.sections[1].end, 36, "Context ends before the blank line above Scope");
        assert_eq!(read.sections[5].end, lines, "Links holds the nested heading under it");
        // A chunk is never headed by a blank line, and every chunk that starts at a heading names it.
        for chunk in &read.chunks {
            let first = chunk.body.lines().next().unwrap_or_default();
            assert!(!first.trim().is_empty(), "a chunk starts at a blank line: {chunk:?}");
            if first.starts_with('#') {
                assert!(chunk.header.contains(first.trim()), "{chunk:?}");
            }
        }
    }

    #[test]
    fn a_heading_with_nothing_under_it_is_still_listed() {
        let read = read("notes.md", "# Title\n## Empty\n## Full\ntext\n", CHUNK_BUDGET);
        let lines: Vec<(u32, u32)> = read.sections.iter().map(|s| (s.line, s.end)).collect();
        assert_eq!(lines, vec![(1, 4), (2, 2), (3, 4)]);
    }

    #[test]
    fn a_hash_in_a_fence_is_not_a_section() {
        let read = read("notes.md", "# Title\n```sh\n# not a heading\n```\n", CHUNK_BUDGET);
        assert_eq!(read.sections.len(), 1);
    }

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
