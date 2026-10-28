//! Folder tree of the POI drawer — D42 folders (`map_pin → folder`,
//! containment) holding the attached objects (`placed_on` edges for
//! media/tour/dashboard at the POI root; containment for any object
//! filed in a folder; D43 placements for devices, whose
//! "location detail" stays editable in the tree row).
//!
//! Drag & drop with the ⠿ handle — dashboard_editor pointer-events school
//! (`drag_template`): pointerdown grabs, pointerup on a folder or the
//! root area drops. No HTML5 drag (touch-hostile); no fine
//! reordering (`sort_key`, V1).
//!
//! Representation model:
//! - object at the POI root = `placed_on` edge (media/tour/dashboard) or
//!   D43 placement (device) — unchanged;
//! - object in a folder = containment row + edge deleted (the label
//!   is recreated on return to the root);
//! - root folder = `map_pin` parent; nested = folder parent.
//!
//! Display names of all kinds are hydrated by the backend
//! (`RefNode.name`) — no client-side name cache.

use std::collections::{HashMap, HashSet};

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::error::ApiError;
use crate::api::resources::{self, RefNode};
use crate::api::viz::{self, DevicePlacement, VizLink};
use crate::components::confirm::ConfirmDialog;
use crate::components::icons;
use crate::components::poi_preview::PreviewTarget;
use crate::state::toasts;

mod model;

use model::*;

// ─────────────────────────── types ───────────────────────────

/// Containment walk from the POI: root folders then subfolders
/// (explicit loop — no async recursion). N+1 accepted (folders are rare).
async fn walk_containment(poi_id: String) -> Result<TreeModel, ApiError> {
    let root = resources::get_containment("map_pin", &poi_id).await?;
    let root_folders: Vec<RefNode> = root
        .children
        .iter()
        .filter(|c| c.kind == "folder")
        .cloned()
        .collect();
    let mut model = TreeModel {
        root_folders,
        ..Default::default()
    };
    let mut queue: Vec<String> = model.root_folders.iter().map(|f| f.id.clone()).collect();
    let mut idx = 0;
    while idx < queue.len() {
        let fid = queue[idx].clone();
        idx += 1;
        let c = resources::get_containment("folder", &fid).await?;
        for child in &c.children {
            if child.kind == "folder" {
                queue.push(child.id.clone());
            } else {
                model
                    .contained
                    .insert((child.kind.clone(), child.id.clone()));
            }
        }
        model.children.insert(fid, c.children);
    }
    Ok(model)
}

// ─────────────────────────── mutations ───────────────────────────

/// Runs a drag & drop drop. `Ok(true)` = filed (toast "in"),
/// `Ok(false)` = moved back to the root (toast "out").
async fn move_item(item: &DragItem, target: &DropId, poi_id: &str) -> Result<bool, ApiError> {
    match target {
        DropId::Folder(fid) => {
            if item.kind == "folder" {
                resources::set_parent("folder", &item.id, Some(("folder", fid))).await?;
            } else if item.kind == "device" {
                // The device containment id = PK (registry), not the slug.
                resources::set_parent("device", &item.id, Some(("folder", fid))).await?;
            } else {
                // Root object filed: containment first, then the `placed_on`
                // edge goes (the order loses nothing on failure).
                resources::set_parent(&item.kind, &item.id, Some(("folder", fid))).await?;
                if let Some(edge_id) = item.edge_id {
                    delete_edge_lenient(edge_id).await;
                }
            }
            Ok(true)
        }
        DropId::Root => {
            if item.kind == "folder" {
                resources::set_parent("folder", &item.id, Some(("map_pin", poi_id))).await?;
            } else if item.kind == "device" {
                resources::set_parent("device", &item.id, None).await?;
            } else {
                // Filed object moved back: `placed_on` edge recreated FIRST
                // (409 = already there, success — idempotent path), then containment
                // detached. The reverse order (finding 2026-09-13) orphaned
                // the object when the edge failed: empty containment parent +
                // no edge → invisible from every tree, reload
                // included. An object already at the root is a no-op filtered
                // upstream (`from == target`).
                create_edge_lenient(
                    ("map_pin", poi_id),
                    (&item.kind, &item.id),
                    Some(serde_json::json!({ "label": item.label })),
                )
                .await?;
                resources::set_parent(&item.kind, &item.id, None).await?;
            }
            Ok(false)
        }
    }
}

