//! The file set: exactly the files Claude Code's Grep tool searches.
//!
//! That tool runs ripgrep with `--hidden` and `--glob !.git` (and `.svn`, `.hg`, `.bzr`, `.jj`, `.sl`),
//! so hidden files are searched and those six folders are not, and everything else is ripgrep's ignore
//! rules: `.gitignore` (inside a git repository only), `.git/info/exclude`, the global gitignore,
//! `.ignore` and `.rgignore`, read from every folder and from the folders above the root. This walks
//! with the `ignore` crate ripgrep walks with, set the same way, so the two sets cannot drift
//! (`tasks/task-2138-unluminous-code-index-tdd.md` R10).
//!
//! A query can narrow the set the way the Grep tool's own arguments do: a path inside the root, `--glob`
//! patterns and `--type` names. `Scope` is that narrowing, and it uses the same `ignore` types ripgrep
//! builds from those arguments.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use ignore::overrides::{Override, OverrideBuilder};
use ignore::types::{Types, TypesBuilder};
use ignore::{Match, WalkBuilder, WalkState};

/// The version control folders the Grep tool leaves out, in the order it passes them.
pub const VCS_FOLDERS: [&str; 6] = [".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];

/// One file of the set, as the walk found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The path relative to the root, with forward slashes.
    pub rel: String,
    /// The size in bytes when it was walked.
    pub size: u64,
    /// The modification time in nanoseconds since the epoch, or zero when the platform has none.
    pub mtime_ns: i64,
}

/// Walks a root and returns every file in the set, sorted by path.
///
/// @param root - the folder to walk
pub fn walk(root: &Path) -> Vec<Found> {
    walk_under(root, root, None)
}

/// The walk ripgrep does, set up the way the Grep tool's flags set it up, starting at a folder inside
/// the root. The ignore files of every folder above the start are read, so a folder walked on its own
/// gives the same files as the same folder inside a walk of the whole root.
///
/// @param root - the root, which the version control exclusions are relative to
/// @param start - the folder to start at
/// @param depth - how deep to go, None for all the way
pub fn walker(root: &Path, start: &Path, depth: Option<usize>) -> WalkBuilder {
    let mut overrides = OverrideBuilder::new(root);
    for folder in VCS_FOLDERS {
        overrides.add(&format!("!{folder}")).expect("a fixed glob is valid");
    }
    let overrides = overrides.build().expect("fixed globs build");
    let mut builder = WalkBuilder::new(start);
    builder
        .hidden(false)
        .parents(true)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(true)
        .follow_links(false)
        .max_depth(depth)
        .add_custom_ignore_filename(".rgignore")
        .overrides(overrides);
    builder
}

/// Walks the files of the set under one folder of the root, sorted by path.
///
/// @param root - the root
/// @param start - the folder inside the root to walk
/// @param depth - how deep to go, None for all the way
pub fn walk_under(root: &Path, start: &Path, depth: Option<usize>) -> Vec<Found> {
    let found = Mutex::new(Vec::new());
    walker(root, start, depth).build_parallel().run(|| {
        let found = &found;
        Box::new(move |entry| {
            let Ok(entry) = entry else { return WalkState::Continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            if let Some(rel) = relative(root, entry.path()) {
                let meta = entry.metadata().ok();
                let size = meta.as_ref().map_or(0, |m| m.len());
                let mtime_ns = meta.as_ref().map_or(0, mtime_of);
                found.lock().expect("the walk's list").push(Found { rel, size, mtime_ns });
            }
            WalkState::Continue
        })
    });
    let mut found = found.into_inner().expect("the walk finished");
    found.sort_by(|a, b| a.rel.cmp(&b.rel));
    found
}

/// The modification time of a file in nanoseconds since the epoch.
///
/// @param meta - the file's metadata
pub fn mtime_of(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i64)
}

/// A path relative to the root with forward slashes, or None when it is not under the root.
///
/// @param root - the root
/// @param path - a path the walk produced
pub fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let text = rel.to_string_lossy().replace('\\', "/");
    Some(text.trim_start_matches("./").to_owned())
}

