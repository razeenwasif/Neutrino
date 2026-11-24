//! Git repository maintenance and compression (server role).
//!
//! Hosted repositories are plain bare git repos. Without maintenance, pushes,
//! imports, mirror fetches and `merge-tree` checks leave loose objects, many
//! small packfiles, loose refs and uncompacted deltas behind.
//!
//! * **After a push or merge**: refresh the split commit-graph with changed-path
//!   Bloom filters, and when loose objects or packfiles build up, run a fast
//!   incremental geometric repack (`--geometric=2 --write-midx`).
//! * **After an import or nightly**: pack refs, run a full repack at maximum
//!   zlib compression (`pack.compression=9`) with a multi-pack index and
//!   reachability bitmap, prune unreachable objects, and write a single
//!   consolidated commit-graph.
//! * **Aggressive compaction** (`ntr compact --aggressive`, or a repo's first
//!   nightly pass): recompute deltas from scratch with `--depth=250 --window=250`
//!   for maximum compression.
//!
//! Work on any given repository is serialised by a per-repo mutex, and callers
//! evict the repo's open gitoxide handle afterwards so unlinked packfiles are
//! closed and their disk blocks reclaimed immediately.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use neutrino_proto::CompactReport;

/// Loose objects threshold that triggers an incremental geometric repack after a push.
const PUSH_LOOSE_THRESHOLD: u64 = 64;
/// Packfile count threshold that triggers an incremental geometric repack after a push.
const PUSH_PACK_THRESHOLD: u64 = 4;
/// Loose objects threshold that triggers a nightly repack on an already-packed repo.
const NIGHTLY_LOOSE_THRESHOLD: u64 = 16;
/// Loose ref files threshold that triggers `pack-refs` during nightly upkeep.
const NIGHTLY_LOOSE_REFS_THRESHOLD: usize = 16;

/// Config marker written once a repository's optimal server settings are applied.
const TUNED_KEY: &str = "neutrino.tuned";
const TUNED_VERSION: &str = "1";
/// Config marker written once a repository has had a full aggressive delta pass.
const PACKED_KEY: &str = "neutrino.packedVersion";
const PACKED_VERSION: &str = "1";

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ObjectCounts {
    pub loose: u64,
    pub in_pack: u64,
    pub packs: u64,
    pub prune_packable: u64,
    pub garbage: u64,
}

impl ObjectCounts {
    pub fn empty(self) -> bool {
        self.loose == 0 && self.in_pack == 0
    }
}

/// Serialises maintenance per repository so concurrent pushes, nightly upkeep
/// and manual `ntr compact` calls never repack the same repo at once.
#[derive(Default)]
pub struct Maintainer {
    locks: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
}

impl Maintainer {
    fn lock_for(&self, git_dir: &Path) -> Arc<Mutex<()>> {
        let mut map = self.locks.lock().unwrap();
        Arc::clone(map.entry(git_dir.to_path_buf()).or_default())
    }

    /// Background work after a push or merge: refresh the commit-graph and,
    /// when loose objects or packfiles accumulate, fold them with a geometric
    /// repack. Calls `on_repack` when packs were rewritten so gitoxide drops
    /// mmap'd handles to deleted packfiles.
    pub fn after_push(self: &Arc<Self>, git_dir: PathBuf, on_repack: impl FnOnce() + Send + 'static) {
        let this = Arc::clone(self);
        std::thread::spawn(move || {
            let repo_lock = this.lock_for(&git_dir);
            let _guard = repo_lock.lock().unwrap();
            if !git_dir.exists() {
                return;
            }
            let _ = git(&git_dir, &["commit-graph", "write", "--reachable", "--split", "--changed-paths"]);
            let Ok(counts) = count_objects(&git_dir) else { return };
            if counts.loose >= PUSH_LOOSE_THRESHOLD || counts.packs >= PUSH_PACK_THRESHOLD {
                let args = [
                    "-c", "pack.compression=9",
                    "-c", "core.compression=9",
                    "-c", "pack.useSparse=true",
                    "-c", "gc.auto=0",
                    "repack", "-d", "--geometric=2", "--write-midx",
                ];
                if git(&git_dir, &args).is_ok() {
                    on_repack();
                }
            }
        });
    }

