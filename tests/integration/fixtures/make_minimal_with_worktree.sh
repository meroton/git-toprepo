#!/usr/bin/env bash
set -eu -o pipefail

mkdir from
git -C from init --quiet --initial-branch main
git -C from commit --allow-empty -m "init"

git clone from top
cat <<EOF > top/.gittoprepo.toml
EOF
git -C top config --local toprepo.config main-worktree:.gittoprepo.toml

git -C top worktree add ../worktree
