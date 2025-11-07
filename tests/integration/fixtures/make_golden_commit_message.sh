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
mkdir repox
mkdir repoy
mkdir repoz
git -C top init -q --initial-branch main
git -C repox init -q --initial-branch main
git -C repoy init -q --initial-branch main
git -C repoz init -q --initial-branch main
cat <<EOF > top/.gittoprepo.toml
[repo.namex]
urls = ["../repox/"]
[repo.namey]
urls = ["../repoy/"]
[repo.namez]
urls = ["../repoz/"]
enabled = false
EOF
git -C top add .gittoprepo.toml

# Create the following commit history for:
# subx/Y-main  1
#              |
# top-main     A

subx_rev_1=$(commit repox "x-1")
subx_rev_2=$(commit repox "sub-2")
subx_rev_3=$(commit repox "all-3")
suby_rev_1=$(commit repoy "y-1")
suby_rev_2=$(commit repoy "top-and-y-2")
suby_rev_3=$(commit repoy "all-3")
subz_rev_1=$(commit repoz "z-1")

git -C top -c protocol.file.allow=always submodule add --force ../repox/ subpathx
git -C top -c protocol.file.allow=always submodule add --force ../repoy/ subpathy
git -C top -c protocol.file.allow=always submodule add --force ../repoz/ subpathz
git -C top submodule deinit -f subpathx subpathy subpathz
git -C top update-index --cacheinfo "160000,${subx_rev_1},subpathx"
git -C top update-index --cacheinfo "160000,${suby_rev_1},subpathy"
git -C top update-index --cacheinfo "160000,0011223344556677889900112233445566778899,subpathz"
git -C top commit --allow-empty -m "top-1

With: a footer"
git -C top update-index --cacheinfo "160000,${subx_rev_2},subpathx"
git -C top update-index --cacheinfo "160000,${suby_rev_2},subpathy"
commit top "top-and-y-2"
git -C top update-index --cacheinfo "160000,${subx_rev_3},subpathx"
git -C top update-index --cacheinfo "160000,${suby_rev_3},subpathy"
commit top "all-3"
# Regress x and let y point to something non-existing
git -C top update-index --cacheinfo "160000,${subx_rev_1},subpathx"
git -C top update-index --cacheinfo "160000,0123456789012345678901234567890123456789,subpathy"
git -C top commit -m "Regress x and missing commit y

End with some extra empty lines that are trimmed.


"
# Commit message with bad encoding.
git -C top -c i18n.commitEncoding=bad-encoding commit -m "$(printf "Bad \xFF encoding")" --allow-empty
# Move subx two steps forward. Change the URL for suby to simulate an unknown
# repository. Remove subz.
git -C top update-index --cacheinfo "160000,${subx_rev_3},subpathx"
sed -i 's/subpathy/sub-unknown/g' top/.gitmodules
git -C top add .gitmodules
git -C top rm subpathz
git -C top commit -m "Update git submodules

With boring body"
# No interesting commit messages at all.
git -C top commit -m "Update git submodules" --allow-empty
