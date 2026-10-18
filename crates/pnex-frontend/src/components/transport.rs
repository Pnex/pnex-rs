//! Shared playback transport: one look for everything that plays over time
//! (case timeline, camera recordings, stream replays, audio / video media).
//! Pieces: [`PlayButton`], [`Clock`], [`ScrubBar`] (covered spans, marks,
//! progress, cursor; click or drag to seek), [`SpeedPicker`], laid out in a
//! [`BAR`] row. [`MediaPlayer`] wraps an `<audio>` / `<video>` element with
//! them instead of the browser's native controls.

use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::icons;
use crate::components::ui::BTN_SECONDARY_SM;

/// Row holding a transport (framed: it also sits on dark media frames).
pub const BAR: &str =
    "flex flex-wrap items-center gap-2 rounded-lg border border-line bg-surface px-2 py-1.5";

/// Play / pause toggle.
#[component]
pub fn PlayButton(playing: bool, on_toggle: EventHandler<()>) -> Element {
    rsx! {
        button {
            class: "{BTN_SECONDARY_SM} h-8 w-9 px-0",
            r#type: "button",
            "aria-label": if playing { t!("transport-pause") } else { t!("transport-play") },
            title: if playing { t!("transport-pause") } else { t!("transport-play") },
            "data-testid": "transport-play",
            onclick: move |_| on_toggle.call(()),
            if playing {
                icons::Pause { class: "h-4 w-4" }
            } else {
                icons::Play { class: "h-4 w-4" }
            }
        }
    }
}

/// The playhead's time (mono), with an optional secondary line (day,
/// duration).
#[component]
pub fn Clock(time: String, sub: Option<String>) -> Element {
    rsx! {
        span { class: "font-mono text-base font-medium text-ink tabular-nums", "{time}" }
        if let Some(sub) = sub {
            span { class: "hidden font-mono text-sm text-slate tabular-nums sm:inline",
                "{sub}"
            }
        }
    }
}

/// Speed choice; values are the caller's own units, labels shown as is.
#[component]
pub fn SpeedPicker(
    value: u32,
    options: Vec<(u32, String)>,
    on_change: EventHandler<u32>,
) -> Element {
    rsx! {
        select {
            class: "h-8 w-20 rounded-md border border-line-strong bg-surface px-2 text-sm text-ink",
            "aria-label": t!("transport-speed"),
            onchange: move |e| {
                if let Ok(v) = e.value().parse() {
                    on_change.call(v);
                }
            },
            for (v, label) in options {
                option { key: "{v}", value: "{v}", selected: v == value, "{label}" }
            }
        }
    }
}

/// `×N` labels of integer speed factors.
pub fn speed_options(factors: &[u32]) -> Vec<(u32, String)> {
    factors.iter().map(|f| (*f, format!("×{f}"))).collect()
}

/// Percent (0–100) of `x` inside `[left, left + width)`.
fn pct_at(x: f64, left: f64, width: f64) -> f64 {
    ((x - left) / width.max(1.0) * 100.0).clamp(0.0, 100.0)
}

