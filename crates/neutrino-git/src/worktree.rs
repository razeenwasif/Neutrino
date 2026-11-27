//! The working copy: status, its diffs, and the writes that change it.
//!
//! Reads go through gitoxide. Writes go through the `git` binary, as the
//! architecture allows, so hooks, signing, your identity and git's own safety
//! checks (a checkout that would lose changes, deleting an unmerged branch)
//! behave exactly as they do in a terminal.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gix::bstr::ByteSlice;
use neutrino_diff::Apply;
use neutrino_proto::{Area, ChangeStatus, FileDiff, LinePick, StatusFile, WorkingStatus};

use crate::{Ctx, Error, Result, fail};

/// Files whose line counts are worked out for the status lists, at most.
const MAX_STATUS_STATS: usize = 400;

// ---------------------------------------------------------------------------
// Running git
// ---------------------------------------------------------------------------

/// `$NEUTRINO_GIT`, else the first git found in the usual places. A launch
/// agent's PATH is short, so Homebrew's git isn't found through it.
pub fn git_binary() -> PathBuf {
    if let Some(p) = std::env::var_os("NEUTRINO_GIT") {
        return PathBuf::from(p);
    }
    for p in ["/opt/homebrew/bin/git", "/usr/local/bin/git", "/usr/bin/git"] {
        if Path::new(p).exists() {
            return PathBuf::from(p);
        }
    }
    PathBuf::from("git")
}

/// Run git in `root`. Returns what it printed; fails with what it printed when it fails.
fn git(root: &Path, args: &[&str], stdin: Option<&[u8]>) -> Result<String> {
    let mut child = Command::new(git_binary())
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ctx("running git")?;
    if let Some(input) = stdin {
        child.stdin.take().expect("piped").write_all(input).ctx("writing to git")?;
    }
    let out = child.wait_with_output().ctx("running git")?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let text = text.trim().to_string();
    if out.status.success() {
        Ok(text)
    } else if text.is_empty() {
        fail(format!("git {} failed", args.first().unwrap_or(&"")))
    } else {
        Err(Error(text))
    }
}

fn workdir(repo: &gix::Repository) -> Result<&Path> {
    repo.workdir().ok_or_else(|| Error("a bare repository has no working copy".into()))
}

/// Paths must stay inside the working copy.
fn check_path(path: &str) -> Result<()> {
    let p = Path::new(path);
    if path.is_empty() || p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) || path.starts_with(".git/") || path == ".git" {
        return fail(format!("{path:?} isn't a path inside the working copy"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The sides of a file
// ---------------------------------------------------------------------------

/// The file's blob id in HEAD, if it's there.
fn head_blob(repo: &gix::Repository, path: &str) -> Result<Option<gix::ObjectId>> {
    let Ok(tree) = repo.head_tree() else { return Ok(None) };
    let entry = tree.lookup_entry_by_path(path).ctx("reading HEAD's tree")?;
    Ok(entry.filter(|e| !e.mode().is_tree()).map(|e| e.object_id()))
}

/// The file's blob id in the index (stage 0), if it's there.
fn index_blob(repo: &gix::Repository, path: &str) -> Result<Option<gix::ObjectId>> {
    let index = repo.index_or_empty().ctx("reading the index")?;
    Ok(index.entry_by_path(path.as_bytes().as_bstr()).map(|e| e.id))
}

fn blob_bytes(repo: &gix::Repository, id: Option<gix::ObjectId>) -> Result<Option<Vec<u8>>> {
    match id {
        None => Ok(None),
        Some(id) => {
            let header = repo.find_header(id).ctx("reading blob header")?;
            if header.size() as usize > neutrino_diff::MAX_DIFF_BYTES {
                return Ok(None);
            }
            Ok(Some(repo.find_blob(id).ctx("reading blob")?.take_data()))
        }
    }
}

/// The file in the working tree: its contents, or a symlink's target.
fn worktree_bytes(root: &Path, path: &str) -> Result<Option<Vec<u8>>> {
    let full = root.join(path);
    let meta = match std::fs::symlink_metadata(&full) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error(format!("reading {path}: {e}"))),
    };
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(&full).ctx(format!("reading {path}"))?;
        return Ok(Some(target.as_os_str().as_encoded_bytes().to_vec()));
    }
    if meta.is_dir() {
        // A submodule or nested repo; there's no text to show.
        return Ok(Some(Vec::new()));
    }
    if meta.len() as usize > neutrino_diff::MAX_DIFF_BYTES {
        return Ok(None);
    }
    Ok(Some(std::fs::read(&full).ctx(format!("reading {path}"))?))
}

