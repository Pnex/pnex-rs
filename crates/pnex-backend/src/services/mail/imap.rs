//! IMAP client of the mailboxes (mail.md D458–D460): TLS from the first
//! byte (IMAPS), the server's certificate checked against the system CAs,
//! the host and every resolved address passed through the egress policy
//! (R8). Folders are opened read-only (`EXAMINE`) and bodies fetched with
//! `BODY.PEEK`: PneX never marks a mail read nor changes the mailbox.

use std::sync::Arc;
use std::time::Duration;

use futures_util::TryStreamExt;
use pnex_core::err_codes;
use pnex_core::mail::{MailboxError, FETCH_BATCH, RAW_MAX_BYTES};
use serde_json::json;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

pub type Session = async_imap::Session<TlsStream<TcpStream>>;

/// A refusal with its machine code (`mail-*`) and arguments.
pub fn error(code: &str, args: serde_json::Value) -> MailboxError {
    MailboxError {
        code: code.to_string(),
        args,
    }
}

/// An error that a new attempt will not fix (bad credentials, bad
/// certificate, unknown folder): the poller stops until the mailbox changes.
pub fn is_permanent(e: &MailboxError) -> bool {
    [
        err_codes::MAIL_AUTH_FAILED,
        err_codes::MAIL_TLS_FAILED,
        err_codes::MAIL_FOLDER_UNKNOWN,
        err_codes::MAIL_NO_PASSWORD,
    ]
    .contains(&e.code.as_str())
}

fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: std::sync::OnceLock<Arc<rustls::ClientConfig>> = std::sync::OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            for cert in rustls_native_certs::load_native_certs().certs {
                let _ = roots.add(cert);
            }
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            Arc::new(
                rustls::ClientConfig::builder_with_provider(provider)
                    .with_safe_default_protocol_versions()
                    .expect("ring supports the default TLS versions")
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            )
        })
        .clone()
}

async fn timed<T>(
    what: impl std::future::Future<Output = Result<T, async_imap::error::Error>>,
    code: &'static str,
) -> Result<T, MailboxError> {
    match tokio::time::timeout(COMMAND_TIMEOUT, what).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => {
            tracing::info!(error = %e, "imap command failed");
            Err(error(code, serde_json::Value::Null))
        }
        Err(_) => Err(error(
            err_codes::MAIL_UNREACHABLE,
            json!({"timeout_s": COMMAND_TIMEOUT.as_secs()}),
        )),
    }
}

/// Opens a logged-in session.
pub async fn connect(
    host: &str,
    port: u16,
    user: &str,
    password: &str,
) -> Result<Session, MailboxError> {
    let unreachable = || error(err_codes::MAIL_UNREACHABLE, json!({"host": host}));
    let policy = pnex_core::egress::policy();
    if pnex_core::egress::check_host(host, policy).is_some() {
        return Err(unreachable());
    }
    let addrs = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| unreachable())?
        .map_err(|_| unreachable())?;
    let allowed = pnex_core::egress::filter_addrs(addrs, policy);
    let Some(addr) = allowed.first() else {
        return Err(unreachable());
    };
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| unreachable())?
        .map_err(|_| unreachable())?;
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| error(err_codes::MAIL_TLS_FAILED, json!({"host": host})))?;
    let tls = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio_rustls::TlsConnector::from(tls_config()).connect(name, tcp),
    )
    .await
    .map_err(|_| unreachable())?
    .map_err(|e| {
        tracing::info!(host, error = %e, "imap tls failed");
        error(err_codes::MAIL_TLS_FAILED, json!({"host": host}))
    })?;
    let mut client = async_imap::Client::new(tls);
    match tokio::time::timeout(CONNECT_TIMEOUT, client.read_response()).await {
        Ok(Ok(Some(_))) => {}
        _ => return Err(error(err_codes::MAIL_PROTOCOL, json!({"host": host}))),
    }
    match tokio::time::timeout(CONNECT_TIMEOUT, client.login(user, password)).await {
        Ok(Ok(s)) => Ok(s),
        Ok(Err((e, _))) => {
            tracing::info!(host, error = %e, "imap login refused");
            Err(error(err_codes::MAIL_AUTH_FAILED, serde_json::Value::Null))
        }
        Err(_) => Err(unreachable()),
    }
}

/// A folder name sent to the server: printable, no quote, backslash nor
/// line break (async-imap quotes it; nothing else is spliced in).
pub fn folder_ok(folder: &str) -> bool {
    !folder.is_empty()
        && folder.len() <= pnex_core::mail::FOLDER_MAX
        && !folder
            .chars()
            .any(|c| c.is_control() || c == '"' || c == '\\')
}

