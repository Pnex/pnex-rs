//! Shared pieces of the ontology UI (ontology.md D186): labels of types
//! and link types, typed property inputs and values, the local graph view
//! and the "object page" link shown on the specialised pages.

pub mod graph_view;
pub mod object_link;
pub mod property_input;

use dioxus::prelude::*;
use dioxus_i18n::prelude::i18n;
use pnex_core::ontology::api::ObjectTypeView;
use pnex_core::ontology::{LinkTypeDef, ObjectTypeDef};

use crate::components::icons;

/// Display name of a type: its own name, or the translated system key.
pub fn type_label(def: &ObjectTypeDef) -> String {
    if !def.name.trim().is_empty() {
        return def.name.clone();
    }
    key_label(&def.key)
}

/// Display name of a type key alone (`onto-type-<key>`, else the key).
pub fn key_label(key: &str) -> String {
    i18n()
        .try_translate(&format!("onto-type-{key}"))
        .unwrap_or_else(|_| key.to_string())
}

/// Label of a type key among the known types.
pub fn type_label_of(types: &[ObjectTypeView], key: &str) -> String {
    types
        .iter()
        .find(|t| t.def.key == key)
        .map(|t| type_label(&t.def))
        .unwrap_or_else(|| key_label(key))
}

/// Link label read from the source (`inverse` = read from the target).
pub fn link_label(def: Option<&LinkTypeDef>, key: &str, inverse: bool) -> String {
    let own = def.map(|d| if inverse { &d.inverse_name } else { &d.name });
    match own.filter(|n| !n.trim().is_empty()) {
        Some(n) => n.clone(),
        None => {
            let k = if inverse {
                format!("onto-link-{key}-inverse")
            } else {
                format!("onto-link-{key}")
            };
            i18n().try_translate(&k).unwrap_or_else(|_| key.to_string())
        }
    }
}

/// Icon of a type from its `icon` name (a few names of the icon set).
#[component]
pub fn TypeIcon(icon: String, class: Option<String>) -> Element {
    let class = class.unwrap_or_else(|| "h-4 w-4".into());
    let c = Some(class);
    match icon.as_str() {
        "map-pin" => rsx! {
            icons::MapPin { class: c }
        },
        "workflow" => rsx! {
            icons::Workflow { class: c }
        },
        "cog" | "wrench" => rsx! {
            icons::Wrench { class: c }
        },
        "droplet" | "thermometer" => rsx! {
            icons::Thermometer { class: c }
        },
        "cpu" => rsx! {
            icons::Cpu { class: c }
        },
        "building" => rsx! {
            icons::Building { class: c }
        },
        "user" => rsx! {
            icons::User { class: c }
        },
        "folder" => rsx! {
            icons::Folder { class: c }
        },
        "image" => rsx! {
            icons::Image { class: c }
        },
        "layers" => rsx! {
            icons::Layers { class: c }
        },
        "tag" => rsx! {
            icons::Tag { class: c }
        },
        "radio" => rsx! {
            icons::Radio { class: c }
        },
        "camera" => rsx! {
            icons::Camera { class: c }
        },
        "server" => rsx! {
            icons::Server { class: c }
        },
        "globe" => rsx! {
            icons::Globe { class: c }
        },
        _ => rsx! {
            icons::Cube { class: c }
        },
    }
}

/// Icon names offered by the type editor.
pub const ICON_NAMES: &[&str] = &[
    "cube", "map-pin", "building", "workflow", "cog", "droplet", "cpu", "user", "folder", "image",
    "layers", "tag", "radio", "camera", "server", "globe",
];

/// Icon of a system type key.
pub fn system_icon(key: &str) -> &'static str {
    match key {
        "device" => "cpu",
        "map_pin" => "map-pin",
        "media_asset" => "image",
        "dashboard" => "layers",
        "tour" => "camera",
        "flow" => "workflow",
        "folder" => "folder",
        _ => "cube",
    }
}

pub fn icon_of(def: &ObjectTypeDef) -> String {
    if def.icon.is_empty() {
        system_icon(&def.key).to_string()
    } else {
        def.icon.clone()
    }
}

/// Short date-time for tables (`YYYY-MM-DD HH:MM`, UTC).
pub fn short_time(rfc3339: &str) -> String {
    rfc3339
        .get(..16)
        .map(|s| s.replace('T', " "))
        .unwrap_or_default()
}

/// Field errors of a 400 `{field: token}` body (no machine `error` code).
pub fn field_errors(
    e: &crate::api::error::ApiError,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let obj = e.body.as_ref()?.as_object()?;
    (e.status == Some(400) && !obj.contains_key("error")).then(|| obj.clone())
}
