//! CI on the server: pushes and pull requests become runs of the workflows in
//! `.neutrino/workflows/`, and the scheduler runs their jobs within a fixed
//! budget of CPUs and memory, each in its own container.
//!
//! A job: check the commit out into `runs/<run>/<job>/workspace`, start a
//! container with the workspace shared in, `exec` each step, then stop and
//! remove the container and delete the workspace, whatever happened. Output
//! goes to `runs/<run>/<job>.log`; the database keeps where each step's part
//! starts and ends.

pub mod nexus;
pub mod runner;
mod upkeep;
mod vault;
mod workspace;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use neutrino_ci::expr::{self, Status};
use neutrino_ci::{Event, Executor, Workflow};
use neutrino_proto::{CiInfo, LogChunk, Run, RunState, WorkflowFile};
use neutrino_store::{NewJob, NewRun};
use serde_json::{Map, Value, json};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Notify, mpsc, watch};
use tokio::time::Instant;

use crate::Daemon;
use neutrino_git::gix;
pub use runner::{Graviton, Local, Runner};

/// What CI may use on this machine, and where jobs run.
#[derive(Clone)]
pub struct CiOptions {
    pub runner: Runner,
    pub cpus: u32,
    pub memory_mib: u64,
    /// How long a workflow's "minute" is: a minute, except in tests.
    pub minute: Duration,
    /// Caches kept, at most, in MiB; the least recently used go first.
    pub cache_mib: u64,
    /// Logs and artifacts of finished runs are kept this many days.
    pub retention_days: u32,
    /// Where nightly database snapshots go (`<state>/backups` when unset).
    pub backup_dir: Option<PathBuf>,
    /// Snapshots kept.
    pub backups_kept: usize,
    /// `runs-on: host`: steps as plain processes on this Mac, unisolated.
    /// Off unless the server is started with `--host-executor`.
    pub host: Option<Local>,
    /// `runs-on: nexus`: jobs sent to the Nexus coordinator.
    pub nexus: Option<nexus::Nexus>,
}

impl std::fmt::Debug for CiOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CiOptions {{ runner: {}, cpus: {}, memory_mib: {} }}", self.runner.name(), self.cpus, self.memory_mib)
    }
}

impl Default for CiOptions {
    fn default() -> Self {
        CiOptions {
            runner: Runner::Graviton(Graviton { socket: runner::graviton_socket() }),
            cpus: 4,
            memory_mib: 6 * 1024,
            minute: Duration::from_secs(60),
            cache_mib: 10 * 1024,
            retention_days: 30,
            backup_dir: None,
            backups_kept: 14,
            host: None,
            nexus: None,
        }
    }
}

struct Active {
    cpus: u32,
    memory_mib: u64,
    cancel: watch::Sender<bool>,
    container: Option<String>,
    services: Vec<String>,
}

pub struct Ci {
    options: CiOptions,
    active: Mutex<HashMap<i64, Active>>,
    wake: Notify,
    started: AtomicBool,
}

/// Log bytes kept in one read of a job's log, at most.
const MAX_LOG_READ: u64 = 256 * 1024;
const WORKFLOW_EXTENSIONS: &[&str] = &[".yml", ".yaml"];

impl Ci {
    pub fn new(options: CiOptions) -> Self {
        Ci { options, active: Mutex::new(HashMap::new()), wake: Notify::new(), started: AtomicBool::new(false) }
    }

    pub fn runner(&self) -> &Runner {
        &self.options.runner
    }

    fn used(&self) -> (u32, u64) {
        let a = self.active.lock().unwrap();
        (a.values().map(|j| j.cpus).sum(), a.values().map(|j| j.memory_mib).sum())
    }
}

/// The `neutrino` context of a run, from what the database keeps.
fn run_context(run: &Run) -> Value {
    let mut v = json!({
        "event": run.event,
        "sha": run.sha,
        "repository": run.repo,
        "run_number": run.number,
        "run_id": run.id,
        "workflow": run.name,
    });
    let o = v.as_object_mut().expect("object");
    if let Some(b) = &run.branch {
        o.insert("branch".into(), json!(b));
        o.insert("ref".into(), json!(format!("refs/heads/{b}")));
    }
    if let Some(n) = run.pull {
        o.insert("pull_request".into(), json!(n));
        o.insert("ref".into(), json!(format!("refs/pull/{n}/head")));
    }
    v
}

fn step_names(job: &neutrino_ci::Job, cx: &expr::Context) -> Vec<String> {
    job.steps
        .iter()
        .map(|s| match &s.name {
            Some(n) => expr::interpolate(n, cx).unwrap_or_else(|_| n.clone()),
            None => s.label(),
        })
        .collect()
}

fn env_list(map: &Map<String, Value>) -> Value {
    Value::Object(map.clone())
}

impl Daemon {
    fn ci(&self) -> Result<&Ci> {
        self.server_only()?;
        self.ci.as_ref().context("CI isn't running on this daemon")
    }

    fn runs_dir(&self) -> PathBuf {
        self.state_dir.join("runs")
    }

    fn job_paths(&self, run: i64, key: &str) -> (PathBuf, PathBuf) {
        let dir = self.runs_dir().join(run.to_string());
        (dir.join(key).join("workspace"), dir.join(format!("{key}.log")))
    }

    fn runs_changed(&self, repo: &str) {
        self.watchers.notify(neutrino_proto::RepoEvent { repo: repo.into(), kind: "runs".into() });
    }

    /// Every workflow file at `rev`, parsed.
    fn workflows_at(&self, repo: &gix::Repository, rev: &str) -> Result<Vec<(String, Result<Workflow, neutrino_ci::Error>)>> {
        let files = neutrino_git::forge::files_at(repo, rev, neutrino_ci::WORKFLOW_DIR, WORKFLOW_EXTENSIONS)?;
        Ok(files
            .into_iter()
            .map(|(name, bytes)| {
                let parsed = std::str::from_utf8(&bytes).map_err(|_| neutrino_ci::Error::new(format!("{name} isn't UTF-8"))).and_then(|s| Workflow::parse(&name, s));
                (name, parsed)
            })
            .collect())
    }

