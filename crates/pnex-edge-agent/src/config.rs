//! Agent configuration: `config.toml` (non-secret settings) + `secrets.json`
//! (device credentials, owner-only permissions) + the pinned CA PEM, all in
//! one platform-specific directory.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

pub const CONFIG_FILE: &str = "config.toml";
pub const SECRETS_FILE: &str = "secrets.json";
pub const CA_FILE: &str = "ca.pem";
pub const QUEUE_FILE: &str = "queue.redb";

/// Non-secret settings (`config.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// PNeX server base URL (`https://pnex.local`).
    pub server: String,
    /// Local API bind address (loopback by default).
    #[serde(default = "default_listen")]
    pub listen: String,
    /// Extra client networks allowed on a non-loopback bind (CIDR). Empty =
    /// any client that can reach the bind address.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Durable queue bound (points); the oldest points are dropped beyond.
    #[serde(default = "default_max_queue_points")]
    pub max_queue_points: u64,
    /// Points older than this are dropped from the queue (seconds).
    #[serde(default = "default_max_age_secs")]
    pub max_age_secs: u64,
    /// Capture box (media-ingest.md D159, lot 6b): announce `media_capture`
    /// and capture the streams the server assigns to this agent. Linux
    /// only, needs ffmpeg (`PNEX_FFMPEG` or `ffmpeg` in `PATH`).
    #[serde(default)]
    pub media_capture: bool,
}

fn default_listen() -> String {
    pnex_core::agent::AGENT_DEFAULT_LISTEN.to_string()
}

fn default_max_queue_points() -> u64 {
    5_000_000
}

fn default_max_age_secs() -> u64 {
    7 * 24 * 3600
}

/// Credentials returned by the enrollment (`secrets.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Secrets {
    pub device_id: String,
    pub token: String,
    /// Base64 pre-shared key of the Noise link (D156).
    pub encryption_key: String,
    pub ws_path: String,
    /// TLS client certificate + key (PEM) issued by the org CA (D153).
    pub client_cert_pem: String,
    pub client_key_pem: String,
    /// Device endpoint (`host[:port]`, D158).
    pub device_host: String,
}

impl Config {
    pub fn listen_addr(&self) -> Result<SocketAddr> {
        self.listen
            .parse()
            .with_context(|| format!("invalid listen address `{}`", self.listen))
    }

    pub fn allow_nets(&self) -> Result<Vec<ipnet::IpNet>> {
        self.allow
            .iter()
            .map(|c| {
                c.parse::<ipnet::IpNet>()
                    .or_else(|_| c.parse::<std::net::IpAddr>().map(ipnet::IpNet::from))
                    .map_err(|_| anyhow!("invalid --allow network `{c}`"))
            })
            .collect()
    }

    /// Server URL, https only: the agent never talks to the server in
    /// clear (D154).
    pub fn https_server(&self) -> Result<&str> {
        let server = self.server.trim_end_matches('/');
        if server.starts_with("https://") {
            Ok(server)
        } else {
            Err(anyhow!("server URL must start with https://"))
        }
    }
}

/// Default directory: system-wide when running as root/Administrator,
/// per-user otherwise.
pub fn default_dir(user_mode: bool) -> PathBuf {
    #[cfg(windows)]
    {
        let _ = user_mode;
        let base = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into());
        PathBuf::from(base).join("PNeX").join("agent")
    }
    #[cfg(not(windows))]
    {
        if user_mode || !is_root() {
            let base = std::env::var("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
                        .join(".local")
                        .join("share")
                });
            base.join("pnex-agent")
        } else {
            PathBuf::from("/var/lib/pnex-agent")
        }
    }
}

#[cfg(unix)]
pub fn is_root() -> bool {
    // `id -u` without libc: the effective uid of this process.
    std::fs::metadata("/proc/self")
        .map(|m| {
            use std::os::unix::fs::MetadataExt;
            m.uid() == 0
        })
        .unwrap_or(false)
}

pub fn load(dir: &Path) -> Result<(Config, Secrets, Option<String>)> {
    let cfg_path = dir.join(CONFIG_FILE);
    let raw = std::fs::read_to_string(&cfg_path).with_context(|| {
        format!(
            "cannot read {} — run `pnex-agent install` first",
            cfg_path.display()
        )
    })?;
    let cfg: Config = toml::from_str(&raw).context("invalid config.toml")?;
    let secrets: Secrets = serde_json::from_str(
        &std::fs::read_to_string(dir.join(SECRETS_FILE)).context("cannot read secrets.json")?,
    )
    .context("invalid secrets.json")?;
    let ca = std::fs::read_to_string(dir.join(CA_FILE)).ok();
    Ok((cfg, secrets, ca))
}

/// Writes a file readable by its owner only (0600 on Unix; the directory
/// ACL protects it on Windows).
pub fn write_private(path: &Path, content: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("cannot write {}", path.display()))?;
        f.write_all(content.as_bytes())?;
        // Pre-existing file: enforce the mode anyway.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, content).with_context(|| format!("cannot write {}", path.display()))
    }
}

/// Creates the agent directory with restrictive permissions.
pub fn ensure_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    {
        // SYSTEM + Administrators only, no inherited ACEs (secrets inside).
        let status = std::process::Command::new("icacls")
            .arg(dir)
            .args([
                "/inheritance:r",
                "/grant:r",
                "*S-1-5-18:(OI)(CI)F",
                "/grant:r",
                "*S-1-5-32-544:(OI)(CI)F",
            ])
            .status();
        if !status.map(|s| s.success()).unwrap_or(false) {
            tracing::warn!(
                "icacls failed — {} keeps inherited permissions",
                dir.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(server: &str) -> Config {
        Config {
            server: server.into(),
            listen: default_listen(),
            allow: vec!["192.168.1.0/24".into(), "10.0.0.7".into()],
            max_queue_points: 10,
            max_age_secs: 10,
            media_capture: false,
        }
    }

    #[test]
    fn server_must_be_https() {
        assert_eq!(
            cfg("https://pnex.local/").https_server().unwrap(),
            "https://pnex.local"
        );
        assert!(cfg("http://10.0.0.2:5150").https_server().is_err());
        assert!(cfg("pnex.local").https_server().is_err());
    }

    #[test]
    fn allow_accepts_cidr_and_single_ip() {
        let nets = cfg("https://x").allow_nets().unwrap();
        assert!(nets[0].contains(&"192.168.1.40".parse::<std::net::IpAddr>().unwrap()));
        assert!(nets[1].contains(&"10.0.0.7".parse::<std::net::IpAddr>().unwrap()));
    }

    #[test]
    fn config_toml_defaults() {
        let c: Config = toml::from_str("server = \"https://h\"").unwrap();
        assert_eq!(c.listen, "127.0.0.1:7070");
        assert!(c.allow.is_empty());
        assert!(!c.media_capture, "capture box is opt-in");
    }
}
