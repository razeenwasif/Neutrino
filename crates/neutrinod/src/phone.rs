//! The phone: its signing key and push token, signed approvals, the Inbox,
//! and push notifications through Apple.
//!
//! Approvals (merging, approving a deploy, cancelling a run) from a device
//! that registered a Secure Enclave key must be signed with it, so a stolen
//! token alone can't do them. Pushes carry only an opaque id and a generic
//! alert; the phone fetches the real text over the paired connection.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use neutrino_proto::{Inbox, NotificationText, PullState, PushInfo, Request, Response, decode_request};
use neutrino_remote::{PushTarget, SIGNATURE_WINDOW_SECS};
use neutrino_store::now;

use crate::{Daemon, Scope};

/// What has to be signed when the device signs.
fn needs_signature(r: &Request) -> bool {
    matches!(
        r,
        Request::MergePull { .. } | Request::ApproveJob { .. } | Request::CancelRun { .. } | Request::RevealSecret { .. }
    )
}

/// Apple's push service: a JWT made from the stored key, good for 50 minutes.
#[derive(Default)]
pub struct Apns {
    jwt: Mutex<Option<(String, Instant)>>,
    http: Mutex<Option<reqwest::Client>>,
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The DER inside a .p8 file (PEM or bare base64).
fn p8_der(p8: &str) -> Result<Vec<u8>> {
    let body: String = p8.lines().filter(|l| !l.starts_with("-----")).collect::<Vec<_>>().join("");
    base64::engine::general_purpose::STANDARD.decode(body.trim()).context("that isn't a .p8 key")
}

/// An ES256 JWT for APNs, signed with the .p8 key.
fn apns_jwt(p8: &str, key_id: &str, team_id: &str, at: i64) -> Result<String> {
    use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
    let rng = ring::rand::SystemRandom::new();
    let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &p8_der(p8)?, &rng).map_err(|e| anyhow::anyhow!("the .p8 key can't be read: {e}"))?;
    let head = b64url(serde_json::json!({ "alg": "ES256", "kid": key_id }).to_string().as_bytes());
    let claims = b64url(serde_json::json!({ "iss": team_id, "iat": at }).to_string().as_bytes());
    let signing_input = format!("{head}.{claims}");
    let sig = key.sign(&rng, signing_input.as_bytes()).map_err(|_| anyhow::anyhow!("signing the push token failed"))?;
    Ok(format!("{signing_input}.{}", b64url(sig.as_ref())))
}

impl Daemon {
    /// Check a signature and hand back what's inside it plus the verified hardware evidence.
    pub(crate) fn unwrap_signed(
        &self,
        scope: &Scope,
        inner: &str,
        at: i64,
        nonce: &str,
        signature: &str,
    ) -> Result<(Request, crate::provenance::SignedEvidence)> {
        let Scope::Remote { id: device, .. } = scope else { bail!("only a paired device signs requests") };
        let remote = self.remote.as_ref().context("this server has no network endpoint")?;
        let t = now();
        if (t - at).abs() > SIGNATURE_WINDOW_SECS {
            bail!("that signed request is stale (or this phone's clock is off)");
        }
        if nonce.len() < 16 || nonce.len() > 64 {
            bail!("a signed request needs a nonce of 16 to 64 characters");
        }
        let payload = format!("{at}\n{nonce}\n{inner}");
        remote.verify(device, payload.as_bytes(), signature)?;
        {
            let mut seen = self.nonces.lock().unwrap();
            seen.retain(|_, when| t - *when <= SIGNATURE_WINDOW_SECS);
            if seen.insert(format!("{device}:{nonce}"), t).is_some() {
                bail!("that signed request was already used");
            }
        }
        let (_, request) = decode_request(inner);
        let request = request.map_err(|e| anyhow::anyhow!("the signed request doesn't parse: {e}"))?;
        if matches!(request, Request::Signed { .. }) {
            bail!("a signed request can't hold another");
        }
        let public_key = remote.signing_key(device).unwrap_or_default();
        Ok((
            request,
            crate::provenance::SignedEvidence {
                device_id: device.clone(),
                public_key,
                signature: signature.to_string(),
                at,
                nonce: nonce.to_string(),
                payload,
            },
        ))
    }

