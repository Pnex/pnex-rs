//! Lecture télémétrie pour la page Visualisation (2026-08-19) — **la
//! manière formalisée de requêter OpenObserve** (à la InfluxDB : une
//! série sur une fenêtre) derrière `GET /api/v1/telemetry/catalog` et
//! `GET /api/v1/telemetry/series`.
//!
//! (Le module `services::telemetry` est le côté INGEST — point de
//! mesure + sink ; celui-ci est le côté LECTURE, ne pas confondre.)
//!
//! Constats e2e v0.92.1 (réutilisés du dashboard) : pas de sélecteur
//! regex sur `__name__` → découverte via `/streams?type=metrics` puis
//! une requête par nom ; lecture en Basic root (passcode refusé) ;
//! `query_range` accepte le nom nu et l'égalité `device_id="…"`, et ne
//! remplit pas les trous entre deux pas.
//!
//! Doctrine dashboard : jamais de provisioning ni de 500 depuis le
//! chemin lecture — credentials via `provisioned_credentials` (read
//! only), échec/timeout O2 → `available: false`.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use futures_util::StreamExt;
use loco_rs::prelude::*;
use pnex_core::{TelemetryCatalog, TelemetryPoint, TelemetrySeriesInfo, TelemetrySeriesResponse};
use sea_orm::DatabaseConnection;

use crate::services::openobserve::client::{PromQuerySample, PromRangeSample};
use crate::services::openobserve::{self, client::Client, valid_metric_name};

/// Max O2 queries in flight for one catalog / batch request (they used to
/// run strictly one after the other: 24 widgets = 24 round-trips).
const O2_CONCURRENCY: usize = 6;

/// Per-pod TTL of the read cache (`PNEX_VIZ_CACHE_TTL_MS`, default 3 s,
/// 0 disables). Dashboards poll every few seconds and many viewers watch
/// the same series: identical queries inside the TTL hit O2 once.
fn cache_ttl() -> Duration {
    static TTL: LazyLock<Duration> = LazyLock::new(|| {
        Duration::from_millis(
            std::env::var("PNEX_VIZ_CACHE_TTL_MS")
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(3_000),
        )
    });
    *TTL
}

/// Bound of the cache map (entries are tiny but keys are per query).
const CACHE_MAX_ENTRIES: usize = 4_096;

type Cell<T> = Arc<tokio::sync::OnceCell<Arc<T>>>;

/// Short-TTL, single-flight cache: concurrent identical lookups share one
/// in-flight fetch; a failed fetch is never cached (the next caller
/// retries). Values are immutable `Arc`s.
pub struct TtlCache<T> {
    entries: Mutex<HashMap<String, (Instant, Cell<T>)>>,
}

impl<T> Default for TtlCache<T> {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl<T> TtlCache<T> {
    pub async fn get_or_fetch<F, Fut>(
        &self,
        key: String,
        ttl: Duration,
        fetch: F,
    ) -> Result<Arc<T>, String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        if ttl.is_zero() {
            return fetch().await.map(Arc::new);
        }
        let cell = {
            let mut map = self.entries.lock().expect("viz cache");
            let now = Instant::now();
            let fresh = map
                .get(&key)
                .filter(|(at, _)| now.duration_since(*at) < ttl)
                .map(|(_, c)| c.clone());
            match fresh {
                Some(c) => c,
                None => {
                    if map.len() >= CACHE_MAX_ENTRIES {
                        map.retain(|_, (at, _)| now.duration_since(*at) < ttl);
                        if map.len() >= CACHE_MAX_ENTRIES {
                            map.clear();
                        }
                    }
                    let c: Cell<T> = Arc::default();
                    map.insert(key.clone(), (now, c.clone()));
                    c
                }
            }
        };
        let out = cell
            .get_or_try_init(|| async { fetch().await.map(Arc::new) })
            .await
            .cloned();
        if out.is_err() {
            let mut map = self.entries.lock().expect("viz cache");
            if map.get(&key).is_some_and(|(_, c)| Arc::ptr_eq(c, &cell)) {
                map.remove(&key);
            }
        }
        out
    }
}

