//! Web proofs (partner-watch.md D308, D310). A relevant page new or changed
//! gets a `capture_proof` task for a browser agent of the org (one open
//! task per page, the D257 lease per target applies). The agent uploads
//! three signed parts: a full-page screenshot, the rendered HTML with
//! scripts, frames and handlers removed (served as a download only), and
//! the readable text. Once the three are in, the server seals the proof: a
//! manifest with the server's reception time, the SHA-256 of each part and
//! the SHA-256 of the org's previous manifest — an append-only chain that a
//! later change breaks (`verify`).

use loco_rs::app::AppContext;
use pnex_core::browser_agent::{
    self as ba, MissionSpec, ProofChainCheck, ProofPart, ProofPartStored, SitePage, WebProof,
};
use pnex_core::err_codes;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::services::extension::{AgentAuth, ExtensionError};

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Asks a proof of `page` (D308): one open task per page, which captures
/// the page as it is when an agent runs it.
pub async fn request(
    db: &DatabaseConnection,
    org_id: i64,
    page: &SitePage,
) -> Result<(), sea_orm::DbErr> {
    let open = db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM extension_missions WHERE org_id = $1 AND kind = 'capture_proof' \
             AND enabled AND spec -> 'params' ->> 'page_id' = $2",
            vec![org_id.into(), page.id.clone().into()],
        ))
        .await?
        .is_some();
    if open {
        return Ok(());
    }
    let Ok((url, target)) = ba::check_page_url(&page.url, "url") else {
        return Ok(());
    };
    let spec = MissionSpec::CaptureProof(ba::CaptureProofParams {
        url,
        page_id: page.id.clone(),
    });
    let label = if page.title.trim().is_empty() {
        &page.url
    } else {
        &page.title
    };
    let name: String = format!("Proof · {label}")
        .chars()
        .take(ba::NAME_MAX)
        .collect();
    // Without an agent online the task waits (D308); the text the server
    // read stays available meanwhile.
    let waiting = !crate::services::extension::any_online(db, org_id).await;
    db.execute_raw(stmt(
        "INSERT INTO extension_missions (id, org_id, name, kind, spec, target, interval_secs, \
            enabled, state_code) VALUES ($1, $2, $3, 'capture_proof', $4, $5, $6, true, $7)",
        vec![
            Uuid::new_v4().into(),
            org_id.into(),
            name.into(),
            serde_json::to_value(&spec).unwrap_or_default().into(),
            target.into(),
            ba::MAX_INTERVAL_SECS.into(),
            waiting.then(|| err_codes::EXT_NO_AGENT.to_string()).into(),
        ],
    ))
    .await?;
    Ok(())
}

#[derive(Debug, Clone, FromQueryResult)]
struct ProofRow {
    id: Uuid,
    seq: i64,
    url: String,
    site_page_id: Option<Uuid>,
    state: String,
    parts: Value,
    manifest: Option<Value>,
    manifest_sha256: Option<String>,
    prev_sha256: Option<String>,
    captured_at: Option<DateTimeWithTimeZone>,
}

const PROOF_COLUMNS: &str = "id, seq, url, site_page_id, state, parts, manifest, manifest_sha256, \
     prev_sha256, captured_at";

