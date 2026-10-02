//! Arbre de dossiers du drawer POI — dossiers D42 (`map_pin → folder`,
//! containment) contenant les objets attachés (arêtes `placed_on` pour
//! média/tour/dashboard à la racine du POI ; containment pour tout objet
//! rangé dans un dossier ; placements D43 pour les devices, dont le
//! « location detail » reste éditable dans la ligne de l'arbre).
//!
//! Drag & drop à la poignée ⠿ — école pointer-events du dashboard_editor
//! (`drag_template`) : pointerdown saisit, pointerup sur un dossier ou la
//! zone racine dépose. Pas de drag HTML5 (touch hostile) ; pas de
//! réordonnancement fin (`sort_key`, V1).
//!
//! Modèle de représentation :
//! - objet à la racine du POI = arête `placed_on` (média/tour/dashboard) ou
//!   placement D43 (device) — inchangé ;
//! - objet dans un dossier = ligne containment + arête supprimée (le label
//!   est recréé au retour à la racine) ;
//! - dossier racine = parent `map_pin` ; imbriqué = parent folder.
//!
//! Les noms d'affichage de tous les kinds sont hydratés par le backend
//! (`RefNode.name`) — pas de cache de noms côté client.

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

/// Marche containment depuis le POI : dossiers racine puis sous-dossiers
/// (boucle explicite — pas de récursion async). N+1 accepté (dossiers rares).
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