static INSTANT_CACHE: LazyLock<TtlCache<Vec<PromQuerySample>>> = LazyLock::new(TtlCache::default);
static RANGE_CACHE: LazyLock<TtlCache<Vec<PromRangeSample>>> = LazyLock::new(TtlCache::default);
static LAST_SEEN_CACHE: LazyLock<TtlCache<HashMap<String, i64>>> = LazyLock::new(TtlCache::default);

/// SQL giving the newest sample time of each device of one metric stream.
/// O2 PromQL cannot: an instant query stamps its result with the evaluation
/// time, `timestamp()` too, and subqueries are not implemented.
fn last_seen_sql(metric: &str) -> String {
    format!("SELECT device_id, max(_timestamp) AS last_ts FROM \"{metric}\" GROUP BY device_id")
}

/// `device_id → last sample time (µs)` from the [`last_seen_sql`] hits.
fn last_seen_of(hits: &[serde_json::Value]) -> HashMap<String, i64> {
    hits.iter()
        .filter_map(|hit| {
            let device = hit.get("device_id")?.as_str()?.to_string();
            let ts = hit.get("last_ts")?.as_i64()?;
            Some((device, ts))
        })
        .collect()
}

/// Cached last-sample times (`device_id → µs`) of one metric over the last
/// `window_secs` — shared by the catalog and the home "latest measurements".
pub(crate) async fn cached_last_seen(
    client: &Client,
    o2_org: &str,
    metric: &str,
    window_secs: i64,
    passcode: &str,
) -> Result<Arc<HashMap<String, i64>>, String> {
    let key = format!(
        "{}|{o2_org}|last_seen|{metric}|{window_secs}",
        client.base_url()
    );
    LAST_SEEN_CACHE
        .get_or_fetch(key, cache_ttl(), || async {
            let end_us = Utc::now().timestamp_micros();
            let start_us = end_us - window_secs * 1_000_000;
            client
                .search_metrics(
                    o2_org,
                    &last_seen_sql(metric),
                    start_us,
                    end_us,
                    1_000,
                    passcode,
                )
                .await
                .map(|r| last_seen_of(&r.hits))
        })
        .await
}

/// Catalog entry of one instant sample; `last_seen` is the real time of the
/// newest sample (µs map from [`cached_last_seen`]), never the evaluation
/// time carried by `sample.value.0` — unknown stays `None`.
fn series_info(
    sample: &PromQuerySample,
    last_seen: &HashMap<String, i64>,
) -> Option<TelemetrySeriesInfo> {
    let metric = sample.metric.get("__name__")?.clone();
    let device_id = sample.metric.get("device_id")?.clone();
    Some(TelemetrySeriesInfo {
        last_seen: last_seen
            .get(&device_id)
            .and_then(|us| chrono::DateTime::from_timestamp_micros(*us))
            .map(|t| t.to_rfc3339()),
        metric,
        pred_dev: sample.metric.get("pred_dev").cloned(),
        last_value: sample.value.1.parse().ok()?,
        device_id,
    })
}

/// Max label sets offered per metric in the catalog (D171).
const LABEL_SETS_PER_METRIC: usize = 50;

