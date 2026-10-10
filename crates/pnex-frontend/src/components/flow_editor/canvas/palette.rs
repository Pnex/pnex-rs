use super::*;

/// Liste canonique des kinds proposés à l'ajout (ordre de la maquette).
pub(crate) const PALETTE_KINDS: [PaletteKind; 31] = [
    PaletteKind::Inject,
    PaletteKind::ControlSource,
    PaletteKind::Weather,
    PaletteKind::Value,
    PaletteKind::DeviceRead,
    PaletteKind::DeviceWrite,
    PaletteKind::Calc,
    PaletteKind::Metric,
    PaletteKind::CoolProp,
    PaletteKind::Display,
    PaletteKind::RegTtHeat,
    PaletteKind::RegTtCool,
    PaletteKind::RegPid,
    PaletteKind::PnexNotify,
    PaletteKind::HttpFetch,
    PaletteKind::Function,
    PaletteKind::JsonSplit,
    PaletteKind::JsonMerge,
    PaletteKind::CameraSource,
    PaletteKind::MediaSource,
    PaletteKind::TopicClassify,
    PaletteKind::VideoRecord,
    PaletteKind::VisionDetect,
    PaletteKind::RangeUpsert,
    PaletteKind::EventLog,
    PaletteKind::MemoryWrite,
    PaletteKind::MemoryRead,
    PaletteKind::Anomaly,
    PaletteKind::Forecast,
    PaletteKind::Debug,
    PaletteKind::Red,
];

/// Clé stable d'un kind — clé de pick de la palette (data plate, jamais
/// d'index de tableau).
pub(crate) fn kind_key(kind: PaletteKind) -> &'static str {
    match kind {
        PaletteKind::Inject => "inject",
        PaletteKind::Value => "value",
        PaletteKind::DeviceRead => "device-read",
        PaletteKind::DeviceWrite => "device-write",
        PaletteKind::Calc => "calc",
        PaletteKind::Metric => "metric",
        PaletteKind::CoolProp => "coolprop",
        PaletteKind::Display => "display",
        PaletteKind::RegTtHeat => "reg-tt-heat",
        PaletteKind::RegTtCool => "reg-tt-cool",
        PaletteKind::RegPid => "reg-pid",
        PaletteKind::PnexNotify => "notify",
        PaletteKind::HttpFetch => "http-fetch",
        PaletteKind::Function => "function",
        PaletteKind::JsonSplit => "json-split",
        PaletteKind::JsonMerge => "json-merge",
        PaletteKind::CameraSource => "camera-source",
        PaletteKind::MediaSource => "media-source",
        PaletteKind::TopicClassify => "topic-classify",
        PaletteKind::RangeUpsert => "range-upsert",
        PaletteKind::VideoRecord => "video-record",
        PaletteKind::VisionDetect => "vision-detect",
        PaletteKind::EventLog => "event-log",
        PaletteKind::MemoryWrite => "memory-write",
        PaletteKind::MemoryRead => "memory-read",
        PaletteKind::ControlSource => "control-source",
        PaletteKind::Weather => "weather",
        PaletteKind::Anomaly => "anomaly",
        PaletteKind::Forecast => "forecast",
        PaletteKind::Debug => "debug",
        PaletteKind::Red => "red",
    }
}

/// Kind depuis la clé de pick — inverse de `kind_key` (un simple find sur la
/// liste canonique : impossible de désynchroniser les deux).
pub(crate) fn kind_from_key(key: &str) -> Option<PaletteKind> {
    PALETTE_KINDS
        .iter()
        .find(|kind| kind_key(**kind) == key)
        .copied()
}

