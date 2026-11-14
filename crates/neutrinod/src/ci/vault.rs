//! Requests for secrets, variables, environments and approvals.

use anyhow::{Context, Result, bail};
use neutrino_proto::{Environment, Response, SecretInfo, VaultInfo, VaultScope};
use neutrino_store::Scope;

use crate::Daemon;

impl Daemon {
    pub(crate) fn vault(&self) -> Result<&neutrino_vault::Vault> {
        self.server_only()?;
        self.vault.as_ref().context("this daemon has no vault")
    }

    fn check_scope(&self, scope: &VaultScope) -> Result<Scope> {
        let s = Scope::from(scope)?;
        if !s.repo.is_empty() {
            self.registry.get(&s.repo)?;
        }
        if !s.environment.is_empty() && self.store()?.environment(&s.repo, &s.environment)?.is_none() {
            bail!("{} has no environment {}; add it first", s.repo, s.environment);
        }
        Ok(s)
    }

    pub(crate) fn vault_info(&self) -> Result<VaultInfo> {
        let key_store = match &self.options.vault {
            neutrino_vault::KeyStore::Keychain => "keychain",
            neutrino_vault::KeyStore::File(_) => "file",
        };
        Ok(VaultInfo { set_up: self.vault()?.is_set_up()?, key_store: key_store.into() })
    }

    pub(crate) fn set_secret(&self, scope: &VaultScope, name: &str, value: &str) -> Result<Response> {
        let s = self.check_scope(scope)?;
        if value.is_empty() {
            bail!("a secret can't be empty");
        }
        if value.len() > 48 * 1024 {
            bail!("secrets are limited to 48 KB");
        }
        neutrino_store::vault_check_name(name)?;
        let sealed = self.vault()?.seal(&s.context(name), value.as_bytes())?;
        self.store()?.set_secret(&s, name, &sealed)?;
        Ok(Response::Secrets(self.store()?.secrets(&s)?))
    }

    pub(crate) fn secrets(&self, scope: &VaultScope) -> Result<Vec<SecretInfo>> {
        let s = Scope::from(scope)?;
        self.store()?.secrets(&s)
    }

    /// Names and values a job gets, opened. Never stored anywhere.
    pub(crate) fn secrets_for_job(&self, repo: &str, environment: &str) -> Result<Vec<(String, String)>> {
        let sealed = self.store()?.sealed_secrets_for(repo, environment)?;
        if sealed.is_empty() {
            return Ok(Vec::new());
        }
        let vault = self.vault()?;
        sealed
            .into_iter()
            .map(|(scope, name, bytes)| {
                let value = vault.open(&scope.context(&name), &bytes).with_context(|| format!("opening secret {name}"))?;
                Ok((name, String::from_utf8_lossy(&value).into_owned()))
            })
            .collect()
    }

    pub(crate) fn set_environment(&self, env: &Environment) -> Result<Response> {
        self.registry.get(&env.repo)?;
        self.store()?.set_environment(env)?;
        Ok(Response::Environments(self.store()?.environments(&env.repo)?))
    }

    /// Approve or turn down a job waiting on its environment.
    pub(crate) fn approve(&self, repo: &str, job: i64, approve: bool, by: &str) -> Result<Response> {
        let store = self.store()?;
        let j = store.job(job)?;
        let run = store.run(j.run)?;
        if run.repo != repo {
            bail!("job {job} isn't in {repo}");
        }
        store.decide(job, approve, by)?;
        if !approve {
            store.settle_run(j.run)?;
        }
        self.runs_changed(repo);
        if let Some(ci) = self.ci.as_ref() {
            ci.wake.notify_one();
        }
        Ok(Response::Run(Box::new(store.run_detail(j.run)?)))
    }
}
