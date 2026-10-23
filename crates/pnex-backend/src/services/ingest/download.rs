//! `file_download` (D318): the HTTP body streamed into a multipart S3
//! upload, hashed on the fly — no local file, one chunk in memory. Same
//! sha256 as the last download of a run that did not fail → port 2.

use std::time::Duration;

use chrono::Utc;
use pnex_core::err_codes;
use pnex_core::ingest::{IngestConfig, IngestMsg, StagedEntry, PORT_UNCHANGED};
use sea_orm::ConnectionTrait;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{stmt, unavailable, Cx, Outcome};

/// One body may take this long (500 MB on a slow link).
const TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub(super) fn http() -> &'static reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(|| {
        pnex_core::egress::guarded(reqwest::Client::builder())
            .redirect(pnex_core::egress::redirect_policy(5))
            .connect_timeout(Duration::from_secs(20))
            .timeout(TIMEOUT)
            .user_agent(concat!("PNEX-Ingest/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client")
    })
}

fn unreachable() -> Outcome {
    Outcome::fail(err_codes::IMPORT_SOURCE_UNREACHABLE, Value::Null)
}

/// sha256 of the last download of this node in a run without error.
async fn previous_sha(cx: &Cx<'_>) -> Option<String> {
    cx.db
        .query_one_raw(stmt(
            "SELECT d.sha256 FROM ingest_steps d WHERE d.org_id = $1 AND d.flow_id = $2 \
               AND d.node_id = $3 AND d.step = 'file_download' AND d.port = 0 \
               AND d.sha256 IS NOT NULL AND d.run_id <> $4 \
               AND NOT EXISTS (SELECT 1 FROM ingest_steps e WHERE e.run_id = d.run_id AND e.port = 1) \
             ORDER BY d.created_at DESC LIMIT 1",
            vec![
                cx.node.org_id.into(),
                cx.node.flow_id.into(),
                cx.node.node_id.clone().into(),
                cx.run_id.into(),
            ],
        ))
        .await
        .ok()
        .flatten()
        .and_then(|r| r.try_get::<String>("", "sha256").ok())
}

pub(super) async fn run(cx: &Cx<'_>, cfg: &IngestConfig, msg: IngestMsg) -> Outcome {
    let raw = if msg.url.is_empty() {
        cfg.url.as_str()
    } else {
        msg.url.as_str()
    };
    let Ok(url) = crate::services::ontology::imports::source::check_url(raw) else {
        return unreachable();
    };
    let room = match cx.room().await {
        Ok(n) => n,
        Err(o) => return *o,
    };
    let cap = cfg.max_bytes();
    let mut resp = match http().get(url.clone()).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            return Outcome::fail(
                err_codes::IMPORT_SOURCE_UNREACHABLE,
                json!({ "status": r.status().as_u16().to_string() }),
            )
        }
        Err(e) => {
            tracing::info!(url = %url, error = %e, "ingest download unreachable");
            return unreachable();
        }
    };
    if let Some(n) = resp.content_length().filter(|n| *n > cap) {
        let args = serde_json::json!({ "bytes": n, "max": cap });
        return Outcome::fail(err_codes::IMPORT_SOURCE_TOO_LARGE, args);
    }
    let name = resp
        .url()
        .path_segments()
        .and_then(|mut s| s.next_back().map(str::to_string))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "download".into());
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let key = cx.key("raw", &name);
    let Ok(mut w) = cx.store.writer(&key).await else {
        return unavailable();
    };
    let (mut hasher, mut bytes) = (Sha256::new(), 0u64);
    let failed = loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                bytes += chunk.len() as u64;
                if bytes > cap {
                    // Counted up to the cap: the source holds at least that.
                    let args = serde_json::json!({ "bytes": bytes, "max": cap });
                    break Some(Outcome::fail(err_codes::IMPORT_SOURCE_TOO_LARGE, args));
                }
                if bytes > room {
                    break Some(Outcome::fail(err_codes::INGEST_STAGING_QUOTA, Value::Null));
                }
                hasher.update(&chunk);
                if w.write(chunk).await.is_err() {
                    break Some(unavailable());
                }
            }
            Ok(None) => break None,
            Err(_) => break Some(unreachable()),
        }
    };
    if let Some(out) = failed {
        let _ = w.abort().await;
        return out;
    }
    if w.close().await.is_err() {
        return unavailable();
    }
    let sha256 = format!("{:x}", hasher.finalize());
    let unchanged =
        cfg.skip_if_unchanged.unwrap_or(true) && previous_sha(cx).await.as_deref() == Some(&sha256);
    let manifest = json!({
        "url": url.as_str(),
        "name": name,
        "bytes": bytes,
        "sha256": sha256,
        "content_type": content_type,
        "fetched_at": Utc::now().to_rfc3339(),
        "unchanged": unchanged,
        "staging_key": if unchanged { Value::Null } else { Value::String(key.clone()) },
    });
    if unchanged {
        // Same bytes as the last good run: nothing to keep.
        let _ = cx.store.delete_key(&key).await;
        let mut out = Outcome::ok(
            IngestMsg {
                url: url.to_string(),
                ..Default::default()
            },
            manifest,
        );
        out.port = PORT_UNCHANGED;
        out.sha256 = Some(sha256);
        return out;
    }
    let entry = StagedEntry {
        name,
        staging_key: key.clone(),
        bytes,
    };
    let mut out = Outcome::ok(
        IngestMsg {
            url: url.to_string(),
            raw_key: key.clone(),
            staging_key: key,
            ..Default::default()
        },
        manifest,
    );
    out.written = vec![entry];
    out.sha256 = Some(sha256);
    out.keep_until = Some(Utc::now() + chrono::Duration::days(cfg.keep_raw_days() as i64));
    out
}
