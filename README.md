# Neutrino

A self-hosted replacement for the parts of GitHub you pay for: repositories,
branches, pull requests, diff review, CI/CD and secrets, on your own
machines. A Rust engine reads git repositories in-process with gitoxide; a
SwiftUI app shows them. The design is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

> Status: **milestone 7 done** (the other executors). Besides Graviton VMs, CI
> jobs can run as plain processes on the server's macOS (`runs-on: host`) or go
> to forge and the other nodes through Nexus (`runs-on: nexus`), and workflows
> can filter on the paths a push changed. Nexus's own three GitHub workflows
> are translated in `docs/examples/nexus/`. Milestone 6 gave the vault and the
> rest of CI; 5 CI on Graviton; 4 pull requests; 3 the server on atlas; 1 and 2
> the client. The iPhone app comes next.

## Layout

```
crates/
  neutrino-proto   Wire types for the control API, JSON lines with request ids
  neutrino-diff    Histogram line diffs, hunks, word highlights (imara-diff)
  neutrino-git     The git engine: reads on gitoxide (refs, history and graph lanes, commits,
                   diffs, status); writes through the git binary (stage, commit, branches);
                   forge.rs compares branches, checks and does merges, follows lines
  neutrino-store   The server's SQLite database: pull requests, versions, notes, rules, overrides,
                   runs, jobs and steps
  neutrino-ci      Workflow files, the ${{ }} expression language, triggers, matrix planning, cron
  neutrino-vault   Sealing secrets (XChaCha20-Poly1305), the master key, masking logs
  neutrino-remote  The server's TLS endpoint: certificate, pairing codes, device tokens, Bonjour
  neutrinod        The daemon, as workstation or --server: repo registry, Unix socket and TLS,
                   concurrent requests, cancel, file watching and change events, hosting, hooks
  ntr              CLI client
apps/
  project.yml      XcodeGen spec → Neutrino.xcodeproj (generated, not committed)
  NeutrinoKit/     Swift package: wire mirror, socket client, demo data, theme
  Mac/             macOS app: Changes, History, commit panel, diff view, Branches, Pull requests,
                   Runs, Secrets
design/            App icon sources; make-icons.sh renders them into the asset catalog
scripts/
  dev-neutrinod.sh         Build and run the daemon in the foreground (pass --server for the server)
  install-launch-agent.sh  Run neutrinod at login (--server on atlas, --uninstall to remove)
  deploy-server.sh         Build, sign and copy neutrinod and ntr to the server, then restart it
  server-flags.sh          Set the flags the server's neutrinod starts with (executors, budget, backups)
```

## Running it

```bash
scripts/dev-neutrinod.sh                 # in one terminal
cargo run -q -p ntr -- add ~/code/myrepo # register a working copy (any folder inside one)
cargo run -q -p ntr -- repos
cd ~/code/myrepo
cargo run -q -p ntr -- log --all -n 30   # with an ASCII graph
cargo run -q -p ntr -- show HEAD
cargo run -q -p ntr -- diff HEAD src/main.rs
cargo run -q -p ntr -- branches
cargo run -q -p ntr -- status
cargo run -q -p ntr -- diff README.md           # working tree; --staged for the index
cargo run -q -p ntr -- stage README.md
cargo run -q -p ntr -- commit -m "Fix the README"
cargo run -q -p ntr -- branch new feature --switch
cargo run -q -p ntr -- switch main
cargo run -q -p ntr -- watch                    # print change events as they happen
cargo run -q -p ntr -- remote list              # or: remote add NAME URL
cargo run -q -p ntr -- fetch
cargo run -q -p ntr -- push -u atlas
```

## The server