    /// The phone's own requests, and the gate on unsigned approvals. `None` lets the request through.
    pub(crate) fn device_request(&self, scope: &Scope, request: &Request, signed: bool) -> Option<Response> {
        let device = match scope {
            Scope::Remote { id, .. } => id,
            Scope::Local => return None,
        };
        let remote = self.remote.as_ref()?;
        let done = |r: Result<()>, ok: &str| match r {
            Ok(()) => Response::Done { output: ok.into() },
            Err(e) => Response::Error { message: format!("{e:#}") },
        };
        match request {
            Request::RegisterKey { public_key } => Some(done(remote.set_signing_key(device, public_key), "key registered; this device's approvals are signed from now on")),
            Request::RegisterPush { token, sandbox, events } => {
                let target = token.as_ref().filter(|t| !t.is_empty()).map(|t| PushTarget {
                    token: t.chars().filter(char::is_ascii_hexdigit).collect(),
                    sandbox: *sandbox,
                    events: events.iter().filter(|e| ["approvals", "failures"].contains(&e.as_str())).cloned().collect(),
                });
                Some(done(remote.set_push(device, target), "push settings saved"))
            }
            r if needs_signature(r) && !signed && remote.signs(device) => {
                Some(Response::Error { message: "this phone signs its approvals, and that request wasn't signed".into() })
            }
            _ => None,
        }
    }

    /// What needs you, everywhere.
    pub(crate) fn inbox(&self) -> Result<Inbox> {
        let store = self.store()?;
        let mut inbox = Inbox { waiting: store.waiting()?, runs: store.runs_everywhere(15)?, events: store.notifications(20)?, ..Default::default() };
        for repo in self.registry.list().into_iter().filter(|r| r.bare && r.error.is_none()) {
            let Ok(handle) = self.registry.get(&repo.id) else { continue };
            let local = neutrino_git::local(&handle);
            for p in store.pulls(&repo.id)?.into_iter().filter(|p| p.state == PullState::Open) {
                let clean = neutrino_git::forge::mergeability(&local, &p.target, &p.source).map(|m| m.clean).unwrap_or(false);
                let checks = store.runs_for_sha(&repo.id, &p.head_sha)?;
                let required = store.rule_for(&repo.id, &p.target)?.map(|r| r.required_checks).unwrap_or_default();
                let passing = crate::pulls::missing_checks(&required, &checks).is_empty() && !checks.iter().any(|r| r.state == neutrino_proto::RunState::Failure);
                if clean && passing { inbox.ready.push(p) } else { inbox.open.push(p) }
            }
        }
        Ok(inbox)
    }

    pub(crate) fn push_setup(&self, p8: &str, key_id: &str, team_id: &str, topic: &str) -> Result<Response> {
        // Check it signs before keeping it.
        apns_jwt(p8, key_id, team_id, now())?;
        if key_id.len() != 10 || team_id.len() != 10 {
            bail!("APNs key and team ids are ten characters each");
        }
        let sealed = self.vault()?.seal("apns\0key", p8.as_bytes())?;
        self.store()?.set_push_config(key_id.trim(), team_id.trim(), topic.trim(), &sealed)?;
        *self.apns.jwt.lock().unwrap() = None;
        Ok(Response::Push(self.push_info()?))
    }

    pub(crate) fn push_info(&self) -> Result<PushInfo> {
        let config = self.store()?.push_config()?;
        let devices = self.remote.as_ref().map(|r| r.devices().iter().filter(|d| d.push).count()).unwrap_or(0) as u32;
        Ok(PushInfo { configured: config.is_some(), topic: config.map(|c| c.2), devices })
    }

    fn jwt(&self) -> Result<Option<(String, String)>> {
        let Some((key_id, team_id, topic, sealed)) = self.store()?.push_config()? else { return Ok(None) };
        let mut cached = self.apns.jwt.lock().unwrap();
        if let Some((jwt, made)) = cached.as_ref() {
            if made.elapsed() < Duration::from_secs(50 * 60) {
                return Ok(Some((jwt.clone(), topic)));
            }
        }
        let p8 = String::from_utf8(self.vault()?.open("apns\0key", &sealed)?)?;
        let jwt = apns_jwt(&p8, &key_id, &team_id, now())?;
        *cached = Some((jwt.clone(), Instant::now()));
        Ok(Some((jwt, topic)))
    }

