//! ntr: Neutrino from the terminal. Talks to neutrinod over its Unix socket.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use neutrino_proto::{
    BranchRule, ChangeStatus, Head, LogEntry, LogQuery, MergeStrategy, PullState, RefKind, Repo, Request, Response, RunDetail, RunState, Side, VaultScope,
    decode_response, encode_request,
};

#[derive(Parser)]
#[command(version, about = "Neutrino from the terminal")]
struct Cli {
    /// The daemon's socket. Default `$NEUTRINO_SOCKET`, else ~/.neutrino/neutrinod.sock.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Repo id. Default `$NEUTRINO_REPO`, else the registered repo you are in.
    #[arg(long, short = 'r', global = true)]
    repo: Option<String>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Check the daemon is up.
    Ping,
    /// The daemon's version and role.
    Info,
    /// Registered repos.
    Repos,
    /// Register a working copy, or forget one.
    #[command(subcommand)]
    Repo(RepoCmd),
    /// Local and remote branches.
    Branches,
    /// History, newest first, with its graph (read by neutrinod, fast on big repos).
    History {
        /// Branch, tag or sha to start from. Default HEAD.
        rev: Option<String>,
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: u32,
        #[arg(long, default_value_t = 0)]
        skip: u32,
        /// Every branch and tag.
        #[arg(long)]
        all: bool,
        /// Only commits whose message, author or sha matches.
        #[arg(long, short = 'q')]
        query: Option<String>,
    },
    /// Take whole files out of the index.
    Unstage {
        #[arg(required = true)]
        paths: Vec<String>,
    },
    /// Throw away working-tree changes; copies go to ~/.neutrino/discarded.log.
    Discard {
        #[arg(required = true)]
        paths: Vec<String>,
    },
    /// Print change events as they happen, until Ctrl-C.
    Watch,
    /// Server: stop hosting a repository. Its bare repo moves to
    /// ~/.neutrino/deleted/ on the server, from where it can be moved back.
    Unhost {
        name: String,
        /// Do it; without this, only say what would happen.
        #[arg(long)]
        yes: bool,
    },
    /// Server: create a hosted repository.
    Host {
        name: String,
        #[arg(long, short = 'd')]
        description: Option<String>,
    },
    /// Server: a hosted repository from another one's branches and tags (its
    /// GitHub repository, say), copied back there after every push.
    Import {
        name: String,
        /// Where to copy from: git@github.com:you/name.git, say.
        #[arg(long)]
        from: String,
        #[arg(long, short = 'd')]
        description: Option<String>,
        /// Copy it once, without mirroring pushes back to it.
        #[arg(long)]
        no_mirror: bool,
    },
    /// Server: where hosted repositories are copied after every push; with no
    /// action, how each copy went.
    Mirror {
        #[command(subcommand)]
        action: Option<MirrorCmd>,
    },
    /// The database snapshots: the server's, or on a workstation the copies of them.
    Backups {
        /// Just the files' paths, one per line.
        #[arg(long)]
        paths: bool,
    },
    /// Server: a one-time code to pair the Mac app (or a phone) with this server.
    Pair,
    /// Server: paired devices; `devices revoke ID` forgets one.
    Devices {
        #[command(subcommand)]
        action: Option<DeviceCmd>,
    },
    /// Server: pull requests on a hosted repository.
    #[command(subcommand)]
    Pr(PrCmd),
    /// Server: branch rules on a hosted repository; with no action, lists them.
    Rules {
        #[command(subcommand)]
        action: Option<RulesCmd>,
    },
    /// Server: CI runs of a hosted repository, newest first.
    Runs {
        /// Only this pull request's.
        #[arg(long)]
        pr: Option<u32>,
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: u32,
    },
    /// Server: one CI run, by its number.
    #[command(subcommand)]
    Run(RunCmd),
    /// Server: the workflow files on a branch, checked.
    Workflows {
        #[arg(long, short = 'b', default_value = "main")]
        branch: String,
    },
    /// Server: what CI is doing, and whether Graviton answers.
    Ci,
    /// Server: the vault's key: status, `setup` (once) or `recover CODE`.
    Vault {
        #[command(subcommand)]
        action: Option<VaultCmd>,
    },
    /// Server: secrets for workflows. Values go in, never come out.
    #[command(subcommand)]
    Secret(SecretCmd),
    /// Server: variables for workflows (`vars.NAME`).
    #[command(subcommand)]
    Var(VarCmd),
    /// Server: deployment environments of a hosted repository.
    #[command(subcommand)]
    Env(EnvCmd),
    /// Server: snapshot the database now.
    Backup,
    /// Server: what needs you: deploys to approve, pull requests ready to merge, runs.
    Inbox,
    /// Server: push notifications to paired phones (through Apple; only an id goes there).
    Notify {
        #[command(subcommand)]
        action: Option<PushCmd>,
    },
    /// Any other command is git's: `ntr status`, `ntr commit -m …`, `ntr rebase -i main`.
    #[command(external_subcommand)]
    Git(Vec<OsString>),
}

#[derive(Subcommand)]
enum RepoCmd {
    /// Register the working copy containing PATH (default: here) with neutrinod.
    Add { path: Option<PathBuf> },
    /// Forget a registered repo. Nothing on disk is touched.
    Rm { repo: String },
}

#[derive(Subcommand)]
enum MirrorCmd {
    /// Copy the repository (-r) to URL after every push, starting now.
    Set { url: String },
    /// Stop copying it.
    Off,
    /// Copy it now, and wait.
    Now,
}

#[derive(Subcommand)]
enum PushCmd {
    /// Store the APNs auth key from your Apple Developer account.
    Setup {
        /// The AuthKey_XXXXXXXXXX.p8 file.
        #[arg(long)]
        key: PathBuf,
        /// Its ten-character key id. Default: from the file's name.
        #[arg(long)]
        key_id: Option<String>,
        #[arg(long)]
        team_id: String,
        /// The iPhone app's bundle id.
        #[arg(long, default_value = "dev.neutrino.ios")]
        topic: String,
    },
    /// Send a test push to every phone that takes them.
    Test,
}

/// Where a secret or variable applies: this repo (default), every repo, or one environment.
#[derive(clap::Args)]
struct ScopeArgs {
    /// Every repository.
    #[arg(long, conflicts_with = "env")]
    global: bool,
    /// Only jobs deploying to this environment.
    #[arg(long)]
    env: Option<String>,
}

#[derive(Subcommand)]
enum VaultCmd {
    /// Create the key and print its recovery code, once.
    Setup,
    /// Put the key back from a recovery code (a new server, say).
    Recover { code: String },
}

