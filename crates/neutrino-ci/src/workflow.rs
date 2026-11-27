//! Workflow files: `.neutrino/workflows/*.yml`, Actions-shaped, `run:` steps only.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};
use serde_yaml::Value as Yaml;

use crate::expr::{self, Condition};
use crate::Error;

pub const DEFAULT_TIMEOUT_MINUTES: u32 = 60;
pub const DEFAULT_CPUS: u32 = 2;
pub const DEFAULT_MEMORY_MIB: u64 = 2048;
/// Jobs a matrix may expand to, at most.
pub const MAX_MATRIX: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Executor {
    /// A Linux VM through Graviton.
    Graviton,
    /// macOS on atlas, unisolated (milestone 7).
    Host,
    /// forge through Nexus (milestone 7).
    Nexus,
}

impl Executor {
    pub fn name(self) -> &'static str {
        match self {
            Executor::Graviton => "graviton",
            Executor::Host => "host",
            Executor::Nexus => "nexus",
        }
    }
}

/// Which refs a trigger fires for. Empty lists mean every branch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefFilter {
    pub branches: Vec<String>,
    pub branches_ignore: Vec<String>,
    pub tags: Vec<String>,
    /// Run only when a changed file matches (a later `!pattern` excludes).
    pub paths: Vec<String>,
    /// Don't run when every changed file matches.
    pub paths_ignore: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Triggers {
    pub push: Option<RefFilter>,
    pub pull_request: Option<RefFilter>,
    pub dispatch: bool,
    /// Cron lines; schedules run from milestone 6.
    pub schedule: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub name: Option<String>,
    pub run: String,
    pub env: BTreeMap<String, String>,
    pub condition: Option<Condition>,
    pub timeout_minutes: Option<u32>,
    pub working_directory: Option<String>,
    pub continue_on_error: bool,
    /// `bash`, `sh`, `pwsh`, `powershell` or `cmd`. Default: bash where
    /// there is one, else sh (or cmd on Windows).
    pub shell: Option<String>,
}

impl Step {
    /// The name shown for it: its own, else the first line of its command.
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            let first = self.run.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("run");
            let mut s: String = first.chars().take(60).collect();
            if first.chars().count() > 60 || self.run.trim().lines().count() > 1 {
                s.push('…');
            }
            s
        })
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Matrix {
    pub axes: Vec<(String, Vec<Value>)>,
    pub include: Vec<Map<String, Value>>,
    pub exclude: Vec<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: String,
    pub name: Option<String>,
    pub runs_on: Executor,
    pub image: Option<String>,
    pub cpus: u32,
    pub memory_mib: u64,
    pub timeout_minutes: u32,
    /// Fail a step that prints nothing for this long.
    pub idle_minutes: Option<u32>,
    pub needs: Vec<String>,
    pub condition: Option<Condition>,
    pub env: BTreeMap<String, String>,
    pub matrix: Option<Matrix>,
    pub continue_on_error: bool,
    pub steps: Vec<Step>,
    pub environment: Option<String>,
    pub concurrency: Option<Concurrency>,
    pub cache: Vec<Cache>,
    /// Paths (or patterns with `*`) in the workspace to keep after the job.
    pub artifacts: Vec<String>,
    pub services: Vec<Service>,
    /// Where a `runs-on: nexus` job may go.
    pub nexus: NexusPlacement,
}

/// What Nexus places a job by.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NexusPlacement {
    /// Only this node (`forge`, say).
    pub node: Option<String>,
    /// `any`, `cuda`, `appleGPU` or `cpu`.
    pub accelerator: Option<String>,
    pub gpu_memory_gb: f64,
}

