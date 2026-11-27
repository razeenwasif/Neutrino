# Neutrino architecture

Neutrino is a self-hosted, single-user replacement for the parts of GitHub you
pay for: repository hosting, branches, pull requests, diff review, CI/CD, and
secret storage. The server runs on a dedicated always-on Mac (`build-server`); the workstation
is a client. CI jobs run in Graviton containers on the server. Nothing leaves your
machines.

Status: milestones 1 to 8 are built (CI on Graviton, the host and through
Nexus, with secrets, environments, caches, artifacts and schedules; the iPhone
app with signed approvals and content-free push notifications), and the server
runs on your build host. See the README for what was measured and what's left.

## 1. Decisions and what they rest on

| # | Decision | Why |
|---|---|---|
| 1 | **Git-compatible storage**, not a new VCS format. | Every existing repo, editor, and tool keeps working, and you can leave at any time. "Custom engine" means our own Rust read/diff/index layer over git's object model. |
| 2 | **Rust daemon + SwiftUI app**, same shape as Graviton. | One codebase style across your apps; `GravitonKit`'s theme and components carry over. |
| 3 | **A dedicated machine is the server.** It hosts the bare repos, pull requests, CI and secrets. The workstation pushes to it. | It is always on, so pushes, schedules and long jobs do not depend on the laptop being open, and CI stops competing with your own work. |
| 4 | **One daemon binary, two roles**: `server` on the server, `workstation` on the laptop. | Working-copy operations (status, stage, commit) must be local and work offline; everything shared lives on the server. |
| 5 | **Workflow files are Actions-shaped YAML, `run:` steps only.** | Familiar syntax without taking on the Actions marketplace runtime (`uses:`), which is most of the complexity of a runner. |
| 6 | **Three executors: `graviton` (Linux arm64 VM on the server), `host` (macOS on the server), `nexus` (remote workers, for Windows and GPU).** | Graviton runs Linux guests only; Xcode builds need macOS; Windows/GPU nodes are reachable only through Nexus. |
| 7 | **Neutrino runs its own jobs on the server instead of sending all CI through Nexus.** | Nexus mirrors job state and logs to Firestore and records the command, so secrets and private logs would leave the house. Nexus is used only where it is the only way in. |
| 8 | **Secrets go into a step's exec environment, never the container spec.** | Graviton saves a container's `RunSpec.env` to disk and shows it in Inspect (`gravitond/src/runtime.rs:511`). |

Decisions 1, 5 and 7 are the ones worth challenging before any code exists.

## 2. Topology

```
 Workstation (laptop, may be away)
 ┌───────────────────────────────────────────────────────────┐
 │ Neutrino.app ── Unix socket ── neutrinod (workstation)     │
 │                                 status · diff · commit     │
 │                                 working copies             │
 └───────┬───────────────────────────────────┬───────────────┘
         │ paired TLS (LAN or Tailscale)     │ git over SSH (host "my-server")
 Build server (always on)                    │
 ┌───────▼───────────────────────────────────▼───────────────┐
 │ neutrinod (server)                                         │
 │   forge · CI scheduler · vault · SQLite · bare repos       │
 │        │                    │                   │          │
 │   gravitond            host executor      nexus-coordinator│
 │   Linux job VMs        macOS / Xcode      (already there)  │
 └─────────────────────────────────────────────────┬─────────┘
                                                   │ Nexus agents
 Windows/GPU worker                         ┌──────▼──────────┐
                                            │ Windows and GPU │
                                            │ jobs            │
                                            └─────────────────┘
```

The iPhone app connects to the server the same way the Mac app does, over the
paired TLS endpoint (section 7).

### Crates

