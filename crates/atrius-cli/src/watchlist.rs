//! The folders the background service keeps indexed, and where Atrius keeps its settings.
//!
//! The list is `watch.conf` in the configuration folder, one absolute folder a line. A line starting
//! with `#` is a comment. The daemon reads it again every 10 seconds, so `atrius watch add` takes effect
//! without restarting anything.

use std::path::{Path, PathBuf};

/// The configuration folder: `ATRIUS_CONFIG`, or `%APPDATA%\atrius-index` on Windows,
/// `~/Library/Application Support/atrius-index` on macOS, `$XDG_CONFIG_HOME/atrius-index` or
/// `~/.config/atrius-index` elsewhere.
pub fn config_folder() -> PathBuf {
    if let Some(dir) = std::env::var_os("ATRIUS_CONFIG") {
        return PathBuf::from(dir);
    }
    let home = || std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    if cfg!(windows) {
        if let Some(dir) = std::env::var_os("APPDATA") {
            return PathBuf::from(dir).join("atrius-index");
        }
    }
    if cfg!(target_os = "macos") {
        if let Some(home) = home() {
            return home.join("Library").join("Application Support").join("atrius-index");
        }
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join("atrius-index");
    }
    home().unwrap_or_else(std::env::temp_dir).join(".config").join("atrius-index")
}

/// The watch list file.
pub fn watch_file() -> PathBuf {
    config_folder().join("watch.conf")
}

/// The folders on the watch list, in the order they were added.
pub fn read() -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(watch_file()) else { return Vec::new() };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(PathBuf::from)
        .collect()
}

/// Writes the watch list.
///
/// @param folders - the folders
fn write(folders: &[PathBuf]) -> Result<(), String> {
    let file = watch_file();
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let mut text = String::from("# The folders `atrius daemon` keeps indexed, one a line. Edit with `atrius watch add` and `atrius watch remove`.\n");
    for folder in folders {
        text.push_str(&folder.to_string_lossy());
        text.push('\n');
    }
    std::fs::write(&file, text).map_err(|e| format!("cannot write {}: {e}", file.display()))
}

/// Adds a folder. Answers whether it was new.
///
/// @param folder - the folder, canonical
pub fn add(folder: &Path) -> Result<bool, String> {
    if !folder.is_dir() {
        return Err(format!("There is no folder at {}.", folder.display()));
    }
    let mut folders = read();
    if folders.iter().any(|f| same(f, folder)) {
        return Ok(false);
    }
    folders.push(folder.to_path_buf());
    write(&folders).map(|()| true)
}

/// Removes a folder. Answers whether it was on the list.
///
/// @param folder - the folder
pub fn remove(folder: &Path) -> Result<bool, String> {
    let mut folders = read();
    let before = folders.len();
    folders.retain(|f| !same(f, folder));
    if folders.len() == before {
        return Ok(false);
    }
    write(&folders).map(|()| true)
}

/// Whether two folders are the same, ignoring case on Windows and macOS.
///
/// @param a - one folder
/// @param b - the other
fn same(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let text = p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_owned();
        if cfg!(any(windows, target_os = "macos")) { text.to_lowercase() } else { text }
    };
    norm(a) == norm(b)
}