/// What a query narrows the file set to: a path inside the root, globs and file types, as the Grep
/// tool's `path`, `glob` and `type` arguments do.
pub struct Scope {
    /// The path the search starts at, relative to the root, without a trailing slash; empty for the root.
    pub path: String,
    /// True when `path` names a file rather than a folder. ripgrep searches a file named on its command
    /// line even when an ignore rule or a glob would leave it out.
    pub explicit_file: bool,
    globs: Option<Override>,
    types: Option<Types>,
}

impl Scope {
    /// The whole root, with nothing narrowing it.
    pub fn everything() -> Scope {
        Scope { path: String::new(), explicit_file: false, globs: None, types: None }
    }

    /// A scope from a query's arguments.
    ///
    /// @param root - the root, which globs are relative to
    /// @param path - the path argument, relative to the root, or empty
    /// @param globs - `--glob` patterns, `!` negating
    /// @param types - `--type` names from ripgrep's default type list
    pub fn new(
        root: &Path,
        path: &str,
        globs: &[String],
        types: &[String],
    ) -> Result<Scope, String> {
        let path = normalise(path);
        let explicit_file = !path.is_empty() && root.join(&path).is_file();
        let globs = if globs.is_empty() {
            None
        } else {
            let mut builder = OverrideBuilder::new(root);
            for glob in globs {
                builder.add(glob).map_err(|e| format!("the glob `{glob}` is not valid: {e}"))?;
            }
            Some(builder.build().map_err(|e| e.to_string())?)
        };
        let types = if types.is_empty() {
            None
        } else {
            let mut builder = TypesBuilder::new();
            builder.add_defaults();
            for name in types {
                builder.select(name);
            }
            Some(builder.build().map_err(|e| e.to_string())?)
        };
        Ok(Scope { path, explicit_file, globs, types })
    }

    /// Whether nothing narrows this scope, so every file of the set is in it.
    pub fn is_everything(&self) -> bool {
        self.path.is_empty() && !self.explicit_file && self.globs.is_none() && self.types.is_none()
    }

    /// Whether a file of the set is inside this scope.
    ///
    /// @param rel - the file's path relative to the root
    pub fn contains(&self, rel: &str) -> bool {
        if self.explicit_file {
            return rel == self.path;
        }
        let under_the_folder =
            rel.starts_with(&self.path) && rel.as_bytes().get(self.path.len()) == Some(&b'/');
        if !(self.path.is_empty() || under_the_folder) {
            return false;
        }
        if let Some(globs) = &self.globs {
            if !passes_globs(globs, rel) {
                return false;
            }
        }
        if let Some(types) = &self.types {
            if !matches!(types.matched(Path::new(rel), false), Match::Whitelist(_)) {
                return false;
            }
        }
        true
    }

    /// The absolute path of the explicit file this scope names, if it names one.
    ///
    /// @param root - the root
    pub fn explicit_path(&self, root: &Path) -> Option<PathBuf> {
        self.explicit_file.then(|| root.join(&self.path))
    }
}

/// Whether a file passes a set of `--glob` overrides the way ripgrep's walk applies them: a folder on
/// its path matched by a negated glob leaves it out, and the file itself must not be ignored.
///
/// @param globs - the built overrides
/// @param rel - the file's path relative to the root
fn passes_globs(globs: &Override, rel: &str) -> bool {
    let mut at = 0;
    while let Some(slash) = rel[at..].find('/') {
        let folder = &rel[..at + slash];
        if globs.matched(Path::new(folder), true).is_ignore() {
            return false;
        }
        at += slash + 1;
    }
    !globs.matched(Path::new(rel), false).is_ignore()
}

/// A path argument as written, made relative and slash separated: `./src/` and `src` are the same.
///
/// @param path - the path argument
pub fn normalise(path: &str) -> String {
    let path = path.replace('\\', "/");
    let mut path = path.trim();
    while let Some(rest) = path.strip_prefix("./") {
        path = rest;
    }
    if path == "." {
        return String::new();
    }
    path.trim_end_matches('/').to_owned()
}
