#!/bin/sh
# Install neutrinod as a launch agent so it starts at login (and restarts if it
# stops). `--uninstall` removes it. Logs go to ~/.neutrino/neutrinod.log.
set -eu
label=dev.neutrino.neutrinod
plist="$HOME/Library/LaunchAgents/$label.plist"
state="${NEUTRINO_HOME:-$HOME/.neutrino}"

if [ "${1:-}" = "--uninstall" ]; then
  launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
  rm -f "$plist"
  echo "Removed $label."
  exit 0
fi

cd "$(dirname "$0")/.."
cargo build --release -p neutrinod -p ntr
target=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
mkdir -p "$state/bin" "$HOME/Library/LaunchAgents"
# A copy, so rebuilding doesn't swap the binary under the running daemon.
cp "$target/release/neutrinod" "$target/release/ntr" "$state/bin/"

cat > "$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$label</string>
  <key>ProgramArguments</key><array><string>$state/bin/neutrinod</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>$state/neutrinod.log</string>
  <key>StandardOutPath</key><string>$state/neutrinod.log</string>
</dict>
</plist>
PLIST
launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$plist"
echo "neutrinod runs at login. ntr is in $state/bin; add it to your PATH."