fn nexus_placement(v: Option<&Yaml>, job: &str) -> Result<NexusPlacement, Error> {
    let Some(v) = v else { return Ok(NexusPlacement::default()) };
    let Yaml::Mapping(m) = v else { return Err(Error::new(format!("jobs.{job}.nexus is {{ node, accelerator, gpu-memory }}"))) };
    let accelerator = key(m, "accelerator").and_then(text);
    if let Some(a) = &accelerator
        && !["any", "cuda", "appleGPU", "cpu"].contains(&a.as_str())
    {
        return Err(Error::new(format!("jobs.{job}.nexus.accelerator: use any, cuda, appleGPU or cpu")));
    }
    let gpu_memory_gb = match key(m, "gpu-memory") {
        None => 0.0,
        Some(g) => text(g).and_then(|s| parse_memory(&s)).map(|mib| mib as f64 / 1024.0).ok_or_else(|| Error::new(format!("jobs.{job}.nexus.gpu-memory should look like 8G")))?,
    };
    Ok(NexusPlacement { node: key(m, "node").and_then(text), accelerator, gpu_memory_gb })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Workflow {
    /// The file's name under `.neutrino/workflows/`.
    pub file: String,
    pub name: String,
    pub on: Triggers,
    pub env: BTreeMap<String, String>,
    pub concurrency: Option<Concurrency>,
    /// In file order.
    pub jobs: Vec<Job>,
    /// Keys Neutrino read but doesn't act on yet.
    pub warnings: Vec<String>,
}

fn key<'a>(map: &'a serde_yaml::Mapping, name: &str) -> Option<&'a Yaml> {
    map.get(Yaml::String(name.into()))
}

