//! Wire protocol for Neutrino's control API.
//!
//! Clients (the `ntr` CLI and the SwiftUI app) talk to `neutrinod` over a Unix
//! socket. Each message is one JSON object on one line. A request carries an
//! `id` the client picks; the response to it carries the same `id`, so one
//! connection can have several requests in flight and answers may arrive in
//! any order:
//!
//! ```json
//! {"id":7,"method":"log","params":{"repo":"nexus","limit":50}}
//! {"id":7,"type":"log","data":{"commits":[...],"has_more":true}}
//! ```
//!
//! `NeutrinoKit/Wire.swift` mirrors these types; tests on both sides pin the
//! JSON shape.

use serde::{Deserialize, Serialize};

/// Bumped whenever a breaking change is made to the control API.
pub const PROTOCOL_VERSION: u32 = 4;

/// TCP port of the server's paired TLS endpoint, and its Bonjour service type.
pub const REMOTE_PORT: u16 = 7844;
pub const BONJOUR_SERVICE: &str = "_neutrino._tcp";

// ---------------------------------------------------------------------------
// Repositories and refs
// ---------------------------------------------------------------------------

/// A working copy (or bare repository) the daemon knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    /// Short id, unique among registered repos (the folder name, made unique).
    pub id: String,
    pub name: String,
    /// Absolute path of the working tree, or of the repository for a bare one.
    pub path: String,
    pub bare: bool,
    /// `None` when the repo can't be read; `error` then says why.
    pub head: Option<Head>,
    /// Set when the folder is gone or no longer holds a repository.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Head {
    /// HEAD points at a branch.
    Branch {
        name: String,
        /// `None` for a branch with no commits yet.
        sha: Option<String>,
    },
    /// HEAD points at a commit directly.
    Detached { sha: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    /// `HEAD` itself, when detached.
    Head,
    Local,
    Remote,
    Tag,
}

/// A ref pointing at a commit, as shown next to it in the history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefLabel {
    /// Short name: `main`, `origin/main`, `v1.0`.
    pub name: String,
    pub kind: RefKind,
    /// Set on the branch HEAD points at.
    #[serde(default)]
    pub is_head: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    /// Short name: `main` or `origin/main`.
    pub name: String,
    /// `local` or `remote`.
    pub kind: RefKind,
    pub sha: String,
    pub subject: String,
    /// Committer time, Unix seconds.
    pub time: i64,
    pub is_head: bool,
    /// The configured upstream of a local branch, e.g. `origin/main`.
    pub upstream: Option<String>,
    /// Commits on this branch not on its upstream, and the other way round.
    /// Both `None` without an upstream (or when the upstream ref is missing).
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub name: String,
    pub email: String,
    /// Unix seconds.
    pub time: i64,
    /// Offset from UTC in seconds, as recorded in the commit.
    pub offset: i32,
}

/// One line of the commit graph, drawn in a row of the history.
///
/// Lanes are columns, numbered from 0 at the left. The commit's dot sits in
/// the middle of the row at `lane`. `upper` segments run from the top edge of
/// the row (at `from`) to the middle (at `to`); `lower` segments run from the
/// middle (at `from`) to the bottom edge (at `to`). A lane that only passes
/// through the row appears in both with `from == to`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphRow {
    pub lane: u16,
    /// Colour index of the commit's lane; the client maps it to a palette.
    pub color: u16,
    pub upper: Vec<GraphSegment>,
    pub lower: Vec<GraphSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphSegment {
    pub from: u16,
    pub to: u16,
    pub color: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub sha: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub author: Signature,
    /// Committer time, Unix seconds.
    pub time: i64,
    pub refs: Vec<RefLabel>,
    /// `None` when the log was filtered, since a filtered list has no graph.
    pub graph: Option<GraphRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogPage {
    pub commits: Vec<LogEntry>,
    /// More commits follow; ask again with `skip` increased by `commits.len()`.
    pub has_more: bool,
    /// Widest the graph gets on this page, in lanes.
    pub lanes: u16,
}

// ---------------------------------------------------------------------------
// Commits and diffs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    /// A file became a symlink or the other way round.
    TypeChanged,
    /// Not tracked yet (working-tree status only).
    Untracked,
    /// Both sides of a merge changed it and it isn't resolved (working-tree status only).
    Conflicted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    /// Set for renames and copies.
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitDetail {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: Signature,
    pub committer: Signature,
    pub subject: String,
    /// The message after the subject line, trimmed; empty when there is none.
    pub body: String,
    pub refs: Vec<RefLabel>,
    /// Changes against the first parent (against nothing for a root commit).
    pub files: Vec<FileChange>,
    /// Set when the commit touches more files than were listed.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// Part of a changed line; `changed` marks the words that differ from the
/// paired line on the other side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub text: String,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    /// The line without its line ending. Invalid UTF-8 is replaced.
    pub text: String,
    /// Word-level highlights; empty unless the line was paired with a similar one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<Segment>,
    /// The file ends on this line without a newline.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_newline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    /// 1-based, as in a unified diff header.
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    pub binary: bool,
    /// Set when a side is too big to diff; `hunks` is then empty.
    pub too_large: bool,
    pub additions: u32,
    pub deletions: u32,
    pub hunks: Vec<Hunk>,
}

// ---------------------------------------------------------------------------
// Working copy
// ---------------------------------------------------------------------------

/// A file in the working-tree status. Line counts are against the other side
/// of its area: the index against HEAD when staged, the working tree against
/// the index otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusFile {
    pub path: String,
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingStatus {
    pub head: Option<Head>,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    /// A merge, rebase, cherry-pick, revert or bisect in progress.
    pub operation: Option<String>,
    pub staged: Vec<StatusFile>,
    pub unstaged: Vec<StatusFile>,
    pub untracked: Vec<StatusFile>,
    pub conflicted: Vec<StatusFile>,
}

/// One changed line picked in a working-tree diff: an added line by its new
/// line number, a removed line by its old one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LinePick {
    pub kind: LineKind,
    pub line: u32,
}

