//! Secrets (sealed by the caller; this never sees a value), variables and
//! environments, each scoped to every repo, one repo, or one repo's environment.

use anyhow::{Result, bail};
use neutrino_proto::{Environment, SecretInfo, Variable, VaultScope};
use rusqlite::params;

use crate::{Store, now};

/// A scope as stored: empty strings for "any".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub repo: String,
    pub environment: String,
}

impl Scope {
    pub fn from(s: &VaultScope) -> Result<Scope> {
        let repo = s.repo.clone().unwrap_or_default();
        let environment = s.environment.clone().unwrap_or_default();
        if repo.is_empty() && !environment.is_empty() {
            bail!("an environment belongs to a repository; say which");
        }
        Ok(Scope { repo, environment })
    }

    fn to_proto(&self) -> VaultScope {
        VaultScope {
            repo: (!self.repo.is_empty()).then(|| self.repo.clone()),
            environment: (!self.environment.is_empty()).then(|| self.environment.clone()),
        }
    }

    /// What a sealed secret's associated data says it is.
    pub fn context(&self, name: &str) -> String {
        format!("secret\0{}\0{}\0{}", self.repo, self.environment, name)
    }
}

/// Names that work as environment variables, and aren't Neutrino's own.
pub fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 100
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit());
    if !ok {
        bail!("{name:?} can't be a name: use letters, digits and '_', not starting with a digit");
    }
    if name.starts_with("NEUTRINO_") || name == "CI" {
        bail!("{name} is set by Neutrino itself");
    }
    Ok(())
}

impl Store {
    pub fn set_secret(&self, scope: &Scope, name: &str, sealed: &[u8]) -> Result<()> {
        check_name(name)?;
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO secrets (repo, environment, name, sealed, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (repo, environment, name) DO UPDATE SET sealed = ?4, updated_at = ?5",
            params![scope.repo, scope.environment, name, sealed, now()],
        )?;
        Ok(())
    }

    pub fn delete_secret(&self, scope: &Scope, name: &str) -> Result<()> {
        let db = self.db.lock().unwrap();
        if db.execute("DELETE FROM secrets WHERE repo = ?1 AND environment = ?2 AND name = ?3", params![scope.repo, scope.environment, name])? == 0 {
            bail!("there's no secret {name} there");
        }
        Ok(())
    }

    /// Names only. With a repo, its own and every repo's; with an environment too, those as well.
    pub fn secrets(&self, scope: &Scope) -> Result<Vec<SecretInfo>> {
        let db = self.db.lock().unwrap();
        let rows = db
            .prepare(
                "SELECT repo, environment, name, updated_at FROM secrets
                 WHERE (repo = '' OR repo = ?1) AND (environment = '' OR environment = ?2)
                 ORDER BY repo, environment, name",
            )?
            .query_map(params![scope.repo, scope.environment], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.into_iter().map(|(repo, environment, name, at)| SecretInfo { scope: Scope { repo, environment }.to_proto(), name, updated_at: at }).collect())
    }