fn text(v: &Yaml) -> Option<String> {
    match v {
        Yaml::String(s) => Some(s.clone()),
        Yaml::Number(n) => Some(n.to_string()),
        Yaml::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn strings(v: Option<&Yaml>, what: &str) -> Result<Vec<String>, Error> {
    match v {
        None | Some(Yaml::Null) => Ok(Vec::new()),
        Some(Yaml::Sequence(items)) => items.iter().map(|i| text(i).ok_or_else(|| Error::new(format!("{what} must be a list of names")))).collect(),
        Some(other) => text(other).map(|s| vec![s]).ok_or_else(|| Error::new(format!("{what} must be a name or a list of names"))),
    }
}

fn env_map(v: Option<&Yaml>, what: &str) -> Result<BTreeMap<String, String>, Error> {
    let mut out = BTreeMap::new();
    match v {
        None | Some(Yaml::Null) => {}
        Some(Yaml::Mapping(m)) => {
            for (k, v) in m {
                let k = text(k).ok_or_else(|| Error::new(format!("{what}: names must be text")))?;
                if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || k.starts_with(|c: char| c.is_ascii_digit()) {
                    return Err(Error::new(format!("{what}: {k:?} isn't a valid variable name")));
                }
                let v = match v {
                    Yaml::Null => String::new(),
                    v => text(v).ok_or_else(|| Error::new(format!("{what}.{k} must be text")))?,
                };
                expr::check(&v).map_err(|e| Error::new(format!("{what}.{k}: {e}")))?;
                out.insert(k, v);
            }
        }
        Some(_) => return Err(Error::new(format!("{what} must be a map of NAME: value"))),
    }
    Ok(out)
}

fn minutes(v: Option<&Yaml>, what: &str) -> Result<Option<u32>, Error> {
    match v {
        None | Some(Yaml::Null) => Ok(None),
        Some(Yaml::Number(n)) => match n.as_u64() {
            Some(m) if (1..=24 * 60).contains(&m) => Ok(Some(m as u32)),
            _ => Err(Error::new(format!("{what} must be between 1 and 1440 minutes"))),
        },
        Some(_) => Err(Error::new(format!("{what} must be a number of minutes"))),
    }
}

fn boolean(v: Option<&Yaml>, what: &str) -> Result<bool, Error> {
    match v {
        None | Some(Yaml::Null) => Ok(false),
        Some(Yaml::Bool(b)) => Ok(*b),
        Some(_) => Err(Error::new(format!("{what} must be true or false"))),
    }
}

/// `512M`, `6G`, `6GiB`, or a number of MiB.
pub fn parse_memory(s: &str) -> Option<u64> {
    let s = s.trim();
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let n: f64 = digits.parse().ok()?;
    let unit = s[digits.len()..].trim().to_ascii_lowercase();
    let mib = match unit.trim_end_matches('b').trim_end_matches('i') {
        "" | "m" => n,
        "g" => n * 1024.0,
        _ => return None,
    };
    (mib >= 1.0).then_some(mib as u64)
}

fn condition(v: Option<&Yaml>, what: &str) -> Result<Option<Condition>, Error> {
    match v {
        None | Some(Yaml::Null) => Ok(None),
        Some(Yaml::Bool(b)) => Ok(Some(Condition::parse(if *b { "true" } else { "false" })?)),
        Some(v) => {
            let s = text(v).ok_or_else(|| Error::new(format!("{what} must be an expression")))?;
            Condition::parse(&s).map(Some).map_err(|e| Error::new(format!("{what}: {e}")))
        }
    }
}

fn ref_filter(v: &Yaml, what: &str) -> Result<RefFilter, Error> {
    match v {
        Yaml::Null => Ok(RefFilter::default()),
        Yaml::Mapping(m) => {
            for (k, _) in m {
                let k = text(k).unwrap_or_default();
                if !["branches", "branches-ignore", "tags", "types", "paths", "paths-ignore"].contains(&k.as_str()) {
                    return Err(Error::new(format!("on.{what}: unknown key {k}")));
                }
            }
            let f = RefFilter {
                branches: strings(key(m, "branches"), &format!("on.{what}.branches"))?,
                branches_ignore: strings(key(m, "branches-ignore"), &format!("on.{what}.branches-ignore"))?,
                tags: strings(key(m, "tags"), &format!("on.{what}.tags"))?,
                paths: strings(key(m, "paths"), &format!("on.{what}.paths"))?,
                paths_ignore: strings(key(m, "paths-ignore"), &format!("on.{what}.paths-ignore"))?,
            };
            if !f.branches.is_empty() && !f.branches_ignore.is_empty() {
                return Err(Error::new(format!("on.{what}: use branches or branches-ignore, not both")));
            }
            if !f.paths.is_empty() && !f.paths_ignore.is_empty() {
                return Err(Error::new(format!("on.{what}: use paths or paths-ignore, not both")));
            }
            Ok(f)
        }
        _ => Err(Error::new(format!("on.{what} must be a map"))),
    }
}

fn triggers(v: Option<&Yaml>) -> Result<Triggers, Error> {
    let mut t = Triggers::default();
    let mut add = |name: &str, value: &Yaml| -> Result<(), Error> {
        match name {
            "push" => t.push = Some(ref_filter(value, "push")?),
            "pull_request" => t.pull_request = Some(ref_filter(value, "pull_request")?),
            "workflow_dispatch" => t.dispatch = true,
            "schedule" => {
                let Yaml::Sequence(items) = value else { return Err(Error::new("on.schedule must be a list of { cron: \"…\" }")) };
                for i in items {
                    let cron = match i {
                        Yaml::Mapping(m) => key(m, "cron").and_then(text),
                        _ => None,
                    };
                    let cron = cron.ok_or_else(|| Error::new("on.schedule entries are { cron: \"…\" }"))?;

                    t.schedule.push(cron);
                }
            }
            other => return Err(Error::new(format!("Neutrino doesn't know the trigger {other}; use push, pull_request, workflow_dispatch or schedule"))),
        }
        Ok(())
    };
    match v {
        None => return Err(Error::new("a workflow needs `on:` to say when it runs")),
        Some(Yaml::String(s)) => add(s, &Yaml::Null)?,
        Some(Yaml::Sequence(items)) => {
            for i in items {
                add(&text(i).ok_or_else(|| Error::new("on: lists trigger names"))?, &Yaml::Null)?;
            }
        }
        Some(Yaml::Mapping(m)) => {
            for (k, v) in m {
                add(&text(k).ok_or_else(|| Error::new("on: keys are trigger names"))?, v)?;
            }
        }
        Some(_) => return Err(Error::new("on: must be a trigger, a list or a map")),
    }
    Ok(t)
}

fn json(v: &Yaml) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn matrix(v: Option<&Yaml>, job: &str) -> Result<Option<Matrix>, Error> {
    let Some(Yaml::Mapping(strategy)) = v else {
        return match v {
            None | Some(Yaml::Null) => Ok(None),
            _ => Err(Error::new(format!("jobs.{job}.strategy must be a map"))),
        };
    };
    let Some(m) = key(strategy, "matrix") else { return Ok(None) };
    let Yaml::Mapping(m) = m else { return Err(Error::new(format!("jobs.{job}.strategy.matrix must be a map"))) };
    let mut out = Matrix::default();
    for (k, v) in m {
        let k = text(k).unwrap_or_default();
        let list = |v: &Yaml, what: &str| -> Result<Vec<Map<String, Value>>, Error> {
            let Yaml::Sequence(items) = v else { return Err(Error::new(format!("jobs.{job}.strategy.matrix.{what} must be a list of maps"))) };
            items
                .iter()
                .map(|i| match json(i) {
                    Value::Object(o) => Ok(o),
                    _ => Err(Error::new(format!("jobs.{job}.strategy.matrix.{what} entries must be maps"))),
                })
                .collect()
        };
        match k.as_str() {
            "include" => out.include = list(v, "include")?,
            "exclude" => out.exclude = list(v, "exclude")?,
            _ => match json(v) {
                Value::Array(values) if !values.is_empty() => out.axes.push((k, values)),
                _ => return Err(Error::new(format!("jobs.{job}.strategy.matrix.{k} must be a non-empty list"))),
            },
        }
    }
    Ok(Some(out))
}

impl Matrix {
    /// Every combination, minus excludes, plus includes: what Actions does,
    /// except that an include always adds its own combination.
    pub fn expand(&self) -> Vec<Map<String, Value>> {
        let mut combos: Vec<Map<String, Value>> = vec![Map::new()];
        for (name, values) in &self.axes {
            combos = combos
                .into_iter()
                .flat_map(|c| {
                    values.iter().map(move |v| {
                        let mut c = c.clone();
                        c.insert(name.clone(), v.clone());
                        c
                    })
                })
                .collect();
        }
        if self.axes.is_empty() {
            combos.clear();
        }
        combos.retain(|c| !self.exclude.iter().any(|ex| ex.iter().all(|(k, v)| c.get(k) == Some(v))));
        for inc in &self.include {
            // An include that only adds keys to an existing combination extends it.
            let axis_keys: Vec<&String> = self.axes.iter().map(|(k, _)| k).collect();
            let matches: Vec<usize> = combos
                .iter()
                .enumerate()
                .filter(|(_, c)| inc.iter().filter(|(k, _)| axis_keys.contains(k)).all(|(k, v)| c.get(k) == Some(v)))
                .map(|(i, _)| i)
                .collect();
            let touches_axes = inc.keys().any(|k| axis_keys.contains(&k));
            if touches_axes && !matches.is_empty() {
                for i in matches {
                    for (k, v) in inc {
                        combos[i].insert(k.clone(), v.clone());
                    }
                }
            } else {
                combos.push(inc.clone());
            }
        }
        combos
    }
}

fn step(v: &Yaml, job: &str, i: usize) -> Result<Step, Error> {
    let what = format!("jobs.{job}.steps[{i}]");
    let Yaml::Mapping(m) = v else { return Err(Error::new(format!("{what} must be a map with run:"))) };
    if key(m, "uses").is_some() {
        return Err(Error::new(format!("{what}: uses: isn't supported; write the commands in run: (checkout is built in)")));
    }
    for (k, _) in m {
        let k = text(k).unwrap_or_default();
        if !["name", "run", "env", "if", "timeout-minutes", "working-directory", "continue-on-error", "id", "shell"].contains(&k.as_str()) {
            return Err(Error::new(format!("{what}: unknown key {k}")));
        }
    }
    let run = key(m, "run").and_then(text).filter(|r| !r.trim().is_empty()).ok_or_else(|| Error::new(format!("{what} needs run:")))?;
    expr::check(&run).map_err(|e| Error::new(format!("{what}.run: {e}")))?;
    Ok(Step {
        name: key(m, "name").and_then(text),
        run,
        env: env_map(key(m, "env"), &format!("{what}.env"))?,
        condition: condition(key(m, "if"), &format!("{what}.if"))?,
        timeout_minutes: minutes(key(m, "timeout-minutes"), &format!("{what}.timeout-minutes"))?,
        working_directory: key(m, "working-directory").and_then(text),
        continue_on_error: boolean(key(m, "continue-on-error"), &format!("{what}.continue-on-error"))?,
        shell: match key(m, "shell").and_then(text) {
            None => None,
            Some(sh) if ["bash", "sh", "pwsh", "powershell", "cmd"].contains(&sh.as_str()) => Some(sh),
            Some(other) => return Err(Error::new(format!("{what}.shell: {other:?} isn't a shell Neutrino knows; use bash, sh, pwsh, powershell or cmd"))),
        },
    })
}

/// Keys that later milestones act on: read, warned about, not refused.
const LATER: &[(&str, &str)] = &[("outputs", "job outputs aren't supported yet")];

/// Runs that share a group run one at a time; with `cancel_in_progress`, a
/// new one cancels the one before instead of waiting for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Concurrency {
    pub group: String,
    pub cancel_in_progress: bool,
}

/// Folders kept between runs under a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cache {
    pub key: String,
    /// Relative to the workspace.
    pub paths: Vec<String>,
    /// Prefixes to fall back to when nothing has `key` exactly.
    pub restore_keys: Vec<String>,
}