/// Which side of the working copy a diff or a line pick is about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Area {
    /// The index against HEAD.
    Staged,
    /// The working tree against the index (untracked files included).
    #[default]
    Unstaged,
}

/// Something changed in a repo the client subscribed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEvent {
    pub repo: String,
    /// `worktree` (files changed) or `refs` (HEAD, branches or the index moved).
    pub kind: String,
}

// ---------------------------------------------------------------------------
// Server, remotes and pairing
// ---------------------------------------------------------------------------

/// A remote of a working copy, as `git remote -v` lists it (fetch URL).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteInfo {
    pub name: String,
    pub url: String,
}

/// One ref a push wants to move, as a git hook reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefUpdate {
    /// All zeros when the ref is created.
    pub old: String,
    /// All zeros when the ref is deleted.
    pub new: String,
    /// Full name, e.g. `refs/heads/main`.
    pub name: String,
    /// The new commit doesn't descend from the old one. The pre-receive hook
    /// works this out, since only it can see the pushed objects.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub forced: bool,
}

/// What `ntr pair` shows on the server: everything a device needs to pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingInfo {
    /// One-time code, valid for `expires_in_secs`.
    pub code: String,
    pub expires_in_secs: u64,
    /// The server's name as devices show it.
    pub host: String,
    pub port: u16,
    /// SHA-256 of the endpoint's certificate, lowercase hex; devices pin it.
    pub fingerprint: String,
    /// The server's LAN addresses.
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    /// Unix seconds.
    pub paired_at: u64,
    pub last_seen: Option<u64>,
}

// ---------------------------------------------------------------------------
// Pull requests and branch rules (server)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullState {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub repo: String,
    pub number: u32,
    pub title: String,
    pub body: String,
    /// The branch being merged.
    pub source: String,
    /// The branch it merges into.
    pub target: String,
    pub state: PullState,
    /// The latest version's head.
    pub head_sha: String,
    pub versions: u32,
    /// Unresolved notes.
    pub open_notes: u32,
    /// Unix seconds.
    pub created_at: i64,
    pub updated_at: i64,
    pub merged_at: Option<i64>,
    /// The commit the merge left on the target.
    pub merge_sha: Option<String>,
}

/// One push to the source branch while the pull request was open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullVersion {
    /// From 1.
    pub version: u32,
    pub head_sha: String,
    /// Where the source left the target when this version was pushed.
    pub base_sha: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// A line of the base version (a removed line).
    Old,
    /// A line of the head version (an added or unchanged line).
    New,
}

