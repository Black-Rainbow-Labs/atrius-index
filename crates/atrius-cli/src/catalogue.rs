//! Every command `atrius` has, in one table.
//!
//! The command line parser, `atrius help`, the MCP tool's schema and description, and
//! `docs/commands.md` are all made from this table, so they cannot disagree. A test fails when
//! `docs/commands.md` is not what [`crate::docs::commands_page`] writes.

/// What kind of value a flag takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Takes {
    /// Text.
    Text,
    /// A whole number.
    Number,
    /// Nothing: the flag is a switch.
    Switch,
}

/// One flag of a command.
#[derive(Clone, Copy, Debug)]
pub struct Flag {
    /// The name after `--`.
    pub name: &'static str,
    /// A one letter alias after `-`, if any.
    pub short: Option<char>,
    /// What the value is called in the usage line, such as `glob`.
    pub value: &'static str,
    /// What kind of value it takes.
    pub takes: Takes,
    /// What it does.
    pub help: &'static str,
}

/// One positional argument of a command.
#[derive(Clone, Copy, Debug)]
pub struct Argument {
    /// The name the verb reads it by.
    pub name: &'static str,
    /// Whether the command needs it.
    pub required: bool,
    /// Whether it takes the rest of the line, so a query of several words needs no quotes.
    pub rest: bool,
    /// What it is.
    pub help: &'static str,
}

/// Where a command runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runs {
    /// Answered by the index host of a checkout, from the command line and from MCP.
    Query,
    /// Run by the command line program itself. Not offered over MCP.
    Local,
}

/// One command.
#[derive(Clone, Copy, Debug)]
pub struct Command {
    /// The word after `atrius`, such as `find`. Two words for a group, such as `watch add`.
    pub name: &'static str,
    /// One sentence about what it does.
    pub summary: &'static str,
    /// Its positional arguments.
    pub arguments: &'static [Argument],
    /// Its own flags. `--root` and `--json` are added to every query.
    pub flags: &'static [Flag],
    /// Commands to copy.
    pub examples: &'static [&'static str],
    /// Where it runs.
    pub runs: Runs,
}

/// A flag that takes text.
const fn text(name: &'static str, value: &'static str, help: &'static str) -> Flag {
    Flag { name, short: None, value, takes: Takes::Text, help }
}

/// A flag that takes a whole number.
const fn number(name: &'static str, value: &'static str, help: &'static str) -> Flag {
    Flag { name, short: None, value, takes: Takes::Number, help }
}

/// A switch.
const fn switch(name: &'static str, help: &'static str) -> Flag {
    Flag { name, short: None, value: "", takes: Takes::Switch, help }
}

/// A required argument that takes one word.
const fn one(name: &'static str, help: &'static str) -> Argument {
    Argument { name, required: true, rest: false, help }
}

/// A required argument that takes the rest of the line.
const fn rest(name: &'static str, help: &'static str) -> Argument {
    Argument { name, required: true, rest: true, help }
}

/// The flag every command that reads a checkout takes.
pub const ROOT: Flag = text(
    "root",
    "folder",
    "The checkout. The git checkout at or above the working folder by default.",
);

/// The flag that prints the whole reply as JSON.
pub const JSON: Flag = switch("json", "Print the whole reply as JSON instead of its text.");

/// The flag that asks for the fields behind the text.
const STRUCTURED: Flag =
    switch("structured", "Also answer with the fields behind the text, for a program to read.");

