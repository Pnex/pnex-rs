//! Vision Lab (media-vision-studio.md §4.6, D198–D200): projects, images of
//! the media library with their boxes, frozen dataset versions and the COCO
//! export. Every function takes the org of the principal (R1); writes are
//! gated by the controller (R2).

use std::io::Write as _;

use axum::http::StatusCode;
use pnex_core::err_codes;
use pnex_core::lab::{
    self, DatasetSnapshot, LabAnnotationsInput, LabBox, LabDatasetVersion, LabImage, LabProject,
    LabProjectInput, LabSplit, LabStats, SnapshotImage,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    FromQueryResult, QueryFilter, QueryOrder, QuerySelect, Set, Statement, TransactionTrait,
};
use uuid::Uuid;

use crate::models::_entities::{
    lab_dataset_versions, lab_images, lab_projects, media_assets, media_versions,
};
use crate::services::media::MediaStore;

/// Coded failure, mapped to `ErrorDetail` by the controller.
#[derive(Debug)]
pub struct LabError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub args: Option<serde_json::Value>,
}

impl LabError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            args: None,
        }
    }

    pub(crate) fn args(mut self, args: serde_json::Value) -> Self {
        self.args = Some(args);
        self
    }

    pub(crate) fn db(e: sea_orm::DbErr) -> Self {
        tracing::error!(error = %e, "vision lab database error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Database error.",
        )
    }

    /// Field-level validation failures (`{field: token}`), 400.
    pub fn fields(errs: Vec<(&'static str, String)>) -> Self {
        let map: serde_json::Map<String, serde_json::Value> = errs
            .into_iter()
            .map(|(f, t)| (f.to_string(), serde_json::Value::String(t)))
            .collect();
        Self::new(
            StatusCode::BAD_REQUEST,
            err_codes::FIELD_INVALID,
            "Invalid fields.",
        )
        .args(serde_json::Value::Object(map))
    }
}

pub(crate) type Res<T> = Result<T, LabError>;

pub(crate) fn not_found(code: &'static str, what: &str) -> LabError {
    LabError::new(StatusCode::NOT_FOUND, code, format!("{what} not found."))
}

fn classes_of(p: &lab_projects::Model) -> Vec<String> {
    serde_json::from_value(p.classes.clone()).unwrap_or_default()
}

fn split_of(s: &str) -> LabSplit {
    match s {
        "train" => LabSplit::Train,
        "val" => LabSplit::Val,
        _ => LabSplit::Auto,
    }
}

fn split_wire(s: LabSplit) -> &'static str {
    match s {
        LabSplit::Auto => "auto",
        LabSplit::Train => "train",
        LabSplit::Val => "val",
    }
}

pub fn image_view(m: &lab_images::Model) -> LabImage {
    LabImage {
        id: m.id.to_string(),
        asset_id: m.media_asset_id.to_string(),
        name: m.name.clone(),
        width: m.width.max(0) as u32,
        height: m.height.max(0) as u32,
        source: m.source.clone(),
        split: split_of(&m.split),
        boxes: serde_json::from_value(m.boxes.clone()).unwrap_or_default(),
        negative: m.negative,
        luma: m.luma,
        frame_ms: m.frame_ms,
        version: m.version,
        updated_at: m.updated_at.to_rfc3339(),
    }
}

#[derive(Debug, FromQueryResult)]
struct Counts {
    project_id: Uuid,
    images: i64,
    annotated: i64,
    versions: i64,
}

/// Image / annotated / version counts of the org's projects.
async fn counts<C: ConnectionTrait>(db: &C, org_id: i64) -> Res<Vec<Counts>> {
    Counts::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        r#"SELECT p.id AS project_id,
                  (SELECT count(*) FROM lab_images i WHERE i.project_id = p.id) AS images,
                  (SELECT count(*) FROM lab_images i WHERE i.project_id = p.id
                     AND (i.negative OR jsonb_array_length(i.boxes) > 0)) AS annotated,
                  (SELECT count(*) FROM lab_dataset_versions v WHERE v.project_id = p.id) AS versions
           FROM lab_projects p WHERE p.org_id = $1"#,
        [org_id.into()],
    ))
    .all(db)
    .await
    .map_err(LabError::db)
}

fn project_view(p: &lab_projects::Model, c: Option<&Counts>) -> LabProject {
    LabProject {
        id: p.id.to_string(),
        name: p.name.clone(),
        description: p.description.clone(),
        classes: classes_of(p),
        image_count: c.map_or(0, |c| c.images.max(0) as u64),
        annotated_count: c.map_or(0, |c| c.annotated.max(0) as u64),
        version_count: c.map_or(0, |c| c.versions.max(0) as u64),
        created_at: p.created_at.to_rfc3339(),
        updated_at: p.updated_at.to_rfc3339(),
    }
}

pub async fn list_projects(db: &DatabaseConnection, org_id: i64) -> Res<Vec<LabProject>> {
    let rows = lab_projects::Entity::find()
        .filter(lab_projects::Column::OrgId.eq(org_id))
        .order_by_desc(lab_projects::Column::UpdatedAt)
        .all(db)
        .await
        .map_err(LabError::db)?;
    let counts = counts(db, org_id).await?;
    Ok(rows
        .iter()
        .map(|p| project_view(p, counts.iter().find(|c| c.project_id == p.id)))
        .collect())
}

