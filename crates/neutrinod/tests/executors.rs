//! The other executors: `runs-on: host` (refused unless allowed, then plain
//! processes), and `runs-on: nexus` against a small fake coordinator that
//! speaks Nexus's HTTP API and runs the job's driver with python3.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use neutrino_proto::{Request, Response, RunDetail, RunState, decode_response, encode_request};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixStream};

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

fn git(dir: &Path, args: &[&str]) {
    let out = git_out(dir, args);
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
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

async fn finished(socket: &Path, repo: &str, id: i64) -> RunDetail {
    for _ in 0..300 {
        if let Response::Run(d) = call(socket, Request::Run { repo: repo.into(), id }).await {
            if d.run.state.finished() {
                return *d;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("run {id} didn't finish");
}

async fn log(socket: &Path, repo: &str, job: i64) -> String {
    match call(socket, Request::JobLog { repo: repo.into(), job, offset: 0 }).await {
        Response::JobLog(l) => l.text,
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// A fake Nexus coordinator
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FakeJob {
    status: String,
    exit_code: Option<i32>,
    log: Vec<String>,
    dir: PathBuf,
    spec: serde_json::Value,
    pid: Option<u32>,
}

#[derive(Default)]
struct Fake {
    projects: HashMap<String, Vec<u8>>,
    jobs: HashMap<String, FakeJob>,
    uploads: usize,
}

async fn respond(stream: &mut tokio::net::TcpStream, code: u16, kind: &str, body: &[u8]) {
    let head = format!("HTTP/1.1 {code} X\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body).await;
}

/// Serve Nexus's job routes until the test ends. Returns its URL.
async fn fake_nexus(root: PathBuf, token: &'static str) -> (String, Arc<Mutex<Fake>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state: Arc<Mutex<Fake>> = Default::default();
    let st = Arc::clone(&state);
    tokio::spawn(async move {
        let mut n = 0;
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            let st = Arc::clone(&st);
            n += 1;
            let root = root.clone();
            tokio::spawn(async move {
                // Head, then a body of content-length.
                let mut buf = Vec::new();
                let mut tmp = [0u8; 65536];
                let head_end = loop {
                    let k = s.read(&mut tmp).await.unwrap();
                    if k == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..k]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let len: usize = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
                while buf.len() < head_end + len {
                    let k = s.read(&mut tmp).await.unwrap();
                    buf.extend_from_slice(&tmp[..k]);
                }
                let body = buf[head_end..head_end + len].to_vec();
                let mut first = head.lines().next().unwrap_or("").split_whitespace();
                let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
                if !head.contains(&format!("Bearer {token}")) {
                    return respond(&mut s, 401, "application/json", br#"{"detail":"missing or wrong bearer token"}"#).await;
                }
                let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
                match (method.as_str(), parts.as_slice()) {
                    ("GET", ["v1", "projects", d]) => {
                        let have = st.lock().unwrap().projects.contains_key(*d);
                        respond(&mut s, if have { 200 } else { 404 }, "application/json", b"{}").await
                    }
                    ("PUT", ["v1", "projects", d]) => {
                        {
                            let mut f = st.lock().unwrap();
                            f.projects.insert(d.to_string(), body);
                            f.uploads += 1;
                        }
                        respond(&mut s, 200, "application/json", br#"{"ok":true}"#).await
                    }
                    ("POST", ["v1", "jobs"]) => {
                        let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
                        let id = format!("j{n}");
                        let dir = root.join(&id);
                        let snap = st.lock().unwrap().projects[spec["project"].as_str().unwrap()].clone();
                        std::fs::create_dir_all(dir.join("project")).unwrap();
                        std::fs::create_dir_all(dir.join("outputs")).unwrap();
                        std::fs::write(dir.join("snap.tgz"), &snap).unwrap();
                        assert!(Command::new("tar").arg("-xzf").arg(dir.join("snap.tgz")).arg("-C").arg(dir.join("project")).status().unwrap().success());
                        let held = spec["name"].as_str().unwrap_or("").contains("held");
                        st.lock().unwrap().jobs.insert(id.clone(), FakeJob { status: if held { "held".into() } else { "running".into() }, dir: dir.clone(), spec, ..Default::default() });
                        if !held {
                            run_job(Arc::clone(&st), id.clone());
                        }
                        let out = serde_json::json!([{ "id": id, "status": "queued" }]);
                        respond(&mut s, 200, "application/json", out.to_string().as_bytes()).await
                    }
                    ("GET", ["v1", "jobs", id]) => {
                        let out = {
                            let f = st.lock().unwrap();
                            let j = &f.jobs[*id];
                            serde_json::json!({ "id": id, "status": j.status, "exit_code": j.exit_code, "reason": "" })
                        };
                        respond(&mut s, 200, "application/json", out.to_string().as_bytes()).await
                    }
                    ("POST", ["v1", "jobs", id, "stop"]) => {
                        let pid = st.lock().unwrap().jobs.get(*id).and_then(|j| j.pid);
                        if let Some(pid) = pid {
                            let _ = Command::new("kill").arg(format!("-{pid}")).status();
                            let _ = Command::new("kill").arg(pid.to_string()).status();
                        }
                        st.lock().unwrap().jobs.get_mut(*id).unwrap().status = "cancelled".into();
                        respond(&mut s, 200, "application/json", b"{}").await
                    }
                    ("GET", ["v1", "jobs", id, "logs"]) => {
                        // Wait for the end, then send it all as server-sent events.
                        loop {
                            let done = matches!(st.lock().unwrap().jobs[*id].status.as_str(), "finished" | "failed" | "cancelled");
                            if done {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                        let lines = st.lock().unwrap().jobs[*id].log.clone();
                        let mut sse = String::new();
                        for l in lines {
                            sse.push_str(&format!("data: {}\n\n", serde_json::to_string(&l).unwrap()));
                        }
                        sse.push_str("event: end\ndata: \"\"\n\n");
                        respond(&mut s, 200, "text/event-stream", sse.as_bytes()).await
                    }
                    ("GET", ["v1", "jobs", id, "outputs"]) => {
                        let dir = st.lock().unwrap().jobs[*id].dir.clone();
                        let out = Command::new("tar").env("COPYFILE_DISABLE", "1").arg("-czf").arg("-").arg("-C").arg(dir.join("outputs")).arg(".").output().unwrap();
                        respond(&mut s, 200, "application/gzip", &out.stdout).await
                    }
                    _ => respond(&mut s, 404, "application/json", br#"{"detail":"no route"}"#).await,
                }
            });
        }
    });
    (url, state)
}

/// Run a job's command the way a Mac node does, with python3 standing in for `uv run`.
fn run_job(st: Arc<Mutex<Fake>>, id: String) {
    std::thread::spawn(move || {
        let (dir, command) = {
            let f = st.lock().unwrap();
            let j = &f.jobs[&id];
            (j.dir.clone(), j.spec["command"].as_str().unwrap().to_string())
        };
        assert_eq!(command, "uv run --no-project python .neutrino-run/run.py");
        let mut child = Command::new("python3")
            .arg(".neutrino-run/run.py")
            .current_dir(dir.join("project"))
            .env("DUET_OUTPUT_DIR", dir.join("outputs"))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .unwrap();
        st.lock().unwrap().jobs.get_mut(&id).unwrap().pid = Some(child.id());
        let mut out = String::new();
        std::io::Read::read_to_string(child.stdout.as_mut().unwrap(), &mut out).unwrap();
        let code = child.wait().unwrap().code();
        let mut f = st.lock().unwrap();
        let j = f.jobs.get_mut(&id).unwrap();
        j.log = out.lines().map(String::from).collect();
        if j.status != "cancelled" {
            j.exit_code = code;
            j.status = if code == Some(0) { "finished".into() } else { "failed".into() };
        }
    });
}

const WORKFLOW: &str = r#"
on: workflow_dispatch
jobs:
  mac:
    runs-on: host
    steps:
      - run: echo "on the host in $(basename "$PWD") with $REGION"
        env: { REGION: "${{ vars.REGION }}" }
  windows:
    runs-on: nexus
    nexus: { node: forge }
    timeout-minutes: 5
    artifacts: [dist/*.txt]
    steps:
      - name: Build
        run: |
          echo "building for ${{ vars.REGION }}"
          mkdir -p dist && echo installer > dist/setup.txt && echo skip > dist/notes.md
      - name: Fails but goes on
        run: exit 3
        continue-on-error: true
      - name: Only after a failure
        if: failure()
        run: echo never
      - name: Always
        if: always()
        shell: sh
        run: echo "always runs; event $NEUTRINO_EVENT"
  secretive:
    runs-on: nexus
    steps:
      - run: echo ${{ secrets.TOKEN }}
"#;

#[tokio::test]
async fn host_and_nexus() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("no python3; skipping");
        return;
    }
    // SAFETY: set before any hook runs, and only read by the daemons this test starts.
    unsafe { std::env::set_var("NEUTRINO_HOOK_EXE", env!("CARGO_BIN_EXE_neutrinod")) };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (nexus_url, fake) = fake_nexus(root.join("nexus"), "test-token").await;
    std::fs::write(root.join("token"), "test-token\n").unwrap();

    let start = |name: &str, host: bool, nexus: bool| {
        let sock = root.join(format!("{name}.sock"));
        let options = neutrinod::Options {
            role: neutrinod::Role::Server,
            remote_port: None,
            advertise: false,
            ci: neutrinod::ci::CiOptions {
                runner: neutrinod::ci::Runner::Local(Default::default()),
                minute: Duration::from_secs(1),
                host: host.then(neutrinod::ci::Local::default),
                nexus: nexus.then(|| neutrinod::ci::nexus::Nexus::new(&nexus_url, &root.join("token")).unwrap()),
                ..Default::default()
            },
            vault: neutrino_vault::KeyStore::File(root.join(format!("{name}.key"))),
            copy_backups_from: None,
        };
        let daemon = neutrinod::Daemon::with(&root.join(name), options).unwrap();
        let s = sock.clone();
        tokio::spawn(async move { neutrinod::serve(daemon, &s, std::future::pending()).await.unwrap() });
        sock
    };

    let sock = start("server", true, true);
    let plain = start("plain", false, false);
    for s in [&sock, &plain] {
        while !s.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    let wc = root.join("wc");
    std::fs::create_dir_all(wc.join(".neutrino/workflows")).unwrap();
    git(&wc, &["init", "-q", "-b", "main"]);
    std::fs::write(wc.join(".neutrino/workflows/ci.yml"), WORKFLOW).unwrap();
    git(&wc, &["add", "."]);
    git(&wc, &["commit", "-q", "-m", "first"]);

    for s in [&sock, &plain] {
        let hosted = match call(s, Request::CreateHosted { name: "app".into(), description: None }).await {
            Response::Repo(r) => r,
            other => panic!("{other:?}"),
        };
        let _ = git_out(&wc, &["remote", "remove", "origin"]);
        git(&wc, &["remote", "add", "origin", &hosted.path]);
        assert!(push(&wc, &["push", "-q", "origin", "main"]).await.0);
        assert!(matches!(
            call(s, Request::SetVariable { scope: Default::default(), name: "REGION".into(), value: "eu".into() }).await,
            Response::Variables(_)
        ));
    }

    // A server that hasn't allowed them refuses host and Nexus jobs, saying how to allow them.
    let id = match call(&plain, Request::DispatchRun { repo: "app".into(), workflow: "ci".into(), branch: "main".into() }).await {
        Response::Run(d) => d.run.id,
        other => panic!("{other:?}"),
    };
    let d = finished(&plain, "app", id).await;
    let err = |key: &str| d.jobs.iter().find(|j| j.key == key).unwrap().error.clone().unwrap_or_default();
    assert!(err("mac").contains("--host-executor"), "{}", err("mac"));
    assert!(err("windows").contains("--nexus-token-file"), "{}", err("windows"));

    // One that has: the host job runs here, the Nexus job through the coordinator.
    let id = match call(&sock, Request::DispatchRun { repo: "app".into(), workflow: "ci".into(), branch: "main".into() }).await {
        Response::Run(d) => d.run.id,
        other => panic!("{other:?}"),
    };
    let d = finished(&sock, "app", id).await;
    let job = |key: &str| d.jobs.iter().find(|j| j.key == key).unwrap().clone();

    let mac = job("mac");
    assert_eq!(mac.state, RunState::Success, "{:?}", mac.error);
    assert!(log(&sock, "app", mac.id).await.contains("on the host in workspace with eu"));

    let win = job("windows");
    let text = log(&sock, "app", win.id).await;
    assert_eq!(win.state, RunState::Success, "continue-on-error keeps the job passing: {text}");
    assert_eq!(
        win.steps.iter().map(|s| (s.state, s.exit_code)).collect::<Vec<_>>(),
        vec![(RunState::Success, Some(0)), (RunState::Failure, Some(3)), (RunState::Skipped, None), (RunState::Success, Some(0))],
        "{text}"
    );
    assert!(text.contains("building for eu") && text.contains("always runs; event workflow_dispatch"), "{text}");
    assert!(!text.contains("##neutrino["), "markers become steps, not log lines: {text}");
    let build = &win.steps[0];
    assert!(text[build.log_start as usize..build.log_end as usize].contains("building for eu"), "each step's output lines up");
    match call(&sock, Request::ListArtifacts { repo: "app".into(), run: id }).await {
        Response::Artifacts(a) => assert_eq!(a.iter().map(|a| a.path.as_str()).collect::<Vec<_>>(), vec!["dist/setup.txt"]),
        other => panic!("{other:?}"),
    }
    {
        let f = fake.lock().unwrap();
        let spec = &f.jobs.values().find(|j| j.spec["node"] == "forge").unwrap().spec;
        assert_eq!(spec["project_name"], "app");
        assert!(spec["requested_by"].as_str().unwrap().starts_with("neutrino:"));
    }

    // Nothing secret goes to Nexus.
    let sec = job("secretive");
    assert_eq!(sec.error.as_deref(), Some("secrets aren't sent to Nexus"));

    // A rerun goes through again (each run's snapshot carries its own run number).
    let again = match call(&sock, Request::RerunRun { repo: "app".into(), id }).await {
        Response::Run(d) => d.run.id,
        other => panic!("{other:?}"),
    };
    let d = finished(&sock, "app", again).await;
    assert_eq!(d.jobs.iter().find(|j| j.key == "windows").unwrap().state, RunState::Success);
    assert_eq!(fake.lock().unwrap().uploads, 2);
}
