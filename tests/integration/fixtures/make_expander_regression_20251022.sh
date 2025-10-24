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
mkdir sub
git -C top init -q --initial-branch main
git -C sub init -q --initial-branch main

cat <<EOF > top/.gittoprepo.toml
[repo.sub]
urls = ["../sub/"]
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
sub_rev__=$(commit sub "1")
sub_rev_2=$(commit sub "2")
sub_rev_3=$(commit sub "3")
sub_rev_4=$(commit sub "4")
git -C sub reset --hard "$sub_rev_3"
sub_rev_5=$(commit sub "5")
sub_rev_6=$(commit sub "6")
sub_rev_7=$(commit sub "7")
git -C sub reset --hard "$sub_rev_6"
sub_rev_8=$(commit sub "8")
unsafe_staged_merge sub "$sub_rev_4"
sub_rev_9=$(commit sub "9")
unsafe_staged_merge sub "$sub_rev_7"
sub_rev__=$(commit sub "10")
sub_rev_11=$(commit sub "11")

# shellcheck disable=SC2269
sub_rev__=$sub_rev__  # unused

commit top "A"
git -C top -c protocol.file.allow=always submodule add --force ../sub/ sub
git -C top submodule deinit -f sub
git -C top update-index --cacheinfo "160000,${sub_rev_2},sub"
top_rev_b=$(commit top "B2")
git -C top update-index --cacheinfo "160000,${sub_rev_5},sub"
top_rev_d=$(commit top "D5")
git -C top reset --hard "$top_rev_b"
git -C top update-index --cacheinfo "160000,${sub_rev_3},sub"
commit top "C3"
unsafe_staged_merge top "$top_rev_d"
git -C top update-index --cacheinfo "160000,${sub_rev_6},sub"
commit top "E6"
git -C top update-index --cacheinfo "160000,${sub_rev_8},sub"
commit top "F8"
git -C top update-index --cacheinfo "160000,${sub_rev_9},sub"
commit top "G9"
git -C top update-index --cacheinfo "160000,${sub_rev_11},sub"
commit top "H11"
