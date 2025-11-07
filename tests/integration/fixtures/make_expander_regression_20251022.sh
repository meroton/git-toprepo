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
    git -C "$repo" merge --no-ff --no-commit --strategy=ours -m "Dummy" "$@"
}

mkdir top
mkdir repo
git -C top init -q --initial-branch main
git -C repo init -q --initial-branch main

cat <<EOF > top/.gittoprepo.toml
[repo.name]
urls = ["../repo/"]
EOF
git -C top add .gittoprepo.toml

# Create the following commit history:
#            -----D-
#           /     | \
# top  A---B---C--+--E----F---G-------H
#          |   |  |  |    |   |       |
# sub  1---2---3--5--6----8---9--10--11
#               \     \      /   /
#                4-----+-----   /
#                       \      /
#                        7-----
sub_rev__=$(commit repo "1")
sub_rev_2=$(commit repo "2")
sub_rev_3=$(commit repo "3")
sub_rev_4=$(commit repo "4")
git -C repo reset --hard "$sub_rev_3"
sub_rev_5=$(commit repo "5")
sub_rev_6=$(commit repo "6")
sub_rev_7=$(commit repo "7")
git -C repo reset --hard "$sub_rev_6"
sub_rev_8=$(commit repo "8")
unsafe_staged_merge repo "$sub_rev_4"
sub_rev_9=$(commit repo "9")
unsafe_staged_merge repo "$sub_rev_7"
sub_rev__=$(commit repo "10")
sub_rev_11=$(commit repo "11")

# shellcheck disable=SC2269
sub_rev__=$sub_rev__  # unused

commit top "A"
git -C top -c protocol.file.allow=always submodule add --force ../repo/ subpath
git -C top submodule deinit -f subpath
git -C top update-index --cacheinfo "160000,${sub_rev_2},subpath"
top_rev_b=$(commit top "B2")
git -C top update-index --cacheinfo "160000,${sub_rev_5},subpath"
top_rev_d=$(commit top "D5")
git -C top reset --hard "$top_rev_b"
git -C top update-index --cacheinfo "160000,${sub_rev_3},subpath"
commit top "C3"
unsafe_staged_merge top "$top_rev_d"
git -C top update-index --cacheinfo "160000,${sub_rev_6},subpath"
commit top "E6"
git -C top update-index --cacheinfo "160000,${sub_rev_8},subpath"
commit top "F8"
git -C top update-index --cacheinfo "160000,${sub_rev_9},subpath"
commit top "G9"
git -C top update-index --cacheinfo "160000,${sub_rev_11},subpath"
commit top "H11"
