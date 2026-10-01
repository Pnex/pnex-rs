//! Uplink: drains the durable queue to `/ws/device` (same tunnel, framing
//! and ChaCha20 frames as the firmware), with a sliding window of in-flight
//! batches and purge on `BatchAck`. Reconnects forever with exponential
//! backoff + jitter; refused credentials park the link in `revoked`.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use pnex_core::frame::{decrypt_frame, encrypt_frame};
use pnex_core::{BatchPoint, DeviceMsg, ServerMsg};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::Message;

use crate::config::{Config, Secrets};
use crate::state::{Link, Shared};

/// Batches in flight before waiting for acknowledgements.
const WINDOW: usize = 4;
/// Plaintext budget of one batch frame.
const MAX_BATCH_BYTES: usize = 64 * 1024;
/// Keepalive (the server watchdog closes silent sessions after 45 s).
const PING_EVERY: Duration = Duration::from_secs(20);
/// An unacknowledged batch older than this restarts the session.
const ACK_TIMEOUT: Duration = Duration::from_secs(60);
/// Backoff bounds.
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Backoff when the server refuses the credentials.
const REVOKED_BACKOFF: Duration = Duration::from_secs(300);

/// Platform id announced as `board` (same ids as the download targets).
pub fn target_id() -> String {
    let arch = match std::env::consts::ARCH {
        "arm" => "armv7",
        other => other,
    };
    format!("{arch}-{}", std::env::consts::OS)
}

/// rustls client config: the pinned local CA only when given, public web
/// roots otherwise. ring provider (portable cross builds).
pub fn tls_config(ca_pem: Option<&str>) -> Result<Arc<rustls::ClientConfig>> {
    use rustls_pki_types::pem::PemObject;
    use rustls_pki_types::CertificateDer;
    let mut roots = rustls::RootCertStore::empty();
    match ca_pem {
        Some(pem) => {
            for cert in CertificateDer::pem_slice_iter(pem.as_bytes()) {
                roots
                    .add(cert.context("invalid CA PEM")?)
                    .context("CA rejected by rustls")?;
            }
            if roots.is_empty() {
                return Err(anyhow!("CA PEM holds no certificate"));
            }
        }
        None => roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned()),
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("TLS protocol setup")?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

enum End {
    /// Normal end (network, server restart…): reconnect with backoff.
    Retry(String),
    /// Credentials refused: long backoff, surfaced as `revoked`.
    Revoked(String),
}

fn close_end(code: CloseCode, reason: &str) -> End {
    let code = u16::from(code);
    match code {
        4001 | 4005 | 4006 | 4007 | 4008 => {
            End::Revoked(format!("server refused the credentials ({code} {reason})"))
        }
        4003 => End::Retry(format!("another agent uses these credentials ({code})")),
        _ => End::Retry(format!("closed by server ({code} {reason})")),
    }
}

pub async fn run(shared: Arc<Shared>, cfg: Config, secrets: Secrets, ca_pem: Option<String>) {
    let key = match pnex_core::frame::decode_key(&secrets.encryption_key) {
        Some(k) => k,
        None => {
            shared.set_link(
                Link::Revoked,
                Some("invalid encryption key in secrets.json".into()),
            );
            return;
        }
    };
    let url = match cfg.ws_base(&secrets.ws_path) {
        Ok(base) => format!(
            "{base}?token={}&device_id={}",
            STANDARD.encode(&secrets.token),
            STANDARD.encode(&secrets.device_id)
        ),
        Err(e) => {
            shared.set_link(Link::Revoked, Some(e.to_string()));
            return;
        }
    };
    let tls = match tls_config(ca_pem.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            shared.set_link(Link::Revoked, Some(e.to_string()));
            return;
        }
    };
    let mut backoff = BACKOFF_MIN;
    while !shared.shutting_down.load(Ordering::Relaxed) {
        shared.set_link(Link::Connecting, None);
        let connector = tokio_tungstenite::Connector::Rustls(tls.clone());
        let connected = tokio::time::timeout(
            Duration::from_secs(20),
            tokio_tungstenite::connect_async_tls_with_config(&url, None, false, Some(connector)),
        )
        .await;
        let end = match connected {
            Ok(Ok((ws, _))) => {
                tracing::info!("connected to {}", cfg.server);
                backoff = BACKOFF_MIN;
                session(&shared, ws, &key).await
            }
            Ok(Err(e)) => End::Retry(format!("connect failed: {e}")),
            Err(_) => End::Retry("connect timeout".into()),
        };
        let wait = match end {
            End::Retry(why) => {
                tracing::warn!("uplink down: {why}");
                shared.set_link(Link::Disconnected, Some(why));
                let jitter = Duration::from_millis(u64::from(rand_u16()) % 1000);
                let wait = backoff + jitter;
                backoff = (backoff * 2).min(BACKOFF_MAX);
                wait
            }
            End::Revoked(why) => {
                tracing::error!("uplink refused: {why} — reinstall with a new enrollment code");
                shared.set_link(Link::Revoked, Some(why));
                REVOKED_BACKOFF
            }
        };
        tokio::time::sleep(wait).await;
    }
}

