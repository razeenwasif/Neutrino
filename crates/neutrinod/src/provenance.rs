//! Hardware-signed SLSA L3 / In-Toto provenance ledger (Wave 5A).
//!
//! Each merge, CI job finish, and environment deploy approval produces a canonical
//! In-Toto v1 JSON statement signed by the server's P-256 provenance key (and paired
//! iPhone Secure Enclave key signature when approved remotely) stored in SQLite and
//! anchored cleanly under `refs/neutrino/attestations/<sha>`.

use std::path::Path;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use neutrino_proto::{ProvenanceKind, ProvenanceRecord, ProvenanceVerification};
use neutrino_store::now;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair, UnparsedPublicKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::Daemon;

/// Evidence from a paired device's Secure Enclave signature (`Request::Signed`).
#[derive(Debug, Clone)]
pub struct SignedEvidence {
    pub device_id: String,
    pub public_key: String,
    pub signature: String,
    pub at: i64,
    pub nonce: String,
    pub payload: String,
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn load_or_create_keypair(state_dir: &Path) -> Result<(EcdsaKeyPair, String, String)> {
    let key_path = state_dir.join("provenance-key.pkcs8");
    let rng = SystemRandom::new();
    let pkcs8 = match std::fs::read(&key_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
                .map_err(|_| anyhow::anyhow!("generating provenance signing key"))?;
            let bytes = doc.as_ref().to_vec();
            if let Some(parent) = key_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            use std::io::Write as _;
            #[cfg(unix)]
            let res = {
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&key_path)
                    .and_then(|mut f| f.write_all(&bytes))
            };
            #[cfg(not(unix))]
            let res = std::fs::write(&key_path, &bytes);
            match res {
                Ok(()) => bytes,
                Err(_) => std::fs::read(&key_path).with_context(|| format!("reading {}", key_path.display()))?,
            }
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", key_path.display())),
    };
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &pkcs8, &rng)
        .map_err(|e| anyhow::anyhow!("loading provenance signing key: {e}"))?;
    let pub_bytes = pair.public_key().as_ref();
    let pub_b64 = b64(pub_bytes);
    let key_id: String = Sha256::digest(pub_bytes).iter().take(8).map(|b| format!("{b:02x}")).collect();
    Ok((pair, key_id, pub_b64))
}

fn kind_label(kind: ProvenanceKind) -> &'static str {
    match kind {
        ProvenanceKind::Merge => "https://neutrino.dev/attestations/merge/v1",
        ProvenanceKind::Ci => "https://neutrino.dev/attestations/ci/v1",
        ProvenanceKind::Deploy => "https://neutrino.dev/attestations/deploy/v1",
    }
}

/// Cryptographically verify a provenance record's server signature and optional Secure Enclave signature.
pub fn verify_provenance_record(rec: &ProvenanceRecord) -> Result<()> {
    let srv_pub = base64::engine::general_purpose::STANDARD
        .decode(rec.server_public_key.trim())
        .context("invalid server_public_key base64")?;
    let srv_sig = base64::engine::general_purpose::STANDARD
        .decode(rec.server_signature.trim())
        .context("invalid server_signature base64")?;
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &srv_pub)
        .verify(rec.statement_json.as_bytes(), &srv_sig)
        .map_err(|_| anyhow::anyhow!("server provenance signature mismatch on record {}", rec.id))?;

    if let (Some(d_pub_b64), Some(d_sig_b64), Some(d_payload)) = (&rec.device_public_key, &rec.device_signature, &rec.device_payload) {
        let d_pub = base64::engine::general_purpose::STANDARD
            .decode(d_pub_b64.trim())
            .context("invalid device_public_key base64")?;
        let d_sig = base64::engine::general_purpose::STANDARD
            .decode(d_sig_b64.trim())
            .context("invalid device_signature base64")?;
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &d_pub)
            .verify(d_payload.as_bytes(), &d_sig)
            .map_err(|_| anyhow::anyhow!("device Secure Enclave signature mismatch on record {}", rec.id))?;
    } else if rec.device_signature.is_some() {
        bail!("record {} has device signature without key or payload", rec.id);
    }
    Ok(())
}