    /// Background full optimization after importing a repository.
    pub fn after_import(self: &Arc<Self>, id: String, git_dir: PathBuf, on_repack: impl FnOnce() + Send + 'static) {
        let this = Arc::clone(self);
        std::thread::spawn(move || {
            if !git_dir.exists() {
                return;
            }
            // Never `--expire=now` in the background: a push may start right after import.
            match this.compact(&id, &git_dir, true, false) {
                Ok(_) => on_repack(),
                Err(e) => eprintln!("optimizing imported repo {id}: {e:#}"),
            }
        });
    }

    /// Pack refs, repack objects with high compression, prune unreachable
    /// objects, and write a consolidated commit-graph with Bloom filters.
    pub fn compact(&self, repo_id: &str, git_dir: &Path, aggressive: bool, prune_now: bool) -> Result<CompactReport> {
        let repo_lock = self.lock_for(git_dir);
        let _guard = repo_lock.lock().unwrap();

        let before_bytes = disk_bytes(git_dir);
        let before = count_objects(git_dir)?;

        let bare = is_bare(git_dir);
        if bare {
            let _ = configure_repo(git_dir);
        }

        // Consolidate loose refs into `packed-refs`: saves 4 KiB APFS blocks
        // per ref and speeds up ref enumeration.
        let _ = git(git_dir, &["pack-refs", "--all", "--prune"]);

        if !before.empty() {
            let has_refs = has_any_ref(git_dir);
            let mut args: Vec<&str> = vec![
                "-c", "pack.compression=9",
                "-c", "core.compression=9",
                "-c", "pack.useSparse=true",
                "-c", "pack.writeBitmapHashCache=true",
                "-c", "gc.auto=0",
                "repack",
                "-d",
            ];
            if prune_now {
                args.push("-a");
            } else {
                // Keep recently created unreachable objects (e.g. an in-flight
                // push or merge-tree) while dropping stale ones.
                args.extend(["-A", "--unpack-unreachable=1.hour.ago"]);
            }
            if aggressive {
                args.extend(["-f", "--depth=250", "--window=250", "--window-memory=1g"]);
            } else {
                args.extend(["--depth=50", "--window=50", "--window-memory=512m"]);
            }
            args.push("--write-midx");

            let want_bitmap = bare && has_refs;
            if want_bitmap {
                let mut with_bitmap = args.clone();
                with_bitmap.push("--write-bitmap-index");
                // Repos whose only refs point to non-commits (e.g. tag to blob)
                // can't build a reachability bitmap; fall back cleanly.
                if git(git_dir, &with_bitmap).is_err() {
                    git(git_dir, &args).with_context(|| format!("repacking {repo_id}"))?;
                }
            } else {
                git(git_dir, &args).with_context(|| format!("repacking {repo_id}"))?;
            }

            // Never prune with `--expire=now` while a push's quarantine directory
            // (`objects/tmp_objdir-*` / `incoming-*`) is live, or git prune could
            // delete it out from under `git receive-pack`.
            let safe_prune_now = prune_now && !has_incoming_push(git_dir);
            let expire = if safe_prune_now { "--expire=now" } else { "--expire=1.hour.ago" };
            let mid = count_objects(git_dir).unwrap_or_default();
            if safe_prune_now || mid.loose > 0 || mid.garbage > 0 {
                let _ = git(git_dir, &["prune", expire]);
            }
            let _ = git(git_dir, &["prune-packed"]);

            if has_refs {
                let _ = git(git_dir, &["commit-graph", "write", "--reachable", "--changed-paths"]);
            }
        }

        if bare && aggressive {
            let _ = git(git_dir, &["config", PACKED_KEY, PACKED_VERSION]);
        }

        let after = count_objects(git_dir)?;
        let after_bytes = disk_bytes(git_dir);
        Ok(CompactReport {
            repo: repo_id.to_string(),
            before_bytes,
            after_bytes,
            loose_before: before.loose,
            loose_after: after.loose,
            packs_before: before.packs,
            packs_after: after.packs,
        })
    }

