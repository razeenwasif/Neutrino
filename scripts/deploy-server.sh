#!/bin/sh
# Build neutrinod and ntr from a commit, sign neutrinod with a stable identity,
# copy both to the server, restart it there and check it runs that commit.
#
#   scripts/deploy-server.sh [ssh-host] [commit]    defaults: mini, HEAD
#
# What it gets right, each the lesson of a deploy that went wrong:
#
#   * What's built. It builds a clean checkout of the commit, never this
#     working copy, in its own target folder. Once cargo judged a build fresh
#     and a stale neutrinod went out; a fresh checkout is newer than anything
#     built before, so the workspace's crates are always rebuilt, and
#     uncommitted changes never ship. The commit goes into the binary
#     (NEUTRINO_COMMIT), and `ntr info` names it.
#   * Signing. The vault's key is in the server's login Keychain, which trusts
#     the program that stored it. An ad hoc signature changes with every build,
#     so each deploy would lose access; a certificate's doesn't.
#     NEUTRINO_SIGN_IDENTITY picks the certificate (default: the first
#     "Apple Development" one in this Mac's Keychain).
#   * Replacing. macOS caches a binary's signature by file; overwriting a signed
#     binary in place can get the new one killed at launch. Each file goes in
#     as a new file, with mv.
#   * Checking. After the restart, the commit neutrinod reports has to be the
#     one built, or the deploy fails.
set -eu

host="${1:-mini}"
rev="${2:-HEAD}"
cd "$(dirname "$0")/.."

commit=$(git rev-parse --verify "$rev^{commit}")
echo "Deploying $(git log -1 --format='%h %s' "$commit" | cut -c1-70) to $host"
if [ "$rev" = HEAD ] && [ -n "$(git status --porcelain -- crates Cargo.toml Cargo.lock)" ]; then
    echo "Note: crates/ has uncommitted changes; they are not deployed, only the commit is." >&2
fi

identity="${NEUTRINO_SIGN_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Apple Development[^"]*\)".*/\1/p' | head -1)}"
if [ -z "$identity" ]; then
    echo "No Apple Development certificate found; set NEUTRINO_SIGN_IDENTITY." >&2
    exit 1
fi

# 1. A clean checkout of the commit, built into its own target folder (kept
#    between deploys, so dependencies aren't rebuilt each time).
target_root="$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
src="$target_root/deploy-src"
git worktree remove --force "$src" 2>/dev/null || rm -rf "$src"
git worktree prune
git worktree add --quiet --detach "$src" "$commit"
trap 'git worktree remove --force "$src" 2>/dev/null || true' EXIT
(cd "$src" && NEUTRINO_COMMIT="$commit" CARGO_TARGET_DIR="$target_root/deploy" \
    cargo build -q --release --locked -p neutrinod -p ntr)
out="$target_root/deploy/release"
codesign --force --identifier dev.neutrino.neutrinod --sign "$identity" --timestamp=none "$out/neutrinod"

# 2. Copy beside the old files, then swap each in as a new file and restart.
ssh_opts="-o BatchMode=yes -o ConnectTimeout=8"
# shellcheck disable=SC2086
scp -q $ssh_opts "$out/neutrinod" "$host:.neutrino/bin/.neutrinod.new"
# shellcheck disable=SC2086
scp -q $ssh_opts "$out/ntr" "$host:.neutrino/bin/.ntr.new"
# shellcheck disable=SC2086
info=$(ssh $ssh_opts "$host" 'cd ~/.neutrino/bin && mv -f .neutrinod.new neutrinod && mv -f .ntr.new ntr &&
    launchctl kickstart -k "gui/$(id -u)/dev.neutrino.neutrinod" &&
    for i in 1 2 3 4 5 6 7 8 9 10; do ./ntr info 2>/dev/null && exit 0; sleep 1; done; ./ntr info')
echo "$info"

# 3. It has to be running the commit that was built.
case "$info" in
    *"commit $commit"*) echo "$host runs $(git rev-parse --short "$commit")." ;;
    *) echo "neutrinod on $host doesn't report commit $commit; the deploy didn't take." >&2; exit 1 ;;
esac
