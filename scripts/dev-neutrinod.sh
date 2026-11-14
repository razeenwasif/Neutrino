#!/bin/sh
# Build neutrinod and ntr, then run the daemon in the foreground.
# State lives in ~/.neutrino unless NEUTRINO_HOME says otherwise.
set -e
cd "$(dirname "$0")/.."
cargo build --release -p ntr
exec cargo run --release -q -p neutrinod -- "$@"
