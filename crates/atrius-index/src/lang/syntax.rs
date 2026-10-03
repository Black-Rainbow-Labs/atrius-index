//! Reading source code into coloured tokens.
//!
//! It lives here, next to the Markdown parser, for the same reason that one does: it reads text and
//! produces spans over it, it draws nothing, and its tests run with no window, no graphics card and
//! no fonts. Nothing here knows what a colour scheme is either — a [`Token`] is what a stretch of
//! text *is*, and the window decides what colour that is.
//!
//! ## What it does, and what it does not
//!
//! One linear pass, no regular expressions, no dependency. The rules are tried in this order, and
//! the order is the whole design:
//!
//! 1. A line comment runs to the end of the line.
//! 2. A block comment runs to its terminator.
//! 3. A string runs to its matching quote, and a backslash escapes the next character.
//! 4. A number is a run of digits, with an optional decimal point, a hexadecimal prefix, or a
//!    suffix such as `u32` or `px`. A grammar may also ask for `#ff0000` to be one.
//! 5. A word is a keyword, a builtin or a type if it is in one of the grammar's three lists.
//! 6. A word directly followed by `(` is a function; one starting with a capital letter is a type.
//! 7. Anything else is text.
//!
//! Comments and strings win over everything, because a keyword inside a string is not a keyword.
//!
//! Rules 6 is a **heuristic and is meant to be one**. `Promise.all(` colours `all` as a function and
//! `Promise` as a type without Unluminous understanding a single thing about JavaScript, which is what
//! `task-1649` asks for and is what a colouring pass is for. Real understanding is a language
//! server, and that is not what this is.
//!
//! ## What a language may ask for on top of that
//!
//! Three of the fields on a [`Grammar`] are off unless a manifest names them, and all three arrived
//! with `task-1671`'s CSS plugin, which none of the rules above could read at all.
//!
//! [`Grammar::word_characters`] are characters that count as part of a word wherever they appear,
//! including the first position. **A hyphen is a letter in CSS**: nearly every property name has one
//! in it, every custom property starts with two and every vendor prefix is one, so a pass that broke
//! a word at a hyphen could not recognise a single CSS property by name.
//!
//! [`Grammar::types`] is a third list of words beside the keywords and the builtins. Until it
//! existed, [`Token::Type`] was reachable only by the capital letter heuristic, which is silent in a
//! language that does not capitalise. CSS has three kinds of word worth telling apart — the at-rule,
//! the property and the value — and two lists could not.
//!
//! [`Grammar::hex_colors`] makes `#` and three, four, six or eight hexadecimal digits a number,
//! which is CSS's colour grammar exactly. Without it `#00ff00` was a number and `#ff0000` was a
//! word, because [`number`] wants a digit first, so half the colours in a stylesheet were coloured
//! and the other half were not.
//!
//! [`Grammar::export_keyword`] and the eight fields under it are `task-1680`'s, and they are what
//! lets a language say how its **imports** are written: which words begin one, whether the module
//! is a string or a path of segments, what a written module resolves to on the disk, and which
//! segments are reserved. They follow the same rule — a plugin that names none of them has exactly
//! the behaviour it had before, and nothing in Unluminous holds a list of which languages have imports.
//!
//! [`Grammar::markup`] and [`Grammar::raw_text`] are `task-1694`'s, and they are the first two that
//! change the rules rather than adding to them. With `markup` on, [`scan`] runs a five-state machine
//! instead of the seven rules above, because HTML is **prose with code in the tags** where every
//! other language Unluminous reads is code with prose in the comments. Seventy-six HTML element names are
//! also ordinary English words — `body`, `table`, `form`, `main`, `code`, `time`, `small` — so a
//! pass that coloured them wherever it found them would colour a paragraph of English like a
//! stylesheet, and an apostrophe read as a quote would make every contraction yellow to the end of
//! its line. The same rule holds: a language that names neither key is read by exactly the code that
//! read it before.
//!
//! Deliberately not handled, so nobody has to discover it: nested block comments in Rust — the first
//! terminator ends the comment; interpolation inside a template literal, which is coloured as part
//! of the string; JSX, which is text; and regular expression literals, which cannot be told from
//! division without parsing.

use std::ops::{ControlFlow, Range};

use crate::lang::symbols::SymbolKind;

/// What a stretch of source is. The window turns this into a colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    /// A word the language reserves: `fn`, `const`, `return`.
    Keyword,
    /// A name the language provides: `console`, `Vec`, `Promise`.
    Builtin,
    /// A word directly followed by an opening bracket.
    Function,
    /// A word starting with a capital letter.
    Type,
    String,
    Number,
    Comment,
    /// One of the grammar's operator characters.
    Operator,
    /// Everything else.
    Text,
}

impl Token {
    /// The name a colour scheme uses for this token in a plugin's manifest.
    pub fn name(self) -> &'static str {
        match self {
            Token::Keyword => "keyword",
            Token::Builtin => "builtin",
            Token::Function => "function",
            Token::Type => "type",
            Token::String => "string",
            Token::Number => "number",
            Token::Comment => "comment",
            Token::Operator => "operator",
            Token::Text => "text",
        }
    }

    pub const ALL: [Token; 9] = [
        Token::Keyword,
        Token::Builtin,
        Token::Function,
        Token::Type,
        Token::String,
        Token::Number,
        Token::Comment,
        Token::Operator,
        Token::Text,
    ];
}