#[derive(Subcommand)]
enum SecretCmd {
    List {
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Set NAME; the value is read from standard input (typed without echo).
    Set {
        name: String,
        #[command(flatten)]
        scope: ScopeArgs,
    },
    Rm {
        name: String,
        #[command(flatten)]
        scope: ScopeArgs,
    },
}

#[derive(Subcommand)]
enum VarCmd {
    List {
        #[command(flatten)]
        scope: ScopeArgs,
    },
    Set {
        name: String,
        value: String,
        #[command(flatten)]
        scope: ScopeArgs,
    },
    Rm {
        name: String,
        #[command(flatten)]
        scope: ScopeArgs,
    },
}

#[derive(Subcommand)]
enum EnvCmd {
    List,
    /// Add an environment, or change one.
    Set {
        name: String,
        /// Jobs wait until someone approves them.
        #[arg(long)]
        approval: bool,
        /// A branch (or pattern) that may deploy here; repeat for more. Default: any.
        #[arg(long = "branch")]
        branches: Vec<String>,
    },
    /// Remove it, with its secrets and variables.
    Rm { name: String },
}

#[derive(Subcommand)]
enum RunCmd {
    /// Its jobs and steps.
    Show { number: u32 },
    /// A job's output; with --follow, until it finishes.
    Logs {
        number: u32,
        /// The job (its key, e.g. test-2). Default: the first one.
        #[arg(long, short = 'j')]
        job: Option<String>,
        #[arg(long, short = 'f')]
        follow: bool,
    },
    Cancel { number: u32 },
    /// The same workflow on the same commit again.
    Rerun { number: u32 },
    /// Let a job waiting on its environment run.
    Approve {
        number: u32,
        /// The job (its key). Default: the one that's waiting.
        #[arg(long, short = 'j')]
        job: Option<String>,
    },
    /// Turn a waiting job down.
    Reject {
        number: u32,
        #[arg(long, short = 'j')]
        job: Option<String>,
    },
    /// Files the run kept.
    Artifacts { number: u32 },
    /// Save an artifact (JOB/PATH, as `artifacts` lists it).
    Download {
        number: u32,
        path: String,
        /// Where to save it. Default: its file name, here.
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,
    },
    /// Start a workflow that has workflow_dispatch.
    Start {
        workflow: String,
        #[arg(long, short = 'b', default_value = "main")]
        branch: String,
    },
}

#[derive(Subcommand)]
enum PrCmd {
    /// Open and recent pull requests.
    List,
    /// Open one to merge SOURCE into TARGET.
    Create {
        source: String,
        /// Default: the repository's default branch.
        target: Option<String>,
        #[arg(long, short = 't')]
        title: String,
        #[arg(long, short = 'm', default_value = "")]
        body: String,
    },
    /// A pull request: versions, files, commits, notes and whether it merges.
    Show { number: u32 },
    Merge {
        number: u32,
        /// merge, squash, rebase or ff.
        #[arg(long, short = 's', default_value = "merge")]
        strategy: String,
        #[arg(long, short = 'm')]
        message: Option<String>,
        /// Delete the source branch afterwards.
        #[arg(long, short = 'd')]
        delete_branch: bool,
    },
    Close { number: u32 },
    Reopen { number: u32 },
    /// Leave a note on a line of the latest version.
    Note {
        number: u32,
        path: String,
        line: u32,
        text: String,
        /// The line number is on the old side.
        #[arg(long)]
        old: bool,
    },
}

#[derive(Subcommand)]
enum RulesCmd {
    /// Guard branches matching PATTERN (`main`, `release/*`).
    Set {
        pattern: String,
        /// Let people push straight to it.
        #[arg(long)]
        allow_push: bool,
        #[arg(long)]
        allow_force: bool,
        #[arg(long)]
        allow_delete: bool,
        /// A workflow (name or file) that must pass before merging; repeat for more.
        #[arg(long = "check")]
        checks: Vec<String>,
    },
    Delete { pattern: String },
    /// Pushes that overrode a rule.
    Overrides,
}

#[derive(Subcommand)]
enum DeviceCmd {
    Revoke { id: String },
}


fn socket_path(arg: Option<PathBuf>) -> PathBuf {
    arg.or_else(|| std::env::var_os("NEUTRINO_SOCKET").map(PathBuf::from)).unwrap_or_else(|| {
        let home = std::env::var_os("NEUTRINO_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".neutrino"));
        home.join("neutrinod.sock")
    })
}

struct Conn {
    stream: BufReader<UnixStream>,
    next: u64,
}

impl Conn {
    fn open(path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(path).with_context(|| {
            format!("can't reach neutrinod at {} (start it with scripts/dev-neutrinod.sh)", path.display())
        })?;
        stream.set_read_timeout(Some(Duration::from_secs(120)))?;
        Ok(Conn { stream: BufReader::new(stream), next: 1 })
    }

    fn call(&mut self, request: Request) -> Result<Response> {
        let id = self.next;
        self.next += 1;
        let line = encode_request(id, &request) + "\n";
        self.stream.get_mut().write_all(line.as_bytes())?;
        let mut buf = String::new();
        loop {
            buf.clear();
            if self.stream.read_line(&mut buf)? == 0 {
                bail!("neutrinod closed the connection");
            }
            let (got, response) = decode_response(buf.trim()).map_err(|e| anyhow!("{e}"))?;
            if got != id {
                continue;
            }
            return match response {
                Response::Error { message } => Err(anyhow!(message)),
                r => Ok(r),
            };
        }
    }
}

fn unexpected(r: Response) -> anyhow::Error {
    anyhow!("unexpected answer from neutrinod: {r:?}")
}

fn repos(c: &mut Conn) -> Result<Vec<Repo>> {
    match c.call(Request::ListRepos)? {
        Response::Repos(r) => Ok(r),
        r => Err(unexpected(r)),
    }
}

// ---------------------------------------------------------------------------
// One command for everything: git's commands, and the server's from anywhere
// ---------------------------------------------------------------------------

/// `ntr <anything else>` is `git <anything else>`, in place of this process,
/// so its pager, colours, prompts and exit code are git's own.
fn run_git(args: &[OsString]) -> Result<()> {
    let args = if args.first().is_some_and(|a| a == "git") { &args[1..] } else { args };
    let err = Command::new("git").args(args).exec();
    Err(anyhow!("running git: {err}"))
}

/// Commands only a server answers.
fn server_only(c: &Cmd) -> bool {
    matches!(
        c,
        Cmd::Host { .. }
            | Cmd::Unhost { .. }
            | Cmd::Import { .. }
            | Cmd::Mirror { .. }
            | Cmd::Pair
            | Cmd::Devices { .. }
            | Cmd::Pr(_)
            | Cmd::Rules { .. }
            | Cmd::Runs { .. }
            | Cmd::Run(_)
            | Cmd::Workflows { .. }
            | Cmd::Ci
            | Cmd::Vault { .. }
            | Cmd::Secret(_)
            | Cmd::Var(_)
            | Cmd::Env(_)
            | Cmd::Backup
            | Cmd::Inbox
            | Cmd::Notify { .. }
    )
}

/// Whether the neutrinod on this Mac is the server.
fn local_is_server(socket: Option<PathBuf>) -> bool {
    let Ok(mut c) = Conn::open(&socket_path(socket)) else { return false };
    matches!(c.call(Request::EngineInfo), Ok(Response::Engine(e)) if e.role == "server")
}

/// Where a server URL points: `ssh://[user@]host[:port]/path` or
/// `[user@]host:/path`. Returns (host, port, path).
fn server_url(url: &str) -> Option<(String, Option<String>, String)> {
    if let Some(rest) = url.strip_prefix("ssh://") {
        let (authority, path) = rest.split_once('/')?;
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h.to_string(), Some(p.to_string())),
            _ => (authority.to_string(), None),
        };
        return Some((host, port, format!("/{path}")));
    }
    if url.contains("://") {
        return None; // https://, git://, file://: not a server over SSH
    }
    let (host, path) = url.split_once(':')?;
    (!host.is_empty() && !host.contains('/') && path.starts_with('/')).then(|| (host.to_string(), None, path.to_string()))
}

fn git_out(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).stderr(std::process::Stdio::null()).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}

