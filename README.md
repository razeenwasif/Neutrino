# Neutrino

A self-hosted replacement for the parts of GitHub you pay for: repositories,
branches, pull requests, diff review, CI/CD and secrets, on your own
machines. A Rust engine reads git repositories in-process with gitoxide; a
SwiftUI app shows them. The design is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

> Status: **milestone 1 done** (the read path). The daemon reads the working
> copies you register on this Mac and the app browses them: history with a
> commit graph, every branch or one, search, commit details, and diffs with
> word-level highlights, unified or side by side. Nothing writes to your
> repositories yet; staging and committing are milestone 2, and everything
> that lives on atlas (pull requests, CI, secrets) comes after.

## Layout

```
crates/
  neutrino-proto   Wire types for the control API, JSON lines with request ids
  neutrino-diff    Histogram line diffs, hunks, word highlights (imara-diff)
  neutrino-git     The git engine on gitoxide: refs, history and graph lanes, commits, diffs
  neutrinod        The daemon: repo registry, Unix socket, concurrent requests, cancel
  ntr              CLI client
apps/
  project.yml      XcodeGen spec → Neutrino.xcodeproj (generated, not committed)
  NeutrinoKit/     Swift package: wire mirror, socket client, demo data, theme
  Mac/             macOS app: History, commit panel, diff view, Branches
design/            App icon sources; make-icons.sh renders them into the asset catalog
scripts/
  dev-neutrinod.sh         Build and run the daemon in the foreground
  install-launch-agent.sh  Run neutrinod at login (--uninstall to remove)
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
```

`ntr` picks the registered repo you're standing in, or `--repo ID`.
State (the list of repos) lives in `~/.neutrino`; `NEUTRINO_HOME` and
`NEUTRINO_SOCKET` move it. Socket paths are limited to 104 bytes.

The Mac app:

```bash
cd apps && xcodegen generate && open Neutrino.xcodeproj
```

It connects to neutrinod when it's running and shows the prototype's demo
repository otherwise; it notices the daemon starting or stopping on its own.
⌘O adds a repository, ⌘R reloads, ⌘1 and ⌘2 switch between History and
Branches, ⌘F searches, ↑ and ↓ move through commits, ⌘⇧D gives the diff the
full width.

Tests:

```bash
cargo test --workspace                     # builds real repos with git in temp folders
swift test --package-path apps/NeutrinoKit
NEUTRINO_LIVE=1 swift test --package-path apps/NeutrinoKit   # also drives a running neutrinod
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
- Nothing notices changes on disk yet; ⌘R reloads. File watching comes with
  status in milestone 2.
- Partial clones: file contents that were never fetched can't be diffed, and
  the app says so; Neutrino doesn't fetch them.
