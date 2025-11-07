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
# subY-main    Y1--Y2-
#                  |  \
# subX-main    X1--X2--X3
#              |   |   |
# top-main     A---B---C

subx_rev_1=$(commit repox "x-1")
commit repoy "y-1"

commit top "init"
git -C top -c protocol.file.allow=always submodule add --force ../repox/ subpathx
git -C top submodule deinit -f subpathx
git -C top update-index --cacheinfo "160000,${subx_rev_1},subpathx"
commit top "A1-X1"

suby_rev_2=$(commit repoy "y-2")
git -C repox -c protocol.file.allow=always submodule add --force ../repoy/ subpathy
git -C repox submodule deinit -f subpathy
git -C repox update-index --cacheinfo "160000,${suby_rev_2},subpathy"
subx_rev_2=$(commit repox "x2-y2")
git -C top update-index --cacheinfo "160000,${subx_rev_2},subpathx"
commit top "B-X2-Y1"

subx_rev_3=$(commit repox "x3-y2")
git -C top update-index --cacheinfo "160000,${subx_rev_3},subpathx"
commit top "C-X3-Y2"
