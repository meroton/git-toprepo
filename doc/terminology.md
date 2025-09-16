# Terminology overview

This describes the terms involved in using the `git-toprepo` tool
to create a _toprepo_ for a _superrepo_ and its _submodules_.
this _combines_ the history of all _repositories_
into one _emulated monorepo_.

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

**git-toprepo**: The tool itself.
`git-toprepo` combines a _repository_
and some of its _submodules_
into a _toprepo_, an _emulated monorepo_.
Takes care to push _assimilated submodules_ to their remote server.

**monorepo**: A _repository_ with all the code,
it does not typically have _submodules_.
This makes it easy to make changes across different components
with a regular `git` workflow,
Generally without _submodule_ bumps and binary deliveries/integration
of first party code.
Gives unparalleled reproducibility
and understanding of the full product.

**pure monorepo**: A commonly sought concept,
such a _repository_ does not have _submodules_ at all.
There is just one _repository_ on the remote `git` server.
This realizes the full value of a _monorepo_,
but has no clear _access control_.

**toprepo**: A client-side construct
that _emulates_ a _monorepo_ for developers
but still tracks code as _submodules_ with their own remote git _repositories_.
This is created by `git-toprepo`.

As a performance optimization a  _monorepo_ created by `git-toprepo`
may still have _regular submodules_ though,
if the user does not want to combine all _submodules_.

**submodule access control**: One can easily apply
access control to individual _submodules_ by restricting access to their git _repositories_.
Such access control is not possible for different directories in a _pure monorepo_.

**commit**: A core `git` concept.

**topcommit**: A `git-toprepo` concept,
a commit in the _toprepo_.

`git-toprepo` shines when a developer wants to make one change across two _submodules_
and can track that as one _topcommit_
-- one _commit_ in the _emulated monorepo_ that consists of one _commit_ in each of the two _submodules_.
Those are meant to be merged together
through compatible CI systems that allow _shared gating_ between the constituent _repositories_.

**shared gating**: A CI system concept.
CI systems like `Gerrit` allows an organization to merge code to multiple _repositories_
atomically if all tests passes.
This allows the _toprepo_ to _emulate_ a _monorepo_ and have a shared gate.
`Gerrit` uses [superproject subscription] for this.

[superproject subscription]: https://gerrit-review.googlesource.com/Documentation/user-submodules.html

### Verbs

**combine**: `git-toprepo` combines the history of one _superrepo_ and (some of) its _submodules_
into _toprepo_ with a combined history for code in the _superrepo_ itself and its _assimilated submodules_.

**assimilate**: `git-toprepo` has _assimilated_ a _submodule_ into the _combined_ history.

### Technical details

For power users and _repository_ maintainers there are a few overlapping concepts.
<!-- TODO: link to our documentation of these. In the implementation documents or something. -->

**toprepo**: The `git-config` namespace for select `git-toprepo` settings that are configured through `git`.

**toprepo**: The `git` subcommand that runs `git-toprepo`.
`git` runs external subcommands like `git-<sub>` as `git <commit>`
to make it easy to create custom tools for `git`.

### Technical terms in the code

**rootrepo**: Emergent from core git concepts,
a _repository_ that is not a _submodule_ to another _repository_.
This is the main development _repository_ for a developer,
it often has _submodules_.

It may either be checked out with _regular submodules_:
`git-submodule init --recursive`
or as a _toprepo_ with `git-toprepo`.

## Examples

### Initialization: Create a toprepo for a repository

A _repository_ can be initialized to become a _toprepo_ with `git-toprepo`.
The configuration of the _toprepo_ is often managed in the _repository_ and is already checked in.

Short-form initialization of a _toprepo_.
```
$ toprepo $ git toprepo clone ssh://gerrit.example/substrate.git toprepo
$ cd toprepo
toprepo $ # This is a toprepo.
```

However, the code can also be checked out with regular git _submodules_.
```
$ git clone ssh://gerrit.example/substrate.git
$ cd substrate
substrate $ git submodule init --recursive
substrate $ # This is not a toprepo.
```

### Initialization: Some submodules are not assimilated

Now imagine that the _toprepo_ has one _submodule_ with a long and weird history,
it may be binary data that takes a lot of space and is not relevant to the developer.
Then it is often not _assimilated_ into the _toprepo_.

_toprepo_:
```
$ toprepo $ git toprepo clone ssh://gerrit.example/substrate.git toprepo
$ cd toprepo
toprepo $ # This is a toprepo.
toprepo $ git submodule status
-4e04771fcf658500987d0be5a9a63f8e77d5e386 binary_data_module
```

regular _repository_:
```
$ git clone ssh://gerrit.example/rootrepo.git
$ cd rootrepo
rootrepo $ git submodule status
-4e04771fcf658500987d0be5a9a63f8e77d5e386 binary_data_module
-661c1b2d568693e3b6b631ae66f6872b194674f1 source_code_module
```

### Pushing: git-toprepo pushes assimilated submodules to their servers

`git-toprepo` shines when a developer wants to make one change across two _submodules_
in one _topcommit_.

```
toprepo $ # modify one/file and two/file
toprepo $ git add one/file two/file; git commit
toprepo $ git-toprepo push HEAD:refs/for/main
```

This pushes the two paths inside the _toprepo_ to their constituent
_repositories_ on the git server (gerrit.example/one.git and gerrit.example/two.git).

The regular workflow with _submodules_, however, is more involved

```
rootrepo $ # modify one/file and two/file
rootrepo $ git -C one add file; git commit
rootrepo $ git -C two add file; git commit
rootrepo $ git -C one push HEAD:refs/for/main
rootrepo $ git -C two push HEAD:refs/for/main
# As you use Gerrit's superproject subscription, you would not need a rootrepo commit:
# rootrepo $ git add one two; git commit
# rootrepo $ git push HEAD:refs/for/main
```

> [!NOTE]
> Though committing inside _regular submodules_ in a _toprepo_ is rare.
> If a _submodule_'s history is not relevant to _combine_ into the _combined_ history
> it is unlikely that developers need to modify the code and make changes.

### Rebasing: git-toprepo gives a shared history that is easy to work with

With `git-toprepo`, rebasing _commits_ in any of the _assimilated submodules_
is as easy as working in a single _repository_.

```
toprepo $ git-toprepo fetch origin
toprepo $ git rebase -i origin/main
```

However when using _regular submodules_ in an _repository_
one needs to automate the workflow within individual _submodules_.

```
rootrepo $ git fetch origin
rootrepo $ git rebase -i origin/main
rootrepo $ submod_commit_hash=$(git ls-files --stage -- one | cut -d' ' -f2)
rootrepo $ git -C one rebase -i "$submod_commit_hash"
rootrepo $ submod_commit_hash=$(git ls-files --stage -- two | cut -d' ' -f2)
rootrepo $ git -C two rebase -i "$submod_commit_hash"
```

In the example, two _submodules_ does not look too bad at the face of it,
but note that the rebasing is not synchronized between the _submodules_.
Therefore, building and testing the code after resolving a merge conflict,
which may have only occurred in one _submodule_, is not trivial.

### Pushing: Push all submodules of a toprepo

As a _toprepo_ may not have _combined_ all _submodules_ into the history
some _submodules_ are left as _regular submodules_.
So to always push changes to all _submodules_ the following invocation is needed:

```
toprepo $ git-toprepo push HEAD:refs/for/main
toprepo $ git submodule for each push HEAD:refs/for/main
```
