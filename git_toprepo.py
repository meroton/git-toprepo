#!/usr/bin/env python3
"""git-submodule made easy with git-toprepo

git-toprepo merges subrepositories into a common history, similar to git-subtree.
"""
import argparse
import itertools
import os
import re
import shutil
import subprocess
import sys
import textwrap
from abc import ABC, abstractmethod
from collections import defaultdict
from dataclasses import dataclass
from functools import cached_property, lru_cache, partial
from pathlib import Path, PurePath, PurePosixPath
from queue import PriorityQueue
from typing import (
    DefaultDict,
    Dict,
    Iterable,
    List,
    Optional,
    Set,
    Tuple,
    TypeVar,
    Union,
)

try:
    import git_filter_repo
except ImportError:
    print("ERROR: git-filter-repo is missing")
    print("Please run:  python3 -m pip install git-filter-repo")
    sys.exit(2)

# git-filter-repo runs `git reset --hard` after filtering. Disable that.
original_git_filter_repo_cleanup = git_filter_repo.RepoFilter.cleanup


def patched_git_filter_repo_cleanup(repo, repack, reset, **kwargs) -> None:
    assert not repack, repack
    reset = False
    original_git_filter_repo_cleanup(repo, repack, reset, **kwargs)


git_filter_repo.RepoFilter.cleanup = patched_git_filter_repo_cleanup

RepoName = str
Url = str
RawUrl = str
RefStr = str
Ref = bytes
TreeHash = bytes
CommitHash = bytes
RepoFilterId = Union[int, CommitHash]


default_fetch_args = ["--prune", "--prune-tags", "--tags"]


class Repo:
    def __init__(self, repo: Path):
        # Make relative for shorter error messages.
        try:
            repo = repo.relative_to(Path.cwd())
        except ValueError:
            pass
        self.path: Path = repo

    @cached_property
    def git_dir(self) -> Path:
        return determine_git_dir(self.path)


class MonoRepo(Repo):
    name: str = "mono repo"

    def __init__(self, repo: Path):
        toplevel_repo_dir = Path(
            subprocess.check_output(
                ["git", "-C", str(repo), "rev-parse", "--show-toplevel"],
                text=True,
            ).rstrip("\n")
        )
        super().__init__(toplevel_repo_dir)

    @lru_cache
    def get_toprepo_fetch_url(self) -> Url:
        fetch_url = self.get_toprepo_fetch_url_impl("remote.origin.url", False)
        if fetch_url is None or fetch_url == "file:///dev/null":
            # TODO: 2024-04-29 Remove after migration.
            fetch_url = self.get_toprepo_fetch_url_impl("toprepo.top.fetchUrl", True)
            subprocess.check_output(
                ["git", "-C", str(self.path), "config", "remote.origin.url", fetch_url],
                text=True,
            ).rstrip()
        # TODO: 2024-04-29 Remove after migration.
        push_url = self.get_toprepo_fetch_url_impl("remote.top.pushUrl", False)
        if push_url is None:
            push_url = self.get_toprepo_fetch_url_impl("toprepo.top.pushUrl", True)
            subprocess.check_output(
                ["git", "-C", str(self.path), "config", "remote.top.pushUrl", push_url],
                text=True,
            ).rstrip()
        return fetch_url

    def get_toprepo_fetch_url_impl(self, toprepo_fetchurl_key, throw) -> Optional[Url]:
        try:
            fetch_url = subprocess.check_output(
                ["git", "-C", str(self.path), "config", toprepo_fetchurl_key], text=True
            ).rstrip()
        except subprocess.CalledProcessError as err:
            if err.returncode == 1:
                if throw:
                    raise ValueError(
                        f"git-config {toprepo_fetchurl_key} is missing in {self.path}"
                    )
                else:
                    return None
            raise
        return fetch_url

    def get_toprepo_dir(self) -> Path:
        return self.get_subrepo_dir(TopRepo.name)

    def get_subrepo_dir(self, name: RepoName):
        return self.git_dir / "repos" / name


class TopRepo(Repo):
    is_top = True
    name = "top"
    name_bytes = b"top"

    def __init__(self, repo: Path, fetch_url: Url, push_url: Url):
        super().__init__(repo=repo)
        self.config = RepoConfig(
            name=TopRepo.name,
            enabled=True,
            raw_urls=[],
            fetch_url=fetch_url,
            fetch_args=default_fetch_args,
            push_url=push_url,
        )

    @staticmethod
    def from_config(repo: Path, config: "Config") -> "TopRepo":
        return TopRepo(
            repo,
            fetch_url=config.top_fetch_url,
            push_url=config.top_push_url,
        )


@dataclass(frozen=True)
class PushRefSpec:
    local_ref: RefStr
    remote_ref: RefStr

    @staticmethod
    def parse(refspec: str) -> "PushRefSpec":
        if refspec.count(":") == 0:
            if not refspec.startswith("refs/"):
                refspec = "refs/heads/" + refspec
            refspec = f"{refspec}:{refspec}"
        if refspec.count(":") != 1:
            raise ValueError(f"Multiple ':' found in refspec {refspec}")
        local_ref, remote_ref = refspec.split(":")
        return PushRefSpec(local_ref, remote_ref)


@dataclass(frozen=True)
class PushInstruction:
    repo: Repo
    commit_hash: str
    extra_args: List[str]

    def same_but_commit(self, other: "PushInstruction"):
        return self.repo.path == other.repo.path and self.extra_args == other.extra_args


_T = TypeVar("_T")


def unique_append(dest: List[_T], item):
    if item not in dest:
        dest.append(item)


def unique_extend(dest: List[_T], items: Iterable[_T]):
    for item in items:
        unique_append(dest, item)


def try_relative_path(path: Path, other: Path = Path.cwd()) -> Path:
    """Returns a relative path, if possible."""
    try:
        return path.relative_to(other)
    except ValueError:
        return path


def determine_git_dir(repo: Path) -> Path:
    git_dir_bytes = git_filter_repo.GitUtils.determine_git_dir(
        str(repo).encode("utf-8")
    )
    return Path(git_dir_bytes.decode("utf-8"))


@dataclass(frozen=True)
class GitModuleInfo:
    name: str
    path: PurePosixPath
    branch: Optional[str]
    url: Url
    raw_url: RawUrl

    def __hash__(self):
        return hash((self.name, self.path, self.branch, self.url, self.raw_url))


def removesuffix(text: str, suffix: str):
    # Available in Python 3.9.
    if text.endswith(suffix):
        text = text[: -len(suffix)]
    return text


def repository_basename(repository: Url) -> str:
    # For both an URL and a file path, assume a limited set of separators.
    idx = max(repository.rfind(sep) for sep in r"/\:")
    # idx+1 also works if no separator was found.
    basename = repository[idx + 1 :]
    basename = removesuffix(basename, ".git")
    return basename


def repository_name(repository: Url) -> str:
    name = repository
    # Handle relative paths.
    name = name.replace("../", "")
    name = name.replace("./", "")
    # Remove scheme.
    idx = name.find("://")
    if idx != -1:
        name = name[idx + 3 :]
        # Remove the domain name.
        idx = name.find("/")
        # idx+1 also works if no separator was found.
        name = name[idx + 1 :]
    # Annoying with double slash.
    name = name.replace("//", "/")
    name = name.strip("/")
    name = removesuffix(name, ".git")
    # For both an URL and a file path, assume a limited set of separators.
    for sep in r"/\:":
        name = name.replace(sep, "-")
    return name


def join_submodule_url(parent: Url, other: RawUrl) -> Url:
    if other.startswith("./") or other.startswith("../") or other == ".":
        idx = parent.find("://")
        scheme_end = idx + 3 if idx != -1 else idx + 1
        scheme = parent[:scheme_end]
        parent = parent[scheme_end:]
        parent = parent.rstrip("/")
        while True:
            if other.startswith("/"):
                # Ignore double slash.
                other = other[1:]
            elif other.startswith("./"):
                other = other[2:]
            elif other.startswith("../"):
                idx = parent.rfind("/")
                if idx != -1:
                    parent = parent[:idx]
                else:
                    # Too many '../', move it from other to parent.
                    parent += "/.."
                other = other[3:]
            else:
                break
        if other in ("", "."):
            ret = f"{scheme}{parent}"
        else:
            ret = f"{scheme}{parent}/{other}"
    else:
        ret = other
    return ret


ANNOTATED_TOP_SUBDIR = b"<top>"


def annotate_message(
    message: bytes, subdir: bytes, orig_commit_hash: CommitHash
) -> bytes:
    ret = message.rstrip(b"\n") + b"\n"
    if b"\n\n" not in ret:
        # Subject only, no message body.
        # Add another LF to avoid folding into the subject line
        # in 'git log --oneline'.
        ret += b"\n"
    ret += b"^-- " + subdir + b" " + orig_commit_hash + b"\n"
    return ret


def join_annotated_commit_messages(messages: List[bytes]) -> bytes:
    top_messages = []
    bottom_messages = []
    for msg in messages:
        if msg.startswith(b"Update git submodules\n\n"):
            # Boring Gerrit branch following bump message.
            # Use the message from the submodule itself instead.
            bottom_messages.append(msg)
        else:
            top_messages.append(msg)
    return b"".join(top_messages + bottom_messages)


def try_parse_top_hash_from_message(message: bytes) -> Optional[CommitHash]:
    return try_parse_commit_hash_from_message(message, ANNOTATED_TOP_SUBDIR)


def try_parse_commit_hash_from_message(
    message: bytes, subdir: bytes
) -> Optional[CommitHash]:
    hash_annotation_regex = rb"^\^-- %s ([0-9a-f]+)$" % subdir
    matches = list(re.finditer(hash_annotation_regex, message, re.MULTILINE))
    if len(matches) == 0:
        return None
    elif len(matches) > 1:
        raise ValueError(f"Multiple hashes found for {subdir} in the message {message}")
    else:
        (match,) = matches
    top_commit_hash = match.group(1)
    return top_commit_hash


def try_get_topic_from_message(message: bytes) -> Optional[str]:
    message_str = message.decode("utf-8")
    topic_regex = r"^Topic: (.+)$"
    matches = list(re.finditer(topic_regex, message_str, re.MULTILINE))
    if len(matches) == 0:
        return None
    if len(matches) > 1:
        raise ValueError(
            f"Expected a single footer 'Topic: <topic>' in the message\n{message_str}"
        )
    (match,) = matches
    topic = match.group(1)
    return topic


def log_run_git(
    repo: Optional[Path],
    args: List[str],
    check=True,
    dry_run=False,
    log_command=True,
    **kwargs,
) -> Optional[subprocess.CompletedProcess]:
    """Log the git command and run it for the correct repo."""
    full_args: List[str]
    if repo is None:
        full_args = ["git"] + args
    else:
        full_args = ["git", "-C", str(repo)] + args
    cmdline = subprocess.list2cmdline(full_args)
    if dry_run:
        print(f"\rWould run  {cmdline}", file=sys.stderr)
        ret = None
    else:
        if log_command:
            print(f"\rRunning   {cmdline}", file=sys.stderr)
        ret = subprocess.run(full_args, check=check, **kwargs)
    return ret


def ref_exists(repo: Repo, ref: str):
    result = subprocess.run(
        ["git", "-C", str(repo.path)]
        + ["rev-parse", "--verify", "--quiet", ref + "^{commit}"],
        check=False,
        stdout=subprocess.DEVNULL,
    )
    if result.returncode == 1:
        return False
    else:
        result.check_returncode()
        return True


