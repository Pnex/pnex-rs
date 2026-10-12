//! Cases API (cases-workspace.md): `/api/v1/cases/*`. Edits of an event's
//! title or date, and of the case itself, go through the ontology API.

use pnex_core::ontology::api::{
    CaseAttachInput, CaseBoard, CaseDetachInput, CaseDetected, CaseEventInput, CaseExport,
    CaseImported, CaseLayout, CaseOpenInput, CasePageInput, CaseProposal, CasePurgeInput,
    CasePurged, CaseRead, CaseSignals, CaseSnapshot, CaseSnapshotFull, CaseSnapshotInput,
    CaseTracks, CaseTracksQuery, CaseWorkspace, DetectQuery, ExpandInput, ExpandResult, LinkView,
    ObjectRef, ObjectView, PageTemplate, ProposalReject, ProposalRetain, SignalsQuery, TrackImport,
    TrackImportInput, TrackImportPreview,
};
use pnex_core::pages::DocSummary;
use reqwest::Method;

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

fn body<T: serde::Serialize>(input: &T) -> Option<serde_json::Value> {
    Some(serde_json::to_value(input).unwrap_or_default())
}

fn path(id: &str, rest: &str) -> String {
    format!("/api/v1/cases/{}/{rest}", urlencode(id))
}

/// Opens a case, possibly from a dated alert (its first event).
pub async fn open(input: &CaseOpenInput) -> Result<ObjectView, ApiError> {
    client::request(Method::POST, "/api/v1/cases", body(input)).await
}

/// Events with their attachments, and undated pieces.
pub async fn workspace(id: &str) -> Result<CaseWorkspace, ApiError> {
    client::request(Method::GET, &path(id, "workspace"), None).await
}

/// A new event and its first attachments, in one gesture.
pub async fn create_event(id: &str, input: &CaseEventInput) -> Result<CaseWorkspace, ApiError> {
    client::request(Method::POST, &path(id, "events"), body(input)).await
}

/// Attach to an event, change a role, or put among the undated pieces.
pub async fn attach(id: &str, input: &CaseAttachInput) -> Result<CaseWorkspace, ApiError> {
    client::request(Method::POST, &path(id, "attach"), body(input)).await
}

/// Detach from an event, or remove from the case.
pub async fn detach(id: &str, input: &CaseDetachInput) -> Result<CaseWorkspace, ApiError> {
    client::request(Method::POST, &path(id, "detach"), body(input)).await
}

/// Raw signals around the members over a window.
pub async fn signals(id: &str, q: &SignalsQuery) -> Result<CaseSignals, ApiError> {
    client::request(Method::POST, &path(id, "signals"), body(q)).await
}

/// Tracks of the members over a window, generalised for the zoom level.
pub async fn tracks(id: &str, q: &CaseTracksQuery) -> Result<CaseTracks, ApiError> {
    client::request(Method::POST, &path(id, "tracks"), body(q)).await
}

/// Previews (dry run) or imports a track file onto a case member.
pub async fn import_track(
    id: &str,
    input: &TrackImportInput,
) -> Result<TrackImportPreview, ApiError> {
    client::request(Method::POST, &path(id, "tracks/import"), body(input)).await
}

pub async fn track_imports(id: &str) -> Result<Vec<TrackImport>, ApiError> {
    client::request(Method::GET, &path(id, "tracks/imports"), None).await
}

pub async fn delete_track_import(id: &str, iid: &str) -> Result<(), ApiError> {
    let p = path(id, &format!("tracks/imports/{}", urlencode(iid)));
    client::request_opt::<serde_json::Value>(Method::DELETE, &p, None)
        .await
        .map(|_| ())
}

/// Runs the detectors over a window; new proposals only.
pub async fn detect(id: &str, q: &DetectQuery) -> Result<CaseDetected, ApiError> {
    client::request(Method::POST, &path(id, "detect"), body(q)).await
}

pub async fn proposals(id: &str) -> Result<Vec<CaseProposal>, ApiError> {
    client::request(Method::GET, &path(id, "proposals"), None).await
}

