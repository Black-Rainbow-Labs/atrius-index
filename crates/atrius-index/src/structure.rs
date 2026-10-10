//! The shape of a definition: what it belongs to, what it takes and returns, what type a field or a
//! variable has, who may use it, and the first line of its documentation (Unluminous `task-2231` §5.2).
//!
//! There is still no parser. Everything here is read from what `crate::lang` already produces: the
//! tokeniser's words, comments and strings, the definitions `crate::lang::symbols` finds after a definer
//! keyword, and the blocks `crate::lang::folding` finds by matching brackets and indentation. What makes
//! it work for a language is a handful of manifest keys ([`crate::lang::syntax::StructureKeys`]), so a
//! language is still data and a language that names none of them gets its definitions exactly as before,
//! with no container and no types.
//!
//! The rules, each one heuristic and each one the same for every language:
//!
//! - **Container.** A definition directly inside a block whose head line starts with a word the language
//!   names in `language.containers` belongs to that block's name. `impl Trait for Type` names `Type`.
//!   A function inside any container that is not a module is a method.
//! - **Fields and variants.** Each line directly inside a block whose head starts with a word named in
//!   `language.records` and that reads `name: Type` is a field; each directly inside one named in
//!   `language.enums` is a variant.
//! - **Parameters and return.** The text between the first bracket after a callable's name and its
//!   matching close, split at the commas outside brackets; and the text after `language.returns`.
//! - **Variables.** `let x: T` takes its type from the annotation; `let x = T::new()`, `new T()`, `T {`
//!   and `T(` take the constructor's name. Nothing else is inferred.
//! - **Imports.** Each statement an import keyword starts, taken apart into the paths it brings in.

use std::ops::Range;

use crate::lang::folding::{Kind as RegionKind, Region};
use crate::lang::symbols::{Confidence, FileSymbols, SymbolKind};
use crate::lang::syntax::{Grammar, ImportStyle};

/// What a definition is, finer than the five kinds the definer keywords name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Function,
    Method,
    Type,
    Struct,
    Enum,
    Variant,
    Trait,
    Interface,
    Class,
    Field,
    Constant,
    Variable,
    Module,
    Parameter,
    TypeAlias,
    Macro,
}

impl Kind {
    /// Every kind, for a message that lists them.
    pub const ALL: [Kind; 16] = [
        Kind::Function,
        Kind::Method,
        Kind::Type,
        Kind::Struct,
        Kind::Enum,
        Kind::Variant,
        Kind::Trait,
        Kind::Interface,
        Kind::Class,
        Kind::Field,
        Kind::Constant,
        Kind::Variable,
        Kind::Module,
        Kind::Parameter,
        Kind::TypeAlias,
        Kind::Macro,
    ];

    /// The word a manifest, the command line and the store use.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Function => "function",
            Kind::Method => "method",
            Kind::Type => "type",
            Kind::Struct => "struct",
            Kind::Enum => "enum",
            Kind::Variant => "variant",
            Kind::Trait => "trait",
            Kind::Interface => "interface",
            Kind::Class => "class",
            Kind::Field => "field",
            Kind::Constant => "constant",
            Kind::Variable => "variable",
            Kind::Module => "module",
            Kind::Parameter => "parameter",
            Kind::TypeAlias => "alias",
            Kind::Macro => "macro",
        }
    }

    /// The kind a word names, if it names one.
    ///
    /// @param word - the word
    pub fn parse(word: &str) -> Option<Kind> {
        Kind::ALL.iter().copied().find(|k| k.name() == word)
    }

    /// The five kinds the definer keywords name, which `search def`'s ranking and the outline's
    /// listing were written against, and which the older `kind` field of a row still carries.
    pub fn family(self) -> &'static str {
        match self {
            Kind::Function | Kind::Method | Kind::Macro => "function",
            Kind::Type
            | Kind::Struct
            | Kind::Enum
            | Kind::Trait
            | Kind::Interface
            | Kind::Class
            | Kind::TypeAlias => "type",
            Kind::Constant => "constant",
            Kind::Variable | Kind::Parameter => "variable",
            Kind::Module => "module",
            Kind::Field => "field",
            Kind::Variant => "variant",
        }
    }

    /// True for a kind a value can be made of or a member looked up on: a type, a struct, a class.
    pub fn is_type(self) -> bool {
        self.family() == "type"
    }

    /// True for a kind that is called with brackets.
    pub fn is_callable(self) -> bool {
        matches!(self, Kind::Function | Kind::Method | Kind::Macro)
    }

    /// The kind a definer keyword's five way kind maps to when the manifest names nothing finer.
    ///
    /// @param kind - the definer's kind
    fn from_symbol(kind: SymbolKind) -> Kind {
        match kind.name() {
            "function" => Kind::Function,
            "type" => Kind::Type,
            "constant" => Kind::Constant,
            "module" => Kind::Module,
            _ => Kind::Variable,
        }
    }
}

/// Who may use a definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Visibility {
    /// Any file: `pub`, `export`, or a member of a language whose members are public by default.
    Public,
    /// Its own package or crate: `pub(crate)`, or TypeScript's `protected`, which is the nearest.
    Crate,
    /// Its own file or type.
    Private,
}

impl Visibility {
    /// The word the store and the command line use.
    pub fn name(self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Crate => "crate",
            Visibility::Private => "private",
        }
    }

    /// The visibility a word names.
    ///
    /// @param word - the word
    pub fn parse(word: &str) -> Option<Visibility> {
        match word {
            "public" => Some(Visibility::Public),
            "crate" => Some(Visibility::Crate),
            "private" => Some(Visibility::Private),
            _ => None,
        }
    }
}

/// One parameter of a callable: its name and, when it is written, its type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parameter {
    pub name: String,
    pub type_text: Option<String>,
}