/// How to read one language. Every field comes out of a plugin's manifest, so a language is data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grammar {
    /// The plugin this came from, for the status bar.
    pub language: String,
    /// The words the language reserves.
    pub keywords: Vec<String>,
    /// The names the language provides.
    pub builtins: Vec<String>,
    /// The words that name a kind of thing, tried after the keywords and the builtins.
    pub types: Vec<String>,
    /// What starts a comment that runs to the end of the line, such as `//` or `#`.
    pub line_comment: Option<String>,
    /// What opens and closes a comment that spans lines.
    pub block_comment: Option<(String, String)>,
    /// The quote characters that open a string.
    pub strings: Vec<char>,
    /// True when a backslash inside a string escapes the next character.
    pub escapes: bool,
    /// The characters drawn as operators.
    pub operators: Vec<char>,
    /// True when a run of digits is a number.
    pub numbers: bool,
    /// Characters that are part of a word wherever they appear in it, on top of the letters, the
    /// digits, the underscore and the dollar every language already has. `-` and `@` for CSS.
    pub word_characters: Vec<char>,
    /// True when `#` and three, four, six or eight hexadecimal digits is a number, which is what a
    /// colour in a stylesheet is.
    pub hex_colors: bool,
    /// The keywords that make the word after them a definition, and what kind of thing it names.
    ///
    /// `language.definers` in the manifest, off unless a language asks for it, exactly as the three
    /// fields above are. `fn=function, struct=type, let=variable` is most of Rust's. This is what
    /// [`crate::lang::symbols::file_definitions`] reads, and it is why go to definition is a language's
    /// own decision rather than a list of languages written into Unluminous: CSS deliberately names
    /// none, because `--brand-hue: 280` defines a custom property by position rather than by
    /// keyword and a rule that read `:` as a definer would call every property a definition.
    pub definers: Vec<(String, SymbolKind)>,
    /// True when a word directly before `(` whose brackets close on the same line and are followed
    /// by `{` is a likely definition.
    ///
    /// `language.brace_definitions`, and it exists for the definition Rust never hides but
    /// JavaScript and TypeScript do: **a class method has no keyword in front of its name**. What
    /// it finds is marked [`crate::lang::symbols::Confidence::Likely`] all the way to the screen.
    pub brace_definitions: bool,
    /// True when the `{` of a likely definition may open on the line after its parameters close.
    ///
    /// `language.brace_next_line`, added by atrius-index for C, C++, C#, Java and the other languages
    /// whose style guides put a function's opening brace on a line of its own. Off unless a language asks.
    pub brace_next_line: bool,
    /// True when words may stand between a likely definition's closing `)` and its `{`: `const`,
    /// `throws IOException`, `-> Int`, `: String`, `where T : new()`.
    ///
    /// `language.definition_tail`, added by atrius-index. A `;` or an `=` before the `{` still means
    /// a declaration or an expression, so neither is read as a definition. Off unless a language asks.
    pub definition_tail: bool,
    /// The word that makes a definition importable from another file: `export`, or `pub`.
    ///
    /// `language.export_keyword`. A language that names none says that nothing is hidden, and every
    /// definition it has is offered when one of its files is imported. What this decides is
    /// [`crate::lang::symbols::Definition::exported`], and the only thing that reads it is import
    /// completion — a `const` inside a function body is a definition and is not an export, and a
    /// list of a module's exports holding every local it has would be worse than no list.
    pub export_keyword: Option<String>,
    /// How this language writes the module an import names, and that it has imports at all.
    ///
    /// `language.imports`, and the key `task-1680` turns everything else in this group on with.
    /// [`None`] for a language that named nothing, which is every plugin that shipped before it.
    pub imports: Option<ImportStyle>,
    /// The words a statement that imports something begins with: `import, export, require` in
    /// TypeScript, `@import` in CSS, `use` in Rust.
    ///
    /// The anchor the whole reading rests on. Without one of these in front of it, `a::b::c` is
    /// ordinary code and `'./layout'` is an ordinary string, and neither is a question about an
    /// import.
    pub import_keywords: Vec<String>,
    /// What a written module may resolve to on the disk, in the order they are tried, each with its
    /// leading dot: `.ts` before `.js`, because TypeScript's manifest says so.
    pub import_extensions: Vec<String>,
    /// The basenames a folder's own module is written in — `index` for TypeScript, `mod, lib, main`
    /// for Rust — in the order they are tried.
    pub import_index: Vec<String>,
    /// True when the specifier a completion inserts drops the extension: `./layout` and not
    /// `./layout.ts`. False in CSS, where `@import 'theme.css'` is written out.
    pub import_omit_extension: bool,
    /// What joins two segments of a module path. `::` in Rust. Only the [`ImportStyle::Path`]
    /// family has one.
    pub path_separator: Option<String>,
    /// The folder a package's module tree is rooted in — `src` — so that `unluminous_core::completion`
    /// can be `crates/unluminous-core/src/completion.rs` without the `src` being written in the path.
    pub source_roots: Vec<String>,
    /// The segments that are not module names, and what each means: `crate=package, self=module,
    /// super=parent`.
    ///
    /// A language's own words rather than Unluminous's, for the reason `language.definers` exists: a
    /// list of languages inside Unluminous is a list a plugin written later can never join.
    pub path_roots: Vec<(String, PathRoot)>,
    /// True when this language is markup: text with tags in it, rather than tags with text in them.
    ///
    /// `language.markup`, and `task-1694`'s one key. It changes the rules rather than adding to a
    /// list, which is why it is a flag and not a word: with it on, [`scan`] runs the five states of
    /// [`markup`] instead of the seven rules at the top of this file, and **a word means nothing
    /// unless it is inside a tag**. Every other language Unluminous reads is code with prose in the
    /// comments; HTML is prose with code in the tags, and seventy-six of its element names are also
    /// ordinary English words, so a pass that coloured `body`, `table` and `form` wherever it found
    /// them would colour a paragraph of English like a stylesheet.
    ///
    /// Off unless a manifest asks for it, so a language that says nothing about it is read by
    /// exactly the code that read it before.
    pub markup: bool,
    /// The elements whose contents are not markup, and which language each one holds.
    ///
    /// `language.raw_text = script=javascript, style=css, textarea, title`. Only read when
    /// [`Self::markup`] is on, and it is what makes `if (a < b)` inside a `<script>` survive: after
    /// such an element's start tag, nothing opens a tag until its own end tag.
    ///
    /// The two halves of the HTML Standard's own distinction are **derived** from it rather than
    /// declared twice: an entry that names a language is a *raw text* element and an entry that
    /// names none is an *escapable raw text* element, so a character reference is read inside
    /// `<title>` and is not read inside `<script>`, which is exactly how a browser reads them.
    ///
    /// The language is not checked when the manifest is read. It is a name for
    /// `Plugins::for_language` to resolve at the moment of use — the same function a ```` ```rust ````
    /// fence in a Markdown document is resolved by, which already answers with nothing for a
    /// language nothing claims. Checking it would mean one plugin refusing to load because another
    /// was switched off.
    pub raw_text: Vec<(String, Option<String>)>,
}

/// How a language writes the module an import names.
///
/// Two, because there are two shapes and no third: a string resolved against the file system, and a
/// path of segments resolved against a module tree. `tasks/task-1680-import-completion-tdd.md` §4.1
/// says what they share, which is only the popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportStyle {
    /// The module is a string literal: `from './layout'`, `@import 'theme.css'`.
    Quoted,
    /// The module is a path of segments: `use unluminous_core::completion`.
    Path,
}

impl ImportStyle {
    /// The word a manifest writes, and what a message lists when it refuses another one.
    pub fn name(self) -> &'static str {
        match self {
            ImportStyle::Quoted => "quoted",
            ImportStyle::Path => "path",
        }
    }

    /// The style this word names, or nothing when a manifest asked for one that does not exist.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim() {
            "quoted" => Some(ImportStyle::Quoted),
            "path" => Some(ImportStyle::Path),
            _ => None,
        }
    }

    pub const ALL: [ImportStyle; 2] = [ImportStyle::Quoted, ImportStyle::Path];
}

/// What one of a path family's reserved first segments means.
///
/// Three, and they are the three a module tree can be walked from without knowing anything about
/// the language: the package this file is in, the module this file is, and the module above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRoot {
    /// The source root of the package holding this file: Rust's `crate`.
    Package,
    /// The module this file itself is: Rust's `self`.
    Module,
    /// The module above this one, and again for each repetition: Rust's `super`.
    Parent,
}

impl PathRoot {
    /// The word a manifest writes on the right of a `word=meaning` pair.
    pub fn name(self) -> &'static str {
        match self {
            PathRoot::Package => "package",
            PathRoot::Module => "module",
            PathRoot::Parent => "parent",
        }
    }

    /// The meaning this word names, or nothing when a manifest asked for one Unluminous does not have.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim() {
            "package" => Some(PathRoot::Package),
            "module" => Some(PathRoot::Module),
            "parent" => Some(PathRoot::Parent),
            _ => None,
        }
    }

    pub const ALL: [PathRoot; 3] = [PathRoot::Package, PathRoot::Module, PathRoot::Parent];
}

impl Grammar {
    /// Whether a word is one of the reserved ones.
    fn is_keyword(&self, word: &str) -> bool {
        self.keywords.iter().any(|known| known == word)
    }

    fn is_builtin(&self, word: &str) -> bool {
        self.builtins.iter().any(|known| known == word)
    }

    fn is_type(&self, word: &str) -> bool {
        self.types.iter().any(|known| known == word)
    }

    /// The kind of thing this keyword defines, if the language named it as a definer.
    ///
    /// A list rather than a map: a language names a handful of them, and a linear search over five
    /// or ten short strings costs less than hashing one.
    pub fn definer(&self, word: &str) -> Option<SymbolKind> {
        self.definers.iter().find(|(keyword, _)| keyword == word).map(|(_, kind)| *kind)
    }

    /// True when this language has said enough for the symbol mechanism to find a definition in it.
    ///
    /// One function, so the menu, the right click menu and `services::file_kind` cannot come to
    /// different answers about whether `Go to Definition` applies to a file. A language that has
    /// said nothing gets the entries **absent** rather than dimmed, which is Unluminous's rule for a
    /// control that can never apply.
    pub fn defines_symbols(&self) -> bool {
        !self.definers.is_empty() || self.brace_definitions
    }

    /// True when this language has said enough for an import to be read out of it.
    ///
    /// One function, for the same reason [`Self::defines_symbols`] is one: the trigger, the pool
    /// and the resolution all ask it, so none of the three can decide on its own that a language
    /// has imports. A language that named `language.imports` but no keywords has said nothing
    /// usable, because the keyword is the anchor the whole reading rests on.
    pub fn completes_imports(&self) -> bool {
        self.imports.is_some() && !self.import_keywords.is_empty()
    }