/// `'…'` for the server's shell.
fn sh_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@,+".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Run this command with the server's ntr, over SSH. The server is the one
/// the working copy's origin is on (and the repo is that hosted repo), else
/// `$NEUTRINO_SERVER`, else `git config neutrino.server`.
fn forward(repo: Option<&str>) -> Result<()> {
    let origin = if repo.is_none() { git_out(&["remote", "get-url", "origin"]).and_then(|u| server_url(&u)) } else { None };
    let (host, port, path) = match origin {
        Some((h, p, path)) => (h, p, Some(path)),
        None => {
            let named = std::env::var("NEUTRINO_SERVER").ok().filter(|s| !s.is_empty()).or_else(|| git_out(&["config", "--get", "neutrino.server"]));
            let named = named.ok_or_else(|| {
                anyhow!("that's a server command, and this Mac's neutrinod isn't the server. Run it inside a working copy whose origin is on the server, or name the server once: git config --global neutrino.server mini")
            })?;
            (named, None, None)
        }
    };

    // The same arguments, minus --socket (it's this Mac's), with the repo named by its path on the server.
    let mut args: Vec<String> = Vec::new();
    let mut skip_next = false;
    for a in std::env::args().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a == "--socket" {
            skip_next = true;
            continue;
        }
        if a.starts_with("--socket=") {
            continue;
        }
        args.push(a);
    }
    if let Some(p) = &path {
        args.splice(0..0, ["-r".to_string(), p.clone()]);
    }
    let ntr = std::env::var("NEUTRINO_SERVER_NTR").unwrap_or_else(|_| ".neutrino/bin/ntr".into());
    let remote: Vec<String> = std::iter::once(ntr).chain(args.iter().map(|a| sh_quote(a))).collect();

    let mut ssh = Command::new("ssh");
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        ssh.arg("-t"); // prompts (a secret typed without echo) and colours
    } else {
        ssh.arg("-T");
    }
    ssh.arg("-o").arg("LogLevel=ERROR");
    if let Some(p) = port {
        ssh.arg("-p").arg(p);
    }
    let status = ssh.arg(&host).arg(remote.join(" ")).status().context("running ssh")?;
    std::process::exit(status.code().unwrap_or(1));
}

/// `--repo`, `$NEUTRINO_REPO`, or the registered repo containing the current folder.
fn pick_repo(c: &mut Conn, arg: Option<String>) -> Result<String> {
    if let Some(r) = arg.or_else(|| std::env::var("NEUTRINO_REPO").ok()) {
        // A path names the repo registered there (how a workstation names a
        // hosted repo when it forwards a command: by its origin's path).
        if r.starts_with('/') {
            let want = Path::new(&r);
            return repos(c)?
                .into_iter()
                .find(|x| Path::new(&x.path) == want)
                .map(|x| x.id)
                .ok_or_else(|| anyhow!("no repository is registered at {r}"));
        }
        return Ok(r);
    }
    let here = std::env::current_dir()?.canonicalize()?;
    repos(c)?
        .into_iter()
        .filter(|r| here.starts_with(&r.path))
        .max_by_key(|r| r.path.len())
        .map(|r| r.id)
        .ok_or_else(|| anyhow!("this folder isn't in a registered repo: run `ntr repo add`, or pass --repo"))
}

struct Style {
    on: bool,
}

impl Style {
    fn paint(&self, code: &str, text: &str) -> String {
        if self.on { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() }
    }
}

fn ago(t: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(t);
    let s = (now - t).max(0);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        86_400..2_592_000 => format!("{} d ago", s / 86_400),
        _ => format!("{} mo ago", s / 2_592_000),
    }
}

fn graph_cell(e: &LogEntry, width: u16) -> String {
    let Some(g) = &e.graph else { return String::new() };
    let mut cells = vec![' '; width as usize];
    for s in g.upper.iter().chain(&g.lower) {
        if s.from == s.to && (s.from as usize) < cells.len() {
            cells[s.from as usize] = '|';
        }
    }
    if (g.lane as usize) < cells.len() {
        cells[g.lane as usize] = '*';
    }
    cells.iter().flat_map(|c| [*c, ' ']).collect()
}

fn refs(e: &LogEntry, st: &Style) -> String {
    if e.refs.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = e
        .refs
        .iter()
        .map(|r| match r.kind {
            RefKind::Local if r.is_head => st.paint("1;36", &format!("HEAD -> {}", r.name)),
            RefKind::Local => st.paint("36", &r.name),
            RefKind::Remote => st.paint("35", &r.name),
            RefKind::Tag => st.paint("33", &format!("tag: {}", r.name)),
            RefKind::Head => st.paint("1;36", "HEAD"),
        })
        .collect();
    format!(" ({})", parts.join(", "))
}

