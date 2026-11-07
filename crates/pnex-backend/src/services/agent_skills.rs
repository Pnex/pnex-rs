//! Skills of hosted agents (agent-environments.md D711, autonomous-agents.md D396). A skill
//! is text of an org, versioned: every new text is a draft, and only a person holding
//! `agents:write` activates a version, after its diff. Nothing activates by itself, not even
//! a text an agent wrote: a skill is read by every run, a trapped one is a permanent
//! injection. A text shaped like a credential is refused, written or activated.
//!
//! Runs get the skills pinned by their agent version as one input, `skills`, a tar of
//! `<name>.md` read from here at the claim and at the transfer (D770): no copy in S3, no env
//! variable, nothing through the LLM gateway.

use pnex_core::agent_skills::{
    body_error, name_error, AgentSkill, AgentSkillActivate, AgentSkillCreate, AgentSkillUpdate,
    AgentSkillVersion, SkillPin, DESCRIPTION_MAX, SKILLS_INPUT,
};
use pnex_core::environments::holds_a_secret;
use pnex_core::runners::SandboxArtifact;
use pnex_core::text_diff::{self, TextDiff};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
    TransactionTrait,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug)]
pub enum SkillError {
    Invalid { field: String, token: String },
    NotFound,
    VersionNotFound,
    NameTaken,
    VersionConflict { current: i32 },
    Secret,
    Db(DbErr),
}

impl From<DbErr> for SkillError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

fn invalid(field: &str, token: &str) -> SkillError {
    SkillError::Invalid {
        field: field.into(),
        token: token.into(),
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn sha256(body: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(body.as_bytes()))
}

const ISO: &str = "'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'";

fn select() -> String {
    format!(
        "SELECT id, name, description, version, latest_number, active_number, \
           to_char(created_at AT TIME ZONE 'UTC', {ISO}) AS created_at, \
           to_char(updated_at AT TIME ZONE 'UTC', {ISO}) AS updated_at \
         FROM agent_skills"
    )
}

fn view(r: &QueryResult) -> Result<AgentSkill, DbErr> {
    Ok(AgentSkill {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        name: r.try_get("", "name")?,
        description: r.try_get("", "description")?,
        version: r.try_get("", "version")?,
        latest_number: r.try_get("", "latest_number")?,
        active_number: r.try_get("", "active_number")?,
        created_at: r.try_get("", "created_at")?,
        updated_at: r.try_get("", "updated_at")?,
    })
}

fn version_select() -> String {
    format!(
        "SELECT number, body, sha256, \
           to_char(created_at AT TIME ZONE 'UTC', {ISO}) AS created_at, \
           to_char(activated_at AT TIME ZONE 'UTC', {ISO}) AS activated_at \
         FROM agent_skill_versions"
    )
}

fn version_view(r: &QueryResult) -> Result<AgentSkillVersion, DbErr> {
    let activated_at: Option<String> = r.try_get("", "activated_at")?;
    Ok(AgentSkillVersion {
        number: r.try_get("", "number")?,
        body: r.try_get("", "body")?,
        sha256: r.try_get("", "sha256")?,
        activated: activated_at.is_some(),
        created_at: r.try_get("", "created_at")?,
        activated_at,
    })
}

fn check_body(body: &str) -> Result<(), SkillError> {
    if let Some(token) = body_error(body) {
        return Err(invalid("body", token));
    }
    if holds_a_secret(body) {
        return Err(SkillError::Secret);
    }
    Ok(())
}

fn check_description(d: &str) -> Result<(), SkillError> {
    if d.chars().count() > DESCRIPTION_MAX {
        return Err(invalid("description", "max_length:1000"));
    }
    Ok(())
}

pub async fn get<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
) -> Result<AgentSkill, SkillError> {
    let row = db
        .query_one_raw(stmt(
            &format!("{} WHERE org_id = $1 AND id = $2", select()),
            vec![org_id.into(), id.into()],
        ))
        .await?
        .ok_or(SkillError::NotFound)?;
    Ok(view(&row)?)
}

pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<AgentSkill>), DbErr> {
    let total: i64 = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM agent_skills WHERE org_id = $1",
            vec![org_id.into()],
        ))
        .await?
        .map(|r| r.try_get("", "n"))
        .transpose()?
        .unwrap_or(0);
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "{} WHERE org_id = $1 ORDER BY name LIMIT $2 OFFSET $3",
                select()
            ),
            vec![org_id.into(), limit.into(), offset.into()],
        ))
        .await?;
    Ok((total, rows.iter().map(view).collect::<Result<_, _>>()?))
}

