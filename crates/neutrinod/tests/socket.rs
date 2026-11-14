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
    assert_eq!(c.call(23, Request::Ping).await, Response::Pong { protocol: 1 });

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
