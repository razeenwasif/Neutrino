//! neutrinod: Neutrino's daemon.
//!
//! One binary, two roles:
//!
//! * **workstation** (the MacBook): reads and changes the working copies you
//!   register, and answers the app and `ntr` over a Unix socket.
//! * **server** (`--server`, on atlas): also hosts bare repositories under
//!   `<state>/repos/`, takes pushes over SSH (git's own transport, with hooks
//!   that report back here), and answers paired devices over TLS.

pub mod hosting;
mod pulls;
mod registry;
mod watch;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use neutrino_git::DiffCache;
use neutrino_proto::{EngineInfo, PROTOCOL_VERSION, RepoEvent, Request, Response, decode_request, encode_response};
use neutrino_remote::Remote;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

pub use registry::Registry;

/// Requests longer than this are refused.
const MAX_LINE: usize = 1 << 20;

/// Where state lives: `$NEUTRINO_HOME`, else `~/.neutrino`.
pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NEUTRINO_HOME") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".neutrino")
}

/// `$NEUTRINO_SOCKET`, else `neutrinod.sock` in the state folder.
pub fn socket_path() -> PathBuf {
    std::env::var_os("NEUTRINO_SOCKET").map(PathBuf::from).unwrap_or_else(|| state_dir().join("neutrinod.sock"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Workstation,
    Server,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Role::Workstation => "workstation",
            Role::Server => "server",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub role: Role,
    /// Server only: the TLS port, or `None` for no network endpoint.
    pub remote_port: Option<u16>,
    /// Server only: advertise the endpoint over Bonjour.
    pub advertise: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { role: Role::Workstation, remote_port: None, advertise: false }
    }
}

/// Who a connection is: this Mac's own user, or a paired device over TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Local,
    Remote,
}

/// What a paired device may ask: reading hosted repos, creating one, and
/// following changes. Never anything that names a path on this Mac, changes a
/// working copy, or manages pairing.
fn allowed_remotely(r: &Request) -> bool {
    matches!(
        r,
        Request::Ping
            | Request::EngineInfo
            | Request::ListRepos
            | Request::Branches { .. }
            | Request::Log(_)
            | Request::Commit { .. }
            | Request::FileDiff { .. }
            | Request::CreateHosted { .. }
            | Request::ListPulls { .. }
            | Request::CreatePull { .. }
            | Request::Pull { .. }
            | Request::UpdatePull { .. }
            | Request::AddNote { .. }
            | Request::ResolveNote { .. }
            | Request::MergePull { .. }
            | Request::Compare { .. }
            | Request::CompareDiff { .. }
            | Request::BranchRules { .. }
            | Request::SetBranchRule { .. }
            | Request::DeleteBranchRule { .. }
            | Request::Overrides { .. }
            | Request::Subscribe
            | Request::Cancel { .. }
    )
}

pub struct Daemon {
    options: Options,
    state_dir: PathBuf,
    registry: Registry,
    diffs: DiffCache,
    watchers: watch::Watchers,
    /// One write at a time, so two never race for the index lock.
    writes: Mutex<()>,
    /// Where discarded work is noted, so it can be got back.
    discarded_log: PathBuf,
    remote: Option<Arc<Remote>>,
    /// Pull requests and rules; the server's only.
    store: Option<neutrino_store::Store>,
    /// The socket hooks call back on; set by `serve`.
    socket: Mutex<Option<PathBuf>>,
    /// The TLS port actually bound (the option may be 0 for "any").
    bound_port: Mutex<Option<u16>>,
}

impl Daemon {
    /// A workstation daemon.
    pub fn new(state_dir: &Path) -> Result<Arc<Self>> {
        Self::with(state_dir, Options::default())
    }

    pub fn with(state_dir: &Path, options: Options) -> Result<Arc<Self>> {
        std::fs::create_dir_all(state_dir).with_context(|| format!("creating {}", state_dir.display()))?;
        let remote = match (options.role, options.remote_port) {
            (Role::Server, Some(port)) => Some(Remote::open(state_dir, port, options.advertise)?),
            _ => None,
        };
        let store = match options.role {
            Role::Server => Some(neutrino_store::Store::open(&pulls::database_path(state_dir))?),
            Role::Workstation => None,
        };
        Ok(Arc::new(Daemon {
            store,
            state_dir: state_dir.to_path_buf(),
            registry: Registry::load(state_dir)?,
            diffs: DiffCache::default(),
            watchers: watch::Watchers::new(),
            writes: Mutex::new(()),
            discarded_log: state_dir.join("discarded.log"),
            remote,
            socket: Mutex::new(None),
            bound_port: Mutex::new(None),
            options,
        }))
    }