/// Free label set of a catalog sample (platform labels dropped), `None`
/// when it has none or one that a selector could not carry safely.
fn label_set_of(sample: &PromQuerySample) -> Option<pnex_core::TelemetryLabelSet> {
    let metric = sample.metric.get("__name__")?.clone();
    let labels: std::collections::BTreeMap<String, String> = sample
        .metric
        .iter()
        .filter(|(k, _)| {
            !k.starts_with("__") && !pnex_core::RESERVED_SERIES_LABELS.contains(&k.as_str())
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if labels.is_empty() || pnex_core::check_series_selector_labels(&labels).is_some() {
        return None;
    }
    Some(pnex_core::TelemetryLabelSet { metric, labels })
}

/// PromQL of one series spec, `None` when it is not safe to query: every
/// interpolated piece passes a closed charset first (injection boundary).
/// Without a device the matching series are summed (D171).
fn series_query(
    metric: &str,
    device_id: &str,
    labels: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    if !valid_metric_name(metric) || pnex_core::check_series_selector_labels(labels).is_some() {
        return None;
    }
    let mut matchers: Vec<String> = Vec::new();
    if !device_id.is_empty() {
        if !valid_device_label(device_id) {
            return None;
        }
        matchers.push(format!(r#"device_id="{device_id}""#));
    } else if labels.is_empty() {
        return None;
    }
    matchers.extend(labels.iter().map(|(k, v)| format!(r#"{k}="{v}""#)));
    let selector = format!("{metric}{{{}}}", matchers.join(","));
    Some(if device_id.is_empty() {
        format!("sum({selector})")
    } else {
        selector
    })
}

/// Cached instant query (`last_over_time` of the catalog).
async fn cached_prom_query(
    client: &Client,
    o2_org: &str,
    query: &str,
    passcode: &str,
) -> Result<Arc<Vec<PromQuerySample>>, String> {
    let key = format!("{}|{o2_org}|{query}", client.base_url());
    INSTANT_CACHE
        .get_or_fetch(key, cache_ttl(), || async {
            client
                .prom_query(o2_org, query, passcode)
                .await
                .map(|r| r.data.result)
        })
        .await
}

/// Cached range query ending now. The key carries the window and step but
/// not the exact `end`: inside the TTL every viewer gets the same answer.
async fn cached_prom_query_range(
    client: &Client,
    o2_org: &str,
    query: &str,
    window_secs: i64,
    passcode: &str,
) -> Result<Arc<Vec<PromRangeSample>>, String> {
    // Step = window / 120 (~120 steps max); O2 returns the real points
    // without filling the gaps — the chart tolerates irregular points.
    let step = (window_secs / 120).max(1);
    let key = format!(
        "{}|{o2_org}|{query}|{window_secs}|{step}",
        client.base_url()
    );
    RANGE_CACHE
        .get_or_fetch(key, cache_ttl(), || async {
            let end = Utc::now().timestamp();
            let start = end - window_secs;
            client
                .prom_query_range(o2_org, query, start, end, step, passcode)
                .await
                .map(|r| r.data.result)
        })
        .await
}

/// Real points of range samples, sorted, capped at [`POINTS_CAP`].
fn points_of(samples: &[PromRangeSample]) -> Vec<TelemetryPoint> {
    let mut points: Vec<TelemetryPoint> = samples
        .iter()
        .flat_map(|s| s.values.iter())
        .filter_map(|(ts, value)| {
            Some(TelemetryPoint {
                ts: *ts,
                value: value.parse().ok()?,
            })
        })
        .collect();
    points.sort_by(|a, b| a.ts.total_cmp(&b.ts));
    points.truncate(POINTS_CAP);
    points
}

/// Fenêtre du catalogue (dernière valeur par série pour montrer que la
/// donnée est vivante).
const CATALOG_WINDOW: &str = "24h";
/// [`CATALOG_WINDOW`] in seconds (SQL last-seen lookup).
const CATALOG_WINDOW_SECS: i64 = 86_400;

/// Timeout de TOUT le chemin O2 (catalogue ou une série) — dégradé
/// silencieux au-delà, jamais de page qui traîne.
const O2_TIMEOUT: Duration = Duration::from_secs(5);

/// Nombre max de métriques énumérées dans le catalogue (défensif : les
/// pickers restent lisibles même avec des métriques dynamiques).
const METRICS_CAP: usize = 50;

/// Points max rendus par série (défensif — le pas choisi vise déjà ~120).
const POINTS_CAP: usize = 240;

/// Fenêtres proposées, en secondes — alignées sur
/// `pnex_core::VIZ_WINDOW_PRESETS` (source unique : les widgets SCADA
/// valident la même liste que cette lecture). Le pas de la query
/// `query_range` est `window / 120` (30 s / 3 m / 12 m / 30 m…).
pub const WINDOWS: &[(&str, i64)] = pnex_core::VIZ_WINDOW_PRESETS;

/// Budget global d'un batch (toutes specs) — au-delà, les items restants
/// sont dégradés (le front garde un dashboard réactif plutôt qu'une page
/// qui traîne).
const BATCH_TIMEOUT: Duration = Duration::from_secs(8);

/// Valeur de label `device_id` sûre à interpoler dans un sélecteur
/// PromQL : charset fermé (nos device_id sont des slugs), aucune
/// quote/brace/backslash possible — l'injection PromQL est bloquée en
/// amont, pas échappée. Vit dans pnex-core (`naming`) : source unique avec
/// le runtime de flows.
use pnex_core::valid_device_label;

/// Séries disponibles de l'org (métrique × device, dernière valeur sur
/// 24 h) — alimente les sélecteurs de la page Visualisation.
/// `client: None` (O2 non configuré) → catalogue dégradé.
pub async fn series_catalog(
    db: &DatabaseConnection,
    client: Option<&Client>,
    org_id: i64,
) -> TelemetryCatalog {
    let degraded = TelemetryCatalog {
        available: false,
        series: vec![],
        label_sets: vec![],
    };
    let Some(client) = client else {
        return degraded;
    };
    // Lecture seule : une org sans données n'est pas provisionnée, on ne
    // provisionne pas depuis le chemin HTTP utilisateur.
    let Some(creds) = openobserve::provisioned_credentials(db, org_id)
        .await
        .ok()
        .flatten()
    else {
        return degraded;
    };
    let fetch = tokio::time::timeout(O2_TIMEOUT, async {
        let streams = client
            .metric_streams(&creds.o2_org, &creds.email_passcode)
            .await?;
        let mut series = Vec::new();
        let mut label_sets: Vec<pnex_core::TelemetryLabelSet> = Vec::new();
        // One query per metric, a bounded number in flight (cached and
        // coalesced per pod for a few seconds).
        let mut jobs = Vec::new();
        for name in streams
            .iter()
            .filter(|n| valid_metric_name(n))
            .take(METRICS_CAP)
            .cloned()
        {
            let query = format!("last_over_time({name}[{CATALOG_WINDOW}])");
            let (o2_org, passcode) = (creds.o2_org.clone(), creds.email_passcode.clone());
            jobs.push(async move {
                let (res, seen) = tokio::join!(
                    cached_prom_query(client, &o2_org, &query, &passcode),
                    cached_last_seen(client, &o2_org, &name, CATALOG_WINDOW_SECS, &passcode),
                );
                (name, res, seen)
            });
        }
        let mut results = futures_util::stream::iter(jobs).buffer_unordered(O2_CONCURRENCY);
        while let Some((name, res, seen)) = results.next().await {
            // A failed last-seen lookup only blanks the age, not the series.
            let seen = seen.unwrap_or_else(|e| {
                tracing::debug!(org_id, metric = %name, error = %e, "last seen not queried");
                Arc::default()
            });
            // Une métrique injoignable n'emporte pas les autres.
            match res {
                Ok(samples) => {
                    // Labelled series (D171) are offered by label set, the
                    // others by device.
                    let mut sets = 0;
                    for s in samples.iter() {
                        if let Some(set) = label_set_of(s) {
                            if sets < LABEL_SETS_PER_METRIC {
                                sets += 1;
                                label_sets.push(set);
                            }
                        } else if let Some(info) = series_info(s, &seen) {
                            series.push(info);
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!(org_id, metric = %name, error = %e, "metric not queried")
                }
            }
        }
        Ok::<_, String>((series, label_sets))
    })
    .await;
    let (mut catalog, mut label_sets) = match fetch {
        Ok(Ok(found)) => found,
        Ok(Err(e)) => {
            tracing::warn!(org_id, erreur = %e, "catalogue : O2 en échec, télémétrie dégradée");
            return degraded;
        }
        Err(_) => {
            tracing::warn!(
                org_id,
                "catalogue : chemin O2 expiré (5 s), télémétrie dégradée"
            );
            return degraded;
        }
    };

    catalog.sort_by(|a, b| {
        a.metric
            .cmp(&b.metric)
            .then_with(|| a.device_id.cmp(&b.device_id))
    });
    label_sets.sort_by(|a, b| (&a.metric, &a.labels).cmp(&(&b.metric, &b.labels)));
    TelemetryCatalog {
        available: true,
        series: catalog,
        label_sets,
    }
}

/// Points d'UNE série (métrique × device) sur une fenêtre preset —
/// `window` est la clé de [`WINDOWS`] ; les paramètres sont validés
/// (anti-injection PromQL) AVANT toute construction de requête, donc
/// y compris quand O2 n'est pas configuré (`client: None` → dégradé).
pub async fn series_points(
    db: &DatabaseConnection,
    client: Option<&Client>,
    org_id: i64,
    metric: &str,
    device_id: &str,
    window: &str,
) -> Result<TelemetrySeriesResponse> {
    let degraded = |metric: &str, device_id: &str| TelemetrySeriesResponse {
        available: false,
        metric: metric.to_string(),
        device_id: device_id.to_string(),
        points: vec![],
    };
    if !valid_metric_name(metric) {
        return Err(Error::BadRequest("metric invalide".into()));
    }
    if !valid_device_label(device_id) {
        return Err(Error::BadRequest("device_id invalide".into()));
    }
    let Some(&(_, window_secs)) = WINDOWS.iter().find(|(key, _)| key == &window) else {
        let presets = WINDOWS
            .iter()
            .map(|(key, _)| *key)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::BadRequest(format!("window invalide ({presets})")));
    };
    let Some(client) = client else {
        return Ok(degraded(metric, device_id));
    };

    let Some(creds) = openobserve::provisioned_credentials(db, org_id)
        .await
        .ok()
        .flatten()
    else {
        return Ok(degraded(metric, device_id));
    };
    let query = format!(r#"{metric}{{device_id="{device_id}"}}"#);
    let fetch = tokio::time::timeout(
        O2_TIMEOUT,
        cached_prom_query_range(
            client,
            &creds.o2_org,
            &query,
            window_secs,
            &creds.email_passcode,
        ),
    )
    .await;
    let samples = match fetch {
        Ok(Ok(samples)) => samples,
        Ok(Err(e)) => {
            tracing::warn!(org_id, metric, erreur = %e, "série : O2 en échec, dégradé");
            return Ok(degraded(metric, device_id));
        }
        Err(_) => {
            tracing::warn!(org_id, metric, "série : chemin O2 expiré (5 s), dégradé");
            return Ok(degraded(metric, device_id));
        }
    };

    // Le sélecteur device_id ne laisse normalement qu'une série ; on
    // fusionne défensivement toutes celles rendues (points réels, valeurs
    // non numériques skippées).
    let points = points_of(&samples);
    Ok(TelemetrySeriesResponse {
        available: true,
        metric: metric.to_string(),
        device_id: device_id.to_string(),
        points,
    })
}

/// `POST /api/v1/telemetry/series-batch` (D31, studio SCADA) — un appel
/// pour toutes les sources d'un dashboard. Validation anti-injection
/// **par spec** AVANT toute construction de requête (et y compris sans
/// O2 configuré) : une spec invalide est **dégradée au niveau item**
/// (`available: false`), jamais une 400 globale — un widget mal réglé ne
/// doit pas priver l'écran des autres. Queries run with bounded
/// concurrency ([`O2_CONCURRENCY`], per-pod TTL cache) under a global
/// budget ([`BATCH_TIMEOUT`]); past it, the remaining items stay degraded.
pub async fn series_batch(
    db: &DatabaseConnection,
    client: Option<&Client>,
    org_id: i64,
    specs: &[pnex_core::SeriesSpec],
) -> pnex_core::SeriesBatchResponse {
    let degraded_item = |s: &pnex_core::SeriesSpec| TelemetrySeriesResponse {
        available: false,
        metric: s.metric.clone(),
        device_id: s.device_id.clone(),
        points: vec![],
    };
    // Forme valide ? (charset + preset) — l'ordre de la réponse suit
    // l'ordre des specs.
    let queries: Vec<Option<String>> = specs
        .iter()
        .map(|s| {
            WINDOWS
                .iter()
                .any(|(key, _)| key == &s.window)
                .then(|| series_query(&s.metric, &s.device_id, &s.labels))
                .flatten()
        })
        .collect();
    let any_valid = queries.iter().any(Option::is_some);
    let mut results: Vec<TelemetrySeriesResponse> = specs.iter().map(degraded_item).collect();
    if !any_valid {
        // Tout invalide (ou batch vide) : pas de provisioning ni de
        // requête — même doctrine que le chemin unitaire.
        return pnex_core::SeriesBatchResponse {
            available: false,
            results,
        };
    }
    let Some(client) = client else {
        return pnex_core::SeriesBatchResponse {
            available: false,
            results,
        };
    };
    let Some(creds) = openobserve::provisioned_credentials(db, org_id)
        .await
        .ok()
        .flatten()
    else {
        return pnex_core::SeriesBatchResponse {
            available: false,
            results,
        };
    };

    // Bounded concurrency (was sequential) under the global budget: items
    // finished before the deadline are kept, the rest stay degraded.
    let results_ref = &mut results;
    let fetch = tokio::time::timeout(BATCH_TIMEOUT, async move {
        let mut jobs = Vec::new();
        for (i, s) in specs.iter().enumerate() {
            let Some(query) = queries[i].clone() else {
                continue; // item already degraded
            };
            let Some(&(_, window_secs)) = WINDOWS.iter().find(|(key, _)| key == &s.window) else {
                continue;
            };
            let (o2_org, passcode) = (creds.o2_org.clone(), creds.email_passcode.clone());
            jobs.push(async move {
                let res =
                    cached_prom_query_range(client, &o2_org, &query, window_secs, &passcode).await;
                (i, s, res)
            });
        }
        let mut done = futures_util::stream::iter(jobs).buffer_unordered(O2_CONCURRENCY);
        while let Some((i, s, res)) = done.next().await {
            match res {
                Ok(samples) => {
                    results_ref[i] = TelemetrySeriesResponse {
                        available: true,
                        metric: s.metric.clone(),
                        device_id: s.device_id.clone(),
                        points: points_of(&samples),
                    };
                }
                Err(e) => {
                    tracing::warn!(
                        org_id,
                        metric = %s.metric,
                        error = %e,
                        "batch: item failed, degraded"
                    );
                }
            }
        }
    })
    .await;
    if fetch.is_err() {
        tracing::warn!(
            org_id,
            "batch : budget global expiré (8 s), items restants dégradés"
        );
    }
    pnex_core::SeriesBatchResponse {
        available: true,
        results,
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn identical_lookups_hit_the_source_once_within_ttl() {
        let cache: TtlCache<u32> = TtlCache::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let ttl = Duration::from_secs(60);
        let fetch = |calls: Arc<AtomicUsize>| async move {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok::<_, String>(7)
        };
        // Concurrent viewers coalesce on one in-flight fetch.
        let (a, b) = tokio::join!(
            cache.get_or_fetch("k".into(), ttl, || fetch(calls.clone())),
            cache.get_or_fetch("k".into(), ttl, || fetch(calls.clone())),
        );
        assert_eq!((*a.unwrap(), *b.unwrap()), (7, 7));
        // A later poll inside the TTL is served from the cache.
        let c = cache
            .get_or_fetch("k".into(), ttl, || fetch(calls.clone()))
            .await;
        assert_eq!(*c.unwrap(), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Another key is another query.
        let _ = cache
            .get_or_fetch("other".into(), ttl, || fetch(calls.clone()))
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn failures_are_not_cached_and_zero_ttl_disables() {
        let cache: TtlCache<u32> = TtlCache::default();
        let ttl = Duration::from_secs(60);
        let err = cache
            .get_or_fetch("k".into(), ttl, || async {
                Err::<u32, _>("o2 down".to_string())
            })
            .await;
        assert!(err.is_err());
        let ok = cache
            .get_or_fetch("k".into(), ttl, || async { Ok::<_, String>(1) })
            .await;
        assert_eq!(*ok.unwrap(), 1, "a failed fetch must not stick");

        let calls = AtomicUsize::new(0);
        for _ in 0..3 {
            let _ = cache
                .get_or_fetch("z".into(), Duration::ZERO, || async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, String>(0)
                })
                .await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }
}

#[cfg(test)]
mod last_seen_tests {
    use super::*;

    fn sample(device: &str, eval_ts: f64, value: &str) -> PromQuerySample {
        PromQuerySample {
            metric: HashMap::from([
                ("__name__".to_string(), "a0".to_string()),
                ("device_id".to_string(), device.to_string()),
            ]),
            value: (eval_ts, value.to_string()),
        }
    }

    #[test]
    fn last_seen_uses_sample_time_not_evaluation_time() {
        let hits = vec![
            serde_json::json!({"device_id": "dev-1", "last_ts": 1_790_758_222_537_000_i64}),
            serde_json::json!({"device_id": 42, "last_ts": 1}),
        ];
        let seen = last_seen_of(&hits);
        assert_eq!(seen.len(), 1);
        let info = series_info(&sample("dev-1", 1_791_054_742.0, "86"), &seen).unwrap();
        assert_eq!(info.last_value, 86.0);
        assert_eq!(
            info.last_seen.as_deref(),
            Some("2026-09-30T08:50:22.537+00:00")
        );
    }

    #[test]
    fn unknown_last_seen_stays_none() {
        let info = series_info(&sample("dev-2", 1_791_054_742.0, "1"), &HashMap::new()).unwrap();
        assert_eq!(info.last_seen, None);
    }

    #[test]
    fn last_seen_sql_quotes_the_stream() {
        assert_eq!(
            last_seen_sql("soil_moisture"),
            r#"SELECT device_id, max(_timestamp) AS last_ts FROM "soil_moisture" GROUP BY device_id"#
        );
    }
}

#[cfg(test)]
mod label_query_tests {
    use super::*;
    use std::collections::BTreeMap;

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn query_by_device_and_by_labels() {
        assert_eq!(
            series_query("t", "dev-1", &labels(&[])).as_deref(),
            Some(r#"t{device_id="dev-1"}"#)
        );
        assert_eq!(
            series_query("etl_m", "flow_3", &labels(&[("stream", "inter")])).as_deref(),
            Some(r#"etl_m{device_id="flow_3",stream="inter"}"#)
        );
        assert_eq!(
            series_query(
                "media_capture_up",
                "",
                &labels(&[("stream", "inter"), ("entity", "x:1")])
            )
            .as_deref(),
            Some(r#"sum(media_capture_up{entity="x:1",stream="inter"})"#)
        );
        // Neither a device nor labels: nothing to select.
        assert_eq!(series_query("t", "", &labels(&[])), None);
    }

    #[test]
    fn injection_attempts_are_refused() {
        for (k, v) in [
            ("stream", r#"x"}"#),
            ("stream", "x\n"),
            ("stream", r#"a\"#),
            ("stream", "a b"),
            ("stream", ""),
            ("stream", "a\",device_id=\"x"),
            ("x\"}", "a"),
            ("device_id", "x"),
            ("__name__", "x"),
        ] {
            assert_eq!(series_query("m", "", &labels(&[(k, v)])), None, "{k}={v:?}");
        }
        assert_eq!(series_query("m}or{", "", &labels(&[("a", "b")])), None);
        assert_eq!(series_query("m", "d\"}", &labels(&[("a", "b")])), None);
        let six: Vec<(String, String)> = (0..6).map(|i| (format!("l{i}"), "v".into())).collect();
        let six: BTreeMap<String, String> = six.into_iter().collect();
        assert_eq!(series_query("m", "", &six), None);
    }

    #[test]
    fn catalog_label_sets_skip_platform_labels() {
        let sample = |pairs: &[(&str, &str)]| PromQuerySample {
            metric: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            value: (0.0, "1".into()),
        };
        let set = label_set_of(&sample(&[
            ("__name__", "etl_mentions"),
            ("device_id", "flow_2"),
            ("source_type", "etl"),
            ("stream", "inter"),
        ]))
        .expect("label set");
        assert_eq!(set.metric, "etl_mentions");
        assert_eq!(set.labels, labels(&[("stream", "inter")]));
        // A plain device series has no free label.
        assert!(label_set_of(&sample(&[("__name__", "t"), ("device_id", "d")])).is_none());
        // A value a selector cannot carry is not offered.
        assert!(label_set_of(&sample(&[("__name__", "t"), ("stream", "a b")])).is_none());
    }
}
