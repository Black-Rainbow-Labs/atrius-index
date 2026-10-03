//! The background service: a login item for this user that runs `atrius daemon`.
//!
//! | Platform | Login item | Started now with |
//! |---|---|---|
//! | Windows | the `AtriusIndex` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, running `conhost.exe --headless atrius daemon` | a detached `atrius daemon` |
//! | macOS | `~/Library/LaunchAgents/com.blackrainbowlabs.atrius-index.plist` | `launchctl bootstrap gui/<uid>` |
//! | Linux with systemd | `~/.config/systemd/user/atrius-index.service` | `systemctl --user enable --now` |
//! | Linux without systemd | `~/.config/autostart/atrius-index.desktop` | a detached `atrius daemon` |
//!
//! None of them needs administrator rights, and each runs as the user, who can read the checkouts.
//! `ATRIUS_SERVICE_NAME` changes the Windows value name and `ATRIUS_SERVICE_DRY=1` writes the files
//! without asking launchctl or systemctl, which is how the tests run.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use crate::watchlist::config_folder;

/// The launchd label and the systemd unit's name.
pub const LABEL: &str = "com.blackrainbowlabs.atrius-index";

/// The Windows Run value's name.
fn run_value_name() -> String {
    std::env::var("ATRIUS_SERVICE_NAME").ok().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "AtriusIndex".into())
}

/// Whether to skip launchctl and systemctl.
fn dry() -> bool {
    std::env::var("ATRIUS_SERVICE_DRY").is_ok_and(|v| v == "1")
}

/// The user's home folder.
fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// The file the daemon writes its process id to.
pub fn pid_file() -> PathBuf {
    config_folder().join("daemon.pid")
}

/// The process id of the running daemon, if one is running.
pub fn daemon_pid() -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(pid_file()).ok()?.trim().parse().ok()?;
    crate::process::is_running(pid).then_some(pid)
}

/// The macOS LaunchAgent for a program.
///
/// @param exe - the `atrius` executable
pub fn launch_agent(exe: &Path) -> String {
    let logs = config_folder().join("daemon.log");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>daemon</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ProcessType</key><string>Background</string>
  <key>LowPriorityIO</key><true/>
  <key>StandardErrorPath</key><string>{}</string>
</dict>
</plist>
"#,
        xml(&exe.to_string_lossy()),
        xml(&logs.to_string_lossy())
    )
}

/// Text made safe inside an XML element.
///
/// @param text - the text
fn xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The systemd user unit for a program.
///
/// @param exe - the `atrius` executable
pub fn systemd_unit(exe: &Path) -> String {
    format!(
        "[Unit]\nDescription=Atrius code index: keeps the watched folders indexed\nDocumentation=https://blackrainbowlabs.com/lab\n\n[Service]\nType=simple\nExecStart=\"{}\" daemon\nRestart=on-failure\nRestartSec=10\nNice=10\nIOSchedulingClass=idle\n\n[Install]\nWantedBy=default.target\n",
        exe.display()
    )
}

/// The XDG autostart entry for a program, for a Linux desktop without systemd.
///
/// @param exe - the `atrius` executable
pub fn autostart_entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Atrius code index\nComment=Keeps the watched folders indexed\nExec=\"{}\" daemon\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
        exe.display()
    )
}

/// The Windows Run value's command.
///
/// @param exe - the `atrius` executable
pub fn run_command(exe: &Path) -> String {
    let windows = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let conhost = windows.join("System32").join("conhost.exe");
    format!("\"{}\" --headless \"{}\" daemon", conhost.display(), exe.display())
}

/// Where the login item lives on this platform.
pub fn login_item_path() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run\{}", run_value_name()))
    } else if cfg!(target_os = "macos") {
        home().join("Library").join("LaunchAgents").join(format!("{LABEL}.plist"))
    } else if has_systemd() {
        systemd_folder().join("atrius-index.service")
    } else {
        xdg_config().join("autostart").join("atrius-index.desktop")
    }
}

/// `$XDG_CONFIG_HOME` or `~/.config`.
fn xdg_config() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"))
}

/// The systemd user unit folder.
fn systemd_folder() -> PathBuf {
    xdg_config().join("systemd").join("user")
}

/// Whether this Linux has a systemd user manager to talk to.
fn has_systemd() -> bool {
    if dry() {
        return std::env::var("ATRIUS_SERVICE_SYSTEMD").is_ok_and(|v| v == "1");
    }
    Command::new("systemctl").args(["--user", "is-system-running"]).output().is_ok_and(|o| !o.stdout.is_empty())
}

