#!/usr/bin/env bash

set -eu -o pipefail

prog="$(basename "$(realpath "$0")")"

usage () {
    cat <<-EOF
		Usage: $prog

		$prog Checks if the current branch has a merge commit with masters HEAD
        as parent. If not, it means that the current branch is behind master.
	EOF
}

repo_root() {
    local -;
    set +e
    # Must check submodules first, this will give the root if we are in a
    # submodule or an empty string if we are in the root, after that we can use
    # --show-toplevel.
    root=.
    while true; do
        newroot="$(git -C "$root" rev-parse --show-superproject-working-tree 2>/dev/null)"
        test -n "$newroot" || {
            root="$(git -C "$root" rev-parse --show-toplevel)"
            break
        }
        root="$newroot"
    done
    test $? = 0 || {
        echo >&2 "$prog: ERROR: this script must be run from within the super repo."
        exit 3
    }
    echo "$root"
}

REPO_ROOT="$(repo_root)"
transform_to_monorepo="$REPO_ROOT"/contrib/transform-to-monorepo.sh

# docker-compose up
# sleep 10

user="admin"
password=secret
host=localhost
port=29418

# # Deterministic git environment to create reproducible commits.
# `lib/testtools/test_util.rs::apply_git_env`
# Inspired by gix-testtools v0.16.1 configure_command().
export -n GIT_DIR
export -n GIT_INDEX_FILE
export -n GIT_OBJECT_DIRECTORY
export -n GIT_ALTERNATE_OBJECT_DIRECTORIES
export -n GIT_WORK_TREE
export -n GIT_COMMON_DIR
export -n GIT_ASKPASS
export -n SSH_ASKPASS
export GIT_CONFIG_NOSYSTEM="1"
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_TERMINAL_PROMPT="false"
export GIT_AUTHOR_NAME="A Name"
export GIT_AUTHOR_EMAIL="a@no.example"
export GIT_AUTHOR_DATE="2023-01-02T03:04:05Z+01:00"
export GIT_COMMITTER_NAME="C Name"
export GIT_COMMITTER_EMAIL="c@no.example"
export GIT_COMMITTER_DATE="2023-06-07T08:09:10Z+01:00"
export GIT_CONFIG_COUNT="0";

workspace="${1:-$(mktemp -d --suffix -git-toprepo-functional-test)}"

./curl.sh >/dev/null 2>&1
./curl.sh >/dev/null 2>&1

# Generate a new token each time as it is only printed once.
# This allows us to rerun the script without restarting the server.
# At the cost of polluting the token space for the account.
# TODO: use an uuid.
# token_name="$(shuf /etc/dictionaries-common/words | head -1 | tr -dc 'a-zA-Z' || true)"
token_name=token

curl -u "$user":"$password" -X DELETE http://"$host":8080/a/accounts/"$user"/tokens/"$token_name"

tokenresponse=$(
    curl -s -u "$user":"$password" \
       -X POST \
       -d '{
       "lifetime": "1d",
     }' \
       http://"$host":8080/a/accounts/"$user"/tokens/"$token_name" \
    | tail -1
)
test "$tokenresponse" = "Not found: /" && {
    echo >&2 "Error: can not reuse existing token: '$token_name'"
    exit 2
}

token=$(echo "$tokenresponse" \
    | jq '.token' | tr -d '"'
)

netrc="$workspace"/netrc
cat > "$netrc" << EOF
machine $host
login $user
password $token
EOF

export NETRC="$netrc"
export GERRIT_CLI_DEFAULT_GERRIT_HTTP_HOST=http://"$host":8080
export GERRIT_CLI_DEFAULT_GERRIT_SSH_HOST=ssh://"$host":29418

trap '{
    echo "wrote $netrc for the token and copied content to clipboard."
    cat "$netrc" | xclip -i -selection primary
    xclip -o -selection primary
}' EXIT

# TODO: Add the username to ssh config for this.
ssh-keygen -f "/home/nwirekli/.ssh/known_hosts" -R "[$host]:$port"

subprojects=(nils albin fredrik zalan oskar isak benjamin sasan chris gustav)
projects=(super "${subprojects[@]}")

for project in "${projects[@]}"; do
    # NB: `create_empty_commit` does not seem to work entirely.
    curl -s -u "$user":"$password" \
        -X POST \
        -d '  {
            "description": "This is a demo project.",
            "submit_type": "INHERIT",
            "create_empty_commit": true,
            "owners": [
              "MyProject-Owners"
            ]
        }' \
            http://"$host":8080/a/projects/"$project".git
        done


cd "$workspace" || exit 1

clone() {
    user=$1; shift
    host=$1; shift
    port=$1; shift
    project=$1; shift

    GIT_SSH_COMMAND="ssh -o StrictHostKeyChecking=accept-new" git clone ssh://"$user"@"$host":"$port"/"$project"
    ( cd "$project" || exit 1;  gitdir=$(git rev-parse --git-dir); scp -p -P "$port" "$user"@"$host":hooks/commit-msg "${gitdir}"/hooks/ )
}

