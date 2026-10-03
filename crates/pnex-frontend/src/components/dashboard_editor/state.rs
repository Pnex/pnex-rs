//! Reducers purs du document dashboard (école `flow_editor/state.rs`) —
//! **toutes** les mutations du layout passent ici, jamais en ligne dans
//! le rsx. Chaque reducer renvoie un nouveau document ; l'undo/redo
//! (absent du flow editor, demandé par la PRD §5) empile des snapshots
//! entiers — un synoptique fait quelques Ko, la simplicité gagne.

use pnex_core::{
    CanvasSpec, DashboardLayout, SourceRef, SymbolOptions, ThermoChartOptions, ThermoPressureUnit,
    Widget, WidgetOptions, WidgetTemplate, Wire,
};

use super::geometry::default_size;

/// Nouveau widget **posé depuis un template** — l'instance est un
/// **snapshot** (D41) : la copie du template est définitive, éditer ou
/// supprimer le modèle n'affecte jamais ce widget.
pub fn place_widget(
    layout: &mut DashboardLayout,
    template: &WidgetTemplate,
    id: String,
    x: i64,
    y: i64,
) {
    let (w, h) = default_size(&template.widget_type);
    let widget = template.to_widget(id, x, y, w, h);
    layout.widgets.push(widget);
}

/// Widget neuf vide (bouton « + Nouveau » de la palette, hors
/// bibliothèque) — source à compléter dans l'inspecteur.
pub fn new_widget(layout: &mut DashboardLayout, id: String, widget_type: &str, x: i64, y: i64) {
    let (w, h) = default_size(widget_type);
    // thermo_chart : pas de source « primaire » vide (les sources viennent
    // des points du cycle, gérées par l'inspecteur) ; options thermo posées.
    // text : idem, aucune source — la validation côté serveur rejette
    // toute source sur un widget texte (`text_with_source`).
    let (source, thermo) = if widget_type == "thermo_chart" {
        (
            vec![],
            Some(ThermoChartOptions {
                diagram: "ph".into(),
                fluid: "R410A".into(),
                isolines: vec![],
                points: vec![],
                pressure: None,
                axis_decimals: None,
                pressure_unit: ThermoPressureUnit::default(),
            }),
        )
    } else if widget_type == "text" {
        (vec![], None)
    } else {
        (
            vec![SourceRef {
                role: "primary".into(),
                metric: String::new(),
                device_id: String::new(),
                window: "1h".into(),
                memory: None,
            }],
            None,
        )
    };
    layout.widgets.push(Widget {
        id,
        widget_type: widget_type.to_owned(),
        title: String::new(),
        x,
        y,
        w,
        h,
        source,
        options: WidgetOptions {
            thermo,
            ..Default::default()
        },
    });
}

/// New `symbol` widget of the catalog shape (static drawing, no source:
/// a live source is opt-in from the inspector). Size follows the aspect.
pub fn new_symbol(layout: &mut DashboardLayout, id: String, shape: &str, x: i64, y: i64) {
    let (w, h) = crate::components::symbols::default_size(shape);
    layout.widgets.push(Widget {
        id,
        widget_type: "symbol".to_owned(),
        title: String::new(),
        x,
        y,
        w,
        h,
        source: vec![],
        options: WidgetOptions {
            symbol: Some(SymbolOptions {
                shape: shape.to_owned(),
                ..Default::default()
            }),
            ..Default::default()
        },
    });
}

/// Déplace (snap déjà appliqué par l'appelant au pointer-up).
pub fn move_widget(layout: &mut DashboardLayout, id: &str, x: i64, y: i64) {
    if let Some(w) = layout.widgets.iter_mut().find(|w| w.id == id) {
        w.x = x;
        w.y = y;
    }
}

/// Redimensionne (le widget reste dans le canvas — bornes vérifiées à
/// la validation).
pub fn resize_widget(layout: &mut DashboardLayout, id: &str, w: i64, h: i64) {
    if let Some(w0) = layout.widgets.iter_mut().find(|w| w.id == id) {
        w0.w = w;
        w0.h = h;
    }
}

