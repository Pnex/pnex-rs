//! Page `/lab/:id` — annotation workspace of a Vision Lab project (D198):
//! image strip, the picture with its boxes (drag to draw, click to
//! select), class picker, live statistics and frozen dataset versions.
//!
//! Edits are applied locally at once and saved in the background, one
//! request at a time per image (each answer carries the version the next
//! save sends: a burst of edits never conflicts with itself).
//! Keyboard: 1–9 class, Delete removes the selected box, ← / → image,
//! N toggles "no object".

use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::lab::{
    self, LabAnnotationsInput, LabBox, LabImage, LabProject, LabSplit, LabSuggestRequest,
    LabSuggestion,
};

use crate::api;
use crate::app::Route;
use crate::components::ui::{BTN_PRIMARY_SM, BTN_SECONDARY_SM, FIELD_LABEL, INPUT};
use crate::state::{org, toasts};

use super::picker::PhotoPicker;
use super::recording::RecordingImport;
use super::ProjectForm;

/// Box colours by class index (cycled), readable on photos.
const CLASS_COLORS: [&str; 8] = [
    "#e11d48", "#2563eb", "#16a34a", "#d97706", "#9333ea", "#0891b2", "#db2777", "#65a30d",
];

/// Smallest box kept (normalized side): a click is not a box.
const MIN_BOX: f32 = 0.005;

pub(crate) fn class_color(class: u32) -> &'static str {
    CLASS_COLORS[class as usize % CLASS_COLORS.len()]
}

/// Normalized box from two corners.
fn rect_of(a: (f32, f32), b: (f32, f32)) -> [f32; 4] {
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    [x0, y0, x1 - x0, y1 - y0]
}

/// CSS of a box overlay (percentages of the picture).
fn box_style(b: &[f32; 4], color: &str) -> String {
    format!(
        "left: {}%; top: {}%; width: {}%; height: {}%; border-color: {color};",
        b[0] * 100.0,
        b[1] * 100.0,
        b[2] * 100.0,
        b[3] * 100.0
    )
}

/// Save state of the image being edited.
#[derive(Clone, Copy, Default, PartialEq)]
struct SaveState {
    saving: bool,
    /// An edit arrived during a save: save again when it returns.
    dirty: bool,
}

