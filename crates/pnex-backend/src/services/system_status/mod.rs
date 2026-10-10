//! Platform status probes (D72) — one [`ComponentStatus`] per infra
//! component, run concurrently, each bounded by [`PROBE_TIMEOUT`].
//!
//! Details are runtime diagnostics (English, displayed verbatim); metric
//! keys are machine tokens resolved by the UI (`system-metric-<key>`).

pub mod host;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use loco_rs::app::AppContext;
use pnex_core::{ComponentStatus, StatusMetric, SystemStatus};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait, PaginatorTrait, QueryFilter,
    Statement,
};

use crate::auth::settings::RauthySettings;
use crate::models::_entities::{
    openobserve_orgs, organizations, sea_orm_active_enums::OpenobserveOrgStatus,
};
use crate::services::ai::config::AiSettings;
use crate::services::artifact_store::{S3Config, S3Store};
use crate::services::firmware::FirmwareSettings;
use crate::services::flow::FlowSettings;
use crate::services::last_cache::ValkeySettings;
use crate::services::media::MediaSettings;
use crate::services::openobserve::{Client as O2Client, OpenobserveSettings};
use crate::services::retention::DeploymentMode;
use crate::services::stitch::StitchSettings;

/// Upper bound of every network probe.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Directory walks are expensive on an SD card: cached this long.
const DIR_CACHE_TTL: Duration = Duration::from_secs(300);
/// Entry bound of one directory walk (result flagged partial beyond).
const DIR_MAX_ENTRIES: usize = 200_000;

const OK: &str = "ok";
const DEGRADED: &str = "degraded";
const DOWN: &str = "down";
const NOT_CONFIGURED: &str = "not_configured";

fn component(key: &str, status: &str) -> ComponentStatus {
    ComponentStatus {
        key: key.to_string(),
        status: status.to_string(),
        latency_ms: None,
        detail: None,
        metrics: Vec::new(),
    }
}

fn metric(key: &str, unit: &str, value: Option<f64>) -> StatusMetric {
    StatusMetric {
        key: key.to_string(),
        unit: unit.to_string(),
        value,
        text: None,
        total: None,
        label: None,
    }
}

fn text_metric(key: &str, text: impl Into<String>) -> StatusMetric {
    StatusMetric {
        text: Some(text.into()),
        ..metric(key, "text", None)
    }
}

fn elapsed_ms(started: Instant) -> Option<u64> {
    Some(started.elapsed().as_millis() as u64)
}

/// Full status report (every probe concurrently).
pub async fn collect(ctx: &AppContext) -> SystemStatus {
    let (database, rauthy, openobserve, valkey, objects, local, host_status, ai, secrets) = tokio::join!(
        probe_database(ctx),
        probe_rauthy(ctx),
        probe_openobserve(ctx),
        probe_valkey(ctx),
        probe_object_storage(ctx),
        probe_local_storage(ctx),
        probe_host(ctx),
        probe_ai(ctx),
        probe_secrets(ctx),
    );
    SystemStatus {
        server_version: crate::app::app_version(),
        deployment_mode: DeploymentMode::from_env().as_str().to_string(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        components: vec![
            database,
            rauthy,
            openobserve,
            valkey,
            objects,
            local,
            host_status,
            ai,
            secrets,
        ],
    }
}

async fn scalar_i64(ctx: &AppContext, sql: &str) -> Option<i64> {
    let backend = ctx.db.get_database_backend();
    let row = ctx
        .db
        .query_one_raw(Statement::from_string(backend, sql.to_string()))
        .await
        .ok()??;
    row.try_get_by_index::<i64>(0).ok()
}

async fn probe_database(ctx: &AppContext) -> ComponentStatus {
    let mut out = component("database", OK);
    let backend = ctx.db.get_database_backend();
    let started = Instant::now();
    let ping = tokio::time::timeout(PROBE_TIMEOUT, ctx.db.execute_unprepared("SELECT 1")).await;
    out.latency_ms = elapsed_ms(started);
    match ping {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => {
            out.status = DOWN.into();
            out.detail = Some(err.to_string());
            return out;
        }
        Err(_) => {
            out.status = DOWN.into();
            out.detail = Some("timeout".into());
            return out;
        }
    }
    let backend_name = match backend {
        DatabaseBackend::Postgres => "postgres",
        _ => "other",
    };
    out.metrics.push(text_metric("db_backend", backend_name));
    let size = match backend {
        DatabaseBackend::Postgres => {
            scalar_i64(ctx, "SELECT pg_database_size(current_database())::bigint").await
        }
        _ => None,
    };
    out.metrics
        .push(metric("db_size", "bytes", size.map(|s| s as f64)));
    if backend == DatabaseBackend::Postgres {
        let sql = "SELECT relname::text AS name, pg_total_relation_size(relid)::bigint AS size \
                   FROM pg_catalog.pg_statio_user_tables ORDER BY size DESC LIMIT 5";
        if let Ok(rows) = ctx
            .db
            .query_all_raw(Statement::from_string(backend, sql.to_string()))
            .await
        {
            for row in rows {
                let (Ok(name), Ok(size)) = (
                    row.try_get::<String>("", "name"),
                    row.try_get::<i64>("", "size"),
                ) else {
                    continue;
                };
                out.metrics.push(StatusMetric {
                    label: Some(name),
                    ..metric("db_table", "bytes", Some(size as f64))
                });
            }
        }
    }
    out
}

fn probe_http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        // Rauthy rejects requests without a User-Agent.
        .user_agent("pnex-server")
        .build()
        .unwrap_or_default()
}