fn status_letter(s: ChangeStatus) -> &'static str {
    match s {
        ChangeStatus::Added => "A",
        ChangeStatus::Modified => "M",
        ChangeStatus::Deleted => "D",
        ChangeStatus::Renamed => "R",
        ChangeStatus::Copied => "C",
        ChangeStatus::TypeChanged => "T",
        ChangeStatus::Untracked => "?",
        ChangeStatus::Conflicted => "U",
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // Anything ntr doesn't know is git's.
    if let Cmd::Git(args) = &cli.command {
        return run_git(args);
    }
    // Server commands from a workstation run on the server, over SSH.
    if server_only(&cli.command) && !local_is_server(cli.socket.clone()) {
        return forward(cli.repo.as_deref());
    }
    let st = Style { on: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() };
    let style = Style { on: st.on };
    let mut c = Conn::open(&socket_path(cli.socket))?;
    let out = &mut std::io::stdout().lock();

    match cli.command {
        Cmd::Git(_) => unreachable!("handled before connecting"),
        Cmd::Ping => match c.call(Request::Ping)? {
            Response::Pong { protocol } => writeln!(out, "neutrinod is up (protocol {protocol})")?,
            r => return Err(unexpected(r)),
        },
        Cmd::Info => match c.call(Request::EngineInfo)? {
            Response::Engine(e) => {
                let commit = match &e.commit {
                    Some(c) => format!("commit {c}"),
                    None => "commit unknown (not built by deploy-server.sh)".into(),
                };
                writeln!(out, "neutrinod {} · {} · {} repos · protocol {} · {commit}", e.version, e.role, e.repos, e.protocol)?
            }
            r => return Err(unexpected(r)),
        },
        Cmd::Repos => {
            let list = repos(&mut c)?;
            if list.is_empty() {
                writeln!(out, "No repos yet. Run `ntr repo add` inside one.")?;
            }
            for r in list {
                let head = match (&r.head, &r.error) {
                    (_, Some(e)) => st.paint("31", e),
                    (Some(Head::Branch { name, .. }), _) => st.paint("36", name),
                    (Some(Head::Detached { sha }), _) => format!("detached at {}", &sha[..7.min(sha.len())]),
                    (None, None) => String::new(),
                };
                writeln!(out, "{:<20} {:<24} {}", st.paint("1", &r.id), head, r.path)?;
            }
        }
        Cmd::Repo(RepoCmd::Add { path }) => {
            let path = path.unwrap_or_else(|| PathBuf::from("."));
            let abs = std::fs::canonicalize(&path).with_context(|| format!("{} doesn't exist", path.display()))?;
            match c.call(Request::AddRepo { path: abs.display().to_string() })? {
                Response::Repo(r) => writeln!(out, "Added {} as {}", r.path, st.paint("1", &r.id))?,
                r => return Err(unexpected(r)),
            }
        }
        Cmd::Repo(RepoCmd::Rm { repo }) => match c.call(Request::RemoveRepo { repo })? {
            Response::Removed { repo } => writeln!(out, "Forgot {repo}. Its files are untouched.")?,
            r => return Err(unexpected(r)),
        },
        Cmd::Branches => {
            let repo = pick_repo(&mut c, cli.repo)?;
            let Response::Branches(list) = c.call(Request::Branches { repo })? else { bail!("unexpected answer") };
            for b in list {
                let mark = if b.is_head { "*" } else { " " };
                let name = if b.kind == RefKind::Remote { st.paint("35", &b.name) } else { st.paint("36", &b.name) };
                let track = match (&b.upstream, b.ahead, b.behind) {
                    (Some(u), Some(a), Some(bh)) => format!(" [{u}: ↑{a} ↓{bh}]"),
                    (Some(u), _, _) => format!(" [{u}: gone]"),
                    _ => String::new(),
                };
                writeln!(out, "{mark} {name:<30} {} {}{} {}", &b.sha[..7], b.subject, track, st.paint("2", &ago(b.time)))?;
            }
        }
        Cmd::History { rev, limit, skip, all, query } => {
            let repo = pick_repo(&mut c, cli.repo)?;
            let started = std::time::Instant::now();
            let q = LogQuery { repo, rev, all, skip, limit: Some(limit), query };
            let Response::Log(page) = c.call(Request::Log(q))? else { bail!("unexpected answer") };
            let took = started.elapsed();
            for e in &page.commits {
                writeln!(
                    out,
                    "{}{} {}{} {}",
                    graph_cell(e, page.lanes),
                    st.paint("33", &e.sha[..7]),
                    e.subject,
                    refs(e, &st),
                    st.paint("2", &format!("{}, {}", e.author.name, ago(e.time)))
                )?;
            }
            if page.has_more {
                writeln!(out, "{}", st.paint("2", &format!("… more with --skip {}", skip + page.commits.len() as u32)))?;
            }
            if std::env::var_os("NTR_TIMING").is_some() {
                eprintln!("{} commits in {:.1} ms", page.commits.len(), took.as_secs_f64() * 1000.0);
            }
        }
        Cmd::Unstage { paths } => {
            let repo = pick_repo(&mut c, cli.repo)?;
            done(out, c.call(Request::Unstage { repo, paths: rel_paths(&paths)? })?)?
        }
        Cmd::Discard { paths } => {
            let repo = pick_repo(&mut c, cli.repo)?;
            done(out, c.call(Request::Discard { repo, paths: rel_paths(&paths)? })?)?;
            writeln!(out, "Discarded. Copies are listed in ~/.neutrino/discarded.log.")?;
        }
        Cmd::Unhost { name, yes } => {
            if !yes {
                writeln!(out, "This stops hosting {name}: its repository moves to ~/.neutrino/deleted/ on the server,")?;
                writeln!(out, "its working copies' origin stops answering, and the GitHub mirror stops being updated.")?;
                writeln!(out, "Nothing is erased. Run again with --yes to do it.")?;
                return Ok(());
            }
            match c.call(Request::DeleteHosted { repo: name.clone() })? {
                Response::Removed { .. } => {
                    writeln!(out, "Stopped hosting {name}. It's in ~/.neutrino/deleted/ on the server;")?;
                    writeln!(out, "move it back to ~/.neutrino/repos/{name}.git and restart neutrinod to host it again.")?;
                }
                r => return Err(unexpected(r)),
            }
        }
        Cmd::Host { name, description } => match c.call(Request::CreateHosted { name, description })? {
            Response::Repo(r) => {
                writeln!(out, "Created {} at {}", style.paint("1", &r.id), r.path)?;
                writeln!(out, "Push to it with:  git remote add atlas ssh://<this-mac>{}", r.path)?;
            }
            r => return Err(unexpected(r)),
        },
        Cmd::Import { name, from, description, no_mirror } => {
            // A big repository takes a while to copy.
            c.stream.get_mut().set_read_timeout(None)?;
            match c.call(Request::ImportHosted { name, url: from.clone(), description, mirror: !no_mirror })? {
                Response::Repo(r) => {
                    writeln!(out, "Imported {} from {from}", style.paint("1", &r.id))?;
                    writeln!(out, "Hosted at      {}", r.path)?;
                    if !no_mirror {
                        writeln!(out, "Mirrored to    {from} after every push")?;
                    }
                    writeln!(out, "Point clones at it:  git remote set-url origin ssh://<this-mac>{}", r.path)?;
                }
                r => return Err(unexpected(r)),
            }
        }
        Cmd::Mirror { action } => {
            let response = match action {
                None => c.call(Request::Mirrors)?,
                Some(a) => {
                    let repo = pick_repo(&mut c, cli.repo)?;
                    match a {
                        MirrorCmd::Set { url } => c.call(Request::SetMirror { repo, url: Some(url) })?,
                        MirrorCmd::Off => c.call(Request::SetMirror { repo, url: None })?,
                        MirrorCmd::Now => {
                            c.stream.get_mut().set_read_timeout(None)?;
                            c.call(Request::MirrorNow { repo })?
                        }
                    }
                }
            };
            match response {
                Response::Mirrors(list) => {
                    if list.is_empty() {
                        writeln!(out, "No hosted repository has a mirror. Add one with: ntr -r REPO mirror set URL")?;
                    }
                    for m in list {
                        let state = match m.state.as_str() {
                            "ok" => style.paint("32", "ok     "),
                            "failing" => style.paint("31", "failing"),
                            _ => style.paint("33", "pending"),
                        };
                        let when = m.last_ok.map(|t| format!("copied {}", ago(t))).unwrap_or_else(|| "never copied".into());
                        writeln!(out, "{state}  {:<24} {:<44} {when}", m.repo, m.url)?;
                        if let (Some(e), true) = (&m.last_error, m.state == "failing") {
                            for line in e.lines().take(3) {
                                writeln!(out, "         {}", style.paint("2", line))?;
                            }
                        }
                        if !m.mirror_only.is_empty() {
                            let names: Vec<&str> = m.mirror_only.iter().map(|r| r.strip_prefix("refs/heads/").or_else(|| r.strip_prefix("refs/tags/")).unwrap_or(r)).collect();
                            writeln!(out, "         {}", style.paint("2", &format!("only on the mirror, left alone: {}", names.join(", "))))?;
                        }
                    }
                }
                r => return Err(unexpected(r)),
            }
        }
        Cmd::Backups { paths } => match c.call(Request::ListBackups)? {
            Response::Backups(b) => {
                if paths {
                    for f in &b.files {
                        writeln!(out, "{}", f.path)?;
                    }
                } else {
                    match &b.source {
                        Some(host) => {
                            let last = b.last_copy.map(ago).unwrap_or_else(|| "not yet".into());
                            writeln!(out, "Copies of {host}'s snapshots in {}; last copied {last}", b.dir)?;
                            if let Some(e) = &b.last_error {
                                writeln!(out, "{} {e}", style.paint("31", "Last try failed:"))?;
                            }
                        }
                        None => writeln!(out, "Snapshots in {}", b.dir)?,
                    }
                    if b.files.is_empty() {
                        writeln!(out, "None yet.")?;
                    }
                    for f in &b.files {
                        writeln!(out, "{:<32} {:>8.1} MB  {}", f.name, f.size as f64 / 1_048_576.0, ago(f.modified))?;
                    }
                }
            }
            r => return Err(unexpected(r)),
        },
        Cmd::Pair => match c.call(Request::PairBegin)? {
            Response::Pairing(p) => {
                writeln!(out, "Pairing code  {}", style.paint("1;35", &format!("{}-{}", &p.code[..4], &p.code[4..])))?;
                writeln!(out, "Server        {} on port {}", p.host, p.port)?;
                writeln!(out, "Addresses     {}", p.addresses.join(", "))?;
                writeln!(out, "Fingerprint   {}", short_fp(&p.fingerprint))?;
                writeln!(out, "\nIn the Mac app, choose Pair with atlas and enter the code. Check the fingerprint")?;
                writeln!(out, "it shows matches the one above. The code works once, for {} minutes.", p.expires_in_secs / 60)?;
            }
            r => return Err(unexpected(r)),
        },
        Cmd::Devices { action: None } => match c.call(Request::ListDevices)? {
            Response::Devices(list) => {
                if list.is_empty() {
                    writeln!(out, "No paired devices. Run `ntr pair` to pair one.")?;
                }
                for d in list {
                    let seen = d.last_seen.map(|t| ago(t as i64)).unwrap_or_else(|| "never".into());
                    let extra = match (d.signs, d.push) {
                        (true, true) => "  signs approvals, takes pushes",
                        (true, false) => "  signs approvals",
                        (false, true) => "  takes pushes",
                        _ => "",
                    };
                    writeln!(out, "{}  {:<24} paired {}, last seen {}{extra}", style.paint("33", &d.id), d.name, ago(d.paired_at as i64), seen)?;
                }
            }
            r => return Err(unexpected(r)),
        },
        Cmd::Devices { action: Some(DeviceCmd::Revoke { id }) } => match c.call(Request::RevokeDevice { id })? {
            Response::Revoked { id } => writeln!(out, "Revoked {id}. Its token no longer works.")?,
            r => return Err(unexpected(r)),
        },
        Cmd::Pr(cmd) => {
            let repo = pick_repo(&mut c, cli.repo)?;
            pr(&mut c, out, &style, repo, cmd)?;
        }
        Cmd::Rules { action } => {
            let repo = pick_repo(&mut c, cli.repo)?;
            let request = match action {
                None => Request::BranchRules { repo },
                Some(RulesCmd::Set { pattern, allow_push, allow_force, allow_delete, checks }) => Request::SetBranchRule {
                    repo,
                    rule: BranchRule { pattern, require_pull: !allow_push, block_force: !allow_force, block_delete: !allow_delete, required_checks: checks },
                },
                Some(RulesCmd::Delete { pattern }) => Request::DeleteBranchRule { repo, pattern },
                Some(RulesCmd::Overrides) => Request::Overrides { repo },
            };
            match c.call(request)? {
                Response::Rules(rules) => {
                    if rules.is_empty() {
                        writeln!(out, "No branch rules. Add one with `ntr rules set main`.")?;
                    }
                    for r in rules {
                        let mut guards = Vec::new();
                        if r.require_pull {
                            guards.push("pull requests only");
                        }
                        if r.block_force {
                            guards.push("no force-push");
                        }
                        if r.block_delete {
                            guards.push("no delete");
                        }
                        let checks = if r.required_checks.is_empty() { String::new() } else { format!("checks {}", r.required_checks.join(", ")) };
                        if !checks.is_empty() {
                            guards.push(&checks);
                        }
                        let guards = if guards.is_empty() { "nothing".to_string() } else { guards.join(", ") };
                        writeln!(out, "{:<20} {guards}", style.paint("1", &r.pattern))?;
                    }
                }
                Response::Overrides(list) => {
                    if list.is_empty() {
                        writeln!(out, "No overrides.")?;
                    }
                    for o in list {
                        writeln!(out, "{}  {:<24} {:<13} {}", style.paint("2", &ago(o.at)), o.ref_name, o.rule, o.reason)?;
                    }
                }
                r => return Err(unexpected(r)),
            }
        }
        Cmd::Runs { pr, limit } => {
            let repo = pick_repo(&mut c, cli.repo)?;
            match c.call(Request::ListRuns { repo, pull: pr, limit: Some(limit) })? {
                Response::Runs(list) => {
                    if list.is_empty() {
                        writeln!(out, "No runs yet. Push a workflow to .neutrino/workflows/.")?;
                    }
                    for r in list {
                        let what = match (r.pull, &r.branch) {
                            (Some(n), _) => format!("#{n}"),
                            (None, Some(b)) => b.clone(),
                            _ => String::new(),
                        };
                        writeln!(
                            out,
                            "{:<5} {}  {:<20} {:<18} {:<12} {}  {}",
                            r.number,
                            state_mark(r.state, &style),
                            r.name,
                            r.event,
                            what,
                            &r.sha[..7.min(r.sha.len())],
                            style.paint("2", &ago(r.created_at))
                        )?;
                    }
                }
                r => return Err(unexpected(r)),
            }
        }
        Cmd::Run(cmd) => {
            let repo = pick_repo(&mut c, cli.repo)?;
            run_cmd(&mut c, out, &style, repo, cmd)?;
        }
        Cmd::Workflows { branch } => {
            let repo = pick_repo(&mut c, cli.repo)?;
            match c.call(Request::Workflows { repo, branch: branch.clone() })? {
                Response::Workflows(files) => {
                    if files.is_empty() {
                        writeln!(out, "No workflow files in .neutrino/workflows/ on {branch}.")?;
                    }
                    for f in files {
                        match &f.error {
                            Some(e) => writeln!(out, "{}  {}", style.paint("31", &f.file), e)?,
                            None => writeln!(
                                out,
                                "{}  {}  on {}  jobs {}",
                                style.paint("1", &f.file),
                                f.name.as_deref().unwrap_or(""),
                                f.triggers.join(", "),
                                f.jobs.join(", ")
                            )?,
                        }
                        for w in &f.warnings {
                            writeln!(out, "    {}", style.paint("33", w))?;
                        }
                    }
                }
                r => return Err(unexpected(r)),
            }
        }
        Cmd::Vault { action } => {
            let request = match action {
                None => Request::VaultInfo,
                Some(VaultCmd::Setup) => Request::VaultSetup,
                Some(VaultCmd::Recover { code }) => Request::VaultRecover { code },
            };
            match c.call(request)? {
                Response::Vault(v) => {
                    if v.set_up {
                        writeln!(out, "The vault is set up; its key is in the {}.", if v.key_store == "keychain" { "login Keychain" } else { "key file" })?;
                    } else {
                        writeln!(out, "The vault isn't set up. Run `ntr vault setup` to create its key.")?;
                    }
                }
                Response::RecoveryCode { code } => {
                    writeln!(out, "Recovery code\n\n    {}\n", style.paint("1;35", &code))?;
                    writeln!(out, "Write it down somewhere safe, away from this Mac. It's shown only now. With it, a")?;
                    writeln!(out, "database backup's secrets open on another server (`ntr vault recover`); without")?;
                    writeln!(out, "it they have to be entered again.")?;
                }
                r => return Err(unexpected(r)),
            }
        }
        Cmd::Secret(cmd) => {
            let (scope, request) = match cmd {
                SecretCmd::List { scope } => {
                    let s = vault_scope(&mut c, cli.repo, scope)?;
                    (s.clone(), Request::ListSecrets { scope: s })
                }
                SecretCmd::Set { name, scope } => {
                    let s = vault_scope(&mut c, cli.repo, scope)?;
                    let value = read_secret(&name)?;
                    (s.clone(), Request::SetSecret { scope: s, name, value })
                }
                SecretCmd::Rm { name, scope } => {
                    let s = vault_scope(&mut c, cli.repo, scope)?;
                    (s.clone(), Request::DeleteSecret { scope: s, name })
                }
            };
            match c.call(request)? {
                Response::Secrets(list) => {
                    if list.is_empty() {
                        writeln!(out, "No secrets{}.", scope_text(&scope))?;
                    }
                    for s in list {
                        writeln!(out, "{:<28} {:<24} {}", style.paint("1", &s.name), scope_text(&s.scope).trim_start(), style.paint("2", &ago(s.updated_at)))?;
                    }
                }
                r => return Err(unexpected(r)),
            }
        }
        Cmd::Var(cmd) => {
            let request = match cmd {
                VarCmd::List { scope } => Request::ListVariables { scope: vault_scope(&mut c, cli.repo, scope)? },
                VarCmd::Set { name, value, scope } => Request::SetVariable { scope: vault_scope(&mut c, cli.repo, scope)?, name, value },
                VarCmd::Rm { name, scope } => Request::DeleteVariable { scope: vault_scope(&mut c, cli.repo, scope)?, name },
            };
            match c.call(request)? {
                Response::Variables(list) => {
                    if list.is_empty() {
                        writeln!(out, "No variables.")?;
                    }
                    for v in list {
                        writeln!(out, "{:<28} {:<24} {}", style.paint("1", &v.name), scope_text(&v.scope).trim_start(), v.value)?;
                    }
                }
                r => return Err(unexpected(r)),
            }
        }
        Cmd::Env(cmd) => {
            let repo = pick_repo(&mut c, cli.repo)?;
            let request = match cmd {
                EnvCmd::List => Request::ListEnvironments { repo },
                EnvCmd::Set { name, approval, branches } => {
                    Request::SetEnvironment { environment: neutrino_proto::Environment { repo, name, require_approval: approval, branches } }
                }
                EnvCmd::Rm { name } => Request::DeleteEnvironment { repo, name },
            };
            match c.call(request)? {
                Response::Environments(list) => {
                    if list.is_empty() {
                        writeln!(out, "No environments. Add one with `ntr env set production --approval --branch main`.")?;
                    }
                    for e in list {
                        let who = if e.branches.is_empty() { "any branch".to_string() } else { e.branches.join(", ") };
                        let gate = if e.require_approval { "needs approval" } else { "no approval" };
                        writeln!(out, "{:<20} {gate}, {who}", style.paint("1", &e.name))?;
                    }
                }
                r => return Err(unexpected(r)),
            }
        }
        Cmd::Backup => done(out, c.call(Request::Backup)?)?,
        Cmd::Inbox => match c.call(Request::Inbox)? {
            Response::Inbox(i) => {
                if i.waiting.is_empty() && i.ready.is_empty() && i.open.is_empty() {
                    writeln!(out, "Nothing needs you.")?;
                }
                for w in &i.waiting {
                    writeln!(
                        out,
                        "{} {} run {}: {} to {}  ({})",
                        style.paint("35", "approve"),
                        w.repo,
                        w.number,
                        w.job_name,
                        w.environment.as_deref().unwrap_or("?"),
                        ago(w.since)
                    )?;
                }
                for p in &i.ready {
                    writeln!(out, "{} {} #{} {}", style.paint("32", "ready  "), p.repo, p.number, p.title)?;
                }
                for p in &i.open {
                    writeln!(out, "{} {} #{} {}", style.paint("2", "open   "), p.repo, p.number, p.title)?;
                }
                let running: Vec<_> = i.runs.iter().filter(|r| !r.state.finished()).collect();
                if !running.is_empty() {
                    writeln!(out)?;
                }
                for r in running {
                    writeln!(out, "{} {} {} {}", state_mark(r.state, &style), r.repo, r.name, r.number)?;
                }
            }
            r => return Err(unexpected(r)),
        },
        Cmd::Notify { action } => {
            let request = match action {
                None => Request::PushInfo,
                Some(PushCmd::Test) => Request::PushTest,
                Some(PushCmd::Setup { key, key_id, team_id, topic }) => {
                    let key_p8 = std::fs::read_to_string(&key).with_context(|| format!("reading {}", key.display()))?;
                    let key_id = match key_id {
                        Some(k) => k,
                        None => key
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .and_then(|s| s.strip_prefix("AuthKey_"))
                            .map(String::from)
                            .ok_or_else(|| anyhow!("give --key-id; it isn't in the file's name"))?,
                    };
                    Request::PushSetup { key_p8, key_id, team_id, topic }
                }
            };
            match c.call(request)? {
                Response::Push(p) => {
                    if p.configured {
                        writeln!(out, "Pushes go out for {} to {} phone{}.", p.topic.unwrap_or_default(), p.devices, if p.devices == 1 { "" } else { "s" })?;
                    } else {
                        writeln!(out, "No APNs key yet: `ntr notify setup --key AuthKey_….p8 --team-id …`. Everything else works without it.")?;
                    }
                }
                Response::Done { output } => writeln!(out, "{output}")?,
                r => return Err(unexpected(r)),
            }
        }
        Cmd::Ci => match c.call(Request::CiInfo)? {
            Response::Ci(i) => {
                let g = if i.graviton { style.paint("32", "answers") } else { style.paint("31", &format!("unreachable: {}", i.graviton_error.unwrap_or_default())) };
                writeln!(out, "Graviton   {g}")?;
                writeln!(out, "Budget     {} CPUs, {} MiB", i.budget_cpus, i.budget_memory_mib)?;
                writeln!(out, "In use     {} CPUs, {} MiB by {} job{}", i.used_cpus, i.used_memory_mib, i.running, if i.running == 1 { "" } else { "s" })?;
                writeln!(out, "Queued     {}", i.queued)?;
                writeln!(out, "Host       {}", if i.host { "allowed (runs-on: host runs unisolated here)" } else { "off (--host-executor)" })?;
                writeln!(out, "Nexus      {}", i.nexus.as_deref().unwrap_or("off (--nexus-token-file)"))?;
            }
            r => return Err(unexpected(r)),
        },
        Cmd::Watch => {
            let id = c.next;
            c.next += 1;
            c.stream.get_mut().write_all((encode_request(id, &Request::Subscribe) + "\n").as_bytes())?;
            c.stream.get_mut().set_read_timeout(None)?;
            let mut buf = String::new();
            loop {
                buf.clear();
                if c.stream.read_line(&mut buf)? == 0 {
                    break;
                }
                if let Ok((_, Response::Event(e))) = decode_response(buf.trim()) {
                    writeln!(out, "{}  {}", style.paint("1", &e.repo), e.kind)?;
                    out.flush()?;
                }
            }
        }
    }
    Ok(())
}