def delete_refs(repo: Repo, refs: Iterable[RefStr]) -> None:
    update_ref_instruction = "".join(f"delete {ref}\n" for ref in refs)
    if update_ref_instruction != "":
        subprocess.run(
            ["git", "-C", str(repo.path), "update-ref", "--stdin"],
            input=update_ref_instruction,
            text=True,
        )


def get_remote_origin_refs(repo: Repo) -> List[RefStr]:
    show_ref_stdout = subprocess.check_output(
        ["git", "-C", str(repo.path), "show-ref"],
        text=True,
    )
    origin_ref_prefix = "refs/remotes/origin/"
    remote_origin_refs: List[str] = []
    for line in show_ref_stdout.splitlines(keepends=False):
        # <commit-hash> SP <ref> LF
        _, ref = line.strip().split(" ", 1)
        if ref.startswith(origin_ref_prefix):
            remote_origin_refs.append(ref)
    return remote_origin_refs


IgnoredCommits = Dict[RawUrl, Set[CommitHash]]


class ConfigParsingError(RuntimeError):
    pass


@dataclass(frozen=True)
class RepoConfig:
    name: RepoName
    """Name of the storage directory and used for pattern matching."""
    enabled: bool
    """Flags if this repos should be expanded or not."""
    raw_urls: List[RawUrl]
    """Exact matching against sub repos configs like .gitmodules.

    These URLs are not resolved any may be relative.
    """
    fetch_url: Url
    """Absolute URL to git-fetch from."""
    fetch_args: List[str]
    """Extra options for git-fetch."""
    push_url: Url
    """Absolute URL to git-push to."""


_ConfigDict_unset = object()


class ConfigDict(DefaultDict[str, List[str]]):
    """ConfigDict maps from a key to a list of values.

    For single value options, use the last value only (values[-1]).
    """

    def __init__(self):
        super().__init__(list)

    @staticmethod
    def parse(config_lines: str) -> "ConfigDict":
        ret = ConfigDict()
        for line in config_lines.splitlines(keepends=False):
            key, value = line.split("=", 1)
            ret[key].append(value)
        return ret

    @staticmethod
    def join(config_dicts: Iterable["ConfigDict"]) -> "ConfigDict":
        ret = ConfigDict()
        for config_dict in config_dicts:
            for key, values in config_dict.items():
                ret[key].extend(values)
        return ret

    def extract_mapping(self, prefix: str) -> Dict[str, "ConfigDict"]:
        """Extracts for example submodule.<name>.<key>=<value>."""
        assert not prefix.endswith("."), prefix
        prefix += "."
        ret = defaultdict(ConfigDict)
        for key, values in self.items():
            if key.startswith(prefix):
                name, subkey = key[len(prefix) :].split(".", 1)
                ret[name][subkey] = values
        return ret

    def get_singleton(
        self, key, default: Optional[str] = _ConfigDict_unset
    ) -> Optional[str]:
        """Varifies that there are no conflicting configuration values."""
        if default == _ConfigDict_unset:
            values = self[key]
        else:
            values = self.get(key, [default])
        values = sorted(set(values))
        assert len(values) != 0, f"The key {key!r} should not exist without a value"
        if len(values) != 1:
            values_str = ", ".join(values[:-1]) + f" and {values[-1]}"
            raise ValueError(f"Conflicting values for {key}: {values_str}")
        return values[0]


class ConfigLoader(ABC):
    def fetch_remote_config(self) -> None:
        pass

    @abstractmethod
    def git_config_list(self) -> str:
        raise NotImplementedError()

    def get_config_dict(self) -> ConfigDict:
        return ConfigDict.parse(self.git_config_list())


class MultiConfigLoader(ConfigLoader):
    def __init__(self, config_loaders: List[ConfigLoader]):
        self.config_loaders: List[ConfigLoader] = config_loaders

    def fetch_remote_config(self) -> None:
        for config_loader in self.config_loaders:
            config_loader.fetch_remote_config()

    def git_config_list(self) -> str:
        parts = []
        for config_loader in self.config_loaders:
            part = config_loader.git_config_list()
            if part != "" and part[-1] != "\n":
                part += "\n"
            parts.append(part)
        # The first part should override everything else.
        parts.reverse()
        return "".join(parts)


class LocalGitConfigLoader(ConfigLoader):
    """Loads configuration from a file on disk."""

    def __init__(self, repo: Repo):
        self.repo = repo

    def git_config_list(self) -> str:
        return subprocess.check_output(
            ["git", "-C", str(self.repo.path), "config", "--list"],
            env=os.environ,  # To make monkeypatching work for tests.
            text=True,
        )


class ContentConfigLoader(ConfigLoader):
    @abstractmethod
    def read_config_file_content(self) -> str:
        raise NotImplementedError()

    def git_config_list(self) -> str:
        config_file_content = self.read_config_file_content()
        return subprocess.check_output(
            ["git", "config", "--file", "-", "--list"],
            input=config_file_content,
            text=True,
        )


class StaticContentConfigLoader(ContentConfigLoader):
    def __init__(self, content: str):
        self.content = content

    def read_config_file_content(self) -> str:
        return self.content


class LocalFileConfigLoader(ContentConfigLoader):
    def __init__(self, filename: Path, allow_missing: bool = False):
        self.filename = filename
        self.allow_missing = allow_missing

    def fetch_remote_config(self) -> None:
        pass

    def read_config_file_content(self) -> str:
        if self.allow_missing and not self.filename.exists():
            return ""
        return self.filename.read_text(encoding="utf-8")


class GitRemoteConfigLoader(ContentConfigLoader):
    def __init__(
        self,
        url: Url,
        remote_ref: RefStr,
        filename: PurePosixPath,
        local_repo: Repo,
        local_ref: RefStr,
    ):
        self.url = url
        self.remote_ref = remote_ref
        self.filename = filename
        self.local_repo = local_repo
        self.local_ref = local_ref

    def fetch_remote_config(self) -> None:
        log_run_git(
            self.local_repo.path,
            ["fetch", "--quiet", self.url, f"+{self.remote_ref}:{self.local_ref}"],
            stdout=sys.__stderr__.fileno(),
            stderr=subprocess.STDOUT,
        )

    def read_config_file_content(self) -> str:
        return subprocess.check_output(
            ["git", "-C", str(self.local_repo.path)]
            + ["show", f"{self.local_ref}:{self.filename.as_posix()}"],
            text=True,
        )


class ConfigAccumulator:
    def __init__(self, monorepo: MonoRepo, online: bool):
        self.monorepo = monorepo
        self.online = online

    def try_load_main_config(self) -> Optional[ConfigDict]:
        try:
            return self.load_main_config()
        except RuntimeError as err:
            print(f"ERROR: Could not find configuration location: {err}")
            return None

    def load_main_config(self) -> ConfigDict:
        """Load from the remote unless specified in .git/config."""
        config_loader = MultiConfigLoader(
            [
                LocalGitConfigLoader(self.monorepo),
                StaticContentConfigLoader(
                    """\
[toprepo.config.default]
    type = "git"
    url = .
    ref = refs/meta/git-toprepo
    path = toprepo.config
"""
                ),
            ]
        )
        return self.load_config(config_loader)

    def load_config(self, config_loader: ConfigLoader) -> ConfigDict:
        full_config_dict = ConfigDict()
        existing_names = set()
        config_loaders_todo = [config_loader]
        while len(config_loaders_todo) != 0:
            config_loader = config_loaders_todo.pop(0)
            if self.online:
                config_loader.fetch_remote_config()
            current_config_dict = config_loader.get_config_dict()
            sub_config_loaders = self.get_config_loaders(
                current_config_dict, full_config_dict
            )
            # Earlier loaded configs overrides later loaded configs.
            full_config_dict = ConfigDict.join([current_config_dict, full_config_dict])
            # Traverse into sub-config-loaders.
            for name, sub_config_loader in sub_config_loaders.items():
                if name in existing_names:
                    raise ValueError(
                        f"toprepo.config.{name} configurations found in multiple sources"
                    )
                existing_names.add(name)
                config_loaders_todo.append(sub_config_loader)
        return full_config_dict

    def get_config_loaders(
        self, config_dict: ConfigDict, overrides: ConfigDict
    ) -> Dict[str, ConfigLoader]:
        config_loaders: Dict[str, ConfigLoader] = {}
        # Accumulate toprepo.config.<id>.* keys.
        own_loader_config_dicts = config_dict.extract_mapping("toprepo.config")
        full_loader_config_dicts = ConfigDict.join(
            [config_dict, overrides]
        ).extract_mapping("toprepo.config")
        for name, own_loader_values in own_loader_config_dicts.items():
            # Check if values are just for overriding or the actual configuration.
            partial_value = own_loader_values.get("partial", "0")
            is_partial = {
                "1": True,
                "true": True,
                "0": False,
                "false": False,
            }[partial_value.lower()]
            if is_partial:
                continue
            # Actual configuration, load.
            full_loader_values = full_loader_config_dicts[name]
            config_loaders[name] = self.get_config_loader(name, full_loader_values)
        return config_loaders

    def get_config_loader(self, name: str, config_dict: ConfigDict) -> ConfigLoader:
        loader_type = config_dict["type"][-1]
        if loader_type == "none":
            config_loader = StaticContentConfigLoader("")
        elif loader_type == "file":
            file_path = config_dict["path"][-1]
            config_loader = LocalFileConfigLoader(self.monorepo.path / file_path)
        elif loader_type == "git":
            # Load.
            raw_url = config_dict["url"][-1]
            ref = config_dict["ref"][-1]
            filename = config_dict["path"][-1]
            # Translate.
            parent_url = self.monorepo.get_toprepo_fetch_url()
            url = join_submodule_url(parent_url, raw_url)
            filename_path = PurePosixPath(filename)
            # Create.
            config_loader = GitRemoteConfigLoader(
                url=url,
                remote_ref=ref,
                filename=filename_path,
                local_repo=self.monorepo,
                local_ref=f"refs/toprepo/config/{name}",
            )
        else:
            raise ValueError(f"Invalid toprepo.config.type {loader_type!r}")
        return config_loader


def get_gitmodules_info(
    config_loader: ConfigLoader, parent_url: Url
) -> List[GitModuleInfo]:
    """Parses the output from 'git config --list --file .gitmodules'."""
    submod_config_mapping = config_loader.get_config_dict().extract_mapping("submodule")

    configs: Dict[PurePosixPath, GitModuleInfo] = {}
    for name, config_dict in submod_config_mapping.items():
        raw_url: RawUrl = config_dict.get_singleton("url")
        resolved_url = join_submodule_url(parent_url, raw_url)
        submod_info = GitModuleInfo(
            name=name,
            path=PurePosixPath(config_dict.get_singleton("path")),
            branch=config_dict.get_singleton("branch", None),
            url=resolved_url,
            raw_url=raw_url,
        )
        if submod_info.path in configs:
            raise ValueError("Duplicated submodule configs for {submod_info.path}")
        configs[submod_info.path] = submod_info

    return list(configs.values())


