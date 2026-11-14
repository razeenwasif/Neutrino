//! A server and a workstation daemon side by side: publishing a working copy,
//! pushing through the hosted repo's hooks, and a device pairing over TLS.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use neutrino_proto::{LogQuery, Request, Response, decode_response, encode_request};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// One request over a fresh socket connection.
async fn call(socket: &Path, request: Request) -> Response {
    let stream = UnixStream::connect(socket).await.unwrap();
    let (read, mut write) = stream.into_split();
    write.write_all(format!("{}\n", encode_request(1, &request)).as_bytes()).await.unwrap();
    let mut lines = BufReader::new(read).lines();
    let line = tokio::time::timeout(Duration::from_secs(30), lines.next_line()).await.unwrap().unwrap().unwrap();
    decode_response(&line).unwrap().1
}

async fn start(state: PathBuf, socket: PathBuf, options: neutrinod::Options) -> (Arc<neutrinod::Daemon>, tokio::sync::oneshot::Sender<()>) {
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

/// A TLS client that pins nothing and records the certificate's fingerprint,
/// as a device does on its first pairing.
#[derive(Debug)]
struct Record(std::sync::Mutex<Option<String>>);

impl rustls::client::danger::ServerCertVerifier for Record {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let fp: String = ring::digest::digest(&ring::digest::SHA256, end_entity.as_ref()).as_ref().iter().map(|b| format!("{b:02x}")).collect();
        *self.0.lock().unwrap() = Some(fp);
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &rustls::crypto::ring::default_provider().signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &rustls::crypto::ring::default_provider().signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
    }
}

struct Device {
    lines: tokio::io::Lines<BufReader<tokio::io::ReadHalf<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>>>,
    write: tokio::io::WriteHalf<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
    fingerprint: String,
}

impl Device {
    async fn connect(port: u16) -> Device {
        let record = Arc::new(Record(Default::default()));
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(record.clone())
            .with_no_client_auth();
        let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect("neutrino.local".try_into().unwrap(), tcp).await.unwrap();
        let fingerprint = record.0.lock().unwrap().clone().unwrap();
        let (read, write) = tokio::io::split(tls);
        Device { lines: BufReader::new(read).lines(), write, fingerprint }
    }

    async fn call(&mut self, id: u64, request: Request) -> Option<Response> {
        self.write.write_all(format!("{}\n", encode_request(id, &request)).as_bytes()).await.ok()?;
        let line = tokio::time::timeout(Duration::from_secs(10), self.lines.next_line()).await.ok()?.ok()??;
        Some(decode_response(&line).unwrap().1)
    }
}

