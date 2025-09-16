# Terminology overview

This describes the terms involved in using `git-toprepo`, the tool,
to emulate a monorepo for a toprepo and its submodules.

## Terms

**git repository**: A core `git` concept,
a _repository_. May be local or on a remote server.

**git submodule**: A core `git` concept,
a _submodule_ is a _repository_ with a child-parent relation ship to another.

**regular submodule**: A core `git` concept,
a regular _submodule_ that is entirely managed through `git-submodule` etc.

**filtered submodule**: A `git-toprepo` concept,
a _submodule_ that has been assimilated into one combined history in the filtered _monorepo_.

**superrepo**: Emergent from core git concepts,
the parent _repository_ to a _submodule_.
It may be a _submodule_ to another _superrepo_.

**git-toprepo**: The tool itself.
`git-toprepo` filters a _toprepo_
and some of its _submodules_
into a _monorepo_ (emulated).
Takes care to push filtered _submodules_ to their remote server.

**toprepo**: A _repository_ with _submodules_.
This is the main development _repository_ for a developer.
the _toprepo_ is the root level _superrepo_
in a potential hierarchy of multiple levels of _submodules_.

It may either be checked out with **regular** `git-submodule init --recursive`
or with `git-toprepo` to create a _monorepo_.
If it is checked out with `git-toprepo`
some _soubmodules_ may not be filtered into the _monorepo_,
then those must be manipulated with `git-submodule` as in the first case.

**monorepo**: A _repository_ with all the code,
it does not typically have _submodules_.
This makes it easy to make changes across different components
with a regular `git` workflow,
Generally without_submodule_ bumps and binary deliveries/integration
of first party code.
Gives unparalleled reproducibility
and understanding of the full product.

Throughout `git-toprepo`'s code and documentation
_monorepo_ is often used to refer to an _emulated monorepo_, for conciseness.

**pure monorepo**: A commonly sought concept,
such a _repository_ does not have _submodules_ at all.
There is just one _repository_ on the remote `git` server.
This realizes the full value of a _monorepo_,
but has no clear _access control_.

**emulated monorepo**: A client side construct
that emulates a _monorepo_ for developer
but still tracks code as _submodules_ with their own remote git _repositories_.
This is created by `git-toprepo`.

As a performance optimization a  _monorepo_ created by `git-toprepo`
may still have _submodules_ though,
if the user does not want to assimilate all _submodules_.

**submodule access control**: One can easily apply
access control to individual _submodules_ by restricting access to their git _repositories_.
Such access control is not possible for different directories in a _pure monorepo_.

**commit**: A core `git` concept.

**monocommit**: A `git-toprepo` concept,
a commit in the _emulated monorepo_ for the _toprepo_.
May consist of multiple _commits_ in multiple _filtered submodules_.

`git-toprepo` shines when a developer wants to make one change across two _submodules_
and can track that as one _supercommit_
-- one _commit_ in the _emulated monorepo_ that consists of one _commit_ in each of the two _submodules_.
Those are meant to be merged together
through compatible CI systems that allow _shared gating_ between _repositories_.

**shared gating**: A CI system concept.
CI systems like `Gerrit` allows an organization to merge code to multiple _repositories_
atomically if all tests passes.
This allows us to emaulate a _monorepo_ and have a shared gate.
`Gerrit` uses [superproject subscription] for this

[superproject subscription]: https://gerrit-review.googlesource.com/Documentation/user-submodules.html

### Verbs

**filter**: `git-toprepo` filters the history of one _toprepo_ and its _regular submodules_
into an _emulated monorepo_ with a combined history for all the _toprepo_ itself and its _filtered submodules_.

**combined**: `git-toprepo` has _combined_ the history into an _emulated monorepo_ with combined history.

**manage**: `git-toprepo` manages a git _toprepo_ and has _expanded_ the history into an _emulated monorepo_.

### Technical details

For power users and _repository_ maintainers there are a few overlapping concepts.
<!-- TODO: link to our documentation of these. In the implementation documents or something. -->

**toprepo**: The `git-config` namespace for select `git-toprepo` settings that are configured through `git`.

**toprepo**: The `git` subcommand that runs `git-toprepo`.
`git` runs external subcommands like `git-<sub>` as `git <commit>`
to make it easy to create custom tools for `git`.

## Examples

### Initialization: The toprepo may be a monorepo

The configuration of a _monorepo_ is often managed in the _toprepo_ and is already checked in.