async fn probe_rauthy(ctx: &AppContext) -> ComponentStatus {
    let mut out = component("rauthy", OK);
    let Ok(settings) = RauthySettings::from_config(&ctx.config) else {
        out.status = NOT_CONFIGURED.into();
        return out;
    };
    let base = settings.base_url.trim_end_matches('/').to_string();
    let http = probe_http();
    let started = Instant::now();
    let health = http.get(format!("{base}/auth/v1/health")).send().await;
    out.latency_ms = elapsed_ms(started);
    match health {
        Ok(resp) if resp.status().is_success() => {
            // `{"db_healthy": bool, "cache_healthy": bool}` (Rauthy 0.36).
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let db_ok = body
                .get("db_healthy")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let cache_ok = body
                .get("cache_healthy")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if !(db_ok && cache_ok) {
                out.status = DEGRADED.into();
                out.detail = Some(format!("db_healthy={db_ok} cache_healthy={cache_ok}"));
            }
        }
        Ok(resp) => {
            out.status = DOWN.into();
            out.detail = Some(format!("HTTP {}", resp.status()));
            return out;
        }
        Err(err) => {
            out.status = DOWN.into();
            out.detail = Some(err.to_string());
            return out;
        }
    }
    if let Ok(resp) = http.get(format!("{base}/auth/v1/version")).send().await {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        if let Some(v) = body.get("current").and_then(|v| v.as_str()) {
            out.metrics.push(text_metric("version", v));
        }
    }
    out.metrics.push(text_metric("issuer", settings.issuer()));
    out
}

async fn probe_openobserve(ctx: &AppContext) -> ComponentStatus {
    let mut out = component("openobserve", OK);
    let Some(settings) = OpenobserveSettings::from_config(&ctx.config) else {
        out.status = NOT_CONFIGURED.into();
        return out;
    };
    let client = O2Client::new(&settings);
    let started = Instant::now();
    let healthy = tokio::time::timeout(PROBE_TIMEOUT, client.healthy())
        .await
        .unwrap_or(false);
    out.latency_ms = elapsed_ms(started);
    if !healthy {
        out.status = DOWN.into();
        out.detail = Some(format!("{}/healthz unreachable", settings.base_url));
        return out;
    }

    let rows = openobserve_orgs::Entity::find()
        .find_also_related(organizations::Entity)
        .all(&ctx.db)
        .await
        .unwrap_or_default();
    let mut storage = 0f64;
    let mut compressed = 0f64;
    let mut docs = 0i64;
    let mut streams = 0usize;
    let mut per_org: Vec<(String, f64)> = Vec::new();
    let mut failures = 0usize;
    for (row, org) in rows {
        if row.status != OpenobserveOrgStatus::Provisioned {
            continue;
        }
        let listing =
            tokio::time::timeout(PROBE_TIMEOUT, client.metric_streams_detailed(&row.o2_org)).await;
        let Ok(Ok(list)) = listing else {
            failures += 1;
            continue;
        };
        let mut org_bytes = 0f64;
        for stream in &list {
            // O2 reports sizes in MB.
            let size = stream.stats.storage_size.unwrap_or(0.0) * 1024.0 * 1024.0;
            org_bytes += size;
            storage += size;
            compressed += stream.stats.compressed_size.unwrap_or(0.0) * 1024.0 * 1024.0;
            docs += stream.stats.doc_num.unwrap_or(0);
        }
        streams += list.len();
        let name = org.map(|o| o.name).unwrap_or_else(|| row.o2_org.clone());
        per_org.push((name, org_bytes));
    }
    if failures > 0 {
        out.status = DEGRADED.into();
        out.detail = Some(format!("{failures} org(s) could not be listed"));
    }
    out.metrics
        .push(metric("o2_storage", "bytes", Some(storage)));
    out.metrics
        .push(metric("o2_compressed", "bytes", Some(compressed)));
    out.metrics
        .push(metric("o2_docs", "count", Some(docs as f64)));
    out.metrics
        .push(metric("o2_streams", "count", Some(streams as f64)));
    per_org.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (name, bytes) in per_org.into_iter().take(5) {
        out.metrics.push(StatusMetric {
            label: Some(name),
            ..metric("o2_org", "bytes", Some(bytes))
        });
    }
    out
}

