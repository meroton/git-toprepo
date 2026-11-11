use std::path::Path;
use std::process::Command;
use anyhow::{Context, Result, anyhow};
use command_error::CommandExt;
use reqwest::blocking::Client;
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Discover HTTP REST host from Gerrit SSH host.
pub fn http_host(ssh_host: &gix::Url) -> Result<gix::Url> {
    let host = ssh_host.host().context("SSH URL missing host")?;
    let mut cmd = Command::new("ssh");
    if let Some(port) = ssh_host.port {
        cmd.args(["-p", &port.to_string()]);
    }
    let target = match ssh_host.user() {
        Some(user) => format!("{user}@{host}"),
        None => host.to_string(),
    };
    // There is no official api to discover the http endpoint over ssh.
    // Therefore we query gerrit for any change. Which includes the host as part
    // of the change url.
    cmd.args([
        &target,
        "gerrit",
        "query",
        "--format=JSON",
        "limit:1",
    ]);

    let output = cmd.output_checked_utf8()
        .context("Failed to run ssh gerrit query to discover HTTP URL")?;

    let url = parse_query_url(&output.stdout)?;
    let url = url.as_str();

    if !url.starts_with("http") {
        anyhow::bail!("Gerrit query returned non-HTTP url: {url}");
    }
    let url = url.split('/').take(3).collect::<Vec<&str>>().join("/");

    let server = gix::Url::try_from(url.as_str())
        .map_err(|e| anyhow!("Failed to parse HTTP URL: {e}"))?;
    Ok(server)
}

/// One row of `gerrit query --format=JSON` output.
///
/// Each line is a JSON object: a change, or a trailing
/// `{"type":"stats",...}` row, or `{"type":"error","message":...}`.
#[derive(Deserialize)]
struct QueryRow {
    #[serde(rename = "type")]
    row_type: Option<String>,
    message: Option<String>,
    url: Option<String>,
}

/// Extract the url of the first change from `gerrit query --format=JSON` output.
///
/// ```
/// use git_toprepo::gerrit::parse_query_url;
///
/// // A change row followed by the stats row.
/// let stdout = r#"{"project":"a/b","number":1,"url":"https://gerrit.example/c/a/b/+/1"}
/// {"type":"stats","rowCount":1,"runTimeMilliseconds":3,"moreChanges":true}
/// "#;
/// assert_eq!(
///     parse_query_url(stdout).unwrap(),
///     "https://gerrit.example/c/a/b/+/1"
/// );
///
/// // No changes.
/// let stdout = r#"{"type":"stats","rowCount":0,"runTimeMilliseconds":1,"moreChanges":false}"#;
/// assert!(parse_query_url(stdout).is_err());
///
/// // Gerrit omits `url` when canonicalWebUrl is not configured.
/// let stdout = r#"{"project":"a/b","number":1}
/// {"type":"stats","rowCount":1}"#;
/// assert!(parse_query_url(stdout).is_err());
///
/// // Error rows are reported.
/// let stdout = r#"{"type":"error","message":"permission denied"}"#;
/// let err = parse_query_url(stdout).unwrap_err();
/// assert!(err.to_string().contains("permission denied"));
///
/// // Unknown row types are rejected.
/// let stdout = r#"{"type":"surprise","url":"https://gerrit.example/c/a/b/+/1"}"#;
/// let err = parse_query_url(stdout).unwrap_err();
/// assert!(err.to_string().contains("surprise"));
///
/// // The human readable text format is rejected.
/// assert!(parse_query_url("  url: https://gerrit.example/c/1").is_err());
/// ```
pub fn parse_query_url(stdout: &str) -> Result<String> {
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let row: QueryRow = serde_json::from_str(line)
            .with_context(|| format!("Bad JSON from gerrit query: {line:?}"))?;
        match row.row_type.as_deref() {
            // Change rows have no "type" field.
            None => {
                if let Some(url) = row.url {
                    return Ok(url);
                }
            }
            Some("stats") => {}
            Some("error") => anyhow::bail!(
                "gerrit query failed: {}",
                row.message.as_deref().unwrap_or("unknown error")
            ),
            Some(other) => {
                anyhow::bail!("Unexpected row type {other:?} in gerrit query output: {line:?}")
            }
        }
    }
    anyhow::bail!(
        "No change with a url in the gerrit query output, \
        are there any visible changes and is gerrit.canonicalWebUrl set?"
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GerritServer {
    pub ssh_url: gix::Url,
    pub http_url: gix::Url,
}

impl GerritServer {
    pub fn from_ssh_remote(ssh_url: gix::Url) -> Result<Self> {
        let http_url = http_host(&ssh_url)?;
        Ok(Self { ssh_url, http_url })
    }

    pub fn netrc_host(&self) -> Result<&str> {
        self.http_url.host().context("Gerrit HTTP URL missing host")
    }

    pub fn api_url(&self, endpoint: &str) -> String {
        let base = self.http_url.to_string();
        format!("{}/a/{}", base.trim_end_matches('/'), endpoint.trim_start_matches('/'))
    }

    pub fn project_remote_url(&self, project: &str) -> gix::Url {
        let project = project.strip_suffix(".git").unwrap_or(project);
        let mut url = self.ssh_url.clone();
        url.path = format!("/{project}.git").into();
        url
    }

    fn ssh_target(&self) -> Result<String> {
        let host = self.ssh_url.host().context("SSH URL missing host")?;
        Ok(match self.ssh_url.user() {
            Some(user) => format!("{user}@{host}"),
            None => host.to_string(),
        })
    }

    fn apply_ssh_port(&self, cmd: &mut Command) {
        if let Some(port) = self.ssh_url.port {
            cmd.args(["-p", &port.to_string()]);
        }
    }
}

/// A Gerrit change summary used for submitted_together and checkout.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct GerritChange {
    /// Qualified change ID: "project~branch~Change-Id"
    pub id: String,
    pub project: String,
    pub branch: String,
    pub change_id: String,
    pub topic: Option<String>,
    pub subject: Option<String>,
    #[serde(default, rename = "_number")]
    pub number: u64,
    pub current_revision: Option<String>,
}