@dataclass(frozen=True)
class Config:
    missing_commits: IgnoredCommits
    """Ignored because they are missing.

    A warning will be issued if the commit suddenly turns up.
    """

    top_fetch_url: Url
    top_push_url: Url

    repos: List[RepoConfig]

    @cached_property
    def raw_url_to_repos(self) -> Dict[RawUrl, List[RepoConfig]]:
        # Map URL to RepoConfig.
        raw_url_to_repos: DefaultDict[RawUrl, List[RepoConfig]] = defaultdict(list)
        for repo_config in self.repos:
            for raw_url in repo_config.raw_urls:
                raw_url_to_repos[raw_url].append(repo_config)
        return raw_url_to_repos

    @staticmethod
    def try_create(config_dict: ConfigDict) -> Optional["Config"]:
        try:
            return Config.create(config_dict)
        except ConfigParsingError as err:
            print(f"ERROR: Could not parse toprepo config: {err}")
            return None

    @staticmethod
    def create(config_dict: ConfigDict) -> "Config":
        # Accumulate toprepo.repo.<id>.* keys.
        repo_config_dicts: DefaultDict[RepoName, DefaultDict[str, List[str]]] = (
            defaultdict(lambda: defaultdict(list))
        )
        for key, values in config_dict.items():
            repo_config_prefix = "toprepo.repo."
            if key.startswith(repo_config_prefix) and key.count(".") == 3:
                _, _, repo_id, subkey = key.split(".", 3)
                repo_config_dicts[repo_id][subkey].extend(values)

        # Resolve the role.
        config_dict.setdefault("toprepo.role.default.repos", ["+.*"])
        role = config_dict.get("toprepo.role", ["default"])[-1]
        wanted_repos_patterns = config_dict.setdefault(f"toprepo.role.{role}.repos", [])
        top_fetch_url = config_dict.get("remote.origin.url", [None])[-1]
        if top_fetch_url is None or top_fetch_url == "file:///dev/null":
            # TODO: 2024-04-29 Remove after migration.
            top_fetch_url = config_dict.get("toprepo.top.fetchurl", [None])[-1]
            if top_fetch_url is None:
                raise ConfigParsingError("Config remote.origin.url is not set")
        top_push_url = config_dict.get("remote.top.pushurl", [None])[-1]
        if top_push_url is None:
            # TODO: 2024-04-29 Remove after migration.
            top_push_url = config_dict.get("toprepo.top.pushurl", [None])[-1]
            if top_push_url is None:
                raise ConfigParsingError("Config remote.top.pushUrl is not set")
        repo_configs = Config.parse_repo_configs(
            repo_config_dicts,
            wanted_repos_patterns,
            top_fetch_url,
            top_push_url,
        )

        # Find configured missing commits.
        missing_commits: DefaultDict[RawUrl, Set[CommitHash]] = defaultdict(set)
        missing_commits_prefix = "toprepo.missing-commits.rev-"
        for key, values in config_dict.items():
            if key.startswith(missing_commits_prefix):
                commit_hash = key[len(missing_commits_prefix) :].encode("utf-8")
                for raw_url in values:
                    missing_commits[raw_url].add(commit_hash)

        return Config(
            missing_commits=missing_commits,
            top_fetch_url=top_fetch_url,
            top_push_url=top_push_url,
            repos=repo_configs,
        )

    @staticmethod
    def parse_repo_configs(
        repo_config_dicts: Dict[RepoName, ConfigDict],
        wanted_repos_patterns: List[str],
        parent_fetch_url: str,
        parent_push_url: str,
    ) -> List[RepoConfig]:
        repo_configs: List[RepoConfig] = []
        for repo_name, repo_config_dict in repo_config_dicts.items():
            repo_configs.append(
                Config.parse_repo_config(
                    repo_name,
                    repo_config_dict,
                    wanted_repos_patterns,
                    parent_fetch_url,
                    parent_push_url,
                )
            )
        return repo_configs

    @staticmethod
    def parse_repo_config(
        name: RepoName,
        repo_config_dict: ConfigDict,
        wanted_repos_patterns: List[str],
        parent_fetch_url: Url,
        parent_push_url: Url,
    ) -> RepoConfig:
        if name == TopRepo.name:
            raise ConfigParsingError(f"Invalid repo name {name}")
        if len(PurePosixPath(name).parts) != 1:
            raise ConfigParsingError(f"Subdirectories not allowed in repo name: {name}")
        wanted_flag = Config.repo_is_wanted(name, wanted_repos_patterns)
        if wanted_flag is None:
            raise ConfigParsingError(
                f"Could not determine if repo {name} is wanted or not"
            )
        raw_urls = repo_config_dict.get("urls")
        if raw_urls is None:
            raise ConfigParsingError(f"toprepo.repo.{name}.urls is unspecified")
        raw_fetch_url = repo_config_dict.get("fetchurl", [None])[-1]
        if raw_fetch_url is None:
            raw_urls_set = set(raw_urls)
            if len(raw_urls_set) != 1:
                raise ConfigParsingError(
                    f"Missing toprepo.repo.{name}.fetchUrl and multiple "
                    + f"toprepo.repo.{name}.urls gives an ambiguous defult"
                )
            raw_fetch_url = raw_urls_set.pop()
        fetch_url = join_submodule_url(parent_fetch_url, raw_fetch_url)
        raw_push_url = repo_config_dict.get("pushurl", [raw_fetch_url])[-1]
        push_url = join_submodule_url(parent_push_url, raw_push_url)
        fetch_args = repo_config_dict.get("fetchargs", default_fetch_args)
        return RepoConfig(
            name=name,
            enabled=wanted_flag,
            raw_urls=raw_urls,
            fetch_url=fetch_url,
            fetch_args=fetch_args,
            push_url=push_url,
        )

    @staticmethod
    def repo_is_wanted(
        name: RepoName, wanted_repos_patterns: List[str]
    ) -> Optional[bool]:
        wanted = None
        for pattern in wanted_repos_patterns:
            if pattern[0] not in "+-":
                raise ConfigParsingError(
                    f"Invalid wanted repo config {pattern} for {name}, "
                    + "should start with '+' or '-' followed by a regex."
                )
            try:
                if re.fullmatch(pattern[1:], name) is not None:
                    wanted = pattern[0] == "+"
            except RuntimeError as err:
                raise ConfigParsingError(
                    f"Invalid wanted repo regex {pattern[1:]} " + f"for {name}: {err}"
                )
        return wanted


def remote_to_repo(
    remote: str, git_modules: List[GitModuleInfo], config: Config
) -> Tuple[Optional[RepoName], Optional[GitModuleInfo]]:
    """Map a remote or URL to a repository.

    A repo can be speicifed by subrepo path inside the toprepo or
    as an full or partial URL.
    """
    # Map a full or partial URL or path to one or more repos.
    remote_to_name: DefaultDict[str, Set[Tuple[RepoName, Optional[GitModuleInfo]]]] = (
        defaultdict(set)
    )

    def add_url(url: str, name: RepoName, gitmod: GitModuleInfo):
        entry = (name, gitmod)
        remote_to_name[url].add(entry)
        # Also match partial URLs.
        # Example: ssh://user@github.com:22/foo/bar.git
        url = removesuffix(url, ".git")
        remote_to_name[url].add(entry)
        if "://" in url:
            _, url = url.split("://", 1)
            remote_to_name[url].add(entry)
        if "@" in url:
            _, url = url.split("@", 1)
            remote_to_name[url].add(entry)
        if "/" in url and not url.startswith("."):
            _, url = url.split("/", 1)
            remote_to_name[url].add(entry)

    remote_to_name["origin"].add((TopRepo.name, None))
    remote_to_name["."].add((TopRepo.name, None))
    remote_to_name[""].add((TopRepo.name, None))
    add_url(config.top_fetch_url, TopRepo.name, None)
    add_url(config.top_push_url, TopRepo.name, None)

    for mod in git_modules:
        mod_repos = config.raw_url_to_repos.get(mod.raw_url, [])
        for cfg in mod_repos:
            # Add URLs from .gitmodules.
            add_url(mod.url, cfg.name, mod)
            add_url(mod.raw_url, cfg.name, mod)
            remote_to_name[mod.name].add((cfg.name, mod))
            remote_to_name[str(mod.path)].add((cfg.name, mod))
            # Add URLs from the toprepo config.
            add_url(cfg.fetch_url, cfg.name, mod)
            add_url(cfg.push_url, cfg.name, mod)
            for raw_url in cfg.raw_urls:
                add_url(raw_url, cfg.name, mod)

    # Now, try to find our repo.
    full_remote = remote
    remote = removesuffix(remote, "/")
    remote = removesuffix(remote, ".git")
    entries = remote_to_name.get(remote)
    if entries is None and "://" in remote:
        _, remote = remote.split("://", 1)
        entries = remote_to_name.get(remote)
    if entries is None and "@" in remote:
        _, remote = remote.split("@", 1)
        entries = remote_to_name.get(remote)
    if entries is None and "/" in remote and not remote.startswith("."):
        _, remote = remote.split("/", 1)
        entries = remote_to_name.get(remote)
    if entries is None:
        print(f"ERROR: Could not resolve {full_remote}")
        print("Is .gitmodules missing?")
        return None, None
    if len(entries) > 1:
        names_str = ", ".join(sorted(name for name, _ in entries))
        print(f"ERROR: Multiple remote candidates: {names_str}")
        return None, None
    ((name, gitmod),) = list(entries)
    return (name, gitmod)


def clone_commit(commit: git_filter_repo.Commit) -> git_filter_repo.Commit:
    return git_filter_repo.Commit(
        commit.branch,
        commit.author_name,
        commit.author_email,
        commit.author_date,
        commit.committer_name,
        commit.committer_email,
        commit.committer_date,
        commit.message,
        commit.file_changes,
        commit.parents,
    )


def clone_file_change(
    file_change: git_filter_repo.FileChange,
) -> git_filter_repo.FileChange:
    # FileChange.__init__() does some processing, avoid that.
    ret = git_filter_repo.FileChange(b"M", b"n/a", -1, b"100644")
    ret.type = file_change.type
    ret.filename = file_change.filename
    ret.mode = file_change.mode
    ret.blob_id = file_change.blob_id
    return ret


class DevNullWriter:
    def write(self, bytes):
        pass


class DevNullOutputRepoFilter:
    """Defines an output pipe for RepoFilter which discards all data."""

    def __init__(self):
        self._output = DevNullWriter()
        self._import_pipes = None


class SubRepo(Repo):
    is_top = False

    def __init__(self, config: RepoConfig, repo: Path):
        super().__init__(repo=repo)
        self.config = config

    @property
    def name(self):
        name = self.config.name
        assert name != TopRepo.name, f"Bad name {name}"
        return name


