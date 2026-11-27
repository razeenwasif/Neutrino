//! Runs, jobs and steps. Logs live in files beside the database; these rows
//! hold where each step's output starts and ends in them.

use anyhow::{Context, Result};
use neutrino_proto::{Run, RunDetail, RunJob, RunState, RunStep};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{Store, now};

/// A job as the planner made it, to insert with its run.
#[derive(Debug, Clone)]
pub struct NewJob {
    pub key: String,
    pub job_id: String,
    pub name: String,
    /// JSON object.
    pub matrix: String,
    pub needs: Vec<String>,
    pub executor: String,
    pub image: Option<String>,
    pub cpus: u32,
    pub memory_mib: u64,
    pub steps: Vec<String>,
    pub environment: Option<String>,
    /// Its concurrency group, already worked out.
    pub concurrency: Option<String>,
}

/// A run as the planner made it.
#[derive(Debug, Clone, Default)]
pub struct NewRun {
    pub repo: String,
    pub workflow: String,
    pub name: String,
    pub event: String,
    pub branch: Option<String>,
    pub sha: String,
    pub pull: Option<u32>,
    pub warnings: Vec<String>,
    /// A run that failed before starting (a workflow file that doesn't parse).
    pub error: Option<String>,
    /// The workflow's concurrency group, already worked out.
    pub concurrency: Option<String>,
}

/// A job waiting for its turn, with what the scheduler needs to decide.
#[derive(Debug, Clone)]
pub struct QueuedJob {
    pub id: i64,
    pub run: i64,
    pub repo: String,
    pub key: String,
    pub job_id: String,
    pub workflow: String,
    pub matrix: String,
    pub needs: Vec<String>,
    pub cpus: u32,
    pub memory_mib: u64,
    pub attempt: u32,
    pub environment: Option<String>,
    pub approved_by: Option<String>,
    pub concurrency: Option<String>,
    pub run_concurrency: Option<String>,
}

const RUN_COLUMNS: &str = "id, repo, number, workflow, name, event, branch, sha, pull, state, error, created_at, started_at, finished_at";

fn run_row(r: &Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: r.get(0)?,
        repo: r.get(1)?,
        number: r.get(2)?,
        workflow: r.get(3)?,
        name: r.get(4)?,
        event: r.get(5)?,
        branch: r.get(6)?,
        sha: r.get(7)?,
        pull: r.get(8)?,
        state: RunState::parse(&r.get::<_, String>(9)?),
        error: r.get(10)?,
        created_at: r.get(11)?,
        started_at: r.get(12)?,
        finished_at: r.get(13)?,
    })
}

