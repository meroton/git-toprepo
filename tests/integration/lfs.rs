use super::fetch::RepoWithTwoSubmodules;
use anyhow::Context as _;
use bstr::ByteSlice as _;
use git_toprepo::gitmodules::SubmoduleUrlExt as _;
use git_toprepo_testtools::test_util::CommandHelper as _;
use git_toprepo_testtools::test_util::cargo_bin_git_toprepo_for_testing;
use git_toprepo_testtools::test_util::git_command_for_testing;
use itertools::Itertools as _;
use predicates::prelude::*;
use sha2::Digest as _;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::read_to_string;
use std::io::Read as _;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

struct RepoWithInnerSubmodule {
    monorepo: PathBuf,
    #[expect(unused)]
    temp_dir_guard: git_toprepo_testtools::test_util::MaybePermanentTempDir,
}

impl RepoWithInnerSubmodule {
    fn new() -> Self {
        let temp_dir_guard = git_toprepo_testtools::test_util::maybe_keep_tempdir(
            gix_testtools::scripted_fixture_writable(
                "../integration/fixtures/make_minimal_with_inner_submodule.sh",
            )
            .unwrap(),
        );
        let temp_dir = temp_dir_guard.canonicalize().unwrap();
        let monorepo = temp_dir.join("mono");
        crate::fixtures::toprepo::clone(&temp_dir.join("top"), &monorepo);
        Self {
            monorepo,
            temp_dir_guard,
        }
    }
}

fn make_fake_git_lfs(dir: &Path, version_exit: i32) -> PathBuf {
    let path = dir.join("git-lfs");
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "version" ]; then
  echo "git-lfs/FAKE"
  exit {version_exit}
fi
if [ "$1" = "fetch" ]; then
  shift
  remote=""
  printf 'cwd=%s\n' "$(pwd)" >> "$GIT_TOPREPO_TEST_LFS_LOG"
  for arg in "$@"; do
    if [ "$arg" = "--include" ]; then
      opt_prefix="--include="
    elif [ "$arg" = "--exclude" ]; then
      opt_prefix="--exclude="
    elif [ -z "$opt_prefix" ]; then
      remote=$arg
    else
      printf 'arg=%s%s\n' "$opt_prefix" "$arg" >> "$GIT_TOPREPO_TEST_LFS_LOG"
      opt_prefix=""
    fi
  done
  printf 'remote=%s\n' "$remote" >> "$GIT_TOPREPO_TEST_LFS_LOG"
  exit "${{GIT_TOPREPO_TEST_LFS_EXIT:-0}}"
fi
echo "unexpected git-lfs command: $*" >&2
exit 64
"#
    );
    git_toprepo::util::create_executable(&path, script).unwrap();
    path
}

#[test]
fn install() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let cmd = cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "install"])
        .assert()
        .failure()
        .stdout("");

    insta::assert_snapshot!(
        cmd.get_output().stderr.to_str().unwrap(),
        @"ERROR: The `lfs install` command is unavailable; use `git toprepo hooks install --git-lfs` instead."
    );
}

#[test]
fn missing_git_lfs_fails_clearly() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    make_fake_git_lfs(bin_dir, 1);

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "subpathx/assets/model.bin"])
        .prepend_path_env(bin_dir.to_path_buf())
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Git LFS is required for 'git toprepo lfs fetch'",
        ))
        .stderr(predicate::str::contains("git lfs version"))
        .stderr(predicate::str::contains("Install Git LFS"));
}

#[test]
fn fetches_top_level_path_from_top_repo_url() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    let log_path = bin_dir.join("lfs.log");
    make_fake_git_lfs(bin_dir, 0);

    let top_url = git_command_for_testing(&repo.monorepo)
        .args(["config", "--get", "remote.origin.url"])
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .trim()
        .to_owned();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "-I", "/video.mov"])
        .env("GIT_TOPREPO_TEST_LFS_LOG", &log_path)
        .prepend_path_env(bin_dir.to_path_buf())
        .assert()
        .success();

    let log = read_to_string(&log_path).unwrap();
    assert!(
        log.contains(&format!("cwd={}\n", repo.monorepo.display())),
        "{log}"
    );
    assert!(log.contains(&format!("remote={top_url}")), "{log}");
    assert!(log.contains("arg=--include=/video.mov"), "{log}");
}

