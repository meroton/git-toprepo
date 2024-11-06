# git-submodule made easy with git-toprepo

The `git-toprepo` script acts a bit like a client side `git-subtree`
based on the submodules in a top repository.
It has support for one level of submodules only,
no recursive submodules will be resolved.

`git toprepo init <repository> [<directory>]` will clone `repository` into `directory`,
replaces the submodule pointers with the actual content in the repository history.

`git toprepo fetch` fetches from the `remote` and performs the submodule resolution.

`git toprepo pull` is the same as `toprepo fetch && git merge`.

`git toprepo push [-n/--dry-run] <rev>:<ref> ...` does a reverse submodule resolution
so that each submodule can be pushed individually to each submodule upstream.
If running with `-n` or `--dry-run`, the resulting `git push` command lines
will be printed but not executed.

## Merging strategy

The basic idea is to join all the history from all the subrepositories
in a reproducible way. This means that users can keep a mono repository
locally on their computers but have share commit hashes with everyone else.

Consider the following history and commits:

    Top repo  A---B---C---D-------E---F---G---H
                  |       |       |       |
    Submodule 1---2-------3---4---5---6---7---8

The merged history will look like:

    Mono repo A---B2---C2---D3--E5---F5--G7--H7
                  /          \  /     \  / \
                 1            D4       F6   G8

... and NOT like:

    BAD REPO  A--B2--C2--D3--D4--E5--F5--G7--H7
                 /\      /         \    /     \
                1  ------            E6       H8

The algorithm steps are:
* Any history before the submodule is added contains the submodule
  directory only (1).
* Empty edge for the submodule history are removed (`2---3`).
  Such empty edges would only pollute the graph.
  The mono repo history for the submodule directory would
  show there is no update between the two commits anyway.
* The top repo will keep the "first parent" line (`D3---E5`).
  D4 might not be buildable and would break
  `git bisect --first-parent`.
* Submodule changes are moved as late as possible before merging (F6).
  The alternative of E6 instead of F6 clutters a graph log view.
  From the top repo view, it is impossible to know if E6 or F6
  is better (buildable) or not.
* Unmerged submodule branches are branched as early as possible.
  It is easier to run `git checkout G8 && git rebase H7` than
  `git checkout H8 && git rebase H7 --onto G7`.
* Unmerged submodule branches are branched from the history of `HEAD`.
  As commit 7 can be used in multiple top repo branches,
  it is impossible to know which branch commit 8 is aimed for.
  Simply checkout a different monorepo branch and run `git toprepo refilter`
  to move unmerged submodule branches around.

## Configuration

