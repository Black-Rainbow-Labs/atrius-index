//! Ranking file paths against what an agent typed to find a file: a glob, as the Glob tool takes, or a
//! fragment of a name, as `Go to File` takes.
//!
//! A glob is matched the way the Glob tool matches it, and the matches are ranked shortest path first,
//! because a glob says nothing about which match was wanted and the shallow file is usually it. A
//! fragment is scored: the file's own name equal to it, then starting with it, then containing it, then
//! the path containing it, then the letters in order; ties go to the shorter path.

use globset::GlobBuilder;

/// The best `limit` paths for a query.
///
/// @param paths - every path of the file set, relative to the root
/// @param query - a glob or a name fragment
/// @param limit - how many to return
pub fn rank(paths: &[String], query: &str, limit: usize) -> Vec<String> {
    let query = query.trim().replace('\\', "/");
    if query.contains(['*', '?', '[', '{']) {
        return by_glob(paths, &query, limit);
    }
    let needle = query.to_lowercase();
    let mut scored: Vec<(u32, usize, &String)> =
        paths.iter().filter_map(|p| score(p, &needle).map(|s| (s, p.len(), p))).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(b.2)));
    scored.into_iter().take(limit).map(|(_, _, p)| p.clone()).collect()
}

/// Paths matching a glob, shortest first. A glob with no slash matches a file's name anywhere, as the
/// Glob tool's `**/` prefix does.
///
/// @param paths - the paths
/// @param glob - the glob
/// @param limit - how many to return
fn by_glob(paths: &[String], glob: &str, limit: usize) -> Vec<String> {
    let pattern = if glob.contains('/') {
        glob.trim_start_matches("./").to_owned()
    } else {
        format!("**/{glob}")
    };
    let Ok(matcher) = GlobBuilder::new(&pattern)
        .case_insensitive(true)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher())
    else {
        return Vec::new();
    };
    let mut found: Vec<&String> = paths.iter().filter(|p| matcher.is_match(p.as_str())).collect();
    found.sort_by(|a, b| {
        a.matches('/')
            .count()
            .cmp(&b.matches('/').count())
            .then(is_entry_point(b).cmp(&is_entry_point(a)))
            .then(a.len().cmp(&b.len()))
            .then(a.cmp(b))
    });
    found.into_iter().take(limit).cloned().collect()
}

/// Whether a file is the entry point of its folder: `lib`, `main`, `mod` or `index` with any extension,
/// or `__init__.py`. Among files at one depth that a glob matches, these are the ones read first.
///
/// @param path - the path
fn is_entry_point(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let stem = name.split('.').next().unwrap_or(&name);
    matches!(stem, "lib" | "main" | "mod" | "index") || name == "__init__.py"
}

/// How well a path matches a fragment, or None when it does not match at all.
///
/// @param path - the path
/// @param needle - the fragment, lower case
fn score(path: &str, needle: &str) -> Option<u32> {
    let lower = path.to_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let stem = name.split('.').next().unwrap_or(name);
    if name == needle || stem == needle {
        return Some(1000);
    }
    if name.starts_with(needle) {
        return Some(800);
    }
    if name.contains(needle) {
        return Some(600);
    }
    if lower.contains(needle) {
        return Some(400);
    }
    let mut letters = needle.chars().filter(|c| *c != ' ');
    let mut want = letters.next();
    for c in lower.chars() {
        if Some(c) == want {
            want = letters.next();
        }
    }
    want.is_none().then_some(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_files_own_name_beats_a_folder_with_the_name() {
        let paths = vec![
            "src/layout/mod.rs".to_owned(),
            "src/layout.rs".to_owned(),
            "docs/layout-notes.md".to_owned(),
        ];
        assert_eq!(rank(&paths, "layout", 3)[0], "src/layout.rs");
        assert_eq!(rank(&paths, "*.md", 3), ["docs/layout-notes.md"]);
    }
}