    /// The entry [`Self::raw_text`] holds for this element name, if it holds one.
    ///
    /// Compared **without regard to case**, unlike the word lists, and the asymmetry is deliberate:
    /// `<SCRIPT>` read as an ordinary element loses the reading of the rest of the file, where a
    /// keyword matched case-sensitively loses the colour of one word. One is worth a second
    /// comparison on the handful of names in this list and the other is not, on a lookup that runs
    /// over every word of every file in Unluminous.
    fn raw_text_entry(&self, name: &str) -> Option<&(String, Option<String>)> {
        self.raw_text.iter().find(|(element, _)| element.eq_ignore_ascii_case(name))
    }

    /// What this word means as the first segment of a module path, if the language reserved it.
    ///
    /// A list rather than a map, for the reason [`Self::definer`] is one: a language names two or
    /// three of them.
    pub fn path_root(&self, word: &str) -> Option<PathRoot> {
        self.path_roots.iter().find(|(name, _)| name == word).map(|(_, root)| *root)
    }

    /// Whether `character` may be part of a word, given where in the word it is.
    ///
    /// A word starts with a letter, an underscore or a dollar, and carries on with those and the
    /// digits. The dollar is there because `$state` is one word in JavaScript and colouring it as an
    /// operator and then a word would look wrong. Anything in `word_characters` is a word character
    /// in either position, which is how `--brand-hue` and `@font-face` are each one word.
    ///
    /// Public because `symbols` grows a word around a point with it, and a second answer to "what
    /// is a word here" would be a second answer to what `count` matches.
    pub fn is_word_character(&self, character: char, first: bool) -> bool {
        if self.word_characters.contains(&character) {
            return true;
        }
        match first {
            true => character.is_alphabetic() || character == '_' || character == '$',
            false => character.is_alphanumeric() || character == '_' || character == '$',
        }
    }
}

/// Read `text` into coloured spans, in order and with no gaps between them that matter.
///
/// Only the spans that are not plain text are produced. Everything the window is not told about is
/// drawn in the document's own colour, which is what makes the result cheap to apply.
pub fn highlight(text: &str, grammar: &Grammar) -> Vec<(Range<usize>, Token)> {
    let mut spans: Vec<(Range<usize>, Token)> = Vec::new();
    scan(text, grammar, |range, token| {
        if token != Token::Text {
            spans.push((range, token));
        }
    });
    spans
}

/// Read `text` and report every token in it, in order, including the plain words.
///
/// The one pass, and the seam [`crate::lang::symbols`] reads a file's definitions through — because a
/// second reading of the same rules would be a second answer to what a word is, and the two would
/// drift the first time a language asked for something new. [`highlight`] is this with the plain
/// words dropped, which is the only difference between colouring a file and reading it.
///
/// A visitor rather than a returned list, deliberately. Colouring runs whenever the text changes,
/// and `task-1666` says that nothing which runs that often may allocate more than it already does:
/// collecting every plain word into a list only to throw most of it away would have made a
/// coloured file cost more than it did before this module grew a second caller.
pub fn scan(text: &str, grammar: &Grammar, report: impl FnMut(Range<usize>, Token)) {
    scan_with_embedded(text, grammar, &mut Vec::new(), report);
}

/// The same reading, also saying which stretches of the text are written in another language.
///
/// The mirror of [`crate::CodeHighlighter`], and it exists for the same reason: a `<style>` block is
/// CSS, this crate holds no plugin registry and must not learn about one, so it **says where the
/// block is and what language it names** and colours nothing. The window, which has the plugins
/// already, runs the ordinary scan over that stretch with that language's grammar and offsets the
/// ranges — so a `<style>` block in an HTML file is coloured exactly as a `.css` file is, and
/// switching the CSS plugin off withdraws it in the same frame.
///
/// [`scan`] is this with the list thrown away, which is what every caller but the window wants. The
/// list never allocates for a language that names no raw text, which is every language but one.
pub fn scan_with_embedded(
    text: &str,
    grammar: &Grammar,
    embedded: &mut Vec<Embedded>,
    mut report: impl FnMut(Range<usize>, Token),
) {
    // Markup is a different reading rather than an extra rule, so it is a different function. The
    // path below is byte for byte the one every other language has always taken.
    if grammar.markup {
        markup::walk(text, grammar, &mut report, Some(embedded), None);
        return;
    }
    read_from(text, grammar, 0, |range, token| {
        report(range, token);
        ControlFlow::Continue(())
    });
}

/// The same reading, begun at `start` and able to stop.
///
/// **Two things a caller has to be right about, and the second is why this is not `pub` on its own.**
/// The reading below is position-independent -- at each byte it looks only at `&text[at..]` -- so
/// starting at `start` gives exactly the tokens starting at zero would give, *provided the scanner
/// was between tokens there*. Inside a block comment or a multi-line string it was not, and the
/// answer from there is nonsense. And a `markup` grammar is a different reading altogether, with
/// state of its own, so it is refused rather than started part way through.
///
/// [`crate::incremental::Tokens`] is what knows both of those and is where the rule lives; this is
/// the reading, with **one loop**, because a second reading of the same rules would be a second
/// answer to what a word is and the two would drift the first time a language asked for something
/// new. `task-1804` §5.2.
pub fn scan_from(
    text: &str,
    grammar: &Grammar,
    start: usize,
    report: impl FnMut(Range<usize>, Token) -> ControlFlow<()>,
) {
    // A markup grammar has no partial reading, so the caller is given the whole file rather than a
    // wrong answer. `Tokens::update` does not ask for one; this is the belt.
    if grammar.markup {
        let mut report = report;
        markup::walk(
            text,
            grammar,
            &mut |range: Range<usize>, token: Token| {
                let _ = report(range, token);
            },
            None,
            None,
        );
        return;
    }
    read_from(text, grammar, start, report);
}

/// The loop itself. One reading of the rules, shared by every entry point above.
fn read_from(
    text: &str,
    grammar: &Grammar,
    start: usize,
    mut report: impl FnMut(Range<usize>, Token) -> ControlFlow<()>,
) {
    let bytes = text.as_bytes();
    let mut at = start.min(bytes.len());
    macro_rules! say {
        ($range:expr, $token:expr) => {
            if report($range, $token).is_break() {
                return;
            }
        };
    }
    while at < bytes.len() {
        let rest = &text[at..];
        if let Some(length) = comment(rest, grammar) {
            say!(at..at + length, Token::Comment);
            at += length;
            continue;
        }
        if let Some(length) = string(rest, grammar) {
            say!(at..at + length, Token::String);
            at += length;
            continue;
        }
        // Before the word, because `#ff0000` begins with a `#` that a grammar may also be drawing as
        // an operator, and before the number, which is the token it becomes.
        if grammar.hex_colors {
            if let Some(length) = hex_colour(rest) {
                say!(at..at + length, Token::Number);
                at += length;
                continue;
            }
        }
        if grammar.numbers {
            if let Some(length) = number(rest) {
                say!(at..at + length, Token::Number);
                at += length;
                continue;
            }
        }
        if let Some(length) = word_length(rest, grammar) {
            let word = &rest[..length];
            say!(at..at + length, classify(word, &rest[length..], grammar));
            at += length;
            continue;
        }
        let character = rest.chars().next().unwrap_or(' ');
        if grammar.operators.contains(&character) {
            say!(at..at + character.len_utf8(), Token::Operator);
        }
        at += character.len_utf8();
    }
}

/// What a word is, once it has been read.
fn classify(word: &str, after: &str, grammar: &Grammar) -> Token {
    if grammar.is_keyword(word) {
        return Token::Keyword;
    }
    if grammar.is_builtin(word) {
        return Token::Builtin;
    }
    // After the two older lists, so that a language adding this one cannot change what a word it
    // already named is coloured as. It is also why `repeat(` in a stylesheet is a function and not a
    // value: a word in a list never reaches the two heuristics below.
    if grammar.is_type(word) {
        return Token::Type;
    }
    // Directly followed by an opening bracket, with no space between: `listByChat(` is a call, and
    // `if (` is not, because `if` was caught as a keyword above.
    if after.starts_with('(') {
        return Token::Function;
    }
    if word.chars().next().is_some_and(char::is_uppercase) {
        return Token::Type;
    }
    Token::Text
}