/// Supprime un widget **et ses traits attachés** (une accroche sans
/// widget n'existe pas).
pub fn delete_widget(layout: &mut DashboardLayout, id: &str) {
    layout.widgets.retain(|w| w.id != id);
    layout
        .wires
        .retain(|t| t.from.widget_id != id && t.to.widget_id != id);
}

/// Ajoute un trait entre deux accroches (les doublons sont ignorés :
/// un trait = une arête, école `viz_links`).
pub fn add_wire(layout: &mut DashboardLayout, wire: Wire) {
    let exists = layout.wires.iter().any(|t| {
        (t.from.widget_id == wire.from.widget_id
            && t.from.side == wire.from.side
            && t.to.widget_id == wire.to.widget_id
            && t.to.side == wire.to.side)
            || (t.from.widget_id == wire.to.widget_id
                && t.from.side == wire.to.side
                && t.to.widget_id == wire.from.widget_id
                && t.to.side == wire.from.side)
    });
    if !exists {
        layout.wires.push(wire);
    }
}

pub fn delete_wire(layout: &mut DashboardLayout, id: &str) {
    layout.wires.retain(|t| t.id != id);
}

/// Redimensionne le canvas (inspecteur) — branché en V2 (l'édition des
/// dimensions du canvas n'a pas été retenue pour la V1 de l'inspecteur).
#[allow(dead_code)]
pub fn resize_canvas(layout: &mut DashboardLayout, width: i64, height: i64) {
    layout.canvas = CanvasSpec {
        width,
        height,
        background: layout.canvas.background.clone(),
    };
}

// ───────────────────────────── Undo / Redo ─────────────────────────────

/// Pile undo/redo par snapshots entiers (cap 50 — au-delà, on perd le
/// plus ancien ; un layout complet pèse quelques Ko).
#[derive(Default, PartialEq)]
pub struct History {
    past: Vec<DashboardLayout>,
    future: Vec<DashboardLayout>,
}

const HISTORY_CAP: usize = 50;

impl History {
    /// À appeler **avant** chaque mutation (le document courant rejoint
    /// le passé). Les gestes continus (drag/resize) ne pushent qu'au
    /// pointer-down, pas à chaque frame.
    pub fn push(&mut self, current: &DashboardLayout) {
        if self.past.len() >= HISTORY_CAP {
            self.past.remove(0);
        }
        self.past.push(current.clone());
        self.future.clear();
    }

    /// Undo → document restauré, ou None si rien à annuler.
    pub fn undo(&mut self, current: &DashboardLayout) -> Option<DashboardLayout> {
        let previous = self.past.pop()?;
        self.future.push(current.clone());
        Some(previous)
    }

    /// Redo → document restauré, ou None.
    pub fn redo(&mut self, current: &DashboardLayout) -> Option<DashboardLayout> {
        let next = self.future.pop()?;
        self.past.push(current.clone());
        Some(next)
    }
}

/// Nouvel id de widget/trait — local, suffisant et lisible.
pub fn next_id(prefix: &str, counter: u32) -> String {
    format!("{prefix}-{counter:04}")
}

