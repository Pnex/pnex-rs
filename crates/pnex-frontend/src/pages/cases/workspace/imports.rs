//! Imported tracks of a member (cases-workspace.md K26): pick a GPX or CSV
//! file, read the preview (points, span, rejected rows), give the time zone
//! when the file's times carry none, then import. The imports of the member
//! are listed with their file fingerprint and can be removed.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{TrackImport, TrackImportInput, TrackImportPreview};

use super::{model, Ws};
use crate::api;
use crate::components::confirm::ConfirmDialog;
use crate::components::ui::{BTN_GHOST, BTN_PRIMARY_SM, BTN_SECONDARY_SM, INPUT};
use crate::state::toasts;

/// IANA time zone of the browser, `UTC` off a browser.
fn browser_zone() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let opts = js_sys::Intl::DateTimeFormat::new(&js_sys::Array::new(), &js_sys::Object::new())
            .resolved_options();
        js_sys::Reflect::get(&opts, &"timeZone".into())
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| "UTC".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "UTC".into()
    }
}

/// Format of a file from its name, `None` when not supported.
fn format_of(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    if lower.ends_with(".gpx") {
        Some("gpx")
    } else if lower.ends_with(".csv") || lower.ends_with(".txt") {
        Some("csv")
    } else {
        None
    }
}

#[derive(Clone, PartialEq)]
struct Pending {
    input: TrackImportInput,
    preview: TrackImportPreview,
}

