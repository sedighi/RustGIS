//! A loopback relay: an MCP client drives the session of the desktop app that is already open.
//!
//! The app starts a listener with its `ui.mcpRelay` command (off until then), and
//! `rustgis-cli mcp --attach` connects to it. The relay is the same [`crate::mcp`] protocol as
//! `rustgis-cli mcp`, one JSON-RPC message per line, so there is no second command protocol:
//! every action reaches the open [`Session`] through [`Session::run`], one undo step per command,
//! exactly as the private-session server does.
//!
//! Safety: the listener binds 127.0.0.1 only ([`Relay::bind`] refuses any other address before
//! opening a socket). Each run draws a token from the operating system's random source and writes
//! it to a private file ([`token_path`]); a connection is dropped unless its first message is the
//! relay's `rustgis/attach` carrying that token. The attach's `access` flag then decides what that
//! connection allows; the existing [`Server::refusal`] policy still runs per call.
//!
//! Limits: connections, unauthenticated connections, queued messages, the first line before the
//! token is checked, and every read and write wait are bounded. A command whose reply does not
//! come in time is cancelled before the app can run it. Stopping the relay, or dropping its last
//! owner, closes the listener and every client and removes the token file.
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rustgis_engine::Session;
use serde_json::Value;

use crate::framing::{self, MAX_LINE, Oversize, read_message_max};
use crate::mcp::{Server, expects_reply};

/// The port `rustgis-cli mcp --attach` connects to unless it names another. (GeoLibre's runner
/// is 8766; the relay takes the next one.)
pub const DEFAULT_PORT: u16 = 8767;

/// The most bytes in a line before the token is checked: an attach is far smaller than this.
const MAX_PREAUTH_LINE: u64 = 4096;
/// Open connections, authenticated or not.
const MAX_CONNECTIONS: usize = 16;
/// Connections that have not attached yet.
const MAX_PREAUTH_CONNECTIONS: usize = 4;
/// Messages waiting for the app's frame.
const MAX_QUEUE: usize = 64;
/// How long a new connection may take to send its attach.
const PREAUTH_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an attached client may stay silent before the relay closes it.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// How long a reply may take to be written to a client.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a request waits for the app to run it; after that it is cancelled.
const REPLY_WAIT: Duration = Duration::from_secs(60);
/// How often the listener, a connection waiting for a reply, and a connection waiting for its
/// client's next line check whether the relay was stopped.
const STOP_POLL: Duration = Duration::from_millis(25);

/// The connection message: `{"jsonrpc":"2.0","method":"rustgis/attach","params":{"access":bool,
/// "token":"…"}}`. It carries no commands; it proves the client is this user's own (`token`,
/// read from [`token_path`]) and says what its `--yes` allows, before its first call.
const ATTACH: &str = "rustgis/attach";

/// The states of a queued request. The connection and the app race for it: the first to move
/// it out of `PENDING` decides. A cancelled request never runs.
const PENDING: u8 = 0;
const RUNNING: u8 = 1;
const CANCELLED: u8 = 2;

/// A queued message: its line, what its connection allows, where its reply goes, and its state.
struct Request {
    line: String,
    /// The token its connection attached with and whether it asked for file and network access
    /// (`--yes`): what it may do is decided when it runs, so a revoked grant or the app's changed
    /// setting applies to requests already queued.
    token: String,
    asked: bool,
    reply: SyncSender<Option<String>>,
    state: Arc<AtomicU8>,
}

/// State shared by the owner, the accept loop and the connections. It never refers back to the
/// owner, so dropping the last [`Relay`] is what shuts the relay down.
struct Shared {
    stopped: AtomicBool,
    requests: Mutex<VecDeque<Request>>,
    /// The run's attach token: only a connection that sends it in its first message stays open.
    token: String,
    /// The token file this run wrote; removed when the relay stops.
    token_file: PathBuf,
    /// What to run when a message is queued or a client attaches or leaves, so the UI thread
    /// draws a frame to run the message or repaint the client count.
    wake: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// Open connections, by id, so stopping the relay can shut their sockets.
    streams: Mutex<HashMap<u64, TcpStream>>,
    next_id: AtomicU64,
    /// Connections that have not attached yet.
    preauth: AtomicUsize,
    /// Authenticated connections.
    attached: AtomicUsize,
    /// How long a request waits for the app before it is cancelled.
    reply_wait: Duration,
    /// The port, naming the grants' token files.
    port: u16,
    /// The app lets agents attached with the relay's own token use files and the network, whether
    /// or not they asked (`--yes`): the user turned agents on in the app ([`Relay::set_allow_access`]).
    allow_access: AtomicBool,
    /// Run-scoped tokens and what each allows ([`Relay::grant`]), by token.
    grants: Mutex<HashMap<String, (bool, PathBuf)>>,
    /// The connections attached with a grant, by connection id, so revoking it closes them.
    granted: Mutex<HashMap<u64, String>>,
    grant_ids: AtomicU64,
    /// This run's own id (random, not secret), naming its grants' files: a grant left over from an
    /// earlier run on the same port can't remove a file of this one.
    run_id: String,
}

impl Shared {
    /// Wakes the UI thread so it draws a frame to run queued messages or repaint the client
    /// count. Does nothing when the app never set a callback with [`Relay::set_wake`].
    fn notify(&self) {
        if let Some(wake) = lock(&self.wake).as_ref() {
            wake();
        }
    }

    /// Stops the listener, closes every client, drops every queued request (its connection sees
    /// the closed reply channel) and removes this run's token file. Idempotent.
    fn shutdown(&self) {
        if self.stopped.swap(true, SeqCst) {
            return;
        }
        let streams: Vec<TcpStream> = lock(&self.streams).drain().map(|(_, s)| s).collect();
        for stream in streams {
            let _ = stream.shutdown(Shutdown::Both);
        }
        // Pending requests are cancelled before they are dropped, so none can run after stop.
        for req in lock(&self.requests).drain(..) {
            let _ = req.state.compare_exchange(PENDING, CANCELLED, SeqCst, SeqCst);
        }
        let _ = std::fs::remove_file(&self.token_file);
        for (_, (_, file)) in lock(&self.grants).drain() {
            let _ = std::fs::remove_file(&file);
        }
        self.notify();
    }

    /// Whether a connection attached with `token` may use files and the network, given whether
    /// it asked (`--yes`): the relay's own token as it asked, or always when the app allows it; a
    /// grant as the grant says. `None` for a token that isn't (or is no longer) valid.
    fn access(&self, token: &str, asked: bool) -> Option<bool> {
        if same_token(token, &self.token) {
            return Some(asked || self.allow_access.load(SeqCst));
        }
        lock(&self.grants).iter().find(|(t, _)| same_token(token, t)).map(|(_, (access, _))| *access)
    }
}

/// A run-scoped token ([`Relay::grant`]): its file, for `rustgis-cli mcp --attach PORT
/// --token-file FILE`. Dropping it revokes the token, removes the file and closes the
/// connections that attached with it.
pub struct Grant {
    token: String,
    file: PathBuf,
    shared: std::sync::Weak<Shared>,
}

