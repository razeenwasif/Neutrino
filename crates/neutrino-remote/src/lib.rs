//! The server's paired TLS endpoint: how the Mac app (and the iPhone app)
//! reaches neutrinod on the server.
//!
//! It listens on TCP [`neutrino_proto::REMOTE_PORT`] with TLS, using a
//! self-signed certificate made once and kept in `<state>/remote/`. Devices
//! pin its SHA-256 fingerprint. A connection speaks the same JSON lines as the
//! Unix socket, but its first message must be one of:
//!
//! * `pair` with a one-time code from `ntr pair` (valid five minutes); the
//!   answer carries a long-lived token for that device.
//! * `auth` with such a token.
//!
//! Tokens are stored only as SHA-256 hashes in `devices.json`; `ntr devices
//! revoke` forgets one. After [`MAX_FAILURES`] wrong codes every pending code
//! is cancelled. The service is advertised over Bonjour as `_neutrino._tcp`.
//!
//! Ported from Graviton's remote endpoint.

use std::collections::HashMap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use neutrino_proto::{PairedDevice, PairingInfo, Request, Response, decode_request, encode_response};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

const CODE_TTL: Duration = Duration::from_secs(300);
pub const MAX_FAILURES: u32 = 5;
const AUTH_WINDOW: Duration = Duration::from_secs(300);
const AUTH_MAX_FAILURES: u32 = 5;
const AUTH_LOCKOUT: Duration = Duration::from_secs(60);
const MAX_AUTH_LIMIT_ENTRIES: usize = 4096;
/// Maximum pre-authentication request size (16 KiB).
pub const MAX_PREAUTH_LINE: usize = 16 * 1024;
/// No 0/O, 1/I/L: easy to read off a screen and type.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

#[derive(Clone, Serialize, Deserialize)]
struct Device {
    id: String,
    name: String,
    token_sha256: String,
    paired_at: u64,
    last_seen: Option<u64>,
    /// A P-256 public key (X9.63, base64) from the device's Secure Enclave.
    /// Once set, approvals from this device must be signed with it.
    #[serde(default)]
    signing_key: Option<String>,
    /// Where Apple's push service reaches this device, and what for.
    #[serde(default)]
    push: Option<PushTarget>,
}

/// A device's APNs token and the events it wants to hear about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushTarget {
    pub token: String,
    /// The development push service (an app built from Xcode) rather than production.
    pub sandbox: bool,
    /// `approvals`, `failures`.
    pub events: Vec<String>,
}

/// How long a signed request stays good, either side of the server's clock.
pub const SIGNATURE_WINDOW_SECS: i64 = 300;

struct PendingCode {
    code: String,
    expires: Instant,
}

struct AuthFailure {
    count: u32,
    window_start: Instant,
    locked_until: Option<Instant>,
}

#[derive(Default)]
struct State {
    devices: Vec<Device>,
    codes: Vec<PendingCode>,
    failures: u32,
    auth_limits: HashMap<IpAddr, AuthFailure>,
}

/// An authenticated TLS connection, ready for ordinary requests.
pub type Session = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;

pub struct Remote {
    state: Mutex<State>,
    devices_path: PathBuf,
    acceptor: tokio_rustls::TlsAcceptor,
    pub fingerprint: String,
    pub bind: IpAddr,
    pub port: u16,
    pub host: String,
    rng: SystemRandom,
    _bonjour: Option<Bonjour>,
}

impl Remote {
    /// Load (or make) the certificate and the paired devices from `<state_dir>/remote/`.
    /// `advertise` registers the service over Bonjour.
    pub fn open(state_dir: &Path, port: u16, advertise: bool) -> Result<Arc<Self>> {
        Self::open_with(state_dir, IpAddr::V4(Ipv4Addr::UNSPECIFIED), port, advertise, None)
    }

