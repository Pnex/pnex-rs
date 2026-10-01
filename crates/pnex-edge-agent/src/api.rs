//! Local ingestion API — no password: loopback by default, LAN bind opt-in
//! with an optional CIDR allowlist. Free-form: any key, any JSON value,
//! optional unit; a point is answered `202` only once it is durably queued.
//!
//! - `POST /v1/points` — `{"key","value","ts"?,"unit"?,"record"?}`, an array
//!   of those, a flat object `{"temp": 21.5, "hum": 40}`, or `text/plain`
//!   lines `key=value`;
//! - `POST /v1/points/{key}` — raw value body (`?unit=`, `?record=`);
//! - `GET /v1/status`, `GET /v1/metrics` (keys seen locally), `GET /healthz`.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use tokio::sync::{mpsc, oneshot};

use crate::queue::StoredPoint;
use crate::state::{Shared, Status, WriteReq};

/// Max points per request (larger payloads: split client-side).
pub const MAX_POINTS_PER_REQUEST: usize = 10_000;
/// Max key length (the server enforces the same bound).
pub const MAX_KEY_LEN: usize = 255;
/// Max request body.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
struct Ctx {
    shared: Arc<Shared>,
    allow: Arc<Vec<ipnet::IpNet>>,
}

#[derive(Debug, Default, Deserialize)]
struct PointQuery {
    unit: Option<String>,
    record: Option<bool>,
}

fn problem(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(serde_json::json!({"code": code, "message": message.into()})),
    )
        .into_response()
}

/// Parses a timestamp field: epoch milliseconds (number) or RFC 3339.
fn parse_ts(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .filter(|t| *t > 0),
        serde_json::Value::String(s) => chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.timestamp_millis()),
        _ => None,
    }
}

fn check_key(key: &str) -> Result<String, String> {
    let k = key.trim();
    if k.is_empty() {
        return Err("empty key".into());
    }
    if k.len() > MAX_KEY_LEN {
        return Err(format!("key longer than {MAX_KEY_LEN} bytes"));
    }
    Ok(k.to_string())
}

fn unit_of(v: Option<&serde_json::Value>) -> Option<String> {
    v.and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(|u| u.chars().take(64).collect())
}

/// Raw text value: JSON when it parses (`21.5`, `true`, `{"a":1}`),
/// otherwise the trimmed string.
fn text_value(raw: &str) -> serde_json::Value {
    let t = raw.trim();
    serde_json::from_str(t).unwrap_or_else(|_| serde_json::Value::String(t.to_string()))
}

/// Body → points (pure, unit-tested). `now_ms` stamps points without `ts`.
pub fn parse_body(
    content_type: &str,
    body: &[u8],
    q_unit: Option<&str>,
    q_record: bool,
    now_ms: i64,
) -> Result<Vec<StoredPoint>, String> {
    let text = std::str::from_utf8(body).map_err(|_| "body is not UTF-8".to_string())?;
    let json_like = content_type.contains("json")
        || matches!(text.trim_start().chars().next(), Some('{' | '['));
    let q_unit = q_unit
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string);
    let mut out = Vec::new();
    if json_like {
        let v: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
        let items: Vec<serde_json::Value> = match v {
            serde_json::Value::Array(a) => a,
            other => vec![other],
        };
        for item in items {
            let serde_json::Value::Object(obj) = item else {
                return Err("each point must be a JSON object".into());
            };
            if let Some(key) = obj.get("key").and_then(serde_json::Value::as_str) {
                // Explicit point.
                let value = obj
                    .get("value")
                    .cloned()
                    .ok_or_else(|| format!("point `{key}` has no `value`"))?;
                out.push(StoredPoint {
                    key: check_key(key)?,
                    value,
                    ts_ms: obj.get("ts").and_then(parse_ts).unwrap_or(now_ms),
                    unit: unit_of(obj.get("unit")).or_else(|| q_unit.clone()),
                    record: obj
                        .get("record")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(q_record),
                });
            } else {
                // Flat object: every field is a point (`ts` = shared stamp).
                let ts = obj.get("ts").and_then(parse_ts).unwrap_or(now_ms);
                for (k, value) in obj {
                    if k == "ts" {
                        continue;
                    }
                    out.push(StoredPoint {
                        key: check_key(&k)?,
                        value,
                        ts_ms: ts,
                        unit: q_unit.clone(),
                        record: q_record,
                    });
                }
            }
        }
    } else {
        for line in text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| format!("expected key=value, got `{line}`"))?;
            out.push(StoredPoint {
                key: check_key(k)?,
                value: text_value(v),
                ts_ms: now_ms,
                unit: q_unit.clone(),
                record: q_record,
            });
        }
    }
    if out.is_empty() {
        return Err("no point in body".into());
    }
    if out.len() > MAX_POINTS_PER_REQUEST {
        return Err(format!(
            "more than {MAX_POINTS_PER_REQUEST} points in one request"
        ));
    }
    Ok(out)
}

