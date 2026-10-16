//! Gallery of an object (assisted-creation.md D231): the media that show it,
//! held by `depicts` links. Pictures as thumbnails, a 3D model in the 3D
//! viewer with its named poses, voices in the shared player. Removing an
//! item closes the link; the media stays in the library.

use base64::Engine;
use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{LinkClose, LinkInput, LinkView, ObjectView};
use pnex_core::ontology::{
    DEPICTS_MODEL, DEPICTS_NAME, DEPICTS_POSE, DEPICTS_ROLE, REL_DEPICTS, ROLE_MODEL, ROLE_POSE,
    ROLE_REFERENCE,
};
use pnex_core::resources::KIND_MEDIA_ASSET;
use serde_json::{json, Map, Value};

use crate::api;
use crate::api::media::{MediaAsset, MediaKind, UploadParams};
use crate::components::gerbe::Gerbe;
use crate::components::mesh_viewer::{MeshViewer, NewPose, SavedPose};
use crate::components::resource_picker::{ResourcePick, ResourcePicker};
use crate::components::transport::MediaPlayer;
use crate::components::ui::{BTN_GHOST, BTN_SECONDARY_SM};
use crate::state::toasts;

/// One gallery item: the link and the media behind it.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    link: LinkView,
    media: MediaAsset,
}

impl Item {
    fn role(&self) -> &str {
        self.link.attributes[DEPICTS_ROLE]
            .as_str()
            .unwrap_or_default()
    }
    fn attr(&self, key: &str) -> &str {
        self.link
            .attributes
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// Role given to a media added to a gallery, from its kind.
fn role_for(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Mesh => ROLE_MODEL,
        MediaKind::Audio => "voice",
        _ => ROLE_REFERENCE,
    }
}

/// Poses of a model, from the pose links that name it.
fn poses_of(items: &[Item], model: &str) -> Vec<SavedPose> {
    items
        .iter()
        .filter(|i| i.role() == ROLE_POSE && i.attr(DEPICTS_MODEL) == model)
        .filter_map(|i| {
            Some(SavedPose {
                name: i.attr(DEPICTS_NAME).to_string(),
                pose: serde_json::from_str(i.attr(DEPICTS_POSE)).ok()?,
            })
        })
        .collect()
}

async fn load(object_id: String) -> Result<Vec<Item>, api::error::ApiError> {
    let links = api::ontology::object_links(&object_id, None, false).await?;
    let mut items = Vec::new();
    for link in links
        .into_iter()
        .filter(|l| l.link_type == REL_DEPICTS && l.target.id == object_id)
    {
        // A media deleted meanwhile drops out of the gallery.
        if let Ok(media) = api::media::detail(&link.source.native_id).await {
            items.push(Item { link, media });
        }
    }
    Ok(items)
}

/// Links a media (library id) to the object under `role`.
async fn attach(
    media_id: &str,
    object_id: &str,
    role: &str,
    mut attributes: Map<String, Value>,
) -> Result<(), api::error::ApiError> {
    let media = api::ontology::resolve(KIND_MEDIA_ASSET, media_id).await?;
    attributes.insert(DEPICTS_ROLE.into(), json!(role));
    api::ontology::create_link(&LinkInput {
        link_type: REL_DEPICTS.into(),
        source_id: media.id,
        target_id: object_id.into(),
        attributes,
        valid_from: None,
    })
    .await
    .map(|_| ())
}

/// Saves a pose: its PNG thumbnail becomes a media of the gallery, the
/// link carries the bone rotations and the model they apply to.
async fn save_pose(
    object: ObjectView,
    model: String,
    p: NewPose,
) -> Result<(), api::error::ApiError> {
    let b64 = p.png.split_once(',').map(|(_, d)| d).unwrap_or_default();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap_or_default();
    let media = api::media::upload(
        &UploadParams {
            name: Some(format!("{} — {}", object.title, p.name)),
            filename: Some("pose.png".into()),
            kind: Some(MediaKind::Photo),
            content_type: Some("image/png".into()),
            note: None,
        },
        bytes,
    )
    .await?;
    let attrs = Map::from_iter([
        (DEPICTS_NAME.to_string(), json!(p.name)),
        (DEPICTS_POSE.to_string(), json!(p.pose.to_string())),
        (DEPICTS_MODEL.to_string(), json!(model)),
    ]);
    attach(&media.id, &object.id, ROLE_POSE, attrs).await
}

#[component]
pub fn Gallery(object: ObjectView, can_write: bool) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut picking = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let oid = object.id.clone();
    let items = use_resource(move || {
        let id = oid.clone();
        async move {
            let _ = reload();
            load(id).await
        }
    });

