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
//!
//! A copy never overwrites what only the mirror has. Before pushing it
//! fetches the mirror's branches and tags into `refs/mirror/` here, then for
//! each one:
//!
//! * the same here and there: nothing to do;
//! * the mirror's tip is in this repository's history: a fast-forward;
//! * the mirror hasn't moved since the last copy: this side was rewritten (a
//!   force-push here), so it's copied over;
//! * otherwise the mirror has commits this side doesn't (a collaborator's
//!   push, a pull request merged on GitHub): that ref is left alone, the
//!   copy reports it, and the commits are here in `refs/mirror/` to merge.
//!
//! A ref only the mirror has is deleted there only when the last copy put it
//! there and it hasn't moved since (it was deleted here); anything else, a
//! branch made on GitHub say, stays. Every update is leased on what was
//! seen, so a change on the mirror mid-copy fails it instead of being lost.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use neutrino_proto::MirrorStatus;
use serde::{Deserialize, Serialize};

use crate::hosting::git_net;

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
    /// What the last copy left on the mirror, ref → sha: how a deletion here
    /// is told from a branch made there, and a rewrite here from a push there.
    #[serde(default)]
    pub copied: BTreeMap<String, String>,
    /// Refs only the mirror has, left alone.
    #[serde(default)]
    pub mirror_only: Vec<String>,
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
    let copied = local_refs(git_dir, "refs/heads", "refs/tags").unwrap_or_default();
    save(git_dir, &Saved { last_ok: Some(now()), last_attempt: Some(now()), copied, ..Default::default() });
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
    Some(MirrorStatus {
        repo: repo.into(),
        url,
        state: state.into(),
        last_ok: s.last_ok,
        last_attempt: s.last_attempt,
        last_error: s.last_error,
        mirror_only: s.mirror_only,
    })
}

/// Where the mirror's refs are fetched to here, by kind.
const SEEN: &str = "refs/mirror/";

