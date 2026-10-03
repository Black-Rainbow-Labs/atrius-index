//! Reads a command line into a command and its named arguments, using the catalogue.
//!
//! The MCP server uses the same reader for a `command` written as one line, such as
//! `find retry --path backend`, so an agent that writes the command the way a person types it is
//! understood.

use serde_json::{Map, Number, Value};

use crate::catalogue::{self, Command, Takes};

/// A command line, read.
#[derive(Debug)]
pub struct Parsed {
    /// The command.
    pub command: &'static Command,
    /// Its named arguments, without `root` and `json`.
    pub arguments: Map<String, Value>,
    /// The `--root` given, if any.
    pub root: Option<String>,
    /// Whether `--json` was given.
    pub json: bool,
}

/// Splits one line into words the way a POSIX shell does for quotes and backslashes, without
/// expanding anything.
///
/// @param line - the line
pub fn split_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some('"') if c == '\\' && matches!(chars.peek(), Some('"') | Some('\\')) => {
                word.push(chars.next().unwrap_or('\\'));
            }
            Some(_) => word.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            None => word.push(c),
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    words
}

/// Finds the command a list of words starts with, and how many words its name took.
///
/// @param words - the words after the program's name
pub fn find_command(words: &[String]) -> Option<(&'static Command, usize)> {
    if words.len() >= 2 {
        let two = format!("{} {}", words[0], words[1]);
        if let Some(command) = catalogue::command(&two) {
            return Some((command, 2));
        }
    }
    words.first().and_then(|w| catalogue::command(w)).map(|c| (c, 1))
}

/// Reads the words after the program's name.
///
/// @param words - the words
pub fn parse(words: &[String]) -> Result<Parsed, String> {
    let Some((command, used)) = find_command(words) else {
        let first = words.first().map(String::as_str).unwrap_or_default();
        let groups = ["watch", "service"];
        if groups.contains(&first) {
            return Err(format!("`{first}` needs a second word. Run `atrius help` for the list."));
        }
        return Err(format!("There is no command called `{first}`. Run `atrius help` for the list."));
    };
    parse_command(command, &words[used..])
}

/// Reads the words after a command's name.
///
/// @param command - the command
/// @param words - the words after its name
pub fn parse_command(command: &'static Command, words: &[String]) -> Result<Parsed, String> {
    let flags = catalogue::all_flags(command);
    let mut arguments = Map::new();
    let mut positional: Vec<String> = Vec::new();
    let mut at = 0;
    while at < words.len() {
        let word = &words[at];
        at += 1;
        let flag = if let Some(name) = word.strip_prefix("--").filter(|n| !n.is_empty()) {
            let (name, inline) = match name.split_once('=') {
                Some((n, v)) => (n, Some(v.to_owned())),
                None => (name, None),
            };
            let Some(flag) = flags.iter().find(|f| f.name == name) else {
                return Err(format!("`{}` has no flag `--{name}`. {}", command.name, usage_hint(command)));
            };
            Some((*flag, inline))
        } else if word.len() == 2 && word.starts_with('-') && word != "--" {
            let letter = word.chars().nth(1).unwrap_or(' ');
            match flags.iter().find(|f| f.short == Some(letter)) {
                Some(flag) => Some((*flag, None)),
                None => None,
            }
        } else {
            None
        };
        let Some((flag, inline)) = flag else {
            if word == "--" {
                positional.extend(words[at..].iter().cloned());
                break;
            }
            positional.push(word.clone());
            continue;
        };
        let value = match flag.takes {
            Takes::Switch => match inline.as_deref() {
                None | Some("true") => Value::Bool(true),
                Some("false") => Value::Bool(false),
                Some(other) => return Err(format!("`--{}` is a switch and takes no value, not `{other}`.", flag.name)),
            },
            takes => {
                let raw = match inline {
                    Some(v) => v,
                    None => {
                        let Some(v) = words.get(at) else {
                            return Err(format!("`--{}` needs a value: <{}>.", flag.name, flag.value));
                        };
                        at += 1;
                        v.clone()
                    }
                };
                if takes == Takes::Number {
                    let n: u64 = raw.trim().parse().map_err(|_| {
                        format!("`--{}` takes a whole number, not `{raw}`.", flag.name)
                    })?;
                    Value::Number(Number::from(n))
                } else {
                    Value::String(raw)
                }
            }
        };
        arguments.insert(flag.name.to_owned(), value);
    }
    fill_positional(command, positional, &mut arguments)?;
    let root = match arguments.remove("root") {
        Some(Value::String(s)) => Some(s),
        _ => None,
    };
    let json = matches!(arguments.remove("json"), Some(Value::Bool(true)));
    Ok(Parsed { command, arguments, root, json })
}

