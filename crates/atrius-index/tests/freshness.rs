//! The index answers with what the files hold now, straight after they change.
//!
//! A git checkout is changed in each of the ways an agent and a person change one, and searched with
//! no pause between the change and the query: a new file, an edited line, a rename, a delete, a burst
//! of 2,000 new files, a branch switch, and five rounds of writing and searching. After each, every
//! query must give ripgrep's lines. A stale or missing line is a failure.
//!
//! A folder that is not a git repository is changed the same way, with no branch switch, because its
//! fence is written outside it (Unluminous `task-2370`): nothing named like a fence may appear in it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
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

/// One cache folder for every test in this binary, because the variable naming it is the process's.
fn cache() -> &'static Path {
    static CACHE: OnceLock<tempfile::TempDir> = OnceLock::new();
    let dir = CACHE.get_or_init(|| tempfile::tempdir().unwrap()).path();
    std::env::set_var("ATRIUS_INDEX_CACHE", dir);
    std::env::set_var("ATRIUS_EMBED", "off");
    dir
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
    cache();
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

/// Writes the files every test starts from: 300 modules and the helper they call.
///
/// @param root - the folder
fn write_modules(root: &Path) {
    for i in 0..300 {
        write(root, &format!("pkg{}/module_{i}.rs", i % 12), &format!("/// Module {i}.\npub fn handler_{i}(input: u32) -> u32 {{\n    shared_helper(input) + {i}\n}}\n"));
    }
    write(root, "src/shared.rs", "pub fn shared_helper(x: u32) -> u32 {\n    x * 2\n}\n");
}

/// Lists a folder's whole tree over and over until told to stop, and answers every fence it saw.
///
/// @param root - the folder
/// @param stop - set when the looking should end
fn look_for_fences(root: PathBuf, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<Vec<String>> {
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        while !stop.load(Ordering::SeqCst) {
            let mut folders = vec![root.clone()];
            while let Some(folder) = folders.pop() {
                for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with("atrius-index-fence-") {
                        seen.push(entry.path().display().to_string());
                    } else if entry.file_type().is_ok_and(|t| t.is_dir()) {
                        folders.push(entry.path());
                    }
                }
            }
        }
        seen
    })
}

#[test]
fn a_folder_that_is_not_a_repository_is_fresh_and_never_holds_a_fence() {
    let Some(rg) = ripgrep() else {
        eprintln!("no ripgrep to compare with: set ATRIUS_RG to one");
        return;
    };
    cache();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("folder");
    std::fs::create_dir_all(&root).unwrap();
    write_modules(&root);
    let root = atrius_index::host::canonical(&root);
    let stop = Arc::new(AtomicBool::new(false));
    let looking = look_for_fences(root.clone(), Arc::clone(&stop));

    let index = Index::open_quietly(&root);
    assert!(index.wait_ready(Duration::from_secs(120)), "the index did not become ready");
    let common = ["shared_helper", r"handler_\d+", "Module 1[0-9]", "fresh_token_[a-z]+"];
    let mut failures = differences(&rg, &index, "loading", &common);

    write(&root, "src/new_file.rs", "fn created() { fresh_token_create(); }\n");
    failures.extend(differences(&rg, &index, "a new file", &["fresh_token_create", "fn created"]));

    write(&root, "pkg3/module_3.rs", "/// Module 3, edited.\npub fn handler_3(input: u32) -> u32 {\n    fresh_token_edit(input)\n}\n");
    failures.extend(differences(&rg, &index, "an edited file", &["fresh_token_edit", "handler_3", "Module 3"]));

    std::fs::rename(root.join("pkg4/module_4.rs"), root.join("pkg4/module_four.rs")).unwrap();
    failures.extend(differences(&rg, &index, "a rename", &["handler_4\\b", "Module 4\\."]));

    std::fs::remove_file(root.join("pkg5/module_5.rs")).unwrap();
    failures.extend(differences(&rg, &index, "a delete", &["handler_5\\b"]));

    std::fs::rename(root.join("pkg6"), root.join("pkg_six")).unwrap();
    failures.extend(differences(&rg, &index, "a renamed folder", &[r"handler_\d+", "fresh_token_[a-z]+"]));

    for i in 0..2000 {
        write(&root, &format!("burst/b{}/file_{i}.txt", i % 40), &format!("burst line {i} fresh_token_burst\n"));
    }
    failures.extend(differences(&rg, &index, "a burst of 2,000 files", &["fresh_token_burst", "burst line 1999 "]));

    // Many small bursts, each searched at once: the case where a batch of the root's events can still be
    // waiting for the watcher's thread when the fence is written in a folder watched apart.
    for round in 0..40 {
        for i in 0..50 {
            write(&root, &format!("rounds/r{round}/f{i}.txt"), &format!("round_token_{round} {i}\n"));
        }
        failures.extend(differences(&rg, &index, &format!("burst round {round}"), &[&format!("round_token_{round}\\b")]));
    }

    for round in 0..5 {
        write(&root, "src/loop.rs", &format!("fn round() {{ loop_round_{round}(); }}\n"));
        failures.extend(differences(&rg, &index, &format!("write round {round}"), &[&format!("loop_round_{round}"), "loop_round_\\d"]));
    }
    failures.extend(differences(&rg, &index, "the end", &common));
    stop.store(true, Ordering::SeqCst);
    let fences = looking.join().unwrap();
    assert!(fences.is_empty(), "a fence appeared in the folder: {:?}", &fences[..fences.len().min(5)]);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
