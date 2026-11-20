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
    f.git(&["checkout", "-q", "-b", "unmerged"]);
    f.commit("only here");
    f.git(&["checkout", "-q", "main"]);

    let repo = f.repo();
    let list = branches(&repo).unwrap();
    let merged = |n: &str| list.iter().find(|b| b.name == n).unwrap().merged;
    assert_eq!(merged("main"), None, "the default branch isn't merged into itself");
    assert_eq!(merged("origin/main"), None, "nor is its remote copy");
    assert_eq!(merged("local-only"), Some(true), "its tip is main's");
    assert_eq!(merged("unmerged"), Some(false));
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

// ---------------------------------------------------------------------------
// Working copy (milestone 2)
// ---------------------------------------------------------------------------

mod working_copy {
    use super::*;
    use crate::worktree::{self, *};
    use neutrino_proto::{Area, LinePick, WorkingStatus};

    fn names(list: &[neutrino_proto::StatusFile]) -> Vec<(String, ChangeStatus)> {
        list.iter().map(|f| (f.path.clone(), f.status)).collect()
    }

    /// Our status, as `git status --porcelain=v1` letters, to compare with git's own.
    fn porcelain(st: &WorkingStatus) -> Vec<String> {
        let letter = |s: ChangeStatus| match s {
            ChangeStatus::Added => 'A',
            ChangeStatus::Modified => 'M',
            ChangeStatus::Deleted => 'D',
            ChangeStatus::Renamed => 'R',
            ChangeStatus::Copied => 'C',
            ChangeStatus::TypeChanged => 'T',
            ChangeStatus::Untracked => '?',
            ChangeStatus::Conflicted => 'U',
        };
        let mut rows: std::collections::BTreeMap<String, (char, char)> = Default::default();
        for f in &st.staged {
            rows.entry(f.path.clone()).or_insert((' ', ' ')).0 = letter(f.status);
        }
        for f in &st.unstaged {
            rows.entry(f.path.clone()).or_insert((' ', ' ')).1 = letter(f.status);
        }
        let mut out: Vec<String> = rows.into_iter().map(|(p, (x, y))| format!("{x}{y} {p}")).collect();
        out.extend(st.untracked.iter().map(|f| format!("?? {}", f.path)));
        out.sort();
        out
    }

    fn git_porcelain(f: &Fixture) -> Vec<String> {
        let out = f.git(&["status", "--porcelain=v1", "--untracked-files=all", "--no-renames"]);
        let mut rows: Vec<String> = out.lines().map(str::to_string).filter(|l| !l.is_empty()).collect();
        rows.sort();
        rows
    }

    fn base() -> Fixture {
        let f = Fixture::new();
        let body: String = (1..=12).map(|i| format!("line {i}\n")).collect();
        f.write("a.txt", body.as_bytes());
        f.write("b.txt", b"keep\n");
        f.write("gone.txt", b"bye\n");
        f.commit("base");
        f
    }

    #[test]
    fn status_matches_git() {
        let f = base();
        f.write("a.txt", b"changed\n");
        f.git(&["add", "a.txt"]);
        f.write("a.txt", b"changed again\n");
        f.write("b.txt", b"edited\n");
        std::fs::remove_file(f.path.join("gone.txt")).unwrap();
        f.write("new/deep/file.txt", b"hello\n");
        f.write("staged-new.txt", b"x\n");
        f.git(&["add", "staged-new.txt"]);
        f.write(".gitignore", b"ignored.log\n");
        f.write("ignored.log", b"noise\n");

        let st = status(&f.repo()).unwrap();
        assert_eq!(porcelain(&st), git_porcelain(&f));
        assert_eq!(names(&st.staged), [("a.txt".into(), ChangeStatus::Modified), ("staged-new.txt".into(), ChangeStatus::Added)]);
        assert!(st.untracked.iter().any(|u| u.path == "new/deep/file.txt"), "untracked files are listed one by one");
        assert!(!st.untracked.iter().any(|u| u.path == "ignored.log"));
        let b = st.unstaged.iter().find(|u| u.path == "b.txt").unwrap();
        assert_eq!((b.additions, b.deletions), (1, 1));
        assert_eq!(st.head, Some(Head::Branch { name: "main".into(), sha: Some(f.git(&["rev-parse", "HEAD"])) }));
    }