impl GerritChange {
    pub fn triplet_id(&self) -> &str {
        &self.id
    }
}

#[derive(Deserialize, Debug)]
struct SubmittedTogetherResponse {
    pub changes: Vec<GerritChange>,
    #[serde(default)]
    #[allow(dead_code)]
    pub non_visible_changes: u32,
}

/// An ephemeral access token created via SSH and deleted on Drop or explicit `revoke()`.
pub struct EphemeralToken {
    pub server: GerritServer,
    pub name: String,
    pub username: String,
    pub secret: String,
    active: bool,
}

impl EphemeralToken {
    pub fn new(server: GerritServer, name: String, username: String, secret: String) -> Self {
        Self {
            server,
            name,
            username,
            secret,
            active: true,
        }
    }

    pub fn revoke(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;

        let target = self.server.ssh_target()?;
        let mut cmd = Command::new("ssh");
        self.server.apply_ssh_port(&mut cmd);
        let output = cmd
            .args([&target, "gerrit", "set-account", "--delete-token", &self.name, "self"])
            .output()
            .context("Failed to execute ssh to delete Gerrit token")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Gerrit failed to delete token '{}': {}", self.name, stderr.trim());
        }
        Ok(())
    }
}

impl Drop for EphemeralToken {
    fn drop(&mut self) {
        if self.active {
            let _ = self.revoke();
        }
    }
}

pub enum Credentials {
    Static {
        username: String,
        secret: String,
    },
    Ephemeral(EphemeralToken),
}

fn path_hash(path: &Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let hex_hash = hex::encode(hasher.finalize());
    hex_hash[..8].to_string()
}

/// Query OpenSSH's evaluated configuration for the host (local only, no network).
fn ssh_configured_user(host: &str) -> Option<String> {
    let output = Command::new("ssh").args(["-G", host]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("user "))
        .map(|user| user.trim().to_string())
}

