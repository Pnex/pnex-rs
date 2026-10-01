use super::*;

/// `PUT /api/v1/devices/{id}/peripherals` — état des périphériques intégrés
/// choisis par l'utilisateur (écran actif = pins réservées côté serveur +
/// define firmware au rebuild). Autorisé si le device peut porter un écran :
/// profil v2 de sa board (`screens[]`) **ou** déclaration du modèle
/// (`predefined_devices.peripherals.screen`).
pub(super) async fn update_peripherals(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(body): Json<pnex_core::UpdateDevicePeripherals>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "device-write-forbidden",
            "Owner, admin or member role required to manage devices.",
        ));
    }
    let Some(device) = find_device(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    // Sources de la capacité écran : profil v2 de la board figée, ou le
    // modèle prédéfini lui-même (boîtes noires à écran).
    let profile_screens = crate::services::provisioning::load_board_details(&ctx.db, &device)
        .await?
        .and_then(|d| d.v2().map(|p| p.peripherals.screens.clone()))
        .unwrap_or_default();
    let predefined = predefined_devices::Entity::find_by_id(device.predefined_device_id)
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let model_screen_kind = predefined.as_ref().and_then(screen_capability);

    // Soldered (builtin) screen: disabling it is refused — it is wired on
    // the board, the pins stay reserved for good.
    if body.screen == ScreenChoice::None {
        if let Some(builtin) = profile_screens.iter().find(|s| s.builtin) {
            return Ok(detail_status(
                StatusCode::BAD_REQUEST,
                &format!(
                    "Soldered builtin screen — it cannot be disabled ({}).",
                    builtin.name
                ),
            ));
        }
    }

    // Resolve the request against the declared capacity (board profile
    // first, model declaration as fallback) into a concrete driver kind.
    let kind: Option<String> = if !profile_screens.is_empty() {
        match &body.screen {
            ScreenChoice::None => None,
            ScreenChoice::LegacyAny => profile_screens.first().map(|s| s.kind.clone()),
            ScreenChoice::Kind(k) => match profile_screens.iter().find(|s| s.kind == *k) {
                Some(_) => Some(k.clone()),
                None => {
                    let declared: Vec<&str> =
                        profile_screens.iter().map(|s| s.kind.as_str()).collect();
                    return Ok(detail_status(
                        StatusCode::BAD_REQUEST,
                        &format!(
                            "Screen \"{k}\" is not declared on the device board (declared screens: {}).",
                            declared.join(", ")
                        ),
                    ));
                }
            },
        }
    } else if let Some(model_kind) = model_screen_kind {
        let requested = match &body.screen {
            ScreenChoice::None => None,
            ScreenChoice::LegacyAny => Some(model_kind.clone()),
            ScreenChoice::Kind(k) => Some(k.clone()),
        };
        match requested {
            // Legacy `true` = the model's screen; kind must match exactly.
            Some(k) if k == model_kind => Some(model_kind),
            Some(k) => {
                return Ok(detail_status(
                    StatusCode::BAD_REQUEST,
                    &format!(
                        "Screen \"{k}\" is not declared by the device model (declared screen: {model_kind})."
                    ),
                ));
            }
            None => None,
        }
    } else if !matches!(body.screen, ScreenChoice::None) {
        return Ok(detail_status(
            StatusCode::BAD_REQUEST,
            "This device cannot carry a builtin screen (none declared on its board profile nor its model).",
        ));
    } else {
        None
    };

    // Wire value: never persist the legacy `true` — the concrete kind (or
    // null) so the build resolution stays unambiguous.
    let wire = kind.clone().map(serde_json::Value::from);
    let mut am: device_registries::ActiveModel = device.into();
    am.peripherals = Set(Some(serde_json::json!({ "screen": wire })));
    am.update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    format::json(serde_json::json!({
        "screen": wire,
        "screen_kind": kind,
        // Le define firmware ne prendra effet qu'au prochain build.
        "rebuild_required": true,
    }))
}

/// Capacité écran du modèle (`predefined_devices.peripherals` jsonb, ex.
/// `{"screen": {"kind": "ssd1306"}}`) → kind du driver.
pub(super) fn screen_capability(pd: &predefined_devices::Model) -> Option<String> {
    pd.peripherals
        .as_ref()
        .and_then(|p| p.get("screen"))
        .and_then(|s| s.get("kind"))
        .and_then(|k| k.as_str())
        .map(str::to_string)
}