class CommitMap:
    def __init__(self):
        self.id_to_commit: Dict[int, git_filter_repo.Commit] = {}
        """Maps a unique git-filter-repo id to a commit."""

        self.hash_to_commit: Dict[CommitHash, git_filter_repo.Commit] = {}
        """Maps from a commit hash to a commit."""

    @staticmethod
    def join(commit_maps: Iterable["CommitMap"]) -> "CommitMap":
        ret = CommitMap()
        for commit_map in commit_maps:
            ret.id_to_commit.update(commit_map.id_to_commit)
            ret.hash_to_commit.update(commit_map.hash_to_commit)
        return ret

    @staticmethod
    def collect_tree_hashes(repo: Repo) -> Dict[CommitHash, TreeHash]:
        """Get all commit hashes and map to tree hashes in a repo."""
        log_stdout = subprocess.check_output(
            ["git", "-C", str(repo.path)] + ["log", "--format=%H %T", "--all", "--"]
        )
        commit_to_tree: Dict[CommitHash, TreeHash] = {}
        for line in log_stdout.splitlines(keepends=False):
            commit_hash, tree_hash = line.split(b" ", 1)
            commit_to_tree[commit_hash] = tree_hash
        return commit_to_tree

    @staticmethod
    def collect_commits(repo: Repo, refs: List[RefStr]) -> "CommitMap":
        """Loads metadata about all commits."""
        print(f"Collecting metadata for {repo.name}...")
        ret = CommitMap()
        commit_to_tree = ret.collect_tree_hashes(repo)

        args = git_filter_repo.FilteringOptions.parse_args(
            ["--partial", "--refs", "dummy"]
            + ["--source", str(repo.path)]
            # --target must be the same as --source but is overridden later.
            + ["--target", str(repo.path)]
        )
        args.refs = refs
        repo_filter = git_filter_repo.RepoFilter(
            args,
            commit_callback=partial(ret._collect_commit_callback, commit_to_tree),
        )
        repo_filter.set_output(DevNullOutputRepoFilter())
        repo_filter.run()
        return ret

    def _collect_commit_callback(self, commit_to_tree, commit, metadata):
        commit.depth = 1 + max(
            (
                self.id_to_commit[parent_id].depth
                for parent_id in commit.parents
                if isinstance(parent_id, int)
            ),
            default=0,
        )
        self.id_to_commit[commit.id] = commit
        self.hash_to_commit[commit.original_id] = commit
        commit.tree_hash = commit_to_tree[commit.original_id]


@dataclass(frozen=True)
class BumpInfo:
    subrepo_commit: git_filter_repo.Commit
    """The original commit from the sub repo."""

    first_mono_commit: git_filter_repo.Commit
    """The commit in the mono repo that updates the sub directory content,
    i.e. the translated top repo commit which did contain a bump of
    the sub repo compared to any parent.

    This variable is used to fast track searches through the first-parent
    chain.
    """


class SubmoduleFilterHelper:
    def __init__(self, source_repo: Repo, parent_url: Url):
        self.current_commit = None
        self.commit_id_to_last_config_change: Dict[RepoFilterId, CommitHash] = {}

        self.repo = source_repo
        self.parent_url = parent_url

    def commit_callback(self, commit: git_filter_repo.Commit) -> None:
        self.current_commit = commit

        first_parent = commit.first_parent()
        if first_parent is None or isinstance(first_parent, bytes):
            self.commit_id_to_last_config_change[commit.id] = commit.original_id
        else:
            assert isinstance(
                first_parent, int
            ), f"Expected int or commit hash as parent of {commit.id}, not {first_parent}"
            self.commit_id_to_last_config_change[commit.id] = (
                self.commit_id_to_last_config_change[first_parent]
            )

        for file_change in commit.file_changes:
            if file_change.filename == b".gitmodules":
                # .gitmodules has changed, reload.
                self.commit_id_to_last_config_change[commit.id] = commit.original_id
                break

    @property
    def submodule_configs(self) -> Dict[bytes, GitModuleInfo]:
        commit_hash = self.commit_id_to_last_config_change[self.current_commit.id]
        return self._load_submodule_configs(commit_hash)

    @lru_cache()
    def _load_submodule_configs(
        self, commit_hash: CommitHash
    ) -> Dict[bytes, GitModuleInfo]:
        gitmodules = get_gitmodules_info(
            GitRemoteConfigLoader(
                url="",
                remote_ref="",
                filename=PurePosixPath(".gitmodules"),
                local_repo=self.repo,
                local_ref=commit_hash.decode("utf-8"),
            ),
            self.parent_url,
        )
        return {config.path.as_posix().encode("utf-8"): config for config in gitmodules}

    def get_submodules(
        self, commit: git_filter_repo.Commit
    ) -> List[Tuple[git_filter_repo.FileChange, Optional[GitModuleInfo]]]:
        ret: List[Tuple[git_filter_repo.FileChange, Optional[GitModuleInfo]]] = []
        for file_change in commit.file_changes:
            submodule_mode = b"160000"
            if file_change.mode == submodule_mode:
                if file_change.type == b"D":
                    ret.append((file_change, None))
                else:
                    submod_config = self.submodule_configs.get(file_change.filename)
                    if submod_config is not None:
                        ret.append((file_change, submod_config))
                    else:
                        print(
                            "\rWARNING: Invalid .gitmodules for "
                            + file_change.filename.decode("utf-8")
                            + " at commit "
                            + commit.original_id.decode("utf-8")
                        )
        return ret


class ReferencedSubmodCommitsCollector:
    def __init__(self, repo: Repo):
        self.referenced_commits: DefaultDict[RawUrl, Set[CommitHash]] = defaultdict(set)
        """Mapping from submodule URL to commit hashes."""

        self.submodule_filter_helper = SubmoduleFilterHelper(
            repo, repo.config.fetch_url
        )

    def _commit_callback(self, commit: git_filter_repo.Commit, metadata):
        self.submodule_filter_helper.commit_callback(commit)
        submods = self.submodule_filter_helper.get_submodules(commit)
        for file_change, submodule_config in submods:
            if submodule_config is not None:
                raw_url = submodule_config.raw_url
                self.referenced_commits[raw_url].add(file_change.blob_id)

    @staticmethod
    def collect(repo: Repo) -> Dict[str, Set[CommitHash]]:
        """Iterates through a repository and collects submodule commits.

        Returns:
            A mapping from submodule URL to commit hashes.
        """
        collector = ReferencedSubmodCommitsCollector(repo)

        args = git_filter_repo.FilteringOptions.parse_args(
            ["--partial", "--refs", "dummy"]
            + ["--source", str(repo.path)]
            # --target must be the same as --source but is overridden later.
            + ["--target", str(repo.path)]
        )
        args.refs = ["--all"]
        repo_filter = git_filter_repo.RepoFilter(
            args,
            commit_callback=collector._commit_callback,
        )
        repo_filter.set_output(DevNullOutputRepoFilter())
        repo_filter.run()

        return collector.referenced_commits


class RepoFetcher:
    def __init__(self, monorepo: MonoRepo):
        self.monorepo = monorepo

    def init_subrepo(self, repo: Repo):
        if not repo.path.exists():
            repo.path.mkdir(parents=True)
            log_run_git(
                repo.path,
                ["init", "--quiet", "--bare"],
            )
            subprocess.check_call(
                ["git", "-C", str(repo.path)]
                + ["config", "remote.origin.fetch", "+refs/heads/*:refs/heads/*"],
            )

    def fetch_repo(self, repo: Repo, ref_args: Optional[List[str]] = None):
        """Make all the repo content available in the monorepo.

        All the blobs and trees need to be accessible within the monorepo.
        This filtering will copy all the data over."""
        self.init_subrepo(repo)
        # First fetch into the individual repository.
        if ref_args is None:
            ref_args = ["+refs/heads/*:refs/heads/*"]
        # TODO: What about relative paths if fetch_url is from the disk?
        log_run_git(
            repo.path,
            ["fetch"] + repo.config.fetch_args + [repo.config.fetch_url] + ref_args,
        )
        # For convenience, log where we fetched from.
        subprocess.check_call(
            ["git", "-C", str(repo.path)]
            + ["config", "remote.origin.url", repo.config.fetch_url]
        )
        subprocess.check_call(
            ["git", "-C", str(repo.path)]
            + ["config", "remote.origin.pushurl", repo.config.push_url]
        )
        # Then move the blobs over to the monorepo.
        # The toprepo itself can be moved by git-filter-repo,
        # but moving the content anyway because 'git-toprepo push' requires
        # the original commits.
        log_run_git(
            self.monorepo.path,
            ["fetch", "--quiet", "--no-tags", "--prune", str(repo.path.absolute())]
            + [f"+refs/*:refs/repos/{repo.name}/*"],
        )


class RepoExpanderBase:
    def __init__(self, monorepo: MonoRepo):
        self.monorepo: MonoRepo = monorepo

    @staticmethod
    def _create_mono_commit_from_subrepo_commit(
        fullref: bytes,
        subdir: bytes,
        subrepo_commit: git_filter_repo.Commit,
        subrepo_id_to_converted_id: Dict[RepoFilterId, RepoFilterId],
    ) -> git_filter_repo.Commit:
        new_commit = clone_commit(subrepo_commit)
        new_commit.branch = fullref
        new_commit.message = annotate_message(
            subrepo_commit.message, subdir, subrepo_commit.original_id
        )
        new_commit.parents = [
            subrepo_id_to_converted_id[pid] for pid in subrepo_commit.parents
        ]
        new_commit.file_changes = list(
            map(clone_file_change, subrepo_commit.file_changes)
        )
        for new_fc in new_commit.file_changes:
            new_fc.filename = subdir + b"/" + new_fc.filename
        return new_commit