/// What the structural reading found about one definition, beyond its name and where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shape {
    pub kind: Kind,
    /// The type or module whose block holds it: `Layout` for `impl Layout { fn x() }`.
    pub container: Option<String>,
    pub parameters: Vec<Parameter>,
    pub returns: Option<String>,
    /// A field's or a variable's type, as written or as its constructor names it.
    pub type_text: Option<String>,
    pub visibility: Visibility,
    /// The first line of the comment above it, or of a Python docstring under it.
    pub doc: Option<String>,
}

/// A definition the reading found that the definer keywords cannot: a field, a variant, or a method an
/// interface declares without a body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    /// The name's own bytes.
    pub range: Range<usize>,
    /// The line it is on, from zero.
    pub line: usize,
    pub shape: Shape,
}

/// One import: the path it brings in, the name it is given, and whether it brings in everything.
///
/// A path import keeps its segments: `use crate::layout::{Layout as L}` is `["crate", "layout",
/// "Layout"]` with alias `L`. A quoted import keeps its specifier first: `import { a } from './m'` is
/// `["./m", "a"]`, a default import `["./m", "default"]` with the local name as its alias, and
/// `import * as m from './m'` is `["./m"]` with alias `m` and `glob` set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    pub path: Vec<String>,
    pub alias: Option<String>,
    pub glob: bool,
    /// The whole statement's bytes.
    pub range: Range<usize>,
}

impl Import {
    /// The name this import makes available in the file: its alias, or the last segment of its path.
    pub fn local_name(&self) -> Option<&str> {
        self.alias.as_deref().or_else(|| self.path.last().map(String::as_str))
    }
}

/// One block that holds members: where it is, the word that made it one, and its name.
#[derive(Clone, Debug)]
struct Container {
    head: usize,
    last: usize,
    keyword: String,
    name: String,
}

/// What one reading of a file carries from step to step.
struct Reader<'a> {
    text: &'a str,
    grammar: &'a Grammar,
    lines: Vec<&'a str>,
    line_starts: Vec<usize>,
    regions: Vec<&'a Region>,
    /// For each line, the innermost block it sits inside (below the block's head), as an index into
    /// `regions`: worked out once so asking it of every definition and every line is not a walk of
    /// every block each time.
    innermost: Vec<Option<usize>>,
    containers: Vec<Container>,
    /// For each block, the container it is, as an index into `containers`.
    container_at: std::collections::HashMap<usize, usize>,
}

/// The structure of one file: a shape for each definition `crate::lang::symbols` finds (in its order),
/// the members it finds itself, and the imports.
pub struct Read {
    pub shapes: Vec<Shape>,
    pub members: Vec<Member>,
    pub imports: Vec<Import>,
}

/// Reads one file's structure.
///
/// @param text - the file's text
/// @param grammar - its language
/// @param symbols - the file's definitions, read already
/// @param regions - the file's folding regions, read already
pub fn read(text: &str, grammar: &Grammar, symbols: &FileSymbols, regions: &[Region]) -> Read {
    let lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let line_starts: Vec<usize> =
        std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let regions: Vec<&Region> =
        regions.iter().filter(|r| matches!(r.kind, RegionKind::Block | RegionKind::Indent)).collect();
    let innermost = innermost_by_line(&regions, lines.len());
    let mut reader = Reader {
        text,
        grammar,
        lines,
        line_starts,
        regions,
        innermost,
        containers: Vec::new(),
        container_at: std::collections::HashMap::new(),
    };
    reader.containers = reader.find_containers();
    reader.container_at = reader
        .regions
        .iter()
        .enumerate()
        .filter_map(|(at, r)| {
            let container = reader.containers.iter().position(|c| c.head == r.head && c.last == r.last())?;
            Some((at, container))
        })
        .collect();
    let shapes = symbols.definitions().iter().map(|d| reader.shape_of(d)).collect();
    let mut members = reader.fields_and_variants(symbols);
    members.sort_by_key(|m| m.range.start);
    Read { shapes, members, imports: imports(text, grammar, symbols) }
}