impl Credentials {
    pub fn resolve(server: &GerritServer, repo_dir: &Path, username_override: Option<&str>) -> Result<Self> {
        // 1. Check ~/.netrc
        if let Ok(netrc) = netrc::Netrc::new() {
            let host = server.netrc_host()?;
            let auth = netrc.hosts.get(host).or_else(|| {
                server.http_url.port.and_then(|p| netrc.hosts.get(&format!("{host}:{p}")))
            });

            if let Some(auth) = auth {
                let username = if !auth.login.is_empty() {
                    auth.login.clone()
                } else if let Some(u) = username_override {
                    u.to_string()
                } else {
                    auth.login.clone()
                };

                return Ok(Self::Static {
                    username,
                    secret: auth.password.clone(),
                });
            }
        }

        // 2. Ephemeral token via SSH
        let token_name = format!("git-toprepo-{}", path_hash(repo_dir));

        let username = username_override
            .map(|u| u.to_string())
            .or_else(|| server.ssh_url.user().map(|u| u.to_string()))
            .or_else(|| server.ssh_url.host().and_then(ssh_configured_user))
            .context("Could not determine username for Gerrit authentication from netrc, remote URL, or ssh config")?;

        let target = server.ssh_target()?;

        // Clean up any stale token with the same name from a previous run
        let mut del_cmd = Command::new("ssh");
        server.apply_ssh_port(&mut del_cmd);
        let _ = del_cmd.args([&target, "gerrit", "set-account", "--delete-token", &token_name, "self"]).output();

        // Generate new token
        let mut gen_cmd = Command::new("ssh");
        server.apply_ssh_port(&mut gen_cmd);
        let output = gen_cmd
            .args([&target, "gerrit", "set-account", "--generate-token", &token_name, "self"])
            .output_checked_utf8()
            .context("Failed to generate ephemeral Gerrit access token via SSH")?;

        let secret = output.stdout
            .split_whitespace()
            .last()
            .context("Could not parse token secret from Gerrit SSH output")?
            .to_string();

        let token = EphemeralToken::new(server.clone(), token_name, username, secret);
        Ok(Self::Ephemeral(token))
    }

    pub fn username(&self) -> &str {
        match self {
            Self::Static { username, .. } => username,
            Self::Ephemeral(token) => &token.username,
        }
    }

    pub fn password(&self) -> &str {
        match self {
            Self::Static { secret, .. } => secret,
            Self::Ephemeral(token) => &token.secret,
        }
    }

    pub fn take_ephemeral(&mut self) -> Option<EphemeralToken> {
        match std::mem::replace(self, Self::Static { username: String::new(), secret: String::new() }) {
            Self::Ephemeral(token) => Some(token),
            _ => None,
        }
    }
}

pub struct GerritClient {
    pub server: GerritServer,
    http_client: Client,
    credentials: Credentials,
}

impl GerritClient {
    pub fn connect(ssh_remote: gix::Url, repo_dir: &Path, username: Option<&str>) -> Result<Self> {
        let server = GerritServer::from_ssh_remote(ssh_remote)?;
        let credentials = Credentials::resolve(&server, repo_dir, username)?;
        Ok(Self {
            server,
            http_client: Client::builder().build()?,
            credentials,
        })
    }

    fn request<T: DeserializeOwned>(&self, method: Method, url: &str) -> Result<T> {
        let res = self.http_client
            .request(method.clone(), url)
            .basic_auth(self.credentials.username(), Some(self.credentials.password()))
            .send()
            .with_context(|| format!("Failed HTTP {method} request to {url}"))?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().unwrap_or_default();
            anyhow::bail!("Gerrit HTTP error {status} for {url}: {body}");
        }

        let text = res.text()?;
        let json_text = text.strip_prefix(")]}'\n").unwrap_or(&text);

        serde_json::from_str(json_text)
            .with_context(|| format!("Failed to parse Gerrit JSON response from {url}"))
    }

    pub fn get_submitted_together(&self, change_id: &str) -> Result<Vec<GerritChange>> {
        let url = self.server.api_url(&format!(
            "changes/{change_id}/submitted_together?o=CURRENT_REVISION&o=DETAILED_ACCOUNTS&o=TOPIC_CLOSURE"
        ));
        let resp: SubmittedTogetherResponse = self.request(Method::GET, &url)?;

        if resp.changes.is_empty() {
            let change = self.get_change(change_id)?;
            return Ok(vec![change]);
        }

        Ok(resp.changes)
    }

    pub fn get_change(&self, change_id: &str) -> Result<GerritChange> {
        let url = self.server.api_url(&format!("changes/{change_id}?o=CURRENT_REVISION"));
        self.request(Method::GET, &url)
    }

    pub fn close(mut self) -> Result<()> {
        if let Some(mut token) = self.credentials.take_ephemeral() {
            token.revoke()?;
        }
        Ok(())
    }
}
