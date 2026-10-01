use super::helpers::*;
use super::*;

use pnex_core::predictive::{
    AnomalyConfig, AnomalyMethod, BreachDirection, ForecastConfig, ForecastModel,
    FORECAST_HORIZON_MAX, PREDICT_MIN_SAMPLES_FLOOR, PREDICT_WINDOW_MAX, PREDICT_WINDOW_MIN,
};

// ─────────────── anomaly / forecast (ml-vision.md step 3) ───────────────

/// Field keys shared by both forms (also the parse-error markers).
const F_KEY: &str = "key";
const F_WINDOW: &str = "window";
const F_MIN: &str = "min_samples";
const F_THRESHOLD: &str = "threshold";
const F_LEVEL: &str = "level";
const F_HAZARD: &str = "hazard";
const F_SEASON: &str = "season_length";
const F_HORIZON: &str = "horizon";
const F_EVERY: &str = "every";

/// Violation code of `check()` that points at a field.
fn field_of_code(code: &str) -> Option<&'static str> {
    match code {
        "predict_window_invalid" => Some(F_WINDOW),
        "predict_min_samples_invalid" => Some(F_MIN),
        "anomaly_threshold_invalid" | "forecast_threshold_invalid" => Some(F_THRESHOLD),
        "predict_level_invalid" => Some(F_LEVEL),
        "anomaly_hazard_invalid" => Some(F_HAZARD),
        "predict_season_invalid" => Some(F_SEASON),
        "forecast_horizon_invalid" => Some(F_HORIZON),
        "forecast_every_invalid" => Some(F_EVERY),
        _ => None,
    }
}

/// Parsed form value: text fields, integers, floats, or an optional float
/// (empty = none — the forecast threshold).
enum Parsed {
    Text(String),
    Int(u32),
    Float(f64),
    OptFloat(Option<f64>),
}

fn parse_field(field: &'static str, raw: &str) -> Option<Parsed> {
    let t = raw.trim();
    match field {
        F_KEY => Some(Parsed::Text(t.to_string())),
        F_THRESHOLD | F_LEVEL | F_HAZARD => parse_secs(t).map(Parsed::Float),
        _ => parse_u32(t).map(Parsed::Int),
    }
}

/// Records the parse outcome of `field`; `true` when the value is usable.
fn track_parse(mut errors: Signal<Vec<&'static str>>, field: &'static str, ok: bool) -> bool {
    errors.with_mut(|errs| {
        errs.retain(|f| *f != field);
        if !ok {
            errs.push(field);
        }
    });
    ok
}

fn apply_anomaly(
    cx: &mut EditorCx,
    field: &'static str,
    raw: String,
    errors: Signal<Vec<&'static str>>,
) {
    let parsed = parse_field(field, &raw);
    if !track_parse(errors, field, parsed.is_some()) {
        return;
    }
    let Some(parsed) = parsed else { return };
    patch_selected(cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::Anomaly { config } = &mut node.kind {
            match (field, parsed) {
                (F_KEY, Parsed::Text(s)) => config.key = s,
                (F_WINDOW, Parsed::Int(n)) => config.window = n,
                (F_MIN, Parsed::Int(n)) => config.min_samples = n,
                (F_SEASON, Parsed::Int(n)) => config.season_length = n,
                (F_THRESHOLD, Parsed::Float(x)) => config.threshold = x,
                (F_LEVEL, Parsed::Float(x)) => config.level = x,
                (F_HAZARD, Parsed::Float(x)) => config.hazard = x,
                _ => {}
            }
        }
    });
}

fn apply_forecast(
    cx: &mut EditorCx,
    field: &'static str,
    raw: String,
    errors: Signal<Vec<&'static str>>,
) {
    let parsed = if field == F_THRESHOLD {
        let t = raw.trim();
        if t.is_empty() {
            Some(Parsed::OptFloat(None))
        } else {
            parse_secs(t).map(|x| Parsed::OptFloat(Some(x)))
        }
    } else {
        parse_field(field, &raw)
    };
    if !track_parse(errors, field, parsed.is_some()) {
        return;
    }
    let Some(parsed) = parsed else { return };
    patch_selected(cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::Forecast { config } = &mut node.kind {
            match (field, parsed) {
                (F_KEY, Parsed::Text(s)) => config.key = s,
                (F_WINDOW, Parsed::Int(n)) => config.window = n,
                (F_MIN, Parsed::Int(n)) => config.min_samples = n,
                (F_SEASON, Parsed::Int(n)) => config.season_length = n,
                (F_HORIZON, Parsed::Int(n)) => config.horizon = n,
                (F_EVERY, Parsed::Int(n)) => config.every = n,
                (F_LEVEL, Parsed::Float(x)) => config.level = x,
                (F_THRESHOLD, Parsed::OptFloat(x)) => config.threshold = x,
                _ => {}
            }
        }
    });
}

