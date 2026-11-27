#!/bin/sh
# Scan git-tracked files for leaked private IPs, personal paths, team IDs,
# personal emails, or hardcoded credentials.
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"

# Regexes are written with character classes where needed so this script does
# not trigger false positives or get altered by history-rewrite rules.
PATTERNS='(192\.168\.6[8]\.|100\.(6[5-9]|[7-9][0-9]|1[01][0-9]|12[0-7])\.|/Users/[r]azeen|razeen\.wasif[0-9]*@|C63K7[2]8GC6|BEGIN [A-Z ]*PRIVATE KEY-----[[:space:]]*MII|gh[pous]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{35})'

if git grep -nE "$PATTERNS" -- . ':!scripts/check-leaks.sh'; then
  echo "check-leaks: potential sensitive data found above" >&2
  exit 1
fi
echo "check-leaks: ok"
