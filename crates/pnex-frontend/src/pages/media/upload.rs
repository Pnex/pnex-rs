use super::*;

/// Upload modal — file input (Dioxus 0.7: `evt.files()` via the
/// `HasFileData` trait), metadata in query, bytes in body
/// (`api::media::upload`, octet-stream).
#[component]
pub(super) fn UploadModal(on_close: Callback<()>, on_uploaded: Callback<()>) -> Element {
    let mut file_name = use_signal(String::new);
    let mut file_size = use_signal(|| 0i64);
    let mut file_type = use_signal(String::new);
    let mut display_name = use_signal(String::new);
    let mut kind = use_signal(|| "auto".to_string());
    let mut submitting = use_signal(|| false);

    rsx! {
        Modal { title: t!("media-upload-title"), max_width: "max-w-md",
            on_close: move |_| on_close.call(()),
            div { class: "space-y-4",
                div {
                    label { class: "block text-sm font-medium text-gray-700 mb-1", {t!("media-upload-file")} }
                    input {
                        class: "w-full text-sm border border-gray-300 rounded-lg px-3 py-2",
                        r#type: "file",
                        onchange: move |evt| {
                            let files = evt.files();
                            if let Some(file) = files.first().cloned() {
                                file_name.set(file.name());
                                file_size.set(file.size() as i64);
                                file_type.set(file.content_type().unwrap_or_default());
                                if display_name().trim().is_empty() {
                                    display_name.set(file.name());
                                }
                                // Read bytes immediately (consumed on
                                // submit) — FileData::read_bytes.
                                spawn(async move {
                                    match file.read_bytes().await {
                                        Ok(bytes) => store_file_bytes(bytes.to_vec()),
                                        Err(err) => toasts::error(format!("lecture du fichier : {err:?}")),
                                    }
                                });
                            }
                        },
                    }
                }
                div {
                    label { class: "block text-sm font-medium text-gray-700 mb-1", {t!("media-upload-name")} }
                    input {
                        class: "w-full border border-gray-300 rounded-lg px-3 py-2 text-sm",
                        placeholder: t!("media-upload-name-placeholder"),
                        value: "{display_name}",
                        oninput: move |evt| display_name.set(evt.value()),
                    }
                }
                div {
                    label { class: "block text-sm font-medium text-gray-700 mb-1", {t!("media-upload-kind")} }
                    select {
                        class: "w-full border border-gray-300 rounded-lg px-3 py-2 text-sm bg-white",
                        value: "{kind}",
                        onchange: move |evt| kind.set(evt.value()),
                        option { value: "auto", {t!("media-upload-kind-auto")} }
                        option { value: "photo", {t!("media-kind-photo")} }
                        option { value: "panorama", {t!("media-kind-panorama")} }
                        option { value: "splat", {t!("media-kind-splat")} }
                        option { value: "floorplan", {t!("media-kind-floorplan")} }
                        option { value: "model", {t!("media-kind-model")} }
                    }
                }
                div { class: "flex justify-end gap-2 pt-2",
                    button {
                        class: "px-4 py-2 text-sm text-gray-700 bg-gray-100 hover:bg-gray-200 rounded-lg",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: "px-4 py-2 text-sm text-white bg-blue-600 hover:bg-blue-700 rounded-lg disabled:opacity-50",
                        disabled: submitting() || file_name().is_empty(),
                        onclick: move |_| {
                            spawn(async move {
                                submitting.set(true);
                                let params = UploadParams {
                                    name: Some(display_name().trim().to_string()),
                                    filename: Some(file_name()),
                                    kind: match kind().as_str() {
                                        "photo" => Some(MediaKind::Photo),
                                        "panorama" => Some(MediaKind::Panorama),
                                        "splat" => Some(MediaKind::Splat),
                                        "floorplan" => Some(MediaKind::Floorplan),
                                        "model" => Some(MediaKind::Model),
                                        _ => None,
                                    },
                                    content_type: {
                                        let ct = file_type();
                                        if ct.is_empty() { None } else { Some(ct) }
                                    },
                                    note: None,
                                };
                                // Bytes stored by the `onchange` (immediate
                                // read), consumed here.
                                match take_file_bytes() {
                                    Some(bytes) => match api::media::upload(&params, bytes).await {
                                        Ok(_) => {
                                            toasts::success(t!("media-upload-added"));
                                            on_uploaded.call(());
                                        }
                                        Err(err) => {
                                            toasts::error(err);
                                        }
                                    },
                                    None => toasts::error("media-upload-no-file"),
                                }
                                submitting.set(false);
                            });
                        },
                        {if submitting() { t!("common-loading") } else { t!("media-upload") }}
                    }
                }
            }
        }
    }
}

/// Slot for the current file's bytes (stored by the input `onchange`,
/// consumed on submit — same idiom as the thread_local client,
/// single-threaded wasm).
fn store_file_bytes(bytes: Vec<u8>) {
    FILE_BYTES.with(|slot| *slot.borrow_mut() = Some(bytes));
}

fn take_file_bytes() -> Option<Vec<u8>> {
    FILE_BYTES.with(|slot| slot.borrow_mut().take())
}

thread_local! {
    static FILE_BYTES: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
}