fn steps_of(db: &Connection, job: i64) -> Result<Vec<RunStep>> {
    Ok(db
        .prepare("SELECT idx, name, state, exit_code, log_start, log_end, started_at, finished_at FROM steps WHERE job = ?1 ORDER BY idx")?
        .query_map(params![job], |r| {
            Ok(RunStep {
                index: r.get(0)?,
                name: r.get(1)?,
                state: RunState::parse(&r.get::<_, String>(2)?),
                exit_code: r.get(3)?,
                log_start: r.get::<_, i64>(4)? as u64,
                log_end: r.get::<_, i64>(5)? as u64,
                started_at: r.get(6)?,
                finished_at: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn job_row(db: &Connection, r: &Row<'_>) -> Result<RunJob> {
    let id: i64 = r.get(0)?;
    Ok(RunJob {
        id,
        run: r.get(1)?,
        key: r.get(2)?,
        name: r.get(3)?,
        executor: r.get(4)?,
        image: r.get(5)?,
        state: RunState::parse(&r.get::<_, String>(6)?),
        error: r.get(7)?,
        needs: serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or_default(),
        attempt: r.get(9)?,
        started_at: r.get(10)?,
        finished_at: r.get(11)?,
        environment: r.get(12)?,
        approved_by: r.get(13)?,
        steps: steps_of(db, id)?,
        frozen_snapshot_path: r.get(14)?,
        frozen_size_bytes: r.get::<_, Option<i64>>(15)?.map(|n| n.max(0) as u64),
        frozen_failed_step: r.get(16)?,
        frozen_exit_code: r.get(17)?,
    })
}

const JOB_COLUMNS: &str = "id, run, key, name, executor, image, state, error, needs, attempt, started_at, finished_at, environment, approved_by, frozen_snapshot_path, frozen_size_bytes, frozen_failed_step, frozen_exit_code";

impl Store {
    /// A run with its jobs, all queued; or, with `error`, a run that failed before starting.
    pub fn create_run(&self, new: &NewRun, jobs: &[NewJob]) -> Result<i64> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let number: u32 = tx.query_row("SELECT COALESCE(MAX(number), 0) + 1 FROM runs WHERE repo = ?1", params![new.repo], |r| r.get(0))?;
        let t = now();
        let state = if new.error.is_some() { RunState::Failure } else { RunState::Queued };
        tx.execute(
            "INSERT INTO runs (repo, number, workflow, name, event, branch, sha, pull, state, error, warnings, created_at, finished_at, concurrency)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                new.repo,
                number,
                new.workflow,
                new.name,
                new.event,
                new.branch,
                new.sha,
                new.pull,
                state.name(),
                new.error,
                serde_json::to_string(&new.warnings)?,
                t,
                new.error.as_ref().map(|_| t),
                new.concurrency
            ],
        )?;
        let run = tx.last_insert_rowid();
        for j in jobs {
            tx.execute(
                "INSERT INTO jobs (run, key, job_id, name, matrix, needs, executor, image, cpus, memory_mib, state, environment, concurrency)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'queued', ?11, ?12)",
                params![
                    run,
                    j.key,
                    j.job_id,
                    j.name,
                    j.matrix,
                    serde_json::to_string(&j.needs)?,
                    j.executor,
                    j.image,
                    j.cpus,
                    j.memory_mib as i64,
                    j.environment,
                    j.concurrency
                ],
            )?;
            let job = tx.last_insert_rowid();
            for (i, s) in j.steps.iter().enumerate() {
                tx.execute("INSERT INTO steps (job, idx, name, state) VALUES (?1, ?2, ?3, 'queued')", params![job, i as u32, s])?;
            }
        }
        tx.commit()?;
        Ok(run)
    }

    pub fn run(&self, id: i64) -> Result<Run> {
        let db = self.db.lock().unwrap();
        db.query_row(&format!("SELECT {RUN_COLUMNS} FROM runs WHERE id = ?1"), params![id], run_row).optional()?.with_context(|| format!("no run {id}"))
    }

    pub fn run_detail(&self, id: i64) -> Result<RunDetail> {
        let run = self.run(id)?;
        let db = self.db.lock().unwrap();
        let warnings: String = db.query_row("SELECT warnings FROM runs WHERE id = ?1", params![id], |r| r.get(0))?;
        let mut stmt = db.prepare(&format!("SELECT {JOB_COLUMNS} FROM jobs WHERE run = ?1 ORDER BY id"))?;
        let mut rows = stmt.query(params![id])?;
        let mut jobs = Vec::new();
        while let Some(r) = rows.next()? {
            jobs.push(job_row(&db, r)?);
        }
        Ok(RunDetail { run, jobs, warnings: serde_json::from_str(&warnings).unwrap_or_default() })
    }

    /// Newest first.
    pub fn runs(&self, repo: &str, pull: Option<u32>, limit: u32) -> Result<Vec<Run>> {
        let db = self.db.lock().unwrap();
        let sql = format!("SELECT {RUN_COLUMNS} FROM runs WHERE repo = ?1 AND (?2 IS NULL OR pull = ?2) ORDER BY id DESC LIMIT ?3");
        Ok(db.prepare(&sql)?.query_map(params![repo, pull, limit.clamp(1, 500)], run_row)?.collect::<rusqlite::Result<_>>()?)
    }

    /// The latest run of each workflow for one commit: a pull request's checks.
    pub fn runs_for_sha(&self, repo: &str, sha: &str) -> Result<Vec<Run>> {
        let db = self.db.lock().unwrap();
        let sql = format!("SELECT {RUN_COLUMNS} FROM runs WHERE id IN (SELECT MAX(id) FROM runs WHERE repo = ?1 AND sha = ?2 GROUP BY workflow) ORDER BY workflow");
        Ok(db.prepare(&sql)?.query_map(params![repo, sha], run_row)?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn job(&self, id: i64) -> Result<RunJob> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(&format!("SELECT {JOB_COLUMNS} FROM jobs WHERE id = ?1"))?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(r) => job_row(&db, r),
            None => anyhow::bail!("no job {id}"),
        }
    }

    /// The job's matrix (JSON) and the job id in its workflow.
    pub fn job_plan(&self, id: i64) -> Result<(String, String)> {
        let db = self.db.lock().unwrap();
        Ok(db.query_row("SELECT job_id, matrix FROM jobs WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))?)
    }

    /// Queued jobs, oldest run first: pull requests' and pushes' in the order they came.
    pub fn queued_jobs(&self) -> Result<Vec<QueuedJob>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare(
                "SELECT j.id, j.run, r.repo, j.key, j.job_id, r.workflow, j.matrix, j.needs, j.cpus, j.memory_mib, j.attempt,
                        j.environment, j.approved_by, j.concurrency, r.concurrency
                 FROM jobs j JOIN runs r ON r.id = j.run WHERE j.state = 'queued' ORDER BY j.run, j.id",
            )?
            .query_map([], |r| {
                Ok(QueuedJob {
                    id: r.get(0)?,
                    run: r.get(1)?,
                    repo: r.get(2)?,
                    key: r.get(3)?,
                    job_id: r.get(4)?,
                    workflow: r.get(5)?,
                    matrix: r.get(6)?,
                    needs: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default(),
                    cpus: r.get(8)?,
                    memory_mib: r.get::<_, i64>(9)? as u64,
                    attempt: r.get(10)?,
                    environment: r.get(11)?,
                    approved_by: r.get(12)?,
                    concurrency: r.get(13)?,
                    run_concurrency: r.get(14)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Job ids that are running: after a restart, these lost their runner.
    pub fn running_jobs(&self) -> Result<Vec<(i64, String, u32)>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare("SELECT id, COALESCE(container, ''), attempt FROM jobs WHERE state = 'running'")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// The states of a run's jobs, by key.
    pub fn job_states(&self, run: i64) -> Result<Vec<(String, RunState)>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare("SELECT key, state FROM jobs WHERE run = ?1")?
            .query_map(params![run], |r| Ok((r.get(0)?, RunState::parse(&r.get::<_, String>(1)?))))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn start_job(&self, job: i64, container: &str) -> Result<()> {
        let db = self.db.lock().unwrap();
        let t = now();
        db.execute("UPDATE jobs SET state = 'running', container = ?2, started_at = ?3, error = NULL WHERE id = ?1", params![job, container, t])?;
        db.execute(
            "UPDATE runs SET state = 'running', started_at = COALESCE(started_at, ?2) WHERE id = (SELECT run FROM jobs WHERE id = ?1) AND state = 'queued'",
            params![job, t],
        )?;
        Ok(())
    }

    /// Finish a job; steps that never started take `rest` (skipped or cancelled).
    pub fn finish_job(&self, job: i64, state: RunState, error: Option<&str>) -> Result<()> {
        let db = self.db.lock().unwrap();
        let t = now();
        db.execute("UPDATE jobs SET state = ?2, error = ?3, finished_at = ?4 WHERE id = ?1", params![job, state.name(), error, t])?;
        let rest = if state == RunState::Cancelled { RunState::Cancelled } else { RunState::Skipped };
        db.execute("UPDATE steps SET state = ?2 WHERE job = ?1 AND state = 'queued'", params![job, rest.name()])?;
        db.execute("UPDATE steps SET state = ?2, finished_at = ?3 WHERE job = ?1 AND state = 'running'", params![job, state.name(), t])?;
        Ok(())
    }

    /// Put a job back in the queue (a lost runner's first time).
    pub fn requeue_job(&self, job: i64) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "UPDATE jobs SET state = 'queued', attempt = attempt + 1, container = NULL, started_at = NULL, error = NULL WHERE id = ?1",
            params![job],
        )?;
        db.execute("UPDATE steps SET state = 'queued', exit_code = NULL, log_start = 0, log_end = 0, started_at = NULL, finished_at = NULL WHERE job = ?1", params![job])?;
        Ok(())
    }

    pub fn start_step(&self, job: i64, index: u32, log_start: u64) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "UPDATE steps SET state = 'running', log_start = ?3, log_end = ?3, started_at = ?4 WHERE job = ?1 AND idx = ?2",
            params![job, index, log_start as i64, now()],
        )?;
        Ok(())
    }

    pub fn step_log_end(&self, job: i64, index: u32, log_end: u64) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute("UPDATE steps SET log_end = ?3 WHERE job = ?1 AND idx = ?2", params![job, index, log_end as i64])?;
        Ok(())
    }

    pub fn finish_step(&self, job: i64, index: u32, state: RunState, exit_code: Option<i32>, log_end: u64) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "UPDATE steps SET state = ?3, exit_code = ?4, log_end = ?5, finished_at = ?6, started_at = COALESCE(started_at, ?6) WHERE job = ?1 AND idx = ?2",
            params![job, index, state.name(), exit_code, log_end as i64, now()],
        )?;
        Ok(())
    }

    /// Settle a run from its jobs once they've all finished. Returns its state.
    pub fn settle_run(&self, run: i64) -> Result<RunState> {
        let states: Vec<RunState> = self.job_states(run)?.into_iter().map(|(_, s)| s).collect();
        let current = self.run(run)?.state;
        if current.finished() {
            return Ok(current);
        }
        if states.iter().any(|s| !s.finished()) {
            return Ok(current);
        }
        let state = if states.contains(&RunState::Cancelled) {
            RunState::Cancelled
        } else if states.contains(&RunState::Failure) {
            RunState::Failure
        } else if states.iter().all(|s| *s == RunState::Skipped) && !states.is_empty() {
            RunState::Skipped
        } else {
            RunState::Success
        };
        let db = self.db.lock().unwrap();
        db.execute("UPDATE runs SET state = ?2, finished_at = ?3 WHERE id = ?1", params![run, state.name(), now()])?;
        Ok(state)
    }

    /// Cancel a run's queued jobs; returns the running ones, which the scheduler has to stop.
    pub fn cancel_run(&self, run: i64) -> Result<Vec<i64>> {
        let running: Vec<i64> = {
            let db = self.db.lock().unwrap();
            let t = now();
            db.execute(
                "UPDATE steps SET state = 'cancelled' WHERE state = 'queued' AND job IN (SELECT id FROM jobs WHERE run = ?1 AND state IN ('queued', 'waiting'))",
                params![run],
            )?;
            db.execute("UPDATE jobs SET state = 'cancelled', finished_at = ?2 WHERE run = ?1 AND state IN ('queued', 'waiting')", params![run, t])?;
            db.prepare("SELECT id FROM jobs WHERE run = ?1 AND state = 'running'")?.query_map(params![run], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
        };
        if running.is_empty() {
            self.settle_run(run)?;
        }
        Ok(running)
    }
}

impl Store {
    /// Name what a job runs in (a container, or `nexus:<id>`).
    pub fn set_container(&self, job: i64, container: &str) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute("UPDATE jobs SET container = ?2 WHERE id = ?1", params![job, container])?;
        Ok(())
    }

    /// Park a job until someone approves its environment.
    pub fn set_waiting(&self, job: i64) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute("UPDATE jobs SET state = 'waiting' WHERE id = ?1 AND state = 'queued'", params![job])?;
        db.execute("UPDATE runs SET state = 'waiting' WHERE id = (SELECT run FROM jobs WHERE id = ?1) AND state IN ('queued', 'running')", params![job])?;
        Ok(())
    }

    /// Let a waiting job run (it goes back in the queue, approved) or turn it down.
    pub fn decide(&self, job: i64, approve: bool, by: &str) -> Result<()> {
        let state: String = {
            let db = self.db.lock().unwrap();
            db.query_row("SELECT state FROM jobs WHERE id = ?1", params![job], |r| r.get(0))?
        };
        if state != "waiting" {
            anyhow::bail!("job {job} isn't waiting for approval");
        }
        if approve {
            let db = self.db.lock().unwrap();
            db.execute("UPDATE jobs SET state = 'queued', approved_by = ?2 WHERE id = ?1", params![job, by])?;
            db.execute(
                "UPDATE runs SET state = CASE WHEN started_at IS NULL THEN 'queued' ELSE 'running' END WHERE id = (SELECT run FROM jobs WHERE id = ?1)",
                params![job],
            )?;
        } else {
            self.finish_job(job, RunState::Failure, Some(&format!("turned down by {by}")))?;
            let db = self.db.lock().unwrap();
            db.execute("UPDATE jobs SET approved_by = ?2 WHERE id = ?1", params![job, by])?;
        }
        Ok(())
    }

    /// Waiting jobs, for the scheduler to put back when a run's state changes.
    pub fn waiting_runs(&self) -> Result<Vec<i64>> {
        let db = self.db.lock().unwrap();
        Ok(db.prepare("SELECT DISTINCT run FROM jobs WHERE state = 'waiting'")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    /// Unfinished runs in a concurrency group, other than `except`, oldest first.
    pub fn runs_in_group(&self, repo: &str, group: &str, except: i64) -> Result<Vec<i64>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare("SELECT id FROM runs WHERE repo = ?1 AND concurrency = ?2 AND id != ?3 AND state IN ('queued', 'running', 'waiting') ORDER BY id")?
            .query_map(params![repo, group, except], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Whether a job in this group is running (in this repo), other than `except`.
    pub fn job_group_busy(&self, repo: &str, group: &str, except: i64) -> Result<bool> {
        let db = self.db.lock().unwrap();
        let n: i64 = db.query_row(
            "SELECT COUNT(*) FROM jobs j JOIN runs r ON r.id = j.run WHERE r.repo = ?1 AND j.concurrency = ?2 AND j.id != ?3 AND j.state = 'running'",
            params![repo, group, except],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    pub fn add_artifact(&self, run: i64, job: &str, path: &str, size: u64) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO artifacts (run, job, path, size) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (run, job, path) DO UPDATE SET size = ?4",
            params![run, job, path, size as i64],
        )?;
        Ok(())
    }

    pub fn artifacts(&self, run: i64) -> Result<Vec<neutrino_proto::Artifact>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare("SELECT run, job, path, size FROM artifacts WHERE run = ?1 ORDER BY job, path")?
            .query_map(params![run], |r| Ok(neutrino_proto::Artifact { run: r.get(0)?, job: r.get(1)?, path: r.get(2)?, size: r.get::<_, i64>(3)? as u64 }))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn forget_artifacts(&self, run: i64) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute("DELETE FROM artifacts WHERE run = ?1", params![run])?;
        Ok(())
    }

    /// A cache's key, if there's one by that name; marks it used.
    pub fn use_cache(&self, repo: &str, key: &str) -> Result<bool> {
        let db = self.db.lock().unwrap();
        Ok(db.execute("UPDATE caches SET last_used = ?3 WHERE repo = ?1 AND key = ?2", params![repo, key, now()])? > 0)
    }

    /// The newest cache whose key starts with `prefix`.
    pub fn cache_with_prefix(&self, repo: &str, prefix: &str) -> Result<Option<String>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .query_row(
                "SELECT key FROM caches WHERE repo = ?1 AND substr(key, 1, length(?2)) = ?2 ORDER BY created_at DESC LIMIT 1",
                params![repo, prefix],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn save_cache(&self, repo: &str, key: &str, size: u64) -> Result<()> {
        let db = self.db.lock().unwrap();
        let t = now();
        db.execute(
            "INSERT INTO caches (repo, key, size, created_at, last_used) VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT (repo, key) DO UPDATE SET size = ?3, last_used = ?4",
            params![repo, key, size as i64, t],
        )?;
        Ok(())
    }

    /// Caches, least recently used first: what to evict.
    pub fn caches_by_use(&self) -> Result<Vec<(String, String, u64)>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare("SELECT repo, key, size FROM caches ORDER BY last_used")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn drop_cache(&self, repo: &str, key: &str) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute("DELETE FROM caches WHERE repo = ?1 AND key = ?2", params![repo, key])?;
        Ok(())
    }

    /// Finished runs older than `before` (Unix seconds), oldest first.
    pub fn finished_before(&self, before: i64) -> Result<Vec<i64>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare("SELECT id FROM runs WHERE finished_at IS NOT NULL AND finished_at < ?1 ORDER BY id")?
            .query_map(params![before], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_job_frozen_snapshot(
        &self,
        job: i64,
        path: &str,
        size_bytes: u64,
        failed_step: Option<&str>,
        exit_code: Option<i32>,
        expires_at: i64,
    ) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "UPDATE jobs SET frozen_snapshot_path = ?1, frozen_size_bytes = ?2, frozen_failed_step = ?3, frozen_exit_code = ?4, frozen_expires_at = ?5 WHERE id = ?6",
            params![path, size_bytes as i64, failed_step, exit_code, expires_at, job],
        )?;
        Ok(())
    }

    pub fn clear_job_frozen_snapshot(&self, job: i64) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "UPDATE jobs SET frozen_snapshot_path = NULL, frozen_size_bytes = NULL, frozen_failed_step = NULL, frozen_exit_code = NULL, frozen_expires_at = NULL WHERE id = ?1",
            params![job],
        )?;
        Ok(())
    }

    pub fn expired_frozen_snapshots(&self, before: i64) -> Result<Vec<(i64, String)>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare("SELECT id, frozen_snapshot_path FROM jobs WHERE frozen_snapshot_path IS NOT NULL AND frozen_expires_at IS NOT NULL AND frozen_expires_at <= ?1 ORDER BY frozen_expires_at")?
            .query_map(params![before], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn frozen_snapshots_oldest_first(&self) -> Result<Vec<(i64, String, u64)>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare("SELECT id, frozen_snapshot_path, COALESCE(frozen_size_bytes, 0) FROM jobs WHERE frozen_snapshot_path IS NOT NULL ORDER BY COALESCE(finished_at, id), id")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)?.max(0) as u64)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// The minute a schedule last fired, so a restart doesn't fire it twice.
    pub fn schedule_mark(&self, repo: &str, workflow: &str, cron: &str) -> Result<Option<i64>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .query_row("SELECT minute FROM schedule_marks WHERE repo = ?1 AND workflow = ?2 AND cron = ?3", params![repo, workflow, cron], |r| r.get(0))
            .optional()?)
    }

    pub fn set_schedule_mark(&self, repo: &str, workflow: &str, cron: &str, minute: i64) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO schedule_marks (repo, workflow, cron, minute) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (repo, workflow, cron) DO UPDATE SET minute = ?4",
            params![repo, workflow, cron, minute],
        )?;
        Ok(())
    }

    /// A consistent, compacted copy of the database at `dest`, taken while it's
    /// in use. Uses `VACUUM INTO` for a zero-free-page snapshot, falling back
    /// to SQLite's online backup API if needed.
    pub fn backup_to(&self, dest: &std::path::Path) -> Result<()> {
        let db = self.db.lock().unwrap();
        if let Some(s) = dest.to_str() {
            if db.execute("VACUUM INTO ?1", params![s]).is_ok() {
                return Ok(());
            }
            let _ = std::fs::remove_file(dest);
        }
        let mut out = Connection::open(dest)?;
        let backup = rusqlite::backup::Backup::new(&db, &mut out)?;
        backup.run_to_completion(256, std::time::Duration::from_millis(0), None)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(key: &str, needs: &[&str]) -> NewJob {
        NewJob {
            key: key.into(),
            job_id: key.into(),
            name: key.into(),
            matrix: "{}".into(),
            needs: needs.iter().map(|s| s.to_string()).collect(),
            executor: "graviton".into(),
            image: Some("alpine:3.20".into()),
            cpus: 2,
            memory_mib: 1024,
            steps: vec!["one".into(), "two".into()],
            environment: None,
            concurrency: None,
        }
    }

    fn new(workflow: &str, event: &str, pull: Option<u32>, error: Option<&str>) -> NewRun {
        NewRun {
            repo: "app".into(),
            workflow: workflow.into(),
            name: workflow.trim_end_matches(".yml").into(),
            event: event.into(),
            branch: Some("main".into()),
            sha: "abc".into(),
            pull,
            error: error.map(String::from),
            ..Default::default()
        }
    }

    #[test]
    fn a_run_from_queue_to_finish() {
        let s = Store::in_memory().unwrap();
        let run = s.create_run(&new("ci.yml", "push", None, None), &[job("build", &[]), job("test", &["build"])]).unwrap();
        let queued = s.queued_jobs().unwrap();
        assert_eq!(queued.iter().map(|q| q.key.as_str()).collect::<Vec<_>>(), vec!["build", "test"]);
        assert_eq!(queued[1].needs, vec!["build"]);

        let build = queued[0].id;
        s.start_job(build, "ntr-1-1").unwrap();
        assert_eq!(s.run(run).unwrap().state, RunState::Running);
        s.start_step(build, 0, 0).unwrap();
        s.finish_step(build, 0, RunState::Success, Some(0), 120).unwrap();
        s.start_step(build, 1, 120).unwrap();
        s.finish_job(build, RunState::Failure, Some("step two failed")).unwrap();
        let d = s.run_detail(run).unwrap();
        assert_eq!(d.jobs[0].steps.iter().map(|st| st.state).collect::<Vec<_>>(), vec![RunState::Success, RunState::Failure]);
        assert_eq!((d.jobs[0].steps[0].log_start, d.jobs[0].steps[0].log_end), (0, 120));
        assert_eq!(s.settle_run(run).unwrap(), RunState::Running, "test hasn't finished");

        let test = queued[1].id;
        s.finish_job(test, RunState::Skipped, None).unwrap();
        assert_eq!(s.run_detail(run).unwrap().jobs[1].steps[0].state, RunState::Skipped);
        assert_eq!(s.settle_run(run).unwrap(), RunState::Failure);
        assert!(s.run(run).unwrap().finished_at.is_some());
    }

    #[test]
    fn cancel_requeue_and_checks() {
        let s = Store::in_memory().unwrap();
        let run = s.create_run(&new("ci.yml", "pull_request", Some(3), None), &[job("a", &[]), job("b", &[])]).unwrap();
        let ids: Vec<i64> = s.queued_jobs().unwrap().iter().map(|q| q.id).collect();
        s.start_job(ids[0], "ntr-x").unwrap();
        assert_eq!(s.running_jobs().unwrap(), vec![(ids[0], "ntr-x".to_string(), 1)]);
        s.requeue_job(ids[0]).unwrap();
        assert_eq!(s.queued_jobs().unwrap()[0].attempt, 2);
        s.start_job(ids[0], "ntr-y").unwrap();
        assert_eq!(s.cancel_run(run).unwrap(), vec![ids[0]], "the running job is left to the scheduler");
        assert_eq!(s.job(ids[1]).unwrap().state, RunState::Cancelled);
        s.finish_job(ids[0], RunState::Cancelled, None).unwrap();
        assert_eq!(s.settle_run(run).unwrap(), RunState::Cancelled);

        let bad = s.create_run(&new("bad.yml", "pull_request", Some(3), Some("bad.yml: needs on:")), &[]).unwrap();
        assert_eq!(s.run(bad).unwrap().state, RunState::Failure);
        let checks = s.runs_for_sha("app", "abc").unwrap();
        assert_eq!(checks.iter().map(|r| r.workflow.as_str()).collect::<Vec<_>>(), vec!["bad.yml", "ci.yml"]);
        assert_eq!(s.runs("app", Some(3), 10).unwrap().len(), 2);
        assert_eq!(s.runs("app", Some(4), 10).unwrap().len(), 0);
        assert_eq!(s.run(bad).unwrap().number, 2);
    }

    #[test]
    fn frozen_snapshot_lifecycle() {
        let s = Store::in_memory().unwrap();
        let run = s.create_run(&new("ci.yml", "push", None, None), &[job("build", &[]), job("test", &[])]).unwrap();
        let ids: Vec<i64> = s.queued_jobs().unwrap().iter().map(|q| q.id).collect();
        s.set_job_frozen_snapshot(ids[0], "/tmp/ci-snapshots/run-1-job-1", 4096, Some("cargo test"), Some(101), 1_000).unwrap();
        s.set_job_frozen_snapshot(ids[1], "/tmp/ci-snapshots/run-1-job-2", 8192, Some("clippy"), Some(1), 5_000).unwrap();

        let j0 = s.job(ids[0]).unwrap();
        assert_eq!(j0.frozen_snapshot_path.as_deref(), Some("/tmp/ci-snapshots/run-1-job-1"));
        assert_eq!(j0.frozen_size_bytes, Some(4096));
        assert_eq!(j0.frozen_failed_step.as_deref(), Some("cargo test"));
        assert_eq!(j0.frozen_exit_code, Some(101));

        assert_eq!(s.expired_frozen_snapshots(2_000).unwrap(), vec![(ids[0], "/tmp/ci-snapshots/run-1-job-1".into())]);
        assert_eq!(s.frozen_snapshots_oldest_first().unwrap().len(), 2);
        s.clear_job_frozen_snapshot(ids[0]).unwrap();
        assert!(s.job(ids[0]).unwrap().frozen_snapshot_path.is_none());
        assert_eq!(s.frozen_snapshots_oldest_first().unwrap().len(), 1);
        let _ = run;
    }
}