/// One labeled input (number or text), red when flagged.
fn field_input(
    label: String,
    hint: String,
    value: Signal<String>,
    numeric: bool,
    invalid: bool,
    disabled: bool,
    oninput: impl FnMut(FormEvent) + 'static,
) -> Element {
    rsx! {
        label { class: "block",
            span { class: "text-xs font-medium text-gray-500 mb-1 block", {label} }
            input {
                class: if invalid {
                    "w-full px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm"
                } else {
                    "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm"
                },
                r#type: if numeric { "number" } else { "text" },
                value: "{value}",
                disabled,
                oninput,
            }
            if !hint.is_empty() {
                span { class: "text-xs text-gray-400 mt-1 block", {hint} }
            }
        }
    }
}

fn method_label(m: AnomalyMethod) -> String {
    match m {
        AnomalyMethod::RobustZ => t!("flows-anomaly-method-robust-z").to_string(),
        AnomalyMethod::ForecastBand => t!("flows-anomaly-method-forecast-band").to_string(),
        AnomalyMethod::Changepoint => t!("flows-anomaly-method-changepoint").to_string(),
    }
}

fn method_help(m: AnomalyMethod) -> String {
    match m {
        AnomalyMethod::RobustZ => t!("flows-anomaly-method-robust-z-help").to_string(),
        AnomalyMethod::ForecastBand => t!("flows-anomaly-method-forecast-band-help").to_string(),
        AnomalyMethod::Changepoint => t!("flows-anomaly-method-changepoint-help").to_string(),
    }
}

/// Compact span of `secs` seconds (`45s`, `5min`, `2h 30min`, `3d 4h`).
fn span_label(secs: u64) -> String {
    let (d, h, m, s) = (
        secs / 86_400,
        (secs % 86_400) / 3600,
        (secs % 3600) / 60,
        secs % 60,
    );
    match (d, h, m) {
        (0, 0, 0) => format!("{s}s"),
        (0, 0, _) if s == 0 => format!("{m}min"),
        (0, 0, _) => format!("{m}min {s}s"),
        (0, _, 0) => format!("{h}h"),
        (0, _, _) => format!("{h}h {m}min"),
        (_, 0, _) => format!("{d}d"),
        _ => format!("{d}d {h}h"),
    }
}

/// Time covered by `horizon` steps at the two most common cadences: the
/// step is the observed sampling interval, unknown until the node runs.
fn horizon_span_hint(horizon: &str) -> Option<String> {
    let steps = parse_u32(horizon.trim()).filter(|n| *n > 0)? as u64;
    Some(
        t!(
            "flows-forecast-horizon-span",
            at_1hz: span_label(steps),
            at_1min: span_label(steps * 60)
        )
        .to_string(),
    )
}

fn model_label(m: ForecastModel) -> String {
    match m {
        ForecastModel::Ets => t!("flows-forecast-model-ets").to_string(),
        ForecastModel::Linear => t!("flows-forecast-model-linear").to_string(),
    }
}

