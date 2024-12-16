use fs::File;
use git_toprepo;
use git_toprepo::config::GitTopRepoConfig;
use git_toprepo::repo_parser::{ChangedFile, DiffStatus, FastExportRepo};
use git_toprepo::util::{commit_env, commit_hash, iter_to_string, log_run_git, GitTopRepoExample};
use std::fs;
use std::io::Write;
use std::str::FromStr;

#[test]
fn pass() {
    assert!(true);
}

#[test]
fn test_repository_name() {
    use git_toprepo::repo::repository_name;

    assert_eq!(
        repository_name(String::from("https://github.com/org/repo")),
        "org-repo"
    );
    assert_eq!(
        repository_name(String::from("https://github.com/org/repo.git")),
        "org-repo"
    );
    assert_eq!(
        repository_name(String::from("https://github.com//org/repo")),
        "org-repo"
    );
    assert_eq!(
        repository_name(String::from("https://github.com/org//repo")),
        "org-repo"
    );
    assert_eq!(
        repository_name(String::from("https://github.com:443/org/repo")),
        "org-repo"
    );
    assert_eq!(
        repository_name(String::from("git://github.com/org/repo")),
        "org-repo"
    );
    assert_eq!(repository_name(String::from(".././org/repo")), "org-repo");
    assert_eq!(
        repository_name(String::from("abc\\org\\repo")),
        "abc-org-repo"
    );
}

#[test]
fn test_join_submodule_url() {
    use git_toprepo::gitmodules::join_submodule_url;

    // Relative.
    assert_eq!(
        join_submodule_url("https://github.com/org/repo", "."),
        "https://github.com/org/repo"
    );
    assert_eq!(
        join_submodule_url("https://github.com/org/repo", "./"),
        "https://github.com/org/repo"
    );
    assert_eq!(
        join_submodule_url("https://github.com/org/repo", "./foo"),
        "https://github.com/org/repo/foo"
    );
    assert_eq!(
        join_submodule_url("https://github.com/org/repo", "../foo"),
        "https://github.com/org/foo"
    );
    assert_eq!(
        join_submodule_url("https://github.com/org/repo", "../../foo"),
        "https://github.com/foo"
    );

    // Ignore double slash.
    assert_eq!(
        join_submodule_url("https://github.com/org/repo", ".//foo"),
        "https://github.com/org/repo/foo"
    );

    // Handle too many '../'.
    assert_eq!(
        join_submodule_url("https://github.com/org/repo", "../../../foo"),
        "https://github.com/../foo"
    );
    assert_eq!(
        join_submodule_url("file:///data/repo", "../../foo"),
        "file:///foo"
    );
    assert_eq!(
        join_submodule_url("file:///data/repo", "../../../foo"),
        "file:///../foo"
    );

    // Absolute.
    assert_eq!(
        join_submodule_url("parent", "ssh://github.com/org/repo"),
        "ssh://github.com/org/repo"
    );

    // Without scheme.
    assert_eq!(join_submodule_url("parent", "/data/repo"), "/data/repo");
    assert_eq!(join_submodule_url("/data/repo", "../other"), "/data/other");
}

#[test]
fn test_config_repo_is_wanted() {
    use git_toprepo::config::Config;
    assert!(Config::repo_is_wanted("Repo", &iter_to_string(["+Repo"])).unwrap());
    assert!(!Config::repo_is_wanted("Repo", &iter_to_string(["+Repo", "-Repo"])).unwrap());
    assert!(Config::repo_is_wanted("Repo", &iter_to_string(["+R"])).is_none());
    assert!(Config::repo_is_wanted("Repo", &iter_to_string(["-o"])).is_none());
    assert!(Config::repo_is_wanted("Repo", &iter_to_string(["-.*", "+Repo"])).unwrap());
    assert!(!Config::repo_is_wanted("Repo", &iter_to_string(["+.*", "-Repo"])).unwrap());
}