The configuration is specified in [Toml](https://toml.io/) format.
By default, it is read from `refs/remotes/origin/HEAD:.gittoprepo.toml`,
but the location can be configured in super repository git-config using
`git config --local toprepo.config.ref worktree` and
`git config --local toprepo.config.path <git-repo-relative-path>`.
Overriding the location is only recommended for testing out a new config and debugging purpose.

### Sub repositories

As `.gitmodules` evolves on the branches over time and
the servers might be relocated, the repository configuration shows how to
access each sub repository in the full history of the top repository.
For example, multiple URLs might have been configured in
the `.gitmodules` file, but all of them refers to the same repository.

By default, each submodule is fetched using
`git fetch --prune <url> +refs/heads/*:refs/repos/<repo-name>/heads/* +refs/tags/*:refs/repos/<repo-name>/tags/*`
where `repo-name` is the path part of the absolute URL without the any `.git` extension.
For example, the repo name for `ssh://git@github.com:meroton/git-toprepo.git` will be `meroton/git-toprepo`.

```# Generated to .git/toprepo/generated-config.

[expansion]
# Default is ["+.*"].
filter = [
  "+.*",
]

# Repo name defaults to base name of url.
[repos.toprepo]
urls = [
  "github.com/meroton/git-toprepo.git",
  "server.internal/git-toprepo.git",
]
# The date of the oldest commit to import.
# If a commit is filtered out, all its parents will also be removed.
# Default is "1970-01-01T00:00:00Z".
since = "2024-04-01T00:00:00Z"

# push.url defaults to fetch.url.
push.url = "ssh://git@github.com/meroton/git-toprepo.git"
push.args = []

[repos.toprepo.fetch]
url = "ssh://git@github.com/meroton/git-toprepo.git"
# Affects the --prune fetch arg, defaults to true.
prune = true
# Does not affect --prune, default is [].
args = [
  "--depth=1",
]
refspecs = [
  "+refs/heads/*:refs/repos/toprepo/heads/*",
  "+refs/tags/*:refs/repos/toprepo/tags/*",
]

[commits]
missing = [
"0123abc",
"0123ab3",
]

[commits.override_parents]
# An empty parents list will create a grafted commit.
"01234" = [ "12345", "abcdef" ]
```












#### Repository related fields

* `toprepo.repo.<repo-name>.urls`: Repositories with this specified URL in the
  .gitmodules file will use the configuration under `repo-name`.
  Multiple values are allowed, in which case `fetchUrl` must also be
  specified to make upstream connections unambiguous.
* `toprepo.repo.<repo-name>.fetchUrl`: Overrides `toprepo.repo.<repo-name>.url`
  for clone and fetch.
* `toprepo.repo.<repo-name>.pushUrl`: Overrides `toprepo.repo.<repo-name>.fetchUrl`
  for push.
* `toprepo.repo.<repo-name>.fetchArgs`: Extra command line arguments for
  git-fetch, multiple uses are accumulated.
  Default is `--prune`, `--prune-tags` and `--tags`.

#### Repository configuration examples

```ini
# The repository will be cloned under `.git/repos/myrepo` and
# the role will filter on the myrepo identifier (case sensitive).
[toprepo.repo "myrepo"]
    url = ../some-repo.git
    url = https://my-git-server/some-repo.git
    # Multiple urls makes fetchUrl required.
    fetchUrl = ../some-repo.git
```

Note that without quotes, the configuration is read in lowercase:

```bash
$ git config --list --file - <<EOF
[toprepo.repo.LowerCase]
    url = ../LowerCase.git
[toprepo.repo "Other_Repo"]
    url = ../Other/Repo.git
EOF

toprepo.repo.lowercase.url=../LowerCase.git
toprepo.repo.Other_Repo.url=../Other/Repo.git
```

###  Missing commits

Sometimes, submodules point to commits that do not exist anymore,
with there branch removed, or are otherwise erroneous.
To give the same view and resolved commit hashes to all users,
every missing commit needs to be listed.
git-toprepo will print the lines to add to your configuration when needed.

#### Missing commits syntax

* `toprepo.missing-commit.rev-<commit-hash>=<raw-url>`: This commit hash
  will be ignored if referenced by a subdmodule that has its URL in the
  `.gitmodules` file specified as `raw-url`.

#### Missing commits example

```
[toprepo.missing-commits]
    rev-b6a50df1c26c6b0f8755cac88203a9f4547adccd = ../some-repo.git
    rev-bfd24a62a7d5d5c67e396dd78e28137f99757508 = https://my-git-server/some-repo.git
```






### Roles

Roles are used to load and filter a set of repositories.
The build-in default configuration includes:

```ini
[toprepo]
    role = default
[toprepo.role.default]
    repos = +.*
```

This means that the role to load resolves to `default` if unset.
The `default` role resolves to filtering all repositories if unset.

#### Role related fields

* `toprepo.role`: A named role to use. Defaults to `default`.
* `toprepo.role.<role>.repos`: Tells which sub repos to use.
  Multiple values are accumulated.
  Each value starts with `+` or `-` followed by a regexp that should match a
  whole repository name. The last matching regex decides whether the repo
  should be expanded or not.
  `toprepo.role.default.repos` defaults to `+.*`.

#### Role configuration examples

```ini
[toprepo.role]
    # Default to this role, git-config can override.
    role = "active-only"

[toprepo.role.all]
    repos = +.*
[toprepo.role.active-only]
    # Remove all repositories.
    repos = -.*
    # Match certain ones.
    repos = +git-toprepo
    repos = +git-filter-repo
```
