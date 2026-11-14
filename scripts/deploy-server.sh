#!/bin/sh
# Build neutrinod and ntr, sign neutrinod with a stable identity, copy both to
# the server and restart it there.
#
#   scripts/deploy-server.sh [ssh-host]        default host: mini
#
# The signature matters: the vault's key is in the server's login Keychain,
# which trusts the program that stored it. An ad-hoc signature changes with
# every build, so each deploy would lose access; a certificate's doesn't.
# NEUTRINO_SIGN_IDENTITY picks the certificate (default: the first
# "Apple Development" one in this Mac's Keychain).
set -eu

host="${1:-mini}"
cd "$(dirname "$0")/.."

cargo build -q --release -p neutrinod -p ntr
target="$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/release"

identity="${NEUTRINO_SIGN_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Apple Development[^"]*\)".*/\1/p' | head -1)}"
if [ -z "$identity" ]; then
    echo "No Apple Development certificate found; set NEUTRINO_SIGN_IDENTITY." >&2
    exit 1
fi
codesign --force --identifier dev.neutrino.neutrinod --sign "$identity" --timestamp=none "$target/neutrinod"

ssh_opts="-o BatchMode=yes -o ConnectTimeout=8"
# shellcheck disable=SC2086
scp -q $ssh_opts "$target/neutrinod" "$target/ntr" "$host:.neutrino/bin/"
# shellcheck disable=SC2086
ssh $ssh_opts "$host" 'launchctl kickstart -k "gui/$(id -u)/dev.neutrino.neutrinod" && sleep 2 && ~/.neutrino/bin/ntr info'