| Crate | Owns | Role |
|-------|------|------|
| `neutrino-proto` | Wire types for the control API. Source of truth; `NeutrinoKit/Wire.swift` mirrors it, with tests pinning the JSON on both sides (Graviton's pattern). | both |
| `neutrino-git` | In-process git engine on `gix` (gitoxide): refs, history walk, tree and blob diff, status, blame, merge-base, commit and branch writes. | both |
| `neutrino-diff` | Hunks, intra-line word diff, rename detection, tree-sitter highlighting, binary and image handling, caching. | both |
| `neutrino-remote` | TLS endpoint, pairing codes, pinned certificates, device tokens, Bonjour. Lifted from `gravitond/src/remote.rs`. | both |
| `neutrino-store` | SQLite schema and migrations; log and artifact files. | server |
| `neutrino-forge` | Pull requests, review comments, branch rules, merges. | server |
| `neutrino-ci` | Workflow parser, expression evaluator, trigger matching, job graph, executors, reaper. | server |
| `neutrino-vault` | Secrets and variables: encryption, scoping, log masking. | server |
| `neutrinod` | The daemon: socket server, event bus, hook endpoint, schedules. | both |
| `ntr` | CLI client. | both |

### Apps

`apps/NeutrinoKit` (models, socket client, remote client, demo engine,
theme), `apps/Mac` and `apps/iOS`. The Mac app holds two connections: the
local daemon for working copies and the server for everything else. It keeps
no repository state of its own and falls back to demo data when neither is
reachable, as Graviton's does. The iPhone app is described in section 7.

The app icon is in `design/` (`icon.svg` for iOS, `icon-macos.svg` with the
macOS plate); `design/make-icons.sh` renders both into the asset catalogs.

### Away from home

- **With Tailscale** (the server has a Tailscale address in Nexus's fleet
  data): everything works as on the LAN.
- **Without a route to the server**: working copies work fully. Pull requests and
  runs show the last state the app saw, read-only. Pushes wait.

## 3. Git engine

Reads never fork a `git` process. `gix` opens packs memory-mapped and the
daemon keeps one open handle per repo.

- **History**: commit-graph file for generation numbers; the log is paged and
  the graph lanes are laid out per page, so a 500k-commit repo costs the same
  to open as a small one.
- **Status**: an FSEvents watcher per working copy marks dirty paths, so
  status re-stats only what changed.
- **Diff**: histogram line diff, then word diff inside changed lines. Results
  are cached by `(old blob, new blob, options)`; blobs are immutable, so the
  cache never invalidates, only evicts (LRU on disk).
- **Highlighting**: tree-sitter, cached per blob id.
- **Maintenance**: the server writes commit-graphs and repacks incrementally
  in the background after pushes.

Working-copy diffs are computed on the workstation. Pull request diffs are
computed on the server, which always has both sides, and streamed as hunks.

Stock `git` is still used where it is the right tool:

- **Transport.** Hosted repos are plain bare repos, so the workstation pushes and
  fetches over SSH (`ssh://my-server/~/.neutrino/repos/<name>.git`). The app sets
  the remote up for you.
- **Merges in bare repos**: `git merge-tree --write-tree` computes a merge and
  its conflicts without a working tree.
- **Anything `gix` does not cover yet.** Its coverage of rebase and some write
  paths has to be checked at milestone 1, not assumed.

Targets (to be measured, warm cache): first page of history under 30 ms,
status on a 100k-file tree under 50 ms, first paint of a 10k-line diff under
100 ms.

## 4. Control API

JSON lines, one protocol on two transports:

- **Local**: a Unix socket, for the app and `ntr` talking to the daemon on the
  same machine, and for git hooks on the server.
- **Remote**: TLS on the server, optionally advertised over Bonjour. A device pairs once with a
  one-time code, pins the server's certificate, and keeps its token in the
  Keychain; the server stores only token hashes. This is gravitond's remote
  design, reused.

Two differences from gravitond's protocol:

- Every request carries an `id`, so one connection multiplexes requests and
  any of them can be cancelled (the user scrolled away from a diff).
- `subscribe` opens an event stream: ref changes, run and job status, log
  tails. The app never polls.

Large results (history, diffs, logs) are paged.

## 5. Forge

**Hosted repos** live on the server in `~/.neutrino/repos/<name>.git`. Server-side
hooks are small scripts that call the daemon:

- `pre-receive` asks the daemon whether each ref update is allowed (branch
  rules). The daemon answers allow or deny with a message.
- `post-receive` reports the updated refs; the daemon emits `push` events and
  updates open pull requests.

**Pull requests** are rows, plus refs that pin their commits:

- `refs/pull/N/head` keeps the head alive after the branch is deleted.
- Each push to the source branch is stored as a new *version*, so the UI can
  show what changed since the last review.
- Mergeability is computed with `merge-tree` whenever either side moves.
- Merge strategies: merge commit, squash, rebase, fast-forward only. The
  server performs them in the bare repo.

**Review comments** anchor to `(commit, path, side, line)`. On a new version
the server maps the anchor through the diff between versions and marks it
outdated if the line is gone.

**Branch rules**: pattern, require a pull request, required checks, no force
push, no delete. With one user these are guardrails against yourself, so each
can be overridden with an explicit confirmation that is recorded.

## 6. CI/CD

### Workflow files

`.neutrino/workflows/*.yml`, read from the commit being built.

```yaml
name: ci
on:
  push: { branches: [main] }
  pull_request: {}
  schedule: [{ cron: "0 3 * * *" }]
  workflow_dispatch: {}

jobs:
  test:
    runs-on: graviton            # or: host, nexus
    image: rust:1.85
    resources: { cpus: 4, memory: 6G }
    timeout-minutes: 20
    services:
      db: { image: postgres:17, env: { POSTGRES_PASSWORD: dev } }
    cache:
      - { key: "cargo-${{ hashFiles('Cargo.lock') }}", path: target }
    steps:
      - run: cargo test --workspace
        timeout-minutes: 15
    artifacts: [target/release/myapp]

  deploy:
    needs: [test]
    environment: production      # scoped secrets, manual approval
    steps:
      - run: ./scripts/deploy.sh
        env: { TOKEN: "${{ secrets.DEPLOY_TOKEN }}" }
```

Supported: `on`, `env`, `jobs.*.needs`, `if`, `strategy.matrix`, `services`,
`concurrency` (with cancel-in-progress), `timeout-minutes`,
`continue-on-error`, and the `${{ }}` contexts `env`, `vars`, `secrets`,
`matrix`, `needs`, and `neutrino` (ref, sha, event). Not supported: `uses:`.
Checkout, cache and artifacts are built into the engine instead.

### Scheduler and sharing the server with Nexus

Events become runs; a run is a graph of jobs. Jobs wait in a queue ordered
manual, pull request, push, schedule.

The build server has 12 threads and 16 GB, and Nexus already places CPU jobs and
llama.cpp models there. Nexus places by reservation, not by measured use, so
it cannot see a CI VM's memory. Two schedulers on one small machine is the
main operational risk of this design.

- **Version 1**: CI gets a fixed budget on the server (a setting; start around 4
  threads and 6 GB), and before admitting a job the scheduler asks the Nexus
  coordinator what is running on the server and waits while it is busy.
- **Proper fix**: a reservation endpoint in the Nexus coordinator, so Neutrino
  holds memory the same way a Nexus job does. A small change to Nexus.

### A job on the Graviton executor

1. **Checkout** the commit from the bare repo into
   `runs/<run>/<job>/workspace`. Both are on the server, so nothing is uploaded.
2. **Run** the container with an idle command, the workspace shared at
   `/workspace`, cache volumes attached, and only non-secret env:
   `Run(RunSpec { name: "ntr-<run>-<job>-<attempt>", command: ["sleep","infinity"], volumes, cpus, memory_mib })`.
3. **Steps**: one `Exec` per step. `ExecOutput` chunks pass through the secret
   masker, are appended to the job's log file, and fan out to subscribers.
   `ExecExited` sets the step's result.
4. **Collect** artifacts from the workspace; it is on the server's disk, so no copy
   out of the VM is needed.
5. **Teardown** always runs: `Stop`, `Remove`, delete the workspace.

Service containers start before step 1's container and are reached by the IP
address Graviton reports, exported to the job as `<NAME>_HOST`.

### Timeouts and cleanup

- **Deadlines** are timers in the daemon, per step and per job, plus a
  no-output timeout for hung steps. On expiry the daemon calls `Stop`, which
  in Graviton means SIGTERM, ten seconds of grace, then VM power-off
  (`runtime.rs:44`). Cancelling a run takes the same path.
- **Reaper**: at daemon start and every minute, list Graviton containers named
  `ntr-*` and remove any that no live job owns. The same pass deletes orphaned
  workspaces.
- **Lost runners**: if a job's container is gone or shows as stopped (gravitond
  restarted), the job fails as "runner lost" and is retried once.