/// `delete_edge` where a 404 is a success (double click, other tab — D26).
async fn delete_edge_lenient(edge_id: i64) {
    match resources::delete_edge(edge_id).await {
        Ok(_) => {}
        Err(err) if err.status == Some(404) => {}
        Err(err) => toasts::error(format!("{err}")),
    }
}

/// `create_edge` where a duplicate (409) is a success — the targeted
/// `placed_on` edge already exists, exactly the goal. Repair paths (back
/// to root, eject) thus stay idempotent: a re-drop or an interrupted
/// drop never blocks the object (blocking 409 = re-drop impossible).
async fn create_edge_lenient(
    source: (&str, &str),
    target: (&str, &str),
    placement: Option<serde_json::Value>,
) -> Result<(), ApiError> {
    match resources::create_edge("placed_on", source, target, placement).await {
        Ok(_) => {}
        Err(err) if err.status == Some(409) => {}
        Err(err) => return Err(err),
    }
    Ok(())
}

/// Ejects a filed object to the POI root: `placed_on` edge recreated
/// with its display name THEN containment detached (edge-first — a failure
/// leaves the object filed, never orphaned). Media/tour/dashboard only
/// for the edge (only these kinds carry `placed_on`); other kinds
/// (filed device) only get the containment detach.
async fn eject_object(
    poi_id: &str,
    id: &str,
    kind: &str,
    name: Option<&str>,
) -> Result<(), ApiError> {
    if matches!(kind, "media_asset" | "tour" | "dashboard") {
        let placement = name.map(|label| serde_json::json!({ "label": label }));
        create_edge_lenient(("map_pin", poi_id), (kind, id), placement).await?;
    }
    resources::set_parent(kind, id, None).await?;
    Ok(())
}

// ─────────────────────────── views ───────────────────────────

/// Row ready to render: everything is resolved before the rsx ("flat rsx"
/// school). Payloads pre-cloned for the closures ('static FnMut, never a
/// `let` in an rsx `for`).
struct TreeRow {
    key: String,
    depth: usize,
    is_folder: bool,
    expanded_now: bool,
    has_children: bool,
    name: String,
    emoji: Option<String>,
    /// Kind for the icon (`None` = folder).
    icon_kind: Option<String>,
    /// device_type for device rows.
    subtitle: Option<String>,
    is_device: bool,
    is_dead: bool,
    is_dragged: bool,
    /// Folder: expansion key.
    toggle: Option<String>,
    /// Drag handle.
    drag: Option<DragItem>,
    /// Folder: drop target.
    drop_id: Option<DropId>,
    /// Previewable object (live media/tour/dashboard): target of the row
    /// click → preview panel.
    preview: Option<PreviewTarget>,
    /// Previewable object: target of the pin star.
    pin_target: Option<PreviewTarget>,
    /// Disjoint copy of `preview` for the Detach handler (preview/pin
    /// cleanup after detach — disjoint per-field captures).
    clear_target: Option<PreviewTarget>,
    /// Row = object currently shown in the preview (highlighted).
    is_previewed: bool,
    /// Row = current pin (filled star).
    is_pinned: bool,
    /// Object: Detach button.
    detach: Option<Detach>,
    /// Folder: (id, name) for deletion.
    delete: Option<(i64, String)>,
    /// Folder: (id, name) for the D42 labels modal (disjoint copy of
    /// `delete` so each handler captures its own field).
    labels: Option<(i64, String)>,
    /// Device: placement for location editing.
    placement: Option<DevicePlacement>,
}

