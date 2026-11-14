//! Neutrino's git engine: reads repositories in-process with gitoxide.
//!
//! Nothing here forks `git`. Every function takes a thread-local
//! [`gix::Repository`]; the daemon keeps one [`gix::ThreadSafeRepository`] per
//! registered repo and hands each request its own thread-local copy.

pub mod graph;
pub mod worktree;

use std::collections::HashMap;
use std::fmt::Display;
use std::path::Path;
use std::sync::Mutex;

use gix::ObjectId;
use gix::bstr::{BStr, ByteSlice};
use neutrino_proto::{
    Branch, ChangeStatus, CommitDetail, FileChange, FileDiff, Head, LogEntry, LogPage, LogQuery, RefKind, RefLabel, Repo, Signature,
};

pub use gix::ThreadSafeRepository;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Attach what we were doing to an error from gitoxide.
trait Ctx<T> {
    fn ctx(self, what: impl Display) -> Result<T>;
}

impl<T, E: Display> Ctx<T> for std::result::Result<T, E> {
    fn ctx(self, what: impl Display) -> Result<T> {
        self.map_err(|e| Error(format!("{what}: {e}")))
    }
}

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(Error(message.into()))
}

/// Commits listed in a commit's file list, at most.
const MAX_FILES: usize = 3_000;
/// Files whose line counts are worked out for the file list, at most.
const MAX_FILES_WITH_STATS: usize = 500;
/// Upper bound when counting how far a branch is ahead of or behind its upstream.
const MAX_AHEAD_BEHIND: usize = 10_000;
pub const DEFAULT_LOG_LIMIT: u32 = 100;
pub const MAX_LOG_LIMIT: u32 = 1_000;

// ---------------------------------------------------------------------------
// Opening
// ---------------------------------------------------------------------------

/// Find the repository containing `path`.
pub fn discover(path: &Path) -> Result<ThreadSafeRepository> {
    let repo = gix::discover(path).ctx(format!("{} is not inside a git repository", path.display()))?;
    Ok(repo.into_sync())
}

/// The working tree for a normal repo, the git dir for a bare one.
pub fn root(repo: &gix::Repository) -> &Path {
    repo.workdir().unwrap_or_else(|| repo.git_dir())
}

pub fn local(repo: &ThreadSafeRepository) -> gix::Repository {
    let mut r = repo.to_thread_local();
    // A history walk by commit time looks most commits up twice.
    r.object_cache_size_if_unset(32 << 20);
    r
}

pub fn summary(id: &str, repo: &gix::Repository) -> Result<Repo> {
    let path = root(repo);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().trim_end_matches(".git").to_string())
        .unwrap_or_else(|| id.to_string());
    Ok(Repo { id: id.to_string(), name, path: path.display().to_string(), bare: repo.is_bare(), head: Some(head(repo)?), error: None })
}

fn head(repo: &gix::Repository) -> Result<Head> {
    let sha = repo.head_id().ok().map(|id| id.to_string());
    match repo.head_name().ctx("reading HEAD")? {
        Some(name) => Ok(Head::Branch { name: name.shorten().to_string(), sha }),
        None => match sha {
            Some(sha) => Ok(Head::Detached { sha }),
            None => fail("HEAD points nowhere"),
        },
    }
}

// ---------------------------------------------------------------------------
// Refs
// ---------------------------------------------------------------------------