/// Runs a program and answers its output, or why it did not run.
///
/// @param program - the program
/// @param args - its arguments
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program).args(args).output().map_err(|e| format!("{program} did not start: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("{program} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Writes a file, creating its folder.
///
/// @param path - the file
/// @param text - its contents
fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Starts a detached `atrius daemon` unless one is running.
///
/// @param exe - the `atrius` executable
fn start_daemon(exe: &Path) -> Result<(), String> {
    if daemon_pid().is_some() || dry() {
        return Ok(());
    }
    let mut command = Command::new(exe);
    command.arg("daemon").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    atrius_index::host::detach(&mut command);
    command.spawn().map(|_| ()).map_err(|e| format!("atrius daemon did not start: {e}"))
}

/// Installs the login item and starts the daemon. Answers what was done, as text.
///
/// @param exe - the `atrius` executable the login item runs
pub fn install(exe: &Path) -> Result<String, String> {
    let item = login_item_path();
    if cfg!(windows) {
        if !dry() {
            let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
            run("reg", &["add", key, "/v", &run_value_name(), "/t", "REG_SZ", "/d", &run_command(exe), "/f"])?;
        }
        start_daemon(exe)?;
    } else if cfg!(target_os = "macos") {
        write_file(&item, &launch_agent(exe))?;
        if !dry() {
            let domain = format!("gui/{}", crate::process::user_id());
            let _ = run("launchctl", &["bootout", &format!("{domain}/{LABEL}")]);
            run("launchctl", &["bootstrap", &domain, &item.to_string_lossy()])?;
        }
    } else if has_systemd() {
        write_file(&item, &systemd_unit(exe))?;
        if !dry() {
            run("systemctl", &["--user", "daemon-reload"])?;
            run("systemctl", &["--user", "enable", "--now", "atrius-index.service"])?;
        }
    } else {
        write_file(&item, &autostart_entry(exe))?;
        start_daemon(exe)?;
    }
    Ok(format!("installed the login item at {}\nthe service runs `{} daemon` and keeps the folders in {} indexed\n", item.display(), exe.display(), crate::watchlist::watch_file().display()))
}

/// Stops the daemon and removes the login item. Answers what was done, as text.
pub fn uninstall() -> Result<String, String> {
    let item = login_item_path();
    if cfg!(windows) {
        if !dry() {
            let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
            let _ = run("reg", &["delete", key, "/v", &run_value_name(), "/f"]);
        }
    } else if cfg!(target_os = "macos") {
        if !dry() {
            let _ = run("launchctl", &["bootout", &format!("gui/{}/{LABEL}", crate::process::user_id())]);
        }
        let _ = std::fs::remove_file(&item);
    } else {
        if item.extension().is_some_and(|e| e == "service") && !dry() {
            let _ = run("systemctl", &["--user", "disable", "--now", "atrius-index.service"]);
        }
        let _ = std::fs::remove_file(&item);
    }
    let stopped = match daemon_pid() {
        Some(pid) => crate::process::stop(pid),
        None => false,
    };
    let _ = std::fs::remove_file(pid_file());
    Ok(format!("removed the login item at {}{}\n", item.display(), if stopped { " and stopped the running daemon" } else { "" }))
}

/// Whether the login item exists on this platform.
pub fn installed() -> bool {
    if cfg!(windows) {
        if dry() {
            return false;
        }
        run("reg", &["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", "/v", &run_value_name()]).is_ok()
    } else {
        login_item_path().exists()
    }
}

/// The service's state, as the JSON `service status` prints.
pub fn status() -> Value {
    let pid = daemon_pid();
    let installed = installed();
    let watched = crate::watchlist::read();
    let text = format!(
        "login item: {} ({})\ndaemon: {}\nwatched folders: {}\n",
        if installed { "installed" } else { "not installed" },
        login_item_path().display(),
        pid.map(|p| format!("running, process {p}")).unwrap_or_else(|| "not running".into()),
        watched.len()
    );
    json!({
        "installed": installed,
        "loginItem": login_item_path().to_string_lossy(),
        "running": pid.is_some(),
        "pid": pid,
        "watched": watched.iter().map(|w| w.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "text": text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_platforms_login_item_runs_the_daemon() {
        let exe = Path::new("/opt/atrius/bin/atrius");
        let plist = launch_agent(exe);
        assert!(plist.contains("<string>/opt/atrius/bin/atrius</string>"));
        assert!(plist.contains("<string>daemon</string>"));
        assert!(plist.contains(LABEL));
        assert!(systemd_unit(exe).contains("ExecStart=\"/opt/atrius/bin/atrius\" daemon"));
        assert!(autostart_entry(exe).contains("Exec=\"/opt/atrius/bin/atrius\" daemon"));
        assert!(run_command(Path::new(r"C:\a b\atrius.exe")).ends_with(r#"--headless "C:\a b\atrius.exe" daemon"#));
    }
}