impl Reader<'_> {
    /// The line a byte is on, from zero.
    fn line_of(&self, at: usize) -> usize {
        self.line_starts.partition_point(|&s| s <= at).saturating_sub(1)
    }

    /// Every block whose head line starts with a container word, with its name.
    fn find_containers(&self) -> Vec<Container> {
        let keys = &self.grammar.structure;
        if keys.containers.is_empty() {
            return Vec::new();
        }
        let mut found = Vec::new();
        for region in &self.regions {
            let Some(head) = self.lines.get(region.head) else { continue };
            if let Some((keyword, name)) = container_head(head, self.grammar) {
                found.push(Container { head: region.head, last: region.last(), keyword, name });
            }
        }
        found
    }

    /// The container a line sits directly inside: the innermost block around it, when that block is a
    /// container.
    fn container_of(&self, line: usize) -> Option<&Container> {
        let region = (*self.innermost.get(line)?)?;
        self.containers.get(*self.container_at.get(&region)?)
    }

    /// The shape of one definition `crate::lang::symbols` found.
    fn shape_of(&self, d: &crate::lang::symbols::Definition) -> Shape {
        let line = self.line_of(d.name_range.start);
        let head = self.lines.get(line).copied().unwrap_or_default();
        let column = d.name_range.start - self.line_starts[line];
        let before = &head[..column.min(head.len())];
        let definer = last_definer(before, self.grammar);
        let mut kind = match definer.as_deref().and_then(|w| self.finer_kind(w)) {
            Some(kind) => kind,
            None => Kind::from_symbol(d.kind),
        };
        let mut container = self.container_of(line).cloned();
        if self.grammar.structure.receivers && kind == Kind::Function {
            if let Some(receiver) = receiver_type(before) {
                container = Some(Container { head: line, last: line, keyword: String::new(), name: receiver });
            }
        }
        let module_like = container.as_ref().is_some_and(|c| self.is_module_word(&c.keyword));
        if kind == Kind::Function && container.is_some() && !module_like {
            kind = Kind::Method;
        }
        if d.confidence == Confidence::Likely && kind == Kind::Function && container.is_some() {
            kind = Kind::Method;
        }
        let stop = char_floor(self.text, (d.name_range.end + 600).min(self.text.len()));
        let after = &self.text[d.name_range.end..stop];
        // `const draw = (ui: Ui): Response => …` is a function, whatever its definer says.
        let as_value = match kind {
            Kind::Variable | Kind::Constant => function_value(after),
            _ => None,
        };
        if as_value.is_some() {
            kind = match container.is_some() && !module_like {
                true => Kind::Method,
                false => Kind::Function,
            };
        }
        let (parameters, returns) = match (as_value, kind.is_callable()) {
            (Some(value), _) => self.parameters_and_returns(value),
            (None, true) => self.parameters_and_returns(after),
            (None, false) => (Vec::new(), None),
        };
        let type_text = match kind {
            Kind::Variable | Kind::Constant | Kind::Field => self.variable_type(after),
            Kind::TypeAlias => alias_target(after),
            _ => None,
        };
        let in_container = container.is_some() && !module_like;
        Shape {
            kind,
            container: container.map(|c| c.name),
            parameters,
            returns,
            type_text,
            visibility: self.visibility(before, d.exported, in_container),
            doc: self.doc_above(line),
        }
    }

    /// The kind `language.kinds` gives a definer keyword.
    fn finer_kind(&self, definer: &str) -> Option<Kind> {
        self.grammar
            .structure
            .kinds
            .iter()
            .find(|(word, _)| word == definer)
            .and_then(|(_, kind)| Kind::parse(kind))
    }

    /// True when a container word is a module rather than a type: a definer whose kind is `module`.
    fn is_module_word(&self, word: &str) -> bool {
        self.grammar.definer(word).is_some_and(|k| k.name() == "module")
    }

    /// Who may use a definition, from the words before its definer keyword.
    ///
    /// @param before - the line up to the name
    /// @param exported - whether the export keyword marked it
    /// @param member - whether it sits inside a type
    fn visibility(&self, before: &str, exported: bool, member: bool) -> Visibility {
        let keys = &self.grammar.structure.visibility;
        let mut words: Vec<&(String, String)> = keys.iter().collect();
        words.sort_by_key(|(w, _)| std::cmp::Reverse(w.len()));
        for (word, level) in words {
            if contains_word(before, word) {
                return Visibility::parse(level).unwrap_or(Visibility::Private);
            }
        }
        // A language that marks public things (`pub`) makes everything else private; one that marks only
        // private things (`private`, `protected`) leaves a member public by default.
        let marks_public = keys.iter().any(|(_, level)| level == "public");
        match (member, marks_public) {
            (true, false) => Visibility::Public,
            _ if exported => Visibility::Public,
            _ if self.grammar.export_keyword.is_none() && !marks_public => Visibility::Public,
            _ => Visibility::Private,
        }
    }

    /// The first line of the comment above a definition, past any attributes and decorators; or the
    /// first line of a docstring under it.
    fn doc_above(&self, line: usize) -> Option<String> {
        let mut at = line;
        let mut found: Option<String> = None;
        while at > 0 {
            let above = self.lines[at - 1].trim();
            if above.starts_with("#[") || above.starts_with('@') {
                at -= 1;
                continue;
            }
            match comment_text(above, self.grammar) {
                Some(text) => {
                    if !text.is_empty() {
                        found = Some(text);
                    }
                    at -= 1;
                }
                None => break,
            }
        }
        found.or_else(|| self.docstring_below(line))
    }

    /// The first line of a Python style docstring on the line under a definition.
    fn docstring_below(&self, line: usize) -> Option<String> {
        let next = self.lines.get(line + 1)?.trim();
        let quote = ["\"\"\"", "'''"].into_iter().find(|q| next.starts_with(q))?;
        let text = next.trim_start_matches(quote).trim_end_matches(quote).trim();
        (!text.is_empty()).then(|| text.chars().take(200).collect())
    }

    /// A callable's parameters and return type, from the text after its name.
    fn parameters_and_returns(&self, after: &str) -> (Vec<Parameter>, Option<String>) {
        let Some(open) = first_parameter_bracket(after) else { return (Vec::new(), None) };
        let Some(close) = matching_close(after, open) else { return (Vec::new(), None) };
        let inside = &after[open + 1..close];
        let parameters = split_top_level(inside, ',')
            .into_iter()
            .filter_map(|part| self.parameter(part))
            .collect();
        (parameters, self.return_type(&after[close + 1..]))
    }

    /// One parameter from its text: a name and, where the annotation key says how, a type.
    fn parameter(&self, part: &str) -> Option<Parameter> {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        let part = strip_default(part);
        let annotation = self.grammar.structure.annotation.as_deref();
        let (name, type_text) = match annotation {
            Some("space") => match part.rsplit_once(char::is_whitespace) {
                Some((name, ty)) => (name.trim(), Some(ty.trim().to_owned())),
                None => (part, None),
            },
            Some(mark) => match find_top_level(part, mark) {
                Some(at) => (part[..at].trim(), Some(part[at + mark.len()..].trim().to_owned())),
                None => (part, None),
            },
            None => (part, None),
        };
        // A destructured parameter, `{ kind, projectId }: Props`, is named by its pattern as written.
        if name.starts_with(['{', '[']) {
            let pattern = name.split_whitespace().collect::<Vec<_>>().join(" ");
            return Some(Parameter {
                name: pattern,
                type_text: type_text.filter(|t| !t.is_empty()).map(|t| t.chars().take(200).collect()),
            });
        }
        let name = name
            .trim_start_matches(['&', '*'])
            .trim_start_matches("mut ")
            .trim_start_matches("...")
            .trim_end_matches(['?', '!']);
        let name = name.rsplit(char::is_whitespace).next().unwrap_or(name).trim_start_matches(['&', '*']);
        if name.is_empty() {
            return None;
        }
        Some(Parameter {
            name: name.to_owned(),
            type_text: type_text.filter(|t| !t.is_empty()).map(|t| t.chars().take(200).collect()),
        })
    }

    /// The return type after a parameter list's close, up to the body or the end of the head.
    fn return_type(&self, rest: &str) -> Option<String> {
        let mark = self.grammar.structure.returns.as_deref()?;
        let rest = rest.trim_start();
        let rest = match mark {
            "space" => rest,
            mark => rest.strip_prefix(mark)?,
        };
        let end = head_end(rest);
        let mut text = rest[..end].trim();
        // An arrow function's body follows `=>`, and a Python block opens with `:` at the end of the head.
        if let Some(arrow) = text.find("=>") {
            text = text[..arrow].trim();
        }
        let text = text.trim_end_matches(':').trim();
        (!text.is_empty()).then(|| text.chars().take(200).collect())
    }

    /// A field's, constant's or variable's type: the annotation when there is one, else the name of a
    /// constructor it is made with.
    fn variable_type(&self, after: &str) -> Option<String> {
        let line = after.split('\n').next().unwrap_or_default();
        let line = line.trim_start().trim_start_matches(['?', '!']).trim_start();
        let annotation = self.grammar.structure.annotation.as_deref();
        if let Some(mark) = annotation.filter(|m| *m != "space") {
            if let Some(rest) = line.strip_prefix(mark).filter(|r| !r.starts_with(':')) {
                let text = rest[..type_end(rest)].trim();
                return (!text.is_empty()).then(|| text.chars().take(200).collect());
            }
        }
        if annotation == Some("space") {
            let word = line.split_whitespace().next().unwrap_or_default();
            if !word.is_empty() && !word.starts_with(['=', ':']) {
                return Some(word.to_owned());
            }
        }
        let value = line.strip_prefix(":=").or_else(|| line.strip_prefix('='))?;
        constructor_name(value.trim_start(), self.grammar)
    }

    /// The fields of every record block and the variants of every enum block, and the methods an
    /// interface declares without a body.
    fn fields_and_variants(&self, symbols: &FileSymbols) -> Vec<Member> {
        let keys = &self.grammar.structure;
        let known: std::collections::HashSet<usize> =
            symbols.definitions().iter().map(|d| self.line_of(d.name_range.start)).collect();
        let mut out = Vec::new();
        for container in &self.containers {
            let record = keys.records.contains(&container.keyword);
            let variants = keys.enums.contains(&container.keyword);
            if !record && !variants {
                continue;
            }
            for line in container.head + 1..=container.last {
                if known.contains(&line) || self.container_of(line).map(|c| c.head) != Some(container.head) {
                    continue;
                }
                let member = match variants {
                    true => self.variant_on(line, container),
                    false => self.field_on(line, container),
                };
                out.extend(member);
            }
        }
        out
    }

    /// The first word of a line inside a block and where it starts, past modifiers and visibility words.
    fn leading_name(&self, line: usize) -> Option<(usize, usize)> {
        let text = self.lines.get(line)?;
        let mut at = 0usize;
        loop {
            let rest = &text[at..];
            let skipped = rest.len() - rest.trim_start().len();
            at += skipped;
            let (start, end) = word_at(text, at, self.grammar)?;
            let word = &text[start..end];
            let after = text[end..].trim_start();
            let modifier = self.grammar.keywords.iter().any(|k| k == word)
                || self.grammar.structure.visibility.iter().any(|(w, _)| w == word)
                || (word == "pub" && after.starts_with('('));
            let names_something = after.starts_with(':') || after.starts_with('=') || after.starts_with('?');
            if modifier && !names_something {
                at = end;
                if text[at..].starts_with('(') {
                    at += text[at..].find(')').map_or(0, |close| close + 1);
                }
                continue;
            }
            return Some((start, end));
        }
    }

    /// A field on a line of a record block: `name: Type`, `name = value`, or Go's `Name Type`; or a
    /// method an interface declares, `name(a: A): B`.
    fn field_on(&self, line: usize, container: &Container) -> Option<Member> {
        let text = self.lines[line];
        let trimmed = text.trim_start();
        if trimmed.is_empty() || trimmed.starts_with(['}', ')', ']', '#', '@', '/', '*']) {
            return None;
        }
        let (start, end) = self.leading_name(line)?;
        let after = &text[end..];
        let rest = after.trim_start().trim_start_matches(['?', '!']).trim_start();
        let annotation = self.grammar.structure.annotation.as_deref();
        let base = self.line_starts[line];
        let range = base + start..base + end;
        let kind;
        let mut shape_parameters = Vec::new();
        let mut shape_returns = None;
        if rest.starts_with('(') || rest.starts_with('<') {
            kind = Kind::Method;
            let (p, r) = self.parameters_and_returns(rest);
            shape_parameters = p;
            shape_returns = r;
        } else if (rest.starts_with(':') && !rest.starts_with("::"))
            || rest.starts_with('=')
            || (annotation == Some("space")
                && rest.chars().next().is_some_and(|c| c.is_alphabetic() || c == '*' || c == '['))
        {
            kind = Kind::Field;
        } else {
            return None;
        }
        let type_text = match kind {
            Kind::Field => self.variable_type(after),
            _ => None,
        };
        Some(Member {
            name: text[start..end].to_owned(),
            range,
            line,
            shape: Shape {
                kind,
                container: Some(container.name.clone()),
                parameters: shape_parameters,
                returns: shape_returns,
                type_text,
                visibility: self.visibility(&text[..start], false, true),
                doc: self.doc_above(line),
            },
        })
    }

    /// A variant on a line of an enum block: a name followed by a comma, a bracket, `=` or nothing.
    fn variant_on(&self, line: usize, container: &Container) -> Option<Member> {
        let text = self.lines[line];
        let trimmed = text.trim_start();
        if trimmed.is_empty() || trimmed.starts_with(['}', ')', ']', '#', '@', '/', '*']) {
            return None;
        }
        let (start, end) = self.leading_name(line)?;
        let rest = text[end..].trim_start();
        if !(rest.is_empty() || rest.starts_with([',', '(', '{', '='])) {
            return None;
        }
        let base = self.line_starts[line];
        Some(Member {
            name: text[start..end].to_owned(),
            range: base + start..base + end,
            line,
            shape: Shape {
                kind: Kind::Variant,
                container: Some(container.name.clone()),
                parameters: Vec::new(),
                returns: None,
                type_text: None,
                visibility: Visibility::Public,
                doc: self.doc_above(line),
            },
        })
    }
}

