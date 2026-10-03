//! The index host, and how a caller reaches it.
//!
//! **One host per checkout owns the index.** Inillucent lets several processes open a file, but a reader
//! waits for a writer, and two watchers on one tree would repeat each other's work. So whichever process
//! takes the lock in the index folder first is the host, and every other caller talks to it over a
//! loopback socket with a token. The host file beside the lock says the port and the token.
//!
//! **Who hosts depends on how long the caller lives.** An MCP server, an editor window and the background
//! service live for as long as somebody is working, so they host in their own process
//! ([`Hosting::InProcess`]) and answer their own calls with no socket at all. A one off `atrius find`
//! would throw a host away the moment it printed, so it starts a detached `atrius serve` instead
//! ([`Hosting::Spawn`]) and asks it. The next call finds it running.
//!
//! **A query never fails because the index is missing.** When no host can be reached or started, `find`
//! is answered by scanning the files directly and says `"index":"none"`.
//!
//! The lock, the host file and the commands are the same ones Unluminous uses, so an Unluminous window
//! and the `atrius` command line share one index of a checkout.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::index::Index;
use crate::protocol::{code, Reply, Request};
use crate::store::index_folder;
use crate::verbs;

/// How long a headless host waits with no request before it stops, unless `ATRIUS_IDLE_MINUTES` says
/// otherwise.
pub const IDLE_EXIT: Duration = Duration::from_secs(2 * 60 * 60);
/// How long a caller waits for a host it started to come up.
const START_WAIT: Duration = Duration::from_secs(20);

/// Where a caller's index host comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hosting {
    /// Host in this process when no other host is running. For an MCP server, a window or the service.
    InProcess,
    /// Start a detached `atrius serve` when no host is running. For a one off command.
    Spawn,
    /// Never host and never start a host: ask a running one, or scan directly.
    Never,
}

/// A running host: the index, the port it answers on, and the lock that makes it the only one.
pub struct Host {
    /// The checkout's index.
    pub index: Arc<Index>,
    /// The loopback port the host answers on.
    pub port: u16,
    /// The token a request must carry.
    pub token: String,
    last_request: Arc<Mutex<Instant>>,
    stopping: AtomicBool,
    _lock: File,
}

/// What the host file in the index folder says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostFile {
    /// The host's process id.
    pub pid: u32,
    /// The host's loopback port.
    pub port: u16,
    /// The token a request must carry.
    pub token: String,
    /// The checkout the host indexes.
    pub root: String,
    /// The program that is hosting, such as `atrius 0.1.0` or `unluminous-cli`.
    pub program: String,
}

/// The program a caller starts as a host, and the arguments before `--root <folder>`.
#[derive(Clone, Debug)]
struct ServerCommand {
    program: PathBuf,
    arguments: Vec<String>,
}

/// The hosts this process runs, by root.
fn hosts() -> &'static Mutex<HashMap<PathBuf, Arc<Host>>> {
    static HOSTS: OnceLock<Mutex<HashMap<PathBuf, Arc<Host>>>> = OnceLock::new();
    HOSTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The server command set by [`set_server_command`], if any.
fn server_command() -> &'static Mutex<Option<ServerCommand>> {
    static COMMAND: OnceLock<Mutex<Option<ServerCommand>>> = OnceLock::new();
    COMMAND.get_or_init(|| Mutex::new(None))
}

/// The name this process gives itself in the host file.
fn program_name() -> &'static Mutex<String> {
    static NAME: OnceLock<Mutex<String>> = OnceLock::new();
    NAME.get_or_init(|| Mutex::new(format!("atrius {}", crate::VERSION)))
}

/// Says which program [`Hosting::Spawn`] starts as a host, and with which arguments before
/// `--root <folder>`. Unluminous sets `unluminous-cli` with `search serve`. Without a call, the
/// `atrius` program is started with `serve`.
///
/// @param program - the executable
/// @param arguments - the arguments before `--root`
pub fn set_server_command(program: PathBuf, arguments: Vec<String>) {
    *server_command().lock().unwrap_or_else(|e| e.into_inner()) =
        Some(ServerCommand { program, arguments });
}