/// Puts the positional words into the command's named arguments.
///
/// @param command - the command
/// @param positional - the words that were not flags
/// @param arguments - the named arguments so far
fn fill_positional(
    command: &Command,
    positional: Vec<String>,
    arguments: &mut Map<String, Value>,
) -> Result<(), String> {
    let mut words = positional.into_iter();
    for argument in command.arguments {
        let value = if argument.rest {
            let rest: Vec<String> = words.by_ref().collect();
            (!rest.is_empty()).then(|| rest.join(" "))
        } else {
            words.next()
        };
        match value {
            Some(v) => {
                arguments.insert(argument.name.to_owned(), Value::String(v));
            }
            None if argument.required && !arguments.contains_key(argument.name) => {
                return Err(format!("`{}` needs <{}>. {}", command.name, argument.name, usage_hint(command)));
            }
            None => {}
        }
    }
    let extra: Vec<String> = words.collect();
    if !extra.is_empty() {
        return Err(format!(
            "`{}` was given more than it takes: {}. {}",
            command.name,
            extra.join(" "),
            usage_hint(command)
        ));
    }
    Ok(())
}

/// The usage line, to add to a refusal.
///
/// @param command - the command
fn usage_hint(command: &Command) -> String {
    format!("Usage: {}", catalogue::usage(command, "atrius"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        split_words(line)
    }

    #[test]
    fn a_query_of_several_words_needs_no_quotes() {
        let parsed = parse(&words("find where do we retry --path backend -i")).unwrap();
        assert_eq!(parsed.command.name, "find");
        assert_eq!(parsed.arguments["query"], "where do we retry");
        assert_eq!(parsed.arguments["path"], "backend");
        assert_eq!(parsed.arguments["ignore-case"], true);
    }

    #[test]
    fn quotes_keep_a_regex_together_and_root_and_json_are_taken_out() {
        let parsed = parse(&words(r#"find "fn \w+_token" --mode regex --root /tmp/x --json"#)).unwrap();
        assert_eq!(parsed.arguments["query"], r"fn \w+_token");
        assert_eq!(parsed.root.as_deref(), Some("/tmp/x"));
        assert!(parsed.json);
        assert!(!parsed.arguments.contains_key("root"));
    }

    #[test]
    fn two_word_commands_and_numbers_read() {
        let parsed = parse(&words("watch add /code/x")).unwrap();
        assert_eq!(parsed.command.name, "watch add");
        assert_eq!(parsed.arguments["folder"], "/code/x");
        let parsed = parse(&words("def Layout --limit=3")).unwrap();
        assert_eq!(parsed.arguments["limit"], 3);
    }

    #[test]
    fn mistakes_are_refused_with_the_usage_line() {
        assert!(parse(&words("find")).unwrap_err().contains("needs <query>"));
        assert!(parse(&words("def a b")).unwrap_err().contains("more than it takes"));
        assert!(parse(&words("def a --limit x")).unwrap_err().contains("whole number"));
        assert!(parse(&words("def a --nope")).unwrap_err().contains("no flag `--nope`"));
        assert!(parse(&words("watch")).unwrap_err().contains("second word"));
        assert!(parse(&words("frobnicate")).unwrap_err().contains("no command"));
    }
}
