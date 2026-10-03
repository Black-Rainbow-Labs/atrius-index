//! `atrius`: the command line for the Atrius code index. `atrius help` lists the commands.

fn main() {
    let words: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(atrius_cli::run::main(&words));
}
