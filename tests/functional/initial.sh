#!/usr/bin/env bash

set -eu -o pipefail

# docker-compose up
# sleep 10
project=nils
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

curl -u admin:secret -X DELETE http://localhost:8080/a/accounts/admin/tokens/"$token_name"

tokenresponse=$(
    curl -s -u admin:secret \
       -X POST \
       -d '{
       "lifetime": "1d",
     }' \
       http://localhost:8080/a/accounts/admin/tokens/"$token_name" \
    | tail -1
)
test "$tokenresponse" = "Not found: /" && {
    echo >&2 "Error: can not reuse existing token: '$token_name'"
    exit 2
}

token=$(echo "$tokenresponse" \
    | jq '.token' | tr -d '"'
)

echo
echo machine "$host"
echo login "$user"
echo password "$token"

# TODO: Add the username to ssh config for this.
ssh-keygen -f "/home/nwirekli/.ssh/known_hosts" -R "[$host]:$port"

projects=(nils albin fredrik zalan oskar isak benjamin sassan chris gustav)

for project in "${projects[@]}"; do
    curl -s -u "$user":"$password" \
        -X POST \
        -d '  {
            "description": "This is a demo project.",
            "submit_type": "INHERIT",
            "owners": [
            "MyProject-Owners"
            ]
        }' \
            http://localhost:8080/a/projects/"$project".git
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
    commit "$project" a.txt "" "$project: initial commit" >/dev/null 2>&1
done

(
    project=albin;
    commit "$project" a.txt "2" "$project: 2" >/dev/null 2>&1
    commit "$project" a.txt "3" "$project: 3" >/dev/null
    commit "$project" a.txt "4" "$project: 4" >/dev/null
)

(project=gustav; commit "$project" a.txt "2" "$project: 2") 2>/dev/null

(project=fredrik; commit "$project" a.txt "2" "$project: 2") 2>/dev/null

(
    project=nils;
    commit "$project" a.txt "2" "$project: 2" >/dev/null 2>&1
    commit "$project" a.txt "3" "$project: 3" >/dev/null 2>&1
    commit "$project" a.txt "4" "$project: 4" >/dev/null 2>&1
    commit "$project" a.txt "5" "$project: 5" >/dev/null 2>&1
    commit "$project" a.txt "6" "$project: 6" >/dev/null 2>&1
    commit "$project" a.txt "7" "$project: 7" >/dev/null 2>&1
)

for project in "${projects[@]}"; do
    # NB: Allow pushing deterministic commits again.
    git -C "$project" push origin HEAD:refs/for/master || true
done