/// A note on a line of a pull request's diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub id: i64,
    /// The version it was written on.
    pub version: u32,
    pub path: String,
    pub side: Side,
    /// The line it was written on, in that version.
    pub line: u32,
    pub text: String,
    pub created_at: i64,
    pub resolved: bool,
    /// Where the line is in the latest version; `None` when that line changed
    /// since (the note is outdated).
    pub current_line: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeStrategy {
    /// A merge commit with both parents.
    Merge,
    /// One new commit with the pull request's changes.
    Squash,
    /// The pull request's commits replayed onto the target.
    Rebase,
    /// Move the target to the source; only when nothing landed on the target meanwhile.
    FastForward,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mergeability {
    /// Merges without conflicts.
    pub clean: bool,
    /// Files that conflict, when it doesn't.
    pub conflicts: Vec<String>,
    /// The target is an ancestor of the source.
    pub fast_forward: bool,
    /// Commits on the target that the source doesn't have.
    pub behind: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchRule {
    /// A branch name, or a pattern with `*` (`release/*`).
    pub pattern: String,
    /// Changes reach these branches only by merging a pull request.
    pub require_pull: bool,
    /// Refuse pushes that rewrite history.
    pub block_force: bool,
    /// Refuse deleting the branch.
    pub block_delete: bool,
    /// Checks that must pass before a merge (enforced once CI exists).
    #[serde(default)]
    pub required_checks: Vec<String>,
}

/// A push that went through despite a rule, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleOverride {
    pub ref_name: String,
    /// Which rule it broke: `require_pull`, `block_force` or `block_delete`.
    pub rule: String,
    pub reason: String,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullDetail {
    pub pull: PullRequest,
    pub versions: Vec<PullVersion>,
    pub notes: Vec<Note>,
    /// For an open pull request.
    pub mergeability: Option<Mergeability>,
    /// The latest version against where it left the target.
    pub files: Vec<FileChange>,
    /// Its commits, oldest first.
    pub commits: Vec<LogEntry>,
    /// The rule that guards the target, if one does.
    pub rule: Option<BranchRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineInfo {
    pub version: String,
    pub protocol: u32,
    /// `workstation` or `server`.
    pub role: String,
    pub repos: u32,
}

// ---------------------------------------------------------------------------
// Control API
// ---------------------------------------------------------------------------

/// A client request. Serialises as `{"method": "...", "params": {...}}`; the
/// envelope adds `"id"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Request {
    Ping,
    EngineInfo,
    ListRepos,
    /// Register a working copy (any folder inside one works).
    AddRepo { path: String },
    /// Forget a repo. Nothing on disk is touched.
    RemoveRepo { repo: String },
    Branches { repo: String },
    Log(LogQuery),
    Commit { repo: String, sha: String },
    /// One file's diff in a commit, against the commit's first parent.
    FileDiff {
        repo: String,
        sha: String,
        path: String,
        /// Lines of context around each change. Default 3.
        #[serde(default)]
        context: Option<u32>,
    },
    /// Staged, unstaged, untracked and conflicted files.
    Status { repo: String },
    /// One file's working-copy diff.
    WorkDiff {
        repo: String,
        path: String,
        #[serde(default)]
        area: Area,
        #[serde(default)]
        context: Option<u32>,
    },
    /// Stage whole files (new, changed or deleted).
    Stage { repo: String, paths: Vec<String> },
    /// Take whole files out of the index.
    Unstage { repo: String, paths: Vec<String> },
    /// Stage (from `unstaged`) or unstage (from `staged`) some lines of one file.
    StageLines { repo: String, path: String, area: Area, lines: Vec<LinePick> },
    /// Throw away working-tree changes to whole files. Their contents are
    /// kept as objects in the repo and listed in ~/.neutrino/discarded.log.
    Discard { repo: String, paths: Vec<String> },
    /// Throw away some working-tree lines of one file.
    DiscardLines { repo: String, path: String, lines: Vec<LinePick> },
    /// Commit what's staged. Runs git, so hooks and signing apply.
    CommitChanges {
        repo: String,
        message: String,
        #[serde(default)]
        amend: bool,
    },
    CreateBranch {
        repo: String,
        name: String,
        /// Where it starts. Default HEAD.
        #[serde(default)]
        start: Option<String>,
        #[serde(default)]
        checkout: bool,
    },
    /// Switch to a local branch, or to a remote one as a new tracking branch.
    Checkout { repo: String, branch: String },
    RenameBranch { repo: String, from: String, to: String },
    /// Delete a local branch; without `force`, only once it's merged.
    DeleteBranch {
        repo: String,
        name: String,
        #[serde(default)]
        force: bool,
    },
    /// Keep this request open and answer it with `event`s as repos change.
    Subscribe,

    // --- Remotes (workstation) ---
    Remotes { repo: String },
    AddRemote { repo: String, name: String, url: String },
    /// Fetch one remote, or all of them.
    Fetch {
        repo: String,
        #[serde(default)]
        remote: Option<String>,
    },
    /// Push the current branch (or `branch`) to `remote` (default: its upstream's, else `origin`).
    Push {
        repo: String,
        #[serde(default)]
        remote: Option<String>,
        #[serde(default)]
        branch: Option<String>,
        /// Make the remote branch the upstream (`git push -u`).
        #[serde(default)]
        set_upstream: bool,
    },

    // --- Hosting (server) ---
    /// Create a hosted bare repository. Answers `repo`; its `path` is what to push to.
    CreateHosted {
        name: String,
        #[serde(default)]
        description: Option<String>,
    },
    /// From the pre-receive hook: may these updates land? Local only.
    HookPreReceive {
        repo: String,
        updates: Vec<RefUpdate>,
        /// `git push -o …` options; `neutrino-override=<reason>` overrides a rule.
        #[serde(default)]
        push_options: Vec<String>,
    },
    /// From the post-receive hook: these updates landed. Local only.
    HookPostReceive { repo: String, updates: Vec<RefUpdate> },

    // --- Pull requests (server) ---
    ListPulls { repo: String },
    CreatePull {
        repo: String,
        title: String,
        #[serde(default)]
        body: String,
        source: String,
        target: String,
    },
    Pull { repo: String, number: u32 },
    /// Change the title or description, or close or reopen.
    UpdatePull {
        repo: String,
        number: u32,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        state: Option<PullState>,
    },
    AddNote { repo: String, number: u32, path: String, side: Side, line: u32, text: String },
    ResolveNote { repo: String, number: u32, note: i64, resolved: bool },
    MergePull {
        repo: String,
        number: u32,
        strategy: MergeStrategy,
        /// The merge or squash commit's message. Default: the pull request's title and number.
        #[serde(default)]
        message: Option<String>,
        /// Delete the source branch afterwards.
        #[serde(default)]
        delete_source: bool,
    },
    /// The files changed between two commits.
    Compare { repo: String, base: String, head: String },
    /// One file's diff between two commits.
    CompareDiff {
        repo: String,
        base: String,
        head: String,
        path: String,
        #[serde(default)]
        context: Option<u32>,
    },
    BranchRules { repo: String },
    /// Add a rule, or replace the one with the same pattern.
    SetBranchRule { repo: String, rule: BranchRule },
    DeleteBranchRule { repo: String, pattern: String },
    /// Pushes that overrode a rule, newest first.
    Overrides { repo: String },

    // --- Pairing (server) ---
    /// A one-time pairing code. Local only.
    PairBegin,
    /// Devices paired with this server. Local only.
    ListDevices,
    /// Forget a device; its token stops working. Local only.
    RevokeDevice { id: String },
    /// Over TLS, as the first message: trade a pairing code for a token.
    Pair { code: String, device: String },
    /// Over TLS, as the first message: authenticate with a token from `pair`.
    Auth { token: String },
    /// Drop the answer to an earlier request on this connection.
    Cancel { id: u64 },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogQuery {
    pub repo: String,
    /// Where to start: a branch, tag or sha. Default `HEAD`.
    #[serde(default)]
    pub rev: Option<String>,
    /// Walk from every branch and tag instead of from `rev`.
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub skip: u32,
    /// Default 100, at most 1000.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Keep commits whose message, author or sha contains this (any case).
    #[serde(default)]
    pub query: Option<String>,
}

/// A daemon response. Serialises as `{"type": "...", "data": ...}`; the
/// envelope adds `"id"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Response {
    Pong { protocol: u32 },
    Engine(EngineInfo),
    Repos(Vec<Repo>),
    Repo(Repo),
    Removed { repo: String },
    Branches(Vec<Branch>),
    Log(LogPage),
    Commit(Box<CommitDetail>),
    FileDiff(FileDiff),
    Status(WorkingStatus),
    /// A write finished; `output` is what git printed, if anything.
    Done { output: String },
    Committed { sha: String, output: String },
    /// One of possibly many answers to `subscribe`.
    Event(RepoEvent),
    Remotes(Vec<RemoteInfo>),
    Pairing(PairingInfo),
    Devices(Vec<PairedDevice>),
    Revoked { id: String },
    Paired { token: String, device_id: String, host: String },
    Pulls(Vec<PullRequest>),
    Pull(Box<PullDetail>),
    Merged { sha: String },
    Compare(Vec<FileChange>),
    Rules(Vec<BranchRule>),
    Overrides(Vec<RuleOverride>),
    Authenticated { host: String, role: String },
    /// The request was cancelled before it finished.
    Cancelled,
    Error { message: String },
}

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum DecodeError {
    Json(serde_json::Error),
    MissingId,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Json(e) => write!(f, "bad request: {e}"),
            DecodeError::MissingId => f.write_str("bad request: no numeric \"id\""),
        }
    }
}