#[test]
fn fetches_subrepo_path_from_subrepo_url() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    let log_path = bin_dir.join("lfs.log");
    make_fake_git_lfs(bin_dir, 0);

    let top_url = git_command_for_testing(&repo.monorepo)
        .args(["config", "--get", "remote.origin.url"])
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .trim()
        .to_owned();
    let submodule_url = git_command_for_testing(&repo.monorepo)
        .args([
            "config",
            "--file",
            ".gitmodules",
            "--get",
            "submodule.subpathx.url",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .trim()
        .to_owned();
    let expected_remote = gix::Url::from_bytes(top_url.as_bytes().as_bstr())
        .unwrap()
        .join(&gix::Url::from_bytes(submodule_url.as_bytes().as_bstr()).unwrap())
        .to_string();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "-I", "subpathx/assets/model.bin"])
        .env("GIT_TOPREPO_TEST_LFS_LOG", &log_path)
        .prepend_path_env(bin_dir.to_path_buf())
        .assert()
        .success();

    let log = read_to_string(&log_path).unwrap();
    assert!(log.contains(&format!("remote={expected_remote}")), "{log}");
    assert!(log.contains("arg=--include"), "{log}");
    assert!(
        log.contains("arg=--include=subpathx/assets/model.bin"),
        "{log}"
    );
}

#[test]
fn fetches_absolute_path_even_from_subdirectory() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    let log_path = bin_dir.join("lfs.log");
    make_fake_git_lfs(bin_dir, 0);
    std::fs::create_dir_all(repo.monorepo.join("subpathx")).unwrap();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(repo.monorepo.join("subpathx"))
        .args(["lfs", "fetch", "-I", "subpathy/assets/model.bin"])
        .env("GIT_TOPREPO_TEST_LFS_LOG", &log_path)
        .prepend_path_env(bin_dir.to_path_buf())
        .assert()
        .success();

    let log = read_to_string(&log_path).unwrap();
    assert!(
        log.contains(&format!("cwd={}\n", repo.monorepo.display())),
        "{log}"
    );
    assert!(
        log.contains("arg=--include=subpathy/assets/model.bin"),
        "{log}"
    );
}

#[test]
fn fetches_multiple_include_paths() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    let log_path = bin_dir.join("lfs.log");
    make_fake_git_lfs(bin_dir, 0);

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "-I", "subpathx/a.bin,subpathy/b.bin"])
        .env("GIT_TOPREPO_TEST_LFS_LOG", &log_path)
        .prepend_path_env(bin_dir.to_path_buf())
        .assert()
        .success();

    let log = read_to_string(&log_path).unwrap();
    assert_eq!(log.matches("cwd=").count(), 3, "{log}");
    // Top level.
    assert!(
        log.contains("arg=--include=subpathx/a.bin,subpathy/b.bin"),
        "{log}"
    );
    // subpathx subrepo.
    assert!(log.contains("arg=--include=/subpathx/a.bin"), "{log}");
    // subpathy subrepo.
    assert!(log.contains("arg=--include=/subpathy/b.bin"), "{log}");
}

#[test]
fn fetch_excludes_are_passed_on() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    let log_path = bin_dir.join("lfs.log");
    make_fake_git_lfs(bin_dir, 0);

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args([
            "lfs",
            "fetch",
            "-X",
            "subpathx/exclude.bin,subpathy/exclude.bin",
        ])
        .env("GIT_TOPREPO_TEST_LFS_LOG", &log_path)
        .prepend_path_env(bin_dir.to_path_buf())
        .assert()
        .success();

    let log = read_to_string(&log_path).unwrap();
    assert_eq!(log.matches("cwd=").count(), 3, "{log}");
    assert_eq!(
        log.matches("arg=--exclude=subpathx/exclude.bin,subpathy/exclude.bin")
            .count(),
        3,
        "{log}"
    );
}