    pub fn open_with(state_dir: &Path, bind: IpAddr, port: u16, advertise: bool, host_override: Option<String>) -> Result<Arc<Self>> {
        ensure_private_dir(state_dir)?;
        let dir = state_dir.join("remote");
        ensure_private_dir(&dir)?;
        let (cert, key) = identity(&dir)?;
        let fingerprint = hex(ring::digest::digest(&ring::digest::SHA256, &cert).as_ref());

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(cert)],
                rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(key)),
            )?;

        let devices_path = dir.join("devices.json");
        let devices = match fs::read(&devices_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("reading devices.json")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let host = host_override
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(computer_name);
        Ok(Arc::new(Self {
            state: Mutex::new(State { devices, ..Default::default() }),
            devices_path,
            acceptor: tokio_rustls::TlsAcceptor::from(Arc::new(config)),
            fingerprint,
            bind,
            port,
            _bonjour: if advertise { Bonjour::register(&host, port) } else { None },
            host,
            rng: SystemRandom::new(),
        }))
    }

    /// `ntr pair`: a fresh one-time code.
    pub fn begin_pairing(&self) -> Result<PairingInfo> {
        let mut bytes = [0u8; 8];
        self.rng.fill(&mut bytes).map_err(|_| anyhow!("no randomness"))?;
        let code: String = bytes.iter().map(|b| CODE_ALPHABET[*b as usize % CODE_ALPHABET.len()] as char).collect();
        let mut state = self.state.lock().unwrap();
        state.codes.retain(|c| c.expires > Instant::now());
        state.codes.push(PendingCode { code: code.clone(), expires: Instant::now() + CODE_TTL });
        state.failures = 0;
        Ok(PairingInfo {
            code,
            expires_in_secs: CODE_TTL.as_secs(),
            host: self.host.clone(),
            port: self.port,
            fingerprint: self.fingerprint.clone(),
            addresses: lan_addresses(),
        })
    }

    pub fn devices(&self) -> Vec<PairedDevice> {
        self.state
            .lock()
            .unwrap()
            .devices
            .iter()
            .map(|d| PairedDevice {
                id: d.id.clone(),
                name: d.name.clone(),
                paired_at: d.paired_at,
                last_seen: d.last_seen,
                signs: d.signing_key.is_some(),
                push: d.push.is_some(),
            })
            .collect()
    }

    /// Give a device its signing key. Only once: a stolen token mustn't be
    /// able to swap in its own key.
    pub fn set_signing_key(&self, device: &str, key_b64: &str) -> Result<()> {
        use base64::Engine as _;
        let key = base64::engine::general_purpose::STANDARD.decode(key_b64.trim()).context("the key isn't base64")?;
        if key.len() != 65 || key[0] != 4 {
            bail!("the key should be an uncompressed P-256 point (65 bytes)");
        }
        let mut state = self.state.lock().unwrap();
        let d = state.devices.iter_mut().find(|d| d.id == device).context("this device isn't paired any more")?;
        match &d.signing_key {
            Some(existing) if existing == key_b64.trim() => return Ok(()),
            Some(_) => bail!("this device already has a signing key; pair it again to change it"),
            None => d.signing_key = Some(key_b64.trim().to_string()),
        }
        self.save(&state.devices);
        Ok(())
    }

    /// Whether this device signs its approvals.
    pub fn signs(&self, device: &str) -> bool {
        self.state.lock().unwrap().devices.iter().any(|d| d.id == device && d.signing_key.is_some())
    }

    /// Check a signature over `message` (ECDSA P-256 with SHA-256, DER) made
    /// with the device's key.
    pub fn verify(&self, device: &str, message: &[u8], signature_b64: &str) -> Result<()> {
        use base64::Engine as _;
        let key = self
            .state
            .lock()
            .unwrap()
            .devices
            .iter()
            .find(|d| d.id == device)
            .and_then(|d| d.signing_key.clone())
            .context("this device has no signing key")?;
        let key = base64::engine::general_purpose::STANDARD.decode(key)?;
        let sig = base64::engine::general_purpose::STANDARD.decode(signature_b64.trim()).context("the signature isn't base64")?;
        ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, key)
            .verify(message, &sig)
            .map_err(|_| anyhow::anyhow!("the signature doesn't match this device's key"))
    }

    pub fn set_push(&self, device: &str, push: Option<PushTarget>) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let d = state.devices.iter_mut().find(|d| d.id == device).context("this device isn't paired any more")?;
        d.push = push;
        self.save(&state.devices);
        Ok(())
    }

    /// Devices that want to hear about `event`.
    pub fn push_targets(&self, event: &str) -> Vec<PushTarget> {
        self.state.lock().unwrap().devices.iter().filter_map(|d| d.push.clone()).filter(|p| p.events.iter().any(|e| e == event)).collect()
    }

    pub fn revoke(&self, id: &str) -> Result<String> {
        let mut state = self.state.lock().unwrap();
        let before = state.devices.len();
        state.devices.retain(|d| d.id != id && !d.id.starts_with(id));
        if state.devices.len() == before {
            bail!("no paired device {id}");
        }
        self.save(&state.devices);
        Ok(id.to_string())
    }

    fn pair(&self, code: &str, device: &str) -> Result<Response> {
        let mut state = self.state.lock().unwrap();
        state.codes.retain(|c| c.expires > Instant::now());
        let typed = normalize_code(code);
        let Some(pos) = state.codes.iter().position(|c| constant_time_eq(c.code.as_bytes(), typed.as_bytes())) else {
            state.failures += 1;
            if state.failures >= MAX_FAILURES {
                state.codes.clear();
                bail!("too many wrong codes; run `ntr pair` on the server again");
            }
            bail!("that pairing code isn't valid (codes last five minutes and work once)");
        };
        state.codes.remove(pos);

        let mut raw = [0u8; 32];
        self.rng.fill(&mut raw).map_err(|_| anyhow!("no randomness"))?;
        let token = hex(&raw);
        let mut id_bytes = [0u8; 6];
        self.rng.fill(&mut id_bytes).map_err(|_| anyhow!("no randomness"))?;
        let name: String = device.chars().filter(|c| !c.is_control()).take(64).collect();
        let entry = Device {
            id: hex(&id_bytes),
            name: if name.trim().is_empty() { "Unnamed device".into() } else { name },
            token_sha256: sha256_hex(token.as_bytes()),
            paired_at: now_secs(),
            last_seen: Some(now_secs()),
            signing_key: None,
            push: None,
        };
        let device_id = entry.id.clone();
        state.devices.push(entry);
        self.save(&state.devices);
        eprintln!("paired {device_id}");
        Ok(Response::Paired { token, device_id, host: self.host.clone() })
    }

    /// The id and name of the device this token belongs to, if any.
    #[cfg(test)]
    fn authenticate(&self, token: &str) -> Option<(String, String)> {
        self.authenticate_from(None, token).ok().flatten()
    }

    fn authenticate_from(&self, peer_ip: Option<IpAddr>, token: &str) -> Result<Option<(String, String)>> {
        let now = Instant::now();
        let hash = sha256_hex(token.as_bytes());
        let mut state = self.state.lock().unwrap();
        if let Some(ip) = peer_ip
            && let Some(entry) = state.auth_limits.get(&ip)
            && entry.locked_until.is_some_and(|until| now < until)
        {
            bail!("too many failed authentication attempts; try again shortly");
        }
        let mut found = None;
        for d in state.devices.iter_mut() {
            if constant_time_eq(d.token_sha256.as_bytes(), hash.as_bytes()) {
                d.last_seen = Some(now_secs());
                found = Some((d.id.clone(), d.name.clone()));
            }
        }
        if found.is_some() {
            if let Some(ip) = peer_ip {
                state.auth_limits.remove(&ip);
            }
            self.save(&state.devices);
        } else if let Some(ip) = peer_ip {
            if state.auth_limits.len() >= MAX_AUTH_LIMIT_ENTRIES {
                state.auth_limits.retain(|_, v| {
                    v.locked_until.is_some_and(|u| u > now)
                        || now.saturating_duration_since(v.window_start) < AUTH_WINDOW
                });
            }
            let entry = state.auth_limits.entry(ip).or_insert(AuthFailure {
                count: 0,
                window_start: now,
                locked_until: None,
            });
            if now.saturating_duration_since(entry.window_start) >= AUTH_WINDOW {
                entry.count = 0;
                entry.window_start = now;
                entry.locked_until = None;
            }
            entry.count += 1;
            if entry.count >= AUTH_MAX_FAILURES {
                entry.locked_until = Some(now + AUTH_LOCKOUT);
                entry.count = 0;
            }
        }
        Ok(found)
    }

    /// Callers hold the state lock while saving, so saves can't interleave
    /// (they share the temporary file) or land out of order.
    fn save(&self, devices: &[Device]) {
        let tmp = self.devices_path.with_extension("json.tmp");
        let ok = write_private(&tmp, &serde_json::to_vec_pretty(devices).expect("devices serialise"))
            .and_then(|_| fs::rename(&tmp, &self.devices_path));
        if let Err(err) = ok {
            eprintln!("couldn't save paired devices: {err}");
        }
    }

    pub async fn bind(&self) -> Result<tokio::net::TcpListener> {
        tokio::net::TcpListener::bind((self.bind, self.port)).await.with_context(|| format!("listening on {}:{}", self.bind, self.port))
    }

    /// TLS handshake, then the first message: pair or authenticate. Returns the
    /// session once it's authenticated, or `None` when the connection is done
    /// (pairing answered, or refused).
    /// Hand back the session and the paired device's id and name once it has authenticated.
    pub async fn accept(&self, tcp: tokio::net::TcpStream, role: &str) -> Result<Option<(Session, String, String)>> {
        let peer_ip = tcp.peer_addr().ok().map(|a| a.ip());
        let mut tls = tokio::time::timeout(Duration::from_secs(10), self.acceptor.accept(tcp)).await.context("TLS handshake timed out")??;
        let first = {
            let mut reader = BufReader::new(&mut tls);
            let line = match tokio::time::timeout(Duration::from_secs(15), read_bounded_line(&mut reader, MAX_PREAUTH_LINE))
                .await
                .context("no hello")?
            {
                Ok(Some(line)) => line,
                Ok(None) => return Ok(None),
                Err(e) => bail!("{e}"),
            };
            // A pairing or auth message fits in one read; nothing else is sent before the answer.
            if !reader.buffer().is_empty() {
                bail!("sent more than one message before authenticating");
            }
            line
        };
        let (id, request) = decode_request(first.trim());
        let id = id.unwrap_or(0);
        let (reply, device) = match request {
            Ok(Request::Pair { code, device }) => (self.pair(&code, &device).unwrap_or_else(|e| Response::Error { message: e.to_string() }), None),
            Ok(Request::Auth { token }) => match self.authenticate_from(peer_ip, &token) {
                Ok(Some(who)) => (Response::Authenticated { host: self.host.clone(), role: role.to_string() }, Some(who)),
                Ok(None) => (Response::Error { message: "this device isn't paired with this server any more".into() }, None),
                Err(e) => (Response::Error { message: e.to_string() }, None),
            },
            _ => (Response::Error { message: "pair or authenticate first".into() }, None),
        };
        tls.write_all(format!("{}\n", encode_response(id, &reply)).as_bytes()).await?;
        Ok(device.map(|(id, name)| (tls, id, name)))
    }
}

