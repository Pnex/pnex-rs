//! Documents « dashboard » du studio SCADA — modèle typé du layout stocké
//! en JSONB (`dashboard_versions.layout`, école `flow.rs` : la même
//! structure valide côté navigateur avant save et côté serveur en 400).
//!
//! Particularités D40/D41 (PRD `docs/architecture/viz-bases.md`) :
//! - **Canvas libre à main levée** — coordonnées **px absolues** dans un
//!   canvas dimensionné (pas de grille) ;
//! - **Traits et accroches** (`Wire`) — liaisons **purement visuelles**
//!   entre widgets (synoptique de process : flux, boucles PID *affichées*)
//!   : zéro sémantique d'exécution, les extrémités sont accrochées aux
//!   milieux des côtés et suivent le widget au déplacement ;
//! - presets de fenêtre partagés avec la lecture télémétrie (source
//!   unique : le service `visualization` et le front lisent la même liste,
//!   école `normalize_measurement_name` — si validation et requête
//!   divergeaient, le PromQL chercherait une fenêtre rejetée).

use serde::{Deserialize, Serialize};

use crate::naming;

/// Types de widget du kit SCADA V1 — enum **ouverte** : un nouveau type
/// s'ajoute ici sans migration (la colonne `kind` reste un string).
/// `thermo_chart` = diagramme thermodynamique (p-h, T-s, psychrométrique)
/// avec cycle live, données via `WidgetOptions.thermo`.
/// `symbol` = process / flowchart symbol (P&ID library), data via
/// `WidgetOptions.symbol`, optionally animated by one source.
pub const VIZ_WIDGET_TYPES: &[&str] = &[
    "gauge",
    "stat",
    "line",
    "indicator",
    "text",
    "thermo_chart",
    "symbol",
    "switch",
    "slider",
    "button",
    "number",
];

/// Control widget types (D125): each drives one org control
/// (`WidgetOptions.control`, provisioned at save when absent, D131) and may
/// show one optional state source.
pub const CONTROL_WIDGET_TYPES: &[&str] = &["switch", "slider", "button", "number"];

/// Max number of sections of a mobile dashboard (D124).
pub const MOBILE_SECTIONS_MAX: usize = 32;

/// Fenêtres proposées par widget (clé → secondes). Source de vérité
/// partagée front/back — `services/visualization.rs::WINDOWS` s'aligne
/// dessus. Les fenêtres courtes (5m/15m/30m) servent au « dernier » des
/// jauges (lookback configurable).
pub const VIZ_WINDOW_PRESETS: &[(&str, i64)] = &[
    ("5m", 300),
    ("15m", 900),
    ("30m", 1_800),
    ("1h", 3_600),
    ("6h", 21_600),
    ("24h", 86_400),
];

/// Clé de preset valide ?
pub fn valid_window(key: &str) -> bool {
    VIZ_WINDOW_PRESETS.iter().any(|(k, _)| *k == key)
}

/// Bornes du canvas (px) — assez grand pour un synoptique d'atelier,
/// assez borné pour qu'un layout piégé ne gèle pas le rendu.
pub const CANVAS_MIN: i64 = 400;
pub const CANVAS_MAX: i64 = 8_000;
/// Taille minimale d'un widget (px) — en dessous, un instrument est
/// illisible et ses accroches deviennent injoignables.
pub const WIDGET_MIN: i64 = 40;
const TITLE_MAX: usize = 200;
const TEXT_MAX: usize = 1_000;

/// Violation de validation d'un layout (rejetée en 400 par l'API,
/// champ `violations` — école `FlowViolation`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VizViolation {
    /// Widget fautif, si la violation est localisée.
    pub widget_id: Option<String>,
    /// Code machine (`unknown_widget_type`, `dangling_wire`…).
    pub code: String,
    /// Message en français, affichable tel quel au client.
    pub message: String,
}

impl VizViolation {
    pub fn new(widget_id: Option<&str>, code: &str, message: impl Into<String>) -> Self {
        Self {
            widget_id: widget_id.map(str::to_owned),
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

/// Dimensions du canvas.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CanvasSpec {
    pub width: i64,
    pub height: i64,
    /// Fond du synoptique (`#rrggbb`), défaut clair.
    #[serde(default)]
    pub background: Option<String>,
}

/// Extrémité d'un trait : accrochée au **milieu d'un côté** du widget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireEndpoint {
    pub widget_id: String,
    /// Côté d'accroche.
    pub side: WireSide,
}

/// Côtés d'accroche (milieu du côté).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WireSide {
    Top,
    Bottom,
    Left,
    Right,
}

/// Trait entre deux widgets — **décoratif**, aucune sémantique
/// d'exécution (le rendu est un coude orthogonal, école câbles flow).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wire {
    pub id: String,
    pub from: WireEndpoint,
    pub to: WireEndpoint,
}

/// Référence d'une série télémétrie par rôle (école PRD §2 : `source`
/// est une liste — le mini-chart peut superposer, la jauge lit
/// `primary`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    #[serde(default = "default_role")]
    pub role: String,
    pub metric: String,
    pub device_id: String,
    /// Preset de fenêtre (`VIZ_WINDOW_PRESETS`) — le lookback du
    /// « dernier » ET la plage du mini-chart.
    pub window: String,
    /// Org shared memory value (flow `memory-write` node) instead of a
    /// telemetry series: `metric`/`device_id`/`window` are then ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<crate::memory::MemoryRef>,
}

impl SourceRef {
    /// Key of this source in the live-values map.
    pub fn series_key(&self) -> String {
        match &self.memory {
            Some(m) => m.series_key(),
            None => format!("{}|{}", self.metric, self.device_id),
        }
    }
}

fn default_role() -> String {
    "primary".to_owned()
}

