//! proposals.md D376: the links packs kept live while waiting for a human
//! (`filiale_de.etat = a_valider`, `a_pour_site.statut = propose`) become
//! proposals and are closed; the state attributes leave every such link.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const PENDING: &str = "e.valid_to IS NULL AND \
     ((e.relation = 'filiale_de' AND e.attributes ->> 'etat' = 'a_valider') \
      OR (e.relation = 'a_pour_site' AND e.attributes ->> 'statut' = 'propose'))";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let db = m.get_connection();
        db.execute_unprepared(&format!(
            "INSERT INTO proposals (id, org_id, kind, type_key, source_id, link_target_id, vals, \
               refs, origin, slot, fingerprint) \
             SELECT gen_random_uuid(), e.org_id, 'link_create', e.relation, \
               e.source_object_id::text, e.target_object_id::text, \
               e.attributes - 'etat' - 'statut', \
               ARRAY[e.source_object_id, e.target_object_id], \
               left('migration:' || coalesce(e.source_ref, 'link'), 255), \
               'link_create|' || e.relation || '|' || e.source_object_id || '|' || e.target_object_id, \
               'migration:' || e.id \
             FROM resource_edges e \
             WHERE {PENDING} AND e.source_object_id IS NOT NULL AND e.target_object_id IS NOT NULL \
             ON CONFLICT DO NOTHING"
        ))
        .await?;
        db.execute_unprepared(&format!(
            "UPDATE resource_edges e SET valid_to = now(), updated_at = now() WHERE {PENDING}"
        ))
        .await?;
        db.execute_unprepared(
            "UPDATE resource_edges SET attributes = attributes - 'etat' \
             WHERE relation = 'filiale_de' AND attributes ? 'etat'; \
             UPDATE resource_edges SET attributes = attributes - 'statut' \
             WHERE relation = 'a_pour_site' AND attributes ? 'statut';",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, _m: &SchemaManager) -> Result<(), DbErr> {
        // Data migration: the proposals stay, nothing to undo.
        Ok(())
    }
}
