//! Uplink: drains the durable queue to `/ws/device` (same tunnel, framing
//! and Noise NNpsk0 link as the firmware, D156), with a sliding window of in-flight
//! batches and purge on `BatchAck`. Reconnects forever with exponential
//! backoff + jitter; refused credentials park the link in `revoked`.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use pnex_core::frame::{Initiator, Link as NoiseLink};
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
/// Max wait for the server's Noise answer after the first message.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
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
pub fn tls_config(ca_pem: Option<&str>, client: (&str, &str)) -> Result<Arc<rustls::ClientConfig>> {
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
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("TLS protocol setup")?
        .with_root_certificates(roots);
    // D153: the agent's org-CA certificate on the device endpoint.
    let (cert_pem, key_pem) = client;
    let chain = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .context("invalid client certificate PEM")?;
    let key = rustls_pki_types::PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
        .context("invalid client key PEM")?;
    let cfg = builder
        .with_client_auth_cert(chain, key)
        .context("client certificate rejected by rustls")?;
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
        4001 | 4005 | 4007 | 4011 | 4014 => {
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
    // The token travels in the Authorization header, never in the URL
    // (D154); the link goes to the device endpoint (D158).
    if let Err(e) = cfg.https_server() {
        shared.set_link(Link::Revoked, Some(e.to_string()));
        return;
    }
    let url = format!("wss://{}{}", secrets.device_host.trim(), secrets.ws_path);
    let client = (
        secrets.client_cert_pem.as_str(),
        secrets.client_key_pem.as_str(),
    );
    let tls = match tls_config(ca_pem.as_deref(), client) {
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
        let request = match authorized_request(&url, &secrets.token) {
            Ok(r) => r,
            Err(e) => {
                shared.set_link(Link::Revoked, Some(e));
                return;
            }
        };
        let connected = tokio::time::timeout(
            Duration::from_secs(20),
            tokio_tungstenite::connect_async_tls_with_config(request, None, false, Some(connector)),
        )
        .await;
        let end = match connected {
            Ok(Ok((ws, _))) => {
                tracing::info!("connected to {}", cfg.server);
                backoff = BACKOFF_MIN;
                session(&shared, ws, &key, &secrets.device_id).await
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
    codec: &mut NoiseLink,
    msg: &DeviceMsg,
) -> Result<()> {
    let plain = serde_json::to_string(msg)?;
    sink.send(Message::Text(codec.seal_text(&plain).into()))
        .await
        .map_err(|e| anyhow!("send failed: {e}"))
}

/// Noise handshake (D156): sends the first message, waits for the
/// server's answer and returns the established link.
async fn handshake(
    sink: &mut futures_util::stream::SplitSink<Ws, Message>,
    stream: &mut futures_util::stream::SplitStream<Ws>,
    key: &[u8; 32],
    device_id: &str,
) -> std::result::Result<NoiseLink, End> {
    let (init, msg1) = Initiator::start(key, device_id);
    sink.send(Message::Text(
        pnex_core::frame::encode_handshake(&msg1).into(),
    ))
    .await
    .map_err(|e| End::Retry(format!("send failed: {e}")))?;
    let wait = async {
        loop {
            match stream.next().await {
                Some(Ok(Message::Text(text))) => return Ok(text.to_string()),
                Some(Ok(Message::Close(frame))) => {
                    return Err(match frame {
                        Some(f) => close_end(f.code, &f.reason),
                        None => End::Retry("closed by server".into()),
                    });
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(End::Retry(format!("read failed: {e}"))),
                None => return Err(End::Retry("connection closed".into())),
            }
        }
    };
    let answer = tokio::time::timeout(HANDSHAKE_TIMEOUT, wait)
        .await
        .unwrap_or_else(|_| Err(End::Retry("no handshake answer from the server".into())))?;
    pnex_core::frame::decode_handshake(&answer)
        .and_then(|msg2| init.finish(&msg2))
        .ok_or_else(|| End::Revoked("Noise handshake failed: wrong key for this device".into()))
}

async fn session(shared: &Arc<Shared>, ws: Ws, key: &[u8; 32], device_id: &str) -> End {
    let (mut sink, mut stream) = ws.split();
    let mut codec = match handshake(&mut sink, &mut stream, key, device_id).await {
        Ok(link) => link,
        Err(end) => return end,
    };
    let codec = &mut codec;
    let announce = DeviceMsg::Announce {
        chip: pnex_core::EDGE_AGENT_CHIP.into(),
        board: target_id(),
        fw: env!("CARGO_PKG_VERSION").into(),
        caps: None,
        pins: None,
    };
    if let Err(e) = send_msg(&mut sink, codec, &announce).await {
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
            if let Err(e) = send_msg(&mut sink, codec, &msg).await {
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
                        let Some(plain) = codec.open_text(text.as_str()) else {
                            tracing::warn!("unauthenticated or replayed server frame");
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
                if let Err(e) = sink.send(Message::Text(codec.seal_text("PING").into())).await {
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
    fn tls_config_rejects_bad_pems() {
        assert!(tls_config(Some("not a pem"), ("x", "y")).is_err());
        assert!(tls_config(None, ("not a pem", "nor a key")).is_err());
    }

    #[test]
    fn target_id_shape() {
        let t = target_id();
        assert!(t.contains('-'), "{t}");
    }
}

/// Upgrade request carrying the device token as `Authorization: Bearer
/// <token>` (D154: never in the URL, so never in an access log).
fn authorized_request(
    url: &str,
    token: &str,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, String> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = url
        .into_client_request()
        .map_err(|e| format!("invalid server URL: {e}"))?;
    let value = format!("Bearer {token}")
        .parse()
        .map_err(|_| "device token is not a valid header value".to_string())?;
    request.headers_mut().insert("Authorization", value);
    Ok(request)
}
