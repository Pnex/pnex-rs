//! `pnex-agent install`: pins the server CA by fingerprint (trust on first
//! use), exchanges the single-use enrollment code for the device
//! credentials, writes the configuration and installs the OS service.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};

use crate::config::{self, Config, Secrets, CA_FILE, CONFIG_FILE, SECRETS_FILE};

pub struct InstallArgs {
    pub server: String,
    pub enroll: String,
    pub ca_sha256: Option<String>,
    pub listen: Option<String>,
    pub allow: Vec<String>,
    pub no_service: bool,
    pub user_mode: bool,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Downloads `/api/v1/meta/ca` WITHOUT verification, then checks its
/// SHA-256 against the fingerprint printed by the PNeX UI.
async fn fetch_pinned_ca(server: &str, expected: &str) -> Result<String> {
    let insecure = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let bytes = insecure
        .get(format!("{server}/api/v1/meta/ca"))
        .send()
        .await
        .context("cannot reach the server")?
        .error_for_status()
        .context("the server has no local CA to pin")?
        .bytes()
        .await?;
    let got = sha256_hex(&bytes);
    if !got.eq_ignore_ascii_case(expected.trim()) {
        return Err(anyhow!(
            "CA fingerprint mismatch: expected {expected}, got {got} — wrong server or interception"
        ));
    }
    String::from_utf8(bytes.to_vec()).context("CA is not UTF-8 PEM")
}

fn client_for(ca_pem: Option<&str>) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20));
    if let Some(pem) = ca_pem {
        b = b
            .tls_built_in_root_certs(false)
            .add_root_certificate(reqwest::Certificate::from_pem(pem.as_bytes())?);
    }
    Ok(b.build()?)
}

async fn enroll(
    server: &str,
    code: &str,
    ca_pem: Option<&str>,
) -> Result<pnex_core::agent::AgentEnrollResponse> {
    let body = pnex_core::agent::AgentEnrollRequest {
        code: code.to_string(),
        hostname: hostname::get()
            .ok()
            .map(|h| h.to_string_lossy().into_owned()),
        os: Some(std::env::consts::OS.to_string()),
        arch: Some(std::env::consts::ARCH.to_string()),
        agent_version: Some(env!("CARGO_PKG_VERSION").to_string()),
    };
    let res = client_for(ca_pem)?
        .post(format!("{server}/api/v1/agent/enroll"))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            if ca_pem.is_none() && e.is_connect() {
                anyhow!(
                    "{e} — if the server uses its local CA, pass --ca-sha256 (printed in the UI)"
                )
            } else {
                anyhow!("{e}")
            }
        })?;
    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("description").or_else(|| v.get("detail")).cloned())
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or(text);
        return Err(anyhow!("enrollment refused ({status}): {detail}"));
    }
    Ok(res.json().await?)
}

pub async fn install(dir: &Path, args: InstallArgs) -> Result<()> {
    let server = args.server.trim().trim_end_matches('/').to_string();
    if !(server.starts_with("https://") || server.starts_with("http://")) {
        return Err(anyhow!(
            "--server must start with https:// (or http:// on a trusted LAN)"
        ));
    }
    let ca_pem = match args.ca_sha256.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(fp) => Some(fetch_pinned_ca(&server, fp).await?),
        None => None,
    };
    println!("Enrolling with {server} ...");
    let creds = enroll(&server, &args.enroll, ca_pem.as_deref()).await?;

    let cfg = Config {
        server: server.clone(),
        listen: args
            .listen
            .unwrap_or_else(|| pnex_core::agent::AGENT_DEFAULT_LISTEN.to_string()),
        allow: args.allow,
        max_queue_points: 5_000_000,
        max_age_secs: 7 * 24 * 3600,
    };
    cfg.listen_addr()?;
    cfg.allow_nets()?;
    config::ensure_dir(dir)?;
    std::fs::write(dir.join(CONFIG_FILE), toml::to_string_pretty(&cfg)?)?;
    let secrets = Secrets {
        device_id: creds.device_id.clone(),
        token: creds.token,
        encryption_key: creds.encryption_key,
        ws_path: creds.ws_path,
    };
    config::write_private(
        &dir.join(SECRETS_FILE),
        &serde_json::to_string_pretty(&secrets)?,
    )?;
    match &ca_pem {
        Some(pem) => std::fs::write(dir.join(CA_FILE), pem)?,
        None => {
            let _ = std::fs::remove_file(dir.join(CA_FILE));
        }
    }
    println!(
        "Enrolled as `{}` — configuration in {}",
        creds.device_id,
        dir.display()
    );

    if args.no_service {
        println!("Service not installed (--no-service). Start in foreground with:");
        println!("  pnex-agent run --dir \"{}\"", dir.display());
    } else {
        crate::service::install(dir, args.user_mode)?;
        println!(
            "Service `{}` installed and started.",
            crate::service::SERVICE_NAME
        );
    }
    let local = cfg.listen.replace("0.0.0.0", "127.0.0.1");
    println!();
    println!("Push a value:");
    println!("  curl -X POST http://{local}/v1/points -H 'content-type: application/json' \\");
    println!("       -d '{{\"key\":\"temperature\",\"value\":21.5,\"unit\":\"°C\"}}'");
    println!("Status: pnex-agent status");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            super::sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