/// How long the comment at the start of `rest` is, if it starts with one.
fn comment(rest: &str, grammar: &Grammar) -> Option<usize> {
    if let Some(opener) = &grammar.line_comment {
        if rest.starts_with(opener.as_str()) {
            let end = rest.find('\n').unwrap_or(rest.len());
            return Some(end);
        }
    }
    let (open, close) = grammar.block_comment.as_ref()?;
    if !rest.starts_with(open.as_str()) {
        return None;
    }
    // The first terminator ends it. Rust nests block comments and this does not, which is written
    // down in the module comment and in the plugin's own description rather than left to be found.
    match rest[open.len()..].find(close.as_str()) {
        Some(at) => Some(open.len() + at + close.len()),
        None => Some(rest.len()),
    }
}

/// How long the string at the start of `rest` is, if it starts with one.
fn string(rest: &str, grammar: &Grammar) -> Option<usize> {
    let quote = rest.chars().next()?;
    if !grammar.strings.contains(&quote) {
        return None;
    }
    let mut length = quote.len_utf8();
    let mut escaped = false;
    for character in rest[length..].chars() {
        length += character.len_utf8();
        if escaped {
            escaped = false;
            continue;
        }
        if grammar.escapes && character == '\\' {
            escaped = true;
            continue;
        }
        if character == quote {
            return Some(length);
        }
        // A string that is not closed by the end of its line is almost always a quote in prose or a
        // half-typed line, and colouring the rest of the file as a string would be worse than
        // colouring one line wrongly. A quote that legitimately spans lines — a template literal or
        // a Python docstring — is the exception, and those are the ones a grammar marks as such.
        if character == '\n' && quote != '`' {
            return Some(length - 1);
        }
    }
    Some(length)
}

/// How long the number at the start of `rest` is, if it starts with one.
fn number(rest: &str) -> Option<usize> {
    let mut characters = rest.char_indices();
    let (_, first) = characters.next()?;
    if !first.is_ascii_digit() {
        return None;
    }
    let mut end = first.len_utf8();
    let mut seen_point = false;
    for (index, character) in characters {
        if character.is_ascii_alphanumeric() || character == '_' {
            end = index + character.len_utf8();
            continue;
        }
        // One decimal point, and only when a digit follows it, so `1.max(2)` is not `1.` and then a
        // word.
        if character == '.'
            && !seen_point
            && rest[index + 1..].starts_with(|next: char| next.is_ascii_digit())
        {
            seen_point = true;
            end = index + 1;
            continue;
        }
        break;
    }
    Some(end)
}

/// How long the word at the start of `rest` is, if it starts with one.
///
/// What counts as a word character is [`Grammar::is_word_character`], because a language may add to
/// it: a hyphen is a letter in CSS and is not one anywhere else.
fn word_length(rest: &str, grammar: &Grammar) -> Option<usize> {
    let mut characters = rest.char_indices();
    let (_, first) = characters.next()?;
    if !grammar.is_word_character(first, true) {
        return None;
    }
    let mut end = first.len_utf8();
    for (index, character) in characters {
        if grammar.is_word_character(character, false) {
            end = index + character.len_utf8();
        } else {
            break;
        }
    }
    Some(end)
}

/// How long the `#ff0000` at the start of `rest` is, if it starts with one.
///
/// Three, four, six or eight hexadecimal digits, which is CSS's own colour grammar, and then
/// something that is not a word character. The lengths are what keep an id selector out of it:
/// `#header` is not hexadecimal and `#abcde` is five digits, so both fall through to being a word.
/// `#face` and `#dad` are hex-shaped and are drawn as colours wherever they appear, which needs the
/// position in the rule to tell apart and is written into the plugin's own limitations.
fn hex_colour(rest: &str) -> Option<usize> {
    let after = rest.strip_prefix('#')?;
    let digits = after.chars().take_while(char::is_ascii_hexdigit).count();
    if !matches!(digits, 3 | 4 | 6 | 8) {
        return None;
    }
    // The digits are ASCII, so counting them and indexing by them are the same number.
    if after[digits..]
        .starts_with(|next: char| next.is_alphanumeric() || next == '_' || next == '-')
    {
        return None;
    }
    Some('#'.len_utf8() + digits)
}

/// A stretch of a markup document written in another language, and which one.
///
/// What [`scan_with_embedded`] reports and what the window turns into colours. The language is the
/// word the manifest wrote on the right of a `language.raw_text` entry — `javascript`, `css` — and
/// it is resolved by whoever is drawing, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Embedded {
    /// The body of the element, without its start and end tags.
    pub range: Range<usize>,
    /// The language the manifest said that body is written in.
    pub language: String,
}

/// One tag in a markup document.
///
/// [`tags`] is what reports them, and folding is what reads them: a start tag opens a region and the
/// matching end tag closes it. Everything here is a byte offset into the text it was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// Where the `<` is.
    pub open: usize,
    /// One past the `>`, or the end of the text for a tag that was never closed.
    pub end: usize,
    /// The element's name, so a caller can compare two tags without re-reading them.
    pub name: Range<usize>,
    /// True for `</div>`.
    pub closing: bool,
    /// True for `<br />`.
    pub self_closing: bool,
}

/// Where in a markup document a point is.
///
/// [`markup_position`] is what answers it, and what reads the answer is completion: the language's
/// own words are the right offer inside a tag and the wrong one in prose. A declaration and a
/// processing instruction count as a tag, because what is being asked is "does the language apply
/// here", and inside `<!DOCTYPE …>` it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkupPosition {
    /// Prose, a comment, or the body of an element whose contents are not markup.
    Text,
    /// The first word of a tag: `<ta│`.
    TagName,
    /// A word inside a tag that is not the first: `<div cl│`.
    Attribute,
    /// After an `=` inside a tag, quoted or not.
    Value,
}

/// Every tag in `text`, in the order they are written.
///
/// A second pass over the file rather than a by-product of the one that colours it, and only for a
/// markup language. What it buys is that a `<` inside a comment, inside an attribute value or
/// inside a `<script>` body is not a tag and cannot open a region — which is the whole reason
/// folding asks this instead of walking the bytes for angle brackets itself.
pub fn tags(text: &str, grammar: &Grammar) -> Vec<Tag> {
    if !grammar.markup {
        return Vec::new();
    }
    let mut found = Vec::new();
    markup::walk(text, grammar, &mut |_, _| {}, None, Some(&mut found));
    found
}

/// Where `offset` is in a markup document, or nothing when the language is not markup.
///
/// Read **backwards from the point**, bounded, which is what [`crate::imports::context_at`] and
/// [`crate::lang::symbols::identifier_at`] already do and for the same reason: this runs while somebody is
/// typing, and a few hundred bytes of scanning a keystroke is a cost nobody can measure where a
/// reading of the file is one they can see.
///
/// The bound is what makes it approximate, and the one place it is wrong is a `<` written inside the
/// body of a `<script>` before the caret — where it answers as though a tag were open. It decides
/// which word list is offered, so being wrong there offers the element names inside a script and
/// costs nothing else.
pub fn markup_position(text: &str, offset: usize, grammar: &Grammar) -> Option<MarkupPosition> {
    if !grammar.markup || offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let from = markup::lookback_start(text, offset);
    let window = &text[from..offset];
    // The last thing that could have changed the state. A `>` closed a tag, so what follows it is
    // text; a `<` opened one, so what follows it is inside a tag.
    let opened = window.rfind('<');
    let closed = window.rfind('>');
    let Some(open) = opened.filter(|open| closed.is_none_or(|closed| closed < *open)) else {
        return Some(MarkupPosition::Text);
    };
    let inside = &window[open + '<'.len_utf8()..];
    // `<!-- … ` with no `-->` before the point is a comment, and a comment is not a tag.
    if inside.starts_with("!--") {
        return Some(MarkupPosition::Text);
    }
    let inside = inside.trim_start_matches(['/', '!', '?']);
    Some(markup::position_in_tag(inside, grammar))
}

