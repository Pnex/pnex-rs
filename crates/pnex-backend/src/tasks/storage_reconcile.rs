//! Read-only object storage ↔ database report: orphan objects (garbage
//! collection candidates) and referenced keys with no object.
//!
//! Usage: `cargo loco task storage_reconcile` (or `pnex-server task
//! storage_reconcile`); `limit:N` lists at most N keys per section
//! (default 50).

use loco_rs::prelude::*;

use crate::services::storage_reconcile;

pub struct StorageReconcile;

#[async_trait]
impl Task for StorageReconcile {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "storage_reconcile".to_string(),
            detail: "Compares the object storage bucket with the database (read-only).\nUsage:\ncargo loco task storage_reconcile [limit:50]".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, vars: &task::Vars) -> Result<()> {
        let limit = vars
            .cli_arg("limit")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(50);
        let report = storage_reconcile::reconcile(ctx)
            .await
            .map_err(|e| Error::string(&e))?;
        println!(
            "{} objects, {} referenced keys: {} orphans, {} missing",
            report.objects,
            report.referenced,
            report.orphans.len(),
            report.missing.len()
        );
        for (title, keys) in [("orphans", &report.orphans), ("missing", &report.missing)] {
            if !keys.is_empty() {
                println!("\n{title}:");
                for key in keys.iter().take(limit) {
                    println!("  {key}");
                }
                if keys.len() > limit {
                    println!("  … {} more", keys.len() - limit);
                }
            }
        }
        Ok(())
    }
}
