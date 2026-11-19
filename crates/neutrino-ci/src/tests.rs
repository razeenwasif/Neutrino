use super::*;
use serde_json::json;

const CI: &str = r#"
name: ci
on:
  push: { branches: [main, "release/**"] }
  pull_request: { branches: [main] }
  workflow_dispatch: {}
env:
  CARGO_TERM_COLOR: always
jobs:
  test:
    image: rust:1.85
    resources: { cpus: 4, memory: 6G }
    timeout-minutes: 20
    strategy:
      matrix:
        toolchain: [stable, nightly]
        mode: [debug, release]
        exclude: [{ toolchain: nightly, mode: release }]
        include: [{ toolchain: stable, mode: release, extra: lto }]
    steps:
      - run: cargo test --${{ matrix.mode }}
        timeout-minutes: 15
      - name: Lint
        run: |
          cargo clippy
          cargo fmt --check
        if: matrix.toolchain == 'stable'
  deploy:
    needs: test
    image: alpine:3.20
    if: neutrino.branch == 'main'
    services: { db: { image: postgres:17 } }
    steps:
      - run: echo deploying ${{ neutrino.sha }}
"#;

#[test]
fn parses_a_workflow() {
    let wf = Workflow::parse("ci.yml", CI).unwrap();
    assert_eq!(wf.name, "ci");
    assert_eq!(wf.jobs.len(), 2);
    let test = &wf.jobs[0];
    assert_eq!((test.cpus, test.memory_mib, test.timeout_minutes), (4, 6144, 20));
    assert_eq!(test.steps[0].timeout_minutes, Some(15));
    assert_eq!(test.steps[0].label(), "cargo test --${{ matrix.mode }}");
    assert_eq!(test.steps[1].label(), "Lint");
    assert_eq!(wf.jobs[1].needs, vec!["test"]);
    assert!(wf.warnings.is_empty(), "{:?}", wf.warnings);
    assert_eq!(wf.jobs[1].services[0].image, "postgres:17");
    assert_eq!(wf.env["CARGO_TERM_COLOR"], "always");
    assert!(wf.on.dispatch);
}

#[test]
fn refuses_what_it_cant_run() {
    let bad = |src: &str, needle: &str| {
        let err = Workflow::parse("x.yml", src).unwrap_err().0;
        assert!(err.contains(needle), "{err:?} should mention {needle:?}");
    };
    bad("on: push\njobs: { a: { image: x, steps: [{ uses: actions/checkout@v4 }] } }", "uses: isn't supported");
    bad("on: push\njobs: { a: { steps: [{ run: ls }] } }", "needs image:");
    bad("on: push\njobs: { a: { image: x, needs: b, steps: [{ run: ls }] } }", "needs b, which isn't a job");
    bad("on: push\njobs: { a: { image: x, needs: b, steps: [{ run: ls }] }, b: { image: x, needs: a, steps: [{ run: ls }] } }", "circle");
    bad("on: push\njobs: { a: { image: x, steps: [] } }", "needs a list of steps");
    bad("on: deploy\njobs: { a: { image: x, steps: [{ run: ls }] } }", "doesn't know the trigger deploy");
    bad("on: push\njobs: { a: { image: x, runs-on: ubuntu-latest, steps: [{ run: ls }] } }", "isn't an executor");
    bad("on: push\njobs: { a: { image: x, steps: [{ run: 'echo ${{ matrix.' }] } }", "steps[0].run");
    bad("on: push\njobs: { a: { image: x, timeout-minutes: 0, steps: [{ run: ls }] } }", "between 1 and 1440");
    bad("on: push\njobs: { a: { image: x, resources: { memory: lots }, steps: [{ run: ls }] } }", "512M or 4G");
    bad("on: push\njobs: { a: { image: x, steps: [{ run: ls }], strategy: { matrix: { n: [1,2,3,4,5,6,7,8,9], m: [1,2,3,4,5,6,7,8] } } } }", "limit is 64");
    bad("jobs: { a: { image: x, steps: [{ run: ls }] } }", "needs `on:`");
    bad("on: push\njobs: { a: { image: x, steps: [{ run: ls }], colour: red } }", "unknown key colour");
    bad("on: push\njobs: [", "x.yml:");
}