struct Sides {
    old: Option<Vec<u8>>,
    new: Option<Vec<u8>>,
    /// A side exists but was too big to read.
    too_large: bool,
    status: ChangeStatus,
}

fn sides(repo: &gix::Repository, path: &str, area: Area) -> Result<Sides> {
    let root = workdir(repo)?;
    let (old_id, new): (Option<gix::ObjectId>, Option<Option<Vec<u8>>>) = match area {
        Area::Staged => {
            let new_id = index_blob(repo, path)?;
            (head_blob(repo, path)?, new_id.map(|id| blob_bytes(repo, Some(id))).transpose()?)
        }
        Area::Unstaged => {
            let mut old_id = index_blob(repo, path)?;
            if old_id.is_none() && is_conflicted(repo, path)? {
                // An unresolved file has no stage-0 entry; compare with HEAD's version.
                old_id = head_blob(repo, path)?;
            }
            let exists = std::fs::symlink_metadata(root.join(path)).is_ok();
            (old_id, if exists { Some(worktree_bytes(root, path)?) } else { None })
        }
    };
    let old = match old_id {
        Some(id) => Some(blob_bytes(repo, Some(id))?),
        None => None,
    };
    let too_large = matches!(old, Some(None)) || matches!(new, Some(None));
    let status = match (&old, &new) {
        (None, Some(_)) if area == Area::Unstaged => ChangeStatus::Untracked,
        (None, Some(_)) => ChangeStatus::Added,
        (Some(_), None) => ChangeStatus::Deleted,
        (None, None) => return fail(format!("{path} has no changes here")),
        _ => ChangeStatus::Modified,
    };
    Ok(Sides { old: old.flatten(), new: new.flatten(), too_large, status })
}

fn is_conflicted(repo: &gix::Repository, path: &str) -> Result<bool> {
    let index = repo.index_or_empty().ctx("reading the index")?;
    let p = path.as_bytes().as_bstr();
    Ok((1..=3).any(|stage| {
        let stage = match stage {
            1 => gix::index::entry::Stage::Base,
            2 => gix::index::entry::Stage::Ours,
            _ => gix::index::entry::Stage::Theirs,
        };
        index.entry_by_path_and_stage(p, stage).is_some()
    }))
}

