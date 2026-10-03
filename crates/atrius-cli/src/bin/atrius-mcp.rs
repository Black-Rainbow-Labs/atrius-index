//! `atrius-mcp`: the MCP server for the Atrius code index, on standard input and output.
//!
//! `atrius-mcp --root <folder>` answers for that checkout; without it, for `ATRIUS_ROOT`,
//! `CLAUDE_PROJECT_DIR`, or the git checkout at or above the working folder. `atrius-mcp serve --root
//! <folder>` runs a headless index host instead, which is what a one off `atrius` query starts when
//! only this program is installed.

fn main() {
    let words: Vec<String> = std::env::args().skip(1).collect();
    match words.first().map(String::as_str) {
        Some("--version") | Some("-V") => {
            println!("atrius-mcp {}", atrius_index::VERSION);
        }
        Some("--help") | Some("-h") => {
            println!("atrius-mcp {}: the MCP server for the Atrius code index\n\nUsage: atrius-mcp [--root <folder>]\n\nIt speaks MCP on standard input and output and offers one tool, `atrius_search`.\nRegister it with an agent, for example: claude mcp add atrius -- atrius-mcp", atrius_index::VERSION);
        }
        Some("serve") => std::process::exit(atrius_cli::run::main(&words)),
        _ => {
            let root = match words.as_slice() {
                [flag, value, ..] if flag == "--root" => Some(value.clone()),
                _ => words.iter().find_map(|w| w.strip_prefix("--root=").map(str::to_owned)),
            };
            std::process::exit(atrius_cli::mcp::serve(root.as_deref()));
        }
    }
}