/// Icône + tuile d'un kind (palette + en-tête d'inspecteur) — classes
/// littérales complètes, tonalité proche de la couleur du nœud au canvas.
pub(crate) fn kind_icon(kind: PaletteKind) -> (PaletteIcon, &'static str) {
    match kind {
        PaletteKind::Inject => (PaletteIcon::Activity, "bg-emerald-50 text-emerald-600"),
        PaletteKind::Value => (PaletteIcon::Hash, "bg-yellow-50 text-yellow-600"),
        PaletteKind::DeviceRead => (PaletteIcon::Cpu, "bg-amber-50 text-amber-600"),
        PaletteKind::DeviceWrite => (PaletteIcon::Cpu, "bg-orange-50 text-orange-600"),
        PaletteKind::Calc => (PaletteIcon::Calculator, "bg-green-50 text-green-600"),
        PaletteKind::Metric => (PaletteIcon::LineChart, "bg-pink-50 text-pink-600"),
        PaletteKind::CoolProp => (PaletteIcon::Thermometer, "bg-amber-50 text-amber-600"),
        PaletteKind::Display => (PaletteIcon::Eye, "bg-cyan-50 text-cyan-600"),
        PaletteKind::RegTtHeat => (PaletteIcon::Thermometer, "bg-orange-50 text-orange-600"),
        PaletteKind::RegTtCool => (PaletteIcon::Snowflake, "bg-sky-50 text-sky-600"),
        PaletteKind::RegPid => (PaletteIcon::Gauge, "bg-indigo-50 text-indigo-600"),
        PaletteKind::PnexNotify => (PaletteIcon::Bell, "bg-purple-50 text-purple-600"),
        PaletteKind::HttpFetch => (PaletteIcon::Globe, "bg-sky-50 text-sky-600"),
        PaletteKind::Function => (PaletteIcon::Braces, "bg-teal-50 text-teal-600"),
        PaletteKind::JsonSplit => (PaletteIcon::Layers, "bg-cyan-50 text-cyan-700"),
        PaletteKind::JsonMerge => (PaletteIcon::Layers, "bg-teal-50 text-teal-700"),
        PaletteKind::CameraSource => (PaletteIcon::Camera, "bg-rose-50 text-rose-600"),
        PaletteKind::MediaSource => (PaletteIcon::Activity, "bg-indigo-50 text-indigo-600"),
        PaletteKind::TopicClassify => (PaletteIcon::Layers, "bg-indigo-50 text-indigo-700"),
        PaletteKind::RangeUpsert => (PaletteIcon::History, "bg-indigo-50 text-indigo-700"),
        PaletteKind::VideoRecord => (PaletteIcon::Video, "bg-red-50 text-red-600"),
        PaletteKind::VisionDetect => (PaletteIcon::Eye, "bg-fuchsia-50 text-fuchsia-600"),
        PaletteKind::EventLog => (PaletteIcon::History, "bg-slate-100 text-slate-600"),
        PaletteKind::MemoryWrite => (PaletteIcon::Database, "bg-lime-50 text-lime-700"),
        PaletteKind::MemoryRead => (PaletteIcon::Database, "bg-green-50 text-green-700"),
        PaletteKind::ControlSource => (PaletteIcon::Gauge, "bg-blue-50 text-blue-600"),
        PaletteKind::Weather => (
            PaletteIcon::Home("home-partly-cloudy"),
            "bg-sky-50 text-sky-600",
        ),
        PaletteKind::Anomaly => (PaletteIcon::Activity, "bg-red-50 text-red-600"),
        PaletteKind::Forecast => (PaletteIcon::Spline, "bg-violet-50 text-violet-700"),
        PaletteKind::Debug => (PaletteIcon::Bug, "bg-violet-50 text-violet-600"),
        PaletteKind::Red => (PaletteIcon::Puzzle, "bg-gray-100 text-gray-600"),
    }
}

