#!/bin/sh
# Build neutrinod and ntr, then run the daemon in the foreground.
# State lives in ~/.neutrino unless NEUTRINO_HOME says otherwise.
set -e
cd "$(dirname "$0")/.."
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$PWD=/src --remap-path-prefix=$HOME=/home"
cargo build --release --locked -p ntr
exec cargo run --release --locked -q -p neutrinod -- "$@"