/// Amorce du compteur d'ids : max des suffixes numériques des ids
/// persistés (`w-…` widgets, `t-…` traits). Sinon le premier ajout d'une
/// session ré-émet `w-0001` en collision avec un id déjà en base ⇒ panic
/// dioxus « keyed siblings must each have a unique key » au rendu.
pub fn seed_counter(layout: &DashboardLayout) -> u32 {
    layout
        .widgets
        .iter()
        .map(|w| w.id.as_str())
        .chain(layout.wires.iter().map(|t| t.id.as_str()))
        .filter_map(|id| id.split_once('-'))
        .filter(|(p, _)| *p == "w" || *p == "t")
        .filter_map(|(_, n)| n.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
}

impl History {
    /// Un bouton undo grisé quand la pile du passé est vide.
    pub fn undo_disabled(&self) -> bool {
        self.past.is_empty()
    }

    /// Un bouton redo grisé quand la pile du futur est vide.
    pub fn redo_disabled(&self) -> bool {
        self.future.is_empty()
    }
}

/// Widget par id (clone — les reducers possèdent leurs données).
pub fn find_widget(layout: &DashboardLayout, id: &str) -> Option<Widget> {
    layout.widgets.iter().find(|w| w.id == id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::{WireEndpoint, WireSide};

    fn empty_layout() -> DashboardLayout {
        DashboardLayout {
            canvas: CanvasSpec {
                width: 1600,
                height: 900,
                background: None,
            },
            widgets: vec![],
            wires: vec![],
            ..Default::default()
        }
    }

    #[test]
    fn pose_move_resize_delete() {
        let mut l = empty_layout();
        let template = WidgetTemplate {
            widget_type: "gauge".into(),
            title: "T".into(),
            source: vec![],
            options: WidgetOptions::default(),
        };
        place_widget(&mut l, &template, "w-0001".into(), 40, 60);
        assert_eq!(l.widgets.len(), 1);
        assert_eq!(l.widgets[0].x, 40);
        assert_eq!(l.widgets[0].w, 240, "taille par défaut du type gauge");
        move_widget(&mut l, "w-0001", 100, 120);
        assert_eq!(l.widgets[0].x, 100);
        resize_widget(&mut l, "w-0001", 300, 260);
        assert_eq!(l.widgets[0].w, 300);
        delete_widget(&mut l, "w-0001");
        assert!(l.widgets.is_empty());
    }

    #[test]
    fn supprimer_un_widget_entraine_ses_traits() {
        let mut l = empty_layout();
        new_widget(&mut l, "w-0001".into(), "stat", 0, 0);
        new_widget(&mut l, "w-0002".into(), "stat", 400, 0);
        add_wire(
            &mut l,
            Wire {
                id: "t-0001".into(),
                from: WireEndpoint {
                    widget_id: "w-0001".into(),
                    side: WireSide::Right,
                },
                to: WireEndpoint {
                    widget_id: "w-0002".into(),
                    side: WireSide::Left,
                },
            },
        );
        assert_eq!(l.wires.len(), 1);
        // Doublon ignoré.
        add_wire(
            &mut l,
            Wire {
                id: "t-0002".into(),
                from: WireEndpoint {
                    widget_id: "w-0002".into(),
                    side: WireSide::Left,
                },
                to: WireEndpoint {
                    widget_id: "w-0001".into(),
                    side: WireSide::Right,
                },
            },
        );
        assert_eq!(l.wires.len(), 1);
        delete_widget(&mut l, "w-0001");
        assert!(l.wires.is_empty(), "le trait attaché disparaît");
    }

    #[test]
    fn undo_redo_par_snapshots() {
        let mut l = empty_layout();
        let mut history = History::default();

        history.push(&l);
        new_widget(&mut l, "w-0001".into(), "stat", 0, 0);
        assert_eq!(l.widgets.len(), 1);

        let restored = history.undo(&l).expect("undo");
        l = restored;
        assert_eq!(l.widgets.len(), 0);

        let redone = history.redo(&l).expect("redo");
        l = redone;
        assert_eq!(l.widgets.len(), 1);

        assert!(history.undo(&l).is_some(), "un undo reste possible");
        // Après ce second undo, le passé est vide : plus rien à annuler.
        assert!(history.undo(&l).is_none());
    }

    #[test]
    fn compteur_amorce_sur_les_ids_persistes() {
        let mut l = empty_layout();
        assert_eq!(seed_counter(&l), 0, "layout vide → 0");
        new_widget(&mut l, "w-0003".into(), "stat", 0, 0);
        new_widget(&mut l, "w-0011".into(), "stat", 40, 0);
        add_wire(
            &mut l,
            Wire {
                id: "t-0007".into(),
                from: WireEndpoint {
                    widget_id: "w-0003".into(),
                    side: WireSide::Right,
                },
                to: WireEndpoint {
                    widget_id: "w-0011".into(),
                    side: WireSide::Left,
                },
            },
        );
        assert_eq!(
            seed_counter(&l),
            11,
            "max des suffixes sur les deux préfixes w/t"
        );
    }

    #[test]
    fn push_avec_cap() {
        let mut history = History::default();
        let mut l = empty_layout();
        for _ in 0..60 {
            history.push(&l);
            l.canvas.width += 1;
        }
        assert!(history.past.len() <= 50, "cap respecté");
    }
}
