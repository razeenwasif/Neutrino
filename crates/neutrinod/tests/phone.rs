//! The phone over TLS: its signing key, signed approvals (and what an
//! unsigned, replayed, stale or forged one gets), push settings and the Inbox.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use neutrino_proto::{Request, Response, decode_response, encode_request};
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
    #[allow(dead_code)]
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


const DEPLOY: &str = r#"
on: workflow_dispatch
jobs:
  ship:
    environment: prod
    image: alpine:3.20
    steps: [{ run: echo shipped }]
"#;

fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A request signed the way the phone signs it.
fn signed(key: &ring::signature::EcdsaKeyPair, inner: &Request, at: i64, nonce: &str) -> Request {
    let line = encode_request(0, inner);
    let rng = ring::rand::SystemRandom::new();
    let sig = key.sign(&rng, format!("{at}\n{nonce}\n{line}").as_bytes()).unwrap();
    Request::Signed { request: line, at, nonce: nonce.into(), signature: b64(sig.as_ref()) }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

async fn waiting_job(sock: &Path) -> i64 {
    let run = match call(sock, Request::DispatchRun { repo: "app".into(), workflow: "deploy".into(), branch: "main".into() }).await {
        Response::Run(d) => d.run.id,
        other => panic!("{other:?}"),
    };
    for _ in 0..100 {
        if let Response::Run(d) = call(sock, Request::Run { repo: "app".into(), id: run }).await {
            if d.jobs[0].state == neutrino_proto::RunState::Waiting {
                return d.jobs[0].id;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the deploy never waited");
}

#[tokio::test]
async fn the_phone_signs_its_approvals() {
    // SAFETY: set before any hook runs, and only read by the daemon this test starts.
    unsafe { std::env::set_var("NEUTRINO_HOOK_EXE", env!("CARGO_BIN_EXE_neutrinod")) };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let sock = root.join("s.sock");
    let options = neutrinod::Options {
        role: neutrinod::Role::Server,
        remote_port: Some(0),
        ci: local_ci(),
        vault: neutrino_vault::KeyStore::File(root.join("vault.key")),
        ..Default::default()
    };
    let (server, _stop) = start(root.join("server"), sock.clone(), options).await;
    let port = loop {
        if let Some(p) = server.remote_port() {
            break p;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    // A hosted repo with a deploy that needs approval.
    let hosted = match call(&sock, Request::CreateHosted { name: "app".into(), description: None }).await {
        Response::Repo(r) => r,
        other => panic!("{other:?}"),
    };
    let env = neutrino_proto::Environment { repo: "app".into(), name: "prod".into(), require_approval: true, branches: vec![] };
    assert!(matches!(call(&sock, Request::SetEnvironment { environment: env }).await, Response::Environments(_)));
    let wc = root.join("wc");
    std::fs::create_dir_all(wc.join(".neutrino/workflows")).unwrap();
    git(&wc, &["init", "-q", "-b", "main"]);
    std::fs::write(wc.join(".neutrino/workflows/deploy.yml"), DEPLOY).unwrap();
    git(&wc, &["add", "."]);
    git(&wc, &["commit", "-q", "-m", "first"]);
    git(&wc, &["remote", "add", "origin", &hosted.path]);
    let wc2 = wc.clone();
    tokio::task::spawn_blocking(move || git(&wc2, &["push", "-q", "origin", "main"])).await.unwrap();

    // Pair two devices: a phone with a key, and a Mac without one.
    let pair = |name: &'static str| {
        let sock = sock.clone();
        async move {
            let Response::Pairing(info) = call(&sock, Request::PairBegin).await else { panic!("no code") };
            let mut d = Device::connect(port).await;
            let token = match d.call(0, Request::Pair { code: info.code, device: name.into() }).await {
                Some(Response::Paired { token, .. }) => token,
                other => panic!("{other:?}"),
            };
            let mut d = Device::connect(port).await;
            assert!(matches!(d.call(0, Request::Auth { token }).await, Some(Response::Authenticated { .. })));
            d
        }
    };
    let mut phone = pair("iPhone").await;
    let mut mac = pair("MacBook").await;

    let rng = ring::rand::SystemRandom::new();
    let alg = &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING;
    let key = ring::signature::EcdsaKeyPair::from_pkcs8(alg, ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap().as_ref(), &rng).unwrap();
    let thief = ring::signature::EcdsaKeyPair::from_pkcs8(alg, ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap().as_ref(), &rng).unwrap();
    use ring::signature::KeyPair as _;
    let public = b64(key.public_key().as_ref());
    assert!(matches!(phone.call(1, Request::RegisterKey { public_key: public.clone() }).await, Some(Response::Done { .. })));
    assert!(
        matches!(phone.call(2, Request::RegisterKey { public_key: b64(thief.public_key().as_ref()) }).await, Some(Response::Error { message }) if message.contains("already has a signing key")),
        "a stolen token can't swap the key"
    );
    assert!(matches!(call(&sock, Request::RegisterKey { public_key: public }).await, Response::Error { .. }), "not over the local socket");

    // Push settings, then the device list says so.
    let push = Request::RegisterPush { token: Some("ab12".repeat(16)), sandbox: true, events: vec!["approvals".into(), "nonsense".into()] };
    assert!(matches!(phone.call(3, push).await, Some(Response::Done { .. })));
    match call(&sock, Request::ListDevices).await {
        Response::Devices(list) => {
            let p = list.iter().find(|d| d.name == "iPhone").unwrap();
            let m = list.iter().find(|d| d.name == "MacBook").unwrap();
            assert!(p.signs && p.push && !m.signs && !m.push);
        }
        other => panic!("{other:?}"),
    }

    // A deploy waits; the Inbox and the activity say so.
    let job = waiting_job(&sock).await;
    match phone.call(4, Request::Inbox).await {
        Some(Response::Inbox(i)) => {
            assert_eq!(i.waiting.len(), 1);
            assert_eq!((i.waiting[0].job, i.waiting[0].environment.as_deref()), (job, Some("prod")));
            assert!(i.runs.iter().any(|r| r.state == neutrino_proto::RunState::Waiting));
            let e = i.events.first().expect("an approval event");
            assert_eq!((e.kind.as_str(), e.title.as_str()), ("approval", "prod needs your approval"));
            let Some(Response::Notification(n)) = phone.call(5, Request::Notification { id: e.id.clone() }).await else { panic!("no notification") };
            assert!(n.body.contains("deploy ship"), "{}", n.body);
        }
        other => panic!("{other:?}"),
    }

    // Unsigned from the phone: refused. Signed by another key, stale, or replayed: refused.
    let approve = Request::ApproveJob { repo: "app".into(), job, approve: true };
    assert!(matches!(phone.call(6, approve.clone()).await, Some(Response::Error { message }) if message.contains("wasn't signed")));
    let forged = signed(&thief, &approve, now(), "nonce-forged-000001");
    assert!(matches!(phone.call(7, forged).await, Some(Response::Error { message }) if message.contains("doesn't match")));
    let stale = signed(&key, &approve, now() - 3600, "nonce-stale-0000001");
    assert!(matches!(phone.call(8, stale).await, Some(Response::Error { message }) if message.contains("stale")));
    let good = signed(&key, &approve, now(), "nonce-good-00000001");
    match phone.call(9, good.clone()).await {
        Some(Response::Run(d)) => assert_eq!(d.jobs[0].approved_by.as_deref(), Some("iPhone")),
        other => panic!("{other:?}"),
    }
    assert!(matches!(phone.call(10, good).await, Some(Response::Error { message }) if message.contains("already used")), "a signature works once");
    // Tampering with what was signed breaks it.
    let mut tampered = signed(&key, &approve, now(), "nonce-tamper-000001");
    if let Request::Signed { request, .. } = &mut tampered {
        *request = request.replace("\"approve\":true", "\"approve\":false");
    }
    assert!(matches!(phone.call(11, tampered).await, Some(Response::Error { .. })));

    // The Mac has no key: its token alone still approves, as before.
    let job2 = waiting_job(&sock).await;
    match mac.call(12, Request::ApproveJob { repo: "app".into(), job: job2, approve: false }).await {
        Some(Response::Run(d)) => assert_eq!(d.jobs[0].approved_by.as_deref(), Some("MacBook")),
        other => panic!("{other:?}"),
    }
    // No key: setup refuses junk, and a test push has nowhere to go.
    assert!(matches!(call(&sock, Request::PushInfo).await, Response::Push(p) if !p.configured && p.devices == 1));
    assert!(matches!(call(&sock, Request::PushSetup { key_p8: "junk".into(), key_id: "ABCDE12345".into(), team_id: "TEAM123456".into(), topic: "dev.neutrino.ios".into() }).await, Response::Error { .. }));
}

fn local_ci() -> neutrinod::ci::CiOptions {
    neutrinod::ci::CiOptions { runner: neutrinod::ci::Runner::Local(Default::default()), ..Default::default() }
}
