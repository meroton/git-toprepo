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

# Create the following commit history:
# subX  1---2  x x
#       |   |  | |
# top   A1--B2-+-C0--E0
#        \     |    /
# top     -----D0---

subx_rev_1=$(commit subx "1")
subx_rev_2=$(commit subx "2")

git -C top -c protocol.file.allow=always submodule add --force ../subx/ subx
git -C top update-index --cacheinfo "160000,${subx_rev_1},subx"
top_rev_a=$(commit top "A")
git -C top update-index --cacheinfo "160000,${subx_rev_2},subx"
commit top "B"
git -C top rm subx
top_rev_c=$(commit top "C")
git -C top reset --hard "$top_rev_a"
git -C top rm subx
commit top "D"
unsafe_staged_merge top "$top_rev_c"
commit top "E"