/// Options d'affichage par type de widget.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WidgetOptions {
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// Décimales affichées (0..=6).
    #[serde(default)]
    pub decimals: Option<u8>,
    /// Seuils de couleur (`gauge`/`indicator`), valeur → couleur.
    #[serde(default)]
    pub thresholds: Vec<Threshold>,
    /// Texte libre du widget `text`.
    #[serde(default)]
    pub text: Option<String>,
    /// Options du widget `thermo_chart` (présentes ⇔ type thermo_chart).
    #[serde(default)]
    pub thermo: Option<ThermoChartOptions>,
    /// `symbol` widget options (present iff type is symbol).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<SymbolOptions>,
    /// Org control driven by a control widget (only on
    /// [`CONTROL_WIDGET_TYPES`]; absent = the server provisions the widget's
    /// own control at save, D131).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<crate::ui_control::ControlRef>,
    /// Mobile card width: 1 = half, 2 = full row (default 2). Ignored on
    /// desktop dashboards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<u8>,
    /// Value → colour / icon / label rules (D135), first match wins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<StateRule>,
    /// Card icon (home icon catalog id, D136) when no state rule sets one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// The card greys out when its newest point is older than this many
    /// seconds (D135). `None` = never stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_after_s: Option<u32>,
}

/// Options of the `symbol` widget — a shape of the front-end symbol
/// catalog (draw.io P&ID + flowchart stencils). The catalog lives in the
/// front end only: the server checks the id *format*, an unknown id renders
/// as a placeholder (open enum, like widget types).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SymbolOptions {
    /// Catalog id (`pid-valves-gate-valve`, `flowchart-decision`…).
    pub shape: String,
    /// Stroke colour `#rrggbb` (default: dark gray).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<String>,
    /// Fill colour `#rrggbb` (default: white). With a source and
    /// thresholds, the threshold colour replaces it live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    /// Clockwise rotation in degrees: 0, 90, 180 or 270.
    #[serde(default)]
    pub rotation: u16,
    /// Horizontal mirror (applied before the rotation).
    #[serde(default)]
    pub flip: bool,
    /// Stretch to the widget box instead of keeping the aspect ratio.
    #[serde(default)]
    pub stretch: bool,
    /// Show the live value of the source under the symbol.
    #[serde(default)]
    pub show_value: bool,
}

/// Max length of a symbol catalog id.
pub const SYMBOL_ID_MAX: usize = 96;

/// Symbol catalog id format: `[a-z0-9-]`, 1..=96 chars.
pub fn valid_symbol_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= SYMBOL_ID_MAX
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Rules of the `symbol` widget: known-format shape id, hex colours, right
/// angle rotation. The source is optional (a static symbol is valid).
fn validate_symbol(widget_id: &str, options: &WidgetOptions) -> Vec<VizViolation> {
    let mut v: Vec<VizViolation> = Vec::new();
    let mut push = |code: &str, message: String| {
        v.push(VizViolation::new(Some(widget_id), code, message));
    };
    let Some(sym) = &options.symbol else {
        push(
            "symbol_options_missing",
            "symbol options are required".into(),
        );
        return v;
    };
    if !valid_symbol_id(&sym.shape) {
        push(
            "symbol_bad_shape",
            format!("invalid symbol id \"{}\"", sym.shape),
        );
    }
    for c in [&sym.stroke, &sym.fill].into_iter().flatten() {
        if !valid_hex_color(c) {
            push("symbol_bad_color", "invalid symbol colour (#rrggbb)".into());
        }
    }
    if ![0, 90, 180, 270].contains(&sym.rotation) {
        push(
            "symbol_bad_rotation",
            "rotation must be 0, 90, 180 or 270".into(),
        );
    }
    v
}

/// Options du widget `thermo_chart` — diagramme thermodynamique (dôme +
/// iso-lignes servies par `POST /api/v1/thermo/diagram`) et cycle live
/// (points résolus par `POST /api/v1/thermo/cycle-points`). Tout
/// `#[serde(default)]` : extension sans migration (JSONB).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ThermoChartOptions {
    /// `"ph" | "ts" | "psychro"` (miroir de `pnex_coolprop::DiagramKind` —
    /// pnex-core reste wasm-safe, pas de dépendance à pnex-coolprop).
    pub diagram: String,
    /// Fluide pur, prédéfini (R410A…), spec inline ou nom de mélange org.
    pub fluid: String,
    /// Iso-lignes affichées (P/T/Q pour ph/ts, RH pour psychro).
    #[serde(default)]
    pub isolines: Vec<ThermoIsoline>,
    /// Points du cycle — chaque point = 2 sources télémétriques.
    #[serde(default)]
    pub points: Vec<ThermoCyclePoint>,
    /// Pression psychrométrique (défaut 101 325 Pa).
    #[serde(default)]
    pub pressure: Option<f64>,
    /// Arrondi des labels d'axes (décimales) — `None` = auto (significatif
    /// selon l'ordre de grandeur).
    #[serde(default)]
    pub axis_decimals: Option<u8>,
    /// Unité d'affichage de l'axe pression du p-h — **absolue** toujours
    /// (conversion d'affichage Pa → bar = ÷1e5 ; le relatif serait non
    /// physique sur un diagramme thermodynamique).
    #[serde(default)]
    pub pressure_unit: ThermoPressureUnit,
}

/// Unité d'affichage de l'axe pression du p-h. Absolu par nature —
/// CoolProp calcule en Pa absolus, la conversion est purement cosmétique.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThermoPressureUnit {
    #[default]
    Pa,
    Bar,
}

/// Iso-ligne du widget thermo (`P`, `T`, `Q` ou `RH` + valeurs SI).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThermoIsoline {
    pub param: String,
    pub values: Vec<f64>,
}

/// Un point du cycle : paire d'inputs CoolProp + 2 sources télémétriques.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThermoCyclePoint {
    pub label: String,
    /// Nom de paire CoolProp (`PT_INPUTS` par défaut ; v1 = pression,
    /// v2 = température).
    #[serde(default = "default_thermo_input_pair")]
    pub input_pair: String,
    /// Source télémétrique de v1 (pour `PT_INPUTS` : la pression).
    pub v1: ThermoSource,
    /// Source télémétrique de v2 (pour `PT_INPUTS` : la température).
    pub v2: ThermoSource,
}

fn default_thermo_input_pair() -> String {
    "PT_INPUTS".into()
}

/// Une source télémétrique d'un point de cycle (dernière valeur de la
/// série `metric` du `device_id`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThermoSource {
    #[serde(default)]
    pub metric: String,
    #[serde(default)]
    pub device_id: String,
    /// Org shared memory value instead of a telemetry series.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<crate::memory::MemoryRef>,
}

