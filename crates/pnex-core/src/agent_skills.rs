//! Skills of hosted agents (autonomous-agents.md D396, agent-environments.md D711): versioned
//! text of an org. Every new text is a draft; only a person activates a version, after
//! reading its diff. An agent pins skills at an activated version, and its runs find them as
//! read-only files under `/artifacts/in/skills/` (D770).

use serde::{Deserialize, Serialize};

/// A name is also the file name in the sandbox: `<name>.md`.
pub const NAME_MAX: usize = 60;
pub const DESCRIPTION_MAX: usize = 1000;
pub const BODY_MAX: usize = 64 * 1024;
/// Skills one agent may pin.
pub const PINS_MAX: usize = 20;
/// Input name under which a run gets the skills of its agent: a folder of `<name>.md`.
pub const SKILLS_INPUT: &str = "skills";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentSkill {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    pub description: String,
    /// Version of the record, for `expected_version`: any change raises it.
    pub version: i32,
    /// Number of the newest text.
    pub latest_number: i32,
    /// Number of the text activated last; `None` while every text is a draft.
    pub active_number: Option<i32>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub updated_at: String,
}

/// One text of a skill. A draft until a person activates it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentSkillVersion {
    pub number: i32,
    pub body: String,
    /// `sha256:<hex>` of the body.
    pub sha256: String,
    /// It was activated once: agents may pin it.
    pub activated: bool,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub activated_at: Option<String>,
}

/// A new skill, its text as a first draft.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentSkillCreate {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub body: String,
}

/// A change on the version that was read; a new text is a new draft.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentSkillUpdate {
    pub expected_version: i32,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
}

/// Activates the text `number`, on the version of the skill that was read.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentSkillActivate {
    pub number: i32,
    pub expected_version: i32,
}

/// A skill an agent runs with, at an activated version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SkillPin {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub skill_id: String,
    pub version: i32,
}

/// `[a-z0-9-]`, not starting with `-`: a safe file name in the sandbox.
pub fn name_error(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("required");
    }
    if name.chars().count() > NAME_MAX {
        return Some("max_length:60");
    }
    let ok = !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    (!ok).then_some("invalid")
}

pub fn body_error(body: &str) -> Option<&'static str> {
    if body.trim().is_empty() {
        return Some("required");
    }
    (body.len() > BODY_MAX).then_some("max_length:65536")
}

/// `(field, token)` of the first problem of the pins of an agent record.
pub fn pins_error(pins: &[SkillPin]) -> Option<(&'static str, &'static str)> {
    if pins.len() > PINS_MAX {
        return Some(("skills", "max_items:20"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for p in pins {
        if uuid::Uuid::parse_str(&p.skill_id).is_err() || p.version < 1 {
            return Some(("skills", "invalid"));
        }
        if !seen.insert(p.skill_id.as_str()) {
            return Some(("skills", "duplicate"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_file_names() {
        assert_eq!(name_error("code-review"), None);
        for bad in ["", "-x", "Code", "a/b", "a.md", "a b"] {
            assert!(name_error(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn pins_are_unique_and_bounded() {
        let pin = |v| SkillPin {
            skill_id: uuid::Uuid::nil().to_string(),
            version: v,
        };
        assert_eq!(pins_error(&[pin(1)]), None);
        assert_eq!(pins_error(&[pin(1), pin(2)]), Some(("skills", "duplicate")));
        assert_eq!(pins_error(&[pin(0)]), Some(("skills", "invalid")));
    }
}
