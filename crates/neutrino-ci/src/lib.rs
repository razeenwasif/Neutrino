//! Neutrino CI, the part that needs no machine: workflow files, the `${{ }}`
//! expression language, which workflows an event triggers, and the jobs a run
//! is made of. The daemon schedules and executes what this plans.

pub mod expr;
pub mod workflow;

use std::fmt;

use serde_json::{Map, Value, json};

pub use workflow::{Executor, Job, Step, Workflow};

/// Where workflow files live in a repository.
pub const WORKFLOW_DIR: &str = ".neutrino/workflows";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// What starts a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A push to a branch or tag. `name` is the full ref.
    Push { name: String, sha: String },
    /// A pull request opened or given a new version.
    PullRequest { number: u32, source: String, target: String, sha: String },
    /// Started by hand on a branch.
    Dispatch { branch: String, sha: String },
}

impl Event {
    pub fn kind(&self) -> &'static str {
        match self {
            Event::Push { .. } => "push",
            Event::PullRequest { .. } => "pull_request",
            Event::Dispatch { .. } => "workflow_dispatch",
        }
    }

    pub fn sha(&self) -> &str {
        match self {
            Event::Push { sha, .. } | Event::PullRequest { sha, .. } | Event::Dispatch { sha, .. } => sha,
        }
    }

    /// The branch it's about: the pushed branch, the pull request's source, the dispatched branch.
    pub fn branch(&self) -> Option<&str> {
        match self {
            Event::Push { name, .. } => name.strip_prefix("refs/heads/"),
            Event::PullRequest { source, .. } => Some(source),
            Event::Dispatch { branch, .. } => Some(branch),
        }
    }

    /// The `neutrino` context.
    pub fn context(&self, repo: &str) -> Value {
        let mut v = json!({ "event": self.kind(), "sha": self.sha(), "repository": repo });
        let o = v.as_object_mut().expect("object");
        match self {
            Event::Push { name, .. } => {
                o.insert("ref".into(), json!(name));
                if let Some(b) = name.strip_prefix("refs/heads/") {
                    o.insert("branch".into(), json!(b));
                }
                if let Some(t) = name.strip_prefix("refs/tags/") {
                    o.insert("tag".into(), json!(t));
                }
            }
            Event::PullRequest { number, source, target, .. } => {
                o.insert("ref".into(), json!(format!("refs/pull/{number}/head")));
                o.insert("branch".into(), json!(source));
                o.insert("base_branch".into(), json!(target));
                o.insert("pull_request".into(), json!(number));
            }
            Event::Dispatch { branch, .. } => {
                o.insert("ref".into(), json!(format!("refs/heads/{branch}")));
                o.insert("branch".into(), json!(branch));
            }
        }
        v
    }
}

/// `*` matches within a path segment, `**` across them, `?` one character.
pub fn glob(pattern: &str, name: &str) -> bool {
    fn go(p: &[u8], s: &[u8]) -> bool {
        match p {
            [] => s.is_empty(),
            [b'*', b'*', rest @ ..] => (0..=s.len()).any(|i| go(rest, &s[i..])),
            [b'*', rest @ ..] => (0..=s.len()).take_while(|&i| i == 0 || s[i - 1] != b'/').any(|i| go(rest, &s[i..])),
            [b'?', rest @ ..] => !s.is_empty() && s[0] != b'/' && go(rest, &s[1..]),
            [c, rest @ ..] => s.first() == Some(c) && go(rest, &s[1..]),
        }
    }
    go(pattern.as_bytes(), name.as_bytes())
}

/// Whether `name` passes a list of patterns, where a later `!pattern` excludes.
fn passes(patterns: &[String], name: &str) -> bool {
    let mut ok = false;
    for p in patterns {
        if let Some(neg) = p.strip_prefix('!') {
            if glob(neg, name) {
                ok = false;
            }
        } else if glob(p, name) {
            ok = true;
        }
    }
    ok
}

impl workflow::RefFilter {
    fn branch(&self, branch: &str) -> bool {
        if !self.branches.is_empty() {
            return passes(&self.branches, branch);
        }
        !self.branches_ignore.iter().any(|p| glob(p, branch))
    }
}