impl ThermoSource {
    /// Key of this source in the live-values map.
    pub fn series_key(&self) -> String {
        match &self.memory {
            Some(m) => m.series_key(),
            None => format!("{}|{}", self.metric, self.device_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Threshold {
    pub value: f64,
    /// Couleur `#rrggbb`.
    pub color: String,
}

/// Max number of state rules of a widget (D135).
pub const STATE_RULES_MAX: usize = 16;
/// Max length of a state rule label (chars).
pub const STATE_LABEL_MAX: usize = 48;
/// Bounds of `stale_after_s` (10 s .. 7 days).
pub const STALE_AFTER_MIN_S: u32 = 10;
pub const STALE_AFTER_MAX_S: u32 = 604_800;

/// Comparison of a state rule against the live value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateOp {
    /// Equal (within 1e-9): discrete states, `0` = closed, `1` = open.
    #[default]
    Eq,
    /// Greater than or equal.
    Gte,
    /// Less than or equal.
    Lte,
}

/// One value → appearance rule of a card (D135). Every field but the
/// comparison is optional: a rule may only relabel, only recolour, etc.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StateRule {
    #[serde(default)]
    pub op: StateOp,
    pub value: f64,
    /// `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Home icon catalog id (D136).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Text shown instead of the number (plain text, never HTML).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl StateRule {
    pub fn matches(&self, v: f64) -> bool {
        match self.op {
            StateOp::Eq => (v - self.value).abs() < 1e-9,
            StateOp::Gte => v >= self.value,
            StateOp::Lte => v <= self.value,
        }
    }
}

/// First rule matching `v` (rule order = priority).
pub fn resolve_state(rules: &[StateRule], v: f64) -> Option<&StateRule> {
    rules.iter().find(|r| r.matches(v))
}

/// Is the newest point (`ts`, epoch seconds like [`crate::TelemetryPoint`])
/// older than the widget's staleness budget at `now` (epoch seconds)?
pub fn is_stale(options: &WidgetOptions, ts: f64, now: f64) -> bool {
    match options.stale_after_s {
        Some(s) => now - ts > f64::from(s),
        None => false,
    }
}

fn validate_states(options: &WidgetOptions, push: &mut impl FnMut(&str, String)) {
    if options.states.len() > STATE_RULES_MAX {
        push(
            "states_too_many",
            format!("at most {STATE_RULES_MAX} state rules"),
        );
    }
    for r in &options.states {
        if !r.value.is_finite() {
            push("state_bad_value", "state rule value must be finite".into());
        }
        if r.color.as_deref().is_some_and(|c| !valid_hex_color(c)) {
            push("state_bad_color", "invalid state colour (#rrggbb)".into());
        }
        if r.icon.as_deref().is_some_and(|i| !valid_symbol_id(i)) {
            push("state_bad_icon", "invalid state icon id".into());
        }
        if let Some(l) = &r.label {
            if l.chars().count() > STATE_LABEL_MAX || l.chars().any(char::is_control) {
                push("state_bad_label", "state label too long or invalid".into());
            }
        }
    }
    if options.icon.as_deref().is_some_and(|i| !valid_symbol_id(i)) {
        push("bad_icon", "invalid card icon id".into());
    }
    if let Some(s) = options.stale_after_s {
        if !(STALE_AFTER_MIN_S..=STALE_AFTER_MAX_S).contains(&s) {
            push(
                "bad_stale_after",
                format!("stale delay must be within {STALE_AFTER_MIN_S}..={STALE_AFTER_MAX_S} s"),
            );
        }
    }
}

/// Un widget posé sur le canvas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Widget {
    pub id: String,
    /// Un de [`VIZ_WIDGET_TYPES`] (string validé — enum ouverte).
    #[serde(rename = "type")]
    pub widget_type: String,
    #[serde(default)]
    pub title: String,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    #[serde(default)]
    pub source: Vec<SourceRef>,
    #[serde(default)]
    pub options: WidgetOptions,
}

/// Dashboard format, chosen at creation and never changed (D123).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DashboardFormat {
    /// Free-hand canvas in absolute px (D40).
    #[default]
    Desktop,
    /// Stack of cards grouped in sections, no px positioning (D124).
    Mobile,
}

/// A section of a mobile dashboard: ordered widget ids.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MobileSection {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub items: Vec<String>,
}

/// Layout d'un dashboard — stocké en JSONB dans `dashboard_versions`.
/// Les points télémétrie ne sont **jamais** stockés (D31).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DashboardLayout {
    /// Existing layouts deserialize as desktop.
    #[serde(default)]
    pub format: DashboardFormat,
    pub canvas: CanvasSpec,
    #[serde(default)]
    pub widgets: Vec<Widget>,
    #[serde(default)]
    pub wires: Vec<Wire>,
    /// Mobile only: widget order and grouping (every widget in exactly one
    /// section).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<MobileSection>,
}

// ─────────────────────────────── Validation ───────────────────────────────

/// Règles du widget `thermo_chart` : diagramme connu, fluide requis, au
/// plus 8 points, chaque point = 2 sources valides **et présentes dans
/// `widget.source`** (le series-batch agrège `source`, pas `options`).
fn validate_thermo(
    widget_id: &str,
    source: &[SourceRef],
    options: &WidgetOptions,
) -> Vec<VizViolation> {
    let mut v: Vec<VizViolation> = Vec::new();
    let mut push = |code: &str, message: String| {
        v.push(VizViolation::new(Some(widget_id), code, message));
    };
    let Some(t) = &options.thermo else {
        push(
            "thermo_options_missing",
            "options thermo requises (diagramme, fluide, points)".into(),
        );
        return v;
    };
    if !["ph", "ts", "psychro"].contains(&t.diagram.as_str()) {
        push(
            "thermo_bad_diagram",
            format!("diagramme inconnu : « {} » (ph, ts, psychro)", t.diagram),
        );
    }
    if t.fluid.trim().is_empty() {
        push("thermo_fluid_missing", "le fluide est requis".into());
    }
    if t.points.len() > 8 {
        push("thermo_too_many_points", "au plus 8 points de cycle".into());
    }
    for (i, p) in t.points.iter().enumerate() {
        if p.label.trim().is_empty() {
            push(
                "thermo_point_label",
                format!("point {} : label requis", i + 1),
            );
        }
        for (slot, s) in [("v1", &p.v1), ("v2", &p.v2)] {
            if let Some(m) = &s.memory {
                if !m.is_valid() {
                    push(
                        "bad_memory_key",
                        format!("point {} : invalid memory key \"{}\"", i + 1, m.key),
                    );
                }
            } else if !naming::valid_metric_name(&s.metric) {
                push(
                    "bad_metric",
                    format!("point {} : métrique invalide « {} »", i + 1, s.metric),
                );
            }
            if s.memory.is_none() && !naming::valid_device_label(&s.device_id) {
                push(
                    "bad_device",
                    format!("point {} : device invalide « {} »", i + 1, s.device_id),
                );
            }
            // La source doit être répliquée dans widget.source (batch).
            let key = s.series_key();
            let present = source.iter().any(|src| src.series_key() == key);
            if !present {
                push(
                    "thermo_source_missing",
                    format!(
                        "point {} ({slot}) : add the source {key} to the widget sources",
                        i + 1
                    ),
                );
            }
        }
    }
    v
}