/// One file's working-copy diff.
pub fn work_diff(repo: &gix::Repository, path: &str, area: Area, context: u32) -> Result<FileDiff> {
    check_path(path)?;
    let s = sides(repo, path, area)?;
    let status = if is_conflicted(repo, path)? { ChangeStatus::Conflicted } else { s.status };
    let (old, new) = (s.old.unwrap_or_default(), s.new.unwrap_or_default());
    let mut out = FileDiff { path: path.to_string(), old_path: None, status, binary: false, too_large: s.too_large, additions: 0, deletions: 0, hunks: Vec::new() };
    if s.too_large {
        return Ok(out);
    }
    if neutrino_diff::is_binary(&old) || neutrino_diff::is_binary(&new) {
        out.binary = true;
        return Ok(out);
    }
    let d = neutrino_diff::diff(&old, &new, context);
    out.additions = d.additions;
    out.deletions = d.deletions;
    out.hunks = d.hunks;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

fn operation(repo: &gix::Repository) -> Option<String> {
    let dir = repo.git_dir();
    let has = |name: &str| dir.join(name).exists();
    if has("rebase-merge") || has("rebase-apply") {
        Some("rebase".into())
    } else if has("MERGE_HEAD") {
        Some("merge".into())
    } else if has("CHERRY_PICK_HEAD") {
        Some("cherry-pick".into())
    } else if has("REVERT_HEAD") {
        Some("revert".into())
    } else if has("BISECT_LOG") {
        Some("bisect".into())
    } else {
        None
    }
}

fn file(path: String, old_path: Option<String>, status: ChangeStatus) -> StatusFile {
    StatusFile { path, old_path, status, additions: 0, deletions: 0, binary: false }
}

pub fn status(repo: &gix::Repository) -> Result<WorkingStatus> {
    workdir(repo)?;
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();
    let mut conflicted: Vec<StatusFile> = Vec::new();

    let iter = repo
        .status(gix::progress::Discard)
        .ctx("reading the status")?
        .untracked_files(gix::status::UntrackedFiles::Files)
        .index_worktree_rewrites(None)
        .into_iter(Vec::<gix::bstr::BString>::new())
        .ctx("reading the status")?;

    use gix::status::Item;
    use gix::status::index_worktree::Item as Wt;
    use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};
    for item in iter {
        let item = item.ctx("reading the status")?;
        match item {
            Item::TreeIndex(change) => {
                use gix::diff::index::ChangeRef as C;
                let f = match change {
                    C::Addition { location, .. } => file(location.to_string(), None, ChangeStatus::Added),
                    C::Deletion { location, .. } => file(location.to_string(), None, ChangeStatus::Deleted),
                    C::Modification { location, previous_entry_mode, entry_mode, .. } => {
                        let s = if previous_entry_mode != entry_mode && (previous_entry_mode.contains(gix::index::entry::Mode::SYMLINK) || entry_mode.contains(gix::index::entry::Mode::SYMLINK)) {
                            ChangeStatus::TypeChanged
                        } else {
                            ChangeStatus::Modified
                        };
                        file(location.to_string(), None, s)
                    }
                    C::Rewrite { source_location, location, copy, .. } => file(
                        location.to_string(),
                        Some(source_location.to_string()),
                        if copy { ChangeStatus::Copied } else { ChangeStatus::Renamed },
                    ),
                };
                staged.push(f);
            }
            Item::IndexWorktree(Wt::Modification { rela_path, status, .. }) => {
                let path = rela_path.to_string();
                match status {
                    EntryStatus::Conflict { .. } => {
                        if !conflicted.iter().any(|c| c.path == path) {
                            conflicted.push(file(path, None, ChangeStatus::Conflicted));
                        }
                    }
                    EntryStatus::Change(Change::Removed) => unstaged.push(file(path, None, ChangeStatus::Deleted)),
                    EntryStatus::Change(Change::Type { .. }) => unstaged.push(file(path, None, ChangeStatus::TypeChanged)),
                    EntryStatus::Change(_) => unstaged.push(file(path, None, ChangeStatus::Modified)),
                    EntryStatus::IntentToAdd => unstaged.push(file(path, None, ChangeStatus::Added)),
                    EntryStatus::NeedsUpdate(_) => {}
                }
            }
            Item::IndexWorktree(Wt::DirectoryContents { entry, .. }) => {
                if entry.status == gix::dir::entry::Status::Untracked {
                    untracked.push(file(entry.rela_path.to_string(), None, ChangeStatus::Untracked));
                }
            }
            Item::IndexWorktree(Wt::Rewrite { .. }) => {}
        }
    }

    // An unresolved file shows only as conflicted.
    let in_conflict: HashSet<String> = conflicted.iter().map(|c| c.path.clone()).collect();
    staged.retain(|f| !in_conflict.contains(&f.path));
    unstaged.retain(|f| !in_conflict.contains(&f.path));
    for list in [&mut staged, &mut unstaged, &mut untracked, &mut conflicted] {
        list.sort_by(|a, b| a.path.cmp(&b.path));
    }

    // Line counts, for the lists' +/- columns.
    let mut budget = MAX_STATUS_STATS;
    for (list, area) in [(&mut staged, Area::Staged), (&mut unstaged, Area::Unstaged), (&mut untracked, Area::Unstaged)] {
        for f in list.iter_mut() {
            if budget == 0 {
                break;
            }
            budget -= 1;
            if let Ok(s) = sides(repo, &f.path, area) {
                let (old, new) = (s.old.unwrap_or_default(), s.new.unwrap_or_default());
                if s.too_large {
                    continue;
                }
                if neutrino_diff::is_binary(&old) || neutrino_diff::is_binary(&new) {
                    f.binary = true;
                } else {
                    (f.additions, f.deletions) = neutrino_diff::count(&old, &new);
                }
            }
        }
    }

    let head = crate::head(repo).ok();
    let (mut upstream, mut ahead, mut behind) = (None, None, None);
    if let Ok(Some(name)) = repo.head_name() {
        (upstream, ahead, behind) = crate::upstream_of(repo, name.as_ref())?;
    }
    Ok(WorkingStatus { head, upstream, ahead, behind, operation: operation(repo), staged, unstaged, untracked, conflicted })
}

