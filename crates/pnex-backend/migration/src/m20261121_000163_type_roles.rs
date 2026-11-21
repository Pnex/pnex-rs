//! Roles on the installed types and link types of the shared concepts
//! (external-apps.md D441): the engine finds them by role, not by key. Packs
//! declare the role since this release; the current definitions of the orgs that
//! installed an earlier version get it here, so no upgrade is needed.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// (pack, keys): the role of each is its key.
const TYPES: &[(&str, &[&str])] = &[
    (
        "investigation",
        &[
            "person",
            "organization",
            "place",
            "identifier",
            "statement",
            "hypothesis",
            "claim",
            "piece",
        ],
    ),
    ("media_coverage", &["party"]),
];

const LINKS: &[(&str, &[&str])] = &[
    (
        "investigation",
        &[
            "uses_identifier",
            "appears_in",
            "said_by",
            "published_by",
            "supports",
            "contradicts",
            "sourced_by",
            "knows",
            "employs",
            "member_of",
            "owns",
        ],
    ),
    ("companies", &["manages"]),
    ("prospecting_b2b", &["recommended"]),
];

fn list(keys: &[&str]) -> String {
    keys.iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let db = m.get_connection();
        for (pack, keys) in TYPES {
            db.execute_unprepared(&format!(
                "UPDATE object_type_versions v SET definition = jsonb_set(v.definition, '{{role}}', to_jsonb(t.key::text)) \
                 FROM object_types t \
                 WHERE v.object_type_id = t.id AND v.version = t.current_version \
                   AND t.pack_key = '{pack}' AND t.key IN ({})",
                list(keys)
            ))
            .await?;
        }
        for (pack, keys) in LINKS {
            db.execute_unprepared(&format!(
                "UPDATE link_types SET definition = jsonb_set(definition, '{{role}}', to_jsonb(key::text)) \
                 WHERE pack_key = '{pack}' AND key IN ({})",
                list(keys)
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let db = m.get_connection();
        db.execute_unprepared("UPDATE object_type_versions SET definition = definition - 'role'")
            .await?;
        db.execute_unprepared("UPDATE link_types SET definition = definition - 'role'")
            .await?;
        Ok(())
    }
}
