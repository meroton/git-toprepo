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
    local stdouterr
    if ! stdouterr=$(git -C "$repo" merge --no-ff --no-commit --strategy=ours -m "Dummy" "$@" 2>&1); then
        # Merge conflicts in submodules are expected.
        if test "$(echo "$stdouterr" | grep -q foo)" == ""; then
            echo "ERROR: git -C $repo merge"
            echo "$stdouterr"
            return 1
        fi
    fi
}

mkdir top
mkdir repox
mkdir repoy
git -C top init -q --initial-branch main
git -C repox init -q --initial-branch main
git -C repoy init -q --initial-branch main

cat <<EOF > top/.gittoprepo.toml
[repo.namex]
urls = ["../repox/"]
[repo.namey]
urls = ["../repoy/"]
EOF
git -C top add .gittoprepo.toml

# Create the following commit history:
# top  A--B--C--D--E--------F
#         |        |        |
# subx 1--2--------3--4--5--7
#          \               /
#           --------------6
subx_rev__=$(commit repox "x1")
subx_rev_2=$(commit repox "x2")
subx_rev_6=$(commit repox "x6")
git -C repox reset --hard "$subx_rev_2"
subx_rev_3=$(commit repox "x3")
subx_rev_4=$(commit repox "x4")
subx_rev_5=$(commit repox "x5")
unsafe_staged_merge repox "$subx_rev_6"
subx_rev_7=$(commit repox "x7")

# shellcheck disable=SC2269
subx_rev__=$subx_rev__  # unused

commit top "A"
git -C top -c protocol.file.allow=always submodule add --force ../repox/ subpathx
git -C top submodule deinit -f subpathx
git -C top update-index --cacheinfo "160000,${subx_rev_2},subpathx"
commit top "B-X2"
git -C top update-index --cacheinfo "160000,${subx_rev_3},subpathx"
commit top "C-X3"
git -C top update-index --cacheinfo "160000,${subx_rev_4},subpathx"
commit top "D-X4"
commit top "E-X4"
git -C top update-index --cacheinfo "160000,${subx_rev_7},subpathx"
commit top "F-X7"