/// A new skill whose text is its first draft.
pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    caller_id: i64,
    body: &AgentSkillCreate,
) -> Result<AgentSkill, SkillError> {
    let name = body.name.trim();
    if let Some(token) = name_error(name) {
        return Err(invalid("name", token));
    }
    check_description(&body.description)?;
    check_body(&body.body)?;
    let id = Uuid::new_v4();
    let txn = db.begin().await?;
    txn.execute_raw(stmt(
        "INSERT INTO agent_skills (id, org_id, name, description, created_by) \
         VALUES ($1, $2, $3, $4, $5)",
        vec![
            id.into(),
            org_id.into(),
            name.into(),
            body.description.trim().into(),
            caller_id.into(),
        ],
    ))
    .await
    .map_err(|e| {
        if e.to_string().contains("duplicate key") {
            SkillError::NameTaken
        } else {
            e.into()
        }
    })?;
    txn.execute_raw(stmt(
        "INSERT INTO agent_skill_versions (skill_id, number, body, sha256, created_by) \
         VALUES ($1, 1, $2, $3, $4)",
        vec![
            id.into(),
            body.body.clone().into(),
            sha256(&body.body).into(),
            caller_id.into(),
        ],
    ))
    .await?;
    txn.commit().await?;
    get(db, org_id, id).await
}

/// A new description, or a new text as the next draft, on the version that was read.
pub async fn update(
    db: &DatabaseConnection,
    org_id: i64,
    caller_id: i64,
    id: Uuid,
    body: &AgentSkillUpdate,
) -> Result<AgentSkill, SkillError> {
    let current = get(db, org_id, id).await?;
    if let Some(d) = &body.description {
        check_description(d)?;
    }
    if let Some(text) = &body.body {
        check_body(text)?;
    }
    let next = current.latest_number + i32::from(body.body.is_some());
    let txn = db.begin().await?;
    let done = txn
        .execute_raw(stmt(
            "UPDATE agent_skills SET version = version + 1, latest_number = $3, \
               description = coalesce($4, description), updated_at = now() \
             WHERE org_id = $1 AND id = $2 AND version = $5",
            vec![
                org_id.into(),
                id.into(),
                next.into(),
                body.description.as_deref().map(str::trim).into(),
                body.expected_version.into(),
            ],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(SkillError::VersionConflict {
            current: current.version,
        });
    }
    if let Some(text) = &body.body {
        txn.execute_raw(stmt(
            "INSERT INTO agent_skill_versions (skill_id, number, body, sha256, created_by) \
             VALUES ($1, $2, $3, $4, $5)",
            vec![
                id.into(),
                next.into(),
                text.clone().into(),
                sha256(text).into(),
                caller_id.into(),
            ],
        ))
        .await?;
    }
    txn.commit().await?;
    get(db, org_id, id).await
}

pub async fn delete(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<(), SkillError> {
    let done = db
        .execute_raw(stmt(
            "DELETE FROM agent_skills WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(SkillError::NotFound);
    }
    Ok(())
}

/// Texts of a skill, newest first. `(total, page)`.
pub async fn versions(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<AgentSkillVersion>), SkillError> {
    let skill = get(db, org_id, id).await?;
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "{} WHERE skill_id = $1 ORDER BY number DESC LIMIT $2 OFFSET $3",
                version_select()
            ),
            vec![id.into(), limit.into(), offset.into()],
        ))
        .await?;
    Ok((
        i64::from(skill.latest_number),
        rows.iter().map(version_view).collect::<Result<_, _>>()?,
    ))
}

pub async fn version(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    number: i32,
) -> Result<AgentSkillVersion, SkillError> {
    get(db, org_id, id).await?;
    let row = db
        .query_one_raw(stmt(
            &format!("{} WHERE skill_id = $1 AND number = $2", version_select()),
            vec![id.into(), number.into()],
        ))
        .await?
        .ok_or(SkillError::VersionNotFound)?;
    Ok(version_view(&row)?)
}

/// What changes from text `from` to text `to`, paragraph by paragraph.
pub async fn diff(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    from: i32,
    to: i32,
) -> Result<TextDiff, SkillError> {
    let old = version(db, org_id, id, from).await?;
    let new = version(db, org_id, id, to).await?;
    let mut d = text_diff::diff(&old.body, &new.body);
    d.from_version = i64::from(from);
    d.to_version = i64::from(to);
    Ok(d)
}