/// Every command.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "find",
        summary: "Search the code: the same lines ripgrep would print for a literal or a regex, grouped by file and cut to a token budget, from an index kept current as files change.",
        arguments: &[rest("query", "What to look for: text, a regex, a name, or a question in plain English such as `where do we retry a failed write`.")],
        flags: &[
            text("mode", "auto|literal|regex|symbol|semantic", "How to read the query. `auto` by default: a regex when it has regex characters, a name's definitions and uses for one identifier, a question about meaning for a sentence, plain text otherwise."),
            text("path", "path", "A folder or file inside the checkout to search, instead of all of it."),
            text("glob", "glob", "Only files matching these globs, as ripgrep takes them: `*.rs`, `!*.test.ts`, `src/**`."),
            text("type", "name", "Only files of this ripgrep type, such as `rust`, `ts` or `py`."),
            Flag { name: "ignore-case", short: Some('i'), value: "", takes: Takes::Switch, help: "Match regardless of case." },
            number("budget", "tokens", "How much text to answer with, in tokens. 1500 by default; 0 for every line."),
            STRUCTURED,
        ],
        examples: &[
            "atrius find resolveSkipToken",
            "atrius find \"fn \\w+_token\" --mode regex --glob *.rs",
            "atrius find watchdog -i --path backend --json",
            "atrius find where do we retry a failed write",
        ],
        runs: Runs::Query,
    },
    Command {
        name: "def",
        summary: "Where a name is defined: each definition's path, line and signature line, the likeliest first.",
        arguments: &[one("name", "The name: a function, type, constant or module. `Type::name` and `Type.name` narrow it to a member.")],
        flags: &[
            number("limit", "number", "How many definitions to list. 10 by default."),
            text("path", "path", "Only definitions under this folder."),
            STRUCTURED,
        ],
        examples: &["atrius def resolveSkipToken", "atrius def Layout --path crates/core --json"],
        runs: Runs::Query,
    },
    Command {
        name: "refs",
        summary: "Every use of a name, grouped by file, the checkout's own source before tests and docs, and in each file the lines that define it first.",
        arguments: &[one("name", "The name.")],
        flags: &[
            text("path", "path", "Only uses under this folder or in this file."),
            text("glob", "glob", "Only files matching these globs."),
            number("budget", "tokens", "How much text to answer with, in tokens. 1500 by default; 0 for every use."),
            STRUCTURED,
        ],
        examples: &["atrius refs resolveSkipToken", "atrius refs relayout --path crates --budget 0"],
        runs: Runs::Query,
    },
    Command {
        name: "fragment",
        summary: "The function or section around a line, with line numbers and the signatures that enclose it, so it can be read without reading the whole file.",
        arguments: &[one("target", "A file and a line, such as `src/layout.rs:386`.")],
        flags: &[
            number("line", "number", "The line, when the target is a file alone."),
            number("context", "lines", "Extra lines before and after it. None by default."),
            STRUCTURED,
        ],
        examples: &["atrius fragment src/layout.rs:386", "atrius fragment backend/src/main.ts --line 40 --context 5"],
        runs: Runs::Query,
    },
    Command {
        name: "outline",
        summary: "A file's definitions, or a Markdown file's headings, one line each with their line ranges.",
        arguments: &[one("path", "The file.")],
        flags: &[STRUCTURED],
        examples: &["atrius outline src/layout.rs", "atrius outline README.md --json"],
        runs: Runs::Query,
    },
    Command {
        name: "files",
        summary: "Files whose path matches a name, a part of one, or a glob, best match first.",
        arguments: &[rest("query", "A file name, part of one, or a glob such as `**/*.test.ts`.")],
        flags: &[
            number("limit", "number", "How many to list. 20 by default."),
            text("path", "folder", "Only files under this folder; a glob is matched relative to it."),
            STRUCTURED,
        ],
        examples: &["atrius files layout.rs", "atrius files \"*.test.ts\" --limit 50", "atrius files \"*.rs\" --path crates/core"],
        runs: Runs::Query,
    },
    Command {
        name: "status",
        summary: "Whether the index of the checkout is ready, how many files it holds, how much memory it uses, where its file is and which process hosts it.",
        arguments: &[],
        flags: &[],
        examples: &["atrius status", "atrius status --json"],
        runs: Runs::Query,
    },
    Command {
        name: "serve",
        summary: "Run the index host for a checkout in this terminal. A query starts one on its own when none is running, so this is only needed to start it ahead of time or to watch it.",
        arguments: &[],
        flags: &[number("idle", "minutes", "Stop after this many minutes with no request. 120 by default, or `ATRIUS_IDLE_MINUTES`; 0 runs until stopped.")],
        examples: &["atrius serve", "atrius serve --root ~/code/project --idle 0"],
        runs: Runs::Local,
    },
    Command {
        name: "stop",
        summary: "Stop the index host of a checkout. The index file stays, so the next host loads it.",
        arguments: &[],
        flags: &[],
        examples: &["atrius stop", "atrius stop --root ~/code/project"],
        runs: Runs::Local,
    },
    Command {
        name: "hosts",
        summary: "Every index host on this machine: its checkout, process, port and whether it answers.",
        arguments: &[],
        flags: &[],
        examples: &["atrius hosts", "atrius hosts --json"],
        runs: Runs::Local,
    },
    Command {
        name: "mcp",
        summary: "Run the MCP server on standard input and output. The same program as `atrius-mcp`.",
        arguments: &[],
        flags: &[],
        examples: &["atrius mcp", "claude mcp add atrius -- atrius mcp"],
        runs: Runs::Local,
    },
    Command {
        name: "watch add",
        summary: "Add a folder to the list the background service keeps indexed.",
        arguments: &[one("folder", "The checkout.")],
        flags: &[],
        examples: &["atrius watch add ~/code/project", "atrius watch add ."],
        runs: Runs::Local,
    },
    Command {
        name: "watch remove",
        summary: "Take a folder off the list the background service keeps indexed.",
        arguments: &[one("folder", "The checkout.")],
        flags: &[],
        examples: &["atrius watch remove ~/code/project"],
        runs: Runs::Local,
    },
    Command {
        name: "watch list",
        summary: "The folders the background service keeps indexed.",
        arguments: &[],
        flags: &[],
        examples: &["atrius watch list"],
        runs: Runs::Local,
    },
    Command {
        name: "service install",
        summary: "Start the background service at every login, and now. A login item for this user: the Run key on Windows, a LaunchAgent on macOS, a systemd user unit or an autostart entry on Linux.",
        arguments: &[],
        flags: &[],
        examples: &["atrius service install"],
        runs: Runs::Local,
    },
    Command {
        name: "service uninstall",
        summary: "Stop the background service and remove its login item.",
        arguments: &[],
        flags: &[],
        examples: &["atrius service uninstall"],
        runs: Runs::Local,
    },
    Command {
        name: "service status",
        summary: "Whether the login item is installed and the background service is running.",
        arguments: &[],
        flags: &[],
        examples: &["atrius service status --json"],
        runs: Runs::Local,
    },
    Command {
        name: "daemon",
        summary: "What the background service runs: hosts every watched folder and never stops on its own. Reads the watch list again every 10 seconds.",
        arguments: &[],
        flags: &[],
        examples: &["atrius daemon"],
        runs: Runs::Local,
    },
    Command {
        name: "doctor",
        summary: "Check this machine: the cache folder, that file events arrive, the Linux inotify limit against the watched folders, and whether embeddings can run.",
        arguments: &[],
        flags: &[],
        examples: &["atrius doctor", "atrius doctor --json"],
        runs: Runs::Local,
    },
    Command {
        name: "languages",
        summary: "The languages the index reads definitions in, and the file extensions each one claims.",
        arguments: &[],
        flags: &[],
        examples: &["atrius languages"],
        runs: Runs::Local,
    },
    Command {
        name: "help",
        summary: "This list, or one command's usage.",
        arguments: &[Argument { name: "command", required: false, rest: true, help: "A command, such as `find` or `watch add`." }],
        flags: &[],
        examples: &["atrius help", "atrius help find"],
        runs: Runs::Local,
    },
];

