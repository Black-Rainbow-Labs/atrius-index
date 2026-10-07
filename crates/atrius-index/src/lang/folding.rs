//! What in a file can be collapsed, and what somebody has collapsed.
//!
//! `task-1686` asks for the reference editor's fold arrows: a chevron beside the line number against a function,
//! an `if` or a large block, and a way of putting everything but the passages you have marked out of
//! sight. `tasks/task-1686-folding-tdd.md` records what was weighed; this module is the half of it
//! that has no window in it.
//!
//! Two things live here and they are deliberately separate.
//!
//! [`regions`] reads the text and says what **could** be folded. It is derived from the file and is
//! not state: open the same file twice and it answers the same thing, and every edit makes it answer
//! again. The tier is the one `task-1675` chose for go to definition and `task-1680` chose for
//! imports — a **syntactic reading**, from the same single `syntax::scan` pass that colours the file
//! — because a language server would answer better on the machines that happen to have one and not
//! at all on the rest, and a fold arrow that is silently absent looks like a fault.
//!
//! [`Folds`] is which of them somebody has collapsed, and it is state. It holds **byte offsets**
//! rather than line numbers, so that it can live in the [`crate::document::Document`] and be moved
//! by the same function that already moves the marked passages when the text changes. A set of line
//! numbers would be wrong the moment a line was typed at the top of the file.

use std::ops::Range;

use crate::lang::syntax::{scan, Grammar, Token};

/// What kind of thing a region is, which is [`Kind`]'s only reason for existing: the Language Server
/// Protocol's `FoldingRangeKind` is the same idea, and it is there so that "fold every comment" can
/// be a command rather than a special case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A bracket that opened on one line and closed on a later one.
    Block,
    /// A block comment over several lines, or a run of line comments.
    Comment,
    /// A line with lines under it that are indented further.
    Indent,
    /// A Markdown heading, down to the next heading at the same level or higher.
    Heading,
}

impl Kind {
    /// The word the command line prints and takes.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Block => "block",
            Kind::Comment => "comment",
            Kind::Indent => "indent",
            Kind::Heading => "heading",
        }
    }
}

/// One thing that can be collapsed: the line that stays, and the lines that go.
///
/// Whole lines, which is what every editor that folds folds and what the Language Server Protocol's
/// `FoldingRange` is. `head` and `body` count paragraphs from zero, which is what `unluminous-core` calls
/// a source line everywhere else and is one less than the number the gutter draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// The line the arrow is drawn against, which stays visible when the region is collapsed.
    pub head: usize,
    /// The lines that disappear. Never empty — a region that would hide nothing is dropped.
    pub body: Range<usize>,
    pub kind: Kind,
}

impl Region {
    /// The last line of the region, which is what the command line reports beside the head.
    pub fn last(&self) -> usize {
        self.body.end.saturating_sub(1)
    }

    /// True when `line` is the head of this region or inside its body — which is what "the region
    /// the caret is in" means.
    pub fn covers(&self, line: usize) -> bool {
        line == self.head || self.body.contains(&line)
    }

    /// How many lines it hides.
    pub fn hidden_lines(&self) -> usize {
        self.body.end - self.body.start
    }

    /// True when `other` is a region nested inside this one.
    ///
    /// The regions are sorted by head and nest properly — a bracket, a tag, an indent and a heading
    /// each close before their parent does — so a region is inside another exactly when its head is
    /// below the parent's and its last line is at or above the parent's last. The region itself does
    /// not contain itself, and a sibling does not contain its neighbour.
    pub fn contains_region(&self, other: &Region) -> bool {
        self.head < other.head && other.last() <= self.last()
    }
}

/// How a file is read for the things in it that could be folded.
///
/// The seam, and the reason there is no list of languages inside this module: the window knows what
/// kind of file it has open — it already asks `services::file_kind` and `services::plugins` — and
/// hands the answer down.
#[derive(Debug, Clone, Copy)]
pub enum Reading<'a> {
    /// A switched-on plugin claims this file, so its brackets and its comments can be read.
    Code(&'a Grammar),
    /// A Markdown document: its headings, and its indentation.
    Markdown,
    /// Nothing is known about it, so its indentation is all there is to go on.
    Plain,
}

/// Every comment and string in a file, in order.
///
/// The one thing reading a file for its blocks needs a tokeniser for: a `}` inside `// }` or inside
/// `"}"` is not a bracket. It is a type of its own so that it can be handed in by a caller who has
/// **already** read the file — the window colours a file on the same text revision it folds it on,
/// and one `syntax::scan` answering both was worth 2.5 ms a keystroke on the largest file in this
/// repository. The same shape, and the same reason, as `imports::Tokens`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tokens {
    /// Each span, and whether it is a comment rather than a string.
    quiet: Vec<(Range<usize>, bool)>,
}

impl Tokens {
    /// Note one span. Called from the visitor a caller is already running.
    pub fn note(&mut self, range: Range<usize>, comment: bool) {
        self.quiet.push((range, comment));
    }

    pub fn is_empty(&self) -> bool {
        self.quiet.is_empty()
    }

