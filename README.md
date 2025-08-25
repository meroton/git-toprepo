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
Where the location is either a git ref `ref:<ref>:<path>`, a file
relative to the main worktree `local:<path>` or a file relative to
the current worktree `worktree:<path>`.
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

## Concepts

### Commits, Topics and Supercommits

XXX This continues to illustrate the individual parts 

### Gerrit the review system

Git-toprepo is primarily developed to work with the [Gerrit] review system
as that is the authors' preferred review tool.
It is currently the only backend, so to speak,
that we have implemented but git-toprepo is meant to later work with multiple backends.
It should also be possible to have some subprojects managed by different review systems.

However, the terminology used here often borrows from [Gerrit] and [Zuul] and other [OpenInfra]
projects for code validation and verification.

### Subprojects and Submodules
We prefer to talk about the repositories in Gerrit as projects,
as that does not denote whether they are filtered into the monorepo history
or are tracked as regular git-submodules.
It is common for a monorepo to eschew filtering some subprojects into the shared history,
if they are very large and not actively developed for instance.

#### Commits
Git's basic concept.
These are created in the individual _subprojects_ or _submodules_
but multiple commits across different subprojects can form one coherent
_supercommit_.
To form these is git-toprepo's core purpose.
So cross-cutting changes across subprojects can be handled as individual commits
during development and in the filtered history.

#### Supercommits
Supercommits may contain multiple commits from different subprojects.
A supercommit is originally created in two ways:

1) by filtering the history of a regular repo with submodules into an emulated monorepo.
2) by committing code inside such a repository.
   Note, regular git workflows do not create supercommits.

1. Is always done on code that has been reviewed verified and merged,
then we use the review system's canonical history.

2. Is performed by developers on new code
and the code is split into supercommits by the developer
according to her wishes.

Another developer will always get the same supercommits for 1 by filtering the history
to find all merged code.
Unmerged code however, can easily be transferred but we do *re*create the supercommits.
To do so there is a simple strategy:

    Commits within the same _topic_ with the same _changeid_ form a supercommit.

From Gerrit's constraint on _changeids_ two important points follow:
a supercommit can only have one commit in a given subproject.
If a topic contains multiple commits in a given subproject

Note that the review system may squash multiple commits from point 2
so that the code that is merged and follows from point 1 may differ.
See [Merged or unmerged] for more details.
In short: One open Gerrit _topic_ may contain multiple supercommits during development
but when submitted those will be squashed into a single supercommit for the entire _topic_.

##### Merged or unmerged
We make a distinction between merged and unmerged supercommits
with slightly different behaviors.
The merged history is the simplest:
whatever was submitted together is one supercommit,
the review system owns the canonical history
and git-toprepo the tool has no discretion.
For [Gerrit] this means that a _[topic]_ is often the supercommit,
or a single commit if it was submitted independently,
though with [manual submission] there are [rare exceptions].

[manual submission]: TODO
[rare exceptions]: TODO

So the split of one topic into multiple supercommits is not performed
in the filtered history.
Ongoing work, however, has a second distinction,
if the original author uses git-toprepo
it is possible to recreate her working state
(except for ordering between repositories, see [partial commit ordering])
[partial commit ordering]: TODO
with git-toprepo.

#### Topic
We use the [topic concept] from the [Gerrit] review platform,
which is our preferred review system and the only platform that has custom integration in `git-toprepo`.
For other review platforms and how to work with one or more of them see [TODO](TODO).

The topic is a way to indicate that many (super)commits should be _merged_ together.
That means that all of the commits in a topic should be submitted as one atomic unit
to the history.
As the review platform is the canonical history
one merged topic will create one supercommit in the filtered history of the super repository.
