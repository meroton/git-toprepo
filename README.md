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
of the configuration file is set in the `git-config` of the super repository using
`git config --local toprepo.config <location>`
Where the location is either a git ref `ref:<ref>:<path>`, a file
relative to the main worktree `local:<path>` or a file relative to
the current worktree `worktree:<path>`.
By default the toprepo has a configuration through a git ref that is committed
but it is possible to override it with a local path.

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

> [!NOTE]
> The following section describes the data model and the concepts involved in
> working with git-toprepo. Some of the functionality is not yet released.

### Commits, Topics and Supercommits

When working with changes that should be merged together (a Gerrit topic)
there are multiple parts to keep track of.
Those are explained here.

A topic contains supercommits,
a supercommit contains commits within projects
those projects are combined into a monorepo
through git-toprepo's history filter.

A picture is worth a thousand words:

![Concept overview](doc/static/toprepo-concepts.drawio.png)

This shows a purple topic that contains two supercommits: A and B.
The A supercommit spans two projects
and the internal model keeps track of them individually.
Git-toprepo then pushes the _four_ commits to Gerrit
and tracks A1, A2 and B as one topic.
Because Gerrit only tracks dependencies (git's parent relationship)
within a project C is now shown to have A2 as its parent,
B is not visible to C after the push within its project
instead the C -> B dependency is handled through the topic.
We will dig deeper into this later
and cover how to go from right-to-left
to download open changes from Gerrit with git-toprepo.

### Gerrit the review system

Git-toprepo is primarily developed to work with the [Gerrit] review system
as that is the authors' preferred review tool.
It is currently the only backend, so to speak,
that we have implemented but git-toprepo is meant to later work with multiple backends.
It should also be possible to have some subprojects managed by different review systems.

However, the terminology used here often borrows from [Gerrit] and [Zuul] and other [OpenInfra]
projects for code validation and verification.

🚧 Gerrit topic integration is coming soon. https://github.com/meroton/git-toprepo/issues/121

### Subprojects and Submodules
We prefer to talk about the repositories in Gerrit as projects,
as that does not denote whether they are filtered into the monorepo history
or are tracked as regular git-submodules.
It is common for a monorepo to eschew filtering some subprojects into the shared history,
if they are very large and not actively developed, for instance.

#### Commits
Commits are the bedrock of working with git.
These are created in the individual _subprojects_ or _submodules_
but multiple commits across different subprojects can form one coherent
_supercommit_.
To form these is git-toprepo's core purpose.
So cross-cutting changes across subprojects can be handled as individual commits
during development and in the filtered history.

#### Supercommits
Supercommits is what we call commits created in the emulated monorepo
either from the filtered history
where merges from the review system are bunched into one atomic unit
or on-going work that may span multiple subprojects.
It is not so simple that a supercommit is always a Gerrit topic,
though that is often the case.
A topic in Gerrit may under certain circumstances be merged with additional commits
that were (or were not) reviewed to be merged together.
As the review system owns the canonical git history
we follow its merge flow
(autobump commits, submit-whole-topic, etc).

Supercommits are fundamentally a client-side construct.
They are created either when committing new work with `git-commit`,
after fetching new history from the remote,
or by fetching on-going work from a colleague.
In the latter case there is no way to know precisely her working state
before she pushed to Gerrit
as we have no way to communicate that.
There is, however, a simple convention that helps users collaborate.
We will go through that in the example and collaboration sections.

🚧 Gerrit topic integration is coming soon. https://github.com/meroton/git-toprepo/issues/121

##### Merged or unmerged
It is natural to make the distinction between merged and unmerged commits
but it is better to have consistent behavior.
The goal here is to try to thread that needle
and communicate the subtleties of large scale collaboration.

The merged history is always the simplest:
whatever was submitted together is one supercommit,
or a single commit if it was submitted independently,
though with __manual submission__ there are __rare exceptions__.
So the split of one topic into multiple supercommits is not performed
in the filtered history.

Recall that a commit may be merged to the development branch
but still be in-review for a release branch.
Then the filtered history on the client-side contains
both merged commits (master branch) and unmerged (development branch).
The developer may want to treat a given topic as multiple supercommits
during review for the development branch
but they are originally one supercommit when fetched from master.

#### Collaboration
There is a simple convention to communicate the supercommits meant to form a topic in Gerrit:

* Use the same commit message and change-id

Then when a developer fetches the topic with git-toprepo
the [`recreate`] strategy can be used to recreate the original working state.
But if the individual commits in the topic were not created with git-toprepo
it is unlikely that they would have the same change-id
then the choice of [fetch strategy] is less clear.
To recreate supercommits means to treat each commit as their own supercommit,
allowing the developer to squash them manually after the fetch.
Instead to squash the topic into one supercommit
there are two other options:

* squash-if-possible: squash the topic into a single supercommit if it is possible.
    If there are multiple commits within a project this will instead give an error
    and the user will need to decide how to proceed.
* force-squash: squash the entire topic into a single supercommit.
    If there were multiple commits within a project their provenance will be tracked
    within the squashed commit message.
    But it is no longer possible to push changes to the original commits in Gerrit.

<!-- TODO: This first describes the good strategy, then explains the others in a list. -->
<!--       Do we want the list to include all of them? -->

🚧 Gerrit topic integration is coming soon. https://github.com/meroton/git-toprepo/issues/121

#### Topic
We use the [topic concept] from the [Gerrit] review platform,
which is our preferred review system and the only platform that has custom integration in `git-toprepo`.

The topic is a way to indicate that many (super)commits should be _merged_ together.
That means that all of the commits in a topic should be submitted as one atomic unit
to the history.
As the review platform is the canonical history
one merged topic will create one supercommit in the filtered history of the super repository.

🚧 Gerrit topic integration is coming soon. https://github.com/meroton/git-toprepo/issues/121

### Commit messages and footers

Just like the distinction between a regular _commit_ that belongs to a _subproject_
and a filtered _supercommit_ that have slightly different behavior
we should also point out that the commit message will vary.
When filtering the history to create the emulated monorepo
each supercommit is given a commit message.
For simple stand alone commits this is the same as the original commit
but supercommits that combine multiple commits will contain information of how they were created.

The format is not guaranteed to be stable
but the information contained is meant to reflect constituents:

* Commits in which subprojects
* Their individual commit messages

Topic information is taken from Gerrit's submodule bump
and is reflected in this message.

#### Zuul's dependency footers

We also support Zuul's dependency indication through footers
Where a commit can specify that it should be gated together with other commits.

https://zuul-ci.org/docs/zuul/latest/gating.html#cross-project-dependencies

🚧 Zuul depends-on integration is coming soon. https://github.com/meroton/git-toprepo/issues/122

#### toprepo footers

There are also a few footers used on the *client side* with git-toprepo
to help the tool operate.

* Topic: When committing "Topic:" can be used to create a topic for one or multiple supercommits.
  This will not be pushed in the commit to the Gerrit backend.
  But other review system backends,
  when they are implemented,
  may need this.

  This will also be written to merged and fetched commits
  that are part of a topic.
