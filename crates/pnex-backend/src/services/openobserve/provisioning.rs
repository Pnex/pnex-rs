//! Provisioning paresseux d'une org OpenObserve par org PNEX (D2) :
//! org `pnex_org_{id}` + user d'ingestion dédié, passcode conservé en base
//! (`openobserve_orgs`) pour le chemin d'ingestion. Déclenché à la
//! première donnée d'une org — jamais au chemin HTTP utilisateur (une
//! panne O2 n'impacte pas l'API).

use loco_rs::prelude::*;
use sea_orm::{
    sea_query::OnConflict, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
};

use crate::models::_entities::{openobserve_orgs, sea_orm_active_enums::OpenobserveOrgStatus};

use super::client::Client;

/// Credentials prêts à ingérer : identifier O2 (segment URL) + Basic
/// `email:passcode`.
#[derive(Debug, Clone)]
pub struct OrgCredentials {
    pub o2_org: String,
    pub email_passcode: String,
}

/// Identité déterministe du user d'ingestion de l'org.
fn ingest_email(org_id: i64) -> String {
    format!("pnex-ingest+org{org_id}@pnex.local")
}

/// Mot de passe fort conforme à la politique O2 (8-128, minuscule,
/// majuscule, chiffre, spécial).
pub fn generate_password() -> String {
    use rand::Rng;
    const LOWER: &[u8] = b"abcdefghijkmnopqrstuvwxyz";
    const UPPER: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
    const DIGIT: &[u8] = b"23456789";
    const SPECIAL: &[u8] = b"!_-@#%+";
    const ALL: [&[u8]; 4] = [LOWER, UPPER, DIGIT, SPECIAL];
    fn pick(rng: &mut impl rand::Rng, set: &[u8]) -> char {
        set[rng.random_range(0..set.len())] as char
    }
    let mut rng = rand::rng();
    let mut chars: Vec<char> = vec![
        pick(&mut rng, LOWER),
        pick(&mut rng, UPPER),
        pick(&mut rng, DIGIT),
        pick(&mut rng, SPECIAL),
    ];
    for _ in 4..24 {
        let set = ALL[rng.random_range(0..ALL.len())];
        chars.push(pick(&mut rng, set));
    }
    // Mélange Fisher-Yates (les 4 premiers imposeraient leur position).
    for i in (1..chars.len()).rev() {
        chars.swap(i, rng.random_range(0..=i));
    }
    chars.into_iter().collect()
}

async fn row_of(db: &DatabaseConnection, org_id: i64) -> Result<Option<openobserve_orgs::Model>> {
    openobserve_orgs::Entity::find()
        .filter(openobserve_orgs::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// Upsert de l'état du provisioning (succès ou échec avec message).
async fn record(
    db: &DatabaseConnection,
    org_id: i64,
    o2_org: Option<String>,
    token: Option<String>,
    status: OpenobserveOrgStatus,
    last_error: Option<String>,
) -> Result<()> {
    let mut conflict = OnConflict::column(openobserve_orgs::Column::OrgId)
        .update_columns([
            openobserve_orgs::Column::Status,
            openobserve_orgs::Column::LastError,
        ])
        .to_owned();
    if let Some(o) = &o2_org {
        conflict.value(openobserve_orgs::Column::O2Org, o.clone());
    }
    if let Some(t) = &token {
        conflict.value(openobserve_orgs::Column::IngestionToken, t.clone());
    }
    openobserve_orgs::Entity::insert(openobserve_orgs::ActiveModel {
        org_id: Set(org_id),
        o2_org: Set(o2_org.unwrap_or_else(|| format!("pnex_org_{org_id}"))),
        ingestion_token: Set(token),
        status: Set(status),
        last_error: Set(last_error),
        ..Default::default()
    })
    .on_conflict(conflict)
    .exec(db)
    .await
    .map_err(|_| Error::InternalServerError)?;
    Ok(())
}

/// Credentials de l'org — depuis la base si déjà provisionnée, sinon
/// provisioning complet (idempotent, y compris récupération après perte de
/// la ligne PG : org retrouvée par nom, mot de passe du user réinitialisé
/// par root, passcode re-lu).
pub async fn ensure_org_credentials(
    db: &DatabaseConnection,
    client: &Client,
    org_id: i64,
) -> Result<OrgCredentials, String> {
    if let Some(row) = row_of(db, org_id).await.map_err(|e| format!("db : {e}"))? {
        if row.status == OpenobserveOrgStatus::Provisioned {
            if let Some(token) = row.ingestion_token.clone() {
                return Ok(OrgCredentials {
                    o2_org: row.o2_org.clone(),
                    email_passcode: token,
                });
            }
        }
    }

    let result = provision(db, client, org_id).await;
    match result {
        Ok(creds) => {
            record(
                db,
                org_id,
                Some(creds.o2_org.clone()),
                Some(creds.email_passcode.clone()),
                OpenobserveOrgStatus::Provisioned,
                None,
            )
            .await
            .map_err(|e| format!("db : {e}"))?;
            Ok(creds)
        }
        Err(msg) => {
            let _ = record(
                db,
                org_id,
                None,
                None,
                OpenobserveOrgStatus::Failed,
                Some(msg.clone()),
            )
            .await;
            Err(msg)
        }
    }
}

/// Credentials de l'org **en lecture seule** — `None` si l'org n'est pas
/// encore provisionnée. Doctrine du module : jamais de provisioning sur le
/// chemin HTTP utilisateur (dashboard) ; une org sans données se voit
/// simplement `telemetry.available == false`.
pub async fn provisioned_credentials(
    db: &DatabaseConnection,
    org_id: i64,
) -> Result<Option<OrgCredentials>, String> {
    let row = row_of(db, org_id).await.map_err(|e| format!("db : {e}"))?;
    Ok(row
        .filter(|r| r.status == OpenobserveOrgStatus::Provisioned)
        .and_then(|r| {
            r.ingestion_token.map(|token| OrgCredentials {
                o2_org: r.o2_org,
                email_passcode: token,
            })
        }))
}

async fn provision(
    _db: &DatabaseConnection,
    client: &Client,
    org_id: i64,
) -> Result<OrgCredentials, String> {
    let name = format!("pnex_org_{org_id}");
    // O2 ne dédoublonne pas les noms : chercher avant de créer.
    let identifier = match client.find_org_by_name(&name).await? {
        Some(existing) => existing,
        None => client.create_org(&name).await?,
    };
    let email = ingest_email(org_id);
    let password = generate_password();
    match client.create_user(&identifier, &email, &password).await {
        Ok(true) => {}
        // User préexistant (ligne PG perdue) : root reprend la main.
        Ok(false) => {
            client
                .reset_user_password(&identifier, &email, &password)
                .await?
        }
        Err(e) => return Err(e),
    }
    let passcode = client.passcode(&identifier, &email, &password).await?;
    Ok(OrgCredentials {
        o2_org: identifier,
        email_passcode: format!("{email}:{passcode}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Politique O2 : 8-128 chars, minuscule + majuscule + chiffre + spécial.
    #[test]
    fn mots_de_passe_conformes() {
        for _ in 0..50 {
            let p = generate_password();
            assert!((8..=128).contains(&p.len()), "longueur {p}");
            assert!(p.chars().any(char::is_lowercase), "minuscule {p}");
            assert!(p.chars().any(char::is_uppercase), "majuscule {p}");
            assert!(p.chars().any(|c| c.is_ascii_digit()), "chiffre {p}");
            assert!(p.chars().any(|c| "!_-@#%+".contains(c)), "spécial {p}");
        }
    }
}