impl Grant {
    /// The token's file, private to this user.
    pub fn token_file(&self) -> &std::path::Path {
        &self.file
    }
}

impl Drop for Grant {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
        let Some(shared) = self.shared.upgrade() else { return };
        lock(&shared.grants).remove(&self.token);
        let ids: Vec<u64> = lock(&shared.granted).iter().filter(|(_, t)| **t == self.token).map(|(id, _)| *id).collect();
        let streams = lock(&shared.streams);
        for id in ids {
            if let Some(stream) = streams.get(&id) {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
    }
}

/// The owner of a running listener. The last [`Relay`] clone that drops stops the relay.
struct Owner {
    shared: Arc<Shared>,
    port: u16,
}

impl Drop for Owner {
    /// Stops the relay when its last owner drops: the listener closes, clients disconnect,
    /// queued requests are cancelled unrun, and the token file is removed.
    fn drop(&mut self) {
        self.shared.shutdown();
    }
}

/// A running listener. Messages queue for [`Relay::poll`], which runs them on the thread that
/// owns the [`Session`] (the app's UI thread), so the session never leaves its owner. Clones
/// share one listener: dropping a clone does nothing, and [`Relay::stop`] stops it for all.
#[derive(Clone)]
pub struct Relay {
    owner: Arc<Owner>,
}

impl std::fmt::Debug for Relay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Relay").field("port", &self.owner.port).finish()
    }
}

/// The folder that holds the run tokens. `$XDG_RUNTIME_DIR/rustgis` when that is an absolute
/// path to a folder (the per-user runtime folder); otherwise a folder named for the user in the
/// system's temporary folder. [`token_path`] and [`read_token`] use the same folder.
pub fn token_dir() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute() && p.is_dir()) {
        return runtime.join("rustgis");
    }
    let user: String = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let user = if user.is_empty() { "user".to_string() } else { user };
    std::env::temp_dir().join(format!("rustgis-mcp-{user}"))
}

/// The token file for `port`, in [`token_dir`]. The relay writes it when it starts (readable by
/// this user only) and removes it when it stops; `rustgis-cli mcp --attach` reads it through
/// [`read_token`], so no token ever travels through a command line, an argument or the clipboard.
pub fn token_path(port: u16) -> PathBuf {
    token_dir().join(format!("{port}.token"))
}

/// Reads the token the running relay on `port` wrote, for `rustgis-cli mcp --attach`. The folder
/// and file must be this user's: a regular file, not a symlink, both owned by the user running
/// this process, and private to that user (no group or other access).
pub fn read_token(port: u16) -> Result<String, String> {
    read_token_file(&token_path(port))
}

/// Reads a token file the relay wrote: its own ([`token_path`]) or a grant's
/// ([`Grant::token_file`]), with the same checks as [`read_token`].
pub fn read_token_file(path: &std::path::Path) -> Result<String, String> {
    let hint = "check the token file path (--token-file) and whether its relay or grant is still active";
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e} ({hint})", path.display()))?;
    if !meta.is_file() || meta.len() > 1024 {
        return Err(format!("{}: not a token file; refused", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
        let dir_meta = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let me = rustix::process::geteuid().as_raw();
        if !dir_meta.is_dir() || dir_meta.uid() != me || meta.uid() != me {
            return Err(format!("{}: not owned by this user; refused", path.display()));
        }
        if dir_meta.permissions().mode() & 0o077 != 0 || meta.permissions().mode() & 0o077 != 0 {
            return Err(format!("{}: readable by others; refused", path.display()));
        }
    }
    let token = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let token = token.trim();
    if token.is_empty() {
        return Err(format!("{}: holds no token ({hint})", path.display()));
    }
    Ok(token.to_string())
}

/// Makes sure [`token_dir`] exists as a private folder owned by the user running this process,
/// and returns it. A folder that is a symlink, is not a folder, belongs to another user or is
/// readable by anyone else is refused, not repaired.
fn private_dir() -> Result<PathBuf, String> {
    let dir = token_dir();
    // Only Unix sets a mode on the builder; elsewhere it stays as created.
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    match builder.create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    }
    let meta = std::fs::symlink_metadata(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !meta.is_dir() {
        return Err(format!("{}: not a folder; refused", dir.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.uid() != rustix::process::geteuid().as_raw() {
            return Err(format!("{}: owned by another user; refused", dir.display()));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(format!("{}: readable by others; it must be private (mode 0700)", dir.display()));
        }
    }
    Ok(dir)
}

/// A fresh token for one relay run: 16 bytes from the operating system's random source, as 32
/// hex characters.
fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| format!("the MCP relay could not read the operating system's random source: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Writes the run's token for `port` into the private folder, readable by this user only, and
/// returns its path. A previous run's file (or a symlink in its place) is replaced, never
/// written through.
fn write_token_file(port: u16, token: &str) -> Result<PathBuf, String> {
    write_private(&format!("{port}.token"), token)
}

/// Writes `token` to the file `name` in the private folder, readable by this user only.
fn write_private(name: &str, token: &str) -> Result<PathBuf, String> {
    let dir = private_dir()?;
    let path = dir.join(name);
    // `remove_file` removes a symlink itself, not what it points to; `create_new` then never
    // follows anything that is left.
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e} (a token file this user cannot remove; remove it to start the MCP relay)", path.display())),
    }
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let file_uid = f.metadata().map_err(|e| format!("{}: {e}", path.display()))?.uid();
        if file_uid != rustix::process::geteuid().as_raw() {
            let _ = std::fs::remove_file(&path);
            return Err(format!("{}: not owned by this user; refused", path.display()));
        }
    }
    let written = f.write_all(token.as_bytes()).and_then(|()| f.write_all(b"\n"));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&path);
        return Err(format!("{}: {e}", path.display()));
    }
    Ok(path)
}