    /// Put the spans back into position order, for a caller that noted them out of order.
    ///
    /// The one caller is the window colouring a markup file: a `<style>` block's own comments and
    /// strings are read after the whole of the outer file has been, so they arrive last and belong
    /// in the middle. [`Tokens::covers`] is a binary search, so a list out of order is not a slower
    /// answer, it is a wrong one.
    pub fn put_in_order(&mut self) {
        self.quiet.sort_by_key(|(range, _)| range.start);
    }

    /// True when `offset` is inside a comment or a string.
    ///
    /// Public because a second reader wants the same answer: `Document::bracket_pair` asks it of
    /// every bracket it walks past, so that a `}` inside `// }` or inside `"}"` is not a bracket.
    /// Reading a file for its blocks and matching one pair of brackets are the same question about
    /// the same tokens, and one of them already has them read.
    pub fn covers(&self, offset: usize) -> bool {
        let index = self.quiet.partition_point(|(range, _)| range.end <= offset);
        self.quiet.get(index).is_some_and(|(range, _)| range.contains(&offset))
    }
}

/// Read a file's comments and strings, for a caller that has not already.
pub fn tokens(text: &str, grammar: &Grammar) -> Tokens {
    let mut read = Tokens::default();
    scan(text, grammar, |range, token| match token {
        Token::Comment => read.note(range, true),
        Token::String => read.note(range, false),
        _ => {}
    });
    read
}

/// Everything in `text` that could be collapsed, sorted by head line and then widest first.
///
/// Regions nest: a method inside a class inside a module is three of them with three arrows. Two are
/// dropped — one that would hide nothing, and the narrower of two that share a head line, because
/// `fn f() {` opens a bracket twice on one line and deserves one arrow rather than two.
pub fn regions(text: &str, reading: Reading<'_>) -> Vec<Region> {
    let read = match reading {
        Reading::Code(grammar) => tokens(text, grammar),
        _ => Tokens::default(),
    };
    regions_from(text, reading, &read)
}

/// The same, for a caller that has already read the file's comments and strings.
pub fn regions_from(text: &str, reading: Reading<'_>, read: &Tokens) -> Vec<Region> {
    let lines = LineIndex::new(text);
    let mut found = Vec::new();
    match reading {
        Reading::Code(grammar) => {
            found.extend(block_regions(text, &lines, read));
            found.extend(comment_regions(text, &lines, read, grammar));
            // A markup document has almost no brackets outside its `<script>` and `<style>` blocks,
            // so what folds in it is a tag. The brackets are still read, because those two blocks
            // are worth folding, and `tidy` already keeps one arrow a head line.
            if grammar.markup {
                found.extend(tag_regions(text, &lines, grammar));
            }
        }
        Reading::Markdown => found.extend(heading_regions(text, &lines)),
        Reading::Plain => {}
    }
    // Indentation last, and only where nothing better already answers for that line. In a braces
    // language every block has both and the bracket's answer is the better one, because it knows
    // where the block closed; what is left over is the languages with no braces at all.
    let already: Vec<usize> = found.iter().map(|region| region.head).collect();
    for region in indent_regions(text, &lines) {
        if !already.contains(&region.head) {
            found.push(region);
        }
    }
    tidy(found)
}

/// What each part of [`regions`] costs, for `examples/folding_cost.rs`. Temporary measurement aid.
pub fn breakdown(text: &str, reading: Reading<'_>) -> Vec<(&'static str, f64)> {
    use std::time::Instant;
    let mut out = Vec::new();
    let start = Instant::now();
    let lines = LineIndex::new(text);
    out.push(("line index", start.elapsed().as_secs_f64() * 1000.0));
    if let Reading::Code(grammar) = reading {
        let start = Instant::now();
        let quiet = tokens(text, grammar);
        out.push(("scan", start.elapsed().as_secs_f64() * 1000.0));
        let start = Instant::now();
        let _ = block_regions(text, &lines, &quiet);
        out.push(("brackets", start.elapsed().as_secs_f64() * 1000.0));
        let start = Instant::now();
        let _ = comment_regions(text, &lines, &quiet, grammar);
        out.push(("comments", start.elapsed().as_secs_f64() * 1000.0));
    }
    let start = Instant::now();
    let found = indent_regions(text, &lines);
    out.push(("indent", start.elapsed().as_secs_f64() * 1000.0));
    let start = Instant::now();
    let _ = tidy(found);
    out.push(("tidy", start.elapsed().as_secs_f64() * 1000.0));
    out
}

/// Put the regions in order and drop the ones that are no use.
///
/// Sorted by head and then **widest first**, so that a caret looking for the innermost region it is
/// in can walk the list and take the last one that covers it.
fn tidy(mut found: Vec<Region>) -> Vec<Region> {
    found.retain(|region| region.body.start < region.body.end);
    found.sort_by(|a, b| a.head.cmp(&b.head).then(b.hidden_lines().cmp(&a.hidden_lines())));
    found.dedup_by_key(|region| region.head);
    found
}

