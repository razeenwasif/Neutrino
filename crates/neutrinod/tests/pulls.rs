//! Pull requests end to end on a server daemon: versions from pushes, notes
//! that follow their lines, merging, and branch rules enforced in the hooks.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use neutrino_proto::{BranchRule, MergeStrategy, PullDetail, PullState, Request, Response, Side, decode_response, encode_request};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

fn git_out(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_out(dir, args);
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A push, off the async runtime: its hooks call back into the daemon.
async fn push(dir: &Path, args: &[&str]) -> (bool, String) {
    let dir = dir.to_path_buf();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = git_out(&dir, &args);
        (out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned())
    })
    .await
    .unwrap()
}

async fn call(socket: &Path, request: Request) -> Response {
    let stream = UnixStream::connect(socket).await.unwrap();
    let (read, mut write) = stream.into_split();
    write.write_all(format!("{}\n", encode_request(1, &request)).as_bytes()).await.unwrap();
    let mut lines = BufReader::new(read).lines();
    let line = tokio::time::timeout(Duration::from_secs(30), lines.next_line()).await.unwrap().unwrap().unwrap();
    decode_response(&line).unwrap().1
}

async fn detail(socket: &Path, request: Request) -> PullDetail {
    match call(socket, request).await {
        Response::Pull(d) => *d,
        other => panic!("{other:?}"),
    }
}

async fn start(state: PathBuf, socket: PathBuf) -> (Arc<neutrinod::Daemon>, tokio::sync::oneshot::Sender<()>) {
    let options = neutrinod::Options { role: neutrinod::Role::Server, ci: local_ci(), vault: neutrino_vault_test_key(), ..Default::default() };
    let daemon = neutrinod::Daemon::with(&state, options).unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let d = Arc::clone(&daemon);
    let s = socket.clone();
    tokio::spawn(async move { neutrinod::serve(d, &s, async { stopped.await.ok(); }).await.unwrap() });
    while !socket.exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (daemon, stop)
}

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}

