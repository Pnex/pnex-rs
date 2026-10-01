//! Catalog seed: organizations created without a subscription tier (e.g. an
//! instance switched from self-hosted to SaaS) get the default tier once the
//! tiers are seeded; the self-hosted seed leaves them untouched.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::models::_entities::{organizations, subscription_tiers};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
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
async fn saas_seed_backfills_orgs_without_tier() {
    let dir = tempfile::tempdir().expect("tmp");
    let uri = format!(
        "sqlite://{}/pnex_seed_backfill.sqlite?mode=rwc",
        dir.path().display()
    );

    let _guard = EnvGuard::capture(&["TEST_DATABASE_URL", "RAUTHY_URL", "PNEX_DEFAULT_ORG_TIER"]);
    unsafe { std::env::set_var("TEST_DATABASE_URL", &uri) };
    unsafe { std::env::remove_var("PNEX_DEFAULT_ORG_TIER") };
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };

    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<pnex_backend::app::App, _, _>(
        config,
        move |_server, ctx| async move {
            let org = organizations::ActiveModel {
                name: Set("No tier org".into()),
                subscription_tier_id: Set(None),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .expect("insert org");
            let fixtures = std::path::Path::new("fixtures");

            // Self-hosted: no tiers, the org keeps none.
            pnex_backend::tasks::seed::seed_catalog(&ctx.db, fixtures, false)
                .await
                .expect("self-hosted seed");
            let reloaded = organizations::Entity::find_by_id(org.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(reloaded.subscription_tier_id, None);

            // SaaS: tiers seeded, the org gets the default (Free) tier.
            pnex_backend::tasks::seed::seed_catalog(&ctx.db, fixtures, true)
                .await
                .expect("saas seed");
            let free = subscription_tiers::Entity::find()
                .filter(subscription_tiers::Column::Name.eq("Free"))
                .one(&ctx.db)
                .await
                .unwrap()
                .expect("Free tier seeded");
            let reloaded = organizations::Entity::find_by_id(org.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(reloaded.subscription_tier_id, Some(free.id));
        },
    )
    .await;
}