/// Every branch and tag, by the commit it points at.
pub fn ref_labels(repo: &gix::Repository) -> Result<HashMap<ObjectId, Vec<RefLabel>>> {
    let head_name = repo.head_name().ctx("reading HEAD")?;
    let mut out: HashMap<ObjectId, Vec<RefLabel>> = HashMap::new();
    let refs = repo.references().ctx("reading refs")?;
    for r in refs.all().ctx("listing refs")?.peeled().ctx("peeling refs")? {
        let Ok(r) = r else { continue };
        let name = r.name();
        let kind = match name.category() {
            Some(gix::reference::Category::LocalBranch) => RefKind::Local,
            Some(gix::reference::Category::RemoteBranch) => RefKind::Remote,
            Some(gix::reference::Category::Tag) => RefKind::Tag,
            _ => continue,
        };
        let short = name.shorten().to_string();
        // `origin/HEAD` only repeats what `origin/main` already says.
        if kind == RefKind::Remote && short.ends_with("/HEAD") {
            continue;
        }
        let is_head = head_name.as_ref().is_some_and(|h| h.as_ref() == name);
        out.entry(r.id().detach()).or_default().push(RefLabel { name: short, kind, is_head });
    }
    if head_name.is_none() {
        if let Ok(id) = repo.head_id() {
            out.entry(id.detach()).or_default().push(RefLabel { name: "HEAD".into(), kind: RefKind::Head, is_head: true });
        }
    }
    for labels in out.values_mut() {
        // HEAD first, then local, remote, tags.
        labels.sort_by_key(|l| (!l.is_head, l.kind as u8, l.name.clone()));
        // A symbolic ref such as `origin/HEAD` peels to the branch it names,
        // which is then listed twice.
        labels.dedup_by(|a, b| a.name == b.name && a.kind == b.kind);
    }
    Ok(out)
}

fn count_walk(repo: &gix::Repository, tip: ObjectId, hidden: ObjectId) -> Result<u32> {
    let walk = repo.rev_walk([tip]).with_hidden([hidden]).all().ctx("walking history")?;
    Ok(walk.take(MAX_AHEAD_BEHIND).filter(|c| c.is_ok()).count() as u32)
}

/// A local branch's upstream, and how far the branch is ahead of and behind it.
pub(crate) fn upstream_of(repo: &gix::Repository, name: &gix::refs::FullNameRef) -> Result<(Option<String>, Option<u32>, Option<u32>)> {
    let Some(Ok(tracking)) = repo.branch_remote_tracking_ref_name(name, gix::remote::Direction::Fetch) else {
        return Ok((None, None, None));
    };
    let upstream = Some(tracking.as_ref().shorten().to_string());
    let Ok(Some(mut up)) = repo.try_find_reference(tracking.as_ref()) else { return Ok((upstream, None, None)) };
    let Ok(up_id) = up.peel_to_id() else { return Ok((upstream, None, None)) };
    let Ok(Some(mut local)) = repo.try_find_reference(name) else { return Ok((upstream, None, None)) };
    let Ok(sha) = local.peel_to_id() else { return Ok((upstream, None, None)) };
    let (up_id, sha) = (up_id.detach(), sha.detach());
    Ok((upstream, Some(count_walk(repo, sha, up_id)?), Some(count_walk(repo, up_id, sha)?)))
}