/// A container started beside the job, reachable at `<NAME>_HOST`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub image: String,
    pub env: BTreeMap<String, String>,
    pub cpus: u32,
    pub memory_mib: u64,
}

fn concurrency(v: Option<&Yaml>, what: &str) -> Result<Option<Concurrency>, Error> {
    match v {
        None | Some(Yaml::Null) => Ok(None),
        Some(Yaml::Mapping(m)) => {
            let group = key(m, "group").and_then(text).ok_or_else(|| Error::new(format!("{what} needs group:")))?;
            expr::check(&group).map_err(|e| Error::new(format!("{what}.group: {e}")))?;
            Ok(Some(Concurrency { group, cancel_in_progress: boolean(key(m, "cancel-in-progress"), &format!("{what}.cancel-in-progress"))? }))
        }
        Some(v) => {
            let group = text(v).ok_or_else(|| Error::new(format!("{what} is a group name or {{ group, cancel-in-progress }}")))?;
            expr::check(&group).map_err(|e| Error::new(format!("{what}: {e}")))?;
            Ok(Some(Concurrency { group, cancel_in_progress: false }))
        }
    }
}

/// A path inside the workspace: relative, without `..`.
fn workspace_path(p: &str, what: &str) -> Result<String, Error> {
    let p = p.trim().trim_start_matches("./").trim_end_matches('/');
    if p.is_empty() || p.starts_with('/') || p.starts_with('~') || p.split('/').any(|c| c == "..") {
        return Err(Error::new(format!("{what}: {p:?} must be a path inside the workspace (relative, no ..)")));
    }
    Ok(p.to_string())
}

