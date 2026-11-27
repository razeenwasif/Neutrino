//! What a job keeps around its workspace: caches restored into it before the
//! steps and saved from it after, and artifacts copied out of it.
//!
//! Everything is copied on atlas's own disk with `cp -c`, which clones on
//! APFS (instant, and no space until a file changes), so it works the same
//! whether steps run in a VM or as plain processes.

use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

/// Minimum log size before compressing a finished job's log to `.log.gz`.
const MIN_COMPRESS_LOG_BYTES: u64 = 4096;

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

/// Remove a file or directory tree, making read-only directories writable first
/// (Go module caches like `.go/pkg/mod` mark directories `0555`, which causes
/// plain `remove_dir_all` to fail with `PermissionDenied`). Never follows symlinks.
pub fn remove_tree(path: &Path) -> Result<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if !meta.is_dir() {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        };
    }
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            make_writable_tree(path);
            match std::fs::remove_dir_all(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(unix)]
fn make_writable_tree(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::symlink_metadata(path) else { return };
    if meta.file_type().is_symlink() {
        return;
    }
    let mode = meta.permissions().mode();
    if meta.is_dir() {
        if mode & 0o700 != 0o700 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o700));
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            for e in entries.flatten() {
                make_writable_tree(&e.path());
            }
        }
    } else if mode & 0o200 == 0 {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o200));
    }
}

#[cfg(not(unix))]
fn make_writable_tree(_path: &Path) {}

/// Path to the gzipped companion of a job's `.log` file (`<job>.log.gz`).
pub fn log_gz_path(log: &Path) -> PathBuf {
    let mut os = log.as_os_str().to_os_string();
    os.push(".gz");
    PathBuf::from(os)
}

