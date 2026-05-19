# Tinkering

This contains some technical notes on the development of `git-toprepo`
and important invariants in git and other review systems.

## Git
### Subcommand dispatch
#### Requires a manpage for git-toprepo

https://github.com/meroton/git-toprepo/issues/285

### Hooks

There is not standard for how to handle multiple push hooks.
Where `toprepo` wants to register a helpful error message if a plain `git push` is used.
But other tools may also perform behind-the-scenes operation in a push hook.
We can amend out hook installation to check for an existing hook create
a launcher hook that runs both.
Though as `toprepo` usually clones and initializes the repository
we are often the first hook on the scene.
So this installation burden is - unfortunately - punted to the next hook
which we don't control.

The best solution is for the repository maintainers to decide on the hook strategy,
it cannot be the responsibility of individual tools that don't know about each other.
We should design `toprepo` to work well in such a scenario
and possibly have a subcommand to just print the hook.
Also to install the real hook under a `.git/hooks/toprepo.push` name
and let the push hook call that would make it easier to override the hook.

This was identified in https://github.com/meroton/git-toprepo/issues/278 .

#### You don't want to launch a debugger on `git toprepo`.

As it then first attaches to `git`.
Instead add the hyphen to save yourself some trouble.

## Gerrit
### Topics can be reused

So the link between a topic and a supercommit is not always right.
In practice this is rare and ambiguities can always be resolved from reading the log
in Gerrit and comparing timestamps.
It would be best if we could configure Gerrit to disallow reuse of already merged topics.

### ChangeIds can be resued

In rare race conditions the ChangeId of a change can be resued.
So multiple changes (the canonical ID in Gerrit's database is just the number)
can have the same ChangeId.
We do not forsee any practical problems with this,
but it is good to keep in mind.

## Debugging

Nils uses `rust-gdb` and it works decently well.
Though many of rust's collection types are still opaque :(.