#[tokio::test]
async fn review_and_merge() {
    // SAFETY: set before any hook runs, and only read by the daemon this test starts.
    unsafe { std::env::set_var("NEUTRINO_HOOK_EXE", env!("CARGO_BIN_EXE_neutrinod")) };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let sock = root.join("s.sock");
    let (_server, stop) = start(root.join("server"), sock.clone()).await;

    let hosted = match call(&sock, Request::CreateHosted { name: "app".into(), description: None }).await {
        Response::Repo(r) => r,
        other => panic!("{other:?}"),
    };
    let id = hosted.id.clone();
    let url = hosted.path.clone();

    let wc = root.join("wc");
    std::fs::create_dir(&wc).unwrap();
    git(&wc, &["init", "-q", "-b", "main"]);
    write(&wc, "app.txt", "one\ntwo\nthree\nfour\n");
    git(&wc, &["add", "."]);
    git(&wc, &["commit", "-q", "-m", "first"]);
    git(&wc, &["remote", "add", "origin", &url]);
    assert!(push(&wc, &["push", "-q", "origin", "main"]).await.0);

    git(&wc, &["checkout", "-q", "-b", "feature"]);
    write(&wc, "app.txt", "one\ntwo\nthree\nFOUR\n");
    git(&wc, &["commit", "-qam", "shout four"]);
    assert!(push(&wc, &["push", "-q", "origin", "feature"]).await.0);

    // Open it; the same branches can't have two open at once.
    let create = || Request::CreatePull { repo: id.clone(), title: "Shout".into(), body: "Louder.".into(), source: "feature".into(), target: "main".into(), stack_id: None, parent_pull: None };
    let d = detail(&sock, create()).await;
    assert_eq!((d.pull.number, d.pull.versions, d.files.len(), d.commits.len()), (1, 1, 1, 1));
    assert!(matches!(call(&sock, create()).await, Response::Error { .. }));
    let m = d.mergeability.unwrap();
    assert!(m.clean && m.fast_forward);
    assert_eq!(git(Path::new(&url), &["rev-parse", "refs/pull/1/head"]), git(&wc, &["rev-parse", "feature"]));

    // A note on line 3, then a push that adds a line above it: the note follows.
    let d = detail(&sock, Request::AddNote { repo: id.clone(), number: 1, path: "app.txt".into(), side: Side::New, line: 3, text: "Keep three".into() }).await;
    assert_eq!(d.pull.open_notes, 1);
    write(&wc, "app.txt", "zero\none\ntwo\nthree\nFOUR\n");
    git(&wc, &["commit", "-qam", "start at zero"]);
    let (ok, said) = push(&wc, &["push", "origin", "feature"]).await;
    assert!(ok && said.contains("version 2"), "{said}");
    let d = detail(&sock, Request::Pull { repo: id.clone(), number: 1 }).await;
    assert_eq!(d.pull.versions, 2);
    assert_eq!((d.notes[0].line, d.notes[0].current_line), (3, Some(4)));
    let d = detail(&sock, Request::ResolveNote { repo: id.clone(), number: 1, note: d.notes[0].id, resolved: true }).await;
    assert_eq!(d.pull.open_notes, 0);

    // Guard main: direct pushes are refused unless overridden, and then recorded.
    let rule = BranchRule { pattern: "main".into(), require_pull: true, block_force: true, block_delete: true, required_checks: vec![] };
    assert!(matches!(call(&sock, Request::SetBranchRule { repo: id.clone(), rule }).await, Response::Rules(r) if r.len() == 1));
    assert!(Path::new(&url).join("neutrino-rules").exists());
    git(&wc, &["checkout", "-q", "main"]);
    write(&wc, "other.txt", "x\n");
    git(&wc, &["add", "."]);
    git(&wc, &["commit", "-qm", "straight to main"]);
    let (ok, said) = push(&wc, &["push", "origin", "main"]).await;
    assert!(!ok && said.contains("takes changes through pull requests"), "{said}");
    let (ok, said) = push(&wc, &["push", "-o", "neutrino-override=hotfix", "origin", "main"]).await;
    assert!(ok, "{said}");
    assert!(matches!(call(&sock, Request::Overrides { repo: id.clone() }).await, Response::Overrides(o) if o.len() == 1 && o[0].reason == "hotfix"));
    // Deleting main is refused too.
    let (ok, _) = push(&wc, &["push", "origin", ":main"]).await;
    assert!(!ok);

    // main moved on, so it's no longer a fast-forward; merge with a merge commit.
    let d = detail(&sock, Request::Pull { repo: id.clone(), number: 1 }).await;
    let m = d.mergeability.unwrap();
    assert!(m.clean && !m.fast_forward && m.behind == 1);
    assert_eq!(d.rule.unwrap().pattern, "main");
    let merged = match call(&sock, Request::MergePull { repo: id.clone(), number: 1, strategy: MergeStrategy::Merge, message: None, delete_source: true }).await {
        Response::Merged { sha } => sha,
        other => panic!("{other:?}"),
    };
    let bare = Path::new(&url);
    assert_eq!(git(bare, &["rev-parse", "main"]), merged);
    assert_eq!(git(bare, &["log", "-1", "--format=%s", "main"]), "Merge pull request #1 from feature");
    assert!(git_out(bare, &["rev-parse", "--verify", "refs/heads/feature"]).status.code() != Some(0), "the source was deleted");
    let d = detail(&sock, Request::Pull { repo: id.clone(), number: 1 }).await;
    assert_eq!((d.pull.state, d.pull.merge_sha.as_deref()), (PullState::Merged, Some(merged.as_str())));
    assert!(d.mergeability.is_none());
    assert_eq!(d.files.len(), 1, "a merged pull request still shows what it changed");

    // A pull request merged by a push is noticed.
    git(&wc, &["fetch", "-q", "origin"]);
    git(&wc, &["checkout", "-q", "-B", "main", "origin/main"]);
    git(&wc, &["checkout", "-q", "-b", "tweak"]);
    write(&wc, "other.txt", "y\n");
    git(&wc, &["commit", "-qam", "tweak"]);
    assert!(push(&wc, &["push", "-q", "origin", "tweak"]).await.0);
    let d = detail(&sock, Request::CreatePull { repo: id.clone(), title: "Tweak".into(), body: String::new(), source: "tweak".into(), target: "main".into(), stack_id: None, parent_pull: None }).await;
    assert_eq!(d.pull.number, 2);
    git(&wc, &["checkout", "-q", "main"]);
    git(&wc, &["merge", "-q", "--ff-only", "tweak"]);
    let (ok, said) = push(&wc, &["push", "-o", "neutrino-override=merged locally", "origin", "main"]).await;
    assert!(ok && said.contains("#2 Tweak is merged"), "{said}");

    // Without the daemon, a repo with rules refuses pushes.
    stop.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    write(&wc, "other.txt", "z\n");
    git(&wc, &["commit", "-qam", "while it's down"]);
    let (ok, said) = push(&wc, &["push", "-o", "neutrino-override=x", "origin", "main"]).await;
    assert!(!ok && said.contains("branch rules"), "{said}");
}