pub(crate) async fn find_project<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
) -> Res<lab_projects::Model> {
    lab_projects::Entity::find_by_id(id)
        .filter(lab_projects::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(LabError::db)?
        .ok_or_else(|| not_found(err_codes::LAB_PROJECT_NOT_FOUND, "Project"))
}

pub async fn get_project(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Res<LabProject> {
    let p = find_project(db, org_id, id).await?;
    let counts = counts(db, org_id).await?;
    Ok(project_view(
        &p,
        counts.iter().find(|c| c.project_id == p.id),
    ))
}

fn trimmed(input: &LabProjectInput) -> LabProjectInput {
    LabProjectInput {
        name: input.name.trim().to_string(),
        description: input.description.trim().to_string(),
        classes: input.classes.iter().map(|c| c.trim().to_string()).collect(),
    }
}

pub async fn create_project(
    db: &DatabaseConnection,
    org_id: i64,
    user: &str,
    input: &LabProjectInput,
) -> Res<LabProject> {
    let input = trimmed(input);
    input.check().map_err(LabError::fields)?;
    let now = chrono::Utc::now().fixed_offset();
    let row = lab_projects::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        name: Set(input.name),
        description: Set(input.description),
        classes: Set(serde_json::json!(input.classes)),
        created_by: Set(user.to_string()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .map_err(LabError::db)?;
    Ok(project_view(&row, None))
}

/// Classes are positional (a box stores an index): they can be renamed in
/// place or appended; only trailing classes can be removed, and only when
/// no box uses them (409 `lab-class-in-use`).
pub async fn update_project(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    input: &LabProjectInput,
) -> Res<LabProject> {
    let input = trimmed(input);
    input.check().map_err(LabError::fields)?;
    let p = find_project(db, org_id, id).await?;
    let old = classes_of(&p);
    if input.classes.len() < old.len() {
        let images = lab_images::Entity::find()
            .filter(lab_images::Column::ProjectId.eq(id))
            .all(db)
            .await
            .map_err(LabError::db)?;
        let used = images
            .iter()
            .flat_map(|m| {
                serde_json::from_value::<Vec<LabBox>>(m.boxes.clone()).unwrap_or_default()
            })
            .map(|b| b.class as usize)
            .filter(|c| *c >= input.classes.len())
            .min();
        if let Some(c) = used {
            let name = old.get(c).cloned().unwrap_or_default();
            return Err(LabError::new(
                StatusCode::CONFLICT,
                err_codes::LAB_CLASS_IN_USE,
                format!("The class {name} still has boxes."),
            )
            .args(serde_json::json!({ "class": name })));
        }
    }
    let mut row: lab_projects::ActiveModel = p.into();
    row.name = Set(input.name);
    row.description = Set(input.description);
    row.classes = Set(serde_json::json!(input.classes));
    row.updated_at = Set(chrono::Utc::now().fixed_offset());
    let row = row.update(db).await.map_err(LabError::db)?;
    get_project(db, org_id, row.id).await
}

pub async fn delete_project(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Res<()> {
    let p = find_project(db, org_id, id).await?;
    lab_projects::Entity::delete_by_id(p.id)
        .exec(db)
        .await
        .map_err(LabError::db)?;
    Ok(())
}

pub async fn list_images(
    db: &DatabaseConnection,
    org_id: i64,
    project: Uuid,
) -> Res<Vec<LabImage>> {
    find_project(db, org_id, project).await?;
    Ok(lab_images::Entity::find()
        .filter(lab_images::Column::ProjectId.eq(project))
        .order_by_asc(lab_images::Column::CreatedAt)
        .order_by_asc(lab_images::Column::Name)
        .all(db)
        .await
        .map_err(LabError::db)?
        .iter()
        .map(image_view)
        .collect())
}

/// Current version of an asset of the org.
async fn current_version(
    db: &DatabaseConnection,
    org_id: i64,
    asset_id: Uuid,
) -> Res<Option<(media_assets::Model, media_versions::Model)>> {
    let Some(asset) = media_assets::Entity::find_by_id(asset_id)
        .filter(media_assets::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(LabError::db)?
    else {
        return Ok(None);
    };
    let Some(vid) = asset.current_version_id else {
        return Ok(None);
    };
    let version = media_versions::Entity::find_by_id(vid)
        .one(db)
        .await
        .map_err(LabError::db)?;
    Ok(version.map(|v| (asset, v)))
}

/// Adds media library pictures to a project. One call = one source (the
/// train / validation split unit). Assets already in the project are
/// skipped; an asset that is not the org's or not a JPEG / PNG answers 400
/// `lab-asset-not-image` and nothing is added.
pub async fn add_images(
    db: &DatabaseConnection,
    store: &dyn MediaStore,
    org_id: i64,
    project: Uuid,
    asset_ids: &[Uuid],
) -> Res<Vec<LabImage>> {
    find_project(db, org_id, project).await?;
    if asset_ids.is_empty() || asset_ids.len() > 500 {
        return Err(LabError::fields(vec![("asset_ids", "range:1..500".into())]));
    }
    let existing: Vec<Uuid> = lab_images::Entity::find()
        .filter(lab_images::Column::ProjectId.eq(project))
        .all(db)
        .await
        .map_err(LabError::db)?
        .into_iter()
        .map(|m| m.media_asset_id)
        .collect();
    let mut rows = Vec::new();
    let source = format!("import-{}", Uuid::new_v4());
    for id in asset_ids {
        if existing.contains(id)
            || rows
                .iter()
                .any(|r: &lab_images::ActiveModel| r.media_asset_id.as_ref() == id)
        {
            continue;
        }
        let not_image = |name: &str| {
            LabError::new(
                StatusCode::BAD_REQUEST,
                err_codes::LAB_ASSET_NOT_IMAGE,
                format!("{name} is not a readable picture."),
            )
            .args(serde_json::json!({ "name": name }))
        };
        let Some((asset, version)) = current_version(db, org_id, *id).await? else {
            return Err(not_image(&id.to_string()));
        };
        // Only pictures of a sane size are read at all.
        if !version.content_type.starts_with("image/") || version.size_bytes > IMAGE_MAX_BYTES {
            return Err(not_image(&asset.name));
        }
        let bytes = store
            .get(&version.storage_key)
            .await
            .map_err(|_| not_image(&asset.name))?;
        let Some((w, h)) = pnex_vision::image_size(&bytes) else {
            return Err(not_image(&asset.name));
        };
        let luma = luma_of(bytes).await;
        let now = chrono::Utc::now().fixed_offset();
        rows.push(lab_images::ActiveModel {
            id: Set(Uuid::new_v4()),
            org_id: Set(org_id),
            project_id: Set(project),
            media_asset_id: Set(*id),
            name: Set(asset.name.clone()),
            width: Set(w as i32),
            height: Set(h as i32),
            source: Set(source.clone()),
            split: Set("auto".into()),
            boxes: Set(serde_json::json!([])),
            negative: Set(false),
            luma: Set(luma),
            frame_ms: Set(None),
            version: Set(1),
            created_at: Set(now),
            updated_at: Set(now),
        });
    }
    let txn = db.begin().await.map_err(LabError::db)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(image_view(&row.insert(&txn).await.map_err(LabError::db)?));
    }
    touch_project(&txn, project).await?;
    txn.commit().await.map_err(LabError::db)?;
    Ok(out)
}

async fn touch_project<C: ConnectionTrait>(db: &C, project: Uuid) -> Res<()> {
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        "UPDATE lab_projects SET updated_at = now() WHERE id = $1",
        [project.into()],
    ))
    .await
    .map_err(LabError::db)?;
    Ok(())
}

async fn find_image(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Res<lab_images::Model> {
    lab_images::Entity::find_by_id(id)
        .filter(lab_images::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(LabError::db)?
        .ok_or_else(|| not_found(err_codes::LAB_IMAGE_NOT_FOUND, "Image"))
}

pub async fn remove_image(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Res<()> {
    let img = find_image(db, org_id, id).await?;
    lab_images::Entity::delete_by_id(img.id)
        .exec(db)
        .await
        .map_err(LabError::db)?;
    Ok(())
}

/// Saves the boxes of an image on the version the editor read: a stale
/// `expected_version` answers 409 `lab-annotations-conflict` (atomic
/// conditional update, no lost write).
pub async fn save_annotations(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    input: &LabAnnotationsInput,
) -> Res<LabImage> {
    let img = find_image(db, org_id, id).await?;
    let project = find_project(db, org_id, img.project_id).await?;
    if let Some(code) = input.check(classes_of(&project).len()) {
        let code = match code {
            "lab-boxes-too-many" => err_codes::LAB_BOXES_TOO_MANY,
            "lab-negative-with-boxes" => err_codes::LAB_NEGATIVE_WITH_BOXES,
            _ => err_codes::LAB_BOX_INVALID,
        };
        return Err(LabError::new(
            StatusCode::BAD_REQUEST,
            code,
            "Invalid boxes.",
        ));
    }
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            db.get_database_backend(),
            "UPDATE lab_images SET boxes = $1, negative = $2, split = $3, version = version + 1, \
             updated_at = now() WHERE id = $4 AND org_id = $5 AND version = $6",
            [
                serde_json::json!(input.boxes).into(),
                input.negative.into(),
                split_wire(input.split).into(),
                id.into(),
                org_id.into(),
                input.expected_version.into(),
            ],
        ))
        .await
        .map_err(LabError::db)?;
    if res.rows_affected() == 0 {
        return Err(LabError::new(
            StatusCode::CONFLICT,
            err_codes::LAB_ANNOTATIONS_CONFLICT,
            "Stale version: reload the image before saving.",
        )
        .args(serde_json::json!({
            "expected": input.expected_version.to_string(),
            "current": img.version.to_string(),
        })));
    }
    touch_project(db, project.id).await?;
    Ok(image_view(&find_image(db, org_id, id).await?))
}

pub async fn stats(db: &DatabaseConnection, org_id: i64, project: Uuid) -> Res<LabStats> {
    let p = find_project(db, org_id, project).await?;
    let images = list_images(db, org_id, project).await?;
    Ok(lab::stats(&classes_of(&p), &images))
}

fn version_view(v: &lab_dataset_versions::Model) -> LabDatasetVersion {
    LabDatasetVersion {
        id: v.id.to_string(),
        version: v.version,
        note: v.note.clone(),
        stats: serde_json::from_value(v.stats.clone()).unwrap_or_default(),
        created_at: v.created_at.to_rfc3339(),
        created_by: v.created_by.clone(),
    }
}

pub async fn list_versions(
    db: &DatabaseConnection,
    org_id: i64,
    project: Uuid,
) -> Res<Vec<LabDatasetVersion>> {
    find_project(db, org_id, project).await?;
    Ok(lab_dataset_versions::Entity::find()
        .filter(lab_dataset_versions::Column::ProjectId.eq(project))
        .order_by_desc(lab_dataset_versions::Column::Version)
        .all(db)
        .await
        .map_err(LabError::db)?
        .iter()
        .map(version_view)
        .collect())
}

/// Freezes the annotated images (boxes + the media version each picture
/// is at) into the next dataset version — what exports and trainings
/// read (D199). Nothing annotated = 400 `lab-dataset-empty`.
pub async fn freeze(
    db: &DatabaseConnection,
    org_id: i64,
    project: Uuid,
    user: &str,
    note: &str,
) -> Res<LabDatasetVersion> {
    let p = find_project(db, org_id, project).await?;
    let classes = classes_of(&p);
    let images = list_images(db, org_id, project).await?;
    let mut snap = DatasetSnapshot {
        classes: classes.clone(),
        images: Vec::new(),
    };
    for img in images.iter().filter(|i| i.annotated()) {
        let asset_id = Uuid::parse_str(&img.asset_id)
            .map_err(|_| LabError::db(sea_orm::DbErr::Custom("asset id".into())))?;
        // An asset deleted from the library leaves the dataset with it.
        let Some((_, version)) = current_version(db, org_id, asset_id).await? else {
            continue;
        };
        snap.images.push(SnapshotImage {
            image_id: img.id.clone(),
            asset_id: img.asset_id.clone(),
            asset_version: version.version_number,
            name: img.name.clone(),
            width: img.width,
            height: img.height,
            split: img.effective_split(),
            boxes: img.boxes.clone(),
        });
    }
    if snap.images.is_empty() {
        return Err(LabError::new(
            StatusCode::BAD_REQUEST,
            err_codes::LAB_DATASET_EMPTY,
            "No annotated image to freeze.",
        ));
    }
    let annotated: Vec<LabImage> = images.into_iter().filter(|i| i.annotated()).collect();
    let stats = lab::stats(&classes, &annotated);
    let txn = db.begin().await.map_err(LabError::db)?;
    // Parent row lock: two concurrent freezes get consecutive numbers.
    txn.execute_raw(Statement::from_sql_and_values(
        txn.get_database_backend(),
        "SELECT id FROM lab_projects WHERE id = $1 FOR UPDATE",
        [project.into()],
    ))
    .await
    .map_err(LabError::db)?;
    let last = lab_dataset_versions::Entity::find()
        .filter(lab_dataset_versions::Column::ProjectId.eq(project))
        .order_by_desc(lab_dataset_versions::Column::Version)
        .one(&txn)
        .await
        .map_err(LabError::db)?
        .map_or(0, |v| v.version);
    let row = lab_dataset_versions::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        project_id: Set(project),
        version: Set(last + 1),
        note: Set(pnex_core::vision_ai::clip(note.trim(), 500)),
        snapshot: Set(serde_json::to_value(&snap).unwrap_or_default()),
        stats: Set(serde_json::to_value(&stats).unwrap_or_default()),
        created_by: Set(user.to_string()),
        created_at: Set(chrono::Utc::now().fixed_offset()),
    }
    .insert(&txn)
    .await
    .map_err(LabError::db)?;
    txn.commit().await.map_err(LabError::db)?;
    Ok(version_view(&row))
}

/// Frames of a camera recording (D79) added as pictures (D202): the frame
/// nearest to every `every_secs` instant is stored as a `photo` of the
/// media library (the JPEG of the MJPEG-AVI segment, no re-encoding), then
/// added to the project — one import = one source, so a whole video lands
/// on the same side of the train / validation split.
pub async fn import_recording(
    ctx: &loco_rs::app::AppContext,
    org_id: i64,
    project: Uuid,
    req: &lab::LabRecordingImport,
) -> Res<Vec<LabImage>> {
    use crate::models::_entities::{device_registries, video_segments};
    use crate::services::media::MediaSettings;
    let db = &ctx.db;
    find_project(db, org_id, project).await?;
    let parse = |raw: &str| chrono::DateTime::parse_from_rfc3339(raw.trim()).ok();
    let (Some(from), Some(to)) = (parse(&req.from), parse(&req.to)) else {
        return Err(LabError::fields(vec![(
            "from",
            err_codes::FIELD_INVALID.to_string(),
        )]));
    };
    let (from_ms, to_ms) = (from.timestamp_millis(), to.timestamp_millis());
    if let Some((field, token)) = req.check(to_ms - from_ms) {
        return Err(LabError::fields(vec![(field, token)]));
    }
    let device = device_registries::Entity::find_by_id(req.device)
        .filter(device_registries::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(LabError::db)?
        .ok_or_else(|| not_found(err_codes::CAMERA_UNKNOWN, "Camera"))?;
    let segments = video_segments::Entity::find()
        .filter(video_segments::Column::OrgId.eq(org_id))
        .filter(video_segments::Column::DeviceRegistryId.eq(device.id))
        .filter(video_segments::Column::StartedAt.lt(to))
        .filter(video_segments::Column::EndedAt.gt(from))
        .order_by_asc(video_segments::Column::StartedAt)
        .all(db)
        .await
        .map_err(LabError::db)?;
    let settings = MediaSettings::from_config(&ctx.config);
    let store = settings
        .store()
        .map_err(|e| LabError::db(sea_orm::DbErr::Custom(e)))?;
    // Frames picked from the segment metadata: only the segments holding a
    // picked frame are read, one at a time.
    let spans: Vec<(i64, i64, usize)> = segments
        .iter()
        .map(|seg| {
            (
                seg.started_at.timestamp_millis(),
                seg.ended_at.timestamp_millis(),
                seg.frame_count.max(0) as usize,
            )
        })
        .collect();
    let picks = lab::pick_frames(&spans, from_ms, to_ms, (req.every_secs * 1000.0) as i64);
    let mut needed: Vec<usize> = picks.iter().map(|(si, _, _)| *si).collect();
    needed.dedup();
    let read_bytes: i64 = needed.iter().map(|si| segments[*si].size_bytes).sum();
    if read_bytes > RECORDING_MAX_BYTES {
        return Err(LabError::fields(vec![("to", "too_large".into())]));
    }
    if picks.is_empty() {
        return Err(LabError::new(
            StatusCode::NOT_FOUND,
            err_codes::CAMERA_EXPORT_EMPTY,
            "No recording in this range.",
        ));
    }
    let source = format!("rec-{}-{}", device.device_id, from.to_rfc3339());
    let mut out = Vec::with_capacity(picks.len());
    let mut current: Option<(usize, Vec<u8>, pnex_core::avi::AviIndex)> = None;
    for (si, fi, t_ms) in picks {
        if current.as_ref().is_none_or(|(c, _, _)| *c != si) {
            // The previous segment is dropped here: one in memory at once.
            current = None;
            let Ok(bytes) = store.get(&segments[si].storage_key).await else {
                continue;
            };
            let Ok(index) = pnex_core::avi::read_avi(&bytes) else {
                continue;
            };
            current = Some((si, bytes, index));
        }
        let Some((_, bytes, index)) = current.as_ref() else {
            continue;
        };
        // The stored frame count may differ from the file: clamp.
        let Some(&(start, len)) = index
            .frames
            .get(fi.min(index.frames.len().saturating_sub(1)))
        else {
            continue;
        };
        let Some(jpeg) = bytes.get(start..start + len) else {
            continue;
        };
        let Some((w, h)) = pnex_vision::image_size(jpeg) else {
            continue;
        };
        let at = chrono::DateTime::from_timestamp_millis(t_ms).unwrap_or_default();
        let name = format!(
            "{} {}",
            device.device_id,
            at.format("%Y-%m-%d %H:%M:%S UTC")
        );
        let frame = Frame {
            name,
            filename: format!("{}-{}.jpg", device.device_id, t_ms),
            description: "Frame of a camera recording (Vision Lab import).",
            jpeg,
            size: (w, h),
            frame_ms: t_ms,
        };
        if let Some(img) = store_frame(ctx, &settings, org_id, project, &source, frame).await? {
            out.push(img);
        }
    }
    touch_project(db, project).await?;
    Ok(out)
}

/// Mean luma of a picture, decoded off the async workers.
async fn luma_of(bytes: Vec<u8>) -> Option<f32> {
    tokio::task::spawn_blocking(move || pnex_vision::mean_luma(&bytes))
        .await
        .ok()
        .flatten()
}

/// One extracted frame to store as a picture of a project.
struct Frame<'a> {
    name: String,
    filename: String,
    description: &'static str,
    jpeg: &'a [u8],
    size: (u32, u32),
    frame_ms: i64,
}

/// Stores a frame as a `photo` of the media library and adds it to the
/// project under `source`. `None` = the bytes could not be stored (logged,
/// the import goes on).
async fn store_frame(
    ctx: &loco_rs::app::AppContext,
    settings: &crate::services::media::MediaSettings,
    org_id: i64,
    project: Uuid,
    source: &str,
    frame: Frame<'_>,
) -> Res<Option<LabImage>> {
    use crate::controllers::media::{write_version, IncomingVersion};
    let db = &ctx.db;
    let asset = media_assets::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        kind: Set("photo".into()),
        name: Set(frame.name.clone()),
        description: Set(Some(frame.description.into())),
        metadata: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .map_err(LabError::db)?;
    if let Err(e) = write_version(
        ctx,
        org_id,
        &asset,
        IncomingVersion {
            filename: frame.filename,
            content_type: "image/jpeg".into(),
            note: None,
            bytes: axum::body::Bytes::copy_from_slice(frame.jpeg),
        },
        settings,
    )
    .await
    {
        tracing::warn!(error = %e, "lab frame import: frame not stored");
        let _ = media_assets::Entity::delete_by_id(asset.id).exec(db).await;
        return Ok(None);
    }
    let luma = luma_of(frame.jpeg.to_vec()).await;
    let now = chrono::Utc::now().fixed_offset();
    let row = lab_images::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        project_id: Set(project),
        media_asset_id: Set(asset.id),
        name: Set(frame.name),
        width: Set(frame.size.0 as i32),
        height: Set(frame.size.1 as i32),
        source: Set(source.to_string()),
        split: Set("auto".into()),
        boxes: Set(serde_json::json!([])),
        negative: Set(false),
        luma: Set(luma),
        frame_ms: Set(Some(frame.frame_ms)),
        version: Set(1),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .map_err(LabError::db)?;
    Ok(Some(image_view(&row)))
}

/// Largest video file the Lab decodes (read in memory, D160 decoder).
pub const VIDEO_MAX_BYTES: i64 = 512 * 1024 * 1024;
/// One video import at a time per process: a 512 MB file peaks near 1 GB
/// (bytes + the fast-start copy), never several at once.
static VIDEO_IMPORTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Frames of a video of the media library (MP4 / MOV) added as pictures.
/// Every import of the same video shares one source (`video-<asset>`), so
/// its frames never straddle train and validation.
pub async fn import_video(
    ctx: &loco_rs::app::AppContext,
    org_id: i64,
    project: Uuid,
    req: &lab::LabVideoImport,
) -> Res<Vec<LabImage>> {
    use crate::services::media::MediaSettings;
    let db = &ctx.db;
    find_project(db, org_id, project).await?;
    if let Some((field, token)) = req.check() {
        return Err(LabError::fields(vec![(field, token)]));
    }
    let asset_field = || LabError::fields(vec![("asset_id", err_codes::FIELD_INVALID.to_string())]);
    let asset_id = Uuid::parse_str(&req.asset_id).map_err(|_| asset_field())?;
    let Some((asset, version)) = current_version(db, org_id, asset_id).await? else {
        return Err(asset_field());
    };
    let unsupported = || {
        LabError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            err_codes::LAB_VIDEO_UNSUPPORTED,
            format!("{} is not a readable video.", asset.name),
        )
        .args(serde_json::json!({ "name": asset.name }))
    };
    let Some(format) = pnex_media_capture::decoder::video_file_format(&version.content_type) else {
        return Err(unsupported());
    };
    if version.size_bytes > VIDEO_MAX_BYTES {
        return Err(LabError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            err_codes::LAB_VIDEO_TOO_LARGE,
            "This video is too large for the Vision Lab (512 MB at most).",
        ));
    }
    let _permit = VIDEO_IMPORTS
        .acquire()
        .await
        .map_err(|_| LabError::db(sea_orm::DbErr::Custom("video import gate closed".into())))?;
    let settings = MediaSettings::from_config(&ctx.config);
    let store = settings
        .store()
        .map_err(|e| LabError::db(sea_orm::DbErr::Custom(e)))?;
    let bytes = store
        .get(&version.storage_key)
        .await
        .map_err(|_| asset_field())?;
    let frames = pnex_media_capture::decode_video_frames(
        bytes,
        format,
        req.from_secs,
        req.every_secs,
        lab::LAB_RECORDING_MAX_FRAMES,
    )
    .await
    .map_err(|e| match e {
        pnex_media_capture::CaptureError::FormatUnsupported => unsupported(),
        other => LabError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            err_codes::LAB_VIDEO_DECODE_FAILED,
            format!("The video could not be decoded ({}).", other.code()),
        )
        .args(serde_json::json!({ "detail": other.code() })),
    })?;
    let from_ms = (req.from_secs * 1000.0) as i64;
    let (every_ms, to_ms) = (
        (req.every_secs * 1000.0) as i64,
        (req.to_secs * 1000.0) as i64,
    );
    let source = format!("video-{}", asset.id);
    let mut out = Vec::new();
    for (k, jpeg) in frames.iter().enumerate() {
        let t_ms = from_ms + k as i64 * every_ms;
        if t_ms >= to_ms {
            break;
        }
        let secs = t_ms / 1000;
        let frame = Frame {
            name: format!("{} {:02}:{:02}", asset.name, secs / 60, secs % 60),
            filename: format!("{}-{}.jpg", asset.id, t_ms),
            description: "Frame of a video (Vision Lab import).",
            jpeg: &jpeg.bytes,
            size: (u32::from(jpeg.width), u32::from(jpeg.height)),
            frame_ms: t_ms,
        };
        if let Some(img) = store_frame(ctx, &settings, org_id, project, &source, frame).await? {
            out.push(img);
        }
    }
    touch_project(db, project).await?;
    Ok(out)
}

