//! The index answers with what the files hold now, straight after they change.
//!
//! A git checkout is changed in each of the ways an agent and a person change one, and searched with
//! no pause between the change and the query: a new file, an edited line, a rename, a delete, a burst
//! of 2,000 new files, a branch switch, and five rounds of writing and searching. After each, every
//! query must give ripgrep's lines. A stale or missing line is a failure.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use atrius_index::exact::ExactRequest;
use atrius_index::files::Scope;
use atrius_index::index::Index;

/// The ripgrep to compare with.
fn ripgrep() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ATRIUS_RG") {
        return Some(PathBuf::from(path));
    }
    Command::new("rg").arg("--version").output().ok().filter(|o| o.status.success()).map(|_| PathBuf::from("rg"))
}

/// Runs git in a folder.
fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git").current_dir(root).args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "core.autocrlf=false"]).args(args).output().expect("git runs");
    assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
}

/// Writes a file, creating its folder.
fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// ripgrep's lines for a pattern, as `path:line`.
fn rg_set(rg: &Path, root: &Path, pattern: &str) -> BTreeSet<String> {
    let out = Command::new(rg)
        .current_dir(root)
        .args(["--no-config", "--hidden", "--glob", "!.git", "-n", "--no-heading", "--with-filename", "-e", pattern, "."])
        .output()
        .expect("rg runs");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let l = l.replace('\\', "/");
            let l = l.trim_start_matches("./");
            let mut parts = l.splitn(3, ':');
            Some(format!("{}:{}", parts.next()?, parts.next()?))
        })
        .collect()
}

/// The index's lines for a pattern, as `path:line`.
fn index_set(index: &Index, pattern: &str) -> BTreeSet<String> {
    let scope = Scope::everything();
    let request = ExactRequest { pattern, case_insensitive: false, scope: &scope };
    let (answer, _) = index.exact(&request).expect("searched");
    answer.hits.iter().map(|h| format!("{}:{}", h.path, h.line)).collect()
}

/// Checks a list of patterns against ripgrep and answers what differs.
fn differences(rg: &Path, index: &Index, step: &str, patterns: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for pattern in patterns {
        let ours = index_set(index, pattern);
        let theirs = rg_set(rg, index.root(), pattern);
        if ours != theirs {
            out.push(format!("after {step}, {pattern:?}: index {} lines, rg {} lines; only index {:?}; only rg {:?}", ours.len(), theirs.len(), ours.difference(&theirs).take(3).collect::<Vec<_>>(), theirs.difference(&ours).take(3).collect::<Vec<_>>()));
        }
    }
    out
}

#[test]
fn every_kind_of_change_is_seen_by_the_next_query() {
    let Some(rg) = ripgrep() else {
        eprintln!("no ripgrep to compare with: set ATRIUS_RG to one");
        return;
    };
    let cache = tempfile::tempdir().unwrap();
    std::env::set_var("ATRIUS_INDEX_CACHE", cache.path());
    std::env::set_var("ATRIUS_EMBED", "off");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("checkout");
    std::fs::create_dir_all(&root).unwrap();
    for i in 0..300 {
        write(&root, &format!("pkg{}/module_{i}.rs", i % 12), &format!("/// Module {i}.\npub fn handler_{i}(input: u32) -> u32 {{\n    shared_helper(input) + {i}\n}}\n"));
    }
    write(&root, "src/shared.rs", "pub fn shared_helper(x: u32) -> u32 {\n    x * 2\n}\n");
    write(&root, ".gitignore", "target/\n");
    git(&root, &["init", "--quiet", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "--quiet", "-m", "base"]);
    git(&root, &["checkout", "--quiet", "-b", "other"]);
    write(&root, "src/shared.rs", "pub fn shared_helper(x: u32) -> u32 {\n    x * 3 // branch_other_marker\n}\n");
    std::fs::rename(root.join("pkg0/module_0.rs"), root.join("pkg0/renamed_on_branch.rs")).unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "--quiet", "-m", "other"]);
    git(&root, &["checkout", "--quiet", "main"]);

    let root = atrius_index::host::canonical(&root);
    let index = Index::open_quietly(&root);
    assert!(index.wait_ready(Duration::from_secs(120)), "the index did not become ready");
    let common = ["shared_helper", r"handler_\d+", "Module 1[0-9]", "branch_other_marker", "fresh_token_[a-z]+"];
    let mut failures = differences(&rg, &index, "loading", &common);

    write(&root, "src/new_file.rs", "fn created() { fresh_token_create(); }\n");
    failures.extend(differences(&rg, &index, "a new file", &["fresh_token_create", "fn created"]));

    // Unluminous `task-2247`: a change deep inside an ignored folder was walked from that folder, where
    // the rule naming the folder above it is never asked, so a build's whole output was indexed.
    write(&root, "target/debug/deps/artifact.rs", "fn built() { build_output_token(); }\n");
    write(&root, "target/release/build/out.txt", "build_output_token again\n");
    failures.extend(differences(&rg, &index, "a build writing into an ignored folder", &["build_output_token", "fn built"]));

    write(&root, "pkg3/module_3.rs", "/// Module 3, edited.\npub fn handler_3(input: u32) -> u32 {\n    fresh_token_edit(input)\n}\n");
    failures.extend(differences(&rg, &index, "an edited file", &["fresh_token_edit", "handler_3", "Module 3"]));

    std::fs::rename(root.join("pkg4/module_4.rs"), root.join("pkg4/module_four.rs")).unwrap();
    failures.extend(differences(&rg, &index, "a rename", &["handler_4\\b", "Module 4\\."]));

    std::fs::remove_file(root.join("pkg5/module_5.rs")).unwrap();
    failures.extend(differences(&rg, &index, "a delete", &["handler_5\\b"]));

    for i in 0..2000 {
        write(&root, &format!("burst/b{}/file_{i}.txt", i % 40), &format!("burst line {i} fresh_token_burst\n"));
    }
    failures.extend(differences(&rg, &index, "a burst of 2,000 files", &["fresh_token_burst", "burst line 1999 "]));

    // The burst is committed so the branch switch below is clean.
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "--quiet", "-m", "burst"]);
    git(&root, &["checkout", "--quiet", "other"]);
    failures.extend(differences(&rg, &index, "a branch switch", &["branch_other_marker", "x \\* 3", "handler_0\\b", "fresh_token_[a-z]+"]));
    git(&root, &["checkout", "--quiet", "main"]);
    failures.extend(differences(&rg, &index, "switching back", &["branch_other_marker", "x \\* 2", "fresh_token_[a-z]+"]));

    for round in 0..5 {
        write(&root, "src/loop.rs", &format!("fn round() {{ loop_round_{round}(); }}\n"));
        failures.extend(differences(&rg, &index, &format!("write round {round}"), &[&format!("loop_round_{round}"), "loop_round_\\d"]));
    }
    failures.extend(differences(&rg, &index, "the end", &common));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