    let on_picked = {
        let oid = object.id.clone();
        move |pick: ResourcePick| {
            picking.set(false);
            let ResourcePick::MediaAsset { id, .. } = pick else {
                return;
            };
            let oid = oid.clone();
            spawn(async move {
                let kind = api::media::detail(&id).await.map(|m| m.media_kind());
                let res = match kind {
                    Ok(k) => attach(&id, &oid, role_for(k), Map::new()).await,
                    Err(e) => Err(e),
                };
                match res {
                    Ok(()) => toasts::success("gallery-added"),
                    Err(e) => toasts::error(e),
                }
                reload.with_mut(|r| *r += 1);
            });
        }
    };

    let unreadable = t!("gallery-read-failed").to_string();
    let on_file = {
        let oid = object.id.clone();
        move |e: FormEvent| {
            let oid = oid.clone();
            let unreadable = unreadable.clone();
            async move {
                let Some(f) = e.files().first().cloned() else {
                    return;
                };
                busy.set(true);
                let res = match f.read_bytes().await {
                    Ok(bytes) => upload_and_attach(&oid, f.name(), bytes.to_vec()).await,
                    Err(_) => Err(api::error::ApiError::new(unreadable)),
                };
                busy.set(false);
                match res {
                    Ok(()) => toasts::success("gallery-added"),
                    Err(e) => toasts::error(e),
                }
                reload.with_mut(|r| *r += 1);
            }
        }
    };

    let list = match &*items.read() {
        Some(Ok(v)) => v.clone(),
        Some(Err(e)) => {
            let msg = crate::api::error_i18n::localize(e);
            return rsx! {
                p { class: "text-sm text-alarm", "{msg}" }
            };
        }
        None => {
            return rsx! {
                div { class: "flex justify-center py-8",
                    Gerbe { size: 28 }
                }
            };
        }
    };
    let models: Vec<Item> = list
        .iter()
        .filter(|i| i.role() == ROLE_MODEL)
        .cloned()
        .collect();
    let pictures: Vec<Item> = list
        .iter()
        .filter(|i| i.role() != ROLE_MODEL && i.role() != "voice")
        .cloned()
        .collect();
    let voices: Vec<Item> = list
        .iter()
        .filter(|i| i.role() == "voice")
        .cloned()
        .collect();

    rsx! {
        div { class: "space-y-6", "data-testid": "gallery",
            if can_write {
                div { class: "flex flex-wrap items-center gap-2",
                    button {
                        class: BTN_SECONDARY_SM,
                        r#type: "button",
                        "data-testid": "gallery-pick",
                        onclick: move |_| picking.set(true),
                        {t!("gallery-add-library")}
                    }
                    label { class: "{BTN_SECONDARY_SM} cursor-pointer",
                        {t!("gallery-upload")}
                        input {
                            r#type: "file",
                            class: "sr-only",
                            "data-testid": "gallery-file",
                            accept: "image/*,audio/*,.glb",
                            disabled: busy(),
                            onchange: on_file,
                        }
                    }
                    if busy() {
                        Gerbe { size: 20 }
                    }
                }
            }
            if list.is_empty() {
                p { class: "text-sm text-slate", {t!("gallery-empty")} }
            }
            for m in models {
                section { key: "{m.link.id}", class: "space-y-2",
                    ItemHeader {
                        item: m.clone(),
                        can_write,
                        on_removed: move |_| reload.with_mut(|r| *r += 1),
                    }
                    ModelView {
                        item: m.clone(),
                        poses: poses_of(&list, &m.link.source.id),
                        object: object.clone(),
                        can_write,
                        on_saved: move |_| reload.with_mut(|r| *r += 1),
                    }
                }
            }
            if !pictures.is_empty() {
                ul { class: "grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 gap-3",
                    for p in pictures {
                        li { key: "{p.link.id}", class: "space-y-1",
                            Thumb {
                                media: p.media.clone(),
                                whole: p.role() == ROLE_POSE,
                            }
                            ItemHeader {
                                item: p.clone(),
                                can_write,
                                on_removed: move |_| reload.with_mut(|r| *r += 1),
                            }
                        }
                    }
                }
            }
            for v in voices {
                section { key: "{v.link.id}", class: "space-y-2",
                    ItemHeader {
                        item: v.clone(),
                        can_write,
                        on_removed: move |_| reload.with_mut(|r| *r += 1),
                    }
                    Voice { media: v.media.clone() }
                }
            }
            if picking() {
                ResourcePicker {
                    initial_tab: None,
                    media_only: true,
                    on_picked,
                    on_close: move |_| picking.set(false),
                }
            }
        }
    }
}

async fn upload_and_attach(
    object_id: &str,
    filename: String,
    bytes: Vec<u8>,
) -> Result<(), api::error::ApiError> {
    let name = filename
        .rsplit_once('.')
        .map(|(n, _)| n.to_string())
        .unwrap_or_else(|| filename.clone());
    let media = api::media::upload(
        &UploadParams {
            name: Some(name),
            filename: Some(filename),
            ..Default::default()
        },
        bytes,
    )
    .await?;
    attach(
        &media.id,
        object_id,
        role_for(media.media_kind()),
        Map::new(),
    )
    .await
}