// ---------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------

pub fn stage(repo: &gix::Repository, paths: &[String]) -> Result<String> {
    let root = workdir(repo)?;
    if paths.is_empty() {
        return fail("nothing to stage");
    }
    let mut args = vec!["add", "-A", "--"];
    for p in paths {
        check_path(p)?;
        args.push(p);
    }
    git(root, &args, None)
}

pub fn unstage(repo: &gix::Repository, paths: &[String]) -> Result<String> {
    let root = workdir(repo)?;
    if paths.is_empty() {
        return fail("nothing to unstage");
    }
    // A staged rename comes out of the index from both of its paths.
    let renames: Vec<(String, String)> =
        status(repo)?.staged.into_iter().filter_map(|f| f.old_path.map(|old| (f.path, old))).collect();
    let mut all: Vec<String> = Vec::new();
    for p in paths {
        check_path(p)?;
        all.push(p.clone());
        if let Some((_, old)) = renames.iter().find(|(new, _)| new == p) {
            all.push(old.clone());
        }
    }
    let unborn = repo.head_id().is_err();
    let mut args: Vec<&str> = if unborn { vec!["rm", "--cached", "-q", "-r", "--"] } else { vec!["restore", "--staged", "--"] };
    args.extend(all.iter().map(String::as_str));
    git(root, &args, None)
}

fn picks(lines: &[LinePick]) -> Result<HashSet<LinePick>> {
    if lines.is_empty() {
        return fail("pick at least one line");
    }
    Ok(lines.iter().copied().collect())
}

/// Every picked line must still be in the diff; otherwise the file changed under the client.
fn check_picks(diff: &FileDiff, picked: &HashSet<LinePick>) -> Result<()> {
    let present: HashSet<LinePick> = diff
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter_map(|l| match l.kind {
            neutrino_proto::LineKind::Added => l.new_line.map(|n| LinePick { kind: l.kind, line: n }),
            neutrino_proto::LineKind::Removed => l.old_line.map(|n| LinePick { kind: l.kind, line: n }),
            neutrino_proto::LineKind::Context => None,
        })
        .collect();
    if picked.is_subset(&present) {
        Ok(())
    } else {
        fail("the file changed since its diff was shown; reload and pick the lines again")
    }
}

fn line_editable(diff: &FileDiff) -> Result<()> {
    if diff.binary {
        return fail("a binary file can only be staged or discarded whole");
    }
    if diff.too_large {
        return fail("this file is too large to stage by line");
    }
    Ok(())
}

/// Stage picked lines from the working tree, or unstage picked lines from the index.
pub fn stage_lines(repo: &gix::Repository, path: &str, area: Area, lines: &[LinePick]) -> Result<String> {
    let root = workdir(repo)?;
    let picked = picks(lines)?;
    let diff = work_diff(repo, path, area, 3)?;
    line_editable(&diff)?;
    check_picks(&diff, &picked)?;
    match area {
        Area::Unstaged => {
            let new_file = diff.status == ChangeStatus::Untracked;
            let patch = neutrino_diff::partial_patch(path, &diff.hunks, &picked, Apply::Forward, new_file).ok_or_else(|| Error("nothing picked".into()))?;
            git(root, &["apply", "--cached", "--recount", "--whitespace=nowarn", "-"], Some(patch.as_bytes()))
        }
        Area::Staged => {
            let patch = neutrino_diff::partial_patch(path, &diff.hunks, &picked, Apply::Reverse, false).ok_or_else(|| Error("nothing picked".into()))?;
            git(root, &["apply", "--cached", "--reverse", "--recount", "--whitespace=nowarn", "-"], Some(patch.as_bytes()))
        }
    }
}