On atlas (any Mac that's always on):

```bash
scripts/install-launch-agent.sh --server   # neutrinod --server at login
ntr pair                                   # a one-time code and the server's fingerprint
ntr host myapp                             # or let the app create repos when publishing
ntr devices                                # paired devices; `devices revoke ID` forgets one
```

The server keeps hosted repos in `~/.neutrino/repos/<name>.git` and listens
for paired devices on TCP 7844 with TLS, advertised over Bonjour as
`_neutrino._tcp`. Pushes and fetches don't go through it: they use git over
SSH (`ssh://mini/Users/you/.neutrino/repos/myapp.git`), so Remote Login must
be on and your SSH key in its `authorized_keys`. Each hosted repo's
`pre-receive` and `post-receive` hooks call `neutrinod hook`, which tells the
daemon what landed; the first branch pushed to an empty repo becomes its
default, and the commit-graph is refreshed after each push.

In the Mac app, choose Pair with atlas in the sidebar, pick the server (or type
its address), check the fingerprint against `ntr pair`, then enter the code.
The app pins that certificate and keeps its token in the Keychain; the server
stores only token hashes. A paired device can read hosted repos, create them
and follow pushes; it can't touch the server's working copies or paths, or
manage pairing. Publish creates a hosted repo, adds it as the `atlas` remote
and pushes the current branch; Fetch and Push in the header do the rest.

### Pull requests and branch rules

They live on the server, in `~/.neutrino/neutrino.db`. From the app (Pull
requests, ⌘4) or on atlas with `ntr`:

```bash
ntr -r myapp pr create feature -t "Add the thing"   # into the default branch
ntr -r myapp pr show 1                               # versions, files, commits, notes
ntr -r myapp pr note 1 src/main.rs 42 "Why this?"    # --old for a removed line
ntr -r myapp pr merge 1 -s squash -d                 # merge|squash|rebase|ff, -d deletes the branch
ntr -r myapp rules set main                          # PRs only, no force-push, no delete
ntr -r myapp rules overrides
```

Each push to an open pull request's branch is a new version, kept alive by
`refs/pull/N/head` so force-pushes don't lose what a note was written on.
Notes are anchored to a line of the version they were written on and followed
through later versions; one whose line changed shows as outdated. A local repo
shows the pull requests of the hosted repo its remote points at.

Merges happen in the bare repo (`merge-tree`, `commit-tree`, then
`update-ref` with the old value, so a racing push wins and the merge fails
cleanly). They're committed as the repo's `user.name` if set, else as the
source branch's author; a rebase keeps each commit's author and date. Pushing
a merge you made locally marks the pull request merged.

Rules guard branches by name or pattern (`release/*`): no direct pushes, no
force-pushes, no deletes, each optional. The pre-receive hook enforces them.
`git push -o neutrino-override="why"` gets past one, and the override is
recorded. While a repo has rules its hook refuses pushes if neutrinod is down;
without rules a push lands regardless. Required checks can be listed but block
merging until CI exists.

### CI

Workflow files live in `.neutrino/workflows/*.yml` and are Actions-shaped,
with `run:` steps only (checkout is built in):

```yaml
name: ci
on:
  push: { branches: [main] }
  pull_request: {}
  workflow_dispatch: {}
jobs:
  test:
    image: rust:1.85               # runs-on: graviton is the default
    resources: { cpus: 4, memory: 6G }
    timeout-minutes: 20
    strategy: { matrix: { toolchain: [stable, nightly] } }
    steps:
      - run: cargo test --workspace
      - name: Clean up
        if: failure()
        run: echo "${{ matrix.toolchain }} failed"
```

Supported: `on` (push with branch, tag and `paths`/`paths-ignore` filters,
pull_request with branch and path filters, workflow_dispatch, schedule), `env` at every level, `needs`, `if` with
`success() failure() always() cancelled()`, `strategy.matrix` with include and
exclude, `timeout-minutes` and `idle-timeout-minutes`, `continue-on-error`,
`working-directory`, `environment`, `concurrency` (workflow and job, with
`cancel-in-progress`), `cache`, `artifacts`, `services`, and `${{ }}` with the
`neutrino`, `env`, `matrix`, `needs`, `vars` and `secrets` contexts and
`hashFiles()`. `uses:` is refused; `outputs` is read and warned about.

```yaml
jobs:
  build:
    image: rust:1.85
    cache:
      - key: cargo-${{ hashFiles('Cargo.lock') }}
        path: [target, .cargo]          # inside the workspace; set CARGO_HOME=.cargo
        restore-keys: [cargo-]
    artifacts: [target/release/app]
    services: { db: { image: postgres:17, env: { POSTGRES_PASSWORD: dev } } }   # $DB_HOST
  deploy:
    needs: build
    environment: production             # its secrets; waits for approval if it needs one
    concurrency: production
    steps:
      - run: ./deploy.sh
        env: { TOKEN: "${{ secrets.DEPLOY_TOKEN }}" }
```

Each job checks its commit out into `~/.neutrino/runs/<run>/<job>/workspace`,
starts a container named `ntr-…` through Graviton with the workspace at
`/workspace`, and runs each step there with `sh -e`. Output goes to
`runs/<run>/<job>.log`, and the database keeps where each step's part starts
and ends. Afterwards the container is stopped and removed and the workspace
deleted, whatever happened. A step past its timeout, or a cancelled run, stops
the container. A reaper removes `ntr-` containers and workspaces no job owns,
at start and every minute; jobs that were running when neutrinod stopped are
tried once more.

Caches are folders inside the workspace, saved after a passing job and copied
back before the next one with APFS clones, so restoring costs nothing; the
least recently used go past `--ci-cache 10G`. Artifacts are copied out after
the job and kept with its logs for `--retention-days 30`. Schedules are cron
lines in UTC, run on each repo's default branch. Services start before the
job's container, through Graviton, and the job reaches each at `<NAME>_HOST`.

CI gets a fixed budget on atlas (`--ci-cpus 4 --ci-memory 6G` by default);
jobs start in order while there's room, and one that asks for more than the
whole budget fails at once. Graviton is found at its usual socket, or
`--graviton-socket`.

```bash
ntr -r myapp runs                    # newest first; --pr N for one pull request's
ntr -r myapp run show 12             # jobs and steps
ntr -r myapp run logs 12 -j test -f  # follow a job's output
ntr -r myapp run cancel 12           # or: rerun 12, start ci -b main
ntr -r myapp workflows -b main       # the files on a branch, checked
ntr -r myapp ci                      # Graviton, the budget, what's queued
ntr -r myapp rules set main --check ci   # merging into main needs ci to pass
ntr -r myapp run approve 12          # let a job waiting on its environment run (or: reject)
ntr -r myapp run artifacts 12        # then: run download 12 build/target/release/app
```

### Where jobs run

| `runs-on` | Where | Isolation | Notes |
|---|---|---|---|
| `graviton` (default) | a Linux arm64 VM on the server | a VM per job | needs `image:`; services, caches |
| `host` | the server's macOS, as processes | none | for Xcode, signing, simulators; off unless `--host-executor` |
| `nexus` | a Nexus node (forge for Windows and the GPU) | Nexus's | `nexus: { node, accelerator, gpu-memory }`; no secrets |

Host jobs run in a fresh checkout in their own process group, so a timeout or
a cancel takes the whole tree, with Homebrew's and your usual tool folders on
the PATH. They can read anything your account on the server can, so keep them
for your own repositories.

Nexus jobs go to the coordinator the way `nexus cpu` does: Neutrino packs the
commit with a small Python driver and the job's steps (each in its own `shell:`
of bash, sh, cmd, pwsh or powershell), uploads it, submits it, follows its log,
turning the driver's markers back into steps, and fetches the artifacts from
the job's outputs. Nexus mirrors jobs and their logs to Firestore, so Neutrino
refuses to send secrets: give such jobs variables. The server needs a Nexus
token (`--nexus-token-file`; `--nexus-coordinator` if it isn't on the same
Mac). With the request-only token every job is a ticket that waits for your
Accept in Nexus, and can't be stopped from Neutrino; the full token runs them
straight away.

