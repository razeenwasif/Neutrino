//! What the phone reads: the Inbox, and the text behind push notifications.

use anyhow::Result;
use neutrino_proto::{NotificationText, Run, RunState, Waiting};
use rusqlite::{OptionalExtension, params};

use crate::Store;

impl Store {
    /// Jobs waiting for approval, in every repo, oldest first.
    pub fn waiting(&self) -> Result<Vec<Waiting>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare(
                "SELECT r.repo, r.id, r.number, r.name, j.id, j.name, j.environment, r.branch, r.created_at
                 FROM jobs j JOIN runs r ON r.id = j.run WHERE j.state = 'waiting' ORDER BY r.id",
            )?
            .query_map([], |r| {
                Ok(Waiting {
                    repo: r.get(0)?,
                    run: r.get(1)?,
                    number: r.get(2)?,
                    run_name: r.get(3)?,
                    job: r.get(4)?,
                    job_name: r.get(5)?,
                    environment: r.get(6)?,
                    branch: r.get(7)?,
                    since: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Unfinished runs in every repo, then the most recent finished ones.
    pub fn runs_everywhere(&self, recent: u32) -> Result<Vec<Run>> {
        let ids: Vec<i64> = {
            let db = self.db.lock().unwrap();
            db.prepare(
                "SELECT id FROM runs WHERE state IN ('queued', 'running', 'waiting')
                 UNION ALL SELECT id FROM (SELECT id FROM runs WHERE state NOT IN ('queued', 'running', 'waiting') ORDER BY id DESC LIMIT ?1)",
            )?
            .query_map(params![recent], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
        };
        let mut runs = ids.into_iter().map(|id| self.run(id)).collect::<Result<Vec<_>>>()?;
        runs.sort_by_key(|r| (r.state.finished(), std::cmp::Reverse(r.id)));
        Ok(runs)
    }

    pub fn add_notification(&self, n: &NotificationText) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO notifications (id, kind, title, body, repo, run, pull, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![n.id, n.kind, n.title, n.body, n.repo, n.run, n.pull, n.at],
        )?;
        // A month of them is plenty.
        db.execute("DELETE FROM notifications WHERE at < ?1", params![n.at - 30 * 86_400])?;
        Ok(())
    }

    /// The latest notifications, newest first.
    pub fn notifications(&self, limit: u32) -> Result<Vec<NotificationText>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare("SELECT id, kind, title, body, repo, run, pull, at FROM notifications WHERE kind != 'test' ORDER BY at DESC, rowid DESC LIMIT ?1")?
            .query_map(params![limit], |r| {
                Ok(NotificationText {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    title: r.get(2)?,
                    body: r.get(3)?,
                    repo: r.get(4)?,
                    run: r.get(5)?,
                    pull: r.get(6)?,
                    at: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn notification(&self, id: &str) -> Result<Option<NotificationText>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .query_row("SELECT id, kind, title, body, repo, run, pull, at FROM notifications WHERE id = ?1", params![id], |r| {
                Ok(NotificationText {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    title: r.get(2)?,
                    body: r.get(3)?,
                    repo: r.get(4)?,
                    run: r.get(5)?,
                    pull: r.get(6)?,
                    at: r.get(7)?,
                })
            })
            .optional()?)
    }

    pub fn set_push_config(&self, key_id: &str, team_id: &str, topic: &str, sealed_key: &[u8]) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO push_config (id, key_id, team_id, topic, sealed_key) VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT (id) DO UPDATE SET key_id = ?1, team_id = ?2, topic = ?3, sealed_key = ?4",
            params![key_id, team_id, topic, sealed_key],
        )?;
        Ok(())
    }

    /// Key id, team id, topic and the sealed .p8.
    pub fn push_config(&self) -> Result<Option<(String, String, String, Vec<u8>)>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .query_row("SELECT key_id, team_id, topic, sealed_key FROM push_config WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()?)
    }

    /// Whether a finished run failed, for the failure notifications.
    pub fn run_failed(&self, run: i64) -> Result<bool> {
        Ok(self.run(run)?.state == RunState::Failure)
    }
}