#[tokio::test]
async fn publish_push_and_pair() {
    // SAFETY: set before any hook runs, and only read by the daemons this test starts.
    unsafe { std::env::set_var("NEUTRINO_HOOK_EXE", env!("CARGO_BIN_EXE_neutrinod")) };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (server_sock, work_sock) = (root.join("s.sock"), root.join("w.sock"));
    let (server, stop_server) = start(
        root.join("server"),
        server_sock.clone(),
        neutrinod::Options { role: neutrinod::Role::Server, remote_port: Some(0), advertise: false, ci: local_ci(), vault: neutrino_vault_test_key() },
    )
    .await;
    let (_work, stop_work) = start(root.join("work-state"), work_sock.clone(), neutrinod::Options::default()).await;

    // A working copy with one commit.
    let wc = root.join("app");
    std::fs::create_dir(&wc).unwrap();
    git(&wc, &["init", "-q", "-b", "main"]);
    std::fs::write(wc.join("README.md"), "hello\n").unwrap();
    git(&wc, &["add", "."]);
    git(&wc, &["commit", "-q", "-m", "first"]);
    assert!(matches!(call(&work_sock, Request::AddRepo { path: wc.display().to_string() }).await, Response::Repo(_)));

    // The workstation refuses server requests.
    assert!(matches!(call(&work_sock, Request::CreateHosted { name: "x".into(), description: None }).await, Response::Error { message } if message.contains("--server")));

    // Create the hosted repo on the server; bad names are refused.
    assert!(matches!(call(&server_sock, Request::CreateHosted { name: "../evil".into(), description: None }).await, Response::Error { .. }));
    let hosted = match call(&server_sock, Request::CreateHosted { name: "app".into(), description: Some("The app".into()) }).await {
        Response::Repo(r) => r,
        other => panic!("{other:?}"),
    };
    assert!(hosted.bare && hosted.path.ends_with("repos/app.git"), "{hosted:?}");
    assert!(Path::new(&hosted.path).join("hooks/post-receive").exists());

    // Follow the server's events.
    let sub = UnixStream::connect(&server_sock).await.unwrap();
    let (sub_read, mut sub_write) = sub.into_split();
    sub_write.write_all(format!("{}\n", encode_request(9, &Request::Subscribe)).as_bytes()).await.unwrap();
    let mut sub_lines = BufReader::new(sub_read).lines();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Point the working copy at it and push; the hooks report back.
    assert!(matches!(call(&work_sock, Request::AddRemote { repo: "app".into(), name: "atlas".into(), url: hosted.path.clone() }).await, Response::Done { .. }));
    match call(&work_sock, Request::Remotes { repo: "app".into() }).await {
        Response::Remotes(r) => assert_eq!((r[0].name.as_str(), r[0].url.as_str()), ("atlas", hosted.path.as_str())),
        other => panic!("{other:?}"),
    }
    let pushed = match call(&work_sock, Request::Push { repo: "app".into(), remote: Some("atlas".into()), branch: None, set_upstream: true }).await {
        Response::Done { output } => output,
        other => panic!("{other:?}"),
    };
    assert!(pushed.contains("neutrino: 1 ref updated"), "the post-receive hook ran: {pushed}");
    let mut kinds = Vec::new();
    while let Ok(Ok(Some(line))) = tokio::time::timeout(Duration::from_millis(1500), sub_lines.next_line()).await {
        if let Ok((_, Response::Event(e))) = decode_response(&line) {
            kinds.push(e.kind);
        }
    }
    assert!(kinds.iter().any(|k| k == "push"), "{kinds:?}");

    // The server reads the pushed history; the working copy now tracks it.
    match call(&server_sock, Request::Log(LogQuery { repo: "app".into(), ..Default::default() })).await {
        Response::Log(p) => assert_eq!(p.commits[0].subject, "first"),
        other => panic!("{other:?}"),
    }
    assert_eq!(git(&wc, &["rev-parse", "--abbrev-ref", "main@{upstream}"]), "atlas/main");
    assert!(matches!(call(&work_sock, Request::Fetch { repo: "app".into(), remote: None }).await, Response::Done { .. }));

    // A device pairs over TLS with a code from the server's own socket.
    let port = server.remote_port().unwrap();
    let Response::Pairing(info) = call(&server_sock, Request::PairBegin).await else { panic!("no pairing code") };
    let mut phone = Device::connect(port).await;
    assert_eq!(phone.fingerprint, info.fingerprint, "the device sees the certificate the server shows");
    let token = match phone.call(0, Request::Pair { code: info.code.clone(), device: "MacBook".into() }).await {
        Some(Response::Paired { token, .. }) => token,
        other => panic!("{other:?}"),
    };

    // With the token: hosted repos can be read, the server's own paths can't be touched.
    let mut dev = Device::connect(port).await;
    assert!(matches!(dev.call(0, Request::Auth { token: token.clone() }).await, Some(Response::Authenticated { role, .. }) if role == "server"));
    assert!(matches!(dev.call(1, Request::ListRepos).await, Some(Response::Repos(r)) if r.iter().any(|r| r.id == "app")));
    assert!(matches!(dev.call(2, Request::Branches { repo: "app".into() }).await, Some(Response::Branches(b)) if b[0].name == "main"));
    for refused in [Request::AddRepo { path: "/etc".into() }, Request::PairBegin, Request::Status { repo: "app".into() }] {
        assert!(matches!(dev.call(3, refused).await, Some(Response::Error { message }) if message.contains("server itself")));
    }

    // Without one, nothing.
    let mut stranger = Device::connect(port).await;
    assert!(matches!(stranger.call(0, Request::ListRepos).await, Some(Response::Error { .. })));
    let mut forged = Device::connect(port).await;
    assert!(matches!(forged.call(0, Request::Auth { token: "0".repeat(64) }).await, Some(Response::Error { .. })));

    // Revoking the device stops its token.
    let Response::Devices(devices) = call(&server_sock, Request::ListDevices).await else { panic!() };
    assert_eq!(devices.len(), 1);
    assert!(matches!(call(&server_sock, Request::RevokeDevice { id: devices[0].id.clone() }).await, Response::Revoked { .. }));
    let mut revoked = Device::connect(port).await;
    assert!(matches!(revoked.call(0, Request::Auth { token }).await, Some(Response::Error { .. })));

    stop_server.send(()).unwrap();
    stop_work.send(()).unwrap();
}

