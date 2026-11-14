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
        steps: steps_of(db, id)?,
    })
}

const JOB_COLUMNS: &str = "id, run, key, name, executor, image, state, error, needs, attempt, started_at, finished_at";

impl Store {
    /// A run with its jobs, all queued. `error` makes a run that failed before
    /// starting (a workflow file that doesn't parse).
    #[allow(clippy::too_many_arguments)]
    pub fn create_run(
        &self,
        repo: &str,
        workflow: &str,
        name: &str,
        event: &str,
        branch: Option<&str>,
        sha: &str,
        pull: Option<u32>,
        jobs: &[NewJob],
        warnings: &[String],
        error: Option<&str>,
    ) -> Result<i64> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let number: u32 = tx.query_row("SELECT COALESCE(MAX(number), 0) + 1 FROM runs WHERE repo = ?1", params![repo], |r| r.get(0))?;
        let t = now();
        let state = if error.is_some() { RunState::Failure } else { RunState::Queued };
        tx.execute(
            "INSERT INTO runs (repo, number, workflow, name, event, branch, sha, pull, state, error, warnings, created_at, finished_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                repo,
                number,
                workflow,
                name,
                event,
                branch,
                sha,
                pull,
                state.name(),
                error,
                serde_json::to_string(warnings)?,
                t,
                error.map(|_| t)
            ],
        )?;
        let run = tx.last_insert_rowid();
        for j in jobs {
            tx.execute(
                "INSERT INTO jobs (run, key, job_id, name, matrix, needs, executor, image, cpus, memory_mib, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'queued')",
                params![run, j.key, j.job_id, j.name, j.matrix, serde_json::to_string(&j.needs)?, j.executor, j.image, j.cpus, j.memory_mib as i64],
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
                "SELECT j.id, j.run, r.repo, j.key, j.job_id, r.workflow, j.matrix, j.needs, j.cpus, j.memory_mib, j.attempt
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
            db.execute("UPDATE steps SET state = 'cancelled' WHERE state = 'queued' AND job IN (SELECT id FROM jobs WHERE run = ?1 AND state = 'queued')", params![run])?;
            db.execute("UPDATE jobs SET state = 'cancelled', finished_at = ?2 WHERE run = ?1 AND state = 'queued'", params![run, t])?;
            db.prepare("SELECT id FROM jobs WHERE run = ?1 AND state = 'running'")?.query_map(params![run], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
        };
        if running.is_empty() {
            self.settle_run(run)?;
        }
        Ok(running)
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
        }
    }

    #[test]
    fn a_run_from_queue_to_finish() {
        let s = Store::in_memory().unwrap();
        let run = s.create_run("app", "ci.yml", "ci", "push", Some("main"), "abc", None, &[job("build", &[]), job("test", &["build"])], &[], None).unwrap();
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
        let run = s.create_run("app", "ci.yml", "ci", "pull_request", Some("f"), "abc", Some(3), &[job("a", &[]), job("b", &[])], &[], None).unwrap();
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

        let bad = s.create_run("app", "bad.yml", "bad.yml", "pull_request", Some("f"), "abc", Some(3), &[], &[], Some("bad.yml: needs on:")).unwrap();
        assert_eq!(s.run(bad).unwrap().state, RunState::Failure);
        let checks = s.runs_for_sha("app", "abc").unwrap();
        assert_eq!(checks.iter().map(|r| r.workflow.as_str()).collect::<Vec<_>>(), vec!["bad.yml", "ci.yml"]);
        assert_eq!(s.runs("app", Some(3), 10).unwrap().len(), 2);
        assert_eq!(s.runs("app", Some(4), 10).unwrap().len(), 0);
        assert_eq!(s.run(bad).unwrap().number, 2);
    }
}
