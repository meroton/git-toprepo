use std::process::Command;
use command_error::CommandExt;

/// Gerrit code review.

/// The base server of an url or endpoint.
pub fn server(url: &gix::Url) -> gix::Url {
    return gix::Url::from_parts(
        url.scheme.clone(),
        url.user().map(|s| s.to_owned()),
        url.password().map(|s| s.to_owned()),
        url.host().map(|s| s.to_owned()),
        url.port,
        "/".into(), // Remove the path.
        false, // NB: We don't have a getter from this so it is reset to the
               // default behavior.
    ).unwrap();
}

pub fn http_host(ssh_host: &gix::Url) -> gix::Url {
    let host = server(ssh_host);

    let cmd = Command::new("ssh")
        .args([
              &host.to_string(),
              "gerrit",
              "query",
              "limit:1",
        ]).output_checked_utf8()
        .expect("Failed to launch process");

    let url = cmd.stdout.lines().into_iter()
        .map(|s| s.strip_prefix("  url: "))
        .find(|o| o.is_some()).flatten();
    let url = url.unwrap();

    return server(&gix::Url::from_bytes(url.into()).unwrap());
}