#[test]
fn rejects_unsupported_options() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    make_fake_git_lfs(bin_dir, 0);

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "--all"])
        .prepend_path_env(bin_dir.to_path_buf())
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            " for '--all': unsupported for 'git toprepo lfs fetch'",
        ));

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "--stdin"])
        .prepend_path_env(bin_dir.to_path_buf())
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            " for '--stdin': unsupported for 'git toprepo lfs fetch'",
        ));

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "--json"])
        .prepend_path_env(bin_dir.to_path_buf())
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            " for '--json': unsupported for 'git toprepo lfs fetch'",
        ));
}

#[test]
fn errors_on_unconfigured_subrepo() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    let log_path = bin_dir.join("lfs.log");
    make_fake_git_lfs(bin_dir, 0);

    git_command_for_testing(&repo.monorepo)
        .args([
            "config",
            "--replace-all",
            "toprepo.config",
            "must:local:.gittoprepo.toml",
        ])
        .assert()
        .success();
    std::fs::write(
        repo.monorepo.join(".gittoprepo.toml"),
        "[repo.namey]\nurl = \"../repoy/\"\n",
    )
    .unwrap();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "-I", "subpathx/file.bin"])
        .env("GIT_TOPREPO_TEST_LFS_LOG", &log_path)
        .prepend_path_env(bin_dir.to_path_buf())
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Missing URL ../repox/ in the Git Toprepo configuration",
        ));

    assert!(!std::fs::exists(&log_path).unwrap());
}

#[test]
fn uses_deepest_matching_submodule_path() {
    let repo = RepoWithInnerSubmodule::new();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    let log_path = bin_dir.join("lfs.log");
    make_fake_git_lfs(bin_dir, 0);

    let top_url = git_command_for_testing(&repo.monorepo)
        .args(["config", "--get", "remote.origin.url"])
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .trim()
        .to_owned();
    let inner_url = git_command_for_testing(&repo.monorepo)
        .args([
            "config",
            "--file",
            ".gitmodules",
            "--get",
            "submodule.subpathx.url",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .to_str()
        .unwrap()
        .trim()
        .to_owned();
    let expected_remote = gix::Url::from_bytes(top_url.as_bytes().as_bstr())
        .unwrap()
        .join(&gix::Url::from_bytes(inner_url.as_bytes().as_bstr()).unwrap())
        .to_string();

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "-I", "subpathx/subpathy/model.bin"])
        .env("GIT_TOPREPO_TEST_LFS_LOG", &log_path)
        .prepend_path_env(bin_dir.to_path_buf())
        .assert()
        .success();

    let log = read_to_string(&log_path).unwrap();
    assert!(log.contains(&format!("remote={expected_remote}")), "{log}");
    assert!(
        log.contains("arg=--include=subpathx/subpathy/model.bin"),
        "{log}"
    );
}

#[test]
fn preserves_spaces_in_include_path() {
    let repo = RepoWithTwoSubmodules::new_minimal_with_two_submodules();
    let temp_dir = git_toprepo_testtools::test_util::MaybePermanentTempDir::create();
    let bin_dir = temp_dir.path();
    let log_path = bin_dir.join("lfs.log");
    make_fake_git_lfs(bin_dir, 0);

    cargo_bin_git_toprepo_for_testing()
        .current_dir(&repo.monorepo)
        .args(["lfs", "fetch", "-I", "subpathx/assets/big file.bin"])
        .env("GIT_TOPREPO_TEST_LFS_LOG", &log_path)
        .prepend_path_env(bin_dir.to_path_buf())
        .assert()
        .success();

    let log = read_to_string(&log_path).unwrap();
    assert!(
        log.lines()
            .contains("arg=--include=subpathx/assets/big file.bin"),
        "{log}"
    );
}

#[derive(Debug, serde::Deserialize)]
struct LfsBatchRequest {
    operation: String,
    objects: Vec<LfsObject>,
    hash_algo: String,
}

#[derive(Debug, serde::Deserialize)]
struct LfsObject {
    oid: String,
    size: u64,
}