/// Locks a relay mutex, recovering the contents when a previous holder panicked: a poisoned
/// mutex would otherwise stop the listener, so the relay keeps its guarded state and runs on.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Relay {
    /// Binds `addr`. Anything but a loopback address is refused before a socket is opened, so a
    /// relay can never be reached from another computer.
    pub fn bind(addr: SocketAddr) -> Result<Self, String> {
        Self::bind_with(addr, REPLY_WAIT)
    }

    /// Binds 127.0.0.1:`port`; 0 asks the system for a free port.
    pub fn start(port: u16) -> Result<Self, String> {
        Self::bind(SocketAddr::from(([127, 0, 0, 1], port)))
    }

    /// [`Relay::bind`] with the time a request waits for the app before it is cancelled.
    fn bind_with(addr: SocketAddr, reply_wait: Duration) -> Result<Self, String> {
        if !addr.ip().is_loopback() {
            return Err(format!("{addr}: the MCP relay listens on the loopback interface only"));
        }
        let listener = TcpListener::bind(addr).map_err(|e| format!("{addr}: {e}"))?;
        let port = listener.local_addr().map_err(|e| format!("{addr}: {e}"))?.port();
        listener.set_nonblocking(true).map_err(|e| format!("{addr}: {e}"))?;
        // The token is written before the accept loop starts, so no client can attach before it
        // exists. If anything below fails, the owner that holds the token file drops and removes it.
        let token = random_token()?;
        let token_file = write_token_file(port, &token)?;
        let shared = Arc::new(Shared {
            stopped: AtomicBool::new(false),
            requests: Mutex::new(VecDeque::new()),
            token,
            token_file,
            wake: Mutex::new(None),
            streams: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            preauth: AtomicUsize::new(0),
            attached: AtomicUsize::new(0),
            reply_wait,
            port,
            allow_access: AtomicBool::new(false),
            grants: Mutex::new(HashMap::new()),
            granted: Mutex::new(HashMap::new()),
            grant_ids: AtomicU64::new(0),
            run_id: random_token()?.chars().take(8).collect(),
        });
        let owner = Arc::new(Owner { shared, port });
        let accepting = Arc::clone(&owner.shared);
        std::thread::Builder::new()
            .name("rustgis-mcp-relay".into())
            .spawn(move || accept_loop(listener, accepting))
            .map_err(|e| format!("the MCP relay could not start: {e}"))?;
        Ok(Self { owner })
    }

    /// The port the listener is on (what `start(0)` chose).
    pub fn port(&self) -> u16 {
        self.owner.port
    }

    /// Whether the listener is still running (false after [`Relay::stop`] or the last owner's drop).
    pub fn is_running(&self) -> bool {
        !self.owner.shared.stopped.load(SeqCst)
    }

    /// How many authenticated clients are attached right now. Unauthenticated connections are
    /// not counted.
    pub fn attached_clients(&self) -> usize {
        self.owner.shared.attached.load(SeqCst)
    }

    /// Stops the listener: it closes, every client is disconnected, queued messages are dropped
    /// unrun, and the token file is removed. Idempotent.
    pub fn stop(&self) {
        self.owner.shared.shutdown();
    }

    /// What runs when a message is queued, or a client attaches or leaves: [`Relay::poll`] only
    /// runs messages on a frame the UI thread draws, and in reactive mode egui draws none while
    /// the window is idle, so the app passes its `request_repaint` here.
    pub fn set_wake(&self, wake: Box<dyn Fn() + Send + Sync>) {
        *lock(&self.owner.shared.wake) = Some(wake);
    }

    /// Lets agents attached with the relay's own token use files and the network without asking
    /// (`--yes`), or goes back to what each asked. The desktop app allows it while the user has
    /// agents turned on. It applies to their next call.
    pub fn set_allow_access(&self, allow: bool) {
        self.owner.shared.allow_access.store(allow, SeqCst);
    }

    /// A run-scoped token for an agent the app starts itself: connections that attach with it may
    /// use files and the network only if `access`. Dropping the [`Grant`] revokes it.
    pub fn grant(&self, access: bool) -> Result<Grant, String> {
        let shared = &self.owner.shared;
        let token = random_token()?;
        let n = shared.grant_ids.fetch_add(1, SeqCst);
        let file = write_private(&format!("{}-{}-{n}.token", shared.port, shared.run_id), &token)?;
        let mut grants = lock(&shared.grants);
        // Checked under the lock `shutdown` drains: a grant made as the relay stops would outlive
        // it with its file left behind.
        if shared.stopped.load(SeqCst) {
            drop(grants);
            let _ = std::fs::remove_file(&file);
            return Err("the MCP relay has stopped".into());
        }
        grants.insert(token.clone(), (access, file.clone()));
        drop(grants);
        Ok(Grant { token, file, shared: Arc::downgrade(shared) })
    }

    /// Runs the messages that arrived since the last call, on `s`. Returns how many ran. The
    /// calls go through [`Session::run`], so each is one undo step as everywhere else. A message
    /// whose connection already gave up is skipped, never run.
    pub fn poll(&self, s: &mut Session) -> usize {
        let pending: Vec<Request> = lock(&self.owner.shared.requests).drain(..).collect();
        // The skills switched on, read once for the batch and only when a message asks for them:
        // this runs on the app's thread, and reading the skill folders is file I/O.
        let mut skills: Option<Vec<crate::skills::Skill>> = None;
        let mut ran = 0;
        for req in pending {
            if req.state.compare_exchange(PENDING, RUNNING, SeqCst, SeqCst).is_err() {
                continue;
            }
            // Claimed first, then checked: a stop that came before the claim keeps the whole
            // batch from running (its connections are already closed).
            if self.owner.shared.stopped.load(SeqCst) {
                req.state.store(CANCELLED, SeqCst);
                continue;
            }
            // Decided now, not when it was queued; a grant revoked meanwhile cancels it.
            let Some(allow_access) = self.owner.shared.access(&req.token, req.asked) else {
                req.state.store(CANCELLED, SeqCst);
                continue;
            };
            let mut server = Server::new(allow_access);
            if asks_for_skills(&req.line) {
                server.skills = skills.get_or_insert_with(crate::skills::installed).clone();
            }
            // A last resort: a panic answers that message with an error instead of breaking the
            // app (Session::run catches panics inside commands; this covers the tools around it).
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| server.handle(s, &req.line)))
                .unwrap_or_else(|_| Some(internal_error(&req.line)));
            let _ = req.reply.send(out); // the connection may have hung up
            ran += 1;
        }
        ran
    }
}

/// Whether a message (or a message in a batch) is `prompts/list` or `prompts/get`, the only ones
/// that read skills.
fn asks_for_skills(line: &str) -> bool {
    let prompts = |m: &Value| matches!(m.get("method").and_then(Value::as_str), Some("prompts/list" | "prompts/get"));
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Array(batch)) => batch.iter().any(prompts),
        Ok(m) => prompts(&m),
        Err(_) => false,
    }
}

/// Accepts connections until the relay is stopped, then closes the listener.
fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    while !shared.stopped.load(SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => admit(stream, &shared),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(STOP_POLL),
            Err(e) => {
                log::warn!("MCP relay: {e}");
                std::thread::sleep(STOP_POLL);
            }
        }
    }
}

/// One connection's bookkeeping. Dropping it (however the connection ends) releases its place in
/// the limits, removes its socket, and repaints the client count if it had attached.
struct Slot {
    shared: Arc<Shared>,
    id: u64,
    preauth: bool,
    attached: bool,
}

impl Drop for Slot {
    /// Releases the connection's place in the limits, removes its socket, and repaints the
    /// client count if it had attached.
    fn drop(&mut self) {
        lock(&self.shared.streams).remove(&self.id);
        lock(&self.shared.granted).remove(&self.id);
        if self.preauth {
            self.shared.preauth.fetch_sub(1, SeqCst);
        }
        if self.attached {
            self.shared.attached.fetch_sub(1, SeqCst);
            self.shared.notify();
        }
    }
}