```bash
scripts/server-flags.sh mini -- --host-executor --nexus-token-file /Users/you/.duet/neutrino-token
ntr ci                                   # on the server: which executors are on
```

### Secrets, variables and environments

```bash
ntr vault setup                            # on the server, once: prints the recovery code
echo -n "$TOKEN" | ntr -r myapp secret set DEPLOY_TOKEN --env production
ntr secret set NPM_TOKEN --global          # typed without echo
ntr -r myapp secret list                   # names only, never values
ntr -r myapp var set REGION eu
ntr -r myapp env set production --approval --branch main
ntr backup                                 # a database snapshot now
```

Secrets are sealed with XChaCha20-Poly1305 under one master key, bound to
their name and scope so a sealed value can't be moved to another; the key is
in the server's login Keychain (`--vault-key-file` keeps it in a file instead).
The recovery code `ntr vault setup` prints is that key: write it down away from
the server, since with a database backup it's what opens the secrets anywhere
else (`ntr vault recover`). Nothing reads a secret back, not `ntr` and not the
app. A job gets the secrets of its repo, every repo and its environment, the
most specific of each name winning, only as the values its steps ask for; the
log masks each of them, their base64 form and each line of a multi-line one,
even when a value is split between two chunks of output. A job can still
write a secret into an artifact.

An environment can require approval: its jobs wait in the Runs screen (or
`ntr run approve`) and record who let them go; it can also limit which
branches may deploy. The server's login Keychain must be unlocked, so atlas
has to be logged in, as it already is for Nexus. `scripts/deploy-server.sh`
signs `neutrinod` with your Apple Development certificate before copying it,
so the Keychain keeps trusting it across deploys.

The database is snapshotted every night after 03:30 UTC to `--backup-dir`
(default `~/.neutrino/backups`; point it at an external disk or a synced
folder to keep copies off the server), keeping the last 14. Secrets in a
snapshot stay sealed; the recovery code opens them.

### Working copies

Writes go through the `git` binary (Homebrew's, else Xcode's; `NEUTRINO_GIT`
picks another), so hooks, signing and your identity apply, and git's own
checks stand: a switch that would overwrite uncommitted work is refused, and
so is deleting an unmerged branch unless you force it. Discarding keeps a copy
of the file as an object in the repo and notes it in
`~/.neutrino/discarded.log`; `git cat-file -p <sha>` gets it back (until git
prunes unreachable objects, after two weeks by default). Untracked files go to
the Trash.