- **Retention**: logs and artifacts expire by age and total size; cache
  volumes are evicted least-recently-used under a size cap.

### The host executor

`runs-on: host` runs steps as child processes on the server in a scratch checkout,
in their own process group so a timeout can kill the whole tree. It exists for
Xcode builds, signing and simulators, so the server needs Xcode and your signing
identities. It is **not isolated**: a host job can read anything your account
on the server can. Keep it for repos you trust, which here means your own.

### The Nexus executor

`runs-on: nexus` submits the job to the Nexus coordinator, which places it on
a Windows/GPU worker. It exists for what the macOS server cannot do: Windows builds (Nexus's own desktop
installer builds on `windows-latest` today) and GPU tests. The job fetches its
commit from the server itself. Limits, all inherited from Nexus:

- Job state, the command and logs are mirrored to Firestore.
- **No secrets** are passed to Nexus jobs in version 1.
- Timeouts, cancellation and cleanup are Nexus's; Neutrino only relays them.

## 7. The iPhone app

A companion for what you do away from the Mac: see what needs you, review and
merge pull requests, watch runs, approve deploys, cancel a run. It has no
working copies and does not commit; a phone is the wrong place to write code.

**Screens**: Inbox (approvals waiting, pull requests ready to merge, running
and recent runs), Pull request (overview, checks, versions, files with diffs
and line notes, merge with a strategy), Run (jobs, steps, live log, cancel),
and Pair with server.

