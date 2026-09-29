#!/usr/bin/env bash
# Keep developer/build-host directories out of distributed Rust binaries.
set -euo pipefail
separator=$'\x1f'
flags="${CARGO_ENCODED_RUSTFLAGS:-}"
if [[ -n "$flags" ]]; then flags+="$separator"; fi
flags+="--remap-path-prefix=$HOME=/path/to/build-home"
flags+="${separator}--remap-path-prefix=$(pwd -P)=/path/to/project"
export CARGO_ENCODED_RUSTFLAGS="$flags"
cargo build --locked --release "$@"
