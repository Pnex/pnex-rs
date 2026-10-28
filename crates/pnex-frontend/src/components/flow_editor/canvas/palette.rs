use super::*;

/// Canonical list of the kinds offered for adding (mockup order).
pub(crate) const PALETTE_KINDS: [PaletteKind; 48] = [
    PaletteKind::Inject,
    PaletteKind::ControlSource,
    PaletteKind::ActionTrigger,
    PaletteKind::MailReceived,
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
    PaletteKind::VisionTrack,
    PaletteKind::VisionZone,
    PaletteKind::VisionCaption,
    PaletteKind::MediaFuse,
    PaletteKind::LlmText,
    PaletteKind::TextDecide,
    PaletteKind::RangeUpsert,
    PaletteKind::OntologyQuery,
    PaletteKind::ObjectUpsert,
    PaletteKind::LinkUpsert,
    PaletteKind::SiteRecipe,
    PaletteKind::WebSearch,
    PaletteKind::OrgResolve,
    PaletteKind::WebVisit,
    PaletteKind::SiteCrawl,
    PaletteKind::PageWrite,
    PaletteKind::PagePdf,
    PaletteKind::EventLog,
    PaletteKind::MemoryWrite,
    PaletteKind::MemoryRead,
    PaletteKind::Anomaly,
    PaletteKind::Forecast,
    PaletteKind::Debug,
];

/// Stable key of a kind — palette pick key (plain data, never an
/// array index).
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
        PaletteKind::VisionTrack => "vision-track",
        PaletteKind::VisionZone => "vision-zone",
        PaletteKind::VisionCaption => "vision-caption",
        PaletteKind::MediaFuse => "media-fuse",
        PaletteKind::LlmText => "llm-text",
        PaletteKind::PageWrite => "page-write",
        PaletteKind::PagePdf => "page-pdf",
        PaletteKind::TextDecide => "text-decide",
        PaletteKind::EventLog => "event-log",
        PaletteKind::MemoryWrite => "memory-write",
        PaletteKind::MemoryRead => "memory-read",
        PaletteKind::ControlSource => "control-source",
        PaletteKind::ActionTrigger => "action-trigger",
        PaletteKind::MailReceived => "mail-received",
        PaletteKind::OntologyQuery => "ontology-query",
        PaletteKind::ObjectUpsert => "object-upsert",
        PaletteKind::LinkUpsert => "link-upsert",
        PaletteKind::SiteRecipe => "site-recipe",
        PaletteKind::WebSearch => "web-search",
        PaletteKind::OrgResolve => "org-resolve",
        PaletteKind::WebVisit => "web-visit",
        PaletteKind::SiteCrawl => "site-crawl",
        PaletteKind::Weather => "weather",
        PaletteKind::Anomaly => "anomaly",
        PaletteKind::Forecast => "forecast",
        PaletteKind::Debug => "debug",
    }
}

/// Kind from the pick key — inverse of `kind_key` (a simple find on the
/// canonical list: impossible to desynchronise the two).
pub(crate) fn kind_from_key(key: &str) -> Option<PaletteKind> {
    PALETTE_KINDS
        .iter()
        .find(|kind| kind_key(**kind) == key)
        .copied()
}

/// Icon + tile of a kind (palette + inspector header) — full literal
/// classes, tone close to the node colour on the canvas.
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
        PaletteKind::VisionTrack => (PaletteIcon::Activity, "bg-fuchsia-50 text-fuchsia-700"),
        PaletteKind::VisionZone => (PaletteIcon::Layers, "bg-fuchsia-50 text-fuchsia-700"),
        PaletteKind::VisionCaption => (PaletteIcon::Eye, "bg-violet-50 text-violet-700"),
        PaletteKind::MediaFuse => (PaletteIcon::Braces, "bg-violet-50 text-violet-700"),
        PaletteKind::LlmText => (PaletteIcon::Braces, "bg-violet-50 text-violet-800"),
        PaletteKind::PageWrite => (PaletteIcon::Layers, "bg-amber-50 text-amber-700"),
        PaletteKind::PagePdf => (PaletteIcon::Layers, "bg-amber-50 text-amber-800"),
        PaletteKind::TextDecide => (PaletteIcon::Gauge, "bg-violet-50 text-violet-700"),
        PaletteKind::EventLog => (PaletteIcon::History, "bg-slate-100 text-slate-600"),
        PaletteKind::MemoryWrite => (PaletteIcon::Database, "bg-lime-50 text-lime-700"),
        PaletteKind::MemoryRead => (PaletteIcon::Database, "bg-green-50 text-green-700"),
        PaletteKind::ControlSource => (PaletteIcon::Gauge, "bg-signal-50 text-signal-600"),
        PaletteKind::ActionTrigger => (PaletteIcon::Gauge, "bg-amber-50 text-amber-700"),
        PaletteKind::MailReceived => (PaletteIcon::Gauge, "bg-sky-50 text-sky-700"),
        PaletteKind::OntologyQuery => (PaletteIcon::Database, "bg-teal-50 text-teal-700"),
        PaletteKind::ObjectUpsert => (PaletteIcon::Database, "bg-teal-50 text-teal-800"),
        PaletteKind::LinkUpsert => (PaletteIcon::Layers, "bg-teal-50 text-teal-700"),
        PaletteKind::SiteRecipe => (PaletteIcon::Globe, "bg-cyan-50 text-cyan-800"),
        PaletteKind::WebSearch => (PaletteIcon::Globe, "bg-cyan-50 text-cyan-700"),
        PaletteKind::OrgResolve => (PaletteIcon::Database, "bg-cyan-50 text-cyan-800"),
        PaletteKind::WebVisit => (PaletteIcon::Eye, "bg-cyan-50 text-cyan-800"),
        PaletteKind::SiteCrawl => (PaletteIcon::Globe, "bg-cyan-50 text-cyan-800"),
        PaletteKind::Weather => (
            PaletteIcon::Home("home-partly-cloudy"),
            "bg-sky-50 text-sky-600",
        ),
        PaletteKind::Anomaly => (PaletteIcon::Activity, "bg-red-50 text-red-600"),
        PaletteKind::Forecast => (PaletteIcon::Spline, "bg-violet-50 text-violet-700"),
        PaletteKind::Debug => (PaletteIcon::Bug, "bg-violet-50 text-violet-600"),
    }
}

