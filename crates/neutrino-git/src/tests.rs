//! Builds real repositories with the `git` CLI and checks what the engine reads.

use super::*;
use neutrino_proto::{LineKind, LogQuery};
use std::cell::Cell;
use std::path::PathBuf;
use std::process::Command;

struct Fixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
    clock: Cell<i64>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("work");
        std::fs::create_dir(&path).unwrap();
        let f = Fixture { _dir: dir, path, clock: Cell::new(1_700_000_000) };
        f.git(&["init", "-q", "-b", "main"]);
        f
    }

    fn git(&self, args: &[&str]) -> String {
        self.git_in(&self.path, args)
    }

    fn git_in(&self, dir: &Path, args: &[&str]) -> String {
        // Each commit one minute after the last, so newest-first order is fixed.
        let t = self.clock.get() + 60;
        self.clock.set(t);
        let date = format!("{t} +0000");
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Ada")
            .env("GIT_AUTHOR_EMAIL", "ada@example.com")
            .env("GIT_COMMITTER_NAME", "Ada")
            .env("GIT_COMMITTER_EMAIL", "ada@example.com")
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn write(&self, name: &str, contents: &[u8]) {
        let p = self.path.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, contents).unwrap();
    }

    fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn repo(&self) -> gix::Repository {
        local(&discover(&self.path).unwrap())
    }
}

fn q(all: bool) -> LogQuery {
    LogQuery { repo: "t".into(), all, ..Default::default() }
}

#[test]
fn empty_repository() {
    let f = Fixture::new();
    let repo = f.repo();
    assert_eq!(summary("t", &repo).unwrap().head, Some(Head::Branch { name: "main".into(), sha: None }));
    let page = log(&repo, &q(false)).unwrap();
    assert!(page.commits.is_empty() && !page.has_more);
    assert!(branches(&repo).unwrap().is_empty());
}

#[test]
fn discovers_from_a_subfolder() {
    let f = Fixture::new();
    f.write("src/a.txt", b"a\n");
    f.commit("first");
    let repo = local(&discover(&f.path.join("src")).unwrap());
    let s = summary("t", &repo).unwrap();
    assert_eq!(s.name, "work");
    assert!(!s.bare);
    assert_eq!(Path::new(&s.path).canonicalize().unwrap(), f.path.canonicalize().unwrap());
}

#[test]
fn history_with_a_merge() {
    let f = Fixture::new();
    f.write("a.txt", b"one\n");
    let a = f.commit("A");
    f.git(&["checkout", "-q", "-b", "side"]);
    f.write("side.txt", b"side\n");
    let s = f.commit("S: side work\n\nWith a body.");
    f.git(&["checkout", "-q", "main"]);
    f.write("a.txt", b"two\n");
    let b = f.commit("B");
    f.git(&["merge", "-q", "--no-ff", "side", "-m", "M"]);
    let m = f.git(&["rev-parse", "HEAD"]);
    f.git(&["tag", "v1", &b]);

    let repo = f.repo();
    let page = log(&repo, &q(false)).unwrap();
    let subjects: Vec<_> = page.commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["M", "B", "S: side work", "A"]);
    assert_eq!(page.commits[0].sha, m);
    assert_eq!(page.commits[0].parents, vec![b.clone(), s.clone()]);
    assert_eq!(page.commits.last().unwrap().sha, a);
    assert_eq!(page.lanes, 2);
    assert_eq!(page.commits[0].author.name, "Ada");

    let m_refs = &page.commits[0].refs;
    assert_eq!(m_refs[0], RefLabel { name: "main".into(), kind: RefKind::Local, is_head: true });
    let b_refs: Vec<_> = page.commits[1].refs.iter().map(|r| (r.name.as_str(), r.kind)).collect();
    assert_eq!(b_refs, [("v1", RefKind::Tag)]);
    let s_row = page.commits[2].graph.as_ref().unwrap();
    assert_eq!(s_row.lane, 1, "the side branch is drawn in its own lane");

    // Paging keeps the graph consistent with the full walk.
    let p2 = log(&repo, &LogQuery { skip: 2, limit: Some(1), ..q(false) }).unwrap();
    assert_eq!(p2.commits.len(), 1);
    assert!(p2.has_more);
    assert_eq!(p2.commits[0].graph, page.commits[2].graph);

    // Filtering finds the body text and drops the graph.
    let found = log(&repo, &LogQuery { query: Some("WITH A BODY".into()), ..q(false) }).unwrap();
    assert_eq!(found.commits.len(), 1);
    assert_eq!(found.commits[0].sha, s);
    assert!(found.commits[0].graph.is_none());
    let by_sha = log(&repo, &LogQuery { query: Some(a[..8].to_string()), ..q(false) }).unwrap();
    assert_eq!(by_sha.commits[0].sha, a);

    let detail = commit(&repo, &s).unwrap();
    assert_eq!(detail.subject, "S: side work");
    assert_eq!(detail.body, "With a body.");
}

#[test]
fn all_refs_and_rev() {
    let f = Fixture::new();
    f.commit("base");
    f.git(&["checkout", "-q", "-b", "feature"]);
    f.commit("feature work");
    f.git(&["checkout", "-q", "main"]);
    let repo = f.repo();
    assert_eq!(log(&repo, &q(false)).unwrap().commits.len(), 1);
    assert_eq!(log(&repo, &q(true)).unwrap().commits.len(), 2);
    let feature = log(&repo, &LogQuery { rev: Some("feature".into()), ..q(false) }).unwrap();
    assert_eq!(feature.commits[0].subject, "feature work");
    assert!(log(&repo, &LogQuery { rev: Some("nope".into()), ..q(false) }).is_err());
}