fn accept_http_request(
    listener: &std::net::TcpListener,
) -> (std::net::TcpStream, String, String, String, Vec<u8>) {
    let (mut stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let (header, body) = read_http_request(&mut stream);
    let first_line = header.lines().next().unwrap();
    let (method, path, _) = first_line.split(' ').collect_tuple().unwrap();
    (stream, method.to_owned(), path.to_owned(), header, body)
}

fn read_http_request(stream: &mut std::net::TcpStream) -> (String, Vec<u8>) {
    let mut req = Vec::new();
    let (header, mut body) = 'header_loop: loop {
        let mut buf = [0; 1024];
        let n = stream.read(&mut buf).unwrap();
        if n == 0 {
            panic!("Connection closed while reading HTTP request");
        }
        println!(
            "Read {} bytes from stream: {:?}",
            n,
            buf[..n].to_str_lossy()
        );
        req.extend_from_slice(&buf[..n]);
        for (splitter, newline_len) in [("\n\n", 2), ("\r\n\r\n", 4)] {
            if let Some((_, partial_body)) = req.split_once_str(splitter) {
                let header_len = req.len() - partial_body.len() - newline_len;
                let header = std::str::from_utf8(&req[..header_len]).unwrap().to_owned();
                req.drain(..header_len + newline_len);
                break 'header_loop (header, req);
            }
        }
    };
    let Some(content_length) = header.lines().find_map(|line| {
        if let Some(value) = line.strip_prefix("Content-Length: ") {
            value.parse::<usize>().ok()
        } else {
            None
        }
    }) else {
        body.clear();
        return (header, body);
    };
    let mut bytes_read = body.len();
    body.resize(content_length, 0);
    while bytes_read < content_length {
        let n = stream.read(&mut body[bytes_read..]).unwrap();
        if n == 0 {
            panic!(
                "Connection closed after reading HTTP request body, {bytes_read}/{content_length} bytes read"
            );
        }
        bytes_read += n;
    }
    (header, body)
}

#[test]
fn end_to_end_lfs_fetch_checkout() {
    end_to_end_lfs_skip_smudge_helper(
        |monorepo: &Path| {
            cargo_bin_git_toprepo_for_testing()
                .current_dir(monorepo)
                .args(["lfs", "fetch", "-I", "subpathx/large-file.txt"])
                .timeout(Duration::from_secs(2))
                .assert()
                .success()
                .stdout("")
                .stderr("");
        },
        // Git LFS checkout function.
        |monorepo: &Path| {
            cargo_bin_git_toprepo_for_testing()
                .current_dir(monorepo)
                .args(["lfs", "checkout", "subpathx/large-file.txt"])
                .assert()
                .success()
                .stdout("Checking out LFS objects: 100% (1/1), 11 B | 0 B/s, done.\n")
                .stderr("");
            let lf_path_x = monorepo.join("subpathx/large-file.txt");
            assert_eq!(&read_to_string(&lf_path_x).unwrap(), "large-repox");
        },
    );
}

