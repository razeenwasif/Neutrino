//! Where job steps run.
//!
//! * [`Runner::Graviton`]: a Linux VM per job through `gravitond`'s Unix
//!   socket. The workspace on atlas's disk is shared into it at `/workspace`;
//!   each step is one `exec`.
//! * [`Runner::Local`]: steps as child processes in the workspace, with no
//!   isolation at all. For tests, and the start of the host executor.
//!
//! Graviton's wire types are mirrored here, not shared: it's a separate
//! project with its own release cycle, and only these few shapes matter.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use super::workspace;

/// Containers Neutrino owns are named with this prefix.
pub const CONTAINER_PREFIX: &str = "ntr-";
/// Where the workspace appears inside a container.
pub const WORKSPACE: &str = "/workspace";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenExport {
    pub size_bytes: u64,
    pub has_vm_state: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredContainer {
    pub id: String,
    pub name: String,
    pub workdir: String,
}

/// What a job's container needs.
#[derive(Debug, Clone)]
pub struct JobBox {
    pub name: String,
    pub image: String,
    pub cpus: u32,
    pub memory_mib: u64,
    pub workspace: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owned {
    pub id: String,
    pub name: String,
    pub running: bool,
}

#[derive(Clone)]
pub enum Runner {
    Graviton(Graviton),
    Local(Local),
}

impl Runner {
    pub fn name(&self) -> &'static str {
        match self {
            Runner::Graviton(_) => "graviton",
            Runner::Local(_) => "local",
        }
    }

    pub async fn ping(&self) -> Result<()> {
        match self {
            Runner::Graviton(g) => g.ping().await,
            Runner::Local(_) => Ok(()),
        }
    }

    /// Start the job's container. Returns its id.
    pub async fn start(&self, b: &JobBox) -> Result<String> {
        match self {
            Runner::Graviton(g) => g.start(b).await,
            Runner::Local(l) => Ok(l.start(b)),
        }
    }

    /// Start a service container with its image's own command; returns its
    /// id and the address the job reaches it at.
    pub async fn start_service(&self, b: &ServiceBox) -> Result<(String, String)> {
        match self {
            Runner::Graviton(g) => g.start_service(b).await,
            Runner::Local(_) => bail!("service containers need Graviton; this server runs steps without containers"),
        }
    }

    /// Run `script` with `sh -e` in `dir` (relative to the workspace); output
    /// goes to `out` as it comes. Returns the exit code.
    pub async fn exec(&self, container: &str, dir: &str, script: &str, env: &[(String, String)], out: mpsc::Sender<Vec<u8>>) -> Result<i32> {
        match self {
            Runner::Graviton(g) => g.exec(container, dir, script, env, out).await,
            Runner::Local(l) => l.exec(container, dir, script, env, out).await,
        }
    }

    /// Stop a container (what's running in it dies); fine if it already stopped.
    pub async fn stop(&self, container: &str) {
        match self {
            Runner::Graviton(g) => {
                let _ = g.call(json!({ "method": "stop", "params": { "id": container } })).await;
            }
            Runner::Local(l) => l.stop(container),
        }
    }

    pub async fn remove(&self, container: &str) {
        match self {
            Runner::Graviton(g) => {
                let _ = g.call(json!({ "method": "remove", "params": { "id": container } })).await;
            }
            Runner::Local(l) => l.remove(container),
        }
    }

    /// Containers with Neutrino's prefix, for the reaper.
    pub async fn owned(&self) -> Result<Vec<Owned>> {
        match self {
            Runner::Graviton(g) => g.owned().await,
            Runner::Local(l) => Ok(l.owned()),
        }
    }

    pub async fn freeze_and_export(
        &self,
        container: &str,
        dest_dir: &Path,
        exit_code: Option<i32>,
        failed_step: Option<&str>,
    ) -> Result<FrozenExport> {
        match self {
            Runner::Graviton(g) => g.freeze_and_export(container, dest_dir, exit_code, failed_step).await,
            Runner::Local(l) => l.freeze_and_export(container, dest_dir, exit_code, failed_step).await,
        }
    }

    pub async fn restore_frozen(&self, bundle_dir: &Path, name: &str, ephemeral: bool) -> Result<RestoredContainer> {
        match self {
            Runner::Graviton(g) => g.restore_frozen(bundle_dir, name, ephemeral).await,
            Runner::Local(l) => l.restore_frozen(bundle_dir, name).await,
        }
    }

    pub async fn save_warm_template(&self, key: &str, container: &str) -> Result<()> {
        match self {
            Runner::Graviton(g) => g.save_warm_template(key, container).await,
            Runner::Local(l) => l.save_warm_template(key, container),
        }
    }

    pub async fn start_from_warm_template(&self, key: &str, b: &JobBox) -> Result<String> {
        match self {
            Runner::Graviton(g) => g.start_from_warm_template(key, b).await,
            Runner::Local(l) => l.start_from_warm_template(key, b),
        }
    }
}

/// The step's command line: `cd` into the folder, then the script under `sh -e`.
fn shell_script(root: &str, dir: &str, script: &str) -> Result<String> {
    if dir.contains('\0') {
        bail!("invalid working-directory");
    }
    let trimmed = dir.trim();
    if !trimmed.is_empty() {
        if trimmed.starts_with('/') {
            bail!("working-directory must be relative to workspace: {dir:?}");
        }
        for c in std::path::Path::new(trimmed).components() {
            if matches!(c, std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)) {
                bail!("working-directory escapes workspace: {dir:?}");
            }
        }
    }
    let sub = trimmed.trim_matches('/');
    let path = if sub.is_empty() { root.to_string() } else { format!("{root}/{sub}") };
    Ok(format!("cd '{}' || exit 1\nset -e\n{script}\n", path.replace('\'', r"'\''")))
}