/// Keep the working-tree file as an object in the repo and note it in `log`,
/// so a discard can be undone with `git cat-file -p <sha>`.
fn keep_copy(root: &Path, path: &str, log: &Path) -> Result<()> {
    if std::fs::symlink_metadata(root.join(path)).map(|m| m.is_dir()).unwrap_or(true) {
        return Ok(());
    }
    let sha = git(root, &["hash-object", "-w", "--", path], None)?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let line = format!("{now}\t{}\t{path}\t{sha}\n", root.display());
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).ctx("creating the state folder")?;
    }
    std::fs::OpenOptions::new().create(true).append(true).open(log).and_then(|mut f| f.write_all(line.as_bytes())).ctx("writing discarded.log")
}

/// Move a file to the user's Trash, or delete it when that isn't possible.
fn trash(path: &Path) -> Result<()> {
    if let Some(home) = std::env::var_os("HOME") {
        let trash = Path::new(&home).join(".Trash");
        if trash.is_dir() {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
            let mut dest = trash.join(&name);
            let mut n = 2;
            while dest.exists() {
                dest = trash.join(format!("{name} {n}"));
                n += 1;
            }
            if std::fs::rename(path, &dest).is_ok() {
                return Ok(());
            }
        }
    }
    std::fs::remove_file(path).ctx(format!("removing {}", path.display()))
}

/// Throw away working-tree changes to whole files: tracked files go back to
/// their staged version, untracked ones go to the Trash.
pub fn discard(repo: &gix::Repository, paths: &[String], log: &Path) -> Result<String> {
    let root = workdir(repo)?.to_path_buf();
    if paths.is_empty() {
        return fail("nothing to discard");
    }
    let st = status(repo)?;
    let untracked: HashSet<&str> = st.untracked.iter().map(|f| f.path.as_str()).collect();
    let mut tracked = Vec::new();
    for p in paths {
        check_path(p)?;
        keep_copy(&root, p, log)?;
        if untracked.contains(p.as_str()) {
            trash(&root.join(p))?;
        } else {
            tracked.push(p.as_str());
        }
    }
    if tracked.is_empty() {
        return Ok(String::new());
    }
    let mut args = vec!["restore", "--worktree", "--"];
    args.extend(tracked);
    git(&root, &args, None)
}

pub fn discard_lines(repo: &gix::Repository, path: &str, lines: &[LinePick], log: &Path) -> Result<String> {
    let root = workdir(repo)?.to_path_buf();
    let picked = picks(lines)?;
    let diff = work_diff(repo, path, Area::Unstaged, 3)?;
    line_editable(&diff)?;
    if diff.status == ChangeStatus::Untracked {
        return fail("discard an untracked file whole");
    }
    check_picks(&diff, &picked)?;
    let patch = neutrino_diff::partial_patch(path, &diff.hunks, &picked, Apply::Reverse, false).ok_or_else(|| Error("nothing picked".into()))?;
    keep_copy(&root, path, log)?;
    git(&root, &["apply", "--reverse", "--recount", "--whitespace=nowarn", "-"], Some(patch.as_bytes()))
}

/// Commit what's staged; returns the new commit and what git (and its hooks) printed.
pub fn commit(repo: &gix::Repository, message: &str, amend: bool) -> Result<(String, String)> {
    let root = workdir(repo)?;
    if message.trim().is_empty() && !amend {
        return fail("write a commit message first");
    }
    let mut args = vec!["commit", "-q"];
    if amend {
        args.push("--amend");
    }
    if message.trim().is_empty() {
        args.push("--no-edit");
    } else {
        args.extend(["--cleanup=strip", "-F", "-"]);
    }
    let output = git(root, &args, if message.trim().is_empty() { None } else { Some(message.as_bytes()) })?;
    let sha = git(root, &["rev-parse", "HEAD"], None)?;
    Ok((sha, output))
}

pub fn create_branch(repo: &gix::Repository, name: &str, start: Option<&str>, checkout: bool) -> Result<String> {
    let root = workdir(repo)?;
    let mut args = if checkout { vec!["switch", "-c", name] } else { vec!["branch", "--", name] };
    if let Some(s) = start {
        args.push(s);
    }
    git(root, &args, None)
}