/// Context for building the views (drawer data + drag state).
struct ViewCtx<'a> {
    model: &'a TreeModel,
    expanded: &'a HashSet<String>,
    dragging: &'a Option<DragItem>,
    /// pk containment → (slug, device_type).
    device_by_pk: HashMap<String, (String, String)>,
    /// slug → placement (location editing in the tree).
    placement_by_slug: HashMap<String, DevicePlacement>,
    /// Current preview (highlight of the displayed row).
    preview: &'a Option<PreviewTarget>,
    /// Current pin (filled star).
    pinned: &'a Option<PreviewTarget>,
}

// ─────────────────────────── component ───────────────────────────

#[component]
pub fn PoiTreeSection(
    poi_id: String,
    /// `placed_on` edges of the POI (poi_detail).
    links: Vec<VizLink>,
    /// D43 placements of the POI (poi_detail).
    placements: Vec<DevicePlacement>,
    /// Org index of devices: (containment pk, slug, device_type) —
    /// the device containment id = integer PK, not the slug.
    devices: Vec<(String, String, String)>,
    /// Drawer version (detail_version) — refetch of the containment walk.
    version: Signal<u64>,
    /// Object shown in the preview panel (switches from asset to asset).
    mut preview: Signal<Option<PreviewTarget>>,
    /// Current pin of the POI (filled star on the matching row).
    mut pinned: Signal<Option<PreviewTarget>>,
    /// Object row click → open the preview (read-only, map panel).
    on_preview: EventHandler<PreviewTarget>,
    /// Star ★: sets or removes the pin (toggle on the drawer side).
    on_pin: EventHandler<PreviewTarget>,
    on_attach: EventHandler<()>,
    /// Refetch drawer + map after a mutation (detail_version idiom).
    on_mutated: EventHandler<()>,
) -> Element {
    let mut expanded = use_signal(HashSet::<String>::new);
    let mut dragging = use_signal(|| None::<DragItem>);
    let mut new_folder = use_signal(String::new);
    let mut pending_delete = use_signal(|| None::<(i64, String)>);
    // Folder whose D42 labels are being edited (inherited by its content).
    let mut labels_folder = use_signal(|| None::<(i64, String)>);

    // Containment walk, refetched on each version bump (attach/detach/
    // move/location save — the drawer bumps `detail_version`).
    let poi_id_for_walk = poi_id.clone();
    let model = use_resource(move || {
        let id = poi_id_for_walk.clone();
        let _v = version();
        async move { walk_containment(id).await }
    });

    // Expanded by default: roots on the first model (only once —
    // the user stays in control afterwards).
    let mut seeded = use_signal(|| false);
    use_effect(move || {
        if seeded() {
            return;
        }
        if let Some(Ok(model)) = &*model.value().read() {
            // Breadcrumb deep link: also unfold the path to the clicked folder.
            let unfold = std::mem::take(&mut *crate::state::map::OPEN_POI_FOLDERS.write());
            expanded.with_mut(|set| {
                for f in &model.root_folders {
                    set.insert(f.id.clone());
                }
                set.extend(unfold);
            });
            seeded.set(true);
        }
    });

    // Owned snapshots for the 'static closures (one per closure — a
    // `move` takes ownership of its capture; never a borrow of props).
    let model_for_drop = model
        .value()
        .read()
        .clone()
        .and_then(|r| r.ok())
        .unwrap_or_default();
    let model_for_delete = model
        .value()
        .read()
        .clone()
        .and_then(|r| r.ok())
        .unwrap_or_default();
    let poi_id_for_drop = poi_id.clone();
    let poi_id_for_delete = poi_id.clone();
    let poi_id_for_create = poi_id.clone();
    // Ejecting a filed object (Detach) → recreates the edge on THIS POI.
    let poi_id_for_eject = poi_id.clone();

    // Drop of a drag: guards then mutation + toast + refetch.
    // Callback = Copy → capturable by several rsx handlers.
    let perform_drop = Callback::new(move |(item, target): (DragItem, DropId)| {
        // Cycle guard: a folder is not dropped into itself or one of
        // its descendants (the server would answer 400).
        if let (DropId::Folder(dst), "folder") = (&target, item.kind.as_str()) {
            let mut forbidden = descendants_of(&model_for_drop.children, &item.id);
            forbidden.insert(item.id.clone());
            if forbidden.contains(dst) {
                toasts::error(t!("poi-tree-cycle").to_string());
                return;
            }
        }
        // Identical drop → silent no-op.
        if item.from == target {
            return;
        }
        // Clones in the body: an `async move` that moves its captures out
        // of the closure would make it FnOnce (handlers = FnMut).
        let poi = poi_id_for_drop.clone();
        spawn(async move {
            match move_item(&item, &target, &poi).await {
                Ok(true) => toasts::success(t!("poi-tree-move-in").to_string()),
                Ok(false) => toasts::success(t!("poi-tree-move-out").to_string()),
                Err(err) => toasts::error(format!("{err}")),
            }
            on_mutated.call(());
        });
    });

    // Deleting a folder: subfolders moved up under the POI, objects
    // ejected to the root (edge recreated), then delete_folder.
    let perform_delete = move |(fid, _name): (i64, String)| {
        // Clones in the body (see perform_drop) — the closure stays FnMut.
        let model = model_for_delete.clone();
        let poi = poi_id_for_delete.clone();
        spawn(async move {
            let fid_string = fid.to_string();
            // 1. Subfolders (any depth) → POI root, otherwise the org-wide
            //    re-root of delete_folder would make them invisible.
            let mut subfolders: Vec<String> = Vec::new();
            let mut queue: Vec<String> = vec![fid_string.clone()];
            let mut idx = 0;
            while idx < queue.len() {
                let cur = queue[idx].clone();
                idx += 1;
                if cur != fid_string {
                    subfolders.push(cur.clone());
                }
                if let Some(kids) = model.children.get(&cur) {
                    for c in kids {
                        if c.kind == "folder" {
                            queue.push(c.id.clone());
                        }
                    }
                }
            }
            for sub in &subfolders {
                if let Err(err) =
                    resources::set_parent("folder", sub, Some(("map_pin", &poi))).await
                {
                    toasts::error(format!("{err}"));
                }
            }
            // 2. **Direct** objects ejected to the root (`placed_on` edge
            //    recreated with their name) — the subfolders moved up in 1
            //    keep their content (containment intact).
            if let Some(kids) = model.children.get(&fid_string) {
                for c in kids {
                    if c.kind == "folder" {
                        continue;
                    }
                    if c.kind == "device" {
                        // Device: containment detached, placement intact.
                        if let Err(err) = resources::set_parent("device", &c.id, None).await {
                            toasts::error(format!("{err}"));
                        }
                    } else if let Err(err) =
                        eject_object(&poi, &c.id, &c.kind, c.name.as_deref()).await
                    {
                        toasts::error(format!("{err}"));
                    }
                }
            }
            // 3. The folder itself.
            match resources::delete_folder(fid).await {
                Ok(_) => toasts::success(t!("poi-tree-folder-deleted").to_string()),
                Err(err) => toasts::error(format!("{err}")),
            }
            on_mutated.call(());
        });
    };

    // Inline creation of a POI root folder.
    // Callback = Copy → capturable by onclick AND onkeydown.
    let create_folder_now = Callback::new(move |(): ()| {
        let name = new_folder().trim().to_string();
        if name.is_empty() {
            return;
        }
        new_folder.set(String::new());
        let poi = poi_id_for_create.clone();
        spawn(async move {
            match resources::create_folder(&name, None).await {
                Ok(folder) => {
                    let new_id = folder.id.to_string();
                    if let Err(err) =
                        resources::set_parent("folder", &new_id, Some(("map_pin", &poi))).await
                    {
                        toasts::error(format!("{err}"));
                    } else {
                        toasts::success(t!("poi-tree-folder-created").to_string());
                    }
                }
                Err(err) => toasts::error(format!("{err}")),
            }
            on_mutated.call(());
        });
    });

    // Unified detach (edge / filed object / placement) — a single Callback
    // (Copy) reusable by both render branches (device div / object
    // button). Filed object: Detach = ejection to the POI root (`placed_on`
    // edge recreated — a pure detach left the object orphaned,
    // invisible from every tree, finding 2026-09-13); filed device:
    // ejection from the folder only (placement intact, D43 amendment); root
    // device: the placement goes. The clones needed by `spawn` are
    // made in the body — a Callback owns its captures, one clone per
    // call, never FnOnce.
    let perform_detach = Callback::new(
        move |(detach, target, name): (Detach, Option<PreviewTarget>, String)| {
            let poi = poi_id_for_eject.clone();
            spawn(async move {
                match detach {
                    Detach::Placement(placement_id) => {
                        // 404 = already detached (other tab) — lenient success.
                        match viz::delete_poi_placement(placement_id).await {
                            Ok(_) => {}
                            Err(err) if err.status == Some(404) => {}
                            Err(err) => toasts::error(format!("{err}")),
                        }
                    }
                    Detach::Contained { kind, id } => {
                        if kind == "device" {
                            // Filed device: only containment goes, the placement
                            // stays (the device becomes a root device again).
                            if let Err(err) = resources::set_parent("device", &id, None).await {
                                toasts::error(format!("{err}"));
                            }
                        } else if let Err(err) = eject_object(&poi, &id, &kind, Some(&name)).await {
                            toasts::error(format!("{err}"));
                        }
                    }
                    Detach::Edge(edge_id) => {
                        delete_edge_lenient(edge_id).await;
                    }
                }
                // Preview/pin pointed at the detached object → immediate local
                // cleanup.
                if let Some(t) = target {
                    if let Some(p) = preview.cloned() {
                        if p.kind == t.kind && p.id == t.id {
                            preview.set(None);
                        }
                    }
                    if let Some(p) = pinned.cloned() {
                        if p.kind == t.kind && p.id == t.id {
                            pinned.set(None);
                        }
                    }
                }
                on_mutated.call(());
            });
        },
    );

    // ── precomputed views (never a `let` in an rsx `for`) ──
    let model_for_views = model
        .value()
        .read()
        .clone()
        .and_then(|r| r.ok())
        .unwrap_or_default();
    let dragging_val = dragging();
    let expanded_val = expanded();
    let preview_val = preview.cloned();
    let pinned_val = pinned.cloned();
    let views = build_views(
        &model_for_views,
        &links,
        &placements,
        &devices,
        &expanded_val,
        &dragging_val,
        &preview_val,
        &pinned_val,
    );
    let total = views.len();

    rsx! {
        div {
            class: "space-y-1.5",
            // Any pointer release outside a target cancels the drag.
            onpointerup: move |_| {
                if dragging().is_some() {
                    dragging.set(None);
                }
            },
            // Actions: counter + ＋ folder + ＋ object
            div { class: "flex items-center justify-between gap-2",
                span { class: "text-xs text-gray-500", {t!("poi-tree-count", count : total)} }
                div { class: "flex gap-1",
                    button {
                        class: "px-2 py-1 text-xs font-medium text-signal-700 bg-signal-50 hover:bg-signal-100 rounded transition-colors",
                        onclick: move |_| create_folder_now(()),
                        {t!("poi-tree-add-folder")}
                    }
                    button {
                        class: "px-2 py-1 text-xs font-medium text-signal-700 bg-signal-50 hover:bg-signal-100 rounded transition-colors",
                        onclick: move |_| on_attach.call(()),
                        {t!("poi-attachment-attach")}
                    }
                }
            }
            // Inline creation of a root folder
            div { class: "flex gap-1",
                input {
                    class: "flex-1 min-w-0 px-2 py-1 text-xs border border-gray-300 rounded focus:outline-none focus:ring-1 focus:ring-signal-400",
                    placeholder: t!("poi-tree-folder-placeholder"),
                    value: "{new_folder}",
                    oninput: move |e| new_folder.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            create_folder_now(());
                        }
                    },
                }
            }
            if views.is_empty() {
                p { class: "text-sm text-gray-500", {t!("poi-attachment-empty")} }
            }
            // Tree rows
            for row in views {
                if row.is_device {
                    // Device: not a button (location editing inside) —
                    // handle + D43 placement card.
                    div {
                        key: "{row.key}",
                        class: if row.is_dragged { "flex items-start gap-1 opacity-40" } else { "flex items-start gap-1" },
                        span {
                            class: "select-none cursor-grab text-gray-300 hover:text-gray-500 pt-2 shrink-0",
                            onpointerdown: move |e| {
                                e.stop_propagation();
                                if let Some(item) = row.drag.clone() {
                                    dragging.set(Some(item));
                                }
                            },
                            "⠿"
                        }
                        div { class: "flex-1 min-w-0",
                            if let Some(placement) = &row.placement {
                                DevicePlacementRow {
                                    key: "loc-{row.key}",
                                    placement: placement.clone(),
                                    device_type: row.subtitle.clone(),
                                    on_save_location: move |(placement_id, value): (i64, String)| {
                                        spawn(async move {
                                            // Empty string = clear (null).
                                            let body = serde_json::json!(
                                                { "location_detail" : if value.trim().is_empty() {
                                                serde_json::Value::Null } else { serde_json::json!(value.trim()) }, }
                                            );
                                            if let Err(err) =
                                                viz::update_poi_placement(placement_id, body).await
                                            {
                                                toasts::error(format!("{err}"));
                                            }
                                            on_mutated.call(());
                                        });
                                    },
                                }
                            } else {
                                // Stored in a folder without a placement on this
                                // site (moved elsewhere meanwhile): name only.
                                span { class: "flex items-center gap-2 text-sm text-gray-800 truncate",
                                    KindIcon { kind: "device".to_string() }
                                    {row.name.clone()}
                                }
                            }
                        }
                        // Detach: root device = placement goes (free
                        // device, D43 amendment); filed device = ejection from the
                        // folder (same semantics as the other objects).
                        if row.detach.is_some() {
                            span {
                                class: "shrink-0 text-xs text-red-600 hover:underline cursor-pointer pt-2",
                                onclick: move |e| {
                                    e.stop_propagation();
                                    if let Some(detach) = row.detach.clone() {
                                        perform_detach.call((detach, row.clear_target.clone(), row.name.clone()));
                                    }
                                },
                                {t!("poi-attachment-detach")}
                            }
                        }
                    }
                } else {
                    button {
                        key: "{row.key}",
                        class: if row.is_dragged { "w-full text-left px-2 py-1.5 flex items-center gap-1 rounded opacity-40 transition-colors" } else if row.is_folder { "w-full text-left px-2 py-1.5 flex items-center gap-1 rounded hover:bg-signal-50 transition-colors" } else if row.is_previewed { "w-full text-left px-2 py-1.5 flex items-center gap-1 rounded bg-signal-50 transition-colors" } else { "w-full text-left px-2 py-1.5 flex items-center gap-1 rounded hover:bg-gray-50 transition-colors" },
                        style: format!("padding-left: {}px", row.depth * 14 + 4),
                        onpointerup: move |e| {
                            e.stop_propagation();
                            if let Some(item) = dragging() {
                                dragging.set(None);
                                if let Some(drop_id) = row.drop_id.clone() {
                                    perform_drop((item, drop_id));
                                }
                            }
                        },
                        onclick: move |_| {
                            // Folder: expand/collapse; previewable object: switches
                            // the preview panel (never a popup or route).
                            if let Some(fid) = row.toggle.clone() {
                                let mut set = expanded.write();
                                if !set.remove(&fid) {
                                    set.insert(fid);
                                }
                            } else if let Some(target) = row.preview.clone() {
                                on_preview.call(target);
                            }
                        },
                        span { class: "select-none shrink-0 text-gray-500 w-3",
                            {if row.expanded_now { "▾" } else if row.has_children { "▸" } else { "•" }}
                        }
                        span {
                            class: "select-none cursor-grab text-gray-300 hover:text-gray-500 shrink-0",
                            onpointerdown: move |e| {
                                e.stop_propagation();
                                if let Some(item) = row.drag.clone() {
                                    dragging.set(Some(item));
                                }
                            },
                            "⠿"
                        }
                        if row.is_folder {
                            if let Some(emoji) = &row.emoji {
                                span { class: "shrink-0 text-sm", {emoji.clone()} }
                            } else {
                                icons::Folder { class: "w-3.5 h-3.5 shrink-0 text-gray-500" }
                            }
                        } else {
                            KindIcon { kind: row.icon_kind.clone().unwrap_or_default() }
                        }
                        span { class: if row.is_dead { "truncate text-xs text-gray-500 line-through flex-1" } else if row.is_folder { "truncate text-sm font-medium text-gray-900 flex-1" } else { "truncate text-sm text-gray-700 flex-1" },
                            {row.name.clone()}
                        }
                        if let Some(sub) = &row.subtitle {
                            span { class: "shrink-0 text-xs text-gray-500", {sub.clone()} }
                        }
                        // Previewable object: pin star (default preview
                        // when the POI opens — first attached object
                        // pinned by the drawer).
                        if let Some(pin_target) = row.pin_target.clone() {
                            span {
                                class: if row.is_pinned { "shrink-0 text-yellow-500 cursor-pointer hover:text-yellow-600 text-sm leading-none" } else { "shrink-0 text-gray-300 hover:text-yellow-500 cursor-pointer text-sm leading-none" },
                                title: if row.is_pinned { t!("poi-unpin").to_string() } else { t!("poi-pin").to_string() },
                                onclick: move |e| {
                                    e.stop_propagation();
                                    on_pin.call(pin_target.clone());
                                },
                                {if row.is_pinned { "★" } else { "☆" }}
                            }
                        }
                        // Object: Detach
                        if row.detach.is_some() {
                            span {
                                class: "shrink-0 text-xs text-red-600 hover:underline cursor-pointer",
                                onclick: move |e| {
                                    e.stop_propagation();
                                    if let Some(detach) = row.detach.clone() {
                                        perform_detach.call((detach, row.clear_target.clone(), row.name.clone()));
                                    }
                                },
                                {t!("poi-attachment-detach")}
                            }
                        }
                        // Folder: D42 labels (inherited by everything inside).
                        if row.labels.is_some() {
                            span {
                                class: "shrink-0 text-gray-300 hover:text-signal-600 cursor-pointer",
                                title: t!("resources-labels-title"),
                                onclick: move |e| {
                                    e.stop_propagation();
                                    if let Some(payload) = row.labels.clone() {
                                        labels_folder.set(Some(payload));
                                    }
                                },
                                icons::Tag { class: "w-3 h-3" }
                            }
                        }
                        // Folder: delete
                        if row.delete.is_some() {
                            span {
                                class: "shrink-0 text-gray-300 hover:text-red-600 cursor-pointer",
                                onclick: move |e| {
                                    e.stop_propagation();
                                    if let Some(payload) = row.delete.clone() {
                                        pending_delete.set(Some(payload));
                                    }
                                },
                                icons::Trash2 { class: "w-3 h-3" }
                            }
                        }
                    }
                }
            }
            // Root drop area
            div {
                class: "flex items-center gap-1 px-2 py-2 rounded border border-dashed border-gray-200 hover:bg-signal-50 transition-colors",
                onpointerup: move |e| {
                    e.stop_propagation();
                    if let Some(item) = dragging() {
                        dragging.set(None);
                        perform_drop((item, DropId::Root));
                    }
                },
                icons::Package { class: "w-3 h-3 shrink-0 text-gray-500" }
                span { class: "text-xs text-gray-500", {t!("poi-tree-root-zone")} }
            }
            // Folder deletion confirmation
            if let Some((fid, name)) = pending_delete() {
                ConfirmDialog {
                    key: "del-{fid}",
                    title: t!("poi-tree-delete-title").to_string(),
                    message: t!("poi-tree-delete-message", name : name.clone()).to_string(),
                    confirm_label: t!("common-delete").to_string(),
                    on_confirm: move |_| {
                        perform_delete((fid, name.clone()));
                        pending_delete.set(None);
                    },
                    on_cancel: move |_| pending_delete.set(None),
                }
            }
            if let Some((fid, fname)) = labels_folder() {
                crate::components::modal::Modal {
                    title: fname,
                    max_width: "max-w-md".to_string(),
                    on_close: move |_| labels_folder.set(None),
                    crate::components::labels_editor::LabelsEditor {
                        key: "{fid}",
                        kind: pnex_core::resources::KIND_FOLDER.to_string(),
                        id: fid.to_string(),
                        can_write: crate::state::access::can("pois:write"),
                    }
                }
            }
        }
    }
}

