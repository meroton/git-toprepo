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
git -C top init -q --initial-branch main
git -C repox init -q --initial-branch main

cat <<EOF > top/.gittoprepo.toml
[repo.namex]
url = "../repox/"
EOF
git -C top add .gittoprepo.toml

# The same submodule repository (repox) is checked out at two different
# paths in top, one following the "main" branch and the other following
# the "feature" branch, which diverge after their common commit 1:
#
# subX-feature           2f------3f
#                       /
# subX-main    1-------2-------3
#              |       |       |
# top-main     A---B---C---D---E
#
# subpathx follows subX-main and subpathy follows subX-feature.

subx_rev_1=$(commit repox "x-1")
git -C repox branch feature

commit top "init"
git -C top -c protocol.file.allow=always submodule add --force -b main ../repox/ subpathx
git -C top -c protocol.file.allow=always submodule add --force -b feature ../repox/ subpathy
git -C top submodule deinit -f subpathx subpathy
git -C top update-index --cacheinfo "160000,${subx_rev_1},subpathx"
git -C top update-index --cacheinfo "160000,${subx_rev_1},subpathy"
commit top "A"

subx_rev_2=$(commit repox "x-main-2")
git -C top update-index --cacheinfo "160000,${subx_rev_2},subpathx"
commit top "B-main-2"

git -C repox checkout -q feature
subx_rev_2f=$(commit repox "x-feature-2")
git -C top update-index --cacheinfo "160000,${subx_rev_2f},subpathy"
commit top "C-feature-2"

git -C repox checkout -q main
subx_rev_3=$(commit repox "x-main-3")
git -C top update-index --cacheinfo "160000,${subx_rev_3},subpathx"
commit top "D-main-3"

git -C repox checkout -q feature
subx_rev_3f=$(commit repox "x-feature-3")
git -C top update-index --cacheinfo "160000,${subx_rev_3f},subpathy"
commit top "E-feature-3"