    /// Nightly check for one hosted repository: returns `Some(aggressive)` if
    /// it should be compacted, or `None` when it is already spotless.
    pub fn nightly_plan(&self, git_dir: &Path) -> Option<bool> {
        let counts = count_objects(git_dir).ok()?;
        if counts.empty() {
            return None;
        }
        if !config_has(git_dir, "packedVersion", PACKED_VERSION) {
            return Some(true);
        }
        if counts.packs > 1 || counts.loose >= NIGHTLY_LOOSE_THRESHOLD || counts.garbage > 0 || counts.prune_packable > 0 {
            return Some(false);
        }
        if count_loose_refs(&git_dir.join("refs")) >= NIGHTLY_LOOSE_REFS_THRESHOLD {
            return Some(false);
        }
        None
    }
}

/// Apply optimal server-side git config once per hosted bare repo.
///
/// Fast path reads `<git_dir>/config` directly so startup across dozens of
/// already-tuned repos forks zero `git` processes.
pub fn configure_repo(git_dir: &Path) -> Result<()> {
    if config_has(git_dir, "tuned", TUNED_VERSION) {
        return Ok(());
    }
    let settings = [
        ("core.compression", "9"),
        ("pack.compression", "9"),
        ("pack.useSparse", "true"),
        ("pack.writeBitmapHashCache", "true"),
        ("repack.writeBitmaps", "true"),
        ("fetch.writeCommitGraph", "true"),
        ("gc.writeCommitGraph", "true"),
        // Let neutrinod run maintenance asynchronously instead of stalling pushes.
        ("receive.autogc", "false"),
        (TUNED_KEY, TUNED_VERSION),
    ];
    for (k, v) in settings {
        git(git_dir, &["config", k, v])?;
    }
    Ok(())
}

/// Quick check of `<git_dir>/config` for a `[neutrino]` key = value without forking git.
fn config_has(git_dir: &Path, key: &str, expected: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(git_dir.join("config")) else { return false };
    let mut in_neutrino = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_neutrino = line.eq_ignore_ascii_case("[neutrino]");
            continue;
        }
        if in_neutrino {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim().eq_ignore_ascii_case(key) && v.trim() == expected {
                    return true;
                }
            }
        }
    }
    false
}

fn has_incoming_push(git_dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(git_dir.join("objects")) else { return false };
    entries.flatten().any(|e| {
        let name = e.file_name();
        let s = name.to_string_lossy();
        s.starts_with("tmp_objdir") || s.starts_with("incoming-")
    })
}

fn is_bare(git_dir: &Path) -> bool {
    git(git_dir, &["rev-parse", "--is-bare-repository"]).map(|s| s.trim() == "true").unwrap_or(false)
}

fn has_any_ref(git_dir: &Path) -> bool {
    git(git_dir, &["for-each-ref", "--count=1", "--format=%(refname)"]).map(|s| !s.trim().is_empty()).unwrap_or(false)
}

pub(crate) fn count_objects(git_dir: &Path) -> Result<ObjectCounts> {
    let out = git(git_dir, &["count-objects", "-v"])?;
    let mut c = ObjectCounts::default();
    for line in out.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let n: u64 = v.trim().parse().unwrap_or(0);
        match k.trim() {
            "count" => c.loose = n,
            "in-pack" => c.in_pack = n,
            "packs" => c.packs = n,
            "prune-packable" => c.prune_packable = n,
            "garbage" => c.garbage = n,
            _ => {}
        }
    }
    Ok(c)
}