impl ProofRow {
    fn view(&self) -> WebProof {
        WebProof {
            id: self.id.to_string(),
            seq: self.seq,
            url: self.url.clone(),
            site_page_id: self.site_page_id.map(|p| p.to_string()),
            state: self.state.clone(),
            parts: self.parts.clone(),
            manifest: self.manifest.clone(),
            manifest_sha256: self.manifest_sha256.clone(),
            captured_at: self.captured_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// The open proof of a capture mission, created at its first part. Its
/// `seq` is taken at sealing time; until then it holds a negative number.
async fn open_proof<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    mission_id: Uuid,
    page_id: Option<Uuid>,
    url: &str,
) -> Result<ProofRow, sea_orm::DbErr> {
    if let Some(p) = ProofRow::find_by_statement(stmt(
        &format!(
            "SELECT {PROOF_COLUMNS} FROM web_proofs WHERE org_id = $1 AND mission_id = $2 \
             AND state = 'open' ORDER BY created_at DESC LIMIT 1"
        ),
        vec![org_id.into(), mission_id.into()],
    ))
    .one(db)
    .await?
    {
        return Ok(p);
    }
    let id = Uuid::new_v4();
    ProofRow::find_by_statement(stmt(
        &format!(
            "INSERT INTO web_proofs (id, org_id, seq, mission_id, site_page_id, url) \
             VALUES ($1, $2, -extract(epoch FROM clock_timestamp())::bigint * 1000 \
                     - (random() * 999)::bigint, $3, $4, $5) \
             RETURNING {PROOF_COLUMNS}"
        ),
        vec![
            id.into(),
            org_id.into(),
            mission_id.into(),
            page_id.into(),
            url.to_string().into(),
        ],
    ))
    .one(db)
    .await?
    .ok_or_else(|| sea_orm::DbErr::Custom("proof not created".into()))
}

/// Stores one part of the proof a `capture_proof` mission of this agent is
/// taking; seals the proof once its three parts are in.
pub async fn store_part(
    ctx: &AppContext,
    agent: &AgentAuth,
    mission_id: &str,
    part: ProofPart,
    bytes: axum::body::Bytes,
) -> Result<ProofPartStored, ExtensionError> {
    let (id, mission, spec) =
        crate::services::extension::own_mission(&ctx.db, agent, mission_id).await?;
    let MissionSpec::CaptureProof(params) = spec else {
        return Err(ExtensionError::MissionNotFound);
    };
    let invalid = |token: String| ExtensionError::Invalid {
        field: "body".into(),
        token,
    };
    let max = match part {
        ProofPart::Screenshot => ba::PROOF_SCREENSHOT_MAX,
        ProofPart::Html => ba::PROOF_HTML_MAX,
        ProofPart::Text => ba::PAGE_TEXT_MAX + 64 * 1024,
    };
    if bytes.is_empty() {
        return Err(invalid(err_codes::FIELD_REQUIRED.into()));
    }
    if bytes.len() > max {
        return Err(invalid(format!("{}:{max}", err_codes::FIELD_MAX_LENGTH)));
    }
    // What is stored, under which name and type: the HTML is never served
    // as a page (opaque type, attachment, sandbox: R11–R13).
    let (kind, filename, content_type, body, final_url) = match part {
        ProofPart::Screenshot => {
            if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                return Err(invalid(err_codes::FIELD_INVALID.into()));
            }
            ("photo", "proof.png", "image/png", bytes.clone(), None)
        }
        // shortcut: stored as a text document, so the document search
        // indexes it (tags included) next to the text part; give media a
        // download-only kind if that noise shows.
        ProofPart::Html => (
            "document",
            "page.html.txt",
            "application/octet-stream",
            clean_html(&bytes).into(),
            None,
        ),
        ProofPart::Text => {
            let snap: ba::PageSnapshot = serde_json::from_slice(&bytes)
                .map_err(|_| invalid(err_codes::FIELD_INVALID.into()))?;
            if snap.text.trim().is_empty() {
                return Err(invalid(err_codes::FIELD_REQUIRED.into()));
            }
            let md = crate::services::extension::page_markdown(&snap);
            let url = Some(snap.url.clone()).filter(|u| u.starts_with("http"));
            (
                "document",
                "page.md",
                "text/markdown",
                md.into_bytes().into(),
                url,
            )
        }
    };
    let page_id = Uuid::parse_str(&params.page_id).ok();
    let proof = open_proof(&ctx.db, agent.org_id, id, page_id, &params.url).await?;
    let asset = store_media(
        ctx,
        agent,
        id,
        &mission.name,
        &params.url,
        proof.id,
        part,
        kind,
        filename,
        content_type,
        body.clone(),
    )
    .await?;
    let mut parts = proof.parts.as_object().cloned().unwrap_or_default();
    parts.insert(
        part.wire().into(),
        json!({ "media_id": asset.to_string(), "sha256": sha256_hex(&body), "bytes": body.len() }),
    );
    if let Some(u) = final_url {
        parts.insert("final_url".into(), u.into());
    }
    ctx.db
        .execute_raw(stmt(
            "UPDATE web_proofs SET parts = $2 WHERE id = $1",
            vec![proof.id.into(), Value::Object(parts.clone()).into()],
        ))
        .await?;
    let complete = ProofPart::ALL.iter().all(|p| parts.contains_key(p.wire()));
    if !complete {
        return Ok(ProofPartStored {
            proof_id: proof.id.to_string(),
            sealed: false,
        });
    }
    seal(ctx, agent, id, &proof, &parts).await?;
    Ok(ProofPartStored {
        proof_id: proof.id.to_string(),
        sealed: true,
    })
}

/// The HTML part as stored: no script, frame, embed nor inline handler
/// (the agent cleans it too; the server never trusts that).
pub fn clean_html(bytes: &[u8]) -> String {
    let mut out = String::from_utf8_lossy(bytes).into_owned();
    // One pattern per element (the regex crate has no backreference); an
    // unclosed one runs to the end of the document.
    for tag in [
        "script", "iframe", "object", "frameset", "noscript", "template",
    ] {
        let re = regex::Regex::new(&format!(r"(?is)<{tag}\b.*?(</{tag}\s*>|$)")).expect("pattern");
        out = re.replace_all(&out, "").into_owned();
    }
    // Void elements: the tag alone.
    for tag in ["embed", "frame"] {
        let re = regex::Regex::new(&format!(r"(?is)<{tag}\b[^>]*>")).expect("pattern");
        out = re.replace_all(&out, "").into_owned();
    }
    let handlers =
        regex::Regex::new(r#"(?i)\s+on[a-z]+\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)"#).expect("pattern");
    let js_urls =
        regex::Regex::new(r#"(?i)(href|src|action)\s*=\s*("|')\s*javascript:[^"']*("|')"#)
            .expect("pattern");
    let out = handlers.replace_all(&out, "");
    js_urls.replace_all(&out, "$1=\"#\"").into_owned()
}

#[allow(clippy::too_many_arguments)]
async fn store_media(
    ctx: &AppContext,
    agent: &AgentAuth,
    mission_id: Uuid,
    mission_name: &str,
    url: &str,
    proof_id: Uuid,
    part: ProofPart,
    kind: &str,
    filename: &str,
    content_type: &str,
    body: axum::body::Bytes,
) -> Result<Uuid, ExtensionError> {
    use crate::models::_entities::media_assets;
    use sea_orm::{ActiveModelTrait, Set};
    let asset = media_assets::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(agent.org_id),
        kind: Set(kind.into()),
        name: Set(format!("{mission_name} · {}", part.wire())
            .chars()
            .take(255)
            .collect()),
        description: Set(None),
        // D281 provenance.
        metadata: Set(Some(json!({
            "source": "browser_agent",
            "mission_id": mission_id.to_string(),
            "source_url": url,
            "proof_id": proof_id.to_string(),
            "proof_part": part.wire(),
            "agent": agent.device_id,
        }))),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await?;
    let settings = crate::services::media::MediaSettings::from_config(&ctx.config);
    crate::controllers::media::write_version(
        ctx,
        agent.org_id,
        &asset,
        crate::controllers::media::IncomingVersion {
            filename: filename.into(),
            content_type: content_type.into(),
            note: None,
            bytes: body,
        },
        &settings,
    )
    .await
    .map_err(|e| ExtensionError::Db(sea_orm::DbErr::Custom(format!("proof part: {e}"))))?;
    Ok(asset.id)
}

/// Manifest bytes hashed in the chain: compact JSON, keys sorted at every
/// level (the stored jsonb reorders keys; the hash must not move).
fn manifest_bytes(manifest: &Value) -> Vec<u8> {
    fn canonical(v: &Value, out: &mut String) {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                out.push('{');
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&Value::String((*k).clone()).to_string());
                    out.push(':');
                    canonical(&m[*k], out);
                }
                out.push('}');
            }
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    canonical(x, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    canonical(manifest, &mut out);
    out.into_bytes()
}

/// Seals a complete proof: next number of the org's chain, manifest with
/// the server's time and the previous manifest's SHA-256.
async fn seal(
    ctx: &AppContext,
    agent: &AgentAuth,
    mission_id: Uuid,
    proof: &ProofRow,
    parts: &serde_json::Map<String, Value>,
) -> Result<(), ExtensionError> {
    crate::services::ontology::changes::deferred(async {
        let txn = ctx.db.begin().await?;
        txn.execute_raw(stmt(
            "SELECT pg_advisory_xact_lock(hashtext('web-proofs'), $1::int)",
            vec![((agent.org_id % i64::from(i32::MAX)) as i32).into()],
        ))
        .await?;
        let prev = txn
            .query_one_raw(stmt(
                "SELECT seq, manifest_sha256 FROM web_proofs WHERE org_id = $1 AND state = 'sealed' \
                 ORDER BY seq DESC LIMIT 1",
                vec![agent.org_id.into()],
            ))
            .await?;
        let (seq, prev_sha) = match prev {
            Some(r) => (
                r.try_get::<i64>("", "seq").unwrap_or(0) + 1,
                r.try_get::<Option<String>>("", "manifest_sha256")
                    .ok()
                    .flatten(),
            ),
            None => (1, None),
        };
        let captured_at = chrono::Utc::now();
        let part_of = |p: ProofPart| parts.get(p.wire()).cloned().unwrap_or(Value::Null);
        let manifest = json!({
            "version": 1,
            "org": agent.org_id,
            "proof_id": proof.id.to_string(),
            "seq": seq,
            "requested_url": proof.url,
            "final_url": parts.get("final_url").cloned().unwrap_or(Value::Null),
            "agent": agent.device_id,
            "mission_id": mission_id.to_string(),
            "captured_at": captured_at.to_rfc3339(),
            "parts": {
                "screenshot": part_of(ProofPart::Screenshot),
                "html": part_of(ProofPart::Html),
                "text": part_of(ProofPart::Text),
            },
            "prev_manifest_sha256": prev_sha,
        });
        let sha = sha256_hex(&manifest_bytes(&manifest));
        txn.execute_raw(stmt(
            "UPDATE web_proofs SET state = 'sealed', seq = $2, manifest = $3, manifest_sha256 = $4, \
               prev_sha256 = $5, captured_at = $6 WHERE id = $1",
            vec![
                proof.id.into(),
                seq.into(),
                manifest.into(),
                sha.clone().into(),
                prev_sha.into(),
                DateTimeWithTimeZone::from(captured_at).into(),
            ],
        ))
        .await?;
        // The page is captured as it was read; the task is done.
        if let Some(page) = proof.site_page_id {
            txn.execute_raw(stmt(
                "UPDATE site_pages SET state = CASE WHEN state = 'gone' THEN state ELSE 'captured' END, \
                   captured_hash = text_hash, last_proof_id = $2 WHERE id = $1",
                vec![page.into(), proof.id.into()],
            ))
            .await?;
        }
        txn.execute_raw(stmt(
            "UPDATE extension_missions SET enabled = false, state = 'idle', state_code = NULL \
             WHERE id = $1",
            vec![mission_id.into()],
        ))
        .await?;
        txn.commit().await?;
            crate::services::ontology::changes::flush();
        if let Some(page) = proof.site_page_id {
            link_parts(
                &ctx.db,
                agent.org_id,
                page,
                parts,
                &captured_at.to_rfc3339(),
                &sha,
            )
            .await;
        }
        Ok(())
    })
    .await
}

/// The crawl's `record.proof_link`: page object → each part's media.
async fn link_parts(
    db: &DatabaseConnection,
    org_id: i64,
    page: Uuid,
    parts: &serde_json::Map<String, Value>,
    captured_at: &str,
    manifest_sha: &str,
) {
    use crate::services::ontology::{links, schema, Actor};
    let row = db
        .query_one_raw(stmt(
            "SELECT p.object_id, m.spec FROM site_pages p JOIN extension_missions m ON m.id = p.mission_id \
             WHERE p.id = $1 AND p.org_id = $2",
            vec![page.into(), org_id.into()],
        ))
        .await
        .ok()
        .flatten();
    let Some(row) = row else {
        return;
    };
    let object: Option<Uuid> = row.try_get("", "object_id").ok().flatten();
    let spec: Option<Value> = row.try_get("", "spec").ok();
    let link_type = spec
        .and_then(|s| serde_json::from_value::<MissionSpec>(s).ok())
        .and_then(|s| match s {
            MissionSpec::CrawlSite(p) => p.record.and_then(|r| r.proof_link),
            _ => None,
        });
    let (Some(object), Some(link_type)) = (object, link_type) else {
        return;
    };
    let Ok(s) = schema(db, org_id).await else {
        return;
    };
    let Some(lt) = s.link_type(&link_type) else {
        return;
    };
    let declared = |k: &str| lt.attributes.iter().any(|a| a.key == k);
    let actor = Actor {
        user_id: None,
        role: crate::models::_entities::sea_orm_active_enums::OrgMemberRole::Member,
        source_ref: format!("proof:{page}"),
    };
    for (part, kind) in [
        ("screenshot", "capture"),
        ("html", "html"),
        ("text", "text"),
    ] {
        let Some(media) = parts[part]["media_id"].as_str() else {
            continue;
        };
        let Ok(target) = crate::services::ontology::objects::find_by_native(
            db,
            org_id,
            pnex_core::resources::KIND_MEDIA_ASSET,
            media,
        )
        .await
        else {
            continue;
        };
        let mut attrs = serde_json::Map::new();
        for (k, v) in [
            ("captured_at", Value::from(captured_at)),
            ("sha256_manifest", Value::from(manifest_sha)),
            ("kind", Value::from(kind)),
        ] {
            if declared(k) {
                attrs.insert(k.into(), v);
            }
        }
        let input = pnex_core::ontology::api::LinkInput {
            link_type: link_type.clone(),
            source_id: object.to_string(),
            target_id: target.id.to_string(),
            attributes: attrs,
            valid_from: None,
            valid_to: None,
        };
        if let Err(e) = links::create(db, org_id, &actor, &input).await {
            tracing::warn!(page = %page, error = %e, "proof part not linked");
        }
    }
}

pub async fn get(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<Option<WebProof>, sea_orm::DbErr> {
    Ok(ProofRow::find_by_statement(stmt(
        &format!("SELECT {PROOF_COLUMNS} FROM web_proofs WHERE org_id = $1 AND id = $2"),
        vec![org_id.into(), id.into()],
    ))
    .one(db)
    .await?
    .map(|r| r.view()))
}

/// Proofs of the org, newest first (`page` filters on a registry page).
pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    page: Option<Uuid>,
) -> Result<Vec<WebProof>, sea_orm::DbErr> {
    Ok(ProofRow::find_by_statement(stmt(
        &format!(
            "SELECT {PROOF_COLUMNS} FROM web_proofs WHERE org_id = $1 \
             AND ($2::uuid IS NULL OR site_page_id = $2) ORDER BY created_at DESC LIMIT 500"
        ),
        vec![org_id.into(), page.into()],
    ))
    .all(db)
    .await?
    .iter()
    .map(ProofRow::view)
    .collect())
}

/// Recomputes the org's chain: each manifest's SHA-256, its link to the
/// previous one, and the SHA-256 of each part against its stored version.
pub async fn verify(
    db: &DatabaseConnection,
    org_id: i64,
) -> Result<ProofChainCheck, sea_orm::DbErr> {
    let rows = ProofRow::find_by_statement(stmt(
        &format!(
            "SELECT {PROOF_COLUMNS} FROM web_proofs WHERE org_id = $1 AND state = 'sealed' \
             ORDER BY seq"
        ),
        vec![org_id.into()],
    ))
    .all(db)
    .await?;
    let mut prev: Option<String> = None;
    for r in &rows {
        let manifest = r.manifest.clone().unwrap_or(Value::Null);
        let sha = sha256_hex(&manifest_bytes(&manifest));
        let linked = manifest["prev_manifest_sha256"]
            .as_str()
            .map(str::to_string)
            == prev
            && r.prev_sha256 == prev;
        let parts_hold = parts_hold(db, org_id, &manifest).await?;
        if Some(sha.as_str()) != r.manifest_sha256.as_deref() || !linked || !parts_hold {
            return Ok(ProofChainCheck {
                ok: false,
                count: rows.len() as i64,
                broken_at: Some(r.seq),
            });
        }
        prev = Some(sha);
    }
    Ok(ProofChainCheck {
        ok: true,
        count: rows.len() as i64,
        broken_at: None,
    })
}

/// Each part's SHA-256 still matches the first version of its media.
async fn parts_hold(
    db: &DatabaseConnection,
    org_id: i64,
    manifest: &Value,
) -> Result<bool, sea_orm::DbErr> {
    for p in ProofPart::ALL {
        let part = &manifest["parts"][p.wire()];
        let (Some(media), Some(sha)) = (part["media_id"].as_str(), part["sha256"].as_str()) else {
            return Ok(false);
        };
        let Ok(media) = Uuid::parse_str(media) else {
            return Ok(false);
        };
        let stored = db
            .query_one_raw(stmt(
                "SELECT v.sha256 FROM media_versions v JOIN media_assets a ON a.id = v.asset_id \
                 WHERE a.org_id = $1 AND a.id = $2 ORDER BY v.version_number LIMIT 1",
                vec![org_id.into(), media.into()],
            ))
            .await?
            .and_then(|r| r.try_get::<Option<String>>("", "sha256").ok().flatten());
        if stored.as_deref() != Some(sha) {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_part_loses_scripts_frames_and_handlers() {
        let out = clean_html(
            br#"<html><body onload="x()"><script>alert(1)</script><a href="javascript:evil()">a</a><iframe src="https://e"></iframe><embed src="x.swf"><p onclick='y'>Prix</p><SCRIPT src=x></SCRIPT></body></html>"#,
        );
        assert!(!out.to_lowercase().contains("<script"));
        assert!(!out.contains("iframe"));
        assert!(!out.contains("onload"));
        assert!(!out.contains("onclick"));
        assert!(!out.contains("javascript:"));
        assert!(out.contains("<p>Prix</p>"));
        assert!(!out.contains("embed"));
        assert!(out.ends_with("</body></html>"));
    }

    #[test]
    fn manifest_hash_is_stable() {
        let m = json!({"b": 1, "a": {"y": 2, "x": 1}});
        let n = json!({"a": {"x": 1, "y": 2}, "b": 1});
        assert_eq!(
            sha256_hex(&manifest_bytes(&m)),
            sha256_hex(&manifest_bytes(&n))
        );
    }
}