/// Retains a proposal: its event, in the user's words.
pub async fn retain(
    id: &str,
    pid: &str,
    input: &ProposalRetain,
) -> Result<CaseWorkspace, ApiError> {
    let p = path(id, &format!("proposals/{}/retain", urlencode(pid)));
    client::request(Method::POST, &p, body(input)).await
}

/// Rejects a proposal; it is never proposed again.
pub async fn reject(
    id: &str,
    pid: &str,
    input: &ProposalReject,
) -> Result<Vec<CaseProposal>, ApiError> {
    let p = path(id, &format!("proposals/{}/reject", urlencode(pid)));
    client::request(Method::POST, &p, body(input)).await
}

pub async fn board(id: &str) -> Result<CaseBoard, ApiError> {
    client::request(Method::GET, &path(id, "board"), None).await
}

/// The whole case as a `pnex.case/1` document.
pub async fn export(id: &str) -> Result<CaseExport, ApiError> {
    client::request(Method::GET, &path(id, "export"), None).await
}

/// Freezes the case as it is now (K33).
pub async fn freeze(id: &str, input: &CaseSnapshotInput) -> Result<CaseSnapshot, ApiError> {
    client::request(Method::POST, &path(id, "snapshots"), body(input)).await
}

/// Frozen states of the case, newest first.
pub async fn snapshots(id: &str) -> Result<Vec<CaseSnapshot>, ApiError> {
    client::request(Method::GET, &path(id, "snapshots"), None).await
}

/// One frozen state, its content as fingerprinted.
pub async fn snapshot(id: &str, seq: i32) -> Result<CaseSnapshotFull, ApiError> {
    client::request(Method::GET, &path(id, &format!("snapshots/{seq}")), None).await
}

/// Ontology relations among the members, closed ones included (K31).
pub async fn relations(id: &str) -> Result<Vec<LinkView>, ApiError> {
    client::request(Method::GET, &path(id, "relations"), None).await
}

/// Neighbours of a graph node: counted facets, or the picked ones (K31).
pub async fn expand(id: &str, input: &ExpandInput) -> Result<ExpandResult, ApiError> {
    client::request(Method::POST, &path(id, "expand"), body(input)).await
}

/// The team's arrangement of the case graph (empty = automatic layout).
pub async fn layout(id: &str) -> Result<CaseLayout, ApiError> {
    client::request(Method::GET, &path(id, "layout"), None).await
}

/// Saves the arrangement; empty positions bring the automatic layout back.
pub async fn save_layout(id: &str, input: &CaseLayout) -> Result<CaseLayout, ApiError> {
    client::request(Method::PUT, &path(id, "layout"), body(input)).await
}

/// Logged reads of persons' tracks (K38); `None` = log unavailable.
pub async fn reads(id: &str) -> Result<Option<Vec<CaseRead>>, ApiError> {
    client::request(Method::GET, &path(id, "reads"), None).await
}

/// A new case from a `pnex.case/1` export.
pub async fn import_case(doc: &CaseExport) -> Result<CaseImported, ApiError> {
    client::request(Method::POST, "/api/v1/cases/import", body(doc)).await
}

/// Page templates of the packs the org installed.
pub async fn templates() -> Result<Vec<PageTemplate>, ApiError> {
    client::request(Method::GET, "/api/v1/cases/templates", None).await
}

/// New page of the case, filled from a template; it joins as an undated piece.
pub async fn add_page(id: &str, input: &CasePageInput) -> Result<DocSummary, ApiError> {
    client::request(Method::POST, &path(id, "pages"), body(input)).await
}

/// Persons and identifiers only this case holds.
pub async fn purge_candidates(id: &str) -> Result<Vec<ObjectRef>, ApiError> {
    client::request(Method::GET, &path(id, "purge"), None).await
}

/// Deletes them for good; the case must be archived.
pub async fn purge(id: &str, input: &CasePurgeInput) -> Result<CasePurged, ApiError> {
    client::request(Method::POST, &path(id, "purge"), body(input)).await
}