/// A person activates text `number` after reading it: from then on agents may pin it, and
/// it is the one the skill shows. The credential check runs again on the stored text.
pub async fn activate(
    db: &DatabaseConnection,
    org_id: i64,
    caller_id: i64,
    id: Uuid,
    body: &AgentSkillActivate,
) -> Result<AgentSkill, SkillError> {
    let current = get(db, org_id, id).await?;
    let text = version(db, org_id, id, body.number).await?;
    if holds_a_secret(&text.body) {
        return Err(SkillError::Secret);
    }
    let txn = db.begin().await?;
    let done = txn
        .execute_raw(stmt(
            "UPDATE agent_skills SET version = version + 1, active_number = $3, updated_at = now() \
             WHERE org_id = $1 AND id = $2 AND version = $4",
            vec![
                org_id.into(),
                id.into(),
                body.number.into(),
                body.expected_version.into(),
            ],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(SkillError::VersionConflict {
            current: current.version,
        });
    }
    txn.execute_raw(stmt(
        "UPDATE agent_skill_versions SET activated_at = coalesce(activated_at, now()), \
           activated_by = coalesce(activated_by, $3) WHERE skill_id = $1 AND number = $2",
        vec![id.into(), body.number.into(), caller_id.into()],
    ))
    .await?;
    txn.commit().await?;
    get(db, org_id, id).await
}

/// `(field, token)` when a pin names no skill of the org, or a text never activated.
pub async fn check_pins<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    pins: &[SkillPin],
) -> Result<Option<(&'static str, &'static str)>, DbErr> {
    for pin in pins {
        let Ok(skill) = Uuid::parse_str(&pin.skill_id) else {
            return Ok(Some(("spec.skills", "invalid")));
        };
        let row = db
            .query_one_raw(stmt(
                "SELECT v.activated_at IS NOT NULL AS activated FROM agent_skills s \
                 LEFT JOIN agent_skill_versions v ON v.skill_id = s.id AND v.number = $3 \
                 WHERE s.org_id = $1 AND s.id = $2",
                vec![org_id.into(), skill.into(), pin.version.into()],
            ))
            .await?;
        match row.map(|r| r.try_get::<Option<bool>>("", "activated")) {
            None => return Ok(Some(("spec.skills", "not_found"))),
            Some(Ok(Some(true))) => {}
            Some(Ok(_)) => return Ok(Some(("spec.skills", "not_active"))),
            Some(Err(e)) => return Err(e),
        }
    }
    Ok(None)
}

/// The skills of a run, `(name, text)`, from the version of the agent it was started from:
/// pinned texts that were activated, drafts never. `run` is the run or its job.
pub async fn texts_of<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    run: Uuid,
) -> Result<Vec<(String, String)>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT s.name, v.body FROM agent_runs a \
             JOIN agent_versions av ON av.agent_id = a.agent_id AND av.number = a.agent_version \
             CROSS JOIN LATERAL jsonb_array_elements(coalesce(av.spec->'skills', '[]'::jsonb)) p \
             JOIN agent_skills s ON s.org_id = a.org_id AND s.id::text = p->>'skill_id' \
             JOIN agent_skill_versions v ON v.skill_id = s.id \
               AND v.number = (p->>'version')::int AND v.activated_at IS NOT NULL \
             WHERE a.org_id = $1 AND $2 IN (a.id, a.job_id) ORDER BY s.name",
            vec![org_id.into(), run.into()],
        ))
        .await?;
    rows.iter()
        .map(|r| Ok((r.try_get("", "name")?, r.try_get("", "body")?)))
        .collect()
}

/// A tar of `<name>.md`, read-only, with fixed owners and times: the same skills always
/// give the same bytes, so the checksum of the claim holds at the transfer.
fn tar_of(texts: &[(String, String)]) -> std::io::Result<Vec<u8>> {
    let mut tar = tar::Builder::new(Vec::new());
    for (name, body) in texts {
        let mut header = tar::Header::new_ustar();
        header.set_path(format!("{name}.md"))?;
        header.set_size(body.len() as u64);
        header.set_mode(0o444);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_cksum();
        tar.append(&header, body.as_bytes())?;
    }
    tar.into_inner()
}

/// The `skills` input of a run, `None` when its agent pins none.
pub async fn bundle_of<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    run: Uuid,
) -> Result<Option<Vec<u8>>, DbErr> {
    let texts = texts_of(db, org_id, run).await?;
    if texts.is_empty() {
        return Ok(None);
    }
    tar_of(&texts)
        .map(Some)
        .map_err(|e| DbErr::Custom(e.to_string()))
}

/// The entry of the `skills` input the transfer containers get with the claim.
pub fn sandbox_entry(bundle: &[u8]) -> SandboxArtifact {
    SandboxArtifact {
        name: SKILLS_INPUT.into(),
        sha256: Some(format!("sha256:{:x}", Sha256::digest(bundle))),
        form: Some(pnex_core::agent_runs::ArtifactForm::Tar),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_is_the_same_bytes_every_time() {
        let texts = vec![("review".to_string(), "Read the diff.".to_string())];
        let a = tar_of(&texts).unwrap();
        assert_eq!(a, tar_of(&texts).unwrap());
        let mut archive = tar::Archive::new(a.as_slice());
        let entry = archive.entries().unwrap().next().unwrap().unwrap();
        assert_eq!(entry.path().unwrap().to_str(), Some("review.md"));
        assert_eq!(entry.header().mode().unwrap(), 0o444);
    }
}