    #[test]
    fn stage_unstage_and_rename() {
        let f = base();
        f.write("b.txt", b"edited\n");
        f.write("fresh.txt", b"new\n");
        std::fs::remove_file(f.path.join("gone.txt")).unwrap();
        let repo = f.repo();
        stage(&repo, &["b.txt".into(), "fresh.txt".into(), "gone.txt".into()]).unwrap();
        let st = status(&f.repo()).unwrap();
        assert_eq!(st.staged.len(), 3);
        assert!(st.unstaged.is_empty() && st.untracked.is_empty());

        unstage(&f.repo(), &["fresh.txt".into(), "gone.txt".into()]).unwrap();
        let st = status(&f.repo()).unwrap();
        assert_eq!(names(&st.staged), [("b.txt".into(), ChangeStatus::Modified)]);
        assert_eq!(porcelain(&st), git_porcelain(&f));

        // A staged rename unstages from both paths.
        f.git(&["mv", "a.txt", "moved.txt"]);
        let st = status(&f.repo()).unwrap();
        let moved = st.staged.iter().find(|s| s.path == "moved.txt").unwrap();
        assert_eq!((moved.status, moved.old_path.as_deref()), (ChangeStatus::Renamed, Some("a.txt")));
        unstage(&f.repo(), &["moved.txt".into()]).unwrap();
        let st = status(&f.repo()).unwrap();
        assert!(!st.staged.iter().any(|s| s.path == "moved.txt" || s.path == "a.txt"));
        assert!(check_path_rejects());
    }

    fn check_path_rejects() -> bool {
        let f = base();
        stage(&f.repo(), &["../outside".into()]).is_err() && stage(&f.repo(), &[".git/config".into()]).is_err()
    }

    #[test]
    fn stage_and_unstage_lines() {
        let f = base();
        // Change line 2 and line 11: two hunks far apart.
        let body: String = (1..=12).map(|i| match i { 2 => "TWO\n".into(), 11 => "ELEVEN\n".into(), _ => format!("line {i}\n") }).collect();
        f.write("a.txt", body.as_bytes());

        let d = work_diff(&f.repo(), "a.txt", Area::Unstaged, 3).unwrap();
        assert_eq!(d.hunks.len(), 2);
        // Stage only the change to line 2.
        let pick = [LinePick { kind: LineKind::Removed, line: 2 }, LinePick { kind: LineKind::Added, line: 2 }];
        stage_lines(&f.repo(), "a.txt", Area::Unstaged, &pick).unwrap();
        let cached = f.git(&["diff", "--cached", "-U0"]);
        assert!(cached.contains("-line 2") && cached.contains("+TWO") && !cached.contains("ELEVEN"), "{cached}");
        let unstaged = f.git(&["diff", "-U0"]);
        assert!(unstaged.contains("+ELEVEN") && !unstaged.contains("TWO"), "{unstaged}");

        // Stage just the added half of the other change, keeping line 11's old text in the index.
        stage_lines(&f.repo(), "a.txt", Area::Unstaged, &[LinePick { kind: LineKind::Added, line: 11 }]).unwrap();
        let index = f.git(&["show", ":a.txt"]);
        assert!(index.contains("line 11\nELEVEN"), "{index}");

        // Unstage the change to line 2 again.
        stage_lines(&f.repo(), "a.txt", Area::Staged, &pick).unwrap();
        let index = f.git(&["show", ":a.txt"]);
        assert!(index.contains("line 2\n") && !index.contains("TWO"), "{index}");
        // The working tree never moved.
        assert_eq!(std::fs::read_to_string(f.path.join("a.txt")).unwrap(), body);

        // A stale pick is refused rather than guessed at.
        let err = stage_lines(&f.repo(), "a.txt", Area::Unstaged, &[LinePick { kind: LineKind::Added, line: 99 }]).unwrap_err();
        assert!(err.0.contains("changed since"), "{err}");
    }

    #[test]
    fn stage_part_of_a_new_file() {
        let f = base();
        f.write("new.txt", b"one\ntwo\nthree\n");
        stage_lines(&f.repo(), "new.txt", Area::Unstaged, &[LinePick { kind: LineKind::Added, line: 1 }, LinePick { kind: LineKind::Added, line: 3 }]).unwrap();
        assert_eq!(f.git(&["show", ":new.txt"]), "one\nthree");
        let st = status(&f.repo()).unwrap();
        assert_eq!(names(&st.staged), [("new.txt".into(), ChangeStatus::Added)]);
        assert_eq!(names(&st.unstaged), [("new.txt".into(), ChangeStatus::Modified)]);
    }