/// The five states a markup document is read in.
///
/// `task-1694` §3 draws them. The order the rules are tried in is the whole design, exactly as it is
/// for the seven at the top of this file:
///
/// 1. A comment wins over everything, so `<!-- <div> -->` holds no tag.
/// 2. A `<` opens a tag **only** when a letter, `/`, `!` or `?` follows it, which is the HTML
///    Standard's own tag-open state. So `5 < 3` in prose is arithmetic.
/// 3. A start tag whose element the language calls raw text swallows its own body, so nothing inside
///    a `<script>` opens a tag.
/// 4. In text, a character reference is a number and a word is plain text.
/// 5. Everything else in text is one character of prose: no strings, no numbers, no operators.
///
/// Rule 5 is the one that matters most and is the easiest to leave out. An apostrophe in prose is an
/// apostrophe, not the start of a string, and every contraction in every English document depends on
/// it.
mod markup {
    use super::{
        classify_attribute, classify_tag_name, string, word_length, Embedded, Grammar,
        MarkupPosition, Range, Tag, Token,
    };

    /// How far back [`super::markup_position`] reads. Two lines of a long tag and a good deal more
    /// of an ordinary one, and a bound is what keeps a keystroke's cost the same on a large file as
    /// on a small one.
    const LOOKBACK: usize = 4096;

    /// The most characters a character reference can be, past the `&`.
    ///
    /// `&CounterClockwiseContourIntegral;` is the longest one in the HTML Standard's own table at 31
    /// characters and a semicolon, so this is that with room to spare. Without a bound, a stray `&`
    /// in prose would be scanned to the end of the file on the chance that a `;` was coming.
    const ENTITY_LIMIT: usize = 34;

    /// Read `text` as markup, reporting every token, and optionally collecting what was found.
    ///
    /// One walker for the three questions — colour it, find its embedded languages, find its tags —
    /// so none of the three can come to a different conclusion about where a tag is. The reporter is
    /// a `dyn` reference rather than a generic so that three callers do not become three copies of
    /// the machine; it costs one indirect call a token and allocates nothing.
    pub(super) fn walk(
        text: &str,
        grammar: &Grammar,
        report: &mut dyn FnMut(Range<usize>, Token),
        mut embedded: Option<&mut Vec<Embedded>>,
        mut tags: Option<&mut Vec<Tag>>,
    ) {
        let mut at = 0usize;
        while at < text.len() {
            let rest = &text[at..];
            // 1. A comment, which in markup is the only thing that wins over a tag.
            if let Some(length) = super::comment(rest, grammar) {
                report(at..at + length, Token::Comment);
                at += length;
                continue;
            }
            // 2. A tag, or one character of prose.
            if !opens_a_tag(rest) {
                at += in_text(text, at, report, true);
                continue;
            }
            let tag = read_tag(text, at, grammar, report);
            let name = text[tag.name.clone()].to_owned();
            let end = tag.end;
            let raw = match tag.closing || tag.self_closing {
                true => None,
                false => grammar.raw_text_entry(&name).cloned(),
            };
            if let Some(list) = tags.as_deref_mut() {
                list.push(tag);
            }
            at = end;
            // 3. An element whose body is not markup. The body ends where its own end tag begins,
            //    which is then read as an ordinary tag on the next turn of this loop.
            let Some((_, language)) = raw else { continue };
            let body = at..close_tag_at(text, at, &name).unwrap_or(text.len());
            if let (Some(language), Some(found)) = (&language, embedded.as_deref_mut()) {
                if !body.is_empty() {
                    found.push(Embedded { range: body.clone(), language: language.clone() });
                }
            }
            // Its words are still reported, so a name used in an inline script is found by
            // `Find References` and is offered by completion. Its character references are read
            // only when the element named no language, which is the HTML Standard's own difference
            // between a raw text element and an escapable raw text one.
            let entities = language.is_none();
            while at < body.end {
                at += in_text(text, at, report, entities);
            }
            at = body.end;
        }
    }

    /// One step through text: a character reference, a word, or one character of prose.
    ///
    /// Returns how far to move. A word is reported as [`Token::Text`] rather than skipped, because
    /// [`crate::lang::symbols`] reads the same pass for the words a file holds and prose is where a
    /// markup document keeps most of them.
    fn in_text(
        text: &str,
        at: usize,
        report: &mut dyn FnMut(Range<usize>, Token),
        entities: bool,
    ) -> usize {
        let rest = &text[at..];
        if entities {
            if let Some(length) = entity(rest) {
                report(at..at + length, Token::Number);
                return length;
            }
        }
        if let Some(length) = word_length(rest, &Grammar::default()) {
            report(at..at + length, Token::Text);
            return length;
        }
        rest.chars().next().map_or(1, char::len_utf8)
    }

    /// Whether a `<` here opens a tag.
    ///
    /// The HTML Standard's tag-open state: a letter begins a start tag, a solidus an end tag, an
    /// exclamation mark a comment or a doctype, and a question mark a processing instruction.
    /// Anything else and the browser draws a less-than sign, so Unluminous does too.
    fn opens_a_tag(rest: &str) -> bool {
        let mut characters = rest.chars();
        if characters.next() != Some('<') {
            return false;
        }
        matches!(characters.next(), Some(next) if next.is_ascii_alphabetic() || matches!(next, '/' | '!' | '?'))
    }

    /// Read one whole tag, reporting its parts, and say what it was.
    ///
    /// `at` is the `<`. The tag ends at its `>`, at a `<` that opens another tag — which is what
    /// makes half-typed markup recover on the next line rather than swallowing the rest of the file
    /// — or at the end of the text.
    fn read_tag(
        text: &str,
        at: usize,
        grammar: &Grammar,
        report: &mut dyn FnMut(Range<usize>, Token),
    ) -> Tag {
        let mut cursor = at;
        operator(text, &mut cursor, grammar, report);
        let mut closing = false;
        for opener in ['/', '!', '?'] {
            if text[cursor..].starts_with(opener) {
                closing |= opener == '/';
                operator(text, &mut cursor, grammar, report);
                break;
            }
        }
        let name = match word_length(&text[cursor..], grammar) {
            Some(length) => {
                let range = cursor..cursor + length;
                report(range.clone(), classify_tag_name(&text[range.clone()], grammar));
                cursor = range.end;
                range
            }
            None => cursor..cursor,
        };
        let mut value_next = false;
        let mut slash = false;
        while cursor < text.len() {
            let rest = &text[cursor..];
            let character = rest.chars().next().expect("inside the text");
            if character == '>' {
                operator(text, &mut cursor, grammar, report);
                break;
            }
            if opens_a_tag(rest) {
                break;
            }
            if character.is_whitespace() {
                cursor += character.len_utf8();
                continue;
            }
            slash = character == '/';
            if matches!(character, '=' | '/') {
                value_next = character == '=';
                operator(text, &mut cursor, grammar, report);
                continue;
            }
            if grammar.strings.contains(&character) {
                let length = string(rest, grammar).unwrap_or(character.len_utf8());
                report(cursor..cursor + length, Token::String);
                cursor += length;
                value_next = false;
                continue;
            }
            if value_next {
                let length = unquoted_value(rest);
                report(cursor..cursor + length, Token::String);
                cursor += length;
                value_next = false;
                continue;
            }
            if let Some(length) = word_length(rest, grammar) {
                let range = cursor..cursor + length;
                report(range.clone(), classify_attribute(&text[range.clone()], grammar));
                cursor = range.end;
                continue;
            }
            operator(text, &mut cursor, grammar, report);
        }
        Tag { open: at, end: cursor, name, closing, self_closing: slash && !closing }
    }

    /// Draw one character as an operator if the language names it, and step over it either way.
    fn operator(
        text: &str,
        cursor: &mut usize,
        grammar: &Grammar,
        report: &mut dyn FnMut(Range<usize>, Token),
    ) {
        let Some(character) = text[*cursor..].chars().next() else { return };
        let length = character.len_utf8();
        if grammar.operators.contains(&character) {
            report(*cursor..*cursor + length, Token::Operator);
        }
        *cursor += length;
    }

