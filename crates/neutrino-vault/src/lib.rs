//! Neutrino's secrets.
//!
//! * **At rest**: each value is sealed with XChaCha20-Poly1305 under one
//!   master key, with the secret's scope and name as associated data, so a
//!   sealed value can't be moved to another name or repo unnoticed.
//! * **The master key** lives in the login Keychain on the server (or in a
//!   file, for tests). It's created by [`Vault::setup`], which returns the
//!   recovery code: the key itself, written to be typed back in. Without it, a
//!   database backup's secrets can't be opened anywhere else.
//! * **In logs**: [`Masker`] replaces each secret, and its base64 form, as
//!   output streams past, even when a value is split across two chunks.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

const NONCE: usize = 24;
const KEY: usize = 32;
/// Keychain service and account the master key is kept under.
pub const KEYCHAIN_SERVICE: &str = "dev.neutrino.vault";
const KEYCHAIN_ACCOUNT: &str = "master-key";

/// Where the master key is kept.
#[derive(Debug, Clone)]
pub enum KeyStore {
    /// The login Keychain. Readable while the user is logged in.
    Keychain,
    /// A file only this user can read. For tests, and Macs without a Keychain session.
    File(PathBuf),
}

impl KeyStore {
    fn load(&self) -> Result<Option<[u8; KEY]>> {
        let bytes = match self {
            KeyStore::Keychain => match security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
                Ok(b) => b,
                // errSecItemNotFound
                Err(e) if e.code() == -25300 => return Ok(None),
                Err(e) => bail!("can't read the vault key from the Keychain ({e}); is this Mac logged in?"),
            },
            KeyStore::File(path) => match std::fs::read(path) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
            },
        };
        let key: [u8; KEY] = bytes.try_into().map_err(|_| anyhow::anyhow!("the stored vault key is the wrong length"))?;
        Ok(Some(key))
    }

    fn save(&self, key: &[u8; KEY]) -> Result<()> {
        match self {
            KeyStore::Keychain => security_framework::passwords::set_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, key)
                .map_err(|e| anyhow::anyhow!("can't save the vault key in the Keychain: {e}")),
            KeyStore::File(path) => {
                use std::io::Write;
                use std::os::unix::fs::OpenOptionsExt;
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).with_context(|| format!("creating {}", path.display()))?;
                f.write_all(key)?;
                Ok(())
            }
        }
    }
}

pub struct Vault {
    store: KeyStore,
    cipher: std::sync::Mutex<Option<XChaCha20Poly1305>>,
}

const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// 32 bytes as 52 characters in groups of four, without the letters people misread.
pub fn recovery_code(key: &[u8; KEY]) -> String {
    let mut bits: u64 = 0;
    let mut n = 0;
    let mut out = String::new();
    for &b in key {
        bits = (bits << 8) | u64::from(b);
        n += 8;
        while n >= 5 {
            n -= 5;
            out.push(ALPHABET[((bits >> n) & 31) as usize] as char);
        }
    }
    if n > 0 {
        out.push(ALPHABET[((bits << (5 - n)) & 31) as usize] as char);
    }
    out.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("-")
}

pub fn parse_recovery_code(code: &str) -> Result<[u8; KEY]> {
    let mut bits: u64 = 0;
    let mut n = 0;
    let mut out = Vec::with_capacity(KEY);
    for c in code.chars().filter(|c| !c.is_whitespace() && *c != '-') {
        // The alphabet leaves out I, O, 0 and 1, so nothing can be misread as them.
        let c = c.to_ascii_uppercase();
        let v = ALPHABET.iter().position(|&a| a as char == c).ok_or_else(|| anyhow::anyhow!("{c:?} isn't in a recovery code"))?;
        bits = (bits << 5) | v as u64;
        n += 5;
        if n >= 8 {
            n -= 8;
            out.push((bits >> n) as u8);
        }
    }
    out.try_into().map_err(|_| anyhow::anyhow!("that recovery code is the wrong length"))
}

impl Vault {
    pub fn new(store: KeyStore) -> Self {
        Vault { store, cipher: std::sync::Mutex::new(None) }
    }