async fn probe_valkey(ctx: &AppContext) -> ComponentStatus {
    let mut out = component("valkey", OK);
    let Some(url) = ValkeySettings::from_config(&ctx.config).and_then(|s| s.url) else {
        out.status = NOT_CONFIGURED.into();
        return out;
    };
    let started = Instant::now();
    let probe = async {
        let client = redis::Client::open(url.as_str()).map_err(|e| e.to_string())?;
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| e.to_string())?;
        let _: String = redis::cmd("PING")
            .query_async(&mut conn)
            .await
            .map_err(|e| e.to_string())?;
        let info: String = redis::cmd("INFO")
            .arg("memory")
            .query_async(&mut conn)
            .await
            .unwrap_or_default();
        let server: String = redis::cmd("INFO")
            .arg("server")
            .query_async(&mut conn)
            .await
            .unwrap_or_default();
        Ok::<_, String>((info, server))
    };
    match tokio::time::timeout(PROBE_TIMEOUT, probe).await {
        Ok(Ok((memory, server))) => {
            out.latency_ms = elapsed_ms(started);
            let field = |text: &str, name: &str| {
                text.lines()
                    .find_map(|l| l.strip_prefix(name))
                    .map(|v| v.trim().to_string())
            };
            let used = field(&memory, "used_memory:").and_then(|v| v.parse::<f64>().ok());
            out.metrics.push(metric("valkey_memory", "bytes", used));
            if let Some(v) =
                field(&server, "valkey_version:").or_else(|| field(&server, "redis_version:"))
            {
                out.metrics.push(text_metric("version", v));
            }
        }
        Ok(Err(err)) => {
            out.status = DOWN.into();
            out.detail = Some(err);
        }
        Err(_) => {
            out.status = DOWN.into();
            out.detail = Some("timeout".into());
        }
    }
    out
}

/// `SUM(size_bytes)` of a catalog table, cast to BIGINT (PG returns
/// NUMERIC for a bigint sum).
async fn sum_bytes(ctx: &AppContext, table: &str) -> Option<i64> {
    scalar_i64(
        ctx,
        &format!("SELECT CAST(COALESCE(SUM(size_bytes), 0) AS BIGINT) FROM {table}"),
    )
    .await
}

