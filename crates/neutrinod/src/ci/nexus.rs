//! `runs-on: nexus`: jobs sent to the Nexus coordinator, which places them on
//! forge (Windows, the GPU) or another node.
//!
//! Nexus runs one command per job, from a project snapshot. Neutrino packs the
//! commit with a small Python driver and the job's steps (`.neutrino-run/`),
//! uploads it as Nexus's CLI does, submits it, follows its log (reading the
//! driver's step markers back into steps), and fetches its outputs, where the
//! driver put the artifacts.
//!
//! Limits, all Nexus's: jobs and their logs are mirrored to Firestore, so no
//! secrets are sent; with the request-only token every job is a ticket that
//! waits for an Accept in Nexus, and can't be stopped from here.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use neutrino_ci::expr::{self, Status};
use neutrino_proto::{Run, RunState};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

use super::Log;
use crate::Daemon;

const DRIVER: &str = include_str!("nexus_run.py");
/// Where the driver and the job's steps go in the snapshot.
const RUN_DIR: &str = ".neutrino-run";

#[derive(Clone)]
pub struct Nexus {
    pub url: String,
    token: String,
    http: reqwest::Client,
}

impl Nexus {
    /// `url` is the coordinator (`http://127.0.0.1:7420` on atlas); the token
    /// is read from `token_file`.
    pub fn new(url: &str, token_file: &Path) -> Result<Nexus> {
        let token = std::fs::read_to_string(token_file).with_context(|| format!("reading {}", token_file.display()))?.trim().to_string();
        if token.is_empty() {
            bail!("{} is empty", token_file.display());
        }
        let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(5)).build()?;
        Ok(Nexus { url: url.trim_end_matches('/').to_string(), token, http })
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http.request(method, format!("{}{path}", self.url)).bearer_auth(&self.token)
    }

    async fn json(&self, r: reqwest::RequestBuilder) -> Result<Value> {
        let resp = r.send().await.context("reaching the Nexus coordinator")?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            bail!("Nexus said {status}: {}", body["detail"].as_str().unwrap_or(""));
        }
        Ok(body)
    }

    /// Upload a snapshot unless Nexus has it already.
    async fn upload(&self, digest: &str, body: Vec<u8>) -> Result<()> {
        let have = self.req(reqwest::Method::GET, &format!("/v1/projects/{digest}")).send().await.context("reaching the Nexus coordinator")?;
        if have.status().is_success() {
            return Ok(());
        }
        self.json(self.req(reqwest::Method::PUT, &format!("/v1/projects/{digest}")).timeout(Duration::from_secs(600)).body(body)).await?;
        Ok(())
    }

    async fn job(&self, id: &str) -> Result<Value> {
        self.json(self.req(reqwest::Method::GET, &format!("/v1/jobs/{id}")).timeout(Duration::from_secs(15))).await
    }

    async fn stop(&self, id: &str) -> Result<()> {
        self.json(self.req(reqwest::Method::POST, &format!("/v1/jobs/{id}/stop")).timeout(Duration::from_secs(30))).await.map(|_| ())
    }
}

