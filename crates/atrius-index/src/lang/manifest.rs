//! The `name = value` format Unluminous's settings, its project state and its plugin manifests are
//! written in, and the part of a plugin manifest that describes a language.
//!
//! It is here rather than in the window because two programs read a `plugin.conf`: the window, which
//! loads plugins, and the code index (`unluminous-index`), which needs each language's `Grammar` to find
//! definitions and chunk files and has no window behind it. One reading in one crate is what keeps the
//! two from disagreeing about what a manifest says.

use std::collections::BTreeMap;

use crate::lang::symbols::SymbolKind;
use crate::lang::syntax::{Grammar, ImportStyle, PathRoot};

/// Named values read from or written to the settings file.
///
/// The store knows nothing about what the names mean; `crate::settings` owns that. Keeping the two apart
/// means the settings can grow a value without the file handling changing at all.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Values(BTreeMap<String, String>);

impl Values {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        self.0.insert(name.to_owned(), value.into());
    }

    /// Take a name out, so the file no longer holds it.
    ///
    /// **What a setting that has gone back to its default needs**, and it is not the same as setting it
    /// to an empty string: several settings here mean "whatever this Unluminous's own default is" by having
    /// no line at all — `terminal.shell`, `appearance.theme`, `appearance.icons` — and an empty line
    /// would read as a shell called nothing. Saving merges over the file that is already there
    /// (`settings::save_with`), so without this a value that was cleared would stay in the file and come
    /// back at the next start. See [`Values::set_or_clear`].
    pub fn remove(&mut self, name: &str) {
        self.0.remove(name);
    }

    /// Write a value, or take the name out when it is empty.
    ///
    /// One function rather than an `if` at each of the seven places that mean "empty is the default", so
    /// a later one cannot forget the second half and leave a setting that cannot be un-chosen.
    pub fn set_or_clear(&mut self, name: &str, value: &str) {
        match value.is_empty() {
            true => self.remove(name),
            false => self.set(name, value.to_owned()),
        }
    }

    pub fn text(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    /// Every name that begins with `prefix`, with the prefix removed, in name order.
    ///
    /// What reads a family of keys whose names are not known in advance, which is what a plugin's
    /// submenus are: `menu.submenu.new` and `menu.submenu.new.entries` are two members of one family
    /// and nothing in Unluminous knows the word `new` until the manifest is read. The order is the map's
    /// order, so a family read twice is read the same way both times and a menu built from one is the
    /// same shape every time.
    pub fn starting_with(&self, prefix: &str) -> Vec<(String, String)> {
        self.0
            .iter()
            .filter_map(|(name, value)| {
                name.strip_prefix(prefix).map(|rest| (rest.to_owned(), value.clone()))
            })
            .collect()
    }

    pub fn number(&self, name: &str) -> Option<f32> {
        self.text(name).and_then(|value| value.trim().parse().ok())
    }

    pub fn flag(&self, name: &str) -> Option<bool> {
        match self.text(name)?.trim() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        }
    }

    /// Read `name = value` lines. A line without an `=` is ignored rather than making the whole file
    /// unreadable.
    ///
    /// A `#` starts a comment **when it is followed by a space or ends the line**. That rule is a
    /// little more particular than "everything after a hash", and it is that way because of colours:
    /// a plugin's colour scheme is written `theme.keyword = #FF79C6`, and the plain rule ate the
    /// value and left the plugin with no colours at all. Writing the hash is what anybody would do,
    /// so the format accommodates it rather than making it a trap. `size = 20  # after the value`
    /// still reads as a comment, because that hash is followed by a space.
    pub fn parse(text: &str) -> Self {
        let mut values = Self::new();
        for line in text.lines() {
            let line = match Self::comment_at(line) {
                Some(at) => &line[..at],
                None => line,
            };
            let Some((name, value)) = line.split_once('=') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            values.set(name, value.trim().to_owned());
        }
        values
    }

    /// Where the comment starts on this line, if it has one.
    ///
    /// A `#` opens a comment when what follows it is whitespace **and there is something after that
    /// whitespace**. A `#` that is the last thing on the line is part of the value.
    ///
    /// **That second half is a fix rather than a nicety.** `task-1922`: without it
    /// `language.line_comment = #` parses to the *empty string*, and an empty line comment is worse
    /// than none at all, because `rest.starts_with("")` is true at every byte — every file of that
    /// language would be drawn as one comment from its first character. It is not hypothetical for
    /// a value either: `plugins/rust/plugin.conf` and `plugins/css/plugin.conf` have both ended
    /// `language.operators` with `#` since they were written, and both have been silently losing it,
    /// so Rust's attribute character and CSS's hash have never been coloured as operators.
    ///
    /// An inline comment still works, because a comment somebody wrote has words in it. A line
    /// ending `value #` with nothing after the hash now keeps the hash, which is the one thing this
    /// gives up and is not something anybody writes on purpose.
    fn comment_at(line: &str) -> Option<usize> {
        line.char_indices()
            .find(|(at, character)| {
                *character == '#'
                    && line[at + 1..].chars().next().map(char::is_whitespace).unwrap_or(true)
                    && !line[at + 1..].trim().is_empty()
            })
            .map(|(at, _)| at)
    }

    pub fn to_text(&self) -> String {
        self.to_text_headed(
            "# Unluminous settings. Written by Unluminous, and safe to edit by hand.",
        )
    }

    /// The same, under a heading of the caller's own. The project state is written in this format too
    /// and is not the settings, so it says so at the top of its own file.
    pub fn to_text_headed(&self, heading: &str) -> String {
        let mut out = format!("{heading}\n");
        for (name, value) in &self.0 {
            out.push_str(name);
            out.push_str(" = ");
            out.push_str(value);
            out.push('\n');
        }
        out
    }
}

