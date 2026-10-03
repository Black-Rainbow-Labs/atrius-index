//! `atrius doctor`: checks that this machine can keep an index current.
//!
//! A file watcher that delivers nothing looks exactly like a tree nobody is editing, so the watcher is
//! checked by making a change and waiting for the event. On Linux each watched directory costs one
//! inotify watch, and when the limit is reached new directories are silently not watched, so the
//! limit is compared with the directories in the watched folders.

use std::path::Path;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use serde_json::{json, Value};

/// One check's result.
struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
}

/// Runs every check and answers the reply's result: `checks`, `ok` and `text`.
pub fn run() -> Value {
    let mut checks = vec![cache(), watcher()];
    if cfg!(target_os = "linux") {
        checks.push(inotify());
    }
    checks.push(embeddings());
    checks.push(service());
    let ok = checks.iter().all(|c| c.ok);
    let mut text = String::new();
    for check in &checks {
        text.push_str(&format!("{} {}: {}\n", if check.ok { "ok  " } else { "FAIL" }, check.name, check.detail));
    }
    json!({
        "ok": ok,
        "checks": checks.iter().map(|c| json!({ "name": c.name, "ok": c.ok, "detail": c.detail })).collect::<Vec<_>>(),
        "text": text,
    })
}

/// The cache folder can be written.
fn cache() -> Check {
    let folder = atrius_index::store::cache_folder().join("atrius-index");
    let probe = folder.join(format!("doctor-{}.tmp", std::process::id()));
    let result = std::fs::create_dir_all(&folder).and_then(|()| std::fs::write(&probe, b"probe"));
    let _ = std::fs::remove_file(&probe);
    match result {
        Ok(()) => Check { name: "cache folder", ok: true, detail: folder.display().to_string() },
        Err(e) => Check { name: "cache folder", ok: false, detail: format!("{} cannot be written: {e}", folder.display()) },
    }
}

/// The platform watcher delivers an event for a file written in a watched folder.
fn watcher() -> Check {
    let name = "file events";
    let folder = atrius_index::store::cache_folder().join("atrius-index").join(format!("doctor-watch-{}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(folder.join("inner")) {
        return Check { name, ok: false, detail: format!("cannot create {}: {e}", folder.display()) };
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            let _ = tx.send(event);
        }
    });
    let mut watcher = match watcher {
        Ok(w) => w,
        Err(e) => return Check { name, ok: false, detail: format!("the watcher did not start: {e}") },
    };
    if let Err(e) = watcher.watch(&folder, RecursiveMode::Recursive) {
        let _ = std::fs::remove_dir_all(&folder);
        return Check { name, ok: false, detail: format!("the folder could not be watched: {e}") };
    }
    let started = Instant::now();
    let target = folder.join("inner").join("probe.txt");
    let _ = std::fs::write(&target, b"probe");
    let mut seen = None;
    while started.elapsed() < Duration::from_secs(5) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(event) if event.paths.iter().any(|p| p.ends_with("probe.txt")) => {
                seen = Some(started.elapsed());
                break;
            }
            _ => {}
        }
    }
    drop(watcher);
    let _ = std::fs::remove_dir_all(&folder);
    match seen {
        Some(after) => Check { name, ok: true, detail: format!("an event arrived {} ms after a write in a subfolder", after.as_millis()) },
        None => Check { name, ok: false, detail: "no event arrived within 5 seconds of a write. The index will still answer correctly, because every query first checks file sizes and times in its scope, but a large checkout will be slower".into() },
    }
}

/// On Linux, the inotify watch limit against the directories of the watched folders.
fn inotify() -> Check {
    let name = "inotify watches";
    let limit: Option<u64> = std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches").ok().and_then(|t| t.trim().parse().ok());
    let Some(limit) = limit else {
        return Check { name, ok: true, detail: "the limit could not be read".into() };
    };
    let needed: u64 = crate::watchlist::read().iter().map(|f| directories(f)).sum();
    if needed * 2 < limit {
        Check { name, ok: true, detail: format!("{needed} directories in the watched folders, limit {limit}") }
    } else {
        Check {
            name,
            ok: false,
            detail: format!("{needed} directories in the watched folders and the limit is {limit}. Raise it with: echo fs.inotify.max_user_watches=524288 | sudo tee /etc/sysctl.d/60-atrius.conf && sudo sysctl --system"),
        }
    }
}

/// How many directories a folder holds, itself included, not following links.
///
/// @param folder - the folder
fn directories(folder: &Path) -> u64 {
    let mut count = 0;
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        count += 1;
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(entry.path());
            }
        }
    }
    count
}

/// Whether `embed()` can run, which plain English search uses when it can.
fn embeddings() -> Check {
    let name = "embeddings";
    let folder = std::env::temp_dir().join(format!("atrius-doctor-{}", std::process::id()));
    let able = atrius_index::store::Store::open(&folder).map(|store| atrius_index::passages::can_embed(&store.session()));
    let _ = std::fs::remove_dir_all(&folder);
    match able {
        Ok(true) => Check { name, ok: true, detail: "the embedding model is installed, so plain English search also ranks by meaning".into() },
        Ok(false) => Check { name, ok: true, detail: "no embedding model is installed. Plain English search ranks by words alone. `inillucent setup-embeddings` installs one".into() },
        Err(e) => Check { name, ok: false, detail: format!("the index file could not be created: {e}") },
    }
}

/// The background service, reported but never failed: it is optional.
fn service() -> Check {
    let status = crate::service::status();
    let detail = status["text"].as_str().unwrap_or_default().trim().replace('\n', "; ");
    Check { name: "background service", ok: true, detail }
}