fn state_mark(s: RunState, style: &Style) -> String {
    let (code, text) = match s {
        RunState::Queued => ("2", "queued   "),
        RunState::Running => ("33", "running  "),
        RunState::Success => ("32", "success  "),
        RunState::Failure => ("31", "failure  "),
        RunState::Cancelled => ("2", "cancelled"),
        RunState::Skipped => ("2", "skipped  "),
        RunState::Waiting => ("35", "waiting  "),
    };
    style.paint(code, text)
}

/// A run's id from its number in the repo.
fn run_id(c: &mut Conn, repo: &str, number: u32) -> Result<i64> {
    match c.call(Request::ListRuns { repo: repo.into(), pull: None, limit: Some(500) })? {
        Response::Runs(list) => list.into_iter().find(|r| r.number == number).map(|r| r.id).ok_or_else(|| anyhow!("there's no run {number} in {repo}")),
        r => Err(unexpected(r)),
    }
}

fn print_run(out: &mut impl Write, style: &Style, d: &RunDetail) -> Result<()> {
    let r = &d.run;
    writeln!(out, "{} {}  {}", style.paint("1", &format!("Run {}", r.number)), style.paint("1", &r.name), state_mark(r.state, style).trim_end())?;
    let at = match (r.pull, &r.branch) {
        (Some(n), _) => format!("pull request #{n}"),
        (None, Some(b)) => b.clone(),
        _ => String::new(),
    };
    writeln!(out, "{} on {at} at {}, {}", r.event, &r.sha[..7.min(r.sha.len())], ago(r.created_at))?;
    if let Some(e) = &r.error {
        writeln!(out, "\n{}", style.paint("31", e))?;
    }
    for w in &d.warnings {
        writeln!(out, "{}", style.paint("33", w))?;
    }
    for j in &d.jobs {
        let took = match (j.started_at, j.finished_at) {
            (Some(a), Some(b)) => format!("  {}s", b - a),
            _ => String::new(),
        };
        writeln!(out, "\n{} {}  {}{}", state_mark(j.state, style), style.paint("1", &j.name), style.paint("2", &j.key), style.paint("2", &took))?;
        if let Some(e) = &j.error {
            writeln!(out, "    {}", style.paint("31", e))?;
        }
        for s in &j.steps {
            let code = s.exit_code.filter(|c| *c != 0).map(|c| format!(" (exit {c})")).unwrap_or_default();
            writeln!(out, "    {} {}{}", state_mark(s.state, style), s.name, code)?;
        }
    }
    Ok(())
}

