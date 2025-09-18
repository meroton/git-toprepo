# Terminology overview

This describes the terms involved in using the `git-toprepo` tool
to create an _emulated monorepo_ for a _toprepo_ and its _submodules_.
this _combines_ the history of all _repositories_.

## Terms

**git repository**: A core `git` concept,
a _repository_. May be local or on a remote server.

**git submodule**: A core `git` concept,
a _submodule_ is a _repository_ with a child-parent relationship to another.

**regular submodule**: A core `git` concept,
a regular _submodule_ that is entirely managed through `git-submodule` etc.

**assimilated submodule**: A `git-toprepo` concept,
a _submodule_ that has been assimilated into the _combined_ history in the _toprepo_.

**superrepo**: Emergent from core git concepts,
the parent _repository_ to a _submodule_.
It may be a _submodule_ to another _superrepo_.

**toprepo**: A regular _repository_ with special configuration and purpose.
It is meant to be used together with `git-toprepo` to _combine_ its _submodules_
to an _emulated monorepo_.
This is generally configured by the organization
but the user may have her own configuration for personal preferences.
There is generally only one such repo
so it is often described in definite form: "the _toprepo_".

It can also be checked out with _regular submodules_:
`git-submodule init --recursive`
but it is not the preferred development workflow.

**git-toprepo**: The tool itself.
`git-toprepo` combines a _repository_
and (a choice of) its _submodules_
into a _toprepo_, an _emulated monorepo_.
Takes care to push _assimilated submodules_ to their remote server.

**monorepo**: A _repository_ with all the code,
it does not typically have _submodules_.
This makes it easy to make changes across different components
with a regular `git` workflow,
generally without _submodule_ bumps and binary deliveries/integration
of first party code.
Gives unparalleled reproducibility
and understanding of the full product.

**pure monorepo**: A commonly sought concept,
such a _repository_ does not have _submodules_ at all.
There is just one _repository_ on the remote `git` server.
This realizes the full value of a _monorepo_,
but has no clear _access control_.

**emulated monorepo**: A client-side construct
that _emulates_ a _monorepo_ for a _toprepo_.
The developer sees a joint history of all _submodules_ and can create _monocommits_
that span multiple _submodules_ and push/fetch them with `git-toprepo`.
The tool keeps track of the _assimilated submodules_ with their own remote git _repositories_.

As a performance optimization a  _monorepo_ created by `git-toprepo`
may still have _regular submodules_ though,
if the user does not want to _combine_ all _submodules_.

**submodule access control**: One can easily apply
access control to individual _submodules_ by restricting access to their git _repositories_.
Such access control is not possible for different directories in a _pure monorepo_.

**commit**: A core `git` concept.

**monocommit**: A `git-toprepo` concept,
a commit in the _emulated monorepo_.

`git-toprepo` shines when a developer wants to make one change across two _submodules_
and can track that as one _monocommit_
-- one _commit_ in the _emulated monorepo_ that consists of one _commit_ in each of the two _submodules_.
Those are meant to be merged together
through compatible CI systems that allow _shared gating_ between the constituent _repositories_.

**shared gating**: A CI system concept.
CI systems like `Gerrit` allows an organization to merge code to multiple _repositories_
atomically if all tests passes.
This allows the shared gating of the constituent _submodules_.
So the merged history is always compatible with an _emulated monorepo_,
there are no race conditions between different _repository_ gates.
`Gerrit` uses [superproject subscription] for this.

[superproject subscription]: https://gerrit-review.googlesource.com/Documentation/user-submodules.html

### Verbs

**combine**: `git-toprepo` _combines_ the history of one _toprepo_ and (some of) its _submodules_
into an _emulated monorepo_ with a _combined_ history for code in the _toprepo_ itself and its _assimilated submodules_.

**assimilate**: `git-toprepo` has _assimilated_ a _submodule_ into the _combined_ _emulated monorepo_ history.

**expand**: The _toprepo_ has been expanded to an _emulated monorepo_.
This verb is not used often but avoids the mention of _submodules_.

### Technical details

For power users and _repository_ maintainers there are a few overlapping concepts.
<!-- TODO: link to our documentation of these. In the implementation documents or something. -->

**toprepo**: The `git-config` namespace for select `git-toprepo` settings that are configured through `git`.

**toprepo**: The `git` subcommand that runs `git-toprepo`.
`git` runs external subcommands like `git-<sub>` as `git <commit>`
to make it easy to create custom tools for `git`.

### Technical terms in the code

**topcommit**: Commits in the _toprepo_'s own remote git server.
These are fetched in `git-toprepo fetch`
these are also formed when pushing new work with `git-toprepo push`
if changes were made to the underlying _toprepo_,
symmetric with _regular commits_ for the constituent _submodules_
that are pushed to the _submodules_' remote git servers.

**monorepo**: In the code we use "_monorepo_" as short-hand notation instead of
"_emulated monorepo_". As the code has no use in a "_pure monorepo_" context.
So the brevity is placed over preciseness of the term within the code.