/// Says what this process calls itself in the host file, such as `unluminous 0.59.0`.
///
/// @param name - the program's name and version
pub fn set_program_name(name: &str) {
    *program_name().lock().unwrap_or_else(|e| e.into_inner()) = name.to_owned();
}

/// The project a search is about: the given folder, or the nearest folder at or above the working folder
/// that holds `.git`, or the working folder itself.
///
/// @param given - a `--root` argument, if one was given
pub fn project_root(given: Option<&str>) -> PathBuf {
    if let Some(given) = given.filter(|g| !g.trim().is_empty()) {
        return canonical(Path::new(given.trim()));
    }
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut at = Some(here.as_path());
    while let Some(folder) = at {
        if folder.join(".git").exists() {
            return canonical(folder);
        }
        at = folder.parent();
    }
    canonical(&here)
}

/// A folder's canonical path without Windows' `\\?\` prefix.
///
/// @param path - the folder
pub fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .map(|p| crate::index::strip_verbatim(&p))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// A token nobody else can guess: the hash of the time, the process and the root.
///
/// @param root - the root
fn new_token(root: &Path) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let seed = format!("{nanos}:{}:{}", std::process::id(), root.display());
    blake3::hash(seed.as_bytes()).to_hex().as_str()[..32].to_owned()
}

/// Reads a host file.
///
/// @param path - the file
fn read_host_file_at(path: &Path) -> Option<HostFile> {
    let text = std::fs::read_to_string(path).ok()?;
    let get = |name: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{name} = ")).map(str::trim).map(str::to_owned))
    };
    Some(HostFile {
        pid: get("pid")?.parse().ok()?,
        port: get("port")?.parse().ok()?,
        token: get("token")?,
        root: get("root").unwrap_or_default(),
        program: get("program").unwrap_or_default(),
    })
}

/// Reads the host file of a root, if there is one.
///
/// @param root - the root
pub fn read_host_file(root: &Path) -> Option<HostFile> {
    read_host_file_at(&index_folder(root).join("host.conf"))
}

/// Opens the host's lock file and holds it, or answers `None` when another process holds it. The lock
/// lasts as long as the returned file is open, which is the life of the host. On Windows the file is
/// opened with no sharing, so a second open fails while the first is held. On Unix it is `flock` with
/// `LOCK_EX | LOCK_NB`.
///
/// @param path - the lock file
fn take_the_lock(path: &Path) -> Option<File> {
    let mut options = File::options();
    options.create(true).truncate(false).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    let file = options.open(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: the descriptor belongs to `file`, which is open for the whole call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return None;
        }
    }
    Some(file)
}

impl Host {
    /// Becomes the host of a root: takes the lock, opens the index, listens on a loopback port and
    /// writes the host file. Fails when another process already holds the lock.
    ///
    /// @param root - the root
    pub fn start(root: &Path) -> Result<Arc<Host>, String> {
        let folder = index_folder(root);
        std::fs::create_dir_all(&folder)
            .map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
        let lock = take_the_lock(&folder.join("host.lock")).ok_or_else(|| {
            "another process is already the index host for this folder".to_owned()
        })?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let token = new_token(root);
        let index = Index::open(root);
        let host = Arc::new(Host {
            index,
            port,
            token: token.clone(),
            last_request: Arc::new(Mutex::new(Instant::now())),
            stopping: AtomicBool::new(false),
            _lock: lock,
        });
        let program = program_name().lock().unwrap_or_else(|e| e.into_inner()).clone();
        let file = format!(
            "pid = {}\nport = {port}\ntoken = {token}\nroot = {}\nprogram = {program}\n",
            std::process::id(),
            root.display()
        );
        std::fs::write(folder.join("host.conf"), file).map_err(|e| e.to_string())?;
        let serving = Arc::clone(&host);
        std::thread::Builder::new()
            .name("atrius-index-listen".into())
            .spawn(move || serving.listen(listener))
            .map_err(|e| e.to_string())?;
        Ok(host)
    }

