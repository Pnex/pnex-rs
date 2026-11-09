//! Temporal semantics of the read operations (markets-research.md D899).
//!
//! A read is `point_in_time` when every field it answers respects the
//! instant it is given, `partial` when some fields stay current (listed),
//! `timeless` when it describes a schema or a catalogue with no dated state,
//! and `current_only` (the default: not listed here) otherwise.
//!
//! A point-in-time access key (D898) reads only `point_in_time` operations,
//! always with their instant (400 `as-of-required`), and `timeless` ones;
//! anything else, writes included, answers 400 `as-of-unsupported`.
//! The guard `crates/pnex-backend/tests/as_of.rs` fails on a listed
//! operation missing from the spec and on an operation taking `as_of` that
//! is not listed; the canary test (D901) walks every `point_in_time` entry.

/// Where the instant of an operation is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instant {
    /// A query parameter with this name.
    Query(&'static str),
    /// The `as_of` field of the JSON body.
    Body,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsOf {
    /// Every answered field respects the instant.
    PointInTime(Instant),
    /// The instant applies, but these fields stay current.
    Partial(Instant, &'static [&'static str]),
    /// No dated state: schemas, catalogues.
    Timeless,
}

impl AsOf {
    /// Value of the `x-pnex-as-of` extension of the spec.
    pub fn label(self) -> &'static str {
        match self {
            AsOf::PointInTime(_) => "point_in_time",
            AsOf::Partial(..) => "partial",
            AsOf::Timeless => "timeless",
        }
    }
}

/// Label of an operation not listed in [`SEMANTICS`].
pub const CURRENT_ONLY: &str = "current_only";

use AsOf::*;
use Instant::*;

pub const SEMANTICS: &[(&str, AsOf)] = &[
    ("datasets_files_get", PointInTime(Query("as_of"))),
    ("datasets_files_list", PointInTime(Query("as_of"))),
    ("datasets_get", Timeless),
    ("datasets_list", Timeless),
    ("datasets_sample", PointInTime(Query("as_of"))),
    ("markets_bars", PointInTime(Query("as_of"))),
    ("markets_chains", PointInTime(Query("as_of"))),
    ("ontology_link_types_list", Timeless),
    ("ontology_objects_get", PointInTime(Query("as_of"))),
    ("ontology_objects_hierarchy", PointInTime(Query("at"))),
    ("ontology_objects_links", PointInTime(Query("as_of"))),
    ("ontology_objects_observations", PointInTime(Query("as_of"))),
    ("ontology_packs_list", Timeless),
    ("ontology_query", PointInTime(Body)),
    ("ontology_types_get", Timeless),
    ("ontology_types_list", Timeless),
    ("telemetry_catalog", Timeless),
];

/// Semantics of an operation; `None` = `current_only`.
pub fn semantics(operation_id: &str) -> Option<AsOf> {
    SEMANTICS
        .iter()
        .find(|(id, _)| *id == operation_id)
        .map(|(_, s)| *s)
}

/// The `x-pnex-as-of` label of an operation.
pub fn label(operation_id: &str) -> &'static str {
    semantics(operation_id).map_or(CURRENT_ONLY, AsOf::label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_and_unique() {
        let ids: Vec<&str> = SEMANTICS.iter().map(|(id, _)| *id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(ids, sorted);
    }
}