fn count_loose_refs(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for e in entries.flatten() {
        match e.file_type() {
            Ok(t) if t.is_dir() => n += count_loose_refs(&e.path()),
            Ok(t) if t.is_file() => n += 1,
            _ => {}
        }
    }
    n
}

/// Allocated disk space of `path` in bytes (512-byte blocks on Unix, so loose
/// objects and loose refs reflect their real APFS/filesystem footprint).
pub fn disk_bytes(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if meta.is_dir() {
        std::fs::read_dir(path).map(|d| d.flatten().map(|e| disk_bytes(&e.path())).sum()).unwrap_or(0)
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            meta.blocks().saturating_mul(512)
        }
        #[cfg(not(unix))]
        {
            meta.len()
        }
    }
}

fn git(git_dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new(neutrino_git::worktree::git_binary())
        .arg("--git-dir")
        .arg(git_dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .context("running git")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let msg = stderr.trim();
        bail!("git {}: {}", args.first().unwrap_or(&""), if msg.is_empty() { "failed" } else { msg });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "A")
            .env("GIT_AUTHOR_EMAIL", "a@x")
            .env("GIT_COMMITTER_NAME", "A")
            .env("GIT_COMMITTER_EMAIL", "a@x")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn compact_shrinks_loose_and_repetitive_history_and_handles_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let m = Maintainer::default();

        // Empty bare repo compacts cleanly without error.
        let empty = tmp.path().join("empty.git");
        run_git(tmp.path(), &["init", "-q", "--bare", empty.to_str().unwrap()]);
        let rep = m.compact("empty", &empty, true, true).unwrap();
        assert_eq!((rep.loose_before, rep.loose_after, rep.packs_after), (0, 0, 0));
        assert!(config_has(&empty, "tuned", TUNED_VERSION));
        assert!(m.nightly_plan(&empty).is_none());

        // Repo with repetitive commits pushed one by one (creating loose objects / small packs).
        let bare = tmp.path().join("demo.git");
        let work = tmp.path().join("work");
        run_git(tmp.path(), &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()]);
        run_git(tmp.path(), &["clone", "-q", bare.to_str().unwrap(), work.to_str().unwrap()]);
        let base_line = "the quick brown fox jumps over the lazy dog 0123456789\n";
        let mut content = base_line.repeat(400);
        for i in 0..25 {
            content.push_str(&format!("revision {i} with a small tweak\n"));
            std::fs::write(work.join("big.txt"), &content).unwrap();
            run_git(&work, &["add", "big.txt"]);
            run_git(&work, &["commit", "-q", "-m", &format!("rev {i}")]);
            run_git(&work, &["push", "-q", "origin", "HEAD:refs/heads/main"]);
        }

        // Before compacting: loose objects exist, never deeply packed.
        let before = count_objects(&bare).unwrap();
        assert!(before.loose > 0 || before.packs > 1, "{before:?}");
        assert_eq!(m.nightly_plan(&bare), Some(true));

        let rep = m.compact("demo", &bare, true, true).unwrap();
        assert_eq!(rep.loose_after, 0);
        assert_eq!(rep.packs_after, 1);
        assert!(rep.after_bytes < rep.before_bytes, "expected compaction to shrink repo: {rep:?}");
        assert!(bare.join("objects/pack/multi-pack-index").exists());
        assert!(bare.join("objects/info/commit-graph").exists());
        assert!(bare.join("packed-refs").exists());
        // Already deeply packed with 0 loose objects and 1 pack: nightly skips it.
        assert_eq!(m.nightly_plan(&bare), None);

        // Gitoxide reads the compacted repo cleanly (packs, midx, commit-graph, packed-refs).
        let gix_repo = neutrino_git::discover(&bare).unwrap();
        let local = neutrino_git::local(&gix_repo);
        let page = neutrino_git::log(&local, &neutrino_proto::LogQuery { repo: "demo".into(), ..Default::default() }).unwrap();
        assert_eq!(page.commits.len(), 25);
        assert_eq!(page.commits[0].subject, "rev 24");
    }
}