/// For each line, the innermost block it sits inside below the block's head. The regions come sorted
/// by head line and widest first, so a block met later is inside the ones met before it, and writing
/// each block's lines in that order leaves every line naming its innermost block.
///
/// @param regions - the blocks, sorted by head and widest first
/// @param lines - how many lines the file has
fn innermost_by_line(regions: &[&Region], lines: usize) -> Vec<Option<usize>> {
    let mut out = vec![None; lines + 1];
    for (at, region) in regions.iter().enumerate() {
        let last = region.last().min(lines);
        if region.head < last {
            out[region.head + 1..=last].fill(Some(at));
        }
    }
    out
}

/// The container word and name of a block's head line, if it starts with a container word.
///
/// Leading keywords that are not container words (`pub`, `export`, `abstract`, `unsafe`) and
/// `pub(crate)` are stepped over. `impl<T> Trait for Type<T>` names `Type`; anything else names the
/// first path after the word, cut to its last segment.
///
/// @param head - the head line
/// @param grammar - the language
fn container_head(head: &str, grammar: &Grammar) -> Option<(String, String)> {
    let keys = &grammar.structure;
    let mut at = 0usize;
    loop {
        let (start, end) = word_at(head, at, grammar).or_else(|| {
            let skip = head[at..].len() - head[at..].trim_start().len();
            (skip > 0).then(|| word_at(head, at + skip, grammar)).flatten()
        })?;
        if !head[at..start].trim().is_empty() && !head[at..start].trim().starts_with('(') {
            return None;
        }
        let word = &head[start..end];
        if keys.containers.iter().any(|c| c == word) {
            let rest = &head[end..];
            return container_name(rest, grammar).map(|name| (word.to_owned(), name));
        }
        if !grammar.keywords.iter().any(|k| k == word) {
            return None;
        }
        at = end;
        if head[at..].starts_with('(') {
            at += head[at..].find(')').map_or(0, |close| close + 1);
        }
    }
}

