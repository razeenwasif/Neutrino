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
    assert_eq!(wf.warnings, vec!["jobs.deploy.services is ignored for now: service containers arrive with milestone 6"]);
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
