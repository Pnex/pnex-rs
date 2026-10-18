//! Sign-in once (D252): browser code + PKCE flow through the instance's own
//! `/api/v1/oauth2/sso` and `/token` proxies, loopback callback, then a
//! stored session refreshed silently. Only `pnex logout` or 48 h without
//! any use brings the browser back.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Loopback callback registered on the Rauthy `pnex` client for MCP clients
/// (D250, `deploy/edge/apply-edge.sh`): the CLI reuses it.
const CALLBACK_PORT: u16 = 33418;
const REDIRECT_URI: &str = "http://localhost:33418/callback";
/// Rauthy only accepts a refresh in the access token's last minute.
const REFRESH_WINDOW: u64 = 60;

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub url: String,
    pub org: Option<i64>,
    access_token: String,
    refresh_token: String,
    /// Unix seconds.
    expires_at: u64,
}

fn path() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("no config directory (set XDG_CONFIG_HOME)")?;
    Ok(base.join("pnex").join("session.json"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Session {
    pub fn load() -> Result<Self> {
        let raw = std::fs::read(path()?)
            .map_err(|_| anyhow::anyhow!("not signed in: run `pnex login <instance url>`"))?;
        serde_json::from_slice(&raw).context("corrupt session file: run `pnex login` again")
    }

    pub fn save(&self) -> Result<()> {
        let path = path()?;
        std::fs::create_dir_all(path.parent().expect("has parent"))?;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        // The file holds the refresh token: owner only.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut file = opts.open(&path)?;
        // `mode` only applies on creation: tighten a pre-existing file too.
        #[cfg(unix)]
        file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn logout() -> Result<bool> {
        match std::fs::remove_file(path()?) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// A valid access token, refreshed (and saved) when it is about to expire.
    pub fn token(&mut self, http: &Client) -> Result<&str> {
        if self.expires_at.saturating_sub(now()) < REFRESH_WINDOW {
            let body = json!({"grant_type": "refresh_token", "refresh_token": self.refresh_token});
            match exchange(http, &self.url, &body) {
                Ok(tokens) => {
                    self.set_tokens(&tokens)?;
                    self.save()?;
                }
                // Refresh not open yet (clock skew): the access token still works.
                Err(_) if self.expires_at > now() => {}
                Err(e) => bail!("session expired ({e}): run `pnex login {}`", self.url),
            }
        }
        Ok(&self.access_token)
    }

    fn set_tokens(&mut self, tokens: &Value) -> Result<()> {
        let field = |k: &str| {
            tokens[k]
                .as_str()
                .map(str::to_owned)
                .with_context(|| format!("token answer without {k}"))
        };
        self.access_token = field("access_token")?;
        self.refresh_token = field("refresh_token")?;
        self.expires_at = now() + tokens["expires_in"].as_u64().unwrap_or(0);
        Ok(())
    }
}

fn exchange(http: &Client, url: &str, body: &Value) -> Result<Value> {
    let res = http
        .post(format!("{url}/api/v1/oauth2/token"))
        .json(body)
        .send()?;
    let status = res.status();
    let body: Value = res.json().unwrap_or(Value::Null);
    if !status.is_success() {
        bail!("HTTP {status}: {body}");
    }
    Ok(body)
}

fn random_b64(len: usize) -> String {
    let mut buf = vec![0u8; len];
    getrandom::getrandom(&mut buf).expect("os random");
    B64.encode(buf)
}

fn challenge(verifier: &str) -> String {
    B64.encode(Sha256::digest(verifier.as_bytes()))
}

/// `code` of a `GET /callback?code=…&state=…` request line, once `state`
/// matches; `Ok(None)` for any other path (favicon…).
fn callback_code(request_line: &str, state: &str) -> Result<Option<String>> {
    let target = request_line.split_whitespace().nth(1).unwrap_or("");
    let Ok(url) = reqwest::Url::parse(&format!("http://localhost{target}")) else {
        return Ok(None);
    };
    if url.path() != "/callback" {
        return Ok(None);
    }
    let param = |k: &str| {
        url.query_pairs()
            .find(|(name, _)| name == k)
            .map(|(_, v)| v.into_owned())
    };
    if let Some(err) = param("error") {
        bail!("sign-in refused: {err}");
    }
    if param("state").as_deref() != Some(state) {
        bail!("sign-in callback with a foreign state, ignored");
    }
    param("code").context("callback without code").map(Some)
}

/// Browser sign-in against `url`, then stores the session.
pub fn login(http: &Client, url: &str) -> Result<Session> {
    let url = url.trim_end_matches('/').to_string();
    let info: pnex_api_contract::ServerInfo = http
        .get(format!("{url}{}", pnex_api_contract::META_VERSION_PATH))
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json())
        .with_context(|| format!("{url} is not a reachable PNeX server"))?;
    if info.service != pnex_api_contract::SERVICE {
        bail!("{url} answers as `{}`, not a PNeX server", info.service);
    }
    if !pnex_api_contract::compatible(info.contract, pnex_api_contract::CONTRACT) {
        bail!(
            "{url} serves API contract {} (server {}), this pnex speaks {}: install the pnex CLI of the server's release",
            info.contract,
            info.version,
            pnex_api_contract::CONTRACT
        );
    }

    let listener = TcpListener::bind(("127.0.0.1", CALLBACK_PORT))
        .with_context(|| format!("port {CALLBACK_PORT} busy (another sign-in running?)"))?;
    let verifier = random_b64(32);
    let state = random_b64(16);
    let mut authorize = reqwest::Url::parse(&format!("{url}/api/v1/oauth2/sso"))?;
    authorize
        .query_pairs_mut()
        .append_pair("code_challenge", &challenge(&verifier))
        .append_pair("code_challenge_method", "S256")
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("state", &state);
    eprintln!("Opening the browser to sign in. If nothing opens, visit:\n  {authorize}");
    let _ = webbrowser::open(authorize.as_str());

    let code = loop {
        let (mut stream, _) = listener.accept()?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line)?;
        let (status, page, outcome) = match callback_code(&line, &state) {
            Ok(Some(code)) => (
                "200 OK",
                "Signed in to PNeX, you can close this tab.",
                Some(Ok(code)),
            ),
            Ok(None) => ("404 Not Found", "", None),
            Err(e) => (
                "400 Bad Request",
                "Sign-in failed, see the terminal.",
                Some(Err(e)),
            ),
        };
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
            page.len()
        );
        if let Some(outcome) = outcome {
            break outcome?;
        }
    };

    let tokens = exchange(
        http,
        &url,
        &json!({
            "grant_type": "authorization_code",
            "code": code,
            "code_verifier": verifier,
            "redirect_uri": REDIRECT_URI,
        }),
    )?;
    let mut session = Session {
        url,
        org: None,
        access_token: String::new(),
        refresh_token: String::new(),
        expires_at: 0,
    };
    session.set_tokens(&tokens)?;
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc7636() {
        // RFC 7636 appendix B.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn callback_checks_state() {
        let ok = callback_code("GET /callback?code=a%2Fb&state=s1 HTTP/1.1", "s1").unwrap();
        assert_eq!(ok.as_deref(), Some("a/b"));
        assert!(callback_code("GET /callback?code=a&state=evil HTTP/1.1", "s1").is_err());
        assert!(callback_code("GET /callback?error=access_denied HTTP/1.1", "s1").is_err());
        assert!(callback_code("GET /favicon.ico HTTP/1.1", "s1")
            .unwrap()
            .is_none());
    }
}
