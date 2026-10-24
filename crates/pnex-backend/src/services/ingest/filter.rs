//! `rows_filter` (D321): keeps the rows whose column — after the D301
//! conversions — equals a value, is in a list, or is in the values of a
//! property on the org's live objects of a type; writes the kept rows as a
//! CSV to the staging area. Runs before the import's record cap.

use std::collections::HashSet;

use pnex_core::err_codes;
use pnex_core::ingest::{FilterOp, IngestConfig, IngestMsg, StagedEntry};
use pnex_core::ontology::import::{convert, PerObjectOnly};
use sea_orm::ConnectionTrait;
use serde_json::{json, Value};

use super::table::{load, Table};
use super::{stmt, unavailable, Cx, Outcome};

/// Values of `property` on the live objects of `type_key` (those whose
/// `only` property holds one of its values, when set).
async fn object_values(
    cx: &Cx<'_>,
    type_key: &str,
    property: &str,
    only: Option<&PerObjectOnly>,
) -> Result<HashSet<String>, Box<Outcome>> {
    let rows = cx
        .db
        .query_all_raw(stmt(
            "SELECT DISTINCT properties ->> $3 AS v FROM objects \
             WHERE org_id = $1 AND type_key = $2 AND valid_to IS NULL AND properties ? $3 \
               AND ($4::text IS NULL OR properties ->> $4 = ANY($5))",
            vec![
                cx.node.org_id.into(),
                type_key.into(),
                property.into(),
                only.map(|o| o.property.trim().to_string()).into(),
                only.map(|o| {
                    o.values
                        .iter()
                        .map(|v| v.trim().to_string())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
                .into(),
            ],
        ))
        .await
        .map_err(|_| Box::new(unavailable()))?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get::<String>("", "v").ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect())
}

/// One CSV field, quoted when it must be.
fn field(s: &str, delim: char) -> String {
    if s.contains(delim) || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub(crate) fn to_csv(columns: &[String], rows: &[&Vec<String>], delim: char) -> String {
    let line = |cells: &mut dyn Iterator<Item = &String>| {
        cells
            .map(|c| field(c, delim))
            .collect::<Vec<_>>()
            .join(&delim.to_string())
    };
    let mut out = line(&mut columns.iter());
    out.push('\n');
    for r in rows {
        out.push_str(&line(&mut r.iter()));
        out.push('\n');
    }
    out
}

/// Kept rows and the distinct kept values of the column.
pub(crate) fn keep<'a>(
    t: &'a Table,
    column: usize,
    cfg: &IngestConfig,
    allowed: &HashSet<String>,
) -> (Vec<&'a Vec<String>>, usize) {
    let mut kept_values = HashSet::new();
    let rows = t
        .rows
        .iter()
        .filter(|r| {
            let raw = r.get(column).cloned().unwrap_or_default();
            let v = match convert(Some(Value::String(raw)), &cfg.convert) {
                Ok(Some(Value::String(s))) => s.trim().to_string(),
                Ok(Some(other)) => other.to_string(),
                // An empty or unconvertible cell matches nothing.
                Ok(None) | Err(_) => return false,
            };
            let hit = allowed.contains(&v);
            if hit {
                kept_values.insert(v);
            }
            hit
        })
        .collect();
    (rows, kept_values.len())
}

pub(super) async fn run(cx: &Cx<'_>, cfg: &IngestConfig, msg: IngestMsg) -> Outcome {
    let (input, table) = match load(cx, cfg, &msg).await {
        Ok(x) => x,
        Err(o) => return *o,
    };
    let column = cfg.column.trim();
    let Some(index) = table.columns.iter().position(|c| c == column) else {
        let mut out = Outcome::fail(
            err_codes::IMPORT_SCHEMA_CHANGED,
            json!({ "columns": column }),
        );
        out.manifest = json!({ "entry": input.name, "columns": table.columns });
        return out;
    };
    let from_setting = match cfg.setting.as_deref() {
        Some(key) => {
            match crate::services::ontology::packs::setting_value(cx.db, cx.node.org_id, key).await
            {
                Ok(v) => Some(
                    v.as_ref()
                        .and_then(Value::as_str)
                        .map(pnex_core::ontology::import::PerObjectSetting::codes)
                        .unwrap_or_default(),
                ),
                Err(_) => return unavailable(),
            }
        }
        None => None,
    };
    // An empty setting keeps every row (D425).
    let keep_all = from_setting.as_ref().is_some_and(Vec::is_empty);
    let allowed: HashSet<String> = match cfg.op {
        FilterOp::In if from_setting.is_some() => from_setting
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect(),
        FilterOp::Equals | FilterOp::In => {
            cfg.values.iter().map(|v| v.trim().to_string()).collect()
        }
        FilterOp::InObjects => {
            match object_values(cx, &cfg.object_type, &cfg.property, cfg.only.as_ref()).await {
                Ok(v) => v,
                Err(o) => return *o,
            }
        }
    };
    let (rows, distinct) = if keep_all {
        (table.rows.iter().collect(), 0)
    } else {
        keep(&table, index, cfg, &allowed)
    };
    let csv = to_csv(&table.columns, &rows, table.delimiter.unwrap_or(';'));
    let bytes = csv.len() as u64;
    match cx.room().await {
        Ok(room) if bytes <= room => {}
        Ok(_) => return Outcome::fail(err_codes::INGEST_STAGING_QUOTA, Value::Null),
        Err(o) => return *o,
    }
    let base = input.name.rsplit('/').next().unwrap_or("table");
    let name = match base.rsplit_once('.') {
        Some((stem, _)) => format!("{stem}.csv"),
        None => format!("{base}.csv"),
    };
    let key = cx.key("f", &format!("{}-{name}", cx.node.node_id));
    if cx
        .store
        .put_bytes(&key, csv.into_bytes().into())
        .await
        .is_err()
    {
        return unavailable();
    }
    let manifest = json!({
        "entry": input.name,
        "column": column,
        "op": cfg.op,
        "only": cfg.only,
        "setting": cfg.setting,
        "allowed_values": allowed.len(),
        "rows_in": table.rows.len(),
        "rows_out": rows.len(),
        "distinct_kept": distinct,
        "staging_key": key,
    });
    let mut out = Outcome::ok(
        IngestMsg {
            staging_key: key.clone(),
            ..msg
        },
        manifest,
    );
    out.written = vec![StagedEntry {
        name,
        staging_key: key,
        bytes,
    }];
    out
}

#[cfg(test)]
mod tests {
    use super::super::table::read_table;
    use super::*;
    use pnex_core::ontology::import::Convert;

    #[test]
    fn filters_on_converted_values_and_writes_csv() {
        let src = "Numero_Fiche;Siret_Certificateur;Nom\n\
            RS6765;\"\t80259488700019\";\"TELE; PILOTE\"\n\
            RS1;\"\t13002526500013\";X\n";
        let t = read_table(src.as_bytes(), "c.csv").unwrap();
        let cfg = IngestConfig {
            column: "Siret_Certificateur".into(),
            op: FilterOp::In,
            values: vec!["80259488700019".into()],
            convert: vec![Convert::Digits],
            ..Default::default()
        };
        let allowed = cfg.values.iter().cloned().collect();
        let (rows, distinct) = keep(&t, 1, &cfg, &allowed);
        assert_eq!((rows.len(), distinct), (1, 1));
        let csv = to_csv(&t.columns, &rows, ';');
        assert_eq!(
            csv,
            "Numero_Fiche;Siret_Certificateur;Nom\nRS6765;\t80259488700019;\"TELE; PILOTE\"\n"
        );
        // The written CSV reads back the same.
        let back = read_table(csv.as_bytes(), "f.csv").unwrap();
        assert_eq!(back.rows[0][2], "TELE; PILOTE");
    }
}
