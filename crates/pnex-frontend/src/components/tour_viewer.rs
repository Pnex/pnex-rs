//! Viewer de tour en lecture seule : une seule scène montée à la fois
//! (S13), navigation par hotspots (polling `take_nav`) + sélecteur
//! d'étages + mini-plan. Utilisé par l'aperçu de l'éditeur (octets via
//! chemins authentifiés) et par la page publique `/share/:token` (octets
//! via l'endpoint public, sans auth).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{ResolvedAnnotationItem, TourDoc};
use std::collections::HashMap;

use crate::components::annotation_editor::popover::AnnotationPopover;
use crate::components::modal::Modal;
use crate::tour_viewer::{AnnotMarkerView, TourHotspotView, TourNav, TourSceneView};

/// Source des octets d'assets (path builder).
#[derive(Clone, PartialEq)]
pub enum ViewerSource {
    /// In-app : `/api/v1/media/{id}/content` (version courante,
    /// authentifié — même chemin que la page Média).
    Auth,
    /// Public : `/api/v1/public/tours/{token}/assets/{id}?v=n`.
    Public { token: String },
}

/// Infos par asset référencé (version + mime connus côté public ;
/// `None` côté in-app — le contenu courant est servi).
#[derive(Clone, Default, PartialEq)]
pub struct AssetInfo {
    pub version: Option<i64>,
    pub content_type: Option<String>,
}

/// Host div **par défaut** du viewer pannellum — une instance donnée doit
/// avoir un host unique (pont JS par id DOM) : les nouveaux appelaires
/// passent leur propre `host_id` (ex. aperçu POI).
pub const HOST_ID: &str = "pnex-tour-viewer-host";

/// Bouton d'étage : (étage, actif, première scène de l'étage — cible).
type FloorButton = (pnex_core::TourFloor, bool, Option<(String, String)>);