/// Starts a thread for an accepted stream, or refuses it when a limit is reached.
fn admit(stream: TcpStream, shared: &Arc<Shared>) {
    if shared.preauth.fetch_add(1, SeqCst) >= MAX_PREAUTH_CONNECTIONS {
        shared.preauth.fetch_sub(1, SeqCst);
        return;
    }
    let mut slot = Slot { shared: Arc::clone(shared), id: shared.next_id.fetch_add(1, SeqCst), preauth: true, attached: false };
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let Ok(clone) = stream.try_clone() else { return };
    {
        let mut streams = lock(&shared.streams);
        // Checked under the lock `shutdown` drains: a connection accepted as the relay stops is
        // closed here rather than outliving the stop.
        if shared.stopped.load(SeqCst) || streams.len() >= MAX_CONNECTIONS {
            return;
        }
        streams.insert(slot.id, clone);
    }
    let spawned = std::thread::Builder::new().name("rustgis-mcp-relay-connection".into()).spawn(move || connection(stream, &mut slot));
    if let Err(e) = spawned {
        log::warn!("MCP relay: {e}");
    }
}

/// A line that starts like an HTTP request (`POST / HTTP/1.1`) or response is not the JSON line
/// protocol: a web page can reach a loopback port with `fetch`, and it must get nowhere.
fn looks_like_http(line: &str) -> bool {
    const METHODS: &[&str] = &["GET ", "POST ", "PUT ", "HEAD ", "DELETE ", "OPTIONS ", "PATCH ", "CONNECT ", "TRACE "];
    line.starts_with("HTTP/") || METHODS.iter().any(|m| line.starts_with(m))
}

/// Compares two tokens without stopping at the first difference.
fn same_token(given: &str, expected: &str) -> bool {
    given.len() == expected.len() && given.bytes().zip(expected.bytes()).fold(0u8, |d, (a, b)| d | (a ^ b)) == 0
}

/// The attaching client's reads, under one deadline for the whole handshake, so a client cannot
/// hold a connection open by sending a byte at a time. Once attached (`deadline` is `None`) each
/// read waits up to [`IDLE_TIMEOUT`]. Reads wake every [`STOP_POLL`] to see whether the relay
/// was stopped: shutting a cloned socket does not wake a blocked read on every platform (not on
/// Windows), so the connection would otherwise outlive the stop by the whole idle timeout.
struct Handshake {
    stream: TcpStream,
    deadline: Option<Instant>,
    shared: Arc<Shared>,
}

impl Read for Handshake {
    /// Reads under the handshake deadline (or [`IDLE_TIMEOUT`] once attached), so a slow client
    /// cannot hold a connection open byte by byte; an expired deadline surfaces as a timeout.
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let deadline = self.deadline.unwrap_or_else(|| Instant::now() + IDLE_TIMEOUT);
        loop {
            if self.shared.stopped.load(SeqCst) {
                return Err(std::io::Error::from(std::io::ErrorKind::ConnectionAborted));
            }
            let left = deadline
                .checked_duration_since(Instant::now())
                .filter(|left| !left.is_zero())
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::TimedOut))?;
            self.stream.set_read_timeout(Some(left.min(STOP_POLL)))?;
            match self.stream.read(buf) {
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                other => return other,
            }
        }
    }
}

