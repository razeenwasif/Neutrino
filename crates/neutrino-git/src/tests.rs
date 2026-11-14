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