    #[test]
    fn discards_keep_a_copy() {
        let f = base();
        let log = f.path.parent().unwrap().join("state/discarded.log");
        let body: String = (1..=12).map(|i| match i { 2 => "TWO\n".into(), 11 => "ELEVEN\n".into(), _ => format!("line {i}\n") }).collect();
        f.write("a.txt", body.as_bytes());
        discard_lines(&f.repo(), "a.txt", &[LinePick { kind: LineKind::Added, line: 11 }, LinePick { kind: LineKind::Removed, line: 11 }], &log).unwrap();
        let now = std::fs::read_to_string(f.path.join("a.txt")).unwrap();
        assert!(now.contains("TWO") && now.contains("line 11") && !now.contains("ELEVEN"), "{now}");

        f.write("b.txt", b"edited\n");
        f.write("scratch.txt", b"draft\n");
        discard(&f.repo(), &["b.txt".into(), "scratch.txt".into()], &log).unwrap();
        assert_eq!(std::fs::read_to_string(f.path.join("b.txt")).unwrap(), "keep\n");
        assert!(!f.path.join("scratch.txt").exists());

        // Everything thrown away can be read back from the log.
        let entries = std::fs::read_to_string(&log).unwrap();
        assert_eq!(entries.lines().count(), 3);
        let scratch_sha = entries.lines().find(|l| l.contains("scratch.txt")).unwrap().split('\t').last().unwrap().to_string();
        assert_eq!(f.git(&["cat-file", "-p", &scratch_sha]), "draft");
    }

    #[test]
    fn commit_and_amend() {
        let f = base();
        assert!(worktree::commit(&f.repo(), "  ", false).is_err());
        assert!(worktree::commit(&f.repo(), "nothing staged", false).is_err(), "git refuses an empty commit");
        f.write("b.txt", b"edited\n");
        stage(&f.repo(), &["b.txt".into()]).unwrap();
        // A hook's output comes back with the commit.
        let hook = f.path.join(".git/hooks/post-commit");
        std::fs::write(&hook, "#!/bin/sh\necho hook ran\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (sha, output) = worktree::commit(&f.repo(), "Edit b\n\nWith a body.", false).unwrap();
        assert_eq!(sha, f.git(&["rev-parse", "HEAD"]));
        assert!(output.contains("hook ran"), "{output:?}");
        assert_eq!(f.git(&["log", "-1", "--format=%B"]), "Edit b\n\nWith a body.");
        let (amended, _) = worktree::commit(&f.repo(), "Edit b, better", true).unwrap();
        assert_ne!(amended, sha);
        assert_eq!(f.git(&["rev-list", "--count", "HEAD"]), "2");
        assert!(status(&f.repo()).unwrap().staged.is_empty());
    }

    #[test]
    fn branch_operations() {
        let f = base();
        create_branch(&f.repo(), "feature", None, true).unwrap();
        assert_eq!(f.git(&["branch", "--show-current"]), "feature");
        f.write("b.txt", b"feature work\n");
        f.commit("feature work");
        checkout(&f.repo(), "main").unwrap();
        // Unmerged: a plain delete is refused, a forced one isn't.
        let err = delete_branch(&f.repo(), "feature", false).unwrap_err();
        assert!(err.0.contains("not fully merged"), "{err}");
        rename_branch(&f.repo(), "feature", "feature-2").unwrap();
        delete_branch(&f.repo(), "feature-2", true).unwrap();
        assert!(!f.git(&["branch"]).contains("feature"));

        // A checkout that would lose changes is refused by git.
        create_branch(&f.repo(), "other", None, false).unwrap();
        checkout(&f.repo(), "other").unwrap();
        f.write("b.txt", b"other\n");
        f.commit("other");
        checkout(&f.repo(), "main").unwrap();
        f.write("b.txt", b"uncommitted\n");
        assert!(checkout(&f.repo(), "other").is_err());
        assert_eq!(f.git(&["branch", "--show-current"]), "main");

        // A remote branch is checked out as a tracking branch.
        let origin = f.path.parent().unwrap().join("origin.git");
        f.git(&["init", "-q", "--bare", origin.to_str().unwrap()]);
        f.git(&["remote", "add", "origin", origin.to_str().unwrap()]);
        f.git(&["push", "-q", "origin", "other"]);
        f.git(&["branch", "-D", "other"]);
        f.git(&["stash", "-q"]);
        checkout(&f.repo(), "origin/other").unwrap();
        assert_eq!(f.git(&["rev-parse", "--abbrev-ref", "other@{upstream}"]), "origin/other");
    }

    #[test]
    fn conflicts() {
        let f = base();
        f.git(&["checkout", "-q", "-b", "side"]);
        f.write("b.txt", b"side\n");
        f.commit("side");
        f.git(&["checkout", "-q", "main"]);
        f.write("b.txt", b"main\n");
        f.commit("main");
        let _ = Command::new("git").args(["merge", "side"]).current_dir(&f.path).env("GIT_CONFIG_GLOBAL", "/dev/null").output();
        let st = status(&f.repo()).unwrap();
        assert_eq!(st.operation.as_deref(), Some("merge"));
        assert_eq!(names(&st.conflicted), [("b.txt".into(), ChangeStatus::Conflicted)]);
        assert!(st.staged.iter().chain(&st.unstaged).all(|s| s.path != "b.txt"));
        let d = work_diff(&f.repo(), "b.txt", Area::Unstaged, 3).unwrap();
        assert_eq!(d.status, ChangeStatus::Conflicted);
        assert!(d.hunks.iter().flat_map(|h| &h.lines).any(|l| l.text.starts_with("<<<<<<<")));
        // Staging the resolved file clears the conflict.
        f.write("b.txt", b"resolved\n");
        stage(&f.repo(), &["b.txt".into()]).unwrap();
        assert!(status(&f.repo()).unwrap().conflicted.is_empty());
    }

    #[test]
    fn unborn_repository() {
        let f = Fixture::new();
        f.write("first.txt", b"hi\n");
        stage(&f.repo(), &["first.txt".into()]).unwrap();
        let st = status(&f.repo()).unwrap();
        assert_eq!(names(&st.staged), [("first.txt".into(), ChangeStatus::Added)]);
        unstage(&f.repo(), &["first.txt".into()]).unwrap();
        assert_eq!(names(&status(&f.repo()).unwrap().untracked), [("first.txt".into(), ChangeStatus::Untracked)]);
        assert!(worktree::git_binary().exists() || worktree::git_binary() == std::path::Path::new("git"));
    }
}

mod forge {
    use super::*;
    use crate::forge::{self, Identity};
    use neutrino_proto::MergeStrategy;

