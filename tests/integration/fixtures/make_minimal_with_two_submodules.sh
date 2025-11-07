#!/usr/bin/env bash
set -eu -o pipefail

function commit {
    local repo="$1"
    local message="$2"
    touch "${repo}/${message}.txt"
    git -C "$repo" add "${message}.txt"
    git -C "$repo" commit -q -m "$message"
    git -C "$repo" rev-parse HEAD
}

mkdir top
mkdir repox
mkdir repoy
git -C top init -q --initial-branch main
git -C repox init -q --initial-branch main
git -C repoy init -q --initial-branch main
# Accept push options.
git -C top config receive.advertisePushOptions true
git -C repox config receive.advertisePushOptions true
git -C repoy config receive.advertisePushOptions true

cat <<EOF > top/.gittoprepo.toml
[repo.namex]
urls = ["../repox/"]
[repo.namey]
urls = ["../repoy/"]
EOF
git -C top add .gittoprepo.toml

# Create the following commit history for:
# subx/Y-main  1
#              |
# top-main     A

subx_rev_1=$(commit repox "x-main-1")
suby_rev_1=$(commit repoy "y-main-1")

commit top "init"
git -C top -c protocol.file.allow=always submodule add --force ../repox/ subpathx
git -C top -c protocol.file.allow=always submodule add --force ../repoy/ subpathy
git -C top submodule deinit -f subpathx subpathy
git -C top update-index --cacheinfo "160000,${subx_rev_1},subpathx"
git -C top update-index --cacheinfo "160000,${suby_rev_1},subpathy"
commit top "A1-main"