Short-form initialization of a _monorepo_.
```
$ monorepo $ git toprepo clone ssh://gerrit.example/toprepo.git monorepo
$ cd monorepo
monorepo $ # This is a monorepo.
```

<!-- Long-form initialization of a _monorepo_. -->
<!-- ``` -->
<!-- $ mkdir monorepo -->
<!-- $ cd monorepo -->
<!-- monorepo $ git toprepo init ssh://gerrit.example/toprepo -->
<!-- monorepo $ git toprepo fetch -->
<!-- monorepo $ # This is a monorepo -->
<!-- ``` -->

However, the code can also be checked out with regular git _submodules_.
```
$ git clone ssh://gerrit.example/toprepo.git
$ cd toprepo
toprepo $ git submodule init --recursive
toprepo $ # This is not a monorepo
```

### Initialization: Some submodules are not filtered in

Now imagine that the _toprepo_ has one _submodule_ with a long and weird history,
it may be binary data that takes a lot of space and is not relevant to the developer.
Then it is often **not filtered** into the _emulated monorepo_.

_monorepo_:
```
$ monorepo $ git toprepo clone ssh://gerrit.example/toprepo.git monorepo
$ cd monorepo
monorepo $ # This is a monorepo.
monorepo $ git submodule status
-4e04771fcf658500987d0be5a9a63f8e77d5e386 binary_data_module
```

regular _toprepo_:
```
$ git clone ssh://gerrit.example/toprepo.git
$ cd toprepo
toprepo $ git submodule status
-4e04771fcf658500987d0be5a9a63f8e77d5e386 binary_data_module
-661c1b2d568693e3b6b631ae66f6872b194674f1 source_code_module
```

### Pushing: git-toprepo pushes filtered submodules to their servers

`git-toprepo` shines when a developer wants to make one change across two _submodules_
in one _supercommit_.

```
monorepo $ # modify one/file and two/file
monorepo $ git add one/file two/file; git commit
monorepo $ git-toprepo push HEAD:refs/for/main
```

This pushes the two paths inside the _monorepo_ to their constituent
_repositories_ on the git server (gerrit.example/one.git and gerrit.example/two.git).

The regular workflow with submodules, however, is more involved

```
toprepo $ # modify one/file and two/file
toprepo $ git -C one add file; git commit
toprepo $ git -C two add file; git commit
toprepo $ git -C one push HEAD:refs/for/main
toprepo $ git -C two push HEAD:refs/for/main
# As you use Gerrit's superproject subscription, you would not need a toprepo commit:
# toprepo $ git add one two; git commit
# toprepo $ git push HEAD:refs/for/main
```

First the two _submodules_ are handled separately
then the _toprepo_ must also bump its _submodule_ pointers to the new commits within them.

> [!NOTE]
> Though committing inside _regular submodules_ in a _monorepo_ is rare.
> If a _submodule_'s history is not relevant to _filter_ into the combined history
> it is unlikely that developers need to modify the code and make changes.

### Rebasing: git-toprepo gives a shared history that is easy to work with

With `git-toprepo`, rebasing _commits_ in any of the _filtered submodules_
is as easy as working in a single _repository_.

```
monorepo $ git-toprepo fetch origin
monorepo $ git rebase -i origin/main
```

However when using _regular submodules_ in an _unmanaged_ _toprepo_
one needs to automate the workflow within individual _submodules_.

```
toprepo $ git fetch origin
toprepo $ git rebase -i origin/main
toprepo $ submod_commit_hash=$(git ls-files --stage -- one | cut -d' ' -f2)
toprepo $ git -C one rebase -i "$submod_commit_hash"
toprepo $ submod_commit_hash=$(git ls-files --stage -- two | cut -d' ' -f2)
toprepo $ git -C two rebase -i "$submod_commit_hash"
```

In the example, two _submodules_ does not look too bad at the face of it,
but note that the rebasing is not synchronized between the _submodules_.
Therefore, building and testing the code after resolving a merge conflict,
which may have only occurred in one _submodule_, is not trivial.

### Pushing: Push all submodules of an emulated monorepo

As an _emulated monorepo_ may not have _expanded_ all _submodules_ into the combined history
some _submodules_ are left as _regular submodules_.
So to always push changes to all _submodules_ the following invocation is needed:

```
monorepo $ git-toprepo push HEAD:refs/for/main
monorepo $ git submodule for each push HEAD:refs/for/main
```