    /// Answers connections, one thread each, one request a connection.
    ///
    /// @param listener - the bound socket
    fn listen(self: Arc<Self>, listener: TcpListener) {
        for stream in listener.incoming().flatten() {
            let host = Arc::clone(&self);
            let _ = std::thread::Builder::new()
                .name("atrius-index-call".into())
                .spawn(move || host.serve_one(stream));
        }
    }

    /// Reads one request from a connection, checks its token, answers it and writes the reply.
    ///
    /// @param stream - the connection
    fn serve_one(&self, stream: TcpStream) {
        let Ok(mut writer) = stream.try_clone() else { return };
        let mut line = String::new();
        if BufReader::new(stream).read_line(&mut line).is_err() {
            return;
        }
        let reply = match serde_json::from_str::<Value>(line.trim())
            .ok()
            .as_ref()
            .and_then(Request::from_json)
        {
            Some(request) if request.token == self.token => {
                self.answer(&request.command, &request.arguments)
            }
            Some(request) => Reply::failed(
                &request.command,
                code::REFUSED,
                "The token does not match this index host.",
            ),
            None => Reply::failed("", code::USAGE, "That was not a request."),
        };
        let _ = writeln!(writer, "{}", reply.to_json());
        let _ = writer.flush();
    }

    /// Answers one command in this process.
    ///
    /// @param command - the command, such as `find` or `search.find`
    /// @param arguments - the command's arguments
    pub fn answer(&self, command: &str, arguments: &Map<String, Value>) -> Reply {
        *self.last_request.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        let verb = command.strip_prefix("search.").unwrap_or(command);
        match verb {
            "serve" => Reply::done(
                command,
                json!({
                    "host": self.describe(),
                    "text": format!("the index host is already running on port {}\n", self.port),
                }),
            ),
            "stop" => {
                self.stopping.store(true, Ordering::SeqCst);
                Reply::done(
                    command,
                    json!({ "host": self.describe(), "text": "the index host is stopping\n" }),
                )
            }
            "status" => match verbs::answer(&self.index, "status", arguments) {
                Ok(mut value) => {
                    value["host"] = self.describe();
                    Reply::done(command, value)
                }
                Err(refusal) => Reply::failed(command, refusal.code, refusal.message),
            },
            _ => match verbs::answer(&self.index, verb, arguments) {
                Ok(value) => Reply::done(command, value),
                Err(refusal) => Reply::failed(command, refusal.code, refusal.message),
            },
        }
    }

    /// The host's own facts for `status`.
    fn describe(&self) -> Value {
        json!({
            "pid": std::process::id(),
            "port": self.port,
            "inProcess": true,
            "program": program_name().lock().unwrap_or_else(|e| e.into_inner()).clone(),
        })
    }

    /// Whether `stop` has been asked of this host.
    pub fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// How long since the last request.
    pub fn idle_for(&self) -> Duration {
        self.last_request.lock().unwrap_or_else(|e| e.into_inner()).elapsed()
    }

    /// Removes the host file, so no caller tries this host after it stops.
    pub fn retire(&self) {
        let file = index_folder(self.index.root()).join("host.conf");
        if read_host_file_at(&file).is_some_and(|f| f.pid == std::process::id()) {
            let _ = std::fs::remove_file(file);
        }
        hosts().lock().unwrap_or_else(|e| e.into_inner()).remove(self.index.root());
    }

    /// Blocks until the host has had no request for `idle`, or `stop` was asked, then removes its host
    /// file. `None` waits for `stop` alone.
    ///
    /// @param idle - how long a quiet host lives
    pub fn serve_until_idle(&self, idle: Option<Duration>) {
        loop {
            std::thread::sleep(Duration::from_millis(250));
            if self.is_stopping() || idle.is_some_and(|limit| self.idle_for() > limit) {
                self.retire();
                return;
            }
        }
    }
}

