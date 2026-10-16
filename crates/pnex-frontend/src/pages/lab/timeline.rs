//! Timeline of a Vision Lab project (design-system.md D220): the pictures
//! of one source (a video, a camera recording, a batch of photos) on the
//! shared timeline, one mark per picture coloured by its state, one lane
//! per class showing where it appears, and the shared transport (previous,
//! play, next, clock, speed). A source picker switches between the sources
//! of the project and tells how far each one is labeled.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::lab::LabImage;

use super::workspace::class_color;
use crate::components::timeline::axis::{zoom_min, Scale};
use crate::components::timeline::{Body, Item, Lane, TimeBase, Timeline};
use crate::components::transport::{speed_options, Clock, PlayButton, SpeedPicker};
use crate::components::ui::{BTN_SECONDARY_SM, PANEL};

/// Element id of the lanes column.
const CANVAS: &str = "lab-frise";
/// Speeds of the playback (pictures per second).
pub(super) const SPEEDS: [u32; 4] = [1, 2, 4, 8];
/// A window is never narrower than this (ms): a short clip stays readable.
const MIN_WINDOW_MS: i64 = 2_000;

/// Where the pictures of a source come from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum SourceKind {
    Video,
    Recording,
    Review,
    Photos,
}

/// One source of the project: its pictures in time order.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Source {
    pub key: String,
    pub kind: SourceKind,
    /// Name shown in the picker (the video, the camera...).
    pub name: String,
    /// `(index in the project list, instant)`, by instant.
    pub frames: Vec<(usize, i64)>,
    pub labeled: usize,
}

impl Source {
    pub fn base(&self) -> TimeBase {
        match self.kind {
            SourceKind::Video => TimeBase::Media,
            SourceKind::Photos => TimeBase::Rank,
            _ => TimeBase::Wall { local: true },
        }
    }

    /// Window framing every picture, padded.
    pub fn window(&self) -> (i64, i64) {
        let first = self.frames.first().map_or(0, |f| f.1);
        let last = self.frames.last().map_or(0, |f| f.1);
        let pad = ((last - first) / 20).max(MIN_WINDOW_MS / 2);
        (first - pad, last + pad)
    }

    /// Position of a picture of the project in this source.
    pub fn position(&self, index: usize) -> Option<usize> {
        self.frames.iter().position(|(i, _)| *i == index)
    }
}

/// The sources of the project, in the order their first picture appears.
/// A picture without an instant (a photo) is ranked one second apart.
pub(super) fn sources(list: &[LabImage]) -> Vec<Source> {
    let mut out: Vec<Source> = Vec::new();
    for (i, img) in list.iter().enumerate() {
        let pos = match out.iter().position(|s| s.key == img.source) {
            Some(p) => p,
            None => {
                let kind = match img.source.split('-').next() {
                    Some("video") => SourceKind::Video,
                    Some("rec") => SourceKind::Recording,
                    Some("review") => SourceKind::Review,
                    _ => SourceKind::Photos,
                };
                // A video frame is named "<video> mm:ss": the video's name.
                let name = match kind {
                    SourceKind::Video => img
                        .name
                        .rsplit_once(' ')
                        .filter(|(_, t)| t.contains(':'))
                        .map_or(img.name.clone(), |(n, _)| n.to_string()),
                    _ => img.name.clone(),
                };
                out.push(Source {
                    key: img.source.clone(),
                    kind,
                    name,
                    frames: Vec::new(),
                    labeled: 0,
                });
                out.len() - 1
            }
        };
        let s = &mut out[pos];
        let at = match (s.kind, img.frame_ms) {
            (SourceKind::Photos, _) | (_, None) => s.frames.len() as i64 * 1000,
            (_, Some(ms)) => ms,
        };
        s.frames.push((i, at));
        s.labeled += usize::from(img.annotated());
    }
    for s in &mut out {
        s.frames.sort_by_key(|f| f.1);
    }
    out
}