#[component]
pub fn TourViewer(
    doc: TourDoc,
    assets: HashMap<String, AssetInfo>,
    source: ViewerSource,
    /// Drag d'un hotspot en preview : (link_id, yaw, pitch) — l'éditeur
    /// patche son doc ; la page share pose un no-op.
    on_hotspot_move: Callback<(String, f64, f64)>,
    /// Id DOM du host pannellum — unique par instance (pont JS par id).
    host_id: String,
    /// Compact mode (POI preview): fills the parent (`flex-1 min-h-0`,
    /// full-frame panel over the map) instead of `h-[70vh]` — the pannellum
    /// host stays absolutely positioned (height:100% trap unchanged).
    compact: bool,
    /// Overlay d'annotations (D58) : fetch du read model + marqueurs. La
    /// page share publique pose `false` (les annotations peuvent révéler
    /// des données device).
    annotations_enabled: bool,
    /// Changement de scène courante (id) — l'éditeur d'annotations suit le
    /// média affiché ; les autres appelaires posent un no-op.
    on_scene_change: Callback<String>,
    /// Panneau latéral (étages/mini-plan/scènes) — masqué sur la page
    /// annotations pour un média simple (pas de chrome de tour à montrer).
    show_side_panel: bool,
    /// Gated marker (arrow) drag: true only in the tour editor preview —
    /// read-only viewers (map POI preview, share page, annotation page)
    /// keep markers fixed. Pushed to the JS host guard at event time.
    editable: bool,
) -> Element {
    // Scène courante : départ explicite, sinon première scène.
    let mut current = use_signal(|| {
        doc.start_scene
            .clone()
            .or_else(|| doc.scenes.first().map(|s| s.id.clone()))
            .unwrap_or_default()
    });
    let mut active_floor = use_signal(|| {
        doc.scenes
            .iter()
            .find(|s| s.id == current.cloned())
            .map(|s| s.floor_id.clone())
            .or_else(|| doc.floors.first().map(|f| f.id.clone()))
            .unwrap_or_default()
    });
    let mut nav_seq = use_signal(|| 0u64);
    let mut move_seq = use_signal(|| 0u64);
    // Garde du poll (une seule boucle par viewer) — l'écriture se fait dans
    // un effet, jamais au rendu.
    let mut poll_started = use_signal(|| false);
    let mut mounted = use_signal(|| false);
    // Garde d'opération en cours (mount) + dernière (scène, url) appliquée —
    // sérialisation des opérations viewer, cf. commentaire dans l'effet.
    let mut mounting = use_signal(|| false);
    let mut applied = use_signal(|| None::<(String, String)>);
    let mut load_failed = use_signal(|| false);

    // ─── Annotations (D58) : toggle, items du read model, popover ───
    // Toggle « Annotations » (on par défaut) ; la boucle de poll du viewer
    // est étendue à `take_annot_click` (seq monotone) ; l'application des
    // marqueurs est un **effet séparé** du mount (la clé de dé-dup du mount
    // reste (scène, url) — JAMAIS les annotations, sinon re-mount à chaque
    // édition).
    let mut annot_on = use_signal(|| true);
    let mut annot_seq = use_signal(|| 0u64);
    let mut annot_selected = use_signal(|| None::<ResolvedAnnotationItem>);
    // Dernier lot de marqueurs appliqué (scène, url, json) — dé-dup des
    // double invocations d'effects ; la scène + url viennent de `applied`
    // (le mount/switch doit être TERMINÉ avant de poser les marqueurs).
    let mut applied_annot = use_signal(|| None::<(String, String, String)>);

    // URL blob/data-URI du panorama de la scène courante (fetch côté Rust) —
    // la resource relit un **Memo** (asset de la scène courante) : une capture
    // en valeur ne redémarre jamais la resource (piège dioxus #2784, cf.
    // app.rs) → le panorama ne changeait jamais de scène.
    let doc_for_memo = doc.clone();
    let scene_asset = use_memo(move || {
        let doc = doc_for_memo.clone();
        let current = current.cloned();
        doc.scenes
            .iter()
            .find(|s| s.id == current)
            .map(|s| s.media_asset_id.clone())
    });
    let source_for_url = source.clone();
    let assets_for_url = assets.clone();
    let url = use_resource(move || {
        let asset_id = scene_asset.read().clone();
        let source = source_for_url.clone();
        let assets = assets_for_url.clone();
        async move {
            let asset_id = asset_id?;
            let info = assets.get(&asset_id).cloned().unwrap_or_default();
            let path = match &source {
                ViewerSource::Auth => crate::api::media::content_path(&asset_id),
                ViewerSource::Public { token } => {
                    crate::api::tours::public_asset_path(token, &asset_id, info.version)
                }
            };
            crate::util::media_blob_url(&path, info.content_type.as_deref()).await
        }
    });

    // Read model annotations de l'asset affiché — la resource relit le
    // Memo `scene_asset` DANS la closure (piège dioxus #2784) : suit les
    // changements de scène. Erreurs avalées → overlay vide (S7).
    // Désactivé par le prop (page share : annotations_enabled = false).
    let annotations = use_resource(move || {
        let asset_id = scene_asset.read().clone();
        let enabled = annotations_enabled;
        async move {
            if !enabled {
                return None;
            }
            let asset_id = asset_id?;
            crate::api::annotation_layers::media_annotations(&asset_id)
                .await
                .ok()
        }
    });

    // Plan de l'étage actif pour la mini-map (blob authentifié, même chaîne
    // que le panorama) — Memo lu DANS la closure de la resource (piège
    // dioxus #2784) : suit les changements d'étage.
    let doc_for_plan = doc.clone();
    let active_plan = use_memo(move || {
        let doc = doc_for_plan.clone();
        let floor_id = active_floor.cloned();
        doc.floors
            .iter()
            .find(|f| f.id == floor_id)
            .and_then(|f| f.plan.as_ref())
            .map(|p| p.media_asset_id.clone())
    });
    let plan_url = use_resource(move || {
        let asset_id = active_plan.read().clone();
        let source = source.clone();
        let assets = assets.clone();
        async move {
            let asset_id = asset_id?;
            let info = assets.get(&asset_id).cloned().unwrap_or_default();
            let path = match &source {
                ViewerSource::Auth => crate::api::media::content_path(&asset_id),
                ViewerSource::Public { token } => {
                    crate::api::tours::public_asset_path(token, &asset_id, info.version)
                }
            };
            crate::util::media_blob_url(&path, info.content_type.as_deref()).await
        }
    });

    // Montage / switch quand l'URL est prête (une seule texture en mémoire).
    // Le doc est cloné par invocation (closure FnMut — jamais de move du
    // prop capturé).
    let doc_for_effect = doc.clone();
    let host_for_effect = host_id.clone();
    use_effect(move || {
        let ready = url.value().read().clone().flatten();
        let Some(url_value) = ready else {
            return;
        };
        let doc = doc_for_effect.clone();
        let Some(scene) = doc
            .scenes
            .iter()
            .find(|s| s.id == current.cloned())
            .cloned()
        else {
            return;
        };
        // Hotspots : liens sortants de la scène dont la cible existe
        // (référence morte → hotspot simplement absent, jamais de panic).
        let hotspots: Vec<TourHotspotView> = doc
            .links
            .iter()
            .filter(|link| link.from == scene.id)
            .filter_map(|link| {
                let target = doc.scenes.iter().find(|s| s.id == link.to)?;
                Some(TourHotspotView {
                    yaw: link.yaw,
                    pitch: link.pitch,
                    target_scene: link.to.clone(),
                    target_floor: target.floor_id.clone(),
                    label: link.label.clone().unwrap_or_else(|| target.label.clone()),
                    kind: link.kind.clone(),
                    link_id: link.id.clone(),
                })
            })
            .collect();
        let view = TourSceneView {
            scene_id: scene.id.clone(),
            floor_id: scene.floor_id.clone(),
            url: url_value.clone(),
            yaw: scene.initial_yaw,
            pitch: scene.initial_pitch,
            hfov: scene.initial_fov,
            hotspots,
        };
        // Dé-dup : la même (scène, url) déjà appliquée ne re-déclenche pas
        // d'opération (la double invocation des effects de dioxus + les
        // re-rendus sans changement réel sinon enchaînent destroy/re-création
        // pour rien — et une opération pendant que pannellum charge encore sa
        // texture laisse le viewer vide).
        let applied_key = (scene.id.clone(), url_value.clone());
        if applied.cloned().as_ref() == Some(&applied_key) {
            return;
        }
        if !mounted() {
            // Un seul mount à la fois (garde `mounting`) ; `mounted` n'est
            // posé qu'après le succès — un changement de scène pendant les
            // retries du mount est ré-appliqué par l'effet quand `mounting`
            // retombe (l'effet lit les deux signaux), pas de switch concurrent.
            if mounting() {
                return;
            }
            mounting.set(true);
            let host = host_for_effect.clone();
            let view = view.clone();
            spawn(async move {
                let ok = crate::tour_viewer::mount(&host, &view).await;
                mounting.set(false);
                load_failed.set(!ok);
                if ok {
                    mounted.set(true);
                    applied.set(Some(applied_key));
                }
            });
        } else {
            // Switch avec retry (comme le mount) ; échec total = badge —
            // plus de viewer vide sans retour, et un nav suivant retente
            // (load_failed = !ok s'efface au prochain succès).
            let host = host_for_effect.clone();
            let view = view.clone();
            spawn(async move {
                let ok = crate::tour_viewer::switch_scene(&host, &view).await;
                load_failed.set(!ok);
                if ok {
                    applied.set(Some(applied_key));
                }
            });
        }
    });

    // Polling des événements viewer (nav hotspot + drag de flèche) : une
    // boucle unique, démarrée une fois par effet — l'ancien pattern
    // « signal écrit au rendu pour se relancer » est mort en dioxus 0.7
    // (écriture au rendu jetée → boucle tuée au premier tick, constat
    // 2026-09-12 : les clics de hotspots ne faisaient plus rien). La tâche
    // est scopée au composant : tuée à la fermeture de la modal.
    use_effect(move || {
        if poll_started() {
            return;
        }
        poll_started.set(true);
        spawn(async move {
            loop {
                crate::util::sleep(std::time::Duration::from_millis(250)).await;
                if let Some(TourNav {
                    seq,
                    scene_id,
                    floor_id,
                }) = crate::tour_viewer::take_nav(nav_seq.cloned()).await
                {
                    nav_seq.set(seq);
                    // Changement de scène : la popover d'un item de l'ancienne
                    // scène n'a plus de sens.
                    annot_selected.set(None);
                    current.set(scene_id);
                    active_floor.set(floor_id);
                }
                // Drag d'une flèche : remonter (link_id, yaw, pitch) au parent.
                if let Some(mv) = crate::tour_viewer::take_hotspot_move(move_seq.cloned()).await {
                    move_seq.set(mv.seq);
                    on_hotspot_move.call((mv.link_id, mv.yaw, mv.pitch));
                }
                // Clic d'un marqueur d'annotation (D58) : ouvrir la popover
                // avec l'item résolu (lookup par id dans le lot courant).
                if annotations_enabled {
                    if let Some(click) =
                        crate::tour_viewer::take_annot_click(annot_seq.cloned()).await
                    {
                        annot_seq.set(click.seq);
                        let found = annotations
                            .value()
                            .read()
                            .as_ref()
                            .and_then(|o| o.as_ref())
                            .and_then(|a| {
                                a.items.iter().find(|it| it.id == click.item_id).cloned()
                            });
                        annot_selected.set(found);
                    }
                }
            }
        });
    });

    // Démontage du viewer (école MediaPreview use_drop).
    let host_for_drop = host_id.clone();
    use_drop(move || {
        crate::tour_viewer::unmount(&host_for_drop);
    });

    // Arrow-drag gate: push the editable flag to the JS host guard (read at
    // event time, so the flag can land before or after the first mount —
    // bridge retries while the bundle/host settle).
    let host_for_editable = host_id.clone();
    use_effect(move || {
        let host = host_for_editable.clone();
        spawn(async move {
            crate::tour_viewer::set_tour_editable(&host, editable).await;
        });
    });

    // Notifier la scène courante (scène INITIALE incluse — sans ça
    // l'éditeur d'annotations ignore le média de départ et la pose
    // échoue silencieusement jusqu'à la première navigation, constat
    // 2026-09-16). Source unique : remplace les appels dispersés dans
    // les handlers de nav/boutons.
    use_effect(move || {
        let scene_id = current.cloned();
        if scene_id.is_empty() {
            return;
        }
        on_scene_change.call(scene_id);
    });

    // Application des marqueurs (D58) : effet **séparé** du mount — il
    // n'agit qu'après la fin du mount/switch (`applied` == clé courante),
    // dé-dup par (scène, url, json). `set_annotations` = remove/add de
    // hotspots post-mount, jamais de re-mount.
    let annot_enabled_for_effect = annotations_enabled;
    let host_for_annot = host_id.clone();
    use_effect(move || {
        if !annot_enabled_for_effect {
            return;
        }
        let Some((scene_id, url_key)) = applied.cloned() else {
            return;
        };
        let items = annotations
            .value()
            .read()
            .as_ref()
            .and_then(|o| o.as_ref())
            .map(|a| a.items.clone())
            .unwrap_or_default();
        let markers: Vec<AnnotMarkerView> = if annot_on() {
            items
                .iter()
                .filter_map(|it| match &it.geometry {
                    pnex_core::AnnotationGeometry::Equirect { yaw, pitch } => {
                        Some(AnnotMarkerView {
                            id: it.id.clone(),
                            yaw: *yaw,
                            pitch: *pitch,
                            kind: it.kind.clone(),
                            label: it.label.clone(),
                        })
                    }
                    _ => None,
                })
                .collect()
        } else {
            Vec::new()
        };
        let json = serde_json::to_string(&markers).unwrap_or_default();
        let key = (scene_id, url_key, json.clone());
        if applied_annot.cloned().as_ref() == Some(&key) {
            return;
        }
        applied_annot.set(Some(key));
        let host = host_for_annot.clone();
        spawn(async move {
            crate::tour_viewer::set_annotations(&host, &markers, false).await;
        });
    });

    // ─── Précalculs (pas de `let` dans le corps rsx) ───
    let current_id = current.cloned();
    let active_floor_id = active_floor.cloned();
    let floor_scenes: Vec<pnex_core::TourScene> = doc
        .scenes
        .iter()
        .filter(|s| s.floor_id == active_floor_id)
        .cloned()
        .collect();
    let floors: Vec<pnex_core::TourFloor> = doc.floors.clone();
    let (plan_w, plan_h) = match doc
        .floors
        .iter()
        .find(|f| f.id == active_floor_id)
        .and_then(|f| f.plan.as_ref())
    {
        Some(plan) => (plan.width.unwrap_or(2000.0), plan.height.unwrap_or(1000.0)),
        None => (2000.0, 1000.0),
    };
    // (étage, actif, première scène de l'étage — cible du bouton).
    let floor_buttons: Vec<FloorButton> = floors
        .iter()
        .map(|floor| {
            let first = doc
                .scenes
                .iter()
                .find(|s| s.floor_id == floor.id)
                .map(|s| (s.id.clone(), floor.id.clone()));
            (floor.clone(), *floor.id == active_floor_id, first)
        })
        .collect();
    // Mini-plan : (id, cx, cy, courante) en viewBox 200×100.
    let mini_dots: Vec<(String, f64, f64, bool)> = floor_scenes
        .iter()
        .map(|s| {
            (
                s.id.clone(),
                s.x / plan_w * 200.0,
                s.y / plan_h * 100.0,
                s.id == current_id,
            )
        })
        .collect();
    // (scène, courante) pour la liste cliquable.
    let scene_buttons: Vec<(pnex_core::TourScene, bool)> = floor_scenes
        .iter()
        .map(|s| (s.clone(), s.id == current_id))
        .collect();
    // Image du plan derrière les dots (None tant que le blob n'est pas prêt).
    let mini_plan_url = plan_url.value().read().clone().flatten();
    // Annotations : toggle rendu si ≥ 1 item sur le média affiché ; item
    // sélectionné pour la popover.
    let has_items = annotations
        .value()
        .read()
        .as_ref()
        .and_then(|o| o.as_ref())
        .is_some_and(|a| !a.items.is_empty());
    let annot_selected_now = annot_selected.cloned();
    // Control / reading items of the shown media: operable and readable in
    // a side panel on the right (D129), next to their markers.
    let surface_items: Vec<ResolvedAnnotationItem> = annotations
        .value()
        .read()
        .as_ref()
        .and_then(|o| o.as_ref())
        .map(|a| {
            a.items
                .iter()
                .filter(|i| crate::components::surface::annotation::is_surface_item(i))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let surface_key = surface_items
        .iter()
        .map(|i| i.id.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let show_surface = annotations_enabled && annot_on() && !surface_items.is_empty();

    rsx! {
        div { class: if compact { "flex w-full gap-3 flex-1 min-h-0" } else { "flex w-full gap-3 h-[70vh]" },
            // Side panel: floors + mini-plan + scenes (driven by
            // show_side_panel, including in compact mode).
            if show_side_panel {
                div { class: "w-60 shrink-0 flex flex-col gap-2 overflow-y-auto",
                    div {
                        span { class: "text-xs font-semibold text-gray-500 uppercase tracking-wide",
                            {t!("tour-viewer-floors")}
                        }
                        div { class: "flex flex-col gap-1 mt-1",
                            for (floor, is_active, target) in floor_buttons.clone() {
                                button {
                                    key: "{floor.id}",
                                    class: if target.is_none() { "w-full text-left px-2 py-1.5 text-sm rounded-lg bg-gray-50 text-gray-400 border border-transparent cursor-not-allowed" } else if is_active { "w-full text-left px-2 py-1.5 text-sm rounded-lg bg-blue-50 text-blue-700 border border-blue-200" } else { "w-full text-left px-2 py-1.5 text-sm rounded-lg hover:bg-gray-50 text-gray-700 border border-transparent" },
                                    // Un étage sans scène n'a rien à montrer : no-op
                                    // silencieux = confusion (constat 2026-09-16) —
                                    // visuellement désactivé.
                                    disabled: target.is_none(),
                                    onclick: move |_| {
                                        if let Some((scene_id, floor_id)) = target.clone() {
                                            current.set(scene_id);
                                            active_floor.set(floor_id);
                                        }
                                    },
                                    {if floor.name.is_empty() { floor.id.clone() } else { floor.name.clone() }}
                                }
                            }
                        }
                    }
                    // Mini-plan : image du plan + positions des scènes (les dots
                    // partagent le mapping x/plan_w*200, y/plan_h*100 du viewBox).
                    if !mini_dots.is_empty() {
                        div { class: "bg-white border border-gray-200 rounded-lg p-1",
                            svg { view_box: "0 0 200 100", class: "w-full",
                                if let Some(url) = mini_plan_url {
                                    image {
                                        href: "{url}",
                                        x: "0",
                                        y: "0",
                                        width: "200",
                                        height: "100",
                                        preserve_aspect_ratio: "none",
                                        "pointer-events": "none",
                                    }
                                }
                                for (id, dot_x, dot_y, is_current) in mini_dots.clone() {
                                    circle {
                                        key: "mini-{id}",
                                        cx: "{dot_x}",
                                        cy: "{dot_y}",
                                        r: if is_current { "5" } else { "3" },
                                        fill: if is_current { "#2563eb" } else { "#94a3b8" },
                                        "pointer-events": "none",
                                    }
                                }
                            }
                        }
                    }
                    // Liste des scènes cliquables de l'étage.
                    ul { class: "divide-y divide-gray-100",
                        for (scene, is_current) in scene_buttons.clone() {
                            li { key: "list-{scene.id}",
                                button {
                                    class: if is_current { "w-full text-left px-2 py-1.5 text-sm bg-blue-50 text-blue-700 rounded-lg" } else { "w-full text-left px-2 py-1.5 text-sm hover:bg-gray-50 text-gray-700 rounded-lg" },
                                    onclick: move |_| {
                                        current.set(scene.id.clone());
                                        active_floor.set(scene.floor_id.clone());
                                    },
                                    {if scene.label.is_empty() { scene.id.clone() } else { scene.label.clone() }}
                                }
                            }
                        }
                    }
                }
            }

            // Viewer host: relative wrapper + absolutely positioned host
            // (pannellum trap — MediaPreview school: .pnlm-container sets
            // height:100% after Tailwind; never size the host with a class).
            div { class: "flex-1 relative min-w-0",
                // Toggle « Annotations » (D58) : rendu si le média affiché
                // porte ≥ 1 item ; on par défaut ; ferme la popover à l'off.
                if has_items && annotations_enabled {
                    button {
                        class: "absolute top-2 left-2 z-20 px-3 py-1.5 text-xs font-medium rounded-lg bg-white/90 text-gray-700 border border-gray-200 shadow-sm hover:bg-white",
                        onclick: move |_| {
                            annot_on.toggle();
                            annot_selected.set(None);
                        },
                        {t!("annot-toggle")}
                    }
                }
                if load_failed() {
                    div {
                        class: "flex items-center justify-center bg-gray-100 rounded-lg",
                        style: "height: 320px;",
                        p { class: "text-sm text-gray-400", {t!("tour-viewer-unavailable")} }
                    }
                }
                div {
                    id: "{host_id}",
                    style: "position: absolute; inset: 0; height: 100%;",
                }
                // Popover live (D58) : item cliqué, hors canvas (panneau
                // dioxus), jamais bloquante.
                if let Some(item) = annot_selected_now {
                    div { key: "annot-pop-{item.id}",
                        AnnotationPopover {
                            item,
                            on_close: move |_| annot_selected.set(None),
                        }
                    }
                }
            }
            if show_surface {
                // Keyed on the block root: dioxus drops a key on a non-root
                // node, so the surface remounts when the item set changes.
                div {
                    key: "{surface_key}",
                    class: "w-64 shrink-0 space-y-2 overflow-y-auto",
                    span { class: "text-xs font-semibold uppercase tracking-wide text-gray-500",
                        {t!("annot-surface-title")}
                    }
                    crate::components::surface::annotation::AnnotationSurface { items: surface_items.clone() }
                }
            }
        }
    }
}

// ───────────────────────── TourViewerModal (map drawer) ─────────────────────────

/// Aperçu autonome d'un tour depuis le drawer POI — charge le doc de la
/// DERNIÈRE version authentifié (`ViewerSource::Auth`, le contenu courant
/// est servi ; la version publiée n'a de sens que pour le lien public).
/// Hotspots en lecture : le drag est sans effet (`on_hotspot_move` no-op,
/// école page `/share`).
#[component]
pub fn TourViewerModal(tour_id: String, on_close: Callback<()>) -> Element {
    let id_for_res = tour_id.clone();
    let detail = use_resource(move || {
        let id = id_for_res.clone();
        async move { crate::api::tours::detail(&id).await }
    });

    rsx! {
        Modal {
            title: match detail.value().read().as_ref() {
                Some(Ok(d)) => d.name.clone(),
                _ => t!("poi-picker-tab-tour").to_string(),
            },
            max_width: "max-w-5xl".to_string(),
            on_close,
            match &*detail.value().read() {
                Some(Ok(d)) => rsx! {
                    TourViewer {
                        key: "tour-view-{tour_id}",
                        doc: d.doc.clone(),
                        assets: Default::default(),
                        source: ViewerSource::Auth,
                        on_hotspot_move: move |_| {},
                        on_scene_change: move |_: String| {},
                        host_id: format!("pnex-tour-viewer-{}", tour_id.replace('-', "")),
                        compact: false,
                        annotations_enabled: true,
                        show_side_panel: true,
                        editable: false,
                    }
                },
                Some(Err(err)) => rsx! {
                    div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                        {err.message.clone()}
                    }
                },
                None => rsx! {
                    div { class: "flex justify-center py-6",
                        span { class: "animate-spin rounded-full h-6 w-6 border-b-2 border-blue-600" }
                    }
                },
            }
        }
    }
}
