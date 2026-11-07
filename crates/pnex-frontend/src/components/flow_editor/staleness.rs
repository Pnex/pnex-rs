//! Pin/device staleness check of the flow editor (Phase 6).

use dioxus::prelude::*;
use pnex_core::{err_codes, FlowGraph, FlowViolation};

use crate::api;

/// Re-checks the device reads/writes of the graph and fills `stale`.
pub(super) fn use_pin_staleness(graph: Signal<FlowGraph>, mut stale: Signal<Vec<FlowViolation>>) {
    // ─── Pin/device staleness (Phase 6) ───
    // A structurally valid graph may no longer return anything: the pin
    // switched in↔out (set_mode), the pin disappeared from the pinout, the device
    // was deleted, disabled, or is simply offline. Each state has
    // its own message — a device waiting to reconnect (server restart,
    // wifi…) must never show as "deleted or inactive".
    // Client-only violations (never persisted) → node + wire in red.
    // Re-fetched when the read config changes (not on drag: the signature
    // leaves positions out) and on the presence tick below.
    let mut stale_signature = use_signal(String::new);
    // Presence changes without any edit (a device drops or comes back): the
    // check is re-run on this tick, not only when the config changes.
    let mut presence_tick = use_signal(|| 0u32);
    let mut presence_polling = use_signal(|| false);
    if !presence_polling() {
        presence_polling.set(true);
        spawn(async move {
            crate::util::sleep(std::time::Duration::from_secs(15)).await;
            presence_polling.set(false);
            presence_tick.with_mut(|t| *t += 1);
        });
    }
    use_effect(move || {
        let tick = presence_tick();
        let g = graph.cloned(); // tracked read: re-triggers on change
                                // (node_id, device_slug, pin, is_write) — the usage qualifies the
                                // check (a written pin needs no subscription).
        let mut reads: Vec<(String, String, String, bool)> = Vec::new();
        let mut signature = String::new();
        for n in &g.nodes {
            if let pnex_core::FlowNodeKind::DeviceRead { config } = &n.kind {
                // Only complete pairs are judged: a read being
                // typed (empty device or pin) is not a staleness
                // violation — `device_bad_read` on save covers it (otherwise the
                // banner shouts 'device "" not found' on a mere click on
                // "Add a read").
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
        signature.push_str(&format!("tick:{tick}"));
        if signature == *stale_signature.cloned() {
            return; // unchanged config (drag/pan): nothing to re-scan
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
            // ── 1. State of each device — one exact query per slug ──
            // The `device_id` filter is exact and has no `active` filter:
            // immune to pagination truncation, and an active but
            // offline device stays in the response (`active` is a DB flag,
            // not a connection state).
            enum DeviceState {
                /// The devices/pinout API is unreachable — we know NOTHING about the
                /// device, we must not say "not found".
                ApiError(String),
                /// Missing from the registry (deleted, or never existed).
                Deleted,
                /// Present but disabled (`active = false`).
                Inactive,
                /// Present and active — the connection state comes from the pinout.
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
            // ── 2. Pinout of active devices (instances + overlay + connection) ──
            let mut pinouts: std::collections::HashMap<String, Result<api::pins::Pinout, String>> =
                std::collections::HashMap::new();
            for slug in reads
                .iter()
                .map(|(_, d, _, _)| d.clone())
                .collect::<std::collections::HashSet<_>>()
            {
                if let Some(DeviceState::Active { pk }) = states.get(&slug) {
                    // Same source as the inspector (`/pinout`: instances +
                    // overlay) — `/pins` (instances only) would declare
                    // "missing" a pin the editor offers via the overlay
                    // (board default of a never-configured device): false red.
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
            // ── 3. One violation per read, deduplicated (node, code, message) ──
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
                        None => {} // active slug not scanned: impossible here
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
                                // ── Write usage (device-write) ──
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
                                // ── Read usage ──
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
                                // Other output modes read (no subscription
                                // expected, same doctrine as digital_out).
                                Some(pin)
                                    if pin
                                        .mode
                                        .as_deref()
                                        .unwrap_or_default()
                                        .ends_with("_out") => {}
                                // Device online, pin as input, but no
                                // subscription: the generic firmware
                                // publishes nothing — the flow will read empty (payload
                                // `{}`) without ever knowing otherwise. (Never
                                // for writing: an output needs no
                                // subscription.)
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
}
