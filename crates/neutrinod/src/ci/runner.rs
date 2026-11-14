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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

/// Containers Neutrino owns are named with this prefix.
pub const CONTAINER_PREFIX: &str = "ntr-";
/// Where the workspace appears inside a container.
pub const WORKSPACE: &str = "/workspace";

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
}

/// The step's command line: `cd` into the folder, then the script under `sh -e`.
fn shell_script(root: &str, dir: &str, script: &str) -> String {
    let dir = dir.trim_matches('/');
    let path = if dir.is_empty() { root.to_string() } else { format!("{root}/{dir}") };
    format!("cd '{}' || exit 1\nset -e\n{script}\n", path.replace('\'', r"'\''"))
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
        let request = json!({
            "method": "exec",
            "params": {
                "id": container,
                "command": ["sh", "-c", shell_script(WORKSPACE, dir, script)],
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
}

impl Local {
    fn start(&self, b: &JobBox) -> String {
        self.boxes.lock().unwrap().insert(b.name.clone(), (b.workspace.clone(), Vec::new()));
        b.name.clone()
    }

    async fn exec(&self, container: &str, dir: &str, script: &str, env: &[(String, String)], out: mpsc::Sender<Vec<u8>>) -> Result<i32> {
        let workspace = self.boxes.lock().unwrap().get(container).map(|(w, _)| w.clone()).ok_or_else(|| anyhow!("container {container} is gone"))?;
        let root = workspace.display().to_string();
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(shell_script(&root, dir, script))
            .current_dir(&workspace)
            .env("CI", "true")
            .env("PATH", host_path())
            .envs(env.iter().map(|(k, v)| (k, v)))
            .env("NEUTRINO_WORKSPACE", &root)
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