/// Tests never touch the login Keychain.
fn neutrino_vault_test_key() -> neutrino_vault::KeyStore {
    neutrino_vault::KeyStore::File(std::env::temp_dir().join(format!("ntr-test-vault-{}-{:?}.key", std::process::id(), std::thread::current().id())))
}

#[tokio::test]
async fn wave2_interdiff_stacked_prs_and_semantic_diff() {
    // SAFETY: set before any hook runs, and only read by the daemon this test starts.
    unsafe { std::env::set_var("NEUTRINO_HOOK_EXE", env!("CARGO_BIN_EXE_neutrinod")) };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let sock = root.join("s.sock");
    let (_server, stop) = start(root.join("server"), sock.clone()).await;

    let hosted = match call(&sock, Request::CreateHosted { name: "stackapp".into(), description: None }).await {
        Response::Repo(r) => r,
        other => panic!("{other:?}"),
    };
    let id = hosted.id;
    let url = hosted.path;
    let wc = root.join("wc");
    std::fs::create_dir(&wc).unwrap();
    git(&wc, &["init", "-q", "-b", "main"]);
    write(&wc, "proto.rs", "pub struct Token {\n    pub val: i32,\n}\n");
    write(&wc, "app.rs", "pub fn use_token(t: Token) -> i32 {\n    t.val\n}\n");
    git(&wc, &["add", "."]);
    git(&wc, &["commit", "-q", "-m", "init"]);
    git(&wc, &["remote", "add", "origin", &url]);
    assert!(push(&wc, &["push", "-q", "origin", "main"]).await.0);

    // PR #1 (s1 -> main): edits proto.rs and app.rs, then force-pushes v2 for interdiff.
    git(&wc, &["checkout", "-q", "-b", "s1"]);
    write(&wc, "proto.rs", "pub struct Token {\n    pub val: i32,\n    pub tag: &'static str,\n}\n");
    write(&wc, "app.rs", "pub fn use_token(t: Token) -> i32 {\n    t.val + 1\n}\n");
    git(&wc, &["commit", "-qam", "s1 v1"]);
    assert!(push(&wc, &["push", "-q", "origin", "s1"]).await.0);
    let d1 = detail(
        &sock,
        Request::CreatePull {
            repo: id.clone(),
            title: "Stack 1".into(),
            body: String::new(),
            source: "s1".into(),
            target: "main".into(),
            stack_id: Some("feat-chain".into()),
            parent_pull: None,
        },
    )
    .await;
    assert_eq!(d1.pull.number, 1);
    assert!(d1.tour.iter().position(|t| t.path == "proto.rs") < d1.tour.iter().position(|t| t.path == "app.rs"));

    // Verify compare_diff returns syntax spans.
    let diff_resp = call(
        &sock,
        Request::CompareDiff {
            repo: id.clone(),
            base: "main".into(),
            head: d1.pull.head_sha.clone(),
            path: "proto.rs".into(),
            context: None,
        },
    )
    .await;
    let Response::FileDiff(fd) = diff_resp else { panic!("{diff_resp:?}") };
    assert!(fd.hunks.iter().flat_map(|h| &h.lines).any(|l| !l.syntax.is_empty()), "expected tree-sitter spans in FileDiff");

    // Note on v1 + force-push v2 -> test CompareVersions interdiff.
    let _ = detail(&sock, Request::AddNote { repo: id.clone(), number: 1, path: "app.rs".into(), side: Side::New, line: 2, text: "Check +1".into() }).await;
    write(&wc, "app.rs", "pub fn use_token(t: Token) -> i32 {\n    t.val + 2\n}\n");
    git(&wc, &["commit", "-qa", "--amend", "--no-edit"]);
    let (ok, said) = push(&wc, &["push", "--force", "origin", "s1"]).await;
    assert!(ok && said.contains("version 2"), "{said}");

    let inter = match call(&sock, Request::CompareVersions { repo: id.clone(), number: 1, from_version: 1, to_version: 2 }).await {
        Response::VersionDiff(v) => v,
        other => panic!("{other:?}"),
    };
    assert_eq!((inter.from_version, inter.to_version, inter.files.len()), (1, 2, 1));
    assert_eq!(inter.files[0].path, "app.rs");
    let vdiff = call(
        &sock,
        Request::CompareVersionDiff { repo: id.clone(), number: 1, from_version: 1, to_version: 2, path: "app.rs".into(), context: None },
    )
    .await;
    assert!(matches!(vdiff, Response::FileDiff(f) if f.path == "app.rs" && f.additions == 1 && f.deletions == 1));

    // Create 3-deep stack: #2 (s2 -> s1) and #3 (s3 -> s2).
    git(&wc, &["checkout", "-q", "-b", "s2"]);
    write(&wc, "s2.rs", "pub fn layer2() -> i32 { 2 }\n");
    git(&wc, &["add", "."]);
    git(&wc, &["commit", "-q", "-m", "s2"]);
    assert!(push(&wc, &["push", "-q", "origin", "s2"]).await.0);
    let d2 = detail(
        &sock,
        Request::CreatePull {
            repo: id.clone(),
            title: "Stack 2".into(),
            body: String::new(),
            source: "s2".into(),
            target: "s1".into(),
            stack_id: None,
            parent_pull: Some(1),
        },
    )
    .await;
    assert_eq!(d2.stack.len(), 2);

    git(&wc, &["checkout", "-q", "-b", "s3"]);
    write(&wc, "s3.rs", "pub fn layer3() -> i32 { 3 }\n");
    git(&wc, &["add", "."]);
    git(&wc, &["commit", "-q", "-m", "s3"]);
    assert!(push(&wc, &["push", "-q", "origin", "s3"]).await.0);
    let d3 = detail(
        &sock,
        Request::CreatePull {
            repo: id.clone(),
            title: "Stack 3".into(),
            body: String::new(),
            source: "s3".into(),
            target: "s2".into(),
            stack_id: None,
            parent_pull: Some(2),
        },
    )
    .await;
    assert_eq!(d3.stack.len(), 3);

    // Also create a conflicting stack PR #4 (s4 -> s1 modifying proto.rs) to verify conflict recording.
    git(&wc, &["checkout", "-q", "-b", "s4", "s1"]);
    write(&wc, "proto.rs", "pub struct Token {\n    pub val: u64,\n}\n");
    git(&wc, &["commit", "-qam", "s4 conflicting proto change"]);
    assert!(push(&wc, &["push", "-q", "origin", "s4"]).await.0);
    let d4 = detail(
        &sock,
        Request::CreatePull {
            repo: id.clone(),
            title: "Stack 4 Conflict".into(),
            body: String::new(),
            source: "s4".into(),
            target: "s1".into(),
            stack_id: Some("conflict-chain".into()),
            parent_pull: Some(1),
        },
    )
    .await;
    let s4_before = d4.pull.head_sha.clone();

    // Advance main with another conflicting edit to proto.rs so merging #1 (actually #1 cleanly merges into main, let's first modify main on proto.rs? Wait: if main conflicts #1, #1 can't merge. Instead, #1 has val+2 on app.rs cleanly, and let's have main change s4's file ONLY before merging #1!).
    git(&wc, &["checkout", "-q", "main"]);
    write(&wc, "proto.rs", "pub struct ConflictOnlyOnMain {}\n");
    git(&wc, &["commit", "-qam", "conflict setup"]);
    // Wait: if main edits proto.rs AND #1 edits proto.rs, #1 will conflict with main! Instead edit a DIFFERENT file on main OR make #4 edit another_file.txt modified on main!
    // Reset cleanly: put the conflict on a new commit on main modifying `shared.txt`, and put `shared.txt` in #4.
    git(&wc, &["reset", "--hard", "HEAD~1"]);
    write(&wc, "shared.txt", "from-main\n");
    git(&wc, &["add", "shared.txt"]);
    git(&wc, &["commit", "-q", "-m", "main shared"]);
    assert!(push(&wc, &["push", "-q", "origin", "main"]).await.0);

    git(&wc, &["checkout", "-q", "s4"]);
    write(&wc, "shared.txt", "from-s4\n");
    git(&wc, &["add", "shared.txt"]);
    git(&wc, &["commit", "-q", "-m", "s4 shared conflict"]);
    assert!(push(&wc, &["push", "-q", "origin", "s4"]).await.0);
    let d4 = detail(&sock, Request::Pull { repo: id.clone(), number: 4 }).await;
    let s4_sha_pre = d4.pull.head_sha.clone();

    // Merge #1 -> auto-retargets #2 to main & restacks #2 and #3 cleanly, while #4 records conflict cleanly without touching refs/heads/s4!
    let merged = match call(
        &sock,
        Request::MergePull { repo: id.clone(), number: 1, strategy: MergeStrategy::Squash, message: None, delete_source: true },
    )
    .await
    {
        Response::Merged { sha } => sha,
        other => panic!("{other:?}"),
    };
    let d2_after = detail(&sock, Request::Pull { repo: id.clone(), number: 2 }).await;
    assert_eq!(d2_after.pull.target, "main");
    assert_eq!(d2_after.pull.parent_pull, None);
    assert!(d2_after.pull.versions >= 2, "expected auto-restack to create v2 on #2");
    assert!(d2_after.pull.stack_conflict.is_none());
    assert!(d2_after.mergeability.unwrap().clean);
    let bare = Path::new(&url);
    assert_eq!(git(bare, &["merge-base", "main", "s2"]), merged);

    let d3_after = detail(&sock, Request::Pull { repo: id.clone(), number: 3 }).await;
    assert_eq!(d3_after.pull.target, "s2");
    assert!(d3_after.pull.versions >= 2, "expected cascading restack on #3");
    assert!(d3_after.pull.stack_conflict.is_none());

    let d4_after = detail(&sock, Request::Pull { repo: id.clone(), number: 4 }).await;
    assert_eq!(d4_after.pull.target, "main");
    assert!(d4_after.pull.stack_conflict.as_deref().is_some_and(|c| c.contains("shared.txt")), "{:?}", d4_after.pull.stack_conflict);
    assert_eq!(git(bare, &["rev-parse", "s4"]), s4_sha_pre, "conflicting branch ref must stay untouched");

    let _ = s4_before;
    stop.send(()).unwrap();
}

/// Tests never touch the real Graviton: steps run as local processes.
fn local_ci() -> neutrinod::ci::CiOptions {
    neutrinod::ci::CiOptions { runner: neutrinod::ci::Runner::Local(Default::default()), ..Default::default() }
}