class TopRepoExpander(RepoExpanderBase):
    def __init__(self, monorepo: MonoRepo, toprepo: TopRepo, config: Config):
        super().__init__(monorepo=monorepo)
        self.toprepo = toprepo
        self.fetcher = RepoFetcher(self.monorepo)
        self.config = config

        self.commit_map: Optional[CommitMap]
        self.submodule_filter_helper = SubmoduleFilterHelper(
            self.toprepo, config.top_fetch_url
        )

        self.mono_id_to_commit: Dict[int, git_filter_repo.Commit] = {}
        # TODO: Refactor to cache per commit instead of resetting on branch change.
        self.subrepo_id_to_converted_id: Dict[RepoFilterId, RepoFilterId]
        self.last_branch = b""

    def expand_toprepo(self, top_refs: List[RefStr], allow_fetching: bool):
        """Perform the monorepo expansion using git-filter-repo.

        Submodules will be fetched and filtered on demand.
        """
        old_toprepo_refs = set(get_remote_origin_refs(self.toprepo))
        print("Collecting referenced submodules...")
        submod_commits = ReferencedSubmodCommitsCollector.collect(self.toprepo)
        subrepos = self._get_subrepos_given_commits(submod_commits)
        for subrepo in subrepos.values():
            self.fetcher.init_subrepo(subrepo)
        commit_map = self.make_commits_available(
            sorted(subrepos.values(), key=lambda repo: repo.name),
            submod_commits,
            allow_fetching=allow_fetching,
        )
        if commit_map is None:
            return False
        self.commit_map = commit_map
        self.mono_id_to_commit = {}

        # TODO: Filter only 1000 commits per branch.
        print("Expanding the top repo to a mono repo...")
        self.subrepo_id_to_converted_id = {}
        args = git_filter_repo.FilteringOptions.parse_args(
            # NOTE: git-filter-repo fails without --force, "expected freshly packed repo".
            ["--partial", "--refs", "dummy", "--force"]
            + ["--source", str(self.toprepo.path)]
            + ["--target", str(self.monorepo.path)]
        )
        args.refs = top_refs
        repo_filter = None
        repo_filter = git_filter_repo.RepoFilter(
            args,
            refname_callback=self._expand_toprepo_refname_callback,
            reset_callback=self._expand_toprepo_reset_callback,
            commit_callback=lambda *args: self._expand_toprepo_commit_callback(
                repo_filter, *args
            ),
        )
        repo_filter.run()

        remote_monorepo_refs = set(get_remote_origin_refs(self.monorepo))
        refs_to_remove = old_toprepo_refs - remote_monorepo_refs
        delete_refs(self.monorepo, refs_to_remove)
        return True

    def _get_subrepos_given_commits(
        self, submod_commits: Dict[str, Set[CommitHash]]
    ) -> Dict[str, SubRepo]:
        subrepos: Dict[str, SubRepo] = {}
        unknown_urls: List[Url] = []
        for url in submod_commits.keys():
            subrepo_configs = self.config.raw_url_to_repos.get(url)
            if subrepo_configs is None:
                unknown_urls.append(url)
                continue
            for subrepo_config in subrepo_configs:
                if not subrepo_config.enabled or subrepo_config.name in subrepos:
                    continue
                subrepo = SubRepo(
                    subrepo_config,
                    self.monorepo.get_subrepo_dir(subrepo_config.name),
                )
                subrepos[subrepo_config.name] = subrepo
        # Print unknown urls.
        if len(unknown_urls) != 0:
            # Group by name.
            name_to_urls: DefaultDict[str, List[Url]] = defaultdict(list)
            for url in unknown_urls:
                name_to_urls[repository_name(url)].append(url)
            print("WARNING: Some subrepo URLs are missing in the git-toprepo config")
            for name, urls in sorted(name_to_urls.items()):
                print(f'[toprepo.repo "{name}"]')
                for url in sorted(urls):
                    print(f"\turls = {url}")
                if len(urls) > 1:
                    print("\tfetchUrl = <fill-in>")
            print(
                "INFO: Printed above is an example git-toprepo config "
                + "to fix the warnings."
            )
        return subrepos

    def make_commits_available(
        self,
        subrepos: List[SubRepo],
        submod_commits: Dict[str, Set[CommitHash]],
        allow_fetching: bool,
    ) -> Optional[CommitMap]:
        """Check that all wanted commits exists.

        If commits are missing, run git-fetch in all the sub repos
        associated with that URL.

        Args:
            submod_commits: A map from a raw URL to needed commit hashes.
        """
        fetched_repos: Set[str] = set()  # subrepo.config.name
        commit_maps = {
            subrepo.config.name: CommitMap.collect_commits(subrepo, ["--all"])
            for subrepo in subrepos
        }
        subrepo_map = {subrepo.config.name: subrepo for subrepo in subrepos}
        missing_commits: List[Tuple[RawUrl, CommitHash]] = []

        for url, referenced_commits in submod_commits.items():
            subrepos = [
                subrepo_map[subrepo_config.name]
                for subrepo_config in self.config.raw_url_to_repos.get(url, [])
                if subrepo_config.enabled
            ]
            if len(subrepos) == 0:
                continue

            def get_commits_to_fetch() -> Set[CommitHash]:
                """Finds what commits need to be fetched from upstream."""
                ret: Set[CommitHash] = (
                    referenced_commits - self.config.missing_commits.get(url, set())
                )
                for subrepo in subrepos:
                    ret.difference_update(
                        commit_maps[subrepo.config.name].hash_to_commit.keys()
                    )
                return ret

            commits_to_fetch = get_commits_to_fetch()
            # Fetch.
            if len(commits_to_fetch) != 0 and allow_fetching:
                for subrepo in subrepos:
                    if subrepo.config.name not in fetched_repos:
                        fetched_repos.add(subrepo.config.name)
                        self.fetcher.fetch_repo(subrepo)
                        commit_maps[subrepo.config.name] = CommitMap.collect_commits(
                            subrepo,
                            ["--all"],
                        )
                # Recalculate.
                commits_to_fetch = get_commits_to_fetch()
            # Check.
            for commit_hash in sorted(commits_to_fetch):
                missing_commits.append((url, commit_hash))

        if len(missing_commits) != 0:
            print("ERROR: Some referenced commits could not be found")
            print("[toprepo.missing-commits]")
            max_missing_commits_to_print = 100
            for url, commit_hash in sorted(missing_commits)[
                :max_missing_commits_to_print
            ]:
                commit_hash_str = commit_hash.decode("utf-8")
                print(f"\trev-{commit_hash_str} = {url}")
            if len(missing_commits) > max_missing_commits_to_print:
                print(f"Totally {len(missing_commits)} unexpected missing commits.")
            print("ERROR: The referenced commits above could not be found")
            print(
                "Either push the following commits or add them to the "
                + "toprepo configuration."
            )
            return None

        # Don't clutter the error message above with warnings
        # about overspecified missing-commits.
        overspecified_missing_commits = False
        for url, referenced_commits in submod_commits.items():
            unexpected_commits = (
                self.config.missing_commits.get(url, set()) - referenced_commits
            )
            for commit_hash in sorted(unexpected_commits):
                if not overspecified_missing_commits:
                    overspecified_missing_commits = True
                    print(
                        "WARNING: The following configured "
                        + "missing-commits actually exists"
                    )
                    print(
                        "Please remove them from the missing-commits "
                        + "toprepo configuration."
                    )
                print("rev-" + commit_hash.decode("utf-8") + " = " + url)

        return CommitMap.join(commit_maps.values())

    def _expand_toprepo_refname_callback(self, ref: bytes):
        """Move toprepo refs into refs/remotes/origin/...

        This does not apply to tags and non-heads.
        """
        if ref.startswith(b"refs/heads/"):
            new_ref = b"refs/remotes/origin/" + ref[11:]
        elif ref.startswith(b"refs/tags/"):
            new_ref = ref
        elif ref == b"refs/toprepo/fetch-head":
            # Special handling.
            new_ref = ref
        else:
            assert ref.startswith(b"refs/"), ref
            new_ref = b"refs/repos/" + TopRepo.name_bytes + ref[4:]
        return new_ref

    def _expand_toprepo_reset_callback(self, reset: git_filter_repo.Reset, metadata):
        # This branch might have bumped the subrepos in a totally different cadence.
        # Cannot keep the conversion cache.
        self.subrepo_id_to_converted_id = {}
        self.last_branch = b""

    def _expand_toprepo_commit_callback(
        self,
        repo_filter: git_filter_repo.RepoFilter,
        mono_commit: git_filter_repo.Commit,
        metadata,
    ):
        if self.last_branch != mono_commit.branch:
            # This branch might have bumped the subrepos in a totally different cadence.
            # Cannot keep the conversion cache.
            self.subrepo_id_to_converted_id = {}
            self.last_branch = mono_commit.branch

        # The refname callback should already have been called.
        assert not mono_commit.branch.startswith(b"refs/heads/"), mono_commit.branch
        self.submodule_filter_helper.commit_callback(mono_commit)

        self.mono_id_to_commit[mono_commit.id] = mono_commit
        first_parent_id = mono_commit.first_parent()
        if first_parent_id is not None:
            first_parent = self.mono_id_to_commit[first_parent_id]
            mono_commit.bumps = dict(first_parent.bumps)  # Copy
        else:
            mono_commit.bumps = {}  # Dict[bytes, BumpInfo]

        commit_message_parts = [
            annotate_message(
                mono_commit.message, ANNOTATED_TOP_SUBDIR, mono_commit.original_id
            )
        ]

        submods = self.submodule_filter_helper.get_submodules(mono_commit)
        for file_change, gitmodule_config in submods:
            _ = gitmodule_config
            if file_change.type == b"M":
                commit_message_parts += self._expand_submod_in_commit_callback(
                    repo_filter,
                    mono_commit,
                    file_change,
                )
            elif file_change.type == b"D":
                mono_commit.bumps.pop(file_change.filename)
            elif file_change.type == b"R":
                raise NotImplementedError("Submodule renames are not implements")
            else:
                assert False, f"Unknown file change type {file_change.type}"

        mono_commit.message = join_annotated_commit_messages(commit_message_parts)

    def _expand_submod_in_commit_callback(
        self,
        repo_filter: git_filter_repo.RepoFilter,
        mono_commit: git_filter_repo.Commit,
        file_change: git_filter_repo.FileChange,
    ) -> List[bytes]:
        """Injects the submodule commit history up to the commit referenced by file_change.

        Returns:
            A list of annotated commit messages to attach to mono_commit.
        """
        commit_message_parts = []
        submod_hash: CommitHash = file_change.blob_id
        submod_commit = self.commit_map.hash_to_commit.get(submod_hash)
        if submod_commit is not None:
            # Swap commit to tree.
            tree_mode = b"040000"
            file_change.mode = tree_mode
            file_change.blob_id = submod_commit.tree_hash
            commit_message_parts.append(
                annotate_message(
                    submod_commit.message, file_change.filename, submod_hash
                )
            )
            # Recreate the history of the submodule commit graph.
            new_mono_parent_ids = self._inject_subrepo(
                repo_filter, mono_commit, file_change.filename, submod_commit
            )
            for pid in new_mono_parent_ids:
                if pid not in mono_commit.parents:
                    mono_commit.parents.append(pid)
            # Register the bump in this commit.
            mono_commit.bumps[file_change.filename] = BumpInfo(
                subrepo_commit=submod_commit,
                first_mono_commit=mono_commit,
            )
        else:
            # Missing commit, leave as a submodule reference.
            mono_commit.bumps.pop(file_change.filename, None)

        return commit_message_parts

    def _inject_subrepo(
        self,
        repo_filter: git_filter_repo.RepoFilter,
        target_mono_commit: git_filter_repo.Commit,
        subdir: bytes,
        subrepo_commit_to_insert: git_filter_repo.Commit,
    ) -> List[int]:
        """Injects the history of subrepo_commit_to_insert into the monorepo.

        subrepo_commit_to_insert refers to a commit in a sub repo that a top repo
        commit refers to. That content is merged into the converted top commit.
        When a sub repo is bumped, there might be a long history in the
        sub repo that also needs to be merged. All those commits are resolved
        and inserted here.
        """
        counter = itertools.count(start=0, step=1)

        def bump_generator(max_target_subrepo_depth: int):
            mono_queue_ids: Set[int] = set()
            mono_queue = PriorityQueue()

            def add_possible_parent(mono_parent, max_subrepo_depth: int):
                bump: BumpInfo = mono_parent.bumps.get(subdir)
                if bump is None:
                    # mono_parent doesn't contain subdir.
                    return
                if bump.subrepo_commit.depth > max_subrepo_depth:
                    # The subrepo pointer has reversed in the history.
                    # Stop tracing that branch, merge from the subrepo
                    # source if needed instead.
                    return
                # Prioritize by subrepo depth, not monorepo depth.
                # Otherwise, we don't know when we have looked far
                # enough as the depths are not correlated.
                if bump.subrepo_commit.id not in mono_queue_ids:
                    mono_queue_ids.add(bump.subrepo_commit.id)
                    mono_queue.put(
                        (
                            -bump.subrepo_commit.depth,
                            next(counter),
                            mono_parent,
                        )
                    )

            for pid in target_mono_commit.parents:
                add_possible_parent(
                    self.mono_id_to_commit[pid], max_target_subrepo_depth
                )

            while not mono_queue.empty():
                _, _, mono_commit = mono_queue.get()
                # Return the latest commit pointing to a subrepo commit.
                # This minimizes the length of feature branches before
                # they are merged back in the history.
                yield mono_commit
                # Dig deeper, look where the current subrepo_commit was bumped.
                bump: BumpInfo = mono_commit.bumps[subdir]
                for pid in bump.first_mono_commit.parents:
                    add_possible_parent(
                        self.mono_id_to_commit[pid], bump.subrepo_commit.depth - 1
                    )

        bump_iterator = bump_generator(subrepo_commit_to_insert.depth - 1)

        commits_to_convert: List[git_filter_repo.Commit] = []

        sub_queue_ids: Set[int] = set()
        sub_queue = PriorityQueue()
        sub_queue.put(
            (-subrepo_commit_to_insert.depth, next(counter), subrepo_commit_to_insert)
        )
        # Get the loop going, initialize with something that
        # gets us into the inner loop.
        bump = BumpInfo(subrepo_commit=subrepo_commit_to_insert, first_mono_commit=None)
        while not sub_queue.empty():
            _, _, subrepo_commit = sub_queue.get()

            while bump.subrepo_commit.depth >= subrepo_commit.depth:
                # Some paths in the monorepo history has found a subrepo
                # commit that is close to a root than subrepo_commit.
                # Don't search further now, because the assumption is that
                # getting closer to a monorepo root should mean closer
                # to the subrepo root.
                try:
                    latest_bump_mono_commit = next(bump_iterator)
                except StopIteration:
                    break
                bump = latest_bump_mono_commit.bumps[subdir]
                # Do not override the bump_commit that was found previously,
                # as this loop is going back in the history and
                # the map should point to (one of the) newest commits.
                # There might be multiple valid solutions,
                # so just use the first one found.
                self.subrepo_id_to_converted_id.setdefault(
                    bump.subrepo_commit.id, latest_bump_mono_commit.id
                )

            if subrepo_commit.id not in self.subrepo_id_to_converted_id:
                # No good already sub->mono converted candidate was found
                # in the monorepo.
                commits_to_convert.append(subrepo_commit)
                for pid in subrepo_commit.parents:
                    if pid not in sub_queue_ids:
                        sub_queue_ids.add(pid)
                        subrepo_parent = self.commit_map.id_to_commit[pid]
                        sub_queue.put(
                            (-subrepo_parent.depth, next(counter), subrepo_parent)
                        )

        # Skip subrepo_commit_to_insert itself,
        # git-filter-repo is inserting it for us when returning.
        if len(commits_to_convert) != 0:
            assert commits_to_convert[0] == subrepo_commit_to_insert
        for subrepo_commit in reversed(commits_to_convert[1:]):
            new_commit = self._create_mono_commit_from_subrepo_commit(
                target_mono_commit.branch,
                subdir,
                subrepo_commit,
                self.subrepo_id_to_converted_id,
            )
            repo_filter.insert(new_commit, direct_insertion=True)
            self.mono_id_to_commit[new_commit.id] = new_commit
            self.subrepo_id_to_converted_id[subrepo_commit.id] = new_commit.id
            # Record subrepo trace info.
            first_parent_id = new_commit.first_parent()
            if first_parent_id is None:
                new_commit.bumps = {}
            else:
                first_parent = self.mono_id_to_commit[first_parent_id]
                new_commit.bumps = dict(first_parent.bumps)  # Copy
            new_commit.bumps[subdir] = BumpInfo(
                subrepo_commit=subrepo_commit,
                first_mono_commit=new_commit,
            )

        ret = [
            self.subrepo_id_to_converted_id[parent_id]
            for parent_id in subrepo_commit_to_insert.parents
        ]
        return ret


