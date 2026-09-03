//! Fichier d'état `<home>/runtime.json` : c'est le contrat de santé lu par le
//! superviseur backend (`GET /api/v1/flows/{id}/runtime`). Écriture atomique
//! (tmp + rename) — un lecteur concurrent ne voit jamais un fichier tronqué.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeState {
    pub pid: u32,
    pub running: bool,
    /// Secondes epoch du démarrage du process.
    pub started_at: u64,
    /// Empreinte du flows.json en exécution (SHA-256 hex, EdgeLinkd).
    pub flow_rev: Option<String>,
    /// Nombre de rechargements à chaud réussis depuis le démarrage.
    pub redeploys: u64,
    /// Métadonnées de version communiquées par le superviseur (optionnel,
    /// via `PNEX_FLOW_META` = "flow_id:version_number").
    pub flow_id: Option<i64>,
    pub version_number: Option<i64>,
}

/// Écriture atomique de l'état. Les erreurs sont loguées, jamais fatales :
/// le fichier d'état est best-effort, il ne doit pas tuer le runtime.
pub fn write(home: &std::path::Path, state: &RuntimeState) {
    if let Err(e) = std::fs::create_dir_all(home) {
        log::warn!("Impossible de créer le répertoire d'état {}: {e}", home.display());
        return;
    }
    let json = match serde_json::to_string_pretty(state) {
        Ok(j) => j,
        Err(e) => {
            log::warn!("État runtime non sérialisable : {e}");
            return;
        }
    };
    let tmp = home.join("runtime.json.tmp");
    let dst = home.join("runtime.json");
    if std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, &dst)).is_err() {
        log::warn!("Écriture de {} impossible", dst.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etat_roundtrip_et_ecriture_atomique() {
        let dir = std::env::temp_dir().join(format!("pnex-flow-state-test-{}", std::process::id()));
        let st = RuntimeState {
            pid: 42,
            running: true,
            started_at: 1,
            flow_rev: Some("abc".into()),
            redeploys: 3,
            flow_id: Some(12),
            version_number: Some(4),
        };
        write(&dir, &st);
        let back: RuntimeState = serde_json::from_str(&std::fs::read_to_string(dir.join("runtime.json")).unwrap())
            .unwrap();
        assert_eq!(back.version_number, Some(4));
        assert_eq!(back.redeploys, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
