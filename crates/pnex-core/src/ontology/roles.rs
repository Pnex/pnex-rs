//! Roles a pack declares on its object types and link types (external-apps.md D441).
//!
//! The engine never names a pack's type or link: it names a role, and the org's
//! schema says which type or link carries it. A role is held by one type (or one
//! link type) of an org at most. The roles are the words of the shared concepts
//! the engine works with (people and their identities, statements, evidence,
//! introduction links); a feature built on one pack only keeps its own keys.

// Object type roles.
pub const PERSON: &str = "person";
pub const ORGANIZATION: &str = "organization";
pub const PLACE: &str = "place";
pub const IDENTIFIER: &str = "identifier";
pub const STATEMENT: &str = "statement";
pub const HYPOTHESIS: &str = "hypothesis";
pub const CLAIM: &str = "claim";
pub const PIECE: &str = "piece";
pub const PARTY: &str = "party";

// Link type roles.
pub const USES_IDENTIFIER: &str = "uses_identifier";
pub const APPEARS_IN: &str = "appears_in";
pub const SAID_BY: &str = "said_by";
pub const PUBLISHED_BY: &str = "published_by";
pub const SUPPORTS: &str = "supports";
pub const CONTRADICTS: &str = "contradicts";
pub const SOURCED_BY: &str = "sourced_by";
pub const KNOWS: &str = "knows";
pub const EMPLOYS: &str = "employs";
pub const MANAGES: &str = "manages";
pub const MEMBER_OF: &str = "member_of";
pub const RECOMMENDED: &str = "recommended";
pub const OWNS: &str = "owns";

/// Roles an object type may declare.
pub const OBJECT: &[&str] = &[
    PERSON,
    ORGANIZATION,
    PLACE,
    IDENTIFIER,
    STATEMENT,
    HYPOTHESIS,
    CLAIM,
    PIECE,
    PARTY,
];

/// Roles a link type may declare.
pub const LINK: &[&str] = &[
    USES_IDENTIFIER,
    APPEARS_IN,
    SAID_BY,
    PUBLISHED_BY,
    SUPPORTS,
    CONTRADICTS,
    SOURCED_BY,
    KNOWS,
    EMPLOYS,
    MANAGES,
    MEMBER_OF,
    RECOMMENDED,
    OWNS,
];

/// The key of the type carrying `role` among `types` (`(key, role)` pairs).
pub fn key_of<'a>(
    pairs: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    role: &str,
) -> Option<&'a str> {
    pairs
        .into_iter()
        .find(|(_, r)| *r == Some(role))
        .map(|(k, _)| k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_unique_and_found_by_role() {
        let mut all: Vec<_> = OBJECT.iter().chain(LINK).collect();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), OBJECT.len() + LINK.len());
        let types = [("contact", Some(PERSON)), ("firm", None)];
        assert_eq!(key_of(types, PERSON), Some("contact"));
        assert_eq!(key_of(types, PLACE), None);
    }
}