/// Approve or turn down the waiting job of a run.
fn decide(c: &mut Conn, out: &mut impl Write, repo: &str, number: u32, job: Option<String>, approve: bool) -> Result<()> {
    let id = run_id(c, repo, number)?;
    let d = match c.call(Request::Run { repo: repo.into(), id })? {
        Response::Run(d) => d,
        r => return Err(unexpected(r)),
    };
    let waiting: Vec<_> = d.jobs.iter().filter(|j| j.state == RunState::Waiting).collect();
    let j = match &job {
        Some(k) => waiting.iter().find(|j| &j.key == k).copied().ok_or_else(|| anyhow!("job {k} of run {number} isn't waiting"))?,
        None => match waiting.as_slice() {
            [one] => *one,
            [] => bail!("nothing in run {number} is waiting for approval"),
            many => bail!("several jobs are waiting ({}); pick one with --job", many.iter().map(|j| j.key.as_str()).collect::<Vec<_>>().join(", ")),
        },
    };
    match c.call(Request::ApproveJob { repo: repo.into(), job: j.id, approve })? {
        Response::Run(_) => writeln!(out, "{} {} ({}).", if approve { "Approved" } else { "Turned down" }, j.name, j.environment.as_deref().unwrap_or(""))?,
        r => return Err(unexpected(r)),
    }
    Ok(())
}