class SubrepoCommitExpander(RepoExpanderBase):
    def __init__(self, monorepo: MonoRepo):
        super().__init__(monorepo=monorepo)
        self.few_mono_commits = 1000
        self.few_subref_commits = 999

    def expand_subrepo_refs(
        self, subdir: bytes, subrepo_ref: RefStr, dest_ref: RefStr
    ) -> bool:
        """Inserts ref from subrepo onto the history of HEAD in the mono repo.

        The oldest possible place is preferred, without creating more commits
        than necessary.
        """
        subrepo_id_to_converted_id: Dict[RepoFilterId, RepoFilterId] = {}

        sub_commit_hash: CommitHash = subprocess.check_output(
            ["git", "-C", str(self.monorepo.path)]
            + ["rev-parse", "--verify", "--quiet", subrepo_ref + "^{commit}"],
        ).rstrip()
        commits_to_convert, converted_commit_hash = self._resolve_commits_to_convert(
            subdir, sub_commit_hash, subrepo_id_to_converted_id
        )
        if commits_to_convert is None:
            return False

        if commits_to_convert == []:
            assert converted_commit_hash is not None
            # Already part of the monorepo. Update dest_ref.
            log_run_git(
                self.monorepo.path,
                ["update-ref", dest_ref, converted_commit_hash.decode("utf8")],
                log_command=False,
            )
        else:
            assert converted_commit_hash is None, converted_commit_hash
            self._insert_commits(
                dest_ref, subdir, commits_to_convert, subrepo_id_to_converted_id
            )
        return True

    def _resolve_commits_to_convert(
        self,
        subdir: bytes,
        sub_commit_hash: CommitHash,
        subrepo_id_to_converted_id: Dict[RepoFilterId, RepoFilterId],
    ) -> Tuple[Optional[List[git_filter_repo.Commit]], Optional[CommitHash]]:
        subref: RefStr = sub_commit_hash.decode("utf-8")
        # First try with the latest few commits.
        commits_to_convert = None
        limiting_subref = f"{subref}~{self.few_subref_commits}"
        monoref = "HEAD"
        limiting_monoref = f"{monoref}~{self.few_mono_commits}"
        if ref_exists(self.monorepo, limiting_subref) and ref_exists(
            self.monorepo, limiting_monoref
        ):
            subrepo_commit_map = CommitMap.collect_commits(
                self.monorepo, [subref, f"^{limiting_subref}"]
            )
            sub_commit = subrepo_commit_map.hash_to_commit.get(sub_commit_hash)
            if sub_commit is not None:
                subdir_hash_to_mono_hash = self._map_subdir_hash_to_mono_hash(
                    [monoref, f"^{limiting_monoref}"], subdir
                )
                commits_to_convert = self._find_missing_subrepo_commits(
                    [sub_commit],
                    subrepo_commit_map,
                    subdir_hash_to_mono_hash,
                    subrepo_id_to_converted_id,
                )

        if commits_to_convert is None:
            # Retry with full history.
            print("Short history not enough, collecting full monorepo history.")
            subrepo_commit_map = CommitMap.collect_commits(self.monorepo, [subref])
            sub_commit = subrepo_commit_map.hash_to_commit.get(sub_commit_hash)
            subdir_hash_to_mono_hash = self._map_subdir_hash_to_mono_hash(
                [monoref], subdir
            )
            commits_to_convert = self._find_missing_subrepo_commits(
                [sub_commit],
                subrepo_commit_map,
                subdir_hash_to_mono_hash,
                subrepo_id_to_converted_id,
            )

        mono_commit_hash = subdir_hash_to_mono_hash.get(sub_commit_hash)

        if commits_to_convert is None:
            subdir_str = subdir.decode("utf-8")
            print(
                f"ERROR: Could not find where under {monoref} to insert "
                + f"{subref} from {subdir_str}"
            )
        return commits_to_convert, mono_commit_hash

    def _map_subdir_hash_to_mono_hash(
        self, mono_refs: List[RefStr], subdir: bytes
    ) -> Dict[CommitHash, CommitHash]:
        """Map a commit hash in a sub directory to a mono commit hash.

        The mono commit that changed the pointer for the subrepo will be used,
        i.e. the older commit the better. The reason is that `git rebase` for
        moving to a newer base is easier to understand than
        `git rebase --onto` for moving to an older base.

        If multiple commits in the top repo set the submodule pointer to
        the same subrepo commit, the newest of them will be choosen.
        """
        log_output = subprocess.check_output(
            ["git", "-C", str(self.monorepo.path), "log", "--format=%H%n%B%x00"]
            + mono_refs
            + ["--"]
        )
        subrepo_hash_to_mono_hash: Dict[CommitHash, CommitHash] = {}
        for entry in log_output.split(b"\0\n"):
            if entry == b"":
                continue
            mono_commit_hash, message = entry.split(b"\n", 1)
            subrepo_commit_hash = try_parse_commit_hash_from_message(message, subdir)
            if subrepo_commit_hash is not None:
                subrepo_hash_to_mono_hash.setdefault(
                    subrepo_commit_hash, mono_commit_hash
                )
        return subrepo_hash_to_mono_hash

    @staticmethod
    def _find_missing_subrepo_commits(
        subrepo_commits: List[git_filter_repo.Commit],
        commit_map: CommitMap,
        subdir_hash_to_mono_hash: Dict[CommitHash, CommitHash],
        subrepo_id_to_converted_id: Dict[RepoFilterId, RepoFilterId],
    ) -> Optional[List[git_filter_repo.Commit]]:
        """Find all subrepo commits to convert.

        This looks through the history of subrepo_commits and find all
        parents until a monorepo commit is found.

        Returns:
            A list of commits to convert, in the order to be inserted.
        """
        commits_to_convert: List[git_filter_repo.Commit] = []
        # Also sort on insertion order because Commit is not comparable.
        counter = itertools.count(start=0, step=1)
        todo_queue = PriorityQueue()
        all_todo_ids: Set[RepoFilterId] = set()
        for commit in subrepo_commits:
            todo_queue.put((-commit.depth, next(counter), commit))
            all_todo_ids.add(commit.id)

        while not todo_queue.empty():
            _, _, subrepo_commit = todo_queue.get()
            mono_hash = subdir_hash_to_mono_hash.get(subrepo_commit.original_id)
            if mono_hash is not None:
                # No need to convert, we found the base.
                subrepo_id_to_converted_id[subrepo_commit.id] = mono_hash
                continue
            # Loop through the parents as well.
            commits_to_convert.append(subrepo_commit)
            for pid in subrepo_commit.parents:
                if isinstance(pid, CommitHash):
                    # Not enough history.
                    return None
                if pid not in all_todo_ids:
                    parent_commit = commit_map.id_to_commit[pid]
                    todo_queue.put((-parent_commit.depth, next(counter), parent_commit))
                    all_todo_ids.add(pid)
        # Convert from the base first.
        commits_to_convert.reverse()
        return commits_to_convert

    def _insert_commits(
        self,
        monoref: RefStr,
        subdir: bytes,
        commits_to_convert: List[git_filter_repo.Commit],
        subrepo_id_to_converted_id: Dict[RepoFilterId, RepoFilterId],
    ):
        args = git_filter_repo.FilteringOptions.parse_args(
            ["--partial", "--refs", "dummy"]
            + ["--source", str(self.monorepo.path)]
            + ["--target", str(self.monorepo.path)]
        )
        args.refs = []
        repo_filter = git_filter_repo.RepoFilter(args)
        repo_filter.importer_only()

        monoref_bytes = monoref.encode("utf-8")
        for subrepo_commit in commits_to_convert:
            new_mono_commit = self._create_mono_commit_from_subrepo_commit(
                monoref_bytes,
                subdir,
                subrepo_commit,
                subrepo_id_to_converted_id,
            )
            repo_filter.insert(new_mono_commit, direct_insertion=True)
            subrepo_id_to_converted_id[subrepo_commit.id] = new_mono_commit.id

        repo_filter.finish()


ParentsList = List[RepoFilterId]
SubrepoParentsMap = Dict[bytes, ParentsList]
"""Maps from subrepo dir to subrepo parent ids.

subdir='' is used for the top repo.
"""


