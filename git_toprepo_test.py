#!/user/bin/env python3
import pytest
import subprocess
from pathlib import PurePosixPath

import git_toprepo


def test_usage_help():
    with pytest.raises(SystemExit) as pytest_err:
        git_toprepo.main(["argv0"])
    assert pytest_err.type == SystemExit
    assert pytest_err.value.code == 2


def test_push_refspec_parser():
    assert git_toprepo.PushRefSpec.parse("abc:refs/def") == git_toprepo.PushRefSpec(
        local_ref="abc", remote_ref="refs/def"
    )
    assert git_toprepo.PushRefSpec.parse("main") == git_toprepo.PushRefSpec(
        local_ref="refs/heads/main", remote_ref="refs/heads/main"
    )
    assert git_toprepo.PushRefSpec.parse("pr/foo") == git_toprepo.PushRefSpec(
        local_ref="refs/heads/pr/foo", remote_ref="refs/heads/pr/foo"
    )
    with pytest.raises(ValueError, match="Multiple ':' "):
        git_toprepo.PushRefSpec.parse("a:b:c")


def test_repository_basename():
    assert git_toprepo.repository_basename("https://github.com/org/repo") == "repo"
    assert git_toprepo.repository_basename("https://github.com/org/repo.git") == "repo"
    assert git_toprepo.repository_basename("git://github.com:repo") == "repo"
    assert git_toprepo.repository_basename("abc\\org\\repo") == "repo"


def test_join_submodule_url():
    # Relative.
    assert (
        git_toprepo.join_submodule_url("https://github.com/org/repo", "./foo")
        == "https://github.com/org/repo/foo"
    )
    assert (
        git_toprepo.join_submodule_url("https://github.com/org/repo", "../foo")
        == "https://github.com/org/foo"
    )
    assert (
        git_toprepo.join_submodule_url("https://github.com/org/repo", "../../foo")
        == "https://github.com/foo"
    )

    # Ignore double slash.
    assert (
        git_toprepo.join_submodule_url("https://github.com/org/repo", ".//foo")
        == "https://github.com/org/repo/foo"
    )

    # Handle too many '../'.
    assert (
        git_toprepo.join_submodule_url("https://github.com/org/repo", "../../../foo")
        == "https://github.com/../foo"
    )

    # Absolute.
    assert (
        git_toprepo.join_submodule_url("parent", "ssh://github.com/org/repo")
        == "ssh://github.com/org/repo"
    )


def test_annotate_message():
    # Don't fold the footer into the subject line, leave an empty line.
    assert (
        git_toprepo.annotate_message(b"Subject line\n", b"sub/dir", b"123hash")
        == b"""\
Subject line

^-- sub/dir 123hash
"""
    )

    assert (
        git_toprepo.annotate_message(b"Subject line, no LF", b"sub/dir", b"123hash")
        == b"""\
Subject line, no LF

^-- sub/dir 123hash
"""
    )

    assert (
        git_toprepo.annotate_message(b"Double subject line\n", b"sub/dir", b"123hash")
        == b"""\
Double subject line

^-- sub/dir 123hash
"""
    )

    assert (
        git_toprepo.annotate_message(
            b"Subject line, extra LFs\n\n\n", b"sub/dir", b"123hash"
        )
        == b"""\
Subject line, extra LFs

^-- sub/dir 123hash
"""
    )

    assert (
        git_toprepo.annotate_message(b"Multi line\n\nmessage\n", b"sub/dir", b"123hash")
        == b"""\
Multi line

message
^-- sub/dir 123hash
"""
    )

    assert (
        git_toprepo.annotate_message(
            b"Multi line\n\nmessage, no LF", b"sub/dir", b"123hash"
        )
        == b"""\
Multi line

message, no LF
^-- sub/dir 123hash
"""
    )

    assert (
        git_toprepo.annotate_message(
            b"Multi line\n\nmessage, extra LFs\n\n\n", b"sub/dir", b"123hash"
        )
        == b"""\
Multi line

message, extra LFs
^-- sub/dir 123hash
"""
    )


def test_join_annotated_commit_messages():
    boring_messages = [
        b"Update git submodules\n^-- <top> 123hash\n",
    ]
    nice_messages = [
        b"An amazing feature\n^-- sub/dir 123hash\n",
    ]
    expected_message = b"".join(nice_messages + boring_messages)

    git_toprepo.join_annotated_commit_messages(
        boring_messages + nice_messages
    ) == expected_message

    git_toprepo.join_annotated_commit_messages(
        nice_messages + boring_messages
    ) == expected_message


def test_try_parse_commit_hash_from_message():
    example_message = b"""\
Single line
^-- other/dir 456abc
Single line
^-- sub/dir 123abc
Multi line

message
^-- <top> def789
"""

    assert git_toprepo.try_parse_top_hash_from_message(example_message) == b"def789"
    assert (
        git_toprepo.try_parse_commit_hash_from_message(example_message, b"sub/dir")
        == b"123abc"
    )
    assert (
        git_toprepo.try_parse_commit_hash_from_message(
            example_message, b"no/matching/dir"
        )
        is None
    )