**Connection**: the same paired TLS endpoint on the server as the Mac app, found
over Bonjour at home and reached over Tailscale elsewhere. Pairing works as in
Graviton: a one-time code from `ntr pair` or the Mac app, the server's
certificate pinned on the phone, and the token in the phone's Keychain.
Revoking the device on the Mac stops the token working.

**Approvals**: approving a deploy, merging, and cancelling a run ask for Face
ID. At pairing the phone makes a key in its Secure Enclave and gives the
server the public half; those requests (and turning a deploy down) are
signed with it over the request, the time and a one-use nonce, so a stolen
token alone cannot approve anything, and a captured request can't be replayed
or altered. A device with a key can't send them unsigned.

**Notifications** are the one place the design leaves the house: iOS wakes a
closed app only through Apple's push service (APNs), so Neutrino uses it, and
keeps what it sends to a minimum.

- **What the server sends**: a push whose payload is only an opaque event id and a
  generic alert ("Something needs you"). No repo names, branches,
  commit messages, or log text. Apple learns that something happened, not
  what.
- **What the phone does**: a Notification Service Extension wakes on the
  push, fetches the event from the server over the paired TLS connection, and
  rewrites the alert with the real text ("Deploy #212 needs approval") before
  it is shown. If the server is unreachable, the generic alert stays.
- **Which events**: a deploy waiting for approval and failed checks, each a
  switch on the phone. Everything else waits for the app to open.
- **Setup**: an APNs auth key (`.p8`) from your Apple Developer account,
  sealed under the vault's key (which is in the server's Keychain), plus the app's Push
  Notifications entitlement. The phone gives the server its device token at
  pairing; revoking the device deletes it.
- **Without the key** everything else works; the Inbox is current whenever the
  app is opened.

## 8. Secrets and variables

- **Scopes**: global, repo, environment. The most specific wins.
- **At rest**: each secret is encrypted (XChaCha20-Poly1305) with a key in
  the server's login Keychain, readable only by `neutrinod`'s signature. Variables
  are plaintext and visible.
- **Recovery key**: shown once at setup. Without it, a dead server means every
  secret has to be re-entered, even with a database backup.
- **The server must be logged in** for the Keychain to be unlocked after a reboot,
  the same constraint the Nexus agent already has as a login item.
- **In the app**: secrets are write-only. Revealing one asks for Touch ID on
  the MacBook, and the server honours it only from a paired device. The
  iPhone app cannot reveal secrets.
- **In a job**: resolved when a step starts and passed in that step's
  `ExecSpec.env`. They are never written to the run record, the container
  spec, or a log.
