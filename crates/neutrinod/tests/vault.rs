//! Milestone 6 end to end, with steps run as local processes: the vault and
//! masking, variables, environments with approvals and branch limits, caches,
//! artifacts, concurrency groups, schedules and database snapshots.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use neutrino_proto::{Environment, Request, Response, RunDetail, RunState, VaultScope, decode_response, encode_request};
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

async fn detail(socket: &Path, repo: &str, id: i64) -> RunDetail {
    match call(socket, Request::Run { repo: repo.into(), id }).await {
        Response::Run(d) => *d,
        other => panic!("{other:?}"),
    }
}

/// Wait until `done` holds for the run.
async fn until(socket: &Path, repo: &str, id: i64, done: impl Fn(&RunDetail) -> bool) -> RunDetail {
    for _ in 0..300 {
        let d = detail(socket, repo, id).await;
        if done(&d) {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("run {id} never got there: {:#?}", detail(socket, repo, id).await);
}

async fn log(socket: &Path, repo: &str, job: i64) -> String {
    match call(socket, Request::JobLog { repo: repo.into(), job, offset: 0 }).await {
        Response::JobLog(l) => l.text,
        other => panic!("{other:?}"),
    }
}

async fn dispatch(socket: &Path, repo: &str, workflow: &str, branch: &str) -> i64 {
    match call(socket, Request::DispatchRun { repo: repo.into(), workflow: workflow.into(), branch: branch.into() }).await {
        Response::Run(d) => d.run.id,
        other => panic!("{other:?}"),
    }
}

fn job<'a>(d: &'a RunDetail, key: &str) -> &'a neutrino_proto::RunJob {
    d.jobs.iter().find(|j| j.key == key).unwrap()
}

const SECRET: &str = "tok-1234-very-secret";
const DEPLOY: &str = "deploy-key-abcdef";

const CI: &str = r#"
on:
  push: { branches: [main] }
  workflow_dispatch: {}
  schedule: [{ cron: "0 4 * * *" }]
jobs:
  build:
    image: alpine:3.20
    cache:
      - key: deps-${{ hashFiles('deps.txt') }}
        path: deps
        restore-keys: [deps-]
    artifacts: [out]
    steps:
      - run: |
          echo "token is ${{ secrets.TOKEN }}"
          echo "env token $TOKEN"
          printf '%s' "$TOKEN" | base64
        env: { TOKEN: "${{ secrets.TOKEN }}" }
      - run: echo "region ${{ vars.REGION }}"
      - run: |
          if [ -f deps/marker ]; then echo "cache warm"; else echo "cache cold"; mkdir -p deps; echo x > deps/marker; fi
          mkdir -p out; echo built > out/app.txt
  deploy:
    needs: build
    environment: prod
    image: alpine:3.20
    steps:
      - run: echo "deploy with ${{ secrets.DEPLOY_KEY }} and ${{ secrets.TOKEN }} in ${{ vars.REGION }}"
"#;

const SERIAL: &str = r#"
on: workflow_dispatch
concurrency: serial
jobs:
  one:
    image: alpine:3.20
    steps: [{ run: sleep 1 }]
"#;

const LATEST: &str = r#"
on: workflow_dispatch
concurrency: { group: "latest-${{ neutrino.branch }}", cancel-in-progress: true }
jobs:
  one:
    image: alpine:3.20
    steps: [{ run: sleep 20 }]
"#;

const SERVICES: &str = r#"
on: workflow_dispatch
jobs:
  with-db:
    image: alpine:3.20
    services: { db: postgres:17 }
    steps: [{ run: echo "$DB_HOST" }]
"#;

