//! CI on the server: pushes and pull requests become runs of the workflows in
//! `.neutrino/workflows/`, and the scheduler runs their jobs within a fixed
//! budget of CPUs and memory, each in its own container.
//!
//! A job: check the commit out into `runs/<run>/<job>/workspace`, start a
//! container with the workspace shared in, `exec` each step, then stop and
//! remove the container and delete the workspace, whatever happened. Output
//! goes to `runs/<run>/<job>.log`; the database keeps where each step's part
//! starts and ends.

pub mod runner;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use neutrino_ci::expr::{self, Status};
use neutrino_ci::{Event, Executor, Workflow};
use neutrino_proto::{CiInfo, LogChunk, Run, RunState, WorkflowFile};
use neutrino_store::NewJob;
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
}

impl std::fmt::Debug for CiOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CiOptions {{ runner: {}, cpus: {}, memory_mib: {} }}", self.runner.name(), self.cpus, self.memory_mib)
    }
}

impl Default for CiOptions {
    fn default() -> Self {
        CiOptions { runner: Runner::Graviton(Graviton { socket: runner::graviton_socket() }), cpus: 4, memory_mib: 6 * 1024, minute: Duration::from_secs(60) }
    }
}

struct Active {
    cpus: u32,
    memory_mib: u64,
    cancel: watch::Sender<bool>,
    container: Option<String>,
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
        let wf = match parsed {
            Ok(wf) => wf,
            Err(e) => return store.create_run(repo, file, file, event, branch, sha, pull, &[], &[], Some(&e.0)),
        };
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
        let base = expr::Context::default().with("neutrino", run_context(&probe)).with("env", json!(wf.env));
        let planned = match neutrino_ci::plan(&wf, &base) {
            Ok(p) => p,
            Err(e) => return store.create_run(repo, file, &wf.name, event, branch, sha, pull, &[], &wf.warnings, Some(&format!("{file}: {e}"))),
        };
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
                    cpus: def.cpus,
                    memory_mib: def.memory_mib,
                    steps: step_names(def, &cx),
                }
            })
            .collect();
        store.create_run(repo, file, &wf.name, event, branch, sha, pull, &jobs, &wf.warnings, None)
    }

    /// Start the runs an event triggers. Returns their ids.
    pub(crate) fn trigger(&self, repo_id: &str, event: &Event) -> Vec<i64> {
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
                Ok(wf) if !wf.triggered_by(event) => continue,
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
        let (text, next) = match std::fs::File::open(&log) {
            Ok(mut f) => {
                use std::io::{Read, Seek, SeekFrom};
                let len = f.metadata()?.len();
                let start = offset.min(len);
                f.seek(SeekFrom::Start(start))?;
                let mut buf = Vec::new();
                f.take(MAX_LOG_READ).read_to_end(&mut buf)?;
                // Don't split a UTF-8 character at the end of a read.
                let end = match std::str::from_utf8(&buf) {
                    Err(e) if e.error_len().is_none() && e.valid_up_to() > 0 => e.valid_up_to(),
                    _ => buf.len(),
                };
                (String::from_utf8_lossy(&buf[..end]).into_owned(), start + end as u64)
            }
            Err(_) => (String::new(), offset),
        };
        let done = j.state.finished() && next >= std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
        Ok(LogChunk { job, offset, text, next, done })
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
            loop {
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
        let live: Vec<String> = ci.active.lock().unwrap().values().filter_map(|a| a.container.clone()).collect();
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
                    if job.path().join("workspace").exists() {
                        let _ = std::fs::remove_dir_all(job.path());
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
            if def.runs_on != Executor::Graviton {
                store.finish_job(q.id, RunState::Failure, Some(&format!("runs-on: {} arrives with milestone 7; use graviton for now", def.runs_on.name())))?;
                self.settled(q.run, &run.repo);
                continue;
            }
            if q.cpus > ci.options.cpus || q.memory_mib > ci.options.memory_mib {
                let why = format!(
                    "it asks for {} CPUs and {} MiB, more than CI's budget on this machine ({} CPUs, {} MiB)",
                    q.cpus, q.memory_mib, ci.options.cpus, ci.options.memory_mib
                );
                store.finish_job(q.id, RunState::Failure, Some(&why))?;
                self.settled(q.run, &run.repo);
                continue;
            }
            let (used_cpus, used_mem) = ci.used();
            if used_cpus + q.cpus > ci.options.cpus || used_mem + q.memory_mib > ci.options.memory_mib {
                // First come, first served: nothing jumps a job that's waiting for room.
                break;
            }
            let name = format!("{}{}-{}-{}", runner::CONTAINER_PREFIX, q.run, q.id, q.attempt);
            store.start_job(q.id, &name)?;
            let (cancel, cancelled) = watch::channel(false);
            ci.active.lock().unwrap().insert(q.id, Active { cpus: q.cpus, memory_mib: q.memory_mib, cancel, container: None });
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

    fn settled(&self, run: i64, repo: &str) {
        if let Ok(store) = self.store() {
            let _ = store.settle_run(run);
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
        let runner = ci.options.runner.clone();
        let (workspace, log_path) = self.job_paths(run.id, &q.key);
        if let Some(dir) = workspace.parent() {
            let _ = tokio::fs::remove_dir_all(dir).await;
            tokio::fs::create_dir_all(dir).await?;
        }
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
            let _ = tokio::fs::remove_dir_all(workspace.parent().unwrap_or(&workspace)).await;
            return Ok(());
        }

        // The container.
        let image = def.image.clone().map(|i| expr::interpolate(&i, &cx).unwrap_or(i)).unwrap_or_default();
        log.note(&format!("starting {image} with {} CPUs and {} MiB on {}", def.cpus, def.memory_mib, runner.name())).await?;
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
                let _ = tokio::fs::remove_dir_all(workspace.parent().unwrap_or(&workspace)).await;
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

        // Teardown runs whatever happened.
        runner.stop(&container).await;
        runner.remove(&container).await;
        let _ = tokio::fs::remove_dir_all(workspace.parent().unwrap_or(&workspace)).await;

        let state = if was_cancelled {
            RunState::Cancelled
        } else if failed && !def.continue_on_error {
            RunState::Failure
        } else {
            RunState::Success
        };
        log.note(&format!("job {}", state.name())).await?;
        store.finish_job(q.id, state, job_error.as_deref())?;
        Ok(())
    }
}

/// A job's log file, appended to.
struct Log {
    file: tokio::fs::File,
    offset: u64,
    at_line_start: bool,
}

impl Log {
    async fn open(path: &Path) -> Result<Log> {
        let file = tokio::fs::OpenOptions::new().create(true).append(true).open(path).await.with_context(|| format!("opening {}", path.display()))?;
        let offset = file.metadata().await?.len();
        Ok(Log { file, offset, at_line_start: true })
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.file.write_all(bytes).await?;
        self.offset += bytes.len() as u64;
        self.at_line_start = bytes.ends_with(b"\n");
        Ok(())
    }

    /// A line from Neutrino itself, marked so the app can tell it apart.
    async fn note(&mut self, text: &str) -> Result<()> {
        let lead = if self.at_line_start { "" } else { "\n" };
        self.write(format!("{lead}\u{25b8} {text}\n").as_bytes()).await
    }
}

async fn append_note(path: &Path, text: &str) {
    if let Ok(mut log) = Log::open(path).await {
        let _ = log.note(text).await;
    }
}