async fn read_bounded_line<R: AsyncBufReadExt + Unpin>(reader: &mut R, max_bytes: usize) -> Result<Option<String>> {
    let mut line = String::new();
    let n = (&mut *reader).take(max_bytes as u64 + 1).read_line(&mut line).await?;
    if n == 0 {
        return Ok(None);
    }
    if line.len() > max_bytes || !line.ends_with('\n') {
        bail!("message exceeds {max_bytes} byte limit");
    }
    Ok(Some(line))
}

/// The endpoint's certificate and PKCS#8 key (DER), made on first use.
/// Uses only a generic DNS SAN so the host's real computer name is never leaked in the TLS cert.
fn identity(dir: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    ensure_private_dir(dir)?;
    let (cert_path, key_path) = (dir.join("cert.der"), dir.join("key.der"));
    if let (Ok(cert), Ok(key)) = (fs::read(&cert_path), fs::read(&key_path)) {
        return Ok((cert, key));
    }
    let names = vec!["neutrino.local".to_string()];
    let certified = rcgen::generate_simple_self_signed(names).context("making the remote certificate")?;
    let cert = certified.cert.der().to_vec();
    let key = certified.signing_key.serialize_der();
    write_private(&key_path, &key)?;
    write_private(&cert_path, &cert)?;
    Ok((cert, key))
}

fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(path)
    }
}

fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        f.write_all(data)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).open(path)?;
        f.write_all(data)
    }
}

pub fn computer_name() -> String {
    std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Mac".into())
}

/// IPv4 addresses on this Mac's real interfaces (not loopback, VM bridges or tunnels).
fn lan_addresses() -> Vec<String> {
    let mut out = Vec::new();
    // SAFETY: getifaddrs allocates a list we walk and then free.
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) != 0 {
            return out;
        }
        let mut cur = list;
        while !cur.is_null() {
            let ifa = &*cur;
            let name = std::ffi::CStr::from_ptr(ifa.ifa_name).to_string_lossy();
            if !ifa.ifa_addr.is_null()
                && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET
                && !name.starts_with("lo")
                && !name.starts_with("bridge")
                && !name.starts_with("utun")
            {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let ip = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                out.push(ip.to_string());
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(list);
    }
    out
}

/// Bonjour registration through the system's DNS-SD API, kept for the daemon's life.
struct Bonjour(*mut std::ffi::c_void);
// SAFETY: the reference is only deallocated, once, on drop.
unsafe impl Send for Bonjour {}
unsafe impl Sync for Bonjour {}

#[link(name = "System")]
unsafe extern "C" {
    fn DNSServiceRegister(
        sd_ref: *mut *mut std::ffi::c_void,
        flags: u32,
        interface_index: u32,
        name: *const std::ffi::c_char,
        regtype: *const std::ffi::c_char,
        domain: *const std::ffi::c_char,
        host: *const std::ffi::c_char,
        port_network_order: u16,
        txt_len: u16,
        txt_record: *const std::ffi::c_void,
        callback: *const std::ffi::c_void,
        context: *mut std::ffi::c_void,
    ) -> i32;
    fn DNSServiceRefDeallocate(sd_ref: *mut std::ffi::c_void);
}