    /// Push one notification to one device. Only the id and a generic alert go to Apple.
    async fn push_to(&self, target: &PushTarget, id: &str) -> Result<()> {
        let Some((jwt, topic)) = self.jwt()? else { bail!("no APNs key; run `ntr notify setup`") };
        let http = self.apns.http.lock().unwrap().get_or_insert_with(|| reqwest::Client::builder().http2_adaptive_window(true).build().expect("client")).clone();
        let host = if target.sandbox { "api.sandbox.push.apple.com" } else { "api.push.apple.com" };
        let body = serde_json::json!({
            "aps": { "alert": { "title": "Neutrino", "body": "Something needs you" }, "mutable-content": 1, "sound": "default" },
            "e": id,
        });
        let resp = http
            .post(format!("https://{host}/3/device/{}", target.token))
            .header("authorization", format!("bearer {jwt}"))
            .header("apns-topic", topic)
            .header("apns-push-type", "alert")
            .header("apns-priority", "10")
            .json(&body)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .context("reaching Apple's push service")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let reason = resp.json::<serde_json::Value>().await.ok().and_then(|v| v["reason"].as_str().map(String::from)).unwrap_or_default();
            bail!("Apple refused the push ({status} {reason})");
        }
        Ok(())
    }

    /// Record an event and push its id to the devices that want it.
    pub(crate) fn notify(self: &std::sync::Arc<Self>, kind: &str, title: String, body: String, repo: &str, run: Option<i64>, pull: Option<u32>) {
        let Ok(store) = self.store() else { return };
        let mut raw = [0u8; 12];
        if getrandom::getrandom(&mut raw).is_err() {
            return;
        }
        let id: String = raw.iter().map(|b| format!("{b:02x}")).collect();
        let text = NotificationText { id: id.clone(), kind: kind.into(), title, body, repo: repo.into(), run, pull, at: now() };
        if let Err(e) = store.add_notification(&text) {
            eprintln!("can't record a notification: {e:#}");
            return;
        }
        let event = if kind == "approval" { "approvals" } else { "failures" };
        let Some(remote) = self.remote.clone() else { return };
        let targets = remote.push_targets(event);
        if targets.is_empty() || store.push_config().ok().flatten().is_none() {
            return;
        }
        let this = std::sync::Arc::clone(self);
        let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
        handle.spawn(async move {
            for t in targets {
                if let Err(e) = this.push_to(&t, &id).await {
                    eprintln!("push to a device failed: {e:#}");
                }
            }
        });
    }

    pub(crate) async fn push_test(self: &std::sync::Arc<Self>) -> Result<String> {
        let remote = self.remote.clone().context("this server has no network endpoint")?;
        let mut targets = remote.push_targets("approvals");
        for t in remote.push_targets("failures") {
            if !targets.contains(&t) {
                targets.push(t);
            }
        }
        if targets.is_empty() {
            bail!("no paired device takes pushes; turn them on in the phone's settings");
        }
        let id = {
            let mut raw = [0u8; 12];
            getrandom::getrandom(&mut raw).map_err(|e| anyhow::anyhow!("{e}"))?;
            raw.iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        self.store()?.add_notification(&NotificationText {
            id: id.clone(),
            kind: "test".into(),
            title: "Neutrino".into(),
            body: "Pushes from your server reach this phone.".into(),
            repo: String::new(),
            run: None,
            pull: None,
            at: now(),
        })?;
        let mut sent = 0;
        let mut errors = Vec::new();
        for t in &targets {
            match self.push_to(t, &id).await {
                Ok(()) => sent += 1,
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
        if sent == 0 {
            bail!("{}", errors.join("; "));
        }
        Ok(format!("sent to {sent} device{}", if sent == 1 { "" } else { "s" }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwts_sign_with_a_p8_key() {
        // A throwaway P-256 key in PKCS#8, as Apple's .p8 files are.
        let rng = ring::rand::SystemRandom::new();
        let doc = ring::signature::EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pem = format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", base64::engine::general_purpose::STANDARD.encode(doc.as_ref()));
        let jwt = apns_jwt(&pem, "ABCDE12345", "TEAM123456", 1_700_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let head: serde_json::Value = serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!((head["alg"].as_str(), head["kid"].as_str()), (Some("ES256"), Some("ABCDE12345")));
        assert_eq!(base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[2]).unwrap().len(), 64, "ES256 signatures are r||s");
        assert!(apns_jwt("not a key", "A", "B", 0).is_err());
    }
}
