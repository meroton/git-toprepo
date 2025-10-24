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

function unsafe_staged_merge {
    local repo="$1"
    shift
    # Skip checking exit code, merging conflicts in submodules will fail.
    git -C "$repo" merge --no-ff --no-commit --strategy=ours -m "Dummy" "$@" || true
}

mkdir top
mkdir subx
mkdir suby
git -C top init -q --initial-branch main
git -C subx init -q --initial-branch main
git -C suby init -q --initial-branch main

cat <<EOF > top/.gittoprepo.toml
[repo.subx]
urls = ["../subx/"]
[repo.suby]
urls = ["../suby/"]
EOF
git -C top add .gittoprepo.toml

# Create the following commit history:
# top  A--B--C--D--E--------F
#         |        |        |
# subx 1--2--------3--4--5--7
#          \               /
#           --------------6
subx_rev__=$(commit subx "x1")
subx_rev_2=$(commit subx "x2")
subx_rev_6=$(commit subx "x6")
git -C subx reset --hard "$subx_rev_2"
subx_rev_3=$(commit subx "x3")
subx_rev_4=$(commit subx "x4")
subx_rev_5=$(commit subx "x5")
unsafe_staged_merge subx "$subx_rev_6"
subx_rev_7=$(commit subx "x7")

# shellcheck disable=SC2269
subx_rev__=$subx_rev__  # unused

commit top "A"
git -C top -c protocol.file.allow=always submodule add --force ../subx/ subx
git -C top submodule deinit -f subx
git -C top update-index --cacheinfo "160000,${subx_rev_2},subx"
commit top "B-X2"
git -C top update-index --cacheinfo "160000,${subx_rev_3},subx"
commit top "C-X3"
git -C top update-index --cacheinfo "160000,${subx_rev_4},subx"
commit top "D-X4"
commit top "E-X4"
git -C top update-index --cacheinfo "160000,${subx_rev_7},subx"
commit top "F-X7"
