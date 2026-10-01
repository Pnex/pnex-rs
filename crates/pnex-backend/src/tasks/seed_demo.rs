//! Seed **démo** par org — un environnement réaliste pour tester les listes
//! et la pagination (D14) : 105 devices, 12 flows, 12 fonctions, 12 tours,
//! 12 dashboards, 12 médias (PNG 1×1 réel en stockage), 12 mélanges,
//! 12 canaux webhook et 12 templates. Inserts directs (hors quotas tier —
//! un environnement démo n'est pas un usage de production).
//!
//! Usage : `cargo loco task seed_demo` (depuis crates/pnex-backend).
//! Idempotent : si des devices `demo-*` existent déjà dans l'org, la task
//! ne fait rien (le nettoyage se fait à la main en base si besoin).

use loco_rs::prelude::*;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use crate::models::_entities::{
    dashboard_versions, dashboards, device_registries, flow_versions, flows, fluid_mixtures,
    function_versions, functions, media_assets, media_versions, notify_channels, notify_templates,
    organization_members, predefined_devices, tour_versions, tours, users,
};
use crate::services::media::{sha256_hex, MediaSettings};

/// PNG 1×1 transparent (70 octets) — blob réaliste pour les médias de démo.
const DEMO_PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 100, 96, 248, 95, 15, 0, 2,
    135, 1, 128, 235, 71, 186, 146, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

/// Code JS d'une fonction de démo (directives complètes — le save serveur
/// n'est pas emprunté : inserts directs).
const DEMO_JS: &str = r#"// @input temperature number "Measured temperature (°C)"
// @input threshold number=20 "Alert threshold"
// @output heating number "1 when heating is requested, 0 otherwise"
function handle(inputs, msg) {
  const heat = inputs.temperature < inputs.threshold ? 1 : 0;
  return { heating: heat };
}"#;

pub struct SeedDemo;