pub fn branches(repo: &gix::Repository) -> Result<Vec<Branch>> {
    let head_name = repo.head_name().ctx("reading HEAD")?;
    let refs = repo.references().ctx("reading refs")?;
    let mut out = Vec::new();
    for (iter, kind) in [(refs.local_branches(), RefKind::Local), (refs.remote_branches(), RefKind::Remote)] {
        for r in iter.ctx("listing branches")? {
            let Ok(mut r) = r else { continue };
            let short = r.name().shorten().to_string();
            if kind == RefKind::Remote && short.ends_with("/HEAD") {
                continue;
            }
            let Ok(commit) = r.peel_to_commit() else { continue };
            let sha = commit.id().detach();
            let subject = commit.message().map(|m| m.summary().to_string()).unwrap_or_default();
            let time = commit.time().map(|t| t.seconds).unwrap_or(0);
            let is_head = head_name.as_ref().is_some_and(|h| h.as_ref() == r.name());

            let (upstream, ahead, behind) = if kind == RefKind::Local { upstream_of(repo, r.name())? } else { (None, None, None) };
            out.push(Branch { name: short, kind, sha: sha.to_string(), subject, time, is_head, upstream, ahead, behind });
        }
    }
    // The checked-out branch first, then the most recently changed.
    out.sort_by(|a, b| (b.is_head, a.kind as u8, b.time).cmp(&(a.is_head, b.kind as u8, a.time)));
    Ok(out)
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

fn signature(sig: gix::actor::SignatureRef<'_>) -> Signature {
    let time = sig.time().unwrap_or_default();
    Signature { name: sig.name.trim().to_str_lossy().into_owned(), email: sig.email.trim().to_str_lossy().into_owned(), time: time.seconds, offset: time.offset }
}

/// Subject (the first paragraph on one line, as git shows it) and body.
fn split_message(raw: &BStr) -> (String, String) {
    let text = raw.to_str_lossy();
    let text = text.trim_start_matches(['\n', '\r']);
    let (subject, body) = match text.find("\n\n").or_else(|| text.find("\r\n\r\n")) {
        Some(i) => (&text[..i], &text[i..]),
        None => (text, ""),
    };
    let subject = subject.lines().map(str::trim).collect::<Vec<_>>().join(" ");
    (subject, body.trim().to_string())
}

fn resolve(repo: &gix::Repository, rev: &str) -> Result<ObjectId> {
    let id = repo.rev_parse_single(rev).ctx(format!("no commit named {rev:?}"))?;
    let commit = id.object().ctx("reading object")?.peel_to_commit().ctx(format!("{rev:?} is not a commit"))?;
    Ok(commit.id().detach())
}

fn tips(repo: &gix::Repository, q: &LogQuery) -> Result<Vec<ObjectId>> {
    if !q.all {
        return match &q.rev {
            Some(rev) => Ok(vec![resolve(repo, rev)?]),
            // An empty repository has no history yet.
            None => Ok(repo.head_id().ok().map(|id| id.detach()).into_iter().collect()),
        };
    }
    let mut out: Vec<ObjectId> = repo.head_id().ok().map(|id| id.detach()).into_iter().collect();
    let refs = repo.references().ctx("reading refs")?;
    for r in refs.all().ctx("listing refs")? {
        let Ok(mut r) = r else { continue };
        if !matches!(
            r.name().category(),
            Some(gix::reference::Category::LocalBranch | gix::reference::Category::RemoteBranch | gix::reference::Category::Tag)
        ) {
            continue;
        }
        if let Ok(c) = r.peel_to_commit() {
            out.push(c.id().detach());
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn matches_query(entry: &LogEntry, body: &str, needle: &str) -> bool {
    let has = |s: &str| s.to_lowercase().contains(needle);
    entry.sha.starts_with(needle) || has(&entry.subject) || has(body) || has(&entry.author.name) || has(&entry.author.email)
}

pub fn log(repo: &gix::Repository, q: &LogQuery) -> Result<LogPage> {
    let limit = q.limit.unwrap_or(DEFAULT_LOG_LIMIT).clamp(1, MAX_LOG_LIMIT) as usize;
    let skip = q.skip as usize;
    let needle = q.query.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_lowercase);

    let tips = tips(repo, q)?;
    let mut page = LogPage { commits: Vec::new(), has_more: false, lanes: 1 };
    if tips.is_empty() {
        return Ok(page);
    }
    let labels = ref_labels(repo)?;
    let walk = repo
        .rev_walk(tips)
        .sorting(gix::revision::walk::Sorting::ByCommitTime(Default::default()))
        .all()
        .ctx("walking history")?;

    let mut lanes = graph::Lanes::new();
    let mut seen = 0usize;
    for info in walk {
        let info = info.ctx("walking history")?;
        let parents: Vec<ObjectId> = info.parent_ids().map(|p| p.detach()).collect();
        // The graph has to see every commit from the top, even skipped ones.
        let row = if needle.is_none() { Some(lanes.row(&info.id, &parents)) } else { None };

        if needle.is_none() && seen < skip {
            seen += 1;
            continue;
        }
        if page.commits.len() == limit {
            page.has_more = true;
            break;
        }

        let commit = info.object().ctx("reading commit")?;
        let (subject, body) = split_message(commit.message_raw_sloppy());
        let entry = LogEntry {
            sha: info.id.to_string(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            subject,
            author: signature(commit.author().ctx("reading author")?),
            time: commit.time().map(|t| t.seconds).unwrap_or(0),
            refs: labels.get(&info.id).cloned().unwrap_or_default(),
            graph: row,
        };
        if let Some(needle) = &needle {
            if !matches_query(&entry, &body, needle) {
                continue;
            }
            if seen < skip {
                seen += 1;
                continue;
            }
        }
        page.commits.push(entry);
    }
    page.lanes = lanes.widest.max(1);
    Ok(page)
}

// ---------------------------------------------------------------------------
// Commits and diffs
// ---------------------------------------------------------------------------

/// One changed file between a commit and its first parent, with blob ids.
struct Change {
    path: String,
    old_path: Option<String>,
    status: ChangeStatus,
    old: Option<(ObjectId, gix::object::tree::EntryMode)>,
    new: Option<(ObjectId, gix::object::tree::EntryMode)>,
}

fn changes(repo: &gix::Repository, commit: &gix::Commit<'_>) -> Result<Vec<Change>> {
    let tree = commit.tree().ctx("reading tree")?;
    let parent_tree = match commit.parent_ids().next() {
        Some(p) => Some(repo.find_commit(p).ctx("reading parent")?.tree().ctx("reading parent tree")?),
        None => None,
    };
    let options = gix::diff::Options::default().with_rewrites(Some(gix::diff::Rewrites::default()));
    let raw = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), options).ctx("comparing trees")?;

    use gix::object::tree::diff::ChangeDetached as C;
    let mut out = Vec::new();
    for c in raw {
        let change = match c {
            C::Addition { location, entry_mode, id, .. } => {
                if entry_mode.is_tree() {
                    continue;
                }
                Change { path: location.to_string(), old_path: None, status: ChangeStatus::Added, old: None, new: Some((id, entry_mode)) }
            }
            C::Deletion { location, entry_mode, id, .. } => {
                if entry_mode.is_tree() {
                    continue;
                }
                Change { path: location.to_string(), old_path: None, status: ChangeStatus::Deleted, old: Some((id, entry_mode)), new: None }
            }
            C::Modification { location, previous_entry_mode, previous_id, entry_mode, id } => {
                if entry_mode.is_tree() && previous_entry_mode.is_tree() {
                    continue;
                }
                let status = if previous_entry_mode.kind() != entry_mode.kind() { ChangeStatus::TypeChanged } else { ChangeStatus::Modified };
                Change { path: location.to_string(), old_path: None, status, old: Some((previous_id, previous_entry_mode)), new: Some((id, entry_mode)) }
            }
            C::Rewrite { source_location, source_entry_mode, source_id, entry_mode, id, location, copy, .. } => {
                if entry_mode.is_tree() {
                    continue;
                }
                Change {
                    path: location.to_string(),
                    old_path: Some(source_location.to_string()),
                    status: if copy { ChangeStatus::Copied } else { ChangeStatus::Renamed },
                    old: Some((source_id, source_entry_mode)),
                    new: Some((id, entry_mode)),
                }
            }
        };
        out.push(change);
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

enum SideError {
    TooLarge,
    Unreadable(Error),
}

/// The bytes a side of a change diffs as: file contents, a symlink's target,
/// or a submodule's commit id.
fn side(repo: &gix::Repository, side: Option<(ObjectId, gix::object::tree::EntryMode)>) -> std::result::Result<Vec<u8>, SideError> {
    match side {
        None => Ok(Vec::new()),
        Some((id, mode)) if mode.is_commit() => Ok(format!("Subproject commit {id}\n").into_bytes()),
        Some((id, _)) => {
            if !repo.has_object(id) {
                // A partial clone fetches file contents on demand; we don't fetch.
                return Err(SideError::Unreadable(Error(format!(
                    "the contents of {} aren't in this repository (a partial clone fetches them on demand, and Neutrino doesn't fetch)",
                    id.to_hex_with_len(7)
                ))));
            }
            let header = repo.find_header(id).ctx("reading blob header").map_err(SideError::Unreadable)?;
            if header.size() as usize > neutrino_diff::MAX_DIFF_BYTES {
                return Err(SideError::TooLarge);
            }
            Ok(repo.find_blob(id).ctx("reading blob").map_err(SideError::Unreadable)?.take_data())
        }
    }
}

fn find_commit<'r>(repo: &'r gix::Repository, sha: &str) -> Result<gix::Commit<'r>> {
    let id = resolve(repo, sha)?;
    repo.find_commit(id).ctx("reading commit")
}

pub fn commit(repo: &gix::Repository, sha: &str) -> Result<CommitDetail> {
    let commit = find_commit(repo, sha)?;
    let id = commit.id().detach();
    let (subject, body) = split_message(commit.message_raw_sloppy());
    let all = changes(repo, &commit)?;
    let truncated = all.len() > MAX_FILES;

    let mut files = Vec::new();
    for (i, c) in all.into_iter().take(MAX_FILES).enumerate() {
        let (mut additions, mut deletions, mut binary) = (0, 0, false);
        if i < MAX_FILES_WITH_STATS {
            match (side(repo, c.old), side(repo, c.new)) {
                (Ok(old), Ok(new)) => {
                    if neutrino_diff::is_binary(&old) || neutrino_diff::is_binary(&new) {
                        binary = true;
                    } else {
                        (additions, deletions) = neutrino_diff::count(&old, &new);
                    }
                }
                // Too big to count; the diff view says so.
                _ => {}
            }
        }
        files.push(FileChange { path: c.path, old_path: c.old_path, status: c.status, additions, deletions, binary });
    }

    Ok(CommitDetail {
        sha: id.to_string(),
        parents: commit.parent_ids().map(|p| p.to_string()).collect(),
        author: signature(commit.author().ctx("reading author")?),
        committer: signature(commit.committer().ctx("reading committer")?),
        subject,
        body,
        refs: ref_labels(repo)?.remove(&id).unwrap_or_default(),
        files,
        truncated,
    })
}

/// Diffs already computed, keyed by both sides' blob ids and the context.
/// Blobs never change, so entries never go stale; the oldest are dropped.
pub struct DiffCache {
    inner: Mutex<(HashMap<(Option<ObjectId>, Option<ObjectId>, u32), FileDiffBody>, std::collections::VecDeque<(Option<ObjectId>, Option<ObjectId>, u32)>)>,
    capacity: usize,
}

#[derive(Clone)]
struct FileDiffBody {
    binary: bool,
    too_large: bool,
    additions: u32,
    deletions: u32,
    hunks: Vec<neutrino_proto::Hunk>,
}

impl DiffCache {
    pub fn new(capacity: usize) -> Self {
        DiffCache { inner: Mutex::new((HashMap::new(), Default::default())), capacity }
    }

    fn get(&self, key: &(Option<ObjectId>, Option<ObjectId>, u32)) -> Option<FileDiffBody> {
        self.inner.lock().unwrap().0.get(key).cloned()
    }

    fn put(&self, key: (Option<ObjectId>, Option<ObjectId>, u32), value: FileDiffBody) {
        let mut guard = self.inner.lock().unwrap();
        let (map, order) = &mut *guard;
        if map.insert(key, value).is_none() {
            order.push_back(key);
            while order.len() > self.capacity {
                if let Some(old) = order.pop_front() {
                    map.remove(&old);
                }
            }
        }
    }
}

impl Default for DiffCache {
    fn default() -> Self {
        DiffCache::new(512)
    }
}

pub fn file_diff(repo: &gix::Repository, sha: &str, path: &str, context: u32, cache: &DiffCache) -> Result<FileDiff> {
    let commit = find_commit(repo, sha)?;
    let Some(c) = changes(repo, &commit)?.into_iter().find(|c| c.path == path) else {
        return fail(format!("{path} did not change in {}", commit.id().to_hex_with_len(7)));
    };
    let key = (c.old.map(|s| s.0), c.new.map(|s| s.0), context);
    let body = match cache.get(&key) {
        Some(body) => body,
        None => {
            let body = match (side(repo, c.old), side(repo, c.new)) {
                (Err(SideError::Unreadable(e)), _) | (_, Err(SideError::Unreadable(e))) => return Err(e),
                (Ok(old), Ok(new)) if neutrino_diff::is_binary(&old) || neutrino_diff::is_binary(&new) => {
                    FileDiffBody { binary: true, too_large: false, additions: 0, deletions: 0, hunks: Vec::new() }
                }
                (Ok(old), Ok(new)) => {
                    let d = neutrino_diff::diff(&old, &new, context);
                    FileDiffBody { binary: false, too_large: false, additions: d.additions, deletions: d.deletions, hunks: d.hunks }
                }
                _ => FileDiffBody { binary: false, too_large: true, additions: 0, deletions: 0, hunks: Vec::new() },
            };
            cache.put(key, body.clone());
            body
        }
    };
    Ok(FileDiff {
        path: c.path,
        old_path: c.old_path,
        status: c.status,
        binary: body.binary,
        too_large: body.too_large,
        additions: body.additions,
        deletions: body.deletions,
        hunks: body.hunks,
    })
}

#[cfg(test)]
mod tests;
