//! Provisioning Brick 0 — admission à l'`Announce` (policy `Validated`, B0.3).
//!
//! Dérive la carte de pins de l'overlay (`mcu_boards.details` →
//! `BoardOverlay`), valide chaque pin contre les chip-caps
//! (`pnex_core::caps::validate` — point unique), persiste les
//! `device_capability_instances` et renvoie les `PinSpec` du
//! `ProvisionAck`. Point d'extension unique pour les policies
//! `Profiled`/`Locked` (P5).

use loco_rs::prelude::*;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::collections::{HashMap, HashSet};

use crate::models::_entities::{
    device_capability_instances, device_registries, mcu_boards, predefined_devices,
};
use pnex_core::{Mode, ModeOpts, PinDecl, PinSpec};
/// Erreur d'admission — message en clair pour `Reject{reason}` / 400 REST.
pub(crate) struct AdmissionError(pub String);

impl From<AdmissionError> for Error {
    fn from(e: AdmissionError) -> Self {
        Error::string(&e.0)
    }
}

/// Mode fil → code (helpers locaux, miroirs de ws_device::str_to_mode).
fn str_to_mode_local(s: &str) -> Mode {
    match s {
        "digital_out" => Mode::DigitalOut,
        "pwm_out" => Mode::PwmOut,
        // `analog_in` = base naming; `adc_in` = the wire string (alias).
        "analog_in" | "adc_in" => Mode::AdcIn,
        _ => Mode::DigitalIn,
    }
}

fn mode_str(m: Mode) -> &'static str {
    match m {
        Mode::DigitalIn => "digital_in",
        Mode::DigitalOut => "digital_out",
        Mode::PwmOut => "pwm_out",
        Mode::AdcIn => "analog_in",
    }
}

