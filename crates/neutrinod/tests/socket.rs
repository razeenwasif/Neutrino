//! Runs a daemon on a temporary socket and talks to it like a client would.

use std::process::Command;
use std::time::Duration;

use neutrino_proto::{LogQuery, Request, Response, decode_response, encode_request};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

struct Client {
    lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    write: tokio::net::unix::OwnedWriteHalf,
}

impl Client {
    async fn send(&mut self, id: u64, request: &Request) {
        self.write.write_all(format!("{}\n", encode_request(id, request)).as_bytes()).await.unwrap();
    }

    async fn raw(&mut self, line: &str) {
        self.write.write_all(format!("{line}\n").as_bytes()).await.unwrap();
    }

    async fn recv(&mut self) -> (u64, Response) {
        let line = tokio::time::timeout(Duration::from_secs(10), self.lines.next_line()).await.unwrap().unwrap().unwrap();
        decode_response(&line).unwrap()
    }

    async fn call(&mut self, id: u64, request: Request) -> Response {
        self.send(id, &request).await;
        let (got, response) = self.recv().await;
        assert_eq!(got, id);
        response
    }
}

#[tokio::test]
async fn end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("My Project");
    std::fs::create_dir(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    std::fs::write(work.join("a.txt"), "one\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "first"]);
    std::fs::write(work.join("a.txt"), "two\n").unwrap();
    git(&work, &["commit", "-q", "-am", "second"]);

    let state = dir.path().join("state");
    let socket = dir.path().join("d.sock");
    let daemon = neutrinod::Daemon::new(&state).unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn({
        let socket = socket.clone();
        async move { neutrinod::serve(daemon, &socket, async { stopped.await.ok(); }).await }
    });
    while !socket.exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let stream = UnixStream::connect(&socket).await.unwrap();
    let (read, write) = stream.into_split();
    let mut c = Client { lines: BufReader::new(read).lines(), write };

    assert_eq!(c.call(1, Request::Ping).await, Response::Pong { protocol: neutrino_proto::PROTOCOL_VERSION });

    // Register from a subfolder-free path; adding twice gives the same repo.
    let added = match c.call(2, Request::AddRepo { path: work.display().to_string() }).await {
        Response::Repo(r) => r,
        other => panic!("{other:?}"),
    };
    assert_eq!(added.id, "my-project");
    assert_eq!(added.name, "My Project");
    let again = c.call(3, Request::AddRepo { path: work.display().to_string() }).await;
    assert!(matches!(again, Response::Repo(r) if r.id == "my-project"));
    match c.call(4, Request::ListRepos).await {
        Response::Repos(list) => assert_eq!(list.len(), 1),
        other => panic!("{other:?}"),
    }

    // Several requests in flight on one connection, answered by id.
    c.send(10, &Request::Log(LogQuery { repo: "my-project".into(), ..Default::default() })).await;
    c.send(11, &Request::Branches { repo: "my-project".into() }).await;
    c.send(12, &Request::EngineInfo).await;
    let mut answers = std::collections::HashMap::new();
    for _ in 0..3 {
        let (id, r) = c.recv().await;
        answers.insert(id, r);
    }
    let Response::Log(page) = &answers[&10] else { panic!("{:?}", answers[&10]) };
    assert_eq!(page.commits.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["second", "first"]);
    assert!(matches!(&answers[&11], Response::Branches(b) if b.len() == 1));
    assert!(matches!(&answers[&12], Response::Engine(e) if e.repos == 1 && e.role == "workstation"));

    let head = page.commits[0].sha.clone();
    match c.call(13, Request::Commit { repo: "my-project".into(), sha: head.clone() }).await {
        Response::Commit(d) => assert_eq!(d.files[0].path, "a.txt"),
        other => panic!("{other:?}"),
    }
    match c.call(14, Request::FileDiff { repo: "my-project".into(), sha: head, path: "a.txt".into(), context: None }).await {
        Response::FileDiff(d) => assert_eq!((d.additions, d.deletions), (1, 1)),
        other => panic!("{other:?}"),
    }

    // Errors come back as answers, not dropped connections.
    assert!(matches!(c.call(20, Request::Branches { repo: "nope".into() }).await, Response::Error { message } if message.contains("no repo")));
    c.raw(r#"{"id":21,"method":"what"}"#).await;
    assert!(matches!(c.recv().await, (21, Response::Error { .. })));
    c.raw("not json").await;
    assert!(matches!(c.recv().await, (0, Response::Error { .. })));
    // Cancelling something that already finished is harmless.
    c.send(22, &Request::Cancel { id: 14 }).await;
    assert_eq!(c.call(23, Request::Ping).await, Response::Pong { protocol: neutrino_proto::PROTOCOL_VERSION });

    // The registry survives a restart.
    assert!(state.join("repos.json").exists());
    assert!(matches!(c.call(24, Request::RemoveRepo { repo: "my-project".into() }).await, Response::Removed { .. }));
    assert!(matches!(c.call(25, Request::ListRepos).await, Response::Repos(l) if l.is_empty()));

    // A second daemon refuses the live socket.
    let other = neutrinod::Daemon::new(&state).unwrap();
    assert!(neutrinod::serve(other, &socket, async {}).await.is_err());

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(!socket.exists(), "the socket is removed on shutdown");
}

#[tokio::test]
async fn missing_folder_is_listed_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("gone");
    std::fs::create_dir(&work).unwrap();
    git(&work, &["init", "-q"]);
    let daemon = neutrinod::Daemon::new(&dir.path().join("state")).unwrap();
    assert!(matches!(daemon.handle(Request::AddRepo { path: work.display().to_string() }).await, Response::Repo(_)));
    // Re-load the registry so nothing is cached, then delete the folder.
    let daemon = neutrinod::Daemon::new(&dir.path().join("state")).unwrap();
    std::fs::remove_dir_all(&work).unwrap();
    match daemon.handle(Request::ListRepos).await {
        Response::Repos(list) => {
            assert_eq!(list.len(), 1);
            assert!(list[0].head.is_none());
            assert!(list[0].error.as_deref().unwrap().contains("no longer exists"));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(daemon.handle(Request::AddRepo { path: "relative".into() }).await, Response::Error { .. }));
}

#[tokio::test]
async fn working_copy_over_the_socket() {
    use neutrino_proto::{Area, LineKind, LinePick};
    let dir = tempfile::tempdir().unwrap();
    // macOS reports FSEvents under the resolved path.
    let work = dir.path().canonicalize().unwrap().join("wc");
    std::fs::create_dir(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    std::fs::write(work.join("a.txt"), "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "first"]);

    let socket = dir.path().join("d.sock");
    let daemon = neutrinod::Daemon::new(&dir.path().join("state")).unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn({
        let socket = socket.clone();
        async move { neutrinod::serve(daemon, &socket, async { stopped.await.ok(); }).await }
    });
    while !socket.exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (read, write) = UnixStream::connect(&socket).await.unwrap().into_split();
    let mut c = Client { lines: BufReader::new(read).lines(), write };
    assert!(matches!(c.call(1, Request::AddRepo { path: work.display().to_string() }).await, Response::Repo(_)));

    // A second connection subscribes; editing a file reaches it.
    let (read2, write2) = UnixStream::connect(&socket).await.unwrap().into_split();
    let mut sub = Client { lines: BufReader::new(read2).lines(), write: write2 };
    sub.send(50, &Request::Subscribe).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(work.join("a.txt"), "1\nTWO\n3\n4\n5\n6\n7\n8\n9\nTEN\n").unwrap();
    let (id, ev) = sub.recv().await;
    assert_eq!(id, 50);
    assert!(matches!(&ev, Response::Event(e) if e.repo == "wc"), "{ev:?}");

    match c.call(2, Request::Status { repo: "wc".into() }).await {
        Response::Status(st) => assert_eq!(st.unstaged.len(), 1),
        other => panic!("{other:?}"),
    }
    let pick = vec![LinePick { kind: LineKind::Removed, line: 2 }, LinePick { kind: LineKind::Added, line: 2 }];
    assert!(matches!(c.call(3, Request::StageLines { repo: "wc".into(), path: "a.txt".into(), area: Area::Unstaged, lines: pick }).await, Response::Done { .. }));
    match c.call(4, Request::CommitChanges { repo: "wc".into(), message: "Two".into(), amend: false }).await {
        Response::Committed { sha, .. } => assert_eq!(sha.len(), 40),
        other => panic!("{other:?}"),
    }
    match c.call(5, Request::Status { repo: "wc".into() }).await {
        Response::Status(st) => {
            assert!(st.staged.is_empty());
            let a = &st.unstaged[0];
            assert_eq!((a.additions, a.deletions), (1, 1), "only line 10's change is left");
        }
        other => panic!("{other:?}"),
    }
    // The commit moved refs, which subscribers hear as well.
    let mut kinds = std::collections::HashSet::new();
    while let Ok(Some(line)) = tokio::time::timeout(Duration::from_millis(800), sub.lines.next_line()).await.map(|r| r.unwrap()) {
        if let Ok((_, Response::Event(e))) = decode_response(&line) {
            kinds.insert(e.kind);
        }
    }
    assert!(kinds.contains("refs"), "{kinds:?}");

    // Cancelling the subscription ends it.
    sub.send(51, &Request::Cancel { id: 50 }).await;
    assert!(matches!(sub.recv().await, (50, Response::Cancelled)));

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}
