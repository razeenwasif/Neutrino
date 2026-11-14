//! Pull requests and branch rules (server role).
//!
//! The store keeps the review; git keeps the code. Each version of a pull
//! request is pinned with `refs/pull/<n>/head`, so force-pushes don't lose the
//! commits a note was written against.

use std::path::Path;

use anyhow::{Context, Result, bail};
use neutrino_git::forge::{self, Identity};
use neutrino_git::gix;
use neutrino_proto::{BranchRule, MergeStrategy, PullDetail, PullState, RefUpdate, RepoEvent, Response, Side};
use neutrino_store::Store;

use crate::{Daemon, hosting};

pub const OVERRIDE_OPTION: &str = "neutrino-override=";

fn zero(sha: &str) -> bool {
    sha.bytes().all(|b| b == b'0')
}

/// Who the server commits merges as: the repo's `user.name` and `user.email`
/// when set, else the source branch's author (atlas has no identity of its own).
fn committer(repo: &gix::Repository, source_tip: gix::ObjectId) -> Result<Identity> {
    let config = repo.config_snapshot();
    if let (Some(name), Some(email)) = (config.string("user.name"), config.string("user.email")) {
        return Ok(Identity { name: name.to_string(), email: email.to_string() });
    }
    Ok(forge::author_of(repo, source_tip)?)
}

impl Daemon {
    pub(crate) fn store(&self) -> Result<&Store> {
        self.server_only()?;
        self.store.as_ref().context("the pull request database isn't open")
    }

    fn hosted(&self, id: &str) -> Result<gix::Repository> {
        Ok(neutrino_git::local(&self.registry.get(id)?))
    }

    fn pulls_changed(&self, repo: &str) {
        self.watchers.notify(RepoEvent { repo: repo.into(), kind: "pulls".into() });
    }

    /// Keep the rules marker in step with the database.
    pub(crate) fn sync_rules_marker(&self, id: &str) {
        let (Ok(store), Ok(repo)) = (self.store(), self.hosted(id)) else { return };
        let has = store.rules(id).map(|r| !r.is_empty()).unwrap_or(false);
        if let Err(e) = hosting::mark_rules(repo.git_dir(), has) {
            eprintln!("can't update the rules marker for {id}: {e:#}");
        }
    }

    pub(crate) fn create_pull(&self, id: &str, title: &str, body: &str, source: &str, target: &str) -> Result<Response> {
        let store = self.store()?;
        let repo = self.hosted(id)?;
        if source == target {
            bail!("a pull request merges one branch into another; pick two different branches");
        }
        let head = forge::branch_tip(&repo, source)?;
        let target_tip = forge::branch_tip(&repo, target)?;
        if head == target_tip || forge::merge_base(&repo, head, target_tip)? == Some(head) {
            bail!("{target} already has everything on {source}");
        }
        let base = forge::merge_base(&repo, target_tip, head)?.unwrap_or(target_tip);
        let _guard = self.writes.lock().unwrap();
        let number = store.create_pull(id, title, body, source, target, &head.to_string(), &base.to_string())?;
        forge::keep_pull_head(&repo, number, &head.to_string())?;
        drop(_guard);
        self.pulls_changed(id);
        let changed = self.changed_files(id, &base.to_string(), &head.to_string());
        self.trigger(id, &neutrino_ci::Event::PullRequest { number, source: source.into(), target: target.into(), sha: head.to_string() }, changed.as_deref());
        self.pull_detail(id, number)
    }