    /// Create one run of a workflow. A workflow that doesn't parse becomes a
    /// failed run that says why.
    #[allow(clippy::too_many_arguments)]
    fn create_run(&self, repo: &str, file: &str, parsed: Result<Workflow, neutrino_ci::Error>, event: &str, branch: Option<&str>, sha: &str, pull: Option<u32>) -> Result<i64> {
        let store = self.store()?;
        let mut new = NewRun {
            repo: repo.into(),
            workflow: file.into(),
            name: file.into(),
            event: event.into(),
            branch: branch.map(String::from),
            sha: sha.into(),
            pull,
            ..Default::default()
        };
        let wf = match parsed {
            Ok(wf) => wf,
            Err(e) => {
                new.error = Some(e.0);
                return store.create_run(&new, &[]);
            }
        };
        new.name = wf.name.clone();
        new.warnings = wf.warnings.clone();
        let probe = Run {
            id: 0,
            repo: repo.into(),
            number: 0,
            workflow: file.into(),
            name: wf.name.clone(),
            event: event.into(),
            branch: branch.map(String::from),
            sha: sha.into(),
            pull,
            state: RunState::Queued,
            error: None,
            created_at: 0,
            started_at: None,
            finished_at: None,
        };
        let vars = store.variables_for(repo, "")?;
        let base = expr::Context::default()
            .with("neutrino", run_context(&probe))
            .with("env", json!(wf.env))
            .with("vars", Value::Object(vars.into_iter().map(|(k, v)| (k, json!(v))).collect()));
        let planned = match neutrino_ci::plan(&wf, &base) {
            Ok(p) => p,
            Err(e) => {
                new.error = Some(format!("{file}: {e}"));
                return store.create_run(&new, &[]);
            }
        };
        let group = match &wf.concurrency {
            Some(c) => match expr::interpolate(&c.group, &base) {
                Ok(g) => Some((g, c.cancel_in_progress)),
                Err(e) => {
                    new.error = Some(format!("{file}: concurrency: {e}"));
                    return store.create_run(&new, &[]);
                }
            },
            None => None,
        };
        new.concurrency = group.as_ref().map(|(g, _)| g.clone());
        let jobs: Vec<NewJob> = planned
            .iter()
            .map(|p| {
                let def = wf.jobs.iter().find(|j| j.id == p.job_id).expect("planned from this workflow");
                let cx = base.clone().with("matrix", Value::Object(p.matrix.clone()));
                NewJob {
                    key: p.key.clone(),
                    job_id: p.job_id.clone(),
                    name: p.name.clone(),
                    matrix: Value::Object(p.matrix.clone()).to_string(),
                    needs: p.needs.clone(),
                    executor: def.runs_on.name().into(),
                    image: def.image.as_ref().map(|i| expr::interpolate(i, &cx).unwrap_or_else(|_| i.clone())),
                    // Services share the job's budget.
                    cpus: def.cpus + def.services.iter().map(|s| s.cpus).sum::<u32>(),
                    memory_mib: def.memory_mib + def.services.iter().map(|s| s.memory_mib).sum::<u64>(),
                    steps: step_names(def, &cx),
                    environment: def.environment.as_ref().map(|e| expr::interpolate(e, &cx).unwrap_or_else(|_| e.clone())),
                    concurrency: def.concurrency.as_ref().map(|c| expr::interpolate(&c.group, &cx).unwrap_or_else(|_| c.group.clone())),
                }
            })
            .collect();
        let id = store.create_run(&new, &jobs)?;
        // A new run in a group that cancels in progress stops the ones before it.
        if let Some((g, true)) = &group {
            for older in store.runs_in_group(repo, g, id)?.into_iter().filter(|o| *o < id) {
                let _ = self.cancel(repo, older);
            }
        }
        Ok(id)
    }

    /// The files changed from `from` to `to` (both sides of a rename), for
    /// `paths` filters; `None` when there's nothing to compare with.
    pub(crate) fn changed_files(&self, repo_id: &str, from: &str, to: &str) -> Option<Vec<String>> {
        if from.bytes().all(|b| b == b'0') {
            return None;
        }
        let repo = neutrino_git::local(&self.registry.get(repo_id).ok()?);
        let files = neutrino_git::forge::compare(&repo, from, to).ok()?;
        Some(files.into_iter().flat_map(|f| std::iter::once(f.path).chain(f.old_path)).collect())
    }