#[test]
fn test_annotate_message() {
    use git_toprepo::util::annotate_message;

    // Don't fold the footer into the subject line, leave an empty line.
    assert_eq!(
        annotate_message("Subject line\n", "sub/dir", &commit_hash("123hash"),),
        "\
Subject line

^-- sub/dir 123hash
"
    );

    assert_eq!(
        annotate_message("Subject line, no LF", "sub/dir", &commit_hash("123hash"),),
        "\
Subject line, no LF

^-- sub/dir 123hash
"
    );

    assert_eq!(
        annotate_message("Double subject line\n", "sub/dir", &commit_hash("123hash"),),
        "\
Double subject line

^-- sub/dir 123hash
"
    );

    assert_eq!(
        annotate_message(
            "Subject line, extra LFs\n\n\n",
            "sub/dir",
            &commit_hash("123hash"),
        ),
        "\
Subject line, extra LFs

^-- sub/dir 123hash
",
    );

    assert_eq!(
        annotate_message(
            "Multi line\n\nmessage\n",
            "sub/dir",
            &commit_hash("123hash")
        ),
        "\
Multi line

message
^-- sub/dir 123hash
",
    );

    assert_eq!(
        annotate_message(
            "Multi line\n\nmessage, no LF",
            "sub/dir",
            &commit_hash("123hash"),
        ),
        "\
Multi line

message, no LF
^-- sub/dir 123hash
",
    );

    assert_eq!(
        annotate_message(
            "Multi line\n\nmessage, extra LFs\n\n\n",
            "sub/dir",
            &commit_hash("123hash"),
        ),
        "\
Multi line

message, extra LFs
^-- sub/dir 123hash
",
    )
}

#[test]
#[should_panic]
fn test_create_config_from_invalid_ref() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp_path = tmp_dir.path().to_path_buf();
    let env = commit_env();

    log_run_git(Some(&tmp_path), ["init"], Some(&env), false, false).unwrap();

    log_run_git(
        Some(&tmp_path),
        ["config", "toprepo.config", ":foobar.toml"],
        Some(&env),
        false,
        false,
    )
    .unwrap();

    GitTopRepoConfig::try_from(tmp_path.as_path()).unwrap();
}

#[test]
fn test_create_config_from_worktree() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp_path = tmp_dir.path().to_path_buf();
    let env = commit_env();

    log_run_git(Some(&tmp_path), ["init"], Some(&env), false, false).unwrap();

    let mut tmp_file = File::create(tmp_path.join("foobar.toml")).unwrap();

    writeln!(
        tmp_file,
        r#"[repo]
[repo.foo.fetch]
url = "ssh://bar/baz.git"
[repos]"#
    )
    .unwrap();

    log_run_git(
        Some(&tmp_path),
        ["add", "foobar.toml"],
        Some(&env),
        false,
        false,
    )
    .unwrap();

    log_run_git(
        Some(&tmp_path),
        ["config", "toprepo.config", ":foobar.toml"],
        Some(&env),
        false,
        false,
    )
    .unwrap();

    let conf = GitTopRepoConfig::try_from(tmp_path.as_path()).unwrap();

    assert!(conf.repo.contains_key("foo"));
    assert_eq!(conf.repo.get("foo").unwrap().fetch.url, "ssh://bar/baz.git");
    assert_eq!(conf.repo.get("foo").unwrap().push.url, "ssh://bar/baz.git");
    assert_eq!(conf.repos.filter.first().unwrap(), "+.*");
}

#[test]
fn test_create_config_from_empty_string() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp_path = tmp_dir.path().to_path_buf();
    let env = commit_env();

    log_run_git(Some(&tmp_path), ["init"], Some(&env), false, false).unwrap();

    log_run_git(
        Some(&tmp_path),
        ["commit", "--allow-empty", "-m", "Initial commit"],
        Some(&env),
        false,
        false,
    )
    .unwrap();

    log_run_git(
        Some(&tmp_path),
        ["update-ref", "refs/toprepo-super/HEAD", "HEAD"],
        Some(&env),
        false,
        false,
    )
    .unwrap();

    let config = GitTopRepoConfig::try_from(tmp_path.as_path()).unwrap();

    assert!(config.repo.is_empty());
    assert_eq!(config.repos.filter.first().unwrap(), "+.*");
}

