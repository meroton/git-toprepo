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

# Create the following commit history:
# subZ                   /-3
# subY     /-2-*--3---3-*  |
# subX  1-/  |  \-2   |  \-3
#       |    |    |   |    |
# top   A1---B2---C---D----E

subx_rev_1=$(commit repox "1")
subx_rev_2=$(commit repox "2")
commit repox "3"

git -C top -c protocol.file.allow=always submodule add --force ../repox/ subpathx
git -C top update-index --cacheinfo "160000,${subx_rev_1},subpathx"
commit top "A"
# Move from subpathx to subpathy.
git -C top mv subpathx subpathy
git -C top update-index --cacheinfo "160000,${subx_rev_2},subpathy"
# Copy back to subpathx and bump subpathy.
commit top "B"
git -C top mv subpathy subpathx
git -C top -c protocol.file.allow=always submodule add --force ../repox/ subpathy
commit top "C"
# Remove subpathx.
git -C top rm -ff subpathx
commit top "D"
# Replace subpathy with both subpathx and subpathz.
git -C top mv subpathy subpathz
git -C top -c protocol.file.allow=always submodule add --force ../repox/ subpathx
commit top "E"