/// Nearest frame of `img`'s source holding boxes, before or after `t_ms`.
async fn annotated_neighbour(
    db: &DatabaseConnection,
    img: &lab_images::Model,
    t_ms: i64,
    before: bool,
) -> Res<Option<lab_images::Model>> {
    use sea_orm::sea_query::Expr;
    let q = lab_images::Entity::find()
        .filter(lab_images::Column::ProjectId.eq(img.project_id))
        .filter(lab_images::Column::Source.eq(img.source.clone()))
        .filter(Expr::cust("jsonb_array_length(boxes) > 0"));
    let q = if before {
        q.filter(lab_images::Column::FrameMs.lt(t_ms))
            .order_by_desc(lab_images::Column::FrameMs)
    } else {
        q.filter(lab_images::Column::FrameMs.gt(t_ms))
            .order_by_asc(lab_images::Column::FrameMs)
    };
    q.limit(1).one(db).await.map_err(LabError::db)
}

/// Propagation by tracking (M4): a registry model proposes boxes on this
/// frame, and each box of the previous annotated frame of the source gives
/// its class to the proposal overlapping it most. Proposals only.
pub async fn propagate(
    ctx: &loco_rs::app::AppContext,
    store: &dyn MediaStore,
    org_id: i64,
    image_id: Uuid,
    req: &lab::LabSuggestRequest,
) -> Res<Vec<lab::LabSuggestion>> {
    let img = find_image(&ctx.db, org_id, image_id).await?;
    let no_previous = || {
        LabError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            err_codes::LAB_PROPAGATE_NO_PREVIOUS,
            "Annotate a frame before this one in the same video.",
        )
    };
    let t_ms = img.frame_ms.ok_or_else(no_previous)?;
    let prev = annotated_neighbour(&ctx.db, &img, t_ms, true)
        .await?
        .ok_or_else(no_previous)?;
    let boxes: Vec<LabBox> = serde_json::from_value(prev.boxes).unwrap_or_default();
    let proposals = suggest(ctx, store, org_id, image_id, req).await?;
    Ok(lab::propagate(&boxes, proposals))
}