/// A deterministic .tar.gz of `dir` without `.git` (sorted, times zeroed), so
/// an unchanged commit isn't uploaded twice. Returns the body, its sha256 and
/// how many files it holds.
fn snapshot(dir: &Path) -> Result<(Vec<u8>, String, usize)> {
    use sha2::{Digest, Sha256};
    fn walk(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            let path = e.path();
            if path.strip_prefix(base).ok() == Some(Path::new(".git")) {
                continue;
            }
            let t = e.file_type()?;
            if t.is_dir() {
                walk(base, &path, out)?;
            } else if t.is_file() || t.is_symlink() {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(dir, dir, &mut files)?;
    files.sort();
    let gz = flate2::GzBuilder::new().mtime(0).write(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    tar.mode(tar::HeaderMode::Deterministic);
    tar.follow_symlinks(false);
    for f in &files {
        let rel = f.strip_prefix(dir).expect("inside");
        tar.append_path_with_name(f, rel).with_context(|| format!("packing {}", rel.display()))?;
    }
    let body = tar.into_inner()?.finish()?;
    let digest: String = Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();
    Ok((body, digest, files.len()))
}

/// What the driver prints around each step.
enum Marker {
    Start(u32),
    End(u32, Option<i32>),
    Skip(u32),
}

fn marker(line: &str) -> Option<Marker> {
    let inner = line.trim().strip_prefix("##neutrino[")?.strip_suffix(']')?;
    let mut parts = inner.split_whitespace();
    let kind = parts.next()?;
    let index: u32 = parts.next()?.parse().ok()?;
    match kind {
        "start" => Some(Marker::Start(index)),
        "skip" => Some(Marker::Skip(index)),
        "end" => Some(Marker::End(index, parts.next().and_then(|c| c.parse().ok()))),
        _ => None,
    }
}

impl Daemon {
    pub(crate) async fn execute_nexus(
        self: &Arc<Self>,
        run: Run,
        q: neutrino_store::QueuedJob,
        def: neutrino_ci::Job,
        mut cx: expr::Context,
        mut cancelled: watch::Receiver<bool>,
    ) -> Result<()> {
        let store = self.store()?;
        let ci = self.ci.as_ref().expect("server");
        let nexus = ci.options.nexus.clone().expect("checked when scheduled");
        let (workspace, log_path) = self.job_paths(run.id, &q.key);
        let scratch = workspace.parent().map(Path::to_path_buf).unwrap_or_else(|| workspace.clone());
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        tokio::fs::create_dir_all(&scratch).await?;
        let mut log = Log::open(&log_path).await?;
        let fail = |why: &str| store.finish_job(q.id, RunState::Failure, Some(why));

        // Nexus mirrors jobs to Firestore: nothing secret goes there.
        let mentions_secrets = def.steps.iter().any(|s| s.run.contains("secrets.") || s.env.values().any(|v| v.contains("secrets.")))
            || def.env.values().any(|v| v.contains("secrets."));
        if mentions_secrets {
            log.note("this job uses secrets, and Nexus jobs can't have any: Nexus mirrors jobs and their logs to Firestore. Use variables for values that aren't secret.").await?;
            fail("secrets aren't sent to Nexus")?;
            return Ok(());
        }

        log.note(&format!("checking out {} ({})", &run.sha[..run.sha.len().min(10)], run.branch.as_deref().unwrap_or("detached"))).await?;
        let repo_handle = self.registry.get(&run.repo)?;
        let (sha, ws) = (run.sha.clone(), workspace.clone());
        if let Err(e) = tokio::task::spawn_blocking(move || neutrino_git::forge::checkout_into(&neutrino_git::local(&repo_handle), &sha, &ws)).await? {
            log.note(&format!("checkout failed: {e}")).await?;
            fail("checkout failed")?;
            let _ = tokio::fs::remove_dir_all(&scratch).await;
            return Ok(());
        }

        // Steps, worked out here: Nexus runs them as they are.
        let vars = store.variables_for(&run.repo, q.environment.as_deref().unwrap_or(""))?;
        cx = cx.with("vars", Value::Object(vars.into_iter().map(|(k, v)| (k, json!(v))).collect()));
        cx.root = Some(workspace.clone());
        let mut job_env = serde_json::Map::new();
        for (k, v) in cx.values.get("env").and_then(Value::as_object).cloned().unwrap_or_default() {
            job_env.insert(k, v);
        }
        for (k, v) in &def.env {
            job_env.insert(k.clone(), json!(expr::interpolate(v, &cx.clone().with("env", Value::Object(job_env.clone())))?));
        }
        let base_env = json!({
            "CI": "true",
            "NEUTRINO": "true",
            "NEUTRINO_EVENT": run.event,
            "NEUTRINO_SHA": run.sha,
            "NEUTRINO_REPOSITORY": run.repo,
            "NEUTRINO_RUN_NUMBER": run.number.to_string(),
            "NEUTRINO_JOB": q.job_id,
            "NEUTRINO_BRANCH": run.branch.clone().unwrap_or_default(),
        });
        cx = cx.with("env", Value::Object(job_env.clone()));
        let minute = ci.options.minute;
        let mut steps = Vec::new();
        for (i, step) in def.steps.iter().enumerate() {
            let holds = |failed: bool| -> Result<bool> {
                let mut c = cx.clone();
                c.status = Status { failed, cancelled: false };
                Ok(match &step.condition {
                    Some(cond) => cond.holds(&c)?,
                    None => !failed,
                })
            };
            let mut env = base_env.as_object().cloned().unwrap_or_default();
            for (k, v) in &job_env {
                env.insert(k.clone(), json!(expr::to_text(v)));
            }
            for (k, v) in &step.env {
                env.insert(k.clone(), json!(expr::interpolate(v, &cx)?));
            }
            steps.push(json!({
                "index": i,
                "script": expr::interpolate(&step.run, &cx)?,
                "shell": step.shell,
                "dir": step.working_directory,
                "env": env,
                "run_if_ok": holds(false)?,
                "run_if_failed": holds(true)?,
                "continue_on_error": step.continue_on_error,
                "timeout_secs": (minute * step.timeout_minutes.unwrap_or(def.timeout_minutes)).as_secs(),
            }));
        }
        let run_dir = workspace.join(RUN_DIR);
        tokio::fs::create_dir_all(&run_dir).await?;
        tokio::fs::write(run_dir.join("run.py"), DRIVER).await?;
        tokio::fs::write(run_dir.join("job.json"), serde_json::to_vec_pretty(&json!({ "steps": steps, "artifacts": def.artifacts }))?).await?;

        // Pack, upload, submit.
        let ws = workspace.clone();
        let (body, digest, files) = tokio::task::spawn_blocking(move || snapshot(&ws)).await??;
        log.note(&format!("sending {files} files ({} KB) to Nexus at {}", body.len().div_ceil(1024), nexus.url)).await?;
        if let Err(e) = nexus.upload(&digest, body).await {
            log.note(&format!("couldn't upload the snapshot: {e:#}")).await?;
            fail("Nexus is unreachable")?;
            let _ = tokio::fs::remove_dir_all(&scratch).await;
            return Ok(());
        }
        let spec = json!({
            "name": format!("{} · run {} · {}", run.repo, run.number, q.key),
            "command": format!("uv run --no-project python {RUN_DIR}/run.py"),
            "project": digest,
            "project_name": run.repo,
            "accelerator": def.nexus.accelerator.clone().unwrap_or_else(|| "any".into()),
            "gpu_memory_gb": def.nexus.gpu_memory_gb,
            "memory_gb": def.memory_mib as f64 / 1024.0,
            "cpus": def.cpus,
            "time_limit_hours": f64::from(def.timeout_minutes) / 60.0,
            "pull_outputs": false,
            "node": def.nexus.node,
            "requested_by": format!("neutrino: {} run {}", run.repo, run.number),
            "project_files": files,
        });
        let submitted = match nexus.json(nexus.req(reqwest::Method::POST, "/v1/jobs").timeout(Duration::from_secs(30)).json(&spec)).await {
            Ok(v) => v,
            Err(e) => {
                log.note(&format!("Nexus refused the job: {e:#}")).await?;
                fail("Nexus refused the job")?;
                let _ = tokio::fs::remove_dir_all(&scratch).await;
                return Ok(());
            }
        };
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        let id = submitted[0]["id"].as_str().context("Nexus didn't say which job it made")?.to_string();
        store.set_container(q.id, &format!("nexus:{id}"))?;
        log.note(&format!("Nexus job {id}")).await?;

        // Follow it: the log (with the driver's markers) and its state.
        let (tx, mut rx) = mpsc::channel::<String>(256);
        let follower = {
            let nexus = nexus.clone();
            let id = id.clone();
            tokio::spawn(async move {
                loop {
                    let resp = nexus.req(reqwest::Method::GET, &format!("/v1/jobs/{id}/logs")).send().await;
                    let Ok(resp) = resp else {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        continue;
                    };
                    let mut stream = resp.bytes_stream();
                    let mut buf = String::new();
                    while let Some(Ok(chunk)) = stream.next().await {
                        buf.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some(end) = buf.find("\n\n") {
                            let event: String = buf.drain(..end + 2).collect();
                            if event.lines().any(|l| l == "event: end") {
                                return;
                            }
                            for data in event.lines().filter_map(|l| l.strip_prefix("data: ")) {
                                if let Ok(line) = serde_json::from_str::<String>(data) {
                                    if tx.send(line).await.is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    // The stream ended without `end`: the coordinator restarted; pick up again.
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            })
        };

        let deadline = tokio::time::Instant::now() + minute * def.timeout_minutes + Duration::from_secs(60);
        let mut current: Option<u32> = None;
        let mut said_held = false;
        let mut stop_reason: Option<&str> = None;
        let mut poll = tokio::time::interval(Duration::from_secs(3));
        let final_state = loop {
            tokio::select! {
                line = rx.recv() => {
                    let Some(line) = line else { continue };
                    match marker(&line) {
                        Some(Marker::Start(i)) => {
                            log.flush().await?;
                            store.start_step(q.id, i, log.offset)?;
                            current = Some(i);
                            self.runs_changed(&run.repo);
                        }
                        Some(Marker::Skip(i)) => store.finish_step(q.id, i, RunState::Skipped, None, log.offset)?,
                        Some(Marker::End(i, code)) => {
                            log.flush().await?;
                            let state = if code == Some(0) { RunState::Success } else { RunState::Failure };
                            if code.is_none() {
                                log.note("the step ran past its timeout").await?;
                            }
                            store.finish_step(q.id, i, state, code, log.offset)?;
                            current = None;
                            self.runs_changed(&run.repo);
                        }
                        None => {
                            let text = line.strip_prefix("##neutrino-note ").map(|n| format!("\u{25b8} {n}\n")).unwrap_or(line + "\n");
                            log.write(text.as_bytes()).await?;
                            if let Some(i) = current {
                                store.step_log_end(q.id, i, log.offset)?;
                            }
                        }
                    }
                }
                _ = poll.tick() => {
                    let job = match nexus.job(&id).await {
                        Ok(j) => j,
                        Err(_) => continue,
                    };
                    match job["status"].as_str().unwrap_or("") {
                        "held" if !said_held => {
                            said_held = true;
                            log.note("waiting for your Accept in Nexus (this server's token asks for jobs as tickets)").await?;
                        }
                        "finished" | "failed" | "cancelled" => break job,
                        _ => {}
                    }
                }
                _ = tokio::time::sleep_until(deadline), if stop_reason.is_none() => {
                    stop_reason = Some("the job ran past its timeout");
                    if let Err(e) = nexus.stop(&id).await {
                        log.note(&format!("couldn't stop Nexus job {id}: {e:#}; stop it in Nexus")).await?;
                    }
                }
                _ = cancelled.changed(), if stop_reason.is_none() => {
                    if *cancelled.borrow() {
                        stop_reason = Some("cancelled");
                        if let Err(e) = nexus.stop(&id).await {
                            log.note(&format!("couldn't stop Nexus job {id}: {e:#}; stop it in Nexus")).await?;
                        }
                    }
                }
            }
        };
        // Let the last of the log arrive.
        let _ = tokio::time::timeout(Duration::from_secs(10), follower).await;
        while let Ok(line) = rx.try_recv() {
            if marker(&line).is_none() {
                log.write(format!("{line}\n").as_bytes()).await?;
            }
        }
        log.flush().await?;

        let status = final_state["status"].as_str().unwrap_or("failed");
        let reason = final_state["reason"].as_str().unwrap_or("");
        let state = match (status, stop_reason) {
            (_, Some("cancelled")) | ("cancelled", None) => RunState::Cancelled,
            ("finished", None) if final_state["exit_code"].as_i64() == Some(0) => RunState::Success,
            _ => RunState::Failure,
        };
        if !reason.is_empty() {
            log.note(&format!("Nexus: {reason}")).await?;
        }

        // Artifacts come back with the job's outputs.
        if !def.artifacts.is_empty() && state != RunState::Cancelled {
            match self.fetch_nexus_artifacts(&nexus, &id, run.id, &q.key).await {
                Ok(0) => log.note("no files matched the artifacts").await?,
                Ok(n) => log.note(&format!("kept {n} artifact file{} from Nexus", if n == 1 { "" } else { "s" })).await?,
                Err(e) => log.note(&format!("couldn't fetch the artifacts: {e:#}")).await?,
            }
        }
        log.note(&format!("job {}", state.name())).await?;
        let error = match (state, stop_reason) {
            (RunState::Failure, Some(why)) => Some(why.to_string()),
            (RunState::Failure, None) if status != "finished" => Some(format!("Nexus job {status}{}", if reason.is_empty() { String::new() } else { format!(": {reason}") })),
            _ => None,
        };
        store.finish_job(q.id, state, error.as_deref())?;
        Ok(())
    }

    /// Unpack `artifacts/` from a Nexus job's outputs into the run's artifacts.
    async fn fetch_nexus_artifacts(&self, nexus: &Nexus, id: &str, run: i64, key: &str) -> Result<usize> {
        let resp = nexus.req(reqwest::Method::GET, &format!("/v1/jobs/{id}/outputs")).timeout(Duration::from_secs(600)).send().await?;
        if !resp.status().is_success() {
            bail!("Nexus said {}", resp.status());
        }
        let body = resp.bytes().await?.to_vec();
        let dest = self.state_dir.join("artifacts").join(run.to_string()).join(key);
        let store = self.store()?;
        let kept = tokio::task::spawn_blocking(move || -> Result<Vec<(String, u64)>> {
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(&body[..]));
            let mut kept = Vec::new();
            for entry in archive.entries()? {
                let mut entry = entry?;
                let path = entry.path()?.to_path_buf();
                // Only what the driver put under artifacts/, and nothing that climbs out.
                let Some(rel) = path.components().skip_while(|c| c.as_os_str() != "artifacts").skip(1).map(|c| c.as_os_str().to_string_lossy().into_owned()).reduce(|a, b| format!("{a}/{b}")) else {
                    continue;
                };
                // `._name` files are macOS metadata some tars add, not output.
                if rel.split('/').any(|c| c == ".." || c.is_empty() || c.starts_with("._")) || !entry.header().entry_type().is_file() {
                    continue;
                }
                let target = dest.join(&rel);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                std::fs::write(&target, &bytes)?;
                kept.push((rel, bytes.len() as u64));
            }
            Ok(kept)
        })
        .await??;
        for (path, size) in &kept {
            store.add_artifact(run, key, path, *size)?;
        }
        Ok(kept.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_and_snapshots() {
        assert!(matches!(marker("##neutrino[start 2]"), Some(Marker::Start(2))));
        assert!(matches!(marker("##neutrino[end 2 0]"), Some(Marker::End(2, Some(0)))));
        assert!(matches!(marker("##neutrino[end 1 timeout]"), Some(Marker::End(1, None))));
        assert!(matches!(marker("##neutrino[skip 3]"), Some(Marker::Skip(3))));
        assert!(marker("plain output").is_none());

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "x").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("b.txt"), "b").unwrap();
        let (one, d1, n) = snapshot(dir.path()).unwrap();
        assert_eq!(n, 2, ".git stays out");
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(dir.path().join("b.txt"), "b").unwrap();
        let (two, d2, _) = snapshot(dir.path()).unwrap();
        assert_eq!((d1, one.len()), (d2, two.len()), "the same files make the same snapshot, whatever their times");
    }
}
