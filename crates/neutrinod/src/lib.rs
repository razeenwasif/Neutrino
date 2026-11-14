//! neutrinod: Neutrino's daemon.
//!
//! On the MacBook it runs as the *workstation*: it reads the working copies you
//! register and answers the app and `ntr` over a Unix socket. The server role
//! on atlas comes in a later milestone.

mod registry;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use neutrino_git::DiffCache;
use neutrino_proto::{EngineInfo, PROTOCOL_VERSION, Request, Response, decode_request, encode_response};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

pub use registry::Registry;

/// Requests longer than this are refused.
const MAX_LINE: usize = 1 << 20;

/// Where state lives: `$NEUTRINO_HOME`, else `~/.neutrino`.
pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NEUTRINO_HOME") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".neutrino")
}

/// `$NEUTRINO_SOCKET`, else `neutrinod.sock` in the state folder.
pub fn socket_path() -> PathBuf {
    std::env::var_os("NEUTRINO_SOCKET").map(PathBuf::from).unwrap_or_else(|| state_dir().join("neutrinod.sock"))
}

pub struct Daemon {
    registry: Registry,
    diffs: DiffCache,
}

impl Daemon {
    pub fn new(state_dir: &Path) -> Result<Arc<Self>> {
        std::fs::create_dir_all(state_dir).with_context(|| format!("creating {}", state_dir.display()))?;
        Ok(Arc::new(Daemon { registry: Registry::load(state_dir)?, diffs: DiffCache::default() }))
    }

    /// Answer one request. Git work runs on the blocking pool.
    pub async fn handle(self: &Arc<Self>, request: Request) -> Response {
        let this = Arc::clone(self);
        match tokio::task::spawn_blocking(move || this.handle_blocking(request)).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => Response::Error { message: format!("{e:#}") },
            Err(e) => Response::Error { message: format!("internal error: {e}") },
        }
    }

    fn handle_blocking(&self, request: Request) -> Result<Response> {
        let repo = |id: &str| self.registry.get(id).map(|r| neutrino_git::local(&r));
        Ok(match request {
            Request::Ping => Response::Pong { protocol: PROTOCOL_VERSION },
            Request::EngineInfo => Response::Engine(EngineInfo {
                version: env!("CARGO_PKG_VERSION").into(),
                protocol: PROTOCOL_VERSION,
                role: "workstation".into(),
                repos: self.registry.len() as u32,
            }),
            Request::ListRepos => Response::Repos(self.registry.list()),
            Request::AddRepo { path } => Response::Repo(self.registry.add(&path)?),
            Request::RemoveRepo { repo } => {
                self.registry.remove(&repo)?;
                Response::Removed { repo }
            }
            Request::Branches { repo: id } => Response::Branches(neutrino_git::branches(&repo(&id)?)?),
            Request::Log(q) => Response::Log(neutrino_git::log(&repo(&q.repo)?, &q)?),
            Request::Commit { repo: id, sha } => Response::Commit(Box::new(neutrino_git::commit(&repo(&id)?, &sha)?)),
            Request::FileDiff { repo: id, sha, path, context } => {
                let context = context.unwrap_or(3).min(10_000);
                Response::FileDiff(neutrino_git::file_diff(&repo(&id)?, &sha, &path, context, &self.diffs)?)
            }
            Request::Cancel { .. } => bail!("cancel is handled by the connection"),
        })
    }
}

/// Serve one client: requests are answered as they finish, tagged with their id.
async fn connection(daemon: Arc<Daemon>, stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let (tx, mut rx) = mpsc::channel::<String>(64);
    let writer = tokio::spawn(async move {
        while let Some(mut line) = rx.recv().await {
            line.push('\n');
            if write.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });

    let in_flight: Arc<Mutex<HashMap<u64, tokio::task::AbortHandle>>> = Default::default();
    let mut lines = BufReader::new(read);
    let mut buf = String::new();
    loop {
        buf.clear();
        match (&mut lines).take(MAX_LINE as u64 + 1).read_line(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buf.len() > MAX_LINE {
            let _ = tx.send(encode_response(0, &Response::Error { message: "request too long".into() })).await;
            break;
        }
        let line = buf.trim();
        if line.is_empty() {
            continue;
        }
        let (id, request) = decode_request(line);
        let request = match request {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(encode_response(id.unwrap_or(0), &Response::Error { message: e.to_string() })).await;
                continue;
            }
        };
        let id = id.unwrap_or(0);
        if let Request::Cancel { id: target } = request {
            let task = in_flight.lock().unwrap().remove(&target);
            if let Some(task) = task {
                task.abort();
                let _ = tx.send(encode_response(target, &Response::Cancelled)).await;
            }
            continue;
        }

        let (daemon, tx2, flights) = (Arc::clone(&daemon), tx.clone(), Arc::clone(&in_flight));
        // Hold the map while spawning so the task can't finish before it's registered.
        let mut map = in_flight.lock().unwrap();
        let task = tokio::spawn(async move {
            let response = daemon.handle(request).await;
            // Only answer if no one cancelled us in the meantime.
            let still_wanted = flights.lock().unwrap().remove(&id).is_some();
            if still_wanted {
                let _ = tx2.send(encode_response(id, &response)).await;
            }
        });
        map.insert(id, task.abort_handle());
        drop(map);
    }
    drop(tx);
    let _ = writer.await;
}

/// Bind the socket, replacing a stale one, and serve until `shutdown` resolves.
pub async fn serve(daemon: Arc<Daemon>, socket: &Path, shutdown: impl std::future::Future<Output = ()>) -> Result<()> {
    if socket.exists() {
        if UnixStream::connect(socket).await.is_ok() {
            bail!("another neutrinod is already listening on {}", socket.display());
        }
        std::fs::remove_file(socket).with_context(|| format!("removing the stale socket {}", socket.display()))?;
    }
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let listener = UnixListener::bind(socket).with_context(|| format!("listening on {}", socket.display()))?;
    // Only this user may talk to the daemon.
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    }

    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                tokio::spawn(connection(Arc::clone(&daemon), stream));
            }
            _ = &mut shutdown => break,
        }
    }
    let _ = std::fs::remove_file(socket);
    Ok(())
}