/// Valide la structure du layout. Retourne **toutes** les violations
/// (l'API les renvoie en 400, le front les affiche avant save).
pub fn validate_layout(l: &DashboardLayout) -> Vec<VizViolation> {
    let mut v = Vec::new();
    let desktop = l.format == DashboardFormat::Desktop;
    // The canvas only exists on desktop: a mobile layout is a card stack.
    if desktop
        && (!(CANVAS_MIN..=CANVAS_MAX).contains(&l.canvas.width)
            || !(CANVAS_MIN..=CANVAS_MAX).contains(&l.canvas.height))
    {
        v.push(VizViolation::new(
            None,
            "canvas_bounds",
            format!(
                "canvas {}×{} hors bornes ({CANVAS_MIN}..={CANVAS_MAX} px)",
                l.canvas.width, l.canvas.height
            ),
        ));
    }
    if let Some(bg) = &l.canvas.background {
        if !valid_hex_color(bg) {
            v.push(VizViolation::new(
                None,
                "canvas_color",
                "couleur de fond invalide (attendu #rrggbb)",
            ));
        }
    }

    let mut seen = std::collections::HashSet::new();
    for w in &l.widgets {
        if w.id.trim().is_empty() || w.id.len() > 128 {
            v.push(VizViolation::new(
                None,
                "widget_id",
                "id de widget absent ou trop long",
            ));
            continue;
        }
        if !seen.insert(w.id.clone()) {
            v.push(VizViolation::new(
                Some(&w.id),
                "duplicate_widget_id",
                format!("id de widget dupliqué : « {} »", w.id),
            ));
        }
        if !VIZ_WIDGET_TYPES.contains(&w.widget_type.as_str()) {
            v.push(VizViolation::new(
                Some(&w.id),
                "unknown_widget_type",
                format!("type de widget inconnu : « {} »", w.widget_type),
            ));
        }
        if w.title.len() > TITLE_MAX || w.title.chars().any(char::is_control) {
            v.push(VizViolation::new(
                Some(&w.id),
                "widget_title",
                "titre absent de borne ou porteur de caractères de contrôle",
            ));
        }
        if !desktop {
            if !matches!(w.options.span, None | Some(1) | Some(2)) {
                v.push(VizViolation::new(
                    Some(&w.id),
                    "widget_span",
                    "card span must be 1 (half) or 2 (full row)",
                ));
            }
        } else if w.x < 0 || w.y < 0 || w.w < WIDGET_MIN || w.h < WIDGET_MIN {
            v.push(VizViolation::new(
                Some(&w.id),
                "widget_geometry",
                format!("géométrie invalide (x/y ≥ 0, w/h ≥ {WIDGET_MIN} px)"),
            ));
        } else if w.x + w.w > l.canvas.width || w.y + w.h > l.canvas.height {
            v.push(VizViolation::new(
                Some(&w.id),
                "widget_overflow",
                "le widget dépasse le canvas",
            ));
        }
        validate_widget(&w.id, &w.widget_type, &w.source, &w.options, &mut v);
    }

    if desktop {
        if !l.sections.is_empty() {
            v.push(VizViolation::new(
                None,
                "desktop_sections",
                "sections only exist on mobile dashboards",
            ));
        }
    } else {
        if !l.wires.is_empty() {
            v.push(VizViolation::new(
                None,
                "mobile_wires_forbidden",
                "wires only exist on desktop dashboards",
            ));
        }
        validate_sections(l, &seen, &mut v);
    }

    // Traits : extrémités accrochées à des widgets existants, jamais à
    // soi-même (un coude sur soi-même n'a pas de sens).
    for wire in &l.wires {
        if wire.id.trim().is_empty() || wire.id.len() > 128 {
            v.push(VizViolation::new(
                None,
                "wire_id",
                "id de trait absent ou trop long",
            ));
            continue;
        }
        for end in [&wire.from, &wire.to] {
            if !seen.contains(&end.widget_id) {
                v.push(VizViolation::new(
                    Some(&end.widget_id),
                    "dangling_wire",
                    format!("le trait « {} » référence un widget inconnu", wire.id),
                ));
            }
        }
        if wire.from.widget_id == wire.to.widget_id {
            v.push(VizViolation::new(
                Some(&wire.from.widget_id),
                "self_wire",
                "un trait ne peut pas relier un widget à lui-même",
            ));
        }
    }
    v
}

/// Mobile rules (D124): unique non-empty section ids, bounded count and
/// titles, every widget placed in exactly one section, no unknown item.
fn validate_sections(
    l: &DashboardLayout,
    widget_ids: &std::collections::HashSet<String>,
    v: &mut Vec<VizViolation>,
) {
    if l.sections.len() > MOBILE_SECTIONS_MAX {
        v.push(VizViolation::new(
            None,
            "sections_too_many",
            format!("at most {MOBILE_SECTIONS_MAX} sections"),
        ));
    }
    let mut section_ids = std::collections::HashSet::new();
    let mut placed: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for sec in &l.sections {
        if sec.id.trim().is_empty() || sec.id.len() > 128 || !section_ids.insert(sec.id.as_str()) {
            v.push(VizViolation::new(
                None,
                "section_id",
                "section id missing, too long or duplicated",
            ));
        }
        if sec.title.len() > TITLE_MAX || sec.title.chars().any(char::is_control) {
            v.push(VizViolation::new(
                None,
                "section_title",
                "section title too long or carrying control characters",
            ));
        }
        for item in &sec.items {
            if !widget_ids.contains(item) {
                v.push(VizViolation::new(
                    Some(item),
                    "section_unknown_widget",
                    format!("section \"{}\" references an unknown widget", sec.id),
                ));
            }
            *placed.entry(item.as_str()).or_default() += 1;
        }
    }
    for w in &l.widgets {
        match placed.get(w.id.as_str()).copied().unwrap_or(0) {
            1 => {}
            0 => v.push(VizViolation::new(
                Some(&w.id),
                "widget_unplaced",
                "the card is in no section",
            )),
            _ => v.push(VizViolation::new(
                Some(&w.id),
                "widget_placed_twice",
                "the card is in several sections",
            )),
        }
    }
}

