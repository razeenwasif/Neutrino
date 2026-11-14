//! CI end to end on a server daemon, with steps run as local processes:
//! runs from pushes and pull requests, needs and matrices, conditions,
//! logs, cancelling, timeouts, a broken workflow file and the budget.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use neutrino_proto::{Request, Response, RunDetail, RunState, decode_response, encode_request};
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

async fn runs(socket: &Path, repo: &str) -> Vec<neutrino_proto::Run> {
    match call(socket, Request::ListRuns { repo: repo.into(), pull: None, limit: None }).await {
        Response::Runs(r) => r,
        other => panic!("{other:?}"),
    }
}

async fn detail(socket: &Path, repo: &str, id: i64) -> RunDetail {
    match call(socket, Request::Run { repo: repo.into(), id }).await {
        Response::Run(d) => *d,
        other => panic!("{other:?}"),
    }
}

async fn finished(socket: &Path, repo: &str, id: i64) -> RunDetail {
    for _ in 0..300 {
        let d = detail(socket, repo, id).await;
        if d.run.state.finished() {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("run {id} didn't finish: {:#?}", detail(socket, repo, id).await);
}

async fn log(socket: &Path, repo: &str, job: i64) -> String {
    match call(socket, Request::JobLog { repo: repo.into(), job, offset: 0 }).await {
        Response::JobLog(l) => l.text,
        other => panic!("{other:?}"),
    }
}

fn job<'a>(d: &'a RunDetail, key: &str) -> &'a neutrino_proto::RunJob {
    d.jobs.iter().find(|j| j.key == key).unwrap_or_else(|| panic!("no job {key} in {:?}", d.jobs.iter().map(|j| &j.key).collect::<Vec<_>>()))
}

const CI: &str = r#"
name: ci
on:
  push: { branches: [main] }
  pull_request: {}
env: { GREETING: hello }
jobs:
  build:
    image: alpine:3.20
    env: { WHO: "${{ neutrino.branch }}" }
    steps:
      - run: echo "$GREETING from $WHO at ${{ neutrino.event }}"; test -f README.md
      - name: In a folder
        run: pwd | grep -q '/src$'
        working-directory: src
  test:
    needs: build
    image: alpine:3.20
    strategy: { matrix: { n: [1, 2] } }
    steps:
      - run: echo "test ${{ matrix.n }}"; test ${{ matrix.n }} -ne 2
      - name: Clean up
        if: failure()
        run: echo cleaning up
      - name: Never
        run: echo never
  after:
    needs: test
    image: alpine:3.20
    steps: [{ run: echo never }]
  always:
    needs: test
    if: always()
    image: alpine:3.20
    steps:
      - run: echo "test was ${{ needs.test.result }}"
"#;

const SLOW: &str = r#"
on: workflow_dispatch
jobs:
  wait:
    image: alpine:3.20
    timeout-minutes: 2
    steps: [{ run: "echo started; sleep 30; echo unreachable" }]
"#;

