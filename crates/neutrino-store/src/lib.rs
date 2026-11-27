//! The server's database (SQLite, WAL): pull requests, their versions and
//! notes, branch rules, and the record of pushes that overrode a rule.
//!
//! Git holds the code; this holds what git doesn't: the review around it.

use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use neutrino_proto::{BranchRule, Note, PullRequest, PullState, PullVersion, RuleOverride, Side, StackInfo, StackMember};
use rusqlite::{Connection, OptionalExtension, params};

mod phone;
mod runs;
mod vault;
pub use runs::{NewJob, NewRun, QueuedJob};
pub use vault::{Scope, check_name as vault_check_name};

/// Each step brings the schema from version `i` to `i + 1`.
const MIGRATIONS: &[&str] = &[
    "CREATE TABLE pulls (
        repo TEXT NOT NULL,
        number INTEGER NOT NULL,
        title TEXT NOT NULL,
        body TEXT NOT NULL DEFAULT '',
        source TEXT NOT NULL,
        target TEXT NOT NULL,
        state TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        merged_at INTEGER,
        merge_sha TEXT,
        PRIMARY KEY (repo, number)
    );
    CREATE TABLE versions (
        repo TEXT NOT NULL,
        number INTEGER NOT NULL,
        version INTEGER NOT NULL,
        head_sha TEXT NOT NULL,
        base_sha TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        PRIMARY KEY (repo, number, version)
    );
    CREATE TABLE notes (
        id INTEGER PRIMARY KEY,
        repo TEXT NOT NULL,
        number INTEGER NOT NULL,
        version INTEGER NOT NULL,
        path TEXT NOT NULL,
        side TEXT NOT NULL,
        line INTEGER NOT NULL,
        text TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        resolved INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX notes_by_pull ON notes (repo, number);
    CREATE TABLE rules (
        repo TEXT NOT NULL,
        pattern TEXT NOT NULL,
        require_pull INTEGER NOT NULL,
        block_force INTEGER NOT NULL,
        block_delete INTEGER NOT NULL,
        required_checks TEXT NOT NULL DEFAULT '[]',
        PRIMARY KEY (repo, pattern)
    );
    CREATE TABLE overrides (
        id INTEGER PRIMARY KEY,
        repo TEXT NOT NULL,
        ref_name TEXT NOT NULL,
        rule TEXT NOT NULL,
        reason TEXT NOT NULL,
        at INTEGER NOT NULL
    );",
    "CREATE TABLE runs (
        id INTEGER PRIMARY KEY,
        repo TEXT NOT NULL,
        number INTEGER NOT NULL,
        workflow TEXT NOT NULL,
        name TEXT NOT NULL,
        event TEXT NOT NULL,
        branch TEXT,
        sha TEXT NOT NULL,
        pull INTEGER,
        state TEXT NOT NULL,
        error TEXT,
        warnings TEXT NOT NULL DEFAULT '[]',
        created_at INTEGER NOT NULL,
        started_at INTEGER,
        finished_at INTEGER,
        UNIQUE (repo, number)
    );
    CREATE INDEX runs_by_pull ON runs (repo, pull);
    CREATE TABLE jobs (
        id INTEGER PRIMARY KEY,
        run INTEGER NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
        key TEXT NOT NULL,
        job_id TEXT NOT NULL,
        name TEXT NOT NULL,
        matrix TEXT NOT NULL DEFAULT '{}',
        needs TEXT NOT NULL DEFAULT '[]',
        executor TEXT NOT NULL,
        image TEXT,
        cpus INTEGER NOT NULL,
        memory_mib INTEGER NOT NULL,
        state TEXT NOT NULL,
        error TEXT,
        container TEXT,
        attempt INTEGER NOT NULL DEFAULT 1,
        started_at INTEGER,
        finished_at INTEGER,
        UNIQUE (run, key)
    );
    CREATE INDEX jobs_by_state ON jobs (state);
    CREATE TABLE steps (
        job INTEGER NOT NULL REFERENCES jobs (id) ON DELETE CASCADE,
        idx INTEGER NOT NULL,
        name TEXT NOT NULL,
        state TEXT NOT NULL,
        exit_code INTEGER,
        log_start INTEGER NOT NULL DEFAULT 0,
        log_end INTEGER NOT NULL DEFAULT 0,
        started_at INTEGER,
        finished_at INTEGER,
        PRIMARY KEY (job, idx)
    );",
    "CREATE TABLE secrets (
        repo TEXT NOT NULL DEFAULT '',
        environment TEXT NOT NULL DEFAULT '',
        name TEXT NOT NULL,
        sealed BLOB NOT NULL,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (repo, environment, name)
    );
    CREATE TABLE variables (
        repo TEXT NOT NULL DEFAULT '',
        environment TEXT NOT NULL DEFAULT '',
        name TEXT NOT NULL,
        value TEXT NOT NULL,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (repo, environment, name)
    );
    CREATE TABLE environments (
        repo TEXT NOT NULL,
        name TEXT NOT NULL,
        require_approval INTEGER NOT NULL,
        branches TEXT NOT NULL DEFAULT '[]',
        PRIMARY KEY (repo, name)
    );
    CREATE TABLE caches (
        repo TEXT NOT NULL,
        key TEXT NOT NULL,
        size INTEGER NOT NULL,
        created_at INTEGER NOT NULL,
        last_used INTEGER NOT NULL,
        PRIMARY KEY (repo, key)
    );
    CREATE TABLE artifacts (
        run INTEGER NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
        job TEXT NOT NULL,
        path TEXT NOT NULL,
        size INTEGER NOT NULL,
        PRIMARY KEY (run, job, path)
    );
    CREATE TABLE schedule_marks (
        repo TEXT NOT NULL,
        workflow TEXT NOT NULL,
        cron TEXT NOT NULL,
        minute INTEGER NOT NULL,
        PRIMARY KEY (repo, workflow, cron)
    );
    ALTER TABLE jobs ADD COLUMN environment TEXT;
    ALTER TABLE jobs ADD COLUMN approved_by TEXT;
    ALTER TABLE jobs ADD COLUMN concurrency TEXT;
    ALTER TABLE runs ADD COLUMN concurrency TEXT;",
    "CREATE TABLE notifications (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        title TEXT NOT NULL,
        body TEXT NOT NULL,
        repo TEXT NOT NULL,
        run INTEGER,
        pull INTEGER,
        at INTEGER NOT NULL
    );
    CREATE TABLE push_config (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        key_id TEXT NOT NULL,
        team_id TEXT NOT NULL,
        topic TEXT NOT NULL,
        sealed_key BLOB NOT NULL
    );",
    "ALTER TABLE pulls ADD COLUMN stack_id TEXT;
    ALTER TABLE pulls ADD COLUMN parent_pull INTEGER;
    ALTER TABLE pulls ADD COLUMN stack_conflict TEXT;
    CREATE INDEX pulls_by_stack ON pulls (repo, stack_id);",
];

