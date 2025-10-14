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
mkdir subx
mkdir suby
git -C top init -q --initial-branch main
git -C subx init -q --initial-branch main
git -C suby init -q --initial-branch main
# Accept push options.
git -C top config receive.advertisePushOptions true
git -C subx config receive.advertisePushOptions true
git -C suby config receive.advertisePushOptions true

cat <<EOF > top/.gittoprepo.toml
[repo.subx]
urls = ["../subx/"]
[repo.suby]
urls = ["../suby/"]
EOF
git -C top add .gittoprepo.toml

# Create the following commit history for:
# subY-main    Y1--Y2-
#                  |  \
# subX-main    X1--X2--X3
#              |   |   |
# top-main     A---B---C

subx_rev_1=$(commit subx "x-1")
commit suby "y-1"

commit top "init"
git -C top -c protocol.file.allow=always submodule add --force ../subx/ subx
git -C top submodule deinit -f subx
git -C top update-index --cacheinfo "160000,${subx_rev_1},subx"
commit top "A1-X1"

suby_rev_2=$(commit suby "y-2")
git -C subx -c protocol.file.allow=always submodule add --force ../suby/ suby
git -C subx submodule deinit -f suby
git -C subx update-index --cacheinfo "160000,${suby_rev_2},suby"
subx_rev_2=$(commit subx "x2-y2")
git -C top update-index --cacheinfo "160000,${subx_rev_2},subx"
commit top "B-X2-Y1"

subx_rev_3=$(commit subx "x3-y2")
git -C top update-index --cacheinfo "160000,${subx_rev_3},subx"
commit top "C-X3-Y2"
