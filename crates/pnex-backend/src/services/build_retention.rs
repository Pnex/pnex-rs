//! Build record retention: every build request inserts a new
//! `build_records` row (new id = new, strictly increasing firmware
//! version), so the history grows with each rebuild. After a successful
//! build the worker keeps the [`KEEP_PER_DEVICE`] most recent records of
//! the device and deletes the older ones together with their versioned OTA
//! artifacts.
//!
//! Never deleted, even when older than the window:
//! - a record still queued/running (a worker owns it);
//! - the record whose version the device currently runs
//!   (`device_registries.fw_version`, as announced);
//! - the record targeted by an active (non-terminal) OTA assignment.
//!
//! The serial-flash artifact (`org_{id}/firmware/{device}-firmware.bin`) is
//! a per-device upsert shared by every record of the device: it is only
//! removed when no kept record references it.

use std::collections::HashSet;

use loco_rs::prelude::*;
use pnex_firmware_builder::ArtifactStore;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};

use crate::models::_entities::{build_records, device_registries, ota_assignments};
use crate::services::firmware::{PHASE_QUEUED, PHASE_RUNNING};
use crate::services::ota;

/// Number of build records kept per device.
pub const KEEP_PER_DEVICE: usize = 5;

/// Prune the build history of one device. Returns the number of deleted
/// records. Store failures are logged and do not abort the pruning of the
/// row (an orphan blob is harmless; a dangling row pointing at a missing
/// blob is caught by the OTA existence check).
pub async fn prune_device_builds(
    db: &DatabaseConnection,
    store: &dyn ArtifactStore,
    org_id: i64,
    device_id: &str,
) -> Result<u64> {
    let rows = build_records::Entity::find()
        .filter(build_records::Column::OrgId.eq(org_id))
        .filter(build_records::Column::DeviceId.eq(device_id))
        .order_by_desc(build_records::Column::Id)
        .all(db)
        .await?;
    if rows.len() <= KEEP_PER_DEVICE {
        return Ok(0);
    }

    // Versions that must survive: the running one + active OTA targets.
    let mut protected: HashSet<String> = HashSet::new();
    if let Some(device) = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org_id))
        .filter(device_registries::Column::DeviceId.eq(device_id))
        .one(db)
        .await?
    {
        if let Some(fw) = device.fw_version.clone() {
            protected.insert(fw);
        }
        let active = ota_assignments::Entity::find()
            .filter(ota_assignments::Column::DeviceRegistryId.eq(device.id))
            .filter(ota_assignments::Column::State.is_in([
                ota::ST_PENDING,
                ota::ST_DOWNLOADING,
                ota::ST_FLASHING,
            ]))
            .all(db)
            .await?;
        protected.extend(active.into_iter().map(|a| a.target_version));
    }

    let (kept, older) = rows.split_at(KEEP_PER_DEVICE);
    let mut kept_keys: HashSet<String> = kept
        .iter()
        .filter_map(|r| r.firmware_bin_s3_key.clone())
        .collect();
    let mut doomed = Vec::new();
    for record in older {
        let in_flight = matches!(record.build_phase.as_str(), PHASE_QUEUED | PHASE_RUNNING);
        let is_protected = record
            .fw_version
            .as_ref()
            .is_some_and(|v| protected.contains(v));
        if in_flight || is_protected {
            if let Some(key) = record.firmware_bin_s3_key.clone() {
                kept_keys.insert(key);
            }
            continue;
        }
        doomed.push(record);
    }

    let mut deleted = 0u64;
    for record in doomed {
        if let Some(version) = record.fw_version.as_deref() {
            let key = pnex_firmware_builder::ota_artifact_key(org_id, device_id, version);
            if let Err(e) = store.delete(&key).await {
                tracing::warn!(build = record.id, %key, "OTA artifact delete failed: {e}");
            }
        }
        if let Some(key) = record.firmware_bin_s3_key.as_deref() {
            if !kept_keys.contains(key) {
                if let Err(e) = store.delete(key).await {
                    tracing::warn!(build = record.id, %key, "firmware artifact delete failed: {e}");
                }
            }
        }
        deleted += build_records::Entity::delete_by_id(record.id)
            .exec(db)
            .await?
            .rows_affected;
    }
    if deleted > 0 {
        tracing::info!(org = org_id, device = %device_id, deleted, "old build records pruned");
    }
    Ok(deleted)
}