async fn enqueue(ctx: &Ctx, points: Vec<StoredPoint>) -> Response {
    let n = points.len();
    let seen: Vec<(String, Option<String>)> = points
        .iter()
        .map(|p| (p.key.clone(), p.unit.clone()))
        .collect();
    let (done, rx) = oneshot::channel();
    // Backpressure: a full writer channel means the disk cannot keep up.
    match ctx.shared.writer.try_send(WriteReq { points, done }) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            let mut r = problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "queue_busy",
                "agent queue is saturated, retry later",
            );
            r.headers_mut()
                .insert("retry-after", axum::http::HeaderValue::from_static("1"));
            return r;
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "shutting_down",
                "agent is stopping",
            );
        }
    }
    match tokio::time::timeout(Duration::from_secs(30), rx).await {
        Ok(Ok(Ok(_last_seq))) => {
            ctx.shared.accepted.fetch_add(n as u64, Ordering::Relaxed);
            let mut keys = ctx.shared.keys.lock().expect("keys");
            for (k, u) in seen {
                let entry = keys.entry(k).or_insert(None);
                if u.is_some() {
                    *entry = u;
                }
            }
            drop(keys);
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({"accepted": n})),
            )
                .into_response()
        }
        Ok(Ok(Err(e))) => problem(StatusCode::INTERNAL_SERVER_ERROR, "queue_write_failed", e),
        _ => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "queue_timeout",
            "durable write did not complete in time",
        ),
    }
}

fn allowed(ctx: &Ctx, peer: &SocketAddr) -> bool {
    let ip = peer.ip();
    ip.is_loopback() || ctx.allow.is_empty() || ctx.allow.iter().any(|n| n.contains(&ip))
}

async fn post_points(
    State(ctx): State<Ctx>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(q): Query<PointQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !allowed(&ctx, &peer) {
        return problem(
            StatusCode::FORBIDDEN,
            "client_not_allowed",
            "client address not allowed",
        );
    }
    let ctype = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let now = chrono::Utc::now().timestamp_millis();
    match parse_body(
        ctype,
        &body,
        q.unit.as_deref(),
        q.record.unwrap_or(false),
        now,
    ) {
        Ok(points) => enqueue(&ctx, points).await,
        Err(e) => problem(StatusCode::BAD_REQUEST, "invalid_body", e),
    }
}

async fn post_point_key(
    State(ctx): State<Ctx>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(key): Path<String>,
    Query(q): Query<PointQuery>,
    body: Bytes,
) -> Response {
    if !allowed(&ctx, &peer) {
        return problem(
            StatusCode::FORBIDDEN,
            "client_not_allowed",
            "client address not allowed",
        );
    }
    let key = match check_key(&key) {
        Ok(k) => k,
        Err(e) => return problem(StatusCode::BAD_REQUEST, "invalid_key", e),
    };
    let Ok(text) = std::str::from_utf8(&body) else {
        return problem(StatusCode::BAD_REQUEST, "invalid_body", "body is not UTF-8");
    };
    if text.trim().is_empty() {
        return problem(StatusCode::BAD_REQUEST, "invalid_body", "empty value");
    }
    let point = StoredPoint {
        key,
        value: text_value(text),
        ts_ms: chrono::Utc::now().timestamp_millis(),
        unit: q
            .unit
            .map(|u| u.trim().chars().take(64).collect())
            .filter(|u: &String| !u.is_empty()),
        record: q.record.unwrap_or(false),
    };
    enqueue(&ctx, vec![point]).await
}

async fn status(State(ctx): State<Ctx>) -> Response {
    Json(Status::of(&ctx.shared)).into_response()
}

async fn metrics(State(ctx): State<Ctx>) -> Response {
    let keys = ctx.shared.keys.lock().expect("keys").clone();
    let list: Vec<serde_json::Value> = keys
        .into_iter()
        .map(|(k, u)| serde_json::json!({"key": k, "unit": u}))
        .collect();
    Json(list).into_response()
}