#[tokio::test]
async fn runs_from_pushes_and_pull_requests() {
    // SAFETY: set before any hook runs, and only read by the daemon this test starts.
    unsafe { std::env::set_var("NEUTRINO_HOOK_EXE", env!("CARGO_BIN_EXE_neutrinod")) };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let sock = root.join("s.sock");
    let options = neutrinod::Options {
        role: neutrinod::Role::Server,
        remote_port: None,
        advertise: false,
        ci: neutrinod::ci::CiOptions {
            runner: neutrinod::ci::Runner::Local(Default::default()),
            cpus: 4,
            memory_mib: 8192,
            // A workflow minute lasts a second here.
            minute: Duration::from_secs(1),
            ..Default::default()
        },
        vault: neutrino_vault::KeyStore::File(root.join("vault.key")),
    };
    let daemon = neutrinod::Daemon::with(&root.join("server"), options).unwrap();
    let d = Arc::clone(&daemon);
    let s = sock.clone();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move { neutrinod::serve(d, &s, async { stopped.await.ok(); }).await.unwrap() });
    while !sock.exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let hosted = match call(&sock, Request::CreateHosted { name: "app".into(), description: None }).await {
        Response::Repo(r) => r,
        other => panic!("{other:?}"),
    };
    let id = hosted.id.clone();

    let wc = root.join("wc");
    std::fs::create_dir_all(wc.join(".neutrino/workflows")).unwrap();
    std::fs::create_dir_all(wc.join("src")).unwrap();
    git(&wc, &["init", "-q", "-b", "main"]);
    std::fs::write(wc.join("README.md"), "hi\n").unwrap();
    std::fs::write(wc.join("src/lib.txt"), "x\n").unwrap();
    std::fs::write(wc.join(".neutrino/workflows/ci.yml"), CI).unwrap();
    std::fs::write(wc.join(".neutrino/workflows/slow.yml"), SLOW).unwrap();
    std::fs::write(wc.join(".neutrino/workflows/broken.yml"), "on: push\njobs: { a: { image: x, steps: [{ uses: x }] } }\n").unwrap();
    std::fs::write(
        wc.join(".neutrino/workflows/big.yml"),
        "on: { push: { branches: [main] } }\njobs: { huge: { image: x, resources: { cpus: 16 }, steps: [{ run: ls }] } }\n",
    )
    .unwrap();
    git(&wc, &["add", "."]);
    git(&wc, &["commit", "-q", "-m", "first"]);
    git(&wc, &["remote", "add", "origin", &hosted.path]);
    let (ok, said) = push(&wc, &["push", "origin", "main"]).await;
    assert!(ok && said.contains("started 3 CI runs"), "{said}");

    // A push to main starts ci, big and the broken file's run; slow is dispatch-only.
    let list = runs(&sock, &id).await;
    let by_file = |f: &str| list.iter().find(|r| r.workflow == f).unwrap_or_else(|| panic!("no run of {f}"));
    let broken = by_file("broken.yml");
    assert_eq!(broken.state, RunState::Failure);
    assert!(broken.error.as_deref().unwrap().contains("uses: isn't supported"), "{:?}", broken.error);

    let big = finished(&sock, &id, by_file("big.yml").id).await;
    assert!(big.jobs[0].error.as_deref().unwrap().contains("more than CI's budget"), "{:?}", big.jobs[0].error);

    let ci = finished(&sock, &id, by_file("ci.yml").id).await;
    assert!(ci.run.error.is_none(), "{:?}", ci.run.error);
    assert_eq!(ci.run.state, RunState::Failure, "test-2 fails");
    assert_eq!((ci.run.event.as_str(), ci.run.branch.as_deref()), ("push", Some("main")));
    let build = job(&ci, "build");
    assert_eq!(build.state, RunState::Success);
    assert_eq!(build.steps.iter().map(|s| s.state).collect::<Vec<_>>(), vec![RunState::Success, RunState::Success]);
    let build_log = log(&sock, &id, build.id).await;
    assert!(build_log.contains("hello from main at push"), "{build_log}");
    let first = &build.steps[0];
    assert!(build_log[first.log_start as usize..first.log_end as usize].contains("hello from main"), "a step's offsets cover its output");

    let (t1, t2) = (job(&ci, "test-1"), job(&ci, "test-2"));
    assert_eq!((t1.name.as_str(), t1.state), ("test (1)", RunState::Success));
    assert_eq!(t2.state, RunState::Failure);
    assert_eq!(t2.steps.iter().map(|s| s.state).collect::<Vec<_>>(), vec![RunState::Failure, RunState::Success, RunState::Skipped], "failure() runs, the rest skip");
    assert_eq!(t2.steps[0].exit_code, Some(1));
    assert!(log(&sock, &id, t2.id).await.contains("cleaning up"));
    assert_eq!(job(&ci, "after").state, RunState::Skipped, "a failed need skips what follows");
    let always = job(&ci, "always");
    assert_eq!(always.state, RunState::Success);
    assert!(log(&sock, &id, always.id).await.contains("test was failure"));
    // Workspaces are gone after the run.
    let leftover = std::fs::read_dir(root.join("server/runs").join(ci.run.id.to_string())).unwrap().flatten().filter(|e| e.path().is_dir()).count();
    assert_eq!(leftover, 0, "workspaces are removed at teardown");

    // A pull request runs ci on its head, with pull_request as the event.
    git(&wc, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(wc.join("README.md"), "changed\n").unwrap();
    git(&wc, &["commit", "-qam", "change"]);
    assert!(push(&wc, &["push", "-q", "origin", "feature"]).await.0);
    let before = runs(&sock, &id).await.len();
    assert!(matches!(
        call(&sock, Request::CreatePull { repo: id.clone(), title: "Change".into(), body: String::new(), source: "feature".into(), target: "main".into() }).await,
        Response::Pull(_)
    ));
    let after = runs(&sock, &id).await;
    assert_eq!(after.len(), before + 2, "ci, and the broken file showing it's broken");
    let pr_run = after.iter().find(|r| r.workflow == "ci.yml").unwrap();
    assert_eq!((pr_run.event.as_str(), pr_run.pull), ("pull_request", Some(1)));
    let pr = finished(&sock, &id, pr_run.id).await;
    assert!(log(&sock, &id, job(&pr, "build").id).await.contains("hello from feature at pull_request"));
    assert!(matches!(call(&sock, Request::ListRuns { repo: id.clone(), pull: Some(1), limit: None }).await, Response::Runs(r) if r.len() == 2));

    // The pull request shows its checks, and a rule requiring ci blocks the merge while ci fails.
    match call(&sock, Request::Pull { repo: id.clone(), number: 1 }).await {
        Response::Pull(d) => assert_eq!(d.checks.iter().map(|r| (r.workflow.as_str(), r.state)).collect::<Vec<_>>(), vec![("broken.yml", RunState::Failure), ("ci.yml", RunState::Failure)]),
        other => panic!("{other:?}"),
    }
    let rule = neutrino_proto::BranchRule { pattern: "main".into(), require_pull: false, block_force: false, block_delete: false, required_checks: vec!["ci".into()] };
    assert!(matches!(call(&sock, Request::SetBranchRule { repo: id.clone(), rule }).await, Response::Rules(_)));
    let merge = Request::MergePull { repo: id.clone(), number: 1, strategy: neutrino_proto::MergeStrategy::Merge, message: None, delete_source: false };
    assert!(matches!(call(&sock, merge).await, Response::Error { message } if message.contains("ci (failure)")), "a failing required check blocks the merge");

    // Cancel a run part-way.
    let slow = match call(&sock, Request::DispatchRun { repo: id.clone(), workflow: "slow".into(), branch: "main".into() }).await {
        Response::Run(d) => d.run.id,
        other => panic!("{other:?}"),
    };
    for _ in 0..100 {
        let d = detail(&sock, &id, slow).await;
        if log(&sock, &id, d.jobs[0].id).await.contains("started") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(matches!(call(&sock, Request::CancelRun { repo: id.clone(), id: slow }).await, Response::Run(_)));
    let cancelled = finished(&sock, &id, slow).await;
    assert_eq!(cancelled.run.state, RunState::Cancelled);
    assert_eq!(cancelled.jobs[0].steps[0].state, RunState::Cancelled);
    assert!(!log(&sock, &id, cancelled.jobs[0].id).await.contains("unreachable"));

    // Rerun it, and let the 2-"minute" timeout (2 s here) stop the 30 s sleep.
    let rerun = match call(&sock, Request::RerunRun { repo: id.clone(), id: slow }).await {
        Response::Run(d) => d.run.id,
        other => panic!("{other:?}"),
    };
    let timed = finished(&sock, &id, rerun).await;
    assert_eq!(timed.run.state, RunState::Failure);
    assert!(timed.jobs[0].error.as_deref().unwrap().contains("2-minute timeout"), "{:?}", timed.jobs[0].error);
    assert!(log(&sock, &id, timed.jobs[0].id).await.contains("timeout"));

    // A workflow that can't be started by hand says so.
    assert!(matches!(call(&sock, Request::DispatchRun { repo: id.clone(), workflow: "ci".into(), branch: "main".into() }).await, Response::Error { message } if message.contains("workflow_dispatch")));
    // The files on a branch, checked.
    match call(&sock, Request::Workflows { repo: id.clone(), branch: "main".into() }).await {
        Response::Workflows(files) => {
            assert_eq!(files.iter().map(|f| f.file.as_str()).collect::<Vec<_>>(), vec!["big.yml", "broken.yml", "ci.yml", "slow.yml"]);
            assert!(files[1].error.is_some() && files[2].triggers == vec!["push", "pull_request"]);
        }
        other => panic!("{other:?}"),
    }
    match call(&sock, Request::CiInfo).await {
        Response::Ci(info) => assert_eq!((info.graviton, info.budget_cpus, info.running), (true, 4, 0)),
        other => panic!("{other:?}"),
    }
    stop.send(()).ok();
}

/// The real thing: a job in a Graviton VM. Needs Graviton running here and
/// pulls alpine:3.20 on first use. `NEUTRINO_LIVE_GRAVITON=1 cargo test -p neutrinod --test ci graviton`
#[tokio::test]
async fn graviton() {
    if std::env::var("NEUTRINO_LIVE_GRAVITON").as_deref() != Ok("1") {
        return;
    }
    unsafe { std::env::set_var("NEUTRINO_HOOK_EXE", env!("CARGO_BIN_EXE_neutrinod")) };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let sock = root.join("s.sock");
    let options = neutrinod::Options {
        role: neutrinod::Role::Server,
        remote_port: None,
        advertise: false,
        ci: neutrinod::ci::CiOptions { cpus: 2, memory_mib: 1024, ..Default::default() },
        vault: neutrino_vault::KeyStore::File(root.join("vault.key")),
    };
    let daemon = neutrinod::Daemon::with(&root.join("server"), options).unwrap();
    let d = Arc::clone(&daemon);
    let s = sock.clone();
    tokio::spawn(async move { neutrinod::serve(d, &s, std::future::pending()).await.unwrap() });
    while !sock.exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let hosted = match call(&sock, Request::CreateHosted { name: "live".into(), description: None }).await {
        Response::Repo(r) => r,
        other => panic!("{other:?}"),
    };
    let wc = root.join("wc");
    std::fs::create_dir_all(wc.join(".neutrino/workflows")).unwrap();
    git(&wc, &["init", "-q", "-b", "main"]);
    std::fs::write(wc.join("hello.txt"), "from the workspace\n").unwrap();
    std::fs::write(
        wc.join(".neutrino/workflows/ci.yml"),
        "on: push\njobs:\n  linux:\n    image: alpine:3.20\n    resources: { cpus: 1, memory: 512M }\n    steps:\n      - run: uname -sm; cat hello.txt; echo \"$CI $NEUTRINO_BRANCH\"\n      - run: echo made-in-vm > out.txt; ls /workspace\n",
    )
    .unwrap();
    git(&wc, &["add", "."]);
    git(&wc, &["commit", "-q", "-m", "first"]);
    git(&wc, &["remote", "add", "origin", &hosted.path]);
    assert!(push(&wc, &["push", "-q", "origin", "main"]).await.0);
    let run = runs(&sock, &hosted.id).await[0].id;
    let mut d = detail(&sock, &hosted.id, run).await;
    for _ in 0..1800 {
        d = detail(&sock, &hosted.id, run).await;
        if d.run.state.finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let text = log(&sock, &hosted.id, d.jobs[0].id).await;
    eprintln!("{text}");
    assert_eq!(d.run.state, RunState::Success, "{text}");
    assert!(text.contains("Linux aarch64") && text.contains("from the workspace") && text.contains("true main"), "{text}");
}