/// `git` in this repository, nothing over the network.
fn git_here(git_dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new(neutrino_git::worktree::git_binary()).arg("--git-dir").arg(git_dir).args(args).output().context("running git")?;
    if !out.status.success() {
        bail!("git {}: {}", args[0], String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `refname → sha` for refs under each prefix.
fn local_refs(git_dir: &Path, heads: &str, tags: &str) -> Result<BTreeMap<String, String>> {
    let out = git_here(git_dir, &["for-each-ref", "--format=%(objectname) %(refname)", heads, tags])?;
    Ok(out.lines().filter_map(|l| l.split_once(' ')).map(|(sha, name)| (name.to_string(), sha.to_string())).collect())
}

fn is_ancestor(git_dir: &Path, ancestor: &str, of: &str) -> bool {
    Command::new(neutrino_git::worktree::git_binary())
        .arg("--git-dir")
        .arg(git_dir)
        .args(["merge-base", "--is-ancestor", ancestor, of])
        .status()
        .is_ok_and(|s| s.success())
}

/// What one copy did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Copied {
    /// Refs created, moved or deleted on the mirror.
    pub changed: usize,
    /// Refs where the mirror has commits this side doesn't, left alone.
    pub refused: Vec<String>,
    /// Refs only the mirror has, left alone.
    pub mirror_only: Vec<String>,
    /// What's on the mirror now, as far as this side knows, ref → sha.
    pub copied: BTreeMap<String, String>,
}

/// Copy branches and tags to `url` without overwriting anything only the
/// mirror has (see the module's notes). `last` is what the last copy left there.
pub(crate) fn copy(git_dir: &Path, url: &str, last: &BTreeMap<String, String>) -> Result<Copied> {
    // What the mirror has now, objects and all, under refs/mirror/.
    let fetch = [
        "fetch",
        "--no-write-fetch-head",
        "--no-tags",
        "--prune",
        "--",
        url,
        "+refs/heads/*:refs/mirror/heads/*",
        "+refs/tags/*:refs/mirror/tags/*",
    ];
    git_net(git_dir, &fetch, COPY_LIMIT).with_context(|| format!("reading {url}"))?;
    let here = local_refs(git_dir, "refs/heads", "refs/tags")?;
    let there: BTreeMap<String, String> = local_refs(git_dir, "refs/mirror/heads", "refs/mirror/tags")?
        .into_iter()
        .map(|(name, sha)| (format!("refs/{}", &name[SEEN.len()..]), sha))
        .collect();

    let mut done = Copied::default();
    let mut leases: Vec<String> = Vec::new();
    let mut refspecs: Vec<String> = Vec::new();
    let mut planned: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (name, sha) in &here {
        match there.get(name) {
            Some(theirs) if theirs == sha => {
                done.copied.insert(name.clone(), sha.clone());
            }
            Some(theirs) => {
                let ours_contains_theirs = is_ancestor(git_dir, theirs, sha);
                let unmoved_since_last_copy = last.get(name) == Some(theirs);
                if ours_contains_theirs || unmoved_since_last_copy {
                    leases.push(format!("--force-with-lease={name}:{theirs}"));
                    refspecs.push(format!("+{name}:{name}"));
                    planned.insert(name.clone(), Some(sha.clone()));
                } else {
                    done.refused.push(name.clone());
                    if let Some(prev) = last.get(name) {
                        done.copied.insert(name.clone(), prev.clone());
                    }
                }
            }
            None => {
                leases.push(format!("--force-with-lease={name}:"));
                refspecs.push(format!("{name}:{name}"));
                planned.insert(name.clone(), Some(sha.clone()));
            }
        }
    }
    for (name, theirs) in &there {
        if here.contains_key(name) {
            continue;
        }
        if last.get(name) == Some(theirs) {
            // Copied there, unmoved since, gone here: deleted here.
            leases.push(format!("--force-with-lease={name}:{theirs}"));
            refspecs.push(format!(":{name}"));
            planned.insert(name.clone(), None);
        } else {
            done.mirror_only.push(name.clone());
        }
    }

    if !refspecs.is_empty() {
        let mut args: Vec<&str> = vec!["push", "--porcelain"];
        args.extend(leases.iter().map(String::as_str));
        args.push("--");
        args.push(url);
        args.extend(refspecs.iter().map(String::as_str));
        // A stale lease or a rejected ref fails the push as a whole; say which.
        let out = match git_net(git_dir, &args, COPY_LIMIT) {
            Ok(out) => out,
            Err(e) => bail!("pushing to {url}: {e:#}"),
        };
        let rejected: Vec<&str> = out.lines().filter(|l| l.starts_with('!')).collect();
        if !rejected.is_empty() {
            bail!("{url} turned down: {}", rejected.join("; "));
        }
        for (name, sha) in planned {
            done.changed += 1;
            if let Some(sha) = sha {
                done.copied.insert(name, sha);
            }
        }
    }
    Ok(done)
}

/// Why a copy left refs alone, and what to do about it.
fn refused_message(refused: &[String]) -> String {
    let names: Vec<&str> = refused.iter().map(|r| r.strip_prefix("refs/heads/").or_else(|| r.strip_prefix("refs/tags/")).unwrap_or(r)).collect();
    let first = refused.first().map(String::as_str).unwrap_or("");
    let seen = format!("{SEEN}{}", first.strip_prefix("refs/").unwrap_or(first));
    format!(
        "the mirror has commits this side doesn't on {}, so those weren't copied (nothing was overwritten). \
         They're here as {seen}: merge them in a clone with `git fetch origin {seen} && git merge FETCH_HEAD`, then push.",
        names.join(", ")
    )
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
    let result = copy(git_dir, &url, &s.copied);
    let mut s = load(git_dir);
    match result {
        Ok(done) => {
            s.copied = done.copied;
            s.mirror_only = done.mirror_only;
            if done.refused.is_empty() {
                s.pending = false;
                s.last_ok = Some(now());
                s.last_error = None;
                s.notified = false;
            } else {
                // Stays owed: once the commits are merged here, the next try copies.
                let why = refused_message(&done.refused);
                eprintln!("mirroring {} to {url}: {why}", git_dir.display());
                if s.last_error.as_deref() != Some(&why) {
                    s.notified = false;
                }
                s.last_error = Some(why);
            }
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

    /// A bare "here", a bare "there" (GitHub), and a clone of each to make commits in.
    struct Pair {
        _tmp: tempfile::TempDir,
        here: PathBuf,
        there: PathBuf,
        work: PathBuf,
        theirs: PathBuf,
    }

    fn pair() -> Pair {
        let tmp = tempfile::tempdir().unwrap();
        let p = |n: &str| tmp.path().join(n);
        let (here, there, work, theirs) = (p("here.git"), p("there.git"), p("work"), p("theirs"));
        git(tmp.path(), &["init", "-q", "--bare", "here.git"]);
        git(tmp.path(), &["init", "-q", "--bare", "there.git"]);
        git(tmp.path(), &["clone", "-q", here.to_str().unwrap(), "work"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&work, &["push", "-q", "origin", "HEAD:refs/heads/main", "HEAD:refs/heads/side"]);
        set_url(&here, Some(there.to_str().unwrap())).unwrap();
        let mirrors = Arc::new(Mirrors::default());
        mirrors.copy_now(&here).unwrap();
        assert_eq!(status("r", &here).unwrap().state, "ok");
        git(tmp.path(), &["clone", "-q", "-b", "main", there.to_str().unwrap(), "theirs"]);
        Pair { _tmp: tmp, here, there, work, theirs }
    }

    #[test]
    fn a_commit_only_the_mirror_has_is_never_overwritten() {
        let p = pair();
        let mirrors = Arc::new(Mirrors::default());
        // Someone pushes to GitHub's main; meanwhile main moves here too.
        git(&p.theirs, &["commit", "-q", "--allow-empty", "-m", "theirs"]);
        git(&p.theirs, &["push", "-q", "origin", "HEAD:main"]);
        let theirs = git(&p.theirs, &["rev-parse", "HEAD"]);
        git(&p.work, &["commit", "-q", "--allow-empty", "-m", "ours"]);
        git(&p.work, &["push", "-q", "origin", "HEAD:main", "HEAD:side"]);
        mirrors.copy_now(&p.here).unwrap();

        // main is left alone there, side (unchanged there) is copied, and it says so.
        assert_eq!(git(&p.there, &["rev-parse", "main"]), theirs, "their commit is still there");
        assert_eq!(git(&p.there, &["rev-parse", "side"]), git(&p.work, &["rev-parse", "HEAD"]));
        let st = status("r", &p.here).unwrap();
        assert_eq!(st.state, "failing");
        assert!(st.last_error.as_deref().is_some_and(|e| e.contains("main") && e.contains("refs/mirror/heads/main")), "{:?}", st.last_error);
        // Their commit is here to merge.
        assert_eq!(git(&p.here, &["rev-parse", "refs/mirror/heads/main"]), theirs);

        // Merge it in a clone and push: the next copy goes through.
        git(&p.work, &["fetch", "-q", "origin", "refs/mirror/heads/main"]);
        git(&p.work, &["merge", "-q", "--no-edit", "FETCH_HEAD"]);
        git(&p.work, &["push", "-q", "origin", "HEAD:main"]);
        mirrors.copy_now(&p.here).unwrap();
        assert_eq!(status("r", &p.here).unwrap().state, "ok");
        assert_eq!(git(&p.there, &["rev-parse", "main"]), git(&p.work, &["rev-parse", "HEAD"]));
    }

    #[test]
    fn a_rewrite_here_is_copied_when_the_mirror_has_not_moved() {
        let p = pair();
        let mirrors = Arc::new(Mirrors::default());
        git(&p.work, &["commit", "-q", "--amend", "--allow-empty", "-m", "one, reworded"]);
        git(&p.work, &["push", "-q", "--force", "origin", "HEAD:main"]);
        mirrors.copy_now(&p.here).unwrap();
        assert_eq!(status("r", &p.here).unwrap().state, "ok");
        assert_eq!(git(&p.there, &["rev-parse", "main"]), git(&p.work, &["rev-parse", "HEAD"]));
    }

    #[test]
    fn a_branch_made_on_the_mirror_stays_and_one_deleted_here_goes() {
        let p = pair();
        let mirrors = Arc::new(Mirrors::default());
        git(&p.theirs, &["push", "-q", "origin", "HEAD:refs/heads/from-github"]);
        git(&p.work, &["push", "-q", "origin", ":side"]);
        mirrors.copy_now(&p.here).unwrap();
        let refs = git(&p.there, &["for-each-ref", "--format=%(refname)"]);
        assert!(refs.contains("refs/heads/from-github"), "made there, so it stays: {refs}");
        assert!(!refs.contains("refs/heads/side"), "deleted here, so it goes: {refs}");
        let st = status("r", &p.here).unwrap();
        assert_eq!((st.state.as_str(), st.mirror_only.as_slice()), ("ok", &["refs/heads/from-github".to_string()][..]));
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