pub fn router(shared: Arc<Shared>, allow: Vec<ipnet::IpNet>) -> Router {
    let ctx = Ctx {
        shared,
        allow: Arc::new(allow),
    };
    Router::new()
        .route("/v1/points", post(post_points))
        .route("/v1/points/{key}", post(post_point_key))
        .route("/v1/status", get(status))
        .route("/v1/metrics", get(metrics))
        .route("/healthz", get(|| async { "ok" }))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(ctx)
}

/// Group-commit writer: a dedicated OS thread owning the durable appends.
/// Requests arriving while a transaction commits are merged into the next
/// one (one fsync for many HTTP requests).
///
/// Holds only a weak handle on the shared state: the thread ends (closing
/// the queue file) once every sender is dropped with the runtime state.
pub fn spawn_writer(
    shared_queue: crate::queue::Queue,
    mut rx: mpsc::Receiver<WriteReq>,
    wake: std::sync::Weak<Shared>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("pnex-agent-writer".into())
        .spawn(move || {
            while let Some(first) = rx.blocking_recv() {
                let mut reqs = vec![first];
                while reqs.len() < 256 {
                    match rx.try_recv() {
                        Ok(r) => reqs.push(r),
                        Err(_) => break,
                    }
                }
                let all: Vec<StoredPoint> =
                    reqs.iter().flat_map(|r| r.points.iter().cloned()).collect();
                let res = shared_queue.append(&all).map_err(|e| e.to_string());
                for r in reqs {
                    let _ = r.done.send(res.clone());
                }
                if res.is_ok() {
                    if let Some(shared) = wake.upgrade() {
                        shared.wake.notify_one();
                    }
                }
            }
        })
        .expect("spawn writer thread")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn explicit_point_with_unit_ts_and_record() {
        let pts = parse_body(
            "application/json",
            br#"{"key":"temp","value":21.5,"unit":"C","ts":"2027-01-15T08:00:00Z","record":true}"#,
            None,
            false,
            NOW,
        )
        .unwrap();
        assert_eq!(pts.len(), 1);
        assert_eq!(pts[0].unit.as_deref(), Some("C"));
        assert!(pts[0].record);
        assert_eq!(pts[0].ts_ms, 1_800_000_000_000);
    }

    #[test]
    fn array_and_flat_object_and_any_value() {
        let pts = parse_body(
            "",
            br#"[{"key":"a","value":{"x":1}},{"key":"b","value":"on","ts":123}]"#,
            Some("u"),
            true,
            NOW,
        )
        .unwrap();
        assert_eq!(pts[0].value, serde_json::json!({"x": 1}));
        assert_eq!(pts[0].unit.as_deref(), Some("u"));
        assert!(pts[0].record);
        assert_eq!(pts[1].ts_ms, 123);

        let flat = parse_body(
            "application/json",
            br#"{"temp":21.5,"hum":40,"ts":5}"#,
            None,
            false,
            NOW,
        )
        .unwrap();
        assert_eq!(flat.len(), 2);
        assert!(flat.iter().all(|p| p.ts_ms == 5));
    }

    #[test]
    fn text_lines() {
        let pts = parse_body(
            "text/plain",
            b"temp=21.5\n# comment\nstate = running\nok=true\n",
            None,
            false,
            NOW,
        )
        .unwrap();
        assert_eq!(pts.len(), 3);
        assert_eq!(pts[0].value, serde_json::json!(21.5));
        assert_eq!(pts[1].key, "state");
        assert_eq!(pts[1].value, serde_json::json!("running"));
        assert_eq!(pts[2].value, serde_json::json!(true));
        assert!(pts.iter().all(|p| p.ts_ms == NOW));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_body("application/json", b"{bad", None, false, NOW).is_err());
        assert!(parse_body("text/plain", b"novalue", None, false, NOW).is_err());
        assert!(parse_body(
            "application/json",
            br#"{"key":"","value":1}"#,
            None,
            false,
            NOW
        )
        .is_err());
        assert!(parse_body("application/json", br#"{"key":"k"}"#, None, false, NOW).is_err());
        assert!(parse_body("application/json", b"[]", None, false, NOW).is_err());
        assert!(parse_body("application/json", b"[1]", None, false, NOW).is_err());
    }
}
