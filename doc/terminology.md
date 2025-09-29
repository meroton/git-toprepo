# Terminology overview

This describes the terms involved in using the `git-toprepo` tool
to _expand_ the _submodules_ of a _toprepo_ into _git-toprepo emulated monorepo_.
This _combines_ the history of all the _repositories_.

## Terms

**git repository**: A core `git` concept,
a _repository_. May be local or on a remote server.

**git submodule**: A core `git` concept,
a _submodule_ is a _repository_ with a child-parent relationship to another _repository_.

**regular submodule**: A core `git` concept,
a regular _submodule_ that is entirely managed through `git-submodule` etc.

**expanded submodule**: A `git-toprepo` concept,
a _submodule_ that has been _expanded_ into the _combined_ history in the _toprepo_.

**superrepo**: Emergent from core git concepts,
the parent _repository_ to a _submodule_.
It may be a _submodule_ to another _superrepo_.

**toprepo**: A regular _repository_ with special configuration and purpose.
It is meant to be used together with `git-toprepo` to _expand_ its _submodules_
to a _git-toprepo emulated monorepo_.
This is generally configured by the organization
but the user may have her own configuration for personal preferences.

A _toprepo_ can also be checked out with _regular submodules_:
`git-submodule init --recursive`
but it is not the preferred development workflow.

There is generally only one such _repository_
so it is often described in definite form: "the _toprepo_".

**git-toprepo**: The tool itself.
`git-toprepo` _expands_ (a choice of) _submodules_ of a _toprepo_
into a _git-toprepo emulated monorepo_,
the git histories are _combined_.
It takes care of pushing _expanded submodules_ to their respective remote server.

**monorepo**: A _repository_ with all the code,
it does not typically have _submodules_.
This makes it easy to make changes across different components
with a regular `git` workflow,
generally without _submodule bumps_ and binary deliveries/integration
of first party code.
Gives unparalleled reproducibility
and understanding of the full product.

**pure monorepo**: A commonly sought concept,
such a _repository_ does not have _submodules_ at all.
There is just one _repository_ on the remote `git` server.
This realizes the full value of a _monorepo_,
but has no clear _access control_.

**git-toprepo emulated monorepo**: A client-side construct,
that _emulates_ a _monorepo_ for a _toprepo_.
The developer sees a joint history of all _submodules_ and can create _mono commits_
that span multiple _submodules_ and push/fetch them with `git-toprepo`.
The tool keeps track of the _assimilated submodules_ with their own remote git _repositories_.

As a performance optimization, an _emulated monorepo_ created by `git-toprepo`
may still have _regular submodules_ though,
if the user does not want to _expand_ all _submodules_.

**submodule access control**: One can easily apply
access control to individual _submodules_ by restricting access to their git _repositories_.
Such access control is not possible for different directories in a _pure git monorepo_.

**commit**: A core `git` concept.

**combined commit**: A `git-toprepo` concept,
a commit in the _git-toprepo emulated monorepo_.

`git-toprepo` shines when a developer wants to make one change across two _submodules_
and can track that as one _combined commit_,
i.e. one _commit_ in the _emulated monorepo_ that consists of one _commit_ in each of the two _assimilated submodules_.
Those are meant to be merged together
through compatible CI systems that allow _shared gating_ between the constituent _repositories_.

**submodule bump**: A core `git` concept,
a change in the _super repository_
of which _commit id_ is wanted for a specific _submodule_ path.

**shared gating**: A CI system concept.
CI systems like [`Zuul CI`] allows an organization to merge code to multiple _repositories_
if all tests passes, atomically if the git server supports it.
By bumping the submodules accordingly,
e.g. by using [superproject subscription] in `Gerrit`,
the history of the constituent _repositories_
can be _recombined_ to the same _combined history_ graph that was pushed.

[`Zuul CI`]: https://zuulci.org/
[superproject subscription]: https://gerrit-review.googlesource.com/Documentation/user-submodules.html

### Verbs