#[tokio::test]
async fn a_restarted_server_adopts_its_repos() {
    unsafe { std::env::set_var("NEUTRINO_HOOK_EXE", env!("CARGO_BIN_EXE_neutrinod")) };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let state = root.join("server");
    neutrinod::hosting::create(&state.join("repos"), "kept", None).unwrap();
    let options = neutrinod::Options { role: neutrinod::Role::Server, remote_port: None, advertise: false, ci: local_ci(), vault: neutrino_vault_test_key() };
    let (_s, stop) = start(state.clone(), root.join("s.sock"), options).await;
    match call(&root.join("s.sock"), Request::ListRepos).await {
        Response::Repos(r) => assert!(r.iter().any(|r| r.id == "kept" && r.bare)),
        other => panic!("{other:?}"),
    }
    // Its hooks were rewritten to call this daemon.
    let hook = std::fs::read_to_string(state.join("repos/kept.git/hooks/pre-receive")).unwrap();
    assert!(hook.contains("hook pre-receive") && hook.contains("s.sock"), "{hook}");
    assert!(matches!(call(&root.join("s.sock"), Request::PairBegin).await, Response::Error { message } if message.contains("no network endpoint")));
    stop.send(()).unwrap();
}

#[tokio::test]
async fn the_first_pushed_branch_becomes_the_default() {
    unsafe { std::env::set_var("NEUTRINO_HOOK_EXE", env!("CARGO_BIN_EXE_neutrinod")) };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let options = neutrinod::Options { role: neutrinod::Role::Server, remote_port: None, advertise: false, ci: local_ci(), vault: neutrino_vault_test_key() };
    let (_s, stop) = start(root.join("server"), root.join("s.sock"), options).await;
    let Response::Repo(hosted) = call(&root.join("s.sock"), Request::CreateHosted { name: "old".into(), description: None }).await else { panic!() };

    let wc = root.join("wc");
    std::fs::create_dir(&wc).unwrap();
    git(&wc, &["init", "-q", "-b", "master"]);
    git(&wc, &["commit", "-q", "--allow-empty", "-m", "on master"]);
    // Off the runtime thread: the daemon answering the hook runs on it.
    let (path, dir) = (hosted.path.clone(), wc.clone());
    let out = tokio::task::spawn_blocking(move || Command::new("git").args(["push", "-q", &path, "master"]).current_dir(&dir).output().unwrap())
        .await
        .unwrap();
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("master is now this repository's default branch"), "{said}");
    match call(&root.join("s.sock"), Request::Log(LogQuery { repo: "old".into(), ..Default::default() })).await {
        Response::Log(p) => assert_eq!(p.commits[0].subject, "on master"),
        other => panic!("{other:?}"),
    }
    stop.send(()).unwrap();
}

/// Tests never touch the login Keychain.
fn neutrino_vault_test_key() -> neutrino_vault::KeyStore {
    neutrino_vault::KeyStore::File(std::env::temp_dir().join(format!("ntr-test-vault-{}-{:?}.key", std::process::id(), std::thread::current().id())))
}

/// Tests never touch the real Graviton: steps run as local processes.
fn local_ci() -> neutrinod::ci::CiOptions {
    neutrinod::ci::CiOptions { runner: neutrinod::ci::Runner::Local(Default::default()), ..Default::default() }
}