/// The name a container's head gives after its word: the type after `for` when there is one, else the
/// first path, cut to its last segment.
///
/// @param rest - the head after the container word
/// @param grammar - the language
fn container_name(rest: &str, grammar: &Grammar) -> Option<String> {
    let rest = skip_generics(rest.trim_start());
    let target = match find_top_level_word(rest, "for") {
        Some(at) => &rest[at + 3..],
        None => rest,
    };
    let target = target.trim_start().trim_start_matches(['&', '*']);
    let mut name = None;
    let mut at = 0usize;
    while let Some((start, end)) = word_at(target, at, grammar) {
        name = Some(target[start..end].to_owned());
        let after = &target[end..];
        if let Some(stripped) = after.strip_prefix("::").or_else(|| after.strip_prefix('.')) {
            at = target.len() - stripped.len();
            continue;
        }
        break;
    }
    name.filter(|n| !grammar.keywords.iter().any(|k| k == n))
}

/// Skips a generic parameter list at the start of a text: `<T: Copy>`.
///
/// @param text - the text
fn skip_generics(text: &str) -> &str {
    if !text.starts_with('<') {
        return text;
    }
    match matching_close(text, 0) {
        Some(close) => &text[close + 1..],
        None => text,
    }
}

/// The identifier that starts at or after a byte, skipping nothing but requiring it to start exactly
/// there; or None.
///
/// @param text - the text
/// @param at - where to look
/// @param grammar - the language, which says what a word character is
fn word_at(text: &str, at: usize, grammar: &Grammar) -> Option<(usize, usize)> {
    let rest = text.get(at..)?;
    let first = rest.chars().next()?;
    if !grammar.is_word_character(first, true) {
        return None;
    }
    let len = rest
        .char_indices()
        .find(|(_, c)| !grammar.is_word_character(*c, false))
        .map_or(rest.len(), |(i, _)| i);
    Some((at, at + len))
}

/// The last definer keyword written before a definition's name on its line.
///
/// @param before - the line up to the name
/// @param grammar - the language
fn last_definer(before: &str, grammar: &Grammar) -> Option<String> {
    before
        .split(|c: char| !grammar.is_word_character(c, false))
        .filter(|w| !w.is_empty())
        .rev()
        .find(|w| grammar.definer(w).is_some() || grammar.structure.kinds.iter().any(|(k, _)| k == w))
        .map(str::to_owned)
}

/// A Go receiver's type, from the text between `func` and the name: `func (s *Server) ` names `Server`.
///
/// @param before - the line up to the name
fn receiver_type(before: &str) -> Option<String> {
    let open = before.find('(')?;
    let close = before[open..].find(')')? + open;
    let inside = before[open + 1..close].trim();
    let ty = inside.rsplit(char::is_whitespace).next()?.trim_start_matches(['*', '&']);
    let ty = ty.split(['[', '<']).next().unwrap_or(ty);
    (!ty.is_empty()).then(|| ty.to_owned())
}