/// Boxes proposed for a frame by linear interpolation between the nearest
/// annotated frames of its source, before and after (Q3). Proposals only,
/// nothing is written; `label` is empty (not a model label).
pub async fn interpolate(
    db: &DatabaseConnection,
    org_id: i64,
    image_id: Uuid,
) -> Res<Vec<lab::LabSuggestion>> {
    let img = find_image(db, org_id, image_id).await?;
    let project = find_project(db, org_id, img.project_id).await?;
    let no_keyframes = || {
        LabError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            err_codes::LAB_INTERPOLATE_NO_KEYFRAMES,
            "Annotate a frame before and a frame after this one in the same video.",
        )
    };
    let Some(t_ms) = img.frame_ms else {
        return Err(no_keyframes());
    };
    let (Some(a), Some(b)) = (
        annotated_neighbour(db, &img, t_ms, true).await?,
        annotated_neighbour(db, &img, t_ms, false).await?,
    ) else {
        return Err(no_keyframes());
    };
    let boxes = |m: &lab_images::Model| -> Vec<LabBox> {
        serde_json::from_value(m.boxes.clone()).unwrap_or_default()
    };
    let (ba, bb) = (boxes(&a), boxes(&b));
    let classes = classes_of(&project);
    Ok(lab::interpolate(
        (a.frame_ms.unwrap_or_default(), &ba),
        (b.frame_ms.unwrap_or_default(), &bb),
        t_ms,
    )
    .into_iter()
    .filter(|x| (x.class as usize) < classes.len())
    .map(|x| lab::LabSuggestion {
        label: String::new(),
        score: 1.0,
        class: Some(x.class),
        bbox: x.bbox,
    })
    .collect())
}

