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
pub const PROTOCOL_VERSION: u32 = 8;

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
    /// Whether the tip is already in the default branch (`main`, else
    /// `master`): `None` for the default branch itself, or when it's unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged: Option<bool>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyntaxKind {
    Keyword,
    Type,
    Function,
    String,
    Number,
    Comment,
    Property,
    Operator,
    Attribute,
    Constant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntaxSpan {
    pub start: u32,
    pub len: u32,
    pub kind: SyntaxKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseKind {
    Formatting,
    ImportSort,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveBlock {
    pub id: u32,
    pub symbol: String,
    pub kind: String,
    pub from_path: String,
    pub from_line: u32,
    pub to_path: String,
    pub to_line: u32,
    pub lines: u32,
    pub changed_lines: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticSummary {
    pub moved_blocks: u32,
    pub formatting_hunks: u32,
    pub import_hunks: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_symbols: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TourStep {
    pub path: String,
    pub tier: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub defines: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub referenced_by: Vec<String>,
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
    /// Tree-sitter syntax highlight spans within `text`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub syntax: Vec<SyntaxSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    /// 1-based, as in a unified diff header.
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
    /// Set when the hunk only changes whitespace/formatting or import order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise: Option<NoiseKind>,
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
    /// AST blocks moved within or into this file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub moves: Vec<MoveBlock>,
    /// Compact AST & noise classification summary for this file diff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_summary: Option<SemanticSummary>,
}

// ---------------------------------------------------------------------------
// Browsing files
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Dir,
    File,
    Executable,
    Symlink,
    Submodule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub name: String,
    /// Repo-relative path of the entry.
    pub path: String,
    pub kind: EntryKind,
    /// Blob size in bytes; None for dirs and submodules.
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeListing {
    /// The commit the listing was read from (full sha).
    pub commit: String,
    pub path: String,
    /// Dirs and submodules first, then everything else; each group sorted by name, case-insensitively.
    pub entries: Vec<TreeEntry>,
}

/// The newest commit to change one entry of a directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryCommit {
    /// Entry name within the listed directory.
    pub name: String,
    pub sha: String,
    pub subject: String,
    /// Commit time, unix seconds.
    pub time: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobContent {
    pub commit: String,
    pub path: String,
    pub size: u64,
    pub binary: bool,
    /// Over the limit (1 MiB); text is None.
    pub too_large: bool,
    /// UTF-8 (lossy) text when not binary and not too large.
    pub text: Option<String>,
    /// Line count of `text` (0 when None). A trailing newline doesn't add a line.
    pub lines: u32,
    /// Tree-sitter syntax spans per line (parallel to lines of `text`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub syntax: Vec<Vec<SyntaxSpan>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageShare {
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoOverview {
    pub commit: String,
    /// Commits reachable from `commit`, counted up to 100_000.
    pub commits: u32,
    pub commits_capped: bool,
    /// Distinct author emails among the counted commits.
    pub contributors: u32,
    /// Blobs in the whole tree (counted up to 200_000 entries walked).
    pub files: u32,
    /// Bytes per language by file extension or name, largest first. At most 8
    /// entries, the rest summed into "Other". Files in no known language are left out.
    pub languages: Vec<LanguageShare>,
    /// The root README's path, if there is one.
    pub readme: Option<String>,
    /// The root licence file's path, if there is one.
    pub license: Option<String>,
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
    /// It signs approvals with a key in its Secure Enclave.
    #[serde(default)]
    pub signs: bool,
    /// It gets push notifications.
    #[serde(default)]
    pub push: bool,
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
    /// Stack identifier shared by stacked PRs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_id: Option<String>,
    /// Direct upstream PR number in a stack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_pull: Option<u32>,
    /// Conflict details if automatic child restacking stopped on conflict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_conflict: Option<String>,
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
    /// Workflows (by name or file) whose latest run on the pull request must pass before a merge.
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
    /// The latest run of each workflow on the latest version.
    #[serde(default)]
    pub checks: Vec<Run>,
    /// Stacked PR chain members ordered from root to leaves.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stack: Vec<StackMember>,
    /// Call-graph / dependency-ordered file review tour.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tour: Vec<TourStep>,
    /// AST move blocks across the PR's changed files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub moves: Vec<MoveBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackMember {
    pub number: u32,
    pub title: String,
    pub source: String,
    pub target: String,
    pub state: PullState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_pull: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_conflict: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackInfo {
    pub stack_id: String,
    pub members: Vec<StackMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackSyncOutcome {
    pub number: u32,
    pub source: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackSyncReport {
    pub root: u32,
    pub updated: Vec<StackSyncOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInterdiff {
    pub number: u32,
    pub from_version: u32,
    pub to_version: u32,
    pub from_sha: String,
    pub to_sha: String,
    pub files: Vec<FileChange>,
    pub commits: Vec<LogEntry>,
    pub notes: Vec<Note>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub moves: Vec<MoveBlock>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tour: Vec<TourStep>,
}

/// Where a run, a job or a step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    Running,
    Success,
    Failure,
    Cancelled,
    /// Its condition said not to run, or what it needed failed.
    Skipped,
    /// Waiting for someone to approve its environment.
    Waiting,
}

impl RunState {
    pub fn finished(self) -> bool {
        !matches!(self, RunState::Queued | RunState::Running | RunState::Waiting)
    }

    pub fn name(self) -> &'static str {
        match self {
            RunState::Queued => "queued",
            RunState::Running => "running",
            RunState::Success => "success",
            RunState::Failure => "failure",
            RunState::Cancelled => "cancelled",
            RunState::Skipped => "skipped",
            RunState::Waiting => "waiting",
        }
    }

    pub fn parse(s: &str) -> RunState {
        match s {
            "running" => RunState::Running,
            "success" => RunState::Success,
            "failure" => RunState::Failure,
            "cancelled" => RunState::Cancelled,
            "skipped" => RunState::Skipped,
            "waiting" => RunState::Waiting,
            _ => RunState::Queued,
        }
    }
}

/// Where a secret or variable applies. Neither set: every repo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultScope {
    #[serde(default)]
    pub repo: Option<String>,
    /// Only for jobs deploying to this environment (needs `repo`).
    #[serde(default)]
    pub environment: Option<String>,
}

/// A secret's name and where it applies; never its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretInfo {
    pub scope: VaultScope,
    pub name: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variable {
    pub scope: VaultScope,
    pub name: String,
    pub value: String,
    pub updated_at: i64,
}

/// A deployment target with its own secrets, which can need approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    pub repo: String,
    pub name: String,
    /// Jobs wait until someone approves them.
    pub require_approval: bool,
    /// Branches that may deploy here (patterns with `*`); empty means any.
    #[serde(default)]
    pub branches: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub run: i64,
    pub job: String,
    /// Relative to the job's workspace.
    pub path: String,
    pub size: u64,
}

/// Part of an artifact, base64-encoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactChunk {
    pub path: String,
    pub offset: u64,
    pub size: u64,
    pub data: String,
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultInfo {
    pub set_up: bool,
    /// `keychain` or `file`.
    pub key_store: String,
}

/// A job waiting for someone to approve its environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiting {
    pub repo: String,
    pub run: i64,
    pub number: u32,
    pub run_name: String,
    pub job: i64,
    pub job_name: String,
    pub environment: Option<String>,
    pub branch: Option<String>,
    /// When the run was created.
    pub since: i64,
}

/// What needs you, across every hosted repo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inbox {
    /// Deploys waiting for approval.
    pub waiting: Vec<Waiting>,
    /// Open pull requests that merge cleanly and whose required checks passed.
    pub ready: Vec<PullRequest>,
    /// Other open pull requests.
    pub open: Vec<PullRequest>,
    /// Running runs, then recent ones, newest first.
    pub runs: Vec<Run>,
    /// What was notified lately, newest first.
    #[serde(default)]
    pub events: Vec<NotificationText>,
}

/// The real text of a push notification, fetched by the phone over the paired
/// connection: Apple only ever carries its id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationText {
    pub id: String,
    /// `approval` or `failure`.
    pub kind: String,
    pub title: String,
    pub body: String,
    pub repo: String,
    pub run: Option<i64>,
    pub pull: Option<u32>,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushInfo {
    /// An APNs key is stored.
    pub configured: bool,
    pub topic: Option<String>,
    /// Devices that get pushes.
    pub devices: u32,
}

/// A workflow file and whether it parses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowFile {
    pub file: String,
    pub name: Option<String>,
    /// `push`, `pull_request`, `workflow_dispatch`, `schedule`.
    pub triggers: Vec<String>,
    pub jobs: Vec<String>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
}

/// One workflow run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: i64,
    pub repo: String,
    /// Per repo, from 1.
    pub number: u32,
    /// The file under `.neutrino/workflows/`.
    pub workflow: String,
    pub name: String,
    /// `push`, `pull_request` or `workflow_dispatch`.
    pub event: String,
    /// The branch (or tag) it ran for.
    pub branch: Option<String>,
    pub sha: String,
    pub pull: Option<u32>,
    pub state: RunState,
    /// Why it failed before any job ran (a bad workflow file, say).
    pub error: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStep {
    pub index: u32,
    pub name: String,
    pub state: RunState,
    pub exit_code: Option<i32>,
    /// Where its output is in the job's log.
    pub log_start: u64,
    pub log_end: u64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunJob {
    pub id: i64,
    pub run: i64,
    /// Unique within the run (`test-2` for a matrix entry).
    pub key: String,
    pub name: String,
    pub executor: String,
    pub image: Option<String>,
    pub state: RunState,
    pub error: Option<String>,
    pub needs: Vec<String>,
    /// The environment it deploys to, whose secrets it gets.
    #[serde(default)]
    pub environment: Option<String>,
    /// Who let it run, when its environment needs approval.
    #[serde(default)]
    pub approved_by: Option<String>,
    pub attempt: u32,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub steps: Vec<RunStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_snapshot_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_failed_step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_exit_code: Option<i32>,
}

/// Metadata returned when attaching a developer shell to a frozen CI failure VM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenAttachment {
    pub job: i64,
    pub container_id: String,
    pub container_name: String,
    pub workdir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub cli_command: String,
    pub deep_link: String,
}

/// Branch-bound stateful Graviton Compose stack checkpoint status (Wave 4A).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchStackStatus {
    pub branch: String,
    pub key: String,
    pub project: String,
    pub has_checkpoint: bool,
    pub running: bool,
    pub services: Vec<String>,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunDetail {
    pub run: Run,
    pub jobs: Vec<RunJob>,
    /// Problems with the workflow file that didn't stop it.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Part of a job's log, from `offset`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogChunk {
    pub job: i64,
    pub offset: u64,
    /// The bytes as text; invalid UTF-8 is replaced.
    pub text: String,
    /// Where the next read starts.
    pub next: u64,
    /// The job has finished, so nothing more will come.
    pub done: bool,
}

/// Where a hosted repository's branches and tags are copied after every push.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirrorStatus {
    pub repo: String,
    pub url: String,
    /// `ok`, `pending` (a copy is due or under way) or `failing`.
    pub state: String,
    /// Unix seconds of the last copy that worked.
    pub last_ok: Option<i64>,
    pub last_attempt: Option<i64>,
    /// What git said when the last copy failed, or which refs it left alone
    /// because the mirror has commits this side doesn't.
    pub last_error: Option<String>,
    /// Refs only the mirror has (a branch made on GitHub, say), left alone.
    #[serde(default)]
    pub mirror_only: Vec<String>,
}

/// One database snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupFile {
    pub name: String,
    pub path: String,
    pub size: u64,
    /// Unix seconds.
    pub modified: i64,
}