/// `(UIDVALIDITY, messages)` of a folder opened read-only.
pub async fn examine(s: &mut Session, folder: &str) -> Result<(u32, u32), MailboxError> {
    if !folder_ok(folder) {
        return Err(error(
            err_codes::MAIL_FOLDER_UNKNOWN,
            json!({"folder": folder}),
        ));
    }
    let mb = match tokio::time::timeout(COMMAND_TIMEOUT, s.examine(folder)).await {
        Ok(Ok(mb)) => mb,
        Ok(Err(_)) => {
            return Err(error(
                err_codes::MAIL_FOLDER_UNKNOWN,
                json!({"folder": folder}),
            ))
        }
        Err(_) => return Err(error(err_codes::MAIL_UNREACHABLE, serde_json::Value::Null)),
    };
    Ok((mb.uid_validity.unwrap_or(0), mb.exists))
}

/// One message fetched: its UID and its raw bytes (headers only above
/// [`RAW_MAX_BYTES`], then `truncated`).
pub struct Fetched {
    pub uid: u32,
    pub raw: Vec<u8>,
    pub truncated: bool,
}

/// UIDs after `after` (or, without a cursor, received in the last `days`),
/// oldest first, at most [`FETCH_BATCH`].
pub async fn new_uids(
    s: &mut Session,
    after: Option<u32>,
    days: i64,
) -> Result<Vec<u32>, MailboxError> {
    let query = match after {
        Some(uid) => format!("UID {}:*", uid.saturating_add(1)),
        None => {
            let since = (chrono::Utc::now() - chrono::Duration::days(days)).format("%d-%b-%Y");
            format!("SINCE {since}")
        }
    };
    let found = timed(s.uid_search(&query), err_codes::MAIL_PROTOCOL).await?;
    let mut uids: Vec<u32> = found
        .into_iter()
        // `n:*` always answers the last message, even below `n`.
        .filter(|u| after.is_none_or(|a| *u > a))
        .collect();
    uids.sort_unstable();
    uids.truncate(FETCH_BATCH);
    Ok(uids)
}

/// Fetches `uids` without marking them read.
pub async fn fetch(s: &mut Session, uids: &[u32]) -> Result<Vec<Fetched>, MailboxError> {
    if uids.is_empty() {
        return Ok(Vec::new());
    }
    let set = uids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    // Sizes first: a message above the cap is fetched by its headers only.
    let sizes: Vec<(u32, u32)> = {
        let stream = timed(
            s.uid_fetch(&set, "(UID RFC822.SIZE)"),
            err_codes::MAIL_PROTOCOL,
        )
        .await?;
        let items: Vec<_> = timed(stream.try_collect::<Vec<_>>(), err_codes::MAIL_PROTOCOL).await?;
        items
            .iter()
            .filter_map(|f| Some((f.uid?, f.size.unwrap_or(0))))
            .collect()
    };
    let (big, small): (Vec<_>, Vec<_>) = sizes
        .into_iter()
        .partition(|(_, n)| *n as usize > RAW_MAX_BYTES);
    let mut out = Vec::new();
    for (list, query, truncated) in [
        (small, "(UID BODY.PEEK[])", false),
        (big, "(UID BODY.PEEK[HEADER])", true),
    ] {
        if list.is_empty() {
            continue;
        }
        let set = list
            .iter()
            .map(|(u, _)| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let stream = timed(s.uid_fetch(&set, query), err_codes::MAIL_PROTOCOL).await?;
        let items: Vec<_> = timed(stream.try_collect::<Vec<_>>(), err_codes::MAIL_PROTOCOL).await?;
        for f in &items {
            let (Some(uid), Some(raw)) = (f.uid, f.body().or_else(|| f.header())) else {
                continue;
            };
            out.push(Fetched {
                uid,
                raw: raw.to_vec(),
                truncated,
            });
        }
    }
    out.sort_by_key(|f| f.uid);
    Ok(out)
}

pub async fn logout(mut s: Session) {
    let _ = tokio::time::timeout(Duration::from_secs(5), s.logout()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_never_carry_a_command() {
        assert!(folder_ok("INBOX"));
        assert!(folder_ok("Newsletters/Veille"));
        assert!(folder_ok("Éléments envoyés"));
        assert!(!folder_ok("INBOX\" DELETE \"x"));
        assert!(!folder_ok("a\r\nb"));
        assert!(!folder_ok(""));
    }

    #[test]
    fn only_server_side_refusals_are_permanent() {
        assert!(is_permanent(&error(
            err_codes::MAIL_AUTH_FAILED,
            serde_json::Value::Null
        )));
        assert!(!is_permanent(&error(
            err_codes::MAIL_UNREACHABLE,
            serde_json::Value::Null
        )));
    }
}