/// Exécute un dépôt drag & drop. `Ok(true)` = rangé (toast « in »),
/// `Ok(false)` = remonté à la racine (toast « out »).
async fn move_item(item: &DragItem, target: &DropId, poi_id: &str) -> Result<bool, ApiError> {
    match target {
        DropId::Folder(fid) => {
            if item.kind == "folder" {
                resources::set_parent("folder", &item.id, Some(("folder", fid))).await?;
            } else if item.kind == "device" {
                // L'id containment device = PK (registre), pas le slug.
                resources::set_parent("device", &item.id, Some(("folder", fid))).await?;
            } else {
                // Objet racine rangé : containment d'abord, puis l'arête
                // `placed_on` part (l'ordre ne perd rien en cas d'échec).
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
                // Objet rangé remonté : arête `placed_on` recréée D'ABORD
                // (409 = déjà là, succès — chemin idempotent), puis containment
                // détaché. L'ordre inverse (constat 2026-09-13) orphelinait
                // l'objet sur un échec de l'arête : parent containment vide +
                // aucune arête → invisible de tous les arbres, rechargement
                // compris. Un objet déjà racine est un no-op filtré en amont
                // (`from == target`).
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

/// `delete_edge` où le 404 est un succès (double-clic, autre onglet — D26).
async fn delete_edge_lenient(edge_id: i64) {
    match resources::delete_edge(edge_id).await {
        Ok(_) => {}
        Err(err) if err.status == Some(404) => {}
        Err(err) => toasts::error(format!("{err}")),
    }
}

/// `create_edge` où le doublon (409) est un succès — l'arête `placed_on`
/// visée existe déjà, exactement le but. Les chemins de réparation (retour
/// racine, éjection) restent ainsi idempotents : un re-dépôt ou un dépôt
/// interrompu ne bloque jamais l'objet (409 bloquant = re-déposer impossible).
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

/// Éjecte un objet rangé vers la racine du POI : arête `placed_on` recréée
/// avec son nom d'affichage PUIS containment détaché (edge-first — un échec
/// laisse l'objet rangé, jamais orphelin). Média/tour/dashboard uniquement
/// pour l'arête (seuls ces kinds portent `placed_on`) ; les autres kinds
/// (device rangé) ne subissent que le détachement de containment.
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

// ─────────────────────────── vues ───────────────────────────

/// Ligne prête à rendre : tout est résolu avant le rsx (école « rsx plat »).
/// Payloads préclonés pour les closures ('static FnMut, jamais de `let`
/// dans un `for` rsx).
struct TreeRow {
    key: String,
    depth: usize,
    is_folder: bool,
    expanded_now: bool,
    has_children: bool,
    name: String,
    emoji: Option<String>,
    /// Kind pour l'icône (`None` = dossier).
    icon_kind: Option<String>,
    /// device_type pour les lignes device.
    subtitle: Option<String>,
    is_device: bool,
    is_dead: bool,
    is_dragged: bool,
    /// Dossier : clé d'expansion.
    toggle: Option<String>,
    /// Poignée de drag.
    drag: Option<DragItem>,
    /// Dossier : cible de dépôt.
    drop_id: Option<DropId>,
    /// Objet aperçuable (média/tour/dashboard non mort) : cible du clic
    /// ligne → panneau d'aperçu.
    preview: Option<PreviewTarget>,
    /// Objet aperçuable : cible de l'étoile d'épinglage.
    pin_target: Option<PreviewTarget>,
    /// Copie disjointe de `preview` pour le handler Detach (nettoyage
    /// aperçu/épingle après détachement — captures disjointes par champ).
    clear_target: Option<PreviewTarget>,
    /// Ligne = objet actuellement affiché dans l'aperçu (surlignée).
    is_previewed: bool,
    /// Ligne = épingle courante (étoile remplie).
    is_pinned: bool,
    /// Objet : bouton Detach.
    detach: Option<Detach>,
    /// Dossier : (id, name) pour la suppression.
    delete: Option<(i64, String)>,
    /// Device : placement pour l'édition de localité.
    placement: Option<DevicePlacement>,
}

/// Contexte de construction des vues (données du drawer + état de drag).
struct ViewCtx<'a> {
    model: &'a TreeModel,
    expanded: &'a HashSet<String>,
    dragging: &'a Option<DragItem>,
    /// pk containment → (slug, device_type).
    device_by_pk: HashMap<String, (String, String)>,
    /// slug → placement (édition de localité dans l'arbre).
    placement_by_slug: HashMap<String, DevicePlacement>,
    /// Aperçu courant (surlignage de la ligne affichée).
    preview: &'a Option<PreviewTarget>,
    /// Épingle courante (étoile remplie).
    pinned: &'a Option<PreviewTarget>,
}

// ─────────────────────────── composant ───────────────────────────

#[component]
pub fn PoiTreeSection(
    poi_id: String,
    /// Arêtes `placed_on` du POI (poi_detail).
    links: Vec<VizLink>,
    /// Placements D43 du POI (poi_detail).
    placements: Vec<DevicePlacement>,
    /// Index org des devices : (pk containment, slug, device_type) —
    /// l'id containment device = PK entière, pas le slug.
    devices: Vec<(String, String, String)>,
    /// Version drawer (detail_version) — refetch de la marche containment.
    version: Signal<u64>,
    /// Objet affiché dans le panneau d'aperçu (bascule d'asset à asset).
    mut preview: Signal<Option<PreviewTarget>>,
    /// Épingle courante du POI (étoile remplie sur la ligne correspondante).
    mut pinned: Signal<Option<PreviewTarget>>,
    /// Clic ligne objet → ouvrir l'aperçu (read-only, panneau carte).
    on_preview: EventHandler<PreviewTarget>,
    /// Étoile ★ : pose ou retrait de l'épingle (toggle côté drawer).
    on_pin: EventHandler<PreviewTarget>,
    on_attach: EventHandler<()>,
    /// Refetch drawer + carte après mutation (idiome detail_version).
    on_mutated: EventHandler<()>,
) -> Element {
    let mut expanded = use_signal(HashSet::<String>::new);
    let mut dragging = use_signal(|| None::<DragItem>);
    let mut new_folder = use_signal(String::new);
    let mut pending_delete = use_signal(|| None::<(i64, String)>);

    // Marche containment, refetch à chaque bump de version (attach/detach/
    // move/save de localité — le drawer bumppe `detail_version`).
    let poi_id_for_walk = poi_id.clone();
    let model = use_resource(move || {
        let id = poi_id_for_walk.clone();
        let _v = version();
        async move { walk_containment(id).await }
    });

    // Déplié par défaut : racines au premier modèle (une seule fois —
    // l'utilisateur garde la main ensuite).
    let mut seeded = use_signal(|| false);
    use_effect(move || {
        if seeded() {
            return;
        }
        if let Some(Ok(model)) = &*model.value().read() {
            expanded.with_mut(|set| {
                for f in &model.root_folders {
                    set.insert(f.id.clone());
                }
            });
            seeded.set(true);
        }
    });

    // Snapshots possédés pour les closures 'static (une par closure — un
    // `move` s'approprie sa capture ; jamais d'emprunt de props).
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
    // Éjection d'un objet rangé (Détacher) → recrée l'arête sur CE POI.
    let poi_id_for_eject = poi_id.clone();

    // Dépôt d'un drag : gardes puis mutation + toast + refetch.
    // Callback = Copy → capturable par plusieurs handlers du rsx.
    let perform_drop = Callback::new(move |(item, target): (DragItem, DropId)| {
        // Garde cycle : un dossier ne se dépose pas dans lui-même ou un de
        // ses descendants (le serveur répondrait 400).
        if let (DropId::Folder(dst), "folder") = (&target, item.kind.as_str()) {
            let mut forbidden = descendants_of(&model_for_drop.children, &item.id);
            forbidden.insert(item.id.clone());
            if forbidden.contains(dst) {
                toasts::error(t!("poi-tree-cycle").to_string());
                return;
            }
        }
        // Dépôt identique → no-op silencieux.
        if item.from == target {
            return;
        }
        // Clones dans le corps : un `async move` qui déplace ses captures
        // hors de la closure la rendrait FnOnce (handlers = FnMut).
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

    // Suppression d'un dossier : sous-dossiers remontés sous le POI, objets
    // éjectés à la racine (arête recréée), puis delete_folder.
    let perform_delete = move |(fid, _name): (i64, String)| {
        // Clones dans le corps (voir perform_drop) — la closure reste FnMut.
        let model = model_for_delete.clone();
        let poi = poi_id_for_delete.clone();
        spawn(async move {
            let fid_string = fid.to_string();
            // 1. Sous-dossiers (toute profondeur) → racine du POI, sinon le
            //    re-root org-wide de delete_folder les rendrait invisibles.
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
            // 2. Objets **directs** éjectés à la racine (arête `placed_on`
            //    recréée avec leur nom) — les sous-dossiers remontés en 1
            //    gardent leur contenu (containment intact).
            if let Some(kids) = model.children.get(&fid_string) {
                for c in kids {
                    if c.kind == "folder" {
                        continue;
                    }
                    if c.kind == "device" {
                        // Device : containment détaché, placement intact.
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
            // 3. Le dossier lui-même.
            match resources::delete_folder(fid).await {
                Ok(_) => toasts::success(t!("poi-tree-folder-deleted").to_string()),
                Err(err) => toasts::error(format!("{err}")),
            }
            on_mutated.call(());
        });
    };

    // Création inline d'un dossier racine du POI.
    // Callback = Copy → capturable par onclick ET onkeydown.
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

    // Détachement unifié (arête / objet rangé / placement) — un seul Callback
    // (Copy) réutilisable par les deux branches du rendu (div device / bouton
    // objet). Objet rangé : Détacher = éjection vers la racine du POI (arête
    // `placed_on` recréée — un détachement pur laissait l'objet orphelin,
    // invisible de tous les arbres, constat 2026-09-13) ; device rangé :
    // éjection du dossier seule (placement intact, amendement D43) ; device
    // racine : le placement part. Les clones nécessaires au `spawn` sont
    // faits dans le corps — un Callback possède ses captures, un clone par
    // appel, jamais FnOnce.
    let perform_detach = Callback::new(
        move |(detach, target, name): (Detach, Option<PreviewTarget>, String)| {
            let poi = poi_id_for_eject.clone();
            spawn(async move {
                match detach {
                    Detach::Placement(placement_id) => {
                        // 404 = déjà détaché (autre onglet) — succès lenient.
                        match viz::delete_poi_placement(placement_id).await {
                            Ok(_) => {}
                            Err(err) if err.status == Some(404) => {}
                            Err(err) => toasts::error(format!("{err}")),
                        }
                    }
                    Detach::Contained { kind, id } => {
                        if kind == "device" {
                            // Device rangé : containment seul part, le placement
                            // reste (le device redevient un device racine).
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
                // Aperçu/épingle pointaient sur l'objet détaché → nettoyage
                // local immédiat.
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

    // ── vues précalculées (jamais de `let` dans un `for` rsx) ──
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
            // Toute relâche de pointeur hors d'une cible annule le drag.
            onpointerup: move |_| {
                if dragging().is_some() {
                    dragging.set(None);
                }
            },
            // Actions : compteur + ＋ dossier + ＋ objet
            div { class: "flex items-center justify-between gap-2",
                span { class: "text-xs text-gray-400", {t!("poi-tree-count", count : total)} }
                div { class: "flex gap-1",
                    button {
                        class: "px-2 py-1 text-xs font-medium text-blue-700 bg-blue-50 hover:bg-blue-100 rounded transition-colors",
                        onclick: move |_| create_folder_now(()),
                        {t!("poi-tree-add-folder")}
                    }
                    button {
                        class: "px-2 py-1 text-xs font-medium text-blue-700 bg-blue-50 hover:bg-blue-100 rounded transition-colors",
                        onclick: move |_| on_attach.call(()),
                        {t!("poi-attachment-attach")}
                    }
                }
            }
            // Création inline d'un dossier racine
            div { class: "flex gap-1",
                input {
                    class: "flex-1 min-w-0 px-2 py-1 text-xs border border-gray-300 rounded focus:outline-none focus:ring-1 focus:ring-blue-400",
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
            // Lignes d'arbre
            for row in views {
                if row.is_device {
                    // Device : pas un bouton (édition de localité dedans) —
                    // poignée + carte placement D43.
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
                                            // Chaîne vide = effacement (null).
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
                            }
                        }
                        // Détacher : device racine = placement part (device
                        // libre, amendement D43) ; device rangé = éjection du
                        // dossier (même sémantique que les autres objets).
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
                        class: if row.is_dragged { "w-full text-left px-2 py-1.5 flex items-center gap-1 rounded opacity-40 transition-colors" } else if row.is_folder { "w-full text-left px-2 py-1.5 flex items-center gap-1 rounded hover:bg-blue-50 transition-colors" } else if row.is_previewed { "w-full text-left px-2 py-1.5 flex items-center gap-1 rounded bg-blue-50 transition-colors" } else { "w-full text-left px-2 py-1.5 flex items-center gap-1 rounded hover:bg-gray-50 transition-colors" },
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
                            // Dossier : (dé)pliage ; objet aperçuable : bascule
                            // du panneau d'aperçu (jamais de popup ni route).
                            if let Some(fid) = row.toggle.clone() {
                                let mut set = expanded.write();
                                if !set.remove(&fid) {
                                    set.insert(fid);
                                }
                            } else if let Some(target) = row.preview.clone() {
                                on_preview.call(target);
                            }
                        },
                        span { class: "select-none shrink-0 text-gray-400 w-3",
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
                                icons::Folder { class: "w-3.5 h-3.5 shrink-0 text-gray-400" }
                            }
                        } else {
                            KindIcon { kind: row.icon_kind.clone().unwrap_or_default() }
                        }
                        span { class: if row.is_dead { "truncate text-xs text-gray-400 line-through flex-1" } else if row.is_folder { "truncate text-sm font-medium text-gray-900 flex-1" } else { "truncate text-sm text-gray-700 flex-1" },
                            {row.name.clone()}
                        }
                        if let Some(sub) = &row.subtitle {
                            span { class: "shrink-0 text-[10px] text-gray-400", {sub.clone()} }
                        }
                        // Objet aperçuable : étoile d'épinglage (aperçu par
                        // défaut à l'ouverture du POI — premier objet attaché
                        // épinglé par le drawer).
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
                        // Objet : Detach
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
                        // Dossier : suppression
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
            // Zone de dépôt racine
            div {
                class: "flex items-center gap-1 px-2 py-2 rounded border border-dashed border-gray-200 hover:bg-blue-50 transition-colors",
                onpointerup: move |e| {
                    e.stop_propagation();
                    if let Some(item) = dragging() {
                        dragging.set(None);
                        perform_drop((item, DropId::Root));
                    }
                },
                icons::Package { class: "w-3 h-3 shrink-0 text-gray-400" }
                span { class: "text-xs text-gray-400", {t!("poi-tree-root-zone")} }
            }
            // Confirmation de suppression de dossier
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
        }
    }
}

/// Icône par kind (dossiers gérés à part avec emoji).
#[component]
pub fn KindIcon(kind: String) -> Element {
    rsx! {
        match kind.as_str() {
            "device" => rsx! {
                icons::Cpu { class: "w-3.5 h-3.5 shrink-0 text-gray-400" }
            },
            "media_asset" => rsx! {
                icons::Image { class: "w-3.5 h-3.5 shrink-0 text-gray-400" }
            },
            "tour" => rsx! {
                icons::Cube { class: "w-3.5 h-3.5 shrink-0 text-gray-400" }
            },
            "dashboard" => rsx! {
                icons::Gauge { class: "w-3.5 h-3.5 shrink-0 text-gray-400" }
            },
            "flow" => rsx! {
                icons::Workflow { class: "w-3.5 h-3.5 shrink-0 text-gray-400" }
            },
            "map_pin" => rsx! {
                icons::MapPin { class: "w-3.5 h-3.5 shrink-0 text-gray-400" }
            },
            _ => rsx! {
                icons::Package { class: "w-3.5 h-3.5 shrink-0 text-gray-400" }
            },
        }
    }
}

/// Carte placement D43 (déplacée de map.rs) — slug, type, édition inline du
/// « location detail ». Le détachement vit au niveau de la ligne (bouton
/// Détacher, amendement D43 2026-09-13) ; la carte ne porte que l'édition.
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
                    div { class: "text-xs text-gray-400", "{subtitle}" }
                }
            }
            if editing() {
                div { class: "flex items-center gap-1 mt-1.5",
                    input {
                        class: "flex-1 min-w-0 px-2 py-1 text-xs border border-gray-300 rounded focus:outline-none focus:ring-1 focus:ring-blue-400",
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
                        class: "text-xs font-semibold text-blue-700 hover:text-blue-800 shrink-0",
                        onclick: move |_| {
                            on_save_location.call((placement_id, value()));
                            editing.set(false);
                        },
                        {t!("viz-save")}
                    }
                }
            } else {
                button {
                    class: "flex items-center gap-1 mt-1.5 text-xs text-gray-500 hover:text-blue-700 transition-colors",
                    title: t!("poi-device-location-hint"),
                    onclick: move |_| editing.set(true),
                    icons::MapPin { class: "h-3 w-3 shrink-0" }
                    span {
                        if let Some(loc) = location.clone() {
                            "{loc}"
                        } else {
                            span { class: "italic text-gray-400", {t!("poi-device-location-add")} }
                        }
                    }
                }
            }
        }
    }
}