#[test]
fn end_to_end_lfs_pull() {
    end_to_end_lfs_skip_smudge_helper(
        // Git LFS fetch function.
        |monorepo: &Path| {
            cargo_bin_git_toprepo_for_testing()
                .current_dir(monorepo)
                .args(["lfs", "pull", "-I", "subpathx/large-file.txt"])
                .timeout(Duration::from_secs(2))
                .assert()
                .success()
                .stdout("Checking out LFS objects: 100% (15/15), 62 B | 0 B/s, done.\n")
                .stderr(r#"Skipped checkout for "large-file.txt", content not local. Use fetch to download.
Skipped checkout for "small-file.txt", content not local. Use fetch to download.
Skipped checkout for "subpathx/small-file.txt", content not local. Use fetch to download.
Skipped checkout for "subpathx/subpathy/large-file.txt", content not local. Use fetch to download.
Skipped checkout for "subpathx/subpathy/small-file.txt", content not local. Use fetch to download.
"#);
        },
        // Git LFS checkout function.
        |monorepo: &Path| {
            let lf_path_root = monorepo.join("large-file.txt");
            let lf_path_x = monorepo.join("subpathx/large-file.txt");
            let lf_path_y = monorepo.join("subpathx/subpathy/large-file.txt");
            assert_eq!(&read_to_string(&lf_path_root).unwrap()[..8], "version ");
            assert_eq!(&read_to_string(&lf_path_x).unwrap(), "large-repox");
            assert_eq!(&read_to_string(&lf_path_y).unwrap()[..8], "version ");
        },
    );
}

fn end_to_end_lfs_skip_smudge_helper(
    lfs_fetch_fn: impl Fn(&Path),
    lfs_checkout_fn: impl Fn(&Path),
) {
    let listener = std::net::TcpListener::bind("localhost:0").unwrap();
    let fake_lfs_server_port = listener.local_addr().unwrap().port();

    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_lfs_with_inner_submodule.sh",
        )
        .unwrap(),
    );

    let monorepo = &temp_dir.join("mono");
    const INSTALL_HOOKS_COMMAND: &str = "hooks install --git-lfs";
    cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg(temp_dir.join("top"))
        .arg(monorepo)
        .assert()
        .success()
        .stderr(predicates::str::contains(format!("Git LFS is available. Add the Git LFS hooks using \'git toprepo {INSTALL_HOOKS_COMMAND}\'")));
    git_command_for_testing(monorepo)
        .args([
            "config",
            "remote.origin.url",
            &format!("http://localhost:{fake_lfs_server_port}/git/server"),
        ])
        .assert()
        .success();

    // Try checking out subpathx/large-file.txt only.
    let large_repox_digest = hex::encode(sha2::Sha256::digest(b"large-repox"));

    // Remove all files, install filters and checkout again to see that smudge
    // works without trying to download LFS objects.
    git_command_for_testing(monorepo)
        .args(["rm", "-r", "."])
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(monorepo)
        .args(INSTALL_HOOKS_COMMAND.split(' '))
        .arg("--git-lfs-skip-smudge")
        .assert()
        .success();
    git_command_for_testing(monorepo)
        .apply_git_toprepo_path()
        .args(["reset", "--hard"])
        .assert()
        .success();
    let lf_path_root = monorepo.join("large-file.txt");
    let lf_path_x = monorepo.join("subpathx/large-file.txt");
    let lf_path_y = monorepo.join("subpathx/subpathy/large-file.txt");
    assert_eq!(&read_to_string(&lf_path_root).unwrap()[..8], "version ");
    assert_eq!(&read_to_string(&lf_path_x).unwrap()[..8], "version ");
    assert_eq!(&read_to_string(&lf_path_y).unwrap()[..8], "version ");

    // Start our Git LFS server stub. Detach the thread in case of failure where
    // the thread might hang.
    let (lfs_server_done_tx, lfs_server_done_rx) = std::sync::mpsc::channel();
    let lfs_server_stub_thread = std::thread::spawn(move || {
        let (mut stream, method, path, _header, body) = accept_http_request(&listener);
        assert_eq!(method, "POST");
        // URL resolution of /git/server/../repox.git
        assert_eq!(path, "/git/repox.git/info/lfs/objects/batch");
        let req = serde_json::from_slice::<LfsBatchRequest>(body.as_bytes()).unwrap();
        assert_eq!(req.operation, "download");
        assert_eq!(req.hash_algo, "sha256");
        assert_eq!(req.objects.len(), 1);
        assert_eq!(req.objects[0].oid, large_repox_digest);
        assert_eq!(req.objects[0].size, 11);
        let response_content = format!(
            r#"{{
  "transfer": "basic",
  "objects": [
    {{
      "oid": "{large_repox_digest}",
      "size": 11,
      "authenticated": false,
      "actions": {{ "download": {{ "href": "http://localhost:{}/blobs/e57" }} }}
    }}
  ],
  "hash_algo": "sha256"
}}"#,
            fake_lfs_server_port
        );
        stream
            .write_all(
                format!(
                    r#"HTTP/1.1 200 OK
Content-Type: application/vnd.git-lfs+json
Content-Length: {}

"#,
                    response_content.len()
                )
                .as_bytes(),
            )
            .unwrap();
        stream.write_all(response_content.as_bytes()).unwrap();
        drop(stream);

        // Next, transfer the blob.
        let (mut stream, method, path, _header, body) = accept_http_request(&listener);
        assert_eq!(method, "GET");
        assert_eq!(path, "/blobs/e57");
        assert_eq!(body, b"");
        stream
            .write_all(
                r#"HTTP/1.1 200 OK
Content-Type: application/octet-stream
Content-Length: 11
Connection: close

large-repox"#
                    .as_bytes(),
            )
            .unwrap();
        drop(stream);
        lfs_server_done_tx.send(()).unwrap();
    });
    lfs_fetch_fn(monorepo);

    lfs_server_done_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    lfs_server_stub_thread.join().unwrap();

    lfs_checkout_fn(monorepo);
}

