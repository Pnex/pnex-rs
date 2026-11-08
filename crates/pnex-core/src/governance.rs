//! Data governance (governance.md D798–D806): markings, clearances, and what a
//! table carries. DTOs and checks, wasm-safe.

use serde::{Deserialize, Serialize};

pub const KEY_MAX: usize = 64;
pub const NAME_MAX: usize = 100;
pub const DESCRIPTION_MAX: usize = 500;
pub const PURPOSE_MAX: usize = 200;
pub const JUSTIFICATION_MAX: usize = 500;
/// Longest clearance, in days.
pub const CLEARANCE_DAYS_MAX: i64 = 365;

/// A compartment of the org: seeing data that carries it needs a clearance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Marking {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub key: String,
    pub name: String,
    pub description: String,
    /// Tables and objects that carry it: a carried marking cannot be deleted.
    pub uses: i64,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MarkingInput {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// A key in English: lowercase letters, digits, `_`.
pub fn key_ok(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= KEY_MAX
        && k.starts_with(|c: char| c.is_ascii_lowercase())
        && k.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl MarkingInput {
    pub fn violation(&self) -> Option<(&'static str, String)> {
        if !key_ok(&self.key) {
            return Some(("key", "invalid".into()));
        }
        if self.name.trim().is_empty() {
            return Some(("name", "required".into()));
        }
        if self.name.chars().count() > NAME_MAX {
            return Some(("name", format!("max_length:{NAME_MAX}")));
        }
        (self.description.chars().count() > DESCRIPTION_MAX)
            .then(|| ("description", format!("max_length:{DESCRIPTION_MAX}")))
    }
}

/// A person cleared for a marking, for a purpose, until a date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Clearance {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    #[cfg_attr(feature = "openapi", schema(format = "int64"))]
    pub user_id: i64,
    pub user_email: String,
    pub marking_key: String,
    pub purpose: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub expires_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub revoked_at: Option<String>,
    /// Neither revoked nor expired.
    pub active: bool,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ClearanceInput {
    #[cfg_attr(feature = "openapi", schema(format = "int64"))]
    pub user_id: i64,
    pub marking_key: String,
    pub purpose: String,
    /// Days from now, 1 to 365.
    pub days: i64,
}

impl ClearanceInput {
    pub fn violation(&self) -> Option<(&'static str, String)> {
        if !key_ok(&self.marking_key) {
            return Some(("marking_key", "invalid".into()));
        }
        if self.purpose.trim().is_empty() {
            return Some(("purpose", "required".into()));
        }
        if self.purpose.chars().count() > PURPOSE_MAX {
            return Some(("purpose", format!("max_length:{PURPOSE_MAX}")));
        }
        (!(1..=CLEARANCE_DAYS_MAX).contains(&self.days))
            .then(|| ("days", format!("range:1:{CLEARANCE_DAYS_MAX}")))
    }
}

/// Markings added to a table; removing one is a declassification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MarkingsAdd {
    pub markings: Vec<String>,
}

/// Removes one marking, for a reason kept in the journal (D802).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Declassify {
    pub marking: String,
    pub justification: String,
}

impl Declassify {
    pub fn violation(&self) -> Option<(&'static str, String)> {
        if !key_ok(&self.marking) {
            return Some(("marking", "invalid".into()));
        }
        if self.justification.trim().chars().count() < 10 {
            return Some(("justification", "min_length:10".into()));
        }
        (self.justification.chars().count() > JUSTIFICATION_MAX)
            .then(|| ("justification", format!("max_length:{JUSTIFICATION_MAX}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_are_checked() {
        let m = MarkingInput {
            key: "hr".into(),
            name: "HR".into(),
            description: String::new(),
        };
        assert!(m.violation().is_none());
        for bad in ["", "HR", "1hr", "hr-data", "h r"] {
            let b = MarkingInput {
                key: bad.into(),
                ..m.clone()
            };
            assert_eq!(b.violation().unwrap().0, "key", "{bad}");
        }
        let c = ClearanceInput {
            user_id: 1,
            marking_key: "hr".into(),
            purpose: "payroll 2026".into(),
            days: 30,
        };
        assert!(c.violation().is_none());
        assert_eq!(
            ClearanceInput {
                days: 0,
                ..c.clone()
            }
            .violation()
            .unwrap()
            .0,
            "days"
        );
        assert_eq!(
            ClearanceInput {
                days: 366,
                ..c.clone()
            }
            .violation()
            .unwrap()
            .0,
            "days"
        );
        assert_eq!(
            ClearanceInput {
                purpose: " ".into(),
                ..c
            }
            .violation()
            .unwrap()
            .0,
            "purpose"
        );
        let d = Declassify {
            marking: "hr".into(),
            justification: "too short".into(),
        };
        assert_eq!(d.violation().unwrap().0, "justification");
    }
}
