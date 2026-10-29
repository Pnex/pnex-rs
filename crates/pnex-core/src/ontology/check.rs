//! Watch checks (network-check.md D426–D431): one run of a check on a
//! subject (a partner) within a scope (a certification), as the
//! `check_record` node and the import service send it. The server keeps a
//! state per (check, subject, scope) and writes an event only when the
//! fingerprint changes. serde-only, wasm-safe, no `flatten`.

use serde::{Deserialize, Serialize};

use super::flow::FlowNodeRef;
use super::CHECK_RESULTS;

/// Rules of one run, objects listed with it, ids in its scope.
pub const RULES_MAX: usize = 64;
pub const OBJECTS_MAX: usize = 50;
pub const SCOPES_MAX: usize = 8;
/// `check`, a rule code, a reason: short machine codes.
pub const CODE_MAX: usize = 128;
/// Case keys and titles given by the pack.
pub const CASE_KEY_MAX: usize = 255;

/// `check_record` has no setting: the payload says everything.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CheckRecordConfig {}

/// One rule of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CheckRule {
    pub code: String,
    #[serde(default)]
    pub version: i64,
    /// `ok` | `unknown` | `finding`.
    pub result: String,
    /// Machine code of an `unknown` (`site-unreachable`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The finding object the rule wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finding_id: Option<String>,
    /// The finding by its natural key, when the rule writes it in the same
    /// run (its id is not known yet): resolved through the run's
    /// `finding_type` / `finding_key` once it exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finding_ref: Option<String>,
}

/// The watch case a run feeds: found by `key`, else opened with `title`
/// (under the case `parent_key`, opened with `parent_title`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WatchCaseRef {
    pub key: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_title: Option<String>,
}

/// One run (`msg.payload` of `check_record`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CheckRun {
    pub check: String,
    pub subject_id: String,
    #[serde(default)]
    pub scope_ids: Vec<String>,
    #[serde(default)]
    pub rules: Vec<CheckRule>,
    /// Context of the run (site, pages, proofs): attached to a new event.
    #[serde(default)]
    pub objects: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<WatchCaseRef>,
    /// Type and unique property the rules' `finding_ref` values name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finding_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finding_key: Option<String>,
}

/// `POST /internal/flow/ontology/check`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckRecordRequest {
    pub node: FlowNodeRef,
    pub run: CheckRun,
}

/// Answer of a recorded run.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CheckRecordAck {
    /// Worst result of the rules.
    pub result: String,
    /// The result differs from the previous run (a new event when the
    /// case is live).
    pub changed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
}

/// Rank of a result: `finding` > `unknown` > `ok`; unknown values rank as
/// `unknown`.
pub fn rank(result: &str) -> usize {
    CHECK_RESULTS.iter().position(|r| *r == result).unwrap_or(1)
}

/// The worst of `results` (`ok` when empty).
pub fn worst<'a>(results: impl IntoIterator<Item = &'a str>) -> &'static str {
    CHECK_RESULTS[results.into_iter().map(rank).max().unwrap_or(0)]
}

fn code_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= CODE_MAX
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c))
}