/// Where every line starts and ends, so a byte offset can be turned into a line number without
/// walking the file once per bracket.
struct LineIndex {
    /// The byte each line starts at, and one more entry holding the length of the text.
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        for (at, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(at + 1);
            }
        }
        starts.push(text.len() + 1);
        Self { starts }
    }

    /// How many lines the text has. The same count [`crate::rope::Rope::len_lines`] gives, so a line
    /// number from here is a paragraph number in the layout.
    fn count(&self) -> usize {
        self.starts.len() - 1
    }

    /// Which line a byte offset falls on.
    fn line_of(&self, offset: usize) -> usize {
        self.starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
            .min(self.count() - 1)
    }

    /// One line's text, without its line break.
    fn text<'a>(&self, source: &'a str, line: usize) -> &'a str {
        let start = self.starts[line];
        let end = self.starts[line + 1].min(source.len() + 1) - 1;
        let end = end.min(source.len());
        source[start..end].trim_end_matches('\r')
    }
}

/// Every bracket that opened on one line and closed on a later one.
///
/// Parentheses count as well as braces and square brackets, because a wrapped argument list is a
/// real thing to fold in Rust and TypeScript and leaving them out would be a rule with no reason
/// behind it.
///
/// One `match` over the bytes rather than a search through a table of pairs for each: this walks
/// every byte of the file on every text change, and `task-1666`'s rule is that what runs that often
/// does the least it can.
fn block_regions(text: &str, lines: &LineIndex, read: &Tokens) -> Vec<Region> {
    let mut open: Vec<(u8, usize)> = Vec::new();
    let mut found = Vec::new();
    for (at, byte) in text.bytes().enumerate() {
        let opening = matches!(byte, b'{' | b'[' | b'(');
        let wants = match byte {
            b'}' => b'{',
            b']' => b'[',
            b')' => b'(',
            _ if opening => 0,
            _ => continue,
        };
        if read.covers(at) {
            continue;
        }
        if opening {
            open.push((byte, at));
            continue;
        }
        // The innermost matching opener. A stray closer with nothing open is ignored rather than
        // taken as an error: half-typed source is the ordinary state of a file being edited.
        let Some(index) = open.iter().rposition(|(kind, _)| *kind == wants) else {
            continue;
        };
        let (_, from) = open.remove(index);
        // Anything still open inside this pair was never closed, so it is thrown away with it.
        open.truncate(index);
        if let Some(region) = block_region(text, lines, from, at) {
            found.push(region);
        }
    }
    found
}

/// One matched bracket pair as a region, if it spans lines at all.
fn block_region(text: &str, lines: &LineIndex, from: usize, to: usize) -> Option<Region> {
    let head = lines.line_of(from);
    let close = lines.line_of(to);
    if close <= head {
        return None;
    }
    // The closing line goes with the body, so folding a function leaves one line on the screen
    // rather than two — unless there is a word after the bracket. `});` and `},` fold away; `} else
    // {` and `} catch (error) {` stay, because hiding the `else` of an `if` somebody folded is
    // hiding the half of the statement they were trying to see the shape of.
    let after = &text[to + 1..lines.starts[close + 1].min(text.len() + 1) - 1];
    let trailing_word = after.chars().any(|c| c.is_alphanumeric() || c == '_');
    let end = if trailing_word { close } else { close + 1 };
    Some(Region { head, body: head + 1..end, kind: Kind::Block })
}

/// Every element that opened on one line and closed on a later one.
///
/// `crate::lang::syntax::tags` is where the tags come from rather than a walk over the angle brackets
/// here, and that is the whole reason this is short: a `<` inside a comment, inside an attribute
/// value or inside a `<script>` body has already been ruled out by the reader that knows about all
/// three.
///
/// **There is no list of void elements**, and not needing one is worth stating, because it is the
/// first thing anybody writing this reaches for. `<br>` and `<img>` never close, so a stack would be
/// left holding them for ever. The rule that removes the need is the one [`block_regions`] already
/// uses for a stray closing brace: an end tag pops back to the nearest matching name on the stack
/// and throws away whatever was above it, so a `<br>` is discarded the moment its parent closes.
fn tag_regions(text: &str, lines: &LineIndex, grammar: &Grammar) -> Vec<Region> {
    let mut open: Vec<(&str, usize)> = Vec::new();
    let mut found = Vec::new();
    for tag in crate::lang::syntax::tags(text, grammar) {
        let name = &text[tag.name.clone()];
        if name.is_empty() {
            continue;
        }
        if !tag.closing {
            if !tag.self_closing {
                open.push((name, tag.open));
            }
            continue;
        }
        let Some(index) = open.iter().rposition(|(known, _)| known.eq_ignore_ascii_case(name))
        else {
            continue;
        };
        let (_, from) = open.remove(index);
        // Anything still open inside this pair never closed, so it goes with it — which is what
        // makes `<br>` and `<img>` cost nothing.
        open.truncate(index);
        if let Some(region) = tag_region(text, lines, from, tag.end) {
            found.push(region);
        }
    }
    found
}