/// Anomaly inspector: method, input key, window / warm-up and the
/// method-specific parameter. Fields failing `check()` turn red.
#[component]
pub(super) fn AnomalyForm(mut cx: EditorCx, initial: AnomalyConfig, can_write: bool) -> Element {
    let key = use_signal(|| initial.key.clone());
    let window = use_signal(|| initial.window.to_string());
    let min_samples = use_signal(|| initial.min_samples.to_string());
    let threshold = use_signal(|| v_to_string(initial.threshold));
    let level = use_signal(|| v_to_string(initial.level));
    let hazard = use_signal(|| v_to_string(initial.hazard));
    let season = use_signal(|| initial.season_length.to_string());
    let errors = use_signal(Vec::<&'static str>::new);

    let current = use_memo(move || {
        let id = cx.selected_node.cloned();
        cx.graph
            .read()
            .nodes
            .iter()
            .find(|n| Some(&n.id) == id.as_ref())
            .and_then(|n| match &n.kind {
                FlowNodeKind::Anomaly { config } => Some(config.clone()),
                _ => None,
            })
            .unwrap_or_default()
    });
    let method = current.read().method;
    let bad_field = current
        .read()
        .check()
        .and_then(|(code, _)| field_of_code(code));
    let flagged = move |f: &'static str| bad_field == Some(f) || errors.read().contains(&f);
    // One closure per field: same shape, the field key is the only change.
    let on = move |f: &'static str, sig: Signal<String>| {
        move |event: FormEvent| {
            let mut sig = sig;
            sig.set(event.value());
            apply_anomaly(&mut cx, f, event.value(), errors);
        }
    };

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-anomaly-help")} }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-anomaly-method")} }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        if let Some(next) = AnomalyMethod::from_wire(&event.value()) {
                            patch_selected(&mut cx, move |node: &mut FlowNode| {
                                if let FlowNodeKind::Anomaly { config } = &mut node.kind {
                                    config.method = next;
                                }
                            });
                        }
                    },
                    for m in AnomalyMethod::ALL {
                        option { key: "{m.wire()}", value: m.wire(), selected: m == method, {method_label(m)} }
                    }
                }
                span { class: "text-xs text-gray-400 mt-1 block", {method_help(method)} }
            }
            {field_input(t!("flows-predict-key").to_string(), t!("flows-predict-key-hint").to_string(),
                key, false, false, !can_write, on(F_KEY, key))}
            div { class: "grid grid-cols-2 gap-2",
                {field_input(t!("flows-predict-window").to_string(),
                    t!("flows-predict-window-hint", min: PREDICT_WINDOW_MIN, max: PREDICT_WINDOW_MAX).to_string(),
                    window, true, flagged(F_WINDOW), !can_write, on(F_WINDOW, window))}
                {field_input(t!("flows-predict-min-samples").to_string(),
                    t!("flows-predict-min-samples-hint", min: PREDICT_MIN_SAMPLES_FLOOR).to_string(),
                    min_samples, true, flagged(F_MIN), !can_write, on(F_MIN, min_samples))}
            }
            match method {
                AnomalyMethod::RobustZ => field_input(t!("flows-anomaly-threshold").to_string(),
                    t!("flows-anomaly-threshold-hint").to_string(),
                    threshold, true, flagged(F_THRESHOLD), !can_write, on(F_THRESHOLD, threshold)),
                AnomalyMethod::ForecastBand => rsx! {
                    div { class: "grid grid-cols-2 gap-2",
                        {field_input(t!("flows-predict-level").to_string(), t!("flows-predict-level-hint").to_string(),
                            level, true, flagged(F_LEVEL), !can_write, on(F_LEVEL, level))}
                        {field_input(t!("flows-predict-season").to_string(), t!("flows-predict-season-hint").to_string(),
                            season, true, flagged(F_SEASON), !can_write, on(F_SEASON, season))}
                    }
                },
                AnomalyMethod::Changepoint => field_input(t!("flows-anomaly-hazard").to_string(),
                    t!("flows-anomaly-hazard-hint").to_string(),
                    hazard, true, flagged(F_HAZARD), !can_write, on(F_HAZARD, hazard)),
            }
            p { class: "text-xs text-gray-400", {t!("flows-anomaly-ports-help")} }
        }
    }
}