#[test]
fn triggers() {
    let wf = Workflow::parse("ci.yml", CI).unwrap();
    let push = |r: &str| Event::Push { name: r.into(), sha: "s".into() };
    assert!(wf.triggered_by(&push("refs/heads/main")));
    assert!(wf.triggered_by(&push("refs/heads/release/1.0/hotfix")), "** crosses slashes");
    assert!(!wf.triggered_by(&push("refs/heads/feature")));
    assert!(!wf.triggered_by(&push("refs/tags/v1")), "tags only when listed");
    let pr = |t: &str| Event::PullRequest { number: 1, source: "f".into(), target: t.into(), sha: "s".into() };
    assert!(wf.triggered_by(&pr("main")));
    assert!(!wf.triggered_by(&pr("dev")));
    assert!(wf.triggered_by(&Event::Dispatch { branch: "x".into(), sha: "s".into() }));

    let any = Workflow::parse("a.yml", "on: [push]\njobs: { a: { image: x, steps: [{ run: ls }] } }").unwrap();
    assert!(any.triggered_by(&push("refs/heads/anything")));
    assert!(!any.triggered_by(&Event::Dispatch { branch: "x".into(), sha: "s".into() }));
    let tags = Workflow::parse("t.yml", "on: { push: { tags: ['v*'] } }\njobs: { a: { image: x, steps: [{ run: ls }] } }").unwrap();
    assert!(tags.triggered_by(&push("refs/tags/v1.2")));
    assert!(!tags.triggered_by(&push("refs/heads/main")), "a tags-only filter ignores branches");
    let ignore = Workflow::parse("i.yml", "on: { push: { branches-ignore: ['wip/*'] } }\njobs: { a: { image: x, steps: [{ run: ls }] } }").unwrap();
    assert!(ignore.triggered_by(&push("refs/heads/main")));
    assert!(!ignore.triggered_by(&push("refs/heads/wip/x")));
    let neg = Workflow::parse("n.yml", "on: { push: { branches: ['**', '!docs/**'] } }\njobs: { a: { image: x, steps: [{ run: ls }] } }").unwrap();
    assert!(neg.triggered_by(&push("refs/heads/feat/a")));
    assert!(!neg.triggered_by(&push("refs/heads/docs/a")));
}

#[test]
fn globs() {
    assert!(glob("release/*", "release/1"));
    assert!(!glob("release/*", "release/1/2"));
    assert!(glob("release/**", "release/1/2"));
    assert!(glob("v?.*", "v1.0"));
    assert!(!glob("main", "maintenance"));
}

#[test]
fn plans_a_matrix_and_needs() {
    let wf = Workflow::parse("ci.yml", CI).unwrap();
    let event = Event::Push { name: "refs/heads/main".into(), sha: "abc".into() };
    let cx = expr::Context::default().with("neutrino", event.context("app"));
    let jobs = plan(&wf, &cx).unwrap();
    let names: Vec<&str> = jobs.iter().map(|j| j.name.as_str()).collect();
    assert_eq!(names, vec!["test (stable, debug)", "test (stable, release, lto)", "test (nightly, debug)", "deploy"]);
    assert_eq!(jobs[1].matrix.get("extra"), Some(&json!("lto")), "an include extends a matching combination");
    assert_eq!(jobs[3].needs, vec!["test-1", "test-2", "test-3"], "needs wait for every matrix entry");
    assert_eq!(event.context("app")["branch"], json!("main"));
}

#[test]
fn memory_units() {
    assert_eq!(workflow::parse_memory("512M"), Some(512));
    assert_eq!(workflow::parse_memory("6G"), Some(6144));
    assert_eq!(workflow::parse_memory("1.5GiB"), Some(1536));
    assert_eq!(workflow::parse_memory("2048"), Some(2048));
    assert_eq!(workflow::parse_memory("big"), None);
}

