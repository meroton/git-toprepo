#!/bin/bash
set -eux

# Run both with and without `--all-features` to make sure that both configurations work.
cargo clippy --all-targets --all-features -- -Dwarnings
cargo clippy --all-targets -- -Dwarnings
cargo build --all-targets --all-features
cargo build --all-targets
RUST_BACKTRACE=1 cargo test --workspace --all-features
RUST_BACKTRACE=1 cargo test --workspace
cargo build --release
