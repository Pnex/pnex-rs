use super::*;

/// Asset detail — full-page two columns (2026-09-19 redesign): preview +
/// metadata strip on the left, info/labels/versions cards on the right,
/// breadcrumb header. "View" previews a non-current version (blob
/// `/versions/{n}/content`, read-only).
#[component]
pub(super) fn MediaDetail(
    asset_id: String,
    can_write: bool,
    on_close: Callback<()>,
    on_upload: Callback<()>,
    on_changed: Callback<()>,
) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut detail = use_signal(|| None::<MediaAsset>);
    let mut error = use_signal(String::new);
    // Version being previewed ("View" on a non-current version) —
    // None = current version.
    let mut viewing_version = use_signal(|| None::<i64>);
    let mut delete_open = use_signal(|| false);
    // Current-version download in flight (button spinner).
    let mut downloading = use_signal(|| false);
    // New-version upload in flight (card button spinner).
    let mut uploading = use_signal(|| false);
    // Version delete target (number) — detail ConfirmDialog.
    let mut delete_target = use_signal(|| None::<i64>);
    // Server HD job running for this asset (Take 360 V2) — drives the
    // "patience" overlay on the preview.
    let mut hd_running = use_signal(|| false);
    let asset_id_for_resource = asset_id.clone();
    let asset_id_for_poll = asset_id.clone();

    // Loads the detail (and on every current-version change).
    use_resource(move || {
        let id = asset_id_for_resource.clone();
        async move {
            let _ = reload();
            match api::media::detail(&id).await {
                Ok(asset) => detail.set(Some(asset)),
                Err(err) => error.set(err.message),
            }
        }
    });

    // Versions always loaded (visible card in the right column — read for
    // every role, writes gated by can_write).
    let asset_id_for_versions = asset_id.clone();
    let versions = use_resource(move || {
        let id = asset_id_for_versions.clone();
        async move {
            let _ = reload();
            api::media::versions(&id).await
        }
    });

    // Take 360 V2: the displayed panorama is the DEVICE PREVIEW (v1, fast
    // render) while the server HD stitch job (v2, several minutes) runs.
    // Server polling of `GET /stitch-jobs/by-asset/{id}` — database source
    // of truth, survives an app restart (unlike the in-memory poller of the
    // capture pipeline). While `queued|running`: "patience" overlay on the
    // preview; on `succeeded`: toast + reload (the preview jumps to the new
    // current version, cf. the MediaPreview key); on `failed`: error toast.
    //
    // NO reactive signal read here (media.rs school: locals + atomics
    // only) — otherwise the success reload would restart the loop and
    // replay the toast forever. "No job" tolerated GRACE_TRIES times
    // (~40 s): frames are sent to the server AFTER the capture toast —
    // when opening the detail the job may not exist yet. Bounded to
    // ~30 min; the `spawn` dies when the component unmounts (detail
    // closed).
    use_effect(move || {
        let asset_id = asset_id_for_poll.clone();
        spawn(async move {
            const POLL_SECS: u64 = 4;
            const GRACE_TRIES: usize = 10; // ~40 s on "no job"
            const MAX_TRIES: usize = 450; // ~30 min of job coverage
            let mut grace = 0usize;
            let mut saw_running = false;
            for _ in 0..MAX_TRIES {
                match api::stitch_jobs::latest_for_asset(&asset_id).await {
                    Ok(Some(job)) => match job.state.as_str() {
                        "queued" | "running" => {
                            saw_running = true;
                            hd_running.set(true);
                        }
                        "succeeded" => {
                            hd_running.set(false);
                            // Toast only if the job was seen running in THIS
                            // open session — an old asset (HD already
                            // published) does not re-toast.
                            if saw_running {
                                toasts::success(t!("media-stitch-hd-done"));
                                reload.with_mut(|r| *r += 1);
                            }
                            return;
                        }
                        "failed" => {
                            hd_running.set(false);
                            if saw_running {
                                let msg = match job.error {
                                    Some(e) if !e.is_empty() => {
                                        format!("{} : {e}", t!("media-stitch-hd-failed"))
                                    }
                                    _ => t!("media-stitch-hd-failed").to_string(),
                                };
                                toasts::error(msg);
                            }
                            return;
                        }
                        _ => return,
                    },
                    Ok(None) => {
                        grace += 1;
                        if grace >= GRACE_TRIES {
                            return; // never stitched server-side
                        }
                    }
                    Err(_) => {} // network/server: next probe at POLL_SECS
                }
                sleep(std::time::Duration::from_secs(POLL_SECS)).await;
            }
        });
    });

    // Synchronous resource reads (CRUD socle doctrine).
    let (versions_rows, versions_error): (Vec<api::media::MediaVersion>, Option<String>) =
        match &*versions.value().read() {
            Some(Ok(rows)) => (rows.clone(), None),
            Some(Err(err)) => (Vec::new(), Some(err.message.clone())),
            None => (Vec::new(), None),
        };
    let crumb_name = detail()
        .as_ref()
        .map(|a| a.name.clone())
        .unwrap_or_else(|| "…".to_string());
    // Dedicated clones per closure (a String can only move into ONE
    // closure — school of owned props).
    let asset_id_for_add = asset_id.clone();
    let asset_id_for_version_del = asset_id.clone();

    rsx! {
        div { class: "p-6",
            // Page header — back button + breadcrumb + upload action.
            div { class: "mb-6 flex flex-wrap items-center justify-between gap-3",
                div { class: "flex min-w-0 items-center gap-2",
                    button {
                        class: "inline-flex items-center gap-2 px-4 py-2.5 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        icons::ArrowLeft { class: "h-5 w-5" }
                    }
                    button {
                        class: "text-sm font-medium text-blue-600 hover:text-blue-800 whitespace-nowrap",
                        onclick: move |_| on_close.call(()),
                        {t!("media-title")}
                    }
                    span { class: "text-gray-300", "›" }
                    span { class: "min-w-0 truncate text-sm font-medium text-gray-900",
                        "{crumb_name}"
                    }
                }
                if can_write {
                    button {
                        class: PRIMARY_BTN,
                        r#type: "button",
                        onclick: move |_| on_upload.call(()),
                        icons::Plus { class: "h-4 w-4 inline mr-1" }
                        {t!("media-upload")}
                    }
                }
            }
            if !error().is_empty() {
                div { class: "bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700",
                    "{error}"
                }
            }
            match detail() {
                Some(asset) => rsx! {
                    div { class: "grid grid-cols-1 items-start gap-6 lg:grid-cols-[minmax(0,1fr)_360px]",
                        // ─── Left column: preview + metadata strip ───
                        div { class: "min-w-0 space-y-3",
                            // Key by CURRENT version + previewed version: any
                            // version switch remounts the preview — the blob
                            // and the 360 viewer start from zero (the blob
                            // use_effect is not reactive by itself).
                            MediaPreview {
                                key: "pv-v{asset.current_version_number.clone().unwrap_or(0)}-{viewing_version().unwrap_or(0)}",
                                asset: asset.clone(),
                                version_override: viewing_version(),
                                show_hd_overlay: hd_running() && viewing_version().is_none(),
                            }
                            // Previewing a non-current version — explicit
                            // banner + way back (the kind/current pills live
                            // on the preview itself).
                            if let Some(n) = viewing_version() {
                                div { class: "flex items-center justify-between gap-2 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-800",
                                    span { {t!("media-version-viewing", n : n)} }
                                    button {
                                        class: "font-medium underline hover:no-underline",
                                        onclick: move |_| viewing_version.set(None),
                                        {t!("media-version-back-to-current")}
                                    }
                                }
                            }
                            // Metadata strip (size/format/dates — the API
                            // contract carries no pixel dimensions).
                            div { class: "flex flex-wrap gap-x-6 gap-y-1 text-sm",
                                div { class: "flex gap-1.5",
                                    span { class: "text-gray-500", {t!("media-col-size")} }
                                    span { class: "font-medium text-gray-900", {format_size(asset.size_bytes)} }
                                }
                                div { class: "flex gap-1.5",
                                    span { class: "text-gray-500", {t!("media-format")} }
                                    span { class: "font-medium text-gray-900", {format_label(&asset)} }
                                }
                                div { class: "flex gap-1.5",
                                    span { class: "text-gray-500", {t!("media-info-added")} }
                                    span { class: "font-medium text-gray-900", {date_label(&asset.created_at)} }
                                }
                                div { class: "flex gap-1.5",
                                    span { class: "text-gray-500", {t!("media-col-updated")} }
                                    span { class: "font-medium text-gray-900", {date_label(&asset.updated_at)} }
                                }
                            }
                        }
                        // ─── Right column: info / labels / versions / actions ───
                        div { class: "space-y-4",
                            // Info card (no version pill here — the preview
                            // badge already carries v{n} · current).
                            div { class: "rounded-xl border border-gray-200 bg-white p-4",
                                div { class: "flex flex-wrap items-center gap-2",
                                    span {
                                        class: format!(
                                            "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium {}",
                                            kind_pill_classes(&asset),
                                        ),
                                        match asset.media_kind() {
                                            MediaKind::Panorama => rsx! {
                                                icons::Image { class: "h-3 w-3" }
                                            },
                                            MediaKind::Splat => rsx! {
                                                icons::Cube { class: "h-3 w-3" }
                                            },
                                            MediaKind::Floorplan => rsx! {
                                                icons::Map { class: "h-3 w-3" }
                                            },
                                            MediaKind::Photo => rsx! {
                                                icons::Image { class: "h-3 w-3" }
                                            },
                                            MediaKind::Model => rsx! {
                                                icons::Eye { class: "h-3 w-3" }
                                            },
                                        }
                                        {kind_label(&asset)}
                                    }
                                }
                                h2 { class: "mt-2 break-all text-lg font-semibold text-gray-900", {asset.name.clone()} }
                                div { class: "mt-3 space-y-1 text-sm",
                                    div { class: "flex justify-between gap-3",
                                        span { class: "text-gray-500", {t!("media-info-added")} }
                                        span { class: "text-gray-900 text-right", {date_label(&asset.created_at)} }
                                    }
                                    div { class: "flex justify-between gap-3",
                                        span { class: "text-gray-500", {t!("media-col-updated")} }
                                        span { class: "text-gray-900 text-right", {date_label(&asset.updated_at)} }
                                    }
                                }
                            }
                            // Where the media sits in the sites tree.
                            div { class: "rounded-xl border border-gray-200 bg-white p-4",
                                crate::components::location_breadcrumb::LocationBreadcrumb {
                                    kind: pnex_core::resources::KIND_MEDIA_ASSET.to_string(),
                                    id: asset_id.clone(),
                                }
                            }
                            // Labels card (D42 transverse editor).
                            div { class: "rounded-xl border border-gray-200 bg-white p-4",
                                LabelsEditor {
                                    kind: "media_asset".to_string(),
                                    id: asset_id.clone(),
                                    can_write,
                                    on_changed,
                                }
                                div { class: "mt-3",
                                    crate::components::ontology::object_link::ObjectPageLink {
                                        kind: "media_asset".to_string(),
                                        native_id: asset_id.clone(),
                                    }
                                }
                            }
                            // Versions card — inline list (the drawer is
                            // gone): "Voir" previews a non-current version,
                            // restore/delete gated can_write.
                            div { class: "rounded-xl border border-gray-200 bg-white p-4",
                                div { class: "flex items-center justify-between gap-2",
                                    div { class: "flex items-center gap-2",
                                        h3 { class: "text-sm font-semibold text-gray-700", {t!("media-versions")} }
                                        span { class: "text-xs px-1.5 py-0.5 rounded-full bg-gray-100 text-gray-600",
                                            "{asset.versions_count}"
                                        }
                                    }
                                    if can_write {
                                        label {
                                        class: "inline-flex cursor-pointer items-center gap-1 rounded-lg bg-blue-50 px-2 py-1 text-xs text-blue-700 hover:bg-blue-100",
                                            if uploading() {
                                                span { class: "animate-spin inline-block rounded-full h-3 w-3 border-b-2 border-blue-600" }
                                            } else {
                                                icons::Upload { class: "h-3 w-3" }
                                            }
                                            input {
                                                class: "hidden",
                                                r#type: "file",
                                                onchange: move |evt| {
                                                    let files = evt.files();
                                                    let Some(file) = files.first().cloned() else {
                                                        return;
                                                    };
                                                    let asset_id = asset_id_for_add.clone();
                                                    spawn(async move {
                                                        uploading.set(true);
                                                        match file.read_bytes().await {
                                                            Ok(bytes) => {
                                                                let params = UploadParams {
                                                                    filename: Some(file.name()),
                                                                    content_type: {
                                                                        let ct = file.content_type().unwrap_or_default();
                                                                        if ct.is_empty() { None } else { Some(ct) }
                                                                    },
                                                                    ..Default::default()
                                                                };
                                                                match api::media::add_version(&asset_id, &params, bytes.to_vec())
                                                                    .await
                                                                {
                                                                    Ok(_) => {
                                                                        toasts::success(t!("media-upload-added"));
                                                                        viewing_version.set(None);
                                                                        reload.with_mut(|r| *r += 1);
                                                                        on_changed.call(());
                                                                    }
                                                                    Err(err) => toasts::error(err),
                                                                }
                                                            }
                                                            Err(err) => toasts::error(format!("lecture du fichier : {err:?}")),
                                                        }
                                                        uploading.set(false);
                                                    });
                                                },
                                            }
                                            {t!("media-versions-add")}
                                        }
                                    }
                                }
                                if let Some(v_err) = versions_error {
                                    div { class: "mt-2 text-sm text-red-600", "{v_err}" }
                                }
                                div { class: "mt-3 max-h-80 space-y-2 overflow-y-auto",
                                    for version in versions_rows {
                                        VersionRow {
                                            key: "{version.id}",
                                            version,
                                            asset: asset.clone(),
                                            can_write,
                                            reload,
                                            viewing_version,
                                            delete_target,
                                            on_changed,
                                        }
                                    }
                                }
                            }
                            // Download the current version (primary action —
                            // read, every role; the server enforces).
                            button {
                                class: "w-full px-4 py-2.5 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium disabled:opacity-50",
                                disabled: downloading(),
                                onclick: move |_| {
                                    let asset = asset.clone();
                                    spawn(async move {
                                        downloading.set(true);
                                        let mime = asset
                                            .content_type
                                            .clone()
                                            .unwrap_or_else(|| "application/octet-stream".to_string());
                                        match api::media::content_bytes(&asset.id).await {
                                            Ok(bytes) => {
                                                let filename = download_filename(&asset);
                                                if trigger_download(&filename, &mime, &bytes) {
                                                    toasts::success(t!("media-download-done"));
                                                } else {
                                                    toasts::error(t!("media-download-failed"));
                                                }
                                            }
                                            Err(err) => {
                                                let msg = format!(
                                                    "{} : {}",
                                                    t!("media-download-failed"),
                                                    err.message,
                                                );
                                                toasts::error(msg);
                                            }
                                        }
                                        downloading.set(false);
                                    });
                                },
                                if downloading() {
                                    span { class: "animate-spin inline-block rounded-full h-3 w-3 border-b-2 border-white mr-1" }
                                } else {
                                    icons::Download { class: "h-4 w-4 inline mr-1" }
                                }
                                {t!("media-download")}
                            }
                            // Delete asset (danger link).
                            button {
                                class: "flex w-full items-center justify-center gap-1.5 rounded-lg px-3 py-1.5 text-sm text-red-600 hover:bg-red-50 transition-colors",
                                onclick: move |_| delete_open.set(true),
                                icons::Trash2 { class: "h-4 w-4" }
                                {t!("media-delete")}
                            }
                        }
                    }
                },
                None if error().is_empty() => rsx! {
                    div { class: "text-center py-12",
                        span { class: "animate-spin inline-block rounded-full h-8 w-8 border-b-2 border-blue-600" }
                    }
                },
                None => rsx! {},
            }
            if delete_open() {
                ConfirmDialog {
                    key: "del-{delete_open()}",
                    title: t!("media-delete-confirm-title"),
                    message: t!("media-delete-confirm-message"),
                    confirm_label: t!("media-delete"),
                    on_confirm: move |_| {
                        let id = asset_id.clone();
                        spawn(async move {
                            match api::media::delete(&id).await {
                                Ok(_) => {
                                    toasts::success(t!("media-deleted"));
                                    on_changed.call(());
                                    on_close.call(());
                                }
                                Err(err) => toasts::error(err),
                            }
                        });
                        delete_open.set(false);
                    },
                    on_cancel: move |_| delete_open.set(false),
                }
            }
            // Version delete confirmation (was carried by the drawer).
            if let Some(n) = delete_target() {
                ConfirmDialog {
                    key: "confirm-delete-{n}",
                    title: t!("media-version-delete-confirm"),
                    message: "",
                    confirm_label: t!("media-version-delete"),
                    on_confirm: move |_| {
                        let id = asset_id_for_version_del.clone();
                        spawn(async move {
                            match api::media::delete_version(&id, n).await {
                                Ok(_) => {
                                    toasts::success(t!("media-version-deleted"));
                                    // If the previewed version just vanished,
                                    // fall back to the current one.
                                    if viewing_version() == Some(n) {
                                        viewing_version.set(None);
                                    }
                                    reload.with_mut(|r| *r += 1);
                                    on_changed.call(());
                                }
                                Err(err) if err.status == Some(409) => {
                                    toasts::error(t!("media-error-409-last-version"));
                                }
                                Err(err) => toasts::error(err),
                            }
                        });
                        delete_target.set(None);
                    },
                    on_cancel: move |_| delete_target.set(None),
                }
            }
        }
    }
}
/// Version row (detail Versions card) — gradient thumbnail per kind,
/// "current" badge, actions: View (read-only preview of a non-current
/// version), restore/delete (gated can_write), download (every role).
#[component]
fn VersionRow(
    version: api::media::MediaVersion,
    asset: MediaAsset,
    can_write: bool,
    mut reload: Signal<u32>,
    mut viewing_version: Signal<Option<i64>>,
    mut delete_target: Signal<Option<i64>>,
    on_changed: Callback<()>,
) -> Element {
    // This-version download in flight (button spinner).
    let mut downloading = use_signal(|| false);
    // Dedicated clones per closure (two `move` closures cannot capture the
    // same String — owned-props school).
    let asset_id_for_dl = asset.id.clone();
    let asset_id_for_restore = asset.id.clone();
    let thumb_gradient = kind_gradient(&asset);
    let v = version.version_number;
    let is_current = version.current;
    let is_viewed = viewing_version() == Some(v);
    let row_classes: &str = if is_current {
        "border-blue-300 bg-blue-50/60"
    } else if is_viewed {
        "border-amber-300 bg-amber-50/60"
    } else {
        "border-gray-200"
    };

    rsx! {
        div { class: "rounded-lg border p-2.5 {row_classes}",
            div { class: "flex items-center gap-3",
                // Thumbnail placeholder — gradient by kind, no fetch.
                div {
                    class: format!(
                        "flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-md bg-gradient-to-br {thumb_gradient}",
                    ),
                    match asset.media_kind() {
                        MediaKind::Panorama => rsx! {
                            icons::Image { class: "h-4 w-4 text-white/90" }
                        },
                        MediaKind::Splat => rsx! {
                            icons::Cube { class: "h-4 w-4 text-white/90" }
                        },
                        MediaKind::Floorplan => rsx! {
                            icons::Map { class: "h-4 w-4 text-white/90" }
                        },
                        MediaKind::Photo => rsx! {
                            icons::Image { class: "h-4 w-4 text-white/90" }
                        },
                        MediaKind::Model => rsx! {
                            icons::Eye { class: "h-4 w-4 text-white/90" }
                        },
                    }
                }
                div { class: "min-w-0 flex-1",
                    div { class: "flex flex-wrap items-center gap-1.5",
                        span { class: "text-sm font-semibold text-gray-900", "v{v}" }
                        if is_current {
                            span { class: "text-xs px-1.5 py-0.5 rounded-full bg-blue-100 text-blue-700",
                                {t!("media-version-current")}
                            }
                        } else if is_viewed {
                            span { class: "text-xs px-1.5 py-0.5 rounded-full bg-amber-100 text-amber-700",
                                {t!("media-version-view-pill")}
                            }
                        }
                    }
                    p { class: "truncate text-xs text-gray-500",
                        "{version.filename} · {format_size(version.size_bytes)} · {date_label(&version.created_at)}"
                    }
                    if let Some(note) = &version.note {
                        p { class: "text-xs text-gray-400 italic mt-0.5", "{note}" }
                    }
                }
                div { class: "flex items-center gap-1 flex-shrink-0",
                    // View — read-only preview of a non-current version
                    // (blob from /versions/{n}/content).
                    if !is_current {
                        button {
                            class: if is_viewed { "text-xs font-medium text-amber-600 hover:underline" } else { "text-xs text-blue-600 hover:underline" },
                            onclick: move |_| {
                                if is_viewed {
                                    viewing_version.set(None);
                                } else {
                                    viewing_version.set(Some(v));
                                }
                            },
                            {t!("media-version-view")}
                        }
                    }
                    if !is_current && can_write {
                        button {
                            class: "text-xs text-blue-600 hover:underline",
                            onclick: move |_| {
                                let id = asset_id_for_restore.clone();
                                spawn(async move {
                                    match api::media::restore(&id, v).await {
                                        Ok(_) => {
                                            toasts::success(t!("media-version-restored"));
                                            viewing_version.set(None);
                                            reload.with_mut(|r| *r += 1);
                                            on_changed.call(());
                                        }
                                        Err(err) => toasts::error(err),
                                    }
                                });
                            },
                            {t!("media-version-restore")}
                        }
                        button {
                            class: "text-red-500 hover:text-red-700",
                            title: t!("media-version-delete"),
                            onclick: move |_| delete_target.set(Some(v)),
                            icons::Trash2 { class: "h-4 w-4" }
                        }
                    }
                    // Download this version's bytes (every role).
                    button {
                        class: "text-blue-600 hover:text-blue-800 disabled:opacity-50",
                        title: t!("media-download"),
                        disabled: downloading(),
                        onclick: move |_| {
                            let asset_id = asset_id_for_dl.clone();
                            let filename = version.filename.clone();
                            let mime = version.content_type.clone();
                            spawn(async move {
                                downloading.set(true);
                                match api::media::version_content_bytes(&asset_id, v).await {
                                    Ok(bytes) => {
                                        if trigger_download(&filename, &mime, &bytes) {
                                            toasts::success(t!("media-download-done"));
                                        } else {
                                            toasts::error(t!("media-download-failed"));
                                        }
                                    }
                                    Err(err) => {
                                        let msg = format!(
                                            "{} : {}",
                                            t!("media-download-failed"),
                                            err.message,
                                        );
                                        toasts::error(msg);
                                    }
                                }
                                downloading.set(false);
                            });
                        },
                        if downloading() {
                            span { class: "animate-spin inline-block rounded-full h-3 w-3 border-b-2 border-blue-600" }
                        } else {
                            icons::Download { class: "h-4 w-4 inline" }
                        }
                    }
                }
            }
        }
    }
}
