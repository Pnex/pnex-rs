//! The issuer chart (markets-research.md D813): closes of the instrument and
//! the dated markers of the same time line, read in three queries.

use chrono::NaiveDate;
use pnex_core::markets::chart::{ChartEvent, ChartEventKind, ChartPoint, IssuerChart};
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use serde_json::Value;
use uuid::Uuid;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// `id` is an organization (its first instrument is drawn) or an
/// instrument (its issuer gives the filings); `since` the first day.
pub async fn issuer<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
    type_key: &str,
    since: NaiveDate,
) -> Result<IssuerChart, DbErr> {
    let linked = |sql: &'static str| async move {
        db.query_one_raw(stmt(sql, vec![org_id.into(), id.into()]))
            .await?
            .map(|r| r.try_get::<Uuid>("", "x"))
            .transpose()
    };
    let (issuer, instrument) = if type_key == "instrument" {
        let issuer = linked(
            "SELECT source_object_id AS x FROM resource_edges WHERE org_id = $1 AND relation = 'issues' \
               AND target_object_id = $2 AND valid_to IS NULL ORDER BY id LIMIT 1",
        )
        .await?;
        (issuer, Some(id))
    } else {
        let instrument = linked(
            "SELECT target_object_id AS x FROM resource_edges WHERE org_id = $1 AND relation = 'issues' \
               AND source_object_id = $2 AND valid_to IS NULL ORDER BY id LIMIT 1",
        )
        .await?;
        (Some(id), instrument)
    };
    let mut chart = IssuerChart {
        instrument_id: instrument.map(|i| i.to_string()),
        ticker: None,
        closes: Vec::new(),
        events: Vec::new(),
    };
    let from = since.to_string();
    if let Some(inst) = instrument {
        chart.ticker = db
            .query_one_raw(stmt(
                "SELECT properties ->> 'ticker' AS t FROM objects WHERE org_id = $1 AND id = $2",
                vec![org_id.into(), inst.into()],
            ))
            .await?
            .and_then(|r| r.try_get::<Option<String>>("", "t").ok().flatten());
        for r in db
            .query_all_raw(stmt(
                "SELECT DISTINCT ON (period_end) period_end::text AS d, value \
                 FROM market_observations WHERE org_id = $1 AND object_id = $2 AND metric = 'close' \
                   AND period_end >= $3::date ORDER BY period_end, known_at DESC",
                vec![org_id.into(), inst.into(), from.clone().into()],
            ))
            .await?
        {
            chart.closes.push(ChartPoint {
                date: r.try_get("", "d")?,
                value: r.try_get("", "value")?,
            });
        }
        for r in db
            .query_all_raw(stmt(
                "SELECT o.id, o.title, o.properties FROM resource_edges e \
                 JOIN objects o ON o.id = e.source_object_id AND o.valid_to IS NULL \
                 WHERE e.org_id = $1 AND e.relation = 'event_about' AND e.target_object_id = $2 \
                   AND e.valid_to IS NULL AND o.properties ->> 'event_at' >= $3 LIMIT 500",
                vec![org_id.into(), inst.into(), from.clone().into()],
            ))
            .await?
        {
            let p: Value = r.try_get("", "properties")?;
            let text = |k: &str| {
                p.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            chart.events.push(ChartEvent {
                id: r.try_get::<Uuid>("", "id")?.to_string(),
                date: text("event_at"),
                kind: ChartEventKind::Move,
                title: r.try_get("", "title")?,
                detail: text("move_direction"),
            });
        }
    }
    if let Some(issuer) = issuer {
        for r in db
            .query_all_raw(stmt(
                "SELECT o.id, o.title, o.properties FROM resource_edges e \
                 JOIN objects o ON o.id = e.source_object_id AND o.valid_to IS NULL \
                 WHERE e.org_id = $1 AND e.relation = 'filed_by' AND e.target_object_id = $2 \
                   AND e.valid_to IS NULL AND o.properties ->> 'filed_on' >= $3 LIMIT 500",
                vec![org_id.into(), issuer.into(), from.into()],
            ))
            .await?
        {
            let p: Value = r.try_get("", "properties")?;
            let text = |k: &str| {
                p.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            chart.events.push(ChartEvent {
                id: r.try_get::<Uuid>("", "id")?.to_string(),
                date: text("filed_on"),
                kind: ChartEventKind::Filing,
                title: r.try_get("", "title")?,
                detail: text("form"),
            });
        }
    }
    chart.events.sort_by(|a, b| a.date.cmp(&b.date));
    Ok(chart)
}