async fn probe_object_storage(ctx: &AppContext) -> ComponentStatus {
    let mut out = component("object_storage", OK);
    let firmware = FirmwareSettings::from_config(&ctx.config);
    let media = MediaSettings::from_config(&ctx.config);
    out.metrics.push(text_metric(
        "firmware_backend",
        firmware.storage_backend.clone(),
    ));
    out.metrics
        .push(text_metric("media_backend", media.storage_backend.clone()));
    // Stored sizes come from the DB catalog: listing a whole bucket is too
    // expensive for a status page.
    let firmware_bytes = sum_bytes(ctx, "firmware_artifacts").await;
    let media_bytes = sum_bytes(ctx, "media_versions").await;
    out.metrics.push(metric(
        "firmware_bytes",
        "bytes",
        Some(firmware_bytes.unwrap_or(0) as f64),
    ));
    out.metrics.push(metric(
        "media_bytes",
        "bytes",
        Some(media_bytes.unwrap_or(0) as f64),
    ));

    // S3 connectivity: probe each distinct configured bucket.
    let mut targets: Vec<S3Config> = Vec::new();
    if firmware.storage_backend == "s3" {
        targets.push(S3Config {
            endpoint: firmware.s3_endpoint.clone(),
            bucket: firmware.s3_bucket.clone(),
            region: firmware.s3_region.clone(),
            access_key: firmware.s3_access_key.clone(),
            secret_key: firmware.s3_secret_key.clone(),
            path_style: firmware.s3_path_style,
        });
    }
    if media.storage_backend == "s3"
        && !targets
            .iter()
            .any(|t| t.endpoint == media.s3_endpoint && t.bucket == media.s3_bucket)
    {
        targets.push(S3Config {
            endpoint: media.s3_endpoint.clone(),
            bucket: media.s3_bucket.clone(),
            region: media.s3_region.clone(),
            access_key: media.s3_access_key.clone(),
            secret_key: media.s3_secret_key.clone(),
            path_style: media.s3_path_style,
        });
    }
    if targets.is_empty() {
        // Everything lives in the DB / local filesystem: nothing to reach.
        out.detail = Some("no S3 backend configured (db/fs storage)".into());
        return out;
    }
    let started = Instant::now();
    let mut errors = Vec::new();
    for target in &targets {
        out.metrics.push(StatusMetric {
            label: Some(target.endpoint.clone()),
            ..text_metric("s3_bucket", target.bucket.clone())
        });
        let result = match S3Store::connect(target) {
            Ok(store) => tokio::time::timeout(PROBE_TIMEOUT, store.check())
                .await
                .unwrap_or_else(|_| Err("timeout".into())),
            Err(err) => Err(err),
        };
        if let Err(err) = result {
            errors.push(format!("{}: {err}", target.bucket));
        }
    }
    out.latency_ms = elapsed_ms(started);
    if !errors.is_empty() {
        out.status = DOWN.into();
        out.detail = Some(errors.join("; "));
    }
    out
}

type DirCache = Option<(Instant, Vec<(String, String, Option<(u64, bool)>)>)>;
static DIR_CACHE: Mutex<DirCache> = Mutex::new(None);