impl std::error::Error for DecodeError {}

fn split_id(line: &str) -> Result<(u64, serde_json::Value), DecodeError> {
    let mut value: serde_json::Value = serde_json::from_str(line).map_err(DecodeError::Json)?;
    let id = value
        .as_object_mut()
        .and_then(|o| o.remove("id"))
        .and_then(|v| v.as_u64())
        .ok_or(DecodeError::MissingId)?;
    Ok((id, value))
}

fn join_id(id: u64, message: &impl Serialize) -> String {
    let mut value = serde_json::to_value(message).expect("wire types serialise");
    if let Some(o) = value.as_object_mut() {
        o.insert("id".into(), id.into());
    }
    value.to_string()
}

/// Parse one request line. The id is returned even when the rest is bad, so
/// the error can be answered.
pub fn decode_request(line: &str) -> (Option<u64>, Result<Request, DecodeError>) {
    match split_id(line) {
        Ok((id, rest)) => (Some(id), serde_json::from_value(rest).map_err(DecodeError::Json)),
        Err(e) => (None, Err(e)),
    }
}

pub fn decode_response(line: &str) -> Result<(u64, Response), DecodeError> {
    let (id, rest) = split_id(line)?;
    Ok((id, serde_json::from_value(rest).map_err(DecodeError::Json)?))
}