/// The name a constructor makes, from the start of a value: `T::new(`, `new T(`, `T {`, `T(`, `&T{`.
///
/// Only a capitalised name is taken, because a lower case call is a function whose return type the
/// reading cannot know.
///
/// @param value - the text after `=`
/// @param grammar - the language
fn constructor_name(value: &str, grammar: &Grammar) -> Option<String> {
    let value = value.trim_start_matches(['&', '*']).trim_start();
    let value = value.strip_prefix("new ").unwrap_or(value).trim_start();
    let value = value.strip_prefix("mut ").unwrap_or(value);
    let mut segments = Vec::new();
    let mut at = 0usize;
    while let Some((start, end)) = word_at(value, at, grammar) {
        segments.push(&value[start..end]);
        let after = &value[end..];
        let after = skip_generics(after);
        match after.strip_prefix("::").or_else(|| after.strip_prefix('.')) {
            Some(stripped) => at = value.len() - stripped.len(),
            None => break,
        }
    }
    segments
        .iter()
        .rev()
        .find(|s| s.chars().next().is_some_and(char::is_uppercase))
        .map(|s| (*s).to_owned())
}

/// What a type alias names: the text after `=`, to the end of the statement.
///
/// @param after - the text after the alias's name
fn alias_target(after: &str) -> Option<String> {
    let line = after.split('\n').next().unwrap_or_default();
    let eq = find_top_level(line, "=")?;
    let rest = line[eq + 1..].trim();
    let rest = rest.trim_end_matches(';').trim();
    (!rest.is_empty() && !rest.starts_with('{')).then(|| rest.chars().take(200).collect())
}

/// The comment text of a line, without its markers, or None when the line is not a comment.
///
/// @param line - the line, trimmed
/// @param grammar - the language
fn comment_text(line: &str, grammar: &Grammar) -> Option<String> {
    let markers: Vec<&str> = grammar
        .line_comment
        .iter()
        .map(String::as_str)
        .chain(grammar.block_comment.iter().map(|(open, _)| open.as_str()))
        .collect();
    let is_comment = markers.iter().any(|m| line.starts_with(m))
        || (grammar.block_comment.is_some() && (line.starts_with('*') || line.ends_with("*/")));
    if !is_comment || line.starts_with("#!") {
        return None;
    }
    let text = line
        .trim_start_matches(['/', '*', '!', '#'])
        .trim_end_matches("*/")
        .trim();
    Some(text.chars().take(200).collect())
}

/// Whether a text holds a word at word boundaries.
///
/// @param text - the text
/// @param word - the word
fn contains_word(text: &str, word: &str) -> bool {
    let mut from = 0usize;
    while let Some(found) = text[from..].find(word) {
        let start = from + found;
        let end = start + word.len();
        let before_ok = text[..start].chars().last().is_none_or(|c| !c.is_alphanumeric() && c != '_');
        let after_ok = text[end..].chars().next().is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

/// The first bracket that opens a parameter list after a name: past a generic list, before the body.
///
/// @param after - the text after the name
fn first_parameter_bracket(after: &str) -> Option<usize> {
    let mut at = 0usize;
    let trimmed = after.trim_start();
    at += after.len() - trimmed.len();
    if trimmed.starts_with('<') {
        at += matching_close(trimmed, 0)? + 1;
    }
    let rest = &after[at..];
    let open = rest.find('(')?;
    // A body, an assignment or the end of the statement before the bracket means the bracket is not
    // this definition's parameter list.
    if rest[..open].contains(['{', ';', '\n', '=']) {
        return None;
    }
    Some(at + open)
}

/// The parameter list of a function written as a value: `= (a: A): B =>`, `= async (a) =>`,
/// `= function (a)`. The text from the list's bracket on, or None when the value is not a function.
///
/// @param after - the text after the name
fn function_value(after: &str) -> Option<&str> {
    let line = after.trim_start();
    let line = match line.find('=') {
        Some(at) if !line[..at].contains(['\n', ';', '{']) => &line[at + 1..],
        _ => return None,
    };
    let value = line.trim_start();
    let value = value.strip_prefix("async").map_or(value, str::trim_start);
    let value = value.strip_prefix("function").map_or(value, str::trim_start);
    let value = skip_generics(value);
    let close = matching_close(value, value.find('(').filter(|&o| o == 0)?)?;
    let tail = &value[close + 1..];
    let arrow = tail.find("=>")?;
    (!tail[..arrow].contains(['{', ';', '\n'])).then_some(value)
}

/// The byte of the bracket that closes the one at `open`, counting `()`, `[]`, `{}` and `<>`.
///
/// @param text - the text
/// @param open - the opening bracket's byte
fn matching_close(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let bytes = text.as_bytes();
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'>' if i > 0 && bytes[i - 1] == b'-' => {}
            b'>' if i > 0 && bytes[i - 1] == b'=' => {}
            b'>' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Splits a text at a separator that is outside every bracket.
///
/// @param text - the text
/// @param separator - the separator
fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    let bytes = text.as_bytes();
    for (i, c) in text.char_indices() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '>' if i > 0 && (bytes[i - 1] == b'-' || bytes[i - 1] == b'=') => {}
            '>' => depth -= 1,
            c if c == separator && depth == 0 => {
                parts.push(&text[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Where a mark first appears outside every bracket, or None. `::` never counts as `:`.
///
/// @param text - the text
/// @param mark - the mark
fn find_top_level(text: &str, mark: &str) -> Option<usize> {
    let mut depth = 0i32;
    let bytes = text.as_bytes();
    for (i, c) in text.char_indices() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '>' if i > 0 && (bytes[i - 1] == b'-' || bytes[i - 1] == b'=') => {}
            '>' => depth -= 1,
            _ => {}
        }
        if depth == 0 && text[i..].starts_with(mark) {
            let doubled = mark == ":" && (text[i + 1..].starts_with(':') || text[..i].ends_with(':'));
            if !doubled {
                return Some(i);
            }
        }
    }
    None
}

/// Where a word first appears outside every angle bracket, at word boundaries.
///
/// @param text - the text
/// @param word - the word
fn find_top_level_word(text: &str, word: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in text.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            '{' => return None,
            _ => {}
        }
        if depth == 0 && text[i..].starts_with(word) {
            let before_ok = text[..i].chars().last().is_none_or(char::is_whitespace);
            let after_ok = text[i + word.len()..].chars().next().is_none_or(char::is_whitespace);
            if before_ok && after_ok {
                return Some(i);
            }
        }
    }
    None
}

/// A parameter without its default value: `x: i32 = 3` is `x: i32`.
///
/// @param part - the parameter's text
fn strip_default(part: &str) -> &str {
    match find_top_level(part, "=") {
        Some(at) if !part[at..].starts_with("=>") => part[..at].trim(),
        _ => part,
    }
}

/// Where a head ends: at the body's `{`, a `where`, an `=`, a `;`, or the end of the line.
///
/// @param rest - the text after the parameters
fn head_end(rest: &str) -> usize {
    let mut depth = 0i32;
    let bytes = rest.as_bytes();
    for (i, c) in rest.char_indices() {
        match c {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' => depth -= 1,
            '>' if i > 0 && (bytes[i - 1] == b'-' || bytes[i - 1] == b'=') => {}
            '>' => depth -= 1,
            '{' | ';' | '\n' if depth <= 0 => return i,
            '=' if depth <= 0 && !rest[i..].starts_with("=>") => return i,
            _ => {}
        }
        if depth <= 0 && rest[i..].starts_with(" where ") {
            return i;
        }
    }
    rest.len()
}

/// Where a written type ends: at a `;`, a `,` or an `=` outside every bracket. An `=>` belongs to a
/// function type, `(title: string) => void`, and does not end it.
///
/// @param text - the text that starts with the type
fn type_end(text: &str) -> usize {
    let mut depth = 0i32;
    let bytes = text.as_bytes();
    for (i, c) in text.char_indices() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '>' if i > 0 && (bytes[i - 1] == b'-' || bytes[i - 1] == b'=') => {}
            '>' => depth -= 1,
            ';' | ',' if depth <= 0 => return i,
            '=' if depth <= 0 && !text[i..].starts_with("=>") => return i,
            _ => {}
        }
        if depth < 0 {
            return i;
        }
    }
    text.len()
}