/// The server's snapshots, or on a workstation the copies it keeps of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupList {
    pub dir: String,
    /// Newest first.
    pub files: Vec<BackupFile>,
    /// Workstation: the SSH host copies come from, when copying is on.
    pub source: Option<String>,
    /// Workstation: when copying last worked, and what went wrong since.
    pub last_copy: Option<i64>,
    pub last_error: Option<String>,
}

/// Before/after sizes and object counts from compacting one repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactReport {
    pub repo: String,
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub loose_before: u64,
    pub loose_after: u64,
    pub packs_before: u64,
    pub packs_after: u64,
}

/// What the scheduler is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiInfo {
    /// Whether gravitond answers.
    pub graviton: bool,
    pub graviton_error: Option<String>,
    pub budget_cpus: u32,
    pub budget_memory_mib: u64,
    pub used_cpus: u32,
    pub used_memory_mib: u64,
    pub running: u32,
    pub queued: u32,
    /// `runs-on: host` is allowed.
    #[serde(default)]
    pub host: bool,
    /// The Nexus coordinator `runs-on: nexus` jobs go to, if any.
    #[serde(default)]
    pub nexus: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineInfo {
    pub version: String,
    pub protocol: u32,
    /// `workstation` or `server`.
    pub role: String,
    pub repos: u32,
    /// The commit this neutrinod was built from, when the build said
    /// (scripts/deploy-server.sh sets it); `None` for an ordinary cargo build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
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
    /// One directory of a commit's tree. `rev` defaults to HEAD, `path` to the root.
    Tree {
        repo: String,
        #[serde(default)]
        rev: Option<String>,
        #[serde(default)]
        path: String,
    },
    /// The newest commit to change each entry of one directory.
    TreeCommits {
        repo: String,
        #[serde(default)]
        rev: Option<String>,
        #[serde(default)]
        path: String,
    },
    /// One file's contents at a commit.
    Blob {
        repo: String,
        #[serde(default)]
        rev: Option<String>,
        path: String,
    },
    /// Counts, languages, README and licence for a commit.
    Overview {
        repo: String,
        #[serde(default)]
        rev: Option<String>,
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
    /// Stop hosting a repository: its bare repo moves to `<state>/deleted/`,
    /// from where moving it back and restarting the server restores it.
    /// Refused while it has CI running. Answers `removed`.
    DeleteHosted { repo: String },
    /// Create a hosted bare repository. Answers `repo`; its `path` is what to push to.
    CreateHosted {
        name: String,
        #[serde(default)]
        description: Option<String>,
    },
    /// Create a hosted repository from another one's branches and tags (a
    /// GitHub repository, say), and with `mirror` copy them back there after
    /// every push. Answers `repo`. Local only.
    ImportHosted {
        name: String,
        url: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        mirror: bool,
    },
    /// Every hosted repository's mirror. Local only.
    Mirrors,
    /// Mirror a hosted repository to `url`, or stop with none. Answers `mirrors`. Local only.
    SetMirror {
        repo: String,
        #[serde(default)]
        url: Option<String>,
    },
    /// Copy a hosted repository to its mirror now, and wait. Answers `mirrors`. Local only.
    MirrorNow { repo: String },
    /// Pack refs, repack objects, prune unreachable objects and rebuild
    /// commit-graphs for one hosted repository (or all of them when `repo` is
    /// `None`). Answers `compacted`. Local only.
    Compact {
        #[serde(default)]
        repo: Option<String>,
        #[serde(default)]
        aggressive: bool,
        #[serde(default)]
        prune_now: bool,
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
        #[serde(default)]
        stack_id: Option<String>,
        #[serde(default)]
        parent_pull: Option<u32>,
    },
    Pull { repo: String, number: u32 },
    CompareVersions { repo: String, number: u32, from_version: u32, to_version: u32 },
    CompareVersionDiff {
        repo: String,
        number: u32,
        from_version: u32,
        to_version: u32,
        path: String,
        #[serde(default)]
        context: Option<u32>,
    },
    ListStacks { repo: String },
    SyncStack { repo: String, pull: u32 },
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

    // --- CI (server) ---
    /// Newest first; only one pull request's when `pull` is set.
    ListRuns {
        repo: String,
        #[serde(default)]
        pull: Option<u32>,
        #[serde(default)]
        limit: Option<u32>,
    },
    Run { repo: String, id: i64 },
    /// A job's log from `offset`.
    JobLog { repo: String, job: i64, #[serde(default)] offset: u64 },
    CancelRun { repo: String, id: i64 },
    /// Run the same workflow on the same commit again.
    RerunRun { repo: String, id: i64 },
    /// Restore a failed job's frozen VM snapshot into a live debug container.
    AttachFrozenJob { repo: String, job: i64 },
    /// List branch-bound Graviton stack checkpoint status for this repo.
    BranchStacks { repo: String },
    /// Hibernate the current repo's Graviton Compose stack under `branch`.
    HibernateBranchStack { repo: String, branch: String },
    /// Restore (thaw) a branch-bound Graviton Compose stack checkpoint.
    ThawBranchStack { repo: String, branch: String },
    /// Discard a branch-bound Graviton Compose stack checkpoint.
    DiscardBranchStack { repo: String, branch: String },
    /// Start a workflow that has `workflow_dispatch` on a branch.
    DispatchRun { repo: String, workflow: String, branch: String },
    /// Check the workflow files on a branch without running them.
    Workflows { repo: String, branch: String },
    CiInfo,

    // --- Vault and the rest of CI (server) ---
    VaultInfo,
    /// Create the master key; answers its recovery code, once. Local only.
    VaultSetup,
    /// Put the master key back from its recovery code. Local only.
    VaultRecover { code: String },
    ListSecrets { #[serde(default)] scope: VaultScope },
    /// Write-only: there's no request that reads a secret back.
    SetSecret { scope: VaultScope, name: String, value: String },
    DeleteSecret { scope: VaultScope, name: String },
    ListVariables { #[serde(default)] scope: VaultScope },
    SetVariable { scope: VaultScope, name: String, value: String },
    DeleteVariable { scope: VaultScope, name: String },
    ListEnvironments { repo: String },
    SetEnvironment { environment: Environment },
    DeleteEnvironment { repo: String, name: String },
    /// Let a job waiting on its environment run, or turn it down.
    ApproveJob { repo: String, job: i64, approve: bool },
    ListArtifacts { repo: String, run: i64 },
    ArtifactData { repo: String, run: i64, path: String, #[serde(default)] offset: u64 },
    /// Snapshot the database now. Local only.
    Backup,
    /// The database snapshots: the server's own, or a workstation's copies of them. Local only.
    ListBackups,

    // --- The phone (server) ---
    /// Over TLS, once after pairing: the device's Secure Enclave public key
    /// (an uncompressed P-256 point, base64). From then on its merges,
    /// approvals and cancels must come wrapped in `Signed`.
    RegisterKey { public_key: String },
    /// Over TLS: where to push, and for what (`approvals`, `failures`); no token turns pushes off.
    RegisterPush {
        #[serde(default)]
        token: Option<String>,
        #[serde(default)]
        sandbox: bool,
        #[serde(default)]
        events: Vec<String>,
    },
    /// A request signed with the device's key: `signature` (DER, base64) is
    /// over `"{at}\n{nonce}\n{request}"`, where `request` is the inner
    /// request's JSON line. Good for five minutes, once.
    Signed { request: String, at: i64, nonce: String, signature: String },
    Inbox,
    /// The text behind a push's id.
    Notification { id: String },
    /// Store an APNs auth key (.p8 contents) to push with. Local only.
    PushSetup { key_p8: String, key_id: String, team_id: String, topic: String },
    PushInfo,
    /// Send a test push to every device that takes them. Local only.
    PushTest,

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
    Tree(TreeListing),
    TreeCommits(Vec<EntryCommit>),
    Blob(BlobContent),
    Overview(RepoOverview),
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
    VersionDiff(Box<VersionInterdiff>),
    Stacks(Vec<StackInfo>),
    StackSynced(StackSyncReport),
    Merged { sha: String },
    Compare(Vec<FileChange>),
    Rules(Vec<BranchRule>),
    Overrides(Vec<RuleOverride>),
    Runs(Vec<Run>),
    Run(Box<RunDetail>),
    JobLog(LogChunk),
    FrozenAttached(FrozenAttachment),
    BranchStacks(Vec<BranchStackStatus>),
    BranchStackUpdated(BranchStackStatus),
    Ci(CiInfo),
    /// Each workflow file on a branch: its name and either its triggers or what's wrong with it.
    Workflows(Vec<WorkflowFile>),
    Vault(VaultInfo),
    /// The vault's recovery code, shown once.
    RecoveryCode { code: String },
    Secrets(Vec<SecretInfo>),
    Variables(Vec<Variable>),
    Environments(Vec<Environment>),
    Artifacts(Vec<Artifact>),
    ArtifactChunk(ArtifactChunk),
    Mirrors(Vec<MirrorStatus>),
    Compacted(Vec<CompactReport>),
    Backups(BackupList),
    Inbox(Box<Inbox>),
    Notification(NotificationText),
    Push(PushInfo),
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
        let line = DiffLine { kind: LineKind::Added, old_line: None, new_line: Some(4), text: "x".into(), segments: vec![], no_newline: false, syntax: vec![] };
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
        let (_, import) = decode_request(r#"{"id":3,"method":"import_hosted","params":{"name":"flux","url":"git@github.com:a/flux.git"}}"#);
        assert_eq!(import.unwrap(), Request::ImportHosted { name: "flux".into(), url: "git@github.com:a/flux.git".into(), description: None, mirror: false });
        let (_, off) = decode_request(r#"{"id":4,"method":"set_mirror","params":{"repo":"flux"}}"#);
        assert_eq!(off.unwrap(), Request::SetMirror { repo: "flux".into(), url: None });
        assert_eq!(wire(&encode_request(5, &Request::ListBackups)), json!({"id": 5, "method": "list_backups"}));
        let (_, compact) = decode_request(r#"{"id":6,"method":"compact","params":{"aggressive":true}}"#);
        assert_eq!(compact.unwrap(), Request::Compact { repo: None, aggressive: true, prune_now: false });
        let report = Response::Compacted(vec![CompactReport {
            repo: "app".into(),
            before_bytes: 2000,
            after_bytes: 1000,
            loose_before: 10,
            loose_after: 0,
            packs_before: 2,
            packs_after: 1,
        }]);
        assert_eq!(decode_response(&encode_response(6, &report)).unwrap().1, report);
    }

    #[test]
    fn browse_shapes() {
        let tree = Request::Tree { repo: "x".into(), rev: None, path: "src".into() };
        assert_eq!(wire(&encode_request(1, &tree)), json!({"id": 1, "method": "tree", "params": {"repo": "x", "rev": null, "path": "src"}}));
        let (_, req) = decode_request(r#"{"id":2,"method":"tree_commits","params":{"repo":"x"}}"#);
        assert_eq!(req.unwrap(), Request::TreeCommits { repo: "x".into(), rev: None, path: String::new() });
        let (_, req) = decode_request(r#"{"id":3,"method":"blob","params":{"repo":"x","rev":"main","path":"a.rs"}}"#);
        assert_eq!(req.unwrap(), Request::Blob { repo: "x".into(), rev: Some("main".into()), path: "a.rs".into() });
        let (_, req) = decode_request(r#"{"id":4,"method":"overview","params":{"repo":"x"}}"#);
        assert_eq!(req.unwrap(), Request::Overview { repo: "x".into(), rev: None });

        let listing = Response::Tree(TreeListing {
            commit: "abc".into(),
            path: "src".into(),
            entries: vec![
                TreeEntry { name: "lib".into(), path: "src/lib".into(), kind: EntryKind::Dir, size: None },
                TreeEntry { name: "run.sh".into(), path: "src/run.sh".into(), kind: EntryKind::Executable, size: Some(12) },
            ],
        });
        assert_eq!(
            wire(&encode_response(1, &listing)),
            json!({"id": 1, "type": "tree", "data": {"commit": "abc", "path": "src", "entries": [
                {"name": "lib", "path": "src/lib", "kind": "dir", "size": null},
                {"name": "run.sh", "path": "src/run.sh", "kind": "executable", "size": 12}
            ]}})
        );
        let overview = Response::Overview(RepoOverview {
            commit: "abc".into(),
            commits: 3,
            commits_capped: false,
            contributors: 2,
            files: 10,
            languages: vec![LanguageShare { name: "Rust".into(), bytes: 300 }],
            readme: Some("README.md".into()),
            license: None,
        });
        assert_eq!(
            wire(&encode_response(5, &overview)),
            json!({"id": 5, "type": "overview", "data": {"commit": "abc", "commits": 3, "commits_capped": false, "contributors": 2, "files": 10,
                "languages": [{"name": "Rust", "bytes": 300}], "readme": "README.md", "license": null}})
        );
        let blob = Response::Blob(BlobContent { commit: "abc".into(), path: "a".into(), size: 2, binary: false, too_large: false, text: Some("a\n".into()), lines: 1, syntax: vec![] });
        assert_eq!(
            wire(&encode_response(6, &blob)),
            json!({"id": 6, "type": "blob", "data": {"commit": "abc", "path": "a", "size": 2, "binary": false, "too_large": false, "text": "a\n", "lines": 1}})
        );
        let commits = Response::TreeCommits(vec![EntryCommit { name: "lib".into(), sha: "abc".into(), subject: "s".into(), time: 9 }]);
        assert_eq!(
            wire(&encode_response(7, &commits)),
            json!({"id": 7, "type": "tree_commits", "data": [{"name": "lib", "sha": "abc", "subject": "s", "time": 9}]})
        );
        for r in [listing, overview, blob, commits] {
            assert_eq!(decode_response(&encode_response(1, &r)).unwrap().1, r);
        }
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

    /// The same line the Swift app's `runWire` test decodes.
    #[test]
    fn run_detail_wire() {
        let line = r#"{"id":4,"type":"run","data":{"run":{"id":7,"repo":"app","number":3,"workflow":"ci.yml","name":"ci","event":"pull_request","branch":"feature","sha":"abcdef1234","pull":2,"state":"failure","error":null,"created_at":100,"started_at":101,"finished_at":130},"jobs":[{"id":11,"run":7,"key":"test-2","name":"test (2)","executor":"graviton","image":"alpine:3.20","state":"failure","error":null,"needs":["build"],"attempt":1,"started_at":110,"finished_at":125,"steps":[{"index":0,"name":"echo test","state":"failure","exit_code":1,"log_start":40,"log_end":52,"started_at":111,"finished_at":112}]}],"warnings":["jobs.deploy.services is ignored for now"]}}"#;
        let (id, r) = decode_response(line).unwrap();
        let Response::Run(d) = r else { panic!("{r:?}") };
        assert_eq!((id, d.run.number, d.jobs[0].steps[0].exit_code), (4, 3, Some(1)));
        let (_, again) = decode_response(&encode_response(4, &Response::Run(d.clone()))).unwrap();
        assert_eq!(again, Response::Run(d));
    }

    #[test]
    fn wave2_semantic_interdiff_and_stack_wire() {
        let req = Request::CompareVersions { repo: "app".into(), number: 3, from_version: 1, to_version: 2 };
        let (_, back) = decode_request(&encode_request(10, &req));
        assert_eq!(back.unwrap(), req);

        let sync = Request::SyncStack { repo: "app".into(), pull: 5 };
        assert_eq!(wire(&encode_request(11, &sync)), json!({"id": 11, "method": "sync_stack", "params": {"repo": "app", "pull": 5}}));

        let stacks = Response::Stacks(vec![StackInfo {
            stack_id: "stack-1".into(),
            members: vec![StackMember {
                number: 1,
                title: "Base".into(),
                source: "s1".into(),
                target: "main".into(),
                state: PullState::Open,
                parent_pull: None,
                stack_conflict: None,
            }],
        }]);
        assert_eq!(decode_response(&encode_response(12, &stacks)).unwrap().1, stacks);

        let fd = FileDiff {
            path: "a.rs".into(),
            old_path: None,
            status: ChangeStatus::Modified,
            binary: false,
            too_large: false,
            additions: 1,
            deletions: 1,
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
                lines: vec![DiffLine {
                    kind: LineKind::Added,
                    old_line: None,
                    new_line: Some(1),
                    text: "fn foo() {}".into(),
                    segments: vec![],
                    no_newline: false,
                    syntax: vec![SyntaxSpan { start: 0, len: 2, kind: SyntaxKind::Keyword }],
                }],
                noise: Some(NoiseKind::Formatting),
            }],
            moves: vec![MoveBlock {
                id: 1,
                symbol: "foo".into(),
                kind: "function".into(),
                from_path: "b.rs".into(),
                from_line: 10,
                to_path: "a.rs".into(),
                to_line: 1,
                lines: 3,
                changed_lines: 0,
            }],
            semantic_summary: Some(SemanticSummary {
                moved_blocks: 1,
                formatting_hunks: 1,
                import_hunks: 0,
                changed_symbols: vec!["foo".into()],
            }),
        };
        let encoded = encode_response(13, &Response::FileDiff(fd.clone()));
        let (_, decoded) = decode_response(&encoded).unwrap();
        assert_eq!(decoded, Response::FileDiff(fd));
    }

    #[test]
    fn wave3_frozen_snapshot_wire() {
        let line = encode_request(14, &Request::AttachFrozenJob { repo: "app".into(), job: 77 });
        let (id, req) = decode_request(&line);
        assert_eq!((id, req.unwrap()), (Some(14), Request::AttachFrozenJob { repo: "app".into(), job: 77 }));

        let resp = Response::FrozenAttached(FrozenAttachment {
            job: 77,
            container_id: "c99".into(),
            container_name: "ntr-5-test-frozen-abc123".into(),
            workdir: "/workspace".into(),
            failed_step: Some("cargo test".into()),
            exit_code: Some(101),
            cli_command: "grav exec -it ntr-5-test-frozen-abc123 sh".into(),
            deep_link: "graviton://container/c99/terminal".into(),
        });
        let encoded = encode_response(14, &resp);
        assert_eq!(decode_response(&encoded).unwrap(), (14, resp));
    }

    #[test]
    fn wave4_branch_stacks_wire() {
        let req = Request::ThawBranchStack { repo: "app".into(), branch: "feat/auth".into() };
        let line = encode_request(15, &req);
        let (id, decoded_req) = decode_request(&line);
        assert_eq!((id, decoded_req.unwrap()), (Some(15), req));

        let status = BranchStackStatus {
            branch: "feat/auth".into(),
            key: "demo-app--feat-auth".into(),
            project: "demo-app".into(),
            has_checkpoint: true,
            running: true,
            services: vec!["db".into(), "api".into()],
            size_bytes: 16_384,
            updated_at: Some(1_700_000_000),
        };
        let resp = Response::BranchStacks(vec![status]);
        let encoded = encode_response(15, &resp);
        assert_eq!(decode_response(&encoded).unwrap(), (15, resp));
    }
}