/// One matched pair of tags as a region, if it spans lines at all.
///
/// The closing line goes with the body — folding a `<div>` leaves one line on the screen rather than
/// two — unless there is a word after the end tag, which is [`block_region`]'s rule and is here for
/// the same reason: `</span> and the rest of the sentence` must not disappear.
fn tag_region(text: &str, lines: &LineIndex, from: usize, to: usize) -> Option<Region> {
    let head = lines.line_of(from);
    let close = lines.line_of(to.saturating_sub(1).max(from));
    if close <= head {
        return None;
    }
    let after = &text[to.min(text.len())..lines.starts[close + 1].min(text.len() + 1) - 1];
    let trailing_word = after.chars().any(|c| c.is_alphanumeric() || c == '_');
    let end = if trailing_word { close } else { close + 1 };
    Some(Region { head, body: head + 1..end, kind: Kind::Block })
}

/// Block comments that span lines, and runs of two or more line comments.
fn comment_regions(text: &str, lines: &LineIndex, read: &Tokens, grammar: &Grammar) -> Vec<Region> {
    let mut found = Vec::new();
    for (range, comment) in &read.quiet {
        if !comment {
            continue;
        }
        let head = lines.line_of(range.start);
        let last = lines.line_of(range.end.saturating_sub(1).max(range.start));
        if last > head {
            found.push(Region { head, body: head + 1..last + 1, kind: Kind::Comment });
        }
    }
    // A run of line comments: the licence header at the top of a file, and the doc comment above a
    // function in a language whose doc comments are line comments, which is most of them.
    let Some(opener) = grammar.line_comment.as_deref().filter(|it| !it.is_empty()) else {
        return found;
    };
    let is_comment_line = |line: usize| {
        let text = lines.text(text, line).trim_start();
        text.starts_with(opener)
    };
    let mut line = 0;
    while line < lines.count() {
        if !is_comment_line(line) {
            line += 1;
            continue;
        }
        let mut end = line + 1;
        while end < lines.count() && is_comment_line(end) {
            end += 1;
        }
        if end > line + 1 {
            found.push(Region { head: line, body: line + 1..end, kind: Kind::Comment });
        }
        line = end.max(line + 1);
    }
    found
}

/// How far a line is indented, counting a tab as one. `None` for a blank line, which is ignored.
///
/// A tab as one rather than as four, because the comparison is only ever between two lines of the
/// same file and a file that mixes them has a worse problem than its fold arrows.
fn indent_of(line: &str) -> Option<usize> {
    let depth = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
    if line[depth.min(line.len())..].trim().is_empty() {
        None
    } else {
        Some(depth)
    }
}

/// VS Code's rule, implemented literally: a region starts at a line with a smaller indent than the
/// lines that follow, and ends at the last line before the indent comes back to that level or below.
///
/// Blank lines are ignored, and trailing blank lines are **not** part of the region — otherwise
/// every fold in a file with a blank line between its functions would swallow the gap after it.
fn indent_regions(text: &str, lines: &LineIndex) -> Vec<Region> {
    let depths: Vec<Option<usize>> =
        (0..lines.count()).map(|line| indent_of(lines.text(text, line))).collect();
    let mut found = Vec::new();
    for head in 0..lines.count() {
        let Some(depth) = depths[head] else { continue };
        let mut end = head;
        for (line, below) in depths.iter().enumerate().skip(head + 1) {
            match below {
                None => continue,
                Some(deeper) if *deeper > depth => end = line,
                Some(_) => break,
            }
        }
        if end > head {
            found.push(Region { head, body: head + 1..end + 1, kind: Kind::Indent });
        }
    }
    found
}

/// A Markdown heading folds everything down to the next heading at the same level or higher.
///
/// A heading inside a fenced code block is not a heading, which the reader already has to know
/// because `#` is a comment in half the languages people put in fences.
fn heading_regions(text: &str, lines: &LineIndex) -> Vec<Region> {
    let levels = heading_levels(text, lines);
    let mut found = Vec::new();
    for (head, level) in levels.iter().enumerate() {
        let Some(level) = level else { continue };
        let mut end = lines.count();
        for (line, below) in levels.iter().enumerate().skip(head + 1) {
            if below.is_some_and(|deeper| deeper <= *level) {
                end = line;
                break;
            }
        }
        // Trailing blank lines belong to whatever comes next, not to the section above them.
        while end > head + 1 && indent_of(lines.text(text, end - 1)).is_none() {
            end -= 1;
        }
        if end > head + 1 {
            found.push(Region { head, body: head + 1..end, kind: Kind::Heading });
        }
    }
    found
}

/// Every Markdown heading in `text`, as its line from zero and its level from one to six.
///
/// A region is only made for a heading with lines under it, so a heading followed straight away by
/// another one has no region. The outline lists every heading whatever is under it, which is why it asks
/// this rather than [`regions`].
pub fn markdown_headings(text: &str) -> Vec<(usize, usize)> {
    let lines = LineIndex::new(text);
    heading_levels(text, &lines)
        .into_iter()
        .enumerate()
        .filter_map(|(line, level)| level.map(|level| (line, level)))
        .collect()
}