/// Kind of an existing node (inspector header: icon + label).
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
        FlowNodeKind::VisionTrack { .. } => PaletteKind::VisionTrack,
        FlowNodeKind::VisionZone { .. } => PaletteKind::VisionZone,
        FlowNodeKind::VisionCaption { .. } => PaletteKind::VisionCaption,
        FlowNodeKind::MediaFuse { .. } => PaletteKind::MediaFuse,
        FlowNodeKind::LlmText { .. } => PaletteKind::LlmText,
        FlowNodeKind::PageWrite { .. } => PaletteKind::PageWrite,
        FlowNodeKind::PagePdf { .. } => PaletteKind::PagePdf,
        FlowNodeKind::TextDecide { .. } => PaletteKind::TextDecide,
        FlowNodeKind::EventLog { .. } => PaletteKind::EventLog,
        FlowNodeKind::MemoryWrite { .. } => PaletteKind::MemoryWrite,
        FlowNodeKind::MemoryRead { .. } => PaletteKind::MemoryRead,
        FlowNodeKind::ControlSource { .. } => PaletteKind::ControlSource,
        FlowNodeKind::ActionTrigger { .. } => PaletteKind::ActionTrigger,
        FlowNodeKind::MailReceived { .. } => PaletteKind::MailReceived,
        FlowNodeKind::OntologyQuery { .. } => PaletteKind::OntologyQuery,
        FlowNodeKind::ObjectUpsert { .. } => PaletteKind::ObjectUpsert,
        FlowNodeKind::LinkUpsert { .. } => PaletteKind::LinkUpsert,
        FlowNodeKind::SiteRecipe { .. } => PaletteKind::SiteRecipe,
        FlowNodeKind::WebSearch { .. } => PaletteKind::WebSearch,
        FlowNodeKind::OrgResolve { .. } => PaletteKind::OrgResolve,
        FlowNodeKind::WebVisit { .. } => PaletteKind::WebVisit,
        FlowNodeKind::SiteCrawl { .. } => PaletteKind::SiteCrawl,
        FlowNodeKind::Weather { .. } => PaletteKind::Weather,
        FlowNodeKind::Anomaly { .. } => PaletteKind::Anomaly,
        FlowNodeKind::Forecast { .. } => PaletteKind::Forecast,
        FlowNodeKind::Debug { .. } => PaletteKind::Debug,
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
    Ontology,
    Ingestion,
    Ai,
    Integrations,
    Debug,
}

