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
    let options = neutrinod::Options { role: neutrinod::Role::Server, remote_port: None, advertise: false, ci: local_ci(), vault: neutrino_vault_test_key(), copy_backups_from: None };
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
    let create = || Request::CreatePull { repo: id.clone(), title: "Shout".into(), body: "Louder.".into(), source: "feature".into(), target: "main".into() };
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
    let d = detail(&sock, Request::CreatePull { repo: id.clone(), title: "Tweak".into(), body: String::new(), source: "tweak".into(), target: "main".into() }).await;
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

/// Tests never touch the real Graviton: steps run as local processes.
fn local_ci() -> neutrinod::ci::CiOptions {
    neutrinod::ci::CiOptions { runner: neutrinod::ci::Runner::Local(Default::default()), ..Default::default() }
}
