//! The exact engine gives the same `(path, line)` set as ripgrep, on a fixture tree holding the awkward
//! cases: CRLF line endings, a UTF-16 file with a byte order mark, binary files with matches before and
//! after the NUL, a file over 1 MB, a nested `.gitignore`, hidden files, a folder the root ignores, and
//! a file named as the search path (`tasks/task-2138-unluminous-code-index-tdd.md` §10).
//!
//! The ripgrep compared against is the one Claude Code runs, with the Grep tool's flags. It is found
//! through `ATRIUS_RG`, an `rg` executable such as a hard link named `rg.exe` to `claude.exe`, or
//! `rg` on the path. Without one the test says so and passes, because there is nothing to compare with.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use atrius_index::exact::{Exact, ExactRequest};
use atrius_index::files::Scope;

/// The ripgrep to compare with, if there is one on this machine.
fn ripgrep() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ATRIUS_RG") {
        return Some(PathBuf::from(path));
    }
    Command::new("rg")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| PathBuf::from("rg"))
}

/// Writes the fixture tree into a fresh git repository.
///
/// @param dir - an empty folder
fn fixture(dir: &Path) {
    Command::new("git").args(["init", "--quiet"]).arg(dir).status().expect("git init");
    let write = |rel: &str, bytes: &[u8]| {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    };
    write(".gitignore", b"node_modules/\n*.log\n");
    write(
        "src/main.rs",
        b"fn main() {\n    println!(\"Hello, world\");\n}\nfn helper_thing() {}\n",
    );
    write("src/crlf.txt", b"foo\r\nbar\r\nfoo bar\r\n");
    let mut utf16 = vec![0xFF, 0xFE];
    for unit in "hello world\nsecond line\n".encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    write("src/utf16.txt", &utf16);
    write("bin/early.bin", b"match here\0match after\n");
    let mut late = b"match before\n".repeat(10_000);
    late.extend_from_slice(b"\0match after\n");
    write("bin/late.bin", &late);
    let mut big = Vec::new();
    for i in 0..60_000 {
        big.extend_from_slice(format!("line {i} with some text\n").as_bytes());
    }
    big.extend_from_slice(b"the needle is here\n");
    write("big/large.txt", &big);
    write("nested/.gitignore", b"ignored.txt\n");
    write("nested/ignored.txt", b"Hello hidden by gitignore\n");
    write("nested/kept.txt", b"Hello kept\nUnicode caf\xc3\xa9 and CAF\xc3\x89\n");
    write(".hidden/secret.txt", b"Hello from a hidden folder\n");
    write("node_modules/pkg/index.js", b"Hello from a dependency\n");
    write("debug.log", b"Hello from a log\n");
    write("src/words.txt", b"testing running\nnothing\n");
    // Two files larger than the index ever reads whole (Unluminous `task-2323`): a log that is searched
    // from the disk in pieces, and a disk image read only as far as its first NUL.
    let mut log = Vec::with_capacity(41 * 1024 * 1024);
    let mut row = 0u64;
    while log.len() < 40 * 1024 * 1024 {
        log.extend_from_slice(format!("row {row} of a long log\n").as_bytes());
        row += 1;
    }
    log.extend_from_slice(b"the needle at the end of the log\n");
    write("huge/server.log.txt", &log);
    let mut image = b"match in the image header\n\0".to_vec();
    image.resize(40 * 1024 * 1024, 0);
    image.extend_from_slice(b"\nmatch far past the header\n");
    write("huge/disk.img", &image);
}

/// What ripgrep answers, with the Grep tool's flags, as a set of `path:line`.
///
/// @param rg - the ripgrep executable
/// @param dir - the fixture
/// @param pattern - the regex
/// @param insensitive - `-i`
/// @param path - the search path
fn rg_set(rg: &Path, dir: &Path, pattern: &str, insensitive: bool, path: &str) -> BTreeSet<String> {
    let mut cmd = Command::new(rg);
    cmd.current_dir(dir).args(["--no-config", "--hidden"]);
    for folder in [".git", ".svn", ".hg", ".bzr", ".jj", ".sl"] {
        cmd.args(["--glob", &format!("!{folder}")]);
    }
    cmd.args(["--max-columns", "500"]);
    if insensitive {
        cmd.arg("-i");
    }
    cmd.args(["--json", "-n", "-e", pattern, path]);
    let out = cmd.output().expect("rg runs");
    let mut set = BTreeSet::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let event: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
        if event["type"] == "match" {
            let file =
                event["data"]["path"]["text"].as_str().unwrap_or_default().replace('\\', "/");
            set.insert(format!(
                "{}:{}",
                file.trim_start_matches("./"),
                event["data"]["line_number"]
            ));
        }
    }
    set
}