/// `--global`, `--env NAME` or this repo.
fn vault_scope(c: &mut Conn, repo: Option<String>, scope: ScopeArgs) -> Result<VaultScope> {
    if scope.global {
        return Ok(VaultScope::default());
    }
    Ok(VaultScope { repo: Some(pick_repo(c, repo)?), environment: scope.env })
}

fn scope_text(s: &VaultScope) -> String {
    match (&s.repo, &s.environment) {
        (None, _) => " (every repo)".into(),
        (Some(r), None) => format!(" in {r}"),
        (Some(r), Some(e)) => format!(" in {r}, {e}"),
    }
}

/// A secret's value from standard input; typed without echo at a terminal.
fn read_secret(name: &str) -> Result<String> {
    let tty = std::io::stdin().is_terminal();
    if tty {
        eprint!("Value for {name} (not shown): ");
        let _ = std::process::Command::new("stty").arg("-echo").stdin(std::process::Stdio::inherit()).status();
    }
    let mut value = String::new();
    let read = if tty { std::io::stdin().read_line(&mut value).map(|_| ()) } else { std::io::Read::read_to_string(&mut std::io::stdin(), &mut value).map(|_| ()) };
    if tty {
        let _ = std::process::Command::new("stty").arg("echo").stdin(std::process::Stdio::inherit()).status();
        eprintln!();
    }
    read?;
    // A trailing newline from `echo` or the Return key isn't part of it.
    let value = value.strip_suffix('\n').map(|v| v.strip_suffix('\r').unwrap_or(v)).unwrap_or(&value).to_string();
    if value.is_empty() {
        bail!("no value given");
    }
    Ok(value)
}

fn run_cmd(c: &mut Conn, out: &mut impl Write, style: &Style, repo: String, cmd: RunCmd) -> Result<()> {
    let detail = |r: Response| -> Result<RunDetail> {
        match r {
            Response::Run(d) => Ok(*d),
            r => Err(unexpected(r)),
        }
    };
    match cmd {
        RunCmd::Approve { number, job } => decide(c, out, &repo, number, job, true)?,
        RunCmd::Reject { number, job } => decide(c, out, &repo, number, job, false)?,
        RunCmd::Artifacts { number } => {
            let id = run_id(c, &repo, number)?;
            match c.call(Request::ListArtifacts { repo, run: id })? {
                Response::Artifacts(list) => {
                    if list.is_empty() {
                        writeln!(out, "Run {number} kept no artifacts.")?;
                    }
                    for a in list {
                        writeln!(out, "{}/{}  {}", style.paint("2", &a.job), a.path, style.paint("2", &format!("{} KB", a.size.div_ceil(1024))))?;
                    }
                }
                r => return Err(unexpected(r)),
            }
        }
        RunCmd::Download { number, path, out: dest } => {
            use base64::Engine as _;
            let id = run_id(c, &repo, number)?;
            let dest = dest.unwrap_or_else(|| PathBuf::from(Path::new(&path).file_name().map(|n| n.to_os_string()).unwrap_or_else(|| "artifact".into())));
            let mut file = std::fs::File::create(&dest).with_context(|| format!("creating {}", dest.display()))?;
            let mut offset = 0;
            loop {
                match c.call(Request::ArtifactData { repo: repo.clone(), run: id, path: path.clone(), offset })? {
                    Response::ArtifactChunk(chunk) => {
                        let bytes = base64::engine::general_purpose::STANDARD.decode(&chunk.data).context("reading the artifact")?;
                        file.write_all(&bytes)?;
                        offset += bytes.len() as u64;
                        if chunk.done || bytes.is_empty() {
                            break;
                        }
                    }
                    r => return Err(unexpected(r)),
                }
            }
            writeln!(out, "Saved {} ({} KB).", dest.display(), offset.div_ceil(1024))?;
        }
        RunCmd::Show { number } => {
            let id = run_id(c, &repo, number)?;
            print_run(out, style, &detail(c.call(Request::Run { repo, id })?)?)?;
        }
        RunCmd::Cancel { number } => {
            let id = run_id(c, &repo, number)?;
            detail(c.call(Request::CancelRun { repo, id })?)?;
            writeln!(out, "Cancelling run {number}.")?;
        }
        RunCmd::Rerun { number } => {
            let id = run_id(c, &repo, number)?;
            let d = detail(c.call(Request::RerunRun { repo, id })?)?;
            writeln!(out, "Started run {} ({}).", d.run.number, d.run.name)?;
        }
        RunCmd::Start { workflow, branch } => {
            let d = detail(c.call(Request::DispatchRun { repo, workflow, branch })?)?;
            writeln!(out, "Started run {} ({}) on {}.", d.run.number, d.run.name, d.run.branch.unwrap_or_default())?;
        }
        RunCmd::Logs { number, job, follow } => {
            let id = run_id(c, &repo, number)?;
            let d = detail(c.call(Request::Run { repo: repo.clone(), id })?)?;
            let j = match &job {
                Some(k) => d.jobs.iter().find(|j| &j.key == k || &j.name == k).ok_or_else(|| {
                    anyhow!("run {number} has no job {k}; it has {}", d.jobs.iter().map(|j| j.key.as_str()).collect::<Vec<_>>().join(", "))
                })?,
                None => d.jobs.first().ok_or_else(|| anyhow!("run {number} has no jobs"))?,
            };
            let mut offset = 0;
            loop {
                match c.call(Request::JobLog { repo: repo.clone(), job: j.id, offset })? {
                    Response::JobLog(chunk) => {
                        write!(out, "{}", chunk.text)?;
                        out.flush()?;
                        let more = chunk.next > offset;
                        offset = chunk.next;
                        if chunk.done || (!follow && !more) {
                            break;
                        }
                        if !more {
                            std::thread::sleep(Duration::from_millis(500));
                        }
                    }
                    r => return Err(unexpected(r)),
                }
            }
        }
    }
    Ok(())
}