/// The `Grammar` a language plugin's manifest describes: every `language.` key the tokeniser, the
/// symbol reading and the import reading use. A key a manifest leaves out keeps the behaviour the
/// manifest key was added without, which is the rule every key since `task-1671` has kept.
///
/// @param values - the manifest
/// @param name - the language's name, `plugin.name` or the plugin's id
pub fn language_grammar(values: &Values, name: &str) -> Result<Grammar, String> {
    Ok(Grammar {
        language: name.to_owned(),
        keywords: list(values, "language.keywords"),
        builtins: list(values, "language.builtins"),
        types: list(values, "language.types"),
        line_comment: values.text("language.line_comment").map(str::to_owned),
        block_comment: pair(values, "language.block_comment"),
        strings: values
            .text("language.strings")
            .unwrap_or("\", '")
            .split(',')
            .filter_map(|quote| quote.trim().chars().next())
            .collect(),
        escapes: values.flag("language.escapes").unwrap_or(true),
        operators: values.text("language.operators").unwrap_or_default().chars().collect(),
        numbers: values.flag("language.numbers").unwrap_or(true),
        // Comma separated single characters, the way `language.strings` names its quotes. Empty for
        // every language but CSS, where a hyphen is a letter.
        word_characters: values
            .text("language.word_characters")
            .unwrap_or_default()
            .split(',')
            .filter_map(|character| character.trim().chars().next())
            .collect(),
        hex_colors: values.flag("language.hex_colors").unwrap_or(false),
        // The two `task-1675` added, both off unless a language asks for them.
        definers: definers(values)?,
        brace_definitions: values.flag("language.brace_definitions").unwrap_or(false),
        brace_next_line: values.flag("language.brace_next_line").unwrap_or(false),
        definition_tail: values.flag("language.definition_tail").unwrap_or(false),
        // The nine `task-1680` added, and the same rule again.
        export_keyword: word(values, "language.export_keyword"),
        imports: import_style(values)?,
        import_keywords: list(values, "language.import_keywords"),
        import_extensions: list(values, "language.import_extensions")
            .into_iter()
            .map(|extension| match extension.starts_with('.') {
                true => extension,
                false => format!(".{extension}"),
            })
            .collect(),
        import_index: list(values, "language.import_index"),
        import_omit_extension: values.flag("language.import_omit_extension").unwrap_or(false),
        path_separator: word(values, "language.path_separator"),
        source_roots: list(values, "language.source_roots"),
        path_roots: path_roots(values)?,
        // The two `task-1694` added, and the same rule a sixth time.
        markup: values.flag("language.markup").unwrap_or(false),
        raw_text: raw_text(values)?,
    })
}

/// The file extensions a language plugin claims, lower case and without the dot.
///
/// @param values - the manifest
pub fn language_extensions(values: &Values) -> Vec<String> {
    list(values, "language.extensions")
        .into_iter()
        .map(|extension| extension.trim_start_matches('.').to_lowercase())
        .collect()
}

