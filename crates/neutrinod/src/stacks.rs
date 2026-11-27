//! Branch-bound stateful Graviton Compose stacks (Wave 4A).
//!
//! Tracks `.neutrino/workspace.toml` `[stacks]` configuration (`project`, `auto_switch`)
//! and orchestrates `hibernate_stack` / `thaw_stack` / `delete_stack_checkpoint` over
//! `gravitond`'s Unix socket. All calls fail open when Graviton is unavailable so git
//! checkouts remain fast and reliable.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use neutrino_proto::{BranchStackStatus, RefKind, RepoEvent};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::Daemon;
use crate::ci::runner::{Graviton, graviton_socket};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackConfig {
    pub project: String,
    pub auto_switch: bool,
}

/// Deterministic Graviton stack checkpoint key (`[a-zA-Z0-9._-]`, <= 96 chars).
pub fn branch_stack_key(project: &str, branch: &str) -> String {
    let sanitize = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    };
    let clean_proj = sanitize(project.trim());
    let clean_proj = clean_proj.trim_matches(|c| c == '.' || c == '-');
    let clean_branch = sanitize(branch.trim());
    let clean_branch = clean_branch.trim_matches(|c| c == '.' || c == '-');
    let raw = format!("{clean_proj}--{clean_branch}");
    if raw.len() <= 80 && raw == format!("{project}--{}", branch.replace('/', "-")) {
        return raw;
    }
    let mut h = Sha256::new();
    h.update(project.as_bytes());
    h.update(b"\0");
    h.update(branch.as_bytes());
    let suffix: String = h.finalize().iter().take(4).map(|b| format!("{b:02x}")).collect();
    let prefix: String = raw.chars().take(70).collect();
    format!("{prefix}--{suffix}").trim_start_matches(|c| c == '.' || c == '-').to_string()
}

/// Parse `.neutrino/workspace.toml` `[stacks]` section with fallback project name.
pub fn parse_workspace_toml(src: &str, fallback_project: &str) -> StackConfig {
    let mut project: Option<String> = None;
    let mut auto_switch = true;
    let mut in_stacks = false;

    for raw in src.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let section = line[1..line.len() - 1].trim();
            in_stacks = section == "stacks";
            continue;
        }
        if !in_stacks {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let key = k.trim();
            let val = v.trim().trim_matches('"').trim_matches('\'').trim();
            match key {
                "project" if !val.is_empty() => project = Some(val.to_string()),
                "auto_switch" | "auto-switch" => match val {
                    "false" | "0" | "no" => auto_switch = false,
                    "true" | "1" | "yes" => auto_switch = true,
                    _ => {}
                },
                _ => {}
            }
        }
    }

    StackConfig {
        project: project.unwrap_or_else(|| sanitize_project(fallback_project)),
        auto_switch,
    }
}

fn sanitize_project(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = s.trim_matches('-');
    if trimmed.is_empty() {
        "workspace".to_string()
    } else {
        trimmed.to_string()
    }
}

fn load_stack_config(repo_root: &Path, fallback_project: &str) -> StackConfig {
    let toml_path = repo_root.join(".neutrino/workspace.toml");
    if let Ok(src) = std::fs::read_to_string(&toml_path) {
        return parse_workspace_toml(&src, fallback_project);
    }
    StackConfig {
        project: sanitize_project(fallback_project),
        auto_switch: true,
    }
}

fn graviton_client() -> Graviton {
    Graviton { socket: graviton_socket() }
}

