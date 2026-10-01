//! Master key rotation of the vault (secrets.md S8, §7), for operators
//! without the web UI. Same singleton as the /admin/status button.
//!
//! Usage: `cargo loco task secrets_rekey` (or `pnex-server task
//! secrets_rekey`), after every pod restarted with the new write key first
//! in `PNEX_SECRETS_KEYS`.

use loco_rs::prelude::*;

use crate::services::secrets::{rekey, Keyring};

pub struct SecretsRekey;

#[async_trait]
impl Task for SecretsRekey {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "secrets_rekey".to_string(),
            detail: "Re-encrypts every vault secret with the write key (first entry of PNEX_SECRETS_KEYS).\nRun after every pod restarted with the new keyring.\nUsage:\ncargo loco task secrets_rekey".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, _vars: &task::Vars) -> Result<()> {
        let ring = Keyring::from_config(&ctx.config).map_err(|e| Error::string(&e.to_string()))?;
        let report = rekey::rekey_singleton(&ctx.db, &ring)
            .await
            .map_err(|e| Error::string(&e.to_string()))?;
        let stats = rekey::stats(&ctx.db, &ring)
            .await
            .map_err(|e| Error::string(&e.to_string()))?;
        println!(
            "write key {}: {} rewritten, {} unreadable, {} skipped, {} still under an older key",
            ring.primary_id(),
            report.rewritten,
            report.unreadable,
            report.skipped,
            stats.stale
        );
        Ok(())
    }
}