#[test]
fn branches_with_upstream() {
    let f = Fixture::new();
    f.commit("one");
    let origin = f.path.parent().unwrap().join("origin.git");
    f.git(&["init", "-q", "--bare", origin.to_str().unwrap()]);
    f.git(&["remote", "add", "origin", origin.to_str().unwrap()]);
    f.git(&["push", "-q", "-u", "origin", "main"]);
    f.commit("two");
    f.commit("three");
    f.git(&["branch", "local-only"]);

    let repo = f.repo();
    let list = branches(&repo).unwrap();
    let main = list.iter().find(|b| b.name == "main").unwrap();
    assert!(main.is_head);
    assert_eq!(list[0].name, "main", "the checked-out branch comes first");
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    assert_eq!((main.ahead, main.behind), (Some(2), Some(0)));
    assert_eq!(main.subject, "three");
    let other = list.iter().find(|b| b.name == "local-only").unwrap();
    assert_eq!((other.upstream.as_deref(), other.ahead), (None, None));
    let remote = list.iter().find(|b| b.name == "origin/main").unwrap();
    assert_eq!(remote.kind, RefKind::Remote);
    assert_eq!(remote.subject, "one");
}

#[test]
fn commit_files_and_diffs() {
    let f = Fixture::new();
    let body: String = (1..=40).map(|i| format!("line {i}\n")).collect();
    f.write("keep.txt", b"a\nb\nc\n");
    f.write("old_name.txt", body.as_bytes());
    f.write("gone.txt", b"bye\n");
    let root = f.commit("root");

    f.git(&["mv", "old_name.txt", "new_name.txt"]);
    f.write("new_name.txt", body.replace("line 20\n", "line twenty\n").as_bytes());
    f.write("keep.txt", b"a\nB\nc\nd\n");
    std::fs::remove_file(f.path.join("gone.txt")).unwrap();
    f.write("image.bin", b"\x89PNG\0\0\x01\x02");
    let second = f.commit("second");

    let repo = f.repo();
    let root_detail = commit(&repo, &root).unwrap();
    assert!(root_detail.parents.is_empty());
    assert!(root_detail.files.iter().all(|c| c.status == ChangeStatus::Added));
    assert_eq!(root_detail.files.len(), 3);

    let d = commit(&repo, &second[..7]).unwrap();
    assert_eq!(d.sha, second);
    let by_path: HashMap<_, _> = d.files.iter().map(|c| (c.path.as_str(), c)).collect();
    assert_eq!(by_path["new_name.txt"].status, ChangeStatus::Renamed);
    assert_eq!(by_path["new_name.txt"].old_path.as_deref(), Some("old_name.txt"));
    assert_eq!((by_path["new_name.txt"].additions, by_path["new_name.txt"].deletions), (1, 1));
    assert_eq!((by_path["keep.txt"].additions, by_path["keep.txt"].deletions), (2, 1));
    assert_eq!(by_path["gone.txt"].status, ChangeStatus::Deleted);
    assert!(by_path["image.bin"].binary);

    let cache = DiffCache::default();
    let keep = file_diff(&repo, &second, "keep.txt", 3, &cache).unwrap();
    assert_eq!(keep.hunks.len(), 1);
    let kinds: Vec<_> = keep.hunks[0].lines.iter().map(|l| (l.kind, l.text.as_str())).collect();
    assert_eq!(
        kinds,
        [(LineKind::Context, "a"), (LineKind::Removed, "b"), (LineKind::Added, "B"), (LineKind::Context, "c"), (LineKind::Added, "d")]
    );

    let renamed = file_diff(&repo, &second, "new_name.txt", 3, &cache).unwrap();
    assert_eq!(renamed.old_path.as_deref(), Some("old_name.txt"));
    assert_eq!((renamed.hunks[0].old_start, renamed.hunks[0].new_start), (17, 17));
    let again = file_diff(&repo, &second, "new_name.txt", 3, &cache).unwrap();
    assert_eq!(again, renamed, "a cached diff is the same diff");

    assert!(file_diff(&repo, &second, "image.bin", 3, &cache).unwrap().binary);
    let gone = file_diff(&repo, &second, "gone.txt", 3, &cache).unwrap();
    assert_eq!(gone.deletions, 1);
    assert!(file_diff(&repo, &second, "nope.txt", 3, &cache).is_err());
}

#[test]
fn detached_head() {
    let f = Fixture::new();
    let first = f.commit("first");
    f.commit("second");
    f.git(&["checkout", "-q", "--detach", &first]);
    let repo = f.repo();
    assert_eq!(summary("t", &repo).unwrap().head, Some(Head::Detached { sha: first.clone() }));
    let page = log(&repo, &q(false)).unwrap();
    assert_eq!(page.commits[0].refs[0], RefLabel { name: "HEAD".into(), kind: RefKind::Head, is_head: true });
}

#[test]
fn messages() {
    assert_eq!(split_message("Subject\n\nBody\nmore\n".into()), ("Subject".into(), "Body\nmore".into()));
    assert_eq!(split_message("Wrapped\nsubject line\n".into()), ("Wrapped subject line".into(), String::new()));
    assert_eq!(split_message("".into()), (String::new(), String::new()));
}