`ntr` picks the registered repo you're standing in, or `--repo ID`.
State (the list of repos) lives in `~/.neutrino`; `NEUTRINO_HOME` and
`NEUTRINO_SOCKET` move it. Socket paths are limited to 104 bytes.

The Mac app:

```bash
cd apps && xcodegen generate && open Neutrino.xcodeproj
```

It connects to neutrinod when it's running and shows the prototype's demo
repository otherwise; it notices the daemon starting or stopping on its own.
⌘O adds a repository, ⌘R reloads, ⌘1 to ⌘6 switch between Changes, History,
Branches, Pull requests, Runs and Secrets, ⌘F searches, ↑ and ↓ move through commits, ⌘⇧D gives the diff
the full width. In Changes, tick a file to stage it, or click lines in its
diff to pick them and stage, unstage or discard just those (⌘⇧S); each hunk
has its own button; ⌘↩ commits.

Tests:

```bash
cargo test --workspace                     # builds real repos with git in temp folders
swift test --package-path apps/NeutrinoKit
NEUTRINO_LIVE=1 swift test --package-path apps/NeutrinoKit   # also drives a running neutrinod
NEUTRINO_LIVE_GRAVITON=1 cargo test -p neutrinod --test ci graviton   # a real job in a Graviton VM
NEUTRINO_LIVE_GRAVITON=1 cargo test -p neutrinod --test vault graviton_services   # a service it reaches
```

## Protocol

One JSON object per line over the Unix socket. Requests carry an `id`; each
answer carries the id it answers, so a client can keep several requests in
flight on one connection:

```json
{"id":7,"method":"log","params":{"repo":"nexus","all":true,"limit":50}}
{"id":7,"type":"log","data":{"commits":[...],"has_more":true,"lanes":3}}
{"id":8,"method":"cancel","params":{"id":7}}
```

`neutrino-proto` is the source of truth; `NeutrinoKit/Wire.swift` mirrors it
and both sides have tests pinning the JSON.

## Measured

On this MacBook, release build, warm cache:

| | |
|---|---|
| First page of history (100 commits), git.git with 78k commits | 8 ms (48 ms cold) |
| The same with every branch and tag | 73 ms |
| Search through messages, authors and hashes, first 20 hits | 21 ms |
| Your repos (up to 1.3k commits) | under 3 ms |

Per-file line counts match `git diff --histogram` on all 59 flux commits
checked.

## Known limits

- Paging deep into a long history re-walks from the top: page 200 of git.git
  (skip 20,000) takes 230 ms. Keeping the walk between pages would fix it.
- No syntax highlighting in diffs yet (tree-sitter is planned).
- Merge commits list their changes against the first parent only.
- The diff cache is in memory and capped at 512 diffs; it isn't kept on disk.
- The watcher skips common build folders (`target`, `node_modules`, `.build`,
  `DerivedData` and a few more) by name rather than by `.gitignore`.
- Line staging uses the working tree's bytes as they are, without git's
  clean filters (LFS, CRLF conversion); files that rely on them should be
  staged whole.
- Server: pushes go over SSH with your own keys; network git never prompts
  (`BatchMode`), so a key that needs a passphrase must be in the SSH agent.
- Server: the TLS endpoint takes one message (pair or authenticate) before
  anything else; a client that sends more at once is disconnected.
- Pull requests: each app request opens its own TLS connection; fine on a LAN,
  slow over a poor link. Notes don't follow a file across a rename. Comparing
  lists at most 250 commits. Mergeability is worked out on every read rather
  than cached.
- CI: Graviton guests are arm64 Linux, so x86-only images won't run. A job's
  container keeps its root filesystem in RAM (a Graviton limit), so big images
  need memory to match. After a timeout or a cancel the container is gone, so
  `if: always()` steps after it don't run. Caches and artifacts are paths
  inside the workspace. Host jobs aren't recovered after a restart of
  neutrinod (their processes keep going; the job is marked lost). A Nexus job's
  steps can use `if:` conditions with the status functions, which the driver
  applies; other conditions are worked out when the job is sent. Nexus jobs
  get no caches or services. Secrets can't be
  revealed in the app (Touch ID-gated reveal is still to come). Schedules
  don't catch up on minutes the server was down.
- Status reads the index but doesn't write back refreshed stat data, so after
  a large checkout the first status re-hashes files that git would have
  marked clean.
- Partial clones: file contents that were never fetched can't be diffed, and
  the app says so; Neutrino doesn't fetch them.