    fn server() -> Identity {
        Identity { name: "Neutrino".into(), email: "neutrino@localhost".into() }
    }

    /// main: a.txt; feature adds b.txt and edits a.txt's last line; main later adds c.txt.
    fn setup() -> (Fixture, PathBuf) {
        let f = Fixture::new();
        f.write("a.txt", b"1\n2\n3\n4\n5\n");
        f.commit("base");
        f.git(&["checkout", "-q", "-b", "feature"]);
        f.write("b.txt", b"new\n");
        f.commit("add b");
        f.write("a.txt", b"1\n2\n3\n4\nfive\n");
        f.commit("edit a");
        f.git(&["checkout", "-q", "main"]);
        f.write("c.txt", b"c\n");
        f.commit("add c on main");
        let bare = f._dir.path().join("hosted.git");
        f.git(&["clone", "-q", "--bare", f.path.to_str().unwrap(), bare.to_str().unwrap()]);
        (f, bare)
    }

    fn open(path: &Path) -> gix::Repository {
        local(&discover(path).unwrap())
    }

    #[test]
    fn compare_from_the_merge_base() {
        let (_f, bare) = setup();
        let repo = open(&bare);
        let files = forge::compare(&repo, "main", "feature").unwrap();
        let names: Vec<_> = files.iter().map(|f| (f.path.as_str(), f.additions, f.deletions)).collect();
        assert_eq!(names, vec![("a.txt", 1, 1), ("b.txt", 1, 0)], "c.txt landed on main and isn't the pull request's");
        let d = forge::compare_diff(&repo, "main", "feature", "a.txt", 3, &DiffCache::default()).unwrap();
        assert_eq!((d.additions, d.deletions), (1, 1));
        let commits = forge::commits_between(&repo, "main", "feature").unwrap();
        assert_eq!(commits.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), vec!["add b", "edit a"]);
    }