    pub fn hosted_dir(&self) -> PathBuf {
        self.state_dir.join("repos")
    }

    pub fn remote(&self) -> Option<&Arc<Remote>> {
        self.remote.as_ref()
    }

    /// The TLS port, once `serve` has bound it.
    pub fn remote_port(&self) -> Option<u16> {
        *self.bound_port.lock().unwrap()
    }

    /// Watch a repo's files so subscribers hear about changes. Needs the Tokio runtime.
    fn watch(&self, id: &str) {
        if let Ok(repo) = self.registry.get(id) {
            let local = neutrino_git::local(&repo);
            let root = neutrino_git::root(&local).to_path_buf();
            let git_dir = local.git_dir().to_path_buf();
            self.watchers.watch(id, root, git_dir);
        }
    }

    fn watch_all(&self) {
        for r in self.registry.list() {
            if r.error.is_none() {
                self.watch(&r.id);
            }
        }
    }

    /// Server start-up: register every hosted repo and point its hooks at this daemon.
    fn adopt_hosted(&self) {
        if self.options.role != Role::Server {
            return;
        }
        for path in hosting::scan(&self.hosted_dir()) {
            match self.registry.add(&path.display().to_string()) {
                Ok(repo) => {
                    self.refresh_hooks(&path, &repo.id);
                    self.sync_rules_marker(&repo.id);
                }
                Err(e) => eprintln!("can't host {}: {e:#}", path.display()),
            }
        }
    }

    fn refresh_hooks(&self, git_dir: &Path, id: &str) {
        let socket = self.socket.lock().unwrap().clone().unwrap_or_else(socket_path);
        // `$NEUTRINO_HOOK_EXE` lets tests point hooks at the built daemon.
        let exe = std::env::var_os("NEUTRINO_HOOK_EXE")
            .map(PathBuf::from)
            .or_else(|| std::env::current_exe().ok())
            .unwrap_or_else(|| PathBuf::from("neutrinod"));
        if let Err(e) = hosting::install_hooks(git_dir, &exe, &socket, id) {
            eprintln!("can't install hooks in {}: {e:#}", git_dir.display());
        }
    }

    fn server_only(&self) -> Result<()> {
        if self.options.role != Role::Server {
            bail!("this neutrinod isn't a server; start it with --server");
        }
        Ok(())
    }

    fn pairing(&self) -> Result<&Arc<Remote>> {
        self.server_only()?;
        self.remote.as_ref().context("this server has no network endpoint (it was started with --no-remote)")
    }

