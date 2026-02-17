#!/usr/bin/env bash

set -eu -o pipefail

test "${GIT_TOPREPO_DESTRUCTIVE_COMMANDS:-0}" = 1 || {
    echo >&2 "Warning: $(basename "$0") destroys the previous repository state and transforms it into a monorepo."
    echo >&2 "This is mostly meant for testing and when creating a completely new repository."
    echo >&2 "To proceed set 'GIT_TOPREPO_DESTRUCTIVE_COMMANDS=1'."
    echo >&2 "Exiting"
    exit 2
}

url="$(git remote get-url origin)"
# Managed by src/repo_name.rs (`Top::to_ref_prefix`).
toprepo_ref_prefix="refs/namespaces/top/"
# Managed by src/config.rs (`TOPREPO_CONFIG_FILE_KEY` and the namespace).
toprepo_config_file_key="toprepo.config"

git config --local \
    "remote.origin.pushUrl" \
    "https://ERROR.invalid/Please use 'git toprepo push ...' instead"
git config --local \
    "remote.origin.url" "$url"
git config --local \
    "--replace-all" \
    "remote.origin.fetch" \
    "+refs/heads/*:${toprepo_ref_prefix}refs/remotes/origin/*"
git config --local \
    "--add" \
    "remote.origin.fetch" \
    "+refs/tags/*:${toprepo_ref_prefix}refs/tags/*"

# TODO: 2025-09-22 Does HEAD always exist on the remote? Is `git ls-remote` needed
# to prioritize HEAD, main, master, etc.
git config --local \
    "--add" \
    "remote.origin.fetch" \
    "+HEAD:${toprepo_ref_prefix}refs/remotes/origin/HEAD"

# See fetch.rs for explanations of these default values.
# NOTE: Sync these default values with `RemoteFetcher::create_command` and
# `testtools::test_util::git_command_for_testing`.
git config --local \
    "remote.origin.tagOpt" "--no-tags"
git config --local \
    "remote.origin.pruneTags" "false"
git config --local \
    "submodule.recurse" "false"
# Avoid `git checkout main` to start following
# `refs/namespaces/top/refs/remotes/origin/main` which `--guess`
# (default) does because it matches `remote.origin.fetch`. Instead, the
# user should run `git checkout -b main origin/main`.
git config --local \
    "checkout.guess" "false"
git config --local \
    "--replace-all" \
    $toprepo_config_file_key \
    "should:repo:${toprepo_ref_prefix}refs/remotes/origin/HEAD:.gittoprepo.toml"
git config --local \
    "--add" \
    $toprepo_config_file_key \
    "may:worktree:.gittoprepo.user.toml"

# # Move the remote refs under the toprepo namespace
git show-ref \
    | grep ' refs/remotes/origin' \
    | ifne xargs -n2 sh -c '
        namespace="$1"; shift
        val="$1"; shift;
        key="$1"; shift;
        echo git update-ref "$namespace"/"$key" "$val";
        echo git update-ref -d "$key"
    ' _ "${toprepo_ref_prefix}"
