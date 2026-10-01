use super::*;

/// Geste en cours (machine à états plate — un seul variant actif).
#[derive(Clone, PartialEq)]
pub(crate) enum Interaction {
    /// Aucun geste.
    Idle,
    /// Pan du fond : delta client depuis le début du geste.
    Panning {
        start_client: (f64, f64),
        start_pan: (f64, f64),
    },
    /// Drag d'un nœud : `grab` = décalage curseur↔origine du nœud, mesuré au
    /// début du geste.
    Dragging {
        id: String,
        grab: (f64, f64),
        rect: (f64, f64, f64, f64),
    },
    /// Câblage depuis un port de sortie (ou **inverse**, depuis une ancre
    /// d'entrée) : curseur en coords graphe + cible survolée (hit-test bbox à
    /// chaque move). En inverse, la cible survolée devient la **source** —
    /// `pin` vise la rangée d'entrée d'un device-write ou d'une fonction
    /// (None ailleurs).
    Wiring {
        from_id: String,
        port: usize,
        cursor: (f64, f64),
        hover_target: Option<String>,
        reverse: bool,
        pin: Option<String>,
    },
}

/// Action runtime demandée depuis la toolbar (Stop / Start / Restart) —
/// un seul signal d'occupation `runtime_busy` pour les trois.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum RuntimeAction {
    Stop,
    Start,
    Restart,
}

/// Paquet de signaux `Copy` partagés entre l'éditeur et ses sous-composants.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct EditorCx {
    /// Graphe en cours d'édition (ce que le Save enverra).
    pub(crate) graph: Signal<FlowGraph>,
    /// Baseline du dirty : graphe de la dernière version enregistrée.
    pub(crate) saved_graph: Signal<FlowGraph>,
    /// Numéro de la dernière version connue côté serveur.
    pub(crate) saved_version: Signal<i64>,
    /// Nœud sélectionné (inspecteur + touche Delete).
    pub(crate) selected_node: Signal<Option<String>>,
    /// Geste en cours.
    pub(crate) interaction: Signal<Interaction>,
    pub(crate) pan: Signal<(f64, f64)>,
    pub(crate) zoom: Signal<f64>,
    /// Violations courantes (locales ou reçues en 400 du serveur).
    pub(crate) violations: Signal<Vec<FlowViolation>>,
    /// Violations de **staleness** (client-only, jamais persistées) : pin
    /// passé en sortie, pin disparu du pinout, device supprimé, inactif ou
    /// hors ligne — le graphe est structurellement valide mais sa lecture
    /// ne remontera rien (tant que le device n'est pas revenu).
    pub(crate) stale: Signal<Vec<FlowViolation>>,
    /// Dernière valeur publiée par nœud Display (id canvas → badge) — le
    /// badge live sous le nœud. Vidé si le moteur est arrêté (les valeurs
    /// d'un moteur stoppé ne sont plus des vérités).
    pub(crate) display_values: Signal<std::collections::HashMap<String, debug::DisplayBadge>>,
    /// Sonde Display dépliée en vue pretty : (id nœud, JSON pretty) — ouverte
    /// par clic sur le badge live du canvas, fermée au clic hors modal ou à
    /// l'arrêt du moteur (les valeurs d'un moteur stoppé ne sont plus des
    /// vérités).
    pub(crate) expanded_display: Signal<Option<(String, String)>>,
}

impl EditorCx {
    /// Mutation du graphe via un réducteur pur de `state.rs` — `mut self`
    /// car `Signal::with_mut` exige `&mut` (et `EditorCx` est `Copy`).
    pub(crate) fn update_graph(mut self, f: impl FnOnce(&mut FlowGraph)) {
        self.graph.with_mut(|graph| {
            f(graph);
            // Suivi auto des json-split : chaque mutation du graphe (édition
            // d'un Value/Merge, câblage/décâblage) régénère les clés — donc
            // le nombre de ports — des splits en mode auto. Idempotent :
            // rien ne bouge quand l'amont n'expose rien de nouveau.
            state::sync_split_keys_auto(graph);
        });
    }

    /// Violations localisées sur un nœud donné (validation + staleness).
    pub(crate) fn violations_of(&self, node_id: &str) -> Vec<FlowViolation> {
        self.violations
            .read()
            .iter()
            .chain(self.stale.read().iter())
            .filter(|v| v.node_id.as_deref() == Some(node_id))
            .cloned()
            .collect()
    }
}
