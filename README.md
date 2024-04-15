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

The configuration is specified in git-config format and read by default
from `refs/meta/git-toprepo:toprepo.config` from the top repo remote.
This default loading location can be overridden by setting
`toprepo.config.default.*` in your own git-config.

### Edit default configuration

To setup and edit the configuration in the default location, run

```bash
mkdir my-toprepo-config
cd my-toprepo-config
git init
# Initial commits.
vim toprepo.config
git add toprepo.config
git commit
git push <repository> HEAD:refs/meta/git-toprepo

# Fetch to edit
git fetch <repository> refs/meta/git-toprepo
git checkout FETCH_HEAD
```

Alternatively, setup the repository with a remote using:

```bash
mkdir my-toprepo-config
cd my-toprepo-config
git init
git remote add origin <repository>
git config remote.origin.fetch +refs/meta/*:refs/remotes/origin/meta/*
git fetch origin
```

### Configuration loading

The configuration is specified in the git-config under the section
`toprepo.config.<name>`. The default setting is:

```
[toprepo.config.default]
    type = "git"
    url = .
    ref = refs/meta/git-toprepo
    path = toprepo.config
```

This will load configuration from `toprepo.config` at `refs/meta/git-toprepo`
in the top repo remote.
More configurations can be loaded recursively and they are parsed using
`git config --file - --list`.

#### Configuration loading related fields
The following fields are available for different
`toprepo.config.<config-name>.type`:

* `toprepo.config.<config-name>.type=file` loads a file from local disk.
  * `toprepo.config.<config-name>.path`: The path to the config file to load.
* `toprepo.config.<config-name>.type=git` loads a file from local disk.
  * `toprepo.config.<config-name>.url`: The local or remote repository
    location. If the URL starts with `.`, it is assumed to be an URL relative
    to the top repository remote origin.
  * `toprepo.config.<config-name>.ref`: The remote reference to load.
  * `toprepo.config.<config-name>.path`: The path to the config file
    in the repository.
* `toprepo.config.<config-name>.type=none` has no more fields.

#### Configuration loading examples

Load from worktree:

```
[toprepo.config.default]
    type = "file"
    path = .gittoprepo
```

Load from remote `HEAD` (instead of `refs/meta/git-toprepo`)

```ini
[toprepo.config.default]
    type = "git"
    url = .
    ref = HEAD
    path = .gittoprepo
```

or simply

```ini
[toprepo.config.default]
    ref = HEAD
    path = .gittoprepo
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

### Sub repositories

As `.gitmodules` evolves on the branches over time and
the servers might be relocated, the repository configuration shows how to
access each sub repository in the full history of the top repository.
For example, multiple URLs might have been configured in
the `.gitmodules` file, but all of them refers to the same repository.

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