/// Pre-annotation (§4.6.1 step 4): runs a detection model of the org's
/// registry on the image and returns proposals (nothing is written). A
/// model label equal to a class name maps to that class.
pub async fn suggest(
    ctx: &loco_rs::app::AppContext,
    store: &dyn MediaStore,
    org_id: i64,
    image_id: Uuid,
    req: &lab::LabSuggestRequest,
) -> Res<Vec<lab::LabSuggestion>> {
    use crate::models::_entities::ml_models;
    use crate::services::vision;
    let db = &ctx.db;
    let img = find_image(db, org_id, image_id).await?;
    let project = find_project(db, org_id, img.project_id).await?;
    let classes = classes_of(&project);
    let model_field = || LabError::fields(vec![("model_id", err_codes::FIELD_INVALID.to_string())]);
    let model_id = Uuid::parse_str(&req.model_id).map_err(|_| model_field())?;
    if !(0.0..=1.0).contains(&req.min_score) {
        return Err(LabError::fields(vec![("min_score", "range:0..1".into())]));
    }
    let m = ml_models::Entity::find_by_id(model_id)
        .filter(ml_models::Column::OrgId.eq(org_id))
        .filter(ml_models::Column::Task.eq(vision::VISION_TASK))
        .one(db)
        .await
        .map_err(LabError::db)?
        .ok_or_else(model_field)?;
    let det = match vision::detector(ctx, &m).await {
        Ok(Some(d)) => d,
        Ok(None) => {
            return Err(LabError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                err_codes::ML_MODEL_MISSING_BYTES,
                "The model file is missing.",
            ))
        }
        Err(e) => {
            return Err(LabError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                err_codes::ML_MODEL_LOAD_FAILED,
                format!("The model could not be loaded: {e}"),
            )
            .args(serde_json::json!({ "detail": e })))
        }
    };
    let threshold = if req.min_score > 0.0 {
        req.min_score
    } else {
        det.spec().score_threshold
    };
    let Some((_, version)) = current_version(db, org_id, img.media_asset_id).await? else {
        return Err(not_found(err_codes::LAB_IMAGE_NOT_FOUND, "Image"));
    };
    let bytes = store
        .get(&version.storage_key)
        .await
        .map_err(|_| not_found(err_codes::LAB_IMAGE_NOT_FOUND, "Image"))?;
    let res = vision::detect(det, bytes).await.map_err(|e| match e {
        vision::VisionError::Busy(_) => LabError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            err_codes::ML_MODEL_INFERENCE_FAILED,
            "The vision workers are busy: try again in a moment.",
        ),
        vision::VisionError::Failed(e) => LabError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            err_codes::ML_MODEL_INFERENCE_FAILED,
            format!("Inference failed: {e}"),
        )
        .args(serde_json::json!({ "detail": e })),
    })?;
    let (w, h) = (res.width.max(1) as f32, res.height.max(1) as f32);
    let mut out: Vec<lab::LabSuggestion> = res
        .detections
        .into_iter()
        .filter(|d| d.score >= threshold)
        .map(|d| {
            let [x, y, bw, bh] = d.bbox;
            let x0 = (x / w).clamp(0.0, 1.0);
            let y0 = (y / h).clamp(0.0, 1.0);
            lab::LabSuggestion {
                class: lab::class_for_label(&classes, &d.label),
                label: d.label,
                score: d.score,
                bbox: [x0, y0, (bw / w).min(1.0 - x0), (bh / h).min(1.0 - y0)],
            }
        })
        .filter(|s| s.bbox[2] > 0.0 && s.bbox[3] > 0.0)
        .collect();
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out.truncate(lab::LAB_MAX_SUGGESTIONS);
    Ok(out)
}