impl Workflow {
    /// Whether this event starts it.
    pub fn triggered_by(&self, event: &Event) -> bool {
        match event {
            Event::Push { name, .. } => {
                let Some(f) = &self.on.push else { return false };
                if let Some(tag) = name.strip_prefix("refs/tags/") {
                    // Tags only when asked for, as in Actions.
                    return !f.tags.is_empty() && passes(&f.tags, tag);
                }
                match name.strip_prefix("refs/heads/") {
                    // A filter that lists only tags doesn't fire for branches.
                    Some(b) => (f.branches.is_empty() && f.branches_ignore.is_empty() && f.tags.is_empty()) || ((!f.branches.is_empty() || !f.branches_ignore.is_empty()) && f.branch(b)),
                    None => false,
                }
            }
            // Pull request filters are about the target branch.
            Event::PullRequest { target, .. } => self.on.pull_request.as_ref().is_some_and(|f| f.branch(target)),
            Event::Dispatch { .. } => self.on.dispatch,
        }
    }
}

/// One job of a run, after the matrix is expanded.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedJob {
    /// Unique in the run: the job id, with `-1`, `-2`… for matrix entries.
    pub key: String,
    pub job_id: String,
    pub name: String,
    pub matrix: Map<String, Value>,
    /// Keys of the planned jobs it waits for.
    pub needs: Vec<String>,
}

/// Values in the order the file lists the axes, then any keys an include added.
fn matrix_label(m: &Map<String, Value>, axes: &[(String, Vec<Value>)]) -> String {
    let mut parts: Vec<String> = axes.iter().filter_map(|(k, _)| m.get(k)).map(expr::to_text).collect();
    parts.extend(m.iter().filter(|(k, _)| !axes.iter().any(|(a, _)| a == *k)).map(|(_, v)| expr::to_text(v)));
    parts.join(", ")
}

/// The jobs a run of `wf` is made of, in an order where needs come first.
pub fn plan(wf: &Workflow, base: &expr::Context) -> Result<Vec<PlannedJob>, Error> {
    let mut by_job: Vec<(String, Vec<PlannedJob>)> = Vec::new();
    for job in &wf.jobs {
        let combos = job.matrix.as_ref().map(|m| m.expand()).unwrap_or_else(|| vec![Map::new()]);
        let mut planned = Vec::new();
        for (i, combo) in combos.iter().enumerate() {
            let cx = base.clone().with("matrix", Value::Object(combo.clone()));
            let name = match &job.name {
                Some(n) => expr::interpolate(n, &cx)?,
                None if combo.is_empty() => job.id.clone(),
                None => format!("{} ({})", job.id, matrix_label(combo, job.matrix.as_ref().map(|m| m.axes.as_slice()).unwrap_or(&[]))),
            };
            let key = if combos.len() == 1 { job.id.clone() } else { format!("{}-{}", job.id, i + 1) };
            planned.push(PlannedJob { key, job_id: job.id.clone(), name, matrix: combo.clone(), needs: Vec::new() });
        }
        by_job.push((job.id.clone(), planned));
    }
    let keys_of = |id: &str| -> Vec<String> { by_job.iter().find(|(j, _)| j == id).map(|(_, p)| p.iter().map(|p| p.key.clone()).collect()).unwrap_or_default() };
    let mut out = Vec::new();
    // Workflow::parse already refused cycles, so this order terminates.
    let mut placed: Vec<&str> = Vec::new();
    while placed.len() < wf.jobs.len() {
        for job in &wf.jobs {
            if placed.contains(&job.id.as_str()) || !job.needs.iter().all(|n| placed.contains(&n.as_str())) {
                continue;
            }
            let needs: Vec<String> = job.needs.iter().flat_map(|n| keys_of(n)).collect();
            for mut p in by_job.iter().find(|(j, _)| *j == job.id).map(|(_, p)| p.clone()).unwrap_or_default() {
                p.needs = needs.clone();
                out.push(p);
            }
            placed.push(&job.id);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
