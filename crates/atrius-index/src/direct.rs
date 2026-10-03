//! The direct scan: an exact search with no index at all, walking the scope and verifying every file.
//!
//! It answers when the index cannot: before the host has reconciled after starting, while a mass change
//! (a branch switch, a build writing thousands of files) is still being reindexed, and in the CLI when no
//! host could be started. It gives the same results as the index because it uses the same walk and the
//! same verification; it is only slower, since it reads every file, which is what ripgrep does on every
//! call (`tasks/task-2138-unluminous-code-index-tdd.md` §6.4).

use std::path::Path;

use grep_searcher::BinaryDetection;
use rayon::prelude::*;

use crate::exact::{matcher, verify, ExactAnswer, ExactRequest, Hit};
use crate::files;

/// Runs one exact search by walking the root and verifying every file in scope.
///
/// @param root - the root
/// @param request - the pattern, case and scope
pub fn scan(root: &Path, request: &ExactRequest) -> Result<ExactAnswer, String> {
    let matcher = matcher(request.pattern, request.case_insensitive)?;
    if let Some(path) = request.scope.explicit_path(root) {
        let bytes = std::fs::read(&path).unwrap_or_default();
        let hits = verify(&matcher, &request.scope.path, &bytes, BinaryDetection::convert(b'\x00'));
        return Ok(ExactAnswer { hits, in_scope: 1, candidates: 1, verified: 1, unbounded: true });
    }
    let found: Vec<files::Found> =
        files::walk(root).into_iter().filter(|f| request.scope.contains(&f.rel)).collect();
    let mut hits: Vec<Hit> = found
        .par_iter()
        .flat_map_iter(|f| {
            let bytes = std::fs::read(root.join(&f.rel)).unwrap_or_default();
            verify(&matcher, &f.rel, &bytes, BinaryDetection::quit(b'\x00'))
        })
        .collect();
    hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    Ok(ExactAnswer {
        hits,
        in_scope: found.len(),
        candidates: found.len(),
        verified: found.len(),
        unbounded: true,
    })
}
