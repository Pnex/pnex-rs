//! Shared rendering of the **surfaces** (D129, `docs/architecture/
//! surfaces-controls.md`): dashboards (desktop canvas, mobile stack) and
//! annotations show the same control cards and readings.
//!
//! "A surface reads, a flow acts": a control card only writes the value of
//! an org control (`POST /controls/{id}/value`); the deployed flows
//! listening to it (`control-source`) decide what happens on the devices.
//!
//! The host of a surface (live view) provides a [`SurfaceControls`]
//! context with [`use_surface_controls`]; without it (editor preview) the
//! cards render disabled.

pub mod annotation;
pub mod control;

use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ui_control::{ControlKind, ControlSpec, ControlValue, UiControl};
use uuid::Uuid;

use crate::api;

pub use control::ControlBody;

/// Control state of a mounted surface (context, `Copy`).
#[derive(Clone, Copy, PartialEq)]
pub struct SurfaceControls {
    /// Org controls by id (`None` while loading or on error).
    defs: Resource<Option<BTreeMap<Uuid, UiControl>>>,
    /// Last commanded values fetched with the surface polling.
    fetched: Resource<Option<BTreeMap<Uuid, Option<ControlValue>>>>,
    /// Values written from this surface since the last poll (the newest
    /// timestamp wins over the fetched one).
    written: Signal<BTreeMap<Uuid, ControlValue>>,
    /// The viewer may operate the controls (member+, live view).
    pub interactive: bool,
    /// Originating surface stamped on every write (`dashboard:{id}`).
    pub via: Signal<String>,
}

impl SurfaceControls {
    /// Definition of a control (`None`: loading, deleted or unknown).
    pub fn def(&self, id: &Uuid) -> Option<UiControl> {
        self.defs.read().as_ref()?.as_ref()?.get(id).cloned()
    }

    /// The definitions are loaded (a missing control is then really gone).
    pub fn loaded(&self) -> bool {
        matches!(&*self.defs.read(), Some(Some(_)))
    }

    /// Current commanded value: the newest of the fetched and the locally
    /// written one.
    pub fn value(&self, id: &Uuid) -> Option<ControlValue> {
        let fetched = self
            .fetched
            .read()
            .as_ref()
            .and_then(|m| m.as_ref())
            .and_then(|m| m.get(id).cloned())
            .flatten();
        let written = self.written.read().get(id).cloned();
        match (fetched, written) {
            (Some(f), Some(w)) => Some(if w.ts_ms >= f.ts_ms { w } else { f }),
            (f, w) => f.or(w),
        }
    }

    /// Records a successful write (optimistic refresh of every card bound
    /// to the same control).
    pub fn record(&self, id: Uuid, value: ControlValue) {
        let mut written = self.written;
        written.with_mut(|m| {
            m.insert(id, value);
        });
    }
}

/// Provides the control context of a surface. `ids` is read in the
/// synchronous part of the value resource (its signals are tracked);
/// `reload` drives both the definitions and the values (surface polling,
/// D31).
pub fn use_surface_controls(
    ids: impl Fn() -> Vec<Uuid> + 'static,
    reload: Signal<u32>,
    via: String,
    interactive: bool,
) -> SurfaceControls {
    let defs = use_resource(move || async move {
        let _ = reload();
        api::controls::list()
            .await
            .ok()
            .map(|list| list.into_iter().map(|c| (c.id, c)).collect())
    });
    let fetched = use_resource(move || {
        let mut ids = ids();
        ids.sort();
        ids.dedup();
        let _ = reload();
        async move {
            if ids.is_empty() {
                return Some(BTreeMap::new());
            }
            api::controls::values(ids).await.ok()
        }
    });
    let written = use_signal(BTreeMap::new);
    let via = use_signal(move || via);
    use_context_provider(move || SurfaceControls {
        defs,
        fetched,
        written,
        interactive,
        via,
    })
}

/// Control ids driven by the widgets of a layout.
pub fn control_ids(layout: &pnex_core::DashboardLayout) -> Vec<Uuid> {
    layout
        .widgets
        .iter()
        .filter_map(|w| w.options.control.as_ref().map(|c| c.control_id))
        .collect()
}

/// Localized name of a control kind.
pub fn kind_text(kind: ControlKind) -> String {
    match kind {
        ControlKind::Switch => t!("controls-kind-switch").to_string(),
        ControlKind::Slider => t!("controls-kind-slider").to_string(),
        ControlKind::Button => t!("controls-kind-button").to_string(),
        ControlKind::Number => t!("controls-kind-number").to_string(),
    }
}

/// Group of a control in the catalogs (D131): its declaring surface
/// (`Dashboard · Machine room`), or the standalone group.
pub fn control_group(c: &UiControl) -> String {
    match &c.origin {
        Some(o) => {
            let name = o
                .surface_name
                .clone()
                .unwrap_or_else(|| t!("controls-origin-gone").to_string());
            let kind = if o.surface == pnex_core::ui_control::ORIGIN_ANNOTATION {
                t!("controls-origin-annotation")
            } else {
                t!("controls-origin-dashboard")
            };
            format!("{kind} · {name}")
        }
        None => t!("controls-origin-standalone").to_string(),
    }
}

/// Display name of a control: `#w-0001 · Light` when declared by a surface
/// item (the label alone when it is the item id), else `Light (light.room)`.
pub fn control_display_name(c: &UiControl) -> String {
    match &c.origin {
        Some(o) if c.label == o.item_id => format!("#{}", o.item_id),
        Some(o) => format!("#{} · {}", o.item_id, c.label),
        None => format!("{} ({})", c.label, c.key),
    }
}