// ---------------------------------------------------------------------------
// Graviton
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Graviton {
    pub socket: PathBuf,
}

#[derive(Serialize)]
struct RunSpec<'a> {
    image: &'a str,
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<Vec<&'a str>>,
    env: Vec<String>,
    cpus: u32,
    memory_mib: u64,
    ports: Vec<String>,
    volumes: Vec<String>,
}

#[derive(Deserialize)]
struct Container {
    id: String,
    name: String,
    status: String,
    #[serde(default)]
    ip_address: Option<String>,
}

/// A service container to start beside a job.
#[derive(Debug, Clone)]
pub struct ServiceBox {
    pub name: String,
    pub image: String,
    pub env: Vec<(String, String)>,
    pub cpus: u32,
    pub memory_mib: u64,
}

/// `$GRAVITON_SOCKET`, else where Graviton.app's daemon listens.
pub fn graviton_socket() -> PathBuf {
    if let Some(s) = std::env::var_os("GRAVITON_SOCKET") {
        return PathBuf::from(s);
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Application Support/Graviton/gravitond.sock")
}

impl Graviton {
    async fn connect(&self) -> Result<UnixStream> {
        tokio::time::timeout(Duration::from_secs(5), UnixStream::connect(&self.socket))
            .await
            .map_err(|_| anyhow!("gravitond didn't answer"))?
            .with_context(|| format!("can't reach gravitond at {} (is Graviton running?)", self.socket.display()))
    }

    /// One request, one answer. Graviton answers `{"type": …, "data": …}`.
    pub async fn call(&self, request: Value) -> Result<Value> {
        let stream = self.connect().await?;
        let (read, mut write) = stream.into_split();
        write.write_all(format!("{request}\n").as_bytes()).await?;
        let mut lines = BufReader::new(read).lines();
        // Pulling an image can take minutes before `run` answers.
        let line = tokio::time::timeout(Duration::from_secs(30 * 60), lines.next_line())
            .await
            .map_err(|_| anyhow!("gravitond took too long to answer"))??
            .ok_or_else(|| anyhow!("gravitond closed the connection"))?;
        let v: Value = serde_json::from_str(&line).context("reading gravitond's answer")?;
        if v["type"] == "error" {
            bail!("{}", v["data"]["message"].as_str().unwrap_or("gravitond failed"));
        }
        Ok(v)
    }

    async fn ping(&self) -> Result<()> {
        let v = self.call(json!({ "method": "ping" })).await?;
        if v["type"] != "pong" {
            bail!("gravitond answered {}", v["type"]);
        }
        Ok(())
    }

    async fn start(&self, b: &JobBox) -> Result<String> {
        let spec = RunSpec {
            image: &b.image,
            name: &b.name,
            // Idle until the steps come; sleep's `infinity` isn't in every image's sleep.
            command: Some(vec!["sh", "-c", "while :; do sleep 3600; done"]),
            env: vec!["CI=true".into()],
            cpus: b.cpus,
            memory_mib: b.memory_mib,
            ports: vec![],
            volumes: vec![format!("{}:{WORKSPACE}", b.workspace.display())],
        };
        let v = self.call(json!({ "method": "run", "params": spec })).await?;
        let c: Container = serde_json::from_value(v["data"].clone()).context("reading the new container")?;
        Ok(c.id)
    }

    async fn start_service(&self, b: &ServiceBox) -> Result<(String, String)> {
        let spec = RunSpec {
            image: &b.image,
            name: &b.name,
            command: None,
            env: b.env.iter().map(|(k, v)| format!("{k}={v}")).collect(),
            cpus: b.cpus,
            memory_mib: b.memory_mib,
            ports: vec![],
            volumes: vec![],
        };
        let v = self.call(json!({ "method": "run", "params": spec })).await?;
        let c: Container = serde_json::from_value(v["data"].clone()).context("reading the service container")?;
        if let Some(ip) = c.ip_address.filter(|ip| !ip.is_empty()) {
            return Ok((c.id, ip));
        }
        // The address arrives once the guest's network is up.
        for _ in 0..80 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let v = self.call(json!({ "method": "list_containers" })).await?;
            let list: Vec<Container> = serde_json::from_value(v["data"].clone()).unwrap_or_default();
            if let Some(found) = list.into_iter().find(|x| x.id == c.id) {
                if found.status != "running" {
                    bail!("service {} stopped as it started", b.name);
                }
                if let Some(ip) = found.ip_address.filter(|ip| !ip.is_empty()) {
                    return Ok((c.id, ip));
                }
            }
        }
        self.call(json!({ "method": "stop", "params": { "id": c.id } })).await.ok();
        bail!("service {} never got a network address", b.name)
    }

    async fn exec(&self, container: &str, dir: &str, script: &str, env: &[(String, String)], out: mpsc::Sender<Vec<u8>>) -> Result<i32> {
        let stream = self.connect().await?;
        let (read, mut write) = stream.into_split();
        let cmd_script = shell_script(WORKSPACE, dir, script)?;
        let request = json!({
            "method": "exec",
            "params": {
                "id": container,
                "command": ["sh", "-c", cmd_script],
                "tty": false,
                "env": env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>(),
            }
        });
        write.write_all(format!("{request}\n").as_bytes()).await?;
        let mut lines = BufReader::new(read).lines();
        while let Some(line) = lines.next_line().await? {
            let v: Value = serde_json::from_str(&line).context("reading gravitond's output")?;
            match v["type"].as_str() {
                Some("exec_output") => {
                    let bytes: Vec<u8> = serde_json::from_value(v["data"]["data"].clone()).unwrap_or_default();
                    if out.send(bytes).await.is_err() {
                        break;
                    }
                }
                Some("exec_exited") => return Ok(v["data"]["code"].as_i64().unwrap_or(-1) as i32),
                Some("error") => bail!("{}", v["data"]["message"].as_str().unwrap_or("exec failed")),
                _ => {}
            }
        }
        bail!("the container stopped before the step finished")
    }

    async fn owned(&self) -> Result<Vec<Owned>> {
        let v = self.call(json!({ "method": "list_containers" })).await?;
        let list: Vec<Container> = serde_json::from_value(v["data"].clone()).context("reading containers")?;
        Ok(list
            .into_iter()
            .filter(|c| c.name.starts_with(CONTAINER_PREFIX))
            .map(|c| Owned { running: c.status == "running", id: c.id, name: c.name })
            .collect())
    }

    async fn freeze_and_export(
        &self,
        container: &str,
        dest_dir: &Path,
        exit_code: Option<i32>,
        failed_step: Option<&str>,
    ) -> Result<FrozenExport> {
        let v = self
            .call(json!({
                "method": "freeze_and_export",
                "params": {
                    "id": container,
                    "dest_bundle_dir": dest_dir,
                    "keep_paused": false,
                    "exit_code": exit_code,
                    "failed_step": failed_step,
                }
            }))
            .await?;
        Ok(FrozenExport {
            size_bytes: v["data"]["size_bytes"].as_u64().unwrap_or(0),
            has_vm_state: v["data"]["has_vm_state"].as_bool().unwrap_or(false),
        })
    }

    async fn restore_frozen(&self, bundle_dir: &Path, name: &str, ephemeral: bool) -> Result<RestoredContainer> {
        let v = self
            .call(json!({
                "method": "restore_frozen",
                "params": {
                    "bundle_dir": bundle_dir,
                    "name": name,
                    "ephemeral": ephemeral,
                }
            }))
            .await?;
        let id = v["data"]["container"]["id"].as_str().context("missing container.id")?.to_string();
        let cname = v["data"]["container"]["name"].as_str().context("missing container.name")?.to_string();
        let workdir = v["data"]["workdir"].as_str().unwrap_or(WORKSPACE).to_string();
        Ok(RestoredContainer { id, name: cname, workdir })
    }

    async fn save_warm_template(&self, key: &str, container: &str) -> Result<()> {
        let _ = self
            .call(json!({
                "method": "save_warm_template",
                "params": {
                    "key": key,
                    "container_id": container,
                }
            }))
            .await?;
        Ok(())
    }

    async fn start_from_warm_template(&self, key: &str, b: &JobBox) -> Result<String> {
        let v = self
            .call(json!({
                "method": "run_from_warm_template",
                "params": {
                    "key": key,
                    "name": b.name,
                    "workspace": b.workspace,
                    "cpus": b.cpus,
                    "memory_mib": b.memory_mib,
                    "env": ["CI=true"],
                }
            }))
            .await?;
        let c: Container = serde_json::from_value(v["data"].clone()).context("reading the warm container")?;
        Ok(c.id)
    }
}

