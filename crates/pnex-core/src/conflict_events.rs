//! Verified conflict events (markets-research.md D741): a UCDP or ACLED
//! event confirms a GDELT lead of the same country, within a day and of a
//! compatible kind. The events themselves are read by the `world_events`
//! pack's pipelines.

/// Kinds that confirm each other: fighting and attacks are coded either way
/// by different sources; the others must be the same.
fn compatible(a: &str, b: &str) -> bool {
    let violent = |k: &str| matches!(k, "armed_conflict" | "attack");
    a == b || (violent(a) && violent(b))
}

/// A lead as the confirmation pass sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Dated {
    pub country: String,
    pub day: chrono::NaiveDate,
    pub kind: String,
}

/// Does `verified` confirm `lead` (D741): same country, at most one day
/// apart, compatible kind.
pub fn confirms(verified: &Dated, lead: &Dated) -> bool {
    verified.country.eq_ignore_ascii_case(&lead.country)
        && (verified.day - lead.day).num_days().abs() <= 1
        && compatible(&verified.kind, &lead.kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_needs_country_day_and_kind() {
        let d = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let at = |c: &str, day: &str, k: &str| Dated {
            country: c.into(),
            day: d(day),
            kind: k.into(),
        };
        let v = at("UA", "2026-11-05", "armed_conflict");
        assert!(confirms(&v, &at("ua", "2026-11-04", "attack")));
        assert!(!confirms(&v, &at("UA", "2026-11-03", "armed_conflict")));
        assert!(!confirms(&v, &at("RU", "2026-11-05", "armed_conflict")));
        assert!(!confirms(&v, &at("UA", "2026-11-05", "sanctions")));
    }
}
