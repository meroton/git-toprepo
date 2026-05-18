# Tinkering

This contains some technical notes on the development of `git-toprepo`
and important invariants in git and other review systems.

## Git
### Subcommand dispatch
#### Requires a manpage for git-toprepo

https://github.com/meroton/git-toprepo/issues/285

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