fn ext_of(content_type: &str) -> &'static str {
    if content_type.contains("png") {
        "png"
    } else {
        "jpg"
    }
}

/// Most images of one export (the archive is built in memory).
pub const EXPORT_MAX_IMAGES: usize = 3000;
/// Most bytes of the pictures of one export (peak memory ≈ twice that).
pub const EXPORT_MAX_BYTES: i64 = 512 * 1024 * 1024;
/// Largest picture added to a project.
pub const IMAGE_MAX_BYTES: i64 = 50 * 1024 * 1024;
/// Most recording bytes read by one import (one segment in memory at once).
pub const RECORDING_MAX_BYTES: i64 = 2 * 1024 * 1024 * 1024;

/// COCO archive of a dataset version: `images/<id>.<ext>` at the frozen
/// media versions, `annotations/{train,val}.json`, `classes.txt`.
/// Returns `(file name, zip bytes)`.
pub async fn export(
    db: &DatabaseConnection,
    store: &dyn MediaStore,
    org_id: i64,
    id: Uuid,
) -> Res<(String, Vec<u8>)> {
    let v = lab_dataset_versions::Entity::find_by_id(id)
        .filter(lab_dataset_versions::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(LabError::db)?
        .ok_or_else(|| not_found(err_codes::LAB_VERSION_NOT_FOUND, "Dataset version"))?;
    let project = find_project(db, org_id, v.project_id).await?;
    let snap: DatasetSnapshot = serde_json::from_value(v.snapshot.clone())
        .map_err(|e| LabError::db(sea_orm::DbErr::Custom(e.to_string())))?;
    // shortcut: the archive is built in memory, capped at EXPORT_MAX_IMAGES;
    // stream it to the store if datasets ever grow past that.
    if snap.images.len() > EXPORT_MAX_IMAGES {
        return Err(LabError::fields(vec![(
            "images",
            format!("range:1..{EXPORT_MAX_IMAGES}"),
        )]));
    }
    let mut versions = Vec::with_capacity(snap.images.len());
    for img in &snap.images {
        let Ok(asset_id) = Uuid::parse_str(&img.asset_id) else {
            continue;
        };
        if let Some(mv) = media_versions::Entity::find()
            .filter(media_versions::Column::AssetId.eq(asset_id))
            .filter(media_versions::Column::OrgId.eq(org_id))
            .filter(media_versions::Column::VersionNumber.eq(img.asset_version))
            .one(db)
            .await
            .map_err(LabError::db)?
        {
            versions.push((img, mv));
        }
    }
    // Sizes are checked before anything is read (the archive is in memory).
    let total: i64 = versions.iter().map(|(_, mv)| mv.size_bytes).sum();
    if total > EXPORT_MAX_BYTES {
        return Err(LabError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            err_codes::LAB_EXPORT_TOO_LARGE,
            "This dataset is too large for one archive.",
        ));
    }
    let mut exts = std::collections::HashMap::new();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for (img, mv) in versions {
        let Ok(bytes) = store.get(&mv.storage_key).await else {
            continue;
        };
        let ext = ext_of(&mv.content_type);
        exts.insert(img.image_id.clone(), ext);
        files.push((format!("images/{}.{ext}", img.image_id), bytes));
    }
    // Images missing from the store are left out of the annotations too.
    let present = DatasetSnapshot {
        classes: snap.classes.clone(),
        images: snap
            .images
            .iter()
            .filter(|i| exts.contains_key(&i.image_id))
            .cloned()
            .collect(),
    };
    let ext = |i: &SnapshotImage| exts.get(&i.image_id).copied().unwrap_or("jpg").to_string();
    let json = |split| {
        serde_json::to_vec_pretty(&lab::coco(&present, Some(split), &ext)).unwrap_or_default()
    };
    files.push(("annotations/train.json".into(), json(LabSplit::Train)));
    files.push(("annotations/val.json".into(), json(LabSplit::Val)));
    files.push((
        "classes.txt".into(),
        (snap.classes.join("\n") + "\n").into_bytes(),
    ));
    let bytes = tokio::task::spawn_blocking(move || zip_files(files))
        .await
        .map_err(|e| LabError::db(sea_orm::DbErr::Custom(e.to_string())))?
        .map_err(|e| LabError::db(sea_orm::DbErr::Custom(e)))?;
    let slug: String = project
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    Ok((
        format!("{}-v{}-coco.zip", slug.trim_matches('-'), v.version),
        bytes,
    ))
}

fn zip_files(files: Vec<(String, Vec<u8>)>) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    // Pictures are already compressed: stored as is; JSON is deflated.
    for (name, bytes) in files {
        let method = if name.starts_with("images/") {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };
        let opts = zip::write::SimpleFileOptions::default().compression_method(method);
        zip.start_file(name, opts).map_err(|e| e.to_string())?;
        zip.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    zip.finish()
        .map(|c| c.into_inner())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_round_trip() {
        let bytes = zip_files(vec![
            ("images/a.jpg".into(), vec![1, 2, 3]),
            ("classes.txt".into(), b"valve\n".to_vec()),
        ])
        .expect("zip");
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("read");
        assert_eq!(archive.len(), 2);
        let mut s = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("classes.txt").unwrap(), &mut s)
            .unwrap();
        assert_eq!(s, "valve\n");
    }
}
