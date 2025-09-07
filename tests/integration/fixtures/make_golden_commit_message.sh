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
mkdir subz
git -C top init -q --initial-branch main
git -C subx init -q --initial-branch main
git -C suby init -q --initial-branch main
git -C subz init -q --initial-branch main
cat <<EOF > top/.gittoprepo.toml
[repo.subx]
urls = ["../subx/"]
[repo.suby]
urls = ["../suby/"]
[repo.subz]
urls = ["../subz/"]
enabled = false
EOF
git -C top add .gittoprepo.toml

# Create the following commit history for:
# subX/Y-main  1
#              |
# top-main     A

subx_rev_1=$(commit subx "x-1")
subx_rev_2=$(commit subx "sub-2")
subx_rev_3=$(commit subx "all-3")
suby_rev_1=$(commit suby "y-1")
suby_rev_2=$(commit suby "top-and-y-2")
suby_rev_3=$(commit suby "all-3")
subz_rev_1=$(commit subz "z-1")

git -C top -c protocol.file.allow=always submodule add --force ../subx/ subx
git -C top -c protocol.file.allow=always submodule add --force ../suby/ suby
git -C top -c protocol.file.allow=always submodule add --force ../subz/ subz
git -C top submodule deinit -f subx suby subz
git -C top update-index --cacheinfo "160000,${subx_rev_1},subx"
git -C top update-index --cacheinfo "160000,${suby_rev_1},suby"
git -C top update-index --cacheinfo "160000,0011223344556677889900112233445566778899,subz"
git -C top commit --allow-empty -m "top-1

With: a footer"
git -C top update-index --cacheinfo "160000,${subx_rev_2},subx"
git -C top update-index --cacheinfo "160000,${suby_rev_2},suby"
commit top "top-and-y-2"
git -C top update-index --cacheinfo "160000,${subx_rev_3},subx"
git -C top update-index --cacheinfo "160000,${suby_rev_3},suby"
commit top "all-3"
# Regress x and let y point to something non-existing
git -C top update-index --cacheinfo "160000,${subx_rev_1},subx"
git -C top update-index --cacheinfo "160000,0123456789012345678901234567890123456789,suby"
git -C top commit -m "Regress x and missing commit y

End with some extra empty lines that are trimmed.


"
# Commit message with bad encoding.
git -C top -c i18n.commitEncoding=bad-encoding commit -m "$(printf "Bad \xFF encoding")" --allow-empty
# Move subx two steps forward. Change the URL for suby to simulate an unknown
# repository. Remove subz.
git -C top update-index --cacheinfo "160000,${subx_rev_3},subx"
sed -i 's/suby/sub-unknown/g' top/.gitmodules
git -C top add .gitmodules
git -C top rm subz
git -C top commit -m "Update git submodules

With boring body"
# No interesting commit messages at all.
git -C top commit -m "Update git submodules" --allow-empty
