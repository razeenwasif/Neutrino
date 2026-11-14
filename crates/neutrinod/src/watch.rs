//! Watches registered working copies (FSEvents, through `notify`) and turns
//! bursts of file events into one `RepoEvent` per repo and kind.

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use neutrino_proto::RepoEvent;
use notify::{RecursiveMode, Watcher as _};
use tokio::sync::{broadcast, mpsc};

/// How long a burst of events is gathered before subscribers hear about it.
const QUIET: Duration = Duration::from_millis(200);

/// Folders whose churn never changes what Neutrino shows. Build output is
/// usually ignored by git too, and watching it would refresh status constantly.
const SKIP_DIRS: &[&str] = &["node_modules", "target", ".build", "DerivedData", ".next", "__pycache__", ".venv"];

/// Which kind of change a path is, or `None` when it doesn't matter.
pub fn classify(root: &Path, git_dir: &Path, path: &Path) -> Option<&'static str> {
    if let Ok(inside) = path.strip_prefix(git_dir) {
        let first = inside.components().next().and_then(|c| c.as_os_str().to_str()).unwrap_or("");
        return match first {
            "index" | "HEAD" | "refs" | "packed-refs" | "MERGE_HEAD" | "CHERRY_PICK_HEAD" | "REVERT_HEAD" | "rebase-merge" | "rebase-apply" => Some("refs"),
            _ => None,
        };
    }
    let rel = path.strip_prefix(root).ok()?;
    let skipped = rel.components().any(|c| match c {
        Component::Normal(n) => n.to_str().is_some_and(|n| SKIP_DIRS.contains(&n) || n == ".git"),
        _ => false,
    });
    if skipped { None } else { Some("worktree") }
}

pub struct Watchers {
    events: broadcast::Sender<RepoEvent>,
    active: Mutex<HashMap<String, notify::RecommendedWatcher>>,
}

impl Watchers {
    pub fn new() -> Self {
        Watchers { events: broadcast::channel(256).0, active: Mutex::new(HashMap::new()) }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RepoEvent> {
        self.events.subscribe()
    }

    /// Tell subscribers about something that isn't a file change (a push landed).
    pub fn notify(&self, event: RepoEvent) {
        let _ = self.events.send(event);
    }

    /// Start watching a repo (again). Must be called inside the Tokio runtime.
    pub fn watch(&self, id: &str, root: PathBuf, git_dir: PathBuf) {
        let (tx, mut rx) = mpsc::unbounded_channel::<&'static str>();
        let (root_c, git_c) = (root.clone(), git_dir.clone());
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                for p in &event.paths {
                    if let Some(kind) = classify(&root_c, &git_c, p) {
                        let _ = tx.send(kind);
                    }
                }
            }
        });
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(e) => {
                eprintln!("can't watch {}: {e}", root.display());
                return;
            }
        };
        if let Err(e) = watcher.watch(&root, RecursiveMode::Recursive) {
            eprintln!("can't watch {}: {e}", root.display());
            return;
        }
        // A git dir outside the working tree (a linked worktree) is watched too.
        if !git_dir.starts_with(&root) {
            let _ = watcher.watch(&git_dir, RecursiveMode::Recursive);
        }

        let events = self.events.clone();
        let repo = id.to_string();
        tokio::spawn(async move {
            while let Some(first) = rx.recv().await {
                tokio::time::sleep(QUIET).await;
                let mut kinds = BTreeSet::from([first]);
                while let Ok(k) = rx.try_recv() {
                    kinds.insert(k);
                }
                for kind in kinds {
                    let _ = events.send(RepoEvent { repo: repo.clone(), kind: kind.to_string() });
                }
            }
            // The watcher was dropped: the repo was removed or re-watched.
        });
        self.active.lock().unwrap().insert(id.to_string(), watcher);
    }

    pub fn unwatch(&self, id: &str) {
        self.active.lock().unwrap().remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::classify;
    use std::path::Path;

    #[test]
    fn what_counts() {
        let (root, git) = (Path::new("/r"), Path::new("/r/.git"));
        assert_eq!(classify(root, git, Path::new("/r/src/main.rs")), Some("worktree"));
        assert_eq!(classify(root, git, Path::new("/r/.git/index")), Some("refs"));
        assert_eq!(classify(root, git, Path::new("/r/.git/refs/heads/main")), Some("refs"));
        assert_eq!(classify(root, git, Path::new("/r/.git/objects/ab/cdef")), None);
        assert_eq!(classify(root, git, Path::new("/r/.git/index.lock")), None);
        assert_eq!(classify(root, git, Path::new("/r/target/debug/x")), None);
        assert_eq!(classify(root, git, Path::new("/r/web/node_modules/a/b.js")), None);
        assert_eq!(classify(root, git, Path::new("/elsewhere/x")), None);
    }
}
