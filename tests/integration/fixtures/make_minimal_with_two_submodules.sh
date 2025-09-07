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
# subX/Y-main  1
#              |
# top-main     A

subx_rev_1=$(commit subx "x-main-1")
suby_rev_1=$(commit suby "y-main-1")

git -C top -c protocol.file.allow=always submodule add --force ../subx/ subx
git -C top -c protocol.file.allow=always submodule add --force ../suby/ suby
git -C top submodule deinit -f subx suby
git -C top update-index --cacheinfo "160000,${subx_rev_1},subx"
git -C top update-index --cacheinfo "160000,${suby_rev_1},suby"
commit top "A1-main"
