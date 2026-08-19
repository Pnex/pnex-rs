//! DTO du dashboard (2026-08-19) — réponse unique de
//! `GET /api/v1/dashboard/summary` : liveness PG + dernières mesures
//! OpenObserve + stats builds.
//!
//! La télémétrie vit uniquement dans OpenObserve (D1, aucune table de
//! mesures en PG) : sa section est donc **dégradée par conception** —
//! `telemetry.available == false` quand l'org n'a pas de credentials O2 ou
//! que la requête échoue/expire. Les sections PG (liveness, builds) sont
//! toujours servies : le summary ne fait jamais 500 depuis la branche O2.
//!
//! Champs dates en chaînes RFC 3339 (sérialisation SeaORM / timestamps
//! epoch Prometheus convertis côté backend), pas de chrono dans le core
//! (wasm32).

use serde::{Deserialize, Serialize};

/// Réponse du `GET /api/v1/dashboard/summary`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSummary {
    pub liveness: LivenessSummary,
    pub telemetry: TelemetrySummary,
    pub builds: BuildStats,
}

/// Devices de l'org avec leur état live (jointure
/// `device_registries` × `device_states`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LivenessSummary {
    /// Comptes complets de l'org — la liste est tronquée, pas les compteurs.
    pub total: u64,
    /// Devices dont `last_seen_at` est plus frais que le TTL de silence.
    pub live: u64,
    /// Triés live d'abord, puis `last_seen` décroissant — plafonnés à ~10
    /// côté backend (demande user : « only latest ~10 », pas toute l'org).
    pub devices: Vec<DeviceLiveness>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceLiveness {
    pub id: i64,
    /// `device_id` métier (PK Django `DeviceRegistry` préservée = `id`).
    pub device_id: String,
    /// Modèle prédéfini (`soil_sensor`, …).
    pub predefined_device_name: String,
    pub device_type: String,
    pub live: bool,
    /// RFC 3339, `None` = jamais vu.
    pub last_seen: Option<String>,
}

/// Dernières mesures — dégradée si OpenObserve indisponible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySummary {
    /// Faux : pas de credentials O2 pour l'org, erreur ou timeout (3 s).
    /// Le front affiche un encart, jamais d'erreur bloquante.
    pub available: bool,
    /// Dernier échantillon par (métrique, device), tri ts décroissant,
    /// plafonné à ~10 lignes côté backend.
    pub latest: Vec<LatestMeasurement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestMeasurement {
    /// Label `__name__` Prometheus (ex. `soil_moisture`).
    pub metric: String,
    pub device_id: String,
    pub value: f64,
    /// RFC 3339 — `None` si l'échantillon n'en portait pas.
    pub timestamp: Option<String>,
}

/// Agrégat des `build_records` de l'org (borné : 1/device par upsert).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildStats {
    pub total: u64,
    pub succeeded: u64,
    /// `succeeded / total` en [0,1]. **0.0 si total == 0** (jamais de NaN :
    /// wasm n'a pas de NaN sérialisable côté front).
    pub success_rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Forme complète telle que sérialisée par le backend.
    #[test]
    fn dashboard_summary_roundtrip() {
        let json = r#"{
            "liveness": {
                "total": 2,
                "live": 1,
                "devices": [
                    {
                        "id": 11,
                        "device_id": "capteur-jardin",
                        "predefined_device_name": "soil_sensor",
                        "device_type": "sensor",
                        "live": true,
                        "last_seen": "2026-08-19T10:00:30+00:00"
                    },
                    {
                        "id": 12,
                        "device_id": "relais-pompe",
                        "predefined_device_name": "4_chan_relay",
                        "device_type": "actuator",
                        "live": false,
                        "last_seen": null
                    }
                ]
            },
            "telemetry": {
                "available": true,
                "latest": [
                    {
                        "metric": "soil_moisture",
                        "device_id": "capteur-jardin",
                        "value": 42.5,
                        "timestamp": "2026-08-19T10:00:30+00:00"
                    }
                ]
            },
            "builds": { "total": 3, "succeeded": 2, "success_rate": 0.6666666666666666 }
        }"#;
        let summary: DashboardSummary = serde_json::from_str(json).unwrap();
        assert_eq!(summary.liveness.total, 2);
        assert_eq!(summary.liveness.live, 1);
        assert!(summary.liveness.devices[0].live);
        assert!(summary.telemetry.available);
        assert_eq!(summary.telemetry.latest[0].metric, "soil_moisture");
        assert_eq!(summary.builds.total, 3);
        let back = serde_json::to_value(&summary).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );
    }

    /// Org vide / O2 absent : tout à zéro, disponible false — la page
    /// dashboard doit pouvoir se dessiner avec cette seule forme.
    #[test]
    fn dashboard_summary_minimal_degrade() {
        let json = r#"{
            "liveness": { "total": 0, "live": 0, "devices": [] },
            "telemetry": { "available": false, "latest": [] },
            "builds": { "total": 0, "succeeded": 0, "success_rate": 0.0 }
        }"#;
        let summary: DashboardSummary = serde_json::from_str(json).unwrap();
        assert!(summary.liveness.devices.is_empty());
        assert!(!summary.telemetry.available);
        assert_eq!(summary.builds.success_rate, 0.0);
    }
}