#[test]
fn test_create_config_from_head() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp_path = tmp_dir.path().to_path_buf();
    let env = commit_env();

    log_run_git(Some(&tmp_path), ["init"], Some(&env), false, false).unwrap();

    let mut tmp_file = File::create(tmp_path.join(".gittoprepo.toml")).unwrap();

    writeln!(
        tmp_file,
        r#"[repo]
[repo.foo.fetch]
url = "ssh://bar/baz.git"
[repos]"#
    )
    .unwrap();

    log_run_git(
        Some(&tmp_path),
        ["add", ".gittoprepo.toml"],
        Some(&env),
        false,
        false,
    )
    .unwrap();

    log_run_git(
        Some(&tmp_path),
        ["commit", "-m", "Initial commit"],
        Some(&env),
        false,
        false,
    )
    .unwrap();

    log_run_git(
        Some(&tmp_path),
        ["update-ref", "refs/toprepo-super/HEAD", "HEAD"],
        Some(&env),
        false,
        false,
    )
    .unwrap();

    log_run_git(
        Some(&tmp_path),
        ["rm", ".gittoprepo.toml"],
        Some(&env),
        false,
        false,
    )
    .unwrap();

    log_run_git(
        Some(&tmp_path),
        ["commit", "-m", "Remove .gittoprepo.toml"],
        Some(&env),
        false,
        false,
    )
    .unwrap();

    let conf = GitTopRepoConfig::try_from(tmp_path.as_path()).unwrap();

    assert!(conf.repo.contains_key("foo"));
    assert_eq!(conf.repo.get("foo").unwrap().fetch.url, "ssh://bar/baz.git");
    assert_eq!(conf.repo.get("foo").unwrap().push.url, "ssh://bar/baz.git");
    assert_eq!(conf.repos.filter.first().unwrap(), "+.*");
}

#[test]
fn test_get_repo_with_new_entry() {
    let mut config = GitTopRepoConfig::from_str("").unwrap();

    config.get_repo_config("ssh://bar/baz.git");

    assert!(config.repo.contains_key("baz"));
    assert_eq!(config.repos.filter.first().unwrap(), "+.*");
}

#[test]
fn test_get_repo_without_new_entry() {
    let mut config = GitTopRepoConfig::from_str(
        r#"[repo.foo]
        urls = ["../bar/repo.git"]

        [repos]"#,
    )
    .unwrap();

    config.get_repo_config("foo");

    assert_eq!(config.repo.len(), 1);
}

#[test]
#[should_panic]
fn test_config_with_duplicate_urls() {
    GitTopRepoConfig::from_str(
        r#"[repo.foo]
        urls = ["ssh://bar/baz.git"]

        [repo.bar]
        urls = ["ssh://bar/baz.git"]

        [repos]"#,
    )
    .unwrap();
}

#[test]
fn test_parse_fast_export_output() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp_path = tmp_dir.path().to_path_buf();

    let example_repo = GitTopRepoExample::new(&tmp_path);
    let example_top_repo = example_repo.init_server_top();

    let mut repo = FastExportRepo::try_from(example_top_repo.as_path()).unwrap();
    let commit_a = repo.next().unwrap();
    let commit_d = repo.nth(2).unwrap();

    assert_eq!(commit_a.branch, b"refs/heads/main");
    assert_eq!(
        commit_a.author_info,
        b"author A Name <a@no.domain> 1672625045 +0100"
    );
    assert_eq!(
        commit_a.committer_info,
        b"committer C Name <c@no.domain> 1686121750 +0100"
    );
    assert_eq!(commit_a.message, b"A\n");
    assert!(commit_a.file_changes.is_empty());
    assert!(commit_a.parents.is_empty());
    assert_eq!(
        commit_a.original_id,
        b"6fc12aa7d6d06400a70bb522244bb184e3678416"
    );
    assert_eq!(commit_a.encoding, None);

    assert_eq!(commit_d.branch, b"refs/heads/main");
    assert_eq!(
        commit_d.author_info,
        b"author A Name <a@no.domain> 1672625045 +0100"
    );
    assert_eq!(
        commit_d.committer_info,
        b"committer C Name <c@no.domain> 1686121750 +0100"
    );
    assert_eq!(commit_d.message, b"D\n");
    assert_eq!(
        // CommitInfo and DiffStatus implements PartialEq trait for this test,
        // might exclude PartialEq and this test if it's not needed elsewhere.
        commit_d.file_changes.first().unwrap(),
        &ChangedFile {
            file: Vec::from(b"sub"),
            mode: Some(Vec::from(b"160000")),
            hash: Some(Vec::from(b"eeb85c77b614a7ec060f6df5825c9a5c10414307")),
            status: DiffStatus::Modified
        }
    );
    assert_eq!(
        commit_d.parents,
        Vec::from([b"ec67a8703750336a938bef740115009b6310892f"])
    );
    assert_eq!(
        commit_d.original_id,
        b"9f781a9707757573b16ee5946ab147e4e66857bc"
    );
    assert_eq!(commit_d.encoding, None);
}