/// The largest character boundary at or below a byte.
///
/// @param text - the text
/// @param at - the byte
fn char_floor(text: &str, mut at: usize) -> usize {
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Every import statement of a file, taken apart.
///
/// @param text - the file's text
/// @param grammar - its language
/// @param symbols - its reading, which says which bytes are comments and strings
fn imports(text: &str, grammar: &Grammar, symbols: &FileSymbols) -> Vec<Import> {
    if grammar.import_keywords.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut line_start = 0usize;
    for line in text.split('\n') {
        let trimmed = line.trim_start();
        let lead = line.len() - trimmed.len();
        let start = line_start + lead;
        line_start += line.len() + 1;
        let keyword = grammar.import_keywords.iter().find(|k| {
            trimmed.starts_with(k.as_str())
                && trimmed[k.len()..].chars().next().is_some_and(|c| c.is_whitespace() || c == '{' || c == '(' || c == '"' || c == '\'')
        });
        let pub_use = trimmed.strip_prefix("pub ").or_else(|| trimmed.strip_prefix("pub(crate) "));
        let (keyword, offset) = match (keyword, pub_use) {
            (Some(k), _) => (k.as_str(), 0),
            (None, Some(rest)) if rest.starts_with("use ") => ("use", trimmed.len() - rest.len()),
            _ => continue,
        };
        if symbols.role_at(start) != crate::lang::symbols::Role::Code {
            continue;
        }
        let statement_start = start + offset;
        match grammar.imports {
            Some(ImportStyle::Quoted) => out.extend(quoted_import(text, statement_start, keyword)),
            _ if keyword == "import" && grammar.imports.is_none() => {
                out.extend(go_import(text, statement_start))
            }
            _ => out.extend(path_import(text, statement_start, keyword, grammar)),
        }
    }
    out
}

/// A path import: Rust's `use a::b::{c, d as e, f::*};` or Python's `import a.b as c` and
/// `from a.b import c, d as e`.
///
/// @param text - the file's text
/// @param start - where the statement starts
/// @param keyword - the import keyword that starts it
/// @param grammar - the language
fn path_import(text: &str, start: usize, keyword: &str, grammar: &Grammar) -> Vec<Import> {
    let separator = grammar.path_separator.clone().unwrap_or_else(|| ".".to_owned());
    let body_start = start + keyword.len();
    let end = statement_end(text, body_start);
    let body = text[body_start..end].trim().trim_end_matches(';').trim();
    let range = start..end;
    if keyword == "from" {
        let Some((module, names)) = body.split_once(" import ") else { return Vec::new() };
        let module: Vec<String> = module.trim().split('.').map(str::to_owned).collect();
        let names = names.trim().trim_start_matches('(').trim_end_matches(')');
        return names
            .split(',')
            .filter_map(|name| named_import(&module, name, &range))
            .collect();
    }
    let mut out = Vec::new();
    for part in split_top_level(body, ',') {
        expand_tree(&[], part.trim(), &separator, &range, &mut out);
    }
    out
}

/// One `name` or `name as alias` of a `from … import` line.
///
/// @param module - the module's segments
/// @param name - the entry
/// @param range - the statement's bytes
fn named_import(module: &[String], name: &str, range: &Range<usize>) -> Option<Import> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let (name, alias) = match name.split_once(" as ") {
        Some((n, a)) => (n.trim(), Some(a.trim().to_owned())),
        None => (name, None),
    };
    let mut path = module.to_vec();
    path.push(name.to_owned());
    Some(Import { glob: name == "*", path, alias, range: range.clone() })
}