#[component]
pub(super) fn TrackImports(object_id: String) -> Element {
    let ws = use_context::<Ws>();
    let mut version = use_signal(|| 0u32);
    let mut pending = use_signal(|| None::<Pending>);
    let mut zone = use_signal(browser_zone);
    let mut removing = use_signal(|| None::<TrackImport>);
    let oid = object_id.clone();
    let list = use_resource(move || {
        let (id, oid) = (ws.case_id.peek().clone(), oid.clone());
        let _ = version();
        async move {
            api::cases::track_imports(&id)
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|i| i.object.id == oid)
                .collect::<Vec<_>>()
        }
    });
    let imports = list.read().clone().unwrap_or_default();
    let local = (ws.local_time)();
    let stamp = |s: &Option<String>| {
        s.as_deref()
            .and_then(model::ms)
            .map(|t| model::stamp(t, local))
            .unwrap_or_default()
    };
    // Preview again (a dry run) with the time zone, or import for real.
    let send = move |dry: bool| {
        let Some(p) = pending() else { return };
        let mut input = p.input.clone();
        input.dry_run = dry;
        input.time_zone = p.preview.needs_time_zone.then(|| zone());
        let id = ws.case_id.peek().clone();
        spawn(async move {
            match api::cases::import_track(&id, &input).await {
                Ok(preview) if dry => pending.set(Some(Pending { input, preview })),
                Ok(done) => {
                    pending.set(None);
                    toasts::success(t!("cw-import-done").to_string());
                    version += 1;
                    // The timeline frames what was just imported, the
                    // detectors look at it.
                    let span = done
                        .t_start
                        .as_deref()
                        .and_then(model::ms)
                        .zip(done.t_end.as_deref().and_then(model::ms));
                    if let Some((a, b)) = span {
                        let pad = ((b - a) / 10).max(60_000);
                        let (mut window, mut cursor) = (ws.window, ws.cursor);
                        window.set((a - pad, b + pad));
                        cursor.set(a);
                    }
                    ws.reload();
                    ws.detect();
                }
                Err(e) => toasts::error(e),
            }
        });
    };
    let oid = object_id.clone();
    let pick = move |evt: FormEvent| {
        let oid = oid.clone();
        async move {
            let Some(file) = evt.files().first().cloned() else {
                return;
            };
            let name = file.name();
            let Some(format) = format_of(&name) else {
                toasts::error(t!("err-track-import-unreadable").to_string());
                return;
            };
            let content = file.read_string().await.unwrap_or_default();
            let input = TrackImportInput {
                object_id: oid,
                file_name: name,
                format: format.into(),
                content,
                time_zone: None,
                dry_run: true,
            };
            let id = ws.case_id.peek().clone();
            match api::cases::import_track(&id, &input).await {
                Ok(preview) => pending.set(Some(Pending { input, preview })),
                Err(e) => toasts::error(e),
            }
        }
    };
    let remove = move |_| {
        let Some(i) = removing() else { return };
        removing.set(None);
        let id = ws.case_id.peek().clone();
        spawn(async move {
            match api::cases::delete_track_import(&id, &i.id).await {
                Ok(()) => {
                    version += 1;
                    ws.reload();
                }
                Err(e) => toasts::error(e),
            }
        });
    };
    rsx! {
        h3 { class: "text-xs font-medium text-slate", {t!("cw-imports")} }
        for i in imports {
            div {
                key: "{i.id}",
                class: "flex items-start gap-2 border-b border-line py-1.5 text-xs",
                "data-testid": "case-import",
                div { class: "min-w-0 flex-1",
                    div { class: "truncate text-ink", "{i.file_name}" }
                    div { class: "text-slate",
                        {t!("cw-import-summary", points : i.points, rejected : i.rejected)}
                    }
                    div { class: "font-mono text-slate",
                        "{stamp(&Some(i.t_start.clone()))} → {stamp(&Some(i.t_end.clone()))}"
                    }
                    div {
                        class: "truncate font-mono text-slate",
                        title: t!("cw-import-sha"),
                        "sha256 {i.sha256}"
                    }
                }
                if ws.can_write {
                    button {
                        class: "{BTN_GHOST} text-alarm",
                        r#type: "button",
                        "aria-label": t!("cw-import-remove"),
                        onclick: {
                            let i = i.clone();
                            move |_| removing.set(Some(i.clone()))
                        },
                        "✕"
                    }
                }
            }
        }
        if ws.can_write {
            if let Some(p) = pending() {
                div {
                    class: "flex flex-col gap-2 rounded-md border border-line bg-gray-50 p-2 text-xs",
                    "data-testid": "case-import-preview",
                    div { class: "font-medium text-ink", "{p.input.file_name}" }
                    if p.preview.needs_time_zone {
                        label { class: "text-slate", {t!("cw-import-zone")} }
                        div { class: "flex gap-2",
                            input {
                                class: INPUT,
                                value: "{zone}",
                                "data-testid": "case-import-zone",
                                oninput: move |e| zone.set(e.value()),
                            }
                            button {
                                class: BTN_SECONDARY_SM,
                                r#type: "button",
                                onclick: move |_| send(true),
                                {t!("cw-import-preview")}
                            }
                        }
                    }
                    if p.preview.points > 0 {
                        p { class: "text-ink",
                            {
                                t!(
                                    "cw-import-summary", points : p.preview.points, rejected : p.preview
                                    .rejected_count
                                )
                            }
                        }
                        p { class: "font-mono text-slate",
                            "{stamp(&p.preview.t_start)} → {stamp(&p.preview.t_end)}"
                        }
                    }
                    for r in p.preview.rejected.iter().take(5) {
                        p { class: "text-slate",
                            {t!("cw-import-rejected-row", row : r.index, reason : reject_label(r.reason))}
                        }
                    }
                    div { class: "flex justify-end gap-2",
                        button {
                            class: BTN_GHOST,
                            r#type: "button",
                            onclick: move |_| pending.set(None),
                            {t!("common-cancel")}
                        }
                        button {
                            class: BTN_PRIMARY_SM,
                            r#type: "button",
                            disabled: p.preview.points == 0,
                            "data-testid": "case-import-confirm",
                            onclick: move |_| send(false),
                            {t!("cw-import-confirm", points : p.preview.points)}
                        }
                    }
                }
            } else {
                label { class: "{BTN_SECONDARY_SM} cursor-pointer self-start",
                    {t!("cw-import")}
                    input {
                        class: "hidden",
                        r#type: "file",
                        accept: ".gpx,.csv,.txt,application/gpx+xml,text/csv",
                        "data-testid": "case-import-file",
                        onchange: pick,
                    }
                }
            }
        }
        if let Some(i) = removing() {
            ConfirmDialog {
                title: t!("cw-import-remove").to_string(),
                message: t!("cw-import-remove-confirm", file : i.file_name.clone()).to_string(),
                confirm_label: t!("cw-import-remove").to_string(),
                on_confirm: remove,
                on_cancel: move |_| removing.set(None),
            }
        }
    }
}

fn reject_label(reason: pnex_core::track::RejectReason) -> String {
    use pnex_core::track::RejectReason as R;
    match reason {
        R::NotFinite | R::Unreadable => t!("cw-reject-unreadable"),
        R::OutOfRange => t!("cw-reject-out-of-range"),
        R::DuplicateTime => t!("cw-reject-duplicate"),
    }
    .to_string()
}