async fn probe_local_storage(ctx: &AppContext) -> ComponentStatus {
    let mut out = component("local_storage", OK);
    let dirs: Vec<(String, String)> = vec![
        ("media".into(), MediaSettings::from_config(&ctx.config).dir),
        (
            "flow_state".into(),
            FlowSettings::from_config(&ctx.config).state_dir,
        ),
        (
            "stitch_models".into(),
            StitchSettings::from_config(&ctx.config).models_dir,
        ),
    ];
    let cached = DIR_CACHE
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .filter(|(at, _)| at.elapsed() < DIR_CACHE_TTL)
        .map(|(_, sizes)| sizes);
    let sizes = match cached {
        Some(sizes) => sizes,
        None => {
            let walk = dirs.clone();
            let sizes = tokio::task::spawn_blocking(move || {
                walk.into_iter()
                    .map(|(key, path)| {
                        let size = host::dir_size(&path, DIR_MAX_ENTRIES);
                        (key, path, size)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            if let Ok(mut guard) = DIR_CACHE.lock() {
                *guard = Some((Instant::now(), sizes.clone()));
            }
            sizes
        }
    };
    for (key, path, size) in sizes {
        let mut m = metric(
            &format!("dir_{key}"),
            "bytes",
            size.map(|(bytes, _)| bytes as f64),
        );
        let partial = size.is_some_and(|(_, partial)| partial);
        m.label = Some(if partial {
            format!("{path} (partial)")
        } else {
            path
        });
        out.metrics.push(m);
    }
    out
}

async fn probe_host(ctx: &AppContext) -> ComponentStatus {
    let mut out = component("host", OK);
    if !cfg!(target_os = "linux") {
        out.status = NOT_CONFIGURED.into();
        out.detail = Some("host metrics are only collected on Linux".into());
        return out;
    }
    let media_dir = MediaSettings::from_config(&ctx.config).dir;
    let facts = tokio::task::spawn_blocking(move || {
        let mut disks = Vec::new();
        if let Some(root) = host::disk_usage("/") {
            disks.push(root);
        }
        // The data directory often sits on another volume (Docker `/data`).
        let data_dir = std::path::Path::new(&media_dir)
            .ancestors()
            .find(|p| p.exists())
            .map(|p| p.to_string_lossy().to_string());
        if let Some(dir) = data_dir {
            if let Some(disk) = host::disk_usage(&dir) {
                if !disks.iter().any(|d| {
                    d.total_bytes == disk.total_bytes && d.available_bytes == disk.available_bytes
                }) {
                    disks.push(disk);
                }
            }
        }
        (
            host::machine_kind(),
            disks,
            host::memory(),
            host::load1(),
            host::uptime_secs(),
        )
    })
    .await;
    let Ok((kind, disks, memory, load, uptime)) = facts else {
        out.status = DEGRADED.into();
        return out;
    };
    out.metrics.push(StatusMetric {
        label: kind.model.clone(),
        ..text_metric("machine_kind", kind.kind)
    });
    out.metrics
        .push(text_metric("arch", std::env::consts::ARCH));
    let cpus = std::thread::available_parallelism().map(|n| n.get()).ok();
    out.metrics
        .push(metric("cpu_count", "count", cpus.map(|n| n as f64)));
    let mut low_disk = false;
    for disk in disks {
        let used = disk.total_bytes.saturating_sub(disk.available_bytes);
        if disk.total_bytes > 0 && (disk.available_bytes as f64) < disk.total_bytes as f64 * 0.1 {
            low_disk = true;
        }
        out.metrics.push(StatusMetric {
            total: Some(disk.total_bytes as f64),
            label: Some(disk.path),
            ..metric("disk", "bytes", Some(used as f64))
        });
    }
    if let Some(mem) = memory {
        out.metrics.push(StatusMetric {
            total: Some(mem.total_bytes as f64),
            ..metric(
                "memory",
                "bytes",
                Some(mem.total_bytes.saturating_sub(mem.available_bytes) as f64),
            )
        });
    }
    out.metrics.push(metric("load1", "count", load));
    out.metrics.push(metric("uptime", "seconds", uptime));
    if low_disk {
        out.status = DEGRADED.into();
        out.detail = Some("less than 10% disk space available".into());
    }
    out
}

/// Vault keyring (secrets.md D112, S8): write key, rows still under an
/// older key (rekey candidates) and rows under a key the keyring lacks.
async fn probe_secrets(ctx: &AppContext) -> ComponentStatus {
    let mut out = component("secrets", OK);
    let ring = match crate::services::secrets::Keyring::from_config(&ctx.config) {
        Ok(ring) => ring,
        Err(err) => {
            out.status = DOWN.into();
            out.detail = Some(err.to_string());
            return out;
        }
    };
    out.metrics
        .push(text_metric("secrets_write_key", ring.primary_id()));
    out.metrics.push(metric(
        "secrets_keys",
        "count",
        Some(ring.key_ids().count() as f64),
    ));
    match crate::services::secrets::rekey::stats(&ctx.db, &ring).await {
        Ok(stats) => {
            out.metrics
                .push(metric("secrets_total", "count", Some(stats.total as f64)));
            out.metrics
                .push(metric("secrets_stale", "count", Some(stats.stale as f64)));
            if stats.unknown_key > 0 {
                out.status = DEGRADED.into();
                out.detail = Some(format!(
                    "{} secret(s) encrypted with a key missing from PNEX_SECRETS_KEYS",
                    stats.unknown_key
                ));
            }
        }
        Err(err) => {
            out.status = DOWN.into();
            out.detail = Some(err.to_string());
        }
    }
    out
}

async fn probe_ai(ctx: &AppContext) -> ComponentStatus {
    let mut out = component("ai", OK);
    if !AiSettings::from_config(&ctx.config).enabled {
        out.status = NOT_CONFIGURED.into();
        out.detail = Some("AI disabled (settings.ai.enabled = false)".into());
        return out;
    }
    let org_providers = crate::models::_entities::llm_providers::Entity::find()
        .filter(crate::models::_entities::llm_providers::Column::OrgId.is_not_null())
        .count(&ctx.db)
        .await
        .unwrap_or(0);
    out.metrics.push(metric(
        "ai_org_providers",
        "count",
        Some(org_providers as f64),
    ));
    // Users bring their own LLM (D119): only the org providers count.
    out
}