/// Expands one entry of a path import, recursively through `{…}` groups.
///
/// @param prefix - the segments before this entry
/// @param entry - the entry's text
/// @param separator - what joins segments
/// @param range - the statement's bytes
/// @param out - where the imports go
fn expand_tree(prefix: &[String], entry: &str, separator: &str, range: &Range<usize>, out: &mut Vec<Import>) {
    if entry.is_empty() {
        return;
    }
    if let Some(open) = entry.find('{') {
        let head: Vec<String> = entry[..open]
            .split(separator)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        let close = matching_close(entry, open).unwrap_or(entry.len() - 1);
        let mut path = prefix.to_vec();
        path.extend(head);
        for part in split_top_level(&entry[open + 1..close.min(entry.len())], ',') {
            let part = part.trim();
            if part == "self" {
                out.push(Import { path: path.clone(), alias: None, glob: false, range: range.clone() });
            } else {
                expand_tree(&path, part, separator, range, out);
            }
        }
        return;
    }
    let (entry, alias) = match entry.split_once(" as ") {
        Some((e, a)) => (e.trim(), Some(a.trim().to_owned())),
        None => (entry, None),
    };
    let mut path = prefix.to_vec();
    path.extend(entry.split(separator).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned));
    let glob = path.last().is_some_and(|s| s == "*");
    if glob {
        path.pop();
    }
    out.push(Import { path, alias, glob, range: range.clone() });
}

/// A quoted import: `import X, { a, b as c } from './m'`, `import * as m from './m'`, `import './m'`.
///
/// @param text - the file's text
/// @param start - where the statement starts
/// @param keyword - `import` or `export`
fn quoted_import(text: &str, start: usize, keyword: &str) -> Vec<Import> {
    if keyword != "import" {
        return Vec::new();
    }
    let end = quoted_statement_end(text, start);
    let statement = &text[start..end];
    let Some(specifier) = last_quoted(statement) else { return Vec::new() };
    let range = start..end;
    let clause = statement["import".len()..].trim_start();
    let clause = clause.strip_prefix("type ").unwrap_or(clause);
    let clause = match clause.rfind(" from") {
        Some(at) => &clause[..at],
        None => return vec![Import { path: vec![specifier], alias: None, glob: false, range }],
    };
    let mut out = Vec::new();
    let mut rest = clause.trim();
    if let Some(open) = rest.find('{') {
        let close = matching_close(rest, open).unwrap_or(rest.len() - 1);
        for part in rest[open + 1..close.min(rest.len())].split(',') {
            let part = part.trim().trim_start_matches("type ").trim();
            if part.is_empty() {
                continue;
            }
            let (name, alias) = match part.split_once(" as ") {
                Some((n, a)) => (n.trim(), Some(a.trim().to_owned())),
                None => (part, None),
            };
            out.push(Import { path: vec![specifier.clone(), name.to_owned()], alias, glob: false, range: range.clone() });
        }
        rest = rest[..open].trim().trim_end_matches(',').trim();
    }
    if let Some(namespace) = rest.strip_prefix("* as ") {
        out.push(Import { path: vec![specifier.clone()], alias: Some(namespace.trim().to_owned()), glob: true, range: range.clone() });
    } else if !rest.is_empty() {
        let default = rest.split(',').next().unwrap_or(rest).trim();
        out.push(Import { path: vec![specifier, "default".to_owned()], alias: Some(default.to_owned()), glob: false, range });
    }
    out
}

/// Go's `import "fmt"` and `import ( "a"; b "c" )`.
///
/// @param text - the file's text
/// @param start - where the statement starts
fn go_import(text: &str, start: usize) -> Vec<Import> {
    let after = &text[start + "import".len()..];
    let trimmed = after.trim_start();
    let body_start = start + "import".len() + (after.len() - trimmed.len());
    let (body, end) = match trimmed.starts_with('(') {
        true => {
            let close = trimmed.find(')').unwrap_or(trimmed.len());
            (&trimmed[1..close], body_start + close + 1)
        }
        false => {
            let line_end = trimmed.find('\n').unwrap_or(trimmed.len());
            (&trimmed[..line_end], body_start + line_end)
        }
    };
    body.lines()
        .filter_map(|line| {
            let spec = last_quoted(line)?;
            let alias = line.split_whitespace().next().filter(|w| !w.starts_with('"')).map(str::to_owned);
            Some(Import { path: vec![spec], alias, glob: false, range: start..end })
        })
        .collect()
}

/// Where a path import statement ends: at its `;`, or for a language with no `;`, at the end of the line
/// once every bracket has closed.
///
/// @param text - the file's text
/// @param from - where the statement's body starts
fn statement_end(text: &str, from: usize) -> usize {
    let mut depth = 0i32;
    for (i, c) in text[from..].char_indices() {
        match c {
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth -= 1,
            ';' if depth <= 0 => return from + i + 1,
            '\n' if depth <= 0 && !text[from..from + i].trim_end().ends_with('\\') => {
                if !text[from..from + i].contains("use ") && !text[..from].ends_with("use") {
                    return from + i;
                }
                if text[from..from + i].trim_end().ends_with(';') {
                    return from + i;
                }
            }
            _ => {}
        }
    }
    text.len()
}

/// Where a quoted import ends: just after the quoted specifier and an optional `;`.
///
/// @param text - the file's text
/// @param start - where the statement starts
fn quoted_statement_end(text: &str, start: usize) -> usize {
    let mut quote: Option<char> = None;
    for (i, c) in text[start..].char_indices() {
        match quote {
            Some(q) if c == q => {
                let after = start + i + 1;
                // `import x from './m'` ends here; a quote inside the braces does not occur.
                let rest = &text[after..];
                return after + usize::from(rest.starts_with(';'));
            }
            Some(_) => {}
            None if c == '\'' || c == '"' || c == '`' => quote = Some(c),
            None if c == ';' => return start + i + 1,
            _ => {}
        }
    }
    text.len()
}

/// The contents of the last quoted string in a text.
///
/// @param text - the text
fn last_quoted(text: &str) -> Option<String> {
    let end = text.rfind(['\'', '"', '`'])?;
    let quote = text[end..].chars().next()?;
    let start = text[..end].rfind(quote)?;
    Some(text[start + 1..end].to_owned())
}

#[cfg(test)]
mod tests;