impl CheckRun {
    /// Shape check (ids are checked against the org by the server): the
    /// first wrong field.
    pub fn check_shape(&self) -> Option<&'static str> {
        if !code_ok(&self.check) {
            return Some("check");
        }
        if self.rules.is_empty() || self.rules.len() > RULES_MAX {
            return Some("rules");
        }
        for r in &self.rules {
            if !code_ok(&r.code)
                || !CHECK_RESULTS.contains(&r.result.as_str())
                || r.reason.as_deref().is_some_and(|x| !code_ok(x))
                || r.finding_ref
                    .as_deref()
                    .is_some_and(|x| x.len() > CASE_KEY_MAX)
            {
                return Some("rules");
            }
        }
        let key_ok = |k: &Option<String>| k.as_deref().is_none_or(super::schema::valid_key);
        if !key_ok(&self.finding_type) || !key_ok(&self.finding_key) {
            return Some("finding_type");
        }
        if self.scope_ids.len() > SCOPES_MAX {
            return Some("scope_ids");
        }
        if self.objects.len() > OBJECTS_MAX {
            return Some("objects");
        }
        if let Some(c) = &self.case {
            let bad = |s: &str| s.trim().is_empty() || s.len() > CASE_KEY_MAX;
            if bad(&c.key)
                || c.title.len() > CASE_KEY_MAX
                || c.parent_key.as_deref().is_some_and(bad)
                || c.parent_title
                    .as_deref()
                    .is_some_and(|t| t.len() > CASE_KEY_MAX)
            {
                return Some("case");
            }
        }
        None
    }

    /// Worst result of the rules.
    pub fn result(&self) -> &'static str {
        worst(self.rules.iter().map(|r| r.result.as_str()))
    }

    /// Scope ids sorted, as one key.
    pub fn scope_key(&self) -> String {
        let mut s: Vec<&str> = self.scope_ids.iter().map(|x| x.trim()).collect();
        s.sort_unstable();
        s.dedup();
        s.join(",")
    }

    /// What the fingerprint hashes: check, subject, sorted scope, sorted
    /// rules with code, version, result, reason and finding. The objects
    /// are context, not part of the result.
    pub fn canonical(&self) -> String {
        let mut rules: Vec<String> = self
            .rules
            .iter()
            .map(|r| {
                format!(
                    "{}|{}|{}|{}|{}|{}",
                    r.code,
                    r.version,
                    r.result,
                    r.reason.as_deref().unwrap_or(""),
                    r.finding_id.as_deref().unwrap_or(""),
                    r.finding_ref.as_deref().unwrap_or("")
                )
            })
            .collect();
        rules.sort_unstable();
        format!(
            "{}\n{}\n{}\n{}",
            self.check,
            self.subject_id.trim(),
            self.scope_key(),
            rules.join("\n")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(code: &str, result: &str) -> CheckRule {
        CheckRule {
            code: code.into(),
            version: 1,
            result: result.into(),
            reason: None,
            finding_id: None,
            finding_ref: None,
        }
    }

    fn run(rules: Vec<CheckRule>) -> CheckRun {
        CheckRun {
            check: "quali_watch.network".into(),
            subject_id: "s".into(),
            scope_ids: vec!["b".into(), "a".into()],
            rules,
            ..Default::default()
        }
    }

    #[test]
    fn worst_result_wins() {
        assert_eq!(worst([]), "ok");
        assert_eq!(worst(["ok", "unknown"]), "unknown");
        assert_eq!(worst(["unknown", "finding", "ok"]), "finding");
    }

    #[test]
    fn fingerprint_ignores_order_and_context() {
        let a = run(vec![rule("ADM-02", "ok"), rule("CPF-03", "finding")]);
        let mut b = run(vec![rule("CPF-03", "finding"), rule("ADM-02", "ok")]);
        b.scope_ids.reverse();
        b.objects = vec!["page".into()];
        assert_eq!(a.canonical(), b.canonical());
        let mut c = a.clone();
        c.rules[0].version = 2;
        assert_ne!(a.canonical(), c.canonical(), "a rule upgrade is a change");
    }

    #[test]
    fn shape_is_checked() {
        assert_eq!(run(vec![rule("ADM-02", "ok")]).check_shape(), None);
        assert_eq!(run(vec![]).check_shape(), Some("rules"));
        assert_eq!(
            run(vec![rule("ADM-02", "bad")]).check_shape(),
            Some("rules")
        );
        let mut r = run(vec![rule("ADM-02", "ok")]);
        r.check = "a b".into();
        assert_eq!(r.check_shape(), Some("check"));
        r.check = "x".into();
        r.case = Some(WatchCaseRef::default());
        assert_eq!(r.check_shape(), Some("case"));
    }
}