**combine**: `git-toprepo` _combines_ the history of one _toprepo_ and (a choice of) its _submodules_
into an _emulated monorepo_ with a _combined_ history for code in the _toprepo_ itself and its _expanded submodules_.

**expand**: The content of the _submodules_ is expanded into an _emulated monorepo_.

**integrate**: A _submodule_ is integrated into the _git-toprepo emulated monorepo_
when the history is _combined_ and the content is (optionally) _expanded_.

### Technical details

For power users and _repository_ maintainers there are a few overlapping concepts.
<!-- TODO: link to our documentation of these. In the implementation documents or something. -->

**git-config**: The `toprepo` namespace is used for the `git-toprepo` settings
that are configured through `git`.

**git toprepo**: Git looks for external executables to run subcommands.
Calling `git toprepo` makes `git` execute `git-toprepo`.

### Technical terms in the code

**top commit**: Commits in the _toprepo_,
the remote _repository_ that has been cloned.
These are fetched using `git-toprepo fetch` (or `git fetch`) and
formed when pushing new work with `git-toprepo push`,
if changes were made to the underlying _toprepo_.

**monorepo**: In the code, "_monorepo_" is used as short-hand notation instead of
"_git-toprepo emulated monorepo_" or "_combined repo_". As the code has no use in
a "_pure monorepo_" context, the brevity is placed over preciseness of the
term within the code.

**mono commit**: In the code, "_mono commit_" is used as short-hand notation
instead of "_git-toprepo emulated monorepo commit_" or "_combined commit_", for
symmetry reasons. As the code has no use in a "_pure monorepo_" context, the
brevity is placed over preciseness of the term within the code.

**sub repo**: A _submodule_, either _assimilated_ or left as _regular submodule_.

## Examples

### Initialization: Expand the toprepo into an emulated monorepo

The _toprepo_ can be initialized to a _git-toprepo emulated monorepo_
with `git-toprepo`.
The configuration for `git-toprepo`
is often managed in the _toprepo_ itself and is already checked in.

Short-form initialization of a _git-toprepo emulated monorepo_.
```
$ git toprepo clone ssh://gerrit.example/toprepo.git emulated-monorepo
$ cd emulated-monorepo
emulated-monorepo $ # This is a git-toprepo emulated monorepo.
```

However, the code can also be checked out with regular git _submodules_
to create the same directory structure.
```
$ git clone ssh://gerrit.example/toprepo.git
$ cd toprepo
toprepo $ git submodule init --recursive
toprepo $ # This is not a git-toprepo emulated monorepo.
```

### Initialization: Some submodules are not expanded

Imagine that the _toprepo_ has one _submodule_ with a long and weird history,
it may be binary data that takes a lot of space and is not relevant to the developer.
Then it might be preferred to not _expanding_ it into the _combined repo_.

_git-toprepo emulated monorepo_:
```
$ git toprepo clone ssh://gerrit.example/toprepo.git emulated-monorepo
$ cd emulated-monorepo
emulated-monorepo $ # This is an emulated monorepo.
emulated-monorepo $ git submodule status
-4e04771fcf658500987d0be5a9a63f8e77d5e386 binary_data_module
```

Regular _repository_:
```
$ git clone ssh://gerrit.example/toprepo.git
$ cd toprepo
toprepo $ git submodule init --recursive
toprepo $ # This is not an emulated monorepo.
toprepo $ git submodule status
-4e04771fcf658500987d0be5a9a63f8e77d5e386 binary_data_module
-661c1b2d568693e3b6b631ae66f6872b194674f1 source_code_module
```

### Pushing: git-toprepo pushes combined repositories to their respective servers

`git-toprepo` shines when a developer wants to make one change across two _submodules_
in one _top commit_.

```
emulated-monorepo $ # modify one/file and two/file
emulated-monorepo $ git add one/file two/file
emulated-monorepo $ git commit
emulated-monorepo $ git-toprepo push HEAD:refs/heads/main
```

This pushes the two paths inside the _emulated monorepo_ to their constituent
_repositories_ on the git server (`gerrit.example/one.git` and `gerrit.example/two.git`).

The regular workflow with _submodules_, however, is more involved

