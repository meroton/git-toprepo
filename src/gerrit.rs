use std::process::Command;
use command_error::CommandExt;

/// Gerrit code review.

pub fn http_host(ssh_host: &gix::Url) -> gix::Url {
    let host = ssh_host.host().unwrap();
    let host = &match ssh_host.user() {
        Some(user) => format!("{user}@{host}"),
        None => host.to_string(),
    };
    let host = &match ssh_host.port {
        Some(port) => format!("ssh://{host}:{port}"),
        None => host.to_string(),
    };
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
    let url = url.unwrap();
    gix::Url::from_bytes(url.into()).unwrap()
}
