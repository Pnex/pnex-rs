//! DTO de télémétrie (2026-08-19) — page Visualisation : séries
//! temporelles lues dans OpenObserve (Prometheus) via
//! `GET /api/v1/telemetry/catalog` et `GET /api/v1/telemetry/series`.
//!
//! Même doctrine dégradée que le dashboard : la lecture O2 ne fait jamais
//! échouer la requête — sans credentials O2, en erreur ou en timeout, le
//! payload renvoie `available == false` et des listes vides.
//!
//! Champs dates : timestamps **epoch secondes** en f64 (forme native
//! Prometheus, directement exploitable par le chart SVG côté front) ;
//! `last_seen` du catalogue en RFC 3339 (converti côté backend). Pas de
//! chrono dans le core (wasm32).

use serde::{Deserialize, Serialize};

/// Réponse du `GET /api/v1/telemetry/catalog` — séries disponibles
/// (métrique × device) pour alimenter les sélecteurs de la page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryCatalog {
    /// Faux : pas de credentials O2 pour l'org, erreur ou timeout (5 s).
    pub available: bool,
    pub series: Vec<TelemetrySeriesInfo>,
    /// Series without a device, read by label set (D171): platform
    /// `media_*` series and labelled `etl_` series. Bounded per metric.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub label_sets: Vec<TelemetryLabelSet>,
}

/// One label set of a metric offered to the dashboard pickers (D171).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryLabelSet {
    pub metric: String,
    pub labels: std::collections::BTreeMap<String, String>,
}

/// Key of a series in the live-values map: `metric|device_id`, plus the
/// labels in their sorted order when there are any (D171).
pub fn series_key(
    metric: &str,
    device_id: &str,
    labels: &std::collections::BTreeMap<String, String>,
) -> String {
    let mut key = format!("{metric}|{device_id}");
    if !labels.is_empty() {
        let pairs: Vec<String> = labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
        key.push('|');
        key.push_str(&pairs.join(","));
    }
    key
}

/// Une série disponible = une métrique sur un device (dernière valeur
/// sur 24 h — sert aussi à montrer que la donnée est vivante).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySeriesInfo {
    /// Label `__name__` Prometheus (ex. `soil_moisture`).
    pub metric: String,
    pub device_id: String,
    /// Modèle prédéfini porté à l'ingest (`pred_dev`), s'il existe.
    pub pred_dev: Option<String>,
    pub last_value: f64,
    /// RFC 3339 du dernier échantillon — `None` s'il n'en portait pas.
    pub last_seen: Option<String>,
}

/// Réponse du `GET /api/v1/telemetry/series?metric=…&device_id=…&window=…`
/// — points d'UNE série sur la fenêtre demandée.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySeriesResponse {
    /// Faux : credentials absents, erreur ou timeout O2 — `points` vide.
    pub available: bool,
    pub metric: String,
    pub device_id: String,
    /// Triés par `ts` croissant, plafonnés côté backend (défensif).
    pub points: Vec<TelemetryPoint>,
}

/// Un point de la série — epoch secondes (f64, forme Prometheus : le
/// fractionnaire est possible), valeur numérique déjà re-parsée.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryPoint {
    pub ts: f64,
    pub value: f64,
}

// ───────────────────────── Batch (studio SCADA, D31) ─────────────────────────

/// Une référence de série du `POST /api/v1/telemetry/series-batch` — la
/// même triple que les sources de widget (`viz::SourceRef`) : un seul
/// appel pour tout un dashboard (un timer au lieu de N, D31).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesSpec {
    pub metric: String,
    pub device_id: String,
    /// Preset de fenêtre (`viz::VIZ_WINDOW_PRESETS`).
    pub window: String,
    /// Free label selector (D171); with labels, `device_id` may be empty
    /// (the read then sums the matching series).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub labels: std::collections::BTreeMap<String, String>,
}

impl SeriesSpec {
    /// Key of this series in the live-values map (same as `SourceRef`).
    pub fn series_key(&self) -> String {
        series_key(&self.metric, &self.device_id, &self.labels)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeriesBatchRequest {
    pub specs: Vec<SeriesSpec>,
}

/// Réponse du batch — un item par spec, **même ordre** ; une spec
/// invalide ou non servie (budget dépassé) est **dégradée** au niveau
/// item (`available: false`), jamais une 400 globale.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeriesBatchResponse {
    pub available: bool,
    pub results: Vec<TelemetrySeriesResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_change_the_series_key() {
        let mut labels = std::collections::BTreeMap::new();
        assert_eq!(series_key("m", "d", &labels), "m|d");
        labels.insert("b".to_string(), "2".to_string());
        labels.insert("a".to_string(), "1".to_string());
        assert_eq!(series_key("m", "", &labels), "m||a=1,b=2");
    }

    /// Forme complète telle que sérialisée par le backend.
    #[test]
    fn telemetry_roundtrip() {
        let json = r#"{
            "available": true,
            "series": [
                {
                    "metric": "soil_moisture",
                    "device_id": "fuzzy-zebra",
                    "pred_dev": "temp_sensor",
                    "last_value": 100.0,
                    "last_seen": "2026-08-19T10:00:30+00:00"
                },
                {
                    "metric": "soil_temperature",
                    "device_id": "fuzzy-zebra",
                    "pred_dev": null,
                    "last_value": 21.5,
                    "last_seen": null
                }
            ]
        }"#;
        let catalog: TelemetryCatalog = serde_json::from_str(json).unwrap();
        assert!(catalog.available);
        assert_eq!(catalog.series.len(), 2);
        assert_eq!(catalog.series[0].metric, "soil_moisture");
        assert_eq!(catalog.series[1].pred_dev, None);
        let back = serde_json::to_value(&catalog).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );

        let json = r#"{
            "available": true,
            "metric": "soil_moisture",
            "device_id": "fuzzy-zebra",
            "points": [
                { "ts": 1787151900.0, "value": 100.0 },
                { "ts": 1787152200.0, "value": 99.5 }
            ]
        }"#;
        let series: TelemetrySeriesResponse = serde_json::from_str(json).unwrap();
        assert!(series.available);
        assert_eq!(series.points.len(), 2);
        assert_eq!(series.points[1].value, 99.5);
        let back = serde_json::to_value(&series).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );
    }

    /// Org sans O2 / timeout : la page doit pouvoir se dessiner avec
    /// cette seule forme (encart dégradé).
    #[test]
    fn telemetry_minimal_degrade() {
        let catalog: TelemetryCatalog =
            serde_json::from_str(r#"{ "available": false, "series": [] }"#).unwrap();
        assert!(!catalog.available);
        assert!(catalog.series.is_empty());

        let series: TelemetrySeriesResponse = serde_json::from_str(
            r#"{ "available": false, "metric": "soil_moisture", "device_id": "x", "points": [] }"#,
        )
        .unwrap();
        assert!(!series.available);
        assert!(series.points.is_empty());
    }
}
