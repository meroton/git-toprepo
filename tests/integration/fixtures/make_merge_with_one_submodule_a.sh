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
git -C top init -q --initial-branch main
git -C subx init -q --initial-branch main
cat <<EOF > top/.gittoprepo.toml
[repo.subx]
urls = ["../subx/"]
EOF
git -C top add .gittoprepo.toml

# Create the following commit history for:
# subX-release  4---5---6
#              /|      /|
# subX        1-+-2---3 |
#             | |     | |
# top-main    A-+-----B |
#              \|      \|
# top-release   C-------D

subx_rev_1=$(commit subx "x-main-1")
commit subx "x-main-2"
subx_rev_3=$(commit subx "x-main-3")
git -C subx reset --hard "$subx_rev_1"
subx_rev_4=$(commit subx "x-release-4")
commit subx "x-release-5"
unsafe_staged_merge subx "$subx_rev_3"
subx_rev_6=$(commit subx "x-release-6")

git -C top -c protocol.file.allow=always submodule add --force ../subx/ subx
git -C top submodule deinit -f subx
git -C top update-index --cacheinfo "160000,${subx_rev_1},subx"
top_rev_a=$(commit top "A1-main")
git -C top update-index --cacheinfo "160000,${subx_rev_3},subx"
top_rev_b=$(commit top "B3-main")
git -C top reset --hard "$top_rev_a"
git -C top update-index --cacheinfo "160000,${subx_rev_4},subx"
commit top "C4-release"
unsafe_staged_merge top "$top_rev_b"
git -C top update-index --cacheinfo "160000,${subx_rev_6},subx"
commit top "D6-release"

