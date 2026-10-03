//! The pages in `docs/`: every page is in the index, every link to a page resolves, and no page uses a
//! phrase `docs/writing-style.md` lists.

use std::path::{Path, PathBuf};

/// The repository's `docs` folder.
fn docs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("docs")
}

/// Every Markdown page under `docs/`, relative to it, with forward slashes.
fn pages() -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![docs()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "md") {
                let rel = path.strip_prefix(docs()).unwrap().to_string_lossy().replace('\\', "/");
                out.push(rel);
            }
        }
    }
    out.sort();
    out
}

/// A page's prose: code blocks, code spans, link targets and parts marked `doc-style: off` removed.
fn prose(text: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    let mut off = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if line.contains("doc-style: off") {
            off = true;
        }
        if line.contains("doc-style: on") {
            off = false;
            continue;
        }
        if fenced || off {
            continue;
        }
        let mut kept = String::new();
        let mut in_code = false;
        for c in line.chars() {
            if c == '`' {
                in_code = !in_code;
                continue;
            }
            if !in_code {
                kept.push(c);
            }
        }
        // Link targets are paths and URLs.
        let mut cleaned = String::new();
        let mut rest = kept.as_str();
        while let Some(at) = rest.find("](") {
            cleaned.push_str(&rest[..at + 1]);
            let after = &rest[at + 2..];
            rest = after.find(')').map(|end| &after[end + 1..]).unwrap_or("");
        }
        cleaned.push_str(rest);
        out.push_str(&cleaned);
        out.push('\n');
    }
    out
}

#[test]
fn every_page_is_in_the_index_and_every_page_link_resolves() {
    let index = std::fs::read_to_string(docs().join("README.md")).unwrap();
    let mut problems = Vec::new();
    for page in pages() {
        if page != "README.md" && !index.contains(&format!("]({page})")) {
            problems.push(format!("{page} is not linked from docs/README.md"));
        }
        let text = std::fs::read_to_string(docs().join(&page)).unwrap();
        let folder = docs().join(&page).parent().unwrap().to_path_buf();
        let mut rest = text.as_str();
        while let Some(at) = rest.find("](") {
            let after = &rest[at + 2..];
            let end = after.find(')').unwrap_or(after.len());
            let target = after[..end].split('#').next().unwrap_or_default();
            if !target.is_empty() && !target.contains("://") && !target.starts_with("mailto:") && !folder.join(target).exists() {
                problems.push(format!("{page} links to {target}, which does not exist"));
            }
            rest = &after[end..];
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn no_page_uses_a_phrase_the_style_guide_lists() {
    let phrases = [
        "\u{2014}", "\u{2013}", " - ", "worth noting", "to be clear", "honestly", "put simply",
        "the whole point", "which is the point", "load bearing", "load-bearing", "first class",
        "first-class", "battle tested", "battle-tested", "the shape of", "vocabulary", "blows up",
        "say the word",
    ];
    // Hyphenated names that are somebody else's, which the style guide allows.
    let names = [
        "B-tree", "nomic-embed-text", "nomic-embed-code", "nomic-embed-text-v1.5", "gte-reranker-modernbert-base",
        "SWE-bench", "claude-context", "cargo-zigbuild", "grep-searcher", "grep-regex", "regex-syntax",
        "zoekt-webserver", "inillucent-driver", "atrius-index", "atrius-cli", "atrius-mcp", "unluminous-cli",
        "Black-Rainbow-Labs", "homebrew-atrius", "black-rainbow-labs", "x86_64-unknown-linux-gnu",
        "lz4", "parent-4.12.0", "gson-parent-2.11.0", "Acc@5", "Recall@5", "arXiv", "notarize",
        "Newtonsoft.Json", "ai-service", "tree-sitter", "nlohmann-json", "newtonsoft-json", "p-value", "Mach-O", "self-signed",
    ];
    let mut problems = Vec::new();
    for page in pages() {
        if page == "commands.md" {
            continue;
        }
        let mut text = prose(&std::fs::read_to_string(docs().join(&page)).unwrap());
        for name in names {
            text = text.replace(name, "");
        }
        for phrase in phrases {
            if text.to_lowercase().contains(&phrase.to_lowercase()) {
                problems.push(format!("{page} uses {phrase:?}"));
            }
        }
        for (number, line) in text.lines().enumerate() {
            let words: Vec<&str> = line.split(|c: char| c.is_whitespace() || "()[]|,.;:!?\"'*".contains(c)).collect();
            for word in words {
                let hyphen = word.find('-');
                let joins_words = hyphen.is_some_and(|at| at > 0 && at + 1 < word.len())
                    && word.split('-').all(|part| part.chars().any(char::is_alphabetic) && part.chars().all(|c| c.is_alphabetic()));
                if joins_words {
                    problems.push(format!("{page}:{}: {word}", number + 1));
                }
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