    #[test]
    fn mergeability_and_strategies() {
        for strategy in [MergeStrategy::Merge, MergeStrategy::Squash, MergeStrategy::Rebase] {
            let (f, bare) = setup();
            let repo = open(&bare);
            let m = forge::mergeability(&repo, "main", "feature").unwrap();
            assert!(m.clean && !m.fast_forward && m.behind == 1, "{m:?}");
            assert!(forge::merge(&repo, "main", "feature", MergeStrategy::FastForward, "", &server()).is_err(), "main moved on");

            let new = forge::merge(&repo, "main", "feature", strategy, "Merge feature", &server()).unwrap();
            let tip = f.git_in(&bare, &["rev-parse", "main"]);
            assert_eq!(tip, new.to_string());
            let files = f.git_in(&bare, &["ls-tree", "--name-only", "main"]);
            assert_eq!(files, "a.txt\nb.txt\nc.txt", "{strategy:?}");
            assert_eq!(f.git_in(&bare, &["show", "main:a.txt"]), "1\n2\n3\n4\nfive");
            let parents = f.git_in(&bare, &["rev-list", "--parents", "-n1", "main"]).split(' ').count() - 1;
            let count = f.git_in(&bare, &["rev-list", "--count", "main"]);
            match strategy {
                MergeStrategy::Merge => assert_eq!((parents, count.as_str()), (2, "5")),
                MergeStrategy::Squash => assert_eq!((parents, count.as_str()), (1, "3")),
                _ => {
                    assert_eq!((parents, count.as_str()), (1, "4"));
                    assert_eq!(f.git_in(&bare, &["log", "-1", "--format=%s|%an|%cn", "main"]), "edit a|Ada|Neutrino", "a rebase keeps authors");
                }
            }
            if strategy != MergeStrategy::Rebase {
                assert_eq!(f.git_in(&bare, &["log", "-1", "--format=%s|%an|%cn", "main"]), "Merge feature|Ada|Neutrino");
            }
            assert!(forge::merge(&repo, "main", "feature", strategy, "again", &server()).is_err(), "nothing left to merge");
        }
    }

    #[test]
    fn fast_forward_and_conflicts() {
        let f = Fixture::new();
        f.write("a.txt", b"one\n");
        f.commit("base");
        f.git(&["checkout", "-q", "-b", "ff"]);
        f.write("a.txt", b"two\n");
        let two = f.commit("two");
        f.git(&["checkout", "-q", "-b", "clash", "main"]);
        f.write("a.txt", b"three\n");
        f.commit("three");
        f.git(&["checkout", "-q", "main"]);
        let repo = f.repo();

        let m = forge::mergeability(&repo, "main", "ff").unwrap();
        assert!(m.clean && m.fast_forward && m.behind == 0);
        assert_eq!(forge::merge(&repo, "main", "ff", MergeStrategy::FastForward, "", &server()).unwrap().to_string(), two);

        let m = forge::mergeability(&repo, "main", "clash").unwrap();
        assert_eq!((m.clean, m.conflicts.clone()), (false, vec!["a.txt".to_string()]));
        let err = forge::merge(&repo, "main", "clash", MergeStrategy::Merge, "m", &server()).unwrap_err();
        assert!(err.0.contains("conflicts with main in a.txt"), "{err}");
        assert!(forge::merge(&repo, "main", "clash", MergeStrategy::Rebase, "m", &server()).is_err());
        assert_eq!(f.git(&["rev-parse", "main"]), two, "a failed merge moves nothing");
    }

    #[test]
    fn lines_follow_edits() {
        let old = b"a\nb\nc\nd\ne\n";
        // Insert two lines after b, change d.
        let new = b"a\nb\nx\ny\nc\nD\ne\n";
        let map = |l| forge::map_through(old, new, l);
        assert_eq!(map(1), Some(1));
        assert_eq!(map(2), Some(2));
        assert_eq!(map(3), Some(5));
        assert_eq!(map(4), None, "the line changed");
        assert_eq!(map(5), Some(7));
        assert_eq!(forge::map_through(b"a\nb\n", b"b\n", 2), Some(1));
        assert_eq!(forge::map_through(b"a\nb\n", b"b\n", 1), None);
        assert_eq!(forge::map_through(b"a\n", b"z\na\n", 1), Some(2), "an insertion at the top");
    }