/// Section order in the palette: the path of a flow (source → processing →
/// output), debug last.
pub(crate) const PALETTE_CATEGORIES: [PaletteCategory; 11] = [
    PaletteCategory::Triggers,
    PaletteCategory::Devices,
    PaletteCategory::Control,
    PaletteCategory::Data,
    PaletteCategory::Code,
    PaletteCategory::Storage,
    PaletteCategory::Ontology,
    PaletteCategory::Ingestion,
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
        | PaletteKind::ActionTrigger
        | PaletteKind::MailReceived
        | PaletteKind::CameraSource
        | PaletteKind::MediaSource
        | PaletteKind::Weather => PaletteCategory::Triggers,
        PaletteKind::DeviceRead | PaletteKind::DeviceWrite => PaletteCategory::Devices,
        PaletteKind::RegTtHeat | PaletteKind::RegTtCool | PaletteKind::RegPid => {
            PaletteCategory::Control
        }
        PaletteKind::Value
        | PaletteKind::Calc
        | PaletteKind::JsonSplit
        | PaletteKind::JsonMerge
        | PaletteKind::TopicClassify
        | PaletteKind::CoolProp => PaletteCategory::Data,
        PaletteKind::Function => PaletteCategory::Code,
        PaletteKind::Metric
        | PaletteKind::MemoryWrite
        | PaletteKind::MemoryRead
        | PaletteKind::VideoRecord
        | PaletteKind::RangeUpsert
        | PaletteKind::PageWrite
        | PaletteKind::PagePdf
        | PaletteKind::EventLog => PaletteCategory::Storage,
        PaletteKind::OntologyQuery | PaletteKind::ObjectUpsert | PaletteKind::LinkUpsert => {
            PaletteCategory::Ontology
        }
        PaletteKind::SiteRecipe
        | PaletteKind::WebSearch
        | PaletteKind::OrgResolve
        | PaletteKind::WebVisit
        | PaletteKind::SiteCrawl => PaletteCategory::Ingestion,
        PaletteKind::VisionDetect
        | PaletteKind::VisionTrack
        | PaletteKind::VisionZone
        | PaletteKind::VisionCaption
        | PaletteKind::MediaFuse
        | PaletteKind::LlmText
        | PaletteKind::TextDecide
        | PaletteKind::Anomaly
        | PaletteKind::Forecast => PaletteCategory::Ai,
        PaletteKind::PnexNotify | PaletteKind::HttpFetch => PaletteCategory::Integrations,
        // Display is an inline probe, not a device node (O49).
        PaletteKind::Debug | PaletteKind::Display => PaletteCategory::Debug,
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
        PaletteCategory::Ontology => t!("flows-category-ontology").to_string(),
        PaletteCategory::Ingestion => t!("flows-category-ingestion").to_string(),
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

/// i18n labels of a kind (literals required for `t!`).
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
        PaletteKind::VisionTrack => (
            t!("flows-palette-vision-track").to_string(),
            t!("flows-palette-vision-track-help").to_string(),
        ),
        PaletteKind::VisionZone => (
            t!("flows-palette-vision-zone").to_string(),
            t!("flows-palette-vision-zone-help").to_string(),
        ),
        PaletteKind::VisionCaption => (
            t!("flows-palette-vision-caption").to_string(),
            t!("flows-palette-vision-caption-help").to_string(),
        ),
        PaletteKind::MediaFuse => (
            t!("flows-palette-media-fuse").to_string(),
            t!("flows-palette-media-fuse-help").to_string(),
        ),
        PaletteKind::LlmText => (
            t!("flows-palette-llm-text").to_string(),
            t!("flows-palette-llm-text-help").to_string(),
        ),
        PaletteKind::PageWrite => (
            t!("flows-palette-page-write").to_string(),
            t!("flows-palette-page-write-help").to_string(),
        ),
        PaletteKind::PagePdf => (
            t!("flows-palette-page-pdf").to_string(),
            t!("flows-palette-page-pdf-help").to_string(),
        ),
        PaletteKind::TextDecide => (
            t!("flows-palette-text-decide").to_string(),
            t!("flows-palette-text-decide-help").to_string(),
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
        PaletteKind::ActionTrigger => (
            t!("flows-palette-action-trigger").to_string(),
            t!("flows-palette-action-trigger-help").to_string(),
        ),
        PaletteKind::MailReceived => (
            t!("flows-palette-mail-received").to_string(),
            t!("flows-palette-mail-received-help").to_string(),
        ),
        PaletteKind::OntologyQuery => (
            t!("flows-palette-ontology-query").to_string(),
            t!("flows-palette-ontology-query-help").to_string(),
        ),
        PaletteKind::ObjectUpsert => (
            t!("flows-palette-object-upsert").to_string(),
            t!("flows-palette-object-upsert-help").to_string(),
        ),
        PaletteKind::LinkUpsert => (
            t!("flows-palette-link-upsert").to_string(),
            t!("flows-palette-link-upsert-help").to_string(),
        ),
        PaletteKind::SiteRecipe => (
            t!("flows-palette-site-recipe").to_string(),
            t!("flows-palette-site-recipe-help").to_string(),
        ),
        PaletteKind::WebSearch => (
            t!("flows-palette-web-search").to_string(),
            t!("flows-palette-web-search-help").to_string(),
        ),
        PaletteKind::OrgResolve => (
            t!("flows-palette-org-resolve").to_string(),
            t!("flows-palette-org-resolve-help").to_string(),
        ),
        PaletteKind::WebVisit => (
            t!("flows-palette-web-visit").to_string(),
            t!("flows-palette-web-visit-help").to_string(),
        ),
        PaletteKind::SiteCrawl => (
            t!("flows-palette-site-crawl").to_string(),
            t!("flows-palette-site-crawl-help").to_string(),
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
    }
}

/// Adds a node of the kind at the visible centre of the canvas and selects it.
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