/// How long a headless host lives with no request: `ATRIUS_IDLE_MINUTES`, where 0 means for ever, or
/// [`IDLE_EXIT`].
pub fn idle_limit() -> Option<Duration> {
    match std::env::var("ATRIUS_IDLE_MINUTES").ok().and_then(|m| m.trim().parse::<u64>().ok()) {
        Some(0) => None,
        Some(minutes) => Some(Duration::from_secs(minutes * 60)),
        None => Some(IDLE_EXIT),
    }
}

/// The host this process runs for a root, starting one when no other process is the host.
///
/// @param root - the root
pub fn in_process(root: &Path) -> Option<Arc<Host>> {
    let mut hosts = hosts().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(host) = hosts.get(root) {
        return Some(Arc::clone(host));
    }
    let host = Host::start(root).ok()?;
    hosts.insert(root.to_path_buf(), Arc::clone(&host));
    Some(host)
}

/// Asks another process's host, if the host file names one that answers.
///
/// @param root - the root
/// @param command - the command
/// @param arguments - the arguments
/// @param timeout - how long to wait
fn ask_running(
    root: &Path,
    command: &str,
    arguments: &Map<String, Value>,
    timeout: Duration,
) -> Option<Reply> {
    let file = read_host_file(root)?;
    if file.pid == std::process::id() {
        return None;
    }
    let request = Request::new(&file.token, command, arguments.clone());
    crate::protocol::ask(file.port, &request, timeout).ok()
}

/// The program and arguments a caller starts as a host.
fn spawn_command() -> Option<ServerCommand> {
    if let Some(set) = server_command().lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Some(set);
    }
    let exe = std::env::current_exe().ok()?;
    // This program when it is `atrius` or `atrius-mcp` under any name that starts with `atrius` (a copy
    // such as `atrius-acf5c46.exe` is how the evaluation runs it), and `atrius` beside it otherwise.
    let is_atrius = exe.file_stem().is_some_and(|s| s.to_string_lossy().starts_with("atrius"));
    let program = if is_atrius {
        exe
    } else {
        exe.with_file_name(if cfg!(windows) { "atrius.exe" } else { "atrius" })
    };
    Some(ServerCommand { program, arguments: vec!["serve".into()] })
}

/// Starts a detached host for a root and waits for its host file to name a port that answers.
///
/// @param root - the root
fn spawn_host(root: &Path) -> bool {
    let Some(server) = spawn_command() else { return false };
    let mut command = std::process::Command::new(&server.program);
    command
        .args(&server.arguments)
        .arg("--root")
        .arg(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    detach(&mut command);
    if command.spawn().is_err() {
        return false;
    }
    let deadline = Instant::now() + START_WAIT;
    while Instant::now() < deadline {
        if let Some(file) = read_host_file(root) {
            if TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], file.port)),
                Duration::from_millis(200),
            )
            .is_ok()
            {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

/// Makes a command start a process that outlives this one and has no console or terminal of its own.
///
/// @param command - the command
pub fn detach(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW: the host outlives this command
        // and never opens a console of its own.
        command.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000);
        keep_standard_handles_to_ourselves();
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // A new session, so closing the terminal that ran the command does not stop the host.
        // SAFETY: setsid is async signal safe and touches nothing but the child's own session.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
}