// ---------------------------------------------------------------------------
// Local
// ---------------------------------------------------------------------------

/// Steps as processes on this Mac, each in its own process group so a stop
/// takes the whole tree.
#[derive(Clone, Default)]
pub struct Local {
    /// Container name → its workspace and the process groups running in it.
    boxes: Arc<Mutex<HashMap<String, (PathBuf, Vec<i32>)>>>,
    /// Saved warm template keys (Wave 4B local test harness mirror).
    warm_templates: Arc<Mutex<HashSet<String>>>,
}

impl Local {
    fn start(&self, b: &JobBox) -> String {
        self.boxes.lock().unwrap().insert(b.name.clone(), (b.workspace.clone(), Vec::new()));
        b.name.clone()
    }

    async fn exec(&self, container: &str, dir: &str, script: &str, env: &[(String, String)], out: mpsc::Sender<Vec<u8>>) -> Result<i32> {
        let workspace = self.boxes.lock().unwrap().get(container).map(|(w, _)| w.clone()).ok_or_else(|| anyhow!("container {container} is gone"))?;
        let root = workspace.display().to_string();
        let cmd_script = shell_script(&root, dir, script)?;
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(cmd_script)
            .current_dir(&workspace)
            .env_clear()
            .env("CI", "true")
            .env("PATH", host_path())
            .env("NEUTRINO_WORKSPACE", &root);
        for key in ["HOME", "USER", "TMPDIR", "LANG", "LC_ALL"] {
            if let Some(val) = std::env::var_os(key) {
                cmd.env(key, val);
            }
        }
        cmd.envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        let mut child = cmd.spawn().context("starting sh")?;
        let pid = child.id().map(|p| p as i32).unwrap_or(0);
        if let Some((_, pids)) = self.boxes.lock().unwrap().get_mut(container) {
            pids.push(pid);
        }
        let pump = |r: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>, out: mpsc::Sender<Vec<u8>>| async move {
            use tokio::io::AsyncReadExt;
            let Some(mut r) = r else { return };
            let mut buf = vec![0u8; 8192];
            while let Ok(n) = r.read(&mut buf).await {
                if n == 0 || out.send(buf[..n].to_vec()).await.is_err() {
                    break;
                }
            }
        };
        let a = tokio::spawn(pump(child.stdout.take().map(|s| Box::new(s) as _), out.clone()));
        let b = tokio::spawn(pump(child.stderr.take().map(|s| Box::new(s) as _), out));
        let status = child.wait().await?;
        let _ = tokio::join!(a, b);
        if let Some((_, pids)) = self.boxes.lock().unwrap().get_mut(container) {
            pids.retain(|p| *p != pid);
        }
        use std::os::unix::process::ExitStatusExt;
        Ok(status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0)))
    }

    fn stop(&self, container: &str) {
        let pids = self.boxes.lock().unwrap().get(container).map(|(_, p)| p.clone()).unwrap_or_default();
        for pid in pids {
            // The whole group: the shell and everything it started.
            unsafe {
                libc_kill(-pid, 9);
            }
        }
    }

    fn remove(&self, container: &str) {
        self.stop(container);
        self.boxes.lock().unwrap().remove(container);
    }

    fn owned(&self) -> Vec<Owned> {
        self.boxes.lock().unwrap().keys().map(|n| Owned { id: n.clone(), name: n.clone(), running: true }).collect()
    }

    /// For tests: the workspace a container uses.
    pub fn workspace(&self, container: &str) -> Option<PathBuf> {
        self.boxes.lock().unwrap().get(container).map(|(w, _)| w.clone())
    }

    async fn freeze_and_export(
        &self,
        container: &str,
        dest_dir: &Path,
        exit_code: Option<i32>,
        failed_step: Option<&str>,
    ) -> Result<FrozenExport> {
        let src_ws = self.workspace(container).ok_or_else(|| anyhow!("container {container} is gone"))?;
        let dest = dest_dir.to_path_buf();
        let cname = container.to_string();
        let step_owned = failed_step.map(String::from);
        let size_bytes = tokio::task::spawn_blocking(move || -> Result<u64> {
            std::fs::create_dir_all(&dest)?;
            workspace::clone(&src_ws, &dest.join("workspace"))?;
            let manifest = json!({
                "version": 1,
                "source_container_name": cname,
                "workdir": WORKSPACE,
                "exit_code": exit_code,
                "failed_step": step_owned,
            });
            std::fs::write(dest.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
            Ok(dir_bytes(&dest))
        })
        .await??;
        Ok(FrozenExport { size_bytes, has_vm_state: false })
    }

    async fn restore_frozen(&self, bundle_dir: &Path, name: &str) -> Result<RestoredContainer> {
        if !bundle_dir.join("manifest.json").is_file() {
            bail!("no frozen snapshot manifest at {}", bundle_dir.display());
        }
        let ws = bundle_dir.join("workspace");
        let mounted = if ws.is_dir() { ws } else { bundle_dir.to_path_buf() };
        self.boxes.lock().unwrap().insert(name.to_string(), (mounted, Vec::new()));
        Ok(RestoredContainer {
            id: name.to_string(),
            name: name.to_string(),
            workdir: WORKSPACE.into(),
        })
    }

    fn save_warm_template(&self, key: &str, container: &str) -> Result<()> {
        if !self.boxes.lock().unwrap().contains_key(container) {
            bail!("container {container} is gone");
        }
        self.warm_templates.lock().unwrap().insert(key.to_string());
        Ok(())
    }

    fn start_from_warm_template(&self, key: &str, b: &JobBox) -> Result<String> {
        if !self.warm_templates.lock().unwrap().contains(key) {
            bail!("warm template {key} not found");
        }
        Ok(self.start(b))
    }
}

fn dir_bytes(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries.flatten().map(|e| dir_bytes(&e.path())).sum()
}

/// A launch agent's PATH is short; steps expect the usual tool folders.
fn host_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs: Vec<String> = ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin"].iter().map(|s| s.to_string()).collect();
    for d in [".cargo/bin", ".bun/bin", ".local/bin", ".deno/bin"] {
        dirs.push(format!("{home}/{d}"));
    }
    let current = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin".into());
    for d in current.split(':') {
        if !dirs.iter().any(|x| x == d) {
            dirs.push(d.to_string());
        }
    }
    dirs.join(":")
}

unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::shell_script;

    #[test]
    fn shell_script_rejects_traversal_and_absolute_paths() {
        assert!(shell_script("/workspace", "apps/Mac", "echo ok").is_ok());
        assert!(shell_script("/workspace", "", "echo ok").is_ok());
        for bad in ["/tmp", "../escape", "apps/../../etc", "sub\0dir"] {
            assert!(shell_script("/workspace", bad, "echo nope").is_err(), "expected error for {bad:?}");
        }
    }
}

