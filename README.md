# git-submodule made easy with git-toprepo

The `git-toprepo` script acts a bit like a client side `git-subtree`
based on the submodules in a top repository.
It has support for one level of submodules only,
no recursive submodules will be resolved.

`git toprepo init <repository> [<directory>]` will clone `repository` into `directory`,
replaces the submodule pointers with the actual content in the repository history.

`git toprepo fetch` fetches from the `remote` and performs the submodule resolution.

`git toprepo push [-n/--dry-run] <remote> <rev>:<ref> ...` does a reverse submodule resolution
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

The configuration is specified in [Toml](https://toml.io/) format. The location
of the configuration file is set in the git-config of the super repository using
`git config --local toprepo.config <location>`
Where the location is either a git ref `ref:<ref>:<path>`, a local file
relative to the current worktree `worktree:<path>` or a local file relative to
the main worktree `main-worktree:<path>`.
By default the toprepo has a configuration through a git ref that is committed.
But it is possible to override it with a local path.

### Sub repositories

As `.gitmodules` evolves on the branches over time and
the servers might be relocated, the repository configuration shows how to
access each sub repository in the full history of the top repository.
For example, multiple URLs might have been configured in
the `.gitmodules` file, but all of them refers to the same repository.

Each submodule is fetched using
`git fetch --prune <url> +refs/heads/*:refs/namespaces/<repo-name>/heads/* +refs/tags/*:refs/namespaces/<repo-name>/tags/*`.

After each run of `git toprepo fetch`, the actually resolved configuration is
written to `.git/toprepo/last-effective-git-toprepo.toml`. This file includes
suggested additions to the `.gittoprepo.toml` configuration.

```
[repo.something]
urls = [
    "https://github.com/meroton/git-toprepo.git",
    "server.internal/git-toprepo.git",
]
# push.url defaults to fetch.url.
push.url = "ssh://git@github.com/meroton/git-toprepo.git"
push.args = []

[repo.something.fetch]
url = "ssh://git@github.com/meroton/git-toprepo.git"
# Affects the --prune fetch arg, defaults to true.
prune = true
# --depth is added if set to non-zero.
depth = 0

[log]
ignored_warnings = [
    "This warning will not be displayed",
]
```