/// Picture `delta` steps from `index` within its source, if any.
pub(super) fn step(sources: &[Source], index: usize, delta: i64) -> Option<usize> {
    let s = sources.iter().find(|s| s.position(index).is_some())?;
    let p = s.position(index)? as i64 + delta;
    (p >= 0)
        .then(|| s.frames.get(p as usize).map(|f| f.0))
        .flatten()
}

/// Spans where a class is seen: each picture holding it covers half the
/// gap to its neighbours; touching spans merge.
pub(super) fn presence(times: &[i64], has: &[bool]) -> Vec<(i64, i64)> {
    let gap = |a: usize, b: usize| (times[b] - times[a]).max(0) / 2;
    let mut out: Vec<(i64, i64)> = Vec::new();
    for i in (0..times.len()).filter(|i| has[*i]) {
        let before = if i > 0 {
            gap(i - 1, i)
        } else if times.len() > 1 {
            gap(0, 1)
        } else {
            500
        };
        let after = if i + 1 < times.len() {
            gap(i, i + 1)
        } else {
            before
        };
        let (a, b) = (times[i] - before, times[i] + after);
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = b,
            _ => out.push((a, b)),
        }
    }
    out
}

/// Fill of a picture's mark: to review, no object, labeled, to label.
fn state_fill(img: &LabImage) -> &'static str {
    if img.review {
        "#8b5cf6"
    } else if img.negative {
        "#9ca3af"
    } else if img.boxes.is_empty() {
        "#fbbf24"
    } else {
        "#10b981"
    }
}

