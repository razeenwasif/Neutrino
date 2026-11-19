//! What the server does on its own: scheduled workflows, pruning old logs,
//! artifacts and caches, and nightly snapshots of the database.

use std::path::PathBuf;

use anyhow::Result;
use neutrino_ci::cron::{Cron, Moment};
use neutrino_store::now;

use crate::Daemon;

impl Daemon {
    /// Start the scheduled workflows due at `at` (Unix seconds, read in UTC),
    /// on each hosted repo's default branch. Each line fires once per minute,
    /// even across restarts. Returns the runs started.
    #[doc(hidden)]
    pub fn fire_schedules(&self, at: i64) -> Vec<i64> {
        let (Ok(store), Some(ci)) = (self.store(), self.ci.as_ref()) else { return Vec::new() };
        let minute = at.div_euclid(60);
        let moment = Moment::utc(minute * 60);
        let mut started = Vec::new();
        for repo in self.registry.list().into_iter().filter(|r| r.bare && r.error.is_none()) {
            let Ok(handle) = self.registry.get(&repo.id) else { continue };
            let local = neutrino_git::local(&handle);
            let (Ok(Some(head)), Ok(sha)) = (local.head_name(), local.head_id()) else { continue };
            let branch = head.shorten().to_string();
            let sha = sha.to_string();
            let Ok(files) = self.workflows_at(&local, &sha) else { continue };
            let before = started.len();
            for (file, parsed) in files {
                let Ok(wf) = parsed else { continue };
                for line in &wf.on.schedule {
                    let Ok(cron) = Cron::parse(line) else { continue };
                    if !cron.matches(moment) || store.schedule_mark(&repo.id, &file, line).ok().flatten() == Some(minute) {
                        continue;
                    }
                    let _ = store.set_schedule_mark(&repo.id, &file, line, minute);
                    match self.create_run(&repo.id, &file, Ok(wf.clone()), "schedule", Some(&branch), &sha, None) {
                        Ok(id) => started.push(id),
                        Err(e) => eprintln!("can't start scheduled {file} in {}: {e:#}", repo.id),
                    }
                    // One run per workflow per minute, however many lines match.
                    break;
                }
            }
            if started.len() > before {
                self.runs_changed(&repo.id);
            }
        }
        if !started.is_empty() {
            ci.wake.notify_one();
        }
        started
    }

    /// Drop logs and artifacts of runs finished longer ago than the retention
    /// period, and the least recently used caches over the size cap.
    pub(crate) fn prune(&self) -> Result<()> {
        let store = self.store()?;
        let ci = self.ci.as_ref().expect("server");
        let before = now() - i64::from(ci.options.retention_days) * 86_400;
        for run in store.finished_before(before)? {
            let _ = std::fs::remove_dir_all(self.runs_dir().join(run.to_string()));
            let artifacts = self.state_dir.join("artifacts").join(run.to_string());
            if artifacts.exists() {
                let _ = std::fs::remove_dir_all(&artifacts);
                store.forget_artifacts(run)?;
            }
        }
        let caches = store.caches_by_use()?;
        let mut total: u64 = caches.iter().map(|(_, _, s)| s).sum();
        let cap = ci.options.cache_mib * 1024 * 1024;
        for (repo, key, size) in caches {
            if total <= cap {
                break;
            }
            let _ = std::fs::remove_dir_all(super::workspace::cache_dir(&self.state_dir, &repo, &key));
            store.drop_cache(&repo, &key)?;
            total = total.saturating_sub(size);
        }
        Ok(())
    }

    pub(crate) fn backup_dir(&self) -> PathBuf {
        self.ci.as_ref().and_then(|c| c.options.backup_dir.clone()).unwrap_or_else(|| self.state_dir.join("backups"))
    }

    /// Snapshot the database; keep the newest few. Returns the new file.
    pub(crate) fn backup_now(&self) -> Result<PathBuf> {
        let store = self.store()?;
        let dir = self.backup_dir();
        std::fs::create_dir_all(&dir)?;
        let m = Moment::utc(now());
        let path = dir.join(format!("neutrino-{:04}-{:02}-{:02}-{:02}{:02}.db", m.year, m.month, m.day, m.hour, m.minute));
        let _ = std::fs::remove_file(&path);
        store.backup_to(&path)?;
        let keep = self.ci.as_ref().map(|c| c.options.backups_kept).unwrap_or(14).max(1);
        let mut old: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("neutrino-") && n.to_string_lossy().ends_with(".db")))
            .collect();
        old.sort();
        let excess = old.len().saturating_sub(keep);
        for p in old.into_iter().take(excess) {
            let _ = std::fs::remove_file(p);
        }
        Ok(path)
    }

    /// Whether today's snapshot (UTC) is still to be taken.
    pub(crate) fn backup_due(&self) -> bool {
        let m = Moment::utc(now());
        if (m.hour, m.minute) < (3, 30) {
            return false;
        }
        let today = format!("neutrino-{:04}-{:02}-{:02}-", m.year, m.month, m.day);
        !std::fs::read_dir(self.backup_dir()).map(|d| d.flatten().any(|e| e.file_name().to_string_lossy().starts_with(&today))).unwrap_or(false)
    }
}
