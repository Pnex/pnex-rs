//! Annotation layers of the cameras (D105) → OpenObserve logs stream
//! `camera_detections`: one document per analysed frame and detection node
//! (layer), timestamped with the frame time so the player finds the boxes
//! of the frame it shows. Boxes only — never pixels: the recording stays
//! the raw MJPEG-AVI, any number of layers is drawn over it.
//!
//! Same O2 doctrine as the events (D84): the write path provisions the org
//! lazily, the read path never does (no org = no annotation).

use loco_rs::prelude::*;
use pnex_core::camera::{
    AnnotationBatch, AnnotationLayer, FrameAnnotation, ANNOTATIONS_MAX, DETECTIONS_STREAM,
};

use crate::services::openobserve::{
    ensure_org_credentials, provisioned_credentials, Client, OpenobserveSettings,
};

fn client(ctx: &AppContext) -> Option<Client> {
    OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s))
}

/// Stored document of one annotation — `detections` serialized to a
/// string (O2 flattens nested objects), `labels` kept searchable.
pub fn document_of(batch: &AnnotationBatch, ann: &FrameAnnotation) -> serde_json::Value {
    let mut labels: Vec<&str> = ann.detections.iter().map(|d| d.label.as_str()).collect();
    labels.sort_unstable();
    labels.dedup();
    let mut doc = serde_json::json!({
        "_timestamp": ann.ts_ms * 1000,
        "device_id": batch.device_id,
        "layer_id": batch.layer_id,
        "layer": batch.layer,
        "width": ann.width,
        "height": ann.height,
        "count": ann.detections.len(),
        "labels": labels.join(","),
        "detections": serde_json::to_string(&ann.detections).unwrap_or_default(),
    });
    if let Some(f) = batch.flow_id {
        doc["flow_id"] = serde_json::Value::from(f);
    }
    doc
}

/// Stored document → annotation, `None` when unreadable.
pub fn annotation_of(hit: &serde_json::Value) -> Option<FrameAnnotation> {
    let detections = serde_json::from_str(hit.get("detections")?.as_str()?).ok()?;
    Some(FrameAnnotation {
        layer_id: hit
            .get("layer_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        ts_ms: hit.get("_timestamp")?.as_i64()? / 1000,
        width: hit.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        height: hit.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        detections,
    })
}

/// Writes a batch (empty detections are skipped by the caller).
pub async fn record(ctx: &AppContext, batch: &AnnotationBatch) -> std::result::Result<(), String> {
    let Some(client) = client(ctx) else {
        return Err("openobserve is not configured".into());
    };
    let docs: Vec<serde_json::Value> = batch
        .annotations
        .iter()
        .filter(|a| !a.detections.is_empty())
        .map(|a| document_of(batch, a))
        .collect();
    if docs.is_empty() {
        return Ok(());
    }
    let creds = ensure_org_credentials(&ctx.db, &client, batch.org_id).await?;
    client
        .ingest_json(
            &creds.o2_org,
            DETECTIONS_STREAM,
            &docs,
            &creds.email_passcode,
        )
        .await
}

fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

/// SQL of a camera's annotations for the selected layers (literals
/// escaped, ascending time).
pub fn search_sql(device_slug: &str, layers: &[String]) -> String {
    let list: Vec<String> = layers.iter().map(|l| format!("'{}'", esc(l))).collect();
    format!(
        "SELECT * FROM \"{DETECTIONS_STREAM}\" WHERE device_id = '{}' AND layer_id IN ({}) ORDER BY _timestamp ASC",
        esc(device_slug),
        list.join(", ")
    )
}

/// SQL of the layers of a camera (one row per layer).
pub fn layers_sql(device_slug: &str) -> String {
    format!(
        "SELECT layer_id, max(layer) AS layer, count(*) AS n FROM \"{DETECTIONS_STREAM}\" \
         WHERE device_id = '{}' GROUP BY layer_id ORDER BY layer_id",
        esc(device_slug)
    )
}

/// Runs a read on the org's O2 — `None` when O2 is not configured, the org
/// never wrote anything or the stream does not exist yet (the overlay is
/// optional, never an error for the player).
async fn read(
    ctx: &AppContext,
    org_id: i64,
    sql: &str,
    from_ms: i64,
    to_ms: i64,
) -> std::result::Result<Option<Vec<serde_json::Value>>, String> {
    let Some(client) = client(ctx) else {
        return Ok(None);
    };
    let Some(creds) = provisioned_credentials(&ctx.db, org_id).await? else {
        return Ok(None);
    };
    match client
        .search_logs(
            &creds.o2_org,
            sql,
            from_ms * 1000,
            to_ms * 1000,
            0,
            ANNOTATIONS_MAX,
            &creds.email_passcode,
        )
        .await
    {
        Ok(r) => Ok(Some(r.hits)),
        Err(e) if e.contains("not found") => Ok(None),
        Err(e) => Err(e),
    }
}

/// Annotations of the selected layers of one camera over `[from_ms, to_ms)`.
pub async fn search(
    ctx: &AppContext,
    org_id: i64,
    device_slug: &str,
    layers: &[String],
    from_ms: i64,
    to_ms: i64,
) -> std::result::Result<Vec<FrameAnnotation>, String> {
    if layers.is_empty() {
        return Ok(Vec::new());
    }
    let hits = read(
        ctx,
        org_id,
        &search_sql(device_slug, layers),
        from_ms,
        to_ms,
    )
    .await?;
    Ok(hits
        .unwrap_or_default()
        .iter()
        .filter_map(annotation_of)
        .collect())
}

/// Layers holding annotations of one camera over `[from_ms, to_ms)`.
pub async fn layers(
    ctx: &AppContext,
    org_id: i64,
    device_slug: &str,
    from_ms: i64,
    to_ms: i64,
) -> std::result::Result<Vec<AnnotationLayer>, String> {
    let hits = read(ctx, org_id, &layers_sql(device_slug), from_ms, to_ms).await?;
    Ok(hits
        .unwrap_or_default()
        .iter()
        .filter_map(|h| {
            let id = h.get("layer_id")?.as_str()?.to_string();
            let name = h
                .get("layer")
                .and_then(|v| v.as_str())
                .unwrap_or(&id)
                .to_string();
            Some(AnnotationLayer {
                id,
                name,
                count: h.get("n").and_then(|v| v.as_i64()).unwrap_or(0),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::vision::Detection;

    #[test]
    fn document_roundtrip() {
        let batch = AnnotationBatch {
            org_id: 1,
            device_id: "proud-robin".into(),
            layer_id: "f15-n3".into(),
            layer: "Persons".into(),
            flow_id: Some(15),
            annotations: Vec::new(),
        };
        let ann = FrameAnnotation {
            layer_id: "f15-n3".into(),
            ts_ms: 1_790_000_000_123,
            width: 640,
            height: 480,
            detections: vec![Detection {
                label: "person".into(),
                class_id: 0,
                score: 0.9,
                bbox: [10.0, 20.0, 100.0, 200.0],
            }],
        };
        let doc = document_of(&batch, &ann);
        assert_eq!(doc["_timestamp"], 1_790_000_000_123_000i64);
        assert_eq!(doc["labels"], "person");
        assert_eq!(annotation_of(&doc), Some(ann));
        assert_eq!(
            search_sql("o'brien", &["f1-a".into(), "f2-b".into()]),
            "SELECT * FROM \"camera_detections\" WHERE device_id = 'o''brien' \
             AND layer_id IN ('f1-a', 'f2-b') ORDER BY _timestamp ASC"
        );
    }
}