/// Kind d'un nœud existant (en-tête de l'inspecteur : icône + libellé).
pub(crate) fn kind_of(kind: &FlowNodeKind) -> PaletteKind {
    match kind {
        FlowNodeKind::Inject { .. } => PaletteKind::Inject,
        FlowNodeKind::Value { .. } => PaletteKind::Value,
        FlowNodeKind::DeviceRead { .. } => PaletteKind::DeviceRead,
        FlowNodeKind::DeviceWrite { .. } => PaletteKind::DeviceWrite,
        FlowNodeKind::Calc { .. } => PaletteKind::Calc,
        FlowNodeKind::Metric { .. } => PaletteKind::Metric,
        FlowNodeKind::CoolProp { .. } => PaletteKind::CoolProp,
        FlowNodeKind::Display { .. } => PaletteKind::Display,
        FlowNodeKind::RegTtHeat { .. } => PaletteKind::RegTtHeat,
        FlowNodeKind::RegTtCool { .. } => PaletteKind::RegTtCool,
        FlowNodeKind::RegPid { .. } => PaletteKind::RegPid,
        FlowNodeKind::PnexNotify { .. } => PaletteKind::PnexNotify,
        FlowNodeKind::HttpFetch { .. } => PaletteKind::HttpFetch,
        FlowNodeKind::PnexFunction { .. } => PaletteKind::Function,
        FlowNodeKind::JsonSplit { .. } => PaletteKind::JsonSplit,
        FlowNodeKind::JsonMerge { .. } => PaletteKind::JsonMerge,
        FlowNodeKind::CameraSource { .. } => PaletteKind::CameraSource,
        FlowNodeKind::MediaSource { .. } => PaletteKind::MediaSource,
        FlowNodeKind::TopicClassify { .. } => PaletteKind::TopicClassify,
        FlowNodeKind::RangeUpsert { .. } => PaletteKind::RangeUpsert,
        FlowNodeKind::VideoRecord { .. } => PaletteKind::VideoRecord,
        FlowNodeKind::VisionDetect { .. } => PaletteKind::VisionDetect,
        FlowNodeKind::EventLog { .. } => PaletteKind::EventLog,
        FlowNodeKind::MemoryWrite { .. } => PaletteKind::MemoryWrite,
        FlowNodeKind::MemoryRead { .. } => PaletteKind::MemoryRead,
        FlowNodeKind::ControlSource { .. } => PaletteKind::ControlSource,
        FlowNodeKind::Weather { .. } => PaletteKind::Weather,
        FlowNodeKind::Anomaly { .. } => PaletteKind::Anomaly,
        FlowNodeKind::Forecast { .. } => PaletteKind::Forecast,
        FlowNodeKind::Debug { .. } => PaletteKind::Debug,
        FlowNodeKind::Red { .. } => PaletteKind::Red,
    }
}

/// Palette sections (D109) — one category per kind, flat data.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PaletteCategory {
    Triggers,
    Devices,
    Control,
    Data,
    Code,
    Storage,
    Ai,
    Integrations,
    Debug,
}

/// Section order in the palette: the path of a flow (source → processing →
/// output), debug last.
pub(crate) const PALETTE_CATEGORIES: [PaletteCategory; 9] = [
    PaletteCategory::Triggers,
    PaletteCategory::Devices,
    PaletteCategory::Control,
    PaletteCategory::Data,
    PaletteCategory::Code,
    PaletteCategory::Storage,
    PaletteCategory::Ai,
    PaletteCategory::Integrations,
    PaletteCategory::Debug,
];

/// Category of a kind — exhaustive match, so a new kind cannot be added
/// without picking its section.
pub(crate) fn kind_category(kind: PaletteKind) -> PaletteCategory {
    match kind {
        PaletteKind::Inject
        | PaletteKind::ControlSource
        | PaletteKind::CameraSource
        | PaletteKind::MediaSource
        | PaletteKind::Weather => PaletteCategory::Triggers,
        PaletteKind::DeviceRead | PaletteKind::DeviceWrite | PaletteKind::Display => {
            PaletteCategory::Devices
        }
        PaletteKind::RegTtHeat | PaletteKind::RegTtCool | PaletteKind::RegPid => {
            PaletteCategory::Control
        }
        PaletteKind::Value
        | PaletteKind::Calc
        | PaletteKind::JsonSplit
        | PaletteKind::JsonMerge
        | PaletteKind::TopicClassify
        | PaletteKind::CoolProp => PaletteCategory::Data,
        PaletteKind::Function | PaletteKind::Red => PaletteCategory::Code,
        PaletteKind::Metric
        | PaletteKind::MemoryWrite
        | PaletteKind::MemoryRead
        | PaletteKind::VideoRecord
        | PaletteKind::RangeUpsert
        | PaletteKind::EventLog => PaletteCategory::Storage,
        PaletteKind::VisionDetect | PaletteKind::Anomaly | PaletteKind::Forecast => {
            PaletteCategory::Ai
        }
        PaletteKind::PnexNotify | PaletteKind::HttpFetch => PaletteCategory::Integrations,
        PaletteKind::Debug => PaletteCategory::Debug,
    }
}