    /// How long the unquoted attribute value at the start of `rest` is.
    ///
    /// To the first whitespace, quote, `=`, `<`, `>` or backtick, which is the HTML Standard's own
    /// rule for one. At least one character, so a value of nothing at all cannot leave the caller
    /// standing still.
    fn unquoted_value(rest: &str) -> usize {
        let mut length = 0;
        for character in rest.chars() {
            if character.is_whitespace() || matches!(character, '"' | '\'' | '=' | '<' | '>' | '`')
            {
                break;
            }
            length += character.len_utf8();
        }
        length.max(rest.chars().next().map_or(0, char::len_utf8))
    }

    /// How long the character reference at the start of `rest` is, if it starts with one.
    ///
    /// The three forms the HTML Standard has: `&amp;`, `&#8212;` and `&#x1F600;`, each ending at a
    /// semicolon. An `&` with no semicolon after it is an ampersand, which is what a browser draws.
    fn entity(rest: &str) -> Option<usize> {
        let after = rest.strip_prefix('&')?;
        let (digits, radix) = match after.strip_prefix('#') {
            Some(number) => match number.strip_prefix(['x', 'X']) {
                Some(hexadecimal) => (hexadecimal, 16),
                None => (number, 10),
            },
            None => (after, 0),
        };
        let body = digits
            .chars()
            .take(ENTITY_LIMIT)
            .take_while(|character| match radix {
                16 => character.is_ascii_hexdigit(),
                10 => character.is_ascii_digit(),
                _ => character.is_ascii_alphanumeric(),
            })
            .count();
        if body == 0 || !digits[body..].starts_with(';') {
            return None;
        }
        // Everything counted is ASCII, so the count and the byte length are the same number.
        Some(rest.len() - digits.len() + body + ';'.len_utf8())
    }

    /// Where this element's own end tag begins, if it has one.
    ///
    /// `</` then the name, ignoring case, then one of the characters that may follow a tag name —
    /// which is the HTML Standard's own rule and is why `</scriptural` does not end a `<script>`.
    fn close_tag_at(text: &str, from: usize, name: &str) -> Option<usize> {
        let mut at = from;
        while let Some(found) = text[at..].find("</") {
            let start = at + found;
            let after = start + "</".len();
            let rest = text.get(after..)?;
            if rest.len() >= name.len() && rest[..name.len()].eq_ignore_ascii_case(name) {
                let next = rest[name.len()..].chars().next();
                if next.is_none_or(|next| next.is_whitespace() || matches!(next, '>' | '/')) {
                    return Some(start);
                }
            }
            at = after;
        }
        None
    }

    /// Where a bounded backwards read starts, on a character boundary.
    pub(super) fn lookback_start(text: &str, offset: usize) -> usize {
        let mut from = offset.saturating_sub(LOOKBACK);
        while from < offset && !text.is_char_boundary(from) {
            from += 1;
        }
        from
    }

    /// Which part of a tag a point inside one is in, given everything written since its `<`.
    pub(super) fn position_in_tag(inside: &str, grammar: &Grammar) -> MarkupPosition {
        // The name runs to the first character that cannot be part of it, so with nothing between
        // the `<` and the point but name characters, the point is still in the name.
        if !inside.chars().any(|character| !grammar.is_word_character(character, false)) {
            return MarkupPosition::TagName;
        }
        // An odd number of quotes means the point is inside the last one.
        for quote in &grammar.strings {
            if inside.matches(*quote).count() % 2 == 1 {
                return MarkupPosition::Value;
            }
        }
        match inside.trim_end_matches(|character| grammar.is_word_character(character, false)) {
            trimmed if trimmed.trim_end().ends_with('=') => MarkupPosition::Value,
            _ => MarkupPosition::Attribute,
        }
    }
}

/// What the first word of a tag is: `<div>`, `<my-widget>`, `<MyPanel>`.
///
/// Its position is certain — there is one tag name in a tag and it is the first word — so this is
/// the one place in a markup document where Unluminous knows what a word is without being told. A name
/// the language names is a keyword; **a name it does not name is a type**, because it is *known* to
/// be a tag name and only unknown whether the language defines it, and custom elements and
/// framework components are a large part of the markup anybody writes now. Drawing them as prose
/// would throw away something the reader is certain of.
fn classify_tag_name(word: &str, grammar: &Grammar) -> Token {
    match grammar.is_keyword(word) {
        true => Token::Keyword,
        false => Token::Type,
    }
}

