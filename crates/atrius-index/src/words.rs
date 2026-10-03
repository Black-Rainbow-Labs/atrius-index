//! Search words for code (`tasks/task-2138-unluminous-code-index-tdd.md` §6.6).
//!
//! Inillucent's `porter` tokenizer already splits `snake_case` and keeps the whole word (R3), but leaves
//! `camelCase`, `PascalCase`, letters beside digits and `kebab-case` whole. So for each identifier that
//! has one of those inside it, the whole identifier in lower case and its parts are written into the
//! passage's `words` column, and a question is split the same way, so `resolveSkipToken` in the code
//! and "resolve the skip token" in a question meet on `resolve`, `skip` and `token`.

/// The parts of one identifier at case changes, digits and hyphens, in lower case. A run of capitals
/// followed by a lower case letter splits before the last capital, so `HTTPServer` is `http server`.
///
/// @param word - the identifier
pub fn parts(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    for i in 0..chars.len() {
        let c = chars[i];
        if c == '_' || c == '-' || c == '.' {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        let previous = if i > 0 { Some(chars[i - 1]) } else { None };
        let next = chars.get(i + 1).copied();
        let boundary = match previous {
            Some(p) if p.is_lowercase() && c.is_uppercase() => true,
            Some(p)
                if p.is_uppercase() && c.is_uppercase() && next.is_some_and(char::is_lowercase) =>
            {
                true
            }
            Some(p)
                if p.is_ascii_digit() != c.is_ascii_digit()
                    && (p.is_alphanumeric() && c.is_alphanumeric()) =>
            {
                true
            }
            _ => false,
        };
        if boundary && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Whether an identifier needs splitting beyond what the engine does: it changes case inside, mixes
/// letters and digits, or has a hyphen.
///
/// @param word - the identifier
fn needs_splitting(word: &str) -> bool {
    let chars: Vec<char> = word.chars().collect();
    word.contains('-')
        || chars.windows(2).any(|w| {
            (w[0].is_lowercase() && w[1].is_uppercase())
                || (w[0].is_ascii_digit() != w[1].is_ascii_digit())
        })
}

/// The extra search words for a text: each identifier that needs splitting, whole and in parts, once.
///
/// @param text - the text, code or prose
pub fn search_words(text: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut out = String::new();
    for word in identifiers(text) {
        if !needs_splitting(word) || !seen.insert(word) {
            continue;
        }
        out.push_str(&word.to_lowercase().replace('-', "_"));
        for part in parts(word) {
            out.push(' ');
            out.push_str(&part);
        }
        out.push(' ');
    }
    out
}

/// Every identifier in a text: a letter or underscore, then letters, digits, underscores and inner
/// hyphens.
///
/// @param text - the text
pub fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    let bytes = text.as_bytes();
    let mut at = 0usize;
    std::iter::from_fn(move || {
        while at < bytes.len() {
            let b = bytes[at];
            if b.is_ascii_alphabetic() || b == b'_' {
                let start = at;
                at += 1;
                while at < bytes.len()
                    && (bytes[at].is_ascii_alphanumeric()
                        || bytes[at] == b'_'
                        || (bytes[at] == b'-'
                            && at + 1 < bytes.len()
                            && bytes[at + 1].is_ascii_alphabetic()))
                {
                    at += 1;
                }
                return Some(&text[start..at]);
            }
            at += 1;
        }
        None
    })
}

/// The words of a question as a query: each identifier and plain word in lower case, identifiers also in
/// parts, without repeats and without the words every question has.
///
/// @param question - the question
pub fn query_words(question: &str) -> Vec<String> {
    const COMMON: &[&str] = &[
        "the", "a", "an", "of", "to", "in", "is", "it", "and", "or", "for", "on", "with", "where",
        "what", "how", "do", "does", "we", "our", "which", "when", "that", "this", "be", "by",
        "from", "are", "as", "at", "get", "gets", "code", "function", "file", "there",
    ];
    let mut out: Vec<String> = Vec::new();
    let mut push = |w: String| {
        if w.len() > 1 && !COMMON.contains(&w.as_str()) && !out.contains(&w) {
            out.push(w);
        }
    };
    for word in identifiers(question) {
        push(word.to_lowercase().replace('-', "_"));
        if needs_splitting(word) || word.contains('_') {
            for part in parts(word) {
                push(part);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_split_at_case_digits_and_hyphens() {
        assert_eq!(parts("resolveSkipToken"), ["resolve", "skip", "token"]);
        assert_eq!(parts("HTTPServer"), ["http", "server"]);
        assert_eq!(parts("utf16Decode"), ["utf", "16", "decode"]);
        assert_eq!(parts("unluminous-cli"), ["unluminous", "cli"]);
        assert_eq!(
            search_words("let x = resolveSkipToken(a);"),
            "resolveskiptoken resolve skip token "
        );
        assert_eq!(search_words("max_retry_count"), "", "the engine splits snake case itself");
    }

    #[test]
    fn a_question_keeps_its_content_words() {
        assert_eq!(
            query_words("Where do we retry a failed board write?"),
            ["retry", "failed", "board", "write"]
        );
    }
}