    pub(crate) fn pull_detail(&self, id: &str, number: u32) -> Result<Response> {
        let store = self.store()?;
        let repo = self.hosted(id)?;
        let pull = store.pull(id, number)?;
        let versions = store.versions(id, number)?;
        let last = versions.last().context("a pull request with no versions")?.clone();
        let open = pull.state == PullState::Open;

        // An open pull request compares against where the target is now;
        // a finished one against where it left the target.
        let target_tip = forge::branch_tip(&repo, &pull.target).ok();
        let base = match (open, target_tip) {
            (true, Some(t)) => t.to_string(),
            _ => last.base_sha.clone(),
        };
        let files = forge::compare(&repo, &base, &last.head_sha)?;
        let commits = forge::commits_between(&repo, &base, &last.head_sha)?;
        let mergeability = match (open, target_tip, forge::branch_tip(&repo, &pull.source).ok()) {
            (true, Some(_), Some(_)) => Some(forge::mergeability(&repo, &pull.target, &pull.source)?),
            _ => None,
        };

        // Follow each note to where its line is in the latest version.
        let old_now = match (gix::ObjectId::from_hex(base.as_bytes()), gix::ObjectId::from_hex(last.head_sha.as_bytes())) {
            (Ok(b), Ok(h)) => forge::merge_base(&repo, b, h)?.map(|m| m.to_string()).unwrap_or(base.clone()),
            _ => base.clone(),
        };
        let mut notes = store.notes(id, number)?;
        for note in &mut notes {
            let Some(v) = versions.iter().find(|v| v.version == note.version) else { continue };
            let (from, to) = match note.side {
                Side::New => (v.head_sha.as_str(), last.head_sha.as_str()),
                Side::Old => (v.base_sha.as_str(), old_now.as_str()),
            };
            note.current_line = forge::map_line(&repo, from, to, &note.path, note.line).unwrap_or(None);
        }

        Ok(Response::Pull(Box::new(PullDetail {
            checks: store.runs_for_sha(id, &last.head_sha)?,
            rule: store.rule_for(id, &pull.target)?,
            pull,
            versions,
            notes,
            mergeability,
            files,
            commits,
        })))
    }

    pub(crate) fn update_pull(&self, id: &str, number: u32, title: Option<String>, body: Option<String>, state: Option<PullState>) -> Result<Response> {
        let store = self.store()?;
        let repo = self.hosted(id)?;
        let pull = store.pull(id, number)?;
        if state == Some(PullState::Open) && pull.state == PullState::Closed {
            forge::branch_tip(&repo, &pull.source).with_context(|| format!("{} is gone, so #{number} can't be reopened", pull.source))?;
            // Catch up with pushes made while it was closed.
            self.add_version(&repo, id, number, &pull.source, &pull.target)?;
        }
        store.update_pull(id, number, title.as_deref(), body.as_deref(), state)?;
        self.pulls_changed(id);
        self.pull_detail(id, number)
    }

    pub(crate) fn add_note(&self, id: &str, number: u32, path: &str, side: Side, line: u32, text: &str) -> Result<Response> {
        let store = self.store()?;
        if line == 0 {
            bail!("lines are numbered from 1");
        }
        let version = store.versions(id, number)?.last().map(|v| v.version).context("a pull request with no versions")?;
        store.add_note(id, number, version, path, side, line, text)?;
        self.pulls_changed(id);
        self.pull_detail(id, number)
    }

    pub(crate) fn resolve_note(&self, id: &str, number: u32, note: i64, resolved: bool) -> Result<Response> {
        self.store()?.resolve_note(id, number, note, resolved)?;
        self.pulls_changed(id);
        self.pull_detail(id, number)
    }

    /// Record a new version when the source has moved since the last one.
    fn add_version(&self, repo: &gix::Repository, id: &str, number: u32, source: &str, target: &str) -> Result<Option<u32>> {
        let store = self.store()?;
        let head = forge::branch_tip(repo, source)?;
        let base = match forge::branch_tip(repo, target) {
            Ok(t) => forge::merge_base(repo, t, head)?.unwrap_or(t),
            Err(_) => head,
        };
        let added = store.add_version(id, number, &head.to_string(), &base.to_string())?;
        if added.is_some() {
            forge::keep_pull_head(repo, number, &head.to_string())?;
            let changed = self.changed_files(id, &base.to_string(), &head.to_string());
            self.trigger(id, &neutrino_ci::Event::PullRequest { number, source: source.into(), target: target.into(), sha: head.to_string() }, changed.as_deref());
        }
        Ok(added)
    }

