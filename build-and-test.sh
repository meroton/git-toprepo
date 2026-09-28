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

# Some tests require certain versions of tools installed.
function version_is_at_least {
  actual=$1
  required=$2
  message=$3
  if [ "$(printf '%s\n%s' "$required" "$actual" | sort --version-sort | head -1)" = "$required" ]; then
    return 0
  else
    message=${message//ACTUAL/$actual}
    message=${message//REQUIRED/$required}
    echo >&2 "$message"
    return 1
  fi
}

filter=

git_version=$(git version | awk '{print $3}')
if ! version_is_at_least "$git_version" "2.36.0" "Warning: Skipping tests for unsupported Git version ACTUAL < REQUIRED"; then
  filter="$filter --skip config::missing_config"
  filter="$filter --skip clone::clone_and_bootstrap"
fi
git_lfs_version=$({ git lfs version 2>/dev/null || echo "git-lfs/0.0.0-not-installed "; } | sed -n 's,git-lfs/\([^ ]*\) ,\1,p')
if ! version_is_at_least "$git_lfs_version" "1.0.0" "Warning: Skipping tests for unsupported Git LFS version ACTUAL < REQUIRED"; then
  filter="$filter --skip lfs::end_to_end"
fi

# Check that flake.nix is matching rust-toolchain.toml.
rust_version=$(sed -n 's/channel = "nightly-\(.*\)"/\1/p' rust-toolchain.toml)
if ! grep --quiet "https://static.rust-lang.org/dist/${rust_version}/channel-rust-nightly.toml" flake.nix; then
  echo >&2 "Error: rust-toolchain.toml and flake.nix have diverged"
  exit 1
fi

set -x

# Run both with and without '--all-features' to make sure that both configurations work.
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