fn caches(v: Option<&Yaml>, job: &str) -> Result<Vec<Cache>, Error> {
    let items = match v {
        None | Some(Yaml::Null) => return Ok(Vec::new()),
        Some(Yaml::Sequence(items)) => items.clone(),
        Some(m @ Yaml::Mapping(_)) => vec![m.clone()],
        Some(_) => return Err(Error::new(format!("jobs.{job}.cache is a list of {{ key, path }}"))),
    };
    items
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let what = format!("jobs.{job}.cache[{i}]");
            let Yaml::Mapping(m) = c else { return Err(Error::new(format!("{what} is {{ key, path }}"))) };
            let key_ = key(m, "key").and_then(text).ok_or_else(|| Error::new(format!("{what} needs key:")))?;
            expr::check(&key_).map_err(|e| Error::new(format!("{what}.key: {e}")))?;
            let paths = strings(key(m, "path"), &format!("{what}.path"))?;
            if paths.is_empty() {
                return Err(Error::new(format!("{what} needs path:")));
            }
            let paths = paths.iter().map(|p| workspace_path(p, &what)).collect::<Result<Vec<_>, _>>()?;
            let restore_keys = strings(key(m, "restore-keys"), &format!("{what}.restore-keys"))?;
            for r in &restore_keys {
                expr::check(r).map_err(|e| Error::new(format!("{what}.restore-keys: {e}")))?;
            }
            Ok(Cache { key: key_, paths, restore_keys })
        })
        .collect()
}