    pub(crate) fn merge_pull(&self, id: &str, number: u32, strategy: MergeStrategy, message: Option<String>, delete_source: bool) -> Result<Response> {
        let store = self.store()?;
        let repo = self.hosted(id)?;
        let pull = store.pull(id, number)?;
        if pull.state != PullState::Open {
            bail!("#{number} is {}", if pull.state == PullState::Merged { "already merged" } else { "closed; reopen it first" });
        }
        if let Some(rule) = store.rule_for(id, &pull.target)? {
            let checks = store.runs_for_sha(id, &pull.head_sha)?;
            let missing = missing_checks(&rule.required_checks, &checks);
            if !missing.is_empty() {
                bail!("the rule for {} needs these checks to pass on the latest version first: {}", rule.pattern, missing.join(", "));
            }
        }
        let _guard = self.writes.lock().unwrap();
        // A push the hooks missed still counts as a version.
        self.add_version(&repo, id, number, &pull.source, &pull.target)?;
        let source_tip = forge::branch_tip(&repo, &pull.source)?;
        let message = message.map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).unwrap_or_else(|| match strategy {
            MergeStrategy::Squash => {
                let body = pull.body.trim();
                if body.is_empty() { format!("{} (#{number})", pull.title) } else { format!("{} (#{number})\n\n{body}", pull.title) }
            }
            _ => format!("Merge pull request #{number} from {}\n\n{}", pull.source, pull.title),
        });
        let who = committer(&repo, source_tip)?;
        let sha = forge::merge(&repo, &pull.target, &pull.source, strategy, &message, &who)?;
        store.mark_merged(id, number, &sha.to_string())?;
        // Other open pull requests may still come from the source.
        if delete_source && store.open_pulls_from(id, &pull.source)?.is_empty() {
            if let Err(e) = forge::delete_branch(&repo, &pull.source, source_tip) {
                eprintln!("merged #{number} but couldn't delete {}: {e:#}", pull.source);
            }
        }
        drop(_guard);
        // Merges don't go through the hooks, so say what moved.
        self.watchers.notify(RepoEvent { repo: id.into(), kind: "push".into() });
        self.pulls_changed(id);
        Ok(Response::Merged { sha: sha.to_string() })
    }

    /// Post-receive: new versions for pull requests whose source moved, and
    /// pull requests merged by a push (someone merged locally and pushed).
    pub(crate) fn pulls_after_push(&self, id: &str, updates: &[RefUpdate]) -> Vec<String> {
        let mut said = Vec::new();
        let (Ok(store), Ok(repo)) = (self.store(), self.hosted(id)) else { return said };
        let Ok(pulls) = store.pulls(id) else { return said };
        for u in updates.iter().filter(|u| !zero(&u.new)) {
            let Some(branch) = u.name.strip_prefix("refs/heads/") else { continue };
            for p in pulls.iter().filter(|p| p.state == PullState::Open) {
                if p.source == branch {
                    match self.add_version(&repo, id, p.number, &p.source, &p.target) {
                        Ok(Some(v)) => said.push(format!("neutrino: #{} {} is now at version {v}", p.number, p.title)),
                        Ok(None) => {}
                        Err(e) => eprintln!("can't add a version to #{}: {e:#}", p.number),
                    }
                }
                if p.target == branch {
                    let (Ok(target), Ok(source)) = (forge::branch_tip(&repo, &p.target), forge::branch_tip(&repo, &p.source)) else { continue };
                    if forge::merge_base(&repo, source, target).ok().flatten() == Some(source) && store.mark_merged(id, p.number, &target.to_string()).is_ok() {
                        said.push(format!("neutrino: #{} {} is merged", p.number, p.title));
                    }
                }
            }
        }
        if !said.is_empty() {
            self.pulls_changed(id);
        }
        said
    }

    /// Pre-receive: may these updates land? Rules refuse direct pushes,
    /// force-pushes and deletes on the branches they guard, unless the push
    /// says why with `-o neutrino-override=<reason>`.
    pub(crate) fn check_push(&self, id: &str, updates: &[RefUpdate], push_options: &[String]) -> Result<String> {
        let store = self.store()?;
        let reason = push_options.iter().find_map(|o| o.strip_prefix(OVERRIDE_OPTION)).map(str::trim).filter(|r| !r.is_empty());
        let mut broken: Vec<(String, &'static str, String)> = Vec::new();
        for u in updates {
            let Some(branch) = u.name.strip_prefix("refs/heads/") else { continue };
            let Some(rule) = store.rule_for(id, branch)? else { continue };
            if zero(&u.new) {
                if rule.block_delete {
                    broken.push((u.name.clone(), "block_delete", format!("{branch} can't be deleted")));
                }
                continue;
            }
            if zero(&u.old) {
                // Creating the branch is how it starts; nothing to protect yet.
                continue;
            }
            if u.forced && rule.block_force {
                broken.push((u.name.clone(), "block_force", format!("{branch} can't be force-pushed")));
            } else if rule.require_pull {
                broken.push((u.name.clone(), "require_pull", format!("{branch} takes changes through pull requests")));
            }
        }
        if broken.is_empty() {
            return Ok(String::new());
        }
        let Some(reason) = reason else {
            let lines: Vec<String> = broken.iter().map(|(_, _, why)| format!("  {why}")).collect();
            bail!(
                "this push breaks a branch rule:\n{}\nOpen a pull request instead, or push again with -o {OVERRIDE_OPTION}\"<why>\" to override it (the override is recorded)",
                lines.join("\n")
            );
        };
        for (name, rule, _) in &broken {
            store.record_override(id, name, rule, reason)?;
        }
        Ok(format!("neutrino: overrode {} rule{} ({reason}); recorded", broken.len(), if broken.len() == 1 { "" } else { "s" }))
    }

    pub(crate) fn set_rule(&self, id: &str, rule: &BranchRule) -> Result<Response> {
        self.hosted(id)?;
        self.store()?.set_rule(id, rule)?;
        self.sync_rules_marker(id);
        self.pulls_changed(id);
        Ok(Response::Rules(self.store()?.rules(id)?))
    }

    pub(crate) fn delete_rule(&self, id: &str, pattern: &str) -> Result<Response> {
        self.store()?.delete_rule(id, pattern)?;
        self.sync_rules_marker(id);
        self.pulls_changed(id);
        Ok(Response::Rules(self.store()?.rules(id)?))
    }
}

/// Required checks that haven't passed: no run, or a run that isn't a success.
/// A check names a workflow by its name or its file, with or without `.yml`.
pub fn missing_checks(required: &[String], runs: &[neutrino_proto::Run]) -> Vec<String> {
    required
        .iter()
        .filter(|name| {
            let matches = |r: &&neutrino_proto::Run| {
                r.name.eq_ignore_ascii_case(name) || r.workflow == **name || r.workflow.trim_end_matches(".yml").trim_end_matches(".yaml") == name.as_str()
            };
            !runs.iter().filter(matches).any(|r| r.state == neutrino_proto::RunState::Success)
        })
        .map(|name| {
            match runs.iter().find(|r| r.name.eq_ignore_ascii_case(name) || r.workflow.trim_end_matches(".yml") == name.as_str()) {
                Some(r) => format!("{name} ({})", r.state.name()),
                None => format!("{name} (hasn't run)"),
            }
        })
        .collect()
}

/// The database lives beside the rest of the server's state.
pub fn database_path(state_dir: &Path) -> std::path::PathBuf {
    state_dir.join("neutrino.db")
}
