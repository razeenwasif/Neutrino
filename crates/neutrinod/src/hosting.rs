//! Hosted repositories (server role): bare repos under `<state>/repos/`, each
//! with hooks that report pushes to the daemon.

use std::io::{BufRead, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use neutrino_proto::{RefUpdate, Request, Response, decode_response, encode_request};

/// Names become folder names and URL paths, so keep them plain.
pub fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 100
        && !name.starts_with(['.', '-'])
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        && !name.ends_with(".git");
    if !ok {
        bail!("{name:?} can't be a repository name: use letters, digits, '.', '_' and '-', not starting with '.' or '-'");
    }
    Ok(())
}

/// Every bare repository in the hosting folder.
pub fn scan(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "git") && p.join("HEAD").exists())
        .collect();
    out.sort();
    out
}

/// `git init --bare` a new repository whose first branch is `main`.
pub fn create(dir: &Path, name: &str, description: Option<&str>) -> Result<PathBuf> {
    check_name(name)?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{name}.git"));
    if path.exists() {
        bail!("a repository called {name} already exists");
    }
    let out = Command::new(neutrino_git::worktree::git_binary())
        .args(["init", "--bare", "--quiet", "--initial-branch=main"])
        .arg(&path)
        .output()
        .context("running git init")?;
    if !out.status.success() {
        bail!("git init failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    if let Some(d) = description.map(str::trim).filter(|d| !d.is_empty()) {
        std::fs::write(path.join("description"), format!("{d}\n"))?;
    }
    Ok(path)
}

/// `'...'` for /bin/sh, safe for any path.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// (Re)write the repo's pre- and post-receive hooks to call this daemon.
pub fn install_hooks(git_dir: &Path, exe: &Path, socket: &Path, repo_id: &str) -> Result<()> {
    let hooks = git_dir.join("hooks");
    std::fs::create_dir_all(&hooks)?;
    for kind in ["pre-receive", "post-receive"] {
        let script = format!(
            "#!/bin/sh\n# Installed by Neutrino; rewritten when neutrinod starts.\nexec {} hook {kind} --repo {} --socket {}\n",
            shell_quote(&exe.display().to_string()),
            shell_quote(repo_id),
            shell_quote(&socket.display().to_string()),
        );
        let path = hooks.join(kind);
        std::fs::write(&path, script)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    // `git push -o neutrino-override=…` needs the server to accept push options.
    let out = Command::new(neutrino_git::worktree::git_binary())
        .arg("--git-dir")
        .arg(git_dir)
        .args(["config", "receive.advertisePushOptions", "true"])
        .output()
        .context("running git config")?;
    if !out.status.success() {
        bail!("git config failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// Present in a hosted repo's git dir while it has branch rules, so its
/// pre-receive hook refuses pushes when the daemon can't check them.
pub const RULES_MARKER: &str = "neutrino-rules";

pub fn mark_rules(git_dir: &Path, has_rules: bool) -> Result<()> {
    let marker = git_dir.join(RULES_MARKER);
    if has_rules {
        std::fs::write(&marker, "This repository has branch rules; its pre-receive hook refuses pushes while neutrinod is down.\n")?;
    } else if marker.exists() {
        std::fs::remove_file(&marker)?;
    }
    Ok(())
}

fn zero(sha: &str) -> bool {
    sha.bytes().all(|b| b == b'0')
}

/// Mark the updates that aren't fast-forwards. Runs inside the hook, whose
/// environment lets git see the pushed objects before they're accepted.
fn mark_forced(updates: &mut [RefUpdate]) {
    for u in updates.iter_mut().filter(|u| !zero(&u.old) && !zero(&u.new) && u.name.starts_with("refs/heads/")) {
        let fast_forward = Command::new(neutrino_git::worktree::git_binary())
            .args(["merge-base", "--is-ancestor", &u.old, &u.new])
            .status()
            .is_ok_and(|s| s.success());
        u.forced = !fast_forward;
    }
}

/// `git push -o …` options, as git hands them to hooks.
fn push_options() -> Vec<String> {
    let count: usize = std::env::var("GIT_PUSH_OPTION_COUNT").ok().and_then(|n| n.parse().ok()).unwrap_or(0);
    (0..count).filter_map(|i| std::env::var(format!("GIT_PUSH_OPTION_{i}")).ok()).collect()
}

/// Read a hook's stdin: one `old new ref` line per update.
pub fn read_updates(input: impl BufRead) -> Vec<RefUpdate> {
    input
        .lines()
        .map_while(|l| l.ok())
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            Some(RefUpdate { old: parts.next()?.to_string(), new: parts.next()?.to_string(), name: parts.next()?.to_string(), forced: false })
        })
        .collect()
}

/// The `neutrinod hook` command, run by git inside a push. Returns the exit code:
/// non-zero from pre-receive refuses the push.
pub fn run_hook(kind: &str, repo: &str, socket: &Path) -> i32 {
    let mut updates = read_updates(std::io::stdin().lock());
    let request = match kind {
        "pre-receive" => {
            mark_forced(&mut updates);
            Request::HookPreReceive { repo: repo.into(), updates, push_options: push_options() }
        }
        "post-receive" => Request::HookPostReceive { repo: repo.into(), updates },
        other => {
            eprintln!("neutrino: unknown hook {other}");
            return 1;
        }
    };
    match call(socket, &request) {
        Ok(Response::Done { output }) => {
            if !output.is_empty() {
                eprintln!("{output}");
            }
            0
        }
        Ok(Response::Error { message }) => {
            eprintln!("neutrino: {message}");
            1
        }
        Ok(_) => 0,
        Err(e) => {
            // Hooks run in the git dir. With rules to enforce, refuse rather
            // than let a push past them; with none, don't block anything.
            let guarded = kind == "pre-receive" && Path::new(RULES_MARKER).exists();
            if guarded {
                eprintln!("neutrino: couldn't reach neutrinod ({e:#}), and this repository has branch rules, so the push is refused until it's back");
                1
            } else {
                eprintln!("neutrino: couldn't reach neutrinod ({e:#}); the push goes ahead without it");
                0
            }
        }
    }
}

fn call(socket: &Path, request: &Request) -> Result<Response> {
    let mut stream = UnixStream::connect(socket).with_context(|| format!("connecting to {}", socket.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.write_all(format!("{}\n", encode_request(1, request)).as_bytes())?;
    let mut line = String::new();
    std::io::BufReader::new(&stream).read_line(&mut line)?;
    let (_, response) = decode_response(line.trim()).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(check_name("nexus").is_ok());
        assert!(check_name("my-app_2.0").is_ok());
        for bad in ["", ".hidden", "-x", "a/b", "../up", "x.git", "sp ace"] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn quoting_and_updates() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        let u = read_updates(&b"000 abc refs/heads/main\n\nabc 000 refs/tags/v1\n"[..]);
        assert_eq!(u.len(), 2);
        assert_eq!(u[1].name, "refs/tags/v1");
    }

    #[test]
    fn create_and_scan() {
        let dir = tempfile::tempdir().unwrap();
        let p = create(dir.path(), "demo", Some("A demo")).unwrap();
        assert!(p.join("HEAD").exists());
        assert_eq!(std::fs::read_to_string(p.join("description")).unwrap(), "A demo\n");
        assert!(create(dir.path(), "demo", None).is_err());
        assert_eq!(scan(dir.path()), vec![p]);
    }
}