## Examples

### Initialization: expand the toprepo to an emulated monorepo

The _toprepo_ can be initialized to an _emulated monorepo_ with `git-toprepo`.
The configuration of the _emulated monorepo_
is often managed in the _toprepo_ itself and is already checked in.

Short-form initialization of the _emulated monorepo_.
```
$ git toprepo clone ssh://gerrit.example/toprepo.git emulated-monorepo
$ cd emulated-monorepo
emulated-monorepo $ # This is an emulated monorepo.
```

However, the code can also be checked out with regular git _submodules_.
```
$ git clone ssh://gerrit.example/toprepo.git
$ cd toprepo
toprepo $ git submodule init --recursive
toprepo $ # This is not an emulated monorepo.
```

### Initialization: Some submodules are not assimilated

Now imagine that the _toprepo_ has one _submodule_ with a long and weird history,
it may be binary data that takes a lot of space and is not relevant to the developer.
Then it is often not _assimilated_ into the _emulated monorepo_.

_emulated monorepo_:
```
$ git toprepo clone ssh://gerrit.example/toprepo.git emulated-monorepo
$ cd emulated-monorepo
emulated-monorepo $ # This is an emulated monorepo.
monorepo $ git submodule status
-4e04771fcf658500987d0be5a9a63f8e77d5e386 binary_data_module
```

regular _repository_:
```
$ git clone ssh://gerrit.example/toprepo.git
$ cd toprepo
toprepo $ git submodule init --recursive
toprepo $ # This is not an emulated monorepo.
toprepo $ git submodule status
-4e04771fcf658500987d0be5a9a63f8e77d5e386 binary_data_module
-661c1b2d568693e3b6b631ae66f6872b194674f1 source_code_module
```

### Pushing: git-toprepo pushes assimilated submodules to their servers

`git-toprepo` shines when a developer wants to make one change across two _submodules_
in one _topcommit_.

```
emulated-monorepo $ # modify one/file and two/file
emulated-monorepo $ git add one/file two/file; git commit
emulated-monorepo $ git-toprepo push HEAD:refs/for/main
```

This pushes the two paths inside the _emulated monorepo_ to their constituent
_repositories_ on the git server (gerrit.example/one.git and gerrit.example/two.git).

The regular workflow with _submodules_, however, is more involved

```
toprepo $ # modify one/file and two/file
toprepo $ git -C one add file; git commit
toprepo $ git -C two add file; git commit
toprepo $ git -C one push HEAD:refs/for/main
toprepo $ git -C two push HEAD:refs/for/main
# Because you use Gerrit's superproject subscription (otherwise git-toprepo does not work),
# you would not need a toprepo commit:
#   toprepo $ git add one two; git commit
#   toprepo $ git push HEAD:refs/for/main
```

> [!NOTE]
> Though committing inside _regular submodules_ in an _emulated monorepo_ is rare.
> If a _submodule_'s history is not relevant to _assimilate_ into the _combined_ history
> it is unlikely that developers need to modify the code and make changes.

### Rebasing: git-toprepo gives a shared history that is easy to work with

With `git-toprepo`, rebasing _commits_ in any of the _assimilated submodules_
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
toprepo $ submod_commit_hash="$(git ls-files --stage -- one | cut -d' ' -f2)"
toprepo $ git -C one rebase -i "$submod_commit_hash"
toprepo $ submod_commit_hash="$(git ls-files --stage -- two | cut -d' ' -f2)"
toprepo $ git -C two rebase -i "$submod_commit_hash"
```

In the example, two _submodules_ does not look too bad at the face of it,
but note that the rebasing is not synchronized between the _submodules_.
Therefore, building and testing the code after resolving a merge conflict,
which may have only occurred in one _submodule_, is not trivial.

### Pushing: Push all submodules of a toprepo

As an _emulated monorepo_:_ may not have _combined_ all _submodules_ into the history
some _submodules_ are left as _regular submodules_.
So to always push changes to all _submodules_ the following invocation is needed:

```
emulated-monorepo $ git-toprepo push HEAD:refs/for/main
emulated-monorepo $ git submodule for each push HEAD:refs/for/main
```

> [!NOTE]
> Recall that  committing inside _regular submodules_ in an _emulated monorepo_ is rare.

### Combination algorithm:

This briefly outlines the _combination_ algorithm
that creates the _shared history_ of the _emulated monorepo_
to further contextualize the pieces and their relationships.

#### Fetch a toprepo commit and create a monocommit

`git-toprepo fetch` first fetches the _regular commit_ (_topcommit_) for the _toprepo_ itself
`git fetch ...`.
Then finds any _submodules_ that are bumped through Gerrit's _superproject subscription_
and fetches their _regular commits_.
All the _regular commits_ in the _rootrepo_ and the _assimilated submodules_
are _combined_ into one _monocommit_.