/// Switch to a local branch; a remote one (`origin/x`) becomes a new tracking branch.
pub fn checkout(repo: &gix::Repository, branch: &str) -> Result<String> {
    let root = workdir(repo)?;
    let local = repo.try_find_reference(format!("refs/heads/{branch}").as_str()).ok().flatten().is_some();
    let remote = repo.try_find_reference(format!("refs/remotes/{branch}").as_str()).ok().flatten().is_some();
    if !local && remote {
        git(root, &["switch", "--track", branch], None)
    } else {
        git(root, &["switch", branch], None)
    }
}

pub fn rename_branch(repo: &gix::Repository, from: &str, to: &str) -> Result<String> {
    git(workdir(repo)?, &["branch", "-m", "--", from, to], None)
}

pub fn delete_branch(repo: &gix::Repository, name: &str, force: bool) -> Result<String> {
    git(workdir(repo)?, &["branch", if force { "-D" } else { "-d" }, "--", name], None)
}

// ---------------------------------------------------------------------------
// Remotes
// ---------------------------------------------------------------------------

/// Network git never waits on a prompt: no password questions, no host-key
/// questions, so a request can't hang behind a dialog nobody sees.
fn git_net(root: &Path, args: &[&str]) -> Result<String> {
    let mut child = Command::new(git_binary());
    child
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", std::env::var("GIT_SSH_COMMAND").unwrap_or_else(|_| "ssh -o BatchMode=yes -o ConnectTimeout=10 -o ForwardAgent=no -o ClearAllForwardings=yes -o PermitLocalCommand=no -o StrictHostKeyChecking=yes".into()))
        .stdin(Stdio::null());
    let out = child.output().ctx("running git")?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let text = text.trim().to_string();
    if out.status.success() { Ok(text) } else { Err(Error(if text.is_empty() { format!("git {} failed", args[0]) } else { text })) }
}

pub fn remotes(repo: &gix::Repository) -> Result<Vec<neutrino_proto::RemoteInfo>> {
    let mut out = Vec::new();
    for name in repo.remote_names() {
        let Ok(remote) = repo.find_remote(name.as_bstr()) else { continue };
        let url = remote.url(gix::remote::Direction::Fetch).map(|u| u.to_bstring().to_string()).unwrap_or_default();
        out.push(neutrino_proto::RemoteInfo { name: name.to_string(), url });
    }
    Ok(out)
}

fn check_remote_name(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('-') || !name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
        return fail(format!("{name:?} isn't a usable remote name"));
    }
    Ok(())
}

pub fn add_remote(repo: &gix::Repository, name: &str, url: &str) -> Result<String> {
    check_remote_name(name)?;
    if url.trim().is_empty() || url.starts_with('-') {
        return fail("give the remote a URL");
    }
    git(workdir(repo)?, &["remote", "add", "--", name, url], None)
}

pub fn fetch(repo: &gix::Repository, remote: Option<&str>) -> Result<String> {
    let root = workdir(repo)?;
    match remote {
        Some(r) => {
            check_remote_name(r)?;
            git_net(root, &["fetch", "--prune", r])
        }
        None => git_net(root, &["fetch", "--all", "--prune"]),
    }
}

/// Push a branch (default: the checked-out one) to `remote` (default: its
/// upstream's remote, else `origin`).
pub fn push(repo: &gix::Repository, remote: Option<&str>, branch: Option<&str>, set_upstream: bool) -> Result<String> {
    let root = workdir(repo)?;
    let branch = match branch {
        Some(b) => b.to_string(),
        None => match repo.head_name().ctx("reading HEAD")? {
            Some(name) => name.shorten().to_string(),
            None => return fail("HEAD isn't on a branch; switch to one to push it"),
        },
    };
    let remote = match remote {
        Some(r) => r.to_string(),
        None => {
            let configured = repo
                .branch_remote_name(branch.as_str(), gix::remote::Direction::Push)
                .map(|n| n.as_bstr().to_string());
            configured.unwrap_or_else(|| "origin".into())
        }
    };
    check_remote_name(&remote)?;
    let mut args = vec!["push", "--porcelain"];
    if set_upstream {
        args.push("--set-upstream");
    }
    args.extend(["--", remote.as_str(), branch.as_str()]);
    git_net(root, &args)
}