    /// Whether a master key exists.
    pub fn is_set_up(&self) -> Result<bool> {
        Ok(self.cipher.lock().unwrap().is_some() || self.store.load()?.is_some())
    }

    /// Create the master key. Returns its recovery code, which is shown once.
    pub fn setup(&self) -> Result<String> {
        if self.store.load()?.is_some() {
            bail!("the vault is already set up; its recovery code was shown then");
        }
        let mut key = [0u8; KEY];
        getrandom::getrandom(&mut key).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
        self.store.save(&key)?;
        *self.cipher.lock().unwrap() = Some(XChaCha20Poly1305::new((&key).into()));
        Ok(recovery_code(&key))
    }

    /// Put a master key back from its recovery code (a new server, say).
    pub fn recover(&self, code: &str) -> Result<()> {
        let key = parse_recovery_code(code)?;
        if let Some(existing) = self.store.load()? {
            if existing == key {
                return Ok(());
            }
            bail!("this vault already has a different key; recovering would make its secrets unreadable");
        }
        self.store.save(&key)?;
        *self.cipher.lock().unwrap() = Some(XChaCha20Poly1305::new((&key).into()));
        Ok(())
    }

    fn with_cipher<T>(&self, f: impl FnOnce(&XChaCha20Poly1305) -> Result<T>) -> Result<T> {
        let mut guard = self.cipher.lock().unwrap();
        if guard.is_none() {
            let key = self.store.load()?.ok_or_else(|| anyhow::anyhow!("the vault isn't set up yet; run `ntr vault setup` on the server"))?;
            *guard = Some(XChaCha20Poly1305::new((&key).into()));
        }
        f(guard.as_ref().expect("set"))
    }

    /// Seal a value; `context` names what it is (scope and name).
    pub fn seal(&self, context: &str, value: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE];
        getrandom::getrandom(&mut nonce).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
        self.with_cipher(|c| {
            let mut out = nonce.to_vec();
            out.extend(c.encrypt(XNonce::from_slice(&nonce), Payload { msg: value, aad: context.as_bytes() }).map_err(|_| anyhow::anyhow!("sealing failed"))?);
            Ok(out)
        })
    }

    pub fn open(&self, context: &str, sealed: &[u8]) -> Result<Vec<u8>> {
        if sealed.len() < NONCE {
            bail!("a sealed value is too short");
        }
        let (nonce, body) = sealed.split_at(NONCE);
        self.with_cipher(|c| {
            c.decrypt(XNonce::from_slice(nonce), Payload { msg: body, aad: context.as_bytes() })
                .map_err(|_| anyhow::anyhow!("a secret can't be opened with this vault's key (a different key, or it was moved)"))
        })
    }
}

// ---------------------------------------------------------------------------
// Masking
// ---------------------------------------------------------------------------

/// What masked text reads as.
pub const MASK: &[u8] = b"***";
/// Values shorter than this aren't masked: they'd blank out ordinary output.
pub const MIN_MASKED: usize = 4;

/// Replaces secrets in a stream of output. Holds back just enough of the end
/// of each chunk to catch a value that continues in the next one.
pub struct Masker {
    needles: Vec<Vec<u8>>,
    keep: usize,
    carry: Vec<u8>,
}

