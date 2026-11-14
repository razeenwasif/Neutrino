//! The working copies the daemon knows about, saved in `repos.json`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow, bail};
use neutrino_git::ThreadSafeRepository;
use neutrino_proto::Repo;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    id: String,
    path: PathBuf,
    /// Unix seconds.
    added_at: u64,
}

pub struct Registry {
    file: PathBuf,
    entries: Mutex<Vec<Entry>>,
    /// Repos opened so far, with their config file's modification time when
    /// opened; gitoxide keeps their packs mapped but reads config only once.
    open: Mutex<HashMap<String, (ThreadSafeRepository, Option<std::time::SystemTime>)>>,
}

/// Lower-case letters, digits and dashes, from a folder name.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim_end_matches(".git").chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "repo".into() } else { out }
}

impl Registry {
    pub fn load(state_dir: &Path) -> Result<Self> {
        let file = state_dir.join("repos.json");
        let entries = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| format!("reading {}", file.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
        };
        Ok(Registry { file, entries: Mutex::new(entries), open: Mutex::new(HashMap::new()) })
    }

    fn save(&self, entries: &[Entry]) -> Result<()> {
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(entries)?)?;
        std::fs::rename(&tmp, &self.file).with_context(|| format!("writing {}", self.file.display()))?;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    /// Register the repository containing `path`. Adding one twice returns the first entry.
    pub fn add(&self, path: &str) -> Result<Repo> {
        let path = Path::new(path);
        if !path.is_absolute() {
            bail!("give an absolute path, not {}", path.display());
        }
        let repo = neutrino_git::discover(path)?;
        let local = neutrino_git::local(&repo);
        let root = neutrino_git::root(&local).canonicalize().context("resolving the repository's folder")?;

        let mut entries = self.entries.lock().unwrap();
        if let Some(e) = entries.iter().find(|e| e.path == root) {
            return Ok(neutrino_git::summary(&e.id, &local)?);
        }
        let base = slug(&root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        let mut id = base.clone();
        let mut n = 2;
        while entries.iter().any(|e| e.id == id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        let added_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        entries.push(Entry { id: id.clone(), path: root, added_at });
        self.save(&entries)?;
        let stamp = config_stamp(&repo);
        self.open.lock().unwrap().insert(id.clone(), (repo, stamp));
        Ok(neutrino_git::summary(&id, &local)?)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let mut entries = self.entries.lock().unwrap();
        let before = entries.len();
        entries.retain(|e| e.id != id);
        if entries.len() == before {
            bail!("no repo called {id:?}");
        }
        self.save(&entries)?;
        self.open.lock().unwrap().remove(id);
        Ok(())
    }

    /// The open repository for `id`, opening it on first use and again
    /// whenever its config changed (a new remote, upstream or setting).
    pub fn get(&self, id: &str) -> Result<ThreadSafeRepository> {
        if let Some((r, stamp)) = self.open.lock().unwrap().get(id) {
            if config_stamp(r) == *stamp {
                return Ok(r.clone());
            }
        }
        let path = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.path.clone())
            .ok_or_else(|| anyhow!("no repo called {id:?}; add it first"))?;
        if !path.exists() {
            bail!("{} no longer exists", path.display());
        }
        let repo = neutrino_git::discover(&path)?;
        let stamp = config_stamp(&repo);
        self.open.lock().unwrap().insert(id.to_string(), (repo.clone(), stamp));
        Ok(repo)
    }

    /// Every registered repo, in the order they were added. A repo that can't
    /// be read is still listed, with the reason.
    pub fn list(&self) -> Vec<Repo> {
        let entries = self.entries.lock().unwrap().clone();
        entries
            .into_iter()
            .map(|e| {
                let read = self.get(&e.id).map_err(|err| err.to_string()).and_then(|r| {
                    neutrino_git::summary(&e.id, &neutrino_git::local(&r)).map_err(|err| err.to_string())
                });
                read.unwrap_or_else(|error| Repo {
                    id: e.id.clone(),
                    name: e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| e.id.clone()),
                    path: e.path.display().to_string(),
                    bare: false,
                    head: None,
                    error: Some(error),
                })
            })
            .collect()
    }
}

fn config_stamp(repo: &ThreadSafeRepository) -> Option<std::time::SystemTime> {
    std::fs::metadata(repo.git_dir().join("config")).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn slugs() {
        assert_eq!(slug("Neutrino"), "neutrino");
        assert_eq!(slug("my repo.git"), "my-repo");
        assert_eq!(slug("--__"), "repo");
    }
}