/// One-line value domain of a spec (`on 1 / off 0`, `0 … 100 % (step 1)`).
pub fn spec_summary(spec: &ControlSpec) -> String {
    let unit = spec.unit.clone().unwrap_or_default();
    let n = |v: Option<f64>| v.map(|v| v.to_string()).unwrap_or_else(|| "…".into());
    let text = match spec.kind {
        ControlKind::Switch => t!(
            "controls-domain-switch",
            on : spec.on_value().to_string(),
            off : spec.off_value().to_string()
        )
        .to_string(),
        ControlKind::Button => {
            t!("controls-domain-button", press : spec.press_value().to_string()).to_string()
        }
        ControlKind::Slider | ControlKind::Number => {
            let range = format!("{} … {} {unit}", n(spec.min_value()), n(spec.max_value()));
            match spec.step_value() {
                Some(step) => t!(
                    "controls-domain-range-step",
                    range : range.trim().to_string(),
                    step : step.to_string()
                )
                .to_string(),
                None => range.trim().to_string(),
            }
        }
    };
    text
}

/// Control key suggested from a label: lowercase ASCII, separators folded
/// to `_` (`Éclairage salle` → `eclairage_salle`), at most 64 chars.
pub fn suggest_key(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        let c = match c {
            'à' | 'â' | 'ä' | 'á' | 'À' | 'Â' | 'Ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
            'î' | 'ï' | 'Î' | 'Ï' => 'i',
            'ô' | 'ö' | 'Ô' | 'Ö' => 'o',
            'ù' | 'û' | 'ü' | 'Ù' | 'Û' | 'Ü' => 'u',
            'ç' | 'Ç' => 'c',
            c => c,
        };
        if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
        if out.len() >= 64 {
            break;
        }
    }
    out.trim_end_matches('_').to_string()
}

/// Draft graph of "Create the flow" (D127, parcours §1.3): a
/// `control-source` on `control_id` (replayed at start) wired to a
/// `device-write` of one pin. A switch (on/off) or a slider (0..100) drives
/// a digital output or a PWM duty as is.
pub fn flow_draft(control_id: Uuid, device_slug: &str, pin_label: &str) -> pnex_core::FlowGraph {
    use pnex_core::ui_control::ControlSourceConfig;
    use pnex_core::{
        DeviceWriteConfig, FlowGraph, FlowInputWiring, FlowNode, FlowNodeKind, FlowWiring, Position,
    };
    FlowGraph {
        nodes: vec![
            FlowNode {
                id: "n1".into(),
                name: None,
                position: Some(Position { x: 80.0, y: 120.0 }),
                outputs: vec![FlowWiring {
                    port: 0,
                    targets: vec!["n2".into()],
                }],
                inputs: vec![],
                kind: FlowNodeKind::ControlSource {
                    config: ControlSourceConfig {
                        controls: vec![control_id],
                        emit_on_start: true,
                    },
                },
            },
            FlowNode {
                id: "n2".into(),
                name: None,
                position: Some(Position { x: 400.0, y: 120.0 }),
                outputs: vec![],
                inputs: vec![FlowInputWiring {
                    pin: pin_label.to_owned(),
                    from: "n1".into(),
                    from_port: 0,
                }],
                kind: FlowNodeKind::DeviceWrite {
                    config: DeviceWriteConfig {
                        device_id: device_slug.to_owned(),
                        pins: vec![pin_label.to_owned()],
                    },
                },
            },
        ],
    }
}

/// Creates the draft flow of a control and opens it in the flow editor
/// (deploy stays the user's call).
pub fn create_flow_draft(control: &UiControl, device_slug: &str, pin_label: &str) {
    let params = pnex_core::CreateFlow {
        name: t!("controls-flow-name", label : control.label.clone()).to_string(),
        device_id: None,
        graph: flow_draft(control.id, device_slug, pin_label),
        author: None,
        note: None,
    };
    let done = t!("controls-flow-created").to_string();
    spawn(async move {
        match api::flows::create(params).await {
            Ok(flow) => {
                crate::state::toasts::success(done);
                crate::state::flows::OPEN_FLOW.with_mut(|v| *v = Some(flow.id));
                navigator().push(crate::app::Route::Flows {});
            }
            Err(e) => crate::state::toasts::error(e),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_draft_is_a_valid_graph_listening_to_the_control() {
        let id = Uuid::from_u128(42);
        let g = flow_draft(id, "proud-ibex", "LED");
        assert!(
            pnex_core::validate_graph(&g).is_empty(),
            "{:?}",
            pnex_core::validate_graph(&g)
        );
        assert_eq!(pnex_core::control_refs_of(&g), vec![id]);
        // Round trip through the stored JSON shape (manual Deserialize).
        let back: pnex_core::FlowGraph =
            serde_json::from_value(serde_json::to_value(&g).unwrap()).unwrap();
        assert_eq!(back, g);
    }

    #[test]
    fn suggested_keys_are_valid_control_keys() {
        assert_eq!(
            suggest_key("Éclairage salle machine"),
            "eclairage_salle_machine"
        );
        assert_eq!(suggest_key("  Fan #2 (duty)  "), "fan_2_duty");
        assert_eq!(suggest_key("pump.main"), "pump.main");
        assert_eq!(suggest_key("***"), "");
        let long = suggest_key(&"a".repeat(100));
        assert_eq!(long.len(), 64);
        for label in ["Éclairage salle machine", "Fan #2 (duty)", "x"] {
            assert!(pnex_core::ui_control::valid_control_key(&suggest_key(
                label
            )));
        }
    }
}