#[test]
fn the_rest_of_ci() {
    let src = r#"
on:
  schedule: [{ cron: "0 3 * * *" }]
  push: {}
concurrency: { group: "deploy-${{ neutrino.branch }}", cancel-in-progress: true }
jobs:
  build:
    image: rust:1.85
    environment: production
    concurrency: prod
    cache:
      - key: cargo-${{ hashFiles('Cargo.lock', '**/*.toml') }}
        path: [target, .cargo]
        restore-keys: [cargo-]
    artifacts: [target/release/app, "dist/*.zip"]
    services:
      db: { image: postgres:17, env: { POSTGRES_PASSWORD: dev } }
      cache: redis:7
    steps: [{ run: make }]
"#;
    let wf = Workflow::parse("deploy.yml", src).unwrap();
    assert!(wf.warnings.is_empty(), "{:?}", wf.warnings);
    assert_eq!(wf.concurrency.as_ref().unwrap().group, "deploy-${{ neutrino.branch }}");
    let j = &wf.jobs[0];
    assert_eq!(j.environment.as_deref(), Some("production"));
    assert_eq!(j.concurrency.as_ref().unwrap(), &workflow::Concurrency { group: "prod".into(), cancel_in_progress: false });
    assert_eq!(j.cache[0].paths, vec!["target", ".cargo"]);
    assert_eq!(j.artifacts, vec!["target/release/app", "dist/*.zip"]);
    assert_eq!(j.services.iter().map(|s| (s.name.as_str(), s.image.as_str())).collect::<Vec<_>>(), vec![("db", "postgres:17"), ("cache", "redis:7")]);
    assert!(wf.triggered_by(&Event::Schedule { branch: "main".into(), sha: "s".into() }));

    let bad = |src: &str, needle: &str| {
        let e = Workflow::parse("x.yml", src).unwrap_err().0;
        assert!(e.contains(needle), "{e}");
    };
    bad("on: push\njobs: { a: { image: x, cache: [{ key: k, path: /etc }], steps: [{ run: ls }] } }", "inside the workspace");
    bad("on: push\njobs: { a: { image: x, artifacts: [../up], steps: [{ run: ls }] } }", "inside the workspace");
    bad("on: { schedule: [{ cron: '61 * * * *' }] }\njobs: { a: { image: x, steps: [{ run: ls }] } }", "outside 0-59");

    // hashFiles reads the workspace.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.lock"), "a").unwrap();
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/x.toml"), "b").unwrap();
    let mut cx = expr::Context::default();
    cx.root = Some(dir.path().to_path_buf());
    let one = expr::interpolate("${{ hashFiles('Cargo.lock') }}", &cx).unwrap();
    let both = expr::interpolate("${{ hashFiles('Cargo.lock', '**/*.toml') }}", &cx).unwrap();
    assert_eq!(one.len(), 64);
    assert_ne!(one, both);
    assert_eq!(expr::interpolate("${{ hashFiles('nothing') }}", &cx).unwrap(), "");
    std::fs::write(dir.path().join("Cargo.lock"), "changed").unwrap();
    assert_ne!(expr::interpolate("${{ hashFiles('Cargo.lock') }}", &cx).unwrap(), one, "the hash follows the contents");
}

#[test]
fn path_filters() {
    let only = Workflow::parse("a.yml", "on: { push: { branches: [main], paths: ['firebase/*.rules', 'firebase/**', '!firebase/README.md'] } }\njobs: { a: { image: x, steps: [{ run: ls }] } }").unwrap();
    let ignore = Workflow::parse("b.yml", "on: { push: { branches: [main], paths-ignore: ['docs/**', '*.md'] }, pull_request: { paths-ignore: ['docs/**'] } }\njobs: { a: { image: x, steps: [{ run: ls }] } }").unwrap();
    let push = Event::Push { name: "refs/heads/main".into(), sha: "s".into() };
    let files = |f: &[&str]| f.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(only.triggered_by_changes(&push, Some(&files(&["firebase/firestore.rules", "src/a.ts"]))));
    assert!(!only.triggered_by_changes(&push, Some(&files(&["src/a.ts"]))));
    assert!(!only.triggered_by_changes(&push, Some(&files(&["firebase/README.md"]))), "a later ! excludes");
    assert!(only.triggered_by_changes(&push, None), "unknown changes run");
    assert!(!ignore.triggered_by_changes(&push, Some(&files(&["docs/a.md", "README.md"]))), "only ignored files changed");
    assert!(ignore.triggered_by_changes(&push, Some(&files(&["docs/a.md", "src/main.rs"]))));
    let pr = Event::PullRequest { number: 1, source: "f".into(), target: "main".into(), sha: "s".into() };
    assert!(!ignore.triggered_by_changes(&pr, Some(&files(&["docs/x"]))));
    assert!(Workflow::parse("c.yml", "on: { push: { paths: [a], paths-ignore: [b] } }\njobs: { a: { image: x, steps: [{ run: ls }] } }").is_err());
}

