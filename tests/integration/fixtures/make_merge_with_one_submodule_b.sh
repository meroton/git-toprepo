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
git -C top init -q --initial-branch main
git -C repox init -q --initial-branch main
cat <<EOF > top/.gittoprepo.toml
[repo.namex]
urls = ["../repox/"]
EOF
git -C top add .gittoprepo.toml

# Create the following commit history for:
# subX-release  5---6---7----
#              /    |  /     \
# subX-main   1---2-+-3---4---8
#             |     | |       |
# top-main    A-----+-B-------F
#              \    |   /-E--/
# top-release   ----C-----D-/

subx_rev_1=$(commit repox "x-main-1")
commit repox "x-main-2"
subx_rev_3=$(commit repox "x-main-3")
git -C repox reset --hard "$subx_rev_1"
commit repox "x-release-5"
subx_rev_6=$(commit repox "x-release-6")
unsafe_staged_merge repox "$subx_rev_3"
subx_rev_7=$(commit repox "x-release-7")
git -C repox reset --hard "$subx_rev_3"
commit repox "x-main-4"
unsafe_staged_merge repox "$subx_rev_7"
subx_rev_8=$(commit repox "x-release-8")

git -C top -c protocol.file.allow=always submodule add --force ../repox/ subpathx
git -C top submodule deinit -f subpathx
git -C top update-index --cacheinfo "160000,${subx_rev_1},subpathx"
top_rev_a=$(commit top "A1-main")
git -C top update-index --cacheinfo "160000,${subx_rev_6},subpathx"
top_rev_c=$(commit top "C6-release")
top_rev_d=$(commit top "D6-release")
git -C top reset --hard "$top_rev_c"
top_rev_e=$(commit top "E6-release")
git -C top reset --hard "$top_rev_a"
git -C top update-index --cacheinfo "160000,${subx_rev_3},subpathx"
commit top "B3-main"
unsafe_staged_merge top "$top_rev_d" "$top_rev_e"
git -C top update-index --cacheinfo "160000,${subx_rev_8},subpathx"
commit top "F8-release"
