//! neutrinod: Neutrino's daemon.
//!
//! One binary, two roles:
//!
//! * **workstation** (the MacBook): reads and changes the working copies you
//!   register, and answers the app and `ntr` over a Unix socket.
//! * **server** (`--server`, on atlas): also hosts bare repositories under
//!   `<state>/repos/`, takes pushes over SSH (git's own transport, with hooks
//!   that report back here), and answers paired devices over TLS.

mod backups;
pub mod ci;
pub mod hosting;
mod mirror;
mod phone;
pub use neutrino_remote;
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
    /// Server only: where CI jobs run and what they may use.
    pub ci: ci::CiOptions,
    /// Server only: where the vault's master key is kept.
    pub vault: neutrino_vault::KeyStore,
    /// Workstation only: an SSH host (the server) to copy database snapshots from.
    pub copy_backups_from: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options { role: Role::Workstation, remote_port: None, advertise: false, ci: ci::CiOptions::default(), vault: neutrino_vault::KeyStore::Keychain, copy_backups_from: None }
    }
}

/// Who a connection is: this Mac's own user, or a paired device over TLS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Local,
    /// A paired device: its id and name.
    Remote { id: String, name: String },
}

impl Scope {
    /// How an approval records who gave it.
    fn who(&self) -> String {
        match self {
            Scope::Local => "the server".into(),
            Scope::Remote { name, .. } => name.clone(),
        }
    }
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
            | Request::Tree { .. }
            | Request::TreeCommits { .. }
            | Request::Blob { .. }
            | Request::Overview { .. }
            | Request::CreateHosted { .. }
            | Request::DeleteHosted { .. }
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
            | Request::ListRuns { .. }
            | Request::Run { .. }
            | Request::JobLog { .. }
            | Request::CancelRun { .. }
            | Request::RerunRun { .. }
            | Request::DispatchRun { .. }
            | Request::Workflows { .. }
            | Request::CiInfo
            | Request::VaultInfo
            | Request::ListSecrets { .. }
            | Request::SetSecret { .. }
            | Request::DeleteSecret { .. }
            | Request::ListVariables { .. }
            | Request::SetVariable { .. }
            | Request::DeleteVariable { .. }
            | Request::ListEnvironments { .. }
            | Request::SetEnvironment { .. }
            | Request::DeleteEnvironment { .. }
            | Request::ApproveJob { .. }
            | Request::ListArtifacts { .. }
            | Request::ArtifactData { .. }
            | Request::RegisterKey { .. }
            | Request::RegisterPush { .. }
            | Request::Inbox
            | Request::Notification { .. }
            | Request::PushInfo
            | Request::Subscribe
            | Request::Cancel { .. }
    )
}