    #[test]
    fn pull_heads_are_kept() {
        let (f, bare) = setup();
        let repo = open(&bare);
        let tip = f.git_in(&bare, &["rev-parse", "feature"]);
        forge::keep_pull_head(&repo, 7, &tip).unwrap();
        assert_eq!(f.git_in(&bare, &["rev-parse", "refs/pull/7/head"]), tip);
        assert_eq!(forge::map_line(&repo, "main", "feature", "a.txt", 2).unwrap(), Some(2));
        assert_eq!(forge::map_line(&repo, "main", "feature", "a.txt", 5).unwrap(), None);
        assert_eq!(forge::map_line(&repo, "main", "feature", "missing.txt", 1).unwrap(), None);
    }
}

mod browse {
    use super::*;
    use crate::browse::{self, MAX_BLOB_BYTES};
    use neutrino_proto::{EntryKind, LanguageShare};

    fn bare(f: &Fixture) -> gix::Repository {
        let bare = f._dir.path().join("hosted.git");
        f.git(&["clone", "-q", "--bare", f.path.to_str().unwrap(), bare.to_str().unwrap()]);
        local(&discover(&bare).unwrap())
    }

    fn kinds(listing: &neutrino_proto::TreeListing) -> Vec<(&str, EntryKind)> {
        listing.entries.iter().map(|e| (e.name.as_str(), e.kind)).collect()
    }

    #[cfg(unix)]
    #[test]
    fn nested_tree_with_kinds_and_sorting() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        f.write("src/main.rs", b"fn main() {}\n");
        f.write("src/lib/mod.rs", b"\n");
        f.write("run.sh", b"#!/bin/sh\n");
        std::fs::set_permissions(f.path.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("src/main.rs", f.path.join("link")).unwrap();
        f.write("b.txt", b"b");
        f.write("A.txt", b"aa");
        f.write("Docs/x.md", b"x");
        f.commit("first");
        let sha = f.git(&["rev-parse", "HEAD"]);
        let repo = bare(&f);

        let root = browse::tree(&repo, None, "").unwrap();
        assert_eq!(root.commit, sha);
        assert_eq!(root.path, "");
        assert_eq!(
            kinds(&root),
            [
                ("Docs", EntryKind::Dir),
                ("src", EntryKind::Dir),
                ("A.txt", EntryKind::File),
                ("b.txt", EntryKind::File),
                ("link", EntryKind::Symlink),
                ("run.sh", EntryKind::Executable),
            ]
        );
        assert_eq!(root.entries[0].size, None);
        assert_eq!(root.entries[2].size, Some(2));
        assert_eq!(root.entries[4].size, Some("src/main.rs".len() as u64));

        let src = browse::tree(&repo, Some("main"), "/src/").unwrap();
        assert_eq!(src.path, "src");
        assert_eq!(kinds(&src), [("lib", EntryKind::Dir), ("main.rs", EntryKind::File)]);
        assert_eq!(src.entries[1].path, "src/main.rs");
        assert_eq!(browse::tree(&repo, Some(&sha[..8]), "src/lib").unwrap().entries[0].path, "src/lib/mod.rs");

        assert!(browse::tree(&repo, None, "nope").is_err());
        assert!(browse::tree(&repo, None, "src/main.rs").is_err());
        assert!(browse::tree(&repo, Some("no-such-branch"), "").is_err());

        let link = browse::blob(&repo, None, "link").unwrap();
        assert_eq!((link.text.as_deref(), link.lines, link.binary), (Some("src/main.rs"), 1, false));
    }

    #[test]
    fn submodules_are_listed() {
        let f = Fixture::new();
        f.write("a.txt", b"a\n");
        let a = f.commit("first");
        f.git(&["update-index", "--add", "--cacheinfo", &format!("160000,{a},sub")]);
        f.git(&["commit", "-q", "-m", "add sub"]);
        let repo = f.repo();
        let root = browse::tree(&repo, None, "").unwrap();
        assert_eq!(kinds(&root), [("sub", EntryKind::Submodule), ("a.txt", EntryKind::File)]);
        assert_eq!(root.entries[0].size, None);
        assert!(browse::blob(&repo, None, "sub").is_err());
    }