/// The command with this name.
///
/// @param name - the name, such as `find` or `watch add`
pub fn command(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name == name)
}

/// The commands an MCP client is offered.
pub fn queries() -> impl Iterator<Item = &'static Command> {
    COMMANDS.iter().filter(|c| c.runs == Runs::Query)
}

/// Every flag a command takes, its own and the shared ones.
///
/// @param command - the command
pub fn all_flags(command: &Command) -> Vec<Flag> {
    let mut flags: Vec<Flag> = command.flags.to_vec();
    let reads_a_checkout = command.runs == Runs::Query
        || matches!(command.name, "serve" | "stop" | "mcp");
    if reads_a_checkout {
        flags.push(ROOT);
    }
    flags.push(JSON);
    flags
}

/// One command's usage line, such as `atrius def <name> [--limit <number>] ...`.
///
/// @param command - the command
/// @param program - the program name to start the line with
pub fn usage(command: &Command, program: &str) -> String {
    let mut out = format!("{program} {}", command.name);
    for argument in command.arguments {
        let word = if argument.rest { format!("{}...", argument.name) } else { argument.name.to_owned() };
        if argument.required {
            out.push_str(&format!(" <{word}>"));
        } else {
            out.push_str(&format!(" [{word}]"));
        }
    }
    for flag in all_flags(command) {
        let short = flag.short.map(|c| format!("-{c}|")).unwrap_or_default();
        match flag.takes {
            Takes::Switch => out.push_str(&format!(" [{short}--{}]", flag.name)),
            _ => out.push_str(&format!(" [{short}--{} <{}>]", flag.name, flag.value)),
        }
    }
    out
}
