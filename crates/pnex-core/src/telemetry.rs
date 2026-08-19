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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryPoint {
    pub ts: f64,
    pub value: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Forme complète telle que sérialisée par le backend.
    #[test]
    fn telemetry_roundtrip() {
        let json = r#"{
            "available": true,
            "series": [
                {
                    "metric": "soil_moisture",
                    "device_id": "fuzzy-zebra",
                    "pred_dev": "soil_sensor",
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