    #[test]
    fn last_commit_per_entry() {
        let f = Fixture::new();
        f.write("src/a.rs", b"a\n");
        f.write("src/b.rs", b"b\n");
        f.write("top.txt", b"t\n");
        let first = f.commit("first");
        f.write("src/b.rs", b"b2\n");
        let second = f.commit("change b\n\nbody");
        f.write("top.txt", b"t2\n");
        let third = f.commit("change top");
        f.write("src/c.rs", b"c\n");
        let fourth = f.commit("add c");
        f.git(&["rm", "-q", "top.txt"]);
        f.commit("drop top");
        f.write("top.txt", b"t3\n");
        let sixth = f.commit("bring top back");
        let repo = bare(&f);

        let root = browse::tree_commits(&repo, None, "").unwrap();
        let got: Vec<_> = root.iter().map(|e| (e.name.as_str(), e.sha.as_str(), e.subject.as_str())).collect();
        assert_eq!(got, [("src", fourth.as_str(), "add c"), ("top.txt", sixth.as_str(), "bring top back")]);
        assert!(root[0].time > 1_700_000_000);

        let src = browse::tree_commits(&repo, None, "src").unwrap();
        let got: Vec<_> = src.iter().map(|e| (e.name.as_str(), e.sha.as_str())).collect();
        assert_eq!(got, [("a.rs", first.as_str()), ("b.rs", second.as_str()), ("c.rs", fourth.as_str())]);
        assert_eq!(src[1].subject, "change b");

        let old = browse::tree_commits(&repo, Some(&third), "").unwrap();
        let got: Vec<_> = old.iter().map(|e| (e.name.as_str(), e.sha.as_str())).collect();
        assert_eq!(got, [("src", second.as_str()), ("top.txt", third.as_str())]);
        assert!(browse::tree_commits(&repo, None, "src/a.rs").is_err());
    }

    #[test]
    fn blobs() {
        let f = Fixture::new();
        f.write("two.txt", b"one\ntwo\n");
        f.write("open.txt", b"one\ntwo");
        f.write("empty.txt", b"");
        f.write("bin.dat", b"a\0b");
        f.write("big.txt", &vec![b'x'; MAX_BLOB_BYTES as usize + 1]);
        f.write("bad.txt", b"caf\xe9\n");
        f.write("dir/x.txt", b"x\n");
        f.commit("files");
        let repo = bare(&f);

        let b = browse::blob(&repo, None, "two.txt").unwrap();
        assert_eq!((b.text.as_deref(), b.lines, b.size, b.binary, b.too_large), (Some("one\ntwo\n"), 2, 8, false, false));
        assert_eq!(browse::blob(&repo, None, "open.txt").unwrap().lines, 2);
        let e = browse::blob(&repo, None, "empty.txt").unwrap();
        assert_eq!((e.text.as_deref(), e.lines), (Some(""), 0));
        let bin = browse::blob(&repo, None, "bin.dat").unwrap();
        assert_eq!((bin.binary, bin.text, bin.lines, bin.size), (true, None, 0, 3));
        let big = browse::blob(&repo, None, "big.txt").unwrap();
        assert_eq!((big.too_large, big.text, big.size), (true, None, MAX_BLOB_BYTES + 1));
        assert_eq!(browse::blob(&repo, None, "bad.txt").unwrap().text.as_deref(), Some("caf\u{fffd}\n"));
        assert_eq!(browse::blob(&repo, None, "dir/x.txt").unwrap().path, "dir/x.txt");
        assert!(browse::blob(&repo, None, "dir").is_err());
        assert!(browse::blob(&repo, None, "missing").is_err());
        assert!(browse::blob(&repo, None, "").is_err());
    }

    #[test]
    fn overview() {
        let f = Fixture::new();
        f.write("src/main.rs", &[b'r'; 300]);
        f.write("app/View.swift", &[b's'; 200]);
        f.write("app/script.py", &[b'p'; 100]);
        f.write("README.txt", b"plain");
        f.write("readme.md", b"# hi");
        f.write("LICENSE-MIT", b"mit");
        f.write("Cargo.toml", &[b't'; 5000]);
        f.write("notes.md", &[b'm'; 5000]);
        f.write("vendor/lib.rs", &[b'v'; 5000]);
        f.write("web/app.min.js", &[b'j'; 5000]);
        f.write("web/app.js", &[b'j'; 10]);
        f.write("Dockerfile", &[b'd'; 20]);
        f.commit("first");
        f.git(&["commit", "-q", "--allow-empty", "--author", "Bob <bob@example.com>", "-m", "bob"]);
        f.git(&["commit", "-q", "--allow-empty", "--author", "Ada <ADA@example.com>", "-m", "ada again"]);
        let head = f.git(&["rev-parse", "HEAD"]);
        let repo = bare(&f);

        let o = browse::overview(&repo, None).unwrap();
        assert_eq!(o.commit, head);
        assert_eq!((o.commits, o.commits_capped, o.contributors), (3, false, 2));
        assert_eq!(o.files, 12);
        let share = |name: &str, bytes: u64| LanguageShare { name: name.into(), bytes };
        assert_eq!(o.languages, [share("Rust", 300), share("Swift", 200), share("Python", 100), share("Dockerfile", 20), share("JavaScript", 10)]);
        assert_eq!(o.readme.as_deref(), Some("readme.md"));
        assert_eq!(o.license.as_deref(), Some("LICENSE-MIT"));

        let first = browse::overview(&repo, Some("HEAD~2")).unwrap();
        assert_eq!((first.commits, first.contributors), (1, 1));
    }

