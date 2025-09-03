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
# Accept push options and set a pre-receive hook.
git -C top config receive.advertisePushOptions true
git -C sub config receive.advertisePushOptions true
top_prereceive_hook_path="$(git -C top rev-parse --path-format=absolute --git-path hooks)/pre-receive"
sub_prereceive_hook_path="$(git -C sub rev-parse --path-format=absolute --git-path hooks)/pre-receive"
cat > "$top_prereceive_hook_path" <<"EOF"
#!/bin/sh
if test -n "$GIT_PUSH_OPTION_COUNT"
then
	i=0
	while test "$i" -lt "$GIT_PUSH_OPTION_COUNT"; do
		eval "value=\$GIT_PUSH_OPTION_$i"
        echo "GIT_PUSH_OPTION_$i=$value"
		i=$((i + 1))
	done
fi
# Allow observing interleaved output.
echo "prereceive hook sleeping"
sleep 1
echo "prereceive hook continues"
EOF
chmod +x "$top_prereceive_hook_path"
cp "$top_prereceive_hook_path" "$sub_prereceive_hook_path"

cat <<EOF > top/.gittoprepo.toml
[repo.sub]
urls = ["../sub/"]
EOF
git -C top add .gittoprepo.toml

# Create the following commit history:
# top  A---B---C---D-------E---F---G
#          |       |       |       |
# sub  1---2-------3---4---5---6---7
sub_rev__=$(commit sub "1")
sub_rev_2=$(commit sub "2")
sub_rev_3=$(commit sub "3")
sub_rev__=$(commit sub "4")
sub_rev_5=$(commit sub "5")
sub_rev__=$(commit sub "6")
sub_rev_7=$(commit sub "7")

commit top "A"
git -C top -c protocol.file.allow=always submodule add --force ../sub/ sub
git -C top submodule deinit -f sub
git -C top update-index --cacheinfo "160000,${sub_rev_2},sub"
commit top "B"
commit top "C"
git -C top update-index --cacheinfo "160000,${sub_rev_3},sub"
commit top "D"
git -C top update-index --cacheinfo "160000,${sub_rev_5},sub"
commit top "E"
commit top "F"
git -C top update-index --cacheinfo "160000,${sub_rev_7},sub"
top_rev_g=$(commit top "G")

# Continue with:
# top  --G----Ha--Ia-----J---------------N
#        |\    |   |   / | \      \     /|
#        | Hb--------Ib  |  K---L--M(10) |
#        |  |  |   |  |  |  |   |        |
# sub  --7----8a--9a----10--------------13
#         \ |         | / \ |   |       /
#         8b---------9b   11---12-----/

sub_rev_8b=$(commit sub "8b")
sub_rev_9b=$(commit sub "9b")
git -C sub reset --hard "$sub_rev_7"
sub_rev_8a=$(commit sub "8a")
sub_rev_9a=$(commit sub "9a")
unsafe_staged_merge sub "$sub_rev_9b"
sub_rev_10=$(commit sub "10")
sub_rev_11=$(commit sub "11")
sub_rev_12=$(commit sub "12")
git -C sub reset --hard "$sub_rev_10"
unsafe_staged_merge sub "$sub_rev_12"
sub_rev_13=$(commit sub "13")

git -C top update-index --cacheinfo "160000,${sub_rev_8b},sub"
commit top "Hb"
git -C top update-index --cacheinfo "160000,${sub_rev_9b},sub"
top_rev_ib=$(commit top "Ib")
git -C top reset --hard "$top_rev_g"
git -C top update-index --cacheinfo "160000,${sub_rev_8a},sub"
commit top "Ha"
git -C top update-index --cacheinfo "160000,${sub_rev_9a},sub"
commit top "Ia"
unsafe_staged_merge top "$top_rev_ib"
git -C top update-index --cacheinfo "160000,${sub_rev_10},sub"
top_rev_j=$(commit top "J")
git -C top update-index --cacheinfo "160000,${sub_rev_11},sub"
commit top "K"
git -C top update-index --cacheinfo "160000,${sub_rev_12},sub"
top_rev_l=$(commit top "L")
git -C top reset --hard "$top_rev_j"
unsafe_staged_merge top "$top_rev_l"
git -C top update-index --cacheinfo "160000,${sub_rev_10},sub"
top_rev_m=$(commit top "M")
git -C top reset --hard "$top_rev_j"
unsafe_staged_merge top "$top_rev_m"
git -C top update-index --cacheinfo "160000,${sub_rev_13},sub"
commit top "N"