/// Nexus's three GitHub workflows, translated (docs/examples/nexus), parse
/// cleanly and fire when the originals did.
#[test]
fn nexus_workflows() {
    let load = |f: &str, src: &str| {
        let wf = Workflow::parse(f, src).unwrap_or_else(|e| panic!("{e}"));
        assert!(wf.warnings.is_empty(), "{f}: {:?}", wf.warnings);
        wf
    };
    let rules = load("deploy-firestore.yml", include_str!("../../../docs/examples/nexus/deploy-firestore.yml"));
    let hosting = load("deploy.yml", include_str!("../../../docs/examples/nexus/deploy.yml"));
    let windows = load("desktop-windows.yml", include_str!("../../../docs/examples/nexus/desktop-windows.yml"));
    let main = Event::Push { name: "refs/heads/main".into(), sha: "s".into() };
    let files = |f: &[&str]| f.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    assert!(rules.triggered_by_changes(&main, Some(&files(&["firebase/firestore.rules"]))));
    assert!(!rules.triggered_by_changes(&main, Some(&files(&["src/App.tsx"]))));
    assert!(hosting.triggered_by_changes(&main, Some(&files(&["src/App.tsx"]))));
    assert!(!hosting.triggered_by_changes(&main, Some(&files(&["cluster/x.py", "README.md"]))));
    assert!(!windows.triggered_by(&main));
    assert!(windows.triggered_by(&Event::Push { name: "refs/tags/v1.4.0".into(), sha: "s".into() }));
    assert_eq!(windows.jobs[0].runs_on, Executor::Nexus);
    assert_eq!(windows.jobs[0].nexus.node.as_deref(), Some("forge"));
    assert_eq!(windows.jobs[0].steps[0].shell.as_deref(), Some("bash"));
    assert!(!format!("{:?}", windows.jobs[0]).contains("secrets."), "nothing secret goes to Nexus");
    assert_eq!(hosting.jobs[0].environment.as_deref(), Some("production"));
    assert!(rules.concurrency.as_ref().is_some_and(|c| !c.cancel_in_progress));
}

/// The workflows of the repositories moved from GitHub (docs/examples): each
/// parses without warnings, plans, and fires when the original did, with the
/// macOS and Windows jobs kept out of everyday runs.
#[test]
fn moved_repo_workflows() {
    let files: [(&str, &str); 8] = [
        ("flux/advisories.yml", include_str!("../../../docs/examples/flux/advisories.yml")),
        ("flux/perf.yml", include_str!("../../../docs/examples/flux/perf.yml")),
        ("flux/native.yml", include_str!("../../../docs/examples/flux/native.yml")),
        ("prism/ci.yml", include_str!("../../../docs/examples/prism/ci.yml")),
        ("neutron/ci.yml", include_str!("../../../docs/examples/neutron/ci.yml")),
        ("neutron/native.yml", include_str!("../../../docs/examples/neutron/native.yml")),
        ("proton/ui-and-paths.yml", include_str!("../../../docs/examples/proton/ui-and-paths.yml")),
        ("proton/native.yml", include_str!("../../../docs/examples/proton/native.yml")),
    ];
    let push = |b: &str| Event::Push { name: format!("refs/heads/{b}"), sha: "s".into() };
    let pr = |target: &str| Event::PullRequest { number: 1, source: "f".into(), target: target.into(), sha: "s".into() };
    let tag = Event::Push { name: "refs/tags/v1.0.0".into(), sha: "s".into() };
    for (name, src) in files {
        let wf = Workflow::parse(name, src).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(wf.warnings.is_empty(), "{name}: {:?}", wf.warnings);
        let main = if name.starts_with("neutron/") { "master" } else { "main" };
        for event in [push(main), pr(main), tag.clone()] {
            if !wf.triggered_by(&event) {
                continue;
            }
            let cx = expr::Context::default().with("neutrino", event.context(name));
            let jobs = plan(&wf, &cx).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!jobs.is_empty(), "{name}");
        }
        let native = name.ends_with("native.yml");
        assert_eq!(wf.triggered_by(&push(main)), !native, "{name}: native jobs stay out of everyday pushes");
        assert!(!native || wf.triggered_by(&tag), "{name}: native jobs run for a version tag");
        for job in &wf.jobs {
            let off_graviton = job.runs_on != Executor::Graviton;
            assert_eq!(off_graviton, native, "{name}: {} runs where it should", job.id);
            assert!(!format!("{job:?}").contains("secrets."), "{name}: no secrets in these");
        }
    }
    let advisories = Workflow::parse("a.yml", include_str!("../../../docs/examples/flux/advisories.yml")).unwrap();
    assert_eq!(advisories.on.schedule, vec!["17 18 * * *".to_string()]);
    let prism = Workflow::parse("p.yml", include_str!("../../../docs/examples/prism/ci.yml")).unwrap();
    assert!(!prism.triggered_by(&pr("develop")), "Prism checks pull requests into main only");
}