    /// Answer one request. Git work runs on the blocking pool.
    pub async fn handle(self: &Arc<Self>, request: Request) -> Response {
        let this = Arc::clone(self);
        match tokio::task::spawn_blocking(move || this.handle_blocking(request)).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => Response::Error { message: format!("{e:#}") },
            Err(e) => Response::Error { message: format!("internal error: {e}") },
        }
    }

    fn handle_blocking(&self, request: Request) -> Result<Response> {
        let repo = |id: &str| self.registry.get(id).map(|r| neutrino_git::local(&r));
        use neutrino_git::worktree as wt;
        Ok(match request {
            Request::Ping => Response::Pong { protocol: PROTOCOL_VERSION },
            Request::EngineInfo => Response::Engine(EngineInfo {
                version: env!("CARGO_PKG_VERSION").into(),
                protocol: PROTOCOL_VERSION,
                role: self.options.role.name().into(),
                repos: self.registry.len() as u32,
            }),
            Request::ListRepos => Response::Repos(self.registry.list()),
            Request::AddRepo { path } => {
                let repo = self.registry.add(&path)?;
                self.watch(&repo.id);
                Response::Repo(repo)
            }
            Request::RemoveRepo { repo } => {
                self.registry.remove(&repo)?;
                self.watchers.unwatch(&repo);
                Response::Removed { repo }
            }

            // Reading
            Request::Branches { repo: id } => Response::Branches(neutrino_git::branches(&repo(&id)?)?),
            Request::Log(q) => Response::Log(neutrino_git::log(&repo(&q.repo)?, &q)?),
            Request::Commit { repo: id, sha } => Response::Commit(Box::new(neutrino_git::commit(&repo(&id)?, &sha)?)),
            Request::FileDiff { repo: id, sha, path, context } => {
                let context = context.unwrap_or(3).min(10_000);
                Response::FileDiff(neutrino_git::file_diff(&repo(&id)?, &sha, &path, context, &self.diffs)?)
            }

            // Working copy
            Request::Status { repo: id } => Response::Status(wt::status(&repo(&id)?)?),
            Request::WorkDiff { repo: id, path, area, context } => {
                Response::FileDiff(wt::work_diff(&repo(&id)?, &path, area, context.unwrap_or(3).min(10_000))?)
            }
            Request::Stage { repo: id, paths } => self.write(|| Ok(wt::stage(&repo(&id)?, &paths)?))?,
            Request::Unstage { repo: id, paths } => self.write(|| Ok(wt::unstage(&repo(&id)?, &paths)?))?,
            Request::StageLines { repo: id, path, area, lines } => self.write(|| Ok(wt::stage_lines(&repo(&id)?, &path, area, &lines)?))?,
            Request::Discard { repo: id, paths } => self.write(|| Ok(wt::discard(&repo(&id)?, &paths, &self.discarded_log)?))?,
            Request::DiscardLines { repo: id, path, lines } => {
                self.write(|| Ok(wt::discard_lines(&repo(&id)?, &path, &lines, &self.discarded_log)?))?
            }
            Request::CommitChanges { repo: id, message, amend } => {
                let _guard = self.writes.lock().unwrap();
                let (sha, output) = wt::commit(&repo(&id)?, &message, amend)?;
                Response::Committed { sha, output }
            }
            Request::CreateBranch { repo: id, name, start, checkout } => {
                self.write(|| Ok(wt::create_branch(&repo(&id)?, &name, start.as_deref(), checkout)?))?
            }
            Request::Checkout { repo: id, branch } => self.write(|| Ok(wt::checkout(&repo(&id)?, &branch)?))?,
            Request::RenameBranch { repo: id, from, to } => self.write(|| Ok(wt::rename_branch(&repo(&id)?, &from, &to)?))?,
            Request::DeleteBranch { repo: id, name, force } => self.write(|| Ok(wt::delete_branch(&repo(&id)?, &name, force)?))?,

            // Remotes
            Request::Remotes { repo: id } => Response::Remotes(wt::remotes(&repo(&id)?)?),
            Request::AddRemote { repo: id, name, url } => self.write(|| Ok(wt::add_remote(&repo(&id)?, &name, &url)?))?,
            Request::Fetch { repo: id, remote } => self.write(|| Ok(wt::fetch(&repo(&id)?, remote.as_deref())?))?,
            Request::Push { repo: id, remote, branch, set_upstream } => {
                self.write(|| Ok(wt::push(&repo(&id)?, remote.as_deref(), branch.as_deref(), set_upstream)?))?
            }

            // Hosting
            Request::CreateHosted { name, description } => {
                self.server_only()?;
                let _guard = self.writes.lock().unwrap();
                let path = hosting::create(&self.hosted_dir(), &name, description.as_deref())?;
                let repo = self.registry.add(&path.display().to_string())?;
                self.refresh_hooks(&path, &repo.id);
                self.watch(&repo.id);
                Response::Repo(repo)
            }
            Request::HookPreReceive { repo: id, updates, push_options } => {
                self.registry.get(&id)?;
                Response::Done { output: self.check_push(&id, &updates, &push_options)? }
            }
            Request::HookPostReceive { repo: id, updates } => {
                self.server_only()?;
                let r = self.registry.get(&id)?;
                // An empty repo's HEAD names a branch nobody pushed (`main`, say,
                // when the first push was `master`): point it at what arrived, as
                // GitHub does, so the hosted repo has a default branch.
                let local = neutrino_git::local(&r);
                let mut adopted = None;
                if local.head_id().is_err() {
                    let zero = |s: &str| s.bytes().all(|b| b == b'0');
                    let pushed: Vec<&str> =
                        updates.iter().filter(|u| !zero(&u.new)).filter_map(|u| u.name.strip_prefix("refs/heads/")).collect();
                    if let Some(branch) = ["main", "master", "trunk"].into_iter().find(|b| pushed.contains(b)).or(pushed.first().copied()) {
                        let ok = std::process::Command::new(neutrino_git::worktree::git_binary())
                            .arg("--git-dir")
                            .arg(local.git_dir())
                            .args(["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")])
                            .status()
                            .is_ok_and(|s| s.success());
                        if ok {
                            adopted = Some(branch.to_string());
                        }
                    }
                }
                self.watchers.notify(RepoEvent { repo: id.clone(), kind: "push".into() });
                // Keep history fast to read: refresh the commit-graph in the background.
                let git_dir = neutrino_git::local(&r).git_dir().to_path_buf();
                std::thread::spawn(move || {
                    let _ = std::process::Command::new(neutrino_git::worktree::git_binary())
                        .arg("--git-dir")
                        .arg(&git_dir)
                        .args(["commit-graph", "write", "--reachable", "--split"])
                        .output();
                });
                let n = updates.len();
                let mut output = format!("neutrino: {n} ref{} updated", if n == 1 { "" } else { "s" });
                for line in self.pulls_after_push(&id, &updates) {
                    output.push('\n');
                    output.push_str(&line);
                }
                if let Some(b) = adopted {
                    output.push_str(&format!("\nneutrino: {b} is now this repository's default branch"));
                }
                Response::Done { output }
            }

            // Pull requests
            Request::ListPulls { repo: id } => {
                self.registry.get(&id)?;
                Response::Pulls(self.store()?.pulls(&id)?)
            }
            Request::CreatePull { repo: id, title, body, source, target } => self.create_pull(&id, &title, &body, &source, &target)?,
            Request::Pull { repo: id, number } => self.pull_detail(&id, number)?,
            Request::UpdatePull { repo: id, number, title, body, state } => self.update_pull(&id, number, title, body, state)?,
            Request::AddNote { repo: id, number, path, side, line, text } => self.add_note(&id, number, &path, side, line, &text)?,
            Request::ResolveNote { repo: id, number, note, resolved } => self.resolve_note(&id, number, note, resolved)?,
            Request::MergePull { repo: id, number, strategy, message, delete_source } => {
                self.merge_pull(&id, number, strategy, message, delete_source)?
            }
            Request::Compare { repo: id, base, head } => Response::Compare(neutrino_git::forge::compare(&repo(&id)?, &base, &head)?),
            Request::CompareDiff { repo: id, base, head, path, context } => {
                let context = context.unwrap_or(3).min(10_000);
                Response::FileDiff(neutrino_git::forge::compare_diff(&repo(&id)?, &base, &head, &path, context, &self.diffs)?)
            }
            Request::BranchRules { repo: id } => {
                self.registry.get(&id)?;
                Response::Rules(self.store()?.rules(&id)?)
            }
            Request::SetBranchRule { repo: id, rule } => self.set_rule(&id, &rule)?,
            Request::DeleteBranchRule { repo: id, pattern } => self.delete_rule(&id, &pattern)?,
            Request::Overrides { repo: id } => {
                self.registry.get(&id)?;
                Response::Overrides(self.store()?.overrides(&id)?)
            }

            // Pairing
            Request::PairBegin => Response::Pairing(self.pairing()?.begin_pairing()?),
            Request::ListDevices => Response::Devices(self.pairing()?.devices()),
            Request::RevokeDevice { id } => Response::Revoked { id: self.pairing()?.revoke(&id)? },
            Request::Pair { .. } | Request::Auth { .. } => bail!("pairing happens over the network endpoint, not this socket"),

            Request::Subscribe => bail!("subscribe is handled by the connection"),
            Request::Cancel { .. } => bail!("cancel is handled by the connection"),
        })
    }

    fn write(&self, f: impl FnOnce() -> Result<String>) -> Result<Response> {
        let _guard = self.writes.lock().unwrap();
        Ok(Response::Done { output: f()? })
    }
}

