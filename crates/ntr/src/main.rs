//! ntr: Neutrino from the terminal. Talks to neutrinod over its Unix socket.

use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use neutrino_proto::{
    Area, ChangeStatus, Head, LineKind, LogEntry, LogQuery, RefKind, Repo, Request, Response, WorkingStatus, decode_response, encode_request,
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
    /// Register the repo containing PATH (default: here).
    Add { path: Option<PathBuf> },
    /// Forget a repo. Nothing on disk is touched.
    Rm { repo: String },
    /// Local and remote branches.
    Branches,
    /// History, newest first.
    Log {
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
    /// A commit and the files it changed.
    Show { sha: String },
    /// A file's changes: `diff PATH` for the working tree, `diff SHA PATH` for a commit.
    Diff {
        #[arg(num_args = 1..=2, required = true)]
        args: Vec<String>,
        /// With one PATH: what's staged, instead of what isn't.
        #[arg(long)]
        staged: bool,
        /// Lines of context.
        #[arg(short = 'U', long, default_value_t = 3)]
        context: u32,
    },
    /// Staged, unstaged, untracked and conflicted files.
    Status,
    /// Stage whole files.
    Stage {
        #[arg(required = true)]
        paths: Vec<String>,
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
    /// Commit what's staged.
    Commit {
        #[arg(short = 'm', long)]
        message: Option<String>,
        #[arg(long)]
        amend: bool,
    },
    /// Switch to a branch; a remote one becomes a tracking branch.
    Switch { branch: String },
    /// Create, rename or delete a branch.
    #[command(subcommand)]
    Branch(BranchCmd),
    /// Print change events as they happen, until Ctrl-C.
    Watch,
}

#[derive(Subcommand)]
enum BranchCmd {
    New {
        name: String,
        /// Where it starts. Default HEAD.
        #[arg(long)]
        start: Option<String>,
        /// Switch to it too.
        #[arg(long, short = 's')]
        switch: bool,
    },
    Rename { from: String, to: String },
    Delete {
        name: String,
        /// Delete it even if it isn't merged.
        #[arg(long)]
        force: bool,
    },
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

/// `--repo`, `$NEUTRINO_REPO`, or the registered repo containing the current folder.
fn pick_repo(c: &mut Conn, arg: Option<String>) -> Result<String> {
    if let Some(r) = arg.or_else(|| std::env::var("NEUTRINO_REPO").ok()) {
        return Ok(r);
    }
    let here = std::env::current_dir()?.canonicalize()?;
    repos(c)?
        .into_iter()
        .filter(|r| here.starts_with(&r.path))
        .max_by_key(|r| r.path.len())
        .map(|r| r.id)
        .ok_or_else(|| anyhow!("this folder isn't in a registered repo: run `ntr add`, or pass --repo"))
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
    let st = Style { on: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() };
    let style = Style { on: st.on };
    let mut c = Conn::open(&socket_path(cli.socket))?;
    let out = &mut std::io::stdout().lock();

    match cli.command {
        Cmd::Ping => match c.call(Request::Ping)? {
            Response::Pong { protocol } => writeln!(out, "neutrinod is up (protocol {protocol})")?,
            r => return Err(unexpected(r)),
        },
        Cmd::Info => match c.call(Request::EngineInfo)? {
            Response::Engine(e) => writeln!(out, "neutrinod {} · {} · {} repos · protocol {}", e.version, e.role, e.repos, e.protocol)?,
            r => return Err(unexpected(r)),
        },
        Cmd::Repos => {
            let list = repos(&mut c)?;
            if list.is_empty() {
                writeln!(out, "No repos yet. Run `ntr add` inside one.")?;
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
        Cmd::Add { path } => {
            let path = path.unwrap_or_else(|| PathBuf::from("."));
            let abs = std::fs::canonicalize(&path).with_context(|| format!("{} doesn't exist", path.display()))?;
            match c.call(Request::AddRepo { path: abs.display().to_string() })? {
                Response::Repo(r) => writeln!(out, "Added {} as {}", r.path, st.paint("1", &r.id))?,
                r => return Err(unexpected(r)),
            }
        }
        Cmd::Rm { repo } => match c.call(Request::RemoveRepo { repo })? {
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
        Cmd::Log { rev, limit, skip, all, query } => {
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
        Cmd::Show { sha } => {
            let repo = pick_repo(&mut c, cli.repo)?;
            let Response::Commit(d) = c.call(Request::Commit { repo, sha })? else { bail!("unexpected answer") };
            writeln!(out, "{} {}", st.paint("33", &format!("commit {}", d.sha)), refs(&LogEntry {
                sha: d.sha.clone(), parents: vec![], subject: String::new(), author: d.author.clone(), time: 0, refs: d.refs.clone(), graph: None,
            }, &st))?;
            if d.parents.len() > 1 {
                writeln!(out, "Merge:  {}  (files compared with the first parent)", d.parents.iter().map(|p| &p[..7]).collect::<Vec<_>>().join(" "))?;
            }
            writeln!(out, "Author: {} <{}>, {}", d.author.name, d.author.email, ago(d.author.time))?;
            writeln!(out, "\n    {}", d.subject)?;
            if !d.body.is_empty() {
                writeln!(out)?;
                for l in d.body.lines() {
                    writeln!(out, "    {l}")?;
                }
            }
            writeln!(out)?;
            let (mut add, mut del) = (0, 0);
            for f in &d.files {
                add += f.additions;
                del += f.deletions;
                let path = match &f.old_path {
                    Some(old) => format!("{old} → {}", f.path),
                    None => f.path.clone(),
                };
                let counts = if f.binary {
                    "binary".to_string()
                } else {
                    format!("{} {}", st.paint("32", &format!("+{}", f.additions)), st.paint("31", &format!("−{}", f.deletions)))
                };
                writeln!(out, " {} {path}  {counts}", status_letter(f.status))?;
            }
            writeln!(out, "\n {} files, +{add} −{del}{}", d.files.len(), if d.truncated { " (list truncated)" } else { "" })?;
        }
        Cmd::Diff { args, staged, context } => {
            let repo = pick_repo(&mut c, cli.repo)?;
            let request = match args.as_slice() {
                [path] => Request::WorkDiff { repo, path: path.clone(), area: if staged { Area::Staged } else { Area::Unstaged }, context: Some(context) },
                [sha, path] => Request::FileDiff { repo, sha: sha.clone(), path: path.clone(), context: Some(context) },
                _ => bail!("give a path, or a commit and a path"),
            };
            let Response::FileDiff(d) = c.call(request)? else { bail!("unexpected answer") };
            let old = d.old_path.as_deref().unwrap_or(&d.path);
            writeln!(out, "{}", st.paint("1", &format!("--- a/{old}\n+++ b/{}", d.path)))?;
            if d.binary {
                writeln!(out, "Binary files differ")?;
            } else if d.too_large {
                writeln!(out, "Too large to diff")?;
            }
            for h in &d.hunks {
                writeln!(out, "{}", st.paint("36", &format!("@@ -{},{} +{},{} @@", h.old_start, h.old_lines, h.new_start, h.new_lines)))?;
                for l in &h.lines {
                    let (sign, code) = match l.kind {
                        LineKind::Context => (' ', ""),
                        LineKind::Added => ('+', "32"),
                        LineKind::Removed => ('-', "31"),
                    };
                    let text = if st.on && !l.segments.is_empty() {
                        l.segments.iter().map(|s| if s.changed { st.paint(&format!("{code};7"), &s.text) } else { st.paint(code, &s.text) }).collect()
                    } else if code.is_empty() {
                        l.text.clone()
                    } else {
                        st.paint(code, &l.text)
                    };
                    writeln!(out, "{}{text}", if code.is_empty() { sign.to_string() } else { st.paint(code, &sign.to_string()) })?;
                    if l.no_newline {
                        writeln!(out, "\\ No newline at end of file")?;
                    }
                }
            }
        }
        Cmd::Status => {
            let repo = pick_repo(&mut c, cli.repo)?;
            let Response::Status(st) = c.call(Request::Status { repo })? else { bail!("unexpected answer") };
            print_status(out, &st, &style)?;
        }
        Cmd::Stage { paths } => {
            let repo = pick_repo(&mut c, cli.repo)?;
            done(out, c.call(Request::Stage { repo, paths: rel_paths(&paths)? })?)?
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
        Cmd::Commit { message, amend } => {
            let repo = pick_repo(&mut c, cli.repo)?;
            match c.call(Request::CommitChanges { repo, message: message.unwrap_or_default(), amend })? {
                Response::Committed { sha, output } => {
                    if !output.is_empty() {
                        writeln!(out, "{output}")?;
                    }
                    writeln!(out, "Committed {}", style.paint("33", &sha[..7]))?;
                }
                r => return Err(unexpected(r)),
            }
        }
        Cmd::Switch { branch } => {
            let repo = pick_repo(&mut c, cli.repo)?;
            done(out, c.call(Request::Checkout { repo, branch })?)?
        }
        Cmd::Branch(b) => {
            let repo = pick_repo(&mut c, cli.repo)?;
            let request = match b {
                BranchCmd::New { name, start, switch } => Request::CreateBranch { repo, name, start, checkout: switch },
                BranchCmd::Rename { from, to } => Request::RenameBranch { repo, from, to },
                BranchCmd::Delete { name, force } => Request::DeleteBranch { repo, name, force },
            };
            done(out, c.call(request)?)?;
        }
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

fn print_status(out: &mut impl Write, st: &WorkingStatus, style: &Style) -> Result<()> {
    match &st.head {
        Some(Head::Branch { name, .. }) => write!(out, "On {}", style.paint("36", name))?,
        Some(Head::Detached { sha }) => write!(out, "Detached at {}", &sha[..7.min(sha.len())])?,
        None => write!(out, "No HEAD")?,
    }
    match (&st.upstream, st.ahead, st.behind) {
        (Some(u), Some(a), Some(b)) => writeln!(out, ", ↑{a} ↓{b} against {u}")?,
        (Some(u), _, _) => writeln!(out, ", tracking {u} (gone)")?,
        _ => writeln!(out)?,
    }
    if let Some(op) = &st.operation {
        writeln!(out, "{}", style.paint("33", &format!("A {op} is in progress.")))?;
    }
    let section = |out: &mut dyn Write, title: &str, code: &str, list: &[neutrino_proto::StatusFile]| -> std::io::Result<()> {
        if list.is_empty() {
            return Ok(());
        }
        writeln!(out, "\n{title}")?;
        for f in list {
            let path = match &f.old_path {
                Some(old) => format!("{old} → {}", f.path),
                None => f.path.clone(),
            };
            let counts = if f.binary { "binary".to_string() } else { format!("+{} −{}", f.additions, f.deletions) };
            writeln!(out, "  {} {}  {}", style.paint(code, status_letter(f.status)), style.paint(code, &path), style.paint("2", &counts))?;
        }
        Ok(())
    };
    section(out, "Conflicted", "31", &st.conflicted)?;
    section(out, "Staged", "32", &st.staged)?;
    section(out, "Not staged", "33", &st.unstaged)?;
    section(out, "Untracked", "2", &st.untracked)?;
    if st.conflicted.is_empty() && st.staged.is_empty() && st.unstaged.is_empty() && st.untracked.is_empty() {
        writeln!(out, "Nothing to commit.")?;
    }
    Ok(())
}
