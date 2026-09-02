//! Provisioning Brick 0 — admission à l'`Announce` (policy `Validated`, B0.3).
//!
//! Dérive la carte de pins de l'overlay (`mcu_boards.details` →
//! `BoardOverlay`), valide chaque pin contre les chip-caps
//! (`pnex_core::caps::validate` — point unique), persiste les
//! `device_capability_instances` et renvoie les `PinSpec` du
//! `ProvisionAck`. Point d'extension unique pour les policies
//! `Profiled`/`Locked` (P5).

use loco_rs::prelude::*;
use std::collections::HashMap;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use crate::models::_entities::{device_capability_instances, device_registries, mcu_boards, predefined_devices};
use pnex_core::{Mode, ModeOpts, PinSpec};
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
        "analog_in" => Mode::AdcIn,
        _ => Mode::DigitalIn,
    }
}

fn mode_str(m: Mode) -> &'static str {
    match m {
        Mode::DigitalIn => "digital_in",
        Mode::DigitalOut => "digital_out",
        Mode::AdcIn => "analog_in",
    }
}

/// Charge et parse l'overlay board du device (mcu_boards.details → BoardOverlay).
/// Erreur explicite si le device n'est pas générique (pas d'overlay) — le
/// message sert au Reject 4007/annadmission.
pub(crate) async fn load_overlay(
    db: &DatabaseConnection,
    device: &device_registries::Model,
) -> Result<pnex_core::BoardOverlay> {
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
    let Some(details) = board.details.as_ref() else {
        return Err(Error::string(
            "pas d'overlay board (mcu_boards.details) pour ce device — /ws/device est réservé aux devices génériques",
        ));
    };
    serde_json::from_value(details.clone())
        .map_err(|e| Error::string(&format!("overlay board invalide : {e}")))
}

/// Admission à l'`Announce` (policy Validated, B0.3) : dérive → valide →
/// persiste → renvoie la pin map du `ProvisionAck`. **Upsert** : les modes
/// choisis par l'utilisateur (SetMode) survivent à un re-announce — seuls
/// les pins nouveaux prennent le défaut overlay (digital_in / analog_in).
pub(crate) async fn admit(
    db: &DatabaseConnection,
    device: &device_registries::Model,
) -> Result<Vec<PinSpec>> {
    let overlay = load_overlay(db, device).await?;
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
        let default_mode = match pin.kind {
            pnex_core::PinKind::Analog => Mode::AdcIn,
            pnex_core::PinKind::Digital => Mode::DigitalIn,
        };
        // Un pin déjà configuré garde son mode/config (survit aux re-announce).
        let row = existing.get(&(pin.gpio as i32));
        let mode = row
            .map(|r| str_to_mode_local(&r.mode))
            .unwrap_or(default_mode);
        let cfg: ModeOpts = row
            .and_then(|r| r.config.as_ref())
            .and_then(|c| serde_json::from_value(c.clone()).ok())
            .unwrap_or_default();
        let validated = pnex_core::caps::validate(pin.gpio, mode, &cfg).map_err(|v| {
            Error::string(&format!(
                "pin {} (gpio {}) : {}",
                pin.label, pin.gpio, v.reason()
            ))
        })?;
        match row {
            Some(r) => {
                let mut am: device_capability_instances::ActiveModel = r.clone().into();
                am.label = Set(pin.label.clone());
                am.mode = Set(mode_str(mode).to_string());
                am.config = Set(Some(serde_json::to_value(cfg).unwrap_or_default()));
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
        });
    }
    Ok(specs)
}