```
toprepo $ # modify one/file and two/file
toprepo $ git -C one add file
toprepo $ git -C one commit
toprepo $ git -C one push HEAD:refs/heads/main
toprepo $ git -C two add file
toprepo $ git -C two commit
toprepo $ git -C two push HEAD:refs/heads/main
```

In both cases, the submodule pointers in the branch `main` in the _toprepo_
need to be updated to point at the latest commits in the submodules.
This can be done using e.g. Gerrit's superproject subscription or manually.

```
toprepo $ git add one two
toprepo $ git commit
toprepo $ git push HEAD:refs/heads/main
```

> [!NOTE]
> Though committing inside _regular submodules_ in a _git-toprepo emulated monorepo_ is rare,
> if a _submodule_'s history is not relevant in the _combined_ history
> it is unlikely that developers need to modify the code and make changes.

### Rebasing: git-toprepo gives a combined history that is easy to work with

With `git-toprepo`, rebasing _commits_ in any of the _expanded submodules_
is as easy as working in a single _repository_.

```
emulated-monorepo $ git-toprepo fetch origin
emulated-monorepo $ git rebase -i origin/main
```

However when using _regular submodules_ in an _repository_
one needs to automate the workflow within individual _submodules_.

```
toprepo $ git fetch origin
toprepo $ git rebase -i origin/main
toprepo $ git submodule foreach 'git -C "$sm_path" rebase -i origin/main'
toprepo $ # On error, run 'git -C <some/path> rebase --continue'
toprepo $ # followed by the same git-submodule-foreach command again.
```

In the example, two _submodules_ does not look too bad at the face of it,
but note that the rebasing is not synchronized between the _submodules_.
Therefore, building and testing the code after resolving a merge conflict,
which may have only occurred in one _submodule_, is not trivial.

### Pushing: Push all submodules of a toprepo

As a _git-toprepo emulated monorepo_ may not have _combined_ all _submodules_ into the history
some _submodules_ are left as _regular submodules_.
To always push changes to all _submodules_ the following invocation is needed:

```
emulated-monorepo $ git-toprepo push HEAD:refs/heads/main
emulated-monorepo $ git submodule foreach git push origin HEAD:refs/heads/main
```

> [!NOTE]
> Recall that committing inside _regular submodules_ in a _git-toprepo emulated monorepo_ is rare.

## History combination algorithm

This briefly outlines the algorithm
that creates the _combined history_ of the _git-toprepo emulated monorepo_,
to further contextualize the pieces and their relationships.

### Fetch a toprepo commit and create a mono commit

`git-toprepo fetch` first fetches the _regular commits_ for the _toprepo_ itself
using (approximately) `git fetch origin +refs/heads/*:refs/namespaces/top/refs/remotes/origin/*`.

The next phase is the load phase where for each submodule:

1. All _top commits_ reachable from `refs/namespaces/top/refs/remotes/*`
are loaded to look for _submodules_ and what _commit ids_ are referenced.
1. All _regular commits_ reachable from `refs/namespaces/<submod>/*` are loaded.
1. If any of the _commit ids_ requested by the _super repository_ was not found,
they are fetched using `git fetch <submod-url> +refs/heads/*:refs/namespaces/<submod>/refs/remotes/origin/*`.
1. All _regular commits_ reachable from `refs/namespaces/<submod>/*` are
checked for inner _submodules_ and what _commit ids_ are referenced.
1. Step 2 then follows recursively.

When all reachable commits have been loaded, the _submodules_ within the _toprepo_
are _expanded_ and the history _combined_.

1. Iterate through all _top commits_ reachable from `refs/namespaces/top/refs/remotes/*`
and start processing from the initial orphan _commits_.
1. For each _regular commit_, look for _submodule bumps_ or changes in `.gitmodules`.
1. _Expand_ each _submodule bump_ by replacing the _submodule_ git-link,
that points out the _commit id_, with the corresponding tree content.
1. Transfer of parents of each _submodule commits_ into the _combined commit_,
by checking which _combined commits_ the parents were _expanded_ in.