impl Masker {
    pub fn new<'a>(secrets: impl IntoIterator<Item = &'a str>) -> Masker {
        let mut needles: Vec<Vec<u8>> = Vec::new();
        for s in secrets {
            if s.len() < MIN_MASKED {
                continue;
            }
            needles.push(s.as_bytes().to_vec());
            needles.push(base64::engine::general_purpose::STANDARD.encode(s).into_bytes());
            // Each line of a multi-line secret on its own, as tools often print one.
            for line in s.lines().map(str::trim).filter(|l| l.len() >= MIN_MASKED && *l != s) {
                needles.push(line.as_bytes().to_vec());
            }
        }
        // Longest first, so a value inside another doesn't leave a piece showing.
        needles.sort_by_key(|n| std::cmp::Reverse(n.len()));
        needles.dedup();
        let keep = needles.first().map(|n| n.len() - 1).unwrap_or(0);
        Masker { needles, keep, carry: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.needles.is_empty()
    }

    fn replace(&self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        let mut i = 0;
        'outer: while i < data.len() {
            for n in &self.needles {
                if data[i..].starts_with(n) {
                    out.extend_from_slice(MASK);
                    i += n.len();
                    continue 'outer;
                }
            }
            out.push(data[i]);
            i += 1;
        }
        out
    }

    /// Mask a chunk; returns what's safe to write now.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        if self.needles.is_empty() {
            return chunk.to_vec();
        }
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(chunk);
        let masked = self.replace(&data);
        // Hold back a tail that could be the start of a secret.
        let split = masked.len().saturating_sub(self.keep);
        // From the first place where what's left could still grow into a secret.
        let hold = (split..masked.len()).find(|&i| self.needles.iter().any(|n| n.starts_with(&masked[i..]))).unwrap_or(masked.len());
        self.carry = masked[hold..].to_vec();
        masked[..hold].to_vec()
    }

    /// Whatever is still held back, at the end of the stream.
    pub fn flush(&mut self) -> Vec<u8> {
        let rest = std::mem::take(&mut self.carry);
        self.replace(&rest)
    }

    /// Mask a whole string at once.
    pub fn mask(&self, text: &str) -> String {
        String::from_utf8_lossy(&self.replace(text.as_bytes())).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::new(KeyStore::File(dir.path().join("vault.key")));
        (dir, v)
    }

    #[test]
    fn seal_open_and_recover() {
        let (dir, v) = vault();
        assert!(!v.is_set_up().unwrap());
        assert!(v.seal("repo/app/TOKEN", b"x").is_err(), "nothing works before setup");
        let code = v.setup().unwrap();
        assert_eq!(code.split('-').count(), 13);
        assert!(v.setup().is_err(), "setup happens once");
        let sealed = v.seal("repo/app/TOKEN", b"hunter22").unwrap();
        assert_ne!(&sealed[24..], b"hunter22");
        assert_eq!(v.open("repo/app/TOKEN", &sealed).unwrap(), b"hunter22");
        assert!(v.open("repo/other/TOKEN", &sealed).is_err(), "a value moved to another name won't open");
        assert_ne!(v.seal("repo/app/TOKEN", b"hunter22").unwrap(), sealed, "each seal has its own nonce");

        // Another server with the recovery code opens the same values.
        let other = Vault::new(KeyStore::File(dir.path().join("other.key")));
        other.recover(&code.to_lowercase().replace('-', " ")).unwrap();
        assert_eq!(other.open("repo/app/TOKEN", &sealed).unwrap(), b"hunter22");
        assert!(other.recover(&recovery_code(&[7; 32])).is_err(), "a different key is refused");
        assert!(parse_recovery_code("ABCD").is_err());
        assert_eq!(parse_recovery_code(&recovery_code(&[0xA5; 32])).unwrap(), [0xA5; 32]);
    }

    #[test]
    fn masking_across_chunks() {
        let mut m = Masker::new(["s3cr3t-value", "abc", "multi\nline-secret"]);
        let mut out = Vec::new();
        for chunk in [&b"token=s3cr"[..], b"3t-value and ", b"abc stays; b64 ", base64::engine::general_purpose::STANDARD.encode("s3cr3t-value").as_bytes(), b" line-secret\n"] {
            out.extend(m.push(chunk));
        }
        out.extend(m.flush());
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "token=*** and abc stays; b64 *** ***\n");
        let m = Masker::new(["password1"]);
        assert_eq!(m.mask("echo password1password1"), "echo ******");
        assert!(Masker::new(["ab"]).is_empty(), "too short to mask");
    }

    #[test]
    fn masking_holds_back_only_what_might_be_a_secret() {
        let mut m = Masker::new(["s3cr3t"]);
        assert_eq!(m.push(b"hello world\n"), b"hello world\n", "nothing held when no secret could start");
        assert_eq!(m.push(b"tail s3c"), b"tail ");
        assert_eq!(m.push(b"ond"), b"s3cond");
        assert_eq!(m.flush(), b"");
    }
}