/// The heading level of each line, or nothing for a line that is not a heading.
fn heading_levels(text: &str, lines: &LineIndex) -> Vec<Option<usize>> {
    let mut levels: Vec<Option<usize>> = Vec::with_capacity(lines.count());
    let mut fence: Option<char> = None;
    for line in 0..lines.count() {
        let source = lines.text(text, line);
        let trimmed = source.trim_start();
        let fence_char = trimmed.chars().next().filter(|c| *c == '`' || *c == '~');
        let fenced = fence_char.is_some_and(|c| trimmed.starts_with(&c.to_string().repeat(3)));
        match (fence, fenced) {
            (None, true) => fence = fence_char,
            (Some(open), true) if Some(open) == fence_char => fence = None,
            _ => {}
        }
        if fence.is_some() || !trimmed.starts_with('#') {
            levels.push(None);
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        let rest = &trimmed[hashes..];
        // `#hashtag` is not a heading; `#` alone on a line is.
        let heading =
            (1..=6).contains(&hashes) && (rest.is_empty() || rest.starts_with(char::is_whitespace));
        levels.push(heading.then_some(hashes));
    }
    levels
}

/// Which regions somebody has collapsed.
///
/// **Byte offsets, not line numbers**: this lives inside a [`crate::document::Document`] so that
/// `Document::splice` — the one place in Unluminous that knows a range of bytes moved — shifts it in
/// the same line that already shifts the marked passages. Line numbers would be wrong the first time
/// somebody typed a line at the top of the file.
///
/// Each offset is the start of a collapsed region's **head line**, and a region counts as collapsed
/// when one of them falls anywhere inside that line. Snapping to the line rather than to the byte is
/// what stops a fold popping open when a letter is typed at the start of the head.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Folds {
    at: Vec<usize>,
}

impl Folds {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.at.is_empty()
    }

    pub fn len(&self) -> usize {
        self.at.len()
    }

    /// The offsets, in order. What the tests read and what a caller rebuilding the set walks.
    pub fn offsets(&self) -> &[usize] {
        &self.at
    }

    /// Collapse the region whose head line starts at `offset`.
    pub fn add(&mut self, offset: usize) {
        if let Err(index) = self.at.binary_search(&offset) {
            self.at.insert(index, offset);
        }
    }

    /// Expand whatever is collapsed inside `line`, which is the head line's byte range.
    pub fn remove_in(&mut self, line: Range<usize>) {
        self.at.retain(|at| !(line.start..line.end.max(line.start + 1)).contains(at));
    }

    pub fn clear(&mut self) {
        self.at.clear();
    }

    /// True when something inside `line` is collapsed.
    pub fn holds(&self, line: &Range<usize>) -> bool {
        let end = line.end.max(line.start + 1);
        let index = self.at.partition_point(|at| *at < line.start);
        self.at.get(index).is_some_and(|at| *at < end)
    }

    /// Text was inserted, so everything after it moves along.
    ///
    /// An offset exactly at the insertion point does **not** move, because the head line still
    /// starts there: typing at the start of a folded head adds to that line rather than pushing it
    /// down the file. Text inserted at the end of the line before it arrives at the same offset and
    /// is the same answer, since the fold is snapped to the line either way.
    pub fn insert(&mut self, at: usize, length: usize) {
        for offset in &mut self.at {
            if *offset > at {
                *offset += length;
            }
        }
    }

    /// Text was removed. An offset inside what went is dropped, and the rest move back.
    pub fn remove(&mut self, range: Range<usize>) {
        let length = range.end - range.start;
        self.at.retain(|offset| !(range.start..range.end).contains(offset));
        for offset in &mut self.at {
            if *offset >= range.end {
                *offset -= length;
            }
        }
    }

    /// Drop anything past the end of a text, which is what reading a set back needs.
    pub fn clamp(&mut self, length: usize) {
        self.at.retain(|offset| *offset <= length);
    }
}

/// Which paragraphs are hidden, worked out from the regions and the collapsed set.
///
/// Sorted and never overlapping, which is what makes [`Hidden::contains`] a binary search — it is
/// asked once per paragraph while a document is laid out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hidden {
    ranges: Vec<Range<usize>>,
}

impl Hidden {
    /// Nothing is folded, which is every caller that does not fold: the Markdown preview, the
    /// diagram view, and every test in this crate that is not about folding.
    pub fn none() -> Self {
        Self::default()
    }