/// Scrub strip. Every position is a percent of the strip: `blocks` are
/// covered spans (gaps stay empty), `marks` are point events, `cursor` is
/// the playhead. Click, drag or arrow keys call `on_seek` with a percent.
#[component]
pub fn ScrubBar(
    cursor: Option<f64>,
    #[props(default)] blocks: Vec<(f64, f64)>,
    #[props(default)] marks: Vec<f64>,
    /// Axis labels under the strip, `(percent, text)`.
    #[props(default)]
    ticks: Vec<(f64, String)>,
    on_seek: EventHandler<f64>,
) -> Element {
    let mut el = use_signal(|| None::<Rc<MountedData>>);
    let mut dragging = use_signal(|| false);
    // Pointer → percent through the strip's rect (works on web and native).
    let seek_at = move |x: f64| async move {
        let Some(m) = el.cloned() else {
            return;
        };
        if let Ok(r) = m.get_client_rect().await {
            on_seek.call(pct_at(x, r.origin.x, r.size.width));
        }
    };
    let at = cursor.map(|c| c.clamp(0.0, 100.0));
    rsx! {
        div { class: "min-w-24 flex-1",
            div {
                class: "relative h-8 cursor-pointer touch-none select-none pointer-coarse:h-11",
                role: "slider",
                tabindex: "0",
                "aria-label": t!("transport-position"),
                "aria-valuemin": "0",
                "aria-valuemax": "100",
                "aria-valuenow": "{at.unwrap_or(0.0).round()}",
                "data-testid": "transport-scrub",
                onmounted: move |e| el.set(Some(e.data())),
                onpointerdown: move |e| {
                    dragging.set(true);
                    spawn(seek_at(e.client_coordinates().x));
                },
                onpointermove: move |e| {
                    if dragging() {
                        spawn(seek_at(e.client_coordinates().x));
                    }
                },
                onpointerup: move |_| dragging.set(false),
                onpointerleave: move |_| dragging.set(false),
                onkeydown: move |e| {
                    let step = match e.key() {
                        Key::ArrowLeft => -1.0,
                        Key::ArrowRight => 1.0,
                        _ => return,
                    };
                    on_seek.call((at.unwrap_or(0.0) + step).clamp(0.0, 100.0));
                },
                div { class: "absolute inset-x-0 top-1/2 h-0.5 -translate-y-1/2 bg-line-strong" }
                for (s, e) in blocks {
                    div {
                        key: "{s}",
                        class: "absolute top-1/2 h-2 -translate-y-1/2 rounded-sm bg-signal-soft border border-signal/40",
                        style: "left: {s}%; width: max(2px, {e - s}%);",
                    }
                }
                if let Some(c) = at {
                    div {
                        class: "absolute left-0 top-1/2 h-0.5 -translate-y-1/2 bg-signal",
                        style: "width: {c}%",
                    }
                }
                for (i, m) in marks.iter().enumerate() {
                    span {
                        key: "{i}",
                        class: "absolute top-1/2 h-2.5 w-2.5 -translate-x-1/2 -translate-y-1/2 rotate-45 border-2 border-ink bg-surface",
                        style: "left: {m}%",
                    }
                }
                if let Some(c) = at {
                    span {
                        class: "absolute inset-y-0 w-0.5 -translate-x-1/2 bg-signal",
                        style: "left: {c}%",
                    }
                    // The handle: a knob to grab (bigger under a finger).
                    span {
                        class: "absolute top-1/2 h-4 w-4 -translate-x-1/2 -translate-y-1/2 cursor-grab rounded-full border-2 border-surface bg-signal shadow transition-transform hover:scale-125 pointer-coarse:h-6 pointer-coarse:w-6",
                        class: if dragging() { "scale-125 cursor-grabbing" },
                        style: "left: {c}%",
                        "data-testid": "transport-handle",
                    }
                }
            }
            if !ticks.is_empty() {
                div { class: "relative h-5 font-mono text-xs text-slate tabular-nums",
                    for (p, label) in ticks {
                        span {
                            key: "{p}",
                            class: "absolute -translate-x-1/2 first:translate-x-0 last:-translate-x-full",
                            style: "left: {p}%",
                            "{label}"
                        }
                    }
                }
            }
        }
    }
}