fn services(v: Option<&Yaml>, job: &str) -> Result<Vec<Service>, Error> {
    let Some(v) = v else { return Ok(Vec::new()) };
    let Yaml::Mapping(m) = v else { return Err(Error::new(format!("jobs.{job}.services is a map of name: {{ image }}"))) };
    let mut out = Vec::new();
    for (k, v) in m {
        let name = text(k).unwrap_or_default();
        let what = format!("jobs.{job}.services.{name}");
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::new(format!("{what}: service names use letters, digits and '_'")));
        }
        let (image, env, mut cpus, mut memory_mib) = match v {
            Yaml::Mapping(s) => (
                key(s, "image").and_then(text).ok_or_else(|| Error::new(format!("{what} needs image:")))?,
                env_map(key(s, "env"), &format!("{what}.env"))?,
                1,
                512,
            ),
            other => (text(other).ok_or_else(|| Error::new(format!("{what} is an image or {{ image, env }}")))?, BTreeMap::new(), 1, 512),
        };
        if let Yaml::Mapping(s) = v
            && let Some(Yaml::Mapping(r)) = key(s, "resources")
        {
            if let Some(c) = key(r, "cpus").and_then(|c| c.as_u64()) {
                cpus = c.clamp(1, 16) as u32;
            }
            if let Some(mem) = key(r, "memory").and_then(text).and_then(|s| parse_memory(&s)) {
                memory_mib = mem;
            }
        }
        out.push(Service { name, image, env, cpus, memory_mib });
    }
    Ok(out)
}

fn environment(v: Option<&Yaml>, job: &str) -> Result<Option<String>, Error> {
    match v {
        None | Some(Yaml::Null) => Ok(None),
        Some(Yaml::Mapping(m)) => key(m, "name").and_then(text).map(Some).ok_or_else(|| Error::new(format!("jobs.{job}.environment needs name:"))),
        Some(v) => text(v).map(Some).ok_or_else(|| Error::new(format!("jobs.{job}.environment is a name"))),
    }
}