class PushSplitError(ValueError):
    pass


class PushSplitter:
    def __init__(self, monorepo: MonoRepo, toprepo: TopRepo, config: Config):
        self.monorepo = monorepo
        self.toprepo = toprepo
        self.config = config

        self.mono_id_to_subrepo_parent_ids: Dict[int, Dict[bytes, ParentsList]] = {}
        self.submodule_filter_helper = SubmoduleFilterHelper(
            self.monorepo, config.top_push_url
        )

        self.error = None

    def split_commits(self, local_ref: RefStr) -> List[PushInstruction]:
        # TODO: Support altering .gitmodules inside the push.
        mono_refs = get_remote_origin_refs(self.monorepo)
        # First split inside the monorepo.
        args = git_filter_repo.FilteringOptions.parse_args(
            ["--partial", "--refs", "dummy"]
            # Keep empty commits in the monorepo, push them to the top repo.
            + ["--prune-empty=never"]
            + ["--source", str(self.monorepo.path)]
            + ["--target", str(self.monorepo.path)],
        )
        args.refs = [local_ref] + [f"^{ref}" for ref in mono_refs]
        to_push: List[PushInstruction] = []
        repo_filter = None
        repo_filter = git_filter_repo.RepoFilter(
            args,
            commit_callback=lambda *args: self._commit_callback(
                repo_filter, to_push, *args
            ),
        )
        repo_filter.run()
        if self.error is not None:
            raise self.error
        return to_push

    def _commit_callback(
        self,
        repo_filter: git_filter_repo.RepoFilter,
        to_push: List[PushInstruction],
        mono_commit: git_filter_repo.Commit,
        metadata,
    ):
        if self.error is not None:
            # Stop fancy filtering, just pass through mono_commits
            # so they don't get removed from the repo.
            return

        try:
            self._commit_callback_impl(repo_filter, to_push, mono_commit, metadata)
        except Exception as err:
            if self.error is not None:
                raise
            self.error = err

    def _commit_callback_impl(
        self,
        repo_filter: git_filter_repo.RepoFilter,
        to_push: List[PushInstruction],
        mono_commit: git_filter_repo.Commit,
        metadata,
    ):
        # Let's leave mono_commit as is, only insert new content.
        self.submodule_filter_helper.commit_callback(mono_commit)
        trimmed_message = self._trim_push_commit_message(mono_commit.message)

        # Get parent commit hashes for each subrepo.
        # Each subdir might have a different set of parents.
        subrepo_parent_ids_map: DefaultDict[bytes, ParentsList] = defaultdict(list)
        for mono_pid in mono_commit.parents:
            resolved_parent_map = self.mono_id_to_subrepo_parent_ids.get(mono_pid)
            if resolved_parent_map is not None:
                # This parent has been resolved, but potentially only generated
                # a commit to push for some subdirs. For some subrepos,
                # there is a single resolved commit to be used as parent.
                # For other subrepos, the grand parents are forwarded.
                if not isinstance(mono_pid, int):
                    raise ValueError(
                        f"Expected mono commit {mono_pid} to have been processed"
                    )
                for subdir, subrepo_pids in resolved_parent_map.items():
                    unique_extend(subrepo_parent_ids_map[subdir], subrepo_pids)
            else:
                # Commit hash, not an ID.
                # Get the original toprepo commit and find the subrepo pointers.
                # They will be the parents.
                if not isinstance(mono_pid, CommitHash):
                    raise ValueError(
                        f"Subrepo map not found for mono commit {mono_pid}"
                    )
                mono_message = subprocess.check_output(
                    ["git", "-C", str(self.monorepo.path)]
                    + ["show", "--quiet", "--format=%B", mono_pid, "--"],
                )
                top_commit_hash = try_parse_top_hash_from_message(mono_message)
                subrepo_map = self._get_top_commit_subrepos(top_commit_hash)
                # Extend the parents list for each subdir.
                unique_append(subrepo_parent_ids_map[b""], top_commit_hash)
                for subdir, subrepo_commit_hash in subrepo_map.items():
                    unique_append(subrepo_parent_ids_map[subdir], subrepo_commit_hash)

        # The commits might not exist in the target repo.
        # Split into parts.
        file_changes_per_subdir: Dict[
            PurePosixPath, List[git_filter_repo.FileChange]
        ] = defaultdict(list)
        for file_change in mono_commit.file_changes:
            for subdir in self.submodule_filter_helper.submodule_configs.keys():
                path_prefix = subdir + b"/"
                if file_change.filename.startswith(path_prefix):
                    subrepo_fc = clone_file_change(file_change)
                    subrepo_fc.filename = file_change.filename[len(path_prefix) :]
                    file_changes_per_subdir[subdir].append(subrepo_fc)
                    break
            else:
                file_changes_per_subdir[b""].append(file_change)
        # Topic handling.
        topic_required = len(file_changes_per_subdir) > 1
        topic = try_get_topic_from_message(mono_commit.message)
        if topic_required and topic is None:
            raise PushSplitError(
                "A commit spread over multiple repositories (submodules) "
                + "need a topic footer ('Topic: <topic>') "
                + "which wasn't found in the following message:\n"
                + textwrap.indent(mono_commit.message.decode("utf-8"), "  ")
            )
        # Inject a bunch of new commits.
        for subdir, file_changes in file_changes_per_subdir.items():
            new_commit = clone_commit(mono_commit)
            new_commit.message = trimmed_message
            new_commit.file_changes = file_changes
            # Exchange parents for the subrepo.
            new_commit.parents = subrepo_parent_ids_map[subdir]
            subrepo_parent_ids_map[subdir] = [new_commit.id]

            repo = self._get_repo_from_subdir(subdir)
            new_branch = f"refs/repos/{repo.name}/toprepo/push"
            new_commit.branch = new_branch.encode("utf-8")

            # NOTE: While inserting, use a backdoor to get hold of the new commit hash.
            new_commit.original_id = b"push-%d" % new_commit.id
            repo_filter.insert(new_commit, direct_insertion=True)
            # orig_parents are only used to check is it was a merge commit or not.
            repo_filter._record_remapping(new_commit, orig_parents=new_commit.parents)
            new_commit_hash = repo_filter._get_rename(new_commit.original_id)

            # Record the branch should be pushed.
            extra_args = []
            if topic is not None:
                # Special Gerrit handling.
                extra_args.extend(["-o", f"topic={topic}"])
            to_push.append(
                PushInstruction(
                    repo=repo,
                    commit_hash=new_commit_hash,
                    extra_args=extra_args,
                )
            )

        # Record the new commits.
        self.mono_id_to_subrepo_parent_ids[mono_commit.id] = subrepo_parent_ids_map

    def _get_repo_from_subdir(self, subdir: bytes) -> Repo:
        if subdir == b"":
            repo = self.toprepo
        else:
            submod = self.submodule_filter_helper.submodule_configs[subdir]
            repo_configs = self.config.raw_url_to_repos.get(submod.raw_url, [])
            if len(repo_configs) == 0:
                subdir_str = subdir.decode("utf-8")
                raise ValueError(
                    f"The URL {submod.raw_url} for submodule {subdir_str} "
                    + "is missing in the toprepo config. "
                    + "Don't which repository to push to."
                )
            elif len(repo_configs) > 1:
                subdir_str = subdir.decode("utf-8")
                raise ValueError(
                    f"The URL {submod.raw_url} for submodule {subdir_str} "
                    + "has multiple toprepo configurations. "
                    + "Don't which repository to push to of "
                    + ", ".join(repo.name for repo in repo_configs[:-1])
                    + f" and {repo_configs[-1].name}."
                )
            else:
                (repo_config,) = repo_configs
            repo = SubRepo(
                repo_config,
                self.monorepo.get_subrepo_dir(repo_config.name),
            )
        return repo

    def _get_top_commit_subrepos(
        self, top_commit_hash: CommitHash
    ) -> Dict[bytes, CommitHash]:
        ls_tree_subrepo_stdout = subprocess.check_output(
            ["git", "-C", str(self.toprepo.path)]
            + ["ls-tree", "-r", top_commit_hash, "--"],
        )
        subrepo_map = {}
        for line in ls_tree_subrepo_stdout.splitlines(keepends=False):
            submodule_mode_and_type_prefix = b"160000 commit "
            if line.startswith(submodule_mode_and_type_prefix):
                hash_and_path = line[len(submodule_mode_and_type_prefix) :]
                submod_hash, subdir = hash_and_path.split(b"\t", 1)
                subrepo_map[subdir] = submod_hash
        return subrepo_map

    @staticmethod
    def _trim_push_commit_message(mono_message: bytes) -> bytes:
        # Avoid pushing cherry-picked commits with ^-- references.
        trimmed_message = mono_message
        idx = trimmed_message.rfind(b"\n^-- ")
        if idx != -1:
            # Try to remove a single trailing ^-- line from an upstream cherry-pick.
            trimmed_message = trimmed_message[: idx + 1]  # Include LF
        if b"\n^-- " in trimmed_message:
            raise PushSplitError(
                "'^-- ' was found in the following commit message. "
                + "It looks like a commit that already exists upstream.\n"
                + textwrap.indent(mono_message.decode("utf-8"), "  ")
            )
        return trimmed_message


def main_init(args) -> int:
    if args.directory is not None:
        subdir = args.directory
    else:
        subdir = repository_basename(args.repository)
    monorepo_dir: Path = args.cwd / subdir
    if monorepo_dir.exists():
        print(f"ERROR: {monorepo_dir} already exists")
        return 1
    if not monorepo_dir.parent.exists():
        print(f"ERROR: The directory {monorepo_dir.parent} is missing")
        return 1
    monorepo_dir.mkdir()
    try:
        log_run_git(monorepo_dir, ["init", "--quiet"])
        # git-submodule and git-filter-repo fail if remote.origin.url is missing.
        log_run_git(
            monorepo_dir,
            ["config", "remote.origin.url", args.repository],
        )
        # Avoid accidental `git push origin`, use `git-toprepo push`.
        log_run_git(
            monorepo_dir,
            ["config", "remote.origin.pushUrl", "file:///dev/null"],
            log_command=False,
        )
        # Power users can push to the "top" remote.
        log_run_git(
            monorepo_dir,
            ["config", "remote.top.pushUrl", args.repository],
        )
        monorepo = MonoRepo(monorepo_dir)
        toprepo_dir = monorepo.get_toprepo_dir()
        toprepo_dir.mkdir(parents=True)
        log_run_git(
            toprepo_dir,
            ["init", "--quiet", "--bare"],
        )
        # TODO: What about relative paths if args.repository is not an URL?
        log_run_git(
            toprepo_dir,
            ["fetch", "--quiet", args.repository, "+refs/toprepo/*:refs/toprepo/*"],
        )
    except Exception as err:
        print(f"Failed to initialize {monorepo.path}: {err}")
        shutil.rmtree(monorepo_dir, ignore_errors=True)
        raise
    print(f"Initialization of {monorepo.path} succeeded!")
    print("To start, run:")
    print("  git toprepo fetch && git checkout origin/main")
    return 0


