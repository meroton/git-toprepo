use std::process::Command;
use command_error::CommandExt;

/// Gerrit code review.

pub fn http_host(ssh_host: &gix::Url) -> String {
    let host = ssh_host.host().unwrap();
    let cmd = Command::new("ssh")
        .args([
              host,
              "gerrit",
              "query",
              "limit:1",
        ]).output_checked_utf8()
        .expect("Failed to launch process");

    let url = cmd.stdout.lines().into_iter()
        .map(|s| s.strip_prefix("  url: "))
        .find(|o| o.is_some()).flatten();
    url.unwrap().split('/').collect::<Vec<&str>>()[2].to_owned()
}
