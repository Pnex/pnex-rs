use super::helpers::*;
use super::*;

use pnex_core::weather::{
    WeatherConfig, WeatherProvider, WEATHER_INTERVAL_MAX, WEATHER_INTERVAL_MIN,
};

// ─────────────── weather source (D140) ───────────────

/// Stores the config in the form signal and in the selected node.
fn commit(cx: &mut EditorCx, mut cfg: Signal<WeatherConfig>, next: WeatherConfig) {
    cfg.set(next.clone());
    patch_selected(cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::Weather { config } = &mut node.kind {
            *config = next;
        }
    });
}

fn parse_coord(raw: &str) -> Option<f64> {
    raw.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

/// Weather inspector: provider (allowlist), coordinates, refresh interval,
/// fetch at start. The three outputs feed memory-write or metric.
#[component]
pub(super) fn WeatherForm(cx: EditorCx, initial: WeatherConfig, can_write: bool) -> Element {
    let cfg = use_signal(move || initial.clone());
    let current = cfg();
    let error = current.check().map(|(code, _)| match code {
        "weather_latitude" => t!("flows-weather-latitude-invalid").to_string(),
        "weather_longitude" => t!("flows-weather-longitude-invalid").to_string(),
        _ => t!(
            "flows-weather-interval-invalid",
            min : WEATHER_INTERVAL_MIN,
            max : WEATHER_INTERVAL_MAX
        )
        .to_string(),
    });
    let non_commercial = current.provider == WeatherProvider::OpenMeteo;
    let attribution = current.provider.attribution();
    let lat = current.latitude.to_string();
    let lon = current.longitude.to_string();
    let interval = current.interval_min.to_string();
    let provider_key = current.provider.as_str();
    let (c1, c2, c3, c4, c5) = (
        current.clone(),
        current.clone(),
        current.clone(),
        current.clone(),
        current.clone(),
    );
    let mut cx_provider = cx;
    let mut cx_lat = cx;
    let mut cx_lon = cx;
    let mut cx_interval = cx;
    let mut cx_start = cx;
    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-weather-help")} }
            label { class: "block",
                span { class: "mb-1 block text-xs font-medium text-gray-500",
                    {t!("flows-weather-provider")}
                }
                select {
                    class: "w-full rounded-lg border border-gray-300 bg-white px-2 py-1.5 text-sm",
                    disabled: !can_write,
                    value: "{provider_key}",
                    onchange: move |e| {
                        let mut next = c1.clone();
                        next.provider = if e.value() == "open_meteo" {
                            WeatherProvider::OpenMeteo
                        } else {
                            WeatherProvider::MetNorway
                        };
                        commit(&mut cx_provider, cfg, next);
                    },
                    option { value: "met_norway", {t!("flows-weather-provider-met")} }
                    option { value: "open_meteo", {t!("flows-weather-provider-open-meteo")} }
                }
                if non_commercial {
                    span { class: "mt-1 block text-xs text-amber-700",
                        {t!("flows-weather-non-commercial")}
                    }
                }
            }
            div { class: "grid grid-cols-2 gap-2",
                label { class: "block",
                    span { class: "mb-1 block text-xs font-medium text-gray-500",
                        {t!("flows-weather-latitude")}
                    }
                    input {
                        class: "w-full rounded-lg border border-gray-300 px-2 py-1.5 text-sm",
                        inputmode: "decimal",
                        value: "{lat}",
                        disabled: !can_write,
                        onchange: move |e| {
                            if let Some(v) = parse_coord(&e.value()) {
                                let mut next = c2.clone();
                                next.latitude = v;
                                commit(&mut cx_lat, cfg, next);
                            }
                        },
                    }
                }
                label { class: "block",
                    span { class: "mb-1 block text-xs font-medium text-gray-500",
                        {t!("flows-weather-longitude")}
                    }
                    input {
                        class: "w-full rounded-lg border border-gray-300 px-2 py-1.5 text-sm",
                        inputmode: "decimal",
                        value: "{lon}",
                        disabled: !can_write,
                        onchange: move |e| {
                            if let Some(v) = parse_coord(&e.value()) {
                                let mut next = c3.clone();
                                next.longitude = v;
                                commit(&mut cx_lon, cfg, next);
                            }
                        },
                    }
                }
            }
            label { class: "block",
                span { class: "mb-1 block text-xs font-medium text-gray-500",
                    {t!("flows-weather-interval")}
                }
                input {
                    class: "w-32 rounded-lg border border-gray-300 px-2 py-1.5 text-sm",
                    r#type: "number",
                    min: "{WEATHER_INTERVAL_MIN}",
                    max: "{WEATHER_INTERVAL_MAX}",
                    value: "{interval}",
                    disabled: !can_write,
                    onchange: move |e| {
                        if let Ok(v) = e.value().trim().parse::<u32>() {
                            let mut next = c4.clone();
                            next.interval_min = v;
                            commit(&mut cx_interval, cfg, next);
                        }
                    },
                }
            }
            label { class: "flex items-center gap-2 text-sm text-gray-700",
                input {
                    r#type: "checkbox",
                    checked: current.emit_on_start,
                    disabled: !can_write,
                    onchange: move |e| {
                        let mut next = c5.clone();
                        next.emit_on_start = e.checked();
                        commit(&mut cx_start, cfg, next);
                    },
                }
                {t!("flows-weather-emit-on-start")}
            }
            if let Some(err) = error {
                p { class: "text-xs text-red-600", "{err}" }
            }
            p { class: "text-xs text-gray-500", {t!("flows-weather-ports-help")} }
            p { class: "text-[10px] text-gray-400", "{attribution}" }
        }
    }
}