/// Compress a finished job's `.log` file to `.log.gz` when it is at least 4 KiB
/// and compression saves space. Atomically writes `.log.gz` before removing `.log`
/// so concurrent readers never see both missing.
pub fn compress_log(log: &Path) -> Result<bool> {
    let meta = match std::fs::metadata(log) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    if meta.len() < MIN_COMPRESS_LOG_BYTES {
        return Ok(false);
    }
    let gz = log_gz_path(log);
    let mut tmp_os = gz.as_os_str().to_os_string();
    tmp_os.push(".tmp");
    let tmp = PathBuf::from(tmp_os);

    let res = (|| -> Result<u64> {
        let mut src = BufReader::new(std::fs::File::open(log)?);
        let dst = std::fs::File::create(&tmp)?;
        let mut enc = GzEncoder::new(BufWriter::new(dst), Compression::best());
        std::io::copy(&mut src, &mut enc)?;
        let mut inner = enc.finish()?;
        inner.flush()?;
        Ok(std::fs::metadata(&tmp)?.len())
    })();

    match res {
        Ok(gz_len) if gz_len < meta.len() => {
            std::fs::rename(&tmp, &gz)?;
            let _ = std::fs::remove_file(log);
            Ok(true)
        }
        Ok(_) => {
            let _ = std::fs::remove_file(&tmp);
            Ok(false)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Read up to `limit` uncompressed bytes from `log` (or its compressed `.log.gz`
/// companion) starting at `offset`, without splitting a UTF-8 sequence.
/// Returns `(text, next_offset, at_eof)`.
pub fn read_log_chunk(log: &Path, offset: u64, limit: u64) -> Result<(String, u64, bool)> {
    if let Ok(mut f) = std::fs::File::open(log) {
        let len = f.metadata()?.len();
        let start = offset.min(len);
        f.seek(SeekFrom::Start(start))?;
        let mut buf = Vec::new();
        f.take(limit).read_to_end(&mut buf)?;
        let end = utf8_boundary(&buf);
        let next = start + end as u64;
        return Ok((String::from_utf8_lossy(&buf[..end]).into_owned(), next, next >= len));
    }
    let gz = log_gz_path(log);
    if let Ok(f) = std::fs::File::open(&gz) {
        let mut dec = GzDecoder::new(BufReader::new(f));
        let skipped = std::io::copy(&mut dec.by_ref().take(offset), &mut std::io::sink())?;
        let mut buf = Vec::new();
        dec.take(limit.saturating_add(1)).read_to_end(&mut buf)?;
        let has_more = buf.len() > limit as usize;
        if has_more {
            buf.truncate(limit as usize);
        }
        let end = utf8_boundary(&buf);
        let at_eof = !has_more && end == buf.len();
        return Ok((String::from_utf8_lossy(&buf[..end]).into_owned(), skipped + end as u64, at_eof));
    }
    Ok((String::new(), offset, true))
}

fn utf8_boundary(buf: &[u8]) -> usize {
    match std::str::from_utf8(buf) {
        Err(e) if e.error_len().is_none() && e.valid_up_to() > 0 => e.valid_up_to(),
        _ => buf.len(),
    }
}

/// Rejects absolute paths, parent traversal (`..`), and NUL bytes.
pub(crate) fn validate_rel_path(p: &str) -> Result<()> {
    if p.is_empty() || p.contains('\0') {
        anyhow::bail!("invalid relative path {p:?}");
    }
    let path = Path::new(p);
    if path.is_absolute() {
        anyhow::bail!("path must be relative: {p:?}");
    }
    for c in path.components() {
        if matches!(c, std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)) {
            anyhow::bail!("path escapes workspace: {p:?}");
        }
    }
    Ok(())
}

fn ensure_contained(base: &Path, candidate: &Path) -> Result<()> {
    let canon_base = base.canonicalize().with_context(|| format!("resolving {}", base.display()))?;
    let check_target = if candidate.exists() {
        candidate.to_path_buf()
    } else if let Some(parent) = candidate.parent() {
        if parent.exists() {
            parent.to_path_buf()
        } else {
            return Ok(());
        }
    } else {
        return Ok(());
    };
    let canon_target = check_target.canonicalize().with_context(|| format!("resolving {}", check_target.display()))?;
    if !canon_target.starts_with(&canon_base) {
        anyhow::bail!("path {} escapes {}", candidate.display(), base.display());
    }
    Ok(())
}

/// Put a cache's paths back into the workspace.
pub fn restore(dir: &Path, paths: &[String], workspace: &Path) -> Result<()> {
    for p in paths {
        validate_rel_path(p)?;
        let saved = dir.join(p);
        if !saved.exists() {
            continue;
        }
        if dir.exists() {
            ensure_contained(dir, &saved)?;
        }
        let dest = workspace.join(p);
        ensure_contained(workspace, &dest)?;
        if dest.exists() {
            let _ = remove_tree(&dest);
        }
        clone(&saved, &dest)?;
    }
    Ok(())
}

/// Save the workspace's paths as a cache, replacing what was there. Returns its size.
pub fn save(dir: &Path, paths: &[String], workspace: &Path) -> Result<u64> {
    let tmp = dir.with_extension("saving");
    let _ = remove_tree(&tmp);
    std::fs::create_dir_all(&tmp)?;
    for p in paths {
        validate_rel_path(p)?;
        let src = workspace.join(p);
        if src.exists() {
            ensure_contained(workspace, &src)?;
            clone(&src, &tmp.join(p))?;
        }
    }
    let _ = remove_tree(dir);
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

    #[cfg(unix)]
    #[test]
    fn remove_tree_cleans_read_only_directories() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let ro = dir.path().join("ws/.go/pkg/mod/example");
        std::fs::create_dir_all(&ro).unwrap();
        let file = ro.join("go.mod");
        std::fs::write(&file, "module example\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        std::fs::set_permissions(ro.parent().unwrap(), std::fs::Permissions::from_mode(0o555)).unwrap();

        remove_tree(&dir.path().join("ws")).unwrap();
        assert!(!dir.path().join("ws").exists());
    }

    #[test]
    fn compressed_logs_read_exact_offsets_and_utf8_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("build.log");
        // Mix ASCII and multi-byte UTF-8 (`▸` is 3 bytes) across > 4 KiB.
        let line = "\u{25b8} compiling crate foo v1.0.0 — ok\n";
        let full = line.repeat(200);
        std::fs::write(&log, &full).unwrap();

        assert!(compress_log(&log).unwrap());
        assert!(!log.exists(), "plain log removed after gzip");
        assert!(log_gz_path(&log).exists(), "gzipped log created");
        assert!(std::fs::metadata(log_gz_path(&log)).unwrap().len() < full.len() as u64 / 4);

        // Stream back in awkward small chunks (17 bytes) that split multi-byte chars.
        let mut rebuilt = String::new();
        let mut offset = 0u64;
        loop {
            let (text, next, eof) = read_log_chunk(&log, offset, 17).unwrap();
            assert_eq!(next - offset, text.len() as u64);
            rebuilt.push_str(&text);
            offset = next;
            if eof {
                break;
            }
        }
        assert_eq!(rebuilt, full);
        assert_eq!(offset, full.len() as u64);
    }

    #[test]
    fn cache_paths_reject_traversal_and_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&cache).unwrap();

        for bad in ["../escape", "/etc/passwd", "sub/../../escape", ""] {
            assert!(save(&cache, &[bad.into()], &ws).is_err(), "save should reject {bad}");
            assert!(restore(&cache, &[bad.into()], &ws).is_err(), "restore should reject {bad}");
        }
        #[cfg(unix)]
        {
            let outside = dir.path().join("secret");
            std::fs::write(&outside, "top-secret").unwrap();
            std::os::unix::fs::symlink(&outside, ws.join("leak")).unwrap();
            assert!(save(&cache, &["leak".into()], &ws).is_err(), "symlink escaping workspace must fail");
        }
    }
}