impl Bonjour {
    fn register(name: &str, port: u16) -> Option<Self> {
        let name = std::ffi::CString::new(name).ok()?;
        let regtype = std::ffi::CString::new(neutrino_proto::BONJOUR_SERVICE).ok()?;
        let mut sd_ref = std::ptr::null_mut();
        // SAFETY: valid C strings; a null callback is allowed (we don't need the result).
        let err = unsafe {
            DNSServiceRegister(&mut sd_ref, 0, 0, name.as_ptr(), regtype.as_ptr(), std::ptr::null(), std::ptr::null(), port.to_be(), 0, std::ptr::null(), std::ptr::null(), std::ptr::null_mut())
        };
        if err != 0 {
            eprintln!("Bonjour registration failed ({err}); devices can still connect by address");
            return None;
        }
        Some(Self(sd_ref))
    }
}

impl Drop for Bonjour {
    fn drop(&mut self) {
        // SAFETY: allocated by DNSServiceRegister and released once.
        unsafe { DNSServiceRefDeallocate(self.0) };
    }
}

/// "3FA2 C91B 00D4 E5F6": the first 16 hex digits, as `ntr pair` prints them.
pub fn short_fingerprint(fp: &str) -> String {
    fp.as_bytes().chunks(4).take(4).map(|c| String::from_utf8_lossy(c).to_uppercase()).collect::<Vec<_>>().join(" ")
}