/// Serve one client: requests are answered as they finish, tagged with their id.
async fn connection<R, W>(daemon: Arc<Daemon>, read: R, mut write: W, scope: Scope)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<String>(64);
    let writer = tokio::spawn(async move {
        while let Some(mut line) = rx.recv().await {
            line.push('\n');
            if write.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
        let _ = write.shutdown().await;
    });

    let in_flight: Arc<Mutex<HashMap<u64, tokio::task::AbortHandle>>> = Default::default();
    let mut lines = BufReader::new(read);
    let mut buf = String::new();
    loop {
        buf.clear();
        match (&mut lines).take(MAX_LINE as u64 + 1).read_line(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buf.len() > MAX_LINE {
            let _ = tx.send(encode_response(0, &Response::Error { message: "request too long".into() })).await;
            break;
        }
        let line = buf.trim();
        if line.is_empty() {
            continue;
        }
        let (id, request) = decode_request(line);
        let request = match request {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(encode_response(id.unwrap_or(0), &Response::Error { message: e.to_string() })).await;
                continue;
            }
        };
        let id = id.unwrap_or(0);
        if scope == Scope::Remote && !allowed_remotely(&request) {
            let _ = tx.send(encode_response(id, &Response::Error { message: "only available on the server itself".into() })).await;
            continue;
        }
        if let Request::Cancel { id: target } = request {
            let task = in_flight.lock().unwrap().remove(&target);
            if let Some(task) = task {
                task.abort();
                let _ = tx.send(encode_response(target, &Response::Cancelled)).await;
            }
            continue;
        }

        if request == Request::Subscribe {
            let mut events = daemon.watchers.subscribe();
            let tx2 = tx.clone();
            let task = tokio::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(event) => {
                            if tx2.send(encode_response(id, &Response::Event(event))).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
            });
            in_flight.lock().unwrap().insert(id, task.abort_handle());
            continue;
        }

        let (daemon, tx2, flights) = (Arc::clone(&daemon), tx.clone(), Arc::clone(&in_flight));
        // Hold the map while spawning so the task can't finish before it's registered.
        let mut map = in_flight.lock().unwrap();
        let task = tokio::spawn(async move {
            let response = daemon.handle(request).await;
            // Only answer if no one cancelled us in the meantime.
            let still_wanted = flights.lock().unwrap().remove(&id).is_some();
            if still_wanted {
                let _ = tx2.send(encode_response(id, &response)).await;
            }
        });
        map.insert(id, task.abort_handle());
        drop(map);
    }
    // Stop this connection's subscriptions with it.
    for (_, task) in in_flight.lock().unwrap().drain() {
        task.abort();
    }
    drop(tx);
    let _ = writer.await;
}