def test_try_get_topic_from_message():
    example_message = b"""\
Subject line

More lines

Footer: my footer
Topic: my topic
Another: another footer
"""
    assert git_toprepo.try_get_topic_from_message(example_message) == "my topic"

    example_message_not_really_a_footer = b"""\
Subject line

Topic: my topic

More lines

Footer: my footer
Another: another footer
"""
    assert (
        git_toprepo.try_get_topic_from_message(example_message_not_really_a_footer)
        == "my topic"
    )

    example_message_no_topic = b"""\
Subject line

More lines

Footer: my footer
Another: another footer
"""
    assert git_toprepo.try_get_topic_from_message(example_message_no_topic) is None

    example_message_multiple_topics = b"""\
Subject line

More lines

Footer: my footer
Topic: my topic
Topic: my topic2
Another: another footer
"""
    with pytest.raises(ValueError, match="Expected a single footer 'Topic: <topic>'"):
        git_toprepo.try_get_topic_from_message(example_message_multiple_topics)


def test_remote_to_repo():
    git_modules = [
        git_toprepo.GitModuleInfo(
            name="submodule-name",
            path=PurePosixPath("sub/dir"),
            branch=".",
            url="ssh://github.com/org/subrepo",
            raw_url="../subrepo",
        ),
    ]
    config = git_toprepo.Config(
        missing_commits={},
        top_fetch_url="ssh://user@toprepo/fetch",
        top_push_url="ssh://user@toprepo/push",
        repos=[
            git_toprepo.RepoConfig(
                name="sub",
                enabled=True,
                raw_urls=[
                    "../subrepo",
                ],
                fetch_url="ssh://user@subrepo/fetch",
                push_url="ssh://user@subrepo/push",
            ),
        ],
    )
    assert git_toprepo.remote_to_repo("origin", git_modules, config) == (
        git_toprepo.TopRepo.name,
        None,
    )
    assert git_toprepo.remote_to_repo(".", git_modules, config) == (
        git_toprepo.TopRepo.name,
        None,
    )
    assert git_toprepo.remote_to_repo("", git_modules, config) == (
        git_toprepo.TopRepo.name,
        None,
    )
    assert git_toprepo.remote_to_repo("toprepo/fetch", git_modules, config) == (
        git_toprepo.TopRepo.name,
        None,
    )
    assert git_toprepo.remote_to_repo("subrepo/push", git_modules, config) == (
        "sub",
        git_modules[0],
    )
    # The URL in .gitmodules should work.
    assert git_toprepo.remote_to_repo("org/subrepo", git_modules, config) == (
        "sub",
        git_modules[0],
    )
    assert git_toprepo.remote_to_repo("no/subrepo", git_modules, config) == (
        None,
        None,
    )


def test_get_config_location(tmp_path):
    server_config = tmp_path / "server/config"
    server_config.mkdir(parents=True)
    subprocess.run(cwd=server_config, check=True, args="git init --quiet".split(" "))
    subprocess.run(
        cwd=server_config,
        check=True,
        args="git commit -q -m InitialCommit --allow-empty".split(" "),
    )
    subprocess.run(
        cwd=server_config, check=True, args="git branch -q config-branch".split(" ")
    )
    subprocess.run(
        cwd=server_config, check=True, args="git checkout -q config-branch".split(" ")
    )
    (server_config / "toprepo.config").write_text(
        "[toprepo.missing-commit]\nrev-test-hash = correct-path"
    )
    subprocess.run(
        cwd=server_config, check=True, args="git add toprepo.config".split(" ")
    )
    subprocess.run(
        cwd=server_config, check=True, args="git commit -q -m Config".split(" ")
    )

    server_top = tmp_path / "server/top"
    server_top.mkdir(parents=True)
    subprocess.run(cwd=server_top, check=True, args="git init --quiet".split(" "))
    (server_top / ".toprepo").write_text(
        """\
[toprepo]
    config-v1 = ../config^refs/heads/config-branch:toprepo.config
"""
    )
    subprocess.run(cwd=server_top, check=True, args="git add .toprepo".split(" "))
    subprocess.run(
        cwd=server_top, check=True, args="git commit -q -m Commit".split(" ")
    )

    worktree_path = tmp_path / "worktree"
    worktree_path.mkdir(parents=True)
    subprocess.run(cwd=worktree_path, check=True, args="git init --quiet".split(" "))
    worktree = git_toprepo.MonoRepo(worktree_path)
    subprocess.run(
        cwd=worktree.path,
        check=True,
        args="git config toprepo.top.fetchUrl ../server/top".split(" "),
    )
    subprocess.run(
        ["git", "config", "toprepo.top.fetchUrl", f"file://{server_top.absolute()}"],
        cwd=worktree.path,
        check=True,
    )
    subprocess.run(
        ["git", "config", "toprepo.missing-commit.rev-test-hash", "local-path"],
        cwd=worktree.path,
        check=True,
    )

    config_loader = git_toprepo.create_toprepo_config_loader(worktree, online=True)
    config_loader.fetch_remote_config()
    config_dict = config_loader.get_config_dict()
    assert config_dict["toprepo.missing-commit.rev-test-hash"] == [
        "correct-path",
        "local-path",
    ]


def test_read_config_from_disk():
    pass


def test_read_config_from_git():
    pass


def test_init_fetch_checkout():
    pass


def test_fetch_fast_filter():
    pass


def test_push():
    pass


def test_refilter_offline():
    pass


def test_refilter_from_scratch():
    pass


def test_missing_commits():
    pass


def test_filtering_keeps_workspace():
    # No git-clean
    # No git-reset-hard
    pass