fn job(id: &str, v: &Yaml, warnings: &mut Vec<String>) -> Result<Job, Error> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') || id.starts_with('-') {
        return Err(Error::new(format!("{id:?} can't be a job id: use letters, digits, '_' and '-'")));
    }
    let Yaml::Mapping(m) = v else { return Err(Error::new(format!("jobs.{id} must be a map"))) };
    for (k, _) in m {
        let k = text(k).unwrap_or_default();
        if let Some((_, why)) = LATER.iter().find(|(name, _)| *name == k) {
            warnings.push(format!("jobs.{id}.{k} is ignored for now: {why}"));
            continue;
        }
        if ![
            "name", "runs-on", "image", "resources", "timeout-minutes", "idle-timeout-minutes", "needs", "if", "env", "strategy", "continue-on-error", "steps",
            "environment", "concurrency", "cache", "artifacts", "services", "nexus",
        ]
        .contains(&k.as_str())
        {
            return Err(Error::new(format!("jobs.{id}: unknown key {k}")));
        }
    }
    let runs_on = match key(m, "runs-on").and_then(text).as_deref() {
        None | Some("graviton") => Executor::Graviton,
        Some("host") => Executor::Host,
        Some("nexus") => Executor::Nexus,
        Some(other) => return Err(Error::new(format!("jobs.{id}.runs-on: {other:?} isn't an executor; use graviton, host or nexus"))),
    };
    let image = key(m, "image").and_then(text);
    if runs_on != Executor::Graviton && image.is_some() {
        warnings.push(format!("jobs.{id}.image is ignored: {} jobs don't run in a container", runs_on.name()));
    }
    if runs_on == Executor::Graviton && image.is_none() {
        return Err(Error::new(format!("jobs.{id} runs on graviton and needs image: (alpine:3.20, rust:1.85, …)")));
    }
    let (mut cpus, mut memory_mib) = (DEFAULT_CPUS, DEFAULT_MEMORY_MIB);
    if let Some(r) = key(m, "resources") {
        let Yaml::Mapping(r) = r else { return Err(Error::new(format!("jobs.{id}.resources must be {{ cpus, memory }}"))) };
        if let Some(c) = key(r, "cpus") {
            cpus = c.as_u64().filter(|c| (1..=64).contains(c)).ok_or_else(|| Error::new(format!("jobs.{id}.resources.cpus must be 1 to 64")))? as u32;
        }
        if let Some(mem) = key(r, "memory") {
            memory_mib = text(mem).and_then(|s| parse_memory(&s)).ok_or_else(|| Error::new(format!("jobs.{id}.resources.memory must look like 512M or 4G")))?;
        }
    }
    let steps = match key(m, "steps") {
        Some(Yaml::Sequence(items)) if !items.is_empty() => items.iter().enumerate().map(|(i, s)| step(s, id, i)).collect::<Result<Vec<_>, _>>()?,
        _ => return Err(Error::new(format!("jobs.{id} needs a list of steps"))),
    };
    let m_ = matrix(key(m, "strategy"), id)?;
    if let Some(mx) = &m_ {
        let n = mx.expand().len();
        if n == 0 {
            return Err(Error::new(format!("jobs.{id}.strategy.matrix leaves no combinations")));
        }
        if n > MAX_MATRIX {
            return Err(Error::new(format!("jobs.{id}.strategy.matrix makes {n} jobs; the limit is {MAX_MATRIX}")));
        }
    }
    let name = key(m, "name").and_then(text);
    if let Some(n) = &name {
        expr::check(n).map_err(|e| Error::new(format!("jobs.{id}.name: {e}")))?;
    }
    Ok(Job {
        id: id.to_string(),
        name,
        runs_on,
        image,
        cpus,
        memory_mib,
        timeout_minutes: minutes(key(m, "timeout-minutes"), &format!("jobs.{id}.timeout-minutes"))?.unwrap_or(DEFAULT_TIMEOUT_MINUTES),
        idle_minutes: minutes(key(m, "idle-timeout-minutes"), &format!("jobs.{id}.idle-timeout-minutes"))?,
        needs: strings(key(m, "needs"), &format!("jobs.{id}.needs"))?,
        condition: condition(key(m, "if"), &format!("jobs.{id}.if"))?,
        env: env_map(key(m, "env"), &format!("jobs.{id}.env"))?,
        matrix: m_,
        continue_on_error: boolean(key(m, "continue-on-error"), &format!("jobs.{id}.continue-on-error"))?,
        steps,
        environment: environment(key(m, "environment"), id)?,
        concurrency: concurrency(key(m, "concurrency"), &format!("jobs.{id}.concurrency"))?,
        cache: caches(key(m, "cache"), id)?,
        artifacts: strings(key(m, "artifacts"), &format!("jobs.{id}.artifacts"))?
            .iter()
            .map(|p| workspace_path(p, &format!("jobs.{id}.artifacts")))
            .collect::<Result<Vec<_>, _>>()?,
        services: services(key(m, "services"), id)?,
        nexus: nexus_placement(key(m, "nexus"), id)?,
    })
}

