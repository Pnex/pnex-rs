//! Logger minimal du runtime : les logs du moteur (crate `log`) partent en
//! **JSON-lines sur stderr** — le canal stdout est réservé aux événements
//! machine (`started`, `debug`, `redeployed`…) consommés par le superviseur
//! Loco. Pas de log4rs : dépendance inutile pour un daemon headless.

use std::io::Write;

pub struct JsonLogger {
    level: log::LevelFilter,
}

impl JsonLogger {
    /// Niveau depuis `PNEX_FLOW_LOG` (défaut : info).
    pub fn from_env() -> Self {
        Self { level: max_level_from_env() }
    }
}

/// Niveau de log global, partagé entre le logger et `log::set_max_level`.
pub fn max_level_from_env() -> log::LevelFilter {
    std::env::var("PNEX_FLOW_LOG")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(log::LevelFilter::Info)
}

impl log::Log for JsonLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = serde_json::json!({
            "ts": epoch_secs(),
            "level": record.level().as_str(),
            "target": record.target(),
            "message": record.args().to_string(),
        });
        let _ = writeln!(std::io::stderr(), "{line}");
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

pub fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}