/// Section header of a category (literal keys required by `t!`).
pub(crate) fn category_label(category: PaletteCategory) -> String {
    match category {
        PaletteCategory::Triggers => t!("flows-category-triggers").to_string(),
        PaletteCategory::Devices => t!("flows-category-devices").to_string(),
        PaletteCategory::Control => t!("flows-category-control").to_string(),
        PaletteCategory::Data => t!("flows-category-data").to_string(),
        PaletteCategory::Code => t!("flows-category-code").to_string(),
        PaletteCategory::Storage => t!("flows-category-storage").to_string(),
        PaletteCategory::Ai => t!("flows-category-ai").to_string(),
        PaletteCategory::Integrations => t!("flows-category-integrations").to_string(),
        PaletteCategory::Debug => t!("flows-category-debug").to_string(),
    }
}

/// Kinds in palette order: grouped by category (section order), canonical
/// order inside each section.
pub(crate) fn ordered_kinds() -> Vec<PaletteKind> {
    PALETTE_CATEGORIES
        .iter()
        .flat_map(|category| {
            PALETTE_KINDS
                .iter()
                .copied()
                .filter(move |kind| kind_category(*kind) == *category)
        })
        .collect()
}

/// Shell palette items — i18n labels + icons + descriptions, grouped into
/// category sections (D109).
pub(crate) fn palette_items() -> Vec<PaletteItem> {
    ordered_kinds()
        .into_iter()
        .map(|kind| {
            let (label, help) = kind_labels(kind);
            let (icon, tile) = kind_icon(kind);
            PaletteItem::new(kind_key(kind), label)
                .with_description(help)
                .with_icon(icon, tile)
                .with_group(category_label(kind_category(kind)))
        })
        .collect()
}

/// Libellés i18n d'un kind (littéraux obligatoires pour `t!`).
pub(crate) fn kind_labels(kind: PaletteKind) -> (String, String) {
    match kind {
        PaletteKind::Inject => (
            t!("flows-palette-inject").to_string(),
            t!("flows-palette-inject-help").to_string(),
        ),
        PaletteKind::Value => (
            t!("flows-palette-value").to_string(),
            t!("flows-palette-value-help").to_string(),
        ),
        PaletteKind::DeviceRead => (
            t!("flows-palette-device-read").to_string(),
            t!("flows-palette-device-read-help").to_string(),
        ),
        PaletteKind::DeviceWrite => (
            t!("flows-palette-device-write").to_string(),
            t!("flows-palette-device-write-help").to_string(),
        ),
        PaletteKind::Calc => (
            t!("flows-palette-calc").to_string(),
            t!("flows-palette-calc-help").to_string(),
        ),
        PaletteKind::Metric => (
            t!("flows-palette-metric").to_string(),
            t!("flows-palette-metric-help").to_string(),
        ),
        PaletteKind::CoolProp => (
            t!("flows-palette-coolprop").to_string(),
            t!("flows-palette-coolprop-help").to_string(),
        ),
        PaletteKind::Display => (
            t!("flows-palette-display").to_string(),
            t!("flows-palette-display-help").to_string(),
        ),
        PaletteKind::RegTtHeat => (
            t!("flows-palette-reg-tt-heat").to_string(),
            t!("flows-palette-reg-tt-heat-help").to_string(),
        ),
        PaletteKind::RegTtCool => (
            t!("flows-palette-reg-tt-cool").to_string(),
            t!("flows-palette-reg-tt-cool-help").to_string(),
        ),
        PaletteKind::RegPid => (
            t!("flows-palette-reg-pid").to_string(),
            t!("flows-palette-reg-pid-help").to_string(),
        ),
        PaletteKind::PnexNotify => (
            t!("flows-palette-notify").to_string(),
            t!("flows-palette-notify-help").to_string(),
        ),
        PaletteKind::HttpFetch => (
            t!("flows-palette-http-fetch").to_string(),
            t!("flows-palette-http-fetch-help").to_string(),
        ),
        PaletteKind::Function => (
            t!("flows-palette-function").to_string(),
            t!("flows-palette-function-help").to_string(),
        ),
        PaletteKind::JsonSplit => (
            t!("flows-palette-json-split").to_string(),
            t!("flows-palette-json-split-help").to_string(),
        ),
        PaletteKind::JsonMerge => (
            t!("flows-palette-json-merge").to_string(),
            t!("flows-palette-json-merge-help").to_string(),
        ),
        PaletteKind::CameraSource => (
            t!("flows-palette-camera-source").to_string(),
            t!("flows-palette-camera-source-help").to_string(),
        ),
        PaletteKind::VideoRecord => (
            t!("flows-palette-video-record").to_string(),
            t!("flows-palette-video-record-help").to_string(),
        ),
        PaletteKind::VisionDetect => (
            t!("flows-palette-vision-detect").to_string(),
            t!("flows-palette-vision-detect-help").to_string(),
        ),
        PaletteKind::EventLog => (
            t!("flows-palette-event-log").to_string(),
            t!("flows-palette-event-log-help").to_string(),
        ),
        PaletteKind::MemoryWrite => (
            t!("flows-palette-memory-write").to_string(),
            t!("flows-palette-memory-write-help").to_string(),
        ),
        PaletteKind::MemoryRead => (
            t!("flows-palette-memory-read").to_string(),
            t!("flows-palette-memory-read-help").to_string(),
        ),
        PaletteKind::ControlSource => (
            t!("flows-palette-control-source").to_string(),
            t!("flows-palette-control-source-help").to_string(),
        ),
        PaletteKind::MediaSource => (
            t!("flows-palette-media-source").to_string(),
            t!("flows-palette-media-source-help").to_string(),
        ),
        PaletteKind::TopicClassify => (
            t!("flows-palette-topic-classify").to_string(),
            t!("flows-palette-topic-classify-help").to_string(),
        ),
        PaletteKind::RangeUpsert => (
            t!("flows-palette-range-upsert").to_string(),
            t!("flows-palette-range-upsert-help").to_string(),
        ),
        PaletteKind::Weather => (
            t!("flows-palette-weather").to_string(),
            t!("flows-palette-weather-help").to_string(),
        ),
        PaletteKind::Anomaly => (
            t!("flows-palette-anomaly").to_string(),
            t!("flows-palette-anomaly-help").to_string(),
        ),
        PaletteKind::Forecast => (
            t!("flows-palette-forecast").to_string(),
            t!("flows-palette-forecast-help").to_string(),
        ),
        PaletteKind::Debug => (
            t!("flows-palette-debug").to_string(),
            t!("flows-palette-debug-help").to_string(),
        ),
        PaletteKind::Red => (
            t!("flows-palette-red").to_string(),
            t!("flows-palette-red-help").to_string(),
        ),
    }
}

