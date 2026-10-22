//! Proposals (proposals.md D371–D377): a write the platform may not make
//! on its own because it is not sure it is true (a group link from a
//! community source, a website candidate, an extraction by a model). It
//! waits in one inbox; a human accepts it (written through the shared
//! services, provenance `proposal:<id>`) or rejects it, and a rejected
//! fingerprint is never proposed again.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::api::ObjectRef;

/// What a proposal would write (closed list).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    ObjectCreate,
    ObjectUpdate,
    LinkCreate,
    LinkUpdate,
    LinkClose,
    /// An event of an investigation case, from a detector on the members'
    /// tracks (P3): `target_id` is the case, `values.key` the detection.
    CaseEvent,
    /// A voice recognized on a media (media-review.md D275), listed in the
    /// inbox but decided on the media review, where it can be heard and its
    /// span moved: never written to the proposals table.
    VoiceSpan,
}

impl ProposalKind {
    pub const ALL: [Self; 7] = [
        Self::ObjectCreate,
        Self::ObjectUpdate,
        Self::LinkCreate,
        Self::LinkUpdate,
        Self::LinkClose,
        Self::CaseEvent,
        Self::VoiceSpan,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ObjectCreate => "object_create",
            Self::ObjectUpdate => "object_update",
            Self::LinkCreate => "link_create",
            Self::LinkUpdate => "link_update",
            Self::LinkClose => "link_close",
            Self::CaseEvent => "case_event",
            Self::VoiceSpan => "voice_span",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    #[default]
    Proposed,
    Accepted,
    Rejected,
    /// A newer proposal for the same target replaced it before a decision.
    Superseded,
}

impl ProposalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Superseded => "superseded",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::Proposed,
            Self::Accepted,
            Self::Rejected,
            Self::Superseded,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

/// What a producer proposes (server side: imports, flow nodes, detectors).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProposalInput {
    pub kind: Option<ProposalKind>,
    /// Object type (object_create), or link type (link_*).
    pub type_key: String,
    /// The object updated (object_update) or the link (link_update, link_close).
    pub target_id: Option<String>,
    /// Ends of a link_create.
    pub source_id: Option<String>,
    pub link_target_id: Option<String>,
    /// Title of an object_create (or a new title).
    pub title: Option<String>,
    /// Properties (objects) or attributes (links) proposed.
    pub values: Map<String, Value>,
    /// Version of the object read (object_update).
    pub base_version: Option<i32>,
    /// Quotes, addresses, proof ids.
    pub evidence: Value,
    pub origin: String,
    /// 0..=100.
    pub confidence: Option<f64>,
}

/// Read model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProposalView {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub kind: ProposalKind,
    pub status: ProposalStatus,
    /// Object type or link type.
    pub type_key: String,
    /// The objects concerned (target, link ends), as of now.
    pub refs: Vec<ObjectRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub values: Map<String, Value>,
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub evidence: Value,
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// Display name of who decided (never an identifier, R4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub decided_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The object or link written on acceptance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Accept,
    Reject,
}

/// `POST /proposals/{id}/decision`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProposalDecision {
    pub decision: DecisionKind,
    /// Why (kept with a rejection), at most 500 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Accept with these values instead of the proposed ones (Q-P5); the
    /// correction is kept on the decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub values: Option<Map<String, Value>>,
}

pub const REASON_MAX: usize = 500;

/// The target of a proposal, independent of the values: two proposals of
/// one slot compete (the newer supersedes the older).
pub fn slot(p: &ProposalInput) -> Option<String> {
    let kind = p.kind?;
    let id = |o: &Option<String>| o.as_deref().unwrap_or_default().to_string();
    Some(match kind {
        ProposalKind::ObjectCreate => format!("{}|{}|{}", kind.as_str(), p.type_key, id(&p.title)),
        ProposalKind::ObjectUpdate | ProposalKind::LinkUpdate | ProposalKind::LinkClose => {
            format!("{}|{}", kind.as_str(), id(&p.target_id))
        }
        ProposalKind::LinkCreate => format!(
            "{}|{}|{}|{}",
            kind.as_str(),
            p.type_key,
            id(&p.source_id),
            id(&p.link_target_id)
        ),
        // Not proposed through this table.
        ProposalKind::VoiceSpan => return None,
        // One detection of one case: never in competition with another.
        ProposalKind::CaseEvent => format!(
            "{}|{}|{}",
            kind.as_str(),
            id(&p.target_id),
            p.values
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ),
    })
}

/// Slot and values, canonical (keys sorted by `Map`), hashed by the server
/// into the fingerprint: a rejected fingerprint is never proposed again,
/// another value is a new proposal.
pub fn fingerprint_text(p: &ProposalInput) -> Option<String> {
    // A detection is identified by its key (detector, members, start), not
    // by its measures, which move with the window it was computed on.
    if p.kind == Some(ProposalKind::CaseEvent) {
        return slot(p);
    }
    let mut values = p.values.clone();
    values.remove("source");
    Some(format!("{}|{}", slot(p)?, Value::Object(values)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn link(target: &str, nature: &str) -> ProposalInput {
        ProposalInput {
            kind: Some(ProposalKind::LinkCreate),
            type_key: "filiale_de".into(),
            source_id: Some("a".into()),
            link_target_id: Some(target.into()),
            values: json!({"nature": nature, "source": "wikidata.org"})
                .as_object()
                .cloned()
                .unwrap(),
            ..Default::default()
        }
    }

    #[test]
    fn same_target_same_values_same_fingerprint() {
        let a = link("m", "wikidata");
        let mut b = link("m", "wikidata");
        b.values.insert("source".into(), json!("another run"));
        assert_eq!(
            fingerprint_text(&a),
            fingerprint_text(&b),
            "the source is provenance, not a value"
        );
        assert_ne!(
            fingerprint_text(&a),
            fingerprint_text(&link("other", "wikidata"))
        );
        assert_ne!(fingerprint_text(&a), fingerprint_text(&link("m", "saisie")));
        assert_eq!(slot(&a), slot(&link("m", "saisie")), "one slot, two values");
        assert!(fingerprint_text(&ProposalInput::default()).is_none());
    }
}