#[component]
pub(super) fn LabTimeline(
    list: Vec<LabImage>,
    classes: Vec<String>,
    sources: Vec<Source>,
    selected: usize,
    playing: bool,
    speed: u32,
    on_select: EventHandler<usize>,
    on_source: EventHandler<usize>,
    on_play: EventHandler<bool>,
    on_speed: EventHandler<u32>,
) -> Element {
    // Zoomed window, kept while the same source is shown.
    let mut zoom = use_signal(|| None::<(String, (i64, i64))>);
    // Where each class appears: on demand, the pictures lane alone by default.
    let mut show_classes = use_signal(|| false);
    let Some(src_i) = sources.iter().position(|s| s.position(selected).is_some()) else {
        return rsx! {};
    };
    let src = sources[src_i].clone();
    let base = src.base();
    let win = match zoom() {
        Some((k, w)) if k == src.key => w,
        _ => src.window(),
    };
    let scale = Scale::linear(win);
    let pos = src.position(selected).unwrap_or(0);
    let cursor = src.frames.get(pos).map_or(0, |f| f.1);
    let times: Vec<i64> = src.frames.iter().map(|f| f.1).collect();

    let frames: Vec<Item> = src
        .frames
        .iter()
        .filter_map(|(i, at)| {
            let img = list.get(*i)?;
            Some(Item {
                id: img.id.clone(),
                from: *at,
                to: *at,
                tip: Some(format!("{} · {}", img.name, state_text(img))),
                fill: Some(state_fill(img).to_string()),
                selected: *i == selected,
                ..Default::default()
            })
        })
        .collect();
    let mut lanes = vec![Lane {
        count: Some(src.frames.len()),
        testid: Some("lab-frames-lane"),
        item_testid: "lab-frame",
        ..Lane::new(
            "frames",
            t!("lab-tl-images"),
            Body::Items {
                items: frames,
                max_rows: 1,
                stack: false,
                empty: None,
            },
        )
    }];
    for (c, name) in classes.iter().enumerate().filter(|_| show_classes()) {
        let has: Vec<bool> = src
            .frames
            .iter()
            .map(|(i, _)| {
                list.get(*i)
                    .is_some_and(|img| img.boxes.iter().any(|b| b.class == c as u32))
            })
            .collect();
        lanes.push(Lane {
            color: Some(class_color(c as u32).to_string()),
            count: Some(has.iter().filter(|h| **h).count()),
            ..Lane::new(
                format!("class-{c}"),
                name.clone(),
                Body::Spans {
                    spans: presence(&times, &has),
                    gaps: false,
                    marks: Vec::new(),
                },
            )
        });
    }

    // Seeking lands on the nearest picture.
    let near = src.frames.clone();
    let seek = move |t: i64| {
        if let Some((i, _)) = near.iter().min_by_key(|f| (f.1 - t).abs()) {
            on_select.call(*i);
        }
    };
    let by_id: Vec<(String, usize)> = src
        .frames
        .iter()
        .filter_map(|(i, _)| list.get(*i).map(|img| (img.id.clone(), *i)))
        .collect();
    let key = src.key.clone();
    let zoom_by = move |factor: f64| {
        zoom.set(Some((
            key.clone(),
            zoom_min(win, cursor, factor, MIN_WINDOW_MS),
        )))
    };
    let mut zoom_in = zoom_by.clone();
    let mut zoom_out = zoom_by;
    let source_label = |s: &Source| {
        let kind = kind_label(s.kind);
        format!(
            "{kind} · {} · {}",
            s.name,
            t!("lab-source-progress", done : s.labeled, total : s.frames.len())
        )
    };

    rsx! {
        section {
            class: "{PANEL} overflow-hidden",
            "aria-label": t!("lab-tl-title"),
            // One bar: transport, then the source, then the zoom.
            div { class: "flex flex-wrap items-center gap-2 border-b border-line px-3 py-2",
                PlayButton { playing, on_toggle: move |_| on_play.call(!playing) }
                // Photos have no time: their rank alone.
                Clock {
                    time: if base == TimeBase::Rank { format!("{} / {}", pos + 1, src.frames.len()) } else { base.clock(cursor) },
                    sub: (base != TimeBase::Rank).then(|| format!("{} / {}", pos + 1, src.frames.len())),
                }
                SpeedPicker {
                    value: speed,
                    options: speed_options(&SPEEDS),
                    on_change: move |v| on_speed.call(v),
                }
                div { class: "min-w-0 flex-1 basis-48",
                    if sources.len() > 1 {
                        label { class: "sr-only", r#for: "lab-source", {t!("lab-source")} }
                        select {
                            id: "lab-source",
                            class: "h-8 min-w-0 w-full rounded-md border border-line-strong bg-surface px-2 text-sm text-ink",
                            "data-testid": "lab-source",
                            onchange: move |e| {
                                if let Ok(k) = e.value().parse::<usize>() {
                                    on_source.call(k);
                                }
                            },
                            for (k, s) in sources.iter().enumerate() {
                                option {
                                    key: "{s.key}",
                                    value: "{k}",
                                    selected: k == src_i,
                                    {source_label(s)}
                                }
                            }
                        }
                    } else {
                        span { class: "block truncate text-sm text-ink", {source_label(&src)} }
                    }
                }
                div { class: "ml-auto flex items-center gap-1",
                    button {
                        class: if show_classes() { "h-8 rounded-md border border-signal bg-signal-soft px-2.5 text-sm text-signal" } else { "h-8 rounded-md border border-line-strong bg-surface px-2.5 text-sm text-ink hover:bg-gray-50" },
                        r#type: "button",
                        "aria-pressed": show_classes(),
                        title: t!("lab-tl-classes-help"),
                        onclick: move |_| show_classes.set(!show_classes()),
                        {t!("lab-tl-classes")}
                    }
                    button {
                        class: "{BTN_SECONDARY_SM} h-8 w-8",
                        r#type: "button",
                        "aria-label": t!("lab-tl-zoom-out"),
                        onclick: move |_| zoom_out(1.5),
                        "−"
                    }
                    button {
                        class: "{BTN_SECONDARY_SM} h-8 w-8",
                        r#type: "button",
                        "aria-label": t!("lab-tl-zoom-in"),
                        onclick: move |_| zoom_in(0.66),
                        "+"
                    }
                    button {
                        class: "{BTN_SECONDARY_SM} h-8",
                        r#type: "button",
                        onclick: move |_| zoom.set(None),
                        {t!("lab-tl-fit")}
                    }
                }
            }
            Timeline {
                canvas: CANVAS,
                scale,
                base,
                cursor,
                lanes,
                header: match base {
                    TimeBase::Rank => t!("lab-tl-rank").to_string(),
                    TimeBase::Media => t!("lab-tl-media").to_string(),
                    TimeBase::Wall { .. } => t!("cw-local-time").to_string(),
                },
                density: false,
                on_seek: seek,
                on_item: move |(_, id): (String, String)| {
                    if let Some((_, i)) = by_id.iter().find(|(x, _)| *x == id) {
                        on_select.call(*i);
                    }
                },
                on_frame: move |w: (i64, i64)| zoom.set(Some((src.key.clone(), w))),
            }
            div { class: "flex flex-wrap items-center gap-3 border-t border-line px-3 py-2 text-xs text-slate",
                Legend { color: "#fbbf24", text: t!("lab-state-todo") }
                Legend { color: "#10b981", text: t!("lab-tl-labeled") }
                Legend { color: "#9ca3af", text: t!("lab-state-negative") }
                Legend { color: "#8b5cf6", text: t!("lab-tl-review") }
            }
        }
    }
}

#[component]
fn Legend(color: &'static str, text: String) -> Element {
    rsx! {
        span { class: "inline-flex items-center gap-1",
            span {
                class: "h-2.5 w-2.5 rotate-45 rounded-sm border border-ink",
                style: "background: {color}",
            }
            "{text}"
        }
    }
}

/// Name of a kind of source. Render scope only (`t!`).
pub(super) fn kind_label(kind: SourceKind) -> String {
    match kind {
        SourceKind::Video => t!("lab-source-video"),
        SourceKind::Recording => t!("lab-source-recording"),
        SourceKind::Review => t!("lab-source-review"),
        SourceKind::Photos => t!("lab-source-photos"),
    }
    .to_string()
}

/// State of a picture, in words. Render scope only (`t!`).
fn state_text(img: &LabImage) -> String {
    if img.review {
        t!("lab-tl-review").to_string()
    } else if img.negative {
        t!("lab-state-negative").to_string()
    } else if img.boxes.is_empty() {
        t!("lab-state-todo").to_string()
    } else {
        t!("lab-state-boxes", count : img.boxes.len()).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(source: &str, frame_ms: Option<i64>, labeled: bool) -> LabImage {
        LabImage {
            id: format!("{source}-{frame_ms:?}"),
            asset_id: String::new(),
            name: "clip.mp4 00:03".into(),
            width: 1,
            height: 1,
            source: source.into(),
            split: Default::default(),
            boxes: Vec::new(),
            negative: labeled,
            review: false,
            proposals: Vec::new(),
            luma: None,
            frame_ms,
            version: 1,
            updated_at: String::new(),
        }
    }

    #[test]
    fn sources_group_and_order_pictures() {
        let list = vec![
            img("video-a", Some(6_000), false),
            img("import-x", None, true),
            img("video-a", Some(0), true),
            img("import-x", None, false),
        ];
        let s = sources(&list);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].kind, SourceKind::Video);
        assert_eq!(s[0].name, "clip.mp4");
        assert_eq!(s[0].frames, vec![(2, 0), (0, 6_000)]);
        assert_eq!(s[0].labeled, 1);
        assert_eq!(s[1].frames, vec![(1, 0), (3, 1_000)]);
        assert_eq!(step(&s, 2, 1), Some(0));
        assert_eq!(step(&s, 0, 1), None, "a step never leaves its source");
    }

    #[test]
    fn presence_merges_neighbouring_pictures() {
        let t = [0, 2_000, 4_000, 6_000];
        assert_eq!(
            presence(&t, &[true, true, false, true]),
            vec![(-1_000, 3_000), (5_000, 7_000)]
        );
        assert!(presence(&t, &[false; 4]).is_empty());
    }
}
