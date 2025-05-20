#!/bin/bash
set -eu

if [ $# -eq 1 ] && [ "$1" == "ci" ]; then
  ci_mode=true
elif [ $# -eq 0 ]; then
  ci_mode=false
else
  echo "Usage: $0 [ci]"
  echo "If 'ci' is passed, it will run in CI mode."
  exit 2
fi

set -x

# Run both with and without `--all-features` to make sure that both configurations work.
if $ci_mode; then
  cargo fmt --all --check
else
  # Normal users usually want the code to be formatted.
  cargo fmt --all
fi
cargo clippy --all-targets --all-features -- -Dwarnings
cargo clippy --all-targets -- -Dwarnings
cargo build --all-targets --all-features
cargo build --all-targets
RUST_BACKTRACE=1 cargo test --workspace --all-features
RUST_BACKTRACE=1 cargo test --workspace