impl Daemon {
    fn repo_stack_context(&self, repo_id: &str) -> Result<(StackConfig, Vec<(String, bool)>, Option<String>)> {
        let entry = self.registry.get(repo_id)?;
        let gix_repo = neutrino_git::local(&entry);
        let root = gix_repo
            .workdir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| entry.path().to_path_buf());
        let cfg = load_stack_config(&root, repo_id);
        let branches = neutrino_git::branches(&gix_repo)?
            .into_iter()
            .filter(|b| b.kind == RefKind::Local)
            .map(|b| (b.name, b.is_head))
            .collect::<Vec<_>>();
        let head = branches.iter().find(|(_, is_head)| *is_head).map(|(n, _)| n.clone());
        Ok((cfg, branches, head))
    }

    pub async fn branch_stacks(&self, repo_id: &str) -> Result<Vec<BranchStackStatus>> {
        let (cfg, branches, _) = self.repo_stack_context(repo_id)?;
        let client = graviton_client();

        let checkpoints: HashMap<String, Value> = match client.call(json!({ "method": "list_stack_checkpoints" })).await {
            Ok(v) => v["data"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|cp| cp["key"].as_str().map(|k| (k.to_string(), cp.clone())))
                        .collect()
                })
                .unwrap_or_default(),
            Err(_) => HashMap::new(),
        };

        let active_services: Vec<String> = match client.call(json!({ "method": "list_containers" })).await {
            Ok(v) => v["data"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter(|c| {
                            c["project"].as_str() == Some(cfg.project.as_str())
                                && c["status"].as_str() == Some("running")
                        })
                        .filter_map(|c| {
                            c["service"]
                                .as_str()
                                .or_else(|| c["name"].as_str())
                                .map(str::to_string)
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };

        Ok(branches
            .into_iter()
            .map(|(branch, is_head)| {
                let key = branch_stack_key(&cfg.project, &branch);
                let cp = checkpoints.get(&key).filter(|v| v["project"].as_str() == Some(cfg.project.as_str()));
                let running = is_head && !active_services.is_empty();
                let services = if running {
                    active_services.clone()
                } else {
                    cp.and_then(|v| v["services"].as_array())
                        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
                        .unwrap_or_default()
                };
                BranchStackStatus {
                    branch,
                    key,
                    project: cfg.project.clone(),
                    has_checkpoint: cp.is_some(),
                    running,
                    services,
                    size_bytes: cp.and_then(|v| v["size_bytes"].as_u64()).unwrap_or(0),
                    updated_at: cp.and_then(|v| v["created_at"].as_i64()),
                }
            })
            .collect())
    }

    pub async fn hibernate_branch_stack(&self, repo_id: &str, branch: &str) -> Result<BranchStackStatus> {
        let (cfg, _, _) = self.repo_stack_context(repo_id)?;
        let key = branch_stack_key(&cfg.project, branch);
        let client = graviton_client();
        let v = client
            .call(json!({
                "method": "hibernate_stack",
                "params": { "key": key, "project": cfg.project }
            }))
            .await
            .context("hibernating stack in Graviton")?;
        self.watchers.notify(RepoEvent { repo: repo_id.to_string(), kind: "branch_stack_updated".into() });
        Ok(BranchStackStatus {
            branch: branch.to_string(),
            key: v["data"]["key"].as_str().unwrap_or(&key).to_string(),
            project: v["data"]["project"].as_str().unwrap_or(&cfg.project).to_string(),
            has_checkpoint: true,
            running: false,
            services: v["data"]["services"]
                .as_array()
                .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            size_bytes: v["data"]["size_bytes"].as_u64().unwrap_or(0),
            updated_at: v["data"]["created_at"].as_i64(),
        })
    }

    pub async fn thaw_branch_stack(&self, repo_id: &str, branch: &str) -> Result<BranchStackStatus> {
        let (cfg, _, _) = self.repo_stack_context(repo_id)?;
        let key = branch_stack_key(&cfg.project, branch);
        let client = graviton_client();
        let v = client
            .call(json!({
                "method": "thaw_stack",
                "params": { "key": key, "stop_conflicts": true }
            }))
            .await
            .context("thawing stack in Graviton")?;
        let services: Vec<String> = v["data"]["containers"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|c| c["service"].as_str().or_else(|| c["name"].as_str()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        self.watchers.notify(RepoEvent { repo: repo_id.to_string(), kind: "branch_stack_updated".into() });
        Ok(BranchStackStatus {
            branch: branch.to_string(),
            key: v["data"]["key"].as_str().unwrap_or(&key).to_string(),
            project: v["data"]["project"].as_str().unwrap_or(&cfg.project).to_string(),
            has_checkpoint: true,
            running: !services.is_empty(),
            services,
            size_bytes: 0,
            updated_at: None,
        })
    }

    pub async fn discard_branch_stack(&self, repo_id: &str, branch: &str) -> Result<BranchStackStatus> {
        let (cfg, _, _) = self.repo_stack_context(repo_id)?;
        let key = branch_stack_key(&cfg.project, branch);
        let client = graviton_client();
        let _ = client
            .call(json!({
                "method": "delete_stack_checkpoint",
                "params": { "key": key }
            }))
            .await
            .context("deleting stack checkpoint in Graviton")?;
        self.watchers.notify(RepoEvent { repo: repo_id.to_string(), kind: "branch_stack_updated".into() });
        Ok(BranchStackStatus {
            branch: branch.to_string(),
            key,
            project: cfg.project,
            has_checkpoint: false,
            running: false,
            services: Vec::new(),
            size_bytes: 0,
            updated_at: None,
        })
    }

    /// Best-effort automatic stack hibernate/thaw on branch switch (fails open if Graviton isn't running).
    pub async fn on_branch_switch(&self, repo_id: &str, old_branch: Option<&str>, new_branch: &str) {
        let Ok((cfg, _, _)) = self.repo_stack_context(repo_id) else {
            return;
        };
        if !cfg.auto_switch {
            self.watchers.notify(RepoEvent { repo: repo_id.to_string(), kind: "branch_switched".into() });
            return;
        }
        let client = graviton_client();
        if !graviton_socket().exists() || client.call(json!({ "method": "ping" })).await.is_err() {
            self.watchers.notify(RepoEvent { repo: repo_id.to_string(), kind: "branch_switched".into() });
            return;
        }

        if let Some(old) = old_branch
            && old != new_branch
        {
            let old_key = branch_stack_key(&cfg.project, old);
            let running = client
                .call(json!({ "method": "list_containers" }))
                .await
                .ok()
                .and_then(|v| {
                    v["data"].as_array().map(|arr| {
                        arr.iter().any(|c| {
                            c["project"].as_str() == Some(cfg.project.as_str())
                                && c["status"].as_str() == Some("running")
                        })
                    })
                })
                .unwrap_or(false);
            if running
                && let Err(e) = client
                    .call(json!({
                        "method": "hibernate_stack",
                        "params": { "key": old_key, "project": cfg.project }
                    }))
                    .await
            {
                eprintln!("neutrinod: auto-hibernate stack {old_key} skipped: {e:#}");
            }
        }

        let new_key = branch_stack_key(&cfg.project, new_branch);
        let has_cp = client
            .call(json!({ "method": "list_stack_checkpoints" }))
            .await
            .ok()
            .and_then(|v| {
                v["data"].as_array().map(|arr| {
                    arr.iter().any(|cp| {
                        cp["key"].as_str() == Some(new_key.as_str())
                            && cp["project"].as_str() == Some(cfg.project.as_str())
                    })
                })
            })
            .unwrap_or(false);
        if has_cp
            && let Err(e) = client
                .call(json!({
                    "method": "thaw_stack",
                    "params": { "key": new_key, "stop_conflicts": true }
                }))
                .await
        {
            eprintln!("neutrinod: auto-thaw stack {new_key} skipped: {e:#}");
        }

        self.watchers.notify(RepoEvent { repo: repo_id.to_string(), kind: "branch_switched".into() });
    }

    pub fn checkout_branch_sync(&self, repo_id: &str, branch: &str) -> Result<(Option<String>, String)> {
        let entry = self.registry.get(repo_id)?;
        let gix_repo = neutrino_git::local(&entry);
        let old_branch = neutrino_git::branches(&gix_repo)
            .ok()
            .and_then(|list| list.into_iter().find(|b| b.kind == RefKind::Local && b.is_head).map(|b| b.name));
        let _guard = self.writes.lock().unwrap();
        let output = neutrino_git::worktree::checkout(&gix_repo, branch)?;
        Ok((old_branch, output))
    }
}

/// Ensure valid branch string isn't empty before stack operations.
pub fn validate_branch_name(branch: &str) -> Result<()> {
    if branch.trim().is_empty() {
        bail!("branch name cannot be empty");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{branch_stack_key, parse_workspace_toml};

    #[test]
    fn parses_workspace_toml_stacks() {
        let cfg = parse_workspace_toml(
            r#"
            # workspace stacks
            [stacks]
            project = "demo-app"
            auto_switch = false
            "#,
            "FallbackRepo",
        );
        assert_eq!(cfg.project, "demo-app");
        assert!(!cfg.auto_switch);

        let fallback = parse_workspace_toml("[other]\nfoo = 1\n", "My App!");
        assert_eq!(fallback.project, "my-app");
        assert!(fallback.auto_switch);
    }

    #[test]
    fn branch_stack_key_is_safe_and_deterministic() {
        let k1 = branch_stack_key("demo-app", "feat/auth-flow");
        let k2 = branch_stack_key("demo-app", "feat/auth-flow");
        assert_eq!(k1, k2);
        assert_eq!(k1, "demo-app--feat-auth-flow");

        let weird = branch_stack_key("demo-app", "feat/a@b:c/very/long/branch/name/exceeding/safe/length/limit/extra");
        assert!(weird.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)));
        assert!(weird.len() <= 80);
    }
}