#[test]
fn end_to_end_smudge_clean_filters() {
    end_to_end_filters_impl(false);
}

#[test]
fn end_to_end_with_filter_process() {
    end_to_end_filters_impl(true);
}

fn end_to_end_filters_impl(with_filter_process: bool) {
    let listener = std::net::TcpListener::bind("localhost:0").unwrap();
    let fake_lfs_server_port = listener.local_addr().unwrap().port();

    let temp_dir = git_toprepo_testtools::test_util::maybe_keep_tempdir(
        gix_testtools::scripted_fixture_writable(
            "../integration/fixtures/make_lfs_with_inner_submodule.sh",
        )
        .unwrap(),
    );

    let monorepo = &temp_dir.join("mono");
    const INSTALL_HOOKS_COMMAND: &str = "hooks install --git-lfs";
    cargo_bin_git_toprepo_for_testing()
        .arg("clone")
        .arg(temp_dir.join("top"))
        .arg(monorepo)
        .assert()
        .success()
        .stderr(predicates::str::contains(format!("Git LFS is available. Add the Git LFS hooks using \'git toprepo {INSTALL_HOOKS_COMMAND}\'")));
    git_command_for_testing(monorepo)
        .args([
            "config",
            "remote.origin.url",
            &format!("http://localhost:{fake_lfs_server_port}/git/top"),
        ])
        .assert()
        .success();

    // Remove all files, install filters and checkout again to see that smudge
    // works and downloads the LFS objects.
    git_command_for_testing(monorepo)
        .args(["rm", "-r", "."])
        .assert()
        .success();
    cargo_bin_git_toprepo_for_testing()
        .current_dir(monorepo)
        .args(INSTALL_HOOKS_COMMAND.split(' '))
        .assert()
        .success();
    if !with_filter_process {
        git_command_for_testing(monorepo)
            .args(["config", "--unset-all", "filter.lfs.process"])
            .assert()
            .success();
    }

    // Start our Git LFS server stub. Detach the thread in case of failure where
    // the thread might hang.
    let (lfs_server_done_tx, lfs_server_done_rx) = std::sync::mpsc::channel();
    let lfs_server_stub_thread = std::thread::spawn(move || {
        let mut files = HashMap::new();
        let mut content_to_transfer = 0;
        // Make sure the resutests are made to different servers.
        files.insert("/git/top.git/info/lfs/objects/batch", "large-top");
        files.insert("/git/repox.git/info/lfs/objects/batch", "large-repox");
        files.insert("/git/repoy.git/info/lfs/objects/batch", "large-repoy");
        while !files.is_empty() || content_to_transfer > 0 {
            let (mut stream, method, path, _header, body) = accept_http_request(&listener);
            match method.as_str() {
                "POST" => {
                    let file_content = files
                        .remove(path.as_str())
                        .with_context(|| format!("Unexpected path: {path}"))
                        .unwrap();
                    let file_digest = hex::encode(sha2::Sha256::digest(file_content));
                    let req = serde_json::from_slice::<LfsBatchRequest>(body.as_bytes()).unwrap();
                    assert_eq!(req.operation, "download");
                    assert_eq!(req.hash_algo, "sha256");
                    assert_eq!(req.objects.len(), 1);
                    assert_eq!(req.objects[0].oid, file_digest);
                    assert_eq!(req.objects[0].size, file_content.len() as u64);
                    let response_content = format!(
                        r#"{{
"transfer": "basic",
"objects": [
{{
    "oid": "{file_digest}",
    "size": {},
    "authenticated": false,
    "actions": {{ "download": {{ "href": "http://localhost:{}/blobs/{file_content}" }} }}
}}
],
"hash_algo": "sha256"
}}"#,
                        file_content.len(),
                        fake_lfs_server_port,
                    );
                    stream
                        .write_all(
                            format!(
                                r#"HTTP/1.1 200 OK
Content-Type: application/vnd.git-lfs+json
Content-Length: {}

"#,
                                response_content.len()
                            )
                            .as_bytes(),
                        )
                        .unwrap();
                    stream.write_all(response_content.as_bytes()).unwrap();
                    drop(stream);
                    // One file ready to transfer.
                    content_to_transfer += 1;
                }
                "GET" => {
                    // One file done.
                    content_to_transfer -= 1;
                    // Transfer the blob.
                    let file_content = path.strip_prefix("/blobs/").unwrap();
                    assert_eq!(body, b"");
                    stream
                        .write_all(
                            format!(
                                r#"HTTP/1.1 200 OK
Content-Type: application/octet-stream
Content-Length: {}
Connection: close

{file_content}"#,
                                file_content.len(),
                            )
                            .as_bytes(),
                        )
                        .unwrap();
                    drop(stream);
                }
                _ => panic!("Unexpected HTTP request: {method} {path}"),
            }
        }
        lfs_server_done_tx.send(()).unwrap();
        listener
    });

    git_command_for_testing(monorepo)
        .apply_git_toprepo_path()
        .args(["reset", "--hard"])
        .timeout(Duration::from_secs(2))
        .assert()
        .success()
        .stderr(if with_filter_process {
            // Git LFS does not print per-file messages for delayed smudging.
            ""
        } else {
            "\
Downloading large-file.txt (9 B)\n\
Downloading subpathx/large-file.txt (11 B)\n\
Downloading subpathx/subpathy/large-file.txt (11 B)\n\
"
        });

    lfs_server_done_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    let listener = lfs_server_stub_thread.join().unwrap();

    let lf_path_root = monorepo.join("large-file.txt");
    let lf_path_x = monorepo.join("subpathx/large-file.txt");
    let lf_path_y = monorepo.join("subpathx/subpathy/large-file.txt");
    assert_eq!(&read_to_string(&lf_path_root).unwrap(), "large-top");
    assert_eq!(&read_to_string(&lf_path_x).unwrap(), "large-repox");
    assert_eq!(&read_to_string(&lf_path_y).unwrap(), "large-repoy");

    std::fs::write(&lf_path_root, "new-top").unwrap();
    std::fs::write(&lf_path_y, "new-repoy").unwrap();
    git_command_for_testing(monorepo)
        .apply_git_toprepo_path()
        .arg("add")
        .arg(&lf_path_root)
        .arg(&lf_path_y)
        .assert()
        .success()
        .stdout("")
        .stderr("");
    git_command_for_testing(monorepo)
        .apply_git_toprepo_path()
        .args(["commit", "-m", "Update\n\nTopic: update"])
        .assert()
        .success()
        .stderr("");
    git_command_for_testing(monorepo)
        .args(["show", "HEAD:large-file.txt"])
        .assert()
        .success()
        .stdout(predicates::str::contains("\nsize 7\n"))
        .stderr("");
    git_command_for_testing(monorepo)
        .args(["show", "HEAD:subpathx/subpathy/large-file.txt"])
        .assert()
        .success()
        .stdout(predicates::str::contains("\nsize 9\n"))
        .stderr("");

    // Start our Git LFS server stub. Detach the thread in case of failure where
    // the thread might hang.
    let (lfs_server_done_tx, lfs_server_done_rx) = std::sync::mpsc::channel();
    let temp_dir_copy = temp_dir.to_owned();
    let lfs_server_stub_thread = std::thread::spawn(move || {
        let temp_dir = temp_dir_copy.as_path();
        let mut uploads = HashSet::new();
        let mut pushes_remaining = 2;
        while pushes_remaining > 0 {
            let (mut stream, method, path, header, body) = accept_http_request(&listener);
            match path.as_str() {
                "/git/top.git/info/lfs/locks/verify" | "/git/repoy.git/info/lfs/locks/verify" => {
                    assert_eq!(method, "POST");
                    assert_eq!(
                        body.to_str().unwrap(),
                        "{\"ref\":{\"name\":\"refs/heads/updated\"}}"
                    );
                    stream
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                        .unwrap();
                }
                "/git/top.git/info/lfs/objects/batch" | "/git/repoy.git/info/lfs/objects/batch" => {
                    assert_eq!(method, "POST");
                    let request = serde_json::from_slice::<LfsBatchRequest>(&body).unwrap();
                    assert_eq!(request.operation, "upload");
                    assert_eq!(request.hash_algo, "sha256");
                    // Extract the repo name.
                    let repo = path[5..].split_once(".git/").unwrap().0;
                    let response = serde_json::to_vec(&serde_json::json!({
                        "transfer": "basic",
                        "objects": request.objects.iter().map(|object| {
                            let small_file = format!("small-{repo}");
                            if object.oid == hex::encode(sha2::Sha256::digest(small_file.as_bytes())) {
                                // Skipping the upload action for the existing "small" file.
                                serde_json::json!({
                                    "oid": object.oid,
                                    "size": object.size,
                                })
                            } else {
                                serde_json::json!({
                                    "oid": object.oid,
                                    "size": object.size,
                                    "actions": &serde_json::json!({
                                        "upload": {
                                            "href": format!(
                                                "http://localhost:{fake_lfs_server_port}/uploads/{repo}/{}", object.oid,
                                            ),
                                        },
                                    }),
                                })
                            }
                        }).collect_vec(),
                        "hash_algo": "sha256",
                    }))
                    .unwrap();
                    stream
                        .write_all(format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.git-lfs+json\r\nContent-Length: {}\r\n\r\n",
                            response.len(),
                        ).as_bytes())
                        .unwrap();
                    stream.write_all(&response).unwrap();
                }
                path if let Some(sub_path) = path.strip_prefix("/uploads/") => {
                    assert_eq!(method, "PUT");
                    let (repo, digest) = sub_path.split_once('/').unwrap();
                    let body = String::from_utf8(body).unwrap();
                    assert_eq!(hex::encode(sha2::Sha256::digest(&body)), digest);
                    assert!(uploads.insert((repo.to_string(), body)));
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                        .unwrap();
                }
                path => {
                    // Assume normal Git server request.
                    eprintln!("HTTP request {method} {path}");
                    eprintln!("{}", body.to_str_lossy());
                    let (path, query) = if let Some(idx) = path.find('?') {
                        (&path[..idx], &path[idx + 1..])
                    } else {
                        (path, "")
                    };
                    let result = git_command_for_testing(temp_dir)
                        .env("GIT_HTTP_EXPORT_ALL", "")
                        .env("REQUEST_METHOD", &method)
                        .env(
                            "PATH_TRANSLATED",
                            temp_dir.join(path.strip_prefix("/git/").unwrap()),
                        )
                        .env("QUERY_STRING", query)
                        .env(
                            "CONTENT_TYPE",
                            header
                                .lines()
                                .find_map(|line| line.strip_prefix("Content-Type: "))
                                .unwrap_or(""),
                        )
                        .env("CONTENT_LENGTH", body.len().to_string())
                        .env("REMOTE_USER", "user")
                        .env("REMOTE_ADDR", "addr")
                        .arg("http-backend")
                        .write_stdin(body)
                        .assert()
                        .success()
                        .stderr("");
                    let response_content = &result.get_output().stdout;
                    eprintln!("HTTP response");
                    eprintln!("{}", response_content.to_str_lossy());
                    stream.write_all(b"HTTP/1.1 200 OK\r\n").unwrap();
                    stream.write_all(response_content).unwrap();
                    if method == "POST" && path.ends_with("/git-receive-pack") {
                        pushes_remaining -= 1;
                    }
                }
            }
        }
        assert_eq!(
            uploads,
            HashSet::from([
                ("top".to_owned(), "new-top".to_owned()),
                ("top".to_owned(), "large-top".to_owned()),
                ("repoy".to_owned(), "new-repoy".to_owned()),
                ("repoy".to_owned(), "large-repoy".to_owned()),
            ]),
        );
        lfs_server_done_tx.send(()).unwrap();
    });

    cargo_bin_git_toprepo_for_testing()
        .current_dir(monorepo)
        .args(["push", "origin", "HEAD:refs/heads/updated"])
        .assert()
        .success()
        .stdout("");

    lfs_server_done_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    lfs_server_stub_thread.join().unwrap();
}