    /// The union of the bodies of the regions that are collapsed.
    ///
    /// A union rather than a list, because regions nest: collapsing a class and a method inside it
    /// hides one stretch of the file, not two overlapping ones.
    pub fn of(ranges: impl IntoIterator<Item = Range<usize>>) -> Self {
        let mut ranges: Vec<Range<usize>> =
            ranges.into_iter().filter(|range| range.start < range.end).collect();
        ranges.sort_by_key(|range| range.start);
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
        for range in ranges {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        Self { ranges: merged }
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// True when this paragraph produces no lines.
    pub fn contains(&self, paragraph: usize) -> bool {
        let index = self.ranges.partition_point(|range| range.end <= paragraph);
        self.ranges.get(index).is_some_and(|range| range.start <= paragraph)
    }

    /// How many paragraphs are hidden altogether, which is what a status line reports.
    pub fn count(&self) -> usize {
        self.ranges.iter().map(|range| range.end - range.start).sum()
    }

    pub fn ranges(&self) -> &[Range<usize>] {
        &self.ranges
    }
}

/// The innermost region that covers `line`, which is what "fold the block the caret is in" means.
///
/// The regions are sorted widest first within a head, and an inner region has a later head than the
/// one holding it, so the **last** one that covers the line is the innermost.
pub fn region_at(regions: &[Region], line: usize) -> Option<&Region> {
    regions.iter().rfind(|region| region.covers(line))
}

/// The region headed by `line`, and every region nested inside it, in the order the regions are.
///
/// `None` when nothing is headed by `line`, which is what "there is no block that starts on this
/// line" means. The root plus every region that [`Region::contains_region`] answers yes for: a
/// grandchild passes the test too, because the test is against the root rather than against the
/// parent, so the whole subtree comes back in one pass. `tasks/task-1707-recursive-folding-tdd.md`
/// section 3.
pub fn region_tree(regions: &[Region], line: usize) -> Option<Vec<&Region>> {
    let root = regions.iter().find(|region| region.head == line)?;
    let mut tree = vec![root];
    tree.extend(regions.iter().filter(|region| root.contains_region(region)));
    Some(tree)
}

/// Every region that has to stay open for `line` to be visible: the ones whose body holds it.
pub fn regions_holding(regions: &[Region], line: usize) -> Vec<&Region> {
    regions.iter().filter(|region| region.body.contains(&line)).collect()
}

/// VS Code's `foldAllExcept`, stated over lines: collapse everything, then keep open every region
/// that holds one of `keep` and every region holding one of those.
///
/// Expanding the parents is what makes it work at all — a marked line inside a method inside a class
/// is visible only if the class and the method are both open.
pub fn collapse_all_but(regions: &[Region], keep: &[usize]) -> Vec<usize> {
    regions
        .iter()
        .filter(|region| !keep.iter().any(|line| region.covers(*line)))
        .map(|region| region.head)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn rust() -> Grammar {
        Grammar {
            language: "rust".to_owned(),
            keywords: vec!["fn".to_owned(), "if".to_owned()],
            line_comment: Some("//".to_owned()),
            block_comment: Some(("/*".to_owned(), "*/".to_owned())),
            strings: vec!['"'],
            escapes: true,
            operators: vec!['{', '}', '(', ')'],
            numbers: true,
            ..Grammar::default()
        }
    }

    /// A markup grammar for the tag-region tests: the one thing that matters is `markup` and the
    /// raw text pair, because a `<script>` body is where a naive bracket or bracket-less `<` walk
    /// would go wrong.
    fn html() -> Grammar {
        Grammar {
            language: "HTML".to_owned(),
            keywords: vec![
                "div".to_owned(),
                "p".to_owned(),
                "br".to_owned(),
                "img".to_owned(),
                "script".to_owned(),
            ],
            markup: true,
            raw_text: vec![("script".to_owned(), Some("javascript".to_owned()))],
            ..Grammar::default()
        }
    }

    fn heads(regions: &[Region]) -> Vec<(usize, usize, &'static str)> {
        regions.iter().map(|r| (r.head, r.last(), r.kind.name())).collect()
    }

    #[test]
    fn a_function_folds_from_its_brace_to_its_closing_brace() {
        let source = "fn one() {\n    let a = 1;\n    let b = 2;\n}\nfn two() {}\n";
        let found = regions(source, Reading::Code(&rust()));
        assert_eq!(heads(&found), vec![(0, 3, "block")]);
        // `fn two() {}` opens and closes on one line, so it is not a region at all.
    }

    #[test]
    fn an_if_inside_a_function_is_a_region_of_its_own() {
        let source = "fn one() {\n    if a {\n        b();\n    }\n}\n";
        let found = regions(source, Reading::Code(&rust()));
        assert_eq!(heads(&found), vec![(0, 4, "block"), (1, 3, "block")]);
        assert_eq!(region_at(&found, 2).map(|r| r.head), Some(1), "the innermost region wins");
        assert_eq!(region_at(&found, 4).map(|r| r.head), Some(0));
    }

    /// The study's own shape: a function holding a `for` holding an `if`, so three regions deep.
    fn nested() -> (&'static str, Grammar) {
        (
            "fn total_area() {\n    let s = 0;\n    for side in sides {\n        if side > 0 {\n            s += side;\n        }\n    }\n    s\n}\nfn other() {\n    x();\n}\n",
            rust(),
        )
    }

    #[test]
    fn a_region_tree_at_the_outer_head_is_the_whole_subtree() {
        let (source, grammar) = nested();
        let found = regions(source, Reading::Code(&grammar));
        // Heads: the function at 0, the for at 2, the if at 3, the other function at 9.
        let tree = region_tree(&found, 0).expect("the function heads line 0");
        assert_eq!(tree.iter().map(|region| region.head).collect::<Vec<_>>(), vec![0, 2, 3]);
    }

    #[test]
    fn a_region_tree_at_a_nested_head_is_that_region_and_its_children() {
        let (source, grammar) = nested();
        let found = regions(source, Reading::Code(&grammar));
        let tree = region_tree(&found, 2).expect("the for heads line 2");
        assert_eq!(tree.iter().map(|region| region.head).collect::<Vec<_>>(), vec![2, 3]);
    }

    #[test]
    fn a_region_tree_at_a_line_that_heads_nothing_is_none() {
        let (source, grammar) = nested();
        let found = regions(source, Reading::Code(&grammar));
        assert!(region_tree(&found, 1).is_none(), "line 1 is inside the function, not a head");
        assert!(
            region_tree(&found, 10).is_none(),
            "line 10 is inside the other function, not a head"
        );
    }

    #[test]
    fn contains_region_is_true_for_a_grandchild_and_false_for_a_sibling_and_itself() {
        let (source, grammar) = nested();
        let found = regions(source, Reading::Code(&grammar));
        let function = &found[0];
        let other = found.iter().find(|region| region.head == 9).expect("the other function");
        assert!(function.contains_region(&found[1]), "the for is inside the function");
        assert!(function.contains_region(&found[2]), "the if is a grandchild, still inside");
        assert!(!function.contains_region(other), "a sibling is not inside");
        assert!(!function.contains_region(function), "a region does not contain itself");
    }

    #[test]
    fn a_closing_line_with_a_word_after_the_bracket_stays_visible() {
        // Hiding the `else` of an `if` somebody folded hides the half of the statement they were
        // trying to see the shape of.
        let source = "if a {\n    one();\n} else {\n    two();\n}\n";
        let found = regions(source, Reading::Code(&rust()));
        assert_eq!(heads(&found), vec![(0, 1, "block"), (2, 4, "block")]);
    }

    #[test]
    fn a_closing_line_with_only_punctuation_after_it_folds_away() {
        let source = "call(|| {\n    one();\n});\nafter();\n";
        let found = regions(source, Reading::Code(&rust()));
        assert_eq!(found[0].head, 0);
        assert_eq!(found[0].last(), 2, "the closing line goes with the body");
    }

    #[test]
    fn a_bracket_in_a_string_or_a_comment_is_not_a_bracket() {
        let source = "fn one() {\n    let a = \"}\";\n    // }\n    let b = 2;\n}\n";
        let found = regions(source, Reading::Code(&rust()));
        assert_eq!(found[0].head, 0);
        assert_eq!(found[0].last(), 4, "the closing brace is the real one on the last line");
    }

    #[test]
    fn an_unclosed_bracket_is_not_a_region() {
        let source = "fn one() {\n    let a = 1;\n";
        let found = regions(source, Reading::Code(&rust()));
        assert!(
            found.iter().all(|region| region.kind != Kind::Block),
            "a half-typed function has no block to fold: {found:?}"
        );
    }

    #[test]
    fn a_block_comment_over_several_lines_folds() {
        let source = "/**\n * One.\n */\nfn one() {}\n";
        let found = regions(source, Reading::Code(&rust()));
        assert_eq!(heads(&found), vec![(0, 2, "comment")]);
    }

    #[test]
    fn a_run_of_line_comments_folds_and_a_single_one_does_not() {
        let source = "// one\n// two\n// three\nfn a() {}\n\n// alone\nfn b() {}\n";
        let found = regions(source, Reading::Code(&rust()));
        assert_eq!(heads(&found), vec![(0, 2, "comment")]);
    }

    #[test]
    fn indentation_folds_a_language_with_no_brackets() {
        let source = "def one():\n    a = 1\n    b = 2\n\ndef two():\n    c = 3\n";
        let found = regions(source, Reading::Plain);
        assert_eq!(heads(&found), vec![(0, 2, "indent"), (4, 5, "indent")]);
    }

    #[test]
    fn a_blank_line_after_a_block_is_not_part_of_it() {
        let source = "one:\n    a\n\n\ntwo:\n    b\n";
        let found = regions(source, Reading::Plain);
        assert_eq!(found[0].last(), 1, "the two blank lines belong to nothing");
    }

    #[test]
    fn a_file_that_is_all_one_level_has_nothing_to_fold() {
        assert!(regions("one\ntwo\nthree\n", Reading::Plain).is_empty());
    }

    #[test]
    fn a_markdown_heading_folds_down_to_the_next_one_at_its_level() {
        let source = "# One\ntext\n## Two\nmore\n## Three\nlast\n# Four\n";
        let found = regions(source, Reading::Markdown);
        let headings: Vec<_> =
            found.iter().filter(|r| r.kind == Kind::Heading).map(|r| (r.head, r.last())).collect();
        assert_eq!(headings, vec![(0, 5), (2, 3), (4, 5)]);
    }

    #[test]
    fn a_hash_inside_a_fence_is_not_a_heading() {
        let source = "# One\n```py\n# a comment\nprint(1)\n```\ndone\n";
        let found = regions(source, Reading::Markdown);
        assert_eq!(
            found.iter().filter(|r| r.kind == Kind::Heading).count(),
            1,
            "only the real heading: {found:?}"
        );
    }

    #[test]
    fn an_indentation_region_is_dropped_where_a_block_already_answers_for_that_line() {
        let source = "fn one() {\n    let a = 1;\n}\n";
        let found = regions(source, Reading::Code(&rust()));
        assert_eq!(found.iter().filter(|r| r.head == 0).count(), 1);
        assert_eq!(found[0].kind, Kind::Block);
    }

    #[test]
    fn folds_move_with_the_text() {
        let mut folds = Folds::new();
        folds.add(20);
        folds.insert(10, 5);
        assert_eq!(folds.offsets(), &[25]);
        folds.remove(0..5);
        assert_eq!(folds.offsets(), &[20]);
        // An offset inside what was removed goes with it.
        folds.remove(18..24);
        assert!(folds.is_empty());
    }

    #[test]
    fn typing_at_the_start_of_a_folded_head_does_not_move_the_fold() {
        let mut folds = Folds::new();
        folds.add(30);
        folds.insert(30, 4);
        assert_eq!(folds.offsets(), &[30], "the head line still starts where it did");
    }

    #[test]
    fn a_fold_is_snapped_to_its_head_line() {
        let mut folds = Folds::new();
        folds.add(12);
        assert!(folds.holds(&(10..20)));
        assert!(!folds.holds(&(0..10)));
        folds.remove_in(10..20);
        assert!(folds.is_empty());
    }

    #[test]
    fn nested_collapsed_regions_hide_one_stretch_of_the_file() {
        let hidden = Hidden::of([1..10, 3..6, 20..21]);
        assert_eq!(hidden.ranges(), &[1..10, 20..21]);
        assert!(hidden.contains(1) && hidden.contains(9) && hidden.contains(20));
        assert!(!hidden.contains(0) && !hidden.contains(10) && !hidden.contains(21));
        assert_eq!(hidden.count(), 10);
    }

    #[test]
    fn collapse_all_but_keeps_the_regions_holding_what_was_marked_and_their_parents() {
        let source =
            "fn one() {\n    if a {\n        marked();\n    }\n}\nfn two() {\n    b();\n}\n";
        let found = regions(source, Reading::Code(&rust()));
        let collapsed = collapse_all_but(&found, &[2]);
        assert_eq!(collapsed, vec![5], "only the function with nothing marked in it");
    }

    #[test]
    fn collapse_all_but_nothing_collapses_everything() {
        let source = "fn one() {\n    a();\n}\nfn two() {\n    b();\n}\n";
        let found = regions(source, Reading::Code(&rust()));
        assert_eq!(collapse_all_but(&found, &[]), vec![0, 3]);
    }

    #[test]
    fn a_tag_pair_folds_even_when_the_file_is_not_indented() {
        // The point of reading tags rather than indentation: the badly-indented file is the one
        // somebody needs to fold, and it has no indentation to fold by.
        let source = "<div>\n<p>one</p>\n<p>two</p>\n</div>\n";
        let found = regions(source, Reading::Code(&html()));
        assert_eq!(heads(&found), vec![(0, 3, "block")]);
    }

    #[test]
    fn a_nested_tag_is_a_region_of_its_own() {
        let source = "<div>\n<p>\ntext\n</p>\n</div>\n";
        let found = regions(source, Reading::Code(&html()));
        assert_eq!(heads(&found), vec![(0, 4, "block"), (1, 3, "block")]);
    }

    #[test]
    fn an_unclosed_tag_is_not_a_region() {
        let source = "<div>\n<p>one</p>\n";
        let found = regions(source, Reading::Code(&html()));
        assert!(found.is_empty(), "a half-typed element has nothing to fold: {found:?}");
    }

    #[test]
    fn a_br_and_an_img_inside_a_tag_do_not_hold_the_stack() {
        // Neither ever closes, and there is no list of void elements: the parent's end tag pops
        // back to itself and throws both away.
        let source = "<div>\n<br>\n<img src=\"a.png\">\n</div>\n";
        let found = regions(source, Reading::Code(&html()));
        assert_eq!(heads(&found), vec![(0, 3, "block")]);
    }

    #[test]
    fn a_less_than_inside_a_script_body_opens_no_tag() {
        // `tag_regions` runs the state machine rather than a walk over the angle brackets, so the
        // comparison in the body is not a tag and cannot open a region.
        let source = "<script>\na < b\nb > a\n</script>\n";
        let found = regions(source, Reading::Code(&html()));
        assert_eq!(heads(&found), vec![(0, 3, "block")]);
    }

    #[test]
    fn a_closing_line_with_a_word_after_the_tag_stays_visible() {
        let source = "<div>\ntext\n</div> and more\n";
        let found = regions(source, Reading::Code(&html()));
        assert_eq!(heads(&found), vec![(0, 1, "block")]);
    }
}