#[component]
pub fn LabProjectPage(id: String) -> Element {
    let pid = id.clone();
    let can_write = org::current_can_write();
    let mut reload = use_signal(|| 0u32);
    let mut images = use_signal(Vec::<LabImage>::new);
    let mut loaded = use_signal(|| false);
    let mut selected = use_signal(|| 0usize);
    let mut class = use_signal(|| 0u32);
    let mut selected_box = use_signal(|| None::<usize>);
    let mut picker_open = use_signal(|| false);
    let mut recording_open = use_signal(|| false);
    let mut editing = use_signal(|| false);
    let mut save_state = use_signal(SaveState::default);
    // Proposals of a registry model, for one image: (image id, boxes).
    let mut suggestions = use_signal(|| (String::new(), Vec::<LabSuggestion>::new()));
    let mut model = use_signal(String::new);
    let mut suggesting = use_signal(|| false);
    let models = use_resource(|| async {
        api::ml_models::list(Some(100), None)
            .await
            .map(|p| p.results)
            .unwrap_or_default()
    });

    let project = use_resource({
        let pid = pid.clone();
        move || {
            let _ = reload();
            let pid = pid.clone();
            async move { api::lab::project(&pid).await }
        }
    });
    use_effect({
        let pid = pid.clone();
        move || {
            let _ = reload();
            let pid = pid.clone();
            spawn(async move {
                match api::lab::images(&pid).await {
                    Ok(list) => images.set(list),
                    Err(e) => toasts::error(e),
                }
                loaded.set(true);
            });
        }
    });

    let project_now: Option<LabProject> = project
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok().cloned());
    let classes: Vec<String> = project_now
        .as_ref()
        .map(|p| p.classes.clone())
        .unwrap_or_default();
    let list = images.read().clone();
    let count = list.len();
    let index = selected().min(count.saturating_sub(1));
    let current: Option<LabImage> = list.get(index).cloned();
    let stats = lab::stats(&classes, &list);

    // Saves the local annotations of image `i`, serialized (see SaveState).
    let mut persist = move |i: usize| {
        if save_state.peek().saving {
            save_state.write().dirty = true;
            return;
        }
        save_state.set(SaveState {
            saving: true,
            dirty: false,
        });
        spawn(async move {
            loop {
                let Some(img) = images.peek().get(i).cloned() else {
                    break;
                };
                let input = LabAnnotationsInput {
                    boxes: img.boxes.clone(),
                    negative: img.negative,
                    split: img.split,
                    expected_version: img.version,
                };
                match api::lab::save_annotations(&img.id, &input).await {
                    Ok(saved) => {
                        if let Some(slot) = images.write().get_mut(i) {
                            slot.version = saved.version;
                            slot.updated_at = saved.updated_at;
                        }
                    }
                    Err(e) => {
                        toasts::error(e);
                        // Resync with the server (another editor, rejected box).
                        reload.with_mut(|r| *r += 1);
                        break;
                    }
                }
                if !save_state.peek().dirty {
                    break;
                }
                save_state.write().dirty = false;
            }
            save_state.set(SaveState::default());
        });
    };

    // Local edit of the current image + background save.
    let mut edit = move |f: Box<dyn FnOnce(&mut LabImage)>| {
        let i = *selected.peek();
        let changed = {
            let mut list = images.write();
            match list.get_mut(i) {
                Some(img) => {
                    f(img);
                    true
                }
                None => false,
            }
        };
        if changed {
            persist(i);
        }
    };

    let mut go = move |delta: i64| {
        let n = images.peek().len() as i64;
        if n == 0 {
            return;
        }
        let next = (*selected.peek() as i64 + delta).clamp(0, n - 1) as usize;
        selected.set(next);
        selected_box.set(None);
    };

    let on_key = move |e: KeyboardEvent| {
        let key = e.key().to_string();
        if let Some(d) = key.chars().next().and_then(|c| c.to_digit(10)) {
            if key.len() == 1 && (1..=9).contains(&d) {
                let c = d - 1;
                class.set(c);
                if let Some(b) = *selected_box.peek() {
                    edit(Box::new(move |img: &mut LabImage| {
                        if let Some(bx) = img.boxes.get_mut(b) {
                            bx.class = c;
                        }
                    }));
                }
            }
            return;
        }
        match key.as_str() {
            "ArrowRight" => go(1),
            "ArrowLeft" => go(-1),
            "Delete" | "Backspace" => {
                let sel = *selected_box.peek();
                if let Some(b) = sel {
                    selected_box.set(None);
                    edit(Box::new(move |img: &mut LabImage| {
                        if b < img.boxes.len() {
                            img.boxes.remove(b);
                        }
                    }));
                }
            }
            "n" | "N" => edit(Box::new(|img: &mut LabImage| {
                img.negative = !img.negative;
                if img.negative {
                    img.boxes.clear();
                }
            })),
            _ => {}
        }
    };

    let title = project_now
        .as_ref()
        .map(|p| p.name.clone())
        .unwrap_or_default();
    let present: Vec<String> = list.iter().map(|i| i.asset_id.clone()).collect();
    let saving = save_state().saving;
    let shown: Vec<LabSuggestion> = {
        let (sid, list) = &*suggestions.read();
        if current.as_ref().is_some_and(|c| &c.id == sid) {
            list.clone()
        } else {
            Vec::new()
        }
    };
    let model_options: Vec<(String, String)> = models
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|m| (m.id, m.name))
        .collect();

    // Keeps proposals as boxes (their mapped class, else the current one).
    let mut accept = move |picked: Vec<usize>| {
        let (sid, list) = suggestions.peek().clone();
        let c = *class.peek();
        let boxes: Vec<LabBox> = picked
            .iter()
            .filter_map(|i| list.get(*i))
            .map(|s| LabBox {
                class: s.class.unwrap_or(c),
                bbox: s.bbox,
            })
            .collect();
        let rest: Vec<LabSuggestion> = list
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !picked.contains(i))
            .map(|(_, s)| s)
            .collect();
        suggestions.set((sid, rest));
        if !boxes.is_empty() {
            edit(Box::new(move |img: &mut LabImage| {
                img.negative = false;
                img.boxes.extend(boxes);
            }));
        }
    };
    let on_suggest = move |_| {
        let Some(img) = images.peek().get(*selected.peek()).cloned() else {
            return;
        };
        let req = LabSuggestRequest {
            model_id: model.peek().clone(),
            min_score: 0.0,
        };
        let none = t!("lab-suggest-none").to_string();
        suggesting.set(true);
        spawn(async move {
            match api::lab::suggest(&img.id, &req).await {
                Ok(list) => {
                    if list.is_empty() {
                        toasts::info(none);
                    }
                    suggestions.set((img.id, list));
                }
                Err(e) => toasts::error(e),
            }
            suggesting.set(false);
        });
    };

    rsx! {
        div { class: "flex flex-col gap-4 p-4 lg:p-6 min-h-0",
            div { class: "flex flex-wrap items-center gap-3",
                Link {
                    class: "text-sm text-gray-500 hover:text-gray-800",
                    to: Route::Lab {},
                    "← "
                    {t!("nav-lab")}
                }
                h1 { class: "text-xl font-semibold text-gray-900 flex-1 min-w-0 truncate",
                    "{title}"
                }
                if saving {
                    span { class: "text-xs text-gray-500", {t!("lab-saving")} }
                }
                if can_write {
                    button {
                        class: BTN_SECONDARY_SM,
                        onclick: move |_| editing.set(true),
                        {t!("lab-edit-short")}
                    }
                    button {
                        class: BTN_PRIMARY_SM,
                        onclick: move |_| picker_open.set(true),
                        {t!("lab-add-images")}
                    }
                    button {
                        class: BTN_SECONDARY_SM,
                        onclick: move |_| recording_open.set(true),
                        {t!("lab-from-recording")}
                    }
                }
            }
            if classes.is_empty() && project_now.is_some() {
                p { class: "text-sm text-amber-700", {t!("lab-no-class")} }
            }
            div { class: "grid gap-4 lg:grid-cols-[14rem_minmax(0,1fr)_17rem]",
                ImageStrip {
                    images: list.clone(),
                    selected: index,
                    loaded: loaded(),
                    on_select: move |i: usize| {
                        selected.set(i);
                        selected_box.set(None);
                    },
                }
                div {
                    class: "space-y-3 outline-none min-w-0",
                    tabindex: "0",
                    onkeydown: on_key,
                    ClassBar {
                        classes: classes.clone(),
                        current: class(),
                        on_pick: move |c: u32| {
                            class.set(c);
                            if let Some(b) = *selected_box.peek() {
                                edit(
                                    Box::new(move |img: &mut LabImage| {
                                        if let Some(bx) = img.boxes.get_mut(b) {
                                            bx.class = c;
                                        }
                                    }),
                                );
                            }
                        },
                    }
                    if let Some(img) = current.clone() {
                        Annotator {
                            key: "{img.id}",
                            image: img.clone(),
                            classes: classes.clone(),
                            class: class(),
                            can_write: can_write && !classes.is_empty(),
                            selected_box: selected_box(),
                            suggestions: shown.clone(),
                            on_accept: move |i: usize| accept(vec![i]),
                            on_select_box: move |b: Option<usize>| selected_box.set(b),
                            on_add_box: move |bbox: [f32; 4]| {
                                let c = *class.peek();
                                edit(
                                    Box::new(move |img: &mut LabImage| {
                                        img.negative = false;
                                        img.boxes.push(LabBox { class: c, bbox });
                                    }),
                                );
                            },
                        }
                        ImageTools {
                            image: img,
                            index,
                            count,
                            can_write,
                            selected_box: selected_box(),
                            on_prev: move |_| go(-1),
                            on_next: move |_| go(1),
                            on_remove_box: move |_| {
                                let sel = *selected_box.peek();
                                if let Some(b) = sel {
                                    selected_box.set(None);
                                    edit(
                                        Box::new(move |img: &mut LabImage| {
                                            if b < img.boxes.len() {
                                                img.boxes.remove(b);
                                            }
                                        }),
                                    );
                                }
                            },
                            on_negative: move |on: bool| {
                                edit(
                                    Box::new(move |img: &mut LabImage| {
                                        img.negative = on;
                                        if on {
                                            img.boxes.clear();
                                        }
                                    }),
                                );
                            },
                            on_split: move |s: LabSplit| edit(Box::new(move |img: &mut LabImage| img.split = s)),
                            on_remove_image: move |_| {
                                let Some(id) = images.peek().get(*selected.peek()).map(|i| i.id.clone()) else {
                                    return;
                                };
                                let done = t!("lab-image-removed").to_string();
                                spawn(async move {
                                    match api::lab::remove_image(&id).await {
                                        Ok(_) => {
                                            toasts::success(done);
                                            selected_box.set(None);
                                            reload.with_mut(|r| *r += 1);
                                        }
                                        Err(e) => toasts::error(e),
                                    }
                                });
                            },
                        }
                        if can_write && !classes.is_empty() {
                            SuggestBar {
                                models: model_options.clone(),
                                model: model(),
                                busy: suggesting(),
                                count: shown.iter().filter(|s| s.class.is_some()).count(),
                                total: shown.len(),
                                on_model: move |m: String| model.set(m),
                                on_suggest,
                                // Only the proposals whose label is a class: the
                                // others are kept one by one, on purpose.
                                on_keep_all: move |_| {
                                    let mapped: Vec<usize> = suggestions
                                        .peek()
                                        .1
                                        .iter()
                                        .enumerate()
                                        .filter(|(_, s)| s.class.is_some())
                                        .map(|(i, _)| i)
                                        .collect();
                                    accept(mapped);
                                },
                                on_dismiss: move |_| suggestions.set((String::new(), Vec::new())),
                            }
                        }
                    } else if loaded() {
                        div { class: "rounded-lg border border-dashed border-gray-300 bg-gray-50 p-8 text-center",
                            p { class: "text-sm text-gray-700", {t!("lab-no-image")} }
                            p { class: "text-xs text-gray-500 mt-1", {t!("lab-no-image-help")} }
                        }
                    }
                }
                div { class: "space-y-4",
                    StatsPanel { classes: classes.clone(), stats }
                    VersionsPanel { project: pid.clone(), can_write }
                }
            }
            if picker_open() {
                PhotoPicker {
                    present,
                    on_close: move |_| picker_open.set(false),
                    on_add: {
                        let pid = pid.clone();
                        move |ids: Vec<String>| {
                            picker_open.set(false);
                            let pid = pid.clone();
                            let done = t!("lab-images-added").to_string();
                            spawn(async move {
                                match api::lab::add_images(&pid, &ids).await {
                                    Ok(_) => {
                                        toasts::success(done);
                                        reload.with_mut(|r| *r += 1);
                                    }
                                    Err(e) => toasts::error(e),
                                }
                            });
                        }
                    },
                }
            }
            if recording_open() {
                RecordingImport {
                    project: pid.clone(),
                    on_close: move |_| recording_open.set(false),
                    on_done: move |_| {
                        recording_open.set(false);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
            if editing() {
                ProjectForm {
                    initial: project_now.clone(),
                    on_close: move |_| editing.set(false),
                    on_saved: move |_| {
                        editing.set(false);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
        }
    }
}

/// Left column: the images, with their annotation state.
#[component]
fn ImageStrip(
    images: Vec<LabImage>,
    selected: usize,
    loaded: bool,
    on_select: EventHandler<usize>,
) -> Element {
    rsx! {
        div { class: "rounded-lg border border-gray-200 bg-white max-h-[70vh] overflow-y-auto",
            if images.is_empty() && loaded {
                p { class: "p-3 text-xs text-gray-500", {t!("lab-strip-empty")} }
            }
            for (i, img) in images.iter().enumerate() {
                ImageRow {
                    key: "{img.id}",
                    image: img.clone(),
                    active: i == selected,
                    on_click: move |_| on_select.call(i),
                }
            }
        }
    }
}

#[component]
fn ImageRow(image: LabImage, active: bool, on_click: EventHandler<()>) -> Element {
    let (dot, state) = if image.negative {
        ("bg-gray-400", t!("lab-state-negative").to_string())
    } else if image.boxes.is_empty() {
        ("bg-amber-400", t!("lab-state-todo").to_string())
    } else {
        (
            "bg-emerald-500",
            t!("lab-state-boxes", count : image.boxes.len()).to_string(),
        )
    };
    let row = if active {
        "w-full text-left px-3 py-2 flex items-center gap-2 bg-signal-50 border-l-2 border-signal-600"
    } else {
        "w-full text-left px-3 py-2 flex items-center gap-2 hover:bg-gray-50 border-l-2 border-transparent"
    };
    rsx! {
        button {
            class: row,
            r#type: "button",
            onclick: move |_| on_click.call(()),
            span { class: "h-2 w-2 rounded-full shrink-0 {dot}" }
            span { class: "flex-1 min-w-0",
                span { class: "block text-sm text-gray-900 truncate", "{image.name}" }
                span { class: "block text-xs text-gray-500", "{state}" }
            }
        }
    }
}

/// Class chips: the class of the next box (keys 1–9).
#[component]
fn ClassBar(classes: Vec<String>, current: u32, on_pick: EventHandler<u32>) -> Element {
    rsx! {
        div { class: "flex flex-wrap items-center gap-2",
            span { class: "text-xs text-gray-500", {t!("lab-class-label")} }
            for (i, name) in classes.iter().enumerate() {
                ClassChip {
                    key: "{i}",
                    index: i as u32,
                    name: name.clone(),
                    on: i as u32 == current,
                    on_pick,
                }
            }
        }
    }
}

#[component]
fn ClassChip(index: u32, name: String, on: bool, on_pick: EventHandler<u32>) -> Element {
    let color = class_color(index);
    let style = if on {
        format!("background: {color}; border-color: {color}; color: white;")
    } else {
        format!("border-color: {color};")
    };
    let shortcut = if index < 9 {
        format!("{} ", index + 1)
    } else {
        String::new()
    };
    rsx! {
        button {
            class: "px-2 py-0.5 rounded-full border-2 text-xs font-medium text-gray-800",
            style: "{style}",
            r#type: "button",
            onclick: move |_| on_pick.call(index),
            span { class: "opacity-70", "{shortcut}" }
            "{name}"
        }
    }
}

/// The picture with its boxes: drag on the picture draws a box of the
/// current class, a click on a box selects it.
#[component]
fn Annotator(
    image: LabImage,
    classes: Vec<String>,
    class: u32,
    can_write: bool,
    selected_box: Option<usize>,
    suggestions: Vec<LabSuggestion>,
    on_accept: EventHandler<usize>,
    on_select_box: EventHandler<Option<usize>>,
    on_add_box: EventHandler<[f32; 4]>,
) -> Element {
    let asset = image.asset_id.clone();
    let url = use_resource(move || {
        let asset = asset.clone();
        async move { crate::util::media_blob_url(&api::media::content_path(&asset), None).await }
    });
    let mut layer = use_signal(|| None::<Rc<MountedData>>);
    // Layer rect in client pixels (left, top, width, height), read on press.
    let mut rect = use_signal(|| None::<(f64, f64, f64, f64)>);
    let mut start = use_signal(|| None::<(f32, f32)>);
    let mut now = use_signal(|| None::<(f32, f32)>);

    let ratio = image.width.max(1) as f64 / image.height.max(1) as f64;
    let frame = format!(
        "aspect-ratio: {} / {}; width: min(100%, calc(70vh * {ratio}));",
        image.width.max(1),
        image.height.max(1)
    );
    let norm = move |x: f64, y: f64| -> Option<(f32, f32)> {
        let (l, t, w, h) = (*rect.peek())?;
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        Some((
            ((x - l) / w).clamp(0.0, 1.0) as f32,
            ((y - t) / h).clamp(0.0, 1.0) as f32,
        ))
    };
    let on_down = move |e: PointerEvent| async move {
        if !can_write {
            return;
        }
        let Some(el) = layer.cloned() else {
            return;
        };
        let Ok(r) = el.get_client_rect().await else {
            return;
        };
        rect.set(Some((r.origin.x, r.origin.y, r.size.width, r.size.height)));
        let c = e.client_coordinates();
        if let Some(p) = norm(c.x, c.y) {
            start.set(Some(p));
            now.set(Some(p));
            on_select_box.call(None);
        }
    };
    let on_move = move |e: PointerEvent| {
        if start.peek().is_none() {
            return;
        }
        let c = e.client_coordinates();
        if let Some(p) = norm(c.x, c.y) {
            now.set(Some(p));
        }
    };
    let mut finish = move || {
        let (Some(a), Some(b)) = (*start.peek(), *now.peek()) else {
            start.set(None);
            return;
        };
        start.set(None);
        now.set(None);
        let bbox = rect_of(a, b);
        if bbox[2] >= MIN_BOX && bbox[3] >= MIN_BOX {
            on_add_box.call(bbox);
        }
    };
    let drawing = match (start(), now()) {
        (Some(a), Some(b)) => Some(rect_of(a, b)),
        _ => None,
    };
    let draft_style = drawing.map(|b| box_style(&b, class_color(class)));
    let src = url.read().clone().flatten();
    let cursor = if can_write {
        "cursor-crosshair"
    } else {
        "cursor-default"
    };

    rsx! {
        div {
            class: "relative mx-auto bg-gray-50 border border-gray-200 rounded-lg overflow-hidden select-none touch-none",
            style: "{frame}",
            if let Some(src) = src {
                img {
                    class: "absolute inset-0 w-full h-full pointer-events-none",
                    src: "{src}",
                    alt: "{image.name}",
                    draggable: "false",
                }
            } else {
                span { class: "absolute inset-0 m-auto h-8 w-8 animate-spin rounded-full border-b-2 border-gray-400" }
            }
            div {
                class: "absolute inset-0 {cursor}",
                onmounted: move |e| layer.set(Some(e.data())),
                onpointerdown: on_down,
                onpointermove: on_move,
                onpointerup: move |_| finish(),
                onpointerleave: move |_| finish(),
                for (i, b) in image.boxes.iter().enumerate() {
                    BoxView {
                        key: "{i}",
                        index: i,
                        bbox: b.bbox,
                        color: class_color(b.class).to_string(),
                        label: classes.get(b.class as usize).cloned().unwrap_or_default(),
                        selected: selected_box == Some(i),
                        on_select: move |i: usize| on_select_box.call(Some(i)),
                    }
                }
                for (i, sug) in suggestions.iter().enumerate() {
                    SuggestionView {
                        key: "s{i}",
                        index: i,
                        suggestion: sug.clone(),
                        class_name: sug.class.and_then(|c| classes.get(c as usize).cloned()),
                        on_accept,
                    }
                }
                if let Some(style) = draft_style {
                    div {
                        class: "absolute border-2 border-dashed pointer-events-none",
                        style: "{style}",
                    }
                }
            }
        }
    }
}

#[component]
fn BoxView(
    index: usize,
    bbox: [f32; 4],
    color: String,
    label: String,
    selected: bool,
    on_select: EventHandler<usize>,
) -> Element {
    let style = box_style(&bbox, &color);
    let class = if selected {
        "absolute border-2 ring-2 ring-white shadow-lg"
    } else {
        "absolute border-2"
    };
    rsx! {
        div {
            class,
            style: "{style}",
            onpointerdown: move |e: PointerEvent| {
                e.stop_propagation();
                on_select.call(index);
            },
            span {
                class: "absolute left-0 -top-5 px-1 rounded text-xs font-medium text-white whitespace-nowrap",
                style: "background: {color};",
                "{label}"
            }
        }
    }
}

/// A proposal of the model: dashed, click to keep it as a box.
#[component]
fn SuggestionView(
    index: usize,
    suggestion: LabSuggestion,
    class_name: Option<String>,
    on_accept: EventHandler<usize>,
) -> Element {
    let color = suggestion.class.map(class_color).unwrap_or("#6b7280");
    let style = box_style(&suggestion.bbox, color);
    let percent = (suggestion.score * 100.0).round();
    let label = match class_name {
        Some(c) => format!("{c} ← {} {percent}%", suggestion.label),
        None => format!("{} {percent}%", suggestion.label),
    };
    let title = t!("lab-suggest-keep-one").to_string();
    rsx! {
        div {
            class: "absolute border-2 border-dashed cursor-pointer hover:bg-white/20",
            style: "{style}",
            title: "{title}",
            onpointerdown: move |e: PointerEvent| {
                e.stop_propagation();
                on_accept.call(index);
            },
            span {
                class: "absolute left-0 -bottom-5 px-1 rounded text-xs text-white whitespace-nowrap opacity-90",
                style: "background: {color};",
                "{label}"
            }
        }
    }
}

/// Pre-annotation: a model of the registry proposes boxes (§4.6.1 step 4).
#[component]
fn SuggestBar(
    models: Vec<(String, String)>,
    model: String,
    busy: bool,
    count: usize,
    total: usize,
    on_model: EventHandler<String>,
    on_suggest: EventHandler<()>,
    on_keep_all: EventHandler<()>,
    on_dismiss: EventHandler<()>,
) -> Element {
    // `count` = proposals whose label matches a class.
    let keep_all = t!("lab-suggest-keep-all", count : count).to_string();
    rsx! {
        div { class: "flex flex-wrap items-center gap-2 rounded-lg border border-gray-200 bg-white p-2",
            span { class: "text-xs text-gray-600", {t!("lab-suggest-label")} }
            select {
                class: "px-2 py-1 border border-gray-300 rounded-lg text-sm max-w-56",
                aria_label: t!("lab-suggest-model").to_string(),
                onchange: move |e| on_model.call(e.value()),
                option { value: "", selected: model.is_empty(), {t!("lab-suggest-model")} }
                for (id, name) in models {
                    option { key: "{id}", value: "{id}", selected: id == model, "{name}" }
                }
            }
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                disabled: model.is_empty() || busy,
                onclick: move |_| on_suggest.call(()),
                {t!("lab-suggest")}
            }
            if count > 0 {
                button {
                    class: BTN_PRIMARY_SM,
                    title: t!("lab-suggest-keep-all-help").to_string(),
                    r#type: "button",
                    onclick: move |_| on_keep_all.call(()),
                    "{keep_all}"
                }
            }
            if total > 0 {
                button {
                    class: "text-xs text-gray-600 hover:underline",
                    r#type: "button",
                    onclick: move |_| on_dismiss.call(()),
                    {t!("lab-suggest-dismiss")}
                }
            }
        }
    }
}

/// Under the picture: navigation, box removal, negative example, split.
#[component]
fn ImageTools(
    image: LabImage,
    index: usize,
    count: usize,
    can_write: bool,
    selected_box: Option<usize>,
    on_prev: EventHandler<()>,
    on_next: EventHandler<()>,
    on_remove_box: EventHandler<()>,
    on_negative: EventHandler<bool>,
    on_split: EventHandler<LabSplit>,
    on_remove_image: EventHandler<()>,
) -> Element {
    let position = format!("{} / {count}", index + 1);
    let effective = match image.effective_split() {
        LabSplit::Val => t!("lab-split-val").to_string(),
        _ => t!("lab-split-train").to_string(),
    };
    let split_value = match image.split {
        LabSplit::Auto => "auto",
        LabSplit::Train => "train",
        LabSplit::Val => "val",
    };
    rsx! {
        div { class: "flex flex-wrap items-center gap-3",
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                disabled: index == 0,
                onclick: move |_| on_prev.call(()),
                "←"
            }
            span { class: "text-sm text-gray-600 tabular-nums", "{position}" }
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                disabled: index + 1 >= count,
                onclick: move |_| on_next.call(()),
                "→"
            }
            if can_write {
                button {
                    class: BTN_SECONDARY_SM,
                    r#type: "button",
                    disabled: selected_box.is_none(),
                    onclick: move |_| on_remove_box.call(()),
                    {t!("lab-remove-box")}
                }
                label { class: "flex items-center gap-2 text-sm text-gray-700",
                    input {
                        class: "h-4 w-4 accent-signal-600",
                        r#type: "checkbox",
                        checked: image.negative,
                        onchange: move |e| on_negative.call(e.checked()),
                    }
                    {t!("lab-negative")}
                }
                button {
                    class: "text-xs text-red-600 hover:underline",
                    r#type: "button",
                    onclick: move |_| on_remove_image.call(()),
                    {t!("lab-remove-image")}
                }
                label { class: "flex items-center gap-2 text-sm text-gray-700",
                    span { {t!("lab-split")} }
                    select {
                        class: "px-2 py-1 border border-gray-300 rounded-lg text-sm",
                        onchange: move |e| on_split.call(split_of(&e.value())),
                        option { value: "auto", selected: split_value == "auto",
                            {t!("lab-split-auto", side : effective.clone())}
                        }
                        option { value: "train", selected: split_value == "train",
                            {t!("lab-split-train")}
                        }
                        option { value: "val", selected: split_value == "val", {t!("lab-split-val")} }
                    }
                }
            }
        }
        p { class: "text-xs text-gray-500", {t!("lab-keys-help")} }
    }
}

fn split_of(v: &str) -> LabSplit {
    match v {
        "train" => LabSplit::Train,
        "val" => LabSplit::Val,
        _ => LabSplit::Auto,
    }
}

/// Live counts and the indicative warnings (§4.6.2).
#[component]
fn StatsPanel(classes: Vec<String>, stats: lab::LabStats) -> Element {
    let summary = t!(
        "lab-stats-summary", annotated : stats.annotated, total : stats.images, negatives : stats
        .negatives
    )
    .to_string();
    let split = t!("lab-stats-split", train : stats.train, val : stats.val).to_string();
    let rows: Vec<(u32, String, usize, usize)> = classes
        .iter()
        .enumerate()
        .map(|(i, c)| {
            (
                i as u32,
                c.clone(),
                stats.images_per_class.get(i).copied().unwrap_or(0),
                stats.boxes_per_class.get(i).copied().unwrap_or(0),
            )
        })
        .collect();
    let warnings: Vec<String> = stats.warnings.iter().map(warning_text).collect();
    rsx! {
        section { class: "rounded-lg border border-gray-200 bg-white p-3 space-y-2",
            h2 { class: "text-sm font-semibold text-gray-900", {t!("lab-stats-title")} }
            p { class: "text-xs text-gray-600", "{summary}" }
            p { class: "text-xs text-gray-600", "{split}" }
            ul { class: "space-y-1",
                for (i, name, imgs, boxes) in rows {
                    li { key: "{i}", class: "flex items-center gap-2 text-xs",
                        span {
                            class: "h-2.5 w-2.5 rounded-sm shrink-0",
                            style: "background: {class_color(i)};",
                        }
                        span { class: "flex-1 truncate text-gray-800", "{name}" }
                        span { class: "tabular-nums text-gray-600",
                            {t!("lab-stats-class", images : imgs, boxes : boxes)}
                        }
                    }
                }
            }
            for (i, w) in warnings.into_iter().enumerate() {
                p { key: "{i}", class: "text-xs text-amber-700", "{w}" }
            }
        }
    }
}

fn warning_text(w: &lab::LabWarning) -> String {
    let class = w.class.clone().unwrap_or_default();
    match w.code.as_str() {
        "lab-warn-class-few" => {
            t!("lab-warn-class-few", class : class, min : lab::LAB_MIN_PER_CLASS).to_string()
        }
        "lab-warn-val-few" => t!("lab-warn-val-few", min : lab::LAB_MIN_VAL_IMAGES).to_string(),
        "lab-warn-no-negative" => t!("lab-warn-no-negative").to_string(),
        "lab-warn-no-class" => t!("lab-warn-no-class").to_string(),
        other => other.to_string(),
    }
}

/// Frozen dataset versions: freeze the current annotations, download one.
#[component]
fn VersionsPanel(project: String, can_write: bool) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut note = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let pid = project.clone();
    let versions = use_resource(move || {
        let _ = reload();
        let pid = pid.clone();
        async move { api::lab::versions(&pid).await }
    });
    let list = versions
        .read()
        .clone()
        .and_then(Result::ok)
        .unwrap_or_default();
    let on_freeze = move |_| {
        let pid = project.clone();
        let n = note.peek().clone();
        let done = t!("lab-frozen").to_string();
        busy.set(true);
        spawn(async move {
            match api::lab::freeze(&pid, &n).await {
                Ok(_) => {
                    toasts::success(done);
                    note.set(String::new());
                    reload.with_mut(|r| *r += 1);
                }
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    rsx! {
        section { class: "rounded-lg border border-gray-200 bg-white p-3 space-y-2",
            h2 { class: "text-sm font-semibold text-gray-900", {t!("lab-versions-title")} }
            p { class: "text-xs text-gray-500", {t!("lab-versions-help")} }
            if can_write {
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("lab-version-note")} }
                    input {
                        class: "{INPUT} w-full",
                        maxlength: "500",
                        value: "{note}",
                        oninput: move |e| note.set(e.value()),
                    }
                }
                button {
                    class: BTN_PRIMARY_SM,
                    r#type: "button",
                    disabled: busy(),
                    onclick: on_freeze,
                    {t!("lab-freeze")}
                }
            }
            ul { class: "divide-y divide-gray-100",
                for v in list {
                    VersionRow { key: "{v.id}", version: v }
                }
            }
        }
    }
}

#[component]
fn VersionRow(version: lab::LabDatasetVersion) -> Element {
    let mut busy = use_signal(|| false);
    let title = t!("lab-version-label", n : version.version).to_string();
    let detail = t!(
        "lab-version-detail", images : version.stats.annotated, val : version.stats.val
    )
    .to_string();
    let date = version.created_at.get(..10).unwrap_or_default().to_string();
    let id = version.id.clone();
    let on_export = move |_| {
        let id = id.clone();
        let file = format!("lab-dataset-v{}.zip", version.version);
        busy.set(true);
        spawn(async move {
            match api::lab::export(&id).await {
                Ok(bytes) => crate::util::save_blob(&file, &bytes),
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    rsx! {
        li { class: "py-2 flex items-center gap-2",
            div { class: "flex-1 min-w-0",
                div { class: "text-sm font-medium text-gray-900", "{title}" }
                div { class: "text-xs text-gray-500", "{date} · {detail}" }
                if !version.note.is_empty() {
                    div { class: "text-xs text-gray-600 truncate", "{version.note}" }
                }
            }
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                disabled: busy(),
                onclick: on_export,
                {t!("lab-export")}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_to_box() {
        let r = rect_of((0.5, 0.6), (0.1, 0.2));
        assert_eq!((r[0], r[1]), (0.1, 0.2));
        assert!((r[2] - 0.4).abs() < 1e-6 && (r[3] - 0.4).abs() < 1e-6);
        assert_eq!(split_of("val"), LabSplit::Val);
        assert_eq!(class_color(9), class_color(1));
    }
}
