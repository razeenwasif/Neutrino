//! Database snapshots kept off the server.
//!
//! The server writes a snapshot of its database every night. On its own disk
//! that protects against a bad change, not against losing the disk. So a
//! workstation started with `--copy-backups-from HOST` asks the server over
//! SSH which snapshots it has (`ntr backups --paths`), copies the ones it's
//! missing into `<state>/server-backups/`, checks each is an SQLite file, and
//! keeps the newest few. It looks once an hour and when it starts, so a laptop
//! that was away catches up when it's back.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use neutrino_proto::{BackupFile, BackupList};
use serde::{Deserialize, Serialize};

/// Copies kept on the workstation.
pub(crate) const KEEP: usize = 14;
const STATUS_FILE: &str = "status.json";

/// Where a workstation keeps its copies.
pub(crate) fn copies_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("server-backups")
}

/// A snapshot's file name: `neutrino-<date>-<time>.db`, nothing that could
/// climb out of a folder or need quoting.
pub(crate) fn snapshot_name(name: &str) -> bool {
    name.starts_with("neutrino-") && name.ends_with(".db") && name.len() < 80 && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// The snapshots in a folder, newest first.
pub(crate) fn list(dir: &Path) -> Vec<BackupFile> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<BackupFile> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !snapshot_name(&name) {
                return None;
            }
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
            Some(BackupFile { path: e.path().display().to_string(), name, size: meta.len(), modified })
        })
        .collect();
    // The names sort by when they were taken.
    files.sort_by(|a, b| b.name.cmp(&a.name));
    files
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Status {
    #[serde(default)]
    last_copy: Option<i64>,
    #[serde(default)]
    last_error: Option<String>,
}

fn load(dir: &Path) -> Status {
    std::fs::read(dir.join(STATUS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(dir: &Path, s: &Status) {
    let _ = ensure_private_dir(dir);
    let _ = write_private(&dir.join(STATUS_FILE), &serde_json::to_vec_pretty(s).expect("serialises"));
}

fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn set_private_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        f.write_all(data)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(path)?;
        f.write_all(data)
    }
}

/// What a workstation shows for `ntr backups`.
pub(crate) fn workstation_list(state_dir: &Path, source: Option<&str>) -> BackupList {
    let dir = copies_dir(state_dir);
    let s = load(&dir);
    BackupList { dir: dir.display().to_string(), files: list(&dir), source: source.map(str::to_string), last_copy: s.last_copy, last_error: s.last_error }
}

/// Where snapshots come from.
pub(crate) trait Source {
    /// Full paths of the snapshots there, any order.
    fn paths(&self) -> Result<Vec<String>>;
    /// Copy one to `to`.
    fn fetch(&self, path: &str, to: &Path) -> Result<()>;
}

/// A server reached over SSH, with its `ntr` in `~/.neutrino/bin` (or
/// `$NEUTRINO_SERVER_NTR`).
pub(crate) struct Ssh {
    pub host: String,
}

fn ssh_opts() -> [&'static str; 12] {
    [
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=15",
        "-o",
        "ForwardAgent=no",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "PermitLocalCommand=no",
        "-o",
        "StrictHostKeyChecking=yes",
    ]
}

fn check_host(host: &str) -> Result<()> {
    if host.is_empty() || host.starts_with('-') || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        bail!("invalid SSH host {host:?}");
    }
    Ok(())
}