/// One line, without the trailing newline.
pub fn encode_request(id: u64, request: &Request) -> String {
    join_id(id, request)
}

pub fn encode_response(id: u64, response: &Response) -> String {
    join_id(id, response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wire(line: &str) -> serde_json::Value {
        serde_json::from_str(line).unwrap()
    }

    #[test]
    fn request_shape() {
        let line = encode_request(
            7,
            &Request::Log(LogQuery { repo: "nexus".into(), limit: Some(50), ..Default::default() }),
        );
        assert_eq!(
            wire(&line),
            json!({"id": 7, "method": "log", "params": {"repo": "nexus", "rev": null, "all": false, "skip": 0, "limit": 50, "query": null}})
        );
        assert_eq!(wire(&encode_request(1, &Request::Ping)), json!({"id": 1, "method": "ping"}));
    }

    #[test]
    fn request_defaults_are_optional() {
        let (id, req) = decode_request(r#"{"id":3,"method":"log","params":{"repo":"x"}}"#);
        assert_eq!(id, Some(3));
        assert_eq!(req.unwrap(), Request::Log(LogQuery { repo: "x".into(), ..Default::default() }));
        let (_, req) = decode_request(r#"{"id":4,"method":"file_diff","params":{"repo":"x","sha":"a","path":"p"}}"#);
        assert_eq!(req.unwrap(), Request::FileDiff { repo: "x".into(), sha: "a".into(), path: "p".into(), context: None });
    }

    #[test]
    fn bad_requests_keep_their_id() {
        let (id, req) = decode_request(r#"{"id":9,"method":"nope"}"#);
        assert_eq!(id, Some(9));
        assert!(req.is_err());
        let (id, req) = decode_request(r#"{"method":"ping"}"#);
        assert_eq!(id, None);
        assert!(matches!(req, Err(DecodeError::MissingId)));
    }

    #[test]
    fn response_shape() {
        let line = encode_response(2, &Response::Pong { protocol: PROTOCOL_VERSION });
        assert_eq!(wire(&line), json!({"id": 2, "type": "pong", "data": {"protocol": PROTOCOL_VERSION}}));
        let line = encode_response(5, &Response::Cancelled);
        assert_eq!(wire(&line), json!({"id": 5, "type": "cancelled"}));
        let head = Head::Branch { name: "main".into(), sha: Some("abc".into()) };
        assert_eq!(serde_json::to_value(&head).unwrap(), json!({"kind": "branch", "name": "main", "sha": "abc"}));
    }

    #[test]
    fn diff_line_omits_empty_extras() {
        let line = DiffLine { kind: LineKind::Added, old_line: None, new_line: Some(4), text: "x".into(), segments: vec![], no_newline: false };
        assert_eq!(serde_json::to_value(&line).unwrap(), json!({"kind": "added", "old_line": null, "new_line": 4, "text": "x"}));
    }

    #[test]
    fn working_copy_shapes() {
        let req = Request::StageLines { repo: "r".into(), path: "a.rs".into(), area: Area::Unstaged, lines: vec![LinePick { kind: LineKind::Added, line: 4 }] };
        assert_eq!(
            wire(&encode_request(3, &req)),
            json!({"id": 3, "method": "stage_lines", "params": {"repo": "r", "path": "a.rs", "area": "unstaged", "lines": [{"kind": "added", "line": 4}]}})
        );
        let (_, req) = decode_request(r#"{"id":1,"method":"work_diff","params":{"repo":"r","path":"p"}}"#);
        assert_eq!(req.unwrap(), Request::WorkDiff { repo: "r".into(), path: "p".into(), area: Area::Unstaged, context: None });
        let ev = Response::Event(RepoEvent { repo: "r".into(), kind: "worktree".into() });
        assert_eq!(wire(&encode_response(9, &ev)), json!({"id": 9, "type": "event", "data": {"repo": "r", "kind": "worktree"}}));
        assert_eq!(serde_json::to_value(ChangeStatus::Untracked).unwrap(), json!("untracked"));
    }

    #[test]
    fn server_shapes() {
        let push = Request::Push { repo: "r".into(), remote: None, branch: None, set_upstream: true };
        assert_eq!(wire(&encode_request(1, &push)), json!({"id": 1, "method": "push", "params": {"repo": "r", "remote": null, "branch": null, "set_upstream": true}}));
        let (_, fetch) = decode_request(r#"{"id":2,"method":"fetch","params":{"repo":"r"}}"#);
        assert_eq!(fetch.unwrap(), Request::Fetch { repo: "r".into(), remote: None });
        let (_, auth) = decode_request(r#"{"id":0,"method":"auth","params":{"token":"t"}}"#);
        assert_eq!(auth.unwrap(), Request::Auth { token: "t".into() });
        let ok = Response::Authenticated { host: "atlas".into(), role: "server".into() };
        assert_eq!(wire(&encode_response(0, &ok)), json!({"id": 0, "type": "authenticated", "data": {"host": "atlas", "role": "server"}}));
    }

    #[test]
    fn round_trip() {
        let resp = Response::Error { message: "no such repo".into() };
        let (id, back) = decode_response(&encode_response(11, &resp)).unwrap();
        assert_eq!((id, back), (11, resp));
    }

    /// The same line the Swift app's `pullWire` test decodes.
    #[test]
    fn pull_detail_wire() {
        let line = r#"{"id":9,"type":"pull","data":{"pull":{"repo":"app","number":3,"title":"Shout","body":"Louder.","source":"feature","target":"main","state":"open","head_sha":"bbb","versions":2,"open_notes":1,"created_at":10,"updated_at":20,"merged_at":null,"merge_sha":null},"versions":[{"version":1,"head_sha":"aaa","base_sha":"000","created_at":10},{"version":2,"head_sha":"bbb","base_sha":"000","created_at":20}],"notes":[{"id":5,"version":1,"path":"app.txt","side":"new","line":3,"text":"Keep three","created_at":11,"resolved":false,"current_line":4},{"id":6,"version":1,"path":"app.txt","side":"old","line":1,"text":"Gone","created_at":12,"resolved":true,"current_line":null}],"mergeability":{"clean":true,"conflicts":[],"fast_forward":false,"behind":1},"files":[{"path":"app.txt","old_path":null,"status":"modified","additions":2,"deletions":1,"binary":false}],"commits":[{"sha":"bbb","parents":["aaa"],"subject":"start at zero","author":{"name":"Ada","email":"ada@example.com","time":20,"offset":0},"time":20,"refs":[],"graph":null}],"rule":{"pattern":"main","require_pull":true,"block_force":true,"block_delete":false}}}"#;
        let (id, r) = decode_response(line).unwrap();
        let Response::Pull(d) = r else { panic!("{r:?}") };
        assert_eq!((id, d.pull.number, d.notes.len(), d.notes[1].current_line), (9, 3, 2, None));
        assert!(d.rule.as_ref().unwrap().required_checks.is_empty());
        let back = encode_response(9, &Response::Pull(d.clone()));
        let (_, again) = decode_response(&back).unwrap();
        assert_eq!(again, Response::Pull(d));
    }
}