    /// Start the runs an event triggers. Returns their ids. `changed` is what
    /// the event changed, for `paths` filters.
    pub(crate) fn trigger(&self, repo_id: &str, event: &Event, changed: Option<&[String]>) -> Vec<i64> {
        let Some(ci) = self.ci.as_ref() else { return Vec::new() };
        let Ok(repo) = self.registry.get(repo_id).map(|r| neutrino_git::local(&r)) else { return Vec::new() };
        let files = match self.workflows_at(&repo, event.sha()) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("can't read workflows in {repo_id} at {}: {e:#}", event.sha());
                return Vec::new();
            }
        };
        let pull = match event {
            Event::PullRequest { number, .. } => Some(*number),
            _ => None,
        };
        let branch = match event {
            Event::Push { name, .. } => name.strip_prefix("refs/heads/").or(name.strip_prefix("refs/tags/")),
            _ => event.branch(),
        };
        let mut ids = Vec::new();
        for (file, parsed) in files {
            match &parsed {
                Ok(wf) if !wf.triggered_by_changes(event, changed) => continue,
                // A broken file shows up on pushes and pull requests, where you'd look.
                Err(_) if matches!(event, Event::Dispatch { .. }) => continue,
                _ => {}
            }
            match self.create_run(repo_id, &file, parsed, event.kind(), branch, event.sha(), pull) {
                Ok(id) => ids.push(id),
                Err(e) => eprintln!("can't create a run of {file} in {repo_id}: {e:#}"),
            }
        }
        if !ids.is_empty() {
            self.runs_changed(repo_id);
            ci.wake.notify_one();
        }
        ids
    }

    pub(crate) fn dispatch(&self, repo_id: &str, file: &str, branch: &str) -> Result<i64> {
        let ci = self.ci()?;
        let repo = neutrino_git::local(&self.registry.get(repo_id)?);
        let sha = neutrino_git::forge::branch_tip(&repo, branch)?.to_string();
        let (name, parsed) = self
            .workflows_at(&repo, &sha)?
            .into_iter()
            .find(|(n, _)| n == file || n.trim_end_matches(".yml").trim_end_matches(".yaml") == file)
            .with_context(|| format!("there's no {file} in {} on {branch}", neutrino_ci::WORKFLOW_DIR))?;
        let wf = parsed.map_err(|e| anyhow::anyhow!("{e}"))?;
        if !wf.on.dispatch {
            bail!("{name} can't be started by hand; add workflow_dispatch to its on:");
        }
        let id = self.create_run(repo_id, &name, Ok(wf), "workflow_dispatch", Some(branch), &sha, None)?;
        self.runs_changed(repo_id);
        ci.wake.notify_one();
        Ok(id)
    }

    pub(crate) fn rerun(&self, repo_id: &str, id: i64) -> Result<i64> {
        let ci = self.ci()?;
        let old = self.store()?.run(id)?;
        if old.repo != repo_id {
            bail!("run {id} isn't in {repo_id}");
        }
        let repo = neutrino_git::local(&self.registry.get(repo_id)?);
        let parsed = self
            .workflows_at(&repo, &old.sha)?
            .into_iter()
            .find(|(n, _)| *n == old.workflow)
            .map(|(_, p)| p)
            .with_context(|| format!("{} isn't in that commit any more", old.workflow))?;
        let new = self.create_run(repo_id, &old.workflow, parsed, &old.event, old.branch.as_deref(), &old.sha, old.pull)?;
        self.runs_changed(repo_id);
        ci.wake.notify_one();
        Ok(new)
    }

    pub(crate) fn cancel(&self, repo_id: &str, id: i64) -> Result<()> {
        let ci = self.ci()?;
        let store = self.store()?;
        let run = store.run(id)?;
        if run.repo != repo_id {
            bail!("run {id} isn't in {repo_id}");
        }
        if run.state.finished() {
            bail!("run {} has already finished", run.number);
        }
        for job in store.cancel_run(id)? {
            if let Some(a) = ci.active.lock().unwrap().get(&job) {
                let _ = a.cancel.send(true);
            }
        }
        self.runs_changed(repo_id);
        Ok(())
    }

    pub(crate) fn workflow_files(&self, repo_id: &str, branch: &str) -> Result<Vec<WorkflowFile>> {
        let repo = neutrino_git::local(&self.registry.get(repo_id)?);
        let sha = neutrino_git::forge::branch_tip(&repo, branch)?.to_string();
        Ok(self
            .workflows_at(&repo, &sha)?
            .into_iter()
            .map(|(file, parsed)| match parsed {
                Ok(wf) => {
                    let mut triggers = Vec::new();
                    if wf.on.push.is_some() {
                        triggers.push("push".into());
                    }
                    if wf.on.pull_request.is_some() {
                        triggers.push("pull_request".into());
                    }
                    if wf.on.dispatch {
                        triggers.push("workflow_dispatch".into());
                    }
                    if !wf.on.schedule.is_empty() {
                        triggers.push("schedule".into());
                    }
                    WorkflowFile { file, name: Some(wf.name), triggers, jobs: wf.jobs.iter().map(|j| j.id.clone()).collect(), error: None, warnings: wf.warnings }
                }
                Err(e) => WorkflowFile { file, name: None, triggers: vec![], jobs: vec![], error: Some(e.0), warnings: vec![] },
            })
            .collect())
    }

    pub(crate) fn job_log(&self, repo_id: &str, job: i64, offset: u64) -> Result<LogChunk> {
        let store = self.store()?;
        let j = store.job(job)?;
        let run = store.run(j.run)?;
        if run.repo != repo_id {
            bail!("job {job} isn't in {repo_id}");
        }
        let (_, log) = self.job_paths(j.run, &j.key);
        let (text, next, at_eof) = workspace::read_log_chunk(&log, offset, MAX_LOG_READ)?;
        let done = j.state.finished() && at_eof;
        Ok(LogChunk { job, offset, text, next, done })
    }

    /// Part of an artifact. Only files recorded as a run's artifacts are served.
    pub(crate) fn artifact_chunk(&self, repo_id: &str, run: i64, path: &str, offset: u64) -> Result<neutrino_proto::ArtifactChunk> {
        use base64::Engine as _;
        use std::io::{Read, Seek, SeekFrom};
        let store = self.store()?;
        if store.run(run)?.repo != repo_id {
            bail!("run {run} isn't in {repo_id}");
        }
        let artifact = store
            .artifacts(run)?
            .into_iter()
            .find(|a| format!("{}/{}", a.job, a.path) == path || a.path == path)
            .with_context(|| format!("run {run} has no artifact {path}"))?;
        workspace::validate_rel_path(&artifact.job)?;
        workspace::validate_rel_path(&artifact.path)?;
        let artifacts_root = self.state_dir.join("artifacts");
        let file = artifacts_root.join(run.to_string()).join(&artifact.job).join(&artifact.path);
        let canon_root = artifacts_root.canonicalize().with_context(|| format!("{path} is unavailable"))?;
        let canon_file = file.canonicalize().with_context(|| format!("{path} is unavailable"))?;
        if !canon_file.starts_with(&canon_root) {
            bail!("artifact {path} escapes artifacts directory");
        }
        let days = self.ci()?.options.retention_days;
        let mut f = std::fs::File::open(&canon_file).with_context(|| format!("{path} is gone (logs and artifacts are kept {days} days)"))?;
        let size = f.metadata()?.len();
        f.seek(SeekFrom::Start(offset.min(size)))?;
        let mut buf = Vec::new();
        f.take(512 * 1024).read_to_end(&mut buf)?;
        let next = offset.min(size) + buf.len() as u64;
        Ok(neutrino_proto::ArtifactChunk { path: path.into(), offset, size, data: base64::engine::general_purpose::STANDARD.encode(&buf), done: next >= size })
    }

    pub(crate) async fn ci_info(&self) -> Result<CiInfo> {
        let ci = self.ci()?;
        let (used_cpus, used_memory_mib) = ci.used();
        let ping = ci.options.runner.ping().await;
        let queued = self.store()?.queued_jobs()?.len() as u32;
        Ok(CiInfo {
            graviton: ping.is_ok(),
            graviton_error: ping.err().map(|e| format!("{e:#}")),
            budget_cpus: ci.options.cpus,
            budget_memory_mib: ci.options.memory_mib,
            used_cpus,
            used_memory_mib,
            running: ci.active.lock().unwrap().len() as u32,
            queued,
            host: ci.options.host.is_some(),
            nexus: ci.options.nexus.as_ref().map(|n| n.url.clone()),
        })
    }

    // -----------------------------------------------------------------------
    // The scheduler
    // -----------------------------------------------------------------------

    /// Start the scheduler and the reaper. Needs the Tokio runtime.
    pub(crate) fn start_ci(self: &Arc<Self>) {
        let Some(ci) = self.ci.as_ref() else { return };
        if ci.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            this.recover().await;
            this.reap().await;
            let mut last_reap = Instant::now();
            let mut last_prune: Option<Instant> = None;
            let mut last_minute = neutrino_store::now().div_euclid(60);
            loop {
                // Schedules, once per minute of wall-clock time.
                let minute = neutrino_store::now().div_euclid(60);
                if minute != last_minute {
                    last_minute = minute;
                    let t = Arc::clone(&this);
                    let _ = tokio::task::spawn_blocking(move || t.fire_schedules(minute * 60)).await;
                }
                if last_prune.is_none_or(|t| t.elapsed() >= Duration::from_secs(3600)) {
                    last_prune = Some(Instant::now());
                    let t = Arc::clone(&this);
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Err(e) = t.prune() {
                            eprintln!("CI upkeep: {e:#}");
                        }
                        if t.backup_due() {
                            match t.backup_now() {
                                Ok(p) => eprintln!("database snapshot: {}", p.display()),
                                Err(e) => eprintln!("database snapshot failed: {e:#}"),
                            }
                            t.maintain_repos();
                        }
                    })
                    .await;
                }
                if let Err(e) = this.schedule() {
                    eprintln!("CI scheduler: {e:#}");
                }
                let ci = this.ci.as_ref().expect("checked");
                tokio::select! {
                    _ = ci.wake.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
                if last_reap.elapsed() >= Duration::from_secs(60) {
                    this.reap().await;
                    last_reap = Instant::now();
                    let t = Arc::clone(&this);
                    let _ = tokio::task::spawn_blocking(move || t.mirror_upkeep()).await;
                }
            }
        });
    }

    /// After a restart, jobs marked running lost their runner: try each once more.
    async fn recover(self: &Arc<Self>) {
        let Ok(store) = self.store() else { return };
        let ci = self.ci.as_ref().expect("server");
        for (job, container, attempt) in store.running_jobs().unwrap_or_default() {
            if !container.is_empty() {
                ci.options.runner.stop(&container).await;
                ci.options.runner.remove(&container).await;
            }
            let Ok(j) = store.job(job) else { continue };
            let (_, log) = self.job_paths(j.run, &j.key);
            let note = if attempt < 2 { "the runner was lost (neutrinod restarted); trying again" } else { "the runner was lost again; giving up" };
            append_note(&log, note).await;
            if attempt < 2 {
                let _ = store.requeue_job(job);
            } else {
                let _ = store.finish_job(job, RunState::Failure, Some("runner lost"));
                let _ = store.settle_run(j.run);
            }
            if let Ok(run) = store.run(j.run) {
                self.runs_changed(&run.repo);
            }
        }
    }

    /// Remove containers and workspaces no live job owns.
    async fn reap(self: &Arc<Self>) {
        let Some(ci) = self.ci.as_ref() else { return };
        let live: Vec<String> = ci.active.lock().unwrap().values().flat_map(|a| a.container.iter().chain(a.services.iter()).cloned().collect::<Vec<_>>()).collect();
        let pending_names: Vec<i64> = ci.active.lock().unwrap().keys().copied().collect();
        if let Ok(owned) = ci.options.runner.owned().await {
            for c in owned {
                // A job that's starting has no container id yet, but its name says whose it is.
                let starting = pending_names.iter().any(|j| c.name.contains(&format!("-{j}-")));
                if live.contains(&c.id) || starting {
                    continue;
                }
                eprintln!("CI reaper: removing {}", c.name);
                ci.options.runner.stop(&c.id).await;
                ci.options.runner.remove(&c.id).await;
            }
        }
        let dir = self.runs_dir();
        let active_runs: Vec<i64> = {
            let store = self.store().ok();
            pending_names.iter().filter_map(|j| store.and_then(|s| s.job(*j).ok()).map(|j| j.run)).collect()
        };
        let _ = tokio::task::spawn_blocking(move || {
            let Ok(runs) = std::fs::read_dir(&dir) else { return };
            for run in runs.flatten() {
                let id: i64 = run.file_name().to_string_lossy().parse().unwrap_or(-1);
                if active_runs.contains(&id) {
                    continue;
                }
                let Ok(jobs) = std::fs::read_dir(run.path()) else { continue };
                for job in jobs.flatten() {
                    if job.file_type().is_ok_and(|t| t.is_dir()) {
                        let _ = workspace::remove_tree(&job.path());
                    }
                }
            }
        })
        .await;
    }

    /// Start every queued job whose turn it is.
    fn schedule(self: &Arc<Self>) -> Result<()> {
        let store = self.store()?;
        let ci = self.ci.as_ref().expect("server");
        let mut workflows: HashMap<i64, Option<Workflow>> = HashMap::new();
        for q in store.queued_jobs()? {
            let states: HashMap<String, RunState> = store.job_states(q.run)?.into_iter().collect();
            if q.needs.iter().any(|n| !states.get(n).is_some_and(|s| s.finished())) {
                continue;
            }
            let run = store.run(q.run)?;
            let wf = workflows
                .entry(q.run)
                .or_insert_with(|| {
                    let repo = self.registry.get(&run.repo).ok().map(|r| neutrino_git::local(&r))?;
                    self.workflows_at(&repo, &run.sha).ok()?.into_iter().find(|(n, _)| *n == run.workflow)?.1.ok()
                })
                .clone();
            let Some(wf) = wf else {
                store.finish_job(q.id, RunState::Failure, Some("the workflow file can't be read at this commit any more"))?;
                self.settled(q.run, &run.repo);
                continue;
            };
            let Some(def) = wf.jobs.iter().find(|j| j.id == q.job_id).cloned() else {
                store.finish_job(q.id, RunState::Failure, Some("the job isn't in the workflow any more"))?;
                self.settled(q.run, &run.repo);
                continue;
            };

            // What the jobs it needs did, by job id (a matrix fails if any entry did).
            let mut needs = Map::new();
            let mut status = Status::default();
            for n in &q.needs {
                let state = states.get(n).copied().unwrap_or(RunState::Skipped);
                status.failed |= state == RunState::Failure;
                status.cancelled |= state == RunState::Cancelled;
                let job_id = n.rsplit_once('-').filter(|(base, i)| i.parse::<u32>().is_ok() && wf.jobs.iter().any(|j| j.id == *base)).map(|(b, _)| b).unwrap_or(n);
                let entry = needs.entry(job_id.to_string()).or_insert_with(|| json!({ "result": "success" }));
                if state != RunState::Success && entry["result"] == "success" {
                    entry["result"] = json!(state.name());
                }
            }
            let matrix: Value = serde_json::from_str(&q.matrix).unwrap_or(json!({}));
            let mut cx = expr::Context::default()
                .with("neutrino", run_context(&run))
                .with("matrix", matrix)
                .with("needs", Value::Object(needs))
                .with("env", json!(wf.env));
            cx.status = status;
            let go = match &def.condition {
                Some(c) => match c.holds(&cx) {
                    Ok(b) => b,
                    Err(e) => {
                        store.finish_job(q.id, RunState::Failure, Some(&format!("if: {e}")))?;
                        self.settled(q.run, &run.repo);
                        continue;
                    }
                },
                None => !status.failed && !status.cancelled,
            };
            if !go {
                store.finish_job(q.id, RunState::Skipped, None)?;
                self.settled(q.run, &run.repo);
                continue;
            }
            // Concurrency: an older run in the same group goes first, and a
            // job's group runs one job at a time.
            if let Some(g) = &q.run_concurrency {
                if store.runs_in_group(&run.repo, g, q.run)?.into_iter().any(|o| o < q.run) {
                    continue;
                }
            }
            if let Some(g) = &q.concurrency {
                if store.job_group_busy(&run.repo, g, q.id)? {
                    continue;
                }
            }
            // Environments: which branches may deploy, and whether someone has to say yes.
            if let Some(env) = &q.environment {
                if let Some(e) = store.environment(&run.repo, env)? {
                    let allowed = e.branches.is_empty() || run.branch.as_deref().is_some_and(|b| e.branches.iter().any(|p| neutrino_ci::glob(p, b)));
                    if !allowed {
                        let why = format!("{} can't deploy to {env}; only {} can", run.branch.as_deref().unwrap_or("this ref"), e.branches.join(", "));
                        store.finish_job(q.id, RunState::Failure, Some(&why))?;
                        self.settled(q.run, &run.repo);
                        continue;
                    }
                    if e.require_approval && q.approved_by.is_none() {
                        store.set_waiting(q.id)?;
                        self.runs_changed(&run.repo);
                        let job_name = store.job(q.id).map(|j| j.name).unwrap_or_else(|_| q.key.clone());
                        self.notify(
                            "approval",
                            format!("{env} needs your approval"),
                            format!("{} run {} wants to deploy {job_name} from {} ({})", run.name, run.number, run.branch.as_deref().unwrap_or("a commit"), run.repo),
                            &run.repo,
                            Some(run.id),
                            run.pull,
                        );
                        continue;
                    }
                }
            }
            let unavailable = match def.runs_on {
                Executor::Graviton => None,
                Executor::Host if ci.options.host.is_none() => {
                    Some("this server doesn't run host jobs; they run unisolated on the server, so start neutrinod with --host-executor to allow them")
                }
                Executor::Nexus if ci.options.nexus.is_none() => Some("this server can't reach Nexus; start neutrinod with --nexus-token-file"),
                _ => None,
            };
            if let Some(why) = unavailable {
                store.finish_job(q.id, RunState::Failure, Some(why))?;
                self.settled(q.run, &run.repo);
                continue;
            }
            if def.runs_on != Executor::Graviton && !def.services.is_empty() {
                store.finish_job(q.id, RunState::Failure, Some("services run only beside graviton jobs"))?;
                self.settled(q.run, &run.repo);
                continue;
            }
            // A Nexus job runs on another machine: its resources are Nexus's to
            // place, so it neither needs nor takes this machine's CI budget.
            let (cpus, memory_mib) = if def.runs_on == Executor::Nexus { (0, 0) } else { (q.cpus, q.memory_mib) };
            if cpus > ci.options.cpus || memory_mib > ci.options.memory_mib {
                let why = format!(
                    "it asks for {} CPUs and {} MiB, more than CI's budget on this machine ({} CPUs, {} MiB)",
                    q.cpus, q.memory_mib, ci.options.cpus, ci.options.memory_mib
                );
                store.finish_job(q.id, RunState::Failure, Some(&why))?;
                self.settled(q.run, &run.repo);
                continue;
            }
            let (used_cpus, used_mem) = ci.used();
            if used_cpus + cpus > ci.options.cpus || used_mem + memory_mib > ci.options.memory_mib {
                // First come, first served: nothing jumps a job that's waiting for room.
                break;
            }
            let name = format!("{}{}-{}-{}", runner::CONTAINER_PREFIX, q.run, q.id, q.attempt);
            store.start_job(q.id, &name)?;
            let (cancel, cancelled) = watch::channel(false);
            ci.active.lock().unwrap().insert(q.id, Active { cpus, memory_mib, cancel, container: None, services: Vec::new() });
            self.runs_changed(&run.repo);
            let this = Arc::clone(self);
            tokio::spawn(async move {
                let repo = run.repo.clone();
                let job = q.id;
                if let Err(e) = this.execute(run, q, def, cx, name, cancelled).await {
                    eprintln!("CI job {job}: {e:#}");
                    if let Ok(store) = this.store() {
                        let _ = store.finish_job(job, RunState::Failure, Some(&format!("{e:#}")));
                    }
                }
                let ci = this.ci.as_ref().expect("server");
                ci.active.lock().unwrap().remove(&job);
                if let Ok(j) = this.store().and_then(|s| s.job(job)) {
                    this.settled(j.run, &repo);
                }
                ci.wake.notify_one();
            });
        }
        Ok(())
    }

    fn settled(self: &Arc<Self>, run: i64, repo: &str) {
        if let Ok(store) = self.store() {
            let before = store.run(run).map(|r| r.state).ok();
            if let (Ok(RunState::Failure), Some(b)) = (store.settle_run(run), before) {
                if !b.finished() {
                    if let Ok(r) = store.run(run) {
                        let at = match (r.pull, &r.branch) {
                            (Some(n), _) => format!("pull request #{n}"),
                            (None, Some(b)) => b.clone(),
                            _ => r.sha.chars().take(7).collect(),
                        };
                        self.notify("failure", format!("{} failed", r.name), format!("{repo} · {at} · run {}", r.number), repo, Some(run), r.pull);
                    }
                }
            }
        }
        self.runs_changed(repo);
    }

    async fn execute(
        self: &Arc<Self>,
        run: Run,
        q: neutrino_store::QueuedJob,
        def: neutrino_ci::Job,
        mut cx: expr::Context,
        name: String,
        mut cancelled: watch::Receiver<bool>,
    ) -> Result<()> {
        let store = self.store()?;
        let ci = self.ci.as_ref().expect("server");
        if def.runs_on == Executor::Nexus {
            return self.execute_nexus(run, q, def, cx, cancelled).await;
        }
        let runner = match def.runs_on {
            Executor::Host => Runner::Local(ci.options.host.clone().expect("checked when scheduled")),
            _ => ci.options.runner.clone(),
        };
        let (workspace, log_path) = self.job_paths(run.id, &q.key);
        let scratch = workspace.parent().map(Path::to_path_buf).unwrap_or_else(|| workspace.clone());
        let rm_scratch = |p: PathBuf| async move {
            let _ = tokio::task::spawn_blocking(move || workspace::remove_tree(&p)).await;
        };
        rm_scratch(scratch.clone()).await;
        tokio::fs::create_dir_all(&scratch).await?;
        let mut log = Log::open(&log_path).await?;
        if q.attempt > 1 {
            log.note(&format!("attempt {}", q.attempt)).await?;
        }

        // Check out.
        log.note(&format!("checking out {} ({})", &run.sha[..run.sha.len().min(10)], run.branch.as_deref().unwrap_or("detached"))).await?;
        let repo_handle = self.registry.get(&run.repo)?;
        let (sha, ws) = (run.sha.clone(), workspace.clone());
        let checkout = tokio::task::spawn_blocking(move || neutrino_git::forge::checkout_into(&neutrino_git::local(&repo_handle), &sha, &ws)).await?;
        if let Err(e) = checkout {
            log.note(&format!("checkout failed: {e}")).await?;
            store.finish_job(q.id, RunState::Failure, Some("checkout failed"))?;
            rm_scratch(scratch).await;
            return Ok(());
        }

        // Secrets and variables for this repo and environment. Values live only
        // in memory and in each step's own environment; the log masks them.
        let env_name = q.environment.clone().unwrap_or_default();
        let secrets = match self.secrets_for_job(&run.repo, &env_name) {
            Ok(s) => s,
            Err(e) => {
                log.note(&format!("can't open this job's secrets: {e:#}")).await?;
                store.finish_job(q.id, RunState::Failure, Some("secrets unavailable"))?;
                rm_scratch(scratch).await;
                return Ok(());
            }
        };
        log.masker = Some(neutrino_vault::Masker::new(secrets.iter().map(|(_, v)| v.as_str())));
        let vars = store.variables_for(&run.repo, &env_name)?;
        cx = cx
            .with("secrets", Value::Object(secrets.iter().map(|(k, v)| (k.clone(), json!(v))).collect()))
            .with("vars", Value::Object(vars.into_iter().map(|(k, v)| (k, json!(v))).collect()));
        cx.root = Some(workspace.clone());
        if let Some(e) = &q.environment {
            log.note(&format!("deploying to {e}{}", q.approved_by.as_ref().map(|b| format!(", approved by {b}")).unwrap_or_default())).await?;
        }

        // Caches: the exact key, else the newest match for a restore key.
        let mut caches: Vec<(String, Vec<String>, workspace::Restored)> = Vec::new();
        for c in &def.cache {
            let key = match expr::interpolate(&c.key, &cx) {
                Ok(k) if !k.trim().is_empty() => k,
                Ok(_) => {
                    log.note("a cache key came out empty; skipping that cache").await?;
                    continue;
                }
                Err(e) => {
                    log.note(&format!("cache key: {e}")).await?;
                    continue;
                }
            };
            let mut found = if store.use_cache(&run.repo, &key)? { Some((key.clone(), workspace::Restored::Hit)) } else { None };
            if found.is_none() {
                for prefix in &c.restore_keys {
                    let prefix = expr::interpolate(prefix, &cx).unwrap_or_else(|_| prefix.clone());
                    if let Some(k) = store.cache_with_prefix(&run.repo, &prefix)? {
                        store.use_cache(&run.repo, &k)?;
                        found = Some((k.clone(), workspace::Restored::From(k)));
                        break;
                    }
                }
            }
            let outcome = match found {
                Some((from, outcome)) => {
                    let dir = workspace::cache_dir(&self.state_dir, &run.repo, &from);
                    let (paths, ws) = (c.paths.clone(), workspace.clone());
                    match tokio::task::spawn_blocking(move || workspace::restore(&dir, &paths, &ws)).await? {
                        Ok(()) => {
                            log.note(&match &outcome {
                                workspace::Restored::Hit => format!("cache {key} restored"),
                                workspace::Restored::From(k) => format!("cache {key} not found; restored {k}"),
                                workspace::Restored::Miss => unreachable!(),
                            })
                            .await?;
                            outcome
                        }
                        Err(e) => {
                            log.note(&format!("couldn't restore cache {from}: {e:#}")).await?;
                            workspace::Restored::Miss
                        }
                    }
                }
                None => {
                    log.note(&format!("cache {key} not found")).await?;
                    workspace::Restored::Miss
                }
            };
            caches.push((key, c.paths.clone(), outcome));
        }

        // Services, started first so they're up when the steps begin.
        let mut services: Vec<String> = Vec::new();
        let mut service_env: Vec<(String, String)> = Vec::new();
        for svc in &def.services {
            let sb = runner::ServiceBox {
                name: format!("{name}-{}", svc.name),
                image: svc.image.clone(),
                env: svc.env.iter().map(|(k, v)| (k.clone(), expr::interpolate(v, &cx).unwrap_or_else(|_| v.clone()))).collect(),
                cpus: svc.cpus,
                memory_mib: svc.memory_mib,
            };
            log.note(&format!("starting service {} ({})", svc.name, svc.image)).await?;
            match runner.start_service(&sb).await {
                Ok((id, host)) => {
                    service_env.push((format!("{}_HOST", svc.name.to_ascii_uppercase()), host));
                    services.push(id);
                }
                Err(e) => {
                    log.note(&format!("couldn't start service {}: {e:#}", svc.name)).await?;
                    for id in &services {
                        runner.stop(id).await;
                        runner.remove(id).await;
                    }
                    store.finish_job(q.id, RunState::Failure, Some(&format!("service {} didn't start", svc.name)))?;
                    rm_scratch(scratch).await;
                    return Ok(());
                }
            }
        }
        if let Some(a) = ci.active.lock().unwrap().get_mut(&q.id) {
            a.services = services.clone();
        }

        // The container.
        let image = def.image.clone().map(|i| expr::interpolate(&i, &cx).unwrap_or(i)).unwrap_or_default();
        if def.runs_on == Executor::Host {
            log.note("running on this Mac, unisolated").await?;
        } else {
            log.note(&format!("starting {image} with {} CPUs and {} MiB on {}", def.cpus, def.memory_mib, runner.name())).await?;
        }
        let spec = runner::JobBox { name: name.clone(), image: image.clone(), cpus: def.cpus, memory_mib: def.memory_mib, workspace: workspace.clone() };
        let container = tokio::select! {
            started = runner.start(&spec) => started,
            _ = cancelled.changed() => Err(anyhow::anyhow!("cancelled")),
        };
        let container = match container {
            Ok(c) => c,
            Err(e) => {
                let cancel = *cancelled.borrow();
                log.note(&if cancel { "cancelled before the container started".to_string() } else { format!("couldn't start the container: {e:#}") }).await?;
                store.finish_job(q.id, if cancel { RunState::Cancelled } else { RunState::Failure }, if cancel { None } else { Some("couldn't start the container") })?;
                // It may have started after all; the reaper catches it otherwise.
                runner.stop(&name).await;
                runner.remove(&name).await;
                for id in &services {
                    runner.stop(id).await;
                    runner.remove(id).await;
                }
                rm_scratch(scratch).await;
                return Ok(());
            }
        };
        if let Some(a) = ci.active.lock().unwrap().get_mut(&q.id) {
            a.container = Some(container.clone());
        }

        // The environment every step starts with.
        let mut env: Vec<(String, String)> = vec![
            ("CI".into(), "true".into()),
            ("NEUTRINO".into(), "true".into()),
            ("NEUTRINO_EVENT".into(), run.event.clone()),
            ("NEUTRINO_SHA".into(), run.sha.clone()),
            ("NEUTRINO_REPOSITORY".into(), run.repo.clone()),
            ("NEUTRINO_RUN_NUMBER".into(), run.number.to_string()),
            ("NEUTRINO_JOB".into(), q.job_id.clone()),
            ("NEUTRINO_WORKSPACE".into(), runner::WORKSPACE.into()),
        ];
        if let Some(b) = &run.branch {
            env.push(("NEUTRINO_BRANCH".into(), b.clone()));
        }
        if let Some(n) = run.pull {
            env.push(("NEUTRINO_PULL_REQUEST".into(), n.to_string()));
        }
        env.extend(service_env.iter().cloned());
        let mut env_cx = Map::new();
        for (k, v) in cx.values.get("env").and_then(|v| v.as_object()).cloned().unwrap_or_default() {
            env_cx.insert(k, v);
        }
        for (k, v) in &def.env {
            let value = expr::interpolate(v, &cx.clone().with("env", env_list(&env_cx))).unwrap_or_else(|_| v.clone());
            env_cx.insert(k.clone(), json!(value));
        }
        for (k, v) in &env_cx {
            env.push((k.clone(), expr::to_text(v)));
        }
        cx = cx.with("env", Value::Object(env_cx.clone()));

        let minute = ci.options.minute;
        let job_deadline = Instant::now() + minute * def.timeout_minutes;
        let idle = def.idle_minutes.map(|m| minute * m);
        let (mut failed, mut stopped, mut was_cancelled) = (false, false, false);
        let mut job_error: Option<String> = None;

        for (i, step) in def.steps.iter().enumerate() {
            let i = i as u32;
            if stopped {
                break;
            }
            cx.status = Status { failed, cancelled: false };
            let runs = match &step.condition {
                Some(c) => c.holds(&cx).unwrap_or(false),
                None => !failed,
            };
            if !runs {
                store.finish_step(q.id, i, RunState::Skipped, None, log.offset)?;
                continue;
            }
            store.start_step(q.id, i, log.offset)?;
            self.runs_changed(&run.repo);
            let script = match expr::interpolate(&step.run, &cx) {
                Ok(s) => s,
                Err(e) => {
                    log.note(&format!("this step's run: has a bad expression: {e}")).await?;
                    store.finish_step(q.id, i, RunState::Failure, None, log.offset)?;
                    failed |= !step.continue_on_error;
                    continue;
                }
            };
            let mut step_env = env.clone();
            for (k, v) in &step.env {
                step_env.push((k.clone(), expr::interpolate(v, &cx).unwrap_or_else(|_| v.clone())));
            }
            let dir = step.working_directory.clone().unwrap_or_default();
            let deadline = match step.timeout_minutes {
                Some(m) => job_deadline.min(Instant::now() + minute * m),
                None => job_deadline,
            };
            let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
            let r = runner.clone();
            let c = container.clone();
            let exec = tokio::spawn(async move { r.exec(&c, &dir, &script, &step_env, tx).await });

            let mut last_output = Instant::now();
            let mut why_stopped: Option<String> = None;
            let mut since_flush = Instant::now();
            loop {
                let idle_at = idle.map(|d| last_output + d);
                tokio::select! {
                    chunk = rx.recv() => match chunk {
                        Some(bytes) => {
                            log.write(&bytes).await?;
                            last_output = Instant::now();
                            if since_flush.elapsed() > Duration::from_millis(500) {
                                store.step_log_end(q.id, i, log.offset)?;
                                since_flush = Instant::now();
                            }
                        }
                        None => break,
                    },
                    _ = tokio::time::sleep_until(deadline), if why_stopped.is_none() => {
                        why_stopped = Some(if deadline == job_deadline {
                            format!("the job ran past its {}-minute timeout", def.timeout_minutes)
                        } else {
                            format!("the step ran past its {}-minute timeout", step.timeout_minutes.unwrap_or(0))
                        });
                        runner.stop(&container).await;
                    }
                    _ = tokio::time::sleep_until(idle_at.unwrap_or(deadline)), if idle_at.is_some() && why_stopped.is_none() => {
                        why_stopped = Some(format!("the step printed nothing for {} minutes", def.idle_minutes.unwrap_or(0)));
                        runner.stop(&container).await;
                    }
                    _ = cancelled.changed(), if why_stopped.is_none() => {
                        if *cancelled.borrow() {
                            why_stopped = Some("cancelled".into());
                            was_cancelled = true;
                            runner.stop(&container).await;
                        }
                    }
                }
            }
            let result = exec.await?;
            log.flush().await?;
            let state = if let Some(why) = &why_stopped {
                stopped = true;
                log.note(why).await?;
                if was_cancelled {
                    RunState::Cancelled
                } else {
                    job_error = Some(why.clone());
                    RunState::Failure
                }
            } else {
                match &result {
                    Ok(0) => RunState::Success,
                    Ok(code) => {
                        log.note(&format!("exited with {code}")).await?;
                        RunState::Failure
                    }
                    Err(e) => {
                        log.note(&format!("the runner failed: {e:#}")).await?;
                        stopped = true;
                        job_error = Some("runner lost".into());
                        RunState::Failure
                    }
                }
            };
            store.finish_step(q.id, i, state, result.as_ref().ok().copied(), log.offset)?;
            if state == RunState::Failure && !step.continue_on_error {
                failed = true;
            }
            self.runs_changed(&run.repo);
        }

        let state = if was_cancelled {
            RunState::Cancelled
        } else if failed && !def.continue_on_error {
            RunState::Failure
        } else {
            RunState::Success
        };

        // Artifacts, whatever the steps did (a failed build's logs are worth keeping); not after a cancel.
        if !def.artifacts.is_empty() && !was_cancelled {
            let dest = self.state_dir.join("artifacts").join(run.id.to_string()).join(&q.key);
            let (ws, patterns) = (workspace.clone(), def.artifacts.clone());
            match tokio::task::spawn_blocking(move || workspace::collect(&ws, &patterns, &dest)).await? {
                Ok(kept) if kept.is_empty() => log.note("no files matched the artifacts").await?,
                Ok(kept) => {
                    let total: u64 = kept.iter().map(|(_, n)| n).sum();
                    for (path, size) in &kept {
                        store.add_artifact(run.id, &q.key, path, *size)?;
                    }
                    log.note(&format!("kept {} artifact file{} ({} KB)", kept.len(), if kept.len() == 1 { "" } else { "s" }, total.div_ceil(1024))).await?;
                }
                Err(e) => log.note(&format!("couldn't keep the artifacts: {e:#}")).await?,
            }
        }
        // Caches are saved after a job that passed, unless they were an exact hit.
        if state == RunState::Success {
            for (key, paths, outcome) in &caches {
                if *outcome == workspace::Restored::Hit {
                    continue;
                }
                let dir = workspace::cache_dir(&self.state_dir, &run.repo, key);
                let (p, ws) = (paths.clone(), workspace.clone());
                match tokio::task::spawn_blocking(move || workspace::save(&dir, &p, &ws)).await? {
                    Ok(size) => {
                        store.save_cache(&run.repo, key, size)?;
                        log.note(&format!("saved cache {key} ({} KB)", size.div_ceil(1024))).await?;
                    }
                    Err(e) => log.note(&format!("couldn't save cache {key}: {e:#}")).await?,
                }
            }
        }

        // Teardown runs whatever happened.
        runner.stop(&container).await;
        runner.remove(&container).await;
        for id in &services {
            runner.stop(id).await;
            runner.remove(id).await;
        }
        rm_scratch(scratch).await;
        log.note(&format!("job {}", state.name())).await?;
        drop(log);
        let lp = log_path.clone();
        let _ = tokio::task::spawn_blocking(move || workspace::compress_log(&lp)).await;
        store.finish_job(q.id, state, job_error.as_deref())?;
        Ok(())
    }
}

