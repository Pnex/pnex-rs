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

use std::time::Duration;

use chrono::Utc;
use loco_rs::prelude::*;
use pnex_core::{TelemetryCatalog, TelemetryPoint, TelemetrySeriesInfo, TelemetrySeriesResponse};
use sea_orm::DatabaseConnection;

use crate::services::openobserve::{self, client::Client, valid_metric_name};

/// Fenêtre du catalogue (dernière valeur par série pour montrer que la
/// donnée est vivante).
const CATALOG_WINDOW: &str = "24h";

/// Timeout de TOUT le chemin O2 (catalogue ou une série) — dégradé
/// silencieux au-delà, jamais de page qui traîne.
const O2_TIMEOUT: Duration = Duration::from_secs(5);

/// Nombre max de métriques énumérées dans le catalogue (défensif : les
/// pickers restent lisibles même avec des métriques dynamiques).
const METRICS_CAP: usize = 50;

/// Points max rendus par série (défensif — le pas choisi vise déjà ~120).
const POINTS_CAP: usize = 240;

/// Fenêtres proposées par la page, en secondes. Le pas de la query
/// `query_range` est `window / 120` (30 s / 3 m / 12 m).
pub const WINDOWS: &[(&str, i64)] = &[("1h", 3600), ("6h", 21_600), ("24h", 86_400)];

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
        for name in streams
            .iter()
            .filter(|n| valid_metric_name(n))
            .take(METRICS_CAP)
        {
            // Une métrique injoignable n'emporte pas les autres.
            match client
                .prom_query(
                    &creds.o2_org,
                    &format!("last_over_time({name}[{CATALOG_WINDOW}])"),
                    &creds.email_passcode,
                )
                .await
            {
                Ok(resp) => series.extend(resp.data.result),
                Err(e) => {
                    tracing::debug!(org_id, metric = %name, erreur = %e, "stream non interrogé")
                }
            }
        }
        Ok::<_, String>(series)
    })
    .await;
    let samples = match fetch {
        Ok(Ok(samples)) => samples,
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

    let mut catalog: Vec<TelemetrySeriesInfo> = samples
        .into_iter()
        .filter_map(|s| {
            let metric = s.metric.get("__name__")?.clone();
            let device_id = s.metric.get("device_id")?.clone();
            Some(TelemetrySeriesInfo {
                metric,
                device_id,
                pred_dev: s.metric.get("pred_dev").cloned(),
                last_value: s.value.1.parse().ok()?,
                last_seen: chrono::DateTime::from_timestamp(s.value.0 as i64, 0)
                    .map(|t| t.to_rfc3339()),
            })
        })
        .collect();
    catalog.sort_by(|a, b| {
        a.metric
            .cmp(&b.metric)
            .then_with(|| a.device_id.cmp(&b.device_id))
    });
    TelemetryCatalog {
        available: true,
        series: catalog,
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
        return Err(Error::BadRequest("window invalide (1h, 6h, 24h)".into()));
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
    let end = Utc::now().timestamp();
    let start = end - window_secs;
    // Pas = fenêtre / 120 (~120 pas max) ; O2 rend les points réels sans
    // remplir les trous — le chart tolère des points irréguliers.
    let step = (window_secs / 120).max(1);
    let query = format!(r#"{metric}{{device_id="{device_id}"}}"#);
    let fetch = tokio::time::timeout(
        O2_TIMEOUT,
        client.prom_query_range(
            &creds.o2_org,
            &query,
            start,
            end,
            step,
            &creds.email_passcode,
        ),
    )
    .await;
    let samples = match fetch {
        Ok(Ok(resp)) => resp.data.result,
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
    Ok(TelemetrySeriesResponse {
        available: true,
        metric: metric.to_string(),
        device_id: device_id.to_string(),
        points,
    })
}
