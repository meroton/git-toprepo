#!/usr/bin/env bash
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

# # Minimum git version is 2.36 .
# Developers with older git releases can't run some tests locally.
# They are filtered out here.
MINIMUM=2.36.0
filter=
lowest="$({ git version | awk '{print $3}' ; echo $MINIMUM; } | sort --version-sort | head -1)"
test "$lowest" = "$MINIMUM" || {
    echo >&2 "Warning: skipping tests for unsupported git version: $lowest"
    filter="$filter --skip config::missing_config"
    filter="$filter --skip clone::clone_and_bootstrap"
}

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

# shellcheck disable=2086
RUST_BACKTRACE=1 cargo test --workspace --all-features -- $filter
# shellcheck disable=2086
RUST_BACKTRACE=1 cargo test --workspace -- $filter