/// The first line of an error, short enough for a notification.
fn first_line(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > 160 { format!("{}…", line.chars().take(160).collect::<String>()) } else { line.to_string() }
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
    /// Pull requests, rules and runs; the server's only.
    store: Option<neutrino_store::Store>,
    /// The CI scheduler; the server's only.
    ci: Option<ci::Ci>,
    /// Secrets' key; the server's only.
    vault: Option<neutrino_vault::Vault>,
    /// Nonces of signed requests seen lately, so none is used twice.
    nonces: Mutex<HashMap<String, i64>>,
    /// Apple's push service, once it has a key.
    apns: phone::Apns,
    /// Copies of hosted repos to their mirrors, in flight.
    mirrors: Arc<mirror::Mirrors>,
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
        let ci = match options.role {
            Role::Server => Some(ci::Ci::new(options.ci.clone())),
            Role::Workstation => None,
        };
        let vault = match options.role {
            Role::Server => Some(neutrino_vault::Vault::new(options.vault.clone())),
            Role::Workstation => None,
        };
        Ok(Arc::new(Daemon {
            nonces: Mutex::new(HashMap::new()),
            apns: phone::Apns::default(),
            mirrors: Arc::default(),
            store,
            ci,
            vault,
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
        if request == Request::PushTest {
            return match self.push_test().await {
                Ok(output) => Response::Done { output },
                Err(e) => Response::Error { message: format!("{e:#}") },
            };
        }
        if request == Request::CiInfo {
            return match self.ci_info().await {
                Ok(info) => Response::Ci(info),
                Err(e) => Response::Error { message: format!("{e:#}") },
            };
        }
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
                commit: option_env!("NEUTRINO_COMMIT").map(String::from),
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
            Request::Tree { repo: id, rev, path } => Response::Tree(neutrino_git::browse::tree(&repo(&id)?, rev.as_deref(), &path)?),
            Request::TreeCommits { repo: id, rev, path } => {
                Response::TreeCommits(neutrino_git::browse::tree_commits(&repo(&id)?, rev.as_deref(), &path)?)
            }
            Request::Blob { repo: id, rev, path } => Response::Blob(neutrino_git::browse::blob(&repo(&id)?, rev.as_deref(), &path)?),
            Request::Overview { repo: id, rev } => Response::Overview(neutrino_git::browse::overview(&repo(&id)?, rev.as_deref())?),

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
            Request::DeleteHosted { repo } => {
                self.server_only()?;
                let _guard = self.writes.lock().unwrap();
                let path = self.registry.get(&repo)?.path().to_path_buf();
                let busy = self.store()?.runs(&repo, None, 50)?.into_iter().filter(|r| !r.state.finished()).count();
                if busy > 0 {
                    bail!("{repo} has {busy} CI run{} going; cancel or wait for {} first", if busy == 1 { "" } else { "s" }, if busy == 1 { "it" } else { "them" });
                }
                let moved = hosting::retire(&self.hosted_dir(), &path)?;
                self.watchers.unwatch(&repo);
                self.registry.remove(&repo)?;
                eprintln!("stopped hosting {repo}; it's in {}", moved.display());
                Response::Removed { repo }
            }
            Request::CreateHosted { name, description } => {
                self.server_only()?;
                let _guard = self.writes.lock().unwrap();
                let path = hosting::create(&self.hosted_dir(), &name, description.as_deref())?;
                let repo = self.registry.add(&path.display().to_string())?;
                self.refresh_hooks(&path, &repo.id);
                self.watch(&repo.id);
                Response::Repo(repo)
            }
            Request::ImportHosted { name, url, description, mirror } => {
                self.server_only()?;
                // No write lock: a big import takes minutes, and only its own name is at stake.
                let path = hosting::import(&self.hosted_dir(), &name, &url, description.as_deref())?;
                let repo = match self.registry.add(&path.display().to_string()) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = std::fs::remove_dir_all(&path);
                        return Err(e);
                    }
                };
                self.refresh_hooks(&path, &repo.id);
                if mirror {
                    mirror::set_url(&path, Some(&url))?;
                    mirror::mark_in_step(&path);
                }
                self.watch(&repo.id);
                write_commit_graph(path);
                Response::Repo(repo)
            }
            Request::Mirrors => Response::Mirrors(self.mirror_list()?),
            Request::SetMirror { repo: id, url } => {
                let dir = self.hosted_git_dir(&id)?;
                mirror::set_url(&dir, url.as_deref())?;
                if url.is_some() {
                    self.mirrors.request(&dir);
                }
                Response::Mirrors(self.mirror_list()?)
            }
            Request::MirrorNow { repo: id } => {
                let dir = self.hosted_git_dir(&id)?;
                self.mirrors.copy_now(&dir)?;
                Response::Mirrors(self.mirror_list()?)
            }
            Request::ListBackups => Response::Backups(match self.options.role {
                Role::Server => {
                    let dir = self.backup_dir();
                    neutrino_proto::BackupList { dir: dir.display().to_string(), files: backups::list(&dir), source: None, last_copy: None, last_error: None }
                }
                Role::Workstation => backups::workstation_list(&self.state_dir, self.options.copy_backups_from.as_deref()),
            }),
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
                write_commit_graph(git_dir.clone());
                let n = updates.len();
                let mut output = format!("neutrino: {n} ref{} updated", if n == 1 { "" } else { "s" });
                if let Some(url) = mirror::url(&git_dir) {
                    self.mirrors.request(&git_dir);
                    output.push_str(&format!("\nneutrino: copying to {url}"));
                }
                for line in self.pulls_after_push(&id, &updates) {
                    output.push('\n');
                    output.push_str(&line);
                }
                let mut started = 0;
                for u in updates.iter().filter(|u| !u.new.bytes().all(|b| b == b'0')) {
                    if u.name.starts_with("refs/heads/") || u.name.starts_with("refs/tags/") {
                        let changed = if u.name.starts_with("refs/heads/") { self.changed_files(&id, &u.old, &u.new) } else { None };
                        started += self.trigger(&id, &neutrino_ci::Event::Push { name: u.name.clone(), sha: u.new.clone() }, changed.as_deref()).len();
                    }
                }
                if started > 0 {
                    output.push_str(&format!("\nneutrino: started {started} CI run{}", if started == 1 { "" } else { "s" }));
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

            // CI
            Request::ListRuns { repo: id, pull, limit } => {
                self.registry.get(&id)?;
                Response::Runs(self.store()?.runs(&id, pull, limit.unwrap_or(50))?)
            }
            Request::Run { repo: id, id: run } => {
                let d = self.store()?.run_detail(run)?;
                if d.run.repo != id {
                    bail!("run {run} isn't in {id}");
                }
                Response::Run(Box::new(d))
            }
            Request::JobLog { repo: id, job, offset } => Response::JobLog(self.job_log(&id, job, offset)?),
            Request::CancelRun { repo: id, id: run } => {
                self.cancel(&id, run)?;
                Response::Run(Box::new(self.store()?.run_detail(run)?))
            }
            Request::RerunRun { repo: id, id: run } => {
                let new = self.rerun(&id, run)?;
                Response::Run(Box::new(self.store()?.run_detail(new)?))
            }
            Request::DispatchRun { repo: id, workflow, branch } => {
                let new = self.dispatch(&id, &workflow, &branch)?;
                Response::Run(Box::new(self.store()?.run_detail(new)?))
            }
            Request::Workflows { repo: id, branch } => Response::Workflows(self.workflow_files(&id, &branch)?),
            Request::CiInfo => bail!("CI info is answered before this"),

            // Vault, environments, approvals, artifacts
            Request::VaultInfo => Response::Vault(self.vault_info()?),
            Request::VaultSetup => Response::RecoveryCode { code: self.vault()?.setup()? },
            Request::VaultRecover { code } => {
                self.vault()?.recover(&code)?;
                Response::Vault(self.vault_info()?)
            }
            Request::ListSecrets { scope } => Response::Secrets(self.secrets(&scope)?),
            Request::SetSecret { scope, name, value } => self.set_secret(&scope, &name, &value)?,
            Request::DeleteSecret { scope, name } => {
                let s = neutrino_store::Scope::from(&scope)?;
                self.store()?.delete_secret(&s, &name)?;
                Response::Secrets(self.store()?.secrets(&s)?)
            }
            Request::ListVariables { scope } => Response::Variables(self.store()?.variables(&neutrino_store::Scope::from(&scope)?)?),
            Request::SetVariable { scope, name, value } => {
                let s = neutrino_store::Scope::from(&scope)?;
                if !s.repo.is_empty() {
                    self.registry.get(&s.repo)?;
                }
                self.store()?.set_variable(&s, &name, &value)?;
                Response::Variables(self.store()?.variables(&s)?)
            }
            Request::DeleteVariable { scope, name } => {
                let s = neutrino_store::Scope::from(&scope)?;
                self.store()?.delete_variable(&s, &name)?;
                Response::Variables(self.store()?.variables(&s)?)
            }
            Request::ListEnvironments { repo: id } => {
                self.registry.get(&id)?;
                Response::Environments(self.store()?.environments(&id)?)
            }
            Request::SetEnvironment { environment } => self.set_environment(&environment)?,
            Request::DeleteEnvironment { repo: id, name } => {
                self.store()?.delete_environment(&id, &name)?;
                Response::Environments(self.store()?.environments(&id)?)
            }
            Request::ApproveJob { .. } => bail!("approvals are answered by the connection"),
            Request::ListArtifacts { repo: id, run } => {
                if self.store()?.run(run)?.repo != id {
                    bail!("run {run} isn't in {id}");
                }
                Response::Artifacts(self.store()?.artifacts(run)?)
            }
            Request::ArtifactData { repo: id, run, path, offset } => Response::ArtifactChunk(self.artifact_chunk(&id, run, &path, offset)?),
            Request::Backup => Response::Done { output: format!("{}", self.backup_now()?.display()) },

            // The phone
            Request::Inbox => Response::Inbox(Box::new(self.inbox()?)),
            Request::Notification { id } => Response::Notification(self.store()?.notification(&id)?.context("that notification is gone")?),
            Request::PushSetup { key_p8, key_id, team_id, topic } => self.push_setup(&key_p8, &key_id, &team_id, &topic)?,
            Request::PushInfo => Response::Push(self.push_info()?),
            Request::PushTest => bail!("a test push is sent by the connection"),
            Request::RegisterKey { .. } | Request::RegisterPush { .. } | Request::Signed { .. } => {
                bail!("only a paired device can do that, over its own connection")
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

    /// A hosted repository's git dir.
    fn hosted_git_dir(&self, id: &str) -> Result<PathBuf> {
        self.server_only()?;
        let repo = self.registry.list().into_iter().find(|r| r.id == id).with_context(|| format!("no repository called {id}"))?;
        if !repo.bare {
            bail!("{id} is a working copy, not a hosted repository");
        }
        Ok(PathBuf::from(repo.path))
    }

    fn mirror_list(&self) -> Result<Vec<neutrino_proto::MirrorStatus>> {
        self.server_only()?;
        Ok(self.registry.list().into_iter().filter(|r| r.bare && r.error.is_none()).filter_map(|r| mirror::status(&r.id, Path::new(&r.path))).collect())
    }

    /// Server, every minute: copy what's owed to mirrors (failed copies every
    /// ten minutes), and tell the phone the first time one starts failing.
    pub(crate) fn mirror_upkeep(self: &Arc<Self>) {
        for r in self.registry.list().into_iter().filter(|r| r.bare && r.error.is_none()) {
            let dir = PathBuf::from(&r.path);
            let Some(url) = mirror::url(&dir) else { continue };
            if self.mirrors.running(&dir) {
                continue;
            }
            let mut s = mirror::load(&dir);
            if let (Some(err), false) = (&s.last_error, s.notified) {
                self.notify("failure", format!("Copying {} to its mirror failed", r.id), format!("{} · {}", first_line(err), url), &r.id, None, None);
                s.notified = true;
                mirror::save(&dir, &s);
            }
            let due = s.pending && (s.last_error.is_none() || s.last_attempt.is_none_or(|t| neutrino_store::now() - t >= mirror::RETRY_AFTER));
            if due {
                self.mirrors.request(&dir);
            }
        }
    }

    fn write(&self, f: impl FnOnce() -> Result<String>) -> Result<Response> {
        let _guard = self.writes.lock().unwrap();
        Ok(Response::Done { output: f()? })
    }
}

/// Keep history fast to read: refresh a repo's commit-graph in the background.
fn write_commit_graph(git_dir: PathBuf) {
    std::thread::spawn(move || {
        let _ = std::process::Command::new(neutrino_git::worktree::git_binary())
            .arg("--git-dir")
            .arg(&git_dir)
            .args(["commit-graph", "write", "--reachable", "--split"])
            .output();
    });
}

/// Workstation: copy the server's database snapshots here when it starts and
/// every hour, so they don't live only on the server's disk.
fn start_backup_copy(daemon: &Arc<Daemon>) {
    let (Role::Workstation, Some(host)) = (daemon.options.role, daemon.options.copy_backups_from.clone()) else { return };
    let dir = backups::copies_dir(&daemon.state_dir);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        loop {
            let (h, d) = (host.clone(), dir.clone());
            match tokio::task::spawn_blocking(move || backups::copy_from(&backups::Ssh { host: h }, &d)).await {
                Ok(Ok(0)) => {}
                Ok(Ok(n)) => eprintln!("copied {n} database snapshot{} from {host}", if n == 1 { "" } else { "s" }),
                Ok(Err(e)) => eprintln!("copying database snapshots from {host}: {e:#}"),
                Err(e) => eprintln!("copying database snapshots from {host}: {e}"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });
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
        // A signed request: check it, then treat what's inside as the request.
        let (request, signed) = match request {
            Request::Signed { request: inner, at, nonce, signature } => match daemon.unwrap_signed(&scope, &inner, at, &nonce, &signature) {
                Ok(r) => (r, true),
                Err(e) => {
                    let _ = tx.send(encode_response(id, &Response::Error { message: format!("{e:#}") })).await;
                    continue;
                }
            },
            r => (r, false),
        };
        if matches!(scope, Scope::Remote { .. }) && !allowed_remotely(&request) {
            let _ = tx.send(encode_response(id, &Response::Error { message: "only available on the server itself".into() })).await;
            continue;
        }
        // The phone's own settings, and the requests it has to sign.
        if let Some(response) = daemon.device_request(&scope, &request, signed) {
            let _ = tx.send(encode_response(id, &response)).await;
            continue;
        }
        // Approvals record who gave them, which only the connection knows.
        if let Request::ApproveJob { repo, job, approve } = &request {
            let (d, who, (repo, job, approve)) = (Arc::clone(&daemon), scope.who(), (repo.clone(), *job, *approve));
            let response = tokio::task::spawn_blocking(move || d.approve(&repo, job, approve, &who))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
                .and_then(|r| r)
                .unwrap_or_else(|e| Response::Error { message: format!("{e:#}") });
            let _ = tx.send(encode_response(id, &response)).await;
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
                Ok(Some((session, id, name))) => {
                    let (read, write) = tokio::io::split(session);
                    connection(daemon, read, write, Scope::Remote { id, name }).await;
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
    daemon.start_ci();
    start_backup_copy(&daemon);
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