#[tokio::test]
async fn the_rest_of_ci() {
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
            minute: Duration::from_secs(1),
            backup_dir: Some(root.join("snapshots")),
            ..Default::default()
        },
        vault: neutrino_vault::KeyStore::File(root.join("vault.key")),
        ..Default::default()
    };
    let daemon = neutrinod::Daemon::with(&root.join("server"), options).unwrap();
    let d = Arc::clone(&daemon);
    let s = sock.clone();
    tokio::spawn(async move { neutrinod::serve(d, &s, std::future::pending()).await.unwrap() });
    while !sock.exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let hosted = match call(&sock, Request::CreateHosted { name: "app".into(), description: None }).await {
        Response::Repo(r) => r,
        other => panic!("{other:?}"),
    };
    let id = hosted.id.clone();
    let repo_scope = VaultScope { repo: Some(id.clone()), environment: None };

    // The vault: nothing until it's set up, then a recovery code once.
    let set = |scope: VaultScope, name: &str, value: &str| Request::SetSecret { scope, name: name.into(), value: value.into() };
    assert!(matches!(call(&sock, set(VaultScope::default(), "TOKEN", "x")).await, Response::Error { message } if message.contains("vault setup")));
    let code = match call(&sock, Request::VaultSetup).await {
        Response::RecoveryCode { code } => code,
        other => panic!("{other:?}"),
    };
    assert_eq!(code.len(), 64, "13 groups of four with dashes");
    assert!(matches!(call(&sock, Request::VaultSetup).await, Response::Error { .. }), "the code is shown once");
    assert!(matches!(call(&sock, set(VaultScope::default(), "TOKEN", "global-value")).await, Response::Secrets(_)));
    assert!(matches!(call(&sock, set(repo_scope.clone(), "TOKEN", SECRET)).await, Response::Secrets(_)));
    let prod = VaultScope { repo: Some(id.clone()), environment: Some("prod".into()) };
    assert!(matches!(call(&sock, set(prod.clone(), "DEPLOY_KEY", DEPLOY)).await, Response::Error { message } if message.contains("no environment prod")));
    let env = Environment { repo: id.clone(), name: "prod".into(), require_approval: true, branches: vec!["main".into()] };
    assert!(matches!(call(&sock, Request::SetEnvironment { environment: env }).await, Response::Environments(e) if e.len() == 1));
    assert!(matches!(call(&sock, set(prod.clone(), "DEPLOY_KEY", DEPLOY)).await, Response::Secrets(_)));
    match call(&sock, Request::ListSecrets { scope: prod.clone() }).await {
        Response::Secrets(list) => {
            assert_eq!(list.len(), 3);
            let text = format!("{list:?}");
            assert!(!text.contains(SECRET) && !text.contains(DEPLOY), "listing never shows values");
        }
        other => panic!("{other:?}"),
    }
    let db = std::fs::read(root.join("server/neutrino.db")).unwrap();
    assert!(!db.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()), "secrets are sealed at rest");
    assert!(matches!(call(&sock, Request::SetVariable { scope: VaultScope::default(), name: "REGION".into(), value: "eu".into() }).await, Response::Variables(_)));

    let wc = root.join("wc");
    std::fs::create_dir_all(wc.join(".neutrino/workflows")).unwrap();
    git(&wc, &["init", "-q", "-b", "main"]);
    std::fs::write(wc.join("deps.txt"), "serde 1\n").unwrap();
    for (f, src) in [("ci.yml", CI), ("serial.yml", SERIAL), ("latest.yml", LATEST), ("services.yml", SERVICES)] {
        std::fs::write(wc.join(".neutrino/workflows").join(f), src).unwrap();
    }
    git(&wc, &["add", "."]);
    git(&wc, &["commit", "-q", "-m", "first"]);
    git(&wc, &["remote", "add", "origin", &hosted.path]);
    let (ok, said) = push(&wc, &["push", "origin", "main"]).await;
    assert!(ok && said.contains("started 1 CI run"), "{said}");
    let first = match call(&sock, Request::ListRuns { repo: id.clone(), pull: None, limit: None }).await {
        Response::Runs(r) => r[0].id,
        other => panic!("{other:?}"),
    };

    // build runs; deploy waits for approval.
    let d = until(&sock, &id, first, |d| job(d, "deploy").state == RunState::Waiting).await;
    assert_eq!(d.run.state, RunState::Waiting);
    let build = job(&d, "build");
    assert_eq!(build.state, RunState::Success);
    let text = log(&sock, &id, build.id).await;
    assert!(!text.contains(SECRET), "the secret is masked: {text}");
    assert!(text.contains("token is ***") && text.contains("env token ***"), "{text}");
    assert!(!text.contains("dG9rLTEyMzQtdmVyeS1zZWNyZXQ="), "and its base64: {text}");
    assert!(text.contains("region eu") && text.contains("cache cold") && text.contains("saved cache deps-"), "{text}");
    assert!(text.contains("kept 1 artifact file"), "{text}");
    match call(&sock, Request::ListArtifacts { repo: id.clone(), run: first }).await {
        Response::Artifacts(a) => assert_eq!((a.len(), a[0].path.as_str(), a[0].size), (1, "out/app.txt", 6)),
        other => panic!("{other:?}"),
    }
    match call(&sock, Request::ArtifactData { repo: id.clone(), run: first, path: "build/out/app.txt".into(), offset: 0 }).await {
        Response::ArtifactChunk(c) => {
            use base64::Engine as _;
            assert_eq!(base64::engine::general_purpose::STANDARD.decode(&c.data).unwrap(), b"built\n");
            assert!(c.done);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(call(&sock, Request::ArtifactData { repo: id.clone(), run: first, path: "../neutrino.db".into(), offset: 0 }).await, Response::Error { .. }));

    // Approve it: it runs with the environment's secret, masked too.
    let deploy = job(&d, "deploy").id;
    assert!(matches!(call(&sock, Request::ApproveJob { repo: id.clone(), job: deploy, approve: true }).await, Response::Run(_)));
    let d = until(&sock, &id, first, |d| d.run.state.finished()).await;
    assert_eq!(d.run.state, RunState::Success);
    let dj = job(&d, "deploy");
    assert_eq!(dj.approved_by.as_deref(), Some("the server"));
    let text = log(&sock, &id, dj.id).await;
    assert!(text.contains("deploy with *** and *** in eu") && text.contains("approved by the server"), "{text}");

    // Again: the cache is warm. This time turn the deploy down.
    let second = dispatch(&sock, &id, "ci", "main").await;
    let d = until(&sock, &id, second, |d| job(d, "deploy").state == RunState::Waiting).await;
    let text = log(&sock, &id, job(&d, "build").id).await;
    assert!(text.contains("cache warm") && text.contains("restored") && !text.contains("saved cache"), "an exact hit isn't saved again: {text}");
    assert!(matches!(call(&sock, Request::ApproveJob { repo: id.clone(), job: job(&d, "deploy").id, approve: false }).await, Response::Run(_)));
    let d = until(&sock, &id, second, |d| d.run.state.finished()).await;
    assert_eq!(d.run.state, RunState::Failure);
    assert_eq!(job(&d, "deploy").error.as_deref(), Some("turned down by the server"));

    // A branch prod doesn't allow can't deploy.
    git(&wc, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(wc.join("deps.txt"), "serde 2\n").unwrap();
    git(&wc, &["commit", "-qam", "bump"]);
    assert!(push(&wc, &["push", "-q", "origin", "feature"]).await.0);
    let third = dispatch(&sock, &id, "ci", "feature").await;
    let d = until(&sock, &id, third, |d| d.run.state.finished()).await;
    assert!(job(&d, "deploy").error.as_deref().unwrap().contains("feature can't deploy to prod"), "{:?}", job(&d, "deploy").error);
    let text = log(&sock, &id, job(&d, "build").id).await;
    assert!(text.contains("not found; restored deps-") && text.contains("cache warm"), "a restore key brings back the closest cache: {text}");

    // A group without cancelling: the second run waits for the first.
    let a = dispatch(&sock, &id, "serial", "main").await;
    let b = dispatch(&sock, &id, "serial", "main").await;
    let (da, db) = (until(&sock, &id, a, |d| d.run.state.finished()).await, until(&sock, &id, b, |d| d.run.state.finished()).await);
    assert!(db.jobs[0].started_at.unwrap() >= da.jobs[0].finished_at.unwrap(), "one at a time");
    // With cancel-in-progress, the newer run cancels the older.
    let old = dispatch(&sock, &id, "latest", "main").await;
    until(&sock, &id, old, |d| d.jobs[0].state == RunState::Running).await;
    let new = dispatch(&sock, &id, "latest", "main").await;
    assert_eq!(until(&sock, &id, old, |d| d.run.state.finished()).await.run.state, RunState::Cancelled);
    assert!(matches!(call(&sock, Request::CancelRun { repo: id.clone(), id: new }).await, Response::Run(_)));

    // Services need Graviton.
    let svc = dispatch(&sock, &id, "services", "main").await;
    let d = until(&sock, &id, svc, |d| d.run.state.finished()).await;
    assert!(log(&sock, &id, d.jobs[0].id).await.contains("service containers need Graviton"));

    // Schedules fire once per minute, on the default branch.
    let four = 1_794_715_200; // 2026-11-15 04:00 UTC
    let fired = tokio::task::spawn_blocking({
        let d = Arc::clone(&daemon);
        move || (d.fire_schedules(four), d.fire_schedules(four + 30), d.fire_schedules(four + 60))
    })
    .await
    .unwrap();
    assert_eq!((fired.0.len(), fired.1.len(), fired.2.len()), (1, 0, 0), "once at 04:00, not again that minute, not at 04:01");
    let sched = detail(&sock, &id, fired.0[0]).await;
    assert_eq!((sched.run.event.as_str(), sched.run.branch.as_deref()), ("schedule", Some("main")));
    assert!(matches!(call(&sock, Request::CancelRun { repo: id.clone(), id: sched.run.id }).await, Response::Run(_)));

    // A snapshot of the database, readable on its own.
    match call(&sock, Request::Backup).await {
        Response::Done { output } => {
            let snap = std::fs::read(&output).unwrap();
            assert!(snap.starts_with(b"SQLite format 3"), "{output}");
            assert!(output.starts_with(root.join("snapshots").to_str().unwrap()));
        }
        other => panic!("{other:?}"),
    }

    // The recovery code opens the same secrets on another server.
    let other = neutrino_vault::Vault::new(neutrino_vault::KeyStore::File(root.join("elsewhere.key")));
    other.recover(&code).unwrap();
}

/// A service container through the real Graviton, reached from the job by its
/// address. `NEUTRINO_LIVE_GRAVITON=1 cargo test -p neutrinod --test vault graviton_services`
#[tokio::test]
async fn graviton_services() {
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
        ci: neutrinod::ci::CiOptions { cpus: 3, memory_mib: 2048, ..Default::default() },
        vault: neutrino_vault::KeyStore::File(root.join("vault.key")),
        ..Default::default()
    };
    let daemon = neutrinod::Daemon::with(&root.join("server"), options).unwrap();
    let d = Arc::clone(&daemon);
    let s = sock.clone();
    tokio::spawn(async move { neutrinod::serve(d, &s, std::future::pending()).await.unwrap() });
    while !sock.exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let hosted = match call(&sock, Request::CreateHosted { name: "svc".into(), description: None }).await {
        Response::Repo(r) => r,
        other => panic!("{other:?}"),
    };
    let wc = root.join("wc");
    std::fs::create_dir_all(wc.join(".neutrino/workflows")).unwrap();
    git(&wc, &["init", "-q", "-b", "main"]);
    std::fs::write(
        wc.join(".neutrino/workflows/ci.yml"),
        "on: push\njobs:\n  ping:\n    image: alpine:3.20\n    resources: { cpus: 1, memory: 512M }\n    services: { cache: redis:7-alpine }\n    steps:\n      - run: |\n          echo \"cache at $CACHE_HOST\"\n          for i in $(seq 1 30); do nc -z \"$CACHE_HOST\" 6379 && break; sleep 1; done\n          printf 'PING\\r\\n' | nc -w 2 \"$CACHE_HOST\" 6379\n",
    )
    .unwrap();
    git(&wc, &["add", "."]);
    git(&wc, &["commit", "-q", "-m", "first"]);
    git(&wc, &["remote", "add", "origin", &hosted.path]);
    assert!(push(&wc, &["push", "-q", "origin", "main"]).await.0);
    let run = match call(&sock, Request::ListRuns { repo: hosted.id.clone(), pull: None, limit: None }).await {
        Response::Runs(r) => r[0].id,
        other => panic!("{other:?}"),
    };
    let mut d = detail(&sock, &hosted.id, run).await;
    for _ in 0..3000 {
        d = detail(&sock, &hosted.id, run).await;
        if d.run.state.finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let text = log(&sock, &hosted.id, d.jobs[0].id).await;
    eprintln!("{text}");
    assert_eq!(d.run.state, RunState::Success, "{text}");
    assert!(text.contains("+PONG"), "{text}");
}
