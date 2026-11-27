//! Pull requests and branch rules (server role).
//!
//! The store keeps the review; git keeps the code. Each version of a pull
//! request is pinned with `refs/pull/<n>/head`, so force-pushes don't lose the
//! commits a note was written against.

use std::path::Path;

use anyhow::{Context, Result, bail};
use neutrino_git::forge::{self, Identity};
use neutrino_git::gix;
use neutrino_proto::{
    BranchRule, MergeStrategy, PullDetail, PullState, RefUpdate, RepoEvent, Response, Side, StackSyncOutcome, StackSyncReport, VersionInterdiff,
};
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

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_pull(
        &self,
        id: &str,
        title: &str,
        body: &str,
        source: &str,
        target: &str,
        stack_id: Option<String>,
        parent_pull: Option<u32>,
    ) -> Result<Response> {
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
        let number = store.create_pull_in_stack(
            id,
            title,
            body,
            source,
            target,
            &head.to_string(),
            &base.to_string(),
            stack_id.as_deref(),
            parent_pull,
        )?;
        forge::keep_pull_head(&repo, number, &head.to_string())?;
        forge::keep_pull_version(&repo, number, 1, &head.to_string())?;
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
        let (moves, tour) = forge::analyze_compare(&repo, &base, &last.head_sha, false).unwrap_or_default();
        let stack = match &pull.stack_id {
            Some(sid) => store.stack_members(id, sid)?,
            None => Vec::new(),
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

        let checks = store.runs_for_sha(id, &last.head_sha)?;
        let receipt = forge::compute_receipt(&repo, &base, &last.head_sha).ok().map(|mut r| {
            r.verified_checks = checks
                .iter()
                .filter(|c| c.state == neutrino_proto::RunState::Success)
                .map(|c| c.name.clone())
                .collect();
            if let Ok(prov) = store.provenance_for_sha(id, &last.head_sha) {
                r.provenance_count = prov.len() as u32;
                r.device_signed_count = prov.iter().filter(|p| p.device_signature.is_some()).count() as u32;
            }
            r
        });

        Ok(Response::Pull(Box::new(PullDetail {
            checks,
            rule: store.rule_for(id, &pull.target)?,
            pull,
            versions,
            notes,
            mergeability,
            files,
            commits,
            stack,
            tour,
            moves,
            receipt,
        })))
    }

    pub(crate) fn compare_versions(&self, id: &str, number: u32, from_version: u32, to_version: u32) -> Result<Response> {
        let store = self.store()?;
        let repo = self.hosted(id)?;
        let versions = store.versions(id, number)?;
        let v_from = versions.iter().find(|v| v.version == from_version).with_context(|| format!("PR #{number} has no version {from_version}"))?;
        let v_to = versions.iter().find(|v| v.version == to_version).with_context(|| format!("PR #{number} has no version {to_version}"))?;
        let files = forge::compare_direct(&repo, &v_from.head_sha, &v_to.head_sha)?;
        let commits = forge::commits_between(&repo, &v_from.head_sha, &v_to.head_sha).unwrap_or_default();
        let (moves, tour) = forge::analyze_compare(&repo, &v_from.head_sha, &v_to.head_sha, true).unwrap_or_default();
        let mut notes = store.notes(id, number)?;
        for note in &mut notes {
            let Some(nv) = versions.iter().find(|v| v.version == note.version) else { continue };
            if note.side == Side::New {
                note.current_line = forge::map_line(&repo, &nv.head_sha, &v_to.head_sha, &note.path, note.line).unwrap_or(None);
            }
        }
        Ok(Response::VersionDiff(Box::new(VersionInterdiff {
            number,
            from_version,
            to_version,
            from_sha: v_from.head_sha.clone(),
            to_sha: v_to.head_sha.clone(),
            files,
            commits,
            notes,
            moves,
            tour,
        })))
    }

    pub(crate) fn compare_version_diff(&self, id: &str, number: u32, from_version: u32, to_version: u32, path: &str, context: u32) -> Result<Response> {
        let store = self.store()?;
        let repo = self.hosted(id)?;
        let versions = store.versions(id, number)?;
        let v_from = versions.iter().find(|v| v.version == from_version).with_context(|| format!("PR #{number} has no version {from_version}"))?;
        let v_to = versions.iter().find(|v| v.version == to_version).with_context(|| format!("PR #{number} has no version {to_version}"))?;
        Ok(Response::FileDiff(forge::compare_direct_diff(&repo, &v_from.head_sha, &v_to.head_sha, path, context, &self.diffs)?))
    }

    pub(crate) fn list_stacks(&self, id: &str) -> Result<Response> {
        self.registry.get(id)?;
        Ok(Response::Stacks(self.store()?.stacks(id)?))
    }

    pub(crate) fn sync_stack(&self, id: &str, root_pull: u32) -> Result<Response> {
        let store = self.store()?;
        let repo = self.hosted(id)?;
        let pull = store.pull(id, root_pull)?;
        let versions = store.versions(id, root_pull)?;
        let last_head = versions.last().map(|v| v.head_sha.clone()).unwrap_or_else(|| pull.head_sha.clone());
        let old_tip = gix::ObjectId::from_hex(last_head.as_bytes()).context("invalid root PR SHA")?;

        let _guard = self.writes.lock().unwrap();
        let mut outcomes = Vec::new();
        match pull.state {
            PullState::Merged => {
                let merge_sha = pull.merge_sha.as_deref().context("merged PR missing merge_sha")?;
                let new_base = gix::ObjectId::from_hex(merge_sha.as_bytes()).context("invalid merge_sha")?;
                self.restack_children(&repo, id, root_pull, old_tip, &pull.target, Some(pull.parent_pull), new_base, &committer(&repo, old_tip)?, &mut outcomes)?;
            }
            PullState::Open => {
                let current_tip = forge::branch_tip(&repo, &pull.source)?;
                let who = committer(&repo, current_tip)?;
                self.restack_children(&repo, id, root_pull, old_tip, &pull.source, None, current_tip, &who, &mut outcomes)?;
            }
            PullState::Closed => bail!("#{root_pull} is closed"),
        }
        drop(_guard);
        self.pulls_changed(id);
        Ok(Response::StackSynced(StackSyncReport { root: root_pull, updated: outcomes }))
    }

    #[allow(clippy::too_many_arguments)]
    fn restack_children(
        &self,
        repo: &gix::Repository,
        id: &str,
        parent: u32,
        old_parent_tip: gix::ObjectId,
        new_target: &str,
        new_parent_override: Option<Option<u32>>,
        new_base: gix::ObjectId,
        who: &Identity,
        outcomes: &mut Vec<StackSyncOutcome>,
    ) -> Result<()> {
        let store = self.store()?;
        let children = store.child_pulls(id, parent)?;
        for child in children {
            let Ok(child_old_tip) = forge::branch_tip(repo, &child.source) else { continue };
            let next_parent = new_parent_override.unwrap_or(Some(parent));
            let target_branch = if new_parent_override.is_some() { new_target } else { &child.target };
            match forge::restack_branch(repo, old_parent_tip, new_base, &child.source, who)? {
                Ok(new_child_tip) => {
                    store.retarget_pull(id, child.number, target_branch, next_parent, None)?;
                    let _ = self.add_version(repo, id, child.number, &child.source, target_branch)?;
                    outcomes.push(StackSyncOutcome {
                        number: child.number,
                        source: child.source.clone(),
                        target: target_branch.to_string(),
                        new_sha: Some(new_child_tip.to_string()),
                        conflict: None,
                    });
                    self.restack_children(repo, id, child.number, child_old_tip, &child.source, None, new_child_tip, who, outcomes)?;
                }
                Err(conflicts) => {
                    let msg = format!("rebase conflict in {}", conflicts.join(", "));
                    store.retarget_pull(id, child.number, target_branch, next_parent, Some(&msg))?;
                    outcomes.push(StackSyncOutcome {
                        number: child.number,
                        source: child.source,
                        target: target_branch.to_string(),
                        new_sha: None,
                        conflict: Some(msg),
                    });
                }
            }
        }
        Ok(())
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
        if let Some(version) = added {
            forge::keep_pull_head(repo, number, &head.to_string())?;
            forge::keep_pull_version(repo, number, version, &head.to_string())?;
            let changed = self.changed_files(id, &base.to_string(), &head.to_string());
            self.trigger(id, &neutrino_ci::Event::PullRequest { number, source: source.into(), target: target.into(), sha: head.to_string() }, changed.as_deref());
        }
        Ok(added)
    }

    pub(crate) fn merge_pull(
        &self,
        id: &str,
        number: u32,
        strategy: MergeStrategy,
        message: Option<String>,
        delete_source: bool,
        evidence: Option<crate::provenance::SignedEvidence>,
    ) -> Result<Response> {
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
        let target_before = forge::branch_tip(&repo, &pull.target).ok().map(|t| t.to_string());
        let sha = forge::merge(&repo, &pull.target, &pull.source, strategy, &message, &who)?;
        store.mark_merged(id, number, &sha.to_string())?;
        let mut _stack_outcomes = Vec::new();
        if let Err(e) = self.restack_children(&repo, id, number, source_tip, &pull.target, Some(pull.parent_pull), sha, &who, &mut _stack_outcomes) {
            eprintln!("merged #{number} but couldn't restack children: {e:#}");
        }
        // Other open pull requests may still come from the source (excluding already-retargeted children).
        if delete_source && store.open_pulls_from(id, &pull.source)?.is_empty() {
            if let Err(e) = forge::delete_branch(&repo, &pull.source, source_tip) {
                eprintln!("merged #{number} but couldn't delete {}: {e:#}", pull.source);
            }
        }
        drop(_guard);
        // Merges don't go through the hooks, so do what a push would: say
        // what moved, start the target branch's push workflows (a deploy on
        // main, say), and copy the repository to its mirror.
        self.watchers.notify(RepoEvent { repo: id.into(), kind: "push".into() });
        self.pulls_changed(id);
        self.maintain_after_push(id, repo.git_dir().to_path_buf());
        let sha = sha.to_string();
        if let Err(e) = self.record_provenance(
            id,
            &sha,
            neutrino_proto::ProvenanceKind::Merge,
            serde_json::json!({
                "pull": number,
                "head_sha": pull.head_sha,
                "source": pull.source,
                "target": pull.target,
                "strategy": format!("{strategy:?}"),
            }),
            evidence,
        ) {
            eprintln!("merged #{number} but couldn't record provenance: {e:#}");
        }
        let changed = target_before.as_deref().and_then(|old| self.changed_files(id, old, &sha));
        let push = neutrino_ci::Event::Push { name: format!("refs/heads/{}", pull.target), sha: sha.clone() };
        self.trigger(id, &push, changed.as_deref());
        self.mirrors.request(repo.git_dir());
        Ok(Response::Merged { sha })
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
