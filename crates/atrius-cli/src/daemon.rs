//! `atrius daemon`: what the background service runs.
//!
//! It hosts every folder on the watch list in this one process, so each folder's watcher is running
//! and its index is current before the first query. It reads the list again every 10 seconds, so a
//! folder added with `atrius watch add` is picked up and a removed one is let go. A folder another
//! process already hosts is left to that process and tried again on the next pass.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use atrius_index::host::{self, Host};

use crate::service::{daemon_pid, pid_file};

/// How often the watch list is read again.
const REREAD: Duration = Duration::from_secs(10);

/// Runs the daemon until it is stopped. Exit code 1 when another daemon is already running.
pub fn run() -> i32 {
    if let Some(pid) = daemon_pid() {
        if pid != std::process::id() {
            eprintln!("atrius daemon: another daemon is already running as process {pid}");
            return 1;
        }
    }
    if let Some(parent) = pid_file().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(pid_file(), std::process::id().to_string()) {
        eprintln!("atrius daemon: cannot write {}: {e}", pid_file().display());
        return 1;
    }
    host::set_program_name(&format!("atrius daemon {}", atrius_index::VERSION));
    let mut hosted: HashMap<PathBuf, Arc<Host>> = HashMap::new();
    loop {
        pass(&mut hosted);
        std::thread::sleep(REREAD);
    }
}

/// One pass: host every listed folder that has no host, and let go of the folders no longer listed.
///
/// @param hosted - the folders this process hosts
fn pass(hosted: &mut HashMap<PathBuf, Arc<Host>>) {
    let wanted: Vec<PathBuf> = crate::watchlist::read().into_iter().map(|f| host::canonical(&f)).collect();
    if hosted.keys().any(|k| !wanted.contains(k)) {
        restart();
    }
    for folder in wanted {
        if hosted.contains_key(&folder) || !folder.is_dir() {
            continue;
        }
        if let Some(h) = host::in_process(&folder) {
            hosted.insert(folder, h);
        }
    }
}

/// Lets go of a folder taken off the list by starting again. A host holds its folder's lock and
/// watcher for the life of the process, so a fresh process is how a folder is released.
///
/// Under launchd or systemd the daemon exits with code 3 and the service manager starts it again,
/// because both restart a service that fails and would stop a child it started itself. Otherwise it
/// starts a detached copy of itself and exits.
fn restart() -> ! {
    let _ = std::fs::remove_file(pid_file());
    let supervised = std::env::var_os("INVOCATION_ID").is_some() || std::env::var_os("XPC_SERVICE_NAME").is_some_and(|v| v != "0");
    if supervised {
        std::process::exit(3);
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut command = std::process::Command::new(exe);
        command.arg("daemon").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        host::detach(&mut command);
        let _ = command.spawn();
    }
    std::process::exit(0);
}
