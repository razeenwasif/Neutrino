//! Speculative Shadow CI on dirty working copies (Wave 5B).
//!
//! Configurable per working copy (`ConfigureShadow`), debounced 1.5s after editor saves
//! (`RepoEvent::kind == "worktree"`), copying dirty working copy state via scratch snapshot
//! when using an air-gapped Graviton container (`--egress offline`) or running low-priority
//! (`nice -n 10`) on the workstation without blocking commits.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use neutrino_proto::{RepoEvent, ShadowState, ShadowStatus};
use neutrino_store::now;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Daemon;
use crate::ci::runner::{Graviton, graviton_socket};

const DEBOUNCE: Duration = Duration::from_millis(1500);
const MAX_LOG_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ShadowEntry {
    repo: String,
    enabled: bool,
    cmd: String,
    use_graviton: bool,
    image: Option<String>,
    state: ShadowState,
    exit_code: Option<i32>,
    duration_ms: Option<u64>,
    updated_at: Option<i64>,
    log_tail: String,
    #[serde(skip)]
    generation: u64,
}

impl From<&ShadowEntry> for ShadowStatus {
    fn from(e: &ShadowEntry) -> Self {
        ShadowStatus {
            repo: e.repo.clone(),
            enabled: e.enabled,
            cmd: e.cmd.clone(),
            use_graviton: e.use_graviton,
            image: e.image.clone(),
            state: e.state,
            exit_code: e.exit_code,
            duration_ms: e.duration_ms,
            updated_at: e.updated_at,
            log_tail: e.log_tail.clone(),
        }
    }
}

pub struct ShadowManager {
    state_file: std::path::PathBuf,
    entries: Mutex<HashMap<String, ShadowEntry>>,
}