/// What a word inside a tag that is not its name is: `class`, `aria-label`, `data-track-id`.
///
/// Here the CSS rule does apply and an unknown name is plain text, which is the opposite of
/// [`classify_tag_name`] and deliberately so. There is one tag name in a tag and there may be a
/// dozen attributes, and the ones outside the list — `data-*`, `hx-get`, `v-if`, `@submit` — are
/// names their author chose, exactly as a class selector is. Colouring every one of them would make
/// a tag a wall of colour and would say something untrue about names Unluminous has never heard of.
fn classify_attribute(word: &str, grammar: &Grammar) -> Token {
    if grammar.is_builtin(word) {
        return Token::Builtin;
    }
    match grammar.is_type(word) {
        true => Token::Type,
        false => Token::Text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn javascript() -> Grammar {
        Grammar {
            language: "JavaScript".to_owned(),
            keywords: [
                "const", "let", "function", "return", "class", "async", "await", "import", "from",
            ]
            .iter()
            .map(|word| (*word).to_owned())
            .collect(),
            builtins: ["console", "Promise", "JSON"]
                .iter()
                .map(|word| (*word).to_owned())
                .collect(),
            line_comment: Some("//".to_owned()),
            block_comment: Some(("/*".to_owned(), "*/".to_owned())),
            strings: vec!['"', '\'', '`'],
            escapes: true,
            operators: "+-*/=<>!&|?:;,.".chars().collect(),
            numbers: true,
            ..Grammar::default()
        }
    }

    /// Enough of the CSS grammar to test the three things `task-1671` added.
    fn css() -> Grammar {
        let words = |list: &str| list.split(' ').map(str::to_owned).collect::<Vec<String>>();
        Grammar {
            language: "CSS".to_owned(),
            keywords: words("@media @font-face and not div a hover nth-child before important"),
            builtins: words("background-color display grid-template-columns content left z-index"),
            types: words("flex absolute ellipsis sans-serif ease-in-out"),
            block_comment: Some(("/*".to_owned(), "*/".to_owned())),
            strings: vec!['"', '\''],
            escapes: true,
            operators: "{}();:,>+~*=/!%[]|^&.#".chars().collect(),
            numbers: true,
            word_characters: vec!['-', '@'],
            hex_colors: true,
            ..Grammar::default()
        }
    }

    /// The tokens a stylesheet produces, as text.
    fn css_tokens(text: &str) -> Vec<(String, Token)> {
        highlight(text, &css())
            .into_iter()
            .map(|(range, token)| (text[range].to_owned(), token))
            .collect()
    }

    /// The tokens a piece of source produces, as text, so a test reads like the thing it is about.
    fn tokens(text: &str) -> Vec<(String, Token)> {
        highlight(text, &javascript())
            .into_iter()
            .map(|(range, token)| (text[range].to_owned(), token))
            .collect()
    }

    #[test]
    fn a_keyword_is_a_keyword_and_a_name_is_not() {
        let found = tokens("const total = 1;");
        assert_eq!(found[0], ("const".to_owned(), Token::Keyword));
        assert!(!found.iter().any(|(text, token)| text == "total" && *token == Token::Keyword));
    }

    #[test]
    fn a_keyword_inside_a_string_is_not_a_keyword() {
        // The whole reason strings are read before words.
        let found = tokens("const name = 'return the class';");
        assert!(found.contains(&("'return the class'".to_owned(), Token::String)));
        assert_eq!(
            found.iter().filter(|(_, token)| *token == Token::Keyword).count(),
            1,
            "only the const outside the string: {found:?}"
        );
    }

    #[test]
    fn a_keyword_inside_a_comment_is_not_a_keyword_either() {
        let found = tokens("// return the class\nconst x = 1;");
        assert!(found.contains(&("// return the class".to_owned(), Token::Comment)));
        assert_eq!(found.iter().filter(|(_, token)| *token == Token::Keyword).count(), 1);
    }

    #[test]
    fn a_block_comment_runs_to_its_terminator_and_no_further() {
        let found = tokens("/** Lists messages. */\nconst x = 1;");
        assert_eq!(found[0], ("/** Lists messages. */".to_owned(), Token::Comment));
        assert!(found.contains(&("const".to_owned(), Token::Keyword)));
    }

    #[test]
    fn a_block_comment_that_is_never_closed_takes_the_rest_of_the_file() {
        let found = tokens("/* forgot to close\nconst x = 1;");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, Token::Comment);
    }

    #[test]
    fn a_string_that_is_never_closed_stops_at_the_end_of_its_line() {
        // Colouring the rest of the file as a string because of one stray quote is worse than
        // colouring one line wrongly.
        let found = tokens("const a = 'oops\nconst b = 2;");
        assert!(found.contains(&("'oops".to_owned(), Token::String)), "{found:?}");
        assert_eq!(
            found.iter().filter(|(_, token)| *token == Token::Keyword).count(),
            2,
            "the const on the second line is still a keyword: {found:?}"
        );
    }

    #[test]
    fn a_backtick_string_may_span_lines_because_it_is_meant_to() {
        let found = tokens("const q = `select *\nfrom messages`;");
        assert!(found.iter().any(|(text, token)| *token == Token::String && text.contains('\n')));
    }

    #[test]
    fn a_backslash_escapes_the_quote_that_would_have_ended_the_string() {
        let found = tokens(r#"const a = "she said \"hello\" once";"#);
        let string = found.iter().find(|(_, token)| *token == Token::String).expect("a string");
        assert!(string.0.ends_with("once\""), "the whole string, not the first half: {string:?}");
    }

    #[test]
    fn a_word_directly_before_a_bracket_is_a_function() {
        let found = tokens("listByChat(chatId)");
        assert_eq!(found[0], ("listByChat".to_owned(), Token::Function));
        // With a space between, it is not a call: nothing in the line is coloured at all.
        let found = tokens("listByChat (chatId)");
        assert!(
            !found.iter().any(|(_, token)| *token == Token::Function),
            "a space before the bracket means it is not a call: {found:?}"
        );
    }

    #[test]
    fn a_word_starting_with_a_capital_is_a_type() {
        let found = tokens("const repository: MessageRepository = build();");
        assert!(found.contains(&("MessageRepository".to_owned(), Token::Type)));
        // A builtin wins over the capital letter rule, because the grammar named it.
        let found = tokens("Promise.resolve()");
        assert_eq!(found[0], ("Promise".to_owned(), Token::Builtin));
    }

    #[test]
    fn numbers_are_read_including_their_prefixes_and_suffixes() {
        let found = tokens("const a = 42; const b = 3.14; const c = 0xFF; const d = 10px;");
        let numbers: Vec<String> = found
            .iter()
            .filter(|(_, token)| *token == Token::Number)
            .map(|(text, _)| text.clone())
            .collect();
        assert_eq!(numbers, vec!["42", "3.14", "0xFF", "10px"]);
    }

    #[test]
    fn a_decimal_point_that_is_a_method_call_is_not_part_of_the_number() {
        let found = tokens("1.max(2)");
        assert_eq!(found[0], ("1".to_owned(), Token::Number));
        assert!(found.contains(&("max".to_owned(), Token::Function)));
    }

    #[test]
    fn a_word_may_start_with_a_dollar_or_an_underscore() {
        let found = tokens("const $state = _private;");
        let names: Vec<&String> = found.iter().map(|(text, _)| text).collect();
        assert!(
            !names.iter().any(|text| *text == "$"),
            "the dollar is part of the word: {found:?}"
        );
    }

    #[test]
    fn every_span_is_inside_the_text_and_they_do_not_overlap() {
        let text =
            "/** doc */\nasync function listByChat(chatId) {\n  return `${chatId}`; // done\n}\n";
        let spans = highlight(text, &javascript());
        let mut previous = 0;
        for (range, _) in &spans {
            assert!(range.start >= previous, "spans overlap at {range:?}");
            assert!(range.end <= text.len(), "a span runs past the end of the text");
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
            previous = range.end;
        }
        assert!(!spans.is_empty());
    }

    #[test]
    fn text_with_no_grammar_behind_it_produces_nothing() {
        let empty = Grammar::default();
        assert!(highlight("const x = 1;", &empty).is_empty());
    }

    #[test]
    fn a_file_with_nothing_in_it_produces_nothing() {
        assert!(highlight("", &javascript()).is_empty());
    }

    #[test]
    fn a_hash_comment_works_as_well_as_a_slash_one() {
        let python = Grammar {
            line_comment: Some("#".to_owned()),
            keywords: vec!["def".to_owned()],
            strings: vec!['"', '\''],
            ..Grammar::default()
        };
        let text = "# a note\ndef thing():";
        let spans = highlight(text, &python);
        assert_eq!(&text[spans[0].0.clone()], "# a note");
        assert_eq!(spans[0].1, Token::Comment);
    }

    #[test]
    fn a_hyphen_is_a_letter_when_the_grammar_says_so() {
        // The whole reason `word_characters` exists: without it this is `background`, `-`, `color`,
        // and no CSS property can be recognised by name.
        let found = css_tokens("background-color: red;");
        assert_eq!(found[0], ("background-color".to_owned(), Token::Builtin));
        let found = css_tokens("@media (min-width: 40rem) {}");
        assert_eq!(found[0], ("@media".to_owned(), Token::Keyword), "the at sign is part of it");
        // A custom property is one word and is nobody's business but its author's.
        let found = css_tokens("--brand-hue: 280;");
        assert!(
            !found.iter().any(|(text, _)| text.starts_with("--")),
            "a custom property is plain text, so it produces no span at all: {found:?}"
        );
        // And a language that did not ask for it is unchanged: the hyphen is still an operator.
        let found = tokens("a - b");
        assert!(found.contains(&("-".to_owned(), Token::Operator)));
    }

    #[test]
    fn the_three_lists_are_tried_in_order_and_the_type_list_is_last() {
        let found = css_tokens("div:hover { display: flex; }");
        assert_eq!(found[0], ("div".to_owned(), Token::Keyword));
        assert!(found.contains(&("hover".to_owned(), Token::Keyword)));
        assert!(found.contains(&("display".to_owned(), Token::Builtin)));
        assert!(found.contains(&("flex".to_owned(), Token::Type)));
        // A word in a list never reaches the two heuristics, which is why `content` stays a property
        // where it is used as a value, and why a word in no list still becomes a function.
        assert!(css_tokens("content: \"x\";").contains(&("content".to_owned(), Token::Builtin)));
        assert!(css_tokens("width: calc(1px);").contains(&("calc".to_owned(), Token::Function)));
    }

    #[test]
    fn a_grammar_with_no_type_list_colours_exactly_what_it_did_before() {
        // Every manifest that shipped before `task-1671` leaves the key out, so this is what proves
        // the third list changed nothing for them.
        assert!(javascript().types.is_empty());
        let found = tokens("const repository: MessageRepository = build();");
        assert!(found.contains(&("MessageRepository".to_owned(), Token::Type)));
    }

    #[test]
    fn a_hex_colour_is_a_number_whichever_digit_it_starts_with() {
        // The fault this rule answers: `number` wants a digit first, so `#00ff00` was a number and
        // `#ff0000` was a word, and half a stylesheet's colours were coloured at random.
        for colour in ["#ff0000", "#00ff00", "#FFF", "#abcd", "#11223344"] {
            let found = css_tokens(&format!("color: {colour};"));
            assert!(
                found.contains(&(colour.to_owned(), Token::Number)),
                "{colour} should be one number: {found:?}"
            );
        }
    }

    #[test]
    fn something_that_is_not_a_colour_is_left_alone() {
        // Three, four, six or eight digits and no more, so an id selector stays a word.
        for name in ["#header", "#abcde", "#a", "#main-nav"] {
            let found = css_tokens(name);
            assert!(
                !found.iter().any(|(_, token)| *token == Token::Number),
                "{name} is not a colour: {found:?}"
            );
        }
        // And a language that did not ask for the rule never sees it.
        assert!(!javascript().hex_colors);
        assert!(!tokens("#ff0000").iter().any(|(_, token)| *token == Token::Number));
    }

    #[test]
    fn a_stylesheet_produces_spans_that_are_in_order_and_inside_the_text() {
        let text = "/* the card */\n@media screen and (min-width: 40rem) {\n  .card::before {\n    content: \"\";\n    background-color: #ff79c6;\n    display: flex;\n    width: calc(100% - 2rem);\n  }\n}\n";
        let spans = highlight(text, &css());
        let mut previous = 0;
        for (range, _) in &spans {
            assert!(range.start >= previous, "spans overlap at {range:?}");
            assert!(range.end <= text.len());
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
            previous = range.end;
        }
        let found: Vec<(String, Token)> =
            spans.iter().map(|(range, token)| (text[range.clone()].to_owned(), *token)).collect();
        assert!(found.contains(&("/* the card */".to_owned(), Token::Comment)));
        assert!(found.contains(&("#ff79c6".to_owned(), Token::Number)));
        assert!(found.contains(&("before".to_owned(), Token::Keyword)));
    }

    #[test]
    fn every_token_has_a_name_a_manifest_can_use() {
        for token in Token::ALL {
            assert!(!token.name().is_empty());
        }
        // The names are what a plugin's `theme.` keys are, so two tokens sharing one would make a
        // colour scheme ambiguous.
        let mut names: Vec<&str> = Token::ALL.iter().map(|token| token.name()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count);
    }

    /// The HTML grammar for the markup tests: the five states and the two raw text pairs.
    fn html() -> Grammar {
        let words = |list: &str| list.split(' ').map(str::to_owned).collect::<Vec<String>>();
        Grammar {
            language: "HTML".to_owned(),
            keywords: words("html head title style body p div span a br img input link script"),
            builtins: words("class id src href alt value type title"),
            block_comment: Some(("<!--".to_owned(), "-->".to_owned())),
            strings: vec!['"', '\''],
            escapes: true,
            operators: "<>=/!".chars().collect(),
            numbers: false,
            word_characters: vec!['-'],
            markup: true,
            raw_text: vec![
                ("script".to_owned(), Some("javascript".to_owned())),
                ("style".to_owned(), Some("css".to_owned())),
                ("textarea".to_owned(), None),
                ("title".to_owned(), None),
            ],
            ..Grammar::default()
        }
    }

    /// The tokens a piece of markup produces, as text, so a test reads like the thing it is about.
    fn html_tokens(text: &str) -> Vec<(String, Token)> {
        highlight(text, &html())
            .into_iter()
            .map(|(range, token)| (text[range].to_owned(), token))
            .collect()
    }

    #[test]
    fn a_less_than_in_prose_is_prose_and_not_a_tag() {
        // The HTML Standard's tag-open state: a letter, `/`, `!` or `?` after the `<`, and a
        // digit is none of those, so `5 < 3` is drawn the way a browser draws it.
        let found = html_tokens("if 5 < 3 then 5 > 3");
        assert!(found.is_empty(), "prose is not coloured: {found:?}");
    }

    #[test]
    fn an_apostrophe_in_prose_is_not_a_string() {
        let found = html_tokens("it's a test of the <b>bold</b> word");
        assert!(!found.iter().any(|(_, token)| *token == Token::String), "{found:?}");
        assert_eq!(
            found.iter().filter(|(text, _)| text == "b").count(),
            2,
            "the tag is read on either side of the apostrophe: {found:?}"
        );
    }

    #[test]
    fn a_tag_is_a_tag_and_a_less_than_with_a_space_is_not() {
        let found = html_tokens("<p>one</p> and < p>two");
        assert_eq!(
            found.iter().filter(|(text, _)| text == "p").count(),
            2,
            "the real tag and its end, and nothing from `< p>`: {found:?}"
        );
    }

    #[test]
    fn a_less_than_inside_a_script_body_opens_no_tag() {
        // The body of a raw text element is not markup, which is what makes the comparison survive.
        let text = "<script>\nif (a < b) {\n  do();\n}\n</script>";
        let found = html_tokens(text);
        assert!(!found.iter().any(|(text, _)| text == "if"), "a word of the script: {found:?}");
        assert!(!found.iter().any(|(text, _)| text == "b"), "{found:?}");
        assert_eq!(
            found.iter().filter(|(text, _)| text == "script").count(),
            2,
            "only the two tags name it: {found:?}"
        );
    }

    #[test]
    fn a_character_reference_is_a_number_in_text_and_in_a_title_and_not_in_a_script() {
        // The HTML Standard's own difference: an escapable raw text element still decodes its
        // references, and a raw text element does not.
        let found = html_tokens("<p>Tom &amp; Jerry</p>");
        assert!(found.contains(&("&amp;".to_owned(), Token::Number)), "{found:?}");
        let found = html_tokens("<title>Tom &amp; Jerry</title>");
        assert!(found.contains(&("&amp;".to_owned(), Token::Number)), "{found:?}");
        let found = html_tokens("<script>x = &amp;</script>");
        assert!(!found.iter().any(|(_, token)| *token == Token::Number), "{found:?}");
    }

    #[test]
    fn a_tag_name_the_language_names_is_a_keyword_and_one_it_does_not_is_a_type() {
        let found = html_tokens("<div>and a <my-widget>here</my-widget></div>");
        assert!(found.contains(&("div".to_owned(), Token::Keyword)), "{found:?}");
        assert!(found.contains(&("my-widget".to_owned(), Token::Type)), "{found:?}");
    }

    #[test]
    fn the_same_word_is_a_tag_name_in_one_place_and_an_attribute_in_another() {
        // `title` is an element and an attribute, and the two are drawn two ways in one file.
        let found = html_tokens("<title>page</title>\n<link title=\"x\">");
        assert!(found.contains(&("title".to_owned(), Token::Keyword)), "{found:?}");
        assert!(found.contains(&("title".to_owned(), Token::Builtin)), "{found:?}");
        assert!(found.contains(&("\"x\"".to_owned(), Token::String)), "{found:?}");
    }

    #[test]
    fn an_unquoted_attribute_value_is_a_string_to_its_first_space() {
        let found = html_tokens("<input value=abc type=text>");
        assert!(found.contains(&("abc".to_owned(), Token::String)), "{found:?}");
        assert!(found.contains(&("text".to_owned(), Token::String)), "{found:?}");
    }

    #[test]
    fn the_body_of_a_style_block_is_reported_as_embedded_css() {
        let text = "<style>\n.color { color: red; }\n</style>";
        let mut embedded = Vec::new();
        scan_with_embedded(text, &html(), &mut embedded, |_, _| {});
        assert_eq!(embedded.len(), 1, "{embedded:?}");
        assert_eq!(embedded[0].language, "css");
        assert_eq!(&text[embedded[0].range.clone()], "\n.color { color: red; }\n");
    }

    #[test]
    fn the_body_of_a_script_block_is_reported_as_embedded_javascript() {
        let text = "<script>var x = 1;</script>";
        let mut embedded = Vec::new();
        scan_with_embedded(text, &html(), &mut embedded, |_, _| {});
        assert_eq!(embedded.len(), 1, "{embedded:?}");
        assert_eq!(embedded[0].language, "javascript");
        assert_eq!(&text[embedded[0].range.clone()], "var x = 1;");
    }

    #[test]
    fn a_point_is_answered_by_the_state_it_is_in() {
        let grammar = html();
        assert_eq!(markup_position("hello <div", 10, &grammar), Some(MarkupPosition::TagName));
        assert_eq!(markup_position("<div class", 10, &grammar), Some(MarkupPosition::Attribute));
        assert_eq!(markup_position("<div class=\"x", 13, &grammar), Some(MarkupPosition::Value));
        assert_eq!(markup_position("<div class=x", 12, &grammar), Some(MarkupPosition::Value));
        assert_eq!(markup_position("hello world", 5, &grammar), Some(MarkupPosition::Text));
        assert_eq!(markup_position("<!-- a note", 5, &grammar), Some(MarkupPosition::Text));
        assert_eq!(
            markup_position("const x = 1;", 5, &javascript()),
            None,
            "a language that is not markup has no position"
        );
    }
}