fn rand_u16() -> u16 {
    let n = uuid::Uuid::new_v4();
    u16::from_le_bytes([n.as_bytes()[0], n.as_bytes()[1]])
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send_msg(
    sink: &mut futures_util::stream::SplitSink<Ws, Message>,
    key: &[u8; 32],
    msg: &DeviceMsg,
) -> Result<()> {
    let plain = serde_json::to_string(msg)?;
    sink.send(Message::Text(encrypt_frame(&plain, key).into()))
        .await
        .map_err(|e| anyhow!("send failed: {e}"))
}

async fn session(shared: &Arc<Shared>, ws: Ws, key: &[u8; 32]) -> End {
    let (mut sink, mut stream) = ws.split();
    let announce = DeviceMsg::Announce {
        chip: pnex_core::EDGE_AGENT_CHIP.into(),
        board: target_id(),
        fw: env!("CARGO_PKG_VERSION").into(),
        caps: None,
        pins: None,
    };
    if let Err(e) = send_msg(&mut sink, key, &announce).await {
        return End::Retry(e.to_string());
    }
    shared.set_link(Link::Connected, None);

    let epoch = shared.queue.epoch().to_string();
    // Unacknowledged points are resent after a reconnect (server dedups).
    let mut sent_up_to: u64 = shared.acked_up_to.load(Ordering::Relaxed);
    let mut inflight: VecDeque<(u64, Instant)> = VecDeque::new();
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    let mut poll = tokio::time::interval(Duration::from_millis(500));

    loop {
        // Fill the window.
        while inflight.len() < WINDOW {
            let limit = shared.max_batch.load(Ordering::Relaxed).max(1) as usize;
            let queue = shared.queue.clone();
            let after = sent_up_to;
            let rows = match tokio::task::spawn_blocking(move || {
                queue.read_after(after, limit, MAX_BATCH_BYTES)
            })
            .await
            {
                Ok(Ok(rows)) => rows,
                Ok(Err(e)) => return End::Retry(format!("queue read failed: {e}")),
                Err(e) => return End::Retry(format!("queue read panicked: {e}")),
            };
            let Some(last) = rows.last().map(|(s, _)| *s) else {
                break;
            };
            let n = rows.len() as u64;
            let points: Vec<BatchPoint> = rows
                .into_iter()
                .map(|(seq, p)| BatchPoint {
                    seq,
                    key: p.key,
                    value: p.value,
                    ts_ms: p.ts_ms,
                    unit: p.unit,
                    record: p.record,
                })
                .collect();
            let msg = DeviceMsg::Batch {
                epoch: epoch.clone(),
                points,
            };
            if let Err(e) = send_msg(&mut sink, key, &msg).await {
                return End::Retry(e.to_string());
            }
            shared.sent.fetch_add(n, Ordering::Relaxed);
            sent_up_to = last;
            inflight.push_back((last, Instant::now()));
        }
        if inflight
            .front()
            .is_some_and(|(_, at)| at.elapsed() > ACK_TIMEOUT)
        {
            return End::Retry("acknowledgement timeout".into());
        }

        tokio::select! {
            incoming = stream.next() => {
                let msg = match incoming {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => return End::Retry(format!("read failed: {e}")),
                    None => return End::Retry("connection closed".into()),
                };
                match msg {
                    Message::Text(text) => {
                        let Some(plain) = decrypt_frame(text.as_str(), key) else {
                            tracing::warn!("undecryptable server frame");
                            continue;
                        };
                        if plain.trim().eq_ignore_ascii_case("pong") {
                            continue;
                        }
                        match serde_json::from_str::<ServerMsg>(&plain) {
                            Ok(ServerMsg::BatchAck { epoch: e, up_to_seq }) if e == epoch => {
                                let queue = shared.queue.clone();
                                if let Ok(Err(err)) = tokio::task::spawn_blocking(move || queue.ack(up_to_seq)).await {
                                    tracing::warn!("queue purge failed: {err}");
                                }
                                shared.acked_up_to.fetch_max(up_to_seq, Ordering::Relaxed);
                                shared.last_ack_ms.store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
                                while inflight.front().is_some_and(|(s, _)| *s <= up_to_seq) {
                                    inflight.pop_front();
                                }
                            }
                            Ok(ServerMsg::AgentConfig { max_batch, max_keys }) => {
                                shared.max_batch.store(u64::from(max_batch.clamp(1, 5000)), Ordering::Relaxed);
                                shared.server_max_keys.store(u64::from(max_keys), Ordering::Relaxed);
                            }
                            Ok(ServerMsg::Reject { reason }) => {
                                return End::Retry(format!("rejected by server: {reason}"));
                            }
                            Ok(other) => tracing::debug!("ignored server message: {other:?}"),
                            Err(e) => tracing::debug!("unknown server message: {e}"),
                        }
                    }
                    Message::Close(frame) => {
                        return match frame {
                            Some(f) => close_end(f.code, &f.reason),
                            None => End::Retry("closed by server".into()),
                        };
                    }
                    _ => {}
                }
            }
            _ = ping.tick() => {
                if let Err(e) = sink.send(Message::Text(encrypt_frame("PING", key).into())).await {
                    return End::Retry(format!("ping failed: {e}"));
                }
            }
            _ = shared.wake.notified() => {}
            _ = poll.tick() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_codes_classification() {
        assert!(matches!(
            close_end(CloseCode::from(4005), ""),
            End::Revoked(_)
        ));
        assert!(matches!(
            close_end(CloseCode::from(4001), ""),
            End::Revoked(_)
        ));
        assert!(matches!(
            close_end(CloseCode::from(4003), ""),
            End::Retry(_)
        ));
        assert!(matches!(close_end(CloseCode::Normal, ""), End::Retry(_)));
    }

    #[test]
    fn tls_config_accepts_public_roots_and_rejects_empty_pem() {
        assert!(tls_config(None).is_ok());
        assert!(tls_config(Some("not a pem")).is_err());
    }

    #[test]
    fn target_id_shape() {
        let t = target_id();
        assert!(t.contains('-'), "{t}");
    }
}