/// Valide type + sources + options d'un widget (réutilisé par la
/// bibliothèque : `viz_widget_library.config` est la même forme sans
/// position).
pub fn validate_widget(
    widget_id: &str,
    widget_type: &str,
    source: &[SourceRef],
    options: &WidgetOptions,
    v: &mut Vec<VizViolation>,
) {
    let mut extra_violations: Vec<VizViolation> = Vec::new();
    let mut push = |code: &str, message: String| {
        v.push(VizViolation::new(Some(widget_id), code, message));
    };
    if !VIZ_WIDGET_TYPES.contains(&widget_type) {
        push(
            "unknown_widget_type",
            format!("type de widget inconnu : « {widget_type} »"),
        );
        return;
    }
    // A control widget without a control is valid: the server provisions
    // its own control when the dashboard is saved (D131).
    let is_control = CONTROL_WIDGET_TYPES.contains(&widget_type);
    if !is_control && options.control.is_some() {
        push(
            "control_unexpected",
            "only control widgets drive a control".into(),
        );
    }
    if is_control {
        // The state source is optional (fallback: last commanded value).
        if source.len() > 1 {
            push(
                "control_state_sources",
                "a control widget shows at most one state source".into(),
            );
        }
        check_sources(widget_type, source, &mut push);
    } else if widget_type == "text" {
        // Le widget texte n'affiche pas de série.
        if !source.is_empty() {
            push(
                "text_with_source",
                "un widget texte ne porte pas de source".into(),
            );
        }
        match &options.text {
            Some(t) if !t.trim().is_empty() && t.len() <= TEXT_MAX => {}
            _ => push("text_missing", "le texte du widget est requis".into()),
        }
    } else if widget_type == "thermo_chart" {
        extra_violations = validate_thermo(widget_id, source, options);
    } else if widget_type == "symbol" && source.is_empty() {
        // A symbol without source is a static drawing element.
        extra_violations = validate_symbol(widget_id, options);
    } else {
        if widget_type == "symbol" {
            extra_violations = validate_symbol(widget_id, options);
        }
        // Les instruments lisent au moins une série, rôle primaire en tête.
        if source.is_empty() {
            push(
                "source_missing",
                "une source (métrique × device × fenêtre) est requise".into(),
            );
        }
        check_sources(widget_type, source, &mut push);
    }
    if let (Some(min), Some(max)) = (options.min, options.max) {
        if min >= max {
            push("bad_range", "min doit être < max".into());
        }
    }
    let decimals = options.decimals.unwrap_or(0);
    if decimals > 6 {
        push("bad_decimals", "decimals doit être ≤ 6".into());
    }
    for t in &options.thresholds {
        if !valid_hex_color(&t.color) {
            push(
                "bad_threshold_color",
                "couleur de seuil invalide (#rrggbb)".into(),
            );
        }
    }
    if let Some(unit) = &options.unit {
        if unit.len() > 16 || unit.chars().any(char::is_control) {
            push("bad_unit", "unité trop longue ou invalide".into());
        }
    }
    validate_states(options, &mut push);
    v.append(&mut extra_violations);
}

/// Per-source rules shared by instruments, control state sources and
/// annotation readings (D129).
pub(crate) fn check_sources<F: FnMut(&str, String)>(
    widget_type: &str,
    source: &[SourceRef],
    push: &mut F,
) {
    for s in source {
        if let Some(m) = &s.memory {
            // A memory value is a single live number: no history.
            if !m.is_valid() {
                push(
                    "bad_memory_key",
                    format!("invalid memory key \"{}\"", m.key),
                );
            }
            if widget_type == "line" {
                push(
                    "memory_line_unsupported",
                    "a memory value has no history: use a gauge, stat or indicator".into(),
                );
            }
            continue;
        }
        if !naming::valid_metric_name(&s.metric) {
            push(
                "bad_metric",
                format!("nom de métrique invalide : « {} »", s.metric),
            );
        }
        if !naming::valid_device_label(&s.device_id) {
            push(
                "bad_device",
                format!("device_id invalide : « {} »", s.device_id),
            );
        }
        if !valid_window(&s.window) {
            push(
                "bad_window",
                format!(
                    "fenêtre inconnue : « {} » (presets : {})",
                    s.window,
                    VIZ_WINDOW_PRESETS
                        .iter()
                        .map(|(k, _)| *k)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
    }
}

/// `#rrggbb` strict.
fn valid_hex_color(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(|b| b.is_ascii_hexdigit())
}

// ─────────────────────────────── DTOs API ───────────────────────────────
// École `flow.rs` : les types de requête/réponse vivent dans pnex-core
// (compilés côté front wasm) — la validation `validate_layout` tourne
// dans le navigateur avant save ET côté serveur en 400.

/// Résumé d'un dashboard (liste D14).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VizDashboardSummary {
    /// UUID sérialisé en string (le front ne le manipule jamais comme
    /// binaire).
    pub id: String,
    pub org_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub current_version_number: i64,
    /// Format of the current version (D123), shown as a list badge.
    #[serde(default)]
    pub format: DashboardFormat,
    pub created_at: String,
    pub updated_at: String,
}

/// Détail d'un dashboard : méta + layout de la version courante.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VizDashboard {
    pub id: String,
    pub org_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub current_version_number: i64,
    pub layout: DashboardLayout,
    pub created_at: String,
    pub updated_at: String,
}

/// Une entrée de l'historique.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VizDashboardVersion {
    pub version_number: i64,
    /// Version pointée par `current_version_number` (école media : le
    /// restore re-positionne ce pointeur, il ne crée pas de version).
    pub current: bool,
    /// Auteur (email du JWT), école `flow_versions.author`.
    pub author: Option<String>,
    pub created_at: String,
}

