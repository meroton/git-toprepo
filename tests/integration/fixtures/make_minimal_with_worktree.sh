#!/usr/bin/env bash
set -eu -o pipefail

mkdir from
git -C from init --quiet --initial-branch main
git -C from commit --allow-empty -m "init"

git clone from mono
cat <<EOF > mono/.gittoprepo.toml
EOF
git -C mono config --local toprepo.config must:local:.gittoprepo.toml

# Cannot create a worktree in the cache because it includes an absolute path
# that does not exist when the caching is done.
# git -C mono worktree add ../worktree
