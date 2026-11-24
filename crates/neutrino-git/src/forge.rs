//! What pull requests need from git: comparing two branches, checking whether
//! they merge, merging them in a bare repository, and following a line of a
//! file from one version to the next.
//!
//! Reads go through gitoxide. Merges go through `git merge-tree` and
//! `git commit-tree`, which work without a working copy, and land with
//! `git update-ref <ref> <new> <old>`, so a push that raced the merge wins and
//! the merge fails rather than overwriting it.

use std::collections::{HashMap, VecDeque};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};

use gix::ObjectId;
use neutrino_proto::{FileChange, FileDiff, LogEntry, MergeStrategy, Mergeability, Side};

use crate::worktree::git_binary;
use crate::{Change, Ctx, DiffCache, Error, Result, fail, root, signature, split_message, tree_changes};

/// Commits listed for a comparison, at most.
pub const MAX_COMMITS: usize = 250;

/// Who a merge commit is by, when the server has no identity of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Run git, returning its exit code rather than failing on a non-zero one.
fn run(dir: &Path, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> Result<Output> {
    let mut cmd = Command::new(git_binary());
    cmd.arg("-C").arg(dir).args(args).env("GIT_TERMINAL_PROMPT", "0");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ctx("running git")?;
    if let Some(input) = stdin {
        child.stdin.take().expect("piped").write_all(input).ctx("writing to git")?;
    }
    let out = child.wait_with_output().ctx("running git")?;
    Ok(Output {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    })
}

/// Run git and fail with what it said when it fails.
fn ok(dir: &Path, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> Result<String> {
    let out = run(dir, args, env, stdin)?;
    if out.code == 0 {
        Ok(out.stdout)
    } else if out.stderr.is_empty() {
        fail(format!("git {} failed", args.first().unwrap_or(&"")))
    } else {
        Err(Error(out.stderr))
    }
}

fn check_branch(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('-') || name.contains("..") || name.contains(char::is_whitespace) {
        return fail(format!("{name:?} is not a branch name"));
    }
    Ok(())
}

/// The commit a local branch points at.
pub fn branch_tip(repo: &gix::Repository, branch: &str) -> Result<ObjectId> {
    check_branch(branch)?;
    let mut r = repo.find_reference(format!("refs/heads/{branch}").as_str()).ctx(format!("there's no branch {branch}"))?;
    Ok(r.peel_to_id().ctx(format!("reading {branch}"))?.detach())
}

fn object(repo: &gix::Repository, rev: &str) -> Result<ObjectId> {
    crate::resolve(repo, rev)
}

/// The best common ancestor of two commits, if they have one.
pub fn merge_base(repo: &gix::Repository, a: ObjectId, b: ObjectId) -> Result<Option<ObjectId>> {
    let out = run(root(repo), &["merge-base", &a.to_string(), &b.to_string()], &[], None)?;
    match out.code {
        0 => Ok(Some(ObjectId::from_hex(out.stdout.as_bytes()).ctx("reading merge-base")?)),
        1 => Ok(None),
        _ => Err(Error(out.stderr)),
    }
}

fn is_ancestor(repo: &gix::Repository, ancestor: ObjectId, of: ObjectId) -> Result<bool> {
    let out = run(root(repo), &["merge-base", "--is-ancestor", &ancestor.to_string(), &of.to_string()], &[], None)?;
    match out.code {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(Error(out.stderr)),
    }
}

/// What `head` changes since it left `base`: the diff from their merge base,
/// so commits that landed on `base` meanwhile don't show up.
fn compared(repo: &gix::Repository, base: &str, head: &str) -> Result<Vec<Change>> {
    let (base, head) = (object(repo, base)?, object(repo, head)?);
    let from = merge_base(repo, base, head)?.unwrap_or(base);
    let old = repo.find_commit(from).ctx("reading commit")?.tree().ctx("reading tree")?;
    let new = repo.find_commit(head).ctx("reading commit")?.tree().ctx("reading tree")?;
    tree_changes(repo, Some(&old), &new)
}

pub fn compare(repo: &gix::Repository, base: &str, head: &str) -> Result<Vec<FileChange>> {
    Ok(crate::file_list(repo, compared(repo, base, head)?).0)
}

pub fn compare_diff(repo: &gix::Repository, base: &str, head: &str, path: &str, context: u32, cache: &DiffCache) -> Result<FileDiff> {
    let Some(c) = compared(repo, base, head)?.into_iter().find(|c| c.path == path) else {
        return fail(format!("{path} isn't changed between {base} and {head}"));
    };
    crate::change_diff(repo, c, context, cache)
}

/// Commits on `head` that aren't on `base`, oldest first, as a pull request lists them.
pub fn commits_between(repo: &gix::Repository, base: &str, head: &str) -> Result<Vec<LogEntry>> {
    let (base, head) = (object(repo, base)?, object(repo, head)?);
    let walk = repo
        .rev_walk([head])
        .with_hidden([base])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(Default::default()))
        .all()
        .ctx("walking history")?;
    let mut out = Vec::new();
    for info in walk.take(MAX_COMMITS) {
        let info = info.ctx("walking history")?;
        let commit = info.object().ctx("reading commit")?;
        let (subject, _) = split_message(commit.message_raw_sloppy());
        out.push(LogEntry {
            sha: info.id.to_string(),
            parents: info.parent_ids().map(|p| p.to_string()).collect(),
            subject,
            author: signature(commit.author().ctx("reading author")?),
            time: commit.time().map(|t| t.seconds).unwrap_or(0),
            refs: Vec::new(),
            graph: None,
        });
    }
    out.reverse();
    Ok(out)
}

/// `git merge-tree` of two commits: the merged tree, or the conflicting files.
fn merge_trees(repo: &gix::Repository, base: Option<ObjectId>, ours: ObjectId, theirs: ObjectId) -> Result<std::result::Result<String, Vec<String>>> {
    let (ours, theirs) = (ours.to_string(), theirs.to_string());
    let base = base.map(|b| b.to_string());
    let mut args = vec!["merge-tree", "--write-tree", "--name-only", "--no-messages"];
    if let Some(b) = &base {
        args.push("--merge-base");
        args.push(b);
    }
    args.push(&ours);
    args.push(&theirs);
    let out = run(root(repo), &args, &[], None)?;
    let mut lines = out.stdout.lines();
    let tree = lines.next().unwrap_or_default().to_string();
    match out.code {
        0 => Ok(Ok(tree)),
        1 => {
            let mut files: Vec<String> = lines.map(str::to_string).filter(|l| !l.is_empty()).collect();
            files.dedup();
            Ok(Err(files))
        }
        _ => Err(Error(if out.stderr.is_empty() { "git merge-tree failed".into() } else { out.stderr })),
    }
}

/// Mergeability already computed, keyed by `(git_dir, target_sha, source_sha)`.
/// Commit OIDs are immutable, so entries never go stale; caching avoids
/// repeated `git merge-tree --write-tree` subprocesses and loose tree objects.
type MergeKey = (PathBuf, ObjectId, ObjectId);
const MERGE_CACHE_CAP: usize = 256;
static MERGE_CACHE: LazyLock<Mutex<(HashMap<MergeKey, Mergeability>, VecDeque<MergeKey>)>> =
    LazyLock::new(|| Mutex::new((HashMap::new(), VecDeque::new())));

pub fn mergeability(repo: &gix::Repository, target: &str, source: &str) -> Result<Mergeability> {
    let (t, s) = (branch_tip(repo, target)?, branch_tip(repo, source)?);
    let key = (repo.git_dir().to_path_buf(), t, s);
    if let Some(cached) = MERGE_CACHE.lock().unwrap().0.get(&key).cloned() {
        return Ok(cached);
    }
    let fast_forward = is_ancestor(repo, t, s)?;
    let behind = ok(root(repo), &["rev-list", "--count", &format!("{s}..{t}")], &[], None)?.parse().unwrap_or(0);
    let (clean, conflicts) = if fast_forward {
        (true, Vec::new())
    } else {
        match merge_trees(repo, None, t, s)? {
            Ok(_) => (true, Vec::new()),
            Err(files) => (false, files),
        }
    };
    let result = Mergeability { clean, conflicts, fast_forward, behind };
    let mut cache = MERGE_CACHE.lock().unwrap();
    if cache.0.insert(key.clone(), result.clone()).is_none() {
        cache.1.push_back(key);
        while cache.1.len() > MERGE_CACHE_CAP {
            if let Some(old) = cache.1.pop_front() {
                cache.0.remove(&old);
            }
        }
    }
    Ok(result)
}

fn identity_env(prefix: &str, who: &Identity) -> [(String, String); 2] {
    [(format!("GIT_{prefix}_NAME"), who.name.clone()), (format!("GIT_{prefix}_EMAIL"), who.email.clone())]
}

/// The author of a commit, for a server with no identity of its own.
pub fn author_of(repo: &gix::Repository, sha: ObjectId) -> Result<Identity> {
    let commit = repo.find_commit(sha).ctx("reading commit")?;
    let s = signature(commit.author().ctx("reading author")?);
    Ok(Identity { name: s.name, email: s.email })
}

fn commit_tree(repo: &gix::Repository, tree: &str, parents: &[ObjectId], message: &str, author: &Identity, committer: &Identity, author_date: Option<&str>) -> Result<ObjectId> {
    let mut args = vec!["commit-tree".to_string(), tree.to_string()];
    for p in parents {
        args.push("-p".into());
        args.push(p.to_string());
    }
    args.push("-F".into());
    args.push("-".into());
    let mut env: Vec<(String, String)> = identity_env("AUTHOR", author).into_iter().chain(identity_env("COMMITTER", committer)).collect();
    if let Some(d) = author_date {
        env.push(("GIT_AUTHOR_DATE".into(), d.into()));
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let message = if message.ends_with('\n') { message.to_string() } else { format!("{message}\n") };
    let sha = ok(root(repo), &args, &env, Some(message.as_bytes()))?;
    ObjectId::from_hex(sha.as_bytes()).ctx("reading the new commit")
}

/// Merge `source` into `target` and move `target`. Returns the new tip of `target`.
///
/// `message` is the merge or squash commit's message; a rebase keeps each
/// commit's own. `committer` is who the server commits as.
pub fn merge(repo: &gix::Repository, target: &str, source: &str, strategy: MergeStrategy, message: &str, committer: &Identity) -> Result<ObjectId> {
    let (t, s) = (branch_tip(repo, target)?, branch_tip(repo, source)?);
    if t == s || is_ancestor(repo, s, t)? {
        return fail(format!("{target} already has everything on {source}"));
    }
    // After a squash or a rebase the source isn't an ancestor, but merging it
    // again would change nothing.
    if let Ok(tree) = merge_trees(repo, None, t, s)? {
        let current = repo.find_commit(t).ctx("reading commit")?.tree_id().ctx("reading tree")?;
        if current.to_string() == tree {
            return fail(format!("{target} already has all of {source}'s changes"));
        }
    }
    let new = match strategy {
        MergeStrategy::FastForward => {
            if !is_ancestor(repo, t, s)? {
                return fail(format!("{target} has commits {source} doesn't, so it can't fast-forward; merge, squash or rebase instead"));
            }
            s
        }
        MergeStrategy::Merge | MergeStrategy::Squash => {
            let tree = match merge_trees(repo, None, t, s)? {
                Ok(tree) => tree,
                Err(files) => return fail(conflict_message(source, target, &files)),
            };
            let author = author_of(repo, s)?;
            let parents: Vec<ObjectId> = if strategy == MergeStrategy::Merge { vec![t, s] } else { vec![t] };
            commit_tree(repo, &tree, &parents, message, &author, committer, None)?
        }
        MergeStrategy::Rebase => rebase(repo, t, s, source, target, committer)?,
    };
    ok(root(repo), &["update-ref", "-m", &format!("neutrino: merge {source}"), &format!("refs/heads/{target}"), &new.to_string(), &t.to_string()], &[], None)
        .map_err(|e| Error(format!("{target} moved while merging; try again ({e})")))?;
    Ok(new)
}

fn conflict_message(source: &str, target: &str, files: &[String]) -> String {
    let shown: Vec<&str> = files.iter().take(5).map(String::as_str).collect();
    let more = if files.len() > 5 { format!(" and {} more", files.len() - 5) } else { String::new() };
    format!("{source} conflicts with {target} in {}{more}; merge {target} into {source} and resolve them, then push", shown.join(", "))
}

/// Replay each commit of `source` since it left `target` onto `target`'s tip,
/// keeping authors, dates and messages: a cherry-pick per commit, done with
/// `merge-tree` so it needs no working copy.
fn rebase(repo: &gix::Repository, t: ObjectId, s: ObjectId, source: &str, target: &str, committer: &Identity) -> Result<ObjectId> {
    let list = ok(root(repo), &["rev-list", "--reverse", "--parents", &format!("{t}..{s}")], &[], None)?;
    let mut tip = t;
    for line in list.lines() {
        let ids: Vec<ObjectId> = line.split_whitespace().filter_map(|h| ObjectId::from_hex(h.as_bytes()).ok()).collect();
        let (commit, parents) = ids.split_first().ok_or_else(|| Error("reading the commits to rebase".into()))?;
        if parents.len() != 1 {
            return fail(format!("{source} has merge commits, which a rebase would flatten; merge or squash instead"));
        }
        let tree = match merge_trees(repo, Some(parents[0]), tip, *commit)? {
            Ok(tree) => tree,
            Err(files) => {
                return fail(format!(
                    "commit {} doesn't apply cleanly onto {target} ({}); merge or squash instead",
                    commit.to_hex_with_len(7),
                    files.join(", ")
                ));
            }
        };
        let c = repo.find_commit(*commit).ctx("reading commit")?;
        let message = c.message_raw_sloppy().to_string();
        let author = c.author().ctx("reading author")?;
        let who = Identity { name: author.name.to_string(), email: author.email.to_string() };
        let date = author.time().ok().map(|t| {
            let (sign, off) = if t.offset < 0 { ('-', -t.offset) } else { ('+', t.offset) };
            format!("{} {sign}{:02}{:02}", t.seconds, off / 3600, off % 3600 / 60)
        });
        tip = commit_tree(repo, &tree, &[tip], &message, &who, committer, date.as_deref())?;
    }
    Ok(tip)
}

/// Point `refs/pull/<number>/head` at a version, so its commits are never collected.
pub fn keep_pull_head(repo: &gix::Repository, number: u32, sha: &str) -> Result<()> {
    ok(root(repo), &["update-ref", &format!("refs/pull/{number}/head"), sha], &[], None).map(|_| ())
}

pub fn delete_branch(repo: &gix::Repository, branch: &str, expected: ObjectId) -> Result<()> {
    check_branch(branch)?;
    ok(root(repo), &["update-ref", "-d", &format!("refs/heads/{branch}"), &expected.to_string()], &[], None).map(|_| ())
}

fn blob_at(repo: &gix::Repository, commit: &str, path: &str) -> Result<Option<Vec<u8>>> {
    let id = object(repo, commit)?;
    let tree = repo.find_commit(id).ctx("reading commit")?.tree().ctx("reading tree")?;
    let Some(entry) = tree.lookup_entry_by_path(path).ctx("finding file")? else { return Ok(None) };
    if !entry.mode().is_blob() {
        return Ok(None);
    }
    Ok(Some(entry.object().ctx("reading file")?.detach().data))
}

/// Where line `line` of `path` at `from` is at `to`, or `None` when that line
/// was changed or removed, or the file is gone.
pub fn map_line(repo: &gix::Repository, from: &str, to: &str, path: &str, line: u32) -> Result<Option<u32>> {
    if from == to {
        return Ok(Some(line));
    }
    let (Some(old), Some(new)) = (blob_at(repo, from, path)?, blob_at(repo, to, path)?) else { return Ok(None) };
    Ok(map_through(&old, &new, line))
}

/// Line `line` of `old`, followed into `new`.
pub fn map_through(old: &[u8], new: &[u8], line: u32) -> Option<u32> {
    if old == new {
        return Some(line);
    }
    if neutrino_diff::is_binary(old) || neutrino_diff::is_binary(new) {
        return None;
    }
    let mut shift: i64 = 0;
    for hunk in neutrino_diff::diff(old, new, 0).hunks {
        let removed: Vec<u32> = hunk.lines.iter().filter(|l| l.new_line.is_none()).filter_map(|l| l.old_line).collect();
        let delta = hunk.new_lines as i64 - hunk.old_lines as i64;
        match (removed.first(), removed.last()) {
            (Some(&first), Some(&last)) => {
                if (first..=last).contains(&line) {
                    return None;
                }
                if line > last {
                    shift += delta;
                }
            }
            // A pure insertion goes after line `old_start`.
            _ => {
                if line > hunk.old_start {
                    shift += delta;
                }
            }
        }
    }
    u32::try_from(line as i64 + shift).ok()
}

/// The commit a side of a version's diff reads from.
pub fn side_commit(side: Side, base: &str, head: &str) -> String {
    match side {
        Side::Old => base.to_string(),
        Side::New => head.to_string(),
    }
}

/// The files directly inside `dir` at commit `rev` whose names end in one of
/// `extensions`, with their contents. Nothing when the folder isn't there.
pub fn files_at(repo: &gix::Repository, rev: &str, dir: &str, extensions: &[&str]) -> Result<Vec<(String, Vec<u8>)>> {
    let id = object(repo, rev)?;
    let tree = repo.find_commit(id).ctx("reading commit")?.tree().ctx("reading tree")?;
    let Some(entry) = tree.lookup_entry_by_path(dir).ctx("finding folder")? else { return Ok(Vec::new()) };
    if !entry.mode().is_tree() {
        return Ok(Vec::new());
    }
    let folder = entry.object().ctx("reading folder")?.into_tree();
    let mut out = Vec::new();
    for e in folder.iter() {
        let e = e.ctx("reading folder")?;
        let name = e.filename().to_string();
        if !e.mode().is_blob() || !extensions.iter().any(|x| name.ends_with(x)) {
            continue;
        }
        let blob = repo.find_object(e.oid()).ctx("reading file")?.detach().data;
        out.push((name, blob));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// A shallow checkout of one commit into `dest`, which must not exist: what a
/// CI job works in. Fetched by id, so pull request heads work too.
pub fn checkout_into(repo: &gix::Repository, sha: &str, dest: &Path) -> Result<()> {
    let source = repo.git_dir().to_path_buf();
    std::fs::create_dir_all(dest).ctx(format!("creating {}", dest.display()))?;
    ok(dest, &["init", "-q"], &[], None)?;
    let url = format!("file://{}", source.display());
    ok(dest, &["fetch", "-q", "--no-tags", "--depth=1", &url, sha], &[], None)?;
    ok(dest, &["-c", "advice.detachedHead=false", "checkout", "-q", "--detach", "FETCH_HEAD"], &[], None)?;
    Ok(())
}