pub struct Store {
    db: Mutex<Connection>,
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn state_name(s: PullState) -> &'static str {
    match s {
        PullState::Open => "open",
        PullState::Merged => "merged",
        PullState::Closed => "closed",
    }
}

fn parse_state(s: &str) -> PullState {
    match s {
        "merged" => PullState::Merged,
        "closed" => PullState::Closed,
        _ => PullState::Open,
    }
}

/// Does a branch name match a rule's pattern? `*` matches any run of characters.
pub fn pattern_matches(pattern: &str, branch: &str) -> bool {
    fn go(p: &[u8], b: &[u8]) -> bool {
        match p.split_first() {
            None => b.is_empty(),
            Some((b'*', rest)) => (0..=b.len()).any(|i| go(rest, &b[i..])),
            Some((c, rest)) => b.first() == Some(c) && go(rest, &b[1..]),
        }
    }
    go(pattern.as_bytes(), branch.as_bytes())
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        db.pragma_update(None, "temp_store", "MEMORY")?;
        db.pragma_update(None, "mmap_size", 67_108_864_i64)?;
        db.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, step) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            db.execute_batch(step).with_context(|| format!("migrating the database to version {}", i + 1))?;
            db.pragma_update(None, "user_version", (i + 1) as i64)?;
        }
        Ok(Store { db: Mutex::new(db) })
    }

    /// Checkpoint and truncate the WAL, reclaim freed pages, and refresh query
    /// planner statistics. Safe to call while the database is in use.
    pub fn optimize(&self) -> Result<()> {
        let db = self.db.lock().unwrap();
        let _ = db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA incremental_vacuum; PRAGMA optimize;");
        Ok(())
    }

    pub fn in_memory() -> Result<Self> {
        let db = Connection::open_in_memory()?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        for step in MIGRATIONS {
            db.execute_batch(step)?;
        }
        Ok(Store { db: Mutex::new(db) })
    }

    // --- Pull requests ---

    fn pull_from_row(db: &Connection, repo: &str, number: u32) -> Result<Option<PullRequest>> {
        let row = db
            .query_row(
                "SELECT title, body, source, target, state, created_at, updated_at, merged_at, merge_sha, stack_id, parent_pull, stack_conflict FROM pulls WHERE repo = ?1 AND number = ?2",
                params![repo, number],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, i64>(6)?,
                        r.get::<_, Option<i64>>(7)?,
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, Option<String>>(9)?,
                        r.get::<_, Option<u32>>(10)?,
                        r.get::<_, Option<String>>(11)?,
                    ))
                },
            )
            .optional()?;
        let Some((title, body, source, target, state, created_at, updated_at, merged_at, merge_sha, stack_id, parent_pull, stack_conflict)) = row else {
            return Ok(None);
        };
        let (versions, head_sha): (u32, String) = db.query_row(
            "SELECT COUNT(*), COALESCE((SELECT head_sha FROM versions WHERE repo = ?1 AND number = ?2 ORDER BY version DESC LIMIT 1), '')
             FROM versions WHERE repo = ?1 AND number = ?2",
            params![repo, number],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let open_notes: u32 =
            db.query_row("SELECT COUNT(*) FROM notes WHERE repo = ?1 AND number = ?2 AND resolved = 0", params![repo, number], |r| r.get(0))?;
        Ok(Some(PullRequest {
            repo: repo.into(),
            number,
            title,
            body,
            source,
            target,
            state: parse_state(&state),
            head_sha,
            versions,
            open_notes,
            created_at,
            updated_at,
            merged_at,
            merge_sha,
            stack_id,
            parent_pull,
            stack_conflict,
        }))
    }

    pub fn pull(&self, repo: &str, number: u32) -> Result<PullRequest> {
        let db = self.db.lock().unwrap();
        Self::pull_from_row(&db, repo, number)?.with_context(|| format!("no pull request #{number} in {repo}"))
    }

    /// Open ones first, then the rest; newest first within each.
    pub fn pulls(&self, repo: &str) -> Result<Vec<PullRequest>> {
        let db = self.db.lock().unwrap();
        let numbers: Vec<u32> = db
            .prepare("SELECT number FROM pulls WHERE repo = ?1 ORDER BY state != 'open', updated_at DESC")?
            .query_map(params![repo], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        numbers.into_iter().filter_map(|n| Self::pull_from_row(&db, repo, n).transpose()).collect()
    }

    /// Open pull requests whose source is `branch`.
    pub fn open_pulls_from(&self, repo: &str, branch: &str) -> Result<Vec<u32>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare("SELECT number FROM pulls WHERE repo = ?1 AND source = ?2 AND state = 'open'")?
            .query_map(params![repo, branch], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Create a pull request with its first version.
    pub fn create_pull(&self, repo: &str, title: &str, body: &str, source: &str, target: &str, head: &str, base: &str) -> Result<u32> {
        self.create_pull_in_stack(repo, title, body, source, target, head, base, None, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_pull_in_stack(
        &self,
        repo: &str,
        title: &str,
        body: &str,
        source: &str,
        target: &str,
        head: &str,
        base: &str,
        stack_id: Option<&str>,
        parent_pull: Option<u32>,
    ) -> Result<u32> {
        if title.trim().is_empty() {
            bail!("give the pull request a title");
        }
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let open: Option<u32> = tx
            .query_row(
                "SELECT number FROM pulls WHERE repo = ?1 AND source = ?2 AND target = ?3 AND state = 'open'",
                params![repo, source, target],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(n) = open {
            bail!("pull request #{n} already merges {source} into {target}");
        }
        let mut resolved_stack = stack_id.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        if let Some(parent) = parent_pull {
            let parent_stack: Option<Option<String>> = tx
                .query_row("SELECT stack_id FROM pulls WHERE repo = ?1 AND number = ?2", params![repo, parent], |r| r.get(0))
                .optional()?;
            let Some(ps) = parent_stack else {
                bail!("parent pull request #{parent} does not exist in {repo}");
            };
            let chosen = resolved_stack.unwrap_or_else(|| ps.unwrap_or_else(|| format!("stack-{parent}")));
            tx.execute(
                "UPDATE pulls SET stack_id = COALESCE(stack_id, ?3) WHERE repo = ?1 AND number = ?2",
                params![repo, parent, chosen],
            )?;
            resolved_stack = Some(chosen);
        }
        let number: u32 = tx.query_row("SELECT COALESCE(MAX(number), 0) + 1 FROM pulls WHERE repo = ?1", params![repo], |r| r.get(0))?;
        let t = now();
        tx.execute(
            "INSERT INTO pulls (repo, number, title, body, source, target, state, created_at, updated_at, stack_id, parent_pull) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7, ?7, ?8, ?9)",
            params![repo, number, title.trim(), body, source, target, t, resolved_stack, parent_pull],
        )?;
        tx.execute(
            "INSERT INTO versions (repo, number, version, head_sha, base_sha, created_at) VALUES (?1, ?2, 1, ?3, ?4, ?5)",
            params![repo, number, head, base, t],
        )?;
        tx.commit()?;
        Ok(number)
    }

    pub fn stack_members(&self, repo: &str, stack_id: &str) -> Result<Vec<StackMember>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare(
                "SELECT number, title, source, target, state, parent_pull, stack_conflict
                 FROM pulls WHERE repo = ?1 AND stack_id = ?2 ORDER BY number ASC",
            )?
            .query_map(params![repo, stack_id], |r| {
                Ok(StackMember {
                    number: r.get(0)?,
                    title: r.get(1)?,
                    source: r.get(2)?,
                    target: r.get(3)?,
                    state: parse_state(&r.get::<_, String>(4)?),
                    parent_pull: r.get(5)?,
                    stack_conflict: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn stacks(&self, repo: &str) -> Result<Vec<StackInfo>> {
        let stack_ids: Vec<String> = {
            let db = self.db.lock().unwrap();
            db.prepare("SELECT DISTINCT stack_id FROM pulls WHERE repo = ?1 AND stack_id IS NOT NULL ORDER BY stack_id ASC")?
                .query_map(params![repo], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut out = Vec::with_capacity(stack_ids.len());
        for sid in stack_ids {
            let members = self.stack_members(repo, &sid)?;
            if !members.is_empty() {
                out.push(StackInfo { stack_id: sid, members });
            }
        }
        Ok(out)
    }

    pub fn child_pulls(&self, repo: &str, parent: u32) -> Result<Vec<PullRequest>> {
        let numbers: Vec<u32> = {
            let db = self.db.lock().unwrap();
            db.prepare("SELECT number FROM pulls WHERE repo = ?1 AND parent_pull = ?2 AND state = 'open' ORDER BY number ASC")?
                .query_map(params![repo, parent], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let db = self.db.lock().unwrap();
        numbers.into_iter().filter_map(|n| Self::pull_from_row(&db, repo, n).transpose()).collect()
    }

    pub fn retarget_pull(&self, repo: &str, number: u32, new_target: &str, new_parent: Option<u32>, conflict: Option<&str>) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "UPDATE pulls SET target = ?3, parent_pull = ?4, stack_conflict = ?5, updated_at = ?6 WHERE repo = ?1 AND number = ?2",
            params![repo, number, new_target, new_parent, conflict, now()],
        )?;
        Ok(())
    }

    pub fn set_stack_conflict(&self, repo: &str, number: u32, conflict: Option<&str>) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "UPDATE pulls SET stack_conflict = ?3, updated_at = ?4 WHERE repo = ?1 AND number = ?2",
            params![repo, number, conflict, now()],
        )?;
        Ok(())
    }

    /// Record a new version when the source moved. Returns its number, or `None` when the head is the same.
    pub fn add_version(&self, repo: &str, number: u32, head: &str, base: &str) -> Result<Option<u32>> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let (last, last_head): (u32, String) = tx.query_row(
            "SELECT version, head_sha FROM versions WHERE repo = ?1 AND number = ?2 ORDER BY version DESC LIMIT 1",
            params![repo, number],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if last_head == head {
            return Ok(None);
        }
        let t = now();
        tx.execute(
            "INSERT INTO versions (repo, number, version, head_sha, base_sha, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![repo, number, last + 1, head, base, t],
        )?;
        tx.execute("UPDATE pulls SET updated_at = ?3 WHERE repo = ?1 AND number = ?2", params![repo, number, t])?;
        tx.commit()?;
        Ok(Some(last + 1))
    }

    pub fn versions(&self, repo: &str, number: u32) -> Result<Vec<PullVersion>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare("SELECT version, head_sha, base_sha, created_at FROM versions WHERE repo = ?1 AND number = ?2 ORDER BY version")?
            .query_map(params![repo, number], |r| Ok(PullVersion { version: r.get(0)?, head_sha: r.get(1)?, base_sha: r.get(2)?, created_at: r.get(3)? }))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn update_pull(&self, repo: &str, number: u32, title: Option<&str>, body: Option<&str>, state: Option<PullState>) -> Result<()> {
        let current = self.pull(repo, number)?;
        if current.state == PullState::Merged && state.is_some() {
            bail!("#{number} is merged; it can't be closed or reopened");
        }
        if state == Some(PullState::Merged) {
            bail!("merge a pull request with merge, not by setting its state");
        }
        if let Some(t) = title {
            if t.trim().is_empty() {
                bail!("a pull request needs a title");
            }
        }
        let db = self.db.lock().unwrap();
        db.execute(
            "UPDATE pulls SET title = COALESCE(?3, title), body = COALESCE(?4, body), state = COALESCE(?5, state), updated_at = ?6 WHERE repo = ?1 AND number = ?2",
            params![repo, number, title.map(str::trim), body, state.map(state_name), now()],
        )?;
        Ok(())
    }

    pub fn mark_merged(&self, repo: &str, number: u32, sha: &str) -> Result<()> {
        let db = self.db.lock().unwrap();
        let t = now();
        db.execute(
            "UPDATE pulls SET state = 'merged', merged_at = ?3, merge_sha = ?4, updated_at = ?3 WHERE repo = ?1 AND number = ?2",
            params![repo, number, t, sha],
        )?;
        Ok(())
    }

    // --- Notes ---

    pub fn add_note(&self, repo: &str, number: u32, version: u32, path: &str, side: Side, line: u32, text: &str) -> Result<i64> {
        if text.trim().is_empty() {
            bail!("write the note first");
        }
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO notes (repo, number, version, path, side, line, text, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![repo, number, version, path, if side == Side::Old { "old" } else { "new" }, line, text.trim(), now()],
        )?;
        db.execute("UPDATE pulls SET updated_at = ?3 WHERE repo = ?1 AND number = ?2", params![repo, number, now()])?;
        Ok(db.last_insert_rowid())
    }

    pub fn resolve_note(&self, repo: &str, number: u32, id: i64, resolved: bool) -> Result<()> {
        let db = self.db.lock().unwrap();
        let n = db.execute("UPDATE notes SET resolved = ?4 WHERE repo = ?1 AND number = ?2 AND id = ?3", params![repo, number, id, resolved])?;
        if n == 0 {
            bail!("no note {id} on #{number}");
        }
        Ok(())
    }

    /// Notes as written; `current_line` is filled in by the caller, which can map lines.
    pub fn notes(&self, repo: &str, number: u32) -> Result<Vec<Note>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare("SELECT id, version, path, side, line, text, created_at, resolved FROM notes WHERE repo = ?1 AND number = ?2 ORDER BY id")?
            .query_map(params![repo, number], |r| {
                Ok(Note {
                    id: r.get(0)?,
                    version: r.get(1)?,
                    path: r.get(2)?,
                    side: if r.get::<_, String>(3)? == "old" { Side::Old } else { Side::New },
                    line: r.get(4)?,
                    text: r.get(5)?,
                    created_at: r.get(6)?,
                    resolved: r.get(7)?,
                    current_line: None,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    // --- Rules ---

    pub fn rules(&self, repo: &str) -> Result<Vec<BranchRule>> {
        let db = self.db.lock().unwrap();
        let rows = db
            .prepare("SELECT pattern, require_pull, block_force, block_delete, required_checks FROM rules WHERE repo = ?1 ORDER BY pattern")?
            .query_map(params![repo], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?, r.get::<_, bool>(2)?, r.get::<_, bool>(3)?, r.get::<_, String>(4)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .map(|(pattern, require_pull, block_force, block_delete, checks)| BranchRule {
                pattern,
                require_pull,
                block_force,
                block_delete,
                required_checks: serde_json::from_str(&checks).unwrap_or_default(),
            })
            .collect())
    }

    /// The rule for a branch: an exact pattern wins over a wildcard one.
    pub fn rule_for(&self, repo: &str, branch: &str) -> Result<Option<BranchRule>> {
        let rules = self.rules(repo)?;
        Ok(rules
            .iter()
            .find(|r| r.pattern == branch)
            .or_else(|| rules.iter().filter(|r| pattern_matches(&r.pattern, branch)).max_by_key(|r| r.pattern.len()))
            .cloned())
    }

    pub fn set_rule(&self, repo: &str, rule: &BranchRule) -> Result<()> {
        let p = rule.pattern.trim();
        if p.is_empty() || p.starts_with('-') || p.contains(char::is_whitespace) || p.starts_with("refs/") {
            bail!("a rule's pattern is a branch name such as main or release/*");
        }
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO rules (repo, pattern, require_pull, block_force, block_delete, required_checks) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (repo, pattern) DO UPDATE SET require_pull = ?3, block_force = ?4, block_delete = ?5, required_checks = ?6",
            params![repo, p, rule.require_pull, rule.block_force, rule.block_delete, serde_json::to_string(&rule.required_checks)?],
        )?;
        Ok(())
    }

    pub fn delete_rule(&self, repo: &str, pattern: &str) -> Result<()> {
        let db = self.db.lock().unwrap();
        if db.execute("DELETE FROM rules WHERE repo = ?1 AND pattern = ?2", params![repo, pattern])? == 0 {
            bail!("no rule for {pattern}");
        }
        Ok(())
    }

    pub fn record_override(&self, repo: &str, ref_name: &str, rule: &str, reason: &str) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute("INSERT INTO overrides (repo, ref_name, rule, reason, at) VALUES (?1, ?2, ?3, ?4, ?5)", params![repo, ref_name, rule, reason, now()])?;
        Ok(())
    }

    pub fn overrides(&self, repo: &str) -> Result<Vec<RuleOverride>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare("SELECT ref_name, rule, reason, at FROM overrides WHERE repo = ?1 ORDER BY id DESC LIMIT 200")?
            .query_map(params![repo], |r| Ok(RuleOverride { ref_name: r.get(0)?, rule: r.get(1)?, reason: r.get(2)?, at: r.get(3)? }))?
            .collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        assert!(pattern_matches("main", "main"));
        assert!(!pattern_matches("main", "maint"));
        assert!(pattern_matches("release/*", "release/1.0"));
        assert!(!pattern_matches("release/*", "hotfix/1.0"));
        assert!(pattern_matches("*", "anything/at/all"));
    }

    #[test]
    fn pull_lifecycle() {
        let s = Store::in_memory().unwrap();
        assert!(s.create_pull("r", " ", "", "f", "main", "h1", "b1").is_err(), "a title is required");
        let n = s.create_pull("r", "Add things", "Body", "feature", "main", "h1", "b1").unwrap();
        assert_eq!(n, 1);
        assert!(s.create_pull("r", "Again", "", "feature", "main", "h1", "b1").is_err(), "one open pull per branch pair");
        assert_eq!(s.create_pull("other", "Elsewhere", "", "feature", "main", "x", "y").unwrap(), 1, "numbers are per repo");

        assert_eq!(s.add_version("r", 1, "h1", "b1").unwrap(), None, "the same head is no new version");
        assert_eq!(s.add_version("r", 1, "h2", "b1").unwrap(), Some(2));
        let p = s.pull("r", 1).unwrap();
        assert_eq!((p.versions, p.head_sha.as_str(), p.state), (2, "h2", PullState::Open));
        assert_eq!(s.open_pulls_from("r", "feature").unwrap(), vec![1]);

        let id = s.add_note("r", 1, 2, "a.rs", Side::New, 10, "Check this").unwrap();
        assert_eq!(s.pull("r", 1).unwrap().open_notes, 1);
        s.resolve_note("r", 1, id, true).unwrap();
        assert_eq!(s.pull("r", 1).unwrap().open_notes, 0);
        assert!(s.notes("r", 1).unwrap()[0].resolved);

        s.update_pull("r", 1, Some("Better title"), None, Some(PullState::Closed)).unwrap();
        assert_eq!(s.pull("r", 1).unwrap().state, PullState::Closed);
        s.update_pull("r", 1, None, None, Some(PullState::Open)).unwrap();
        s.mark_merged("r", 1, "m1").unwrap();
        let p = s.pull("r", 1).unwrap();
        assert_eq!((p.state, p.merge_sha.as_deref(), p.title.as_str()), (PullState::Merged, Some("m1"), "Better title"));
        assert!(s.update_pull("r", 1, None, None, Some(PullState::Open)).is_err(), "a merged pull stays merged");
    }

    #[test]
    fn rules_and_overrides() {
        let s = Store::in_memory().unwrap();
        let rule = |p: &str| BranchRule { pattern: p.into(), require_pull: true, block_force: true, block_delete: true, required_checks: vec![] };
        s.set_rule("r", &rule("release/*")).unwrap();
        s.set_rule("r", &rule("main")).unwrap();
        assert!(s.set_rule("r", &rule("refs/heads/x")).is_err());
        assert_eq!(s.rule_for("r", "main").unwrap().unwrap().pattern, "main");
        assert_eq!(s.rule_for("r", "release/2").unwrap().unwrap().pattern, "release/*");
        assert!(s.rule_for("r", "feature").unwrap().is_none());
        let mut relaxed = rule("main");
        relaxed.block_delete = false;
        s.set_rule("r", &relaxed).unwrap();
        assert!(!s.rule_for("r", "main").unwrap().unwrap().block_delete, "setting a pattern again replaces it");
        s.delete_rule("r", "release/*").unwrap();
        assert_eq!(s.rules("r").unwrap().len(), 1);
        s.record_override("r", "refs/heads/main", "require_pull", "hotfix").unwrap();
        assert_eq!(s.overrides("r").unwrap()[0].reason, "hotfix");
    }

    #[test]
    fn migrations_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("neutrino.db");
        let s = Store::open(&path).unwrap();
        s.create_pull("r", "One", "", "a", "main", "h", "b").unwrap();
        drop(s);
        let again = Store::open(&path).unwrap();
        assert_eq!(again.pulls("r").unwrap().len(), 1, "data and schema survive reopening");
        again.optimize().unwrap();
        let snap = dir.path().join("snap.db");
        again.backup_to(&snap).unwrap();
        let restored = Store::open(&snap).unwrap();
        assert_eq!(restored.pulls("r").unwrap().len(), 1, "VACUUM INTO snapshot preserves data");
    }
}
