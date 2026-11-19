//! Mirrors (server role): after every push to a hosted repository, its
//! branches and tags are copied to another remote (its GitHub repository, say),
//! so a copy of everything lives off this Mac.
//!
//! The URL is kept in the repository's own config (`neutrino.mirror`), so it
//! survives losing the database. How the last copy went is kept beside it in
//! `neutrino-mirror.json`. Copies run on their own thread, one at a time per
//! repository; pushes that land meanwhile are folded into one more copy. A
//! copy that fails is tried again every ten minutes, and the first failure is
//! sent to the phone.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use neutrino_proto::MirrorStatus;
use serde::{Deserialize, Serialize};

use crate::hosting::{BRANCHES_AND_TAGS, git_net};

const URL_KEY: &str = "neutrino.mirror";
const STATUS_FILE: &str = "neutrino-mirror.json";
/// How long one copy may take.
const COPY_LIMIT: Duration = Duration::from_secs(20 * 60);
/// How often a failed copy is tried again.
pub(crate) const RETRY_AFTER: i64 = 10 * 60;

/// What's kept between copies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Saved {
    #[serde(default)]
    pub last_ok: Option<i64>,
    #[serde(default)]
    pub last_attempt: Option<i64>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// A copy is owed: set before one starts, cleared when one works. Survives
    /// a restart, so a push that landed just before one isn't left uncopied.
    #[serde(default)]
    pub pending: bool,
    /// The current run of failures has been sent to the phone.
    #[serde(default)]
    pub notified: bool,
}

fn now() -> i64 {
    neutrino_store::now()
}

fn git_config(git_dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new(neutrino_git::worktree::git_binary())
        .arg("--git-dir")
        .arg(git_dir)
        .arg("config")
        .args(args)
        .output()
        .context("running git config")
}

/// The repository's mirror, if it has one.
pub(crate) fn url(git_dir: &Path) -> Option<String> {
    let out = git_config(git_dir, &["--get", URL_KEY]).ok()?;
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !url.is_empty()).then_some(url)
}

