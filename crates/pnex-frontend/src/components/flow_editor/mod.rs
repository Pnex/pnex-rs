//! Éditeur de flows drag & drop (Phase 5 du chantier ETL, D18).
//!
//! L'éditeur ne parle qu'à l'API Loco — jamais au runtime (garde-fou PRD,
//! docs/architecture/flow-engine.md). Toutes les mutations du graphe passent
//! par les réducteurs purs de `state.rs`, la géométrie est testée dans
//! `geometry.rs`, le rendu/gestes dans `canvas.rs`, la config des nœuds
//! dans `inspector.rs`, l'historique dans `versions.rs`.

pub(crate) mod canvas;
pub(crate) mod debug;
pub(crate) mod function_picker;
pub(crate) mod geometry;
pub(crate) mod inspector;
pub(crate) mod state;
pub(crate) mod versions;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{validate_graph, FlowGraph, FlowVersionDetail, FlowViolation, UpdateFlow};

use crate::api;
use crate::components::confirm::ConfirmDialog;
use crate::components::editor_shell::{
    Chip, EditorShell, EditorStatus, InspectorPanel, PalettePopover, StatusTone,
};
use crate::components::icons;
use crate::components::modal::Modal;
use crate::state::session;
use crate::state::toasts;
use pnex_core::err_codes;
pub(crate) mod cx;

use cx::{EditorCx, Interaction, RuntimeAction};

