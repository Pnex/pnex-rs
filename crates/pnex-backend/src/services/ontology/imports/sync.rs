//! Rows of a Table as a mapping reads them (pipelines.md D577, D1050): a
//! `json` column expanded into its records, and the plan of a Table's files,
//! which seeds the base of an import converted into a pipeline.

use futures_util::TryStreamExt;
use pnex_core::datasets::IDENTITY_TITLE;
use pnex_core::err_codes;
use pnex_core::markets::PlannedObservation;
use pnex_core::ontology::import::{Convert, FieldSpec, Mapping, Schema};
use sea_orm::ConnectionTrait;
use serde_json::Value;

use super::diff::Planned;
use super::{OntologyError, Result};
use crate::services::artifact_store::S3Store;
use crate::services::datasets::{parquet, ViewFile};

/// Objects planned in one transaction before refusing.
// shortcut: bounds two whole plans held in the server, plan in the worker if a real
// source needs more (see §9.6 for what was measured).
const PLANNED_MAX: usize = 5_000_000;

/// The column read as is, up to what erasure ignores anyway: another conversion
/// leaves a stored value the column does not hold, so it cannot identify a row.
fn identity_column(f: &FieldSpec) -> Option<&str> {
    let plain = f.convert.iter().all(|c| {
        matches!(
            c,
            Convert::Trim | Convert::Upper | Convert::Lower | Convert::Email
        )
    });
    f.from.as_deref().filter(|_| plain && f.concat.is_empty())
}

/// Erasing an object must reach its rows (§3.11): the mapping says which
/// columns hold its personal values, as `datasets_identity_set` would. Only
/// the columns the Table `table` has; adds, never removes.
pub async fn declare_identity<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    table: &str,
    m: &Mapping,
) -> Result<()> {
    let Some(row) = db
        .query_one_raw(super::stmt(
            "SELECT id, schema FROM datasets WHERE org_id = $1 AND name = $2",
            vec![org_id.into(), table.into()],
        ))
        .await?
    else {
        return Ok(());
    };
    let id: uuid::Uuid = row.try_get("", "id")?;
    let columns: Value = row.try_get("", "schema")?;
    let has = |c: &str| {
        columns
            .as_array()
            .is_some_and(|a| a.iter().any(|f| f["name"] == c))
    };
    let s = super::schema(db, org_id).await?;
    for o in m.objects.iter().filter(|o| o.each.is_none()) {
        let Some(def) = s.object_type(&o.type_key) else {
            continue;
        };
        let title = o
            .title
            .as_ref()
            .filter(|_| def.title_classification.is_personal())
            .and_then(identity_column)
            .map(|c| (c, IDENTITY_TITLE));
        let properties = o.properties.iter().filter_map(|(k, f)| {
            def.properties
                .iter()
                .any(|p| p.key == *k && p.classification.is_personal())
                .then(|| identity_column(f).map(|c| (c, k.as_str())))
                .flatten()
        });
        for (column, property) in title.into_iter().chain(properties) {
            if !has(column) {
                continue;
            }
            db.execute_raw(super::stmt(
                "INSERT INTO dataset_identity_columns (dataset_id, column_name, type_key, property) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
                vec![
                    id.into(),
                    column.into(),
                    o.type_key.clone().into(),
                    property.into(),
                ],
            ))
            .await?;
        }
    }
    Ok(())
}

/// A table row as the mapping reads it. `column` holds JSON
/// (a pipeline's `json` column): its object, or each object of its array,
/// is a record, beside the row's other columns. A value that is no JSON
/// object gives no record.
pub(super) fn expanded(row: Value, column: &str) -> Vec<Value> {
    let Value::Object(mut cols) = row else {
        return vec![row];
    };
    if column.is_empty() {
        return vec![Value::Object(cols)];
    }
    let parsed = cols.remove(column).and_then(|v| {
        v.as_str()
            .and_then(|t| serde_json::from_str::<Value>(t).ok())
    });
    let records = match parsed {
        Some(Value::Array(a)) => a,
        Some(v) => vec![v],
        None => Vec::new(),
    };
    records
        .into_iter()
        .filter_map(|r| match r {
            Value::Object(mut o) => {
                for (k, v) in &cols {
                    o.insert(k.clone(), v.clone());
                }
                Some(Value::Object(o))
            }
            _ => None,
        })
        .collect()
}

pub(super) async fn plan(
    store: &S3Store,
    files: &[ViewFile],
    m: &Mapping,
    s: &Schema<'_>,
    history: bool,
) -> Result<(Planned, Vec<PlannedObservation>)> {
    let unreadable = |e: ::parquet::errors::ParquetError| {
        tracing::warn!("dataset sync read: {e}");
        OntologyError::Import {
            code: err_codes::DATASET_STORAGE_UNAVAILABLE,
            args: None,
        }
    };
    let mut planned = Planned::default();
    // A Table a pipeline appends to (a connector read past its cursor, a
    // webhook's inbox) is a history: the later row of a key is its newer state.
    planned.replace = history;
    let mut observed = Vec::new();
    // Line 1 of a CSV holds the headers: line numbers read as in the source.
    let mut row_no = 1;
    for file in files {
        let mut batches = parquet::batches(store, &file.key, file.bytes)
            .await
            .map_err(unreadable)?;
        while let Some(batch) = batches.try_next().await.map_err(unreadable)? {
            for row in parquet::records(&batch).map_err(unreadable)? {
                row_no += 1;
                for row in expanded(row, &m.json_column) {
                    planned.push(m, s, row_no, &row, PLANNED_MAX)?;
                    if m.observations.is_some() {
                        observed.extend(pnex_core::markets::observation(&row));
                    }
                }
            }
            tokio::task::yield_now().await;
        }
    }
    Ok((planned, pnex_core::markets::first_reports(observed)))
}

#[cfg(test)]
mod tests {
    use super::expanded;
    use serde_json::json;

    #[test]
    fn a_json_column_becomes_the_records_beside_the_other_columns() {
        let row = json!({"queried": "1", "record": "[{\"a\": {\"b\": 2}}, 3, {\"a\": 1}]"});
        let got = expanded(row, "record");
        assert_eq!(
            got,
            vec![
                json!({"a": {"b": 2}, "queried": "1"}),
                json!({"a": 1, "queried": "1"})
            ]
        );
        assert!(expanded(json!({"record": "not json"}), "record").is_empty());
        assert_eq!(expanded(json!({"x": "1"}), ""), vec![json!({"x": "1"})]);
    }
}