fn pr(c: &mut Conn, out: &mut impl Write, style: &Style, repo: String, cmd: PrCmd) -> Result<()> {
    let request = match cmd {
        PrCmd::List => Request::ListPulls { repo },
        PrCmd::Create { source, target, title, body } => {
            let target = match target {
                Some(t) => t,
                None => match repos(c)?.into_iter().find(|r| r.id == repo).and_then(|r| r.head) {
                    Some(Head::Branch { name, .. }) => name,
                    _ => bail!("{repo} has no default branch; name the target"),
                },
            };
            Request::CreatePull { repo, title, body, source, target }
        }
        PrCmd::Show { number } => Request::Pull { repo, number },
        PrCmd::Merge { number, strategy, message, delete_branch } => {
            let strategy = match strategy.as_str() {
                "merge" => MergeStrategy::Merge,
                "squash" => MergeStrategy::Squash,
                "rebase" => MergeStrategy::Rebase,
                "ff" | "fast-forward" => MergeStrategy::FastForward,
                other => bail!("{other:?} isn't a strategy: use merge, squash, rebase or ff"),
            };
            Request::MergePull { repo, number, strategy, message, delete_source: delete_branch }
        }
        PrCmd::Close { number } => Request::UpdatePull { repo, number, title: None, body: None, state: Some(PullState::Closed) },
        PrCmd::Reopen { number } => Request::UpdatePull { repo, number, title: None, body: None, state: Some(PullState::Open) },
        PrCmd::Note { number, path, line, text, old } => {
            Request::AddNote { repo, number, path, side: if old { Side::Old } else { Side::New }, line, text }
        }
    };
    let state = |s: PullState| match s {
        PullState::Open => style.paint("32", "open"),
        PullState::Merged => style.paint("35", "merged"),
        PullState::Closed => style.paint("31", "closed"),
    };
    match c.call(request)? {
        Response::Pulls(list) => {
            if list.is_empty() {
                writeln!(out, "No pull requests. Open one with `ntr pr create BRANCH -t TITLE`.")?;
            }
            for p in list {
                let notes = if p.open_notes > 0 { format!("  {} notes", p.open_notes) } else { String::new() };
                writeln!(
                    out,
                    "{} {:<6} {}  {} → {}  {}{}",
                    style.paint("1", &format!("#{:<3}", p.number)),
                    state(p.state),
                    p.title,
                    style.paint("36", &p.source),
                    style.paint("36", &p.target),
                    style.paint("2", &ago(p.updated_at)),
                    notes
                )?;
            }
        }
        Response::Pull(d) => {
            let p = &d.pull;
            writeln!(out, "{} {}  {}", style.paint("1", &format!("#{}", p.number)), style.paint("1", &p.title), state(p.state))?;
            writeln!(out, "{} → {}, version {}, opened {}", style.paint("36", &p.source), style.paint("36", &p.target), p.versions, ago(p.created_at))?;
            if !p.body.is_empty() {
                writeln!(out, "\n{}", p.body)?;
            }
            if let Some(m) = &d.mergeability {
                let how = if m.fast_forward {
                    style.paint("32", "can fast-forward")
                } else if m.clean {
                    style.paint("32", &format!("merges cleanly ({} behind {})", m.behind, p.target))
                } else {
                    style.paint("31", &format!("conflicts in {}", m.conflicts.join(", ")))
                };
                writeln!(out, "\n{how}")?;
            }
            if let Some(sha) = &p.merge_sha {
                writeln!(out, "\nMerged as {}", &sha[..7.min(sha.len())])?;
            }
            if let Some(r) = &d.rule {
                writeln!(out, "{} is guarded by the rule {}", p.target, style.paint("1", &r.pattern))?;
            }
            if !d.checks.is_empty() {
                writeln!(out, "\nChecks")?;
                for r in &d.checks {
                    writeln!(out, "  {} {}  run {}", state_mark(r.state, style), r.name, r.number)?;
                }
            }
            writeln!(out, "\nCommits")?;
            for e in &d.commits {
                writeln!(out, "  {} {}", style.paint("33", &e.sha[..7]), e.subject)?;
            }
            writeln!(out, "\nFiles")?;
            for f in &d.files {
                let counts = if f.binary { "binary".to_string() } else { format!("+{} −{}", f.additions, f.deletions) };
                writeln!(out, "  {} {}  {}", status_letter(f.status), f.path, style.paint("2", &counts))?;
            }
            if !d.notes.is_empty() {
                writeln!(out, "\nNotes")?;
                for n in &d.notes {
                    let at = match n.current_line {
                        Some(l) => format!("{}:{l}", n.path),
                        None => format!("{}:{} (outdated)", n.path, n.line),
                    };
                    let mark = if n.resolved { style.paint("2", "resolved") } else { style.paint("33", "open") };
                    writeln!(out, "  [{}] {} {}  {}", n.id, mark, style.paint("36", &at), n.text)?;
                }
            }
        }
        Response::Merged { sha } => writeln!(out, "Merged as {}", &sha[..7.min(sha.len())])?,
        r => return Err(unexpected(r)),
    }
    Ok(())
}

fn short_fp(fp: &str) -> String {
    fp.as_bytes().chunks(4).take(4).map(|c| String::from_utf8_lossy(c).to_uppercase()).collect::<Vec<_>>().join(" ")
}

/// Paths relative to the repo root, from paths given relative to here.
fn rel_paths(paths: &[String]) -> Result<Vec<String>> {
    let out = std::process::Command::new("git").args(["rev-parse", "--show-prefix"]).output();
    let prefix = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => String::new(),
    };
    Ok(paths.iter().map(|p| format!("{prefix}{}", p.trim_start_matches("./"))).collect())
}

fn done(out: &mut impl Write, r: Response) -> Result<()> {
    match r {
        Response::Done { output } => {
            if !output.is_empty() {
                writeln!(out, "{output}")?;
            }
            Ok(())
        }
        r => Err(unexpected(r)),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_urls() {
        assert_eq!(server_url("ssh://mini/Users/me/.neutrino/repos/Prism.git"), Some(("mini".into(), None, "/Users/me/.neutrino/repos/Prism.git".into())));
        assert_eq!(server_url("ssh://me@atlas:2222/srv/x.git"), Some(("me@atlas".into(), Some("2222".into()), "/srv/x.git".into())));
        assert_eq!(server_url("mini:/Users/me/x.git"), Some(("mini".into(), None, "/Users/me/x.git".into())));
        // GitHub and relative scp paths aren't a Neutrino server.
        assert_eq!(server_url("git@github.com:me/x.git"), None);
        assert_eq!(server_url("https://github.com/me/x.git"), None);
    }

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("pr"), "pr");
        assert_eq!(sh_quote("/Users/me/.neutrino/repos/a.git"), "/Users/me/.neutrino/repos/a.git");
        assert_eq!(sh_quote("Fix it's thing"), r"'Fix it'\''s thing'");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(sh_quote("$(rm -rf ~)"), "'$(rm -rf ~)'");
    }
}