impl Source for Ssh {
    fn paths(&self) -> Result<Vec<String>> {
        check_host(&self.host)?;
        let ntr = std::env::var("NEUTRINO_SERVER_NTR").unwrap_or_else(|_| ".neutrino/bin/ntr".into());
        let out = Command::new("ssh")
            .args(ssh_opts())
            .arg("--")
            .arg(&self.host)
            .arg(format!("{ntr} backups --paths"))
            .stdin(std::process::Stdio::null())
            .output()
            .context("running ssh")?;
        if !out.status.success() {
            bail!("asking {} for its snapshots failed: {}", self.host, String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
    }

    fn fetch(&self, path: &str, to: &Path) -> Result<()> {
        check_host(&self.host)?;
        let out = Command::new("scp")
            .args(["-q", "-p"])
            .args(ssh_opts())
            .arg("--")
            .arg(format!("{}:{path}", self.host))
            .arg(to)
            .stdin(std::process::Stdio::null())
            .output()
            .context("running scp")?;
        if !out.status.success() {
            bail!("copying {path} from {} failed: {}", self.host, String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }
}

/// Copy the snapshots `source` has that `dir` doesn't (the newest `KEEP`),
/// then drop all but the newest `KEEP` here. Returns how many arrived.
pub(crate) fn copy_from(source: &dyn Source, dir: &Path) -> Result<usize> {
    ensure_private_dir(dir).with_context(|| format!("creating {}", dir.display()))?;
    let result = (|| -> Result<usize> {
        let mut wanted: Vec<(String, String)> = source
            .paths()?
            .into_iter()
            .filter_map(|p| {
                let name = p.rsplit('/').next()?.to_string();
                // A path with spaces or quotes would need quoting for scp; snapshots never have them.
                (snapshot_name(&name) && p.starts_with('/') && !p.chars().any(|c| c.is_whitespace() || "'\"\\;$`".contains(c))).then_some((name, p))
            })
            .collect();
        wanted.sort_by(|a, b| b.0.cmp(&a.0));
        wanted.truncate(KEEP);
        let mut copied = 0;
        for (name, path) in wanted {
            let dest = dir.join(&name);
            if dest.exists() {
                continue;
            }
            let part = dir.join(format!("{name}.part"));
            let _ = std::fs::remove_file(&part);
            source.fetch(&path, &part)?;
            let mut head = [0u8; 16];
            let looks_right = std::fs::File::open(&part).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head)).is_ok() && &head == b"SQLite format 3\0";
            if !looks_right {
                let _ = std::fs::remove_file(&part);
                bail!("{path} on the server isn't an SQLite database; not keeping it");
            }
            set_private_file(&part);
            std::fs::rename(&part, &dest)?;
            set_private_file(&dest);
            copied += 1;
        }
        for old in list(dir).into_iter().skip(KEEP) {
            let _ = std::fs::remove_file(old.path);
        }
        Ok(copied)
    })();
    let mut s = load(dir);
    match &result {
        Ok(_) => {
            s.last_copy = Some(neutrino_store::now());
            s.last_error = None;
        }
        Err(e) => s.last_error = Some(format!("{e:#}")),
    }
    save(dir, &s);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Snapshots in a local folder.
    struct Folder(PathBuf);

    impl Source for Folder {
        fn paths(&self) -> Result<Vec<String>> {
            Ok(std::fs::read_dir(&self.0)?.flatten().map(|e| e.path().display().to_string()).collect())
        }
        fn fetch(&self, path: &str, to: &Path) -> Result<()> {
            std::fs::copy(path, to)?;
            Ok(())
        }
    }

    fn sqlite(path: &Path) {
        let mut b = b"SQLite format 3\0".to_vec();
        b.extend([0u8; 100]);
        std::fs::write(path, b).unwrap();
    }

    #[test]
    fn copies_what_is_missing_and_keeps_the_newest() {
        let tmp = tempfile::tempdir().unwrap();
        let (server, here) = (tmp.path().join("server"), tmp.path().join("here"));
        std::fs::create_dir(&server).unwrap();
        for day in 1..=20 {
            sqlite(&server.join(format!("neutrino-2026-11-{day:02}-0330.db")));
        }
        std::fs::write(server.join("notes.txt"), "not a snapshot").unwrap();
        let src = Folder(server.clone());

        assert_eq!(copy_from(&src, &here).unwrap(), KEEP);
        let names: Vec<String> = list(&here).into_iter().map(|f| f.name).collect();
        assert_eq!(names.len(), KEEP);
        assert_eq!(names[0], "neutrino-2026-11-20-0330.db");
        assert_eq!(names[KEEP - 1], "neutrino-2026-11-07-0330.db");

        // Nothing new: nothing copied. A new night: one, and the oldest goes.
        assert_eq!(copy_from(&src, &here).unwrap(), 0);
        sqlite(&server.join("neutrino-2026-11-21-0330.db"));
        assert_eq!(copy_from(&src, &here).unwrap(), 1);
        let names: Vec<String> = list(&here).into_iter().map(|f| f.name).collect();
        assert_eq!((names.len(), names[0].as_str(), names[KEEP - 1].as_str()), (KEEP, "neutrino-2026-11-21-0330.db", "neutrino-2026-11-08-0330.db"));
        assert!(load(&here).last_error.is_none() && load(&here).last_copy.is_some());

        // Something that isn't a database is refused, and the failure is kept.
        std::fs::write(server.join("neutrino-2026-11-22-0330.db"), "garbage").unwrap();
        assert!(copy_from(&src, &here).is_err());
        assert!(!here.join("neutrino-2026-11-22-0330.db").exists());
        assert!(load(&here).last_error.is_some());
    }

    #[test]
    fn names() {
        assert!(snapshot_name("neutrino-2026-11-19-0341.db"));
        for bad in ["../neutrino-x.db", "neutrino-a b.db", "other.db", "neutrino-x.db.part"] {
            assert!(!snapshot_name(bad), "{bad}");
        }
    }
}