/// Ajoute un nœud du kind au centre visible du canevas et le sélectionne.
pub(crate) fn add_node_to_canvas(mut cx: EditorCx, kind: PaletteKind) {
    let Some(rect) = geometry::canvas_rect() else {
        return;
    };
    let pan = cx.pan.cloned();
    let zoom = cx.zoom.cloned();
    let center = geometry::to_graph(
        (rect.0 + rect.2 / 2.0, rect.1 + rect.3 / 2.0),
        rect,
        pan,
        zoom,
    );
    let id = state::next_node_id(&cx.graph.peek().clone());
    let new_id = id.clone();
    cx.update_graph(move |graph| {
        let node = state::make_node(&new_id, kind, geometry::cascade_origin(graph, center));
        graph.nodes.push(node);
    });
    cx.selected_node.set(Some(id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_kinds_lists_every_kind_once() {
        let ordered = ordered_kinds();
        assert_eq!(ordered.len(), PALETTE_KINDS.len());
        for kind in PALETTE_KINDS {
            assert_eq!(
                ordered.iter().filter(|k| **k == kind).count(),
                1,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn ordered_kinds_follow_category_order() {
        let ranks: Vec<usize> = ordered_kinds()
            .into_iter()
            .map(|kind| {
                PALETTE_CATEGORIES
                    .iter()
                    .position(|c| *c == kind_category(kind))
                    .expect("category listed in PALETTE_CATEGORIES")
            })
            .collect();
        assert!(ranks.windows(2).all(|w| w[0] <= w[1]), "{ranks:?}");
    }

    #[test]
    fn every_category_has_at_least_one_kind() {
        for category in PALETTE_CATEGORIES {
            assert!(
                PALETTE_KINDS.iter().any(|k| kind_category(*k) == category),
                "{category:?}"
            );
        }
    }
}