/// Historique complet avec layouts (le drawer versions rend un aperçu).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VizDashboardVersionDetail {
    pub version_number: i64,
    pub current: bool,
    pub layout: DashboardLayout,
    pub author: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateDashboard {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub layout: Option<DashboardLayout>,
}

/// PATCH = save — `expected_version_number` doit viser la version
/// courante (409 sinon, école flows). Save = nouvelle version = live
/// (D24 : pas de publish séparé en V1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateDashboard {
    #[serde(default)]
    pub name: Option<String>,
    pub expected_version_number: i64,
    pub layout: DashboardLayout,
}

/// Contenu d'un **template** de la bibliothèque (D41) : un widget sans
/// position — le x/y/w/h est choisi à la pose, le reste est un snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetTemplate {
    /// Un de [`VIZ_WIDGET_TYPES`].
    #[serde(rename = "type")]
    pub widget_type: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub source: Vec<SourceRef>,
    #[serde(default)]
    pub options: WidgetOptions,
}

impl WidgetTemplate {
    /// Convertit le template en widget posé (l'éditeur fournit id,
    /// position et taille par défaut du type).
    pub fn to_widget(&self, id: String, x: i64, y: i64, w: i64, h: i64) -> Widget {
        Widget {
            id,
            widget_type: self.widget_type.clone(),
            title: self.title.clone(),
            x,
            y,
            w,
            h,
            source: self.source.clone(),
            options: self.options.clone(),
        }
    }

    /// Snapshot inverse : enregistre un widget posé comme template
    /// (position et taille perdues par construction).
    pub fn from_widget(w: &Widget) -> Self {
        Self {
            widget_type: w.widget_type.clone(),
            title: w.title.clone(),
            source: w.source.clone(),
            options: w.options.clone(),
        }
    }
}

