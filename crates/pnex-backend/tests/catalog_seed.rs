//! Catalog seed: the device tables mirror the typed registry
//! (`pnex_core::catalog`, D121) and a second run changes nothing.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::models::_entities::{
    device_capabilities, device_types, mcu_boards, predefined_device_capabilities,
    predefined_devices,
};
use pnex_core::catalog;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use serial_test::serial;

/// Restores the mutated env vars, even on panic.
struct EnvGuard(Vec<(&'static str, Option<String>)>);

impl EnvGuard {
    fn capture(keys: &[&'static str]) -> Self {
        Self(keys.iter().map(|k| (*k, std::env::var(k).ok())).collect())
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, prev) in self.0.drain(..) {
            match prev {
                Some(v) => unsafe { std::env::set_var(key, v) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

#[tokio::test]
#[serial]
async fn seed_writes_the_typed_catalog_idempotently() {
    let _guard = EnvGuard::capture(&["RAUTHY_URL"]);
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };

    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<pnex_backend::app::App, _, _>(
        config,
        move |_server, ctx| async move {
            let fixtures = std::path::Path::new("fixtures");
            for _ in 0..2 {
                pnex_backend::tasks::seed::seed_catalog(&ctx.db, fixtures, false)
                    .await
                    .expect("seed");
            }

            let count = device_types::Entity::find().count(&ctx.db).await.unwrap();
            assert_eq!(count as usize, catalog::DeviceType::ALL.len());
            let count = device_capabilities::Entity::find()
                .count(&ctx.db)
                .await
                .unwrap();
            assert_eq!(count as usize, catalog::Capability::ALL.len());

            let rows = mcu_boards::Entity::find().all(&ctx.db).await.unwrap();
            assert_eq!(rows.len(), catalog::boards().len());
            for b in catalog::boards() {
                let row = rows.iter().find(|r| r.name == b.name).expect(b.name);
                assert_eq!(row.soc, b.soc_str(), "{}", b.name);
                assert_eq!(row.pretty_name.as_deref(), b.pretty_name, "{}", b.name);
                assert_eq!(row.pio_board.as_deref(), b.pio_board, "{}", b.name);
                let want = b.profile.map(|p| serde_json::to_value(p()).unwrap());
                assert_eq!(row.details, want, "{}", b.name);
            }

            let rows = predefined_devices::Entity::find()
                .all(&ctx.db)
                .await
                .unwrap();
            assert_eq!(rows.len(), catalog::products().len());
            for p in catalog::products() {
                let row = rows.iter().find(|r| r.name == p.name).expect(p.name);
                let board = mcu_boards::Entity::find_by_id(row.board_id)
                    .one(&ctx.db)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(board.name, p.board.name);
                assert_eq!(row.revision, p.revision);
                assert_eq!(row.description_i18n.as_deref(), Some(p.description_i18n));
                assert_eq!(
                    row.prestashop_product_id.as_deref(),
                    p.shop.map(|s| s.prestashop_product_id)
                );
                let links = predefined_device_capabilities::Entity::find()
                    .filter(predefined_device_capabilities::Column::PredefinedDeviceId.eq(row.id))
                    .count(&ctx.db)
                    .await
                    .unwrap();
                assert_eq!(links as usize, p.capabilities.len(), "{}", p.name);
            }
        },
    )
    .await;
}