impl Daemon {
    pub(crate) fn record_provenance(
        &self,
        repo_id: &str,
        sha: &str,
        kind: ProvenanceKind,
        predicate: Value,
        device: Option<SignedEvidence>,
    ) -> Result<ProvenanceRecord> {
        let store = self.store()?;
        let (pair, key_id, pub_b64) = load_or_create_keypair(&self.state_dir)?;
        let ts = now();
        let statement = json!({
            "_type": "https://in-toto.io/Statement/v1",
            "subject": [{ "name": repo_id, "digest": { "gitCommit": sha } }],
            "predicateType": kind_label(kind),
            "predicate": predicate,
            "recordedAt": ts,
            "signerKeyId": key_id,
        });
        let statement_json = serde_json::to_string(&statement)?;
        let rng = SystemRandom::new();
        let sig = pair
            .sign(&rng, statement_json.as_bytes())
            .map_err(|_| anyhow::anyhow!("signing provenance statement failed"))?;

        let (device_id, device_public_key, device_signature, device_signed_at, device_nonce, device_payload) = match device {
            Some(ev) => (
                Some(ev.device_id),
                Some(ev.public_key),
                Some(ev.signature),
                Some(ev.at),
                Some(ev.nonce),
                Some(ev.payload),
            ),
            None => (None, None, None, None, None, None),
        };

        let mut rec = ProvenanceRecord {
            id: 0,
            repo: repo_id.to_string(),
            sha: sha.to_string(),
            kind,
            statement_json,
            server_key_id: key_id,
            server_public_key: pub_b64,
            server_signature: b64(sig.as_ref()),
            device_id,
            device_public_key,
            device_signature,
            device_signed_at,
            device_nonce,
            device_payload,
            git_ref: None,
            created_at: ts,
        };

        let id = store.add_provenance(&rec)?;
        rec.id = id;

        if let Ok(entry) = self.registry.get(repo_id) {
            let gix_repo = neutrino_git::local(&entry);
            if let Ok(all) = store.provenance_for_sha(repo_id, sha)
                && let Ok(bytes) = serde_json::to_vec_pretty(&all)
                && let Ok(ref_name) = neutrino_git::forge::write_attestation_ref(&gix_repo, sha, &bytes)
            {
                let _ = store.update_provenance_git_ref(id, &ref_name);
                rec.git_ref = Some(ref_name);
            }
        }
        Ok(rec)
    }

    pub(crate) fn provenance(&self, repo_id: &str, rev: &str) -> Result<Vec<ProvenanceRecord>> {
        let entry = self.registry.get(repo_id)?;
        let gix_repo = neutrino_git::local(&entry);
        let sha = neutrino_git::resolve(&gix_repo, rev)?.to_string();
        let store = self.store()?;
        let mut records = store.provenance_for_sha(repo_id, &sha)?;
        if records.is_empty()
            && let Some(bytes) = neutrino_git::forge::read_attestation_ref(&gix_repo, &sha)?
            && let Ok(from_ref) = serde_json::from_slice::<Vec<ProvenanceRecord>>(&bytes)
        {
            records = from_ref;
        }
        Ok(records)
    }

    pub(crate) fn verify_provenance(&self, repo_id: &str, rev: &str) -> Result<ProvenanceVerification> {
        let entry = self.registry.get(repo_id)?;
        let gix_repo = neutrino_git::local(&entry);
        let sha = neutrino_git::resolve(&gix_repo, rev)?.to_string();
        let records = self.provenance(repo_id, &sha)?;
        let mut verified_count = 0u32;
        let mut device_signed_count = 0u32;
        let mut issues = Vec::new();
        for rec in &records {
            if rec.sha != sha {
                issues.push(format!("record {} has sha {} instead of {sha}", rec.id, rec.sha));
                continue;
            }
            match verify_provenance_record(rec) {
                Ok(()) => {
                    verified_count += 1;
                    if rec.device_signature.is_some() {
                        device_signed_count += 1;
                    }
                }
                Err(e) => issues.push(format!("{e:#}")),
            }
        }
        if records.is_empty() {
            issues.push(format!("no provenance attestations found for {}", &sha[..sha.len().min(10)]));
        }
        Ok(ProvenanceVerification {
            sha,
            valid: issues.is_empty() && verified_count > 0,
            records,
            verified_count,
            device_signed_count,
            issues,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_verifies_and_detects_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let (pair, key_id, pub_b64) = load_or_create_keypair(dir.path()).unwrap();
        let statement = r#"{"_type":"https://in-toto.io/Statement/v1","predicate":{"ok":true}}"#.to_string();
        let rng = SystemRandom::new();
        let sig = pair.sign(&rng, statement.as_bytes()).unwrap();
        let mut rec = ProvenanceRecord {
            id: 1,
            repo: "app".into(),
            sha: "abc1234567".into(),
            kind: ProvenanceKind::Ci,
            statement_json: statement,
            server_key_id: key_id,
            server_public_key: pub_b64,
            server_signature: b64(sig.as_ref()),
            device_id: None,
            device_public_key: None,
            device_signature: None,
            device_signed_at: None,
            device_nonce: None,
            device_payload: None,
            git_ref: None,
            created_at: 1,
        };
        assert!(verify_provenance_record(&rec).is_ok());
        rec.statement_json = r#"{"_type":"https://in-toto.io/Statement/v1","predicate":{"ok":false}}"#.to_string();
        assert!(verify_provenance_record(&rec).is_err());
    }
}