/// `language.definers`: a comma list of `keyword=kind`, such as `fn=function, struct=type`.
///
/// @param values - the manifest
fn definers(values: &Values) -> Result<Vec<(String, SymbolKind)>, String> {
    let mut found = Vec::new();
    for (keyword, kind) in pairs(values, "language.definers") {
        let Some(kind) = kind else {
            return Err(format!(
                "language.definers holds `{keyword}`, which is not `keyword=kind`"
            ));
        };
        let Some(parsed) = SymbolKind::parse(&kind) else {
            let known: Vec<&str> = SymbolKind::ALL.iter().map(|kind| kind.name()).collect();
            return Err(format!(
                "language.definers says `{keyword}={kind}`, and a definition in Unluminous is one of {}",
                known.join(", ")
            ));
        };
        if keyword.is_empty() {
            return Err(format!(
                "language.definers holds `{keyword}={kind}`, which names no keyword"
            ));
        }
        found.push((keyword, parsed));
    }
    Ok(found)
}

/// `language.imports`: which of the two ways a module is written this language uses, if any.
///
/// @param values - the manifest
fn import_style(values: &Values) -> Result<Option<ImportStyle>, String> {
    let Some(named) = word(values, "language.imports") else {
        return Ok(None);
    };
    match ImportStyle::parse(&named) {
        Some(style) => Ok(Some(style)),
        None => {
            let known: Vec<&str> = ImportStyle::ALL.iter().map(|style| style.name()).collect();
            Err(format!(
                "language.imports is `{named}`, and an import in Unluminous is written {}",
                known.join(" or ")
            ))
        }
    }
}

/// `language.path_roots`: a comma list of `word=meaning` naming the segments of a module path that
/// are not module names, such as `crate=package, self=module, super=parent`.
///
/// @param values - the manifest
fn path_roots(values: &Values) -> Result<Vec<(String, PathRoot)>, String> {
    let mut found = Vec::new();
    for (word, meaning) in pairs(values, "language.path_roots") {
        let Some(meaning) = meaning else {
            return Err(format!("language.path_roots holds `{word}`, which is not `word=meaning`"));
        };
        let Some(parsed) = PathRoot::parse(&meaning) else {
            let known: Vec<&str> = PathRoot::ALL.iter().map(|root| root.name()).collect();
            return Err(format!(
                "language.path_roots says `{word}={meaning}`, and a root in Unluminous is one of {}",
                known.join(", ")
            ));
        };
        if word.is_empty() {
            return Err(format!(
                "language.path_roots holds `{word}={meaning}`, which names no word"
            ));
        }
        found.push((word, parsed));
    }
    Ok(found)
}

/// `language.raw_text`: the elements whose body is not markup, each with the language it is written in,
/// or with none for an escapable one.
///
/// @param values - the manifest
fn raw_text(values: &Values) -> Result<Vec<(String, Option<String>)>, String> {
    let mut found = Vec::new();
    for (element, language) in pairs(values, "language.raw_text") {
        if element.is_empty() {
            let entry = match &language {
                Some(language) => format!("{element}={language}"),
                None => element.clone(),
            };
            return Err(format!("language.raw_text holds `{entry}`, which names no element"));
        }
        // A bare element (no `=` at all) has no language, and that is an escapable one rather than a
        // mistake. An `=` with nothing after it is the mistake.
        if language.as_deref().is_some_and(str::is_empty) {
            return Err(format!(
                "language.raw_text holds `{element}=`, which names an element and no language"
            ));
        }
        found.push((element, language));
    }
    Ok(found)
}

/// A value with its spaces trimmed, or None when it is missing or empty.
///
/// @param values - the manifest
/// @param name - the key
pub fn word(values: &Values, name: &str) -> Option<String> {
    values.text(name).map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned)
}

/// A comma separated value as a list, with the spaces trimmed and the empty entries dropped.
///
/// @param values - the manifest
/// @param name - the key
pub fn list(values: &Values, name: &str) -> Vec<String> {
    values
        .text(name)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Two comma separated values, which is what a block comment's opener and terminator are.
///
/// @param values - the manifest
/// @param name - the key
pub fn pair(values: &Values, name: &str) -> Option<(String, String)> {
    let parts = list(values, name);
    match parts.as_slice() {
        [open, close] => Some((open.clone(), close.clone())),
        _ => None,
    }
}

/// `list(values, name)`, with each entry split on its first `=` into a left and right half. An entry
/// with no `=` comes back with no right side.
///
/// @param values - the manifest
/// @param name - the key
pub fn pairs(values: &Values, name: &str) -> Vec<(String, Option<String>)> {
    list(values, name)
        .into_iter()
        .map(|entry| match entry.split_once('=') {
            Some((left, right)) => (left.trim().to_owned(), Some(right.trim().to_owned())),
            None => (entry, None),
        })
        .collect()
}