commit() {
    project=$1; shift
    file=$1; shift
    content=$1; shift
    message="$*"

    touch "$project"/"$file"
    echo "$content" >> "$project"/"$file"
    git -C "$project" add "$file"
    git -C "$project" commit "$file" -m "$message"
}

for project in "${projects[@]}"; do
    clone "$user" "$host" "$port" "$project" >/dev/null 2>&1

    # Create a first commit if it is needed.
    # TODO: do this through the REST API if possible.
    initial_commit_message="$project initial commit"
    gerrit query "status:merged $initial_commit_message" >/dev/null || {
        commit "$project" a.txt "" "$initial_commit_message" >/dev/null 2>&1
        git -C "$project" push origin HEAD:refs/for/master || true
    }
done

# # NB: Submit an initial commit for each repository.

# shellcheck disable=SC2086
gerrit query 'status:open initial commit' --output changeid \
    | xargs --no-run-if-empty -n1 sh -c '
        set -eu
        cred_pair=$1; shift
        host=$1; shift
        change="$1"; shift;

        gerrit vote --change "$change" --vote Code-Review=2
        curl -u "$cred_pair" -X POST http://"$host":8080/a/changes/"$change"/submit
    ' _ "$user":"$password" "$host" || true

for project in "${subprojects[@]}"; do
    commit "$project" a.txt "1" "$project 1" >/dev/null 2>&1
done

(
    project=albin;
    commit "$project" a.txt "2" "$project 2" >/dev/null 2>&1
    commit "$project" a.txt "3" "$project 3" >/dev/null 2>&1
    commit "$project" a.txt "4" "$project 4" >/dev/null 2>&1
)

(project=gustav; commit "$project" a.txt "2" "$project 2") >/dev/null 2>&1

(project=fredrik; commit "$project" a.txt "2" "$project 2") >/dev/null 2>&1

(
    project=nils;
    commit "$project" a.txt "2" "$project 2" >/dev/null 2>&1
    commit "$project" a.txt "3" "$project 3" >/dev/null 2>&1
    commit "$project" a.txt "4" "$project 4" >/dev/null 2>&1
    commit "$project" a.txt "5" "$project 5" >/dev/null 2>&1
    commit "$project" a.txt "6" "$project 6" >/dev/null 2>&1
    commit "$project" a.txt "7" "$project 7" >/dev/null 2>&1
)

for project in "${projects[@]}"; do
    # NB: Allow pushing deterministic commits again.
    git -C "$project" push origin HEAD:refs/for/master || true
done

# # Set topics to bind the stacks together.

set_topic() {
    topic=$1; shift
    message=$1; shift
    gerrit query "subject:\"$message\"" | choose 0 | xargs gerrit topic --topic "$topic"
}

#        oskar zalan nils albin isak benjamin fredrik sasan chris gustav
#        ----- ----- ---- ----- ---- -------- ------- ----- ----- ------
#                    1
#                    2
# topic  1           3    1                   1
# TOPIC        1     4
# tema               5    2
# TEMA               6    3                   2                   1
# group              7    4     1    1                1     1     2
#

set_topic topic "oskar 1"
set_topic topic "albin 1"
set_topic topic "fredrik 1"
set_topic topic "nils 3"

set_topic TOPIC "zalan 1"
set_topic TOPIC "nils 4"

set_topic tema "nils 5"
set_topic tema "albin 2"

set_topic TEMA "nils 6"
set_topic TEMA "albin 3"
set_topic TEMA "fredrik 2"
set_topic TEMA "gustav 1"

set_topic group "nils 7"
set_topic group "albin 4"
set_topic group "isak 1"
set_topic group "benjamin 1"
set_topic group "sasan 1"
set_topic group "chris 1"
set_topic group "gustav 2"

# # Setup a super toprepo
{
    super=super;
    for project in "${subprojects[@]}"; do
        git -C "$super" submodule add ssh://"$user"@"$host":29418/"$project".git
    done
    set -x
    submodule_commit_message="$super add submodules"
    git -C "$super" commit -m "$submodule_commit_message" >/dev/null
    # gerrit query "status:merged $submodule_commit_message" >/dev/null || {
    #     git -C "$super" push origin HEAD:refs/for/master || true
    # }
}

(
    toprepo_config_file=gittoprepo.toml
    cd super || exit 1
    GIT_TOPREPO_ENABLE_DESTRUCTIVE_COMMANDS=1 $transform_to_monorepo
    git config --local --replace-all toprepo.config must:worktree:"$toprepo_config_file"
    git toprepo config bootstrap > "$toprepo_config_file"
    git toprepo recombine

    # shellcheck disable=SC2046
    env GIT_TOPREPO_ENABLE_EXPERIMENTAL_COMMANDS=1 \
        git toprepo checkout --strategy force-squash --dry-run \
        $(gerrit query 'status:open subject:"nils 7"' --output fetch)
)