#[test]
fn every_fixture_search_gives_ripgreps_lines() {
    let Some(rg) = ripgrep() else {
        eprintln!("no ripgrep to compare with: set ATRIUS_RG to one");
        return;
    };
    let dir = tempfile::tempdir().expect("a folder");
    fixture(dir.path());
    let held = Exact::build(dir.path());
    // The same index with its content let go of, as an editor that hosts it runs it (Unluminous
    // `task-2279`): every candidate is read from the disk, and the lines must be ripgrep's all the same.
    atrius_index::exact::set_holding_content(false);
    let mut on_disk = held.clone();
    on_disk.let_go_of_content();
    atrius_index::exact::set_holding_content(true);
    assert!(
        on_disk.files.iter().flatten().all(|r| !r.is_held() || r.size == 0 || r.is_large()),
        "nothing is held"
    );
    let cases: &[(&str, bool, &str)] = &[
        ("Hello", false, "."),
        ("hello", true, "."),
        ("foo$", false, "."),
        ("^bar", false, "."),
        ("match", false, "."),
        (r"\w+ing\b", false, "."),
        ("wor.d", false, "."),
        (r"fn\s+\w+", false, "."),
        ("café", true, "."),
        ("needle", false, "."),
        ("second line", false, "."),
        ("Hello", false, "nested"),
        ("Hello", false, "nested/ignored.txt"),
        ("match", false, "bin/early.bin"),
        ("foo|nothing", false, "."),
        ("x{0}", false, "src"),
    ];
    let mut differences = Vec::new();
    for (index, how) in [(&held, "held"), (&on_disk, "read from the disk")] {
        for &(pattern, insensitive, path) in cases {
            let scope = Scope::new(dir.path(), path, &[], &[]).expect("a scope");
            let request = ExactRequest { pattern, case_insensitive: insensitive, scope: &scope };
            let ours: BTreeSet<String> = index
                .search(dir.path(), &request)
                .expect("searched")
                .hits
                .iter()
                .map(|h| format!("{}:{}", h.path, h.line))
                .collect();
            let theirs = rg_set(&rg, dir.path(), pattern, insensitive, path);
            if ours != theirs {
                differences
                    .push(format!("{how}: {pattern:?} in {path}: index {ours:?}, rg {theirs:?}"));
            }
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

/// A file larger than [`atrius_index::exact::LARGEST_HELD`] is never in memory whole (Unluminous
/// `task-2323`, a window that reached 180 GB): the log is a record with nothing held, the disk image holds
/// only its header, and the whole index is a few megabytes however large the two files are.
#[test]
fn a_file_larger_than_the_limit_is_never_held_whole() {
    let dir = tempfile::tempdir().expect("a folder");
    fixture(dir.path());
    let exact = Exact::build(dir.path());
    let record = |rel: &str| {
        let id = exact.by_path[rel];
        exact.files[id as usize].clone().expect("a live record")
    };
    let log = record("huge/server.log.txt");
    assert!(log.is_large() && log.blocks.is_empty(), "the log holds nothing");
    assert!(log.size > atrius_index::exact::LARGEST_HELD);
    let image = record("huge/disk.img");
    assert!(image.binary && image.is_partial() && !image.is_large());
    assert_eq!(image.bytes(), b"match in the image header\n\0");
    assert!(exact.heap_bytes() < 8 * 1024 * 1024, "the index is {} bytes", exact.heap_bytes());

    let scope = Scope::everything();
    let request =
        ExactRequest { pattern: "needle at the end", case_insensitive: false, scope: &scope };
    let hits = exact.search(dir.path(), &request).expect("searched").hits;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "huge/server.log.txt");
    assert_eq!(hits[0].text, "the needle at the end of the log");
}
