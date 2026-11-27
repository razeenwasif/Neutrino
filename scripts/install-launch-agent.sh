#!/bin/sh
# Install neutrinod as a launch agent so it starts at login (and restarts if it
# stops). Logs go to ~/.neutrino/neutrinod.log.
#
#   scripts/install-launch-agent.sh             the workstation (this Mac)
#   scripts/install-launch-agent.sh --server    the server: hosts repos, pairs devices
#   scripts/install-launch-agent.sh --uninstall
#
# Flags for neutrinod go after `--`, e.g. on a workstation, to keep copies of
# the server's nightly database snapshots:
#   scripts/install-launch-agent.sh -- --copy-backups-from my-server
set -eu
label=dev.neutrino.neutrinod
plist="$HOME/Library/LaunchAgents/$label.plist"
state="${NEUTRINO_HOME:-$HOME/.neutrino}"
role_arg=""
if [ "${1:-}" = "--server" ]; then role_arg="<string>--server</string>"; shift; fi
extra=""
if [ "${1:-}" = "--" ]; then
  shift
  for a in "$@"; do
    esc=$(printf '%s' "$a" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g')
    extra="$extra<string>$esc</string>"
  done
fi

if [ "${1:-}" = "--uninstall" ]; then
  launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
  rm -f "$plist"
  echo "Removed $label."
  exit 0
fi

cd "$(dirname "$0")/.."
RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$PWD=/src --remap-path-prefix=$HOME=/home" \
  cargo build --release --locked -p neutrinod -p ntr
target=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
mkdir -p "$state/bin" "$HOME/Library/LaunchAgents"
chmod 700 "$state" "$state/bin"
# A copy, so rebuilding doesn't swap the binary under the running daemon.
cp "$target/release/neutrinod" "$target/release/ntr" "$state/bin/"

cat > "$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$label</string>
  <key>ProgramArguments</key><array><string>$state/bin/neutrinod</string>$role_arg$extra</array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>$state/neutrinod.log</string>
  <key>StandardOutPath</key><string>$state/neutrinod.log</string>
</dict>
</plist>
PLIST
chmod 600 "$plist"
launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$plist"
if [ -n "$role_arg" ]; then
  echo "neutrinod runs at login as the server. Pair the Mac app with: $state/bin/ntr pair"
  echo "Pushes arrive over SSH, so Remote Login must be on (System Settings > General > Sharing)."
else
  echo "neutrinod runs at login. ntr is in $state/bin; add it to your PATH."
fi