/// Accept paired devices over TLS until the daemon stops.
async fn serve_remote(daemon: Arc<Daemon>, remote: Arc<Remote>, listener: tokio::net::TcpListener) {
    loop {
        let Ok((tcp, peer)) = listener.accept().await else { continue };
        let (daemon, remote) = (Arc::clone(&daemon), Arc::clone(&remote));
        tokio::spawn(async move {
            match remote.accept(tcp, daemon.options.role.name()).await {
                Ok(Some(session)) => {
                    let (read, write) = tokio::io::split(session);
                    connection(daemon, read, write, Scope::Remote).await;
                }
                Ok(None) => {}
                Err(e) => eprintln!("remote {peer}: {e:#}"),
            }
        });
    }
}

/// Bind the socket (replacing a stale one) and, for a server, the TLS port,
/// then serve until `shutdown` resolves.
pub async fn serve(daemon: Arc<Daemon>, socket: &Path, shutdown: impl std::future::Future<Output = ()>) -> Result<()> {
    if socket.exists() {
        if UnixStream::connect(socket).await.is_ok() {
            bail!("another neutrinod is already listening on {}", socket.display());
        }
        std::fs::remove_file(socket).with_context(|| format!("removing the stale socket {}", socket.display()))?;
    }
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let listener = UnixListener::bind(socket).with_context(|| format!("listening on {}", socket.display()))?;
    // Only this user may talk to the daemon.
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    }
    *daemon.socket.lock().unwrap() = Some(socket.to_path_buf());

    daemon.adopt_hosted();
    daemon.watch_all();
    let remote_task = match daemon.remote.clone() {
        Some(remote) => {
            let tcp = remote.bind().await?;
            *daemon.bound_port.lock().unwrap() = tcp.local_addr().ok().map(|a| a.port());
            eprintln!(
                "remote endpoint on port {} (fingerprint {})",
                tcp.local_addr().map(|a| a.port()).unwrap_or(remote.port),
                neutrino_remote::short_fingerprint(&remote.fingerprint)
            );
            Some(tokio::spawn(serve_remote(Arc::clone(&daemon), remote, tcp)))
        }
        None => None,
    };

    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let (read, write) = stream.into_split();
                tokio::spawn(connection(Arc::clone(&daemon), read, write, Scope::Local));
            }
            _ = &mut shutdown => break,
        }
    }
    if let Some(t) = remote_task {
        t.abort();
    }
    let _ = std::fs::remove_file(socket);
    Ok(())
}
