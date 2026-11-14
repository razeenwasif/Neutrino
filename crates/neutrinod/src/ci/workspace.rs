//! What a job keeps around its workspace: caches restored into it before the
//! steps and saved from it after, and artifacts copied out of it.
//!
//! Everything is copied on atlas's own disk with `cp -c`, which clones on
//! APFS (instant, and no space until a file changes), so it works the same
//! whether steps run in a VM or as plain processes.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// How a cache restore went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restored {
    /// Exactly this key.
    Hit,
    /// The newest cache matching a restore key.
    From(String),
    Miss,
}

/// A cache's folder: `<state>/cache/<repo>/<hash of the key>`.
pub fn cache_dir(state: &Path, repo: &str, key: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let h: String = Sha256::digest(key.as_bytes()).iter().take(16).map(|b| format!("{b:02x}")).collect();
    state.join("cache").join(repo).join(h)
}

/// Copy `from` to `to` (a file or a folder), cloning where the disk can.
pub fn clone(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let run = |clone: bool| {
        let mut cmd = Command::new("/bin/cp");
        if clone {
            cmd.arg("-c");
        }
        cmd.arg("-R").arg(from).arg(to).output()
    };
    let out = run(true).context("running cp")?;
    if out.status.success() {
        return Ok(());
    }
    // Not APFS, or across volumes: copy plainly.
    let out = run(false).context("running cp")?;
    if !out.status.success() {
        bail!("copying {} failed: {}", from.display(), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

pub fn size_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if meta.is_dir() {
        std::fs::read_dir(path).map(|d| d.flatten().map(|e| size_of(&e.path())).sum()).unwrap_or(0)
    } else {
        meta.len()
    }
}

/// Put a cache's paths back into the workspace.
pub fn restore(dir: &Path, paths: &[String], workspace: &Path) -> Result<()> {
    for p in paths {
        let saved = dir.join(p);
        if !saved.exists() {
            continue;
        }
        let dest = workspace.join(p);
        if dest.exists() {
            let _ = std::fs::remove_dir_all(&dest).or_else(|_| std::fs::remove_file(&dest));
        }
        clone(&saved, &dest)?;
    }
    Ok(())
}

/// Save the workspace's paths as a cache, replacing what was there. Returns its size.
pub fn save(dir: &Path, paths: &[String], workspace: &Path) -> Result<u64> {
    let tmp = dir.with_extension("saving");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    for p in paths {
        let src = workspace.join(p);
        if src.exists() {
            clone(&src, &tmp.join(p))?;
        }
    }
    let _ = std::fs::remove_dir_all(dir);
    std::fs::rename(&tmp, dir)?;
    Ok(size_of(dir))
}

/// Every file in the workspace one of the patterns names: a file, a folder
/// (everything in it), or a pattern with `*`. `.git` is left out.
pub fn matching_files(workspace: &Path, patterns: &[String]) -> Vec<String> {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == ".git" && rel.is_empty() {
                continue;
            }
            let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path(), &path, out),
                Ok(t) if t.is_file() => out.push(path),
                _ => {}
            }
        }
    }
    let mut files = Vec::new();
    walk(workspace, "", &mut files);
    let mut out: Vec<String> = files
        .into_iter()
        .filter(|f| patterns.iter().any(|p| neutrino_ci::glob(p, f) || f.starts_with(&format!("{}/", p.trim_end_matches('/')))))
        .collect();
    out.sort();
    out
}

/// Copy artifacts out of the workspace into `dest`. Returns each file and its size.
pub fn collect(workspace: &Path, patterns: &[String], dest: &Path) -> Result<Vec<(String, u64)>> {
    let mut kept = Vec::new();
    for f in matching_files(workspace, patterns) {
        let from = workspace.join(&f);
        clone(&from, &dest.join(&f))?;
        kept.push((f.clone(), std::fs::metadata(&from).map(|m| m.len()).unwrap_or(0)));
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_and_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(ws.join("target/debug")).unwrap();
        std::fs::create_dir_all(ws.join("dist")).unwrap();
        std::fs::write(ws.join("target/debug/app"), "binary").unwrap();
        std::fs::write(ws.join("dist/a.zip"), "zip").unwrap();
        std::fs::write(ws.join("dist/notes.txt"), "n").unwrap();

        let cache = cache_dir(dir.path(), "app", "cargo-abc");
        let size = save(&cache, &["target".into(), "missing".into()], &ws).unwrap();
        assert_eq!(size, 6);
        let fresh = dir.path().join("fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        restore(&cache, &["target".into()], &fresh).unwrap();
        assert_eq!(std::fs::read_to_string(fresh.join("target/debug/app")).unwrap(), "binary");

        let out = dir.path().join("artifacts");
        let kept = collect(&ws, &["dist/*.zip".into(), "target".into()], &out).unwrap();
        assert_eq!(kept, vec![("dist/a.zip".to_string(), 3), ("target/debug/app".to_string(), 6)]);
        assert!(out.join("target/debug/app").exists() && !out.join("dist/notes.txt").exists());
    }
}