/// One attached client: queue what it sends for the app's frame, write the app's replies back.
fn connection(stream: TcpStream, slot: &mut Slot) {
    let shared = Arc::clone(&slot.shared);
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let Ok(mut output) = stream.try_clone() else { return };
    let mut input =
        std::io::BufReader::new(Handshake { stream, deadline: Some(Instant::now() + PREAUTH_TIMEOUT), shared: Arc::clone(&shared) });
    // What the client asked for (`--yes`) and the token it attached with: what it may do is looked
    // up for each call, so the app's setting and a revoked grant apply at once.
    let mut asked = false;
    let mut token = String::new();
    loop {
        let max = if slot.attached { MAX_LINE } else { MAX_PREAUTH_LINE };
        let line = match read_message_max(&mut input, max, Oversize::Stop) {
            Ok(Some(line)) => line,
            Ok(None) | Err(framing::Error::Io(_)) => return,
            // Too long or not UTF-8: say why, then close (nothing more of the line is read).
            Err(why) => {
                send_line(&mut output, &error_reply(&Value::Null, -32700, &why.to_string()));
                return;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        if !slot.attached && looks_like_http(&line) {
            log::warn!("MCP relay: dropped a connection that spoke HTTP, not the relay protocol");
            return;
        }
        // Before the attach the connection has proven nothing, so a bad line drops it. After
        // a valid attach the line belongs to the client: a non-JSON line is queued like any
        // other, and `Server::handle` answers the parse error while the session stays open.
        let msg = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => Some(msg),
            Err(_) if slot.attached => None,
            Err(_) => return,
        };
        if let Some(msg) = &msg {
            // The first non-empty message must be a valid attach: the run's token, or the
            // connection goes. A repeat attach still carries the token (it may update what the
            // connection allows).
            let is_attach = msg.get("method").and_then(Value::as_str) == Some(ATTACH);
            if !slot.attached || is_attach {
                let given = msg.get("params").and_then(|p| p.get("token")).and_then(Value::as_str).unwrap_or("");
                if !is_attach || shared.access(given, false).is_none() {
                    return;
                }
                token = given.to_string();
                if same_token(given, &shared.token) {
                    // Re-attached with the relay's own token: no longer a grant's connection.
                    lock(&shared.granted).remove(&slot.id);
                } else {
                    lock(&shared.granted).insert(slot.id, token.clone());
                    // A grant revoked between the check above and this insert: the revocation
                    // missed this connection, so it ends here.
                    if shared.access(given, false).is_none() {
                        return;
                    }
                }
                asked = msg.get("params").and_then(|p| p.get("access")).and_then(Value::as_bool).unwrap_or(false);
                if !slot.attached {
                    slot.attached = true;
                    slot.preauth = false;
                    shared.preauth.fetch_sub(1, SeqCst);
                    shared.attached.fetch_add(1, SeqCst);
                    input.get_mut().deadline = None;
                    shared.notify();
                }
                continue;
            }
        }
        if shared.stopped.load(SeqCst) {
            return;
        }
        // A revoked grant ends the connection.
        if shared.access(&token, asked).is_none() {
            return;
        }
        let expects = expects_reply(&line);
        let id = msg.as_ref().map_or(Value::Null, request_id);
        let state = Arc::new(AtomicU8::new(PENDING));
        let (tx, rx) = sync_channel(1);
        let queued = {
            let mut queue = lock(&shared.requests);
            if shared.stopped.load(SeqCst) {
                return;
            }
            if queue.len() >= MAX_QUEUE {
                false
            } else {
                queue.push_back(Request { line, token: token.clone(), asked, reply: tx, state: Arc::clone(&state) });
                true
            }
        };
        if !queued {
            if expects && !send_line(&mut output, &error_reply(&id, -32001, "the app is busy; try again")) {
                return;
            }
            continue;
        }
        // The app's frame runs queued messages, and egui draws no frame the UI thread isn't
        // asked for: wake it now, or the client waits until the user moves the mouse.
        shared.notify();
        if !expects {
            continue;
        }
        let reply = match wait_reply(&rx, &shared, shared.reply_wait) {
            Ok(reply) => reply,
            Err(RecvTimeoutError::Timeout) => {
                // Not yet running: cancel it, so the app can never run it after the client has
                // been told it failed.
                if state.compare_exchange(PENDING, CANCELLED, SeqCst, SeqCst).is_ok() {
                    send_line(&mut output, &error_reply(&id, -32002, "the app did not run the command in time; it was cancelled"));
                    return;
                }
                // Already running on the app's frame: wait one more bounded interval for its reply.
                // If it still has not come, its outcome is unknown; say so instead of waiting on.
                match wait_reply(&rx, &shared, shared.reply_wait) {
                    Ok(reply) => reply,
                    Err(RecvTimeoutError::Timeout) => {
                        send_line(&mut output, &error_reply(&id, -32003, "the app is still running the command; its outcome is unknown"));
                        return;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            // The relay stopped, or the app dropped the request.
            Err(RecvTimeoutError::Disconnected) => return,
        };
        match reply {
            Some(reply) => {
                if !send_line(&mut output, &reply) {
                    return;
                }
            }
            // No reply to this message.
            None => return,
        }
    }
}

/// Waits up to `wait` for a request's reply, checking every [`STOP_POLL`] whether the relay
/// stopped: a stop ends the wait at once as `Disconnected`, even while the app is still running
/// the request, so no connection thread outlives a stop by more than that.
fn wait_reply(rx: &Receiver<Option<String>>, shared: &Shared, wait: Duration) -> Result<Option<String>, RecvTimeoutError> {
    let deadline = Instant::now() + wait;
    loop {
        if shared.stopped.load(SeqCst) {
            return Err(RecvTimeoutError::Disconnected);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(RecvTimeoutError::Timeout);
        }
        match rx.recv_timeout(left.min(STOP_POLL)) {
            Err(RecvTimeoutError::Timeout) => {}
            done => return done,
        }
    }
}

/// Writes one line and flushes it. `false` when the client has gone.
fn send_line(output: &mut TcpStream, text: &str) -> bool {
    writeln!(output, "{text}").and_then(|()| output.flush()).is_ok()
}

/// The JSON-RPC id of a message, when it is a string or a number.
fn request_id(msg: &Value) -> Value {
    msg.get("id").filter(|id| id.is_string() || id.is_number()).cloned().unwrap_or(Value::Null)
}

/// A JSON-RPC error reply to `id`.
fn error_reply(id: &Value, code: i64, message: &str) -> String {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

/// The internal-error reply to a message that panicked, with the id the client can match it to.
fn internal_error(message: &str) -> String {
    let id = serde_json::from_str::<Value>(message).ok().map_or(Value::Null, |v| request_id(&v));
    error_reply(&id, -32603, "internal error")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    /// Only prompt requests read the skill folders (on the app's thread); a batch with one does.
    #[test]
    fn only_prompt_requests_load_skills() {
        assert!(super::asks_for_skills(r#"{"jsonrpc":"2.0","id":1,"method":"prompts/list"}"#));
        assert!(super::asks_for_skills(r#"{"jsonrpc":"2.0","id":1,"method":"prompts\/get","params":{"name":"x"}}"#));
        assert!(super::asks_for_skills(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","id":2,"method":"prompts/list"}]"#));
        for line in
            [r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#, r#"{"jsonrpc":"2.0","id":1,"method":"tools/call"}"#, "not json", "[1, 2]", "{}"]
        {
            assert!(!super::asks_for_skills(line), "{line}");
        }
    }

    use std::io::{BufRead, BufReader, Write};
    use std::sync::atomic::AtomicUsize;

    use rustgis_engine::{Project, Session};
    use serde_json::json;

    use super::*;

    fn sample() -> Session {
        let mut s = Session::new(Project::default());
        s.run("project.sample", &Value::Null).unwrap();
        s
    }

    /// Reads the one reply a request gets.
    fn reply(client: &TcpStream) -> Value {
        let mut input = BufReader::new(client.try_clone().unwrap());
        let mut line = String::new();
        input.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    /// The token the relay wrote for its port: the same file the CLI reads.
    fn token(relay: &Relay) -> String {
        read_token(relay.port()).unwrap()
    }

    /// Attaches a client the way `rustgis-cli mcp --attach` does.
    fn attach(client: &mut TcpStream, relay: &Relay, access: bool) {
        let hello = json!({"jsonrpc": "2.0", "method": "rustgis/attach", "params": {"access": access, "token": token(relay)}});
        writeln!(client, "{hello}").unwrap();
        client.flush().unwrap();
    }

    /// Lets the connection thread hand the request to the app's frame, which runs it.
    fn settle(relay: &Relay, s: &mut Session) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while relay.poll(s) == 0 {
            assert!(std::time::Instant::now() < deadline, "the relay never handed the request to the app");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Waits until `cond` holds, failing after five seconds.
    fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn add_sample_call(id: u64) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {
            "name": "run_command",
            "arguments": {"command": "layer.addSample", "params": {"name": "countries"}}
        }})
    }

    /// A command sent through the relay runs on the target Session through Session::run (its
    /// undo history shows it), and a bind away from loopback is refused.
    #[test]
    fn relay_runs_a_command_on_the_target_session() {
        // The negative control: anything but loopback is refused, before any socket opens.
        for addr in ["0.0.0.0:0", "[::]:0", "192.0.2.1:8767"] {
            let a: SocketAddr = addr.parse().unwrap();
            let e = Relay::bind(a).unwrap_err();
            assert!(e.contains("loopback"), "{addr}: {e}");
        }

        let relay = Relay::start(0).unwrap();
        let mut s = sample();
        let layers = s.map().layers.len();
        let undo = s.undo_depth();
        let mut client = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        attach(&mut client, &relay, false);
        writeln!(client, "{}", add_sample_call(1)).unwrap();
        client.flush().unwrap();
        settle(&relay, &mut s);
        let reply = reply(&client);
        assert_eq!(reply["id"], 1, "{reply}");
        assert_eq!(reply["result"]["isError"], false, "{reply}");
        // The session the relay was started for: the command ran here, one undo step.
        assert_eq!(s.map().layers.len(), layers + 1, "{reply}");
        assert_eq!(s.undo_depth(), undo + 1, "one undo step, as in the private-session server");
        relay.stop();
    }

    /// A connection that doesn't start with the run's token is dropped: a self-declared access
    /// flag proves nothing, no command runs, and a line that speaks HTTP gets nowhere.
    #[test]
    fn relay_drops_connections_without_the_token() {
        let relay = Relay::start(0).unwrap();
        let mut s = sample();
        // A wrong token: the connection is closed and nothing was queued.
        let mut client = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        let bad = json!({"jsonrpc": "2.0", "method": "rustgis/attach", "params": {"access": true, "token": "not-the-token"}});
        writeln!(client, "{bad}").unwrap();
        client.flush().unwrap();
        let mut input = BufReader::new(client.try_clone().unwrap());
        let mut line = String::new();
        assert_eq!(input.read_line(&mut line).unwrap(), 0, "the relay kept a client without the token: {line}");
        assert_eq!(relay.poll(&mut s), 0, "a connection without the token queued a message");
        assert_eq!(relay.attached_clients(), 0);
        // A command with no attach at all: dropped the same way.
        let mut client = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "list_layers", "arguments": {}}});
        writeln!(client, "{call}").unwrap();
        client.flush().unwrap();
        let mut input = BufReader::new(client.try_clone().unwrap());
        let mut line = String::new();
        assert_eq!(input.read_line(&mut line).unwrap(), 0, "the relay kept a client that never attached: {line}");
        assert_eq!(relay.poll(&mut s), 0, "a connection that never attached queued a message");
        // An HTTP request line — a web page's fetch at the loopback port — is a protocol
        // violation: dropped, no parse-error reply to keep the exchange going.
        let mut client = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        writeln!(client, "POST / HTTP/1.1").unwrap();
        client.flush().unwrap();
        let mut input = BufReader::new(client.try_clone().unwrap());
        let mut line = String::new();
        assert_eq!(input.read_line(&mut line).unwrap(), 0, "the relay kept an HTTP request open: {line}");
        assert_eq!(relay.poll(&mut s), 0, "an HTTP request queued a message");
        relay.stop();
    }

    /// After a valid attach a non-JSON line is not a protocol violation of its own: the server
    /// answers it with a parse error and the same connection keeps serving requests.
    #[test]
    fn relay_returns_parse_error_after_attach_and_stays_usable() {
        let relay = Relay::start(0).unwrap();
        let mut s = sample();
        let mut client = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        attach(&mut client, &relay, false);
        writeln!(client, "this is not JSON").unwrap();
        client.flush().unwrap();
        settle(&relay, &mut s);
        let parse_error = reply(&client);
        assert_eq!(parse_error["error"]["code"], -32700, "{parse_error}");
        // The same connection still answers a real request afterwards.
        let ping = json!({"jsonrpc": "2.0", "id": 7, "method": "ping"});
        writeln!(client, "{ping}").unwrap();
        client.flush().unwrap();
        settle(&relay, &mut s);
        let pong = reply(&client);
        assert_eq!(pong["id"], 7, "{pong}");
        relay.stop();
    }

    /// A file command still needs --yes: without the connection's access message the existing
    /// refusal policy answers it.
    #[test]
    fn relay_refuses_file_commands_without_yes() {
        let relay = Relay::start(0).unwrap();
        let mut s = sample();
        let mut client = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        attach(&mut client, &relay, false);
        let out = std::env::temp_dir().join(format!("rustgis-relay-test-{}.png", std::process::id()));
        let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "run_command",
            "arguments": {"command": "share.exportMap", "params": {"path": out.display().to_string()}}
        }});
        writeln!(client, "{call}").unwrap();
        client.flush().unwrap();
        settle(&relay, &mut s);
        let reply = reply(&client);
        assert_eq!(reply["result"]["isError"], true, "{reply}");
        assert!(reply["result"]["content"][0]["text"].as_str().unwrap().contains("--yes"), "{reply}");
        assert!(!out.exists(), "the file command did not run");
        let _ = std::fs::remove_file(out);
        relay.stop();
    }

    /// Attaches with any token, asking for file and network access or not.
    fn attach_with(client: &mut TcpStream, token: &str, access: bool) {
        let hello = json!({"jsonrpc": "2.0", "method": "rustgis/attach", "params": {"access": access, "token": token}});
        writeln!(client, "{hello}").unwrap();
        client.flush().unwrap();
    }

    /// Asks the relay to export the map to `out`; the reply.
    fn export(client: &mut TcpStream, relay: &Relay, s: &mut Session, out: &std::path::Path, id: u64) -> Value {
        let call = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {
            "name": "run_command",
            "arguments": {"command": "share.exportMap", "params": {"path": out.display().to_string()}}
        }});
        writeln!(client, "{call}").unwrap();
        client.flush().unwrap();
        settle(relay, s);
        reply(client)
    }

    /// With agents turned on in the app, an attached agent may use files and the network without
    /// asking (`--yes`); turned off, what it asked for decides, as before.
    #[test]
    fn relay_app_lets_agents_use_files_without_yes() {
        let relay = Relay::start(0).unwrap();
        let mut s = sample();
        let out = std::env::temp_dir().join(format!("rustgis-relay-app-access-{}.png", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let mut client = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        attach(&mut client, &relay, false);
        // Not asked, not allowed by the app: refused, pointing at --yes (as before).
        let r = export(&mut client, &relay, &mut s, &out, 1);
        assert_eq!(r["result"]["isError"], true, "{r}");
        assert!(!out.exists());
        // The app allows it: the same connection's next call runs, without --yes.
        relay.set_allow_access(true);
        let r = export(&mut client, &relay, &mut s, &out, 2);
        assert_ne!(r["result"]["isError"], true, "{r}");
        assert!(out.exists(), "the export didn't run once the app allowed it");
        let _ = std::fs::remove_file(&out);
        // Turned off again, it's refused again; a client that asked still may.
        relay.set_allow_access(false);
        assert_eq!(export(&mut client, &relay, &mut s, &out, 3)["result"]["isError"], true);
        let mut asked = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        attach(&mut asked, &relay, true);
        assert_ne!(export(&mut asked, &relay, &mut s, &out, 4)["result"]["isError"], true);
        let _ = std::fs::remove_file(&out);
        relay.stop();
    }

    /// A grant is a run-scoped token with its own access; dropping it revokes the token, removes
    /// its file and closes its connections.
    #[test]
    fn relay_grants_are_scoped_and_revocable() {
        let relay = Relay::start(0).unwrap();
        let mut s = sample();
        let out = std::env::temp_dir().join(format!("rustgis-relay-grant-{}.png", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let full = relay.grant(true).unwrap();
        let map_only = relay.grant(false).unwrap();
        let token = read_token_file(full.token_file()).unwrap();
        assert_ne!(token, read_token(relay.port()).unwrap(), "a grant has a token of its own");
        // The full grant may export, though the client didn't ask and the app's setting is off.
        let mut client = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        attach_with(&mut client, &token, false);
        assert_ne!(export(&mut client, &relay, &mut s, &out, 1)["result"]["isError"], true);
        assert!(out.exists());
        let _ = std::fs::remove_file(&out);
        // The map-only grant may not, even when the client asks.
        let mut limited = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        attach_with(&mut limited, &read_token_file(map_only.token_file()).unwrap(), true);
        assert_eq!(export(&mut limited, &relay, &mut s, &out, 2)["result"]["isError"], true);
        assert!(!out.exists());
        // Revoked: the file goes, the connection closes, and the token no longer attaches.
        let file = full.token_file().to_path_buf();
        drop(full);
        assert!(!file.exists());
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut line = String::new();
        assert_eq!(
            BufReader::new(client.try_clone().unwrap()).read_line(&mut line).unwrap_or(0),
            0,
            "a revoked grant's connection stayed open"
        );
        let mut again = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        attach_with(&mut again, &token, true);
        again.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut line = String::new();
        assert_eq!(BufReader::new(again.try_clone().unwrap()).read_line(&mut line).unwrap_or(0), 0, "a revoked token attached");
        // Stopping the relay removes the other grant's file, and refuses new grants.
        let other = map_only.token_file().to_path_buf();
        relay.stop();
        assert!(!other.exists());
        assert!(relay.grant(true).is_err(), "a grant after the relay stopped");
        // Grant files carry the run's id: another run on the same port names its own.
        wait_until("the port to close", || TcpStream::connect(("127.0.0.1", relay.port())).is_err());
        let next = Relay::start(relay.port()).unwrap();
        let fresh = next.grant(true).unwrap();
        assert_ne!(fresh.token_file(), other.as_path());
        next.stop();
    }

    /// Permission is decided when the app runs a request: one queued before its grant was revoked
    /// never runs. A connection that re-attached with the relay's own token isn't the grant's.
    #[test]
    fn relay_revocation_reaches_queued_requests_and_spares_reattached_connections() {
        let relay = Relay::start(0).unwrap();
        let mut s = sample();
        let out = std::env::temp_dir().join(format!("rustgis-relay-queued-{}.png", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let grant = relay.grant(true).unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        attach_with(&mut client, &read_token_file(grant.token_file()).unwrap(), false);
        let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
            "name": "run_command",
            "arguments": {"command": "share.exportMap", "params": {"path": out.display().to_string()}}
        }});
        writeln!(client, "{call}").unwrap();
        client.flush().unwrap();
        // Queued, not yet run: the grant goes, then the app's frame comes.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while lock(&relay.owner.shared.requests).is_empty() {
            assert!(std::time::Instant::now() < deadline, "the request was never queued");
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(grant);
        assert_eq!(relay.poll(&mut s), 0, "a request queued before its grant was revoked ran");
        assert!(!out.exists());
        // Attached with a grant, then re-attached with the relay's own token: revoking the grant
        // leaves it open.
        let other = relay.grant(true).unwrap();
        let mut moved = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        attach_with(&mut moved, &read_token_file(other.token_file()).unwrap(), false);
        attach(&mut moved, &relay, true);
        // A round trip: the re-attach line has been handled once this is answered.
        writeln!(moved, "{}", json!({"jsonrpc": "2.0", "id": 9, "method": "tools/list"})).unwrap();
        moved.flush().unwrap();
        settle(&relay, &mut s);
        assert!(reply(&moved)["result"]["tools"].is_array());
        drop(other);
        let r = export(&mut moved, &relay, &mut s, &out, 2);
        assert_ne!(r["result"]["isError"], true, "a connection on the relay's own token was closed with the grant: {r}");
        let _ = std::fs::remove_file(&out);
        relay.stop();
    }

    /// A command whose reply times out is cancelled: the client gets an error, and the app's
    /// next frame does not run it.
    #[test]
    fn relay_cancels_a_timed_out_command_so_it_never_runs() {
        let relay = Relay::bind_with(SocketAddr::from(([127, 0, 0, 1], 0)), Duration::from_millis(100)).unwrap();
        let mut s = sample();
        let layers = s.map().layers.len();
        let mut client = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        attach(&mut client, &relay, false);
        writeln!(client, "{}", add_sample_call(3)).unwrap();
        client.flush().unwrap();
        // Nobody polls, so the wait runs out and the client is told.
        let timed_out = reply(&client);
        assert_eq!(timed_out["id"], 3, "{timed_out}");
        assert_eq!(timed_out["error"]["code"], -32002, "{timed_out}");
        // The app's frame now finds the queue: the cancelled command must not run.
        assert_eq!(relay.poll(&mut s), 0, "a cancelled command ran");
        assert_eq!(s.map().layers.len(), layers, "a cancelled command changed the session");
        relay.stop();
    }

    /// Stopping closes the listener and every client, removes the token, and drops the
    /// attached count; a temporary clone that drops first does not stop the relay.
    #[test]
    fn relay_stop_closes_clients_and_removes_the_token() {
        let relay = Relay::start(0).unwrap();
        let port = relay.port();
        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        attach(&mut client, &relay, false);
        wait_until("the attach", || relay.attached_clients() == 1);
        assert!(token_path(port).exists(), "the token file was not written");
        drop(relay.clone());
        assert!(relay.is_running(), "a dropped temporary clone stopped the relay");
        relay.stop();
        assert!(!relay.is_running());
        assert!(!token_path(port).exists(), "the token file survived stop");
        wait_until("the attached count to fall", || relay.attached_clients() == 0);
        let mut input = BufReader::new(client.try_clone().unwrap());
        let mut line = String::new();
        assert_eq!(input.read_line(&mut line).unwrap(), 0, "a client stayed connected after stop: {line}");
        wait_until("the port to close", || TcpStream::connect(("127.0.0.1", port)).is_err());
    }

    /// The last owner's drop stops the relay: no thread keeps the listener, and the token goes.
    #[test]
    fn relay_last_owner_drop_closes_the_listener() {
        let relay = Relay::start(0).unwrap();
        let port = relay.port();
        let copy = relay.clone();
        drop(relay);
        assert!(copy.is_running(), "dropping one owner stopped a relay another clone still holds");
        drop(copy);
        wait_until("the port to close", || TcpStream::connect(("127.0.0.1", port)).is_err());
        assert!(!token_path(port).exists(), "the token file survived the last drop");
    }

    /// Unauthenticated connections are capped: one more than the limit is closed at once.
    #[test]
    fn relay_caps_unauthenticated_connections() {
        let relay = Relay::start(0).unwrap();
        let port = relay.port();
        let held: Vec<TcpStream> = (0..MAX_PREAUTH_CONNECTIONS).map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap()).collect();
        let extra = TcpStream::connect(("127.0.0.1", port)).unwrap();
        extra.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut line = String::new();
        let n = BufReader::new(extra).read_line(&mut line);
        assert!(matches!(n, Ok(0)), "the relay kept a connection over the unauthenticated limit: {n:?} {line}");
        drop(held);
        relay.stop();
    }

    /// A line longer than the pre-attach limit is refused without being buffered: no attach,
    /// no queued message, and the connection is closed.
    #[test]
    fn relay_refuses_an_oversized_unauthenticated_line() {
        let relay = Relay::start(0).unwrap();
        let mut s = sample();
        let mut client = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        let big = vec![b'a'; 1 << 20];
        // The relay closes once the line is too long, so the rest of the write may fail.
        let _ = client.write_all(&big);
        let _ = client.write_all(b"\n");
        let mut input = BufReader::new(client.try_clone().unwrap());
        let mut line = String::new();
        let _ = input.read_line(&mut line);
        assert!(line.is_empty() || line.contains("-32700"), "the relay answered an oversized line: {line}");
        assert_eq!(relay.poll(&mut s), 0);
        assert_eq!(relay.attached_clients(), 0);
        relay.stop();
    }

    /// Bare relative grant paths work in a private directory and retain all permission checks.
    #[cfg(unix)]
    #[test]
    fn relative_token_file_uses_its_current_directory() {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        if std::env::var_os("RUSTGIS_TEST_RELATIVE_TOKEN").is_some() {
            let file = std::path::Path::new("grant.token");
            assert_eq!(read_token_file(file).unwrap(), "synthetic-token");
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(read_token_file(file).unwrap_err().contains("readable by others"));
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::set_permissions(".", std::fs::Permissions::from_mode(0o750)).unwrap();
            assert!(read_token_file(file).unwrap_err().contains("readable by others"));
            std::fs::set_permissions(".", std::fs::Permissions::from_mode(0o700)).unwrap();
            std::os::unix::fs::symlink("grant.token", "link.token").unwrap();
            assert!(read_token_file(std::path::Path::new("link.token")).is_err());
            let missing = read_token_file(std::path::Path::new("missing.token")).unwrap_err();
            assert!(missing.contains("--token-file") && missing.contains("relay or grant"));
            return;
        }
        // A child process isolates the current directory from tests running in parallel.
        let dir = std::env::temp_dir().join(format!("rustgis-relative-token-{}", random_token().unwrap()));
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(dir.join("grant.token")).unwrap();
        file.write_all(b"synthetic-token").unwrap();
        drop(file);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "relay::tests::relative_token_file_uses_its_current_directory", "--nocapture"])
            .env("RUSTGIS_TEST_RELATIVE_TOKEN", "1")
            .current_dir(&dir)
            .status()
            .unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(status.success());
    }

    /// The token is 32 hex characters from the operating system, differs per run, and is
    /// readable by this user only; its folder is private too.
    #[test]
    fn relay_token_is_random_and_private() {
        let a = Relay::start(0).unwrap();
        let ta = token(&a);
        assert_eq!(ta.len(), 32, "{ta}");
        assert!(ta.bytes().all(|b| b.is_ascii_hexdigit()), "{ta}");
        let b = Relay::start(0).unwrap();
        assert_ne!(ta, token(&b), "two runs drew the same token");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file_mode = std::fs::metadata(token_path(a.port())).unwrap().permissions().mode();
            assert_eq!(file_mode & 0o077, 0, "the token file is readable by others: {file_mode:o}");
            let dir_mode = std::fs::metadata(token_dir()).unwrap().permissions().mode();
            assert_eq!(dir_mode & 0o077, 0, "the token folder is readable by others: {dir_mode:o}");
        }
        a.stop();
        b.stop();
    }

    /// The count of authenticated clients follows attach and disconnect, and each change wakes
    /// the UI so the indicator repaints.
    #[test]
    fn relay_counts_authenticated_clients_and_wakes_on_change() {
        let relay = Relay::start(0).unwrap();
        let wakes = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&wakes);
        relay.set_wake(Box::new(move || {
            counted.fetch_add(1, SeqCst);
        }));
        let mut client = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        attach(&mut client, &relay, false);
        // The relay changes the count, then wakes the UI: wait for both, as the test thread can
        // see the new count before the wake.
        wait_until("the attach", || relay.attached_clients() == 1);
        wait_until("the attach to wake the UI", || wakes.load(SeqCst) >= 1);
        let before = wakes.load(SeqCst);
        drop(client);
        wait_until("the disconnect", || relay.attached_clients() == 0);
        wait_until("the disconnect to wake the UI", || wakes.load(SeqCst) > before);
        relay.stop();
    }

    /// Stopping while a client waits for its reply ends the wait at once (not after the reply
    /// wait), closes the client, and the queued command never runs afterwards.
    #[test]
    fn relay_stop_ends_a_reply_wait_and_the_command_never_runs() {
        let relay = Relay::start(0).unwrap();
        let mut s = sample();
        let layers = s.map().layers.len();
        let mut client = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        attach(&mut client, &relay, false);
        writeln!(client, "{}", add_sample_call(4)).unwrap();
        client.flush().unwrap();
        wait_until("the request to queue", || lock(&relay.owner.shared.requests).len() == 1);
        let started = Instant::now();
        relay.stop();
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut line = String::new();
        let n = BufReader::new(client.try_clone().unwrap()).read_line(&mut line);
        assert!(matches!(n, Ok(0)) || n.is_err(), "a client stayed connected after stop: {n:?} {line}");
        assert!(started.elapsed() < Duration::from_secs(5), "the reply wait outlived the stop");
        assert_eq!(relay.poll(&mut s), 0, "a command ran after stop");
        assert_eq!(s.map().layers.len(), layers, "a command changed the session after stop");
    }

    /// A command the app took but did not finish in time is reported as unknown, not as
    /// cancelled, and the wait still ends.
    #[test]
    fn relay_reports_a_running_command_past_its_wait_as_unknown() {
        let relay = Relay::bind_with(SocketAddr::from(([127, 0, 0, 1], 0)), Duration::from_secs(1)).unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        attach(&mut client, &relay, false);
        writeln!(client, "{}", add_sample_call(5)).unwrap();
        client.flush().unwrap();
        wait_until("the request to queue", || lock(&relay.owner.shared.requests).len() == 1);
        // The app's frame claims it and is still running it: its reply channel stays open.
        let running = lock(&relay.owner.shared.requests).pop_front().unwrap();
        assert!(running.state.compare_exchange(PENDING, RUNNING, SeqCst, SeqCst).is_ok());
        let unknown = reply(&client);
        assert_eq!(unknown["id"], 5, "{unknown}");
        assert_eq!(unknown["error"]["code"], -32003, "{unknown}");
        drop(running);
        relay.stop();
    }

    /// The attach deadline is absolute: a client that sends one byte at a time, never a whole
    /// line, is still closed once the handshake time runs out.
    #[test]
    fn relay_closes_a_trickling_handshake_at_its_deadline() {
        let relay = Relay::start(0).unwrap();
        let client = TcpStream::connect(("127.0.0.1", relay.port())).unwrap();
        let mut writer = client.try_clone().unwrap();
        let trickle = std::thread::spawn(move || {
            for _ in 0..40 {
                if writer.write_all(b" ").and_then(|()| writer.flush()).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        let started = Instant::now();
        client.set_read_timeout(Some(PREAUTH_TIMEOUT * 3)).unwrap();
        let mut line = String::new();
        let n = BufReader::new(client.try_clone().unwrap()).read_line(&mut line);
        assert!(matches!(n, Ok(0)) || n.is_err(), "the relay kept a trickling handshake: {n:?} {line}");
        assert!(started.elapsed() < PREAUTH_TIMEOUT * 2, "the handshake outlived its deadline: {:?}", started.elapsed());
        assert_eq!(relay.attached_clients(), 0);
        let _ = client.shutdown(Shutdown::Both);
        let _ = trickle.join();
        relay.stop();
    }
}
