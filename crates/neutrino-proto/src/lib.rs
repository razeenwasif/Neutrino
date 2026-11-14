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
pub const PROTOCOL_VERSION: u32 = 1;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
        assert_eq!(wire(&line), json!({"id": 2, "type": "pong", "data": {"protocol": 1}}));
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
    fn round_trip() {
        let resp = Response::Error { message: "no such repo".into() };
        let (id, back) = decode_response(&encode_response(11, &resp)).unwrap();
        assert_eq!((id, back), (11, resp));
    }
}
