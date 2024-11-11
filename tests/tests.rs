use anyhow::Ok;
use git_toprepo;
use git_toprepo::config::GitTopRepoConfig;
use git_toprepo::util::{commit_hash, iter_to_string};
use std::fs;

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
fn test_load_toprepo_conf() {
    let conf = fs::read_to_string("./tests/.gittoprepo-example").expect("Could not open file");
    let top_repo_config: GitTopRepoConfig = toml::from_str(&conf).unwrap();
    assert!(top_repo_config.repo.contains_key("something"));
}