def main_config(args) -> int:
    monorepo = MonoRepo(args.cwd)
    config_dict = ConfigAccumulator(monorepo, online=args.online).try_load_main_config()
    if config_dict is None:
        return 1
    if args.key is not None:
        if args.key not in config_dict:
            print("ERROR: Missing configuration key {args.key}")
            return 1
        value = config_dict[args.key][-1]
        print(value)
    elif args.list:
        for key, values in sorted(config_dict.items()):
            for value in values:
                print(f"{key}={value}")
    else:
        assert False, "Bad args {args}"
    return 0


def main_refilter(args) -> int:
    monorepo = MonoRepo(args.cwd)
    config_dict = ConfigAccumulator(monorepo, args.online).try_load_main_config()
    if config_dict is None:
        return 1
    config = Config.try_create(config_dict)
    if config is None:
        return 1
    toprepo = TopRepo.from_config(monorepo.get_toprepo_dir(), config)

    expander = TopRepoExpander(monorepo, toprepo, config)
    if args.from_scratch:
        # Remove all translated monorepo refs.
        top_fetch_head = "refs/toprepo/fetch-head"
        refs_to_delete = get_remote_origin_refs(monorepo) + [top_fetch_head]
        delete_refs(monorepo, refs_to_delete)
        # TODO: Clear the caches.
        raise NotImplementedError("refilter from scratch")
    if not expander.expand_toprepo(top_refs=["--all"], allow_fetching=args.online):
        return 1
    return 0


def main_fetch(args) -> int:
    monorepo = MonoRepo(args.cwd)
    config_dict = ConfigAccumulator(monorepo, online=True).try_load_main_config()
    if config_dict is None:
        return 1
    config = Config.try_create(config_dict)
    if config is None:
        return 1
    toprepo = TopRepo.from_config(monorepo.get_toprepo_dir(), config)
    repo_fetcher = RepoFetcher(monorepo)

    git_modules = get_gitmodules_info(
        LocalFileConfigLoader(monorepo.path / ".gitmodules", allow_missing=True),
        monorepo.get_toprepo_fetch_url(),
    )
    remote_name, git_module = remote_to_repo(args.remote, git_modules, config)
    if remote_name is None:
        return 1

    if remote_name == TopRepo.name:
        expander = TopRepoExpander(monorepo, toprepo, config)
        repo_to_fetch = toprepo
    else:
        expander = SubrepoCommitExpander(monorepo)
        for subrepo_config in config.repos:
            if subrepo_config.name == remote_name:
                repo_to_fetch = SubRepo(
                    subrepo_config,
                    monorepo.get_subrepo_dir(subrepo_config.name),
                )
                subdir = git_module.path.as_posix().encode("utf-8")
                break
        else:
            print(f"ERROR: Could not resolve the remote {args.remote}")
            return 1

    ref_args: List[str]
    if args.ref is None:
        # Just fetch everything in that repo and do standard filtering.
        repo_fetcher.fetch_repo(repo_to_fetch)
        if args.do_filter:
            if not expander.expand_toprepo(top_refs=["--all"], allow_fetching=True):
                return 1
        else:
            print("Skipped expanding the toprepo into the monorepo.")
    else:
        # Fetch ref to refs/toprepo/fetch-head instead of FETCH_HEAD.
        # Then there is no need for extra args to git-fetch or git-filter-repo
        # to pick up FETCH_HEAD.
        ref_args = [f"{args.ref}:refs/toprepo/fetch-head"]
        repo_fetcher.fetch_repo(repo_to_fetch, ref_args)
        if args.do_filter:
            mono_fetch_head_ref = "refs/toprepo/fetch-head"
            if repo_to_fetch.is_top:
                # Special handling of toprepo/fetch-head in the refname_callback.
                top_fetch_head_ref = mono_fetch_head_ref
                # TODO: Only expand top_fetch_head_ref, i.e. remove "--all".
                # Currently, omitting --all gives different result.
                if not expander.expand_toprepo(
                    top_refs=[top_fetch_head_ref, "--all"], allow_fetching=True
                ):
                    return 1
            else:
                subrepo_ref = f"refs/repos/{repo_to_fetch.name}/toprepo/fetch-head"
                if not expander.expand_subrepo_refs(
                    subdir, subrepo_ref, dest_ref=mono_fetch_head_ref
                ):
                    return 1
            # Update FETCH_HEAD.
            subprocess.check_call(
                ["git", "-C", str(monorepo.path)]
                + ["update-ref", "FETCH_HEAD", mono_fetch_head_ref],
            )
            print("Updated FETCH_HEAD")
        else:
            print(
                "Fetched refs/toprepo/fetch-head but skipped creating a "
                + "monorepo FETCH_HEAD."
            )
    return 0


def main_push(args) -> int:
    monorepo = MonoRepo(args.cwd)
    config_dict = ConfigAccumulator(monorepo, online=True).try_load_main_config()
    if config_dict is None:
        return 1
    config = Config.try_create(config_dict)
    if config is None:
        return 1
    toprepo = TopRepo.from_config(monorepo.get_toprepo_dir(), config)

    splitter = PushSplitter(monorepo, toprepo, config)

    refspec: PushRefSpec = args.local_and_remote_ref
    try:
        push_instructions = splitter.split_commits(refspec.local_ref)
    except PushSplitError as err:
        print(f"\nERROR: {err}")
        return 1

    # Push to each subrepo.
    repos_to_push = {push.repo.name: push.repo for push in push_instructions}
    for repo in repos_to_push.values():
        log_run_git(
            monorepo.path,
            ["push", "--quiet", "--force", str(repo.path.absolute())]
            + [f"refs/repos/{repo.name}/toprepo/push:refs/toprepo/push"],
            log_command=False,
        )

    # Sort per branch and remove unnecessary pushes.
    repo_to_pushes: DefaultDict[RepoName, List[PushInstruction]] = defaultdict(list)
    for new_push in push_instructions:
        push_list = repo_to_pushes[new_push.repo.name]
        if len(push_list) != 0 and push_list[-1].same_but_commit(new_push):
            push_list.pop()
        push_list.append(new_push)

    # Push per repo
    for repo_name, push_list in repo_to_pushes.items():
        for push in push_list:
            push_rev = push.commit_hash.decode("utf-8")
            log_run_git(
                push.repo.path,
                ["push", "--quiet", push.repo.config.push_url]
                + [f"{push_rev}:{refspec.remote_ref}"]
                + push.extra_args,
                log_command=True,
                dry_run=args.dry_run,
                check=False,
            )
    return 0


def _parse_arguments(argv):
    # Support pasting normal git commands to this script.
    # For example
    #   git-toprepo git fetch <server> ref
    # should map to
    #   git-toprepo fetch <server> ref
    if len(argv) > 2 and argv[1] == "git":
        argv.pop(1)

    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawTextHelpFormatter,
        prog=Path(argv[0]).name,
    )
    parser.add_argument(
        "-C",
        dest="cwd",
        type=Path,
        default=Path.cwd(),
        help="Working directory, defaults to '.'.",
    )
    parser.set_defaults(func=None)
    subparsers = parser.add_subparsers()

    init_parser = subparsers.add_parser(
        "init",
        description="""\
            Clones a top repository and initializes a mono repository in the current directory.
        """,
    )
    init_parser.set_defaults(func=main_init)
    init_parser.add_argument(
        "repository",
        type=str,
        help="""\
            The URL to the top repository to clone,
            i.e. the repository containing the submodules.""",
    )
    init_parser.add_argument(
        "directory",
        type=PurePath,
        nargs="?",
        help="""\
            Where to initialize the repository.
            Defaults to the base name of the repository.""",
    )

    config_parser = subparsers.add_parser(
        "config",
        description="""\
            Reads the mono repository configuration.
        """,
    )
    config_parser.set_defaults(func=main_config)
    config_parser.add_argument(
        "--offline",
        action="store_false",
        dest="online",
        help="""\
            Disallow fetching the configuration remotely,
            use existing information only.""",
    )
    config_key_group = config_parser.add_mutually_exclusive_group(required=True)
    config_key_group.add_argument(
        "--list",
        action="store_true",
        help="List all configurations.",
    )
    config_key_group.add_argument(
        "key",
        type=str,
        nargs="?",
        help="The name of the configuration to get.",
    )

    refilter_parser = subparsers.add_parser(
        "refilter",
        description="Performes a refiltering of the monorepo.",
    )
    refilter_parser.set_defaults(func=main_refilter)
    refilter_parser.add_argument(
        "--from-scratch",
        dest="from_scratch",
        action="store_true",
        help="""\
            Removes previous filtering results and starts over again.

            This option will remove all refs/* apart from refs/heads/*
            and clear the caches about what commits have been filtered.
            Performing this refiltering might generate new commit hashes
            in the git history, if the algorithm has changed or
            the submodule commit ignore list has been updated.""",
    )
    refilter_parser.add_argument(
        "--offline",
        action="store_false",
        dest="online",
        help="Disallow fetching submodules, use existing information only.",
    )

    fetch_parser = subparsers.add_parser(
        "fetch",
        description="""\
            Fetches the top repository and resolves all refs into the monorepo.
            If any referenced submodule commit is missing,
            the submodule will also be fetched.

            FETCH_HEAD will be updated if a single ref is is specified.
            """,
    )
    fetch_parser.set_defaults(func=main_fetch)
    fetch_parser.add_argument(
        "--skip-filter",
        action="store_false",
        dest="do_filter",
        help="Fetch from the remote but skip monorepo filtering.",
    )
    fetch_parser.add_argument(
        "remote",
        type=str,
        nargs="?",
        default="origin",
        help="""\
            The URL or a submodule path to fetch from.
            Will fetch from the top repository remote
            if 'origin', '.' or '' is specified.
            Defaults to 'origin'.""",
    )
    fetch_parser.add_argument(
        "ref",
        type=str,
        nargs="?",
        help="""\
            The 'refspec' to be fetched from the specified remote.
            If a single ref is specified,
            FETCH_HEAD will be updated accordingly.""",
    )

    push_parser = subparsers.add_parser(
        "push",
        description="""\
            Splits the monorepo into commits to push and pushes them.

            'refs/heads/push' will be updated in the top repository and
            each affected submodule.""",
    )
    push_parser.set_defaults(func=main_push)
    push_parser.add_argument(
        "--dry-run",
        "-n",
        action="store_true",
        help="""\
            Split the monorepo commits and write the git-push commands
            that should have been executed.

            Use this option to push to manually push a different repository
            than the default configured 'origin'.""",
    )
    push_parser.add_argument(
        "remote",
        type=str,
        nargs="?",
        choices=["origin"],
        help="""\
            Unused placeholder in case the user writes 'origin'
            on the command line, like with git-push.""",
    )
    push_parser.add_argument(
        "local_and_remote_ref",
        metavar="local-ref:remote-ref",
        type=PushRefSpec.parse,
        help="""\
            The refspec describing what to push, just like git-push.

            If a single branch name is specified, it is translated into
            'refs/heads/<branch>:refs/heads/<branch>'.""",
    )

    args = parser.parse_args(argv[1:])
    if args.func is None:
        parser.print_help()
        parser.exit(status=2)
    args.cwd = try_relative_path(args.cwd)
    return args


def main(argv=sys.argv):
    args = _parse_arguments(argv)
    try:
        returncode = args.func(args=args)
    except subprocess.CalledProcessError as err:
        cmdline = subprocess.list2cmdline(err.cmd)
        print(f"\rFailed to call  {cmdline}")
        raise
    assert isinstance(returncode, int), returncode
    return returncode


if __name__ == "__main__":
    sys.exit(main())