/// Board du device : la variante **figée à l'enregistrement**
/// (`device_registries.board_id`) d'abord, défaut du
/// modèle en secours (row supprimée = résilience, jamais d'erreur).
pub(crate) async fn device_board(
    db: &DatabaseConnection,
    device: &device_registries::Model,
) -> Result<mcu_boards::Model> {
    if let Some(bid) = board_id_of(device) {
        if let Some(b) = mcu_boards::Entity::find_by_id(bid)
            .one(db)
            .await
            .map_err(|_| Error::InternalServerError)?
        {
            return Ok(b);
        }
    }
    let pd = predefined_devices::Entity::find_by_id(device.predefined_device_id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or_else(|| AdmissionError("predefined device introuvable".into()))?;
    let board = mcu_boards::Entity::find_by_id(pd.board_id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or_else(|| AdmissionError("board du device introuvable".into()))?;
    Ok(board)
}

/// Id de la board figée du device (`device_registries.board_id`).
pub(crate) fn board_id_of(device: &device_registries::Model) -> Option<i64> {
    device.board_id
}

/// Parse l'état périphériques du device (jsonb, défaut = tout désactivé).
pub(crate) fn device_peripherals(
    device: &device_registries::Model,
) -> pnex_core::DevicePeripherals {
    device
        .peripherals
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default()
}

/// Screen compiled into the firmware for this device — resolved from the
/// frozen board profile (builtin wins, then the user's pick, fail closed on
/// unknown kind) or the predefined model declaration (black boxes).
/// `None` = no screen compiled.
pub(crate) fn screen_for_build(
    details: Option<&pnex_core::BoardDetails>,
    predefined_peripherals: Option<&serde_json::Value>,
    peripherals: &pnex_core::DevicePeripherals,
) -> Option<pnex_firmware_builder::ScreenSpec> {
    // 1. Board profile v2 — the single source of the wiring (roles → gpios).
    if let Some(details) = details {
        if let Some(screen) = details.resolved_screen(peripherals) {
            return Some(pnex_firmware_builder::ScreenSpec {
                kind: screen.kind.clone(),
                pins: screen
                    .pins
                    .iter()
                    .map(|(role, gpio)| (role.clone(), *gpio))
                    .collect(),
            });
        }
    }
    // 2. Model declaration fallback (black boxes): `{"screen": {"kind": …}}`.
    let screen = predefined_peripherals?.get("screen")?;
    let kind = match &peripherals.screen {
        pnex_core::boards::ScreenChoice::None => return None,
        pnex_core::boards::ScreenChoice::LegacyAny => screen.get("kind")?.as_str()?.to_string(),
        pnex_core::boards::ScreenChoice::Kind(k) => k.clone(),
    };
    Some(pnex_firmware_builder::ScreenSpec { kind, pins: vec![] })
}

/// Détails board du device (device → variante figée ou défaut modèle →
/// `mcu_boards.details`), parse tolérant v1/v2. `Ok(None)` si détails
/// absents ou illisibles (warn) — jamais d'erreur : l'appelant décide
/// (admission Tier 1 vs Tier 2, pinout, éditeur).
pub(crate) async fn load_board_details(
    db: &DatabaseConnection,
    device: &device_registries::Model,
) -> Result<Option<pnex_core::BoardDetails>> {
    let board = device_board(db, device).await?;
    let Some(details) = board.details.as_ref() else {
        return Ok(None);
    };
    match serde_json::from_value::<pnex_core::BoardDetails>(details.clone()) {
        Ok(d) => Ok(Some(d)),
        Err(e) => {
            tracing::warn!(
                board = %board.name,
                error = %e,
                "détails board illisibles (v1/v2) — traités comme pas de profil"
            );
            Ok(None)
        }
    }
}

/// Charge et parse l'overlay **d'admission** du device (mcu_boards.details →
/// BoardOverlay) avec le SoC du board. v1 → passthrough ; v2 → pins du
/// profil filtrées chip-caps (`admission_pins`) **et** périphériques
/// activés (`reserved_gpios`) — un pin écran activé n'obtient JAMAIS
/// d'instance. Erreur explicite si le device n'est pas générique (pas de
/// profil) — le message sert au Reject d'admission.
pub(crate) async fn load_overlay(
    db: &DatabaseConnection,
    device: &device_registries::Model,
) -> Result<(pnex_core::BoardOverlay, pnex_core::Soc)> {
    let board = device_board(db, device).await?;
    let soc = pnex_core::Soc::from_board_soc(&board.soc).ok_or_else(|| {
        Error::string(&format!(
            "soc « {} » du board {} non géré par les chip-caps",
            board.soc, board.name
        ))
    })?;
    let Some(details) = board.details.as_ref() else {
        return Err(Error::string(
            "pas d'overlay board (mcu_boards.details) pour ce device — /ws/device est réservé aux devices génériques",
        ));
    };
    let details: pnex_core::BoardDetails = serde_json::from_value(details.clone())
        .map_err(|e| Error::string(&format!("overlay board invalide : {e}")))?;
    let peripherals = device_peripherals(device);
    let overlay = match &details {
        pnex_core::BoardDetails::V1(o) => o.clone(),
        pnex_core::BoardDetails::V2(p) => {
            let mut pins = details.admission_pins(soc);
            let reserved: HashSet<u16> = details.reserved_gpios(&peripherals).into_iter().collect();
            pins.retain(|pin| !reserved.contains(&pin.gpio));
            pnex_core::BoardOverlay {
                board: p.board.clone(),
                pins,
            }
        }
    };
    Ok((overlay, soc))
}

/// SoC du board d'un device (device → predefined → board) — pour les
/// validations `caps::validate` hors admission (push de commandes, cast
/// régulation). Fallback Tier 2 : le soc **observé** à l'admission
/// (persisté sur le device) quand le soc du board est
/// inconnu des chip-caps (« generic »).
pub(crate) async fn device_soc(
    db: &DatabaseConnection,
    device: &device_registries::Model,
) -> Result<pnex_core::Soc> {
    let board = device_board(db, device).await?;
    match pnex_core::Soc::from_board_soc(&board.soc) {
        Some(soc) => Ok(soc),
        None => device
            .soc
            .as_deref()
            .and_then(pnex_core::Soc::from_board_soc)
            .ok_or_else(|| {
                Error::string(&format!(
                    "soc « {} » du board {} non géré par les chip-caps et aucun soc observé — connectez le device une fois pour l'enregistrer",
                    board.soc, board.name
                ))
            }),
    }
}

/// SoC **observé** : persiste le chip annoncé (si reconnu) sur le device
/// au premier announce — best-effort, l'admission n'en dépend pas.
pub(crate) async fn persist_observed_soc(db: &DatabaseConnection, device_id: i64, chip: &str) {
    let Some(soc_str) = pnex_core::Soc::from_board_soc(chip) else {
        return; // chip inconnu : rien à enregistrer (l'admission a warn)
    };
    let device = device_registries::Entity::find_by_id(device_id)
        .one(db)
        .await;
    if let Ok(Some(d)) = device {
        let mut am: device_registries::ActiveModel = d.into();
        am.soc = Set(Some(soc_str.name().to_string()));
        let _ = am.update(db).await; // best-effort
    }
}

/// Manifeste d'admission : pins déclarées par le sketch + SoC déduit du
/// chip annoncé (`None` = chip inconnu → admission permissive, warn).
pub(crate) struct AdmissionManifest<'a> {
    pub soc: Option<pnex_core::Soc>,
    pub pins: Option<&'a [PinDecl]>,
}

/// Admission à l'`Announce` (policy Validated, B0.3) : dérive → valide →
/// persiste → renvoie la pin map du `ProvisionAck`.
///
/// - **Tier 1 (overlay board)** : l'overlay reste l'autorité — upsert
///   inchangé ; un manifeste annoncé divergent est journalisé (warn).
/// - **Tier 2 (custom, sans overlay)** : les pins déclarées dans le
///   sketch font foi — mode/label/opts du sketch appliqués à CHAQUE
///   announce (un changement de sketch s'applique au re-announce), les
///   gpio non déclarés sont purgés, `interval_ms` préservé (leçon
///   2026-09-03).
/// - Ni overlay ni pins → erreur d'admission (Reject à l'announce).
pub(crate) async fn admit(
    db: &DatabaseConnection,
    device: &device_registries::Model,
    manifest: Option<AdmissionManifest<'_>>,
) -> Result<Vec<PinSpec>> {
    match load_overlay(db, device).await {
        Ok((overlay, soc)) => {
            if let Some(pins) = manifest.as_ref().and_then(|m| m.pins) {
                tracing::warn!(
                    device_id = %device.device_id,
                    n = pins.len(),
                    "pins déclarées par un device overlay — ignorées (overlay board = autorité)"
                );
            }
            admit_overlay(db, device, overlay, soc).await
        }
        // Pas d'overlay (custom) : le manifeste sketch fait foi.
        Err(_) => admit_manifest(db, device, manifest).await,
    }
}

/// Tier 1 — admission par overlay board (comportement historique).
async fn admit_overlay(
    db: &DatabaseConnection,
    device: &device_registries::Model,
    overlay: pnex_core::BoardOverlay,
    soc: pnex_core::Soc,
) -> Result<Vec<PinSpec>> {
    let existing: HashMap<i32, device_capability_instances::Model> =
        device_capability_instances::Entity::find()
            .filter(device_capability_instances::Column::DeviceRegistryId.eq(device.id))
            .all(db)
            .await?
            .into_iter()
            .map(|r| (r.gpio, r))
            .collect();
    let mut specs = Vec::with_capacity(overlay.pins.len());
    for pin in &overlay.pins {
        // UART0 console (TX/RX) stays with the firmware's Serial: never
        // provisioned (v1 overlays still list it; v2 profiles already drop
        // it in admission_pins) rather than failing the whole admission.
        if pnex_core::caps::is_console_pin(soc, pin.gpio) {
            continue;
        }
        // Mode par défaut dérivé des chip-caps (v1 ET v2) : DigitalIn si
        // légal, sinon AdcIn (convention A0 esp8266 — gpio 17). Préserve le
        // comportement historique : A0 → analog_in, xiao D0–D3 → digital_in.
        let default_mode =
            if pnex_core::caps::validate(soc, pin.gpio, Mode::DigitalIn, &ModeOpts::default())
                .is_ok()
            {
                Mode::DigitalIn
            } else {
                Mode::AdcIn
            };
        // Board-wired default (e.g. an onboard LED = digital_out, safe
        // state high for an active-low one), kept only when legal.
        let (default_mode, default_cfg) = match pin.default_mode {
            Some(m) => {
                let opts = ModeOpts {
                    pullup: None,
                    safe_state: pin.safe_state,
                };
                if pnex_core::caps::validate(soc, pin.gpio, m, &opts).is_ok() {
                    (m, opts)
                } else {
                    (default_mode, ModeOpts::default())
                }
            }
            None => (default_mode, ModeOpts::default()),
        };
        // Un pin déjà configuré garde son mode/config (survit aux re-announce).
        let row = existing.get(&(pin.gpio as i32));
        let mode = row
            .map(|r| str_to_mode_local(&r.mode))
            .unwrap_or(default_mode);
        let cfg: ModeOpts = match row {
            Some(r) => r
                .config
                .as_ref()
                .and_then(|c| serde_json::from_value(c.clone()).ok())
                .unwrap_or_default(),
            None => default_cfg,
        };
        let validated = pnex_core::caps::validate(soc, pin.gpio, mode, &cfg).map_err(|v| {
            Error::string(&format!(
                "pin {} (gpio {}) : {}",
                pin.label,
                pin.gpio,
                v.reason()
            ))
        })?;
        match row {
            Some(r) => {
                let mut am: device_capability_instances::ActiveModel = r.clone().into();
                am.label = Set(pin.label.clone());
                am.mode = Set(mode_str(mode).to_string());
                // ⚠ config conservée TELLE QUELLE (pullup, safe_state ET
                // interval_ms) : le round-trip par `ModeOpts` — sans champ
                // interval_ms — réécrivait la config à CHAQUE re-announce et
                // effaçait la cadence persistée (« Read every 1 s » perdu au
                // premier reconnect — leçon 2026-09-03).
                am.constraints_snapshot =
                    Set(Some(serde_json::to_value(validated).unwrap_or_default()));
                am.enabled = Set(true);
                am.update(db).await?;
            }
            None => {
                device_capability_instances::ActiveModel {
                    device_registry_id: Set(device.id),
                    gpio: Set(pin.gpio as i32),
                    label: Set(pin.label.clone()),
                    mode: Set(mode_str(mode).to_string()),
                    config: Set(Some(serde_json::to_value(cfg).unwrap_or_default())),
                    constraints_snapshot: Set(Some(
                        serde_json::to_value(validated).unwrap_or_default(),
                    )),
                    enabled: Set(true),
                    ..Default::default()
                }
                .insert(db)
                .await?;
            }
        }
        specs.push(PinSpec {
            gpio: pin.gpio,
            label: pin.label.clone(),
            mode,
            safe_state: Some(validated.safe_state),
            // Round-trip du pullup persisté (avant : réinitialisé au device
            // à chaque re-announce, PinSpec ne portait pas opts).
            opts: Some(ModeOpts {
                pullup: cfg.pullup,
                safe_state: Some(validated.safe_state),
            }),
        });
    }
    // Rows provisioned before the console rule: disabled (hidden from the
    // pins UI, rejected by commands) instead of lingering as live pins.
    for r in existing.values() {
        if r.enabled && r.gpio >= 0 && pnex_core::caps::is_console_pin(soc, r.gpio as u16) {
            let mut am: device_capability_instances::ActiveModel = r.clone().into();
            am.enabled = Set(false);
            am.update(db).await?;
        }
    }
    Ok(specs)
}

/// Tier 2 — admission par les pins déclarées dans le sketch (custom,
/// sans overlay). Validation chip-caps stricte si le SoC est connu,
/// permissive sinon (chip inconnu, warn). Upsert : le sketch est la
/// source (mode/label/opts réappliqués), `interval_ms` conservé, gpio
/// non déclarés purgés.
async fn admit_manifest(
    db: &DatabaseConnection,
    device: &device_registries::Model,
    manifest: Option<AdmissionManifest<'_>>,
) -> Result<Vec<PinSpec>> {
    let declared_pins: &[PinDecl] = match manifest.as_ref().and_then(|m| m.pins) {
        Some(pins) if !pins.is_empty() => pins,
        _ => {
            return Err(Error::from(AdmissionError(
                "device sans overlay board ni pins déclarées dans l'announce — /ws/device requiert des pins (sketch) ou un overlay"
                    .into(),
            )));
        }
    };
    let soc = manifest.and_then(|m| m.soc);
    if soc.is_none() {
        tracing::warn!(
            device_id = %device.device_id,
            "chip non reconnu — admission permissive (pas de validation chip-caps)"
        );
    }
    let existing: HashMap<i32, device_capability_instances::Model> =
        device_capability_instances::Entity::find()
            .filter(device_capability_instances::Column::DeviceRegistryId.eq(device.id))
            .all(db)
            .await?
            .into_iter()
            .map(|r| (r.gpio, r))
            .collect();
    let mut specs = Vec::with_capacity(declared_pins.len());
    let mut kept_gpios: HashSet<i32> = HashSet::with_capacity(declared_pins.len());
    for decl in declared_pins {
        // Same console rule as the overlay path: skipped (then pruned), the
        // rest of the sketch is still admitted.
        if soc.is_some_and(|s| pnex_core::caps::is_console_pin(s, decl.gpio)) {
            tracing::warn!(
                device_id = %device.device_id,
                gpio = decl.gpio,
                "declared pin is the UART0 console (TX/RX) — not provisioned"
            );
            continue;
        }
        let label = {
            let t = decl.label.trim();
            if t.is_empty() {
                format!("gpio{}", decl.gpio)
            } else {
                t.to_string()
            }
        };
        let mode = decl.mode;
        let cfg = ModeOpts {
            pullup: decl.pullup,
            safe_state: decl.safe_state,
        };
        // SoC connu → chip-caps strictes ; inconnu → permissif (défauts).
        let validated = match soc {
            Some(s) => pnex_core::caps::validate(s, decl.gpio, mode, &cfg).map_err(|v| {
                Error::string(&format!(
                    "pin {label} (gpio {}) : {}",
                    decl.gpio,
                    v.reason()
                ))
            })?,
            None => pnex_core::ValidatedPin {
                gpio: decl.gpio,
                mode,
                pullup: cfg.pullup.unwrap_or(false),
                safe_state: cfg.safe_state.unwrap_or(pnex_core::SafeState::Low),
            },
        };
        let row = existing.get(&(decl.gpio as i32));
        // interval_ms conservé en vif dans le jsonb (ne JAMAIS réécrire la
        // config entière — leçon 2026-09-03 : cadence effacée sinon).
        let interval_ms = row
            .and_then(|r| r.config.as_ref())
            .and_then(|c| c.get("interval_ms"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let mut cfg_json = serde_json::to_value(cfg).unwrap_or_default();
        if interval_ms > 0 {
            cfg_json["interval_ms"] = serde_json::json!(interval_ms);
        }
        match row {
            Some(r) => {
                // Sketch = source : mode/label/opts réappliqués (un
                // changement de sketch s'applique au re-announce) ; seul
                // interval_ms survit de la config précédente.
                let mut am: device_capability_instances::ActiveModel = r.clone().into();
                am.label = Set(label.clone());
                am.mode = Set(mode_str(mode).to_string());
                am.config = Set(Some(cfg_json));
                am.constraints_snapshot =
                    Set(Some(serde_json::to_value(validated).unwrap_or_default()));
                am.enabled = Set(true);
                am.update(db).await?;
            }
            None => {
                device_capability_instances::ActiveModel {
                    device_registry_id: Set(device.id),
                    gpio: Set(decl.gpio as i32),
                    label: Set(label.clone()),
                    mode: Set(mode_str(mode).to_string()),
                    config: Set(Some(cfg_json)),
                    constraints_snapshot: Set(Some(
                        serde_json::to_value(validated).unwrap_or_default(),
                    )),
                    enabled: Set(true),
                    ..Default::default()
                }
                .insert(db)
                .await?;
            }
        }
        kept_gpios.insert(decl.gpio as i32);
        specs.push(PinSpec {
            gpio: decl.gpio,
            label,
            mode,
            safe_state: Some(validated.safe_state),
            opts: Some(ModeOpts {
                pullup: Some(validated.pullup),
                safe_state: Some(validated.safe_state),
            }),
        });
    }
    // Prune : le sketch est la source — un gpio non déclaré n'existe plus
    // (sa cadence disparaît avec lui ; l'UI /pins ne l'affichera plus).
    for r in existing.values() {
        if !kept_gpios.contains(&r.gpio) {
            device_capability_instances::Entity::delete_by_id(r.id)
                .exec(db)
                .await?;
        }
    }
    Ok(specs)
}