/// Name, role and the remove button of an item.
#[component]
fn ItemHeader(item: Item, can_write: bool, on_removed: EventHandler<()>) -> Element {
    let role = item.role().to_string();
    let label = match role.as_str() {
        ROLE_MODEL => t!("gallery-role-model"),
        ROLE_POSE => t!("gallery-role-pose"),
        "voice" => t!("gallery-role-voice"),
        "render" => t!("gallery-role-render"),
        _ => t!("gallery-role-reference"),
    };
    let title = match item.attr(DEPICTS_NAME) {
        "" => item.media.name.clone(),
        n => n.to_string(),
    };
    let link_id = item.link.id;
    rsx! {
        div { class: "flex items-center gap-2 min-w-0", "data-role": "{role}",
            div { class: "min-w-0 flex-1",
                p { class: "truncate text-sm font-medium text-ink", "{title}" }
                p { class: "text-xs text-slate", "{label}" }
            }
            if can_write {
                button {
                    class: BTN_GHOST,
                    r#type: "button",
                    title: t!("gallery-remove"),
                    aria_label: t!("gallery-remove"),
                    onclick: move |_| {
                        spawn(async move {
                            match api::ontology::close_link(link_id, &LinkClose::default()).await {
                                Ok(_) => toasts::success("gallery-removed"),
                                Err(e) => toasts::error(e),
                            }
                            on_removed.call(());
                        });
                    },
                    crate::components::icons::Trash { class: "h-4 w-4" }
                }
            }
        }
    }
}

/// Blob URL of a media's current content.
fn use_blob(media: &MediaAsset) -> Signal<Option<String>> {
    let mut url = use_signal(|| None::<String>);
    let id = media.id.clone();
    let mime = media.content_type.clone();
    use_effect(move || {
        let (id, mime) = (id.clone(), mime.clone());
        spawn(async move {
            let path = api::media::content_path(&id);
            url.set(crate::util::media_blob_url(&path, mime.as_deref()).await);
        });
    });
    url
}

/// `whole`: a pose thumbnail is a wide view, shown entirely, never cropped.
#[component]
fn Thumb(media: MediaAsset, whole: bool) -> Element {
    let url = use_blob(&media);
    let is_image = matches!(
        media.media_kind(),
        MediaKind::Photo | MediaKind::Floorplan | MediaKind::Panorama
    );
    rsx! {
        div { class: "aspect-square overflow-hidden rounded-lg border border-line bg-gray-50 flex items-center justify-center",
            match (is_image, url()) {
                (true, Some(u)) => rsx! {
                    img {
                        src: "{u}",
                        alt: "{media.name}",
                        class: if whole { "h-full w-full object-contain" } else { "h-full w-full object-cover" },
                    }
                },
                (true, None) => rsx! {
                    Gerbe { size: 24 }
                },
                _ => rsx! {
                    crate::components::icons::Image { class: "h-8 w-8 text-slate" }
                },
            }
        }
    }
}

#[component]
fn Voice(media: MediaAsset) -> Element {
    let url = use_blob(&media);
    let id = format!("gallery-voice-{}", media.id.replace('-', ""));
    rsx! {
        match url() {
            Some(u) => rsx! {
                MediaPlayer { id, src: u, video: false }
            },
            None => rsx! {
                Gerbe { size: 24 }
            },
        }
    }
}

#[component]
fn ModelView(
    item: Item,
    poses: Vec<SavedPose>,
    object: ObjectView,
    can_write: bool,
    on_saved: EventHandler<()>,
) -> Element {
    let url = use_blob(&item.media);
    let host_id = format!("gallery-mesh-{}", item.link.id);
    let model = item.link.source.id.clone();
    let on_save = move |p: NewPose| {
        let (object, model) = (object.clone(), model.clone());
        spawn(async move {
            match save_pose(object, model, p).await {
                Ok(()) => toasts::success("mesh-pose-saved"),
                Err(e) => toasts::error(e),
            }
            on_saved.call(());
        });
    };
    rsx! {
        match url() {
            Some(u) => rsx! {
                MeshViewer {
                    host_id,
                    url: u,
                    poses,
                    on_save_pose: can_write.then_some(EventHandler::new(on_save)),
                }
            },
            None => rsx! {
                div { class: "flex h-[420px] items-center justify-center rounded-lg bg-gray-900",
                    Gerbe { size: 36, inherit: true }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_follow_the_media_kind() {
        assert_eq!(role_for(MediaKind::Mesh), ROLE_MODEL);
        assert_eq!(role_for(MediaKind::Audio), "voice");
        assert_eq!(role_for(MediaKind::Photo), ROLE_REFERENCE);
    }
}
