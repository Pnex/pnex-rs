//! Reducers purs du document dashboard (école `flow_editor/state.rs`) —
//! **toutes** les mutations du layout passent ici, jamais en ligne dans
//! le rsx. Chaque reducer renvoie un nouveau document ; l'undo/redo
//! (absent du flow editor, demandé par la PRD §5) empile des snapshots
//! entiers — un synoptique fait quelques Ko, la simplicité gagne.

use pnex_core::{
    CanvasSpec, DashboardFormat, DashboardLayout, MobileSection, SourceRef, SymbolOptions,
    ThermoChartOptions, ThermoPressureUnit, Widget, WidgetOptions, WidgetTemplate, Wire,
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
    } else if widget_type == "text" || pnex_core::CONTROL_WIDGET_TYPES.contains(&widget_type) {
        // Control cards: the state source is optional (opt-in from the
        // inspector), the control itself is picked there.
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
    for sec in &mut layout.sections {
        sec.items.retain(|i| i != id);
    }
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

// ─────────────────────────── Mobile composer (D124) ───────────────────────────

/// Next free section id (`s-N`, max + 1 of the existing ones).
pub fn next_section_id(layout: &DashboardLayout) -> String {
    let n = layout
        .sections
        .iter()
        .filter_map(|s| s.id.strip_prefix("s-"))
        .filter_map(|n| n.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("s-{}", n + 1)
}

/// Appends a section; returns its id.
pub fn add_section(layout: &mut DashboardLayout, title: &str) -> String {
    let id = next_section_id(layout);
    layout.sections.push(MobileSection {
        id: id.clone(),
        title: title.to_owned(),
        items: vec![],
    });
    id
}

pub fn rename_section(layout: &mut DashboardLayout, id: &str, title: &str) {
    if let Some(sec) = layout.sections.iter_mut().find(|s| s.id == id) {
        sec.title = title.to_owned();
    }
}

/// Removes a section; its cards join the previous section (the next one
/// for the first). The last section is never removed (a card always has
/// a home).
pub fn delete_section(layout: &mut DashboardLayout, id: &str) {
    if layout.sections.len() < 2 {
        return;
    }
    let Some(pos) = layout.sections.iter().position(|s| s.id == id) else {
        return;
    };
    let removed = layout.sections.remove(pos);
    let target = pos.saturating_sub(1).min(layout.sections.len() - 1);
    if pos == 0 {
        let mut items = removed.items;
        items.append(&mut layout.sections[0].items);
        layout.sections[0].items = items;
    } else {
        layout.sections[target].items.extend(removed.items);
    }
}

/// Moves a section up (`-1`) or down (`+1`).
pub fn shift_section(layout: &mut DashboardLayout, id: &str, delta: i32) {
    let Some(pos) = layout.sections.iter().position(|s| s.id == id) else {
        return;
    };
    let to = pos as i64 + i64::from(delta);
    if to < 0 || to >= layout.sections.len() as i64 {
        return;
    }
    layout.sections.swap(pos, to as usize);
}

/// Puts a card at the end of `section` (the last section when `None` or
/// unknown; a first section is created when there is none).
pub fn place_in_section(layout: &mut DashboardLayout, widget_id: &str, section: Option<&str>) {
    for sec in &mut layout.sections {
        sec.items.retain(|i| i != widget_id);
    }
    if layout.sections.is_empty() {
        add_section(layout, "");
    }
    let pos = section
        .and_then(|id| layout.sections.iter().position(|s| s.id == id))
        .unwrap_or(layout.sections.len() - 1);
    layout.sections[pos].items.push(widget_id.to_owned());
}

/// Moves a card into `section` at `index` (clamped; drag and drop).
pub fn move_card(layout: &mut DashboardLayout, widget_id: &str, section: &str, index: usize) {
    if !layout.sections.iter().any(|s| s.id == section) {
        return;
    }
    for sec in &mut layout.sections {
        sec.items.retain(|i| i != widget_id);
    }
    if let Some(sec) = layout.sections.iter_mut().find(|s| s.id == section) {
        let index = index.min(sec.items.len());
        sec.items.insert(index, widget_id.to_owned());
    }
}

/// Moves a card just before `before_id` (drop on a card; any section).
pub fn move_card_before(layout: &mut DashboardLayout, widget_id: &str, before_id: &str) {
    if widget_id == before_id || card_position(layout, before_id).is_none() {
        return;
    }
    for sec in &mut layout.sections {
        sec.items.retain(|i| i != widget_id);
    }
    if let Some((section, index)) = card_position(layout, before_id) {
        if let Some(sec) = layout.sections.iter_mut().find(|s| s.id == section) {
            sec.items.insert(index, widget_id.to_owned());
        }
    }
}

/// Section id and index of a card.
pub fn card_position(layout: &DashboardLayout, widget_id: &str) -> Option<(String, usize)> {
    layout.sections.iter().find_map(|s| {
        s.items
            .iter()
            .position(|i| i == widget_id)
            .map(|p| (s.id.clone(), p))
    })
}

/// Moves a card one step back (`-1`) or forward (`+1`); past the edge of
/// its section it joins the end of the previous / the start of the next.
pub fn shift_card(layout: &mut DashboardLayout, widget_id: &str, delta: i32) {
    let Some(si) = layout
        .sections
        .iter()
        .position(|s| s.items.iter().any(|i| i == widget_id))
    else {
        return;
    };
    let items = &mut layout.sections[si].items;
    let pos = items.iter().position(|i| i == widget_id).unwrap_or(0);
    let to = pos as i64 + i64::from(delta);
    if to >= 0 && (to as usize) < items.len() {
        items.swap(pos, to as usize);
        return;
    }
    let card = items.remove(pos);
    if to < 0 && si > 0 {
        layout.sections[si - 1].items.push(card);
    } else if to >= 0 && si + 1 < layout.sections.len() {
        layout.sections[si + 1].items.insert(0, card);
    } else {
        // No neighbour section: the card stays where it was.
        layout.sections[si].items.insert(pos, card);
    }
}

/// Card width on mobile: 1 = half, 2 = full row.
pub fn set_span(layout: &mut DashboardLayout, widget_id: &str, span: u8) {
    if let Some(w) = layout.widgets.iter_mut().find(|w| w.id == widget_id) {
        w.options.span = Some(span.clamp(1, 2));
    }
}

/// Empty layout of a new dashboard of `format` (mobile: one section).
pub fn initial_layout(format: DashboardFormat, first_section: &str) -> DashboardLayout {
    let mut layout = DashboardLayout {
        format,
        canvas: CanvasSpec {
            width: 1600,
            height: 900,
            background: None,
        },
        ..Default::default()
    };
    if format == DashboardFormat::Mobile {
        add_section(&mut layout, first_section);
    }
    layout
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

    fn mobile() -> DashboardLayout {
        let mut l = initial_layout(DashboardFormat::Mobile, "General");
        for id in ["w-0001", "w-0002", "w-0003"] {
            new_widget(&mut l, id.into(), "switch", 0, 0);
            place_in_section(&mut l, id, None);
        }
        l
    }

    fn items(l: &DashboardLayout) -> Vec<Vec<&str>> {
        l.sections
            .iter()
            .map(|s| s.items.iter().map(String::as_str).collect())
            .collect()
    }

    #[test]
    fn mobile_cards_move_within_and_across_sections() {
        let mut l = mobile();
        assert_eq!(items(&l), vec![vec!["w-0001", "w-0002", "w-0003"]]);
        let s2 = add_section(&mut l, "Garage");
        assert_eq!(s2, "s-2");
        // Past the end of the first section → start of the next one.
        shift_card(&mut l, "w-0003", 1);
        assert_eq!(items(&l), vec![vec!["w-0001", "w-0002"], vec!["w-0003"]]);
        shift_card(&mut l, "w-0001", 1);
        assert_eq!(items(&l), vec![vec!["w-0002", "w-0001"], vec!["w-0003"]]);
        // Back past the start → end of the previous section.
        shift_card(&mut l, "w-0003", -1);
        assert_eq!(items(&l), vec![vec!["w-0002", "w-0001", "w-0003"], vec![]]);
        // No neighbour: unchanged.
        shift_card(&mut l, "w-0002", -1);
        assert_eq!(items(&l)[0][0], "w-0002");
        // Drop on a card: lands just before it, whatever the direction.
        move_card_before(&mut l, "w-0002", "w-0003");
        assert_eq!(items(&l)[0], vec!["w-0001", "w-0002", "w-0003"]);
        move_card_before(&mut l, "w-0003", "w-0001");
        assert_eq!(items(&l)[0], vec!["w-0003", "w-0001", "w-0002"]);
        move_card_before(&mut l, "w-0003", "w-0002");
        assert_eq!(items(&l)[0], vec!["w-0001", "w-0003", "w-0002"]);
        move_card_before(&mut l, "w-0003", "w-0001");
        move_card_before(&mut l, "w-0002", "w-0001");
        assert_eq!(items(&l)[0], vec!["w-0003", "w-0002", "w-0001"]);
        move_card_before(&mut l, "w-0002", "w-0003");
        move_card_before(&mut l, "w-0001", "w-0003");
        assert_eq!(items(&l)[0], vec!["w-0002", "w-0001", "w-0003"]);
        move_card(&mut l, "w-0001", "s-2", 9);
        assert_eq!(items(&l), vec![vec!["w-0002", "w-0003"], vec!["w-0001"]]);
        assert_eq!(card_position(&l, "w-0001"), Some(("s-2".into(), 0)));
        assert!(pnex_core::validate_layout(&l)
            .iter()
            .all(|v| v.code == "control_missing"));
    }

    #[test]
    fn mobile_sections_delete_keeps_cards_and_widgets_leave_sections() {
        let mut l = mobile();
        add_section(&mut l, "B");
        move_card(&mut l, "w-0002", "s-2", 0);
        // Deleting a section hands its cards to the previous one.
        delete_section(&mut l, "s-2");
        assert_eq!(items(&l), vec![vec!["w-0001", "w-0003", "w-0002"]]);
        // The last section stays.
        delete_section(&mut l, "s-1");
        assert_eq!(l.sections.len(), 1);
        // Deleting the first section hands its cards to the next one, in front.
        add_section(&mut l, "C");
        move_card(&mut l, "w-0001", "s-2", 0);
        delete_section(&mut l, "s-1");
        assert_eq!(items(&l), vec![vec!["w-0003", "w-0002", "w-0001"]]);
        delete_widget(&mut l, "w-0002");
        assert_eq!(items(&l), vec![vec!["w-0003", "w-0001"]]);
        set_span(&mut l, "w-0003", 7);
        assert_eq!(find_widget(&l, "w-0003").unwrap().options.span, Some(2));
        shift_section(&mut l, "s-2", -1);
        assert_eq!(l.sections[0].id, "s-2");
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