impl ShadowManager {
    pub fn load(state_dir: &Path) -> Self {
        let state_file = state_dir.join("shadow.json");
        let loaded: Vec<ShadowEntry> = std::fs::read(&state_file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut map = HashMap::new();
        for mut e in loaded {
            if e.state == ShadowState::Running {
                e.state = if e.enabled { ShadowState::Idle } else { ShadowState::Disabled };
            }
            map.insert(e.repo.clone(), e);
        }
        ShadowManager {
            state_file,
            entries: Mutex::new(map),
        }
    }

    fn save_locked(&self, map: &HashMap<String, ShadowEntry>) {
        let list: Vec<&ShadowEntry> = map.values().collect();
        if let Ok(bytes) = serde_json::to_vec_pretty(&list) {
            let _ = std::fs::write(&self.state_file, bytes);
        }
    }

    pub fn status(&self, repo: &str) -> ShadowStatus {
        let map = self.entries.lock().unwrap();
        map.get(repo).map(ShadowStatus::from).unwrap_or_else(|| ShadowStatus {
            repo: repo.to_string(),
            enabled: false,
            cmd: "cargo check".to_string(),
            use_graviton: false,
            image: None,
            state: ShadowState::Disabled,
            exit_code: None,
            duration_ms: None,
            updated_at: None,
            log_tail: String::new(),
        })
    }

    pub fn configure(
        &self,
        repo: &str,
        enabled: bool,
        cmd: Option<String>,
        use_graviton: Option<bool>,
        image: Option<String>,
    ) -> Result<ShadowStatus> {
        let mut map = self.entries.lock().unwrap();
        let entry = map.entry(repo.to_string()).or_insert_with(|| ShadowEntry {
            repo: repo.to_string(),
            enabled: false,
            cmd: "cargo check".to_string(),
            use_graviton: false,
            image: None,
            state: ShadowState::Disabled,
            exit_code: None,
            duration_ms: None,
            updated_at: None,
            log_tail: String::new(),
            generation: 0,
        });
        entry.enabled = enabled;
        if let Some(c) = cmd {
            let trimmed = c.trim();
            if trimmed.is_empty() {
                bail!("shadow command cannot be empty");
            }
            entry.cmd = trimmed.to_string();
        }
        if let Some(g) = use_graviton {
            entry.use_graviton = g;
        }
        if let Some(img) = image {
            let trimmed = img.trim();
            entry.image = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
        }
        entry.state = if enabled { ShadowState::Idle } else { ShadowState::Disabled };
        let st = ShadowStatus::from(&*entry);
        self.save_locked(&map);
        Ok(st)
    }

    fn bump_if_enabled(&self, repo: &str) -> Option<u64> {
        let mut map = self.entries.lock().unwrap();
        let entry = map.get_mut(repo)?;
        if !entry.enabled {
            return None;
        }
        entry.generation = entry.generation.wrapping_add(1);
        Some(entry.generation)
    }

    fn current_generation(&self, repo: &str) -> Option<u64> {
        self.entries.lock().unwrap().get(repo).filter(|e| e.enabled).map(|e| e.generation)
    }

    fn mark_running(&self, repo: &str) -> Option<(String, bool, Option<String>)> {
        let mut map = self.entries.lock().unwrap();
        let entry = map.get_mut(repo)?;
        entry.state = ShadowState::Running;
        Some((entry.cmd.clone(), entry.use_graviton, entry.image.clone()))
    }

    fn finish(&self, repo: &str, state: ShadowState, exit_code: Option<i32>, duration_ms: u64, log_tail: String) -> ShadowStatus {
        let mut map = self.entries.lock().unwrap();
        let entry = map.entry(repo.to_string()).or_insert_with(|| ShadowEntry {
            repo: repo.to_string(),
            enabled: true,
            cmd: "cargo check".to_string(),
            use_graviton: false,
            image: None,
            state,
            exit_code,
            duration_ms: Some(duration_ms),
            updated_at: Some(now()),
            log_tail: String::new(),
            generation: 0,
        });
        entry.state = state;
        entry.exit_code = exit_code;
        entry.duration_ms = Some(duration_ms);
        entry.updated_at = Some(now());
        entry.log_tail = log_tail;
        let out = ShadowStatus::from(&*entry);
        self.save_locked(&map);
        out
    }
}

fn copy_worktree_clean(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if matches!(name_str.as_ref(), ".git" | "target" | ".build" | "node_modules" | "DerivedData") {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copy_worktree_clean(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

async fn run_command_in_workdir(root: &Path, cmd: &str) -> Result<(i32, String)> {
    let out = tokio::process::Command::new("nice")
        .args(["-n", "10", "/bin/sh", "-c", cmd])
        .current_dir(root)
        .output()
        .await
        .context("running shadow command")?;
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&out.stderr));
    if combined.len() > MAX_LOG_BYTES {
        combined = combined[combined.len() - MAX_LOG_BYTES..].to_string();
    }
    Ok((out.status.code().unwrap_or(-1), combined.trim().to_string()))
}

async fn run_command_in_graviton(repo: &str, root: &Path, image: &str, cmd: &str) -> Result<(i32, String)> {
    let scratch = std::env::temp_dir().join(format!("neutrino-shadow-{repo}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let src = root.to_path_buf();
    let dst = scratch.clone();
    tokio::task::spawn_blocking(move || copy_worktree_clean(&src, &dst)).await??;

    let client = Graviton { socket: graviton_socket() };
    let cname = format!("ntr-shadow-{repo}");
    let run_res = async {
        let v = client
            .call(json!({
                "method": "run",
                "params": {
                    "image": image,
                    "name": cname,
                    "command": ["sh", "-c", "while :; do sleep 300; done"],
                    "env": ["CI=true", "NEUTRINO_SHADOW=1"],
                    "cpus": 2,
                    "memory_mib": 2048,
                    "ports": [],
                    "volumes": [format!("{}:/workspace", scratch.display())],
                }
            }))
            .await?;
        let cid = v["data"]["id"].as_str().context("missing container id")?.to_string();
        let _ = client
            .call(json!({
                "method": "set_network_policy",
                "params": {
                    "id": cid,
                    "egress": { "mode": "offline" },
                    "chaos": null
                }
            }))
            .await;

        let runner = crate::ci::runner::Runner::Graviton(client.clone());
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);
        let cmd_owned = cmd.to_string();
        let cid_clone = cid.clone();
        let exec = tokio::spawn(async move { runner.exec(&cid_clone, "", &cmd_owned, &[], tx).await });
        let mut buf = Vec::new();
        while let Some(chunk) = rx.recv().await {
            buf.extend_from_slice(&chunk);
            if buf.len() > MAX_LOG_BYTES {
                buf.drain(..buf.len() - MAX_LOG_BYTES);
            }
        }
        let code = exec.await??;
        let _ = client.call(json!({ "method": "stop", "params": { "id": cid } })).await;
        let _ = client.call(json!({ "method": "remove", "params": { "id": cid } })).await;
        Ok::<_, anyhow::Error>((code, String::from_utf8_lossy(&buf).trim().to_string()))
    }
    .await;

    let _ = std::fs::remove_dir_all(&scratch);
    run_res
}

impl Daemon {
    pub(crate) fn shadow_status(&self, repo: &str) -> Result<ShadowStatus> {
        self.registry.get(repo)?;
        Ok(self.shadow.status(repo))
    }

    pub(crate) fn configure_shadow(
        &self,
        repo: &str,
        enabled: bool,
        cmd: Option<String>,
        use_graviton: Option<bool>,
        image: Option<String>,
    ) -> Result<ShadowStatus> {
        self.registry.get(repo)?;
        let st = self.shadow.configure(repo, enabled, cmd, use_graviton, image)?;
        self.watchers.notify(RepoEvent { repo: repo.to_string(), kind: "shadow".into() });
        Ok(st)
    }

    pub(crate) async fn run_shadow_now(&self, repo: &str) -> Result<ShadowStatus> {
        let entry = self.registry.get(repo)?;
        let gix_repo = neutrino_git::local(&entry);
        let root = neutrino_git::root(&gix_repo).to_path_buf();
        let current = self.shadow.status(repo);
        let (cmd, use_graviton, image) = if let Some(pair) = self.shadow.mark_running(repo) {
            pair
        } else {
            (current.cmd, current.use_graviton, current.image)
        };
        self.watchers.notify(RepoEvent { repo: repo.to_string(), kind: "shadow".into() });

        let started = Instant::now();
        let res = if use_graviton && let Some(ref img) = image {
            run_command_in_graviton(repo, &root, img, &cmd).await
        } else {
            run_command_in_workdir(&root, &cmd).await
        };
        let duration_ms = started.elapsed().as_millis() as u64;
        let out = match res {
            Ok((code, log)) => {
                let state = if code == 0 { ShadowState::Passed } else { ShadowState::Failed };
                self.shadow.finish(repo, state, Some(code), duration_ms, log)
            }
            Err(e) => self.shadow.finish(repo, ShadowState::Failed, None, duration_ms, format!("{e:#}")),
        };
        self.watchers.notify(RepoEvent { repo: repo.to_string(), kind: "shadow".into() });
        Ok(out)
    }

    pub(crate) fn start_shadow(self: &Arc<Self>) {
        if self.options.role != crate::Role::Workstation {
            return;
        }
        let daemon = Arc::clone(self);
        let mut events = daemon.watchers.subscribe();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(ev) if ev.kind == "worktree" => {
                        if let Some(generation) = daemon.shadow.bump_if_enabled(&ev.repo) {
                            let d = Arc::clone(&daemon);
                            let repo = ev.repo;
                            tokio::spawn(async move {
                                tokio::time::sleep(DEBOUNCE).await;
                                if d.shadow.current_generation(&repo) == Some(generation) {
                                    let _ = d.run_shadow_now(&repo).await;
                                }
                            });
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shadow_config_and_execution_work() {
        let dir = tempfile::tempdir().unwrap();
        let wc = dir.path().join("wc");
        std::fs::create_dir_all(&wc).unwrap();
        std::process::Command::new("git").args(["-C", wc.to_str().unwrap(), "init", "-q"]).status().unwrap();

        let daemon = Daemon::new(&dir.path().join("state")).unwrap();
        let repo = daemon.registry.add(wc.to_str().unwrap()).unwrap();

        let cfg = daemon.configure_shadow(&repo.id, true, Some("echo shadow-ok".into()), Some(false), None).unwrap();
        assert!(cfg.enabled && cfg.state == ShadowState::Idle);

        let passed = daemon.run_shadow_now(&repo.id).await.unwrap();
        assert_eq!(passed.state, ShadowState::Passed);
        assert_eq!(passed.exit_code, Some(0));
        assert!(passed.log_tail.contains("shadow-ok"));

        daemon.configure_shadow(&repo.id, true, Some("echo shadow-boom >&2; exit 3".into()), Some(false), None).unwrap();
        let failed = daemon.run_shadow_now(&repo.id).await.unwrap();
        assert_eq!(failed.state, ShadowState::Failed);
        assert_eq!(failed.exit_code, Some(3));
        assert!(failed.log_tail.contains("shadow-boom"));
    }
}