impl Workflow {
    /// Parse and check a workflow file. `file` is its name, for messages.
    pub fn parse(file: &str, source: &str) -> Result<Workflow, Error> {
        let doc: Yaml = serde_yaml::from_str(source).map_err(|e| Error::new(format!("{file}: {e}")))?;
        Self::from_yaml(file, &doc).map_err(|e| Error::new(format!("{file}: {}", e.0)))
    }

    fn from_yaml(file: &str, doc: &Yaml) -> Result<Workflow, Error> {
        let Yaml::Mapping(m) = doc else { return Err(Error::new("a workflow is a map with on: and jobs:")) };
        let mut warnings = Vec::new();
        for (k, _) in m {
            let k = text(k).unwrap_or_default();
            if !["name", "on", "env", "jobs", "run-name", "permissions", "defaults", "concurrency"].contains(&k.as_str()) {
                return Err(Error::new(format!("unknown key {k}")));
            }
            if ["permissions", "defaults", "run-name"].contains(&k.as_str()) {
                warnings.push(format!("{k} is ignored"));
            }
        }
        let Some(Yaml::Mapping(jobs_yaml)) = key(m, "jobs") else { return Err(Error::new("a workflow needs jobs:")) };
        let mut jobs = Vec::new();
        for (k, v) in jobs_yaml {
            let id = text(k).ok_or_else(|| Error::new("job ids are text"))?;
            jobs.push(job(&id, v, &mut warnings)?);
        }
        if jobs.is_empty() {
            return Err(Error::new("a workflow needs at least one job"));
        }
        let ids: BTreeSet<&str> = jobs.iter().map(|j| j.id.as_str()).collect();
        for j in &jobs {
            for n in &j.needs {
                if !ids.contains(n.as_str()) {
                    return Err(Error::new(format!("jobs.{}.needs {n}, which isn't a job here", j.id)));
                }
            }
        }
        // Refuse cycles: repeatedly take the jobs whose needs are all taken.
        let mut done: BTreeSet<&str> = BTreeSet::new();
        while done.len() < jobs.len() {
            let ready: Vec<&str> = jobs.iter().filter(|j| !done.contains(j.id.as_str()) && j.needs.iter().all(|n| done.contains(n.as_str()))).map(|j| j.id.as_str()).collect();
            if ready.is_empty() {
                let stuck: Vec<&str> = jobs.iter().map(|j| j.id.as_str()).filter(|id| !done.contains(id)).collect();
                return Err(Error::new(format!("jobs {} need each other in a circle", stuck.join(", "))));
            }
            done.extend(ready);
        }
        let name = key(m, "name").and_then(text).unwrap_or_else(|| file.trim_end_matches(".yml").trim_end_matches(".yaml").to_string());
        let on = triggers(key(m, "on"))?;
        for c in &on.schedule {
            crate::cron::Cron::parse(c).map_err(|e| Error::new(format!("on.schedule: {e}")))?;
        }
        Ok(Workflow {
            file: file.to_string(),
            name,
            on,
            env: env_map(key(m, "env"), "env")?,
            concurrency: concurrency(key(m, "concurrency"), "concurrency")?,
            jobs,
            warnings,
        })
    }
}