/// A job's log file, appended to.
pub(crate) struct Log {
    file: tokio::fs::File,
    pub(crate) offset: u64,
    at_line_start: bool,
    /// Replaces secrets in everything written.
    masker: Option<neutrino_vault::Masker>,
}

impl Log {
    pub(crate) async fn open(path: &Path) -> Result<Log> {
        let file = tokio::fs::OpenOptions::new().create(true).append(true).open(path).await.with_context(|| format!("opening {}", path.display()))?;
        let offset = file.metadata().await?.len();
        Ok(Log { file, offset, at_line_start: true, masker: None })
    }

    /// Output from a step: masked, so part of it may be held back until the next write.
    pub(crate) async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let masked = match self.masker.as_mut() {
            Some(m) => m.push(bytes),
            None => bytes.to_vec(),
        };
        self.raw(&masked).await
    }

    /// Whatever the masker held back.
    pub(crate) async fn flush(&mut self) -> Result<()> {
        if let Some(rest) = self.masker.as_mut().map(|m| m.flush()) {
            self.raw(&rest).await?;
        }
        Ok(())
    }

    async fn raw(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.file.write_all(bytes).await?;
        self.offset += bytes.len() as u64;
        self.at_line_start = bytes.ends_with(b"\n");
        Ok(())
    }

    /// A line from Neutrino itself, marked so the app can tell it apart.
    pub(crate) async fn note(&mut self, text: &str) -> Result<()> {
        self.flush().await?;
        let text = self.masker.as_ref().map(|m| m.mask(text)).unwrap_or_else(|| text.to_string());
        let lead = if self.at_line_start { "" } else { "\n" };
        self.raw(format!("{lead}\u{25b8} {text}\n").as_bytes()).await
    }
}

async fn append_note(path: &Path, text: &str) {
    if let Ok(mut log) = Log::open(path).await {
        let _ = log.note(text).await;
    }
}