/// Marks this process's standard input, output and error as not inheritable, before it starts a host.
///
/// Windows hands a new process every inheritable handle its parent holds, not only the three it is
/// given. A command run from a shell or from an agent's Bash tool writes to a pipe, and the shell
/// waits for that pipe to close. A host that inherited it would hold it open for as long as the host
/// runs, so the command would look as if it never finished.
#[cfg(windows)]
fn keep_standard_handles_to_ourselves() {
    use windows_sys::Win32::Foundation::{
        SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: GetStdHandle returns this process's own handle or null, and SetHandleInformation only
        // changes the inherit flag of a handle this process holds.
        unsafe {
            let handle = GetStdHandle(which);
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

/// Answers a command for a root: through this process's host, another process's host, a host started
/// for the purpose, or, when none of those can be had, a direct scan.
///
/// @param root - the root
/// @param command - the command, such as `find`
/// @param arguments - the arguments
/// @param timeout - how long to wait for another process
/// @param hosting - whether this process may host or start a host
pub fn ask(
    root: &Path,
    command: &str,
    arguments: &Map<String, Value>,
    timeout: Duration,
    hosting: Hosting,
) -> Reply {
    // A folder that is not there would otherwise be given a host and an empty index, and every
    // answer about it would look like a project with nothing in it.
    if !root.is_dir() {
        return Reply::failed(
            command,
            code::NOT_FOUND,
            format!("There is no folder at {}.", root.display()),
        );
    }
    if let Some(host) = hosts().lock().unwrap_or_else(|e| e.into_inner()).get(root).cloned() {
        return host.answer(command, arguments);
    }
    if let Some(reply) = ask_running(root, command, arguments, timeout) {
        return reply;
    }
    match hosting {
        Hosting::InProcess => {
            if let Some(host) = in_process(root) {
                return host.answer(command, arguments);
            }
        }
        Hosting::Spawn => {
            if spawn_host(root) {
                if let Some(reply) = ask_running(root, command, arguments, timeout) {
                    return reply;
                }
            }
        }
        Hosting::Never => {}
    }
    if let Some(reply) = ask_running(root, command, arguments, timeout) {
        return reply;
    }
    without_a_host(root, command, arguments)
}

/// The answer when no host can be had: `find` by scanning the files, anything else refused.
///
/// @param root - the root
/// @param command - the command
/// @param arguments - the arguments
fn without_a_host(root: &Path, command: &str, arguments: &Map<String, Value>) -> Reply {
    let verb = command.strip_prefix("search.").unwrap_or(command);
    if verb != "find" {
        return Reply::failed(
            command,
            code::NOT_RUNNING,
            "No index host is running for this folder and none could be started, and only `find` can be answered without one.",
        );
    }
    match verbs::scan_without_index(root, arguments) {
        Ok(mut value) => {
            if arguments.get("structured").and_then(Value::as_bool).unwrap_or(false) {
                value["structured"] = Value::Bool(true);
            }
            Reply::done(command, value)
        }
        Err(refusal) => Reply::failed(command, refusal.code, refusal.message),
    }
}

/// Runs a headless host for a root in this process and does not return until it has been idle for
/// `idle` or was asked to stop. Exit code 0, or 1 when another process is already the host.
///
/// @param root - the root
/// @param idle - how long a quiet host lives; `None` for ever
pub fn serve(root: &Path, idle: Option<Duration>) -> Result<(), String> {
    let host = Host::start(root)?;
    hosts().lock().unwrap_or_else(|e| e.into_inner()).insert(root.to_path_buf(), Arc::clone(&host));
    host.serve_until_idle(idle);
    Ok(())
}

/// One host found in the cache folder.
#[derive(Clone, Debug)]
pub struct RunningHost {
    /// What its host file says.
    pub file: HostFile,
    /// Whether its port answered a connection.
    pub answering: bool,
}

/// Every host whose host file is in the cache folder, with whether each one answers.
pub fn running_hosts() -> Vec<RunningHost> {
    let base = crate::store::cache_folder().join("atrius-index").join("index");
    let Ok(entries) = std::fs::read_dir(base) else { return Vec::new() };
    let mut out: Vec<RunningHost> = entries
        .flatten()
        .filter_map(|entry| read_host_file_at(&entry.path().join("host.conf")))
        .map(|file| {
            let answering = TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], file.port)),
                Duration::from_millis(300),
            )
            .is_ok();
            RunningHost { file, answering }
        })
        .collect();
    out.sort_by(|a, b| a.file.root.cmp(&b.file.root));
    out
}

/// Asks the host of a root to stop. Answers whether one was running and was asked.
///
/// @param root - the root
pub fn stop(root: &Path) -> bool {
    if let Some(host) = hosts().lock().unwrap_or_else(|e| e.into_inner()).get(root).cloned() {
        host.stopping.store(true, Ordering::SeqCst);
        return true;
    }
    ask_running(root, "stop", &Map::new(), Duration::from_secs(5)).is_some_and(|reply| reply.ok)
}