/// Icon per kind (folders handled separately with an emoji).
#[component]
pub fn KindIcon(kind: String) -> Element {
    rsx! {
        match kind.as_str() {
            "device" => rsx! {
                icons::Cpu { class: "w-3.5 h-3.5 shrink-0 text-gray-500" }
            },
            "media_asset" => rsx! {
                icons::Image { class: "w-3.5 h-3.5 shrink-0 text-gray-500" }
            },
            "tour" => rsx! {
                icons::Cube { class: "w-3.5 h-3.5 shrink-0 text-gray-500" }
            },
            "dashboard" => rsx! {
                icons::Gauge { class: "w-3.5 h-3.5 shrink-0 text-gray-500" }
            },
            "flow" => rsx! {
                icons::Workflow { class: "w-3.5 h-3.5 shrink-0 text-gray-500" }
            },
            "map_pin" => rsx! {
                icons::MapPin { class: "w-3.5 h-3.5 shrink-0 text-gray-500" }
            },
            _ => rsx! {
                icons::Package { class: "w-3.5 h-3.5 shrink-0 text-gray-500" }
            },
        }
    }
}

/// D43 placement card (moved from map.rs) — slug, type, inline editing of
/// the "location detail". Detaching lives at the row level (Detach
/// button, D43 amendment 2026-09-13); the card only carries the editing.
#[component]
fn DevicePlacementRow(
    placement: DevicePlacement,
    device_type: Option<String>,
    on_save_location: EventHandler<(i64, String)>,
) -> Element {
    let mut editing = use_signal(|| false);
    let mut value = use_signal(|| placement.location_detail.clone().unwrap_or_default());
    let subtitle = device_type.unwrap_or_else(|| t!("poi-device-kind").to_string());
    let placement_id = placement.id;
    let slug = placement.device_id.clone();
    let location = placement.location_detail.clone();
    rsx! {
        div { class: "px-3 py-2 bg-gray-50 rounded-lg",
            div { class: "flex items-center gap-2",
                icons::Cpu { class: "h-4 w-4 text-gray-500 shrink-0" }
                div { class: "flex-1 min-w-0",
                    div { class: "text-sm font-medium text-gray-900 truncate", "{slug}" }
                    div { class: "text-xs text-gray-500", "{subtitle}" }
                }
            }
            if editing() {
                div { class: "flex items-center gap-1 mt-1.5",
                    input {
                        class: "flex-1 min-w-0 px-2 py-1 text-xs border border-gray-300 rounded focus:outline-none focus:ring-1 focus:ring-signal-400",
                        placeholder: t!("poi-device-location-hint"),
                        value: "{value}",
                        autofocus: true,
                        oninput: move |e| value.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter {
                                on_save_location.call((placement_id, value()));
                                editing.set(false);
                            }
                        },
                    }
                    button {
                        class: "text-xs font-semibold text-signal-700 hover:text-signal-800 shrink-0",
                        onclick: move |_| {
                            on_save_location.call((placement_id, value()));
                            editing.set(false);
                        },
                        {t!("viz-save")}
                    }
                }
            } else {
                button {
                    class: "flex items-center gap-1 mt-1.5 text-xs text-gray-500 hover:text-signal-700 transition-colors",
                    title: t!("poi-device-location-hint"),
                    onclick: move |_| editing.set(true),
                    icons::MapPin { class: "h-3 w-3 shrink-0" }
                    span {
                        if let Some(loc) = location.clone() {
                            "{loc}"
                        } else {
                            span { class: "italic text-gray-500", {t!("poi-device-location-add")} }
                        }
                    }
                }
            }
        }
    }
}