/// Set the mirror (or with `None`, stop mirroring) and start afresh.
pub(crate) fn set_url(git_dir: &Path, url: Option<&str>) -> Result<()> {
    match url {
        Some(u) => {
            if u.trim().is_empty() || u.starts_with('-') || u.chars().any(|c| c.is_whitespace() || c.is_control()) {
                bail!("{u:?} isn't a usable git URL");
            }
            let out = git_config(git_dir, &[URL_KEY, u])?;
            if !out.status.success() {
                bail!("git config failed: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
        }
        None => {
            // Exit code 5 just means there was nothing to unset.
            let _ = git_config(git_dir, &["--unset", URL_KEY])?;
        }
    }
    let _ = std::fs::remove_file(git_dir.join(STATUS_FILE));
    Ok(())
}

pub(crate) fn load(git_dir: &Path) -> Saved {
    std::fs::read(git_dir.join(STATUS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub(crate) fn save(git_dir: &Path, saved: &Saved) {
    let path = git_dir.join(STATUS_FILE);
    let tmp = path.with_extension("json.tmp");
    let ok = std::fs::write(&tmp, serde_json::to_vec_pretty(saved).expect("serialises")).and_then(|_| std::fs::rename(&tmp, &path));
    if let Err(e) = ok {
        eprintln!("can't record the mirror's state in {}: {e}", git_dir.display());
    }
}

/// Record that the repository and its mirror hold the same thing (just after
/// an import from it, say).
pub(crate) fn mark_in_step(git_dir: &Path) {
    save(git_dir, &Saved { last_ok: Some(now()), last_attempt: Some(now()), ..Default::default() });
}

pub(crate) fn status(repo: &str, git_dir: &Path) -> Option<MirrorStatus> {
    let url = url(git_dir)?;
    let s = load(git_dir);
    let state = if s.pending {
        if s.last_error.is_some() { "failing" } else { "pending" }
    } else if s.last_error.is_some() {
        "failing"
    } else {
        "ok"
    };
    Some(MirrorStatus { repo: repo.into(), url, state: state.into(), last_ok: s.last_ok, last_attempt: s.last_attempt, last_error: s.last_error })
}

/// Copy every branch and tag to `url`, deleting there what was deleted here.
/// Nothing else on the far side is touched (GitHub's pull request refs, say).
pub(crate) fn copy(git_dir: &Path, url: &str) -> Result<String> {
    let mut args = vec!["push", "--porcelain", "--prune", "--", url];
    args.extend(BRANCHES_AND_TAGS);
    let out = git_net(git_dir, &args, COPY_LIMIT)?;
    // `--porcelain` exits 0 even when some refs were refused; look.
    let refused: Vec<&str> = out.lines().filter(|l| l.starts_with('!')).collect();
    if !refused.is_empty() {
        bail!("{} refused: {}", url, refused.join("; "));
    }
    Ok(out)
}

/// One copy and its bookkeeping. Returns whether it worked.
fn copy_once(git_dir: &Path) -> bool {
    let Some(url) = url(git_dir) else {
        let mut s = load(git_dir);
        s.pending = false;
        save(git_dir, &s);
        return true;
    };
    let mut s = load(git_dir);
    s.pending = true;
    s.last_attempt = Some(now());
    save(git_dir, &s);
    let result = copy(git_dir, &url);
    let mut s = load(git_dir);
    match result {
        Ok(_) => {
            s.pending = false;
            s.last_ok = Some(now());
            s.last_error = None;
            s.notified = false;
        }
        Err(e) => {
            eprintln!("mirroring {} to {url} failed: {e:#}", git_dir.display());
            s.last_error = Some(format!("{e:#}"));
        }
    }
    save(git_dir, &s);
    s.last_error.is_none()
}

/// Copies in flight, by repository: `true` when another is owed after it.
#[derive(Default)]
pub struct Mirrors {
    busy: Mutex<HashMap<PathBuf, bool>>,
}

impl Mirrors {
    /// Copy the repository to its mirror soon, on a thread of its own.
    pub(crate) fn request(self: &Arc<Self>, git_dir: &Path) {
        if url(git_dir).is_none() {
            return;
        }
        let mut s = load(git_dir);
        if !s.pending {
            s.pending = true;
            save(git_dir, &s);
        }
        {
            let mut busy = self.busy.lock().unwrap();
            if let Some(again) = busy.get_mut(git_dir) {
                *again = true;
                return;
            }
            busy.insert(git_dir.to_path_buf(), false);
        }
        let (this, dir) = (Arc::clone(self), git_dir.to_path_buf());
        std::thread::spawn(move || {
            loop {
                copy_once(&dir);
                let mut busy = this.busy.lock().unwrap();
                match busy.get_mut(&dir) {
                    Some(again) if *again => *again = false,
                    _ => {
                        busy.remove(&dir);
                        break;
                    }
                }
            }
        });
    }

    /// Copy now on this thread, unless a copy is already running (then wait
    /// for that one and the one after). Returns the state afterwards.
    pub(crate) fn copy_now(self: &Arc<Self>, git_dir: &Path) -> Result<()> {
        if url(git_dir).is_none() {
            bail!("this repository has no mirror; set one with ntr mirror set URL");
        }
        loop {
            let running = self.busy.lock().unwrap().contains_key(git_dir);
            if !running {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        {
            let mut busy = self.busy.lock().unwrap();
            busy.insert(git_dir.to_path_buf(), false);
        }
        copy_once(git_dir);
        // A push during this copy asked for another; let a thread do it.
        let again = self.busy.lock().unwrap().remove(git_dir).unwrap_or(false);
        if again {
            self.request(git_dir);
        }
        Ok(())
    }

    pub(crate) fn running(&self, git_dir: &Path) -> bool {
        self.busy.lock().unwrap().contains_key(git_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
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
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn copies_branches_and_tags_and_prunes() {
        let tmp = tempfile::tempdir().unwrap();
        let (here, there, work) = (tmp.path().join("here.git"), tmp.path().join("there.git"), tmp.path().join("work"));
        git(tmp.path(), &["init", "-q", "--bare", "here.git"]);
        git(tmp.path(), &["init", "-q", "--bare", "there.git"]);
        git(tmp.path(), &["clone", "-q", here.to_str().unwrap(), "work"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&work, &["push", "-q", "origin", "HEAD:refs/heads/main", "HEAD:refs/heads/old"]);
        git(&work, &["tag", "v1"]);
        git(&work, &["push", "-q", "origin", "v1"]);

        assert!(status("r", &here).is_none());
        set_url(&here, Some(there.to_str().unwrap())).unwrap();
        let mirrors = Arc::new(Mirrors::default());
        mirrors.copy_now(&here).unwrap();
        let refs = git(&there, &["for-each-ref", "--format=%(refname)"]);
        assert!(refs.contains("refs/heads/main") && refs.contains("refs/heads/old") && refs.contains("refs/tags/v1"), "{refs}");
        let st = status("r", &here).unwrap();
        assert_eq!(st.state, "ok");
        assert!(st.last_ok.is_some());

        // Something only the mirror has that isn't a branch or tag stays.
        git(&there, &["update-ref", "refs/pull/1/head", &git(&work, &["rev-parse", "HEAD"])]);
        // A deleted branch goes from the mirror too; the pull ref doesn't.
        git(&work, &["push", "-q", "origin", ":old"]);
        mirrors.copy_now(&here).unwrap();
        let refs = git(&there, &["for-each-ref", "--format=%(refname)"]);
        assert!(!refs.contains("refs/heads/old") && refs.contains("refs/pull/1/head"), "{refs}");

        // A mirror that can't be reached is failing, and stays owed.
        set_url(&here, Some(tmp.path().join("missing.git").to_str().unwrap())).unwrap();
        mirrors.copy_now(&here).unwrap();
        let st = status("r", &here).unwrap();
        assert_eq!(st.state, "failing");
        assert!(st.last_error.is_some());
        assert!(load(&here).pending);

        set_url(&here, None).unwrap();
        assert!(status("r", &here).is_none());
        assert!(set_url(&here, Some("--upload-pack=x")).is_err());
    }

    #[test]
    fn requests_fold_into_one_more_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let (here, there, work) = (tmp.path().join("here.git"), tmp.path().join("there.git"), tmp.path().join("work"));
        git(tmp.path(), &["init", "-q", "--bare", "here.git"]);
        git(tmp.path(), &["init", "-q", "--bare", "there.git"]);
        git(tmp.path(), &["clone", "-q", here.to_str().unwrap(), "work"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&work, &["push", "-q", "origin", "HEAD:refs/heads/main"]);
        set_url(&here, Some(there.to_str().unwrap())).unwrap();
        let mirrors = Arc::new(Mirrors::default());
        for _ in 0..5 {
            mirrors.request(&here);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while mirrors.running(&here) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!mirrors.running(&here));
        assert_eq!(status("r", &here).unwrap().state, "ok");
        assert_eq!(git(&there, &["rev-parse", "main"]), git(&work, &["rev-parse", "HEAD"]));
    }
}
