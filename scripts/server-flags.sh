#!/bin/sh
# Set the flags the server's neutrinod starts with, and restart it.
#
#   scripts/server-flags.sh [ssh-host] -- [neutrinod flags…]     default host: mini
#   scripts/server-flags.sh mini -- --host-executor --nexus-token-file /Users/you/.duet/neutrino.token
#   scripts/server-flags.sh mini --                               back to just --server
#
# Rewrites the launch agent installed by install-launch-agent.sh --server, so
# the flags survive restarts and reboots. `ntr ci` on the server shows what's on.
set -eu

host=mini
if [ "${1:-}" != "--" ] && [ -n "${1:-}" ]; then host="$1"; shift; fi
[ "${1:-}" = "--" ] && shift

args="<string>--server</string>"
for a in "$@"; do
    esc=$(printf '%s' "$a" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g')
    args="$args<string>$esc</string>"
done

# shellcheck disable=SC2087
ssh -o BatchMode=yes -o ConnectTimeout=8 "$host" sh -s <<REMOTE
set -eu
label=dev.neutrino.neutrinod
plist="\$HOME/Library/LaunchAgents/\$label.plist"
state="\${NEUTRINO_HOME:-\$HOME/.neutrino}"
[ -f "\$plist" ] || { echo "no \$plist; install the server first" >&2; exit 1; }
cat > "\$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>\$label</string>
  <key>ProgramArguments</key><array><string>\$state/bin/neutrinod</string>$args</array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>\$state/neutrinod.log</string>
  <key>StandardOutPath</key><string>\$state/neutrinod.log</string>
</dict>
</plist>
PLIST
launchctl bootout "gui/\$(id -u)/\$label" 2>/dev/null || true
# bootout returns before the old one has stopped; bootstrapping then fails
# with "5: Input/output error" and leaves the server down.
for _ in 1 2 3 4 5 6 7 8 9 10; do launchctl print "gui/\$(id -u)/\$label" >/dev/null 2>&1 || break; sleep 1; done
launchctl bootstrap "gui/\$(id -u)" "\$plist"
for _ in 1 2 3 4 5 6 7 8 9 10; do "\$state/bin/ntr" info 2>/dev/null && exit 0; sleep 1; done
"\$state/bin/ntr" info
REMOTE
