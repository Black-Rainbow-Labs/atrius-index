//! `docs/commands.md`, written from the catalogue.
//!
//! `atrius-cli`'s test `commands_page_is_current` fails when the committed page differs from what this
//! writes, and `cargo run -p atrius-cli --bin atrius -- help --markdown > docs/commands.md` writes it.

use crate::catalogue::{self, Runs, Takes};

/// The command reference page.
pub fn commands_page() -> String {
    let mut out = String::from(
        "# Command reference\n\nEvery command `atrius` has, with its arguments and flags. This page is written from the command\ntable in `crates/atrius-cli/src/catalogue.rs` by `atrius help --markdown`, and a test fails when the\ntwo differ.\n\nThe query commands are also the commands of the MCP tool `atrius_search`. [MCP](mcp.md) explains how\nan agent calls them.\n\n",
    );
    out.push_str("| Command | What it does |\n|---|---|\n");
    for command in catalogue::COMMANDS {
        out.push_str(&format!("| [`{}`](#{}) | {} |\n", command.name, anchor(command.name), command.summary));
    }
    out.push('\n');
    for (title, runs) in [("Query commands", Runs::Query), ("Commands that run locally", Runs::Local)] {
        out.push_str(&format!("## {title}\n\n"));
        if runs == Runs::Query {
            out.push_str("A query is answered by the index host of the checkout. The first query starts one, and it stays\nrunning for two hours after the last query.\n\n");
        }
        for command in catalogue::COMMANDS.iter().filter(|c| c.runs == runs) {
            out.push_str(&format!("### {}\n\n{}\n\n```\n{}\n```\n\n", command.name, command.summary, catalogue::usage(command, "atrius")));
            if !command.arguments.is_empty() {
                out.push_str("| Argument | What it is |\n|---|---|\n");
                for argument in command.arguments {
                    let need = if argument.required { "" } else { " Optional." };
                    out.push_str(&format!("| `{}` | {}{need} |\n", argument.name, argument.help));
                }
                out.push('\n');
            }
            let flags = catalogue::all_flags(command);
            out.push_str("| Flag | What it does |\n|---|---|\n");
            for flag in flags {
                let short = flag.short.map(|c| format!("`-{c}`, ")).unwrap_or_default();
                let shown = match flag.takes {
                    Takes::Switch => format!("{short}`--{}`", flag.name),
                    _ => format!("{short}`--{} <{}>`", flag.name, flag.value),
                };
                out.push_str(&format!("| {shown} | {} |\n", flag.help));
            }
            out.push_str("\nExamples:\n\n```sh\n");
            for example in command.examples {
                out.push_str(example);
                out.push('\n');
            }
            out.push_str("```\n\n");
        }
    }
    out.push_str("## Exit codes\n\n| Code | Meaning |\n|---|---|\n| 0 | The command was carried out |\n| 1 | The command was refused: a folder or file that is not there, a pattern that does not compile, a host that would not start |\n| 2 | The command line was wrong: an unknown command, a missing argument, an unknown flag |\n");
    out
}

/// A heading's anchor as GitHub writes it.
///
/// @param name - the heading text
fn anchor(name: &str) -> String {
    name.to_lowercase().replace(' ', "-")
}
