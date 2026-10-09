//! `PUT /api/v1/devices/{id}/board` (O39): a device moves to another board
//! variant of its chip, the soldered screen follows the board, write roles
//! only (R2), org-scoped (R1).

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::models::_entities::{
    device_registries, mcu_boards, organization_members, sea_orm_active_enums::OrgMemberRole,
};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serial_test::serial;

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, loco_rs::app::AppContext, String, String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let alice = common::valid_token(
        &base,
        "brd-alice-000000000000000",
        "alice",
        "brd-alice@example.com",
    );
    let bob = common::valid_token(
        &base,
        "brd-bob-00000000000000000",
        "bob",
        "brd-bob@example.com",
    );
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, ctx, alice, bob).await;
        },
    )
    .await;
}

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

/// Inserts a catalogue board (profile v2 included), as the boot seed does.
async fn insert_board(
    db: &sea_orm::DatabaseConnection,
    board: pnex_core::catalog::CatalogBoard,
) -> i64 {
    let mut am = <mcu_boards::ActiveModel as Default>::default();
    am.name = Set(board.name.to_string());
    am.soc = Set(board.soc_str().to_string());
    am.pretty_name = Set(board.pretty_name.map(str::to_string));
    am.pio_board = Set(board.pio_board.map(str::to_string));
    am.details = Set(board.profile.map(|p| serde_json::to_value(p()).unwrap()));
    am.insert(db).await.expect("board").id
}

fn put_board(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    device: i64,
    board: i64,
) -> axum_test::TestRequest {
    server
        .put(&format!("/api/v1/devices/{device}/board"))
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({ "board_id": board }))
}

#[tokio::test]
#[serial]
async fn board_change_follows_the_chip_and_moves_the_soldered_screen() {
    with_app(|server, ctx, alice, bob| async move {
        let org = personal_org(&server, &alice).await;
        let bob_org = personal_org(&server, &bob).await;
        // O39: registered without board_id → default NodeMCU board.
        let res = server
            .post("/api/v1/devices")
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "device_id": "o39-nodemcu",
                "predefined_device_name": "generic_esp8266",
            }))
            .await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let device = res.json::<serde_json::Value>()["id"].as_i64().unwrap();
        let default_board = device_registries::Entity::find_by_id(device)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap()
            .board_id
            .unwrap();

        let oled = insert_board(&ctx.db, pnex_core::catalog::boards::NODEMCU_V3_OLED).await;
        let c6 = insert_board(&ctx.db, pnex_core::catalog::boards::WAVESHARE_ESP32C6_ZERO).await;

        // Another org never sees the device (R1).
        put_board(&server, &bob, bob_org, device, oled)
            .await
            .assert_status(axum_test::http::StatusCode::NOT_FOUND);

        // Unknown board, other chip: refused with a machine code.
        let res = put_board(&server, &alice, org, device, oled + 100_000).await;
        res.assert_status(axum_test::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            res.json::<serde_json::Value>()["error"],
            "device-board-unknown"
        );
        let res = put_board(&server, &alice, org, device, c6).await;
        res.assert_status(axum_test::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            res.json::<serde_json::Value>()["error"],
            "device-board-soc-mismatch"
        );

        // Same chip: moved, the soldered OLED is turned on.
        let res = put_board(&server, &alice, org, device, oled).await;
        res.assert_status_ok();
        assert_eq!(res.json::<serde_json::Value>()["rebuild_required"], true);
        let row = device_registries::Entity::find_by_id(device)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.board_id, Some(oled));
        assert_eq!(
            row.peripherals
                .as_ref()
                .and_then(|p| p.get("screen"))
                .cloned(),
            Some(serde_json::json!("ssd1306"))
        );

        // Back to the default board: the soldered screen goes away with it.
        put_board(&server, &alice, org, device, default_board)
            .await
            .assert_status_ok();
        let row = device_registries::Entity::find_by_id(device)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.board_id, Some(default_board));
        assert_eq!(
            row.peripherals
                .as_ref()
                .and_then(|p| p.get("screen"))
                .cloned(),
            Some(serde_json::Value::Null)
        );

        // A viewer of the org cannot change it (R2).
        let bob_id = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {bob}"))
            .await
            .json::<serde_json::Value>()["id"]
            .as_i64()
            .unwrap();
        organization_members::ActiveModel {
            org_id: Set(org),
            user_id: Set(bob_id),
            role: Set(OrgMemberRole::Viewer),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        let res = put_board(&server, &bob, org, device, oled).await;
        res.assert_status(axum_test::http::StatusCode::FORBIDDEN);
        assert_eq!(
            res.json::<serde_json::Value>()["error"],
            "device-write-forbidden"
        );
        let row = device_registries::Entity::find_by_id(device)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.board_id, Some(default_board));
    })
    .await;
}