/// Forecast inspector: model, input key, window / warm-up, horizon,
/// season (ETS), confidence level, breach threshold + direction, refit
/// period. Fields failing `check()` turn red.
#[component]
pub(super) fn ForecastForm(mut cx: EditorCx, initial: ForecastConfig, can_write: bool) -> Element {
    let key = use_signal(|| initial.key.clone());
    let window = use_signal(|| initial.window.to_string());
    let min_samples = use_signal(|| initial.min_samples.to_string());
    let horizon = use_signal(|| initial.horizon.to_string());
    let season = use_signal(|| initial.season_length.to_string());
    let level = use_signal(|| v_to_string(initial.level));
    let threshold = use_signal(|| initial.threshold.map(v_to_string).unwrap_or_default());
    let every = use_signal(|| initial.every.to_string());
    let errors = use_signal(Vec::<&'static str>::new);

    let current = use_memo(move || {
        let id = cx.selected_node.cloned();
        cx.graph
            .read()
            .nodes
            .iter()
            .find(|n| Some(&n.id) == id.as_ref())
            .and_then(|n| match &n.kind {
                FlowNodeKind::Forecast { config } => Some(config.clone()),
                _ => None,
            })
            .unwrap_or_default()
    });
    let model = current.read().model;
    let direction = current.read().direction;
    let bad_field = current
        .read()
        .check()
        .and_then(|(code, _)| field_of_code(code));
    let flagged = move |f: &'static str| bad_field == Some(f) || errors.read().contains(&f);
    let on = move |f: &'static str, sig: Signal<String>| {
        move |event: FormEvent| {
            let mut sig = sig;
            sig.set(event.value());
            apply_forecast(&mut cx, f, event.value(), errors);
        }
    };

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-forecast-help")} }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-forecast-model")} }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        if let Some(next) = ForecastModel::from_wire(&event.value()) {
                            patch_selected(&mut cx, move |node: &mut FlowNode| {
                                if let FlowNodeKind::Forecast { config } = &mut node.kind {
                                    config.model = next;
                                }
                            });
                        }
                    },
                    for m in ForecastModel::ALL {
                        option { key: "{m.wire()}", value: m.wire(), selected: m == model, {model_label(m)} }
                    }
                }
            }
            {field_input(t!("flows-predict-key").to_string(), t!("flows-predict-key-hint").to_string(),
                key, false, false, !can_write, on(F_KEY, key))}
            div { class: "grid grid-cols-2 gap-2",
                {field_input(t!("flows-predict-window").to_string(),
                    t!("flows-predict-window-hint", min: PREDICT_WINDOW_MIN, max: PREDICT_WINDOW_MAX).to_string(),
                    window, true, flagged(F_WINDOW), !can_write, on(F_WINDOW, window))}
                {field_input(t!("flows-predict-min-samples").to_string(),
                    t!("flows-predict-min-samples-hint", min: PREDICT_MIN_SAMPLES_FLOOR).to_string(),
                    min_samples, true, flagged(F_MIN), !can_write, on(F_MIN, min_samples))}
                {field_input(t!("flows-forecast-horizon").to_string(),
                    t!("flows-forecast-horizon-hint", max: FORECAST_HORIZON_MAX).to_string(),
                    horizon, true, flagged(F_HORIZON), !can_write, on(F_HORIZON, horizon))}
                {field_input(t!("flows-predict-level").to_string(), t!("flows-predict-level-hint").to_string(),
                    level, true, flagged(F_LEVEL), !can_write, on(F_LEVEL, level))}
            }
            if let Some(span) = horizon_span_hint(&horizon.read()) {
                p { class: "text-xs text-gray-400", {span} }
            }
            if model == ForecastModel::Ets {
                {field_input(t!("flows-predict-season").to_string(), t!("flows-predict-season-hint").to_string(),
                    season, true, flagged(F_SEASON), !can_write, on(F_SEASON, season))}
            }
            div { class: "grid grid-cols-2 gap-2",
                {field_input(t!("flows-forecast-threshold").to_string(), t!("flows-forecast-threshold-hint").to_string(),
                    threshold, true, flagged(F_THRESHOLD), !can_write, on(F_THRESHOLD, threshold))}
                label { class: "block",
                    span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-forecast-direction")} }
                    select {
                        class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                        disabled: !can_write,
                        onchange: move |event| {
                            let next = if event.value() == "below" { BreachDirection::Below } else { BreachDirection::Above };
                            patch_selected(&mut cx, move |node: &mut FlowNode| {
                                if let FlowNodeKind::Forecast { config } = &mut node.kind {
                                    config.direction = next;
                                }
                            });
                        },
                        option { value: "above", selected: direction == BreachDirection::Above, {t!("flows-forecast-direction-above")} }
                        option { value: "below", selected: direction == BreachDirection::Below, {t!("flows-forecast-direction-below")} }
                    }
                }
            }
            {field_input(t!("flows-forecast-every").to_string(), t!("flows-forecast-every-hint").to_string(),
                every, true, flagged(F_EVERY), !can_write, on(F_EVERY, every))}
            p { class: "text-xs text-gray-400", {t!("flows-forecast-ports-help")} }
        }
    }
}
