# Neutrino

A self-hosted replacement for the parts of GitHub you pay for: repositories,
branches, pull requests, diff review, CI/CD and secrets, on your own
machines. A Rust engine reads git repositories in-process with gitoxide; a
SwiftUI app shows them. The design is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

> Status: **milestone 9 done** (moving in from GitHub): `ntr import` brings a
> GitHub repository's branches and tags onto the server, which copies every
> push back to GitHub so a full copy lives off atlas; the MacBook keeps copies
> of the server's nightly database snapshots; `scripts/move-in.sh` moves
> working copies over. Milestone 8 gave the iPhone app. The phone shows what needs
> you (deploys waiting for approval, pull requests ready to merge, runs) and
> approves, turns down, merges and cancels, each signed with a key in its
> Secure Enclave after Face ID. Push notifications go through APNs carrying no
> details; the phone fetches the text from the server. Milestone 7 gave the
> other executors (`runs-on: host`, `runs-on: nexus`); 6 the vault and the rest
> of CI; 5 CI on Graviton; 4 pull requests; 3 the server on atlas; 1 and 2 the
> client.

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
  iOS/             iPhone app: Inbox, pull requests with diffs, runs with logs, approvals
  iOSNotify/       Notification service extension: swaps a push's placeholder for the real text
design/            App icon sources; make-icons.sh renders them into the asset catalog
scripts/
  dev-neutrinod.sh         Build and run the daemon in the foreground (pass --server for the server)
  install-launch-agent.sh  Run neutrinod at login (--server on atlas, --uninstall to remove)
  deploy-server.sh         Build, sign and copy neutrinod and ntr to the server, then restart it
  server-flags.sh          Set the flags the server's neutrinod starts with (executors, budget, backups)
  move-in.sh               Move working copies' repositories from GitHub to the server
```

## Running it

```bash
scripts/dev-neutrinod.sh                 # in one terminal
cargo run -q -p ntr -- repo add ~/code/myrepo   # register a working copy (any folder inside one)
cargo run -q -p ntr -- repos
cd ~/code/myrepo
cargo run -q -p ntr -- history --all -n 30      # read by neutrinod, with its graph
cargo run -q -p ntr -- branches                 # with ahead and behind
cargo run -q -p ntr -- unstage README.md        # or: discard, which keeps a copy
cargo run -q -p ntr -- watch                    # print change events as they happen
```

`ntr` is one command for everything. Whatever it doesn't know itself is git's,
run in its place with the same arguments, so its pager, prompts, aliases and
exit code are git's own: `ntr status`, `ntr commit -m …`, `ntr rebase -i
main`, `ntr push`. Server commands (`pr`, `runs`, `run`, `secret`, `var`,
`env`, `rules`, `mirror`, `inbox`, `ci` and the rest) run on the server: on a
workstation, `ntr` sends them over SSH to the server its working copy's origin
is on (`ssh://mini/…`), naming the hosted repository by its path there, so
from any working copy `ntr pr list` lists that repository's pull requests.
Outside one, it uses `git config --global neutrino.server` (or
`$NEUTRINO_SERVER`). On the server itself they run locally, as before.

