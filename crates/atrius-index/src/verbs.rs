//! The `search` area's verbs, answered from an `Index` (`tasks/task-2138-unluminous-code-index-tdd.md`
//! §6.10). The window and the headless host both call these, so a search typed into the command line, a
//! search an agent makes through MCP and a search the window answers are one function.
//!
//! Each verb takes the request's arguments as the catalogue names them and returns the reply's `result`:
//! structured fields for a program, and `text`, which is what the command line prints and what an agent
//! is shown.

use serde_json::{json, Map, Value};

use crate::exact::ExactRequest;
use crate::files::Scope;
use crate::index::Index;
use crate::shape::{self, DEFAULT_BUDGET};

/// Why a verb refused, with the protocol's code.
#[derive(Debug)]
pub struct Refusal {
    /// One of the protocol's codes: `usage`, `not-found`, `failed`.
    pub code: &'static str,
    /// What to tell the caller.
    pub message: String,
}

/// A refusal with the `usage` code.
///
/// @param message - what was wrong with the request
fn usage(message: impl Into<String>) -> Refusal {
    Refusal { code: "usage", message: message.into() }
}

/// A text argument.
///
/// @param args - the request's arguments
/// @param name - the argument's name
fn text<'a>(args: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    args.get(name).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// A whole number argument, given as a number or as text.
///
/// @param args - the request's arguments
/// @param name - the argument's name
fn whole(args: &Map<String, Value>, name: &str) -> Option<usize> {
    match args.get(name)? {
        Value::Number(n) => n.as_u64().map(|n| n as usize),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// A switch argument: present and not `false`.
///
/// @param args - the request's arguments
/// @param name - the argument's name
fn switch(args: &Map<String, Value>, name: &str) -> bool {
    match args.get(name) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s != "false",
        Some(Value::Null) | None => false,
        Some(_) => true,
    }
}

/// Splits a `glob` argument the way the Grep tool does: on white space, and on commas outside braces.
///
/// @param glob - the argument as given
pub fn split_globs(glob: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in glob.split_whitespace() {
        if word.contains('{') && word.contains('}') {
            out.push(word.to_owned());
        } else {
            out.extend(word.split(',').filter(|s| !s.is_empty()).map(str::to_owned));
        }
    }
    out
}

/// Whether a query reads as a regular expression rather than as plain text.
///
/// @param query - the query
fn looks_like_regex(query: &str) -> bool {
    query.contains(['\\', '^', '$', '|', '*', '+', '?', '(', ')', '[', ']', '{', '}'])
        || query.contains(".*")
}

/// Answers one verb.
///
/// @param index - the checkout's index
/// @param verb - the verb, such as `find`
/// @param args - the request's arguments
pub fn answer(index: &Index, verb: &str, args: &Map<String, Value>) -> Result<Value, Refusal> {
    let (resolved, note) = resolve_path(index, args);
    let args = &resolved;
    let mut value = match verb {
        "find" => find(index, args),
        "files" => files(index, args),
        "def" => def(index, args),
        "refs" => refs(index, args),
        "outline" => outline(index, args),
        "fragment" => fragment(index, args),
        "status" => Ok(status(index)),
        other => Err(Refusal {
            code: "unknown-command",
            message: format!("There is no search verb called `{other}`."),
        }),
    }?;
    // Says the caller asked for the structured result, which the MCP server only sends when asked:
    // an agent is shown the text, a program asks for the fields (`mcp::server::tool_result`).
    if switch(args, "structured") || verb == "status" {
        value["structured"] = Value::Bool(true);
    }
    if let Some(note) = note {
        let text = value["text"].as_str().unwrap_or_default().to_owned();
        value["text"] = Value::String(format!("{note}\n{text}"));
    }
    Ok(value)
}

/// A `path` that names no folder or file at the root, read as the one folder or file in the project
/// whose path ends with it, such as `components` for `crates/unluminous-app/src/components`. In the
/// fifth dev agent run an agent asked `files` under `components` three times and was told three times
/// that no file matched. When several end with it, the path is left as it is and the note names them.
///
/// @param index - the index
/// @param args - the call's arguments
fn resolve_path(index: &Index, args: &Map<String, Value>) -> (Map<String, Value>, Option<String>) {
    let mut out = args.clone();
    let Some(given) = text(args, "path")
        .map(crate::files::normalise)
        .filter(|g| !g.is_empty() && !g.contains(".."))
    else {
        return (out, None);
    };
    if index.root().join(&given).exists() || std::path::Path::new(&given).is_absolute() {
        return (out, None);
    }
    let paths = index
        .with_exact(|exact| exact.by_path.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let suffix = format!("/{given}");
    let mut found: Vec<String> = Vec::new();
    for path in &paths {
        let mut at = path.as_str();
        loop {
            if (at == given || at.ends_with(&suffix)) && !found.iter().any(|f| f == at) {
                found.push(at.to_owned());
            }
            match at.rfind('/') {
                Some(cut) => at = &at[..cut],
                None => break,
            }
        }
        if found.len() > 5 {
            break;
        }
    }
    // Of several, the one that holds code is what a search of code means: `components` is both the
    // window's components and a folder of design pictures.
    // Of several, one that holds three times as many files as any other is the one meant: `components`
    // is both the window's components and a small folder of design pictures.
    if found.len() > 1 {
        let holds =
            |folder: &str| paths.iter().filter(|p| p.starts_with(&format!("{folder}/"))).count();
        let mut counted: Vec<(usize, String)> =
            found.iter().map(|f| (holds(f), f.clone())).collect();
        counted.sort_by_key(|c| std::cmp::Reverse(c.0));
        if counted[0].0 > 0 && counted[0].0 >= 3 * counted[1].0 {
            found = vec![counted[0].1.clone()];
        }
    }
    match found.as_slice() {
        [only] => {
            out.insert("path".into(), Value::String(only.clone()));
            (out, Some(format!("(path `{given}` read as `{only}`)")))
        }
        [] => (out, None),
        several => (
            out,
            Some(format!(
                "(no `{given}` at the root; did you mean {}?)",
                several.iter().map(|p| format!("`{p}`")).collect::<Vec<_>>().join(", ")
            )),
        ),
    }
}

/// What `search find` was asked, read out of its arguments.
struct FindRequest {
    query: String,
    auto: bool,
    semantic: bool,
    regex: bool,
    pattern: String,
    scope: Scope,
    budget: usize,
    case_insensitive: bool,
}

/// Reads `search find`'s arguments.
///
/// @param root - the root, which a path and globs are relative to
/// @param args - query, mode, path, glob, type, ignore-case, budget
fn read_find(root: &std::path::Path, args: &Map<String, Value>) -> Result<FindRequest, Refusal> {
    // The query exactly as given: a pattern's leading or trailing space is part of what it matches, so
    // `url ` and `url` are different searches (two F2 queries end in a space).
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .filter(|q| !q.trim().is_empty())
        .ok_or_else(|| usage("search find needs a query."))?
        .to_owned();
    let mode = text(args, "mode").unwrap_or("auto");
    let regex = match mode {
        "regex" => true,
        "literal" | "semantic" | "symbol" => false,
        "auto" => looks_like_regex(&query),
        other => {
            return Err(usage(format!(
            "`{other}` is not a mode this version has: auto, literal, regex, symbol or semantic."
        )))
        }
    };
    let pattern = if regex { query.clone() } else { regex::escape(&query) };
    let globs = text(args, "glob").map(split_globs).unwrap_or_default();
    let types: Vec<String> = text(args, "type").map(|t| vec![t.to_owned()]).unwrap_or_default();
    let scope =
        Scope::new(root, text(args, "path").unwrap_or(""), &globs, &types).map_err(usage)?;
    let budget = whole(args, "budget").unwrap_or(DEFAULT_BUDGET);
    Ok(FindRequest {
        auto: mode == "auto",
        semantic: mode == "semantic",
        query,
        regex,
        pattern,
        scope,
        budget,
        case_insensitive: switch(args, "ignore-case"),
    })
}

/// `search find`: an exact search, literal or regex, through the gate. In `auto` mode one identifier
/// that the symbol table knows is answered with its definitions first and then its uses.
///
/// @param index - the index
/// @param args - query, mode, path, glob, type, ignore-case, budget
fn find(index: &Index, args: &Map<String, Value>) -> Result<Value, Refusal> {
    let asked = read_find(index.root(), args)?;
    if asked.semantic || (asked.auto && is_sentence(&asked.query)) {
        return semantic(index, &asked);
    }
    if (asked.auto || text(args, "mode") == Some("symbol"))
        && !asked.case_insensitive
        && is_identifier(&asked.query)
    {
        if let Some(value) = find_symbol(index, &asked)? {
            return Ok(value);
        }
    }
    let request = ExactRequest {
        pattern: &asked.pattern,
        case_insensitive: asked.case_insensitive,
        scope: &asked.scope,
    };
    let started = std::time::Instant::now();
    let (found, engine) =
        index.exact(&request).map_err(|e| usage(format!("The pattern was refused: {e}")))?;
    // Words that are not a regex and are not in the code literally are a question about meaning.
    if asked.auto
        && !asked.regex
        && found.hits.is_empty()
        && asked.query.split_whitespace().count() > 1
    {
        return semantic(index, &asked);
    }
    let mut value = found_value(&asked, &found, engine, started.elapsed(), index.last_gate());
    let files: std::collections::BTreeSet<&str> =
        found.hits.iter().map(|h| h.path.as_str()).collect();
    if asked.budget > 0
        && !found.hits.is_empty()
        && found.hits.len() <= INLINE_HITS
        && files.len() <= 2
    {
        let first = &found.hits[0];
        let shown = value["text"].as_str().unwrap_or_default().to_owned();
        if let Some(code) = inline_code(
            index,
            &first.path,
            first.line as u32,
            asked.budget.saturating_sub(shape::tokens(&shown)),
        ) {
            value["text"] = Value::String(format!(
                "{shown}\nthe code around {}:{}:\n{code}",
                first.path, first.line
            ));
            value["inlined"] = json!([first.path, first.line]);
        }
    }
    Ok(value)
}

/// How many chunks a plain English answer shows as text.
const SEMANTIC_SHOWN: usize = 5;

/// The most files a `files` answer may list and still carry the best one's outline.
const FILES_OUTLINED: usize = 3;

/// The largest outline a `files` answer carries, in tokens.
const OUTLINE_TOKENS: usize = 800;

/// The most hits an exact answer may have and still carry the code around its first one.
const INLINE_HITS: usize = 5;

/// The longest definition an answer carries whole, in lines.
const INLINE_LINES: u32 = 60;

/// The code around a line, as `fragment` gives it, when it is at most `INLINE_LINES` long and fits in
/// the tokens left. Measured on the dev agent run: 154 of the index arm's 290 Read calls opened a file
/// the search just before them had named, 93 of them straight after a `find`, to see the code around a
/// hit. When the answer is small, it carries that code, and the turn and the Read are not needed.
///
/// @param index - the index
/// @param rel - the file, relative to the root
/// @param line - the line
/// @param room - the tokens left in the answer's budget
fn inline_code(index: &Index, rel: &str, line: u32, room: usize) -> Option<String> {
    let mut args = Map::new();
    args.insert("target".into(), Value::String(format!("{rel}:{line}")));
    let around = fragment(index, &args).ok()?;
    let (start, end) = (around["start"].as_u64()?, around["end"].as_u64()?);
    let text = around["text"].as_str()?;
    (end + 1 - start <= u64::from(INLINE_LINES) && shape::tokens(text) <= room)
        .then(|| text.to_owned())
}

/// `search find` with no index at all, by scanning the files, for a caller that could reach no host.
///
/// @param root - the root
/// @param args - the same arguments `find` takes
pub fn scan_without_index(
    root: &std::path::Path,
    args: &Map<String, Value>,
) -> Result<Value, Refusal> {
    let asked = read_find(root, args)?;
    let request = ExactRequest {
        pattern: &asked.pattern,
        case_insensitive: asked.case_insensitive,
        scope: &asked.scope,
    };
    let started = std::time::Instant::now();
    let found = crate::direct::scan(root, &request)
        .map_err(|e| usage(format!("The pattern was refused: {e}")))?;
    Ok(found_value(&asked, &found, "none", started.elapsed(), std::time::Duration::ZERO))
}

/// The reply of an exact search: counts, the hits shown, the work done, and the shaped text.
///
/// @param asked - the request
/// @param found - what the search found
/// @param engine - which engine answered
/// @param took - how long the search took, the gate included
/// @param gate - how long the freshness gate took
fn found_value(
    asked: &FindRequest,
    found: &crate::exact::ExactAnswer,
    engine: &str,
    took: std::time::Duration,
    gate: std::time::Duration,
) -> Value {
    let needle = if asked.regex { "" } else { asked.query.as_str() };
    // Code before tests before documents, so a budget cuts the documents first. In path order a broad
    // pattern led with `.claude/repo-plan.md`, because a dot sorts first. Left alone when the caller
    // asks for every hit, which is also the case the speed runs time.
    let ordered;
    let hits = if asked.budget == 0 {
        &found.hits
    } else {
        let order = |path: &str| match crate::passages::kind_of(path) {
            "code" => 0,
            "test" => 1,
            _ => 2,
        };
        let mut sorted = found.hits.clone();
        sorted.sort_by_key(|h| order(&h.path));
        ordered = sorted;
        &ordered
    };
    let shaped = shape::shape(hits, needle, asked.budget);
    let files: std::collections::BTreeSet<&str> =
        found.hits.iter().map(|h| h.path.as_str()).collect();
    json!({
        "index": engine,
        "mode": if asked.regex { "regex" } else { "literal" },
        "total": found.hits.len(),
        "files": files.len(),
        "omitted": { "hits": shaped.omitted_hits, "files": shaped.omitted_files },
        "hits": shaped.shown.iter().map(|h| json!([h.path, h.line, h.text])).collect::<Vec<_>>(),
        "work": { "inScope": found.in_scope, "candidates": found.candidates, "verified": found.verified, "unbounded": found.unbounded, "micros": took.as_micros() as u64, "gateMicros": gate.as_micros() as u64 },
        "text": shaped.text,
    })
}

/// `search files`: paths ranked by how well they match a name fragment or a glob.
///
/// @param index - the index
/// @param args - query, limit
fn files(index: &Index, args: &Map<String, Value>) -> Result<Value, Refusal> {
    let query = text(args, "query").ok_or_else(|| usage("search files needs a name or a glob."))?;
    let limit = whole(args, "limit").unwrap_or(20);
    let paths = index
        .with_exact(|exact| exact.by_path.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_else(|| crate::files::walk(index.root()).into_iter().map(|f| f.rel).collect());
    // A folder narrows the search the way the Glob tool's `path` does: the glob is matched against paths
    // relative to that folder, and the answers are given relative to the project again.
    let folder = text(args, "path").map(crate::files::normalise).unwrap_or_default();
    let prefix = if folder.is_empty() { String::new() } else { format!("{folder}/") };
    let within: Vec<String> =
        paths.iter().filter_map(|p| p.strip_prefix(prefix.as_str()).map(str::to_owned)).collect();
    let mut ranked: Vec<String> = crate::paths::rank(&within, query, limit)
        .into_iter()
        .map(|p| format!("{prefix}{p}"))
        .collect();
    // A name that is part of the folder's own path, such as `backend/scripts` asked inside
    // `backend/scripts`, matches nothing once the folder is taken off, so it is asked again of the
    // folder's files by their whole path. An agent made exactly that call and was told no file matched.
    if ranked.is_empty() && !prefix.is_empty() {
        let whole_paths: Vec<String> = within.iter().map(|p| format!("{prefix}{p}")).collect();
        ranked = crate::paths::rank(&whole_paths, query, limit);
    }
    let mut text = if ranked.is_empty() {
        "no files match\n".to_owned()
    } else {
        ranked.iter().map(|p| format!("{p}\n")).collect()
    };
    // A short answer carries the best file's outline. In the second dev agent run 37 reads came
    // straight after `files`, most of them of the whole file; with the outline in hand an agent can
    // ask for one function with `fragment` instead.
    if let (true, Some(best)) = (ranked.len() <= FILES_OUTLINED, ranked.first()) {
        let mut args = Map::new();
        args.insert("path".into(), Value::String(best.clone()));
        if let Some(shown) =
            outline(index, &args).ok().and_then(|v| v["text"].as_str().map(str::to_owned))
        {
            if shape::tokens(&shown) <= OUTLINE_TOKENS {
                text.push_str(&format!("\noutline of {shown}read one with `fragment path:line`\n"));
            }
        }
    }
    Ok(json!({ "files": ranked, "total": ranked.len(), "text": text }))
}

/// `search status`: whether the index is ready, what it holds and what the gate has done.
///
/// @param index - the index
fn status(index: &Index) -> Value {
    let st = index.status();
    let (files, trigrams, bytes) =
        index.with_exact(|e| (e.live(), e.postings.len(), e.heap_bytes())).unwrap_or((0, 0, 0));
    let text = format!(
        "{} ({}), {} files, {} trigrams, {} MB in memory, {} gates, {} files read again\n{}\n",
        if st.ready { "ready" } else { "building" },
        st.origin,
        files,
        trigrams,
        bytes / (1 << 20),
        st.gates,
        st.reindexed,
        st.file.display()
    );
    json!({
        "root": index.root().to_string_lossy(),
        "ready": st.ready,
        "origin": st.origin,
        "loadMs": st.load_ms as u64,
        "files": files,
        "trigrams": trigrams,
        "memoryBytes": bytes,
        "gates": st.gates,
        "reindexed": st.reindexed,
        "passages": st.passages,
        "passagesReady": st.passages_ready,
        "vectors": st.vectors,
        "canEmbed": st.can_embed,
        "notEmbedding": st.not_embedding,
        "storeError": st.store_error,
        "indexFile": st.file.to_string_lossy(),
        "versions": { "schema": crate::store::SCHEMA_VERSION, "trigram": crate::store::TRIGRAM_VERSION },
        "text": text,
    })
}

/// Whether a query reads as a question or a sentence rather than as text to find: four words or more,
/// or a question mark at the end.
///
/// @param query - the query
fn is_sentence(query: &str) -> bool {
    !looks_like_regex(query.trim_end_matches('?'))
        && (query.split_whitespace().count() >= 4 || query.trim_end().ends_with('?'))
}

/// Whether a query is one identifier, which `auto` mode answers with its definitions and its uses.
///
/// @param query - the query
fn is_identifier(query: &str) -> bool {
    let mut chars = query.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// One definition as a row of the reply: path, line, end, kind and signature.
///
/// @param d - the definition
fn definition_value(d: &crate::symbols::Defined) -> Value {
    json!({ "path": d.path, "line": d.definition.line, "end": d.definition.end, "kind": d.definition.kind, "name": d.definition.name, "signature": d.definition.signature })
}

/// `search def`: where a name is defined, best first, with each definition's signature line.
///
/// @param index - the index
/// @param args - name, limit, path
fn def(index: &Index, args: &Map<String, Value>) -> Result<Value, Refusal> {
    let asked = text(args, "name").ok_or_else(|| usage("search def needs a name."))?.to_owned();
    let limit = whole(args, "limit").unwrap_or(10);
    let within = text(args, "path").map(crate::files::normalise);
    let (qualifier, name) = split_qualified(&asked);
    let found = index
        .with_symbols(|table, _| qualified_lookup(table, qualifier, &name, limit * 4))
        .ok_or_else(|| Refusal {
            code: "not-applicable",
            message: "The index is still being built; ask again in a moment.".into(),
        })?;
    let found: Vec<_> = found
        .into_iter()
        .filter(|d| within.as_ref().is_none_or(|w| w.is_empty() || d.path.starts_with(w.as_str())))
        .take(limit)
        .collect();
    let mut text = String::new();
    for d in &found {
        text.push_str(&format!(
            "{}:{}: {}\n",
            d.path,
            d.definition.line,
            shape::trim_line(&d.definition.signature, &name)
        ));
    }
    if found.is_empty() {
        text.push_str(&format!(
            "no definition of `{name}` found; try `find {name}` for every use\n"
        ));
    }
    if let [only] = found.as_slice() {
        if let Some(code) = inline_code(index, &only.path, only.definition.line, DEFAULT_BUDGET) {
            text.push('\n');
            text.push_str(&code);
        }
    }
    Ok(
        json!({ "name": name, "definitions": found.iter().map(definition_value).collect::<Vec<_>>(), "total": found.len(), "text": text }),
    )
}

/// A name as an agent writes it, `Shell::open` or `Database.open`, split into the type it is asked
/// about and the name itself. In the third dev agent run, two `def` calls for `Shell::open` and
/// `Database::open` found nothing and cost the agent a turn each.
///
/// @param asked - the name as given
fn split_qualified(asked: &str) -> (Option<&str>, String) {
    let cut = asked.rfind("::").map(|at| (at, 2)).or_else(|| asked.rfind('.').map(|at| (at, 1)));
    match cut {
        Some((at, width)) if at > 0 && at + width < asked.len() => {
            let qualifier = &asked[..at];
            let last = qualifier.rsplit(['.', ':']).next().unwrap_or(qualifier);
            (Some(last), asked[at + width..].to_owned())
        }
        _ => (None, asked.to_owned()),
    }
}

/// The definitions of a name, those inside a definition of the qualifier first, when there is one.
///
/// @param table - the symbol table
/// @param qualifier - the type the name was asked about, if any
/// @param name - the name
/// @param limit - how many to return
fn qualified_lookup(
    table: &crate::symbols::SymbolTable,
    qualifier: Option<&str>,
    name: &str,
    limit: usize,
) -> Vec<crate::symbols::Defined> {
    let mut found = table.lookup(name, if qualifier.is_some() { limit * 4 } else { limit });
    if let Some(qualifier) = qualifier {
        let containers: Vec<_> = table
            .lookup(qualifier, 50)
            .into_iter()
            .filter(|c| c.definition.name.eq_ignore_ascii_case(qualifier))
            .collect();
        let inside = |d: &crate::symbols::Defined| {
            containers.iter().any(|c| {
                c.path == d.path
                    && c.definition.line <= d.definition.line
                    && d.definition.line <= c.definition.end
                    && c.definition.end > c.definition.line
            })
        };
        // A method of `impl Shell` is not inside the `struct Shell` definition, so two weaker signs
        // follow: the signature names the type, and the file is named after it.
        let word = regex::Regex::new(&format!(r"(?i)\b{}\b", regex::escape(qualifier))).ok();
        let names_it = |d: &crate::symbols::Defined| {
            word.as_ref().is_some_and(|w| w.is_match(&d.definition.signature))
        };
        let named_after = |d: &crate::symbols::Defined| {
            std::path::Path::new(&d.path).file_stem().is_some_and(|s| {
                s.to_string_lossy().replace(['_', '-'], "").eq_ignore_ascii_case(qualifier)
            })
        };
        found.sort_by_key(|d| (!inside(d), !names_it(d), !named_after(d)));
    }
    found.truncate(limit);
    found
}

/// The uses of a name: a whole word search, with the lines that define it taken out and the project's
/// own source before tests and generated files.
///
/// @param index - the index
/// @param name - the name
/// @param scope - where to look
fn uses(index: &Index, name: &str, scope: &Scope) -> Result<Vec<crate::exact::Hit>, Refusal> {
    let pattern = format!(r"\b{}\b", regex::escape(name));
    let request = ExactRequest { pattern: &pattern, case_insensitive: false, scope };
    let (found, _) =
        index.exact(&request).map_err(|e| usage(format!("The name was refused: {e}")))?;
    let defined: std::collections::HashSet<(String, u64)> = index
        .with_symbols(|table, _| {
            table
                .lookup(name, 50)
                .into_iter()
                .filter(|d| d.definition.name == name)
                .map(|d| (d.path, u64::from(d.definition.line)))
                .collect()
        })
        .unwrap_or_default();
    // Definition lines are kept and put first in their file. A line that defines a name very often uses
    // it too, an `impl` method's line is a use of the trait method it implements, and
    // `let untracked = entry.untracked()` defines one thing and uses another; leaving such lines out lost
    // uses ripgrep finds (F3 recall fell below rg's on all three corpora).
    let mut hits = found.hits;
    let order = |path: &str| match crate::passages::kind_of(path) {
        "code" => 0,
        "test" => 1,
        _ => 2,
    };
    hits.sort_by(|a, b| {
        order(&a.path)
            .cmp(&order(&b.path))
            .then(a.path.cmp(&b.path))
            .then(
                (!defined.contains(&(a.path.clone(), a.line)))
                    .cmp(&!defined.contains(&(b.path.clone(), b.line))),
            )
            .then(a.line.cmp(&b.line))
    });
    Ok(hits)
}

/// `search refs`: every use of a name, grouped by file, definitions left out, within the budget.
///
/// @param index - the index
/// @param args - name, path, glob, budget
fn refs(index: &Index, args: &Map<String, Value>) -> Result<Value, Refusal> {
    let name = text(args, "name").ok_or_else(|| usage("search refs needs a name."))?.to_owned();
    let globs = text(args, "glob").map(split_globs).unwrap_or_default();
    let scope =
        Scope::new(index.root(), text(args, "path").unwrap_or(""), &globs, &[]).map_err(usage)?;
    let budget = whole(args, "budget").unwrap_or(DEFAULT_BUDGET);
    let hits = uses(index, &name, &scope)?;
    let shaped = shape::shape(&hits, &name, budget);
    let files: std::collections::BTreeSet<&str> = hits.iter().map(|h| h.path.as_str()).collect();
    Ok(json!({
        "name": name,
        "total": hits.len(),
        "files": files.len(),
        "omitted": { "hits": shaped.omitted_hits, "files": shaped.omitted_files },
        "hits": shaped.shown.iter().map(|h| json!([h.path, h.line, h.text])).collect::<Vec<_>>(),
        "text": shaped.text,
    }))
}

/// `find` in auto mode for one identifier: its definitions first, then its uses, in one budget.
///
/// @param index - the index
/// @param asked - the request
fn find_symbol(index: &Index, asked: &FindRequest) -> Result<Option<Value>, Refusal> {
    let Some(defs) = index.with_symbols(|table, _| table.lookup(&asked.query, 5)) else {
        return Ok(None);
    };
    let defs: Vec<_> = defs
        .into_iter()
        .filter(|d| {
            d.definition.name == asked.query || d.definition.name.eq_ignore_ascii_case(&asked.query)
        })
        .collect();
    if defs.is_empty() {
        return Ok(None);
    }
    let mut text = String::from("defined at:\n");
    for d in &defs {
        text.push_str(&format!(
            "  {}:{}: {}\n",
            d.path,
            d.definition.line,
            shape::trim_line(&d.definition.signature, &asked.query)
        ));
    }
    if let ([only], true) = (defs.as_slice(), asked.budget > 0) {
        if let Some(code) = inline_code(index, &only.path, only.definition.line, asked.budget / 2) {
            text.push_str(&code);
        }
    }
    let hits = uses(index, &asked.query, &asked.scope)?;
    let budget = asked.budget.saturating_sub(shape::tokens(&text));
    let shaped =
        shape::shape(&hits, &asked.query, if asked.budget == 0 { 0 } else { budget.max(200) });
    text.push_str(&format!("used in {} places:\n", hits.len()));
    text.push_str(&shaped.text);
    Ok(Some(json!({
        "index": "symbol",
        "mode": "symbol",
        "definitions": defs.iter().map(definition_value).collect::<Vec<_>>(),
        "total": hits.len(),
        "omitted": { "hits": shaped.omitted_hits, "files": shaped.omitted_files },
        "hits": shaped.shown.iter().map(|h| json!([h.path, h.line, h.text])).collect::<Vec<_>>(),
        "text": text,
    })))
}

/// A file's text as the index holds it, or as it is on disk when the index does not hold it.
///
/// @param index - the index
/// @param rel - the path, relative to the root
fn file_text(index: &Index, rel: &str) -> Option<String> {
    let held = index
        .with_exact(|exact| {
            exact
                .by_path
                .get(rel)
                .and_then(|&id| exact.files[id as usize].as_ref())
                .map(|r| r.bytes())
        })
        .flatten();
    let bytes = held.or_else(|| std::fs::read(index.root().join(rel)).ok())?;
    Some(String::from_utf8_lossy(&crate::exact::searched_text(&bytes)).into_owned())
}

/// Splits `path:line` into its two halves; a bare path has no line.
///
/// @param target - what was given
fn path_and_line(target: &str) -> (String, Option<u32>) {
    match target.rsplit_once(':') {
        Some((path, line))
            if !path.is_empty()
                && line.chars().all(|c| c.is_ascii_digit())
                && !line.is_empty()
                && (path.len() != 1 || !cfg!(windows)) =>
        {
            (crate::files::normalise(path), line.parse().ok())
        }
        _ => (crate::files::normalise(target), None),
    }
}

/// A path given absolute, or relative to the working folder, made relative to the root.
///
/// @param index - the index
/// @param path - the path as given
fn relative_to_root(index: &Index, path: &str) -> String {
    let candidate = std::path::Path::new(path);
    if candidate.is_absolute() {
        if let Some(rel) = crate::files::relative(index.root(), candidate) {
            return rel;
        }
    }
    crate::files::normalise(path)
}

/// `search outline`: a file's definitions, one line each, indented by how deep they sit.
///
/// @param index - the index
/// @param args - path
fn outline(index: &Index, args: &Map<String, Value>) -> Result<Value, Refusal> {
    let given = text(args, "path").ok_or_else(|| usage("search outline needs a file."))?;
    let rel = relative_to_root(index, &path_and_line(given).0);
    let content = file_text(index, &rel).ok_or_else(|| Refusal {
        code: "not-found",
        message: format!("There is no file `{rel}` in this project."),
    })?;
    let read = crate::outline::read(&rel, &content, crate::outline::CHUNK_BUDGET);
    let mut text = format!("{rel} ({} lines)\n", content.lines().count());
    let rows: Vec<Value> = if read.definitions.is_empty() {
        read.chunks
            .iter()
            .filter(|c| c.kind == "section")
            .map(|c| {
                let heading = c.body.lines().next().unwrap_or_default().trim().to_owned();
                text.push_str(&format!("  {}: {}\n", c.start, heading));
                json!({ "line": c.start, "end": c.end, "kind": "section", "signature": heading })
            })
            .collect()
    } else {
        read.definitions.iter().filter(|d| crate::outline::is_listed(d)).map(|d| {
            text.push_str(&format!("{}{}-{}: {}\n", "  ".repeat(d.depth as usize + 1), d.line, d.end, shape::trim_line(&d.signature, "")));
            json!({ "line": d.line, "end": d.end, "kind": d.kind, "name": d.name, "signature": d.signature, "depth": d.depth })
        }).collect()
    };
    if rows.is_empty() {
        text.push_str("  no definitions or headings found\n");
    }
    Ok(json!({ "path": rel, "definitions": rows, "text": text }))
}

/// The longest definition `fragment` returns whole, in lines; a longer one is shown a chunk at a time.
pub const FRAGMENT_LINES: u32 = 300;

/// `search fragment`: the chunk around a line, with line numbers and its header, so a function can be
/// read without reading its file.
///
/// @param index - the index
/// @param args - target (`path:line`), line, context
fn fragment(index: &Index, args: &Map<String, Value>) -> Result<Value, Refusal> {
    let target = text(args, "target")
        .ok_or_else(|| usage("search fragment needs a path and a line, such as src/main.rs:42."))?;
    let (path, line) = path_and_line(target);
    let line = whole(args, "line").map(|l| l as u32).or(line).unwrap_or(1).max(1);
    let rel = relative_to_root(index, &path);
    let context = whole(args, "context").unwrap_or(0) as u32;
    let content = file_text(index, &rel).ok_or_else(|| Refusal {
        code: "not-found",
        message: format!("There is no file `{rel}` in this project."),
    })?;
    let read = crate::outline::read(&rel, &content, crate::outline::CHUNK_BUDGET);
    let lines: Vec<&str> = content.split('\n').collect();
    // The innermost definition around the line, from its doc comment to its end, when it is short
    // enough to read whole; otherwise the chunk around the line.
    let definition = read
        .definitions
        .iter()
        .filter(|d| {
            d.start <= line && line <= d.end && d.end > d.line && d.end - d.start < FRAGMENT_LINES
        })
        .min_by_key(|d| d.end - d.start);
    let chunk = read
        .chunks
        .iter()
        .find(|c| c.start <= line && line <= c.end)
        .or_else(|| read.chunks.last());
    let (start, end, header) = match (definition, chunk) {
        (Some(d), _) => {
            let enclosing: Vec<&str> = read
                .definitions
                .iter()
                .filter(|o| o.line < d.line && o.end >= d.end)
                .map(|o| o.signature.as_str())
                .collect();
            let header = if enclosing.is_empty() {
                rel.clone()
            } else {
                format!("{rel} | {}", enclosing.join(" ; "))
            };
            (
                d.start.saturating_sub(context).max(1),
                (d.end + context).min(lines.len() as u32),
                header,
            )
        }
        (None, Some(c)) => (
            c.start.saturating_sub(context).max(1),
            (c.end + context).min(lines.len() as u32),
            c.header.clone(),
        ),
        (None, None) => (1, (lines.len() as u32).min(40), rel.clone()),
    };
    let mut text = format!("{header}\n");
    for n in start..=end {
        text.push_str(&format!(
            "{n:>5}  {}\n",
            lines.get(n as usize - 1).map_or("", |l| l.trim_end_matches('\r'))
        ));
    }
    if end < lines.len() as u32 {
        text.push_str(&format!("  … {} more lines in the file\n", lines.len() as u32 - end));
    }
    Ok(json!({ "path": rel, "start": start, "end": end, "header": header, "text": text }))
}

/// The share of a question's words the best chunk must hold before passage search will answer with
/// it. Below it the answer is empty, with the nearest definitions as hints, because a chunk that shares
/// one word with a question is not an answer to it. Measured on the dev split with vectors and code
/// ranked before documents (lever 8): at 0.31 F6 scored 0.116 and F8 0.554. A higher cutoff scores more
/// on the weighted metric only by turning answerable questions away, and a lower one lets most
/// questions with no answer in the code through. It applies with vectors as well as without.
pub const ABSTAIN_BELOW: f64 = 0.31;

/// The abstention threshold in force: `ABSTAIN_BELOW`, or `ATRIUS_SEARCH_ABSTAIN` when the
/// evaluation's improvement loop is trying another value (TDD §8.3, lever 8).
fn abstain_below() -> f64 {
    std::env::var("ATRIUS_SEARCH_ABSTAIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(ABSTAIN_BELOW)
}

/// The confidence below which a hybrid passage search answers with nothing, off by default. R8
/// measured a clean split on Inillucent's own corpus, but on code it does not hold: a ticket's whole
/// text scores near 0.001 and a one line question between 0.3 and 0.4 whether or not the code answers
/// it, so any cutoff turned every ticket away before it turned a question with no answer away.
pub const CONFIDENCE_BELOW: f64 = 0.0;

/// The confidence threshold in force: `CONFIDENCE_BELOW`, or `ATRIUS_SEARCH_CONFIDENCE` while the
/// improvement loop tries another value.
fn confidence_below() -> f64 {
    std::env::var("ATRIUS_SEARCH_CONFIDENCE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CONFIDENCE_BELOW)
}

/// How many words of a question a text holds, as a share of the question's words.
///
/// @param question_words - the question's words, lower case
/// @param text - the chunk's header and body
fn coverage(question_words: &[String], text: &str) -> f64 {
    if question_words.is_empty() {
        return 0.0;
    }
    let lower = format!("{} {}", text.to_lowercase(), crate::words::search_words(text));
    let held = question_words.iter().filter(|w| lower.contains(w.as_str())).count();
    held as f64 / question_words.len() as f64
}

/// `find --mode semantic`: the chunks that best answer a question written in plain English, each as its
/// file, its line range, its header and the lines holding the question's words, within the budget.
///
/// @param index - the index
/// @param asked - the request
fn semantic(index: &Index, asked: &FindRequest) -> Result<Value, Refusal> {
    let started = std::time::Instant::now();
    let found = index
        .passages(&asked.query, 30, None)
        .map_err(|e| Refusal { code: "not-applicable", message: e })?;
    let with_vectors = index.status().vectors > 0;
    let question_words = crate::words::query_words(&asked.query);
    let mut chosen = Vec::new();
    let mut text = String::new();
    let mut used = 0usize;
    let budget = if asked.budget == 0 { usize::MAX } else { asked.budget };
    for hit in found.iter().filter(|h| asked.scope.is_everything() || asked.scope.contains(&h.path))
    {
        let Some(content) = file_text(index, &hit.path) else { continue };
        let lines: Vec<&str> = content.split('\n').collect();
        let body = lines
            .get(hit.start as usize - 1..(hit.end as usize).min(lines.len()))
            .map(|l| l.join("\n"))
            .unwrap_or_default();
        let share = coverage(&question_words, &format!("{}\n{}", hit.header, body));
        // Whether the best chunk answers the question at all is decided by how many of its words it
        // holds, and, when a confidence cutoff is set, by the engine's confidence as well.
        let unsure =
            share < abstain_below() || (with_vectors && hit.confidence < confidence_below());
        if chosen.is_empty() && unsure {
            break;
        }
        // The header starts with the path, so only what follows it is printed.
        let signatures = hit
            .header
            .strip_prefix(hit.path.as_str())
            .map(|s| s.trim_start_matches(" | "))
            .unwrap_or(&hit.header);
        let mut block = format!(
            "{}:{}-{} {}\n",
            hit.path,
            hit.start,
            hit.end,
            shape::trim_line(signatures, "")
        )
        .replace(" \n", "\n");
        let mut shown = 0;
        for (i, line) in
            lines.iter().enumerate().take(hit.end as usize).skip(hit.start as usize - 1)
        {
            let lower = line.to_lowercase();
            if shown < 2 && question_words.iter().any(|w| lower.contains(w.as_str())) {
                block.push_str(&format!("  {}: {}\n", i + 1, shape::trim_line(line, "")));
                shown += 1;
            }
        }
        if used + shape::tokens(&block) > budget && !chosen.is_empty() {
            break;
        }
        // The text shows the first SEMANTIC_SHOWN chunks and the fields hold up to ten. In the fourth
        // dev agent run a plain English `find` cost about 3,000 characters a call against 60 to 850
        // for a Grep that listed files, and every turn after it read those characters again.
        if chosen.len() < SEMANTIC_SHOWN {
            used += shape::tokens(&block);
            text.push_str(&block);
        }
        chosen.push((hit.clone(), share));
        if chosen.len() >= 10 {
            break;
        }
    }
    if chosen.is_empty() {
        let hints = index
            .with_symbols(|table, _| {
                question_words
                    .iter()
                    .flat_map(|w| table.lookup(w, 2))
                    .take(5)
                    .map(|d| format!("{}:{} {}", d.path, d.definition.line, d.definition.name))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        text.push_str("no confident match in this project\n");
        if !hints.is_empty() {
            text.push_str(&format!("nearest names: {}\n", hints.join(", ")));
        }
    } else {
        // The best chunk is shown whole when it is short, which is the read an agent makes next.
        let (best, _) = &chosen[0];
        if let Some(code) =
            inline_code(index, &best.path, best.start.max(1), DEFAULT_BUDGET.saturating_sub(used))
        {
            text.push_str(&format!("\nthe code at {}:{}:\n{code}", best.path, best.start));
        }
        text.push_str("read another with `fragment path:line`\n");
    }
    let files: Vec<&str> = {
        let mut seen = Vec::new();
        for (h, _) in &chosen {
            if !seen.contains(&h.path.as_str()) {
                seen.push(h.path.as_str());
            }
        }
        seen
    };
    Ok(json!({
        "index": "passage",
        "mode": "semantic",
        "total": chosen.len(),
        "files": files,
        "hits": chosen.iter().map(|(h, share)| json!([h.path, h.start, h.header, h.end, h.score, (share * 1000.0).round() / 1000.0, h.confidence])).collect::<Vec<_>>(),
        "work": { "micros": started.elapsed().as_micros() as u64, "retrieved": found.len() },
        "text": text,
    }))
}

#[cfg(test)]
mod tests {
    use super::split_qualified;

    #[test]
    fn a_qualified_name_is_split_into_its_type_and_its_name() {
        assert_eq!(split_qualified("Shell::open"), (Some("Shell"), "open".to_owned()));
        assert_eq!(split_qualified("Database.open"), (Some("Database"), "open".to_owned()));
        assert_eq!(
            split_qualified("crate::engine::Database::open"),
            (Some("Database"), "open".to_owned())
        );
        assert_eq!(split_qualified("open"), (None, "open".to_owned()));
        // A leading or trailing separator is not a qualifier.
        assert_eq!(split_qualified("::open"), (None, "::open".to_owned()));
        assert_eq!(split_qualified("open."), (None, "open.".to_owned()));
    }
}