/// `m:ss` (or `h:mm:ss`) of a position in seconds.
pub fn media_clock(secs: f64) -> String {
    let s = if secs.is_finite() {
        secs.max(0.0) as u64
    } else {
        0
    };
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Moves the [`MediaPlayer`] of element `id` to `secs`.
pub fn seek_media(id: &str, secs: f64) {
    let js = format!(
        "const el = document.getElementById({}); if (el) el.currentTime = {};",
        serde_json::Value::from(id),
        secs.max(0.0)
    );
    let _ = dioxus::document::eval(&js);
}

/// Starts the [`MediaPlayer`] of element `id` (chained pieces of audio).
pub fn play_media(id: &str) {
    let js = format!(
        "const el = document.getElementById({}); if (el) el.play();",
        serde_json::Value::from(id)
    );
    let _ = dioxus::document::eval(&js);
}

/// Playback rates of [`MediaPlayer`], in percent.
const MEDIA_RATES: [(u32, &str); 4] = [(50, "×0.5"), (100, "×1"), (150, "×1.5"), (200, "×2")];

/// `<audio>` / `<video>` with the shared transport instead of the native
/// controls. `id` must be unique in the page (it is also how callers read
/// the element, e.g. its `currentTime`).
#[component]
pub fn MediaPlayer(
    id: String,
    src: String,
    video: bool,
    /// `(position s, duration s)` on every time update (synchronised views:
    /// transcript, timeline).
    #[props(default)]
    on_time: EventHandler<(f64, f64)>,
    /// Drawn over the video, same box (detection boxes of the review).
    #[props(default)]
    overlay: Option<Element>,
) -> Element {
    // (position s, duration s, paused) as last read from the element.
    let mut state = use_signal(|| (0.0f64, 0.0f64, true));
    let mut rate = use_signal(|| 100u32);
    let js_el = format!(
        "document.getElementById({})",
        serde_json::Value::from(id.clone())
    );
    let read_js = format!(
        "const el = {js_el}; return el ? [el.currentTime, el.duration || 0, el.paused] : null;"
    );
    let refresh = move || {
        let js = read_js.clone();
        spawn(async move {
            if let Ok(v) = dioxus::document::eval(&js).await {
                let num = |i: usize| v.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0);
                let paused = v.get(2).and_then(|x| x.as_bool()).unwrap_or(true);
                state.set((num(0), num(1), paused));
                on_time.call((num(0), num(1)));
            }
        });
    };
    let run = {
        let js_el = js_el.clone();
        move |body: String| {
            let js = format!("const el = {js_el}; if (el) {{ {body} }}");
            let _ = dioxus::document::eval(&js);
        }
    };
    let (pos, dur, paused) = state();
    let toggle = {
        let run = run.clone();
        move || run("if (el.paused) el.play(); else el.pause();".into())
    };
    let toggle_frame = toggle.clone();
    let seek = {
        let run = run.clone();
        move |p: f64| {
            run(format!(
                "if (el.duration) el.currentTime = el.duration * {} / 100;",
                p
            ))
        }
    };
    let set_rate = move |r: u32| {
        rate.set(r);
        run(format!("el.playbackRate = {} / 100;", r));
    };
    let cursor = (dur > 0.0).then(|| pos / dur * 100.0);
    let (r1, r2, r3, r4, r5) = (
        refresh.clone(),
        refresh.clone(),
        refresh.clone(),
        refresh.clone(),
        refresh.clone(),
    );
    let speeds: Vec<(u32, String)> = MEDIA_RATES
        .iter()
        .map(|(v, l)| (*v, l.to_string()))
        .collect();
    rsx! {
        div { class: "w-full space-y-2",
            if video {
                div { class: "relative",
                    video {
                        id: "{id}",
                        class: "w-full max-h-[420px] rounded-lg bg-gray-900 cursor-pointer",
                        src: "{src}",
                        preload: "metadata",
                        "playsinline": "true",
                        onclick: move |_| toggle_frame(),
                        ontimeupdate: move |_| r1(),
                        ondurationchange: move |_| r2(),
                        onplay: move |_| r3(),
                        onpause: move |_| r4(),
                        onended: move |_| r5(),
                    }
                    {overlay}
                }
            } else {
                audio {
                    id: "{id}",
                    src: "{src}",
                    preload: "metadata",
                    ontimeupdate: move |_| r1(),
                    ondurationchange: move |_| r2(),
                    onplay: move |_| r3(),
                    onpause: move |_| r4(),
                    onended: move |_| r5(),
                }
            }
            div { class: BAR,
                PlayButton { playing: !paused, on_toggle: move |_| toggle() }
                Clock { time: media_clock(pos), sub: Some(media_clock(dur)) }
                ScrubBar { cursor, on_seek: seek }
                SpeedPicker { value: rate(), options: speeds, on_change: set_rate }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_and_percent() {
        assert_eq!(media_clock(0.0), "0:00");
        assert_eq!(media_clock(75.9), "1:15");
        assert_eq!(media_clock(3725.0), "1:02:05");
        assert_eq!(media_clock(f64::NAN), "0:00");
        assert_eq!(pct_at(150.0, 100.0, 200.0), 25.0);
        assert_eq!(pct_at(50.0, 100.0, 200.0), 0.0);
        assert_eq!(pct_at(400.0, 100.0, 200.0), 100.0);
    }
}