/// Un template de la bibliothèque de l'org.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VizWidget {
    pub id: String,
    pub org_id: i64,
    pub name: String,
    /// Un de [`VIZ_WIDGET_TYPES`].
    pub kind: String,
    pub config: WidgetTemplate,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateVizWidget {
    pub name: String,
    pub kind: String,
    pub config: WidgetTemplate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateVizWidget {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub config: Option<WidgetTemplate>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widget(id: &str, widget_type: &str) -> Widget {
        Widget {
            id: id.to_owned(),
            widget_type: widget_type.to_owned(),
            title: "Température".into(),
            x: 0,
            y: 0,
            w: 240,
            h: 160,
            source: vec![SourceRef {
                role: "primary".into(),
                metric: "soil_moisture".into(),
                device_id: "soil-01".into(),
                window: "1h".into(),
                memory: None,
            }],
            options: WidgetOptions::default(),
        }
    }

    fn layout(widgets: Vec<Widget>) -> DashboardLayout {
        DashboardLayout {
            canvas: CanvasSpec {
                width: 1600,
                height: 900,
                background: None,
            },
            widgets,
            wires: vec![],
            ..Default::default()
        }
    }

    fn control_widget(id: &str, widget_type: &str) -> Widget {
        let mut w = widget(id, widget_type);
        w.source.clear();
        w.options.control = Some(crate::ui_control::ControlRef {
            control_id: uuid::Uuid::from_u128(9),
        });
        w
    }

    fn mobile(widgets: Vec<Widget>, sections: Vec<MobileSection>) -> DashboardLayout {
        DashboardLayout {
            format: DashboardFormat::Mobile,
            // A mobile layout carries no meaningful canvas.
            canvas: CanvasSpec::default(),
            widgets,
            wires: vec![],
            sections,
        }
    }

    fn section(id: &str, items: &[&str]) -> MobileSection {
        MobileSection {
            id: id.into(),
            title: "General".into(),
            items: items.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn layout_valide_passe() {
        let mut l = layout(vec![widget("w1", "gauge"), widget("w2", "line")]);
        l.widgets[1].source.push(SourceRef {
            role: "secondary".into(),
            metric: "soil_moisture".into(),
            device_id: "soil-02".into(),
            window: "5m".into(),
            memory: None,
        });
        l.wires.push(Wire {
            id: "t1".into(),
            from: WireEndpoint {
                widget_id: "w1".into(),
                side: WireSide::Right,
            },
            to: WireEndpoint {
                widget_id: "w2".into(),
                side: WireSide::Left,
            },
        });
        assert_eq!(validate_layout(&l), vec![]);
    }

    #[test]
    fn canvas_hors_bornes_et_type_inconnu() {
        let mut l = layout(vec![widget("w1", "psychrometric")]);
        l.canvas.width = 10;
        let v = validate_layout(&l);
        assert!(v.iter().any(|x| x.code == "canvas_bounds"));
        assert!(v.iter().any(|x| x.code == "unknown_widget_type"));
    }

    #[test]
    fn memory_sources_validate() {
        let mem = |key: &str, field: &str| crate::memory::MemoryRef {
            key: key.into(),
            field: field.into(),
        };
        let mut g = widget("w1", "gauge");
        g.source[0].memory = Some(mem("cycle.p1", "Hmass"));
        g.source[0].metric = String::new();
        g.source[0].device_id = String::new();
        assert_eq!(g.source[0].series_key(), "mem:cycle.p1#Hmass");
        assert_eq!(validate_layout(&layout(vec![g.clone()])), vec![]);

        let mut bad = g.clone();
        bad.source[0].memory = Some(mem("a b", ""));
        let v = validate_layout(&layout(vec![bad]));
        assert!(v.iter().any(|x| x.code == "bad_memory_key"), "{v:?}");

        let mut line = g;
        line.widget_type = "line".into();
        let v = validate_layout(&layout(vec![line]));
        assert!(
            v.iter().any(|x| x.code == "memory_line_unsupported"),
            "{v:?}"
        );
    }

    #[test]
    fn thermo_chart_valide_et_rejette() {
        let mut w = widget("t1", "thermo_chart");
        w.options.thermo = Some(ThermoChartOptions {
            diagram: "ph".into(),
            fluid: "R410A".into(),
            isolines: vec![],
            points: vec![ThermoCyclePoint {
                label: "aspiration".into(),
                input_pair: "PT_INPUTS".into(),
                v1: ThermoSource {
                    metric: "pressure".into(),
                    device_id: "dev-1".into(),
                    memory: None,
                },
                v2: ThermoSource {
                    metric: "temperature".into(),
                    device_id: "dev-1".into(),
                    memory: None,
                },
            }],
            pressure: None,
            axis_decimals: None,
            pressure_unit: ThermoPressureUnit::default(),
        });
        // Sources des points absentes de widget.source → thermo_source_missing.
        let v = validate_layout(&layout(vec![w.clone()]));
        assert!(v.iter().any(|x| x.code == "thermo_source_missing"), "{v:?}");

        // Sources répliquées dans widget.source → vert (hors autres codes).
        w.source = vec![
            SourceRef {
                role: "cycle0.v1".into(),
                metric: "pressure".into(),
                device_id: "dev-1".into(),
                window: "5m".into(),
                memory: None,
            },
            SourceRef {
                role: "cycle0.v2".into(),
                metric: "temperature".into(),
                device_id: "dev-1".into(),
                window: "5m".into(),
                memory: None,
            },
        ];
        let v = validate_layout(&layout(vec![w.clone()]));
        assert!(!v.iter().any(|x| x.code.starts_with("thermo_")), "{v:?}");

        // Diagramme inconnu → thermo_bad_diagram.
        if let Some(t) = &mut w.options.thermo {
            t.diagram = "mollier".into();
        }
        let v = validate_layout(&layout(vec![w.clone()]));
        assert!(v.iter().any(|x| x.code == "thermo_bad_diagram"), "{v:?}");
    }

    #[test]
    fn widget_hors_canvas_et_source_absente() {
        let mut w = widget("w1", "gauge");
        w.x = 1500; // 1500 + 240 > 1600
        w.source.clear();
        let v = validate_layout(&layout(vec![w]));
        assert!(v.iter().any(|x| x.code == "widget_overflow"));
        assert!(v.iter().any(|x| x.code == "source_missing"));
    }

    #[test]
    fn trait_dangling_et_sur_soit_meme() {
        let mut l = layout(vec![widget("w1", "gauge")]);
        l.wires.push(Wire {
            id: "t1".into(),
            from: WireEndpoint {
                widget_id: "w1".into(),
                side: WireSide::Right,
            },
            to: WireEndpoint {
                widget_id: "inconnu".into(),
                side: WireSide::Left,
            },
        });
        l.wires.push(Wire {
            id: "t2".into(),
            from: WireEndpoint {
                widget_id: "w1".into(),
                side: WireSide::Top,
            },
            to: WireEndpoint {
                widget_id: "w1".into(),
                side: WireSide::Bottom,
            },
        });
        let v = validate_layout(&l);
        assert!(v.iter().any(|x| x.code == "dangling_wire"));
        assert!(v.iter().any(|x| x.code == "self_wire"));
    }

    #[test]
    fn widget_texte_exige_texte_sans_source() {
        let mut w = widget("t1", "text");
        w.source = vec![];
        w.options.text = None;
        let v = validate_layout(&layout(vec![w]));
        assert!(v.iter().any(|x| x.code == "text_missing"));

        let mut ok = widget("t2", "text");
        ok.source = vec![];
        ok.options.text = Some("Boucle PID n°1".into());
        assert_eq!(validate_layout(&layout(vec![ok])), vec![]);
    }

    #[test]
    fn symbol_widget_rules() {
        let mut w = widget("s1", "symbol");
        w.source = vec![];
        let v = validate_layout(&layout(vec![w.clone()]));
        assert!(v.iter().any(|x| x.code == "symbol_options_missing"));

        w.options.symbol = Some(SymbolOptions {
            shape: "pid-valves-gate-valve".into(),
            ..Default::default()
        });
        assert_eq!(validate_layout(&layout(vec![w.clone()])), vec![]);

        let mut bad = w.clone();
        bad.options.symbol = Some(SymbolOptions {
            shape: "../evil".into(),
            fill: Some("red".into()),
            rotation: 45,
            ..Default::default()
        });
        let v = validate_layout(&layout(vec![bad]));
        for code in [
            "symbol_bad_shape",
            "symbol_bad_color",
            "symbol_bad_rotation",
        ] {
            assert!(v.iter().any(|x| x.code == code), "{code}: {v:?}");
        }

        // With a source, the source is validated like any instrument.
        let mut live = widget("s2", "symbol");
        live.options.symbol = w.options.symbol.clone();
        assert_eq!(validate_layout(&layout(vec![live.clone()])), vec![]);
        live.source[0].metric = "bad metric!".into();
        let v = validate_layout(&layout(vec![live]));
        assert!(v.iter().any(|x| x.code == "bad_metric"));
    }

    #[test]
    fn sources_invalide_fenetre_et_device() {
        let mut w = widget("w1", "stat");
        w.source[0].window = "7j".into();
        w.source[0].device_id = "soil;01".into(); // injection PromQL
        let v = validate_layout(&layout(vec![w]));
        assert!(v.iter().any(|x| x.code == "bad_window"));
        assert!(v.iter().any(|x| x.code == "bad_device"));
    }

    #[test]
    fn presets_fenetres_couverts() {
        for (key, secs) in VIZ_WINDOW_PRESETS {
            assert!(valid_window(key));
            assert!(*secs > 0);
        }
        assert!(!valid_window("7j"));
    }

    #[test]
    fn roundtrip_serde_layout() {
        let l = layout(vec![widget("w1", "gauge")]);
        let json = serde_json::to_string(&l).unwrap();
        assert!(json.contains("\"type\":\"gauge\""));
        let back: DashboardLayout = serde_json::from_str(&json).unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn legacy_layout_deserializes_as_desktop() {
        let l: DashboardLayout =
            serde_json::from_str(r#"{"canvas":{"width":1600,"height":900},"widgets":[]}"#).unwrap();
        assert_eq!(l.format, DashboardFormat::Desktop);
        assert!(l.sections.is_empty());
        let json = serde_json::to_value(&l).unwrap();
        assert!(json.get("sections").is_none());
    }

    #[test]
    fn control_widgets_accept_one_state_source_and_an_optional_control() {
        let ok = control_widget("sw", "switch");
        assert!(validate_layout(&layout(vec![ok.clone()])).is_empty());

        let mut with_state = control_widget("sl", "slider");
        with_state.source = widget("x", "stat").source;
        assert!(validate_layout(&layout(vec![with_state.clone()])).is_empty());

        let mut two = with_state.clone();
        two.source.push(two.source[0].clone());
        let v = validate_layout(&layout(vec![two]));
        assert!(v.iter().any(|x| x.code == "control_state_sources"));

        // No control yet: valid, the server provisions it at save (D131).
        let mut unbound = ok.clone();
        unbound.options.control = None;
        assert!(validate_layout(&layout(vec![unbound])).is_empty());

        let mut gauge = widget("g", "gauge");
        gauge.options.control = ok.options.control.clone();
        let v = validate_layout(&layout(vec![gauge]));
        assert!(v.iter().any(|x| x.code == "control_unexpected"));
    }

    #[test]
    fn mobile_layout_rules() {
        let a = control_widget("a", "switch");
        let b = widget("b", "gauge");
        let good = mobile(
            vec![a.clone(), b.clone()],
            vec![section("s1", &["a"]), section("s2", &["b"])],
        );
        assert!(
            validate_layout(&good).is_empty(),
            "{:?}",
            validate_layout(&good)
        );

        let unplaced = mobile(vec![a.clone(), b.clone()], vec![section("s1", &["a"])]);
        assert!(validate_layout(&unplaced)
            .iter()
            .any(|x| x.code == "widget_unplaced"));

        let twice = mobile(
            vec![a.clone()],
            vec![section("s1", &["a"]), section("s2", &["a"])],
        );
        assert!(validate_layout(&twice)
            .iter()
            .any(|x| x.code == "widget_placed_twice"));

        let unknown = mobile(vec![a.clone()], vec![section("s1", &["a", "ghost"])]);
        assert!(validate_layout(&unknown)
            .iter()
            .any(|x| x.code == "section_unknown_widget"));

        let dup = mobile(
            vec![a.clone()],
            vec![section("s", &["a"]), section("s", &[])],
        );
        assert!(validate_layout(&dup).iter().any(|x| x.code == "section_id"));

        let mut wide = a.clone();
        wide.options.span = Some(3);
        let bad_span = mobile(vec![wide], vec![section("s1", &["a"])]);
        assert!(validate_layout(&bad_span)
            .iter()
            .any(|x| x.code == "widget_span"));

        let mut wired = good.clone();
        wired.wires.push(Wire {
            id: "w".into(),
            from: WireEndpoint {
                widget_id: "a".into(),
                side: WireSide::Right,
            },
            to: WireEndpoint {
                widget_id: "b".into(),
                side: WireSide::Left,
            },
        });
        assert!(validate_layout(&wired)
            .iter()
            .any(|x| x.code == "mobile_wires_forbidden"));

        let mut desktop_with_sections = layout(vec![b]);
        desktop_with_sections.sections = vec![section("s1", &["b"])];
        assert!(validate_layout(&desktop_with_sections)
            .iter()
            .any(|x| x.code == "desktop_sections"));
    }

    #[test]
    fn state_rules_first_match_wins() {
        let rules = vec![
            StateRule {
                op: StateOp::Eq,
                value: 0.0,
                label: Some("Closed".into()),
                ..Default::default()
            },
            StateRule {
                op: StateOp::Gte,
                value: 0.5,
                label: Some("Open".into()),
                ..Default::default()
            },
            StateRule {
                op: StateOp::Gte,
                value: 0.0,
                label: Some("Ajar".into()),
                ..Default::default()
            },
        ];
        let label = |v| resolve_state(&rules, v).and_then(|r| r.label.clone());
        assert_eq!(label(0.0).as_deref(), Some("Closed"));
        assert_eq!(label(1.0).as_deref(), Some("Open"));
        assert_eq!(label(0.2).as_deref(), Some("Ajar"));
        assert_eq!(label(-1.0), None);
        let lte = StateRule {
            op: StateOp::Lte,
            value: 10.0,
            ..Default::default()
        };
        assert!(lte.matches(10.0) && !lte.matches(10.1));
    }

    #[test]
    fn stale_follows_the_budget() {
        let mut o = WidgetOptions::default();
        assert!(!is_stale(&o, 0.0, 1e9));
        o.stale_after_s = Some(60);
        assert!(!is_stale(&o, 1_000.0, 1_060.0));
        assert!(is_stale(&o, 1_000.0, 1_061.0));
    }

    #[test]
    fn state_rules_are_validated() {
        let mut w = widget("w1", "stat");
        w.options.states = vec![StateRule {
            value: 1.0,
            color: Some("red".into()),
            icon: Some("Bad Icon".into()),
            label: Some("x".repeat(STATE_LABEL_MAX + 1)),
            ..Default::default()
        }];
        w.options.icon = Some("home-bulb".into());
        w.options.stale_after_s = Some(1);
        let mut v = Vec::new();
        validate_widget(&w.id, &w.widget_type, &w.source, &w.options, &mut v);
        let codes: Vec<&str> = v.iter().map(|x| x.code.as_str()).collect();
        for c in [
            "state_bad_color",
            "state_bad_icon",
            "state_bad_label",
            "bad_stale_after",
        ] {
            assert!(codes.contains(&c), "{c} missing in {codes:?}");
        }
        assert!(!codes.contains(&"bad_icon"));
        w.options.states = (0..=STATE_RULES_MAX)
            .map(|i| StateRule {
                value: i as f64,
                ..Default::default()
            })
            .collect();
        w.options.stale_after_s = Some(300);
        let mut v = Vec::new();
        validate_widget(&w.id, &w.widget_type, &w.source, &w.options, &mut v);
        assert_eq!(
            v.iter().map(|x| x.code.as_str()).collect::<Vec<_>>(),
            vec!["states_too_many"]
        );
    }

    #[test]
    fn state_rules_roundtrip_and_stay_compact() {
        let mut o = WidgetOptions::default();
        let json = serde_json::to_value(&o).unwrap();
        assert!(json.get("states").is_none() && json.get("icon").is_none());
        o.states.push(StateRule {
            op: StateOp::Gte,
            value: 2.0,
            color: Some("#22c55e".into()),
            ..Default::default()
        });
        let back: WidgetOptions =
            serde_json::from_value(serde_json::to_value(&o).unwrap()).unwrap();
        assert_eq!(back, o);
        let legacy: StateRule = serde_json::from_str(r#"{"value": 1}"#).unwrap();
        assert_eq!(legacy.op, StateOp::Eq);
    }
}