#[async_trait]
impl Task for SeedDemo {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "seed_demo".to_string(),
            detail: "Demo seed per org (105 devices, 12 flows, 12 functions, 12 tours, 12 dashboards, 12 PNG media, 12 mixtures, 12 channels, 12 templates) to test lists and pagination.\nUsage:\ncargo loco task seed_demo".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, _vars: &task::Vars) -> Result<()> {
        let db = &ctx.db;

        // ── Org cible : celle d'admin@example.com (compte de dev) ──
        let user = users::Entity::find()
            .filter(users::Column::Email.eq("admin@example.com"))
            .one(db)
            .await?
            .ok_or_else(|| Error::string("utilisateur admin@example.com introuvable (se connecter une fois à l'app d'abord)"))?;
        let member = organization_members::Entity::find()
            .filter(organization_members::Column::UserId.eq(user.id))
            .one(db)
            .await?
            .ok_or_else(|| Error::string("aucune organisation pour admin@example.com"))?;
        let org_id = member.org_id;
        println!("  org cible : #{org_id} (admin@example.com)");

        // ── Garde idempotence : déjà seedé ? ──
        let existing = device_registries::Entity::find()
            .filter(device_registries::Column::OrgId.eq(org_id))
            .filter(device_registries::Column::DeviceId.starts_with("demo-"))
            .one(db)
            .await?;
        if existing.is_some() {
            println!("✅ Déjà seedé (devices demo-* présents) — rien à faire.");
            return Ok(());
        }

        // ── Devices : 105 (pagination réelle sur 11 pages) ──
        let predefined = predefined_devices::Entity::find()
            .filter(predefined_devices::Column::Name.eq("generic_esp8266"))
            .one(db)
            .await?
            .ok_or_else(|| Error::string("predefined device generic_esp8266 absent (lancer `cargo loco task seed` d'abord)"))?;
        for i in 1..=105 {
            device_registries::ActiveModel {
                device_id: Set(format!("demo-sensor-{i:03}")),
                org_id: Set(org_id),
                predefined_device_id: Set(predefined.id),
                active: Set(i % 7 != 0), // ~1 sur 7 inactif (variété badge)
                allow_dynamic_measurements: Set(false),
                max_unique_measurements: Set(10),
                ..Default::default()
            }
            .insert(db)
            .await?;
        }
        println!("  devices : 105");

        // ── Flows basiques : 12 (status variés pour les badges) ──
        let statuses = [
            "draft", "draft", "draft", "draft", "draft", "draft", "draft", "draft", "deployed",
            "deployed", "stopped", "error",
        ];
        for (i, status) in statuses.iter().enumerate() {
            let flow = flows::ActiveModel {
                name: Set(format!("demo-flow-{i:02}")),
                status: Set(status.to_string()),
                org_id: Set(org_id),
                ..Default::default()
            }
            .insert(db)
            .await?;
            let version = flow_versions::ActiveModel {
                flow_id: Set(flow.id),
                version_number: Set(1),
                graph: Set(serde_json::json!({"nodes": []})),
                author: Set(Some("seed_demo".to_string())),
                note: Set(Some("demo flow".to_string())),
                ..Default::default()
            }
            .insert(db)
            .await?;
            if *status == "deployed" {
                let mut f: flows::ActiveModel = flow.into();
                f.deployed_version_id = Set(Some(version.id));
                f.update(db).await?;
            }
        }
        println!("  flows : 12");

        // ── Fonctions : 12 (js, v1 avec code squelette) ──
        for i in 1..=12 {
            let function = functions::ActiveModel {
                org_id: Set(org_id),
                name: Set(format!("demo-fn-{i:02}")),
                language: Set("js".to_string()),
                description: Set(Some(format!("Demo function {i}"))),
                ..Default::default()
            }
            .insert(db)
            .await?;
            let version = function_versions::ActiveModel {
                function_id: Set(function.id),
                org_id: Set(org_id),
                version_number: Set(1),
                code: Set(DEMO_JS.to_string()),
                inputs: Set(serde_json::json!([])),
                outputs: Set(serde_json::json!([])),
                note: Set(Some("seed_demo".to_string())),
                ..Default::default()
            }
            .insert(db)
            .await?;
            let mut f: functions::ActiveModel = function.into();
            f.current_version_id = Set(Some(version.id));
            f.update(db).await?;
        }
        println!("  fonctions : 12");

        // ── Tours : 12 (doc par défaut, mode panorama) ──
        for i in 1..=12 {
            let tour = tours::ActiveModel {
                id: Set(uuid::Uuid::new_v4()),
                org_id: Set(org_id),
                name: Set(format!("demo-tour-{i:02}")),
                description: Set(Some(format!("Demo tour {i}"))),
                mode: Set("panorama".to_string()),
                ..Default::default()
            }
            .insert(db)
            .await?;
            tour_versions::ActiveModel {
                id: Set(uuid::Uuid::new_v4()),
                tour_id: Set(tour.id),
                version_number: Set(1),
                doc: Set(serde_json::json!({
                    "mode": "panorama",
                    "start_scene": null,
                    "floors": [],
                    "scenes": [],
                    "links": []
                })),
                author: Set(Some("seed_demo".to_string())),
                note: Set(Some("seed_demo".to_string())),
                ..Default::default()
            }
            .insert(db)
            .await?;
        }
        println!("  tours : 12");

        // ── Dashboards : 12 (layout vide — canvas vide au live) ──
        for i in 1..=12 {
            let dashboard = dashboards::ActiveModel {
                id: Set(uuid::Uuid::new_v4()),
                org_id: Set(org_id),
                name: Set(format!("demo-dash-{i:02}")),
                description: Set(Some(format!("Demo dashboard {i}"))),
                current_version_number: Set(1),
                ..Default::default()
            }
            .insert(db)
            .await?;
            dashboard_versions::ActiveModel {
                id: Set(uuid::Uuid::new_v4()),
                dashboard_id: Set(dashboard.id),
                version_number: Set(1),
                layout: Set(serde_json::json!({"widgets": []})),
                author: Set(Some("seed_demo".to_string())),
                ..Default::default()
            }
            .insert(db)
            .await?;
        }
        println!("  dashboards : 12");

        // ── Médias : 12 photos (PNG 1×1 réel en stockage) ──
        let settings = MediaSettings::from_config(&ctx.config);
        let store = settings.store().map_err(|e| {
            Error::string(&format!("media storage unavailable: {e} (check RustFS)"))
        })?;
        for i in 1..=12 {
            let asset = media_assets::ActiveModel {
                id: Set(uuid::Uuid::new_v4()),
                org_id: Set(org_id),
                kind: Set("photo".to_string()),
                name: Set(format!("demo-media-{i:02}.png")),
                description: Set(Some(format!("Demo media {i}"))),
                ..Default::default()
            }
            .insert(db)
            .await?;
            let key = MediaSettings::storage_key(org_id, asset.id, 1, "demo.png");
            store
                .put(&key, axum::body::Bytes::from_static(DEMO_PNG))
                .await
                .map_err(|e| Error::string(&format!("écriture blob {key} impossible : {e}")))?;
            let version = media_versions::ActiveModel {
                id: Set(uuid::Uuid::new_v4()),
                asset_id: Set(asset.id),
                org_id: Set(org_id),
                version_number: Set(1),
                filename: Set("demo.png".to_string()),
                content_type: Set("image/png".to_string()),
                size_bytes: Set(DEMO_PNG.len() as i64),
                storage_key: Set(key),
                sha256: Set(Some(sha256_hex(DEMO_PNG))),
                ..Default::default()
            }
            .insert(db)
            .await?;
            let mut a: media_assets::ActiveModel = asset.into();
            a.current_version_id = Set(Some(version.id));
            a.update(db).await?;
        }
        println!("  médias : 12 (PNG 1×1 réels)");

        // ── Mélanges : 12 (Propane/Ethane 50/50, base molaire) ──
        for i in 1..=12 {
            fluid_mixtures::ActiveModel {
                org_id: Set(org_id),
                name: Set(format!("demo-mix-{i:02}")),
                description: Set(Some(format!("Demo mixture {i}"))),
                composition: Set(serde_json::json!({
                    "basis": "mole",
                    "components": [
                        {"fluid": "Propane", "fraction": 0.5},
                        {"fluid": "Ethane", "fraction": 0.5}
                    ],
                    "mole_fractions": [],
                    "molar_mass": null
                })),
                ..Default::default()
            }
            .insert(db)
            .await?;
        }
        println!("  mélanges : 12");

        // ── Canaux webhook : 12 (enabled variés) ──
        for i in 1..=12 {
            notify_channels::ActiveModel {
                id: Set(uuid::Uuid::new_v4()),
                org_id: Set(org_id),
                kind: Set("webhook".to_string()),
                name: Set(format!("demo-hook-{i:02}")),
                config: Set(serde_json::json!({
                    "url": "https://example.invalid/demo-hook"
                })),
                enabled: Set(i % 3 != 0), // 2 sur 3 actifs (variété badge)
                ..Default::default()
            }
            .insert(db)
            .await?;
        }
        println!("  canaux webhook : 12");

        // ── Templates : 12 ──
        for i in 1..=12 {
            notify_templates::ActiveModel {
                id: Set(uuid::Uuid::new_v4()),
                org_id: Set(org_id),
                name: Set(format!("demo-tpl-{i:02}")),
                subject: Set(Some(format!("Demo alert {i} — {{{{ site }}}}",))),
                body: Set(format!(
                    "Threshold exceeded on {{{{ site }}}}: {{{{ value }}}}. (demo template {i})"
                )),
                vars: Set(serde_json::json!(["site", "value"])),
                ..Default::default()
            }
            .insert(db)
            .await?;
        }
        println!("  templates : 12");

        println!("✅ Seed démo terminé (org #{org_id}) — les listes paginent désormais.");
        Ok(())
    }
}