/// Éditeur monté par la page `/flows` — orchestration : chargement, save,
/// conflit 409, deploy, chip runtime, drawer versions.
#[component]
pub fn FlowEditor(
    flow_id: i64,
    can_write: bool,
    on_back: Callback<()>,
    on_changed: Callback<()>,
) -> Element {
    let mut reload_meta = use_signal(|| 0u32);
    let detail = use_resource(move || async move {
        let _ = reload_meta();
        api::flows::detail(flow_id).await
    });

    // --- État éditeur (les signaux vivent dans le paquet `cx`) ---
    let mut graph = use_signal(FlowGraph::default);
    let mut saved_graph = use_signal(FlowGraph::default);
    let mut saved_version = use_signal(|| 0i64);
    let mut selected_node = use_signal(|| None::<String>);
    let interaction = use_signal(|| Interaction::Idle);
    let pan = use_signal(|| (0.0_f64, 0.0_f64));
    let zoom = use_signal(|| 1.0_f64);
    let mut violations = use_signal(Vec::<FlowViolation>::new);
    let mut stale = use_signal(Vec::<FlowViolation>::new);
    let mut loaded_from = use_signal(|| None::<i64>);
    let mut display_values =
        use_signal(std::collections::HashMap::<String, debug::DisplayBadge>::new);
    let mut expanded_display = use_signal(|| None::<(String, String)>);

    // Vision model names for the `vision-detect` canvas subtitles — one
    // fetch per editor mount (scoped task, render-safe global write).
    use_hook(|| {
        spawn(async move {
            if let Ok(page) = api::ml_models::list(Some(100), None).await {
                crate::state::vision::remember(&page.results);
            }
            // Control keys for the `control-source` subtitles (O30: the raw
            // id showed until the inspector was opened); `list` fills the
            // key cache.
            let _ = api::controls::list().await;
        });
    });

    // Garde du chargement initial (jamais de `.set()` pendant le render).
    let mut loaded = use_signal(|| false);
    use_effect(move || {
        let resource = detail.value();
        let value = resource.read();
        let Some(Ok(flow)) = &*value else {
            return;
        };
        if loaded() {
            return;
        }
        let mut fresh = flow.graph.clone();
        geometry::ensure_positions(&mut fresh);
        // Suivi auto dès le chargement : un split en mode auto régénère ses
        // clés (donc ses ports) depuis son amont sans attendre une édition.
        state::sync_split_keys_auto(&mut fresh);
        graph.set(fresh.clone());
        saved_graph.set(fresh);
        saved_version.set(flow.latest_version_number);
        loaded.set(true);
    });

    let mut cx = EditorCx {
        graph,
        saved_graph,
        saved_version,
        selected_node,
        interaction,
        pan,
        zoom,
        violations,
        stale,
        display_values,
        expanded_display,
    };

    // Dirty dérivé — jamais de signal dédié à tenir à jour.
    let dirty = graph() != saved_graph();

    // ─── Staleness pin/device (Phase 6) ───
    // Un graphe structurellement valide peut ne plus rien remonter : le pin
    // a basculé in↔out (set_mode), le pin a disparu du pinout, le device a
    // été supprimé, désactivé, ou est simplement hors ligne. Chaque état a
    // son message distinct — un device en attente de reconnexion (restart
    // serveur, wifi…) ne doit jamais s'afficher « supprimé ou inactif ».
    // Violations client-only (jamais persistées) → nœud + câble en rouge.
    // Re-fetch uniquement au changement de CONFIG de lecture (pas au drag —
    // la signature exclut les positions).
    let mut stale_signature = use_signal(String::new);
    use_effect(move || {
        let g = graph.cloned(); // lecture suivie : re-déclenche au changement
                                // (node_id, device_slug, pin, is_write) — l'usage qualifie la
                                // vérification (une pin écrite n'a pas besoin de souscription).
        let mut reads: Vec<(String, String, String, bool)> = Vec::new();
        let mut signature = String::new();
        for n in &g.nodes {
            if let pnex_core::FlowNodeKind::DeviceRead { config } = &n.kind {
                // Seuls les couples complets sont jugés : un read en cours de
                // saisie (device ou pin vide) n'est pas une violation de
                // staleness — `device_bad_read` au save le couvre (sinon le
                // bandeau crie « device «  » introuvable » au simple clic sur
                // « Ajouter une lecture »).
                for pin in config
                    .pins
                    .iter()
                    .filter(|pin| !config.device_id.is_empty() && !pin.trim().is_empty())
                {
                    reads.push((n.id.clone(), config.device_id.clone(), pin.clone(), false));
                    signature.push_str(&format!("{}:{}/{};", n.id, config.device_id, pin));
                }
            } else if let pnex_core::FlowNodeKind::DeviceWrite { config } = &n.kind {
                for pin in config
                    .pins
                    .iter()
                    .filter(|pin| !config.device_id.is_empty() && !pin.trim().is_empty())
                {
                    reads.push((n.id.clone(), config.device_id.clone(), pin.clone(), true));
                    signature.push_str(&format!("{}:{}/{}#w;", n.id, config.device_id, pin));
                }
            }
        }
        if signature == *stale_signature.cloned() {
            return; // config inchangée (drag/pan) : rien à re-scruter
        }
        stale_signature.set(signature);
        spawn(async move {
            if reads.is_empty() {
                stale.set(Vec::new());
                return;
            }
            /// Machine code per device state — never deduplicated away
            /// (dedup key = node + code + serialized args). `message` is the
            /// canonical English fallback; display resolves `err-<kebab>`
            /// with `args` at render time (error_i18n).
            fn push(
                found: &mut Vec<FlowViolation>,
                seen: &mut std::collections::HashSet<(String, String, String)>,
                node_id: &str,
                code: &str,
                message: &str,
                args: serde_json::Value,
            ) {
                if seen.insert((node_id.to_owned(), code.to_owned(), args.to_string())) {
                    found.push(FlowViolation::with_args(Some(node_id), code, message, args));
                }
            }
            // ── 1. État de chaque device — une requête exacte par slug ──
            // Le filtre `device_id` est exact et sans filtre `active` :
            // insensible au troncage pagination, et un device actif mais
            // hors ligne reste dans la réponse (`active` est un flag DB,
            // pas un état de connexion).
            enum DeviceState {
                /// L'API devices/pinout est injoignable — on ne sait RIEN du
                /// device, on ne doit pas dire « introuvable ».
                ApiError(String),
                /// Absent du registre (supprimé, ou jamais existé).
                Deleted,
                /// Présent mais désactivé (`active = false`).
                Inactive,
                /// Présent et actif — l'état de connexion vient du pinout.
                Active { pk: i64 },
            }
            let mut states: std::collections::HashMap<String, DeviceState> =
                std::collections::HashMap::new();
            for slug in reads
                .iter()
                .map(|(_, d, _, _)| d.clone())
                .collect::<std::collections::HashSet<_>>()
            {
                let state = match api::devices::list(&api::devices::DeviceFilters {
                    device_id: Some(slug.clone()),
                    limit: Some(1),
                    ..Default::default()
                })
                .await
                {
                    Err(err) => DeviceState::ApiError(err.message),
                    Ok(page) if page.results.is_empty() => DeviceState::Deleted,
                    Ok(page) => match page.results[0].active {
                        false => DeviceState::Inactive,
                        true => DeviceState::Active {
                            pk: page.results[0].id,
                        },
                    },
                };
                states.insert(slug, state);
            }
            // ── 2. Pinout des devices actifs (instances + overlay + connexion) ──
            let mut pinouts: std::collections::HashMap<String, Result<api::pins::Pinout, String>> =
                std::collections::HashMap::new();
            for slug in reads
                .iter()
                .map(|(_, d, _, _)| d.clone())
                .collect::<std::collections::HashSet<_>>()
            {
                if let Some(DeviceState::Active { pk }) = states.get(&slug) {
                    // Même source que l'inspecteur (`/pinout` : instances +
                    // overlay) — `/pins` (instances seules) déclarerait
                    // « absent » un pin que l'éditeur propose via l'overlay
                    // (défaut carte d'un device jamais configuré) : faux rouge.
                    let result = api::pins::pinout(*pk).await.map_err(|e| e.message);
                    pinouts.insert(slug, result);
                }
            }
            // ── 2b. Metrics published by the devices (custom firmware
            // `addMetric`): a device-read entry that is not a pin is valid
            // when the device publishes a series of that name. ──
            let needs_catalog = reads.iter().any(|(_, slug, pin, is_write)| {
                !is_write
                    && matches!(
                        pinouts.get(slug),
                        Some(Ok(p)) if !p.pins.iter().any(|x| &x.label == pin)
                    )
            });
            let published: std::collections::HashSet<(String, String)> = if needs_catalog {
                api::telemetry::catalog()
                    .await
                    .map(|c| {
                        c.series
                            .into_iter()
                            .map(|s| (s.device_id, s.metric.to_ascii_lowercase()))
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Default::default()
            };
            // ── 3. Une violation par read, dédupliquée (nœud, code, message) ──
            let mut found: Vec<FlowViolation> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for (node_id, device_slug, pin_label, is_write) in reads {
                let Some(state) = states.get(&device_slug) else {
                    continue;
                };
                match state {
                    DeviceState::ApiError(err) => push(
                        &mut found,
                        &mut seen,
                        &node_id,
                        err_codes::PIN_CHECK_UNAVAILABLE,
                        "check unavailable",
                        serde_json::json!({ "device": device_slug, "msg": err }),
                    ),
                    DeviceState::Deleted => push(
                        &mut found,
                        &mut seen,
                        &node_id,
                        err_codes::DEVICE_DELETED,
                        "device not found (deleted)",
                        serde_json::json!({ "device": device_slug }),
                    ),
                    DeviceState::Inactive => push(
                        &mut found,
                        &mut seen,
                        &node_id,
                        err_codes::DEVICE_INACTIVE,
                        "device inactive — re-enable it from the Devices page",
                        serde_json::json!({ "device": device_slug }),
                    ),
                    DeviceState::Active { .. } => match pinouts.get(&device_slug) {
                        None => {} // slug actif non scruté : impossible ici
                        Some(Err(err)) => push(
                            &mut found,
                            &mut seen,
                            &node_id,
                            err_codes::PIN_CHECK_UNAVAILABLE,
                            "pinout unavailable",
                            serde_json::json!({ "device": device_slug, "msg": err }),
                        ),
                        Some(Ok(pinout)) => {
                            if !pinout.connected {
                                push(
                                    &mut found,
                                    &mut seen,
                                    &node_id,
                                    err_codes::DEVICE_OFFLINE,
                                    "device offline — reads resume on reconnect",
                                    serde_json::json!({ "device": device_slug }),
                                );
                            }
                            match pinout.pins.iter().find(|p| p.label == pin_label) {
                                None if !is_write
                                    && published.contains(&(
                                        device_slug.clone(),
                                        pin_label.to_ascii_lowercase(),
                                    )) => {}
                                None => push(
                                    &mut found,
                                    &mut seen,
                                    &node_id,
                                    err_codes::PIN_MISSING,
                                    "pin missing from device",
                                    serde_json::json!({ "pin": pin_label, "device": device_slug }),
                                ),
                                // ── Usage écriture (device-write) ──
                                Some(pin)
                                    if is_write
                                        && !pin
                                            .mode
                                            .as_deref()
                                            .unwrap_or_default()
                                            .ends_with("_out") =>
                                {
                                    push(
                                        &mut found,
                                        &mut seen,
                                        &node_id,
                                        err_codes::DEVICE_WRITE_PIN_MODE,
                                        "pin is not an output (digital_out/pwm_out required to write)",
                                        serde_json::json!({ "pin": pin_label }),
                                    )
                                }
                                // ── Usage lecture ──
                                Some(pin)
                                    if !is_write && pin.mode.as_deref() == Some("digital_out") =>
                                {
                                    push(
                                        &mut found,
                                        &mut seen,
                                        &node_id,
                                        err_codes::PIN_IS_OUTPUT,
                                        "pin is an output (digital_out) — reads will stay empty",
                                        serde_json::json!({ "pin": pin_label }),
                                    )
                                }
                                // Autres modes sortie lus (pas de souscription
                                // attendue, même doctrine que digital_out).
                                Some(pin)
                                    if pin
                                        .mode
                                        .as_deref()
                                        .unwrap_or_default()
                                        .ends_with("_out") => {}
                                // Device en ligne, pin en entrée, mais aucune
                                // souscription : le firmware générique ne
                                // publie rien — le flow lira du vide (payload
                                // `{}`) sans jamais le savoir sinon. (Jamais
                                // pour l'écriture : une sortie n'a pas besoin
                                // de souscription.)
                                Some(pin) if !is_write && pin.subscribed_ms.is_none() => push(
                                    &mut found,
                                    &mut seen,
                                    &node_id,
                                    err_codes::PIN_NOT_SUBSCRIBED,
                                    "pin not subscribed — the device publishes nothing; subscribe it (interval) from Devices → Pins tab",
                                    serde_json::json!({ "pin": pin_label, "device": device_slug }),
                                ),
                                _ => {}
                            }
                        }
                    },
                }
            }
            stale.set(found);
        });
    });

    // --- Save (validate → PATCH) ---
    let mut saving = use_signal(|| false);
    let mut conflict = use_signal(|| None::<String>);
    // Proposition de deploy après save d'un flow déjà déployé : la version
    // en exécution reste l'ancienne (une update ne recharge jamais le
    // moteur) — petit encart « Déployer vN ? » plutôt qu'un auto-deploy
    // qui purgerait le feed debug à chaque itération (choix utilisateur).
    let mut propose_deploy = use_signal(|| None::<i64>);
    // Enregistrer (bouton, nom inchangé) et renommer (titre de la coquille)
    // partagent le même PATCH — le graphe voyage avec : renommer vaut
    // sauvegarde de l'état courant.
    let save = Callback::new(move |name: Option<String>| {
        if saving() {
            return;
        }
        let local = validate_graph(&graph.peek().clone());
        if !local.is_empty() {
            violations.set(local);
            return;
        }
        saving.set(true);
        let renamed = name.is_some();
        let params = UpdateFlow {
            expected_version_number: saved_version(),
            graph: graph(),
            name,
            author: session::user().map(|user| user.username),
            note: None,
        };
        spawn(async move {
            match api::flows::update(flow_id, params).await {
                Ok(flow) => {
                    graph.with_mut(|g| state::adopt_stored_secrets(g, &flow.graph));
                    saved_graph.set(flow.graph.clone());
                    saved_version.set(flow.latest_version_number);
                    violations.set(Vec::new());
                    loaded_from.set(None);
                    toasts::success("toast-flow-saved");
                    // Flow déjà déployé : la version en exécution reste
                    // l'ancienne — proposer le deploy (l'édition ultérieure
                    // masque l'encart, la condition de rendu porte sur dirty).
                    if flow.status == "deployed" {
                        propose_deploy.set(Some(flow.latest_version_number));
                    }
                    // The header title comes from the detail resource.
                    if renamed {
                        reload_meta.with_mut(|r| *r += 1);
                    }
                    on_changed.call(());
                }
                Err(err) => match api::flows::classify_save_error(&err) {
                    api::flows::SaveError::Conflict { description } => {
                        conflict.set(Some(description));
                    }
                    api::flows::SaveError::Invalid(invalid) => violations.set(invalid),
                    api::flows::SaveError::Other(message) => toasts::error(message),
                },
            }
            saving.set(false);
        });
    });

    // --- Résolution du conflit 409 ---
    let mut conflict_reload = move |_: ()| {
        conflict.set(None);
        spawn(async move {
            match api::flows::detail(flow_id).await {
                Ok(flow) => {
                    let mut fresh = flow.graph.clone();
                    geometry::ensure_positions(&mut fresh);
                    graph.set(fresh.clone());
                    saved_graph.set(fresh);
                    saved_version.set(flow.latest_version_number);
                    violations.set(Vec::new());
                }
                Err(err) => toasts::error(err),
            }
        });
    };
    let conflict_overwrite = move |_| {
        conflict.set(None);
        spawn(async move {
            let Ok(server) = api::flows::detail(flow_id).await else {
                toasts::error(t!("common-error").to_string());
                return;
            };
            let params = UpdateFlow {
                // Version fraîche du serveur : écrasement assumé.
                expected_version_number: server.latest_version_number,
                graph: graph.peek().clone(),
                name: None,
                author: session::user().map(|user| user.username),
                note: None,
            };
            match api::flows::update(flow_id, params).await {
                Ok(flow) => {
                    graph.with_mut(|g| state::adopt_stored_secrets(g, &flow.graph));
                    saved_graph.set(flow.graph.clone());
                    saved_version.set(flow.latest_version_number);
                    violations.set(Vec::new());
                    toasts::success("toast-flow-saved");
                    on_changed.call(());
                }
                Err(err) => toasts::error(err),
            }
        });
    };

    // --- Deploy ---
    let mut deploying = use_signal(|| false);
    // Multi-user awareness (fed by the 5 s runtime poll): last deployed
    // version observed (`None` = not observed yet) and the newer version
    // saved by someone else, if any.
    let mut seen_deployed = use_signal(|| None::<Option<i64>>);
    let mut remote_version = use_signal(|| None::<i64>);
    let mut deploy = move |_| {
        if deploying() {
            return;
        }
        deploying.set(true);
        // Deploy exactly the version shown in the editor: without it the
        // server picks the latest saved version, possibly one saved by
        // another user that this editor never displayed.
        let version = Some(saved_version()).filter(|v| *v > 0);
        spawn(async move {
            match api::flows::deploy(flow_id, version).await {
                Ok(deployed) => {
                    // Our own deploy: never reported as a remote change.
                    seen_deployed.set(Some(deployed.flow.deployed_version_number));
                    toasts::success("toast-flow-deployed");
                    // Références notify périmées (D50) : toast info dédié —
                    // le deploy passe, mais les canaux/modèles manquants ou
                    // désactivés n'enverront rien (nœud dégradé, warn).
                    let stale = deployed.stale_notify_nodes.len();
                    if stale > 0 {
                        toasts::info(t!("flows-deploy-stale-notify", count: stale).to_string());
                    }
                    // Onglets d'autres flows isolés à la projection (graphe
                    // illisible) : le deploy passe, mais ces flows ne tournent
                    // plus — à reconstruire ou supprimer.
                    let skipped = deployed.skipped_tabs.len();
                    if skipped > 0 {
                        toasts::info(t!("flows-deploy-skipped-tabs", count: skipped).to_string());
                    }
                    on_changed.call(());
                    reload_meta.with_mut(|r| *r += 1);
                    propose_deploy.set(None);
                }
                // Pré-flight rejeté (400 engine_load) : le message moteur
                // **réel** est affiché — plus jamais un « HTTP 400 » nu.
                // `ToastMessage::Api` resolves violations + code at render.
                Err(err) => toasts::error(err),
            }
            deploying.set(false);
        });
    };

    // --- Contrôles runtime (Stop / Start / Restart) ---
    // Un seul signal d'occupation : stop/start/restart touchent tous le même
    // runtime, aucune action concurrente.
    let mut runtime_busy = use_signal(|| false);
    // --- Drawer versions ---
    let mut versions_open = use_signal(|| false);
    // Drawer de debug (même garde dev/debug que le bouton).
    let mut debug_open = use_signal(|| false);
    // Câble à couper (from, port, to) — confirmation à la demande.
    let mut pending_wire = use_signal(|| None::<(String, usize, String, String)>);

    // --- Chip runtime (poll 5 s, pattern devices.rs) ---
    let mut reload_runtime = use_signal(|| 0u32);
    let mut runtime_polling = use_signal(|| false);
    let runtime = use_resource(move || async move {
        let _ = reload_runtime();
        api::flows::runtime(flow_id).await
    });

    let mut runtime_action = move |action: RuntimeAction| {
        if runtime_busy() {
            return;
        }
        runtime_busy.set(true);
        spawn(async move {
            let outcome = match action {
                RuntimeAction::Stop => api::flows::stop(flow_id).await.map(|_| ()),
                RuntimeAction::Start => api::flows::start(flow_id).await.map(|_| ()),
                RuntimeAction::Restart => api::flows::restart(flow_id).await.map(|_| ()),
            };
            match outcome {
                Ok(()) => {
                    toasts::success(match action {
                        RuntimeAction::Stop => "toast-flow-stopped",
                        RuntimeAction::Start => "toast-flow-started",
                        RuntimeAction::Restart => "toast-flow-restarted",
                    });
                    on_changed.call(());
                    reload_meta.with_mut(|r| *r += 1);
                    // Le chip reflète la nouvelle santé au prochain poll.
                    reload_runtime.with_mut(|r| *r += 1);
                }
                Err(err) => toasts::error(err),
            }
            runtime_busy.set(false);
        });
    };
    // Changes made by another user, observed through the runtime poll: a
    // newer saved version (banner offering a reload) and a redeploy or a
    // status change (detail refresh so the runtime controls stay right).
    use_effect(move || {
        let value = runtime.value();
        let value = value.read();
        let Some(Ok(status)) = &*value else {
            return;
        };
        let mine = *saved_version.peek();
        if let Some(latest) = status.latest_version_number {
            if mine > 0 && latest > mine && *remote_version.peek() != Some(latest) {
                remote_version.set(Some(latest));
            }
        }
        let mut refresh = false;
        let deployed = status.deployed_version_number;
        let previous_seen = *seen_deployed.peek();
        match previous_seen {
            None => seen_deployed.set(Some(deployed)),
            Some(previous) if previous != deployed => {
                seen_deployed.set(Some(deployed));
                if deployed.is_some() && !*deploying.peek() && !*runtime_busy.peek() {
                    toasts::info("toast-flow-deployed-remote");
                }
                refresh = true;
            }
            _ => {}
        }
        let detail_status = match &*detail.peek() {
            Some(Ok(flow)) => Some(flow.status.clone()),
            _ => None,
        };
        if status.flow_status.is_some()
            && detail_status.is_some()
            && status.flow_status != detail_status
        {
            refresh = true;
        }
        if refresh {
            reload_meta.with_mut(|r| *r += 1);
        }
    });
    if !runtime_polling() {
        runtime_polling.set(true);
        spawn(async move {
            crate::util::sleep(std::time::Duration::from_secs(5)).await;
            runtime_polling.set(false);
            reload_runtime.with_mut(|r| *r += 1);
        });
    }

    // Outils de debug actifs ? Porté par le chip runtime (pas d'endpoint
    // dédié) — mode run : boutons non rendus, pas de poll. Signal (pas un
    // booléen dérivé) : la resource de feed doit le **suivre**.
    let mut debug_tools = use_signal(|| false);
    use_effect(move || {
        let active = match &*runtime.value().read() {
            Some(Ok(status)) => status.debug_tools,
            _ => false,
        };
        if debug_tools() != active {
            debug_tools.set(active);
        }
    });

    // --- Feed debug (badges Display) — poll 5 s tant que debug_tools ---
    let mut reload_debug = use_signal(|| 0u32);
    let mut debug_polling = use_signal(|| false);
    let debug_feed = use_resource(move || async move {
        let _ = reload_debug();
        if !debug_tools() {
            return None;
        }
        api::flows::debug(flow_id).await.ok()
    });
    if debug_tools() && !debug_polling() {
        debug_polling.set(true);
        spawn(async move {
            crate::util::sleep(std::time::Duration::from_secs(5)).await;
            debug_polling.set(false);
            reload_debug.with_mut(|r| *r += 1);
        });
    }
    // --- Camera/vision node statuses (D103) — polled 5 s in every mode
    // (health, not debug output: no debug_tools gate) ---
    let mut reload_status = use_signal(|| 0u32);
    let mut status_polling = use_signal(|| false);
    let node_status = use_resource(move || async move {
        let _ = reload_status();
        api::flows::node_status(flow_id).await.ok()
    });
    if !status_polling() {
        status_polling.set(true);
        spawn(async move {
            crate::util::sleep(std::time::Duration::from_secs(5)).await;
            status_polling.set(false);
            reload_status.with_mut(|r| *r += 1);
        });
    }
    // Pliage : dernière valeur par nœud Display, purgées si le moteur est
    // arrêté (les valeurs d'un moteur stoppé ne sont plus des vérités) —
    // et le modal pretty ouvert ferme avec elles.
    use_effect(move || {
        let running = match &*runtime.value().read() {
            Some(Ok(status)) => status.running,
            // Statut inconnu : on ne purge pas par prudence.
            _ => true,
        };
        let mut fresh: std::collections::HashMap<String, debug::DisplayBadge> =
            std::collections::HashMap::new();
        if running {
            if let Some(Some(feed)) = &*debug_feed.value().read() {
                for entry in feed.entries.iter().filter(|e| e.source == "pnex-display") {
                    fresh.insert(
                        entry.node_id.clone(),
                        debug::DisplayBadge {
                            label: debug::display_value_label(&entry.msg),
                            pretty: debug::display_value_pretty(&entry.msg),
                            status: None,
                        },
                    );
                }
            }
            // Camera/vision node statuses (D103): latest one per node;
            // label and detail are localized at render time. Same poll: the
            // last message of every debug/display node, in every mode (the
            // side panel history alone stays behind debug_tools).
            if let Some(Some(feed)) = &*node_status.value().read() {
                for entry in &feed.entries {
                    if matches!(entry.source.as_str(), "debug" | "pnex-display") {
                        fresh.insert(
                            entry.node_id.clone(),
                            debug::DisplayBadge {
                                label: debug::debug_value_label(&entry.msg),
                                pretty: debug::display_value_pretty(&entry.msg)
                                    .or_else(|| Some(debug::display_value_compact(&entry.msg))),
                                status: None,
                            },
                        );
                        continue;
                    }
                    if let Ok(st) =
                        serde_json::from_value::<pnex_core::vision::NodeStatus>(entry.msg.clone())
                    {
                        fresh.insert(
                            entry.node_id.clone(),
                            debug::DisplayBadge {
                                label: String::new(),
                                pretty: None,
                                status: Some(st),
                            },
                        );
                    }
                }
            }
        }
        display_values.set(fresh);
        if !running && expanded_display.read().is_some() {
            expanded_display.set(None);
        }
    });

    // Statut DB pour le conditionnement des contrôles runtime (Stop/Restart
    // sur `deployed`, Start sur `stopped`).
    let flow_status = match &*detail.value().read() {
        Some(Ok(flow)) => flow.status.clone(),
        _ => String::new(),
    };

    // ─── Statut unifié (point + libellé) — fusion du statut DB et de la
    // santé runtime (l'ancien chip « Moteur actif · v12 »). Priorité :
    // erreur moteur > arrêté > périmé (« Déployé · à redéployer ») > déployé
    // > non déployé > détail pas chargé. L'infobulle garde pid · vN + la
    // dernière erreur moteur, verbatim.
    let editor_status: EditorStatus = {
        let detail_value = detail.value();
        let runtime_value = runtime.value();
        let detail_ref = detail_value.read();
        let runtime_ref = runtime_value.read();
        match (&*detail_ref, &*runtime_ref) {
            (Some(Ok(_flow)), Some(Ok(status))) => {
                let engine_error = status.engine_status.as_deref() == Some("error");
                let engine_stopped = status.engine_status.as_deref() == Some("stopped");
                let deployed_version = status.deployed_version_number;
                let outdated = status.running
                    && !engine_error
                    && !engine_stopped
                    && deployed_version.is_some()
                    && deployed_version != Some(saved_version());
                let tooltip = {
                    let base = match (status.pid, deployed_version) {
                        (Some(pid), Some(version)) => format!("pid {pid} · v{version}"),
                        (Some(pid), None) => format!("pid {pid}"),
                        _ => String::new(),
                    };
                    match &status.last_error {
                        Some(err) if !err.is_empty() => format!("{base} — {err}"),
                        _ => base,
                    }
                };
                if engine_error {
                    EditorStatus::new(StatusTone::Red, t!("flows-status-error"))
                        .with_tooltip(tooltip)
                } else if engine_stopped || !status.running {
                    EditorStatus::new(StatusTone::Slate, t!("flows-status-stopped"))
                        .with_tooltip(tooltip)
                } else if outdated {
                    EditorStatus::new(
                        StatusTone::Amber,
                        format!(
                            "{} · {}",
                            t!("flows-status-deployed"),
                            t!("flows-runtime-outdated")
                        ),
                    )
                    .with_tooltip(tooltip)
                } else if deployed_version.is_some() {
                    EditorStatus::new(StatusTone::Green, t!("flows-status-deployed"))
                        .with_tooltip(tooltip)
                } else {
                    EditorStatus::new(StatusTone::Slate, t!("flows-status-draft"))
                        .with_tooltip(tooltip)
                }
            }
            (Some(Ok(flow)), _) => match flow.status.as_str() {
                "deployed" => EditorStatus::new(StatusTone::Green, t!("flows-status-deployed")),
                "stopped" => EditorStatus::new(StatusTone::Slate, t!("flows-status-stopped")),
                "error" => EditorStatus::new(StatusTone::Red, t!("flows-status-error")),
                _ => EditorStatus::new(StatusTone::Slate, t!("flows-status-draft")),
            },
            _ => EditorStatus::new(StatusTone::Slate, t!("common-loading")),
        }
    };

    // Nom + réf de l'en-tête (None tant que le détail n'est pas chargé).
    let (flow_name, flow_subtitle) = match &*detail.value().read() {
        Some(Ok(flow)) => (flow.name.clone(), Some(format!("#{}", flow.id))),
        _ => (String::new(), None),
    };

    // Slot inspecteur : monté seulement si un nœud est sélectionné
    // (principe 4 — le panneau n'existe pas à l'état vide). Titre = nom du
    // nœud sinon libellé du kind ; remount par `key` pour que les champs
    // repartent de la config du nœud.
    let inspector_slot: Option<Element> = cx.selected_node.cloned().map(|node_id| {
        let graph_read = cx.graph.read();
        let node = graph_read.nodes.iter().find(|n| n.id == node_id);
        let kind = node.map(|n| canvas::kind_of(&n.kind));
        let name = node.and_then(|n| n.name.clone()).filter(|s| !s.is_empty());
        let label = kind.map(|k| canvas::kind_labels(k).0);
        let icon = kind.map(|k| canvas::kind_icon(k).0);
        drop(graph_read);
        let title = match name {
            Some(nom) => nom,
            None => label.unwrap_or_else(|| node_id.clone()),
        };
        rsx! {
            InspectorPanel {
                key: "{node_id}",
                icon,
                title,
                subtitle: Some(format!("#{}", node_id)),
                on_close: move |_| cx.selected_node.set(None),
                body: rsx! {
                    inspector::InspectorBody { cx, can_write, flow_id }
                },
            }
        }
    });

    // Reveal the selected node when the inspector would cover its output
    // ports (node near the right edge): pan once the gesture ends, on a new
    // selection or at the end of a drag — never while the user pans.
    let mut revealed_for = use_signal(|| None::<String>);
    let mut was_dragging = use_signal(|| false);
    use_effect(move || {
        let selected = cx.selected_node.cloned();
        let idle = matches!(*cx.interaction.read(), Interaction::Idle);
        let dragging = matches!(*cx.interaction.read(), Interaction::Dragging { .. });
        if dragging {
            was_dragging.set(true);
            return;
        }
        if !idle {
            return;
        }
        let drag_ended = *was_dragging.peek();
        was_dragging.set(false);
        if selected.is_none() || (!drag_ended && *revealed_for.peek() == selected) {
            revealed_for.set(selected);
            return;
        }
        revealed_for.set(selected.clone());
        let Some(rect) = geometry::canvas_rect() else {
            return;
        };
        let node_x = cx
            .graph
            .peek()
            .nodes
            .iter()
            .find(|n| Some(&n.id) == selected.as_ref())
            .and_then(|n| n.position)
            .map(|p| p.x);
        let Some(node_x) = node_x else { return };
        let (pan_x, pan_y) = *cx.pan.peek();
        let zoom = *cx.zoom.peek();
        if let Some(next_x) = geometry::pan_to_reveal_node(node_x, pan_x, zoom, rect.2) {
            cx.pan.set((next_x, pan_y));
        }
    });

    // Bandeau : violations de validation + staleness pin/device (Phase 6).
    let banner: Vec<FlowViolation> = violations
        .cloned()
        .into_iter()
        .chain(stale.cloned())
        .collect();

    // ─── Vue pretty d'une sonde Display (clic sur le badge live) ───
    // Titre résolu ici (pas de `let` dans rsx) : nom du nœud, sinon id
    // canvas (règle du badge).
    let display_modal = expanded_display().map(|(node_id_open, pretty)| {
        let name = cx
            .graph
            .read()
            .nodes
            .iter()
            .find(|n| n.id == node_id_open)
            .and_then(|n| n.name.clone())
            .unwrap_or_else(|| node_id_open.clone());
        (name, pretty)
    });

    rsx! {
        EditorShell {
            on_back,
            title: flow_name,
            subtitle: flow_subtitle,
            on_rename: if can_write { Some(Callback::new(move |name: String| save.call(Some(name)))) } else { None },
            status: editor_status,
            version: if saved_version() > 0 { Some(saved_version()) } else { None },
            extra_chips: rsx! {
                if dirty {
                    Chip { tone: StatusTone::Amber, label: t!("flows-dirty-unsaved").to_string() }
                }
                if let Some(version) = loaded_from() {
                    Chip {
                        tone: StatusTone::Purple,
                        label: format!("v{version}"),
                        on_remove: conflict_reload,
                    }
                }
            },
            actions: rsx! {
                if can_write {
                    button {
                        class: "px-3 py-1.5 text-sm bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                        disabled: saving() || !dirty,
                        onclick: move |_| save.call(None),
                        {t!("common-save")}
                    }
                    button {
                        class: if !dirty {
                            "px-3 py-1.5 text-sm bg-emerald-600 text-white rounded-lg hover:bg-emerald-700 transition-colors font-medium disabled:opacity-40 disabled:cursor-not-allowed"
                        } else {
                            "px-3 py-1.5 text-sm bg-emerald-600 text-white rounded-lg transition-colors font-medium opacity-40 cursor-not-allowed"
                        },
                        disabled: deploying() || runtime_busy() || dirty,
                        title: if dirty { t!("flows-deploy-need-save") } else { t!("flows-deploy") },
                        onclick: deploy,
                        if deploying() {
                            span { class: "animate-spin inline-block rounded-full h-3.5 w-3.5 border-2 border-white border-t-transparent mr-1.5 align-[-2px]" }
                            {t!("flows-deploying")}
                        } else {
                            icons::Zap { class: "h-4 w-4 inline mr-1" }
                            {t!("flows-deploy")}
                        }
                    }
                    // Contrôles runtime : Stop/Restart sur un flow déployé,
                    // Start sur un flow arrêté — conditionnés au statut DB
                    // (la santé process est l'affaire du chip).
                    if flow_status == "deployed" {
                        button {
                            class: "px-3 py-1.5 text-sm text-rose-700 bg-rose-50 border border-rose-300 rounded-lg hover:bg-rose-100 transition-colors font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                            disabled: runtime_busy() || deploying(),
                            title: t!("flows-stop-title"),
                            onclick: move |_| runtime_action(RuntimeAction::Stop),
                            {t!("flows-stop")}
                        }
                        button {
                            class: "px-3 py-1.5 text-sm text-slate-700 bg-slate-50 border border-slate-300 rounded-lg hover:bg-slate-100 transition-colors font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                            disabled: runtime_busy() || deploying(),
                            title: t!("flows-restart-title"),
                            onclick: move |_| runtime_action(RuntimeAction::Restart),
                            {t!("flows-restart")}
                        }
                    } else if flow_status == "stopped" {
                        button {
                            class: "px-3 py-1.5 text-sm text-emerald-700 bg-emerald-50 border border-emerald-300 rounded-lg hover:bg-emerald-100 transition-colors font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                            disabled: runtime_busy() || deploying(),
                            title: t!("flows-start-title"),
                            onclick: move |_| runtime_action(RuntimeAction::Start),
                            {t!("flows-start")}
                        }
                    }
                }
                // Le chip runtime historique (« Moteur actif · v12 ») est
                // absorbé par le statut unifié de la coquille (point + libellé
                // + infobulle pid · vN — erreur au survol).
                if debug_tools() {
                    button {
                        class: if debug_open() {
                            "px-3 py-1.5 text-sm text-cyan-700 bg-cyan-50 border border-cyan-400 rounded-lg transition-colors"
                        } else {
                            "px-3 py-1.5 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors"
                        },
                        title: t!("flows-debug-panel"),
                        aria_label: t!("flows-debug-panel"),
                        onclick: move |_| debug_open.set(!debug_open()),
                        icons::Bug { class: "h-4 w-4 inline sm:mr-1" }
                        span { class: "hidden sm:inline", {t!("flows-debug-panel")} }
                    }
                }
                button {
                    class: "inline-flex items-center px-3 py-1.5 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
                    title: t!("flows-versions"),
                    aria_label: t!("flows-versions"),
                    onclick: move |_| versions_open.set(true),
                    icons::History { class: "h-4 w-4 sm:mr-1" }
                    span { class: "hidden sm:inline", {t!("flows-versions")} }
                }
            },
            banner: rsx! {
                // ─── Bandeau de violations (locales, serveur) + staleness ───
                if !banner.is_empty() {
                    div { class: "bg-red-50 border border-red-200 rounded-lg px-4 py-2 text-sm text-red-700",
                        span { class: "font-semibold mr-2", {t!("flows-violations-banner-title")} }
                        ul { class: "list-disc list-inside",
                            for violation in banner.clone() {
                                li { key: "{violation.code}-{violation.node_id:?}-{violation.message}",
                                    {
                                        match &violation.node_id {
                                            Some(node_id) => format!("#{node_id} : {}", violation.message),
                                            None => violation.message.clone(),
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // ─── Newer version saved by another user ───
                {
                    match remote_version() {
                        Some(version) if version > saved_version() => rsx! {
                            div { class: "bg-amber-50 border border-amber-200 rounded-lg px-4 py-2 text-sm text-amber-800 flex items-center gap-3",
                                span { class: "flex-1",
                                    if dirty {
                                        {t!("flows-remote-version-dirty", version : version)}
                                    } else {
                                        {t!("flows-remote-version", version : version)}
                                    }
                                }
                                button {
                                    class: "px-3 py-1 text-xs bg-amber-600 text-white rounded-lg hover:bg-amber-700 transition-colors font-medium",
                                    onclick: move |_| {
                                        remote_version.set(None);
                                        propose_deploy.set(None);
                                        conflict_reload(());
                                    },
                                    {t!("flows-remote-version-reload")}
                                }
                            }
                        },
                        _ => rsx! {},
                    }
                }

                // ─── Encart « Déployer la version enregistrée ? » ───
                // Proposé après un save d'un flow déjà déployé ; masqué si
                // l'utilisateur édite à nouveau (la version proposée ne serait
                // plus celle du canvas) ou après un deploy réussi.
                {
                    match propose_deploy() {
                        Some(version) if !dirty => rsx! {
                            div { class: "bg-blue-50 border border-blue-200 rounded-lg px-4 py-2 text-sm text-blue-800 flex items-center gap-3",
                                span { class: "flex-1", {t!("flows-deploy-propose-text", version : version)} }
                                button {
                                    class: "px-3 py-1 text-xs bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                                    disabled: deploying(),
                                    onclick: move |evt| {
                                        propose_deploy.set(None);
                                        deploy(evt);
                                    },
                                    {t!("flows-deploy-propose-go", version : version)}
                                }
                                button {
                                    class: "px-3 py-1 text-xs text-blue-700 hover:bg-blue-100 rounded-lg transition-colors",
                                    onclick: move |_| propose_deploy.set(None),
                                    {t!("flows-deploy-propose-later")}
                                }
                            }
                        },
                        _ => rsx! {},
                    }
                }
            },
            canvas: rsx! {
                canvas::Canvas {
                    cx,
                    on_cut_wire: move |wire| {
                        pending_wire.set(Some(wire));
                    },
                }
            },
            palette: rsx! {
                PalettePopover {
                    add_title: t!("eshell-add-node").to_string(),
                    title: t!("flows-palette-title").to_string(),
                    search_placeholder: t!("eshell-search").to_string(),
                    items: canvas::palette_items(),
                    on_pick: move |key: String| {
                        if let Some(kind) = canvas::kind_from_key(&key) {
                            canvas::add_node_to_canvas(cx, kind);
                        }
                    },
                }
            },
            inspector: inspector_slot,
            empty_hint: if cx.graph.cloned().nodes.is_empty() { Some(t!("eshell-empty-hint").to_string()) } else { None },
        }

        // ─── Drawer debug (monté seulement si ouvert : le poll meurt avec) ───
        if debug_open() {
            debug::DebugDrawer { flow_id, on_close: move |_| debug_open.set(false) }
        }

        // ─── Drawer versions ───
        if versions_open() {
            versions::VersionsDrawer {
                flow_id,
                can_write,
                on_close: move |_| versions_open.set(false),
                on_loaded: move |version: FlowVersionDetail| {
                    // Chargement d'une version dans l'éditeur : si c'est
                    // la dernière, pas de dirty ; sinon save créera
                    // v(n+1) avec ce graphe (restauration par édition).
                    let mut fresh = version.graph;
                    geometry::ensure_positions(&mut fresh);
                    graph.set(fresh.clone());
                    if version.version_number == saved_version() {
                        saved_graph.set(fresh);
                        loaded_from.set(None);
                    } else {
                        saved_graph.set(FlowGraph::default());
                        loaded_from.set(Some(version.version_number));
                    }
                    selected_node.set(None);
                    violations.set(Vec::new());
                    versions_open.set(false);
                },
                on_deployed: move |_| {
                    reload_meta.with_mut(|r| *r += 1);
                    on_changed.call(());
                },
            }
        }

        // ─── Modales ───
        if let Some(description) = conflict() {
            Modal {
                title: t!("flows-conflict-title"),
                max_width: "max-w-md".to_string(),
                on_close: move |_| conflict.set(None),
                div { class: "space-y-4",
                    p { class: "text-sm text-gray-600", {description.clone()} }
                    p { class: "text-sm text-gray-600", {t!("flows-conflict-message")} }
                    div { class: "flex flex-col gap-2 pt-2",
                        button {
                            class: "px-4 py-2 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
                            onclick: move |_| conflict_reload(()),
                            {t!("flows-conflict-reload")}
                        }
                        button {
                            class: "px-4 py-2 text-sm font-semibold text-white bg-red-600 rounded-lg hover:bg-red-700 transition-colors",
                            onclick: conflict_overwrite,
                            {t!("flows-conflict-overwrite")}
                        }
                    }
                }
            }
        }
        if let Some((from, port, to, pin)) = pending_wire() {
            ConfirmDialog {
                title: t!("flows-wire-remove-title"),
                message: t!("flows-wire-remove-message"),
                confirm_label: t!("flows-wire-remove"),
                on_confirm: move |_| {
                    cx.update_graph(|g| {
                        if pin.is_empty() {
                            // Plain source-side wire.
                            state::remove_target(g, &from, port, &to);
                        } else {
                            // Annotated pin row: the runtime wire only
                            // goes if the source feeds no other pin.
                            state::cut_input_wire(g, &from, port, &to, &pin);
                        }
                    });
                    pending_wire.set(None);
                },
                on_cancel: move |_| pending_wire.set(None),
            }
        }

        // ─── Vue pretty d'une sonde Display (clic sur le badge live) ───
        if let Some((name, pretty)) = display_modal {
            Modal {
                title: t!("flows-display-value-title", name : name),
                max_width: "max-w-2xl".to_string(),
                on_close: move |_| expanded_display.set(None),
                pre { class: "text-xs font-mono whitespace-pre-wrap break-all bg-gray-50 border border-gray-200 rounded-lg p-4 text-gray-800 m-0",
                    {pretty}
                }
            }
        }
    }
}