    /// The sealed secrets a job in `repo` deploying to `environment` gets, the
    /// most specific of each name winning, with the scope each was sealed under.
    pub fn sealed_secrets_for(&self, repo: &str, environment: &str) -> Result<Vec<(Scope, String, Vec<u8>)>> {
        let db = self.db.lock().unwrap();
        let rows = db
            .prepare(
                "SELECT repo, environment, name, sealed FROM secrets
                 WHERE (repo = '' AND environment = '') OR (repo = ?1 AND environment = '') OR (repo = ?1 AND environment = ?2 AND ?2 != '')
                 ORDER BY (repo != '') + (environment != '')",
            )?
            .query_map(params![repo, environment], |r| Ok((Scope { repo: r.get(0)?, environment: r.get(1)? }, r.get::<_, String>(2)?, r.get::<_, Vec<u8>>(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // Later rows are more specific; keep the last of each name.
        let mut out: Vec<(Scope, String, Vec<u8>)> = Vec::new();
        for row in rows {
            out.retain(|(_, n, _)| *n != row.1);
            out.push(row);
        }
        Ok(out)
    }

    pub fn set_variable(&self, scope: &Scope, name: &str, value: &str) -> Result<()> {
        check_name(name)?;
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO variables (repo, environment, name, value, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (repo, environment, name) DO UPDATE SET value = ?4, updated_at = ?5",
            params![scope.repo, scope.environment, name, value, now()],
        )?;
        Ok(())
    }

    pub fn delete_variable(&self, scope: &Scope, name: &str) -> Result<()> {
        let db = self.db.lock().unwrap();
        if db.execute("DELETE FROM variables WHERE repo = ?1 AND environment = ?2 AND name = ?3", params![scope.repo, scope.environment, name])? == 0 {
            bail!("there's no variable {name} there");
        }
        Ok(())
    }

    pub fn variables(&self, scope: &Scope) -> Result<Vec<Variable>> {
        let db = self.db.lock().unwrap();
        let rows = db
            .prepare(
                "SELECT repo, environment, name, value, updated_at FROM variables
                 WHERE (repo = '' OR repo = ?1) AND (environment = '' OR environment = ?2)
                 ORDER BY (repo != '') + (environment != ''), name",
            )?
            .query_map(params![scope.repo, scope.environment], |r| {
                Ok(Variable { scope: Scope { repo: r.get(0)?, environment: r.get(1)? }.to_proto(), name: r.get(2)?, value: r.get(3)?, updated_at: r.get(4)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The `vars` a job gets: the most specific value of each name.
    pub fn variables_for(&self, repo: &str, environment: &str) -> Result<Vec<(String, String)>> {
        let mut out: Vec<(String, String)> = Vec::new();
        for v in self.variables(&Scope { repo: repo.into(), environment: environment.into() })? {
            out.retain(|(n, _)| *n != v.name);
            out.push((v.name, v.value));
        }
        out.sort();
        Ok(out)
    }

    pub fn set_environment(&self, env: &Environment) -> Result<()> {
        let name = env.name.trim();
        if name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
            bail!("{name:?} can't be an environment name: use letters, digits, '-', '_' and '.'");
        }
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO environments (repo, name, require_approval, branches) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (repo, name) DO UPDATE SET require_approval = ?3, branches = ?4",
            params![env.repo, name, env.require_approval, serde_json::to_string(&env.branches)?],
        )?;
        Ok(())
    }

    pub fn delete_environment(&self, repo: &str, name: &str) -> Result<()> {
        let db = self.db.lock().unwrap();
        if db.execute("DELETE FROM environments WHERE repo = ?1 AND name = ?2", params![repo, name])? == 0 {
            bail!("there's no environment {name}");
        }
        db.execute("DELETE FROM secrets WHERE repo = ?1 AND environment = ?2", params![repo, name])?;
        db.execute("DELETE FROM variables WHERE repo = ?1 AND environment = ?2", params![repo, name])?;
        Ok(())
    }

    pub fn environments(&self, repo: &str) -> Result<Vec<Environment>> {
        let db = self.db.lock().unwrap();
        Ok(db
            .prepare("SELECT repo, name, require_approval, branches FROM environments WHERE repo = ?1 ORDER BY name")?
            .query_map(params![repo], |r| {
                Ok(Environment { repo: r.get(0)?, name: r.get(1)?, require_approval: r.get(2)?, branches: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default() })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn environment(&self, repo: &str, name: &str) -> Result<Option<Environment>> {
        Ok(self.environments(repo)?.into_iter().find(|e| e.name == name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(repo: &str, env: &str) -> Scope {
        Scope { repo: repo.into(), environment: env.into() }
    }

    #[test]
    fn the_most_specific_wins() {
        let s = Store::in_memory().unwrap();
        s.set_secret(&scope("", ""), "TOKEN", b"global").unwrap();
        s.set_secret(&scope("app", ""), "TOKEN", b"repo").unwrap();
        s.set_secret(&scope("app", "prod"), "TOKEN", b"prod").unwrap();
        s.set_secret(&scope("", ""), "OTHER", b"o").unwrap();
        s.set_secret(&scope("lib", ""), "TOKEN", b"lib").unwrap();
        let get = |repo: &str, env: &str| -> Vec<(String, Vec<u8>)> { s.sealed_secrets_for(repo, env).unwrap().into_iter().map(|(_, n, v)| (n, v)).collect() };
        let has = |list: &[(String, Vec<u8>)], name: &str| list.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        assert_eq!(has(&get("app", "prod"), "TOKEN"), Some(b"prod".to_vec()));
        assert_eq!(has(&get("app", ""), "TOKEN"), Some(b"repo".to_vec()));
        assert_eq!(has(&get("app", "staging"), "TOKEN"), Some(b"repo".to_vec()));
        assert_eq!(has(&get("other", ""), "TOKEN"), Some(b"global".to_vec()));
        assert_eq!(get("app", "prod").len(), 2);
        assert_eq!(s.secrets(&scope("app", "")).unwrap().len(), 3, "global and the repo's, not prod's or lib's");
        assert!(s.set_secret(&scope("", ""), "NEUTRINO_SHA", b"x").is_err());
        assert!(s.set_secret(&scope("", ""), "1BAD", b"x").is_err());
        assert!(Scope::from(&VaultScope { repo: None, environment: Some("prod".into()) }).is_err());

        s.set_variable(&scope("", ""), "REGION", "eu").unwrap();
        s.set_variable(&scope("app", "prod"), "REGION", "us").unwrap();
        assert_eq!(s.variables_for("app", "prod").unwrap(), vec![("REGION".into(), "us".into())]);
        assert_eq!(s.variables_for("app", "").unwrap(), vec![("REGION".into(), "eu".into())]);

        s.set_environment(&Environment { repo: "app".into(), name: "prod".into(), require_approval: true, branches: vec!["main".into()] }).unwrap();
        assert!(s.environment("app", "prod").unwrap().unwrap().require_approval);
        s.delete_environment("app", "prod").unwrap();
        assert_eq!(has(&get("app", "prod"), "TOKEN"), Some(b"repo".to_vec()), "deleting an environment takes its secrets with it");
        s.delete_secret(&scope("", ""), "OTHER").unwrap();
        assert!(s.delete_secret(&scope("", ""), "OTHER").is_err());
    }
}