```bash
git config --global neutrino.server mini   # once, for commands outside a working copy
ln -s ~/.neutrino/bin/ntr ~/.local/bin/ntr # or anywhere on your PATH
cd ~/code/myrepo
ntr status && ntr commit -am "Fix the README" && ntr push   # git
ntr pr create my-branch -t "Fix the README"                 # the server
ntr runs                                                    # the server
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

### Moving in from GitHub

```bash
ntr import flux --from git@github.com:you/flux.git      # on the server
ntr mirror                                              # each mirror: ok, pending or failing
ntr -r flux mirror now                                  # copy now and wait (or: set URL, off)
scripts/move-in.sh ~/code/flux ~/code/nexus             # on the MacBook: all of the below
```

`ntr import` creates a hosted repository from another one's branches and tags
(not GitHub's pull request refs) with the same default branch; nothing is left
behind if the copy fails. Unless told `--no-mirror`, it then copies the
repository back to where it came from after every push: every branch and tag,
with branches and tags deleted here deleted there too, so GitHub holds a full
copy that's at most a push behind. The copy runs in the background after the
push has landed; pushes meanwhile fold into one more. One that fails is tried
again every ten minutes, and the first failure goes to the phone.

A copy never overwrites what only GitHub has. It first fetches GitHub's
branches and tags into `refs/mirror/` on the server, then moves each one there
only when GitHub's tip is already in the server's history, or GitHub hasn't
moved since the last copy (so a rebase pushed to the server still goes
through). If GitHub has commits the server doesn't (a collaborator's push, a
pull request merged on github.com), that branch is left alone, the mirror
shows as failing with which branch and what to run, and the rest are copied:

```bash
git fetch origin refs/mirror/heads/main && git merge FETCH_HEAD && git push
```

A branch deleted on the server is deleted on GitHub only if the last copy put
it there and it hasn't moved since; one made on GitHub stays, and `ntr mirror`
lists it. Every update is leased on what the copy saw, so a push to GitHub in
the middle of one fails it rather than being lost. The mirror's URL is in the hosted repository's own git config
(`neutrino.mirror`) and the last copy's outcome beside it, in
`neutrino-mirror.json`. The server pushes with its own SSH key, so that key
must be able to push to the GitHub repository.

`scripts/move-in.sh DIR…` does a whole working copy from the MacBook: the
server imports its GitHub repository under the same name (a repository someone
else owns is imported without the mirror), origin is pointed at the server, the
GitHub URL stays as the `github` remote, and it lists the branches with commits
still to push. `--dry-run` says what it would do; `--disable-actions` turns
GitHub Actions off for what it imports, so the copies pushed there don't start
workflows (and use minutes). Workflows move by hand: copy a translation into
the repository's `.neutrino/workflows/`. [docs/examples](docs/examples) has
them for flux, nexus, Prism, Neutron and Proton, with a note at the top of each
file on what changed. Each repository's macOS and Windows jobs are in a
`native.yml` that runs for version tags and when started by hand, since host
jobs need `--host-executor` and every Nexus job is a ticket to accept.

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
(default `~/.neutrino/backups`), keeping the last 14. Secrets in a snapshot
stay sealed; the recovery code opens them. To keep copies off the server, start
the MacBook's neutrinod with `--copy-backups-from mini`: when it starts and
every hour it asks the server over SSH for its snapshots (`ntr backups
--paths`), copies the ones it's missing to `~/.neutrino/server-backups/`,
checks each is an SQLite file, and keeps the newest 14. A laptop that was away
catches up when it's back. `ntr backups` lists them, on either machine, and on
the MacBook says when copying last worked.

```bash
scripts/install-launch-agent.sh -- --copy-backups-from mini   # on the MacBook
ntr backups                                                   # the copies, and the last copy
```

### The iPhone app

```bash
cd apps && xcodegen generate && open Neutrino.xcodeproj   # scheme NeutrinoIOS
```

Pair it like the Mac app: on atlas run `ntr pair`, pick the server on the
phone (or type its address), check the fingerprint, enter the code. Pairing
also makes a P-256 key in the phone's Secure Enclave, usable only after Face
ID or the passcode, and gives the server its public half (once: a device can't
swap its key later). The Inbox lists deploys waiting for approval, pull
requests ready to merge and the rest that are open, unfinished and recent runs,
and what happened lately; a pull request opens to its checks and diff, a run
to its jobs, steps and logs.

Approving, turning down, merging and cancelling from the phone are signed: the
request, the time and a random nonce, so the server refuses one that was
changed, is more than five minutes old, or was sent before. Once a device has a
key the server refuses those requests from it unsigned, so a stolen token alone
can't approve a deploy. `ntr devices` shows which devices sign; the approver is
recorded by device name.

Notifications need an APNs key from your Apple developer account (Keys, with
Apple Push Notification service). The server seals it like a secret, so set up
the vault first. On atlas:

```bash
ntr notify setup --key AuthKey_ABC123.p8 --key-id ABC123 --team-id YOUR_TEAM_ID --topic dev.neutrino.ios
ntr notify test                            # to every phone that takes pushes
ntr notify                                 # what's set up, and who receives
```

The server pushes when a deploy starts waiting for approval and when a run
fails. The push itself says only "Something
needs you" with an id; the notification extension asks the server for the
words, so repo names and branches never pass through Apple. Phones choose
which kinds they take in Settings.

### Working copies

Writes go through the `git` binary (Homebrew's, else Xcode's; `NEUTRINO_GIT`
picks another), so hooks, signing and your identity apply, and git's own
checks stand: a switch that would overwrite uncommitted work is refused, and
so is deleting an unmerged branch unless you force it. Discarding keeps a copy
of the file as an object in the repo and notes it in
`~/.neutrino/discarded.log`; `git cat-file -p <sha>` gets it back (until git
prunes unreachable objects, after two weeks by default). Untracked files go to
the Trash.

`ntr` picks the registered repo you're standing in, or `--repo ID` (or a
repo's path, which is how a workstation names a hosted repo to the server).
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
- The phone talks to the server directly, so away from home it needs a way
  back to atlas (Tailscale or similar); there's no relay.
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
- Mirrors copy branches and tags only (no Git LFS objects, no GitHub issues,
  releases or wikis). Commits pushed straight to the mirror stop that
  branch's copies until they're merged on the server. Mirror state isn't
  shown in the apps yet;
  `ntr mirror` and the phone's failure notice are where to look.