    #[test]
    fn overview_folds_small_languages_into_other() {
        let f = Fixture::new();
        for (i, ext) in ["rs", "swift", "py", "go", "c", "java", "rb", "php", "lua", "zig"].iter().enumerate() {
            f.write(&format!("f.{ext}"), &vec![b'x'; 100 - i]);
        }
        f.commit("many");
        let o = browse::overview(&f.repo(), None).unwrap();
        assert_eq!(o.languages.len(), 8);
        assert_eq!(o.languages[0].name, "Rust");
        assert_eq!(o.languages[7].name, "Other");
        assert_eq!(o.languages[7].bytes, 93 + 92 + 91);
        assert_eq!((o.readme, o.license), (None, None));
    }
}

#[test]
fn pages_match_one_long_page_and_see_new_commits() {
    let f = Fixture::new();
    for i in 0..12 {
        f.commit(&format!("main {i}"));
        if i % 4 == 1 {
            // Side branches merged back, so the graph has lanes to keep.
            f.git(&["checkout", "-q", "-b", &format!("side{i}")]);
            f.commit(&format!("side {i} a"));
            f.commit(&format!("side {i} b"));
            f.git(&["checkout", "-q", "main"]);
            f.commit(&format!("main {i} meanwhile"));
            f.git(&["merge", "-q", "--no-ff", &format!("side{i}"), "-m", &format!("merge {i}")]);
        }
    }
    let repo = f.repo();
    let whole = log(&repo, &LogQuery { limit: Some(1000), ..q(true) }).unwrap();
    assert!(!whole.has_more);

    let mut paged = Vec::new();
    let mut lanes = 0;
    loop {
        let page = log(&repo, &LogQuery { skip: paged.len() as u32, limit: Some(5), ..q(true) }).unwrap();
        lanes = lanes.max(page.lanes);
        paged.extend(page.commits);
        if !page.has_more {
            break;
        }
    }
    assert_eq!(paged.len(), whole.commits.len());
    for (a, b) in paged.iter().zip(&whole.commits) {
        assert_eq!((&a.sha, &a.graph, &a.refs), (&b.sha, &b.graph, &b.refs), "page rows match the long page");
    }
    assert_eq!(lanes, whole.lanes);

    // A new commit moves the tip, so page one walks again and shows it.
    f.commit("newest");
    let first = log(&repo, &LogQuery { limit: Some(5), ..q(true) }).unwrap();
    assert_eq!(first.commits[0].subject, "newest");
}

/// Timing, not a check: `NEUTRINO_BENCH_REPO=/path cargo test -p neutrino-git
/// --release paging_timings -- --ignored --nocapture` prints how long pages
/// take at increasing depths, cold and then with the kept walk.
#[test]
#[ignore]
fn paging_timings() {
    let Ok(path) = std::env::var("NEUTRINO_BENCH_REPO") else { return };
    let repo = gix::open(&path).unwrap();
    // Scrolling: every page in turn, as the History screen asks for them.
    let (mut total, mut worst) = (0.0f64, 0.0f64);
    for skip in (0u32..=20_000).step_by(100) {
        let t = std::time::Instant::now();
        let page = log(&repo, &LogQuery { skip, limit: Some(100), ..q(false) }).unwrap();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        (total, worst) = (total + ms, worst.max(ms));
        if [0, 1_000, 10_000, 20_000].contains(&skip) {
            println!("page at skip {skip:>6}: {ms:>7.1} ms ({} commits)", page.commits.len());
        }
    }
    println!("201 pages in turn: {total:.0} ms in all, slowest {worst:.1} ms");
}