- **Masking**: the log masker replaces each secret value (and its base64
  form) in output, with a carry-over window so a value split across two
  chunks is still caught. It cannot stop a job from writing a secret into an
  artifact.
- **Environments** can require approval: the job waits until you approve it in
  the app.

## 9. Storage

On the server:

```
~/.neutrino/
  neutrinod.sock
  neutrino.db            repos, pull requests, comments, runs, jobs, steps,
                         secrets (ciphertext), variables, rules, devices
  repos/<name>.git       hosted bare repos
  runs/<run>/<job>/      workspace (deleted at teardown), log.zst, steps.idx
  artifacts/<run>/
  cache/                 diff and highlight caches
```

On the MacBook: `~/.neutrino/` holds the socket, the list of working copies,
the diff cache, and the last-seen server state for offline viewing.

Logs are append-only compressed files with an index of step offsets; SQLite
holds metadata only.

## 10. What Graviton needs

Verified against Graviton's source. Graviton also has to be installed and
running on the server. Version 1 works with none of the changes below, but the
first is close to required on a 16 GB machine.

| Gap | Today | Change |
|---|---|---|
| **Root filesystem is in RAM.** A container's files unpack into a tmpfs sized at 90% of the VM's memory (`graviton-agent/src/container.rs:43`). | A `rust:1.85` job needs several GiB of memory before it compiles anything, which on a 16 GB host means one such job at a time. The workspace and caches avoid this because they are shared folders and volumes. | Disk-backed root filesystem. The most valuable change for CI. |
| `ExecSpec` has no working directory or user. | Wrap each step in `sh -c 'cd … && …'`. | Expose `cwd` and `user`; the agent's `Exec` already takes both. |
| No labels on containers. | Ownership by `ntr-` name prefix. | Labels on `RunSpec`. |
| Containers and their logs are saved. | The reaper removes them. | An ephemeral flag that skips saved state. |
| Containers cannot find each other by name. | Pass service IPs as env. | Name resolution on the NAT network. |
| No network policy. | Jobs have full outbound access. | Per-container egress off. |
| No change events. | Poll `ListContainers`. | A subscribe request. |

Also true today: guests are arm64 only, so x86-only images will not run.

## 11. Risks

- **Server capacity.** See the scheduler section. Until the root
  filesystem is disk-backed and Nexus knows about CI's memory, expect one
  heavy job at a time.
- **Backups.** Git history is on the server, in every working copy and, for
  repositories imported from GitHub, in their GitHub mirror, which gets every
  push. Pull requests, comments, run history and secrets are in the server's
  database: it's snapshotted nightly, and the workstation's neutrinod copies the
  snapshots over SSH (`--copy-backups-from my-server`), so a dead server costs at
  most a day of them.
- **Scope.** This is a forge, a git client, and a CI system across two
  machines. The milestones below are ordered so each one is usable alone.
- **Outside services.** Jobs sent through the Nexus executor are visible in
  Firestore, and Apple's push service sees when (not what) a notification
  is sent. Everything else stays on your machines.

## 12. Milestones

0. **UI prototype**: history, diff viewer, branches, pull request, run view,
   secrets. Clickable, demo data.
1. **Read path**: workstation daemon, socket, repo list, history, diff. The
   app browses real repos on the workstation.
2. **Working copy**: status, stage by hunk and line, commit, branches.
3. **Server role**: server role, pairing, hosted repos, push and fetch
   over SSH, hooks.
4. **Pull requests**: versions, comments, mergeability, merge strategies,
   branch rules.
5. **CI**: workflow parser, Graviton executor on the server, live logs, timeouts,
   reaper, the fixed budget.
6. **Vault and the rest of CI**: secrets, variables, environments, caches,
   artifacts, schedules, required checks, database backup.
7. **Host and Nexus executors.** Moving macOS/Xcode and Windows/GPU jobs onto Neutrino.
8. **iPhone app**: pairing, Inbox, pull requests, runs, signed approvals,
   then push notifications through APNs with content-free payloads.
9. **Moving in**: import GitHub repositories onto the server and mirror every
   push back to GitHub, so a full copy of each lives off the server; copy the
   nightly database snapshots to the workstation; a script that moves working
   copies over. Workflows move by hand, one `.neutrino/workflows/` file at a
   time.