fn normalize_code(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn sha256_hex(data: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, data).as_ref())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_comparisons() {
        assert_eq!(normalize_code("abcd-ef23"), "ABCDEF23");
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert_eq!(short_fingerprint("3fa2c91b00d4e5f6aa"), "3FA2 C91B 00D4 E5F6");
        assert_eq!(sha256_hex(b"hello").len(), 64);
    }

    #[test]
    fn pairing_and_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let remote = Remote::open(dir.path(), 0, false).unwrap();
        assert_eq!(remote.fingerprint.len(), 64);
        let info = remote.begin_pairing().unwrap();
        assert_eq!(info.code.len(), 8);

        // A wrong code is refused; the right one (any case, with a dash) works once.
        assert!(remote.pair("WRONGCOD", "Mac").is_err());
        let typed = format!("{}-{}", info.code[..4].to_lowercase(), &info.code[4..]);
        let Response::Paired { token, device_id, .. } = remote.pair(&typed, "MacBook").unwrap() else { panic!() };
        assert!(remote.pair(&info.code, "again").is_err(), "codes work once");
        assert!(remote.authenticate(&token).is_some());
        assert!(remote.authenticate("not-a-token").is_none());

        // Tokens survive a restart (only their hashes are stored), and revoking stops them.
        let stored = fs::read_to_string(dir.path().join("remote/devices.json")).unwrap();
        assert!(!stored.contains(&token));
        let again = Remote::open(dir.path(), 0, false).unwrap();
        assert_eq!(again.fingerprint, remote.fingerprint, "the certificate is kept");
        assert!(again.authenticate(&token).is_some());
        again.revoke(&device_id).unwrap();
        assert!(again.authenticate(&token).is_none());
    }

    #[test]
    fn too_many_wrong_codes_cancel_pairing() {
        let dir = tempfile::tempdir().unwrap();
        let remote = Remote::open(dir.path(), 0, false).unwrap();
        let info = remote.begin_pairing().unwrap();
        for _ in 0..MAX_FAILURES {
            let _ = remote.pair("ZZZZZZZZ", "x");
        }
        assert!(remote.pair(&info.code, "x").is_err());
    }

    #[test]
    fn identity_uses_generic_san_and_private_perms() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let remote = Remote::open_with(dir.path(), IpAddr::V4(Ipv4Addr::LOCALHOST), 0, false, Some("custom-host".into())).unwrap();
        assert_eq!(remote.host, "custom-host");
        let remote_dir = dir.path().join("remote");
        assert_eq!(fs::metadata(&remote_dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(remote_dir.join("key.der")).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(remote_dir.join("cert.der")).unwrap().permissions().mode() & 0o777, 0o600);
        let cert_der = fs::read(remote_dir.join("cert.der")).unwrap();
        assert!(cert_der.windows(b"neutrino.local".len()).any(|w| w == b"neutrino.local"));
        let host_label = format!("{}.local", computer_name().replace(' ', "-"));
        assert!(
            !cert_der.windows(host_label.len()).any(|w| w == host_label.as_bytes()),
            "certificate must not leak real computer name {host_label}"
        );
    }

    #[tokio::test]
    async fn bounded_line_rejects_oversized_preauth() {
        let data = format!("{}\n", "a".repeat(MAX_PREAUTH_LINE + 64));
        let mut reader = BufReader::new(data.as_bytes());
        assert!(read_bounded_line(&mut reader, MAX_PREAUTH_LINE).await.is_err());

        let ok = "{\"id\":1,\"method\":\"auth\",\"params\":{\"token\":\"t\"}}\n";
        let mut reader = BufReader::new(ok.as_bytes());
        assert_eq!(read_bounded_line(&mut reader, MAX_PREAUTH_LINE).await.unwrap().as_deref(), Some(ok));
    }

    #[test]
    fn per_ip_auth_rate_limit_locks_out_after_five_failures() {
        let dir = tempfile::tempdir().unwrap();
        let remote = Remote::open_with(dir.path(), IpAddr::V4(Ipv4Addr::LOCALHOST), 0, false, None).unwrap();
        let info = remote.begin_pairing().unwrap();
        let Response::Paired { token, .. } = remote.pair(&info.code, "Mac").unwrap() else { panic!() };
        let ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
        for _ in 0..AUTH_MAX_FAILURES {
            assert!(remote.authenticate_from(Some(ip), "wrong").unwrap().is_none());
        }
        assert!(remote.authenticate_from(Some(ip), &token).is_err(), "locked IP must be rejected even with valid token");
        let other = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 11));
        assert!(remote.authenticate_from(Some(other), &token).unwrap().is_some());
    }
}
