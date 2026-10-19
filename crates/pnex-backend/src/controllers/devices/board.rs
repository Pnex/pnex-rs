use super::*;

/// `PUT /api/v1/devices/{id}/board` — moves the device to another board
/// variant of the same chip (O39: a NodeMCU registered on the default
/// board instead of its OLED variant had to be deleted and registered
/// again). The board stays frozen in between: only this explicit change
/// moves it, and it takes effect at the next build.
///
/// The pin instances survive (they are validated against the chip, not the
/// board). A soldered screen follows the board: the new board's builtin
/// screen is turned on, the old board's one is turned off.
#[utoipa::path(
    put,
    path = "/api/v1/devices/{id}/board",
    tag = "devices",
    operation_id = "devices_update_board",
    summary = "Move the device to another board variant of the same chip",
    description = "Takes effect at the next build. Pin instances are kept; the builtin screen follows the board.",
    params(("id" = i64, Path, description = "Device registry id")),
    request_body = pnex_core::UpdateDeviceBoard,
    responses(
        (status = 200, body = pnex_core::DeviceBoardUpdated),
        (status = 403, body = crate::openapi::ErrorDetail),
        (status = 404, body = crate::openapi::ErrorDetail),
        (status = 422, description = "Unknown board or other chip", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn update_board(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(body): Json<pnex_core::UpdateDeviceBoard>,
) -> Result<Response> {
    let Some(device) = find_device(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let Some(target) = mcu_boards::Entity::find_by_id(body.board_id)
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::CustomError(
            StatusCode::UNPROCESSABLE_ENTITY,
            loco_rs::controller::ErrorDetail::new(
                err_codes::DEVICE_BOARD_UNKNOWN,
                "Unknown board.",
            ),
        ));
    };
    let current = crate::services::provisioning::device_board(&ctx.db, &device).await?;
    if current.id == target.id {
        return format::json(pnex_core::DeviceBoardUpdated {
            board_id: target.id,
            rebuild_required: false,
        });
    }
    // Same rule as at registration: the chip-caps SoC, or the raw value for
    // boards with an unknown SoC.
    let compatible = match (
        pnex_core::Soc::from_board_soc(&target.soc),
        pnex_core::Soc::from_board_soc(&current.soc),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => target.soc == current.soc,
    };
    if !compatible {
        return Err(Error::CustomError(
            StatusCode::UNPROCESSABLE_ENTITY,
            loco_rs::controller::ErrorDetail::new(
                err_codes::DEVICE_BOARD_SOC_MISMATCH,
                "The board does not carry the device's chip.",
            ),
        ));
    }

    let builtin_screen = |board: &mcu_boards::Model| -> Option<String> {
        board
            .details
            .clone()
            .and_then(|d| serde_json::from_value::<pnex_core::BoardDetails>(d).ok())
            .and_then(|p| {
                p.peripherals
                    .screens
                    .iter()
                    .find(|s| s.builtin)
                    .map(|s| s.kind.clone())
            })
    };
    // A custom firmware owns every pin: no screen is ever seeded for it.
    let peripherals = if device.firmware_project_id.is_some() {
        device.peripherals.clone()
    } else {
        match (builtin_screen(&current), builtin_screen(&target)) {
            (_, Some(kind)) => Some(serde_json::json!({ "screen": kind })),
            (Some(_), None) => Some(serde_json::json!({ "screen": null })),
            (None, None) => device.peripherals.clone(),
        }
    };

    let mut am: device_registries::ActiveModel = device.into();
    am.board_id = Set(Some(target.id));
    am.peripherals = Set(peripherals);
    am.update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    tracing::info!(
        device = id,
        from = current.id,
        to = target.id,
        by = org.auth.user.id,
        "device board changed"
    );
    format::json(pnex_core::DeviceBoardUpdated {
        board_id: target.id,
        rebuild_required: true,
    })
}
